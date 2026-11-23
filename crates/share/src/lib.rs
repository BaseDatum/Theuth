//! Public web sharing, the client plugin (design doc 210): dials the share edge on `theuth/share/1`,
//! terminates visitors' TLS with the mesh's name key, proxies to a loopback port or a directory, and
//! owns the share store, tools, RPC and consent. Wire types come from `share-proto`.
//!
//! Public surface:
//! - [`SharePlugin`] (registration), [`rpc_module`] (the share RPC), [`declare_wire`]
//! - events [`model::ShareChanged`] (`share.changed`), [`model::ShareTraffic`]
//!   (`share.traffic`), [`model::ShareStatus`] (`share.status`)
//! - tools `share_publish` (destructive, human-only), `share_stop`, `share_list`
//! - seams for other crates: [`edge::EdgeDialer`], [`edge::DelegationSource`],
//!   [`keys::NameKeyStore`], [`sources::OwnPorts`], [`sources::ProcessCheck`]

pub mod config;
pub mod duties;
pub mod edge;
pub mod engine;
pub mod family;
pub mod gate;
pub mod holder;
pub mod keys;
pub mod model;
pub mod names;
pub mod proxy;
pub mod rekey;
pub mod routes;
pub mod rpc_share;
pub mod sources;
pub mod store;
pub mod tools;
pub mod visitor;
pub mod wiring;

use std::sync::{Arc, OnceLock};

use runtime::Runtime;

pub use config::{EdgeChoice, Expiry, ShareConfig};
pub use engine::{Engine, Hooks, Published};
pub use model::{Share, ShareChanged, ShareStatus, ShareTraffic};

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_share::rpc_module(server)
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// The running service for RPC handlers.
pub mod rpc_support {
    use std::sync::Arc;

    use jsonrpsee::core::RpcResult;
    use jsonrpsee::types::ErrorObjectOwned;
    use rpc_core::NOT_RUNNING;

    /// The share engine; `NOT_RUNNING` when the plugin is not started.
    pub fn engine() -> RpcResult<Arc<crate::Engine>> {
        super::ENGINE
            .get()
            .cloned()
            .ok_or_else(|| ErrorObjectOwned::owned(NOT_RUNNING, "sharing is not running", None::<()>))
    }
}

/// Declare the share wire contracts.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<ShareChanged>().await?;
    rt.register_wire_event::<ShareTraffic>().await?;
    rt.register_wire_event::<ShareStatus>().await?;
    Ok(())
}

/// The plugin: reads `share`, opens `<data_dir>/share.sqlite`, attaches the replicated tables,
/// starts the [`Engine`] and registers the three tools.
pub struct SharePlugin {
    /// This engine's endpoint id (the mesh identity's public key); empty = read it from `mesh`
    /// at start.
    pub engine_id: String,
    /// Seams the wiring fills.
    pub hooks: Hooks,
}

impl SharePlugin {
    /// The engine's plugin: every seam on the mesh and vault adapters of [`wiring`], the
    /// engine id from the mesh identity. List it after `mesh::MeshPlugin`.
    pub fn engine() -> Self {
        Self {
            engine_id: String::new(),
            hooks: wiring::engine_hooks(),
        }
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for SharePlugin {
    fn name(&self) -> &'static str {
        "share"
    }

    async fn declare_wire(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        if let Ok(raw) = rt.section::<serde_json::Value>("share") {
            ShareConfig::warn_ignored(&raw);
        }
        let cfg: ShareConfig = rt.section("share").unwrap_or_default();
        let store = store::Store::open(&rt.data_dir.join("share.sqlite")).await?;
        let replica = match replica::attach(store.pool(), &store::REPLICATED).await {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!("share: replicated tables not attached: {e:#}");
                None
            }
        };
        let SharePlugin {
            mut engine_id,
            hooks,
        } = *self;
        if engine_id.is_empty() {
            engine_id = mesh::self_id()
                .or_else(|| mesh::stored_self_id(&rt.data_dir))
                .unwrap_or_default();
            if engine_id.is_empty() {
                tracing::warn!("share: no mesh identity; sharing cannot attach to an edge");
            }
        }
        // D22: removal duties run whatever the sharing switch says.
        if let Some(dialer) = hooks.dialer.clone() {
            tokio::spawn(duties::run(store.clone(), rt.data_dir.clone(), dialer));
        }
        let engine = Engine::new(cfg, store, rt.data_dir.clone(), engine_id, hooks, Some(rt.clone()))?;
        engine.start().await?;
        let _ = ENGINE.set(engine.clone());
        if let Some(h) = replica {
            tokio::spawn(announce_replicated(engine.clone(), h));
        }
        tokio::spawn(csr_heal_timer(engine.clone()));
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::SharePublishTool::new(
                        engine.clone(),
                    ))))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::ShareStopTool {
                        engine: engine.clone(),
                    })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::ShareListTool { engine })))
                    .await;
            }
        });
        Ok(())
    }
}

/// `share.changed` for `shares` rows another machine wrote, as they arrive by replication
/// (local writes announce themselves).
async fn announce_replicated(engine: Arc<Engine>, h: replica::ReplicaHandle) {
    let mut rx = h.subscribe();
    loop {
        let batch = match rx.recv().await {
            Ok(b) => b,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if &batch.origin == h.origin() {
            continue;
        }
        let ids: std::collections::BTreeSet<&str> = batch
            .changes
            .iter()
            .filter(|c| c.table == "shares")
            .map(|c| c.pk.as_str())
            .collect();
        for id in ids {
            if let Ok(Some(s)) = engine.store.get(id).await {
                engine.emit(&s).await;
            }
        }
    }
}

/// The holder's `csr_mismatch` heal timer (§6.2, P0-6), once a minute.
async fn csr_heal_timer(engine: Arc<Engine>) {
    let mut since = std::collections::HashMap::new();
    let mut posted = std::collections::HashMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tick.tick().await;
        engine
            .csr_heal_tick(&mut since, &mut posted, platform::time::now_ms())
            .await;
    }
}
