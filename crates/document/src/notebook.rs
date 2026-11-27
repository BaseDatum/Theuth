//! Notebooks (design 144 §3, §13.1): the frozen wire shapes of the container, its membership rows,
//! sources, the context dial, citations and the source event — ts-rs exported like `DocKind` — plus
//! the actor messages the `notebook_*` / `cite_resolve` RPCs and tools send the [`DocumentActor`].
//!
//! Shapes here are the §13 contract: four workers code against them; a change is a design amendment,
//! never a local edit.

use std::collections::HashMap;

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use serde::{Deserialize, Serialize};
use surface::anchor::{Locator, TheuthAnchor, TheuthKind};

#[cfg(not(target_os = "ios"))]
pub use codeindex::LocateStatus;

/// How an anchor relates to the working tree or block text now.
#[cfg(target_os = "ios")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum LocateStatus {
    /// Target exists and its text hashes to `text_sha` (or none was given).
    Same,
    /// Target exists and its text differs.
    Changed,
    /// Target moved to another position.
    Moved,
    /// Target is gone.
    Missing,
}

use crate::actor::{DocumentActor, ImportReply};
use crate::meta::{Binding, DocEvent, DocId, DocKind, DocSummary, DocumentChanged, Principal};
use crate::store::{NotebookRow, now_ms};

/// What a `notebook_items` row points at (144 §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    /// A `DocKind::Source` document.
    Source,
    /// A `Note | Board | MeetingNote` document.
    Note,
    /// A `SessionId` (owner-only; never covered by the derived share grant, D13).
    Session,
    /// An artifact `call_id` (overviews, §10).
    Artifact,
}

impl ItemKind {
    /// The `kind` column text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Note => "note",
            Self::Session => "session",
            Self::Artifact => "artifact",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "source" => Some(Self::Source),
            "note" => Some(Self::Note),
            "session" => Some(Self::Session),
            "artifact" => Some(Self::Artifact),
            _ => None,
        }
    }
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One membership row (144 §3.3): the row id is the deterministic text triple `<notebook>:<kind>:<item>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NotebookItem {
    /// The notebook.
    pub notebook: DocId,
    /// What `item` is.
    pub kind: ItemKind,
    /// The item id: a `DocId` for sources and notes, a `SessionId`, an artifact `call_id`.
    pub item: String,
    /// When it was added, unix ms.
    pub added_ms: i64,
    /// Who added it: `owner` | `agent:<session>` | `member:<pub8>`.
    pub added_by: String,
    /// Soft removal, unix ms; `None` while the item is in the notebook. "Remove then add again" flips it back.
    pub removed_ms: Option<i64>,
}

impl NotebookItem {
    /// The deterministic row id `<notebook>:<kind>:<item>` (§3.4: no clock needed).
    pub fn row_id(notebook: &DocId, kind: ItemKind, item: &str) -> String {
        format!("{notebook}:{kind}:{item}")
    }
}

/// One notebook as `notebook_list` / `notebook_get` show it (144 §13.1).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Notebook {
    /// The `DocKind::Notebook` document: its title is the name, its body the description.
    pub doc: DocSummary,
    /// Optional link to the `project` label tasks and sessions carry.
    pub project: Option<String>,
    /// Archived, unix ms; `None` while live (an archived notebook leaves the default list).
    pub archived_ms: Option<i64>,
    /// Live `source` items.
    pub sources: u32,
    /// Live `note` items.
    pub notes: u32,
    /// Sessions bound to this notebook (`sessions.notebook`, §8).
    pub chats: u32,
}

/// The MCP page a source came from (144 §4.1): which skin the card wears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum McpLabel {
    /// A Confluence page.
    Confluence,
    /// A Jira issue.
    Jira,
    /// Any other MCP tool result.
    Other,
}

/// How a YouTube source was (or is to be) extracted (144 §4.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum YoutubeRoute {
    /// The video's captions.
    Captions,
    /// `yt-dlp` audio → local STT.
    Audio,
}

/// Where a source came from (144 §4.1); stored as JSON in `sources.origin`, shown verbatim on the source page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceOrigin {
    /// A file from disk or an upload (`file:<sha256>`).
    File {
        /// Original file name.
        name: String,
        /// sha256 of the bytes, lowercase hex.
        sha256: String,
        /// Extension without the dot, lowercase.
        ext: String,
    },
    /// A web page (`url:<canonical>`).
    Url {
        /// As given.
        url: String,
        /// After canonicalisation (122's rules); `None` until fetched.
        canonical: Option<String>,
    },
    /// A meeting's transcript (`meeting:<id>`).
    Meeting {
        /// The meeting id.
        meeting: String,
    },
    /// Pasted text (`text:<sha256>`).
    Text {
        /// The title the user gave, or the first line.
        title: String,
        /// sha256 of the text, lowercase hex.
        sha256: String,
    },
    /// A past chat (`session:<id>`).
    Session {
        /// The session id.
        session: String,
    },
    /// An MCP tool result (`mcp:<server>/<tool>/<sha256(args)>`).
    Mcp {
        /// MCP server name.
        server: String,
        /// Tool name.
        tool: String,
        /// The call's arguments.
        args: serde_json::Value,
        /// Which skin: Confluence, Jira or other.
        label: McpLabel,
    },
    /// A YouTube video (`youtube:<video id>`).
    Youtube {
        /// The video id.
        video: String,
        /// The URL as given.
        url: String,
        /// Captions or audio.
        route: YoutubeRoute,
    },
    /// An audio file (`file:<sha256>`, transcribed).
    Audio {
        /// Original file name.
        name: String,
        /// sha256 of the bytes, lowercase hex.
        sha256: String,
        /// Extension without the dot, lowercase.
        ext: String,
    },
    /// A video file (`file:<sha256>`, audio track transcribed).
    Video {
        /// Original file name.
        name: String,
        /// sha256 of the bytes, lowercase hex.
        sha256: String,
        /// Extension without the dot, lowercase.
        ext: String,
    },
}

impl SourceOrigin {
    /// The origin key (144 §4.1) — the `Binding::Source` string every engine mints the same id from.
    pub fn key(&self) -> String {
        match self {
            Self::File { sha256, .. } | Self::Audio { sha256, .. } | Self::Video { sha256, .. } => {
                format!("file:{sha256}")
            }
            Self::Url { url, canonical } => format!("url:{}", canonical.as_deref().unwrap_or(url)),
            Self::Meeting { meeting } => format!("meeting:{meeting}"),
            Self::Text { sha256, .. } => format!("text:{sha256}"),
            Self::Session { session } => format!("session:{session}"),
            Self::Mcp {
                server, tool, args, ..
            } => format!(
                "mcp:{server}/{tool}/{}",
                crate::store::sha256(args.to_string().as_bytes())
            ),
            Self::Youtube { video, .. } => format!("youtube:{video}"),
        }
    }
}

/// The ingest job's state (144 §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    /// Row written, card exists; not yet queued.
    New,
    /// Waiting for a job slot.
    Queued,
    /// Extracting; `Progress` events while it runs.
    Running,
    /// Text imported, indexed, default insight written.
    Completed,
    /// `error` says why; `415 unsupported`, `too large`, `refused` are permanent.
    Failed,
}

impl SourceStatus {
    /// The `status` column text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "new" => Some(Self::New),
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl std::fmt::Display for SourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One source as `source_list` / `source_get` show it (144 §13.1): the document plus the `sources` row.
/// TS name `NotebookSource`: `knowledge::Source` already owns `Source.ts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "NotebookSource")
)]
pub struct Source {
    /// The `DocKind::Source` document.
    pub doc: DocSummary,
    /// Where it came from.
    pub origin: SourceOrigin,
    /// The ingest job's state.
    pub status: SourceStatus,
    /// 0–100 while `running` (STT, captions); `None` otherwise.
    pub progress: Option<u8>,
    /// Why it failed, when it did.
    pub error: Option<String>,
    /// Ingest attempts so far (`source_refresh` increments).
    pub attempts: u32,
    /// Last successful fetch, unix ms.
    pub fetched_ms: Option<i64>,
    /// sha256 of the derived markdown at the last success.
    pub content_sha: Option<String>,
    /// The previous `fetched_ms` when a refresh changed the text and the source has not been opened since.
    pub changed_since: Option<i64>,
    /// Original size in bytes.
    pub bytes: Option<u64>,
    /// Word count of the derived markdown.
    pub words: Option<u32>,
    /// Media length (YouTube, audio, video), ms.
    pub duration_ms: Option<u64>,
    /// Page count (PDF, documents).
    pub pages: Option<u32>,
    /// Insight sections written at the end of the document.
    pub insights: u32,
    /// Extractor metadata (author, published date, page title, …), shown verbatim.
    pub meta: serde_json::Value,
}

/// The context dial (144 §5.1): how much of an item the model sees in a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ContextLevel {
    /// Not in the context at all.
    Off,
    /// Title and id only.
    Mention,
    /// The default insight.
    Summary,
    /// The whole text.
    Full,
}

/// What became of one item in the estimate (144 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ContextStatus {
    /// Injected in full at its level.
    Available,
    /// Cut to fit the budget, with a notice.
    Truncated,
    /// Left out: the budget was spent.
    Omitted,
}

/// One item of the pre-turn estimate (144 §5.2, wave-2 contract §1). TS name `NotebookContextItem`:
/// `agent::ContextItem` already owns `ContextItem.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "NotebookContextItem")
)]
pub struct ContextItem {
    /// The item id.
    pub id: String,
    /// What `id` is.
    pub kind: ItemKind,
    /// Display title (a source's card title, a note's first heading, a chat's title).
    pub title: String,
    /// The effective level used at the next turn.
    pub level: ContextLevel,
    /// `true` when the user/agent set it (a `session_context` row); `false` = the default rule.
    pub pinned: bool,
    /// Tokens this item contributes at `level` (chars / 4 on the hub).
    pub tokens: u32,
    /// What the first injection's capacity did to it (157 D4: the thread's own context, not a budget).
    pub status: ContextStatus,
}

/// The pre-turn estimate (144 §5.2, wave-2 contract §1; 157 D9: no budget — the first injection is sized by
/// the thread's context, then the block is never repeated).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextEstimate {
    /// Every item of the notebook with its level and fate, in injection order (full, summary, mention, off;
    /// newest first within a level).
    pub items: Vec<ContextItem>,
    /// Tokens the injection costs (at the dial levels once the block is in the chat).
    pub total_tokens: u32,
    /// Items left out: `off`, or beyond what the first injection could carry.
    pub omitted: u32,
    /// Every item `off` (or no items): the block carries the "nothing in context" sentence.
    pub empty: bool,
    /// The chat's current epoch already carries the `<notebook>` block (or its post-compaction manifest).
    pub injected: bool,
    /// Change lines the next turn's `<notebook_changes>` message would carry; 0 before the block.
    pub pending_changes: u32,
}

/// What `insight_run` should produce (144 §4.3): exactly one of a builtin (`key_insights` | `toc` |
/// `open_questions`) or a skill whose front matter has `notebook_insight: true`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InsightInstruction {
    /// A builtin's name.
    #[serde(default)]
    pub builtin: Option<String>,
    /// A skill's name.
    #[serde(default)]
    pub skill: Option<String>,
}

/// `insight_run` reply: the first block id of the appended `###` section.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InsightRun {
    /// The `### <name> · <model> · <date>` heading block.
    pub block: String,
}

/// The target kind of a `theuth://` citation (144 §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CiteKind {
    /// `theuth://notes/<id>#<block>`.
    Notes,
    /// `theuth://sources/<id>#<block>` or `@mm:ss`.
    Sources,
    /// `theuth://knowledge/<id>[@<commit>]`.
    Knowledge,
    /// `theuth://meetings/<id>@mm:ss`.
    Meetings,
    /// `theuth://sessions/<id>#<turn>`.
    Sessions,
}

/// One resolved citation (144 §6.4): what `cite_resolve` answers per href.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Citation {
    /// The href as given.
    pub href: String,
    /// Target kind.
    pub kind: CiteKind,
    /// Target id (document, concept, meeting or session id).
    pub id: String,
    /// `#<block>`, `@mm:ss` or `#<turn>` as written; `None` for a whole-document citation.
    pub locator: Option<String>,
    /// The `?s=` drift hash, when given.
    pub sha8: Option<String>,
    /// `same` | `changed` | `moved` | `missing` (`codeindex::LocateStatus` reused).
    pub status: LocateStatus,
    /// `false` when the document is trashed or not readable by the viewer (one "unavailable" card).
    pub available: bool,
    /// The target's title now.
    pub title: String,
    /// The block's text now, when the locator is a block that exists.
    pub text: Option<String>,
}

/// Bus event (144 §4.2, `subscribe_events`): a source's status or progress changed. Wire shape
/// `{ "type": "Changed", … }` / `{ "type": "Progress", … }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SourceEvent {
    /// The ingest job moved to `status`.
    Changed {
        /// The source document.
        source: DocId,
        /// Its state now.
        status: SourceStatus,
    },
    /// STT or captions progress while `running`.
    Progress {
        /// The source document.
        source: DocId,
        /// 0–100.
        percent: u8,
    },
}

impl wire::WireEvent for SourceEvent {
    const TOPIC: &'static str = "document.sources";
    const VERSION: u32 = 1;
}

/// `notebook_get` reply: the notebook and its live items.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NotebookDetail {
    /// The notebook with counts.
    pub notebook: Notebook,
    /// Items with `removed_ms = NULL`, oldest first.
    pub items: Vec<NotebookItem>,
}

/// A source the notebook being deleted shares with other live notebooks (144 §3.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SharedSource {
    /// The source document.
    pub source: DocId,
    /// Its title.
    pub title: String,
    /// How many other live notebooks hold it ("also in N other notebooks").
    pub also_in: u32,
}

/// `notebook_delete_preview` reply (144 §3.4).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DeletePreview {
    /// Note items (never deleted with the notebook).
    pub notes: u32,
    /// Sources no other live notebook references — deleted when `delete_exclusive_sources`.
    pub sources_exclusive: u32,
    /// Sources also in other notebooks — kept.
    pub sources_shared_elsewhere: Vec<SharedSource>,
}

/// `notebook_item_remove` reply: the row is soft-removed; `also_in` other live notebooks still hold the item.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ItemRemoved {
    /// Other live notebooks holding the same item.
    pub also_in: u32,
}

// ---- Actor messages (144-C) ----------------------------------------------------------------------------

/// Reply type every handler below uses; `NotHub:<id>` for hub-only writes off the hub (69 Y).
type Res<T> = Result<T, String>;

/// `sha256(text.trim_end())`, first 8 lowercase hex — the `?s=` drift hash of a cited block (144 §6.4).
pub fn sha8(text: &str) -> String {
    let mut s = platform::sha256::hex(text.trim_end().as_bytes());
    s.truncate(8);
    s
}

/// The `added_by` column text for a principal (§3.3): `owner` | `agent:<session>` | `member:<pub8>`.
fn added_by(by: &Principal) -> String {
    match by {
        Principal::Owner { .. } | Principal::Helper | Principal::File => "owner".to_owned(),
        Principal::Agent { session } => format!("agent:{session}"),
        Principal::Member { pub_, .. } => format!("member:{}", &pub_[..pub_.len().min(8)]),
    }
}

/// `Note` items are `Note | Board | MeetingNote` documents (§3.3).
fn note_like(kind: DocKind) -> bool {
    matches!(kind, DocKind::Note | DocKind::Board | DocKind::MeetingNote)
}

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

impl DocumentActor {
    /// The live `DocKind::Notebook` summary of `id`, or why not.
    async fn notebook_summary(&self, id: &DocId) -> Res<DocSummary> {
        match self.store.summary(id).await.map_err(err)? {
            Some(s) if s.kind == DocKind::Notebook => Ok(s),
            Some(_) => Err("not_a_notebook".to_owned()),
            None => Err("notebook_not_found".to_owned()),
        }
    }

    async fn notebook_view(
        &self,
        doc: DocSummary,
        counts: &HashMap<String, (u32, u32)>,
    ) -> Notebook {
        let row = self.store.notebook_row(&doc.id).await.ok().flatten();
        let (sources, notes) = counts.get(doc.id.as_str()).copied().unwrap_or_default();
        Notebook {
            project: row.as_ref().and_then(|r| r.project.clone()),
            archived_ms: row.and_then(|r| r.archived_ms),
            sources,
            notes,
            // `sessions.notebook` is the agent crate's (W3); the RPC layer fills the count when it lands.
            chats: 0,
            doc,
        }
    }

    /// The notebook's own `DocumentChanged`: the list and the page refetch on it.
    async fn notebook_changed(&mut self, id: &DocId) {
        let meta = match self.open.get(id) {
            Some(o) => o.meta.clone(),
            None => match self.store.read_meta(id) {
                Ok(m) => m,
                Err(_) => return,
            },
        };
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
    }

    /// Other live notebooks holding `(kind, item)` than `except`.
    async fn also_in(&self, kind: ItemKind, item: &str, except: &DocId) -> Res<u32> {
        let nbs = self
            .store
            .notebooks_of(kind, item, true)
            .await
            .map_err(err)?;
        Ok(nbs.iter().filter(|n| *n != except).count() as u32)
    }
}

/// `ask` → `Vec<Notebook>`: live notebooks, newest update first; `archived` includes archived ones.
/// Served on any engine from the replica.
#[derive(Clone, Debug, Default)]
pub struct NotebookList {
    /// Include archived notebooks.
    pub archived: bool,
}

impl Message<NotebookList> for DocumentActor {
    type Reply = Res<Vec<Notebook>>;

    async fn handle(&mut self, m: NotebookList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rows = self.store.notebook_rows(m.archived).await.map_err(err)?;
        let counts = self.store.item_counts().await.map_err(err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(doc) = self.store.summary(&row.id).await.map_err(err)? {
                out.push(self.notebook_view(doc, &counts).await);
            }
        }
        Ok(out)
    }
}

/// `ask` → [`NotebookDetail`]. Any engine; a member must be a member of the notebook (§11).
#[derive(Clone, Debug)]
pub struct NotebookGet {
    /// The notebook.
    pub id: DocId,
    /// Who asks.
    pub by: Principal,
}

impl Message<NotebookGet> for DocumentActor {
    type Reply = Res<NotebookDetail>;

    async fn handle(&mut self, m: NotebookGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let doc = self.notebook_summary(&m.id).await?;
        self.admit(&m.id, &m.by, false).await?;
        let counts = self.store.item_counts().await.map_err(err)?;
        let notebook = self.notebook_view(doc, &counts).await;
        let items = self.store.items(&m.id).await.map_err(err)?;
        Ok(NotebookDetail { notebook, items })
    }
}

/// `ask` → [`Notebook`]: a `DocKind::Notebook` document headed by `title` with `description` as its body,
/// plus the `notebooks` row. Hub only.
#[derive(Clone, Debug)]
pub struct NotebookCreate {
    /// The name.
    pub title: String,
    /// The description body, markdown.
    pub description: String,
    /// Optional `project` label.
    pub project: Option<String>,
    /// Who creates it.
    pub by: Principal,
}

impl Message<NotebookCreate> for DocumentActor {
    type Reply = DelegatedReply<Res<Notebook>>;

    async fn handle(
        &mut self,
        m: NotebookCreate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 184 §5.2: notebooks are hub-served; off the holder `Cmd::Create` asks it.
        match self.route_home(None) {
            Ok(None) => {}
            Ok(Some(to)) => {
                let req = crate::relay::CreateReq::Notebook {
                    title: m.title,
                    description: m.description,
                    project: m.project,
                    by: m.by.clone(),
                };
                let client = crate::relay::Client::of(&m.by);
                let ask = self.create_on::<Notebook>(to, DocId::new(), client, req, None);
                tokio::spawn(async move { tx.send(ask.await) });
                return reply;
            }
            Err(e) => {
                tx.send(Err(e.into()));
                return reply;
            }
        }
        let project = m.project.clone();
        let id = self
            .create_doc(
                Binding::Notebook,
                None,
                m.title,
                m.description,
                m.by,
                None,
                move |doc| {
                    ImportReply::CreatedNotebook(
                        Notebook {
                            doc,
                            project,
                            archived_ms: None,
                            sources: 0,
                            notes: 0,
                            chats: 0,
                        },
                        tx,
                    )
                },
            )
            .await;
        let row = NotebookRow {
            id,
            project: m.project,
            archived_ms: None,
        };
        if let Err(e) = self.store.notebook_upsert(&row).await {
            tracing::warn!(doc = %row.id, error = %e, "notebooks row not written");
        }
        reply
    }
}

/// `ask` → `()`: rename, re-project or (un)archive. Hub only.
#[derive(Clone, Debug)]
pub struct NotebookUpdate {
    /// The notebook.
    pub id: DocId,
    /// New name.
    pub title: Option<String>,
    /// New project (`Some(None)` clears it).
    pub project: Option<Option<String>>,
    /// Archive (`true`) or restore (`false`).
    pub archived: Option<bool>,
    /// Who updates it.
    pub by: Principal,
}

impl Message<NotebookUpdate> for DocumentActor {
    type Reply = Res<()>;

    async fn handle(
        &mut self,
        m: NotebookUpdate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.hub.gate_str()?;
        self.notebook_summary(&m.id).await?;
        if m.project.is_some() || m.archived.is_some() {
            let mut row = self
                .store
                .notebook_row(&m.id)
                .await
                .map_err(err)?
                .unwrap_or(NotebookRow {
                    id: m.id.clone(),
                    project: None,
                    archived_ms: None,
                });
            if let Some(p) = m.project {
                row.project = p;
            }
            if let Some(a) = m.archived {
                row.archived_ms = a.then(now_ms);
            }
            self.store.notebook_upsert(&row).await.map_err(err)?;
        }
        if let Some(title) = m
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        {
            let meta = {
                let o = self.load(&m.id)?;
                o.meta.title = title.clone();
                o.meta.clone()
            };
            self.store.write_meta(&meta).await.map_err(err)?;
            self.fan_out(
                &m.id,
                DocEvent::Meta {
                    title: title.clone(),
                },
                None,
            );
            let me = ctx.actor_ref().clone();
            let doc = m.id.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::actor::retitle_heading(me, doc.clone(), title).await {
                    tracing::debug!(doc = %doc, error = %e, "notebook heading not retitled");
                }
            });
        }
        self.notebook_changed(&m.id).await;
        Ok(())
    }
}

/// `ask` → [`DeletePreview`]. Any engine.
#[derive(Clone, Debug)]
pub struct NotebookDeletePreview {
    /// The notebook.
    pub id: DocId,
}

impl Message<NotebookDeletePreview> for DocumentActor {
    type Reply = Res<DeletePreview>;

    async fn handle(
        &mut self,
        m: NotebookDeletePreview,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.notebook_summary(&m.id).await?;
        let mut preview = DeletePreview {
            notes: 0,
            sources_exclusive: 0,
            sources_shared_elsewhere: Vec::new(),
        };
        for item in self.store.items(&m.id).await.map_err(err)? {
            match item.kind {
                ItemKind::Note => preview.notes += 1,
                ItemKind::Source => {
                    let also_in = self.also_in(ItemKind::Source, &item.item, &m.id).await?;
                    if also_in == 0 {
                        preview.sources_exclusive += 1;
                    } else if let Some(source) = DocId::parse(&item.item) {
                        let title = self
                            .store
                            .summary(&source)
                            .await
                            .map_err(err)?
                            .map(|s| s.title)
                            .unwrap_or_default();
                        preview.sources_shared_elsewhere.push(SharedSource {
                            source,
                            title,
                            also_in,
                        });
                    }
                }
                ItemKind::Session | ItemKind::Artifact => {}
            }
        }
        Ok(preview)
    }
}

/// `ask` → `()`: trash the notebook document, soft-remove its items and, when asked, the sources no other
/// live notebook references (144 §3.4). Notes are never deleted. Hub only.
#[derive(Clone, Debug)]
pub struct NotebookDelete {
    /// The notebook.
    pub id: DocId,
    /// Also trash sources exclusive to this notebook.
    pub delete_exclusive_sources: bool,
    /// Who deletes it.
    pub by: Principal,
}

impl Message<NotebookDelete> for DocumentActor {
    type Reply = Res<()>;

    async fn handle(
        &mut self,
        m: NotebookDelete,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.hub.gate_str()?;
        self.notebook_summary(&m.id).await?;
        let removed = self
            .store
            .items_remove_all(&m.id, now_ms())
            .await
            .map_err(err)?;
        for item in removed {
            let Some(doc) = DocId::parse(&item.item) else {
                continue;
            };
            match item.kind {
                ItemKind::Note => self.revoke_derived(&doc, "revoked").await,
                ItemKind::Source => {
                    if m.delete_exclusive_sources
                        && self.also_in(ItemKind::Source, &item.item, &m.id).await? == 0
                    {
                        self.delete_doc(&doc).await?;
                    } else {
                        self.revoke_derived(&doc, "revoked").await;
                    }
                }
                ItemKind::Session | ItemKind::Artifact => {}
            }
        }
        self.delete_doc(&m.id).await
    }
}

/// `ask` → `Vec<NotebookItem>`: live items, oldest first. Any engine; members of the notebook only.
#[derive(Clone, Debug)]
pub struct NotebookItemsList {
    /// The notebook.
    pub notebook: DocId,
    /// Who asks.
    pub by: Principal,
}

impl Message<NotebookItemsList> for DocumentActor {
    type Reply = Res<Vec<NotebookItem>>;

    async fn handle(
        &mut self,
        m: NotebookItemsList,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.notebook_summary(&m.notebook).await?;
        self.admit(&m.notebook, &m.by, false).await?;
        self.store.items(&m.notebook).await.map_err(err)
    }
}

/// `ask` → [`NotebookItem`]: one row, revived when it was removed. Hub only.
#[derive(Clone, Debug)]
pub struct NotebookItemAdd {
    /// The notebook.
    pub notebook: DocId,
    /// What `item` is.
    pub kind: ItemKind,
    /// The item id.
    pub item: String,
    /// Who adds it (becomes `added_by`).
    pub by: Principal,
}

impl Message<NotebookItemAdd> for DocumentActor {
    type Reply = Res<NotebookItem>;

    async fn handle(
        &mut self,
        m: NotebookItemAdd,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.hub.gate_str()?;
        self.notebook_summary(&m.notebook).await?;
        let item = m.item.trim().to_owned();
        if item.is_empty() {
            return Err("item must not be empty".to_owned());
        }
        if matches!(m.kind, ItemKind::Note | ItemKind::Source) {
            let doc =
                DocId::parse(&item).ok_or_else(|| format!("`{item}` is not a document id"))?;
            let kind = self
                .store
                .summary(&doc)
                .await
                .map_err(err)?
                .ok_or_else(|| "item_not_found".to_owned())?
                .kind;
            match (m.kind, kind) {
                (ItemKind::Source, DocKind::Source) => {}
                (ItemKind::Note, k) if note_like(k) => {}
                (ItemKind::Source, _) => return Err("not_a_source".to_owned()),
                _ => return Err("not_a_note".to_owned()),
            }
        }
        let row = NotebookItem {
            notebook: m.notebook.clone(),
            kind: m.kind,
            item,
            added_ms: now_ms(),
            added_by: added_by(&m.by),
            removed_ms: None,
        };
        self.store.item_add(&row).await.map_err(err)?;
        self.notebook_changed(&m.notebook).await;
        // The stored row: a revived one keeps its first `added_ms`/`added_by`.
        let stored = self
            .store
            .items(&m.notebook)
            .await
            .map_err(err)?
            .into_iter()
            .find(|i| i.kind == row.kind && i.item == row.item);
        Ok(stored.unwrap_or(row))
    }
}

/// `ask` → [`ItemRemoved`]: soft removal (`removed_ms`); members whose grant came only through this
/// notebook get `Closed{revoked}` on the item (§11). Hub only.
#[derive(Clone, Debug)]
pub struct NotebookItemRemove {
    /// The notebook.
    pub notebook: DocId,
    /// What `item` is.
    pub kind: ItemKind,
    /// The item id.
    pub item: String,
    /// Who removes it.
    pub by: Principal,
}

impl Message<NotebookItemRemove> for DocumentActor {
    type Reply = Res<ItemRemoved>;

    async fn handle(
        &mut self,
        m: NotebookItemRemove,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.hub.gate_str()?;
        self.notebook_summary(&m.notebook).await?;
        let removed = self
            .store
            .item_remove(&m.notebook, m.kind, &m.item, now_ms())
            .await
            .map_err(err)?;
        if !removed {
            return Err("not_in_notebook".to_owned());
        }
        if matches!(m.kind, ItemKind::Note | ItemKind::Source)
            && let Some(doc) = DocId::parse(&m.item)
        {
            self.revoke_derived(&doc, "revoked").await;
        }
        self.notebook_changed(&m.notebook).await;
        Ok(ItemRemoved {
            also_in: self.also_in(m.kind, &m.item, &m.notebook).await?,
        })
    }
}

/// `ask` → `bool`: whether `by` may read `doc` — owner always; a member directly or through a notebook
/// (§11). The RPC layer gates the read-only guest methods whose bodies live elsewhere (`source_get`).
#[derive(Clone, Debug)]
pub struct MayRead {
    /// Document.
    pub doc: DocId,
    /// Who asks.
    pub by: Principal,
}

impl Message<MayRead> for DocumentActor {
    type Reply = DelegatedReply<Res<()>>;

    async fn handle(&mut self, m: MayRead, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let local = self.admit(&m.doc, &m.by, false).await;
        // 184 §7.3, Z184-R2: when the document is served elsewhere the server decides a member's
        // read, even where this replica says yes (a revoke there may not have reached it); a guest
        // admitted there seconds ago is not in this replica yet either. Only a local `not_member`
        // is asked again: this engine's `sharing_disabled` or `viewer_readonly` stands.
        let ask_server = match &local {
            Ok(()) => true,
            Err(e) => e == "not_member",
        };
        match self.server_of(&m.doc) {
            Ok(Some(server))
                if ask_server && self.hub.homes_on && !self.relaying && m.by.member().is_some() =>
            {
                let client = crate::relay::Client::of(&m.by);
                let ask = self.leg(server, m.doc, client, |reply| crate::relay::Cmd::MayRead {
                    reply,
                });
                tokio::spawn(async move { tx.send(ask.await) });
            }
            _ => tx.send(local),
        }
        reply
    }
}

/// `ask` → `Vec<Citation>`, positionally aligned with `hrefs` (144 §6.4). Any engine; `by` decides
/// `available` for a member.
#[derive(Clone, Debug)]
pub struct CiteResolve {
    /// `theuth://…` hrefs.
    pub hrefs: Vec<String>,
    /// Who asks.
    pub by: Principal,
}

/// A block citation whose text the helper must read.
struct BlockLookup {
    at: usize,
    doc: DocId,
    block: String,
}

impl Message<CiteResolve> for DocumentActor {
    type Reply = DelegatedReply<Res<Vec<Citation>>>;

    async fn handle(
        &mut self,
        m: CiteResolve,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let mut out = Vec::with_capacity(m.hrefs.len());
        let mut lookups = Vec::new();
        for href in m.hrefs {
            let anchor = TheuthAnchor::parse(&href).ok();
            let mut c = Citation {
                kind: anchor
                    .as_ref()
                    .map_or(CiteKind::Notes, |a| cite_kind(a.kind)),
                id: anchor.as_ref().map(|a| a.id.clone()).unwrap_or_default(),
                locator: anchor
                    .as_ref()
                    .and_then(|a| a.locator.as_ref().map(Locator::text)),
                sha8: anchor.as_ref().and_then(|a| a.sha8.clone()),
                status: LocateStatus::Missing,
                available: false,
                title: String::new(),
                text: None,
                href,
            };
            if let Some(a) = anchor {
                self.resolve_cite(&a, &m.by, &mut c, out.len(), &mut lookups)
                    .await;
            }
            out.push(c);
        }
        let helper = self.helper.clone();
        tokio::spawn(async move {
            for l in lookups {
                let c = &mut out[l.at];
                let text = helper
                    .markdown_of(
                        &l.doc,
                        Some(std::slice::from_ref(&l.block)),
                        false,
                        &HashMap::new(),
                    )
                    .await
                    .ok()
                    .filter(|t| !t.trim().is_empty());
                match (text, c.sha8.as_deref()) {
                    (Some(t), None) => {
                        c.status = LocateStatus::Same;
                        c.text = Some(t);
                    }
                    (Some(t), Some(sha)) => {
                        c.status = if sha8(&t) == sha {
                            LocateStatus::Same
                        } else {
                            LocateStatus::Changed
                        };
                        c.text = Some(t);
                    }
                    (None, None) => c.status = LocateStatus::Missing,
                    (None, Some(sha)) => {
                        // ponytail: one helper call per block of the document; fine for notes and
                        // sources under the §4.2 cap, add a text index lookup if it ever shows.
                        c.status = LocateStatus::Missing;
                        let Ok(outline) = helper.outline(&l.doc, None).await else {
                            continue;
                        };
                        for id in block_ids(&outline.blocks) {
                            let Ok(t) = helper
                                .markdown_of(
                                    &l.doc,
                                    Some(std::slice::from_ref(&id)),
                                    false,
                                    &HashMap::new(),
                                )
                                .await
                            else {
                                continue;
                            };
                            if sha8(&t) == sha {
                                c.status = LocateStatus::Moved;
                                c.locator = Some(format!("#{id}"));
                                c.text = Some(t);
                                break;
                            }
                        }
                    }
                }
            }
            tx.send(Ok(out));
        });
        reply
    }
}

fn cite_kind(k: TheuthKind) -> CiteKind {
    match k {
        TheuthKind::Notes => CiteKind::Notes,
        TheuthKind::Sources => CiteKind::Sources,
        TheuthKind::Knowledge => CiteKind::Knowledge,
        TheuthKind::Meetings => CiteKind::Meetings,
        TheuthKind::Sessions => CiteKind::Sessions,
    }
}

/// Every block id of an outline tree, document order.
fn block_ids(blocks: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(blocks: &[serde_json::Value], out: &mut Vec<String>) {
        for b in blocks {
            if let Some(id) = b["id"].as_str() {
                out.push(id.to_owned());
            }
            if let Some(children) = b["children"].as_array() {
                walk(children, out);
            }
        }
    }
    walk(blocks, &mut out);
    out
}

impl DocumentActor {
    /// The synchronous half of one citation: existence, kind, readability, title, time locators.
    /// Block locators are queued for the helper.
    async fn resolve_cite(
        &mut self,
        a: &TheuthAnchor,
        by: &Principal,
        c: &mut Citation,
        at: usize,
        lookups: &mut Vec<BlockLookup>,
    ) {
        match a.kind {
            TheuthKind::Notes | TheuthKind::Sources => {
                let Some(doc) = DocId::parse(&a.id) else {
                    return;
                };
                let Ok(Some(summary)) = self.store.summary(&doc).await else {
                    return;
                };
                let kind_ok = match a.kind {
                    TheuthKind::Sources => summary.kind == DocKind::Source,
                    _ => summary.kind != DocKind::Source,
                };
                if !kind_ok || self.admit(&doc, by, false).await.is_err() {
                    return;
                }
                c.available = true;
                c.title = summary.title;
                match &a.locator {
                    None => c.status = LocateStatus::Same,
                    Some(Locator::Block(block)) => {
                        if self.load(&doc).is_ok() {
                            lookups.push(BlockLookup {
                                at,
                                doc,
                                block: block.clone(),
                            });
                        }
                    }
                    Some(Locator::Time(ms)) => {
                        let duration = self
                            .store
                            .source_row(&doc)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|r| r.duration_ms);
                        c.status = match duration {
                            Some(d) if *ms > d => LocateStatus::Missing,
                            _ => LocateStatus::Same,
                        };
                    }
                    Some(Locator::Turn(_)) => c.status = LocateStatus::Missing,
                }
            }
            TheuthKind::Knowledge => {
                let concept = a.id.split('@').next().unwrap_or(&a.id);
                if okf::ConceptId::parse(concept).is_err() {
                    return;
                }
                let root = match by {
                    Principal::Agent { session } => match crate::memory::router() {
                        Ok(router) => match router.root_for(runtime::SessionId(*session)).await {
                            Ok(root) => root,
                            Err(_) => return,
                        },
                        Err(_) => return,
                    },
                    _ => self.data_dir.clone(),
                };
                let path = root.join(format!("knowledge/{concept}.md"));
                if let Ok(text) = std::fs::read_to_string(&path) {
                    c.available = true;
                    c.status = LocateStatus::Same;
                    c.title = crate::actor::title_of(DocKind::Knowledge, &text);
                }
            }
            TheuthKind::Meetings => {
                if self.data_dir.join("meetings").join(&a.id).is_dir() {
                    c.available = true;
                    c.status = LocateStatus::Same;
                    let note = self
                        .store
                        .lookup(&Binding::MeetingNote(a.id.clone()))
                        .await
                        .ok()
                        .flatten();
                    c.title = match note {
                        Some(id) => self
                            .store
                            .summary(&id)
                            .await
                            .ok()
                            .flatten()
                            .map(|s| s.title)
                            .unwrap_or_default(),
                        None => String::new(),
                    };
                }
            }
            TheuthKind::Sessions => {
                // Sessions live in the agent crate; the document crate cannot check them (as-built
                // Deviations). The chip renders; the UI's session route reports a missing one.
                c.available = true;
                c.status = LocateStatus::Same;
                c.title = "Chat".to_owned();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ApplyUpdate, Edit, Subscribe};
    use crate::meta::{AgentStyle, DocMeta, Member, Role};
    use kameo::actor::{ActorRef, Spawn as _};

    /// An actor over a fresh store; the helper is never spawned (every path below is a store read or
    /// a refusal before the helper).
    async fn actor() -> (
        ActorRef<DocumentActor>,
        crate::store::Store,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let store = crate::store::Store::open(dir.path()).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper =
            crate::helper::Helper::new(tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            crate::DocumentsConfig::default(),
            rt,
        ));
        (actor, store, dir)
    }

    fn member(pub_: &str, role: Role) -> Member {
        Member {
            pub_: pub_.into(),
            name: pub_.into(),
            role,
            color: "#000".into(),
            added_ms: 1,
            via: None,
            last_seen_ms: 1,
        }
    }

    async fn doc(
        store: &crate::store::Store,
        binding: Binding,
        title: &str,
        members: Vec<Member>,
    ) -> DocId {
        let id = DocId::for_binding(&binding);
        let meta = DocMeta {
            derived: binding.derived(&id),
            id: id.clone(),
            kind: binding.kind(),
            title: title.into(),
            binding,
            fragment: crate::FRAGMENT.into(),
            members,
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        };
        store.create(&meta).await.unwrap();
        id
    }

    fn guest(pub_: &str) -> Principal {
        Principal::Member {
            pub_: pub_.into(),
            device: Some(7),
        }
    }

    const OWNER: Principal = Principal::Owner { device: None };

    /// 144 §11 D12: a notebook member opens the notebook's notes and sources at the notebook's role,
    /// later additions included; a removed item closes the guest's subscription with `Closed{revoked}`;
    /// a stranger stays `not_member`. §3.1: a source refuses owner and member updates (`source_readonly`)
    /// and agent edits; §3.4: the preview counts, the delete trashes exclusive sources only.
    #[tokio::test]
    async fn derived_grant_revocation_source_readonly_and_delete() {
        let (a, store, _dir) = actor().await;
        let nb = doc(
            &store,
            Binding::Notebook,
            "NB",
            vec![member("alice", Role::Viewer)],
        )
        .await;
        let other = doc(&store, Binding::Notebook, "Other", vec![]).await;
        let note = doc(&store, Binding::Note, "Note", vec![]).await;
        let src = doc(&store, Binding::Source("url:http://a/".into()), "A", vec![]).await;
        let shared = doc(&store, Binding::Source("url:http://b/".into()), "B", vec![]).await;
        store
            .notebook_upsert(&NotebookRow {
                id: nb.clone(),
                project: None,
                archived_ms: None,
            })
            .await
            .unwrap();

        // Nothing granted before the items exist.
        assert_eq!(
            a.ask(MayRead {
                doc: note.clone(),
                by: guest("alice")
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_member"
        );
        for (kind, item) in [
            (ItemKind::Note, &note),
            (ItemKind::Source, &src),
            (ItemKind::Source, &shared),
        ] {
            let row = a
                .ask(NotebookItemAdd {
                    notebook: nb.clone(),
                    kind,
                    item: item.to_string(),
                    by: OWNER,
                })
                .await
                .unwrap();
            assert_eq!((row.kind, row.added_by.as_str()), (kind, "owner"));
        }
        a.ask(NotebookItemAdd {
            notebook: other.clone(),
            kind: ItemKind::Source,
            item: shared.to_string(),
            by: Principal::Agent {
                session: uuid::Uuid::nil(),
            },
        })
        .await
        .unwrap();
        // Kind checks on add.
        assert_eq!(
            a.ask(NotebookItemAdd {
                notebook: nb.clone(),
                kind: ItemKind::Source,
                item: note.to_string(),
                by: OWNER
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_a_source"
        );
        assert_eq!(
            a.ask(NotebookItemAdd {
                notebook: note.clone(),
                kind: ItemKind::Note,
                item: src.to_string(),
                by: OWNER
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_a_notebook"
        );

        // The derived grant: alice reads the note and both sources, bob (nobody) does not.
        for d in [&note, &src, &shared] {
            a.ask(MayRead {
                doc: d.clone(),
                by: guest("alice"),
            })
            .await
            .unwrap();
            assert_eq!(
                a.ask(MayRead {
                    doc: d.clone(),
                    by: guest("bob")
                })
                .await
                .unwrap_err()
                .to_string(),
                "not_member"
            );
        }
        // A viewer's write is `viewer_readonly`; the owner's write on a source is `source_readonly`.
        let upd = "AAA=".to_owned();
        assert_eq!(
            a.ask(ApplyUpdate {
                doc: note.clone(),
                update: upd.clone(),
                by: guest("alice")
            })
            .await
            .unwrap_err()
            .to_string(),
            "viewer_readonly"
        );
        assert_eq!(
            a.ask(ApplyUpdate {
                doc: src.clone(),
                update: upd.clone(),
                by: OWNER
            })
            .await
            .unwrap_err()
            .to_string(),
            "source_readonly"
        );
        let e = a
            .ask(Edit {
                doc: src.clone(),
                edits: vec![],
                by: Principal::Agent {
                    session: uuid::Uuid::nil(),
                },
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("source_readonly"), "{e}");

        // Lists and the notebook views.
        let list = a.ask(NotebookList { archived: false }).await.unwrap();
        let mine = list.iter().find(|n| n.doc.id == nb).unwrap();
        assert_eq!(
            (mine.sources, mine.notes, mine.doc.title.as_str()),
            (2, 1, "NB")
        );
        let detail = a
            .ask(NotebookGet {
                id: nb.clone(),
                by: guest("alice"),
            })
            .await
            .unwrap();
        assert_eq!(detail.items.len(), 3);
        assert_eq!(
            a.ask(NotebookGet {
                id: nb.clone(),
                by: guest("bob")
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_member"
        );
        assert_eq!(
            a.ask(NotebookItemsList {
                notebook: nb.clone(),
                by: guest("alice")
            })
            .await
            .unwrap()
            .len(),
            3
        );

        // Archive hides from the default list; the row and the grant survive.
        a.ask(NotebookUpdate {
            id: nb.clone(),
            title: None,
            project: Some(Some("p".into())),
            archived: Some(true),
            by: OWNER,
        })
        .await
        .unwrap();
        assert!(
            a.ask(NotebookList { archived: false })
                .await
                .unwrap()
                .iter()
                .all(|n| n.doc.id != nb)
        );
        let archived = a.ask(NotebookList { archived: true }).await.unwrap();
        let mine = archived.iter().find(|n| n.doc.id == nb).unwrap();
        assert!(mine.archived_ms.is_some());
        assert_eq!(mine.project.as_deref(), Some("p"));
        a.ask(MayRead {
            doc: note.clone(),
            by: guest("alice"),
        })
        .await
        .unwrap();

        // Removal closes the derived subscription with `revoked`; the shared source says "also in 1".
        let mut sub = a
            .ask(Subscribe {
                doc: note.clone(),
                by: guest("alice"),
            })
            .await
            .unwrap();
        let removed = a
            .ask(NotebookItemRemove {
                notebook: nb.clone(),
                kind: ItemKind::Note,
                item: note.to_string(),
                by: OWNER,
            })
            .await
            .unwrap();
        assert_eq!(removed.also_in, 0);
        assert_eq!(
            sub.recv().await,
            Some(DocEvent::Closed {
                reason: "revoked".into()
            })
        );
        assert_eq!(
            a.ask(MayRead {
                doc: note.clone(),
                by: guest("alice")
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_member"
        );
        assert_eq!(
            a.ask(NotebookItemRemove {
                notebook: nb.clone(),
                kind: ItemKind::Note,
                item: note.to_string(),
                by: OWNER
            })
            .await
            .unwrap_err()
            .to_string(),
            "not_in_notebook"
        );

        // Delete preview and delete: `src` is exclusive, `shared` is also in `other`.
        let preview = a
            .ask(NotebookDeletePreview { id: nb.clone() })
            .await
            .unwrap();
        assert_eq!((preview.notes, preview.sources_exclusive), (0, 1));
        assert_eq!(
            preview.sources_shared_elsewhere,
            vec![SharedSource {
                source: shared.clone(),
                title: "B".into(),
                also_in: 1
            }]
        );
        let mut sub = a
            .ask(Subscribe {
                doc: shared.clone(),
                by: guest("alice"),
            })
            .await
            .unwrap();
        a.ask(NotebookDelete {
            id: nb.clone(),
            delete_exclusive_sources: true,
            by: OWNER,
        })
        .await
        .unwrap();
        assert_eq!(
            sub.recv().await,
            Some(DocEvent::Closed {
                reason: "revoked".into()
            })
        );
        assert_eq!(store.summary(&nb).await.unwrap(), None);
        assert_eq!(
            store.summary(&src).await.unwrap(),
            None,
            "exclusive source trashed"
        );
        assert!(
            store.summary(&shared).await.unwrap().is_some(),
            "shared source kept"
        );
        assert!(
            store.summary(&note).await.unwrap().is_some(),
            "notes are never deleted"
        );
        assert_eq!(
            a.ask(NotebookGet {
                id: nb.clone(),
                by: OWNER
            })
            .await
            .unwrap_err()
            .to_string(),
            "notebook_not_found"
        );
    }

    /// 144 §6.4: `cite_resolve` statuses that need no block text — whole-document citations, trashed
    /// and foreign-kind targets, member readability, time locators against `duration_ms`, knowledge
    /// files, sessions — and a malformed href.
    #[tokio::test]
    async fn cite_resolve_statuses_without_blocks() {
        let (a, store, dir) = actor().await;
        let note = doc(&store, Binding::Note, "Note", vec![]).await;
        let src = doc(&store, Binding::Source("url:http://a/".into()), "A", vec![]).await;
        let gone = doc(&store, Binding::Note, "Gone", vec![]).await;
        store.delete(&gone).await.unwrap();
        store
            .source_upsert(&crate::store::SourceRow {
                id: src.clone(),
                origin: SourceOrigin::Url {
                    url: "http://a/".into(),
                    canonical: None,
                },
                status: SourceStatus::Completed,
                error: None,
                attempts: 1,
                blob: None,
                fetched_ms: None,
                content_sha: None,
                changed_since: None,
                bytes: None,
                words: None,
                duration_ms: Some(60_000),
                pages: None,
                meta: serde_json::Value::Null,
                insights: 0,
            })
            .await
            .unwrap();
        std::fs::create_dir_all(dir.path().join("knowledge/facts")).unwrap();
        std::fs::write(
            dir.path().join("knowledge/facts/k.md"),
            "---\ntitle: Kay\n---\nbody\n",
        )
        .unwrap();
        let hrefs = vec![
            format!("theuth://notes/{note}"),
            format!("theuth://notes/{gone}"),
            format!("theuth://sources/{note}"),
            format!("theuth://sources/{src}@00:30"),
            format!("theuth://sources/{src}@01:30"),
            "theuth://knowledge/facts/k@abc".to_owned(),
            "theuth://knowledge/facts/none".to_owned(),
            "theuth://sessions/s1#3".to_owned(),
            "not a link".to_owned(),
        ];
        let got = a
            .ask(CiteResolve {
                hrefs: hrefs.clone(),
                by: OWNER,
            })
            .await
            .unwrap();
        let st: Vec<(bool, LocateStatus)> = got.iter().map(|c| (c.available, c.status)).collect();
        assert_eq!(
            st,
            vec![
                (true, LocateStatus::Same),
                (false, LocateStatus::Missing),
                (false, LocateStatus::Missing),
                (true, LocateStatus::Same),
                (true, LocateStatus::Missing),
                (true, LocateStatus::Same),
                (false, LocateStatus::Missing),
                (true, LocateStatus::Same),
                (false, LocateStatus::Missing),
            ]
        );
        assert_eq!(got[0].title, "Note");
        assert_eq!(
            (got[3].kind, got[3].locator.as_deref()),
            (CiteKind::Sources, Some("@00:30"))
        );
        assert_eq!(got[5].title, "Kay");
        assert_eq!(got[7].href, "theuth://sessions/s1#3");
        // A stranger sees the note as unavailable; nothing else changes.
        let guest = a
            .ask(CiteResolve {
                hrefs: hrefs[..1].to_vec(),
                by: guest("bob"),
            })
            .await
            .unwrap();
        assert!(!guest[0].available);
        assert_eq!(sha8("abc \n"), sha8("abc"));
        assert_eq!(sha8("abc").len(), 8);
    }

    #[test]
    fn origin_keys_follow_the_grammar_and_dedupe_media_by_sha() {
        let file = SourceOrigin::File {
            name: "a.pdf".into(),
            sha256: "ab".into(),
            ext: "pdf".into(),
        };
        let audio = SourceOrigin::Audio {
            name: "a.m4a".into(),
            sha256: "ab".into(),
            ext: "m4a".into(),
        };
        assert_eq!(file.key(), "file:ab");
        assert_eq!(audio.key(), file.key());
        assert_eq!(
            SourceOrigin::Url {
                url: "http://x/?b".into(),
                canonical: Some("http://x/".into())
            }
            .key(),
            "url:http://x/"
        );
        assert_eq!(
            SourceOrigin::Youtube {
                video: "dQw4w9WgXcQ".into(),
                url: "https://youtu.be/dQw4w9WgXcQ".into(),
                route: YoutubeRoute::Captions
            }
            .key(),
            "youtube:dQw4w9WgXcQ"
        );
        let mcp = SourceOrigin::Mcp {
            server: "atlassian".into(),
            tool: "get_page".into(),
            args: serde_json::json!({"id": 7}),
            label: McpLabel::Confluence,
        };
        assert!(mcp.key().starts_with("mcp:atlassian/get_page/"));
        assert_eq!(mcp.key().len(), "mcp:atlassian/get_page/".len() + 64);
    }

    #[test]
    fn wire_tags_are_the_frozen_words() {
        assert_eq!(
            serde_json::to_string(&ItemKind::Source).unwrap(),
            "\"source\""
        );
        assert_eq!(
            serde_json::to_string(&SourceStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&ContextLevel::Full).unwrap(),
            "\"full\""
        );
        let ev = SourceEvent::Progress {
            source: DocId("a".repeat(26)),
            percent: 40,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "Progress");
        assert_eq!(v["percent"], 40);
        let o = serde_json::to_value(SourceOrigin::Meeting {
            meeting: "m1".into(),
        })
        .unwrap();
        assert_eq!(o["kind"], "meeting");
        let id = DocId("b".repeat(26));
        assert_eq!(
            NotebookItem::row_id(&id, ItemKind::Note, "x"),
            format!("{id}:note:x")
        );
    }
}
