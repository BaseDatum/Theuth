//! Visitor gates served by the hosting engine (210 §7.5): the interstitial and the password
//! gate, both with signed `__Host-` cookies (`cookie`'s signed jar, key
//! `<data_dir>/share/cookie.key`). Enforced for honest clients only (blind edge).

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use cookie::{Cookie, CookieJar, Key, SameSite};
use http::header::{self, HeaderMap, HeaderValue};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Limited};
use parking_lot::Mutex;
use tokio::sync::Semaphore;

use crate::model::Access;
use crate::proxy::{Body, BoxError, full, text};
use crate::routes::Mount;

/// Gate endpoints below a mount (never forwarded upstream).
pub const CONTINUE_PATH: &str = "/.theuth-share/continue";
/// Password form target.
pub const LOGIN_PATH: &str = "/.theuth-share/login";
/// Attempts per IP per minute.
pub const ATTEMPTS_PER_MIN: u32 = 10;
/// Concurrent argon2 hashes per engine.
pub const HASHING: usize = 2;
/// Waiting hashes beyond which the gate answers 429.
pub const HASH_QUEUE: usize = 16;

/// What the gate does with a request.
pub enum Step {
    /// Through to the source.
    Pass,
    /// Answer this.
    Respond(Response<Body>),
    /// `POST …/continue`: set the interstitial cookie.
    Continue,
    /// `POST …/login`: check the password.
    Login,
}

/// Operator text for the interstitial, from `welcome`.
#[derive(Clone, Debug, Default)]
pub struct Operator {
    /// The edge's domain or operator name.
    pub name: String,
    /// The abuse URL.
    pub abuse_url: String,
}

/// The gate of one engine.
pub struct Gate {
    key: Key,
    passwords: Mutex<HashMap<String, String>>,
    attempts: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    hashing: Semaphore,
    waiting: AtomicUsize,
    operator: Mutex<Operator>,
}

/// Load or create the 64-byte cookie key, 0600 (§8.3).
pub fn load_key(path: &Path) -> anyhow::Result<Key> {
    if let Ok(b) = std::fs::read(path)
        && b.len() >= 64
    {
        return Ok(Key::from(&b[..64]));
    }
    let key = Key::generate();
    if let Some(dir) = path.parent() {
        platform::files::private_dir(dir)?;
    }
    platform::files::write_private(path, key.master())?;
    Ok(key)
}

/// A fresh password the engine shows to the user (never a tool argument, §7.1).
pub fn generate_password() -> String {
    let mut b = uuid::Uuid::new_v4().into_bytes().to_vec();
    b.extend(uuid::Uuid::new_v4().into_bytes());
    data_encoding::BASE32_NOPAD.encode(&b)[..20].to_ascii_lowercase()
}

/// argon2id PHC string of `password`.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::PasswordHasher as _;
    Ok(argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?
        .to_string())
}

fn verify(password: &str, phc: &str) -> bool {
    use argon2::PasswordVerifier as _;
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), phc)
        .is_ok()
}

fn seen_cookie(share: &str) -> String {
    format!("__Host-share-seen-{share}")
}

fn pass_cookie(share: &str) -> String {
    format!("__Host-share-{share}")
}

impl Gate {
    /// With the signing key.
    pub fn new(key: Key) -> Self {
        Self {
            key,
            passwords: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            hashing: Semaphore::new(HASHING),
            waiting: AtomicUsize::new(0),
            operator: Mutex::new(Operator::default()),
        }
    }

    /// Set the interstitial's operator text.
    pub fn set_operator(&self, op: Operator) {
        *self.operator.lock() = op;
    }

    /// Install or drop a share's password hash.
    pub fn set_password(&self, share: &str, phc: Option<String>) {
        let mut p = self.passwords.lock();
        match phc {
            Some(h) => p.insert(share.to_owned(), h),
            None => p.remove(share),
        };
    }

    fn jar(headers: &HeaderMap) -> CookieJar {
        let mut jar = CookieJar::new();
        for v in headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
        {
            for c in Cookie::split_parse(v.to_owned()).flatten() {
                jar.add_original(c);
            }
        }
        jar
    }

    fn signed(&self, headers: &HeaderMap, name: &str) -> Option<String> {
        Self::jar(headers)
            .signed(&self.key)
            .get(name)
            .map(|c| c.value().to_owned())
    }

    fn set_cookie(&self, name: String, value: String) -> HeaderValue {
        let mut jar = CookieJar::new();
        jar.signed_mut(&self.key).add(
            Cookie::build((name, value))
                .secure(true)
                .http_only(true)
                .path("/")
                .same_site(SameSite::Lax),
        );
        let c = jar
            .delta()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default();
        HeaderValue::from_str(&c).unwrap_or_else(|_| HeaderValue::from_static(""))
    }

    fn pass_value(&self, share: &str) -> Option<String> {
        let p = self.passwords.lock();
        p.get(share).map(|h| h.chars().rev().take(12).collect())
    }

    /// Decide before the source sees the request.
    pub fn step(&self, m: &Mount, rest: &str, method: &Method, headers: &HeaderMap) -> Step {
        if rest == CONTINUE_PATH && method == Method::POST {
            return Step::Continue;
        }
        if rest == LOGIN_PATH && method == Method::POST {
            return Step::Login;
        }
        let navigate = method == Method::GET
            && headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) == Some("navigate");
        if m.access == Access::Password {
            let ok = match self.pass_value(&m.share) {
                Some(want) => {
                    self.signed(headers, &pass_cookie(&m.share)).as_deref() == Some(&want)
                }
                None => false,
            };
            if !ok {
                return Step::Respond(if navigate {
                    html(StatusCode::UNAUTHORIZED, &password_page(m, false))
                } else {
                    text(StatusCode::UNAUTHORIZED, "password required")
                });
            }
        }
        if m.interstitial
            && navigate
            && headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|a| a.contains("text/html"))
            && self.signed(headers, &seen_cookie(&m.share)).is_none()
        {
            return Step::Respond(html(StatusCode::OK, &self.interstitial_page(m)));
        }
        Step::Pass
    }

    /// `POST …/continue`: set the cookie and go back to the mount.
    pub fn proceed(&self, m: &Mount, headers: &HeaderMap) -> Response<Body> {
        if !same_origin(headers) {
            return text(StatusCode::FORBIDDEN, "cross-site request refused");
        }
        let mut r = redirect(&format!("{}/", m.prefix));
        r.headers_mut().append(
            header::SET_COOKIE,
            self.set_cookie(seen_cookie(&m.share), "1".into()),
        );
        r
    }

    /// `POST …/login`: rate limit, bounded argon2 verification, cookie.
    pub async fn login<B>(&self, m: &Mount, ip: IpAddr, req: Request<B>) -> Response<Body>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        if !same_origin(req.headers()) {
            return text(StatusCode::FORBIDDEN, "cross-site request refused");
        }
        {
            let mut a = self.attempts.lock();
            let now = Instant::now();
            a.retain(|_, (t, _)| now.duration_since(*t) < Duration::from_secs(60));
            let e = a.entry(ip).or_insert((now, 0));
            e.1 += 1;
            if e.1 > ATTEMPTS_PER_MIN {
                return text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many attempts, wait a minute",
                );
            }
        }
        let Some(phc) = self.passwords.lock().get(&m.share).cloned() else {
            return text(StatusCode::UNAUTHORIZED, "password required");
        };
        let Ok(body) = Limited::new(req.into_body(), 4096).collect().await else {
            return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
        };
        let body = body.to_bytes();
        let password = form_field(&body, "password").unwrap_or_default();
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= HASH_QUEUE + HASHING {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return text(StatusCode::TOO_MANY_REQUESTS, "busy, try again");
        }
        let ok = {
            let _permit = self.hashing.acquire().await;
            tokio::task::spawn_blocking(move || verify(&password, &phc))
                .await
                .unwrap_or(false)
        };
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        if !ok {
            return html(StatusCode::UNAUTHORIZED, &password_page(m, true));
        }
        let mut r = redirect(&format!("{}/", m.prefix));
        if let Some(v) = self.pass_value(&m.share) {
            r.headers_mut().append(
                header::SET_COOKIE,
                self.set_cookie(pass_cookie(&m.share), v),
            );
        }
        r
    }

    fn interstitial_page(&self, m: &Mount) -> String {
        let op = self.operator.lock().clone();
        let operator = if op.name.is_empty() {
            "the edge's operator".into()
        } else {
            escape(&op.name)
        };
        let report = if op.abuse_url.is_empty() {
            String::new()
        } else {
            format!(
                r#" <a href="{}" rel="noopener">Report abuse ↗</a>"#,
                escape(&op.abuse_url)
            )
        };
        page(&format!(
            "<p>This is an app a theuth user is sharing from their own computer. Neither theuth nor \
             {operator} checked it. Don't enter passwords you use elsewhere.{report}</p>\
             <form method=post action=\"{}{CONTINUE_PATH}\"><button>Continue</button></form>",
            escape(&m.prefix)
        ))
    }
}

fn password_page(m: &Mount, wrong: bool) -> String {
    page(&format!(
        "<p>{}</p><form method=post action=\"{}{LOGIN_PATH}\"><input type=password name=password \
         autofocus autocomplete=off><button>Open</button></form>",
        if wrong {
            "Wrong password."
        } else {
            "This share needs a password."
        },
        escape(&m.prefix)
    ))
}

fn page(body: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
         <title>Shared with theuth</title><body style=\"font:16px system-ui;max-width:36em;margin:4em auto\">{body}"
    )
}

fn html(status: StatusCode, body: &str) -> Response<Body> {
    let mut r = Response::new(full(body.to_owned()));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    r
}

fn redirect(to: &str) -> Response<Body> {
    let mut r = Response::new(full(Bytes::new()));
    *r.status_mut() = StatusCode::SEE_OTHER;
    if let Ok(v) = HeaderValue::from_str(to) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn same_origin(h: &HeaderMap) -> bool {
    h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("same-origin")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn form_field(body: &[u8], name: &str) -> Option<String> {
    let s = std::str::from_utf8(body).ok()?;
    s.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| {
            let v = v.replace('+', " ");
            let b = v.as_bytes();
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%'
                    && let Some(x) = b
                        .get(i + 1..i + 3)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(x);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
    use http_body_util::Full;

    fn m(access: Access) -> Mount {
        Mount {
            share: "s1".into(),
            host: "app.sky.tun.zip".into(),
            prefix: String::new(),
            source: Source::Port(1),
            access,
            interstitial: true,
        }
    }

    fn nav() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("sec-fetch-mode", HeaderValue::from_static("navigate"));
        h.insert("accept", HeaderValue::from_static("text/html"));
        h
    }

    #[tokio::test]
    async fn interstitial_then_password() {
        let g = Gate::new(Key::generate());
        let mount = m(Access::Public);
        assert!(matches!(
            g.step(&mount, "/", &Method::GET, &nav()),
            Step::Respond(_)
        ));
        assert!(matches!(
            g.step(&mount, "/x.js", &Method::GET, &HeaderMap::new()),
            Step::Pass
        ));
        let mut h = nav();
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        let r = g.proceed(&mount, &h);
        let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
        assert!(
            set.starts_with("__Host-share-seen-s1=")
                && set.contains("Secure")
                && set.contains("Path=/")
        );
        let pair = set.split(';').next().unwrap().to_owned();
        let mut h = nav();
        h.insert(header::COOKIE, HeaderValue::from_str(&pair).unwrap());
        assert!(matches!(g.step(&mount, "/", &Method::GET, &h), Step::Pass));

        let pm = Mount {
            interstitial: false,
            ..m(Access::Password)
        };
        g.set_password("s1", Some(hash_password("hunter22").unwrap()));
        assert!(matches!(
            g.step(&pm, "/", &Method::GET, &nav()),
            Step::Respond(_)
        ));
        let req = |pw: &str, site: &'static str| {
            Request::builder()
                .method(Method::POST)
                .uri(LOGIN_PATH)
                .header("sec-fetch-site", site)
                .body(Full::new(Bytes::from(format!("password={pw}"))))
                .unwrap()
        };
        let ip: IpAddr = "198.51.100.1".parse().unwrap();
        assert_eq!(
            g.login(&pm, ip, req("hunter22", "cross-site"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            g.login(&pm, ip, req("nope", "same-origin")).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let ok = g.login(&pm, ip, req("hunter22", "same-origin")).await;
        assert_eq!(ok.status(), StatusCode::SEE_OTHER);
        let pair = ok.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let mut h = nav();
        h.insert(header::COOKIE, HeaderValue::from_str(&pair).unwrap());
        assert!(matches!(g.step(&pm, "/", &Method::GET, &h), Step::Pass));
        for _ in 0..ATTEMPTS_PER_MIN {
            let _ = g.login(&pm, ip, req("x", "same-origin")).await;
        }
        assert_eq!(
            g.login(&pm, ip, req("x", "same-origin")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
