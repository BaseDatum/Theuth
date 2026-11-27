//! Link-claim rendezvous (design doc 224 §16.12, the phone-only path): a cloud node made from a
//! phone that is not a mesh member has a one-time link claim in its user data. Once its engine is
//! up it parks its link identity here under the claim's id; the phone polls for it, then pairs over
//! the ordinary `/h/<id>` relay path with the claim's PSK (Noise `IKpsk2`), which is the single
//! use. The relay never sees the PSK: the id is a hash of it, and the `mac` (HMAC-SHA256 under the
//! PSK) lets the phone reject an answer the relay or anyone else forged.
//!
//! `PUT /claim/<id>` `{engine, name, mac}` → 204; the same body again → 204, another → 409.
//! `GET /claim/<id>` → 200 `{engine, name, mac}` | 404. Entries live [`TTL`] from the first put
//! (the claim's own lifetime cap); nothing is consumed by a read, so a phone the OS froze or killed
//! reads again after it comes back. Public data only (an engine's public key and display name).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::{Path, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::App;

/// How long an entry lives: the longest a link claim may live (`link::claim::MAX_TTL_MS`).
pub const TTL: Duration = Duration::from_secs(30 * 60);
/// Entries kept at once; a put past it answers 503.
const MAX_ENTRIES: usize = 4096;
/// `base64url(16 bytes)`.
const ID_LEN: usize = 22;
/// `base64url(32 bytes)`.
const KEY_LEN: usize = 43;
/// Display name bytes.
const MAX_NAME: usize = 128;

/// What a node parks: its link identity's public key, its display name, and the MAC binding both
/// to the claim's PSK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `base64url` X25519 public key (the `/h/<id>` the phone dials).
    pub engine: String,
    /// Display name.
    pub name: String,
    /// `base64url(HMAC-SHA256(psk, …))`, checked by the phone only.
    pub mac: String,
}

/// The parked entries.
#[derive(Default)]
pub struct Claims {
    entries: Mutex<HashMap<String, (Entry, Instant)>>,
    ttl: Option<Duration>,
}

impl Claims {
    fn ttl(&self) -> Duration {
        self.ttl.unwrap_or(TTL)
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Entry, Instant)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The routes, merged into `main`'s router.
pub fn routes() -> Router<App> {
    Router::new().route("/claim/{id}", get(read).put(park))
}

fn b64url(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn no_store(status: StatusCode, body: impl IntoResponse) -> Response {
    let mut r = (status, body).into_response();
    r.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

async fn park(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !app.relay.allow(crate::client_ip(&headers, addr)) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(entry) = serde_json::from_slice::<Entry>(&body) else {
        return no_store(StatusCode::BAD_REQUEST, "expected {engine, name, mac}");
    };
    if !b64url(&id, ID_LEN)
        || !b64url(&entry.engine, KEY_LEN)
        || !b64url(&entry.mac, KEY_LEN)
        || entry.name.is_empty()
        || entry.name.len() > MAX_NAME
    {
        return no_store(StatusCode::BAD_REQUEST, "malformed claim");
    }
    let ttl = app.claims.ttl();
    let mut map = app.claims.entries();
    map.retain(|_, (_, at)| at.elapsed() < ttl);
    match map.get(&id) {
        Some((e, _)) if *e == entry => no_store(StatusCode::NO_CONTENT, ()),
        Some(_) => no_store(StatusCode::CONFLICT, "claim already parked"),
        None if map.len() >= MAX_ENTRIES => no_store(StatusCode::SERVICE_UNAVAILABLE, "busy"),
        None => {
            map.insert(id, (entry, Instant::now()));
            no_store(StatusCode::NO_CONTENT, ())
        }
    }
}

async fn read(State(app): State<App>, Path(id): Path<String>) -> Response {
    if !b64url(&id, ID_LEN) {
        return no_store(StatusCode::BAD_REQUEST, "malformed claim id");
    }
    let ttl = app.claims.ttl();
    let mut map = app.claims.entries();
    match map.get(&id) {
        Some((e, at)) if at.elapsed() < ttl => no_store(StatusCode::OK, axum::Json(e.clone())),
        Some(_) => {
            map.remove(&id);
            no_store(StatusCode::NOT_FOUND, "")
        }
        None => no_store(StatusCode::NOT_FOUND, ""),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::Request;
    use axum::extract::connect_info::MockConnectInfo;
    use tower::ServiceExt;

    use super::*;
    use crate::relay::Relay;

    const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const KEY: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    const MAC: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";

    fn router(ttl: Option<Duration>) -> Router {
        let app = App {
            relay: Arc::new(Relay::new(64)),
            web: None,
            oauth: Arc::new(crate::oauth_do::DoOauth::new(None)),
            claims: Arc::new(Claims {
                entries: Mutex::default(),
                ttl,
            }),
        };
        routes()
            .with_state(app)
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 4242))))
    }

    async fn call(r: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, String) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
            .unwrap();
        let res = r.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn entry(name: &str) -> serde_json::Value {
        serde_json::json!({ "engine": KEY, "name": name, "mac": MAC })
    }

    #[tokio::test]
    async fn park_then_read_any_number_of_times_first_write_wins() {
        let r = router(None);
        let uri = format!("/claim/{ID}");
        assert_eq!(call(&r, "GET", &uri, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&r, "PUT", &uri, Some(entry("lab"))).await.0, StatusCode::NO_CONTENT);
        assert_eq!(
            call(&r, "PUT", &uri, Some(entry("lab"))).await.0,
            StatusCode::NO_CONTENT,
            "the node's retry of the same body"
        );
        assert_eq!(
            call(&r, "PUT", &uri, Some(entry("evil"))).await.0,
            StatusCode::CONFLICT,
            "another body cannot replace it"
        );
        for _ in 0..2 {
            let (status, body) = call(&r, "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK);
            let got: Entry = serde_json::from_str(&body).unwrap();
            assert_eq!((got.engine.as_str(), got.name.as_str(), got.mac.as_str()), (KEY, "lab", MAC));
        }
    }

    #[tokio::test]
    async fn malformed_is_400() {
        let r = router(None);
        assert_eq!(call(&r, "GET", "/claim/short", None).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            call(&r, "PUT", &format!("/claim/{ID}"), Some(serde_json::json!({"engine": "x", "name": "a", "mac": MAC}))).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&r, "PUT", &format!("/claim/{ID}"), Some(entry(""))).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&r, "PUT", &format!("/claim/{ID}"), Some(entry(&"n".repeat(MAX_NAME + 1)))).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(call(&r, "PUT", &format!("/claim/{ID}"), None).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn entries_expire() {
        let r = router(Some(Duration::ZERO));
        let uri = format!("/claim/{ID}");
        assert_eq!(call(&r, "PUT", &uri, Some(entry("lab"))).await.0, StatusCode::NO_CONTENT);
        assert_eq!(call(&r, "GET", &uri, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            call(&r, "PUT", &uri, Some(entry("other"))).await.0,
            StatusCode::NO_CONTENT,
            "an expired entry no longer blocks the id"
        );
    }
}
