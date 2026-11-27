//! Sign in with DigitalOcean for cloud nodes (design doc 224): the relay holds the OAuth app's client
//! secret and does the code exchange; the engine proves it started the flow with a PKCE-style verifier.
//!
//! `GET  /oauth/digitalocean/start?state=S&challenge=C` → remember `S → C` (10 min), 302 to DigitalOcean.
//! `GET  /oauth/digitalocean/callback?code&state`        → exchange the code, keep the token JSON under `S`.
//! `POST /oauth/digitalocean/redeem {state, verifier}`    → the token JSON once, when `b64url(sha256(verifier)) == C`.
//! `POST /oauth/digitalocean/refresh {refresh_token}`     → DigitalOcean's refresh answer, status and body as-is.
//!
//! Tokens, codes and secrets never reach a log line or a URL; logs carry a state prefix and the outcome.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::App;

/// Default `redirect_uri`, registered with the DigitalOcean OAuth app.
pub const DEFAULT_REDIRECT: &str = "https://relay.theuth.io/oauth/digitalocean/callback";
/// DigitalOcean's authorize page.
pub const AUTHORIZE_URL: &str = "https://cloud.digitalocean.com/v1/oauth/authorize";
/// DigitalOcean's token endpoint.
pub const TOKEN_URL: &str = "https://cloud.digitalocean.com/v1/oauth/token";

const NOT_CONFIGURED: &str = "DigitalOcean sign-in is not configured on this relay";
/// A started sign-in lives this long, redeemed or not.
const TTL: Duration = Duration::from_secs(10 * 60);
/// Sign-ins in flight at once; beyond, `start` and pre-start `redeem` refuse.
const MAX_PENDING: usize = 1024;
/// Finished-state tombstones retained to prevent replay/resurrection; oldest evicted beyond this.
const MAX_FINISHED: usize = MAX_PENDING * 4;
/// `state` and `challenge`: base64url (no pad) of 32 bytes.
const B64_32: usize = 43;

/// The OAuth app.
pub struct Config {
    /// `THEUTH_DO_OAUTH_CLIENT_ID`.
    pub client_id: String,
    /// `THEUTH_DO_OAUTH_CLIENT_SECRET`.
    pub client_secret: String,
    /// `redirect_uri`, `THEUTH_DO_OAUTH_REDIRECT`.
    pub redirect: String,
    /// Token endpoint (overridable for tests).
    pub token_url: String,
}

impl Config {
    /// `None` unless both id and secret are non-empty.
    pub fn from_parts(
        client_id: Option<String>,
        client_secret: Option<String>,
        redirect: String,
        token_url: String,
    ) -> Option<Self> {
        let client_id = client_id.filter(|s| !s.is_empty())?;
        let client_secret = client_secret.filter(|s| !s.is_empty())?;
        Some(Self {
            client_id,
            client_secret,
            redirect,
            token_url,
        })
    }
}

enum Outcome {
    /// The callback is exchanging the code right now.
    Exchanging,
    /// DigitalOcean's token JSON.
    Tokens(Bytes),
    /// What went wrong, for the engine (redeem → 400 `{error}`).
    Error(String),
}

struct Pending {
    challenge: String,
    created: Instant,
    outcome: Option<Outcome>,
}

enum Entry {
    /// An in-flight sign-in attempt.
    Pending(Pending),
    /// Tombstone for a redeemed or expired state, kept for `ttl` so it cannot be reused.
    Finished(Instant),
}

/// Evict stale tombstones, convert expired pending entries into tombstones, and bound tombstone count.
fn prune(entries: &mut HashMap<String, Entry>, now: Instant, ttl: Duration) {
    entries.retain(|_, entry| match entry {
        Entry::Finished(at) => now.duration_since(*at) < ttl,
        Entry::Pending(p) => {
            if now.duration_since(p.created) < ttl {
                true
            } else {
                let expired_at = p.created.checked_add(ttl).unwrap_or(now);
                if now.duration_since(expired_at) < ttl {
                    *entry = Entry::Finished(expired_at);
                    true
                } else {
                    false
                }
            }
        }
    });
    cap_finished(entries);
}

fn cap_finished(entries: &mut HashMap<String, Entry>) {
    let finished = entries
        .values()
        .filter(|e| matches!(e, Entry::Finished(_)))
        .count();
    if finished <= MAX_FINISHED {
        return;
    }
    let excess = finished - MAX_FINISHED;
    let mut tombstones: Vec<(Instant, String)> = entries
        .iter()
        .filter_map(|(k, v)| match v {
            Entry::Finished(at) => Some((*at, k.clone())),
            Entry::Pending(_) => None,
        })
        .collect();
    tombstones.sort_unstable_by_key(|(at, _)| *at);
    for (_, k) in tombstones.into_iter().take(excess) {
        entries.remove(&k);
    }
}

fn active_pending(entries: &HashMap<String, Entry>) -> usize {
    entries
        .values()
        .filter(|e| matches!(e, Entry::Pending(_)))
        .count()
}

/// OAuth state: the app config and the sign-ins in flight.
pub struct DoOauth {
    cfg: Option<Config>,
    http: reqwest::Client,
    pending: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
}

impl DoOauth {
    /// `cfg: None` → every route answers 503.
    pub fn new(cfg: Option<Config>) -> Self {
        // reqwest's rustls needs a process provider when both ring and aws-lc-rs are linked (see crates/slack).
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            cfg,
            http,
            pending: Mutex::new(HashMap::new()),
            ttl: TTL,
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// POST the form to the token endpoint; `(status, body)`.
    async fn token(&self, cfg: &Config, form: &[(&str, &str)]) -> reqwest::Result<(u16, Bytes)> {
        let res = self.http.post(&cfg.token_url).form(form).send().await?;
        let status = res.status().as_u16();
        Ok((status, res.bytes().await?))
    }
}

/// The four routes, merged into `main`'s router.
pub fn routes() -> Router<App> {
    Router::new()
        .route("/oauth/digitalocean/start", get(start))
        .route("/oauth/digitalocean/callback", get(callback))
        .route("/oauth/digitalocean/redeem", post(redeem))
        .route("/oauth/digitalocean/refresh", post(refresh))
}

fn is_b64url(s: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Log-safe prefix of a state.
fn tag(state: &str) -> &str {
    &state[..state.len().min(6)]
}

/// RFC 3986 percent-encoding of everything but unreserved characters (spaces → `%20`).
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({ "error": msg }))).into_response()
}

fn json_body(status: StatusCode, body: Bytes) -> Response {
    let mut res = (status, body).into_response();
    res.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The callback page: matches `crates/oauth/src/callback.html` with no scripts, links, or tokens.
fn page(status: StatusCode, title: &str, text: &str) -> Response {
    let ok = status.is_success();
    let state = if ok { "ok" } else { "err" };
    let mark = if ok {
        r#"<path d="M15 27l7 7 15-16"/>"#
    } else {
        r#"<path d="M18 18l16 16M34 18L18 34"/>"#
    };
    let title = html_escape(title);
    let text = html_escape(text);
    let body = format!(
        r#"<!doctype html>
<html lang="en" data-state="{state}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>theuth · {title}</title>
<style>
  :root {{
    --bg: #e8dcc8; --card: #f3eadf; --fg: #1a1612; --muted: #6b5e4f;
    --line: #d4c4a8; --ok: #2f5d4a; --err: #9b2d1f;
    --glow-ok: rgba(47, 93, 74, .18); --glow-err: rgba(155, 45, 31, .18);
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg: #12100e; --card: #1c1915; --fg: #e8dcc8; --muted: #c9b89a;
      --line: #2a241c; --ok: #4a8a6e; --err: #c45a48;
      --glow-ok: rgba(74, 138, 110, .22); --glow-err: rgba(196, 90, 72, .22);
    }}
  }}
  * {{ box-sizing: border-box; }}
  html, body {{ height: 100%; margin: 0; }}
  body {{
    display: grid; place-items: center; padding: 24px;
    background: var(--bg); color: var(--fg);
    font: 16px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, sans-serif;
    -webkit-font-smoothing: antialiased;
  }}
  body::before {{
    content: ""; position: fixed; inset: -40vmax; z-index: -1; pointer-events: none;
    background: radial-gradient(closest-side, var(--glow) 0%, transparent 70%);
    animation: breathe 6s ease-in-out infinite alternate;
  }}
  [data-state="ok"]  {{ --glow: var(--glow-ok);  --accent: var(--ok);  }}
  [data-state="err"] {{ --glow: var(--glow-err); --accent: var(--err); }}

  .card {{
    width: min(100%, 400px); padding: 44px 36px 36px; text-align: center;
    background: var(--card); border: 1px solid var(--line); border-radius: 20px;
    box-shadow: 0 1px 2px rgba(26, 22, 18, .06), 0 24px 64px -24px rgba(26, 22, 18, .3);
    animation: rise .5s cubic-bezier(.2,.8,.2,1) both;
  }}
  .mark {{ width: 72px; height: 72px; margin: 0 auto 24px; display: block; }}
  .mark circle, .mark path {{
    fill: none; stroke: var(--accent); stroke-width: 2.5;
    stroke-linecap: round; stroke-linejoin: round;
  }}
  .mark circle {{ stroke-dasharray: 151; stroke-dashoffset: 151; animation: draw .6s .15s ease-out forwards; }}
  .mark path   {{ stroke-dasharray: 40;  stroke-dashoffset: 40;  animation: draw .35s .6s ease-out forwards; }}

  h1 {{ margin: 0 0 8px; font-size: 22px; font-weight: 600; letter-spacing: -.01em; }}
  p  {{ margin: 0; color: var(--muted); overflow-wrap: anywhere; }}
  .brand {{ margin-top: 28px; font-size: 12px; color: var(--muted); letter-spacing: .08em; text-transform: uppercase; }}

  @keyframes draw    {{ to {{ stroke-dashoffset: 0; }} }}
  @keyframes rise    {{ from {{ opacity: 0; transform: translateY(12px) scale(.98); }} }}
  @keyframes breathe {{ to {{ transform: scale(1.15); }} }}
  @media (prefers-reduced-motion: reduce) {{ * {{ animation: none !important; }} .mark circle, .mark path {{ stroke-dashoffset: 0; }} }}
</style>
</head>
<body>
  <main class="card" role="status" aria-live="polite">
    <svg class="mark" viewBox="0 0 52 52" aria-hidden="true">
      <circle cx="26" cy="26" r="24"/>
      {mark}
    </svg>
    <h1>{title}</h1>
    <p>{text}</p>
    <div class="brand">theuth</div>
  </main>
</body>
</html>
"#
    );
    let mut res = (status, body).into_response();
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    res
}

/// DigitalOcean's error text from a token-endpoint body.
fn do_error(status: u16, body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    ["error_description", "error"]
        .iter()
        .find_map(|k| v.get(k).and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| format!("DigitalOcean answered HTTP {status}"))
}

/// 503 when unconfigured, 429 when the IP is over its budget.
fn gate<'a>(
    app: &'a App,
    headers: &HeaderMap,
    addr: SocketAddr,
    json: bool,
) -> Result<&'a Config, Box<Response>> {
    let Some(cfg) = app.oauth.cfg.as_ref() else {
        return Err(Box::new(if json {
            json_error(StatusCode::SERVICE_UNAVAILABLE, NOT_CONFIGURED)
        } else {
            (StatusCode::SERVICE_UNAVAILABLE, NOT_CONFIGURED).into_response()
        }));
    };
    if !app.relay.allow(crate::client_ip(headers, addr)) {
        return Err(Box::new(StatusCode::TOO_MANY_REQUESTS.into_response()));
    }
    Ok(cfg)
}

async fn start(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let cfg = match gate(&app, &headers, addr, false) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (Some(state), Some(challenge)) = (q.get("state"), q.get("challenge")) else {
        return (StatusCode::BAD_REQUEST, "state and challenge required").into_response();
    };
    if !is_b64url(state, B64_32..=B64_32) || !is_b64url(challenge, B64_32..=B64_32) {
        return (
            StatusCode::BAD_REQUEST,
            "state and challenge must be base64url of 32 bytes",
        )
            .into_response();
    }
    {
        let mut entries = app.oauth.pending();
        let now = Instant::now();
        prune(&mut entries, now, app.oauth.ttl);
        match entries.get(state) {
            // The same browser reopening the start URL, or a pre-start `redeem` poll that arrived first.
            Some(Entry::Pending(p))
                if p.outcome.is_none() && ct_eq(p.challenge.as_bytes(), challenge.as_bytes()) => {}
            Some(_) => return (StatusCode::CONFLICT, "state already in use").into_response(),
            None if active_pending(&entries) >= MAX_PENDING => {
                tracing::warn!("do-oauth: pending map full");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many sign-ins in progress; try again shortly",
                )
                    .into_response();
            }
            None => {
                entries.insert(
                    state.clone(),
                    Entry::Pending(Pending {
                        challenge: challenge.clone(),
                        created: now,
                        outcome: None,
                    }),
                );
            }
        }
    }
    tracing::info!(state = tag(state), "do-oauth: start");
    let location = format!(
        "{AUTHORIZE_URL}?client_id={}&redirect_uri={}&response_type=code&scope=read%20write&state={state}",
        pct(&cfg.client_id),
        pct(&cfg.redirect),
    );
    let mut res = StatusCode::FOUND.into_response();
    match HeaderValue::from_str(&location) {
        Ok(v) => {
            res.headers_mut().insert(LOCATION, v);
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn callback(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let cfg = match gate(&app, &headers, addr, false) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    const FAILED: &str = "DigitalOcean sign-in failed";
    let unknown = || {
        page(
            StatusCode::BAD_REQUEST,
            FAILED,
            "This sign-in link is unknown or has expired. Start again from theuth.",
        )
    };
    let already_used = || {
        page(
            StatusCode::BAD_REQUEST,
            FAILED,
            "This sign-in link was already used. Start again from theuth.",
        )
    };
    let Some(state) = q.get("state").filter(|s| is_b64url(s, B64_32..=B64_32)) else {
        return unknown();
    };
    let denied = q
        .get("error")
        .map(|e| q.get("error_description").unwrap_or(e).clone());
    let code = q.get("code").filter(|c| !c.is_empty());
    {
        let mut entries = app.oauth.pending();
        let now = Instant::now();
        let ttl = app.oauth.ttl;
        match entries.get_mut(state) {
            None => return unknown(),
            Some(Entry::Finished(at)) => {
                if now.duration_since(*at) >= ttl {
                    entries.remove(state);
                    return unknown();
                }
                return already_used();
            }
            Some(Entry::Pending(p)) => {
                if now.duration_since(p.created) >= ttl {
                    entries.insert(state.clone(), Entry::Finished(now));
                    cap_finished(&mut entries);
                    return unknown();
                }
                if p.outcome.is_some() {
                    return already_used();
                }
                p.outcome = Some(match (&denied, code) {
                    (Some(e), _) => Outcome::Error(e.clone()),
                    (None, None) => {
                        Outcome::Error("DigitalOcean returned no authorization code".into())
                    }
                    (None, Some(_)) => Outcome::Exchanging,
                });
            }
        }
    }
    if let Some(e) = denied {
        tracing::info!(state = tag(state), "do-oauth: denied at DigitalOcean");
        return page(StatusCode::BAD_REQUEST, FAILED, &e);
    }
    let Some(code) = code else {
        tracing::info!(state = tag(state), "do-oauth: callback without code");
        return page(
            StatusCode::BAD_REQUEST,
            FAILED,
            "DigitalOcean returned no authorization code.",
        );
    };
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("redirect_uri", cfg.redirect.as_str()),
    ];
    let outcome = match app.oauth.token(cfg, &form).await {
        Ok((200..=299, body))
            if serde_json::from_slice::<Value>(&body)
                .is_ok_and(|v| v.get("access_token").is_some_and(Value::is_string)) =>
        {
            Outcome::Tokens(body)
        }
        Ok((status, body)) => Outcome::Error(do_error(status, &body)),
        Err(e) => {
            tracing::warn!(state = tag(state), err = %e.without_url(), "do-oauth: token endpoint unreachable");
            Outcome::Error("could not reach DigitalOcean".into())
        }
    };
    let res = match &outcome {
        Outcome::Tokens(_) => {
            tracing::info!(state = tag(state), "do-oauth: signed in");
            page(
                StatusCode::OK,
                "Signed in",
                "Signed in to DigitalOcean. You can close this tab and return to theuth.",
            )
        }
        Outcome::Error(e) => {
            tracing::info!(state = tag(state), "do-oauth: code exchange refused");
            page(StatusCode::BAD_GATEWAY, FAILED, e)
        }
        Outcome::Exchanging => unreachable!("set above"),
    };
    // Pruned meanwhile (expired): the engine sees 410 and starts over.
    if let Some(Entry::Pending(p)) = app.oauth.pending().get_mut(state) {
        p.outcome = Some(outcome);
    }
    res
}

#[derive(Deserialize)]
struct Redeem {
    state: String,
    verifier: String,
}

async fn redeem(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = gate(&app, &headers, addr, true) {
        return *r;
    }
    let Ok(Redeem { state, verifier }) = serde_json::from_slice(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "expected {state, verifier}");
    };
    if !is_b64url(&state, B64_32..=B64_32) || !is_b64url(&verifier, B64_32..=128) {
        return json_error(StatusCode::BAD_REQUEST, "malformed state or verifier");
    }
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut entries = app.oauth.pending();
    let now = Instant::now();
    let ttl = app.oauth.ttl;
    if let Some(Entry::Pending(p)) = entries.get(&state)
        && now.duration_since(p.created) >= ttl
    {
        entries.insert(state, Entry::Finished(now));
        prune(&mut entries, now, ttl);
        return json_error(StatusCode::GONE, "sign-in expired");
    }
    prune(&mut entries, now, ttl);
    match entries.get(&state) {
        Some(Entry::Finished(_)) => json_error(StatusCode::GONE, "sign-in already used or expired"),
        Some(Entry::Pending(p)) => {
            if !ct_eq(p.challenge.as_bytes(), challenge.as_bytes()) {
                tracing::info!(state = tag(&state), "do-oauth: redeem with wrong verifier");
                return json_error(StatusCode::FORBIDDEN, "verifier does not match");
            }
            match &p.outcome {
                None | Some(Outcome::Exchanging) => {
                    (StatusCode::ACCEPTED, axum::Json(json!({ "pending": true }))).into_response()
                }
                Some(_) => {
                    let Some(Entry::Pending(Pending {
                        outcome: Some(outcome),
                        ..
                    })) = entries.insert(state.clone(), Entry::Finished(now))
                    else {
                        unreachable!("checked above")
                    };
                    cap_finished(&mut entries);
                    drop(entries);
                    match outcome {
                        Outcome::Tokens(body) => {
                            tracing::info!(state = tag(&state), "do-oauth: redeemed");
                            json_body(StatusCode::OK, body)
                        }
                        Outcome::Error(e) => {
                            tracing::info!(state = tag(&state), "do-oauth: redeemed an error");
                            json_error(StatusCode::BAD_REQUEST, &e)
                        }
                        Outcome::Exchanging => unreachable!("matched above"),
                    }
                }
            }
        }
        None => {
            if active_pending(&entries) >= MAX_PENDING {
                tracing::warn!("do-oauth: pending map full");
                return json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many sign-ins in progress; try again shortly",
                );
            }
            tracing::debug!(state = tag(&state), "do-oauth: pre-start redeem registered");
            entries.insert(
                state,
                Entry::Pending(Pending {
                    challenge,
                    created: now,
                    outcome: None,
                }),
            );
            (StatusCode::ACCEPTED, axum::Json(json!({ "pending": true }))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct Refresh {
    refresh_token: String,
}

async fn refresh(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let cfg = match gate(&app, &headers, addr, true) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Ok(Refresh { refresh_token }) = serde_json::from_slice(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "expected {refresh_token}");
    };
    if refresh_token.is_empty() || refresh_token.len() > 4096 {
        return json_error(StatusCode::BAD_REQUEST, "malformed refresh_token");
    }
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
    ];
    match app.oauth.token(cfg, &form).await {
        Ok((status, body)) => {
            tracing::info!(status, "do-oauth: refresh");
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            json_body(status, body)
        }
        Err(e) => {
            tracing::warn!(err = %e.without_url(), "do-oauth: refresh: token endpoint unreachable");
            json_error(StatusCode::BAD_GATEWAY, "could not reach DigitalOcean")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::extract::{Form, Request};
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    use super::*;
    use crate::relay::Relay;

    const STATE: &str = "c3RhdGVzdGF0ZXN0YXRlc3RhdGVzdGF0ZXN0YXRlc3Q";
    const VERIFIER: &str = "dmVyaWZpZXJ2ZXJpZmllcnZlcmlmaWVydmVyaWZpZXI";

    fn challenge(verifier: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }

    /// A fake DigitalOcean token endpoint: records each form, answers per grant type.
    async fn fake_do() -> (String, mpsc::UnboundedReceiver<HashMap<String, String>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let router = Router::new().route(
            "/v1/oauth/token",
            post(move |Form(f): Form<HashMap<String, String>>| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(f.clone());
                    match (f["grant_type"].as_str(), f.get("code").map(String::as_str)) {
                        ("authorization_code", Some("good")) => (
                            StatusCode::OK,
                            r#"{"access_token":"at1","refresh_token":"rt1","expires_in":2592000,"token_type":"bearer"}"#,
                        ),
                        ("authorization_code", _) => (
                            StatusCode::UNAUTHORIZED,
                            r#"{"error":"invalid_grant","error_description":"The provided authorization grant is invalid"}"#,
                        ),
                        ("refresh_token", _) if f["refresh_token"] == "rt1" => {
                            (StatusCode::OK, r#"{"access_token":"at2","refresh_token":"rt2"}"#)
                        }
                        _ => (StatusCode::UNAUTHORIZED, r#"{"error":"invalid_grant"}"#),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });
        (format!("http://{addr}/v1/oauth/token"), rx)
    }

    fn router(oauth: DoOauth) -> Router {
        let app = App {
            relay: Arc::new(Relay::new(64)),
            web: None,
            oauth: Arc::new(oauth),
            claims: Arc::default(),
        };
        routes()
            .with_state(app)
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 4242))))
    }

    fn configured(token_url: &str) -> DoOauth {
        DoOauth::new(Config::from_parts(
            Some("cid 1".into()),
            Some("csecret".into()),
            DEFAULT_REDIRECT.into(),
            token_url.into(),
        ))
    }

    async fn get(r: &Router, uri: &str) -> (StatusCode, HeaderMap, String) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let res = r.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        let body = axum::body::to_bytes(body, 1 << 16).await.unwrap();
        (
            parts.status,
            parts.headers,
            String::from_utf8(body.to_vec()).unwrap(),
        )
    }

    async fn post_json(r: &Router, uri: &str, v: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap();
        let res = r.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn start_uri(state: &str, verifier: &str) -> String {
        format!(
            "/oauth/digitalocean/start?state={state}&challenge={}",
            challenge(verifier)
        )
    }

    #[test]
    fn empty_env_is_unconfigured() {
        assert!(
            Config::from_parts(
                Some(String::new()),
                Some("s".into()),
                String::new(),
                String::new()
            )
            .is_none()
        );
        assert!(Config::from_parts(Some("i".into()), None, String::new(), String::new()).is_none());
        assert!(
            Config::from_parts(
                Some("i".into()),
                Some("s".into()),
                String::new(),
                String::new()
            )
            .is_some()
        );
    }

    #[tokio::test]
    async fn unconfigured_answers_503() {
        let r = router(DoOauth::new(None));
        let (s, _, body) = get(&r, &start_uri(STATE, VERIFIER)).await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, NOT_CONFIGURED);
        let (s, _, _) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=x&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        let (s, v) = post_json(
            &r,
            "/oauth/digitalocean/redeem",
            json!({"state": STATE, "verifier": VERIFIER}),
        )
        .await;
        assert_eq!(
            (s, v["error"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some(NOT_CONFIGURED))
        );
        let (s, _) = post_json(
            &r,
            "/oauth/digitalocean/refresh",
            json!({"refresh_token": "rt1"}),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn start_redirects_to_digitalocean_and_remembers_the_challenge() {
        let oauth = Arc::new(configured("http://127.0.0.1:9/unused"));
        let app = App {
            relay: Arc::new(Relay::new(64)),
            web: None,
            oauth: oauth.clone(),
            claims: Arc::default(),
        };
        let r = routes()
            .with_state(app)
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 4242))));
        let (s, h, _) = get(&r, &start_uri(STATE, VERIFIER)).await;
        assert_eq!(s, StatusCode::FOUND);
        let loc = h[LOCATION].to_str().unwrap();
        assert_eq!(
            loc,
            format!(
                "https://cloud.digitalocean.com/v1/oauth/authorize?client_id=cid%201\
                 &redirect_uri=https%3A%2F%2Frelay.theuth.io%2Foauth%2Fdigitalocean%2Fcallback\
                 &response_type=code&scope=read%20write&state={STATE}"
            )
        );
        match &oauth.pending()[STATE] {
            Entry::Pending(p) => assert_eq!(p.challenge, challenge(VERIFIER)),
            Entry::Finished(_) => panic!("expected pending entry"),
        }
        // Reopening the same start URL is fine; reusing the state with another challenge is not.
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        assert_eq!(
            get(&r, &start_uri(STATE, "other")).await.0,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn malformed_input_is_400() {
        let r = router(configured("http://127.0.0.1:9/unused"));
        for uri in [
            "/oauth/digitalocean/start?state=short&challenge=short".to_owned(),
            format!("/oauth/digitalocean/start?state={STATE}"),
            format!(
                "/oauth/digitalocean/start?state={}&challenge={}",
                STATE.replace('c', "+"),
                challenge(VERIFIER)
            ),
        ] {
            assert_eq!(get(&r, &uri).await.0, StatusCode::BAD_REQUEST, "{uri}");
        }
        let (s, _) = post_json(
            &r,
            "/oauth/digitalocean/redeem",
            json!({"state": "x", "verifier": VERIFIER}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = post_json(&r, "/oauth/digitalocean/redeem", json!({"nope": 1})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = post_json(
            &r,
            "/oauth/digitalocean/refresh",
            json!({"refresh_token": ""}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn callback_for_an_unknown_state_is_a_400_page() {
        let r = router(configured("http://127.0.0.1:9/unused"));
        let (s, h, body) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(h[CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
        assert_eq!(
            h[CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'"
        );
        assert!(body.contains(r#"data-state="err""#));
        assert!(body.contains(r#"<div class="brand">theuth</div>"#));
        assert!(!body.contains("<script"));
        assert!(body.contains("unknown or has expired"));
        let (s, _, _) = get(&r, "/oauth/digitalocean/callback?code=good&state=bad").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn full_flow_with_a_fake_digitalocean() {
        let (token_url, mut seen) = fake_do().await;
        let r = router(configured(&token_url));
        let redeem = |verifier: &'static str| {
            let r = r.clone();
            async move {
                post_json(
                    &r,
                    "/oauth/digitalocean/redeem",
                    json!({"state": STATE, "verifier": verifier}),
                )
                .await
            }
        };
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        // Before the callback: pending for the right verifier, 403 for any other.
        assert_eq!(
            redeem(VERIFIER).await,
            (StatusCode::ACCEPTED, json!({"pending": true}))
        );
        let wrong = "d3Jvbmd3cm9uZ3dyb25nd3Jvbmd3cm9uZ3dyb25nd3I";
        assert_eq!(redeem(wrong).await.0, StatusCode::FORBIDDEN);

        let (s, h, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(page.contains(r#"data-state="ok""#));
        assert!(page.contains("<title>theuth · Signed in</title>"));
        assert!(page.contains(r#"<div class="brand">theuth</div>"#));
        assert!(!page.contains("<script"));
        assert!(
            page.contains(
                "Signed in to DigitalOcean. You can close this tab and return to theuth."
            )
        );
        assert!(!page.contains("at1") && !page.contains("rt1"));
        assert_eq!(
            h[CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'"
        );
        let form = seen.recv().await.unwrap();
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "good");
        assert_eq!(form["client_id"], "cid 1");
        assert_eq!(form["client_secret"], "csecret");
        assert_eq!(form["redirect_uri"], DEFAULT_REDIRECT);

        // A replayed callback does not exchange again.
        let (s, _, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(page.contains(r#"data-state="err""#));
        assert!(page.contains("already used"));
        assert!(seen.try_recv().is_err());

        assert_eq!(redeem(wrong).await.0, StatusCode::FORBIDDEN);
        let (s, v) = redeem(VERIFIER).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["access_token"], "at1");
        assert_eq!(v["refresh_token"], "rt1");
        assert_eq!(v["expires_in"], 2592000);
        // One-time: subsequent redeem gets 410 Gone, start gets 409, callback says already used.
        assert_eq!(redeem(VERIFIER).await.0, StatusCode::GONE);
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::CONFLICT
        );
        let (s, _, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(page.contains("already used"));
    }

    #[tokio::test]
    async fn pre_start_redeem_race_succeeds_and_stays_one_time() {
        let (token_url, _seen) = fake_do().await;
        let r = router(configured(&token_url));
        let redeem = |verifier: &'static str| {
            let r = r.clone();
            async move {
                post_json(
                    &r,
                    "/oauth/digitalocean/redeem",
                    json!({"state": STATE, "verifier": verifier}),
                )
                .await
            }
        };
        let wrong = "d3Jvbmd3cm9uZ3dyb25nd3Jvbmd3cm9uZ3dyb25nd3I";

        // Engine polls /redeem BEFORE the browser opens /start.
        assert_eq!(
            redeem(VERIFIER).await,
            (StatusCode::ACCEPTED, json!({"pending": true}))
        );
        // Wrong verifier during the pre-start window is still refused.
        assert_eq!(redeem(wrong).await.0, StatusCode::FORBIDDEN);
        // Browser hits /start with matching challenge → 302; mismatched challenge → 409.
        assert_eq!(
            get(&r, &start_uri(STATE, "other")).await.0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        // Callback exchanges the code.
        let (s, _, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(page.contains(r#"data-state="ok""#));
        // Redeem returns tokens once, then 410 Gone.
        let (s, v) = redeem(VERIFIER).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["access_token"], "at1");
        assert_eq!(v["refresh_token"], "rt1");
        assert_eq!(redeem(VERIFIER).await.0, StatusCode::GONE);
    }

    #[tokio::test]
    async fn digitalocean_errors_reach_the_page_and_the_redeem() {
        const STATE2: &str = "c3RhdGUyc3RhdGUyc3RhdGUyc3RhdGUyc3RhdGUyc3Q";
        let (token_url, _seen) = fake_do().await;
        let r = router(configured(&token_url));
        let redeem = |state: &'static str| {
            let r = r.clone();
            async move {
                post_json(
                    &r,
                    "/oauth/digitalocean/redeem",
                    json!({"state": state, "verifier": VERIFIER}),
                )
                .await
            }
        };

        // The user pressed Deny.
        get(&r, &start_uri(STATE, VERIFIER)).await;
        let (s, h, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?error=access_denied&error_description=%3Cb%3Eno%3C%2Fb%3E&state={STATE}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(
            h[CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'"
        );
        assert!(page.contains(r#"data-state="err""#));
        assert!(page.contains(r#"<div class="brand">theuth</div>"#));
        assert!(!page.contains("<script"));
        assert!(page.contains("&lt;b&gt;no&lt;/b&gt;"));
        assert!(!page.contains("<b>no</b>"));
        assert_eq!(
            redeem(STATE).await,
            (StatusCode::BAD_REQUEST, json!({"error": "<b>no</b>"}))
        );
        assert_eq!(redeem(STATE).await.0, StatusCode::GONE);

        // The code exchange is refused (fresh state; finished STATE cannot be reused).
        assert_eq!(
            get(&r, &start_uri(STATE2, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        let (s, _, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=stale&state={STATE2}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_GATEWAY);
        assert!(page.contains(r#"data-state="err""#));
        assert!(page.contains(r#"<div class="brand">theuth</div>"#));
        assert!(!page.contains("<script"));
        assert_eq!(
            redeem(STATE2).await,
            (
                StatusCode::BAD_REQUEST,
                json!({"error": "The provided authorization grant is invalid"})
            )
        );
        assert_eq!(redeem(STATE2).await.0, StatusCode::GONE);
    }

    #[tokio::test]
    async fn refresh_passes_digitalocean_through() {
        let (token_url, mut seen) = fake_do().await;
        let r = router(configured(&token_url));
        let (s, v) = post_json(
            &r,
            "/oauth/digitalocean/refresh",
            json!({"refresh_token": "rt1"}),
        )
        .await;
        assert_eq!(
            (s, v),
            (
                StatusCode::OK,
                json!({"access_token": "at2", "refresh_token": "rt2"})
            )
        );
        let form = seen.recv().await.unwrap();
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["client_id"], "cid 1");
        assert_eq!(form["client_secret"], "csecret");
        let (s, v) = post_json(
            &r,
            "/oauth/digitalocean/refresh",
            json!({"refresh_token": "nope"}),
        )
        .await;
        assert_eq!(
            (s, v),
            (StatusCode::UNAUTHORIZED, json!({"error": "invalid_grant"}))
        );
    }

    #[tokio::test]
    async fn sign_ins_expire() {
        const STATE2: &str = "c3RhdGUyc3RhdGUyc3RhdGUyc3RhdGUyc3RhdGUyc3Q";
        let mut oauth = configured("http://127.0.0.1:9/unused");
        oauth.ttl = Duration::from_millis(50);
        let r = router(oauth);
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        let (s, _) = post_json(
            &r,
            "/oauth/digitalocean/redeem",
            json!({"state": STATE, "verifier": VERIFIER}),
        )
        .await;
        assert_eq!(s, StatusCode::GONE);
        // Tombstone keeps the expired state from being resurrected immediately.
        assert_eq!(
            post_json(
                &r,
                "/oauth/digitalocean/redeem",
                json!({"state": STATE, "verifier": VERIFIER}),
            )
            .await
            .0,
            StatusCode::GONE
        );
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::CONFLICT
        );

        assert_eq!(
            get(&r, &start_uri(STATE2, VERIFIER)).await.0,
            StatusCode::FOUND
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        let (s, _, page) = get(
            &r,
            &format!("/oauth/digitalocean/callback?code=good&state={STATE2}"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(page.contains("expired"));
    }

    #[tokio::test]
    async fn the_pending_map_is_capped() {
        let oauth = Arc::new(configured("http://127.0.0.1:9/unused"));
        {
            let mut p = oauth.pending();
            for i in 0..MAX_PENDING {
                p.insert(
                    format!("{i:0>43}"),
                    Entry::Pending(Pending {
                        challenge: String::new(),
                        created: Instant::now(),
                        outcome: None,
                    }),
                );
            }
        }
        let app = App {
            relay: Arc::new(Relay::new(64)),
            web: None,
            oauth: oauth.clone(),
            claims: Arc::default(),
        };
        let r = routes()
            .with_state(app)
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 4242))));
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            post_json(
                &r,
                "/oauth/digitalocean/redeem",
                json!({"state": STATE, "verifier": VERIFIER}),
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );

        // Finished tombstones do not count toward MAX_PENDING and are bounded at MAX_FINISHED.
        {
            let mut p = oauth.pending();
            p.clear();
            let base = Instant::now();
            for i in 0..(MAX_FINISHED + 10) {
                p.insert(
                    format!("{i:0>43}"),
                    Entry::Finished(base - Duration::from_millis((MAX_FINISHED + 10 - i) as u64)),
                );
            }
            cap_finished(&mut p);
            assert_eq!(p.len(), MAX_FINISHED);
            // Oldest entries were evicted; newest remain.
            assert!(!p.contains_key(&format!("{:0>43}", 0)));
            assert!(p.contains_key(&format!("{:0>43}", MAX_FINISHED + 9)));
        }
        assert_eq!(
            get(&r, &start_uri(STATE, VERIFIER)).await.0,
            StatusCode::FOUND
        );
    }
}
