// 144-W1 owns this file.
//! Source ingest (design 144 §4): the nine origin routes, the [`IngestActor`] (hub-only,
//! `notebooks.ingest_concurrency` jobs at a time), freshness and provenance. The four functions
//! below are what the `source_*` RPCs and tools call.
//!
//! Shape: `source_add` pre-flights the origin (§4.1 end), creates the `Source` document and the
//! `sources` row (`new`), files the item and queues the job; [`job::run`] does extract → markdown →
//! `ImportMarkdown` as `Principal::Helper` → snapshot → index hook → insight hook → `completed`.
//! Reads (`source_get` / `source_list`) are [`DocumentActor`] messages served on any engine.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};

use crate::actor::{Create, DocumentActor, Lookup};
use crate::meta::{Binding, DocId, DocKind, DocSummary, Principal};
use crate::notebook::{ItemKind, NotebookItem, Source, SourceEvent, SourceOrigin, SourceStatus};
use crate::store::{SourceRow, Store};

pub mod captions;
pub mod insights;
pub(crate) mod job;
pub(crate) mod routes;

/// Registry name of the [`IngestActor`].
pub const INGEST_ACTOR: &str = "ingest";

/// The §13.2 `source_add` arguments: exactly one of `path | url | text | meeting | session | mcp`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SourceAddArgs {
    /// The notebook the source is filed in.
    pub notebook: DocId,
    /// A file on this machine (PDF, document, audio, video, …); becomes `File | Audio | Video` by extension.
    #[serde(default)]
    pub path: Option<String>,
    /// A web page or a YouTube URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Pasted text.
    #[serde(default)]
    pub text: Option<String>,
    /// A meeting id (its transcript).
    #[serde(default)]
    pub meeting: Option<String>,
    /// A past session id (its transcript).
    #[serde(default)]
    pub session: Option<String>,
    /// An MCP tool call (Confluence page, Jira issue, …).
    #[serde(default)]
    pub mcp: Option<McpRef>,
    /// Title override; default from the origin (file name, page title, meeting title, first line).
    #[serde(default)]
    pub title: Option<String>,
}

/// One MCP call a source is fetched by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpRef {
    /// MCP server name.
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// The call's arguments.
    pub args: serde_json::Value,
}

/// `notebooks.youtube_route` (§4.1.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum YoutubeRouteCfg {
    /// Captions, then audio on failure.
    #[default]
    Auto,
    /// Captions only.
    Captions,
    /// `yt-dlp` + STT only.
    Audio,
}

/// `notebooks.mcp` (§14): registered MCP tool names; `None` hides the "+ Add" entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpTools {
    /// The Confluence page reader.
    pub confluence_tool: Option<String>,
    /// The Jira issue reader.
    pub jira_tool: Option<String>,
}

/// `notebooks:` config section (§14), the ingest keys. W2/W3 add theirs here (`serde(default)`:
/// unknown keys are ignored, missing ones take the default).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebooksConfig {
    /// Cap on an original (file bytes, fetched body); over → `too large`.
    pub max_source_bytes: u64,
    /// Cap on the derived markdown (§4.2 W-e); over → the first N chars and a notice block.
    pub max_source_chars: usize,
    /// Jobs running at once.
    pub ingest_concurrency: usize,
    /// YouTube: `auto | captions | audio`.
    pub youtube_route: YoutubeRouteCfg,
    /// Caption languages in preference order; none match → the first track.
    pub youtube_langs: Vec<String>,
    /// `yt-dlp` path; `None` = `yt-dlp` on `PATH`.
    pub youtube_ytdlp: Option<String>,
    /// Registered MCP tool names for Confluence / Jira.
    pub mcp: McpTools,
    /// §4.3 (E1): `provider/model` for the insight side calls; `None` = `agent.title_model` → the session model.
    pub insight_model: Option<String>,
    /// One Dense summary per source at the end of ingest.
    pub insight_default: bool,
    /// How much of the source the insight call reads (tokens ≈ chars / 4).
    pub insight_input_tokens: u32,
}

impl Default for NotebooksConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 25 * 1024 * 1024,
            max_source_chars: 2_000_000,
            ingest_concurrency: 2,
            youtube_route: YoutubeRouteCfg::Auto,
            youtube_langs: vec!["en".into()],
            youtube_ytdlp: None,
            mcp: McpTools::default(),
            insight_model: None,
            insight_default: true,
            insight_input_tokens: 24_000,
        }
    }
}

/// What a route hands back (§4.1 "stored"): the markdown and whatever the origin knows about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extracted {
    /// GFM; headings per page / section / segment.
    pub markdown: String,
    /// The origin's own title (page `<title>`, meeting title, `# KEY summary`); `None` keeps the card's.
    pub title: Option<String>,
    /// Route-specific provenance (`PageMeta`, `segments`, `turns`, `url/space/version`, …).
    pub meta: serde_json::Value,
    /// Original size when there is one.
    pub bytes: Option<u64>,
    /// PDF pages when known.
    pub pages: Option<u32>,
    /// Media length when known.
    pub duration_ms: Option<u64>,
    /// Attachment path of the original on the hub (files, `yt-dlp` audio).
    pub blob: Option<String>,
    /// The route rewrote the origin (YouTube fell back to audio); the key never changes.
    pub origin: Option<SourceOrigin>,
}

/// Permanent pre-flight / extraction refusals (§4.1 end): never retried by a job.
pub const UNSUPPORTED: &str = "415 unsupported";
/// Over `notebooks.max_source_bytes`.
pub const TOO_LARGE: &str = "too large";
/// The fetcher's SSRF guard said no.
pub const REFUSED: &str = "refused: private address";
/// YouTube with neither captions nor `yt-dlp`.
pub const NO_ROUTE: &str = "no transcript route";

/// `true` for an error a retry cannot fix (§4.2: `415 unsupported`, `too large`, `refused` and the
/// converter's own verdicts); everything else is transient (network, deadline, a model not loaded).
pub fn permanent(error: &str) -> bool {
    [UNSUPPORTED, TOO_LARGE, REFUSED, NO_ROUTE]
        .iter()
        .any(|p| error.starts_with(p))
        || error.contains("need OCR")
        || error.contains("no audio track")
        || error.contains("no decodable audio")
}

/// `mm:ss` (hours fold into minutes, as the meeting crate does).
pub fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Whitespace-separated tokens, the `words` column.
pub fn word_count(markdown: &str) -> u32 {
    markdown.split_whitespace().count() as u32
}

/// §4.2 W-e: keep the first `max` chars and end with a visible notice block.
pub fn cap_chars(mut markdown: String, max: usize) -> String {
    let Some((cut, _)) = markdown.char_indices().nth(max) else {
        return markdown;
    };
    markdown.truncate(cut);
    markdown.push_str(&format!(
        "\n\n> **Notice:** this source was cut at {max} characters (`notebooks.max_source_chars`); \
         the rest is not here — split the file to keep it.\n"
    ));
    markdown
}

/// `## [mm:ss]` blocks from timed words, one per ~`every_ms` (§4.1 YouTube / audio rows).
pub fn timed_blocks(words: impl IntoIterator<Item = (u64, String)>, every_ms: u64) -> String {
    let mut out = String::new();
    let mut block_start: Option<u64> = None;
    let mut text = String::new();
    let flush = |out: &mut String, start: u64, text: &mut String| {
        if !text.trim().is_empty() {
            out.push_str(&format!("## [{}]\n\n{}\n\n", mmss(start), text.trim()));
        }
        text.clear();
    };
    for (t0, w) in words {
        match block_start {
            Some(s) if t0 < s + every_ms => {}
            Some(s) => {
                flush(&mut out, s, &mut text);
                block_start = Some(t0);
            }
            None => block_start = Some(t0),
        }
        if !text.is_empty() && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(w.trim());
    }
    if let Some(s) = block_start {
        flush(&mut out, s, &mut text);
    }
    out
}

/// `<data_dir>/attachments/<sha16>.<ext>` — the store `put_attachment` writes to (`api.rs` `save_blob`);
/// the document crate cannot import `agent`, so the three lines live here too.
pub fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    let hex = crate::store::sha256(bytes);
    let dir = data_dir.join("attachments");
    let path = dir.join(format!("{}.{ext}", &hex[..16]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

/// The title a card wears before its route has run; the job renames the document when the origin
/// says better and the title is still this one (a caller's `title` is never overwritten).
pub fn provisional_title(origin: &SourceOrigin) -> String {
    match origin {
        SourceOrigin::File { name, .. }
        | SourceOrigin::Audio { name, .. }
        | SourceOrigin::Video { name, .. } => name.clone(),
        SourceOrigin::Url { url, .. } => url.clone(),
        SourceOrigin::Meeting { meeting } => format!("Meeting {meeting}"),
        SourceOrigin::Text { title, .. } => title.clone(),
        SourceOrigin::Session { session } => format!("Chat {session}"),
        SourceOrigin::Mcp { tool, args, .. } => {
            let arg = args
                .as_object()
                .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_owned)))
                .unwrap_or_default();
            format!("{tool} {arg}").trim().to_owned()
        }
        SourceOrigin::Youtube { video, .. } => format!("YouTube {video}"),
    }
}

/// `added_by` of the item row (§3.3): `owner | agent:<session> | member:<pub8>`.
fn added_by(by: &Principal) -> String {
    match by {
        Principal::Owner { .. } | Principal::Helper | Principal::File => "owner".into(),
        Principal::Agent { session } => format!("agent:{session}"),
        Principal::Member { pub_, .. } => {
            format!("member:{}", pub_.chars().take(8).collect::<String>())
        }
    }
}

/// `sources` row + `docs` summary → the wire shape.
pub(crate) fn project(row: SourceRow, doc: DocSummary, progress: Option<u8>) -> Source {
    Source {
        doc,
        origin: row.origin,
        status: row.status,
        progress,
        error: row.error,
        attempts: row.attempts,
        fetched_ms: row.fetched_ms,
        content_sha: row.content_sha,
        changed_since: row.changed_since,
        bytes: row.bytes,
        words: row.words,
        duration_ms: row.duration_ms,
        pages: row.pages,
        insights: row.insights,
        meta: row.meta,
    }
}

pub(crate) async fn summaries(store: &Store) -> anyhow::Result<HashMap<DocId, DocSummary>> {
    Ok(store
        .list(Some(DocKind::Source), u32::MAX)
        .await?
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect())
}

// ---- Reads: DocumentActor messages, any engine (§3.5) ------------------------------------------

/// `ask` → `Result<Source, String>`.
#[derive(Clone, Debug)]
pub struct SourceGet(pub DocId);

impl Message<SourceGet> for DocumentActor {
    type Reply = Result<Source, String>;

    async fn handle(&mut self, m: SourceGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let row = self
            .store
            .source_row(&m.0)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("no such source: {}", m.0))?;
        let doc = summaries(&self.store)
            .await
            .map_err(|e| format!("{e:#}"))?
            .remove(&m.0)
            .ok_or_else(|| format!("source document missing: {}", m.0))?;
        Ok(project(row, doc, None))
    }
}

/// `ask` → `Result<Vec<Source>, String>`: a notebook's sources or every live one.
#[derive(Clone, Debug)]
pub struct SourceList(pub Option<DocId>);

impl Message<SourceList> for DocumentActor {
    type Reply = Result<Vec<Source>, String>;

    async fn handle(&mut self, m: SourceList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rows = self
            .store
            .source_rows(m.0.as_ref())
            .await
            .map_err(|e| format!("{e:#}"))?;
        let mut docs = summaries(&self.store).await.map_err(|e| format!("{e:#}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| docs.remove(&r.id).map(|d| project(r, d, None)))
            .collect())
    }
}

/// `ask` → `Result<(), String>`: `Err("NotHub:<id>")` off the hub (69 Y). The ingest actor asks before
/// it writes a row the replica carries.
#[derive(Clone, Copy, Debug)]
pub struct HubGate;

impl Message<HubGate> for DocumentActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: HubGate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.hub.gate_str()
    }
}

// ---- The job runner ----------------------------------------------------------------------------

/// What a job needs, cloned into its task.
#[derive(Clone)]
pub(crate) struct Ctx {
    pub docs: ActorRef<DocumentActor>,
    pub store: Store,
    pub rt: runtime::Runtime,
    pub cfg: Arc<NotebooksConfig>,
    pub data_dir: PathBuf,
}

/// The ingest job runner (§4.2): one per engine, hub-only by construction — every write it makes
/// goes through a hub-gated [`DocumentActor`] message or follows one.
#[derive(kameo::Actor)]
pub struct IngestActor {
    ctx: Ctx,
    sem: Arc<tokio::sync::Semaphore>,
    /// Queued or running here; `source_refresh` is refused for these (§4.2).
    active: HashSet<DocId>,
}

impl IngestActor {
    /// Spawn and register from the document plugin's start-up; reads the `notebooks` section.
    pub fn start(
        rt: &runtime::Runtime,
        store: Store,
        docs: ActorRef<DocumentActor>,
    ) -> anyhow::Result<ActorRef<Self>> {
        let cfg: NotebooksConfig = rt.section("notebooks").unwrap_or_else(|e| {
            tracing::debug!("no [notebooks] config section, using defaults: {e}");
            NotebooksConfig::default()
        });
        Self::start_with(rt, store, docs, cfg)
    }

    /// [`Self::start`] with an explicit config (tests).
    pub fn start_with(
        rt: &runtime::Runtime,
        store: Store,
        docs: ActorRef<DocumentActor>,
        cfg: NotebooksConfig,
    ) -> anyhow::Result<ActorRef<Self>> {
        let sem = Arc::new(tokio::sync::Semaphore::new(cfg.ingest_concurrency.max(1)));
        let actor = Self::spawn(Self {
            ctx: Ctx {
                docs,
                store,
                data_dir: rt.data_dir.clone(),
                rt: rt.clone(),
                cfg: Arc::new(cfg),
            },
            sem,
            active: HashSet::new(),
        });
        // A restart replaces a stale registration (tests spin the plugin up more than once).
        if let Ok(Some(_)) = ActorRef::<Self>::lookup(INGEST_ACTOR) {
            kameo::registry::ACTOR_REGISTRY
                .lock()
                .unwrap()
                .remove(INGEST_ACTOR);
        }
        actor.register(INGEST_ACTOR)?;
        Ok(actor)
    }

    fn enqueue(&mut self, me: ActorRef<Self>, id: DocId, refresh: bool) {
        self.active.insert(id.clone());
        let (ctx, sem) = (self.ctx.clone(), self.sem.clone());
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            job::run(&ctx, &id, refresh).await;
            let _ = me.tell(Done(id)).await;
        });
    }

    async fn gate(&self) -> Result<(), String> {
        flat(self.ctx.docs.ask(HubGate).await)
    }

    async fn get(&self, id: &DocId) -> Result<Source, String> {
        let mut s = flat(self.ctx.docs.ask(SourceGet(id.clone())).await)?;
        if self.active.contains(id) && s.status == SourceStatus::Completed {
            // The row is the last run's; the card is queued again.
            s.status = SourceStatus::Queued;
        }
        Ok(s)
    }
}

/// `ask` → `Result<Source, String>`: pre-flight, document + row + item, queue.
#[derive(Clone, Debug)]
pub struct Add {
    /// The arguments.
    pub args: SourceAddArgs,
    /// Who adds (the item row's `added_by`).
    pub by: Principal,
    /// The adder's `tools::egress::lan()`, read in its own task: the URL pre-check may pass a
    /// local address only with it (16 §5, amended 2026-11-11).
    pub lan: bool,
}

impl Message<Add> for IngestActor {
    type Reply = Result<Source, String>;

    async fn handle(&mut self, m: Add, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.gate().await?;
        let c = self.ctx.clone();
        let args = m.args;
        if c.store
            .notebook_row(&args.notebook)
            .await
            .map_err(|e| format!("{e:#}"))?
            .is_none()
        {
            return Err(format!("no such notebook: {}", args.notebook));
        }
        // Pre-flight (§4.1 end): the origin and, when it cannot work, the permanent reason.
        let (origin, refusal) = routes::preflight(&c, &args, m.lan)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let key = origin.key();
        let title = args
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| provisional_title(&origin));
        let binding = Binding::Source(key);
        let existing = flat(
            c.docs
                .ask(Lookup {
                    binding: binding.clone(),
                })
                .await,
        )?;
        let (id, fresh) = match existing {
            Some(id) => (id, false),
            None => {
                let doc = flat(
                    c.docs
                        .ask(Create {
                            binding,
                            title: title.clone(),
                            markdown: String::new(),
                            by: m.by.clone(),
                            home: crate::actor::Homing::Hub,
                        })
                        .await,
                )?;
                (doc.id, true)
            }
        };
        let mut row = match c
            .store
            .source_row(&id)
            .await
            .map_err(|e| format!("{e:#}"))?
        {
            Some(r) if !fresh => r,
            _ => SourceRow {
                id: id.clone(),
                origin: origin.clone(),
                status: SourceStatus::New,
                error: None,
                attempts: 0,
                blob: None,
                fetched_ms: None,
                content_sha: None,
                changed_since: None,
                bytes: None,
                words: None,
                duration_ms: None,
                pages: None,
                meta: serde_json::Value::Null,
                insights: 0,
            },
        };
        let queue = match (&refusal, row.status) {
            (Some(why), _) => {
                row.status = SourceStatus::Failed;
                row.error = Some(why.clone());
                false
            }
            // A card the user adds again after a failure runs again; a finished one is reused (§3.1 dedupe).
            (None, SourceStatus::Completed) => false,
            (None, _) => !self.active.contains(&id),
        };
        if row.blob.is_none() {
            row.blob = match (&origin, &args.text) {
                (SourceOrigin::Text { .. }, Some(text)) => Some(
                    save_blob(text.as_bytes(), "md", &c.data_dir)
                        .map_err(|e| format!("save pasted text: {e}"))?
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    SourceOrigin::File { .. }
                    | SourceOrigin::Audio { .. }
                    | SourceOrigin::Video { .. },
                    _,
                ) => args.path.clone(),
                _ => None,
            };
        }
        c.store
            .source_upsert(&row)
            .await
            .map_err(|e| format!("{e:#}"))?;
        c.store
            .item_add(&NotebookItem {
                notebook: args.notebook.clone(),
                kind: ItemKind::Source,
                item: id.to_string(),
                added_ms: crate::store::now_ms(),
                added_by: added_by(&m.by),
                removed_ms: None,
            })
            .await
            .map_err(|e| format!("{e:#}"))?;
        if queue {
            c.store
                .source_set_status(&id, SourceStatus::Queued, None, row.attempts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            self.enqueue(ctx.actor_ref().clone(), id.clone(), false);
        }
        c.rt.publish(SourceEvent::Changed {
            source: id.clone(),
            status: if queue {
                SourceStatus::Queued
            } else {
                row.status
            },
        })
        .await;
        self.get(&id).await
    }
}

/// `ask` → `Result<Source, String>`: re-run from the origin or the blob; refused while queued or running.
#[derive(Clone, Debug)]
pub struct Refresh {
    /// The source.
    pub id: DocId,
}

impl Message<Refresh> for IngestActor {
    type Reply = Result<Source, String>;

    async fn handle(&mut self, m: Refresh, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.gate().await?;
        if self.active.contains(&m.id) {
            return Err(format!("source {} is already queued or running", m.id));
        }
        let row = self
            .ctx
            .store
            .source_row(&m.id)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("no such source: {}", m.id))?;
        self.ctx
            .store
            .source_set_status(&m.id, SourceStatus::Queued, None, row.attempts)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.enqueue(ctx.actor_ref().clone(), m.id.clone(), true);
        self.ctx
            .rt
            .publish(SourceEvent::Changed {
                source: m.id.clone(),
                status: SourceStatus::Queued,
            })
            .await;
        self.get(&m.id).await
    }
}

/// `tell`: a job left the runner.
#[derive(Clone, Debug)]
pub struct Done(pub DocId);

impl Message<Done> for IngestActor {
    type Reply = ();

    async fn handle(&mut self, m: Done, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.active.remove(&m.0);
    }
}

/// `ask` → `Vec<DocId>`: what is queued or running (tests, the source page).
#[derive(Clone, Copy, Debug)]
pub struct Active;

impl Message<Active> for IngestActor {
    type Reply = Vec<DocId>;

    async fn handle(&mut self, _: Active, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.active.iter().cloned().collect()
    }
}

/// A handler's `Err(String)` verbatim (a `NotHub:` refusal included); a dead actor as text.
fn flat<M, T>(r: Result<T, kameo::error::SendError<M, String>>) -> Result<T, String>
where
    kameo::error::SendError<M, String>: std::fmt::Display,
{
    r.map_err(flat_err)
}

/// The text of a failed `ask` whose handler errs with a `String`.
pub(crate) fn flat_err<M>(e: kameo::error::SendError<M, String>) -> String
where
    kameo::error::SendError<M, String>: std::fmt::Display,
{
    match e {
        kameo::error::SendError::HandlerError(e) => e,
        e => e.to_string(),
    }
}

fn ingest() -> anyhow::Result<ActorRef<IngestActor>> {
    match ActorRef::<IngestActor>::lookup(INGEST_ACTOR) {
        Ok(Some(a)) => Ok(a),
        Ok(None) => Err(anyhow::anyhow!("ingest not running")),
        Err(e) => Err(anyhow::anyhow!("ingest lookup: {e}")),
    }
}

// ---- The four entry points -----------------------------------------------------------------------

/// Pre-flight the origin, write the `sources` row (`new`) and the `Source` document, file it in the
/// notebook, queue the job; answers at once with the card. Hub only (`NotHub` off the hub). A URL on
/// the local network passes only inside the caller's `tools::egress` grant (an owner's tool call, the
/// owner's RPC) and with `web.allow_private_network`; otherwise it is refused as private.
pub async fn source_add(
    _actor: &ActorRef<DocumentActor>,
    args: SourceAddArgs,
    by: Principal,
) -> anyhow::Result<Source> {
    let lan = tools::egress::lan();
    flat(ingest()?.ask(Add { args, by, lan }).await).map_err(|e| anyhow::anyhow!(e))
}

/// One source with its ingest state. Any engine.
pub async fn source_get(actor: &ActorRef<DocumentActor>, id: DocId) -> anyhow::Result<Source> {
    flat(actor.ask(SourceGet(id)).await).map_err(|e| anyhow::anyhow!(e))
}

/// Sources of `notebook`, or every source (unfiled included). Any engine.
pub async fn source_list(
    actor: &ActorRef<DocumentActor>,
    notebook: Option<DocId>,
) -> anyhow::Result<Vec<Source>> {
    flat(actor.ask(SourceList(notebook)).await).map_err(|e| anyhow::anyhow!(e))
}

/// Re-run the job from the origin or the blob (§4.2); refused while `running | queued`. Hub only.
pub async fn source_refresh(
    _actor: &ActorRef<DocumentActor>,
    id: DocId,
    _by: Principal,
) -> anyhow::Result<Source> {
    flat(ingest()?.ask(Refresh { id }).await).map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_keeps_the_first_n_chars_and_says_so() {
        let s = cap_chars("héllo wörld".into(), 5);
        assert!(s.starts_with("héllo\n\n> **Notice:**"), "{s}");
        assert!(s.contains("notebooks.max_source_chars"));
        assert_eq!(cap_chars("short".into(), 5), "short");
    }

    #[test]
    fn timed_blocks_group_by_window_and_stamp_the_first_word() {
        let words = [
            (0, "a"),
            (30_000, "b"),
            (59_999, "c"),
            (60_000, "d"),
            (130_000, "e"),
        ]
        .map(|(t, w)| (t, w.to_string()));
        let md = timed_blocks(words, 60_000);
        assert_eq!(
            md,
            "## [00:00]\n\na b c\n\n## [01:00]\n\nd\n\n## [02:10]\n\ne\n\n"
        );
    }

    #[test]
    fn permanent_errors_are_the_preflight_words_and_the_converter_verdicts() {
        assert!(permanent("415 unsupported: xyz"));
        assert!(permanent("too large: 30 MB over 25 MB"));
        assert!(permanent("refused: private address"));
        assert!(permanent(
            "no transcript route: set notebooks.youtube_ytdlp"
        ));
        assert!(permanent("pages 3, 4 of 12 are scanned (need OCR)"));
        assert!(!permanent("fetch timed out after 8 s"));
    }

    #[test]
    fn config_defaults_follow_section_14() {
        let c = NotebooksConfig::default();
        assert_eq!(
            (c.max_source_bytes, c.max_source_chars, c.ingest_concurrency),
            (26_214_400, 2_000_000, 2)
        );
        assert_eq!(c.youtube_langs, ["en"]);
        let parsed: NotebooksConfig =
            serde_json::from_str(r#"{"youtube_route":"captions","mcp":{"jira_tool":"jira_get_issue"},"embedding_model":"x"}"#)
                .unwrap();
        assert_eq!(parsed.youtube_route, YoutubeRouteCfg::Captions);
        assert_eq!(parsed.mcp.jira_tool.as_deref(), Some("jira_get_issue"));
    }
}
