//! Knowledge plugin: an OKF v0.1 bundle the agent reads by id, searches, and
//! builds while the user works (design doc 10; plugin cut in 12 §9).
//!
//! - [`store`]: the bundle on disk + the FTS5 index (`Knowledge`).
//! - [`repo`]: local git history of the bundle via `gix` (no binary).
//! - [`harvest`]: the cache-hot continuation's instruction and reply parser,
//!   and the `<knowledge>` hint rendering. The harvest *model call* stays in
//!   the agent loop, which owns the frame.
//! - [`tools`]: the `knowledge_*` tools.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`KNOWLEDGE_ACTOR`] actor ([`KnowledgeActor`]): [`Handle`],
//!   [`Search`], [`Get`], [`List`], [`PutConcept`], [`Delete`], [`HistoryOf`], [`Restore`]
//! - event: [`KnowledgeChanged`], published by the store after every write

use std::sync::Arc;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
// Leading `::`: the `tools` crate, not this crate's `tools` module.
use ::tools::{REGISTRY_ACTOR, ToolRegistry};

pub mod harvest;
pub mod repo;
pub mod store;
pub mod tools;
pub mod types;

pub use store::{Knowledge, terms};
pub use types::*;

/// Registry name of the [`KnowledgeActor`].
pub const KNOWLEDGE_ACTOR: &str = "knowledge";
/// Default mid-turn hint cadence in rounds (10 §4.2; `agent.knowledge.hint_rounds`).
pub const KNOWLEDGE_HINT_ROUNDS: u32 = 4;
/// Bundle directory under the data dir (10 §2.1).
pub const DIR: &str = "knowledge";
/// Derived FTS index file under the data dir (10 §3.1).
pub const INDEX_FILE: &str = "knowledge.sqlite";
/// Max body bytes accepted by `put` (10 §2.2).
pub const BODY_MAX: usize = 16 * 1024;
/// Description clip in hint lines (10 §4.1).
pub const HINT_DESCRIPTION_CHARS: usize = 120;

/// `knowledge:` config section (12 §9.2). Loop policy (`hint_limit`, harvest
/// cadence) lives in `agent.knowledge`; this section only switches the plugin.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    /// `false`: no actor, no tools; the agent's lookup finds nothing and hints/harvest are off.
    pub enabled: bool,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Event: a concept was written or removed (10 §8). The plugin's own bus type,
/// not an agent event (12 §9.1).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct KnowledgeChanged {
    /// Concept id.
    pub id: String,
    /// What happened.
    pub op: KnowledgeOp,
    /// Who did it.
    pub source: Source,
}

/// The [`KNOWLEDGE_ACTOR`] actor: wraps the store. Handlers await the store on
/// the mailbox — local I/O (sqlite, files, `spawn_blocking` for the repo).
#[derive(kameo::Actor)]
pub struct KnowledgeActor {
    k: Arc<Knowledge>,
}

/// `ask` → `Arc<Knowledge>`: the loop's hot path (hint/sync/put per round, no mailbox hop).
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle;

/// `ask` → `Result<Vec<Hit>, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Search {
    /// Free text.
    pub query: String,
    /// Default 10, max 50.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Concept, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Get {
    /// Concept id.
    pub id: String,
}

/// `ask` → `Result<Vec<Meta>, String>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct List {
    /// Id prefix.
    pub prefix: Option<String>,
    /// Frontmatter type.
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Tag.
    pub tag: Option<String>,
    /// Default 50.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Meta, String>`: create or update a concept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutConcept {
    /// The document.
    pub put: Put,
    /// Who is writing (`User` from RPC/TUI).
    pub source: Source,
}

/// `ask` → `Result<(), String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delete {
    /// Concept id.
    pub id: String,
}

/// `ask` → `Result<History, String>` (10 §2.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryOf {
    /// Concept id; `None` for the whole bundle.
    pub id: Option<String>,
    /// Default 20.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Change, String>`: rewrite a concept from a commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restore {
    /// Concept id.
    pub id: String,
    /// Abbreviated commit id; default: the commit before the latest change.
    pub commit: Option<String>,
}

impl Message<Handle> for KnowledgeActor {
    type Reply = Arc<Knowledge>;

    async fn handle(&mut self, _: Handle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k.clone()
    }
}

impl Message<Search> for KnowledgeActor {
    type Reply = Result<Vec<Hit>, String>;

    async fn handle(&mut self, m: Search, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        self.k
            .search(&m.query, &cwd, m.limit.unwrap_or(10).clamp(1, 50))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Get> for KnowledgeActor {
    type Reply = Result<Concept, String>;

    async fn handle(&mut self, m: Get, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k.get(&m.id).await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<List> for KnowledgeActor {
    type Reply = Result<Vec<Meta>, String>;

    async fn handle(&mut self, m: List, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .list(
                m.prefix.as_deref(),
                m.type_.as_deref(),
                m.tag.as_deref(),
                m.limit.unwrap_or(50),
            )
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<PutConcept> for KnowledgeActor {
    type Reply = Result<Meta, String>;

    async fn handle(&mut self, m: PutConcept, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .put(m.put, m.source)
            .await
            .map(|(meta, _)| meta)
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Delete> for KnowledgeActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Delete, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k.delete(&m.id).await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<HistoryOf> for KnowledgeActor {
    type Reply = Result<History, String>;

    async fn handle(&mut self, m: HistoryOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .history(m.id.as_deref(), m.limit.unwrap_or(20))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Restore> for KnowledgeActor {
    type Reply = Result<Change, String>;

    async fn handle(&mut self, m: Restore, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .restore(&m.id, m.commit.as_deref())
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// The plugin. Opens the bundle, spawns [`KnowledgeActor`] as [`KNOWLEDGE_ACTOR`],
/// then registers the `knowledge_*` tools once the registry is up (12 §9.1).
pub struct KnowledgePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for KnowledgePlugin {
    fn name(&self) -> &'static str {
        "knowledge"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: KnowledgeConfig = rt.section("knowledge").unwrap_or_else(|e| {
            tracing::debug!("no [knowledge] config section, using defaults: {e}");
            KnowledgeConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("knowledge disabled by config");
            return Ok(());
        }
        let k = match Knowledge::open(&rt.data_dir, rt.clone()).await {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!("knowledge bundle unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = KnowledgeActor::spawn(KnowledgeActor { k: k.clone() });
        actor.register(KNOWLEDGE_ACTOR)?;

        // First use of the registry (00 §4, 12 §2).
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                crate::tools::register(&reg, k).await;
            }
        });
        Ok(())
    }
}
