//! The listener (158 §12.2): `127.0.0.1:<port>`, HTTP/1 through hyper on a raw `TcpListener`,
//! `Authorization: Bearer <token>` checked here (401), Host/Origin checked by rmcp's own
//! DNS-rebinding guard (403), everything else by `StreamableHttpService`. No framework, no CORS.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::Catalog;

/// The one path a client pastes: `http://127.0.0.1:<port>/mcp`.
pub const PATH: &str = "/mcp";

type Svc = StreamableHttpService<Catalog, LocalSessionManager>;

/// rmcp's config for `port` (158 §12.2): Host pinned to the bound authority, Origin absent or
/// loopback, SSE replies with a 15 s keep-alive, every session ends when `cancel` fires.
pub fn config(port: u16, cancel: CancellationToken) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_allowed_hosts([format!("127.0.0.1:{port}"), format!("localhost:{port}")])
        .with_allowed_origins(["http://localhost", "http://127.0.0.1"])
        .with_sse_keep_alive(Some(Duration::from_secs(15)))
        .with_json_response(false)
        .with_cancellation_token(cancel)
}

/// Bind and serve until `cancel`; returns the bound port.
pub async fn serve(
    port: u16,
    token: String,
    catalog: Catalog,
    cancel: CancellationToken,
) -> anyhow::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    let svc: Arc<Svc> = Arc::new(StreamableHttpService::new(
        move || Ok(catalog.clone()),
        Arc::new(LocalSessionManager::default()),
        config(port, cancel.clone()),
    ));
    let token: Arc<str> = token.into();
    tokio::spawn(async move {
        loop {
            let (stream, _) = tokio::select! {
                r = listener.accept() => match r {
                    Ok(c) => c,
                    Err(e) => { tracing::debug!("mcp accept: {e}"); continue }
                },
                _ = cancel.cancelled() => break,
            };
            let (svc, token, cancel) = (svc.clone(), token.clone(), cancel.clone());
            tokio::spawn(async move {
                let conn = http1::Builder::new().serve_connection(
                    TokioIo::new(stream),
                    service_fn(move |req| handle(svc.clone(), token.clone(), req)),
                );
                tokio::pin!(conn);
                tokio::select! {
                    r = &mut conn => if let Err(e) = r { tracing::debug!("mcp conn: {e}") },
                    _ = cancel.cancelled() => conn.as_mut().graceful_shutdown(),
                }
            });
        }
    });
    Ok(port)
}

async fn handle(
    svc: Arc<Svc>,
    token: Arc<str>,
    req: Request<Incoming>,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    if req.uri().path() != PATH {
        return Ok(plain(StatusCode::NOT_FOUND, "not found"));
    }
    // ponytail: plain `==` like the RPC layer; loopback-only bind, timing side channel out of scope.
    let ok = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == &*token);
    if !ok {
        return Ok(plain(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(svc.handle(req).await)
}

fn plain(status: StatusCode, text: &'static str) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from_static(text.as_bytes())).boxed())
        .expect("static response")
}
