use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use runtime::Runtime;
use rpc_core::ModuleFactory;
use tokio::net::TcpListener;
use agent::rpc_forward as forward;
use crate::{RpcConfig, configured_listen, accept, api, engine, acp};

/// Install immutable forwarding hooks that keep reading the current registries.
pub(crate) fn install_forward_hooks() {
    forward::install_forward_hooks(forward::ForwardHooks {
        sessions_running: || sessionsync::actor().is_some(),
        expected: sessionsync::owner::expected,
        attachment_name: artifact::mesh::attachment_name,
        attachments_dir: artifact::mesh::attachments_dir,
        fetch_attachment: |t, dir, name, prefer| {
            Box::pin(artifact::mesh::fetch_attachment_via(t, dir, name, prefer))
        },
    });
}

/// Construct shared RPC state after installing plugin-owned startup hooks.
pub fn new_server(rt: Runtime) -> rpc_core::Server {
    install_forward_hooks();
    agent::rpc_document::install_insight_hooks(&rt);
    rpc_core::Server::new(rt)
}

/// The plugin. Reads `<data_dir>/token` (creating it) and requires it as
/// `Authorization: Bearer <token>` on the WebSocket upgrade.
pub struct RpcPlugin {
    /// 63 §6.4: the server nobody asked for — the engine behind a `t`, `-p`
    /// or `--acp` without `--serve` — so every engine is attachable. Binds
    /// `rpc.listen` when the config sets one, else an ephemeral loopback
    /// port; a busy configured port falls back to ephemeral with a warning
    /// instead of failing the start. `false` = `--serve`/`--headless`: the
    /// configured address, bind failure fatal, as before.
    pub implicit: bool,
    /// Method assembly supplied by the composition root.
    pub module: ModuleFactory,
}

/// Any loopback port.
const EPHEMERAL: &str = "127.0.0.1:0";

#[async_trait::async_trait]
impl runtime::Plugin for RpcPlugin {
    fn name(&self) -> &'static str {
        "rpc"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let module = self.module;
        let mut cfg: RpcConfig = rt.section("rpc").unwrap_or_default();
        if self.implicit && configured_listen(rt).is_none() {
            cfg.listen = EPHEMERAL.to_owned();
        }
        let addr = match serve(rt, cfg.clone(), module).await {
            Err(e) if self.implicit && cfg.listen != EPHEMERAL => {
                tracing::warn!(listen = %cfg.listen, error = %e, "rpc.listen busy; binding an ephemeral port");
                cfg.listen = EPHEMERAL.to_owned();
                serve(rt, cfg, module).await?
            }
            r => r?,
        };
        engine::set_listen(addr.to_string());
        // The link plugin dispatches decrypted device requests into our
        // methods and serves ACP over mode-1 conns (32 §4.3); it registers
        // in its own start, so hand it the dispatch once it is there.
        let link_rt = rt.clone();
        tokio::spawn(async move {
            let rt = link_rt;
            let methods = methods(rt.clone(), module).await;
            if let Some(link) = rt.lookup_when::<link::LinkActor>(link::LINK_ACTOR).await {
                let acp: link::AcpServe = Arc::new(move |rt, ch: link::RecordChannel| {
                    Box::pin(acp::serve_records(rt, ch.inbound, ch.outbound, module))
                });
                let _ = link.tell(link::SetDispatch { methods, acp }).await;
            }
        });
        // 67 §4.2: the same ACP over the mesh's `theuth/acp/1` streams, and
        // mesh engines registered with `agent` as nodes.
        tokio::spawn(acp::serve_mesh(rt.clone(), module));
        Ok(())
    }
}

/// Assemble RPC methods over the single owner-declared wire feed.
pub async fn methods(rt: Runtime, module: ModuleFactory) -> jsonrpsee::Methods {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<api::StreamControl>().await
        .expect("rpc.stream contract");
    module(new_server(rt)).into()
}

/// Start the server; returns the bound address. The server
/// stops when `rt.shutdown` is cancelled.
pub async fn serve(rt: &Runtime, cfg: RpcConfig, module: ModuleFactory) -> anyhow::Result<SocketAddr> {
    let token_path = rt.data_dir.join("token");
    let token = load_token(&token_path)?;
    let methods = methods(rt.clone(), module).await;
    let listener = TcpListener::bind(cfg.listen.as_str()).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, token = %token_path.display(), "rpc listening");
    let shutdown = rt.shutdown.clone();
    tokio::spawn(accept(listener, methods, token, async move {
        shutdown.cancelled().await;
    }));
    Ok(addr)
}

/// Read the token, or create it (64 hex chars, owner-only).
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
    platform::files::write_private(path, token.as_bytes())?;
    Ok(token.into())
}
