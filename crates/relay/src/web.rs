//! `--web <dir>`: serve the UI bundle with CSP and the `__Host-link` cookie (design doc 31 §7.6).

use std::path::Path;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::App;
use crate::cookie::{self, Signer};

/// CSP on every served page (streamdown needs inline styles).
pub const CSP: &str = "default-src 'self'; connect-src 'self' wss: ws:; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'";

/// Static-serving state.
pub struct Web {
    /// Cookie HMAC.
    pub signer: Signer,
    /// `dir`, any non-file path → `index.html`.
    pub files: ServeDir<ServeFile>,
    /// Listening with `--tls`, so our own origin is `https://` unless a proxy says otherwise.
    pub tls: bool,
}

impl Web {
    /// Serve `dir`.
    pub fn new(dir: &Path, signer: Signer, tls: bool) -> Self {
        let files = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        Self { signer, files, tls }
    }
}

fn header(h: &HeaderMap, name: impl axum::http::header::AsHeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn valid_cookie(web: &Web, h: &HeaderMap) -> bool {
    header(h, COOKIE)
        .and_then(cookie::from_header)
        .is_some_and(|v| web.signer.verify(v))
}

/// `false` when the upgrade's `Origin` is the relay's own origin and no valid
/// cookie rides along. Other or absent origins (native apps) pass.
pub fn same_origin_ok(web: &Web, h: &HeaderMap) -> bool {
    let (Some(origin), Some(host)) = (header(h, ORIGIN), header(h, HOST)) else {
        return true;
    };
    let scheme = header(h, "x-forwarded-proto").unwrap_or(if web.tls { "https" } else { "http" });
    let ours = origin
        .strip_prefix(scheme)
        .and_then(|r| r.strip_prefix("://"))
        .is_some_and(|origin_host| origin_host.eq_ignore_ascii_case(host));
    !ours || valid_cookie(web, h)
}

/// Fallback handler: the UI bundle, CSP, and a cookie when none (or a bad one) is present.
pub async fn pages(State(app): State<App>, req: Request) -> Response {
    let Some(web) = app.web.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let has_cookie = valid_cookie(web, req.headers());
    let res = match web.files.clone().oneshot(req).await {
        Ok(r) => r,
        Err(e) => match e {},
    };
    let mut res = res.map(Body::new);
    res.headers_mut()
        .insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    if !has_cookie && let Ok(v) = HeaderValue::from_str(&web.signer.set_cookie()) {
        res.headers_mut().append(SET_COOKIE, v);
    }
    res
}
