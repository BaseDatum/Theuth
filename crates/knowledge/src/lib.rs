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
//!   [`search::SearchHistory`], [`search::ReadRevision`], [`search::RefreshSearch`];
//!   136 §2.1: [`Merge`], [`Supersede`], [`Archive`], [`Verify`], [`Retitle`], [`Stats`],
//!   [`RegisterConsolidator`]
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
/// cadence) lives in `agent.knowledge`; this section switches the plugin and
/// carries the type lifetimes of 136 §5.2.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    /// `false`: no actor, no tools; the agent's lookup finds nothing and hints/harvest are off.
    pub enabled: bool,
    /// Seconds between bundle syncs with every connected engine (67 §5.2);
    /// a sync also runs on each connect.
    pub sync_secs: u64,
    /// `knowledge.lifetimes` (136 §5.2, days). The rank reads these; dreaming's
    /// `lifetimes` mirror is for its archive/verify sampling only.
    pub lifetimes: Lifetimes,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sync_secs: 300,
            lifetimes: Lifetimes::default(),
        }
    }
}

/// Days a type stays on the hint shelf by age (136 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lifetimes {
    /// `type: record` ranks 0 past this age.
    pub record: u32,
    /// Verify sample: facts whose `verified` is older than this.
    pub fact: u32,
    /// Procedures unopened this long are archive candidates.
    pub procedure: u32,
    /// Entities / hosts / projects unopened this long are archive candidates.
    pub entity: u32,
}

impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            record: 14,
            fact: 30,
            procedure: 90,
            entity: 60,
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
    /// The commit that recorded it (abbreviated); `None` without a repo or
    /// when the commit failed. 136: dreaming's undo receipt restores to its
    /// first parent, so the pre-change state is read without a history walk.
    #[serde(default)]
    pub commit: Option<String>,
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

/// `ask` → `Result<Receipt, String>` (136 §2.1): fold `ids` into `into`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Merge {
    /// Ids folded into `into` (become redirect stubs).
    pub ids: Vec<String>,
    /// The canonical id (may be one of `ids` or new).
    pub into: String,
    /// The canonical's body.
    pub body: String,
    /// Session ids / `user` added to the canonical's `evidence`.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Who is merging; the canonical keeps this scope's owner.
    pub scope: AgentScope,
}

/// `ask` → `Result<Receipt, String>` (136 §2.1): `loser` is replaced by `winner`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Supersede {
    /// The outdated concept.
    pub loser: String,
    /// The newer claim.
    pub winner: String,
    /// RFC 3339 — when the loser stopped being true (from the evidence).
    #[serde(default)]
    pub valid_until: Option<String>,
    /// One line for the loser's body.
    #[serde(default)]
    pub note: Option<String>,
}

/// `ask` → `Result<Receipt, String>` (136 §2.1): `status: archived, hint: false`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Archive {
    /// Concept id.
    pub id: String,
    /// Why; appended under `## Archived`.
    pub reason: String,
}

/// `ask` → `Result<Receipt, String>` (136 §2.1): a read-tool check's verdict.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Verify {
    /// Concept id.
    pub id: String,
    /// `ok` → `verified: <today>`; `stale` → `status: stale` + note.
    pub outcome: VerifyOutcome,
    /// What changed (stale) or what was checked (ok).
    #[serde(default)]
    pub note: Option<String>,
    /// The sources the check read (paths, URLs, `session:`/`meeting:` ids);
    /// unioned into the concept's `evidence`.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// `ask` → `Result<Receipt, String>` (136 §2.1): shorten a title.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Retitle {
    /// Concept id.
    pub id: String,
    /// The new title (≤ 120 chars).
    pub title: String,
    /// The paragraph that keeps the old title's content at the top of the body.
    pub summary: String,
}

/// `ask` → `Result<Vec<Uses>, String>` (136 §2.1): impressions and opens per id.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Concept ids; zeros for unknown ones.
    pub ids: Vec<String>,
}

/// Answers "may this attended session consolidate" (136 §2.1 / D6): the
/// five mutating `knowledge_*` tools of dreaming run only from the open
/// night's dreaming session ([`Knowledge::register_consolidating_session`])
/// or the owner's own root session. The agent plugin installs one with
/// [`RegisterConsolidator`]; until then no attended session may.
#[async_trait::async_trait]
pub trait Consolidator: Send + std::marker::Sync {
    /// `true` when `session` is the owner's own attended root (136 D6:
    /// `scope = 'owner'`, no parent, no desk, not unattended). Unattended
    /// sessions are answered by the store's registered night, never here.
    async fn may_consolidate(&self, session: runtime::SessionId) -> bool;
}

/// `ask` → `()`: install the [`Consolidator`]; the agent plugin sends it once.
pub struct RegisterConsolidator(pub Arc<dyn Consolidator>);

impl Message<RegisterConsolidator> for KnowledgeActor {
    type Reply = ();

    async fn handle(&mut self, m: RegisterConsolidator, _: &mut Context<Self, Self::Reply>) {
        self.k.set_consolidator(m.0);
    }
}

impl Message<Merge> for KnowledgeActor {
    type Reply = Result<Receipt, String>;

    async fn handle(&mut self, m: Merge, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .merge(&m.ids, &m.into, &m.body, &m.evidence, &m.scope)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Supersede> for KnowledgeActor {
    type Reply = Result<Receipt, String>;

    async fn handle(&mut self, m: Supersede, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .supersede(
                &m.loser,
                &m.winner,
                m.valid_until.as_deref(),
                m.note.as_deref(),
            )
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Archive> for KnowledgeActor {
    type Reply = Result<Receipt, String>;

    async fn handle(&mut self, m: Archive, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .archive(&m.id, &m.reason)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Verify> for KnowledgeActor {
    type Reply = Result<Receipt, String>;

    async fn handle(&mut self, m: Verify, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .verify(&m.id, m.outcome, m.note.as_deref(), &m.evidence)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Retitle> for KnowledgeActor {
    type Reply = Result<Receipt, String>;

    async fn handle(&mut self, m: Retitle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k
            .retitle(&m.id, &m.title, &m.summary)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Stats> for KnowledgeActor {
    type Reply = Result<Vec<Uses>, String>;

    async fn handle(&mut self, m: Stats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.k.stats(&m.ids).await.map_err(|e| format!("{e:#}"))
    }
}

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

/// A test scratch directory under `$TMPDIR`, `theuth-<tag>-<uuid>`, removed
/// on drop — on a passing test and on a panic alike (141 R4: leaked
/// `theuth-*` test dirs filled the shared machine's disk). Bind it before
/// anything that uses the path, so it drops last.
#[cfg(test)]
pub(crate) struct TestDir(pub(crate) std::path::PathBuf);

#[cfg(test)]
impl TestDir {
    /// An uncreated `theuth-<tag>-<uuid>` path (for a scratch file); dropped
    /// like the directory form.
    pub(crate) fn path(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!("theuth-{tag}-{}", uuid::Uuid::new_v4())))
    }

    pub(crate) fn new(tag: &str) -> Self {
        let s = Self::path(tag);
        std::fs::create_dir_all(&s.0).unwrap();
        s
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0).or_else(|_| std::fs::remove_file(&self.0));
    }
}
