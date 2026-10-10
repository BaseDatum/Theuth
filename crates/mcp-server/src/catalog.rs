//! The catalogue (158 §12.4, Amendment A2): every tool is one thin call into an engine RPC
//! through [`Rpc`], the same list over HTTP and `t mcp`. Lists page in the catalogue (`limit`
//! ≤ 200, opaque cursors, never a time budget); Private sessions are dropped from the session
//! and artifact tools (§12.5); results pass the vault's redaction like tool output; every call
//! appends one audit line.

use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::audit::{Audit, args_sha256};
use crate::{AuditLine, Rpc};

/// Default page size; `limit` is clamped to [`MAX_LIMIT`].
pub const DEFAULT_LIMIT: u32 = 50;
/// 158 §12.4: no list returns more than this per call.
pub const MAX_LIMIT: u32 = 200;

/// What `ServerInfo.instructions` says (158 §12.4: route to the smallest tool).
pub const INSTRUCTIONS: &str = "theuth's data on this machine: meetings (transcripts, summaries, briefs), \
briefs, knowledge, documents, notebooks, sessions, tasks, artifacts, calendar. \
Search first (`meeting_search`, `knowledge_search`, `brief_search`, `notebook_search`), then read one item \
(`meeting_summary` before `meeting_transcript`; `knowledge_get`, `doc_markdown`). Lists page: pass the \
`next_cursor` you got back as `cursor`. Write tools change the user's data at once; there is no undo here.";

/// The server: one engine client, one transport label, the audit file, the vault for redaction
/// (`None` in `t mcp`, which has no vault handle).
#[derive(Clone)]
pub struct Catalog {
    rpc: Arc<dyn Rpc>,
    transport: &'static str,
    audit: Audit,
    vault: Option<Arc<secrets::Vault>>,
}

// ---- argument shapes -------------------------------------------------------------------------

/// No arguments.
#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

// Paging, shared by every list (a plain comment: flattened, its doc would replace the tool's).
#[derive(Deserialize, JsonSchema, Default)]
struct Page {
    /// Items per page, 1..=200 (default 50).
    limit: Option<u32>,
    /// `next_cursor` from the previous page; omit for the first page.
    cursor: Option<String>,
}

impl Page {
    fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize
    }
    fn offset(&self) -> usize {
        self.cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0)
    }
    /// How many rows to ask an RPC with its own `limit` for: one past the page, so `slice` can
    /// tell whether a next page exists.
    fn fetch(&self) -> u32 {
        (self.offset() + self.limit() + 1) as u32
    }
    /// `{items, next_cursor}` over the full list; `next_cursor` only when more rows exist.
    fn slice(&self, items: Vec<Value>) -> Value {
        let (off, lim) = (self.offset(), self.limit());
        let more = items.len() > off + lim;
        let page: Vec<Value> = items.into_iter().skip(off).take(lim).collect();
        json!({ "items": page, "next_cursor": more.then(|| (off + lim).to_string()) })
    }
}

#[derive(Deserialize, JsonSchema)]
struct Id {
    /// The item's id.
    id: String,
}

#[derive(Deserialize, JsonSchema)]
struct MeetingList {
    /// Only meetings of this project.
    project: Option<String>,
    /// Only meetings started at or after this RFC 3339 time.
    since: Option<String>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct TranscriptArgs {
    /// Meeting id.
    id: String,
    /// Index of the first segment (default 0; the previous page's `next`).
    from_seg: Option<u32>,
    /// Segments per page, 1..=200 (default 200).
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct Query {
    /// Full-text query.
    query: String,
    /// Max hits (default 10, at most 50).
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct BriefList {
    /// Only briefs carrying this tag.
    tag: Option<String>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct KnowledgeList {
    /// Id prefix, e.g. `decisions/`.
    prefix: Option<String>,
    /// `fact | entity | decision | procedure | project`.
    #[serde(rename = "type")]
    type_: Option<String>,
    /// Only concepts with this tag.
    tag: Option<String>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct DocList {
    /// Document kind.
    kind: Option<document::DocKind>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct NotebookList {
    /// Include archived notebooks.
    archived: Option<bool>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct NotebookItems {
    /// Notebook id.
    notebook: String,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct NotebookSearch {
    /// Full-text query over notebook blocks.
    query: String,
    /// Restrict to these notebook ids.
    notebooks: Option<Vec<String>>,
    /// Max hits (default 10).
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct SessionList {
    /// Only sessions of this project.
    project: Option<String>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct SessionRead {
    /// Session id.
    id: String,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct TaskList {
    #[serde(flatten)]
    filter: tasks::TaskFilter,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct ArtifactList {
    /// Only this session's artifacts.
    session: Option<String>,
    #[serde(flatten)]
    page: Page,
}

#[derive(Deserialize, JsonSchema)]
struct Window {
    /// RFC 3339 start of the window.
    from: String,
    /// RFC 3339 end of the window.
    to: String,
}

#[derive(Deserialize, JsonSchema)]
struct MeetingEdit {
    /// Meeting id.
    id: String,
    /// Edits, applied all-or-none.
    ops: Vec<meeting::EditOp>,
}

#[derive(Deserialize, JsonSchema)]
struct SetProject {
    /// Meeting id.
    id: String,
    /// Project label; omit to clear.
    project: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct SetEvent {
    /// Meeting id.
    id: String,
    /// Calendar event id (`source:uid:recurrence_id`); omit to clear.
    event: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct SetTemplate {
    /// Meeting id.
    id: String,
    /// Template slug (`meeting_templates` lists them in the UI).
    template: String,
}

#[derive(Deserialize, JsonSchema)]
struct DocCreate {
    /// Title of the new note.
    title: String,
    /// Initial markdown body.
    markdown: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct Rename {
    /// Document id.
    id: String,
    /// New title.
    title: String,
}

#[derive(Deserialize, JsonSchema)]
struct TaskUpdate {
    /// Task id.
    id: String,
    #[serde(flatten)]
    patch: tasks::TaskPatch,
}

#[derive(Deserialize, JsonSchema)]
struct TaskMove {
    /// Task id.
    id: String,
    /// Target state.
    state: tasks::TaskState,
    /// Why, shown in the task's thread.
    note: Option<String>,
    /// For `done` from `review`: the next task's number.
    next: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct TaskComment {
    /// Task id.
    id: String,
    /// Comment text.
    text: String,
}

#[derive(Deserialize, JsonSchema)]
struct SourceAdd {
    /// Display name.
    name: String,
    /// `https://` or `webcal://` ICS address.
    url: String,
}

// ---- the table ---------------------------------------------------------------------------------

/// `(name, description, schema, read-only)`; the order clients see.
fn table() -> Vec<(
    &'static str,
    &'static str,
    Arc<serde_json::Map<String, Value>>,
    bool,
)> {
    let s = |x: Arc<serde_json::Map<String, Value>>| x;
    vec![
        (
            "meeting_list",
            "Recorded meetings, newest first (summaries: id, title, started_at, state, project). Use `meeting_search` to find one by words.",
            s(schema_for_type::<MeetingList>()),
            true,
        ),
        (
            "meeting_get",
            "One meeting's manifest: title, times, process, calendar event, project, template, note document.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "meeting_transcript",
            "A page of transcript segments (speaker, t0_ms, t1_ms, text). Read `meeting_summary` first; this is long.",
            s(schema_for_type::<TranscriptArgs>()),
            true,
        ),
        (
            "meeting_summary",
            "The meeting's summary markdown (decisions, action items, open questions). Start here for any meeting.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "meeting_briefs",
            "The in-meeting briefs (title + bullets) a meeting produced.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "meeting_search",
            "Full-text search over meeting titles and transcripts; hits carry a snippet and the meeting summary row.",
            s(schema_for_type::<Query>()),
            true,
        ),
        (
            "meeting_speakers",
            "Enrolled speaker profiles (names, sample counts; never embeddings).",
            s(schema_for_type::<NoArgs>()),
            true,
        ),
        (
            "brief_search",
            "Full-text search over cached briefs.",
            s(schema_for_type::<Query>()),
            true,
        ),
        (
            "brief_get",
            "One cached brief by id.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "brief_list",
            "Cached briefs, newest first, optionally by tag.",
            s(schema_for_type::<BriefList>()),
            true,
        ),
        (
            "knowledge_search",
            "Semantic + full-text search over the knowledge base; hits carry a snippet. Use `knowledge_get` for the whole note.",
            s(schema_for_type::<Query>()),
            true,
        ),
        (
            "knowledge_get",
            "One knowledge concept: metadata, text, links.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "knowledge_list",
            "Knowledge concepts by id prefix, type or tag (metadata only).",
            s(schema_for_type::<KnowledgeList>()),
            true,
        ),
        (
            "doc_list",
            "Documents (notes, meeting notes, artifacts, notebooks…), newest first; `doc_markdown` reads one.",
            s(schema_for_type::<DocList>()),
            true,
        ),
        (
            "doc_markdown",
            "A document's content as markdown.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "notebook_list",
            "Notebooks with their source, note and chat counts.",
            s(schema_for_type::<NotebookList>()),
            true,
        ),
        (
            "notebook_items",
            "A notebook's items (sources, notes, sessions, artifacts), oldest first.",
            s(schema_for_type::<NotebookItems>()),
            true,
        ),
        (
            "notebook_search",
            "Block-level search inside notebooks; hits carry heading and snippet.",
            s(schema_for_type::<NotebookSearch>()),
            true,
        ),
        (
            "session_list",
            "Chat sessions (title, project, model, times), newest first. Private sessions are never listed.",
            s(schema_for_type::<SessionList>()),
            true,
        ),
        (
            "session_read",
            "A page of one session's history (user, assistant, tool entries). Private sessions are refused.",
            s(schema_for_type::<SessionRead>()),
            true,
        ),
        (
            "task_list",
            "Tasks, oldest first, filtered by owner, state or project.",
            s(schema_for_type::<TaskList>()),
            true,
        ),
        (
            "task_get",
            "One task by id.",
            s(schema_for_type::<Id>()),
            true,
        ),
        (
            "artifact_list",
            "Artifact metadata (name, kind, mime, bytes, session); artifacts of Private sessions are dropped.",
            s(schema_for_type::<ArtifactList>()),
            true,
        ),
        (
            "calendar_events",
            "Calendar events overlapping a window, start ascending.",
            s(schema_for_type::<Window>()),
            true,
        ),
        (
            "calendar_sources",
            "Calendar sources (macOS Calendar, ICS subscriptions) with their last fetch state.",
            s(schema_for_type::<NoArgs>()),
            true,
        ),
        (
            "meeting_edit",
            "Edit a meeting transcript: relabel/rename/merge/split segments, fix text, enroll a speaker, mark reviewed. All-or-none.",
            s(schema_for_type::<MeetingEdit>()),
            false,
        ),
        (
            "meeting_set_project",
            "Set or clear a meeting's project label.",
            s(schema_for_type::<SetProject>()),
            false,
        ),
        (
            "meeting_set_event",
            "Link a meeting to a calendar event, or unlink it.",
            s(schema_for_type::<SetEvent>()),
            false,
        ),
        (
            "meeting_set_template",
            "Set the notes template a meeting is summarised with.",
            s(schema_for_type::<SetTemplate>()),
            false,
        ),
        (
            "doc_create",
            "Create a note document with optional markdown.",
            s(schema_for_type::<DocCreate>()),
            false,
        ),
        (
            "doc_rename",
            "Rename a note document.",
            s(schema_for_type::<Rename>()),
            false,
        ),
        (
            "doc_delete",
            "Move a note document to the trash.",
            s(schema_for_type::<Id>()),
            false,
        ),
        (
            "knowledge_put",
            "Create or replace a knowledge concept (id, type, title, tags, body).",
            s(schema_for_type::<knowledge::Put>()),
            false,
        ),
        (
            "knowledge_delete",
            "Delete a knowledge concept.",
            s(schema_for_type::<Id>()),
            false,
        ),
        (
            "task_create",
            "Create a task (title, brief, owner, state…).",
            s(schema_for_type::<tasks::TaskCreate>()),
            false,
        ),
        (
            "task_update",
            "Patch a task's fields.",
            s(schema_for_type::<TaskUpdate>()),
            false,
        ),
        (
            "task_move",
            "Move a task to another state (backlog, todo, doing, review, blocked, done, cancelled).",
            s(schema_for_type::<TaskMove>()),
            false,
        ),
        (
            "task_comment",
            "Append a comment to a task's thread.",
            s(schema_for_type::<TaskComment>()),
            false,
        ),
        (
            "brief_put",
            "Create (no id) or update a cached brief.",
            s(schema_for_type::<meeting::CachedBriefPut>()),
            false,
        ),
        (
            "brief_delete",
            "Delete a cached brief.",
            s(schema_for_type::<Id>()),
            false,
        ),
        (
            "calendar_source_add",
            "Subscribe to an ICS calendar address.",
            s(schema_for_type::<SourceAdd>()),
            false,
        ),
        (
            "calendar_source_delete",
            "Remove a calendar source.",
            s(schema_for_type::<Id>()),
            false,
        ),
        (
            "calendar_source_refresh",
            "Fetch a calendar source now.",
            s(schema_for_type::<Id>()),
            false,
        ),
    ]
}

/// The tools as clients see them.
pub fn tools() -> Vec<Tool> {
    table()
        .into_iter()
        .map(|(name, desc, schema, read_only)| {
            Tool::new(name, desc, schema).annotate(
                ToolAnnotations::new()
                    .read_only(read_only)
                    .destructive(!read_only)
                    .open_world(false),
            )
        })
        .collect()
}

/// Tool names in catalogue order.
pub fn names() -> Vec<&'static str> {
    table().into_iter().map(|t| t.0).collect()
}

// ---- the handler -------------------------------------------------------------------------------

/// What a tool hands back: JSON (pretty-printed for the client) or plain text.
enum Out {
    Json(Value),
    Text(String),
}

fn args<T: serde::de::DeserializeOwned>(req: &CallToolRequestParams) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(req.arguments.clone().unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))
}

fn is_private(v: &Value) -> bool {
    v.get("retention").and_then(Value::as_str) == Some("private")
}

fn as_array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

impl Catalog {
    /// A catalogue over `rpc`; `transport` is the audit label (`http` | `stdio`).
    pub fn new(
        rpc: Arc<dyn Rpc>,
        transport: &'static str,
        audit: Audit,
        vault: Option<Arc<secrets::Vault>>,
    ) -> Self {
        Self {
            rpc,
            transport,
            audit,
            vault,
        }
    }

    async fn rpc(&self, method: &str, params: Vec<Value>) -> Result<Value, String> {
        self.rpc.call(method, params).await
    }

    /// Session ids with `retention == private` (158 §12.5).
    async fn private_sessions(&self) -> Result<std::collections::HashSet<String>, String> {
        let sessions = as_array(self.rpc("list_sessions", vec![]).await?);
        Ok(sessions
            .iter()
            .filter(|s| is_private(s))
            .filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect())
    }

    /// One tool call; `Err(text)` is the tool's error content.
    async fn dispatch(&self, req: &CallToolRequestParams) -> Result<Out, Error> {
        use Out::{Json, Text};
        let v = match req.name.as_ref() {
            "meeting_list" => {
                let a: MeetingList = args(req)?;
                // `since` filters after the fetch, so take the whole index then (rows are small).
                let limit = a.since.as_ref().map_or(a.page.fetch(), |_| u32::MAX);
                let rows = as_array(
                    self.rpc("meeting_list", vec![json!(limit), json!(a.project)])
                        .await?,
                );
                let rows: Vec<Value> = match &a.since {
                    Some(since) => rows
                        .into_iter()
                        .filter(|m| {
                            m.get("started_at")
                                .and_then(Value::as_str)
                                .is_some_and(|s| s >= since.as_str())
                        })
                        .collect(),
                    None => rows,
                };
                Json(a.page.slice(rows))
            }
            "meeting_get" => {
                let a: Id = args(req)?;
                Json(self.rpc("meeting_get", vec![json!(a.id)]).await?)
            }
            "meeting_transcript" => {
                let a: TranscriptArgs = args(req)?;
                let mut t = self.rpc("meeting_transcript", vec![json!(a.id)]).await?;
                let from = a.from_seg.unwrap_or(0) as usize;
                let limit = a.limit.unwrap_or(MAX_LIMIT).clamp(1, MAX_LIMIT) as usize;
                let segments = t
                    .get_mut("segments")
                    .map(Value::take)
                    .map(as_array)
                    .unwrap_or_default();
                let next = (segments.len() > from + limit).then(|| from + limit);
                let page: Vec<Value> = segments.into_iter().skip(from).take(limit).collect();
                Json(json!({
                    "meeting": t.get("meeting"),
                    "speakers": t.get("speakers"),
                    "diarized": t.get("diarized"),
                    "segments": page,
                    "next": next,
                }))
            }
            "meeting_summary" => {
                let a: Id = args(req)?;
                Text(text(self.rpc("meeting_summary", vec![json!(a.id)]).await?))
            }
            "meeting_briefs" => {
                let a: Id = args(req)?;
                Json(self.rpc("meeting_briefs", vec![json!(a.id)]).await?)
            }
            "meeting_search" => {
                let a: Query = args(req)?;
                let r = self
                    .rpc(
                        "search",
                        vec![json!(a.query), json!(["meetings"]), json!(a.limit)],
                    )
                    .await?;
                let page = as_array(r.get("groups").cloned().unwrap_or_default())
                    .into_iter()
                    .find(|g| g.get("category").and_then(Value::as_str) == Some("meetings"))
                    .and_then(|g| g.get("page").cloned())
                    .unwrap_or_else(|| json!({ "hits": [] }));
                Json(page)
            }
            "meeting_speakers" => Json(self.rpc("meeting_speakers", vec![]).await?),
            "brief_search" => {
                let a: Query = args(req)?;
                Json(
                    self.rpc("brief_search", vec![json!(a.query), json!(a.limit)])
                        .await?,
                )
            }
            "brief_get" => {
                let a: Id = args(req)?;
                Json(self.rpc("brief_get", vec![json!(a.id)]).await?)
            }
            "brief_list" => {
                let a: BriefList = args(req)?;
                let rows = self
                    .rpc("brief_list", vec![json!(a.tag), json!(a.page.fetch())])
                    .await?;
                Json(a.page.slice(as_array(rows)))
            }
            "knowledge_search" => {
                let a: Query = args(req)?;
                Json(
                    self.rpc("knowledge_search", vec![json!(a.query), json!(a.limit)])
                        .await?,
                )
            }
            "knowledge_get" => {
                let a: Id = args(req)?;
                Json(self.rpc("knowledge_get", vec![json!(a.id)]).await?)
            }
            "knowledge_list" => {
                let a: KnowledgeList = args(req)?;
                let rows = self
                    .rpc(
                        "knowledge_list",
                        vec![
                            json!(a.prefix),
                            json!(a.type_),
                            json!(a.tag),
                            json!(a.page.fetch()),
                        ],
                    )
                    .await?;
                Json(a.page.slice(as_array(rows)))
            }
            "doc_list" => {
                let a: DocList = args(req)?;
                let rows = self
                    .rpc("doc_list", vec![json!(a.kind), json!(a.page.fetch())])
                    .await?;
                Json(a.page.slice(as_array(rows)))
            }
            "doc_markdown" => {
                let a: Id = args(req)?;
                Text(text(self.rpc("doc_markdown", vec![json!(a.id)]).await?))
            }
            "notebook_list" => {
                let a: NotebookList = args(req)?;
                let rows = self.rpc("notebook_list", vec![json!(a.archived)]).await?;
                Json(a.page.slice(as_array(rows)))
            }
            "notebook_items" => {
                let a: NotebookItems = args(req)?;
                let rows = self
                    .rpc("notebook_items_list", vec![json!(a.notebook)])
                    .await?;
                Json(a.page.slice(as_array(rows)))
            }
            "notebook_search" => {
                let a: NotebookSearch = args(req)?;
                Json(
                    self.rpc(
                        "nb_search",
                        vec![json!({
                            "query": a.query,
                            "notebooks": a.notebooks,
                            "limit": a.limit,
                        })],
                    )
                    .await?,
                )
            }
            "session_list" => {
                let a: SessionList = args(req)?;
                let rows = as_array(self.rpc("list_sessions", vec![]).await?)
                    .into_iter()
                    .filter(|s| !is_private(s))
                    .filter(|s| {
                        a.project.as_ref().is_none_or(|p| {
                            s.get("project").and_then(Value::as_str) == Some(p.as_str())
                        })
                    })
                    .collect();
                Json(a.page.slice(rows))
            }
            "session_read" => {
                let a: SessionRead = args(req)?;
                // ponytail: the whole history per page (bounded by the session); `before`/`limit`
                // paging by store seq if a session ever gets too big for one load.
                let mut s = self
                    .rpc(
                        "load_session",
                        vec![json!(a.id), Value::Null, Value::Null, Value::Null],
                    )
                    .await?;
                if is_private(&s) {
                    return Err(Error::Tool("private session: not shared over MCP".into()));
                }
                let history = s
                    .get_mut("history")
                    .map(Value::take)
                    .map(as_array)
                    .unwrap_or_default();
                let mut out = a.page.slice(history);
                out["project"] = s.get("project").cloned().unwrap_or(Value::Null);
                out["kind"] = s.get("kind").cloned().unwrap_or(Value::Null);
                Json(out)
            }
            "task_list" => {
                let a: TaskList = args(req)?;
                let rows = self.rpc("tasks_list", vec![json!(a.filter)]).await?;
                Json(a.page.slice(as_array(rows)))
            }
            "task_get" => {
                let a: Id = args(req)?;
                Json(self.rpc("task_get", vec![json!(a.id)]).await?)
            }
            "artifact_list" => {
                let a: ArtifactList = args(req)?;
                let private = self.private_sessions().await?;
                let rows = as_array(
                    self.rpc("list_artifacts", vec![json!(a.session), Value::Null])
                        .await?,
                )
                .into_iter()
                .filter(|r| {
                    !r.get("session")
                        .and_then(Value::as_str)
                        .is_some_and(|s| private.contains(s))
                })
                .collect();
                Json(a.page.slice(rows))
            }
            "calendar_events" => {
                let a: Window = args(req)?;
                Json(
                    self.rpc("calendar_events", vec![json!(a.from), json!(a.to)])
                        .await?,
                )
            }
            "calendar_sources" => Json(self.rpc("calendar_sources", vec![]).await?),
            // ---- writes (A2: no theuth-side gate; the client's approval prompt is the control)
            "meeting_edit" => {
                let a: MeetingEdit = args(req)?;
                Json(
                    self.rpc("meeting_edit", vec![json!(a.id), json!(a.ops)])
                        .await?,
                )
            }
            "meeting_set_project" => {
                let a: SetProject = args(req)?;
                Json(
                    self.rpc("meeting_set_project", vec![json!(a.id), json!(a.project)])
                        .await?,
                )
            }
            "meeting_set_event" => {
                let a: SetEvent = args(req)?;
                Json(
                    self.rpc("meeting_set_event", vec![json!(a.id), json!(a.event)])
                        .await?,
                )
            }
            "meeting_set_template" => {
                let a: SetTemplate = args(req)?;
                Json(
                    self.rpc("meeting_set_template", vec![json!(a.id), json!(a.template)])
                        .await?,
                )
            }
            "doc_create" => {
                let a: DocCreate = args(req)?;
                Json(
                    self.rpc("doc_create", vec![json!(a.title), json!(a.markdown)])
                        .await?,
                )
            }
            "doc_rename" => {
                let a: Rename = args(req)?;
                Json(
                    self.rpc("doc_rename", vec![json!(a.id), json!(a.title)])
                        .await?,
                )
            }
            "doc_delete" => {
                let a: Id = args(req)?;
                Json(self.rpc("doc_delete", vec![json!(a.id)]).await?)
            }
            "knowledge_put" => {
                let a: knowledge::Put = args(req)?;
                Json(self.rpc("knowledge_put", vec![json!(a)]).await?)
            }
            "knowledge_delete" => {
                let a: Id = args(req)?;
                Json(self.rpc("knowledge_delete", vec![json!(a.id)]).await?)
            }
            "task_create" => {
                let a: tasks::TaskCreate = args(req)?;
                Json(self.rpc("task_create", vec![json!(a)]).await?)
            }
            "task_update" => {
                let a: TaskUpdate = args(req)?;
                Json(
                    self.rpc("task_update", vec![json!(a.id), json!(a.patch)])
                        .await?,
                )
            }
            "task_move" => {
                let a: TaskMove = args(req)?;
                Json(
                    self.rpc(
                        "task_move",
                        vec![json!(a.id), json!(a.state), json!(a.note), json!(a.next)],
                    )
                    .await?,
                )
            }
            "task_comment" => {
                let a: TaskComment = args(req)?;
                Json(
                    self.rpc("task_comment", vec![json!(a.id), json!(a.text)])
                        .await?,
                )
            }
            "brief_put" => {
                let a: meeting::CachedBriefPut = args(req)?;
                Json(self.rpc("brief_put", vec![json!(a)]).await?)
            }
            "brief_delete" => {
                let a: Id = args(req)?;
                Json(self.rpc("brief_delete", vec![json!(a.id)]).await?)
            }
            "calendar_source_add" => {
                let a: SourceAdd = args(req)?;
                Json(
                    self.rpc("calendar_source_add", vec![json!(a.name), json!(a.url)])
                        .await?,
                )
            }
            "calendar_source_delete" => {
                let a: Id = args(req)?;
                Json(
                    self.rpc("calendar_source_delete", vec![json!(a.id)])
                        .await?,
                )
            }
            "calendar_source_refresh" => {
                let a: Id = args(req)?;
                Json(
                    self.rpc("calendar_source_refresh", vec![json!(a.id)])
                        .await?,
                )
            }
            other => {
                return Err(Error::Protocol(ErrorData::invalid_params(
                    format!("unknown tool {other}"),
                    None,
                )));
            }
        };
        Ok(v)
    }
}

/// A string result as text; anything else as JSON.
fn text(v: Value) -> String {
    match v {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Protocol errors (bad arguments, unknown tool) vs tool errors (the engine said no).
enum Error {
    Protocol(ErrorData),
    Tool(String),
}

impl From<ErrorData> for Error {
    fn from(e: ErrorData) -> Self {
        Error::Protocol(e)
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Tool(e)
    }
}

impl ServerHandler for Catalog {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let started = Instant::now();
        let r = self.dispatch(&req).await;
        let line = AuditLine {
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            transport: self.transport.to_owned(),
            client: context.client_info().map(|i| i.name).unwrap_or_default(),
            tool: req.name.to_string(),
            args_sha256: args_sha256(&Value::Object(req.arguments.clone().unwrap_or_default())),
            ok: r.is_ok(),
            ms: started.elapsed().as_millis() as u64,
        };
        self.audit.append(&line);
        let result = match r {
            Ok(Out::Json(mut v)) => {
                if let Some(vault) = &self.vault {
                    vault.redact_value(&mut v);
                }
                CallToolResult::success(vec![ContentBlock::text(
                    serde_json::to_string_pretty(&v).unwrap_or_default(),
                )])
            }
            Ok(Out::Text(s)) => {
                let s = match &self.vault {
                    Some(vault) => vault.redact(&s).into_owned(),
                    None => s,
                };
                CallToolResult::success(vec![ContentBlock::text(s)])
            }
            Err(Error::Tool(text)) => CallToolResult::error(vec![ContentBlock::text(text)]),
            Err(Error::Protocol(e)) => return Err(e),
        };
        Ok(result.into())
    }
}
