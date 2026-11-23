//! The visitor-facing HTTP service (210 §4.2 step 5, §5.6): route by host and longest prefix
//! (`421` for hosts this engine does not route), gate, then reverse-proxy a `port` source or serve
//! a `dir` source. Header policy: strip incoming forwarding headers, set `X-Forwarded-*`, rewrite
//! `Host` to `localhost:<port>`, rewrite `Location`/`Content-Location` whose origin is the
//! upstream. Upgrades (WebSocket) go through `hyper::upgrade::on` + `copy_bidirectional`.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Version};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper_util::rt::TokioIo;

use crate::gate::{Gate, Step};
use crate::model::Source;
use crate::routes::{Lookup, Mount, Table, normalize_host};
use crate::sources::{self, PortGuard};

/// Body error.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// Response body of every visitor answer.
pub type Body = UnsyncBoxBody<Bytes, BoxError>;

/// A fixed body.
pub fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|e| match e {}).boxed_unsync()
}

/// A plain-text answer.
pub fn text(status: StatusCode, msg: &str) -> Response<Body> {
    let mut r = Response::new(full(msg.to_owned()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

/// Forwarding headers a visitor may not supply (§5.6).
pub const STRIPPED: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "x-forwarded-prefix",
    "x-forwarded-port",
    "x-real-ip",
];

/// Hop-by-hop headers (RFC 9110 §7.6.1) dropped on both legs unless upgrading.
const HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn is_upgrade(h: &HeaderMap) -> bool {
    h.get(header::UPGRADE).is_some()
        && h.get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| {
                v.split(',')
                    .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
            })
}

fn strip_hop(h: &mut HeaderMap, keep_upgrade: bool) {
    let named: Vec<HeaderName> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .filter(|n| !(keep_upgrade && n == header::UPGRADE))
        .collect();
    for n in named {
        h.remove(n);
    }
    for n in HOP {
        if keep_upgrade && (*n == "connection" || *n == "upgrade") {
            continue;
        }
        h.remove(*n);
    }
}

/// The request-side header policy (§5.6).
pub fn request_headers(
    h: &mut HeaderMap,
    visitor: IpAddr,
    public_host: &str,
    prefix: &str,
    port: u16,
) {
    let upgrade = is_upgrade(h);
    strip_hop(h, upgrade);
    for n in STRIPPED {
        h.remove(*n);
    }
    let set = |h: &mut HeaderMap, n: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(n, v);
        }
    };
    set(h, "x-forwarded-for", &visitor.to_string());
    set(h, "x-forwarded-proto", "https");
    set(h, "x-forwarded-host", public_host);
    if !prefix.is_empty() {
        set(h, "x-forwarded-prefix", prefix);
    }
    set(h, "host", &format!("localhost:{port}"));
}

/// Rewrite a `Location`/`Content-Location` whose origin is the upstream to the public URL with the
/// mount prefixed; `None` leaves the value as it is.
pub fn rewrite_location(value: &str, port: u16, public_origin_with_prefix: &str) -> Option<String> {
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        let origin = format!("http://{host}:{port}");
        if let Some(rest) = value.strip_prefix(&origin)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            let rest = if rest.is_empty() { "/" } else { rest };
            return Some(format!("{public_origin_with_prefix}{rest}"));
        }
    }
    None
}

/// The response-side header policy.
pub fn response_headers(h: &mut HeaderMap, port: u16, m: &Mount, upgraded: bool) {
    strip_hop(h, upgraded);
    for name in [header::LOCATION, header::CONTENT_LOCATION] {
        if let Some(v) = h.get(&name).and_then(|v| v.to_str().ok())
            && let Some(new) = rewrite_location(v, port, &m.origin_with_prefix())
            && let Ok(v) = HeaderValue::from_str(&new)
        {
            h.insert(name, v);
        }
    }
}

/// The visitor service of one engine.
pub struct Proxy {
    /// Routes.
    pub table: Arc<Table>,
    /// `port` checks.
    pub ports: PortGuard,
    /// Interstitial and password gate.
    pub gate: Arc<Gate>,
}

impl Proxy {
    /// Answer one visitor request.
    pub async fn handle<B>(&self, req: Request<B>, visitor: SocketAddr) -> Response<Body>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let host = req
            .uri()
            .authority()
            .map(|a| a.as_str().to_owned())
            .or_else(|| {
                req.headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let (m, rest) = match self.table.lookup(&host, req.uri().path()) {
            Lookup::Misdirected => {
                return text(StatusCode::MISDIRECTED_REQUEST, "misdirected request");
            }
            Lookup::NotFound => return text(StatusCode::NOT_FOUND, "not found"),
            Lookup::Found(m, rest) => (m, rest),
        };
        match self.gate.step(&m, &rest, req.method(), req.headers()) {
            Step::Pass => {}
            Step::Respond(r) => return r,
            Step::Continue => return self.gate.proceed(&m, req.headers()),
            Step::Login => return self.gate.login(&m, visitor.ip(), req).await,
        }
        match &m.source {
            Source::Port(port) => self.forward(req, &m, *port, rest, visitor.ip()).await,
            Source::Dir(root) => {
                serve_dir(Path::new(root), &rest, req.method(), req.headers()).await
            }
        }
    }

    async fn forward<B>(
        &self,
        mut req: Request<B>,
        m: &Mount,
        port: u16,
        rest: String,
        visitor: IpAddr,
    ) -> Response<Body>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let stream = match self.ports.connect(port).await {
            Ok(s) => s,
            Err(e) => return text(StatusCode::BAD_GATEWAY, &e.to_string()),
        };
        let upgrade = is_upgrade(req.headers());
        let visitor_up = upgrade.then(|| hyper::upgrade::on(&mut req));
        let (mut parts, body) = req.into_parts();
        let pq = match parts.uri.query() {
            Some(q) => format!("{rest}?{q}"),
            None => rest,
        };
        parts.uri = match pq.parse() {
            Ok(u) => u,
            Err(_) => return text(StatusCode::BAD_REQUEST, "bad path"),
        };
        parts.version = Version::HTTP_11;
        request_headers(
            &mut parts.headers,
            visitor,
            &normalize_host(&m.host),
            &m.prefix,
            port,
        );
        let req = Request::from_parts(parts, body.map_err(Into::into).boxed_unsync());
        let (mut sender, conn) =
            match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
                Ok(x) => x,
                Err(e) => return text(StatusCode::BAD_GATEWAY, &e.to_string()),
            };
        tokio::spawn(async move {
            let _ = conn.with_upgrades().await;
        });
        let mut resp = match sender.send_request(req).await {
            Ok(r) => r,
            Err(e) => return text(StatusCode::BAD_GATEWAY, &e.to_string()),
        };
        let upgraded = resp.status() == StatusCode::SWITCHING_PROTOCOLS;
        if upgraded && let Some(visitor_up) = visitor_up {
            let upstream_up = hyper::upgrade::on(&mut resp);
            tokio::spawn(async move {
                if let (Ok(v), Ok(u)) = tokio::join!(visitor_up, upstream_up) {
                    let _ =
                        tokio::io::copy_bidirectional(&mut TokioIo::new(v), &mut TokioIo::new(u))
                            .await;
                }
            });
        }
        response_headers(resp.headers_mut(), port, m, upgraded);
        resp.map(|b| b.map_err(Into::into).boxed_unsync())
    }
}

/// Serve a file of a `dir` source (§5.6, F-m8): full bodies with an `ETag` from size and mtime.
async fn serve_dir(
    root: &Path,
    rest: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Response<Body> {
    if method != Method::GET && method != Method::HEAD {
        return text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    }
    let (root, rest) = (root.to_owned(), rest.to_owned());
    let opened = tokio::task::spawn_blocking(move || {
        let root = root.canonicalize().ok()?;
        let canon = sources::resolve(&root, &rest)?;
        let f = sources::open_nofollow(&canon).ok()?;
        let meta = f.metadata().ok()?;
        Some((canon, f, meta))
    })
    .await
    .ok()
    .flatten();
    let Some((canon, f, meta)) = opened else {
        return text(StatusCode::NOT_FOUND, "not found");
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let etag = format!("\"{:x}-{:x}\"", meta.len(), mtime);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        let mut r = Response::new(full(Bytes::new()));
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        return r;
    }
    let mime = mime_guess::from_path(&canon).first_or_octet_stream();
    let body = if method == Method::HEAD {
        full(Bytes::new())
    } else {
        let stream = tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(f));
        StreamBody::new(futures::StreamExt::map(stream, |r| {
            r.map(hyper::body::Frame::data)
                .map_err(|e| Box::new(e) as BoxError)
        }))
        .boxed_unsync()
    };
    let mut r = Response::new(body);
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Access;
    use crate::sources::{NoOwnPorts, NoProcessCheck};

    fn mount(host: &str, prefix: &str, source: Source) -> Mount {
        Mount {
            share: "s1".into(),
            host: host.into(),
            prefix: prefix.into(),
            source,
            access: Access::Public,
            interstitial: false,
        }
    }

    #[test]
    fn header_policy() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        h.insert("forwarded", HeaderValue::from_static("for=6.6.6.6"));
        h.insert("x-real-ip", HeaderValue::from_static("6.6.6.6"));
        h.insert(
            "connection",
            HeaderValue::from_static("keep-alive, x-secret"),
        );
        h.insert("x-secret", HeaderValue::from_static("1"));
        h.insert("host", HeaderValue::from_static("app.sky.tun.zip"));
        request_headers(
            &mut h,
            "203.0.113.9".parse().unwrap(),
            "app.sky.tun.zip",
            "",
            5173,
        );
        assert_eq!(h["x-forwarded-for"], "203.0.113.9");
        assert_eq!(h["x-forwarded-proto"], "https");
        assert_eq!(h["x-forwarded-host"], "app.sky.tun.zip");
        assert_eq!(h["host"], "localhost:5173");
        assert!(h.get("forwarded").is_none() && h.get("x-real-ip").is_none());
        assert!(h.get("connection").is_none() && h.get("x-secret").is_none());
        assert!(h.get("x-forwarded-prefix").is_none());

        let mut up = HeaderMap::new();
        up.insert("connection", HeaderValue::from_static("Upgrade"));
        up.insert("upgrade", HeaderValue::from_static("websocket"));
        request_headers(&mut up, "::1".parse().unwrap(), "h", "/p", 1);
        assert_eq!(up["upgrade"], "websocket");
        assert_eq!(up["x-forwarded-prefix"], "/p");
    }

    #[test]
    fn location_rewrite() {
        let o = "https://app.sky.tun.zip";
        assert_eq!(
            rewrite_location("http://localhost:5173/login?x=1", 5173, o).as_deref(),
            Some("https://app.sky.tun.zip/login?x=1")
        );
        assert_eq!(
            rewrite_location("http://127.0.0.1:5173", 5173, "https://sky.tun.zip/docs").as_deref(),
            Some("https://sky.tun.zip/docs/")
        );
        assert_eq!(rewrite_location("http://localhost:51730/x", 5173, o), None);
        assert_eq!(rewrite_location("https://example.org/", 5173, o), None);
        assert_eq!(rewrite_location("/relative", 5173, o), None);
    }

    fn proxy(table: Arc<Table>) -> Proxy {
        Proxy {
            table,
            ports: PortGuard {
                own: Arc::new(NoOwnPorts),
                process: Arc::new(NoProcessCheck),
            },
            gate: Arc::new(Gate::new(cookie::Key::generate())),
        }
    }

    fn get(host: &str, path: &str) -> Request<Full<Bytes>> {
        Request::builder()
            .uri(path)
            .header("host", host)
            .body(Full::new(Bytes::new()))
            .unwrap()
    }

    #[tokio::test]
    async fn misdirected_and_dir_and_upstream() {
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "<h1>hi</h1>").unwrap();
        let table = Arc::new(Table::default());
        table.insert(mount(
            "sky.tun.zip",
            "/docs",
            Source::Dir(tmp.path().to_str().unwrap().into()),
        ));

        // An upstream that echoes the headers it saw and redirects to its own origin.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(
                        |req: Request<hyper::body::Incoming>| async move {
                            let seen = format!(
                                "{}|{}|{}",
                                req.headers()["host"].to_str().unwrap(),
                                req.headers()["x-forwarded-for"].to_str().unwrap(),
                                req.uri()
                            );
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(302)
                                    .header("location", format!("http://localhost:{port}/next"))
                                    .body(Full::new(Bytes::from(seen)))
                                    .unwrap(),
                            )
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(s), svc)
                        .await;
                });
            }
        });
        table.insert(Mount {
            share: "s2".into(),
            ..mount("app.sky.tun.zip", "", Source::Port(port))
        });
        let p = proxy(table);
        let v: SocketAddr = "198.51.100.7:4000".parse().unwrap();

        let r = p.handle(get("docs.sky.tun.zip", "/"), v).await;
        assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST);
        let r = p.handle(get("sky.tun.zip", "/other"), v).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = p.handle(get("sky.tun.zip", "/docs/"), v).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.into_body().collect().await.unwrap().to_bytes(),
            "<h1>hi</h1>"
        );

        let mut req = get("app.sky.tun.zip", "/a?b=1");
        req.headers_mut()
            .insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        let r = p.handle(req, v).await;
        assert_eq!(r.status(), StatusCode::FOUND);
        assert_eq!(r.headers()["location"], "https://app.sky.tun.zip/next");
        let body = r.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, format!("localhost:{port}|198.51.100.7|/a?b=1"));
    }
}
