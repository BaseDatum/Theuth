//! Knowledge document copies use the same physical partition as their concepts.
//! Isolated editors are local actors: no RPC registration, mirror replication or mesh relay.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{DocumentActor, Exported, Flush, Tick};

/// Session selector installed by the knowledge plugin, without a crate dependency cycle.
#[async_trait::async_trait]
pub trait Router: Send + Sync {
    /// Physical base directory of the selected bundle, or a fail-closed error.
    async fn root_for(&self, session: SessionId) -> anyhow::Result<PathBuf>;
    /// Reconcile a completed local editor export into its original bundle.
    async fn sync_root(&self, root: &Path) -> anyhow::Result<()>;
    /// Parent configuration and cancellation; local copies use a separate event bus.
    fn runtime(&self) -> Runtime;
}

static ROUTER: RwLock<Option<Arc<dyn Router>>> = RwLock::new(None);
struct Local {
    actor: ActorRef<DocumentActor>,
    stop: CancellationToken,
}
static LOCALS: LazyLock<Mutex<HashMap<PathBuf, Local>>> = LazyLock::new(Mutex::default);

/// Install the engine's session selector before serving model calls.
pub fn install(router: Arc<dyn Router>) {
    *ROUTER.write().unwrap_or_else(|p| p.into_inner()) = Some(router);
}

/// The installed selector; absence never grants main-memory access.
pub fn router() -> anyhow::Result<Arc<dyn Router>> {
    ROUTER
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .ok_or_else(|| anyhow::anyhow!("isolated memory unavailable here: identity not ready"))
}

#[derive(kameo::Actor)]
struct Exports {
    router: Arc<dyn Router>,
    root: PathBuf,
}
impl Message<Exported> for Exports {
    type Reply = ();
    async fn handle(&mut self, _: Exported, _: &mut Context<Self, ()>) {
        if let Err(e) = self.router.sync_root(&self.root).await {
            tracing::warn!("isolated document sync: {e:#}");
        }
    }
}

/// Lazily open an unregistered, non-replicating editor beside the isolated bundle.
pub async fn local(
    root: PathBuf,
    router: Arc<dyn Router>,
) -> anyhow::Result<ActorRef<DocumentActor>> {
    let mut locals = LOCALS.lock().await;
    if let Some(local) = locals.get(&root) {
        return Ok(local.actor.clone());
    }
    let main = router.runtime();
    let mut rt = Runtime::new(main.config.clone(), root.clone());
    rt.shutdown = main.shutdown.child_token();
    let stop = rt.shutdown.clone();
    let store = crate::store::Store::open(&root).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let helper = crate::helper::Helper::new(crate::helper_path(), tx, stop.clone());
    let mut cfg = rt
        .section::<crate::DocumentsConfig>("documents")
        .unwrap_or_default();
    cfg.sharing = "disabled".into();
    let actor = DocumentActor::spawn(DocumentActor::new(store, helper, cfg, rt.clone()));
    let exports = Exports::spawn_with_mailbox(
        Exports {
            router,
            root: root.clone(),
        },
        runtime::subscriber_mailbox(),
    );
    rt.subscribe::<Exports, Exported>(&exports).await;
    let (a, shutdown) = (actor.clone(), stop.clone());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => { let _ = a.ask(Flush).await; a.kill(); exports.kill(); break; }
                ev = rx.recv() => match ev { Some(ev) => { if a.tell(ev).await.is_err() { break; } }, None => break },
                _ = tick.tick() => { if a.tell(Tick).await.is_err() { break; } }
            }
        }
    });
    locals.insert(
        root,
        Local {
            actor: actor.clone(),
            stop,
        },
    );
    Ok(actor)
}

/// An already-open local editor, for knowledge file change notifications.
pub async fn lookup(root: &Path) -> Option<ActorRef<DocumentActor>> {
    LOCALS.lock().await.get(root).map(|l| l.actor.clone())
}

/// Stop a removed agent's editor and helper before its directory moves.
pub async fn remove(root: &Path) {
    if let Some(local) = LOCALS.lock().await.remove(root) {
        local.stop.cancel();
        let _ = local.actor.stop_gracefully().await;
        local.actor.wait_for_shutdown().await;
    }
}
