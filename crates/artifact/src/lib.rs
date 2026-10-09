//! Artifact plugin (design doc 35): files the agent makes for the user, shown
//! in chat. One tool, `show` ([`tools::ShowTool`]); a content-addressed blob
//! store with a sqlite index ([`store::State`]); Typst PDF compile ([`pdf`])
//! and SVG raster ([`svg`]).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`ARTIFACT_ACTOR`] actor ([`ArtifactActor`]): [`Show`],
//!   [`List`], [`Export`], [`search::SearchArtifacts`], [`search::Get`],
//!   [`search::ReadSearchMatch`], [`search::RefreshSearch`]; for the app
//!   plugin (46 §3): [`Commit`], [`GetRecord`], [`ClaimInvocation`],
//!   [`CompleteInvocation`], [`MarkInvocationUnknown`], [`GetInvocation`],
//!   [`ListInvocations`]
//! - events: [`ArtifactShown`]

pub mod archive;
pub mod export;
pub mod inline;
pub mod kind;
pub mod mesh;
pub mod pdf;
pub mod preview;
pub mod search;
pub mod sniff;
pub mod store;
pub mod svg;
pub mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

pub use export::{EXPORT_SESSION, ExportAsset, ExportDoc, ExportFormat, ExportResult};
pub use pdf::{Diag, Source};
pub use store::{
    Claim, CommitError, Invocation, InvocationStatus, MAX_INVOCATION_RESULT, MAX_LIST_INVOCATIONS,
    State, Stores, args_hash,
};
pub use svg::Raster;
pub use tools::ShowArgs;

/// Registry name of the [`ArtifactActor`].
pub const ARTIFACT_ACTOR: &str = "artifact";
/// Largest artifact, any kind (35 §9).
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// `artifact:` config section (35 §10).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactConfig {
    /// `false`: no `show` tool, no prompt block; blobs and index kept.
    pub enabled: bool,
    /// Extra directories searched for PDF fonts, besides system and embedded fonts.
    pub fonts: Vec<PathBuf>,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fonts: Vec::new(),
        }
    }
}

/// How an artifact renders; derived from the extension at show time (35 §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `md`, `markdown`.
    Markdown,
    /// `txt`, `log`, code.
    Text,
    /// `csv`, `tsv`.
    Csv,
    /// `png`, `jpg`, `jpeg`, `gif`, `webp`.
    Image,
    /// `svg`.
    Svg,
    /// `pdf`.
    Pdf,
    /// `html`, `htm`.
    Html,
    /// A validated OpenUI app (`.openui`, 46 §3.2). Never derived from an
    /// extension: only [`Commit`] creates one.
    App,
    /// `zip`, `tar`, `tgz`, `tar.gz` (and `tar.bz2`/`tbz2`): a listing without
    /// extraction as a `<blob>.listing.json` sidecar ([`archive`]).
    Archive,
    /// `mmd`, `mermaid`: a standalone diagram source.
    Mermaid,
    /// Anything else: download only.
    File,
}

impl Kind {
    /// The wire / index spelling (`"markdown"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Csv => "csv",
            Self::Image => "image",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::App => "app",
            Self::Archive => "archive",
            Self::Mermaid => "mermaid",
            Self::File => "file",
        }
    }

    /// Inverse of [`Kind::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Markdown,
            Self::Text,
            Self::Csv,
            Self::Image,
            Self::Svg,
            Self::Pdf,
            Self::Html,
            Self::App,
            Self::Archive,
            Self::Mermaid,
            Self::File,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// One shown file: an immutable snapshot with a name and a version (35 §3.3).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Artifact {
    /// `sha256[..16]` of the bytes, hex; also the blob file stem.
    pub id: String,
    /// Owning session.
    pub session: SessionId,
    /// The `show` tool call this record belongs to.
    pub call_id: String,
    /// `report.pdf`.
    pub name: String,
    /// Card title.
    pub title: String,
    /// Rendering kind.
    pub kind: Kind,
    /// Mime type.
    pub mime: String,
    /// Blob size.
    pub bytes: u64,
    /// Per `(session, name)`, 1-based.
    pub version: u32,
    /// `<data_dir>/artifacts/<id>.<ext>`.
    pub path: PathBuf,
    /// `<data_dir>/artifacts/<sha16>.md|.typ` for compiled PDFs.
    pub source: Option<PathBuf>,
    /// `<data_dir>/artifacts/<id>.preview.png` for compiled PDFs and SVGs.
    pub preview: Option<PathBuf>,
    /// Page count of a compiled PDF.
    pub pages: Option<u32>,
    /// The `path` argument when the model showed an existing file.
    pub origin: Option<PathBuf>,
    /// Unix seconds.
    pub created_at: i64,
}

/// Event: a `show` call completed (35 §3.3). Wire shape `{ "type": "ArtifactShown", … }`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct ArtifactShown {
    /// Owning session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// The record.
    pub artifact: Artifact,
}

/// `ask` → `Result<Artifact, String>`: snapshot, render, record, publish
/// [`ArtifactShown`]. Runs off the mailbox: a compile takes seconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    /// Owning session.
    pub session: SessionId,
    /// Tool call id.
    pub call_id: String,
    /// Tool arguments.
    pub args: ShowArgs,
}

/// `ask` → `Vec<Artifact>`, newest first. `session: Some` → that session;
/// `descendants` widens to its subtree (asked from the agent supervisor).
/// `None` → every session (the explorer).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct List {
    /// Root session; `None` = every session.
    pub session: Option<SessionId>,
    /// Include every session under it.
    pub descendants: bool,
}

/// `ask` → `Result<PathBuf, String>`: copy the blob to `dest` (parent created;
/// refuses to overwrite).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    /// Artifact id.
    pub id: String,
    /// Destination file.
    pub dest: PathBuf,
}

/// `ask` → `Result<Artifact, CommitError>`: publish already-validated bytes as
/// a record of any [`Kind`], including [`Kind::App`], with an optional opaque
/// source blob (46 §3.2–§3.3). The blob and source are written first; the row
/// is inserted with its version in one statement that also checks
/// `expected_base`; [`ArtifactShown`] publishes only after both. Records are
/// immutable: a `call_id` already recorded returns that record when session,
/// name, kind, title, bytes and source sidecar all match, and
/// `CommitError::Other` otherwise. Runs off the mailbox like [`Show`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Commit {
    /// Owning session.
    pub session: SessionId,
    /// Record key: the publishing tool call.
    pub call_id: String,
    /// `panel.openui`; needs an extension.
    pub name: String,
    /// Card title.
    pub title: String,
    /// Rendering kind; the caller vouches for validation.
    pub kind: Kind,
    /// The artifact bytes.
    pub bytes: Vec<u8>,
    /// `(suffix, bytes)` of a sidecar stored as `<sha16(bytes)>.<suffix>`
    /// (its own hash, not the artifact's), e.g. `("app.json", metadata)`.
    pub source: Option<(String, Vec<u8>)>,
    /// Compare-and-swap: insert only while this `call_id` is the latest
    /// version of `(session, name)`; `None` inserts unconditionally.
    pub expected_base: Option<String>,
}

/// `ask` → `Result<Artifact, String>`: the record of one tool call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetRecord {
    /// Record key.
    pub call_id: String,
}

/// `ask` → `Result<Claim, String>`: atomically insert a `running` app
/// invocation keyed by `request_id` (46 §6.4), only while `record_call_id`
/// is a [`Kind::App`] record of `session` (`Err` otherwise). `args` are
/// hashed canonically ([`args_hash`]); a repeat with the same identity and
/// hash returns the stored row, a repeat with different input is a conflict.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimInvocation {
    /// Client-chosen, stable across transport retries of one activation.
    pub request_id: String,
    /// Owning session (purged with it).
    pub session: SessionId,
    /// The app record (`Artifact::call_id`).
    pub record_call_id: String,
    /// The bound Query/Mutation statement.
    pub statement_id: String,
    /// The press this invocation belongs to, if any.
    pub action_id: Option<String>,
    /// Resolved tool name.
    pub tool: String,
    /// Concrete arguments; only their hash is stored.
    pub args: serde_json::Value,
}

/// `ask` → `Result<(), String>`: finish a `running` invocation. `result` is
/// the caller-redacted display value, stored up to [`MAX_INVOCATION_RESULT`]
/// serialized bytes (larger → an `{"omitted": …}` marker). `Err` when the row
/// is missing or no longer running.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompleteInvocation {
    /// Row key.
    pub request_id: String,
    /// `true` → `succeeded`, `false` → `failed`.
    pub ok: bool,
    /// Redacted, bounded result.
    pub result: Option<serde_json::Value>,
}

/// `ask` → `Result<Option<Invocation>, String>`: current status and stored
/// result; `Ok(None)` is "no such request", `Err` an unreadable index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetInvocation {
    /// Row key.
    pub request_id: String,
}

/// `ask` → `Result<(), String>`: a `running` invocation whose dispatch
/// outcome cannot be proven (the registry call failed after the durable
/// claim, the reply never arrived) becomes `unknown`, never `failed`: the
/// tool may have run. `reason` is caller-redacted like
/// [`CompleteInvocation::result`] and stored as `{"unknown": reason}`,
/// bounded the same way. `Err` when the row is missing or no longer running.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkInvocationUnknown {
    /// Row key.
    pub request_id: String,
    /// Redacted, short: why the outcome is unknowable.
    pub reason: Option<String>,
}

/// `ask` → `Result<Vec<Invocation>, String>`: the invocations of one app
/// record, newest first, at most `min(limit, MAX_LIST_INVOCATIONS)`. Rows
/// exist only while the record does (claims require it, session deletion
/// purges both), so reopening an app after a restart can still find its
/// request ids and their final statuses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListInvocations {
    /// The app record.
    pub record_call_id: String,
    /// Newest rows wanted; capped at [`MAX_LIST_INVOCATIONS`].
    pub limit: usize,
}

/// The [`ARTIFACT_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct ArtifactActor {
    stores: Arc<Stores>,
}

impl ArtifactActor {
    /// The actor over `stores`; the caller spawns, registers and subscribes it.
    pub fn new(stores: Arc<Stores>) -> Self {
        Self { stores }
    }
}

/// Id-keyed lookups carry no session: ask the durable store first, then every
/// open private tree; the first answer `found` accepts wins, else the durable
/// one.
async fn any<T, E, Fut>(
    states: Vec<Arc<State>>,
    found: impl Fn(&Result<T, E>) -> bool,
    f: impl Fn(Arc<State>) -> Fut,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
{
    let mut first = None;
    for s in states {
        let r = f(s).await;
        if found(&r) {
            return r;
        }
        first.get_or_insert(r);
    }
    first.expect("the durable store is always present")
}

impl Message<Show> for ArtifactActor {
    type Reply = DelegatedReply<Result<Artifact, String>>;

    async fn handle(&mut self, m: Show, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let state = match self.stores.state_for(m.session) {
            Ok(s) => s,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        tokio::spawn(async move {
            tx.send(
                state
                    .show(m.session, m.call_id, m.args)
                    .await
                    .map(|s| s.artifact),
            );
        });
        reply
    }
}

impl Message<List> for ArtifactActor {
    type Reply = DelegatedReply<Vec<Artifact>>;

    async fn handle(&mut self, m: List, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // `None` is the explorer: durable rows only; a private tree is never listed.
        let state = match m.session.map_or(Ok(self.stores.durable().clone()), |s| {
            self.stores.state_for(s)
        }) {
            Ok(s) => s,
            Err(_) => {
                tx.send(Vec::new());
                return reply;
            }
        };
        // Leaves the mailbox: `descendants` asks the agent supervisor.
        tokio::spawn(async move {
            let list = state
                .list(m.session, m.descendants)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(session = ?m.session, "artifacts unreadable: {e:#}");
                    Vec::new()
                });
            tx.send(list);
        });
        reply
    }
}

impl Message<Export> for ArtifactActor {
    type Reply = Result<PathBuf, String>;

    async fn handle(&mut self, m: Export, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        any(self.stores.all(), Result::is_ok, |s| {
            let (id, dest) = (m.id.clone(), m.dest.clone());
            async move { s.export(&id, &dest).await }
        })
        .await
    }
}

impl Message<ExportDoc> for ArtifactActor {
    type Reply = DelegatedReply<Result<ExportResult, String>>;

    async fn handle(&mut self, m: ExportDoc, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let state = self.stores.durable().clone();
        tokio::spawn(async move { tx.send(state.export_doc(m).await) });
        reply
    }
}

impl Message<Commit> for ArtifactActor {
    type Reply = DelegatedReply<Result<Artifact, CommitError>>;

    async fn handle(&mut self, m: Commit, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let state = match self.stores.state_for(m.session) {
            Ok(s) => s,
            Err(e) => {
                tx.send(Err(CommitError::Other(e)));
                return reply;
            }
        };
        tokio::spawn(async move {
            tx.send(state.commit(m).await);
        });
        reply
    }
}

impl Message<search::SearchArtifacts> for ArtifactActor {
    type Reply = DelegatedReply<Result<::search::SearchPage<search::ArtifactSearchHit>, String>>;

    async fn handle(
        &mut self,
        m: search::SearchArtifacts,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // Durable rows only: a private tree is never indexed (51 §4.5).
        let state = self.stores.durable().clone();
        // Leaves the mailbox: asks the agent for live sessions, matches on the blocking pool.
        tokio::spawn(async move {
            tx.send(state.search_artifacts(m.query, m.limit).await);
        });
        reply
    }
}

impl Message<GetRecord> for ArtifactActor {
    type Reply = Result<Artifact, String>;

    async fn handle(&mut self, m: GetRecord, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        any(self.stores.all(), Result::is_ok, |s| {
            let call_id = m.call_id.clone();
            async move { s.record(&call_id).await }
        })
        .await
    }
}

impl Message<ClaimInvocation> for ArtifactActor {
    type Reply = Result<Claim, String>;

    async fn handle(
        &mut self,
        m: ClaimInvocation,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stores.state_for(m.session)?.claim(m).await
    }
}

impl Message<CompleteInvocation> for ArtifactActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: CompleteInvocation,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        any(self.stores.all(), Result::is_ok, |s| {
            let m = m.clone();
            async move { s.complete(m).await }
        })
        .await
    }
}

impl Message<GetInvocation> for ArtifactActor {
    type Reply = Result<Option<Invocation>, String>;

    async fn handle(
        &mut self,
        m: GetInvocation,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        any(
            self.stores.all(),
            |r| matches!(r, Ok(Some(_))),
            |s| {
                let request_id = m.request_id.clone();
                async move { s.invocation(&request_id).await }
            },
        )
        .await
    }
}

impl Message<MarkInvocationUnknown> for ArtifactActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: MarkInvocationUnknown,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        any(self.stores.all(), Result::is_ok, |s| {
            let m = m.clone();
            async move { s.mark_unknown(m).await }
        })
        .await
    }
}

impl Message<ListInvocations> for ArtifactActor {
    type Reply = Result<Vec<Invocation>, String>;

    async fn handle(
        &mut self,
        m: ListInvocations,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        any(
            self.stores.all(),
            |r: &Result<Vec<Invocation>, String>| matches!(r, Ok(v) if !v.is_empty()),
            |s| {
                let record = m.record_call_id.clone();
                async move { s.list_invocations(&record, m.limit).await }
            },
        )
        .await
    }
}

impl Message<search::Get> for ArtifactActor {
    type Reply = DelegatedReply<Result<Artifact, String>>;

    async fn handle(
        &mut self,
        m: search::Get,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let states = self.stores.all();
        // Leaves the mailbox: asks the agent whether the owning session exists.
        tokio::spawn(async move {
            tx.send(
                any(states, Result::is_ok, |s| {
                    let call_id = m.call_id.clone();
                    async move { s.get(&call_id).await }
                })
                .await,
            );
        });
        reply
    }
}

impl Message<search::ReadSearchMatch> for ArtifactActor {
    type Reply = DelegatedReply<Result<search::ArtifactMatchContext, String>>;

    async fn handle(
        &mut self,
        m: search::ReadSearchMatch,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let states = self.stores.all();
        tokio::spawn(async move {
            tx.send(
                any(states, Result::is_ok, |s| {
                    let (call_id, query) = (m.call_id.clone(), m.query.clone());
                    async move { s.read_search_match(call_id, query).await }
                })
                .await,
            );
        });
        reply
    }
}

impl Message<search::RefreshSearch> for ArtifactActor {
    type Reply = ();

    async fn handle(&mut self, _: search::RefreshSearch, _: &mut Context<Self, Self::Reply>) {
        for s in self.stores.all() {
            s.refresh_search();
        }
    }
}

impl Message<agent::AgentEvent> for ArtifactActor {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        let agent::AgentEvent::SessionDeleted { session } = ev else {
            return;
        };
        // A member of a closed private tree has nothing left; `state_for`
        // says so. Unknown ids are durable no-ops: the delete matches no rows.
        let Ok(state) = self.stores.state_for(session) else {
            return;
        };
        if let Err(e) = state.delete_session(session).await {
            tracing::warn!(%session, "artifact rows not deleted: {e:#}");
        }
    }
}

impl Message<document::Exported> for ArtifactActor {
    type Reply = ();

    /// 59 §7.2: a settled user edit of a markdown artifact's document — the
    /// export landed — becomes the next version of its `(session, name)`
    /// lineage and an `artifact-v<N>` snapshot. Off the mailbox: a blob write
    /// and a helper snapshot.
    async fn handle(&mut self, m: document::Exported, _: &mut Context<Self, Self::Reply>) {
        let document::Binding::Artifact { session, name } = m.binding else {
            return;
        };
        let Ok(session) = session.parse::<SessionId>() else {
            return;
        };
        let state = self.stores.durable().clone();
        tokio::spawn(async move {
            let bytes = match tokio::fs::read(&m.path).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(%name, "artifact export unreadable: {e}");
                    return;
                }
            };
            match state.edit_version(session, &name, bytes).await {
                Ok(Some(a)) => {
                    if let Some(docs) = document::Handle::lookup()
                        && let Err(e) = docs
                            .actor()
                            .ask(document::Snapshot {
                                doc: m.doc,
                                label: format!("artifact-v{}", a.version),
                            })
                            .await
                    {
                        tracing::warn!(%name, "artifact edit snapshot: {e}");
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(%name, "artifact edit version: {e}"),
            }
        });
    }
}

impl Message<::tools::TreeOpened> for ArtifactActor {
    type Reply = ();

    async fn handle(&mut self, m: ::tools::TreeOpened, _: &mut Context<Self, Self::Reply>) {
        // In the mailbox on purpose: the next message for a member must find
        // its store. Migrating an empty in-memory index takes milliseconds.
        if let Err(e) = self
            .stores
            .open_tree(m.root, &m.sessions, &m.resources)
            .await
        {
            tracing::error!(root = %m.root, "private artifact store not opened: {e:#}");
        }
    }
}

impl Message<::tools::CloseTree> for ArtifactActor {
    type Reply = DelegatedReply<::tools::CloseAck>;

    async fn handle(
        &mut self,
        m: ::tools::CloseTree,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let stores = self.stores.clone();
        // Leaves the mailbox: waits for admitted writers (a PDF compile takes
        // seconds) and removes the blobs. Members are unmapped synchronously
        // inside `close_tree` before the first await, so nothing new is admitted.
        tokio::spawn(async move {
            let (ok, detail) = match stores.close_tree(&m.subtree).await {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            };
            tx.send(::tools::CloseAck {
                owner: "artifact".into(),
                ok,
                detail,
            });
        });
        reply
    }
}

/// The plugin (35 §0.10): reads `artifact`, opens the store, spawns
/// [`ArtifactActor`] as [`ARTIFACT_ACTOR`], subscribes it to
/// [`agent::AgentEvent`], sweeps orphan blobs, and registers `show`, the
/// prompt block and the private-tree lifecycle (51 §4.5) once the registry
/// is up.
pub struct ArtifactPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ArtifactPlugin {
    fn name(&self) -> &'static str {
        "artifact"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: ArtifactConfig = rt.section("artifact").unwrap_or_else(|e| {
            tracing::debug!("no [artifact] config section, using defaults: {e}");
            ArtifactConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("artifact disabled by config");
            return Ok(());
        }
        let state = match State::open(&rt.data_dir, rt.clone(), cfg.fonts).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!("artifact unavailable: {e:#}");
                return Ok(());
            }
        };
        let stores = Arc::new(Stores::new(state.clone()));
        let actor = ArtifactActor::spawn_with_mailbox(
            ArtifactActor {
                stores: stores.clone(),
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(ARTIFACT_ACTOR)?;
        rt.subscribe::<ArtifactActor, agent::AgentEvent>(&actor)
            .await;
        rt.subscribe::<ArtifactActor, document::Exported>(&actor)
            .await;
        // 68 §7: blobs and spill files for replicas, on `theuth/artifacts/1`.
        tokio::spawn(mesh::serve(rt.clone(), stores.clone()));

        tokio::spawn(async move {
            match state.sweep().await {
                Ok(0) => {}
                Ok(n) => tracing::info!("artifact sweep removed {n} orphan blobs"),
                Err(e) => tracing::warn!("artifact sweep failed: {e:#}"),
            }
        });

        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::ReadTool {
                        stores: stores.clone(),
                    })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::DocExportTool {
                        state: stores.durable().clone(),
                    })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::ShowTool {
                        stores: stores.clone(),
                    })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(preview::PagePreviewTool {
                        rt: rt.clone(),
                        stores,
                    })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterPrompt {
                        plugin: "artifact",
                        text: tools::PROMPT.to_owned(),
                    })
                    .await;
                let _ = reg
                    .tell(::tools::RegisterLifecycle {
                        owner: "artifact",
                        opened: Some(actor.clone().recipient()),
                        close: actor.reply_recipient(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}
