//! Code index plugin (design doc 60): symbols and references of a source tree
//! from tree-sitter's shipped tags queries, in one derived SQLite per root
//! under `<data_dir>/codeindex/`, reconciled by mtime on every call. Steps 1–2
//! of 60 §16: store, parse, import/name resolution, `code_find`,
//! `code_outline`, `code_refs`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`CODEINDEX_ACTOR`] actor ([`CodeindexActor`]): [`Handle`],
//!   [`Stats`], [`Reconcile`]
//! - no events in v1 (60 §10)

use std::path::PathBuf;
use std::sync::Arc;

use ::tools::{REGISTRY_ACTOR, ToolRegistry};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub mod index;
pub mod lang;
pub mod parse;
pub mod rank;
pub mod resolve;
pub mod root;
pub mod store;
pub mod tools;
pub mod walk;

pub use index::{Codeindex, Reconciled, RootIndex};

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
        }
    }
}

/// Reply of [`Stats`] and [`Reconcile`] (60 §10).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct IndexStats {
    /// Canonical root.
    pub root: PathBuf,
    /// `files` rows.
    pub files: i64,
    /// `symbols` rows.
    pub symbols: i64,
    /// `refs` rows.
    pub refs: i64,
    /// Resolved refs (0 until step 2).
    pub resolved: i64,
    /// Ambiguous refs (0 until step 2).
    pub ambiguous: i64,
    /// Unix seconds of the last write.
    pub last_reconcile: Option<u64>,
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
        let ri = self.ix.open_root(&root).await.map_err(|e| format!("{e:#}"))?;
        self.ix.stats(&ri).await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<Reconcile> for CodeindexActor {
    type Reply = Result<IndexStats, String>;

    async fn handle(&mut self, m: Reconcile, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let root = root::discover(m.root.as_deref()).map_err(|e| format!("{e:#}"))?;
        let ri = self.ix.open_root(&root).await.map_err(|e| format!("{e:#}"))?;
        self.ix
            .reconcile(&ri, true)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ix.stats(&ri).await.map_err(|e| format!("{e:#}"))
    }
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

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: CodeindexConfig = rt.section("codeindex").unwrap_or_else(|e| {
            tracing::debug!("no [codeindex] config section, using defaults: {e}");
            CodeindexConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("codeindex disabled by config");
            return Ok(());
        }
        let ix = match Codeindex::new(cfg, &rt.data_dir) {
            Ok(ix) => Arc::new(ix),
            Err(e) => {
                tracing::warn!("codeindex unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = CodeindexActor::spawn(CodeindexActor { ix: ix.clone() });
        actor.register(CODEINDEX_ACTOR)?;
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                tools::register(&reg, ix).await;
            }
        });
        Ok(())
    }
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
}
