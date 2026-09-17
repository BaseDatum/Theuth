//! RPC plugin. Methods are `ask`s on named actors; `subscribe_events` streams
//! every wire-visible bus event as `{"type": ..., ...}`. See design doc §7.
//!
//! [`api::Api`] is the single list of harness commands. In-process callers
//! (the TUI) hold an [`api::Server`] from [`api::Server::new`] and call its
//! [`ApiServer`] methods directly; `serve` exposes the same trait over the
//! socket for remote clients.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::{Either, Ready, ready};
use jsonrpsee::core::BoxError;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse, ServerBuilder};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tower::Service;
use tower::layer::layer_fn;

pub mod acp;
pub mod api;
pub mod bridge;

#[cfg(test)]
mod export;

pub use acp::AcpPlugin;
pub use api::ApiServer;

/// `[rpc]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RpcConfig {
    /// Bind address; loopback only by design.
    pub listen: String,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7777".into(),
        }
    }
}

/// The plugin. Reads `<data_dir>/token` (creating it) and requires it as
/// `Authorization: Bearer <token>` on the WebSocket upgrade.
pub struct RpcPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for RpcPlugin {
    fn name(&self) -> &'static str {
        "rpc"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        serve(rt, rt.section("rpc")?).await?;
        // The link plugin dispatches decrypted device requests into our
        // methods and serves ACP over mode-1 conns (32 §4.3); it registers
        // in its own start, so hand it the dispatch once it is there.
        let link_rt = rt.clone();
        tokio::spawn(async move {
            let rt = link_rt;
            let methods = methods(rt.clone()).await;
            if let Some(link) = rt.lookup_when::<link::LinkActor>(link::LINK_ACTOR).await {
                let acp: link::AcpServe = Arc::new(|rt, ch: link::RecordChannel| {
                    Box::pin(acp::serve_records(rt, ch.inbound, ch.outbound))
                });
                let _ = link.tell(link::SetDispatch { methods, acp }).await;
            }
        });
        // 39 §3.4: how many frontends hold a receiver; the updater only
        // restarts a service daemon when this is 0.
        let rt = rt.clone();
        tokio::spawn(async move {
            let tx = bridge::start(&rt).await;
            if let Some(u) = rt
                .lookup_when::<update::UpdateActor>(update::UPDATE_ACTOR)
                .await
            {
                let _ = u
                    .tell(update::SetAttached(Arc::new(move || tx.receiver_count())))
                    .await;
            }
        });
        Ok(())
    }
}

/// The full [`api::Api`] as a jsonrpsee `Methods` object, fed by the shared
/// [`bridge`]: what `serve` binds to the socket and what the link plugin
/// dispatches decrypted requests into with `raw_json_request` (31 §6.2).
pub async fn methods(rt: Runtime) -> jsonrpsee::Methods {
    let events = bridge::start(&rt).await;
    api::Server { events, rt }.into_rpc().into()
}

/// Start the bridge and the server; returns the bound address. The server
/// stops when `rt.shutdown` is cancelled.
pub async fn serve(rt: &Runtime, cfg: RpcConfig) -> anyhow::Result<SocketAddr> {
    let token_path = rt.data_dir.join("token");
    let token = load_token(&token_path)?;
    let methods = methods(rt.clone()).await;

    let auth = tower::ServiceBuilder::new().layer(layer_fn(move |inner| Auth {
        inner,
        token: token.clone(),
    }));
    let server = ServerBuilder::new()
        .set_http_middleware(auth)
        .build(cfg.listen.as_str())
        .await?;
    let addr = server.local_addr()?;
    let handle = server.start(methods);
    tracing::info!(%addr, token = %token_path.display(), "rpc listening");

    let shutdown = rt.shutdown.clone();
    tokio::spawn(async move {
        shutdown.cancelled().await;
        let _ = handle.stop();
        handle.stopped().await;
    });
    Ok(addr)
}

/// Read the token, or create it (64 hex chars, mode 0600).
fn load_token(path: &Path) -> anyhow::Result<Arc<str>> {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.into());
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    std::fs::write(path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(token.into())
}

/// HTTP middleware: 401 unless `Authorization: Bearer <token>` or `?token=<token>`.
#[derive(Clone)]
struct Auth<S> {
    inner: S,
    token: Arc<str>,
}

impl<S> Auth<S> {
    fn authorized(&self, req: &HttpRequest) -> bool {
        let token = &*self.token;
        let header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| t == token);
        // ponytail: plain `==`; loopback-only bind, timing side channel not in the threat model.
        header
            || req.uri().query().is_some_and(|q| {
                q.split('&')
                    .any(|kv| kv.strip_prefix("token=") == Some(token))
            })
    }
}

impl<S> Service<HttpRequest> for Auth<S>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError>,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Either<Ready<Result<HttpResponse, BoxError>>, S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        if self.authorized(&req) {
            Either::Right(self.inner.call(req))
        } else {
            let resp = HttpResponse::builder()
                .status(401)
                .body(HttpBody::from("unauthorized"))
                .unwrap_or_default();
            Either::Left(ready(Ok(resp)))
        }
    }
}
