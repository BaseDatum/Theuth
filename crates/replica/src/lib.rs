//! Replica plugin (design doc 67 §5.1): small-record replication for every
//! plugin that marks tables as replicated. Per-column last-writer-wins under a
//! hybrid logical clock; the change feed is exchanged over the mesh's
//! `theuth/records` stream on connect and on every local commit. The plugin
//! knows nothing about iroh: the mesh hands it a [`Transport`].
//!
//! Public surface consumed by other plugins:
//! - [`attach`]: mark tables of a pool as replicated, get a [`ReplicaHandle`]
//! - [`RegisterTransport`] on the [`REPLICA_ACTOR`] actor (the mesh calls it)
//! - the `ALPN_*` stream names every mesh stream user shares
//!
//! How it works: [`engine`] (capture, clocks, feed), `apply` (LWW merge),
//! `sync` (the `theuth/records` protocol), [`files`] (directories as rows).
//! Tests and the sync engine drive an [`Engine`] directly; the plugin owns one.

mod apply;
pub mod engine;
pub mod files;
pub mod store;
mod sync;
pub mod types;
mod value;

use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use sqlx::SqlitePool;

pub use engine::{Clock, Engine, ReplicaHandle, Stats, stats};
pub use sync::{caught_up_with, plugins_started};
pub use types::{ByteStream, Change, ChangeBatch, Hlc, PeerEvent, PeerId, Transport};

/// Registry name of the [`Replica`] actor.
pub const REPLICA_ACTOR: &str = "replica";

/// Sqlite file under `data_dir` holding sync bookkeeping (never the records).
pub const DB: &str = "replica.sqlite";

/// Stream ALPN: the small-record change feed (67 §4.2, §5.1).
pub const ALPN_RECORDS: &str = "theuth/records/1";
/// Stream ALPN: ACP for a remote child session (67 §4.2, decision 4).
pub const ALPN_ACP: &str = "theuth/acp/1";
/// Stream ALPN: session row log shipping, stage 2 (67 §5.3).
pub const ALPN_SESSIONS: &str = "theuth/sessions/1";
/// Stream ALPN: knowledge bundle fetch (67 §5.2).
pub const ALPN_KNOWLEDGE: &str = "theuth/knowledge/1";
/// Stream ALPN: lease and presence gossip (67 §4.3, §4.6).
pub const ALPN_GOSSIP: &str = "theuth/gossip/1";
/// Stream ALPN: one forwarded JSON-RPC request per stream, front door → owner (68 §5).
pub const ALPN_RPC: &str = "theuth/rpc/1";
/// Stream ALPN: artifact blobs and spill files on demand (68 §7).
pub const ALPN_ARTIFACTS: &str = "theuth/artifacts/1";
/// Stream ALPN: a non-hub front door relays a client's document session to the hub (69 §5, `document::relay`).
pub const ALPN_DOCS: &str = "theuth/docs/1";
/// Stream ALPN: double training bundles to a GPU node and results back (126 §8.4, `double::mesh`).
pub const ALPN_DOUBLE: &str = "theuth/double/1";
/// Stream ALPN: one PTY job's `subscribe_pty` items relayed from the engine running it (224, `agent::rpc_engine`).
pub const ALPN_PTY: &str = "theuth/pty/1";

/// `ask` → `()`: the mesh hands over its transport once the endpoint is up.
/// Replaces any earlier one.
pub struct RegisterTransport(pub Arc<dyn Transport>);

/// `ask` → the registered transport, `None` until the mesh registers one.
/// Other ALPN users (`rpc_compose::acp` for `theuth/acp`, knowledge sync) reach peers through it.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetTransport;

/// The mesh transport, waiting for the replica actor and then for the mesh
/// to register one; `None` on shutdown.
pub async fn transport(rt: &Runtime) -> Option<Arc<dyn Transport>> {
    let replica = rt.lookup_when::<Replica>(REPLICA_ACTOR).await?;
    loop {
        if let Ok(Some(t)) = replica.ask(GetTransport).await {
            return Some(t);
        }
        tokio::select! {
            () = rt.shutdown.cancelled() => return None,
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
}

/// `ask` → the handle: what [`attach`] sends the actor.
pub struct Attach {
    /// The plugin's pool.
    pub pool: SqlitePool,
    /// Tables of that file to replicate.
    pub tables: Vec<String>,
}

/// `ask` → the handle: what [`files::mirror`] sends the actor.
pub struct Mirror {
    /// The plugin's pool.
    pub pool: SqlitePool,
    /// Directory to mirror.
    pub dir: std::path::PathBuf,
    /// Table (created if missing) holding one row per file.
    pub table: String,
    /// 185: local names and same-name clashes ([`files::Policy`]).
    pub policy: Option<std::sync::Arc<dyn files::Policy>>,
}

/// Mark `tables` of `pool` as replicated: install the change capture and
/// forward local commits; peers' batches are merged in as they arrive. Each
/// table needs a single-column primary key and an `hlc TEXT` column; table
/// names are global across plugins. Errors name the offending table.
pub async fn attach(pool: &SqlitePool, tables: &[&str]) -> anyhow::Result<ReplicaHandle> {
    let actor = actor().ok_or_else(|| anyhow::anyhow!("replica plugin is not running"))?;
    actor
        .ask(Attach {
            pool: pool.clone(),
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
        })
        .await
        .map_err(|e| match e {
            kameo::error::SendError::HandlerError(msg) => anyhow::anyhow!(msg),
            other => anyhow::anyhow!("replica actor: {other}"),
        })
}

/// The [`REPLICA_ACTOR`] actor: one [`Engine`] and the transport (once the
/// mesh registers it).
#[derive(kameo::Actor)]
pub struct Replica {
    #[allow(
        dead_code,
        reason = "shutdown and lookups once the sync engine reads config"
    )]
    rt: Runtime,
    engine: Arc<Engine>,
    transport: Option<Arc<dyn Transport>>,
}

impl Message<RegisterTransport> for Replica {
    type Reply = ();

    async fn handle(
        &mut self,
        m: RegisterTransport,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.engine.register_transport(m.0.clone());
        self.transport = Some(m.0);
    }
}

impl Message<GetTransport> for Replica {
    type Reply = Option<Arc<dyn Transport>>;

    async fn handle(&mut self, _: GetTransport, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.engine.transport()
    }
}

impl Message<Attach> for Replica {
    type Reply = Result<ReplicaHandle, String>;

    async fn handle(&mut self, m: Attach, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let tables: Vec<&str> = m.tables.iter().map(String::as_str).collect();
        self.engine
            .attach(&m.pool, &tables)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Mirror> for Replica {
    type Reply = Result<ReplicaHandle, String>;

    async fn handle(&mut self, m: Mirror, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.engine
            .mirror(&m.pool, &m.dir, &m.table, m.policy)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// The plugin: opens `replica.sqlite`, spawns [`Replica`] as [`REPLICA_ACTOR`].
pub struct ReplicaPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ReplicaPlugin {
    fn name(&self) -> &'static str {
        "replica"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let engine = match Engine::open(&rt.data_dir.join(DB)).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("replica unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = Replica::spawn(Replica {
            rt: rt.clone(),
            engine,
            transport: None,
        });
        actor.register(REPLICA_ACTOR)?;
        Ok(())
    }
}

/// The replica actor; `None` before the plugin started or when its store
/// failed to open. Other plugins reach it this way (the `ActorRef::lookup`
/// pattern of `schedule::tools::actor`).
pub fn actor() -> Option<ActorRef<Replica>> {
    ActorRef::<Replica>::lookup(REPLICA_ACTOR).ok().flatten()
}

/// The mesh transport, waiting up to `wait` for the replica plugin and the
/// mesh to register it (polls every second); `None` when neither shows up.
pub async fn transport_within(wait: std::time::Duration) -> Option<Arc<dyn Transport>> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if let Some(a) = actor()
            && let Ok(Some(t)) = a.ask(GetTransport).await
        {
            return Some(t);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests;
