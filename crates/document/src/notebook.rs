//! Notebooks (design 144 §3, §13.1): the frozen wire shapes of the container, its membership rows,
//! sources, the context dial, citations and the source event — ts-rs exported like `DocKind` — plus
//! the actor messages the `notebook_*` / `cite_resolve` RPCs and tools send the [`DocumentActor`].
//!
//! Shapes here are the §13 contract: four workers code against them; a change is a design amendment,
//! never a local edit.

use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};

pub use codeindex::LocateStatus;

use crate::actor::DocumentActor;
use crate::meta::{DocId, DocSummary, Principal};

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

/// One item of the pre-turn estimate (144 §5.2). TS name `NotebookContextItem`: `agent::ContextItem`
/// already owns `ContextItem.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "NotebookContextItem")
)]
pub struct ContextItem {
    /// What `item` is.
    pub kind: ItemKind,
    /// The item id.
    pub item: String,
    /// The dial position for this thread.
    pub level: ContextLevel,
    /// What the budget did to it.
    pub status: ContextStatus,
    /// Tokens it costs at `level`.
    pub tokens: u32,
}

/// The pre-turn estimate (144 §5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextEstimate {
    /// Every item of the notebook with its level and fate.
    pub items: Vec<ContextItem>,
    /// Tokens the injection will cost.
    pub tokens: u32,
    /// The budget (`notebooks.context_budget_tokens`).
    pub budget: u32,
    /// Items left out.
    pub omitted: u32,
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

// ---- Actor messages (144-C owns the handlers; bodies land in phase 2) ----------------------------------

/// Reply type every handler below uses; `NotHub:<id>` for hub-only writes off the hub (69 Y).
type Res<T> = Result<T, String>;

const NOT_IMPLEMENTED: &str = "144: not implemented yet";

/// `ask` → `Vec<Notebook>`: live notebooks, newest update first; `archived` includes archived ones.
/// Served on any engine from the replica.
#[derive(Clone, Debug, Default)]
pub struct NotebookList {
    /// Include archived notebooks.
    pub archived: bool,
}

impl Message<NotebookList> for DocumentActor {
    type Reply = Res<Vec<Notebook>>;

    async fn handle(&mut self, _: NotebookList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
    }
}

/// `ask` → [`NotebookDetail`]. Any engine.
#[derive(Clone, Debug)]
pub struct NotebookGet {
    /// The notebook.
    pub id: DocId,
}

impl Message<NotebookGet> for DocumentActor {
    type Reply = Res<NotebookDetail>;

    async fn handle(&mut self, _: NotebookGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
    type Reply = Res<Notebook>;

    async fn handle(
        &mut self,
        _: NotebookCreate,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
        _: NotebookUpdate,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
        _: NotebookDeletePreview,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
        _: NotebookDelete,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
    }
}

/// `ask` → `Vec<NotebookItem>`: live items, oldest first. Any engine.
#[derive(Clone, Debug)]
pub struct NotebookItemsList {
    /// The notebook.
    pub notebook: DocId,
}

impl Message<NotebookItemsList> for DocumentActor {
    type Reply = Res<Vec<NotebookItem>>;

    async fn handle(
        &mut self,
        _: NotebookItemsList,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
        _: NotebookItemAdd,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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
        _: NotebookItemRemove,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
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

impl Message<CiteResolve> for DocumentActor {
    type Reply = Res<Vec<Citation>>;

    async fn handle(&mut self, _: CiteResolve, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(NOT_IMPLEMENTED.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
