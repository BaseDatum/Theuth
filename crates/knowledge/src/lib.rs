//! Knowledge plugin: an OKF v0.1 bundle the agent reads by id, searches, and
//! builds while the user works (design doc 10; plugin cut in 12 §9).
//!
//! - [`store`]: the bundle on disk + the FTS5 index (`Knowledge`).
//! - [`repo`]: local git history of the bundle via `gix` (no binary).
//! - [`search`]: human fuzzy search over current concepts and their full
//!   history, and read-at-commit (design doc 49). FTS retrieval is untouched.
//! - [`harvest`]: the cache-hot continuation's instruction and reply parser,
//!   and the `<knowledge>` hint rendering. The harvest *model call* stays in
//!   the agent loop, which owns the frame.
//! - [`tools`]: the `knowledge_*` tools.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`KNOWLEDGE_ACTOR`] actor ([`KnowledgeActor`]): [`Handle`],
//!   [`Search`], [`Get`], [`List`], [`PutConcept`], [`Write`], [`Delete`], [`HistoryOf`],
//!   [`RevisionOf`], [`Restore`], [`SyncNow`], [`RegisterScoper`], [`search::SearchConcepts`],
//!   [`search::SearchHistory`], [`search::ReadRevision`], [`search::RefreshSearch`]
//! - event: [`KnowledgeChanged`], published by the store after every write
//!
//! Design 59 §7.1: a concept open in the BlockNote editor is a `document`;
//! its export rewrites `<id>.md`, and this plugin hears [`document::Exported`]
//! and runs `sync` so the file is indexed and committed like any other edit.
//! Writes through the store hand the new text back to the document by block
//! diff (`store::notify_document`); `delete` moves the document to `.trash`.

use std::sync::Arc;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
// Leading `::`: the `tools` crate, not this crate's `tools` module.
use ::tools::{REGISTRY_ACTOR, ToolRegistry};

pub mod harvest;
pub mod repo;
pub mod search;
pub mod store;
pub mod sync;
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
    /// Seconds between bundle syncs with every connected engine (67 §5.2);
    /// a sync also runs on each connect.
    pub sync_secs: u64,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sync_secs: 300,
        }
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

/// `ask` → `Result<Meta, String>`: create or update a concept from raw OKF
/// text (the editor path). Frontmatter is kept as given plus `source: user`
/// and a fresh `timestamp`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Write {
    /// Concept id.
    pub id: String,
    /// The whole document: frontmatter and body.
    pub text: String,
}

/// `ask` → `()`: install the [`Scoper`] that says which agent a session is
/// (67 §9); the roster plugin sends it once. Until then every session reads
/// everything and writes as the user.
pub struct RegisterScoper(pub Arc<dyn Scoper>);

impl Message<RegisterScoper> for KnowledgeActor {
    type Reply = ();

    async fn handle(&mut self, m: RegisterScoper, _: &mut Context<Self, Self::Reply>) {
        self.k.set_scoper(m.0);
    }
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

/// `ask` → `Result<Revision, String>`: one commit against its first parent (10 §2.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevisionOf {
    /// Abbreviated or full commit id.
    pub commit: String,
    /// Concept id; `None` for every concept the commit touched.
    pub id: Option<String>,
}

/// `ask` → `Result<Change, String>`: rewrite a concept from a commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restore {
    /// Concept id.
    pub id: String,
    /// Abbreviated commit id; default: the commit before the latest change.
    pub commit: Option<String>,
}

/// `ask` → `Result<Sync, String>`: reconcile the index with the files now (10 §3.2).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SyncNow;

impl Message<document::Exported> for KnowledgeActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: document::Exported,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !matches!(m.binding, document::Binding::Knowledge(_)) {
            return;
        }
        // The editor rewrote a concept file (59 §4.3): index + commit it (10 §2.3 step 6).
        if let Err(e) = self.k.sync().await {
            tracing::warn!(
                error = format!("{e:#}"),
                "knowledge sync after document export failed"
            );
        }
    }
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

impl Message<Write> for KnowledgeActor {
    type Reply = Result<Meta, String>;

    async fn handle(&mut self, m: Write, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .write(&m.id, &m.text)
            .await
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

impl Message<RevisionOf> for KnowledgeActor {
    type Reply = Result<Revision, String>;

    async fn handle(&mut self, m: RevisionOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .revision(&m.commit, m.id.as_deref())
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

impl Message<SyncNow> for KnowledgeActor {
    type Reply = Result<Sync, String>;

    async fn handle(&mut self, _: SyncNow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k.sync().await.map_err(|e| format!("{e:#}"))
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
        let actor = KnowledgeActor::spawn_with_mailbox(
            KnowledgeActor { k: k.clone() },
            runtime::subscriber_mailbox(),
        );
        actor.register(KNOWLEDGE_ACTOR)?;
        rt.subscribe::<KnowledgeActor, document::Exported>(&actor)
            .await;
        sync::start(k.clone(), cfg.sync_secs);

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
