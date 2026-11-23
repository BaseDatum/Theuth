//! Code index plugin (design doc 60): symbols and references of a source tree
//! from tree-sitter's shipped tags queries, in one derived SQLite per root
//! under `<data_dir>/codeindex/`, reconciled by mtime on every call. Steps 1–4
//! of 60 §16: store, parse, import/name resolution, PageRank map, git signals
//! (`gix` diffs and the co-change walk), `code_find`, `code_outline`,
//! `code_refs`, `code_map`, `code_symbol`, `code_impact`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`CODEINDEX_ACTOR`] actor ([`CodeindexActor`]): [`Handle`],
//!   [`Stats`], [`Reconcile`], [`Locate`] (71 §2.5), the lens messages of
//!   [`lens`] (73 §5.2)
//! - the `codeindex_stats` RPC method answers [`Stats`], `code_locate` answers
//!   [`Locate`] (60 §10), `codemap_*` answer the lens messages
//! - events [`CodeindexEvent`] on the bus (73 §5.1)

use std::path::PathBuf;
use std::sync::Arc;

use ::tools::{REGISTRY_ACTOR, ToolRegistry};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub mod event;
pub mod git;
pub mod index;
pub mod lang;
pub mod lens;
pub mod locate;
pub mod map;
pub mod outline_hook;
pub mod parse;
pub mod profile;
pub mod rank;
pub mod resolve;
pub mod root;
pub mod skill;
pub mod store;
pub mod syntax_hook;
pub mod tools;
pub mod walk;
pub mod rpc_support;
/// Code map RPC server and client traits.
pub mod rpc;
pub use rpc::rpc_module;

pub use event::{CodeToolSummary, CodeindexEvent, SummaryDefinition, SummaryImpact, SummaryRange};
pub use index::{Codeindex, Reconciled, RootIndex};
pub use lens::{
    Change, ChangeFile, ChangeLens, ChangeSym, DirRow, Fit, FitEdge, FitLens, FitNode, Here,
    HereLens, LensMeta, MapRow, Touch,
};
pub use locate::{Locate, LocateAnchor, LocateStatus, Located};
pub use map::{
    MapAnchor, MapChild, MapEdge, MapElement, MapEvidence, MapExport, MapFields, MapHit, MapIndex,
    MapJob, MapListing, MapMeta, MapOrigin, MapPerspective, MapProvenance, MapState, MapTreeRow,
    ProfileLine,
};

/// Registry name of the [`CodeindexActor`].
pub const CODEINDEX_ACTOR: &str = "codeindex";
/// Index directory under the data dir (60 §4).
pub const DIR: &str = "codeindex";

/// `codeindex:` config section (60 §9). Every field has a default; the
/// section may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeindexConfig {
    /// `false`: no plugin, no `code_*` functions, nothing on disk.
    pub enabled: bool,
    /// Changed files re-indexed synchronously before answering; more → answer
    /// stale, finish in the background.
    pub sync_limit: usize,
    /// Larger files are listed, not parsed.
    pub max_file_bytes: u64,
    /// First-parent commits walked for co-change (step 4); 0 disables.
    pub cochange_commits: u32,
    /// Languages to parse; others are files only. `typescript` covers TSX.
    pub languages: Vec<String>,
    /// `codeindex.codemap` (147 §10.4): the generated map.
    pub codemap: CodemapConfig,
}

impl Default for CodeindexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sync_limit: 64,
            max_file_bytes: 1_048_576,
            cochange_commits: 300,
            languages: ["rust", "typescript", "javascript", "python", "go"]
                .map(str::to_owned)
                .to_vec(),
            codemap: CodemapConfig::default(),
        }
    }
}

/// `codeindex.codemap:` (147 §10.4, W2). Every field has a default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodemapConfig {
    /// Provider/model for generation (`ModelRole::Named`); empty → the
    /// opening session's model (`ModelRole::Session`).
    pub model: String,
}

/// Reply of [`Stats`] and [`Reconcile`] (60 §10); the `codeindex_stats` RPC payload.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct IndexStats {
    /// Canonical root.
    pub root: PathBuf,
    /// `files` rows.
    pub files: i64,
    /// Files with a parsed language (graph nodes).
    pub parsed_files: i64,
    /// `symbols` rows.
    pub symbols: i64,
    /// `refs` rows.
    pub refs: i64,
    /// Refs with a `to_symbol`.
    pub resolved: i64,
    /// Refs left `ambiguous`.
    pub ambiguous: i64,
    /// `imports` rows.
    pub imports: i64,
    /// Imports with `provenance = 'unresolved'`.
    pub unresolved_imports: i64,
    /// `cochanges` rows (0 outside a git repo or with `cochange_commits: 0`).
    pub cochange_pairs: i64,
    /// Unix seconds of the last write.
    pub last_reconcile_at: Option<u64>,
    /// HEAD at the last co-change walk.
    pub git_head: Option<String>,
    /// PageRank iterations of the last write.
    pub rank_iterations: Option<u64>,
    /// PageRank wall time of the last write, ms.
    pub rank_ms: Option<u64>,
    /// A background reconcile is running.
    pub stale: bool,
    /// Files that background reconcile still has to write.
    pub pending: usize,
}

/// The [`CODEINDEX_ACTOR`] actor: wraps the engine. Handlers await local
/// I/O (walk, sqlite) on the mailbox.
#[derive(kameo::Actor)]
pub struct CodeindexActor {
    ix: Arc<Codeindex>,
}

impl CodeindexActor {
    /// Over an open index; the plugin spawns one at start, tests spawn their own.
    pub fn new(ix: Arc<Codeindex>) -> Self {
        Self { ix }
    }
}

/// `ask` → `Arc<Codeindex>`: the tools' hot path (no mailbox hop per call).
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle;

/// `ask` → `Result<IndexStats, String>`: counts for a root (default: the
/// git top-level of the cwd). Opens the index; does not reconcile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Repository root.
    pub root: Option<PathBuf>,
}

/// `ask` → `Result<IndexStats, String>`: force a synchronous reconcile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reconcile {
    /// Repository root.
    pub root: Option<PathBuf>,
}

impl Message<Handle> for CodeindexActor {
    type Reply = Arc<Codeindex>;

    async fn handle(&mut self, _: Handle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ix.clone()
    }
}

impl Message<Stats> for CodeindexActor {
    type Reply = Result<IndexStats, String>;

    async fn handle(&mut self, m: Stats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let root = root::discover(m.root.as_deref()).map_err(|e| format!("{e:#}"))?;
        let ri = self
            .ix
            .open_root(&root)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ix.stats(&ri).await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<Reconcile> for CodeindexActor {
    type Reply = Result<IndexStats, String>;

    async fn handle(&mut self, m: Reconcile, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let root = root::discover(m.root.as_deref()).map_err(|e| format!("{e:#}"))?;
        let ri = self
            .ix
            .open_root(&root)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ix
            .reconcile(&ri, true)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ix.stats(&ri).await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<Locate> for CodeindexActor {
    type Reply = Result<Vec<Located>, String>;

    async fn handle(&mut self, m: Locate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        locate::locate(&self.ix, m)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// The lens messages (73 §5.2) and the map messages (147 §10.3): store reads,
/// no walk; `Touch` writes, `Generate`/`Cancel` drive the one job per root.
/// The work runs on its own task so a panic in it is an `Err` reply, not a
/// stopped actor (kameo's default `on_panic` would end every later `code_*`).
macro_rules! lens_handler {
    ($($msg:ty => $reply:ty, $f:path;)*) => {$(
        impl Message<$msg> for CodeindexActor {
            type Reply = Result<$reply, String>;

            async fn handle(&mut self, m: $msg, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
                let ix = self.ix.clone();
                match tokio::spawn(async move { $f(&ix, m).await }).await {
                    Ok(r) => r.map_err(|e| format!("{e:#}")),
                    Err(join) => Err(panic_message(stringify!($msg), join)),
                }
            }
        }
    )*};
}

/// The `Err` reply for a handler task that panicked (or was cancelled).
fn panic_message(msg: &str, join: tokio::task::JoinError) -> String {
    if !join.is_panic() {
        return format!("{msg}: handler task cancelled");
    }
    format!(
        "{msg}: handler panicked: {}",
        panic_text(&*join.into_panic())
    )
}

/// The message of a panic payload (`panic!("…")` is a `String` or a `&str`).
pub(crate) fn panic_text(p: &(dyn std::any::Any + Send)) -> &str {
    p.downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| p.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
}
lens_handler! {
    Here => HereLens, lens::here;
    Fit => FitLens, lens::fit;
    Change => ChangeLens, lens::change;
    Touch => LensMeta, lens::touch;
    map::Overview => MapIndex, map::skeleton::overview;
    map::Element => MapElement, map::skeleton::element;
    map::Generate => MapJob, map::generator::generate;
    map::Cancel => MapJob, map::generator::cancel;
    map::Search => Vec<MapHit>, map::skeleton::search;
    map::Export => MapExport, map::export::export;
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<CodeindexEvent>().await
}

/// The plugin: reads `codeindex`, creates `<data_dir>/codeindex/`, spawns
/// [`CodeindexActor`], registers the `code_*` tools once the registry is up.
/// Nothing is indexed at start; the first call on a root pays (60 §0.8).
pub struct CodeindexPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for CodeindexPlugin {
    fn name(&self) -> &'static str {
        "codeindex"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: CodeindexConfig = rt.section("codeindex").unwrap_or_else(|e| {
            tracing::debug!("no [codeindex] config section, using defaults: {e}");
            CodeindexConfig::default()
        });
        // The hook needs no index: text-ref symbols work even when codeindex is off.
        syntax_hook::register();
        if !cfg.enabled {
            tracing::info!("codeindex disabled by config");
            return Ok(());
        }
        skill::install(&rt.data_dir).unwrap_or_else(|e| {
            tracing::warn!("codeindex: bundled skill not written: {e}");
            false
        });
        let ix = match Codeindex::new(cfg, &rt.data_dir) {
            Ok(ix) => Arc::new(ix.with_runtime(rt.clone())),
            Err(e) => {
                tracing::warn!("codeindex unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = CodeindexActor::spawn(CodeindexActor { ix: ix.clone() });
        actor.register(CODEINDEX_ACTOR)?;
        outline_hook::register(ix.clone());
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                tools::register(&reg, ix).await;
            }
        });
        Ok(())
    }
}

/// A test scratch directory `theuth-<tag>-*` under `$TMPDIR`, removed on drop
/// — on a passing test and on a panic alike (141 R4: leaked `theuth-*` test
/// dirs filled the shared machine's disk). Bind it before anything that uses
/// the path, so it drops last.
#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("theuth-{tag}-"))
        .tempdir()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design() {
        let d = CodeindexConfig::default();
        assert!(d.enabled);
        assert_eq!(d.sync_limit, 64);
        assert_eq!(d.max_file_bytes, 1_048_576);
        assert_eq!(d.cochange_commits, 300);
        assert_eq!(
            d.languages,
            ["rust", "typescript", "javascript", "python", "go"]
        );
    }

    #[derive(Clone, Debug)]
    struct Boom;
    async fn boom(_: &Codeindex, _: Boom) -> anyhow::Result<()> {
        panic!("boom at {}", "skeleton.rs:310")
    }
    lens_handler! { Boom => (), boom; }

    /// A panicking handler is an `Err` reply; the actor keeps serving.
    #[tokio::test]
    async fn handler_panic_does_not_stop_the_actor() {
        let data = std::env::temp_dir().join(format!("theuth-actor-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&data).unwrap();
        let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data).unwrap());
        let actor = CodeindexActor::spawn(CodeindexActor::new(ix.clone()));
        let err = actor.ask(Boom).await.unwrap_err();
        let kameo::error::SendError::HandlerError(text) = err else {
            panic!("expected the handler's Err, got {err:?}");
        };
        assert_eq!(text, "Boom: handler panicked: boom at skeleton.rs:310");
        assert!(actor.is_alive());
        let got = actor.ask(Handle).await.unwrap();
        assert!(Arc::ptr_eq(&got, &ix));
        let _ = std::fs::remove_dir_all(&data);
    }
}
