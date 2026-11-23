//! The `knowledge_*` tools (design doc 10 §6), registered by
//! `KnowledgePlugin::start` once the tool registry is up (12 §9).

use std::path::PathBuf;
use std::sync::Arc;

use decision::classify::{BoolItem, BoolVerdict};
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::KnowledgeConfig;
use crate::store::Knowledge;
use crate::types::{AgentScope, Concept, Meta, Put, Source, Visibility};

/// Name of [`GetTool`]; the loop counts its calls for harvest cadence (10 §5.1).
pub const GET: &str = "knowledge_get";

/// Name of [`PeekTool`]; the loop collects its relevant ids for the compaction head (212 §4.3).
pub const PEEK: &str = "peek_memory";

/// The plugin's system-prompt block (07 §4 part 7, 10 §4.3): when to reach for
/// the knowledge base. Static, so it never busts the system cache.
pub const PROMPT: &str = "Durable facts about the user's machines, projects, accounts and decisions belong in the knowledge base: \
check <knowledge> hints and `knowledge_search()` before assuming, record with `knowledge_put()` (both python). \
Before reading hinted concepts you are unsure of, `peek_memory(question, ids)` (python) asks the local model which of them answer your question and returns only those in full.";

/// Register every `knowledge_*` tool, `peek_memory` and the prompt block on `reg`.
pub async fn register(
    reg: &ActorRef<ToolRegistry>,
    knowledge: Arc<Knowledge>,
    cfg: &KnowledgeConfig,
) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "knowledge",
            text: PROMPT.to_owned(),
        })
        .await;
    let k = &knowledge;
    let _ = reg.ask(RegisterTool(Arc::new(SearchTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(GetTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ListTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(PutTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ShareTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DeleteTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(HistoryTool(k.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(RestoreTool(k.clone()))))
        .await;
    // 136 §2.1: usage counters and the five consolidation ops (bodies W1's).
    let _ = reg.ask(RegisterTool(Arc::new(StatsTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(MergeTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SupersedeTool(k.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(ArchiveTool(k.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(VerifyTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(RetitleTool(k.clone()))))
        .await;
    // 212: check memories against a question with the local decision model.
    let peek = PeekTool {
        knowledge: k.clone(),
        judge: Arc::new(DecisionJudge {
            checkpoint: cfg.peek_checkpoint.clone(),
        }),
        threshold: cfg.peek_threshold,
    };
    let _ = reg.ask(RegisterTool(Arc::new(peek))).await;
}

/// Text for the transcript, the same data as a Python value for the runtime.
fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// 51 §5: a private session leaves no trace in the durable bundle.
fn writable(cx: &ToolCx) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cx.resources.is_private(),
        "knowledge mutations are disabled in a private session"
    );
    Ok(())
}

/// 136 §2.1: the five consolidation tools run only from the dreaming
/// routine's session or the owner's own root session (plus [`writable`]).
async fn consolidating(k: &Knowledge, name: &str, cx: &ToolCx) -> anyhow::Result<()> {
    writable(cx)?;
    anyhow::ensure!(
        k.may_consolidate(cx.session).await,
        "{name} runs only in the dreaming routine's session or the owner's own root session (136 §2.1)"
    );
    Ok(())
}

fn meta_line(s: &mut String, m: &Meta) {
    s.push_str(&format!("`{}` — {} [{}]", m.id, m.title, m.type_));
    if !m.description.is_empty() {
        s.push_str(": ");
        s.push_str(&m.description);
    }
}

/// Arguments for [`SearchTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Free-text query; words are stemmed and OR-ed.
    pub query: String,
    /// Max hits (default 10, max 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_search`: full-text search over the bundle.
pub struct SearchTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for SearchTool {
    const NAME: &'static str = "knowledge_search";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SearchArgs;

    fn description(&self) -> String {
        "Search the knowledge base (durable facts about the user's environment, decisions, procedures, \
         entities) by keywords. Returns ids to pass to knowledge_get."
            .into()
    }

    async fn call(&self, a: SearchArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        let limit = a.limit.unwrap_or(10).clamp(1, 50);
        let hits = k
            .search_for(&a.query, &cwd(), limit, Some(cx.session))
            .await?;
        let mut s = String::new();
        let mut rows = Vec::with_capacity(hits.len());
        for h in &hits {
            // 136 §3.1: archived (and other non-live) items stay searchable, marked.
            let mark = if h.meta.status.is_live() {
                String::new()
            } else {
                format!(" [{}]", h.meta.status.as_str())
            };
            s.push_str(&format!(
                "`{}` — {} [{}]{mark} ({:.1}): {}\n",
                h.meta.id, h.meta.title, h.meta.type_, h.score, h.snippet
            ));
            // Frontmatter fields plus score and snippet, flat.
            let mut row = serde_json::to_value(&h.meta)?;
            row["score"] = h.score.into();
            row["snippet"] = h.snippet.as_str().into();
            rows.push(row);
        }
        if s.is_empty() {
            s.push_str("no matches");
        }
        Ok(structured(s, rows))
    }
}

/// Arguments naming one concept.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IdArgs {
    /// Concept id (`dir/slug`); `dir/slug@<commit>` reads a past revision (read-only).
    pub id: String,
}

/// `knowledge_get`: one concept verbatim with its links and backlinks.
pub struct GetTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for GetTool {
    const NAME: &'static str = GET;
    const RISK: Risk = Risk::ReadOnly;
    type Args = IdArgs;

    fn description(&self) -> String {
        "Read a knowledge concept by id: the file verbatim (frontmatter incl. source/timestamp, body), \
         its links (→ missing when the target does not exist) and the concepts citing it. \
         `<id>@<commit>` (as `search` prints it) reads that past revision, read-only."
            .into()
    }

    async fn call(&self, a: IdArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        if let Some((id, commit)) = a.id.split_once('@') {
            let c = k.read_revision(id, commit).await?;
            let date = c.time.get(..10).unwrap_or(&c.time);
            return Ok(ToolOutput::ok(format!(
                "revision {} of {date} (read-only; latest: knowledge_get(\"{}\"))\n{}",
                &c.commit[..7.min(c.commit.len())],
                c.id,
                c.text
            )));
        }
        let c = k.get(&a.id).await?;
        record_open(&k, &c.meta.id, cx.session).await;
        Ok(ToolOutput::ok(render_concept(c)))
    }
}

/// 136 §5.3: every read is a vote for the hint shelf — except the night's own
/// (R-28): Tidy reads hundreds of entries and would make `opened == 0`
/// unreachable for anything it touched.
async fn record_open(k: &Knowledge, id: &str, session: runtime::SessionId) {
    if !k.is_consolidating_session(session)
        && let Err(e) = k.record_opened(id, chrono::Utc::now()).await
    {
        tracing::debug!(id = %id, "record_opened: {e:#}");
    }
}

/// What `knowledge_get` returns for a concept, shared with `peek_memory`: the
/// file verbatim, then the `Links:` and `Cited by:` lines, each only when non-empty.
fn render_concept(c: Concept) -> String {
    let mut s = c.text;
    if !c.links.is_empty() {
        s.push_str("\nLinks: ");
        let links: Vec<String> = c
            .links
            .iter()
            .map(|(t, ok)| {
                if *ok {
                    t.clone()
                } else {
                    format!("{t} → missing")
                }
            })
            .collect();
        s.push_str(&links.join(", "));
        s.push('\n');
    }
    if !c.cited_by.is_empty() {
        s.push_str("Cited by: ");
        s.push_str(&c.cited_by.join(", "));
        s.push('\n');
    }
    s
}

/// Max chars of a `peek_memory` question (212 §3).
const PEEK_QUESTION_CHARS: usize = 500;
/// Max distinct ids per `peek_memory` call.
const PEEK_MAX_IDS: usize = 12;
/// Bytes of a memory the judge reads (title, description, body).
const PEEK_STATE_BYTES: usize = 4096;
/// Question chars quoted in the receipt's headline.
const PEEK_HEADLINE_CHARS: usize = 80;
/// Decision family the judge runs under (an unconfigured `tool.<x>` behaves as `tool`).
const PEEK_FAMILY: &str = "tool.memory";
/// Per-chunk budget of the judge.
const PEEK_BUDGET_MS: u64 = 3000;

/// Judges memories yes/no against a question (212 §3); the test seam of [`PeekTool`].
#[async_trait::async_trait]
pub trait MemoryJudge: Send + Sync + 'static {
    /// One verdict per item, in input order.
    async fn judge(&self, items: Vec<BoolItem>) -> anyhow::Result<Vec<BoolVerdict>>;
}

/// The real judge: the local decision model's `Bool` question.
pub struct DecisionJudge {
    /// Decision checkpoint (`knowledge.peek_checkpoint`).
    pub checkpoint: String,
}

#[async_trait::async_trait]
impl MemoryJudge for DecisionJudge {
    async fn judge(&self, items: Vec<BoolItem>) -> anyhow::Result<Vec<BoolVerdict>> {
        decision::classify::classify_bools(&self.checkpoint, PEEK_FAMILY, items, PEEK_BUDGET_MS)
            .await
    }
}

/// Arguments for [`PeekTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeekArgs {
    /// What you want to know (1–500 characters).
    pub question: String,
    /// Concept ids from a `<knowledge>` hint or `knowledge_search()` (1–12).
    pub ids: Vec<String>,
}

/// `peek_memory` (212): the local decision model filters memories against a
/// question; only the relevant ones come back in full.
pub struct PeekTool {
    /// The bundle.
    pub knowledge: Arc<Knowledge>,
    /// Who answers "does this memory help".
    pub judge: Arc<dyn MemoryJudge>,
    /// Minimum P(true) for "relevant" (`knowledge.peek_threshold`).
    pub threshold: f32,
}

/// One requested id's outcome.
enum Peeked {
    /// Absent, or not visible to this session (same text: never reveal existence).
    Missing,
    /// An `@commit` revision.
    Revision,
    /// Read and judged.
    Judged(Box<Concept>, Verdict),
}

enum Verdict {
    Relevant(f32),
    NotRelevant(Option<f32>),
    Undecided(String),
}

impl Verdict {
    fn of(v: &BoolVerdict, threshold: f32) -> Self {
        if let Some(e) = &v.error {
            return Self::Undecided(e.clone());
        }
        match (v.choice, v.p_true) {
            (Some(true), Some(p)) if p >= threshold => Self::Relevant(p),
            (Some(_), p) => Self::NotRelevant(p),
            (None, _) => Self::Undecided("no answer".into()),
        }
    }
}

/// The judge's view of a memory: title, description and body (no
/// frontmatter), cut to [`PEEK_STATE_BYTES`].
fn peek_state(c: &Concept) -> String {
    let body = okf::Document::parse(&c.text).map_or_else(|_| c.text.clone(), |d| d.body);
    let s = format!(
        "{}\n{}\n\n{}",
        c.meta.title,
        c.meta.description,
        body.trim()
    );
    platform::text::cut_bytes(&s, PEEK_STATE_BYTES).to_owned()
}

/// The ids a `peek_memory` receipt returned in full (its `` ### `id` — relevant `` lines),
/// for the loop's compaction head (212 §4.3).
pub fn peek_relevant_ids(receipt: &str) -> impl Iterator<Item = &str> {
    receipt.lines().filter_map(|l| {
        l.strip_prefix("### `")?
            .split_once("` — relevant")
            .map(|(id, _)| id)
    })
}

#[async_trait::async_trait]
impl Tool for PeekTool {
    const NAME: &'static str = PEEK;
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "str";
    const CONSUMES_SECRETS: bool = false;
    type Args = PeekArgs;

    fn description(&self) -> String {
        "Check memories against a question before pulling them into context: the local decision model \
         reads each named concept and answers yes/no \"does this help answer the question\"; only the \
         relevant ones come back in full, the rest as one line each. Use it on <knowledge> hint ids or \
         knowledge_search() results when you are not sure a memory is worth reading; use \
         knowledge_get(id) when you already know you need it."
            .into()
    }

    async fn call(&self, a: PeekArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let question = a.question.trim();
        anyhow::ensure!(
            (1..=PEEK_QUESTION_CHARS).contains(&question.chars().count()),
            "peek_memory: question must be 1–500 characters"
        );
        let mut ids: Vec<&str> = Vec::with_capacity(a.ids.len());
        for id in a.ids.iter().map(|i| i.trim()) {
            if !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
        anyhow::ensure!(
            (1..=PEEK_MAX_IDS).contains(&ids.len()),
            "peek_memory: give 1–12 ids"
        );
        let k = self.knowledge.for_session(cx.session).await?;
        let scope = k.scope_for(Some(cx.session));

        // Read what this session may see; everything else is `missing`.
        let mut found: Vec<Option<Concept>> = Vec::with_capacity(ids.len());
        let mut items = Vec::new();
        for id in &ids {
            let c = if id.contains('@') {
                None
            } else {
                k.get(id)
                    .await
                    .ok()
                    .filter(|c| AgentScope::sees(scope.as_ref(), &c.meta.owner, c.meta.visibility))
            };
            if let Some(c) = &c {
                items.push(BoolItem {
                    statement: format!("This memory helps answer the question: {question}"),
                    state: peek_state(c),
                });
            }
            found.push(c);
        }

        // Fail closed: a row without an answer is undecided, never pulled in.
        let n = items.len();
        let answers = if items.is_empty() {
            Ok(Vec::new())
        } else {
            self.judge.judge(items).await
        };
        let verdicts: Vec<Verdict> = match answers {
            Ok(v) => v.iter().map(|v| Verdict::of(v, self.threshold)).collect(),
            Err(e) => {
                let e = format!("{e:#}");
                (0..n).map(|_| Verdict::Undecided(e.clone())).collect()
            }
        };
        let mut verdicts = verdicts.into_iter();
        let rows: Vec<(&str, Peeked)> = ids
            .iter()
            .zip(found)
            .map(|(id, c)| {
                let row = match c {
                    Some(c) => Peeked::Judged(
                        Box::new(c),
                        verdicts
                            .next()
                            .unwrap_or_else(|| Verdict::Undecided("no answer".into())),
                    ),
                    None if id.contains('@') => Peeked::Revision,
                    None => Peeked::Missing,
                };
                (*id, row)
            })
            .collect();

        let (mut judged, mut relevant, mut undecided) = (0, 0, 0);
        for (_, row) in &rows {
            if let Peeked::Judged(_, v) = row {
                judged += 1;
                match v {
                    Verdict::Relevant(_) => relevant += 1,
                    Verdict::Undecided(_) => undecided += 1,
                    Verdict::NotRelevant(_) => {}
                }
            }
        }
        let unavailable = judged > 0 && undecided == judged;

        let mut s = format!(
            "peek_memory: {relevant} of {} relevant to \"{}\"\n\n",
            rows.len(),
            platform::text::clip_then_ellipsis(question, PEEK_HEADLINE_CHARS)
        );
        if relevant == 0 {
            s.push_str("No memory was judged relevant.\n");
        }
        let mut rest = String::new();
        let mut first = true;
        for (id, row) in rows {
            match row {
                Peeked::Judged(c, Verdict::Relevant(p)) => {
                    record_open(&k, &c.meta.id, cx.session).await;
                    if !first {
                        s.push('\n');
                    }
                    first = false;
                    s.push_str(&format!("### `{id}` — relevant (p={p:.2})\n"));
                    s.push_str(&render_concept(*c));
                    if !s.ends_with('\n') {
                        s.push('\n');
                    }
                }
                Peeked::Judged(c, Verdict::NotRelevant(p)) => {
                    rest.push_str(&format!("- `{id}` — {} — not relevant", c.meta.title));
                    if let Some(p) = p {
                        rest.push_str(&format!(" (p={p:.2})"));
                    }
                    rest.push('\n');
                }
                Peeked::Judged(c, Verdict::Undecided(e)) => rest.push_str(&format!(
                    "- `{id}` — {} — undecided ({}); knowledge_get(id) reads it anyway\n",
                    c.meta.title,
                    e.replace('\n', " ")
                )),
                Peeked::Revision => rest.push_str(&format!(
                    "- `{id}` — not peekable: revisions are read with knowledge_get\n"
                )),
                Peeked::Missing => rest.push_str(&format!("- `{id}` — missing\n")),
            }
        }
        if !rest.is_empty() {
            s.push_str("\nNot pulled in:\n");
            s.push_str(&rest);
        }
        if unavailable {
            s.push_str(
                "The local decision model is unavailable; read with knowledge_get(id) instead.\n",
            );
        }
        Ok(ToolOutput::ok(s))
    }
}

/// Arguments for [`ListTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListArgs {
    /// Id prefix (`facts/`).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Frontmatter `type` (fact | entity | decision | procedure | project).
    #[serde(default, rename = "type")]
    pub type_: Option<String>,
    /// A tag every result must carry.
    #[serde(default)]
    pub tag: Option<String>,
    /// Max lines (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_list`: concepts by id prefix, type and tag.
pub struct ListTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ListTool {
    const NAME: &'static str = "knowledge_list";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListArgs;

    fn description(&self) -> String {
        "List knowledge concepts sorted by id, optionally filtered by id prefix, type and tag."
            .into()
    }

    async fn call(&self, a: ListArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        let metas = k
            .list_for(
                a.prefix.as_deref(),
                a.type_.as_deref(),
                a.tag.as_deref(),
                a.limit.unwrap_or(50),
                Some(cx.session),
            )
            .await?;
        let mut s = String::new();
        for m in &metas {
            meta_line(&mut s, m);
            s.push('\n');
        }
        if s.is_empty() {
            s.push_str("no concepts");
        }
        Ok(structured(s, &metas))
    }
}

/// `knowledge_put`: create or update a concept.
pub struct PutTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for PutTool {
    const NAME: &'static str = "knowledge_put";
    // ReadOnly on purpose (10 §6): a validated file in the harness's own bundle,
    // committed and restorable. Asking on every fact would stop the model recording.
    const RISK: Risk = Risk::ReadOnly;
    type Args = Put;

    fn description(&self) -> String {
        "Record durable knowledge across sessions: facts about the user's environment (hosts, paths, \
         toolchains, accounts, conventions), decisions and their reasons, procedures that worked, and \
         entities (projects, services, people) belong here. Put on an existing id updates it — check \
         knowledge_search first and reuse the id rather than adding a near-duplicate. Ids are lowercase \
         `dir/slug`; include the evidence (command, path, error, the user's words) in the body and link \
         related concepts as [title](/other/id.md)."
            .into()
    }

    async fn call(&self, p: Put, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        // 136 §2.1: the consolidation keys are the night's (or the owner's):
        // gated like the five ops and stamped `source: dreaming`, so the
        // night's undo receipt sees the tidy puts and a live session cannot
        // archive or un-hint a concept by hand through a plain put.
        let consolidates = p.status.is_some()
            || p.hint.is_some()
            || p.verified.is_some()
            || p.valid_until.is_some()
            || p.superseded_by.is_some()
            || p.merged_into.is_some();
        let source = if consolidates {
            consolidating(&k, "knowledge_put with a consolidation key", &cx).await?;
            Source::Dreaming
        } else {
            writable(&cx)?;
            Source::Model
        };
        let (meta, created) = k.put_for(p, source, Some(cx.session)).await?;
        let how = if created { "new" } else { "updated" };
        Ok(ToolOutput::ok(format!("put {} ({how})", meta.id)))
    }
}

/// Arguments for [`ShareTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShareArgs {
    /// Concept id.
    pub id: String,
    /// `private` (owner only) | `team` (owner's reporting line) | `shared` (everyone).
    pub visibility: Visibility,
}

/// `knowledge_share`: change who may read a concept (67 §9).
pub struct ShareTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ShareTool {
    const NAME: &'static str = "knowledge_share";
    // ReadOnly like `knowledge_put`: the closing turn of a `read_only` desk must be able to share.
    const RISK: Risk = Risk::ReadOnly;
    type Args = ShareArgs;

    fn description(&self) -> String {
        "Change who may read a knowledge concept: `private` (you alone), `team` (you, your managers and the \
         agents reporting to you) or `shared` (everyone). Concepts you record are private by default; share \
         what your team or everyone should know. Only the owner or someone in its reporting line may change it."
            .into()
    }

    async fn call(&self, a: ShareArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        writable(&cx)?;
        let meta = k.share(&a.id, a.visibility, Some(cx.session)).await?;
        Ok(ToolOutput::ok(format!(
            "{} is now {}",
            meta.id,
            meta.visibility.as_str()
        )))
    }
}

/// `knowledge_delete`: remove a concept (recoverable via `knowledge_restore`).
pub struct DeleteTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for DeleteTool {
    const NAME: &'static str = "knowledge_delete";
    const RISK: Risk = Risk::Destructive;
    type Args = IdArgs;

    fn description(&self) -> String {
        "Delete a knowledge concept by id. History keeps it; knowledge_restore brings it back."
            .into()
    }

    async fn call(&self, a: IdArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        writable(&cx)?;
        k.delete(&a.id).await?;
        Ok(ToolOutput::ok(format!("deleted {}", a.id)))
    }
}

/// Arguments for [`HistoryTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct HistoryArgs {
    /// Concept id; omit for the whole bundle's history.
    #[serde(default)]
    pub id: Option<String>,
    /// Max commits (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_history`: commits touching a concept (or the bundle), with the latest diff.
pub struct HistoryTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for HistoryTool {
    const NAME: &'static str = "knowledge_history";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    type Args = HistoryArgs;

    fn description(&self) -> String {
        "Commits that changed a knowledge concept (or the whole bundle when id is omitted), newest first: \
         `commit  time  message`. With an id, the unified diff of its latest change follows."
            .into()
    }

    async fn call(&self, a: HistoryArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        let h = k.history(a.id.as_deref(), a.limit.unwrap_or(20)).await?;
        let mut s = String::new();
        for c in &h.changes {
            s.push_str(&format!("{}  {}  {}\n", c.commit, c.time, c.message));
        }
        if s.is_empty() {
            s.push_str("no history");
        }
        if let Some(diff) = &h.diff {
            s.push('\n');
            s.push_str(diff);
        }
        Ok(structured(s, &h))
    }
}

/// Arguments for [`RestoreTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// Concept id.
    pub id: String,
    /// Abbreviated commit to restore from (default: the commit before the latest change).
    #[serde(default)]
    pub commit: Option<String>,
}

/// `knowledge_restore`: rewrite a concept from an earlier commit.
pub struct RestoreTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for RestoreTool {
    const NAME: &'static str = "knowledge_restore";
    const RISK: Risk = Risk::Mutating;
    type Args = RestoreArgs;

    fn description(&self) -> String {
        "Restore a knowledge concept to its content at an earlier commit (see knowledge_history); \
         also undoes a delete. Default: the commit before the latest change."
            .into()
    }

    async fn call(&self, a: RestoreArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        writable(&cx)?;
        let c = k.restore(&a.id, a.commit.as_deref()).await?;
        Ok(ToolOutput::ok(format!(
            "restored {} from {}",
            a.id, c.commit
        )))
    }
}

/// Arguments for [`StatsTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatsArgs {
    /// Concept ids.
    pub ids: Vec<String>,
}

/// `knowledge_stats` (136 §2.1): impressions and opens per id.
pub struct StatsTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for StatsTool {
    const NAME: &'static str = "knowledge_stats";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = StatsArgs;

    fn description(&self) -> String {
        "Usage counters of knowledge concepts: how often each id was shown in a <knowledge> hint and opened \
         with knowledge_get, with the last times. Unknown ids report zeros."
            .into()
    }

    async fn call(&self, a: StatsArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        let uses = k.stats(&a.ids).await?;
        let mut s = String::new();
        for u in &uses {
            s.push_str(&format!(
                "`{}` shown {} (last {}) opened {} (last {})\n",
                u.id,
                u.shown,
                u.last_shown.as_deref().unwrap_or("never"),
                u.opened,
                u.last_opened.as_deref().unwrap_or("never")
            ));
        }
        if s.is_empty() {
            s.push_str("no ids");
        }
        Ok(structured(s, uses))
    }
}

/// Arguments for [`MergeTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MergeArgs {
    /// Ids folded into `into`; each becomes a redirect stub.
    pub ids: Vec<String>,
    /// The canonical id (one of `ids` or a new one).
    pub into: String,
    /// The canonical's body.
    pub body: String,
    /// Session ids (or `user`) to add to the canonical's `evidence`.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// `knowledge_merge` (136 §2.1): fold near-duplicates into one living entry.
pub struct MergeTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for MergeTool {
    const NAME: &'static str = "knowledge_merge";
    const RISK: Risk = Risk::Mutating;
    type Args = MergeArgs;

    fn description(&self) -> String {
        "Dreaming only: merge several knowledge concepts into one canonical entry (`into`, with `body`); \
         the folded ids become `status: merged` redirect stubs, evidence and tags are unioned, links retargeted."
            .into()
    }

    async fn call(&self, a: MergeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        consolidating(&k, Self::NAME, &cx).await?;
        // The session's agent scope (67 §9); the user's when it has none.
        let scope = k
            .scope_for(Some(cx.session))
            .unwrap_or_else(|| crate::types::AgentScope {
                owner: crate::types::USER_OWNER.to_owned(),
                line: Default::default(),
            });
        let r = k
            .merge(&a.ids, &a.into, &a.body, &a.evidence, &scope)
            .await?;
        Ok(structured(
            format!("merged {} into {}", r.ids[1..].join(", "), r.ids[0]),
            r,
        ))
    }
}

/// Arguments for [`SupersedeTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SupersedeArgs {
    /// The outdated concept.
    pub loser: String,
    /// The concept that replaces it.
    pub winner: String,
    /// RFC 3339: when the loser stopped being true (from the evidence, not now).
    #[serde(default)]
    pub valid_until: Option<String>,
    /// One line for the loser's body.
    #[serde(default)]
    pub note: Option<String>,
}

/// `knowledge_supersede` (136 §2.1): a newer claim replaces an older one.
pub struct SupersedeTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for SupersedeTool {
    const NAME: &'static str = "knowledge_supersede";
    const RISK: Risk = Risk::Mutating;
    type Args = SupersedeArgs;

    fn description(&self) -> String {
        "Dreaming only: mark `loser` as superseded by `winner` (`superseded_by`, `valid_until`, `hint: false`); \
         the loser stays searchable and restorable."
            .into()
    }

    async fn call(&self, a: SupersedeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        consolidating(&k, Self::NAME, &cx).await?;
        let r = k
            .supersede(
                &a.loser,
                &a.winner,
                a.valid_until.as_deref(),
                a.note.as_deref(),
            )
            .await?;
        Ok(structured(
            format!("{} superseded by {}", a.loser, a.winner),
            r,
        ))
    }
}

/// Arguments for [`ArchiveTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveArgs {
    /// Concept id.
    pub id: String,
    /// Why it leaves the hint shelf; appended under `## Archived`.
    pub reason: String,
}

/// `knowledge_archive` (136 §2.1): off the hint shelf, still searchable.
pub struct ArchiveTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ArchiveTool {
    const NAME: &'static str = "knowledge_archive";
    const RISK: Risk = Risk::Mutating;
    type Args = ArchiveArgs;

    fn description(&self) -> String {
        "Dreaming only: archive a knowledge concept (`status: archived, hint: false`) with a reason; \
         never deleted — still searchable and knowledge_restore-able."
            .into()
    }

    async fn call(&self, a: ArchiveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        consolidating(&k, Self::NAME, &cx).await?;
        let r = k.archive(&a.id, &a.reason).await?;
        Ok(structured(format!("archived {}", a.id), r))
    }
}

/// Arguments for [`VerifyTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct VerifyArgs {
    /// Concept id.
    pub id: String,
    /// `ok` (the claim still holds) or `stale` (a read-tool check failed).
    pub outcome: crate::types::VerifyOutcome,
    /// What was checked, or what changed.
    #[serde(default)]
    pub note: Option<String>,
    /// What the check read: a path (absolute or relative to the data dir) or
    /// an http(s) URL. `ok` needs at least one of those; a `session:<uuid>` /
    /// `meeting:<id>` id counts only in addition and only when the concept's
    /// own `evidence` already cites it. `stale` accepts any one of the three.
    /// All are added to the concept's `evidence`.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// 136 P gap 2: a path a read tool could have read, or an http(s) URL.
fn is_read_source(e: &str, data_dir: &std::path::Path) -> bool {
    let e = e.trim();
    !e.is_empty()
        && (e.starts_with("http://") || e.starts_with("https://") || data_dir.join(e).exists())
}

/// A well-formed `session:<uuid>` / `meeting:<id>` reference.
fn is_ref(e: &str) -> bool {
    let e = e.trim();
    if let Some(s) = e.strip_prefix("session:") {
        return s.parse::<runtime::SessionId>().is_ok();
    }
    e.strip_prefix("meeting:").is_some_and(|m| !m.is_empty())
}

/// 136 P2 D2: the evidence rule. `ok` must rest on a read (path or URL);
/// a `session:`/`meeting:` id is accepted only alongside one and only when
/// `cited` (the concept's own `evidence`) already names it — otherwise any
/// session id would stamp `verified:` without a read. `stale` keeps the
/// looser rule: one item of any kind.
fn check_evidence(
    outcome: crate::types::VerifyOutcome,
    evidence: &[String],
    cited: &[String],
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let read = evidence.iter().any(|e| is_read_source(e, data_dir));
    match outcome {
        crate::types::VerifyOutcome::Ok => {
            anyhow::ensure!(
                read,
                "verify ok needs a read: evidence must include a path that exists or an http(s) URL \
                 (a session:/meeting: id counts only in addition, and only one the concept already cites)"
            );
            for e in evidence.iter().filter(|e| is_ref(e)) {
                anyhow::ensure!(
                    cited.iter().any(|c| c == e.trim()),
                    "verify ok: `{e}` is not in the concept's evidence — a session:/meeting: id counts \
                     only when the concept already cites it; cite the path or URL you read"
                );
            }
        }
        crate::types::VerifyOutcome::Stale => anyhow::ensure!(
            read || evidence.iter().any(|e| is_ref(e)),
            "verify needs evidence from a read: pass evidence with a path that exists, an http(s) URL, or a session:/meeting: id"
        ),
    }
    Ok(())
}

/// `knowledge_verify` (136 §2.1): record a read-tool check's verdict.
pub struct VerifyTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for VerifyTool {
    const NAME: &'static str = "knowledge_verify";
    const RISK: Risk = Risk::Mutating;
    type Args = VerifyArgs;

    fn description(&self) -> String {
        "Dreaming only: record the outcome of checking a knowledge concept against the world with read tools: \
         `ok` sets `verified: <today>`, `stale` sets `status: stale` and keeps the note. Never a model's opinion: \
         `ok` needs `evidence` with the path or http(s) URL you read this turn (a session:/meeting: id counts \
         only in addition, and only when the concept already cites it); `stale` takes any of the three."
            .into()
    }

    async fn call(&self, a: VerifyArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        consolidating(&k, Self::NAME, &cx).await?;
        let cited = k.get(&a.id).await?.meta.evidence;
        check_evidence(a.outcome, &a.evidence, &cited, &cx.resources.root)?;
        let r = k
            .verify(&a.id, a.outcome, a.note.as_deref(), &a.evidence)
            .await?;
        Ok(structured(format!("verified {}: {:?}", a.id, a.outcome), r))
    }
}

/// Arguments for [`RetitleTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RetitleArgs {
    /// Concept id.
    pub id: String,
    /// New title, ≤ 120 chars — the first clause of the old one.
    pub title: String,
    /// Paragraph prepended to the body so nothing of the old title is lost.
    pub summary: String,
}

/// `knowledge_retitle` (136 §2.1): a readable hint line.
pub struct RetitleTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for RetitleTool {
    const NAME: &'static str = "knowledge_retitle";
    const RISK: Risk = Risk::Mutating;
    type Args = RetitleArgs;

    fn description(&self) -> String {
        "Dreaming only: shorten a knowledge concept's title (≤ 120 chars) and prepend the rest of the old \
         title to the body as a Summary paragraph."
            .into()
    }

    async fn call(&self, a: RetitleArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let k = self.0.for_session(cx.session).await?;
        consolidating(&k, Self::NAME, &cx).await?;
        let r = k.retitle(&a.id, &a.title, &a.summary).await?;
        Ok(structured(format!("retitled {}", a.id), r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_match_doc() {
        assert_eq!(
            [
                SearchTool::NAME,
                GetTool::NAME,
                ListTool::NAME,
                PutTool::NAME,
                DeleteTool::NAME,
                HistoryTool::NAME,
                RestoreTool::NAME,
            ],
            [
                "knowledge_search",
                "knowledge_get",
                "knowledge_list",
                "knowledge_put",
                "knowledge_delete",
                "knowledge_history",
                "knowledge_restore",
            ]
        );
        assert_eq!(PutTool::RISK, Risk::ReadOnly);
        assert_eq!(DeleteTool::RISK, Risk::Destructive);
        assert_eq!(RestoreTool::RISK, Risk::Mutating);
        // 136 §2.1
        assert_eq!(
            [
                StatsTool::NAME,
                MergeTool::NAME,
                SupersedeTool::NAME,
                ArchiveTool::NAME,
                VerifyTool::NAME,
                RetitleTool::NAME,
            ],
            [
                "knowledge_stats",
                "knowledge_merge",
                "knowledge_supersede",
                "knowledge_archive",
                "knowledge_verify",
                "knowledge_retitle",
            ]
        );
        assert_eq!(StatsTool::RISK, Risk::ReadOnly);
        for r in [
            MergeTool::RISK,
            SupersedeTool::RISK,
            ArchiveTool::RISK,
            VerifyTool::RISK,
            RetitleTool::RISK,
        ] {
            assert_eq!(r, Risk::Mutating);
        }
    }

    struct Main;
    #[async_trait::async_trait]
    impl crate::Scoper for Main {
        fn scope_for(&self, _: runtime::SessionId) -> Option<crate::AgentScope> {
            None
        }
        async fn bundle_for(
            &self,
            _: runtime::SessionId,
        ) -> std::result::Result<Option<String>, String> {
            Ok(None)
        }
    }

    fn cx(private: bool) -> ToolCx {
        let dir = std::env::temp_dir();
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: Arc::new(tools::SessionResources {
                root: dir.clone(),
                retention: if private {
                    tools::Retention::Private
                } else {
                    tools::Retention::Durable
                },
                generation: 1,
            }),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    fn put() -> Put {
        Put {
            id: "facts/x".into(),
            type_: "fact".into(),
            title: "X".into(),
            description: None,
            tags: Vec::new(),
            scope: None,
            body: "body".into(),
            visibility: None,
            ..Put::default()
        }
    }

    #[tokio::test]
    async fn private_session_refuses_mutations_durable_writes() {
        let scratch = crate::test_dir("ktools");
        let dir = scratch.path().to_path_buf();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.set_scoper(Arc::new(Main));
        let msg = "knowledge mutations are disabled in a private session";
        let e = PutTool(k.clone()).call(put(), cx(true)).await.unwrap_err();
        assert_eq!(e.to_string(), msg);
        let id = IdArgs {
            id: "facts/x".into(),
        };
        let e = DeleteTool(k.clone()).call(id, cx(true)).await.unwrap_err();
        assert_eq!(e.to_string(), msg);
        let restore = RestoreArgs {
            id: "facts/x".into(),
            commit: None,
        };
        let e = RestoreTool(k.clone())
            .call(restore, cx(true))
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), msg);
        assert!(k.get("facts/x").await.is_err());
        PutTool(k.clone()).call(put(), cx(false)).await.unwrap();
        assert!(k.get("facts/x").await.is_ok());
    }

    /// 136 §3.1: `knowledge_search` keeps archived items and marks them;
    /// `knowledge_get` records an open.
    #[tokio::test]
    async fn search_marks_archived_and_get_records_an_open() {
        let scratch = crate::test_dir("ktools");
        let dir = scratch.path().to_path_buf();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.set_scoper(Arc::new(Main));
        k.put(put(), Source::Model).await.unwrap();
        k.put(
            Put {
                id: "facts/y".into(),
                status: Some(crate::types::Status::Archived),
                ..put()
            },
            Source::Model,
        )
        .await
        .unwrap();
        let out = SearchTool(k.clone())
            .call(
                SearchArgs {
                    query: "body".into(),
                    limit: None,
                },
                cx(false),
            )
            .await
            .unwrap();
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 2);
        let y = lines.iter().find(|l| l.starts_with("`facts/y`")).unwrap();
        assert!(y.contains("[fact] [archived]"), "{y}");
        let x = lines.iter().find(|l| l.starts_with("`facts/x`")).unwrap();
        assert!(x.contains("[fact] (") && !x.contains("[archived]"), "{x}");

        GetTool(k.clone())
            .call(
                IdArgs {
                    id: "facts/x".into(),
                },
                cx(false),
            )
            .await
            .unwrap();
        let s = k.stats(&["facts/x".into()]).await.unwrap();
        assert_eq!((s[0].shown, s[0].opened), (0, 1));
    }

    /// Says yes to exactly one session.
    struct OneSession(runtime::SessionId);

    #[async_trait::async_trait]
    impl crate::Consolidator for OneSession {
        async fn may_consolidate(&self, s: runtime::SessionId) -> bool {
            s == self.0
        }
    }

    /// 136 §2.1: the five consolidation tools refuse without a gate, refuse a
    /// session the gate rejects, and reach the store (which wants a file) for one it
    /// allows; `knowledge_stats` answers anywhere.
    #[tokio::test]
    async fn consolidation_tools_need_owner_root_or_unattended() {
        let scratch = crate::test_dir("ktools");
        let dir = scratch.path().to_path_buf();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.set_scoper(Arc::new(Main));
        let archive = || ArchiveArgs {
            id: "facts/x".into(),
            reason: "old".into(),
        };
        // No gate installed: nobody may.
        let e = ArchiveTool(k.clone())
            .call(archive(), cx(false))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("knowledge_archive runs only"), "{e}");
        // Private sessions are refused first.
        let e = ArchiveTool(k.clone())
            .call(archive(), cx(true))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("private session"), "{e}");
        // A gate: the allowed session reaches the store; another is refused.
        let allowed = cx(false);
        k.set_consolidator(Arc::new(OneSession(allowed.session)));
        let e = ArchiveTool(k.clone())
            .call(archive(), allowed.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("no such concept: facts/x"), "{e}");
        let e = VerifyTool(k.clone())
            .call(
                VerifyArgs {
                    id: "facts/x".into(),
                    outcome: crate::types::VerifyOutcome::Ok,
                    note: None,
                    evidence: Vec::new(),
                },
                cx(false),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("knowledge_verify runs only"), "{e}");
        // A plain put is anyone's and `source: model`; a put that sets a
        // consolidation key is gated the same way and, on an existing file,
        // keeps `source` and marks `consolidated_at` (D5).
        let tidy = || Put {
            status: Some(crate::Status::Tidy),
            ..put()
        };
        let e = PutTool(k.clone())
            .call(tidy(), cx(false))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("knowledge_put with a consolidation key runs only"),
            "{e}"
        );
        PutTool(k.clone()).call(put(), cx(false)).await.unwrap();
        assert_eq!(
            k.get("facts/x").await.unwrap().meta.source,
            Some(Source::Model)
        );
        PutTool(k.clone())
            .call(tidy(), allowed.clone())
            .await
            .unwrap();
        let m = k.get("facts/x").await.unwrap().meta;
        assert_eq!(
            (m.source, m.status, m.consolidated_at.is_some()),
            (Some(Source::Model), crate::Status::Tidy, true)
        );
        // R-28: the registered dreaming session's own reads are not opens;
        // a plain session's are.
        let night = cx(false);
        k.register_consolidating_session(night.session);
        let get = GetTool(k.clone());
        let read = |c: ToolCx| {
            get.call(
                IdArgs {
                    id: "facts/x".into(),
                },
                c,
            )
        };
        read(night.clone()).await.unwrap();
        assert_eq!(k.stats(&["facts/x".into()]).await.unwrap()[0].opened, 0);
        read(cx(false)).await.unwrap();
        assert_eq!(k.stats(&["facts/x".into()]).await.unwrap()[0].opened, 1);
        k.clear_consolidating_session();
        read(night).await.unwrap();
        assert_eq!(k.stats(&["facts/x".into()]).await.unwrap()[0].opened, 2);
        // Stats is read-only and real.
        let out = StatsTool(k.clone())
            .call(
                StatsArgs {
                    ids: vec!["facts/x".into()],
                },
                cx(true),
            )
            .await
            .unwrap();
        assert!(out.content.contains("shown 0"), "{}", out.content);
        assert_eq!(out.structured.unwrap()[0]["opened"], 2);
    }

    /// 136 P gap 2 + P2 D2: `knowledge_verify ok` refuses without a read
    /// (a path that exists, relative to the data dir or absolute, or an
    /// http(s) URL); a `session:`/`meeting:` id never carries `ok` alone and
    /// counts only when the concept already cites it. `stale` takes any one
    /// of the three kinds.
    #[tokio::test]
    async fn verify_needs_evidence_from_a_read() {
        use crate::types::VerifyOutcome::{Ok as VOk, Stale};
        let scratch = crate::test_dir("ktools");
        let dir = scratch.path().to_path_buf();
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        std::fs::write(dir.join("conf/pg.toml"), "port = 5432\n").unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.set_scoper(Arc::new(Main));
        let cited = runtime::SessionId::new();
        k.put(
            Put {
                evidence: vec![format!("session:{cited}")],
                ..put()
            },
            Source::Model,
        )
        .await
        .unwrap();
        let night = ToolCx {
            resources: Arc::new(tools::SessionResources {
                root: dir.clone(),
                retention: tools::Retention::Durable,
                generation: 1,
            }),
            ..cx(false)
        };
        k.register_consolidating_session(night.session);
        let tool = VerifyTool(k.clone());
        let verify = |outcome, evidence: Vec<&str>| {
            tool.call(
                VerifyArgs {
                    id: "facts/x".into(),
                    outcome,
                    note: None,
                    evidence: evidence.into_iter().map(str::to_owned).collect(),
                },
                night.clone(),
            )
        };
        let other = runtime::SessionId::new();
        let cited_s = format!("session:{cited}");
        let other_s = format!("session:{other}");
        // No read at all: refused, whatever the ids say.
        for bad in [
            vec![],
            vec!["I checked it"],
            vec!["conf/missing.toml"],
            vec!["session:not-a-uuid"],
            vec!["meeting:"],
            vec!["ftp://x"],
            vec![cited_s.as_str()],
            vec![other_s.as_str(), "meeting:06"],
        ] {
            let e = verify(VOk, bad.clone()).await.unwrap_err().to_string();
            assert!(e.starts_with("verify ok needs a read:"), "{bad:?}: {e}");
        }
        // A read plus an id the concept does not cite: refused, naming the id.
        let e = verify(VOk, vec!["conf/pg.toml", other_s.as_str()])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.starts_with(&format!(
                "verify ok: `{other_s}` is not in the concept's evidence"
            )),
            "{e}"
        );
        assert!(k.get("facts/x").await.unwrap().meta.verified.is_none());
        // One real source admits the call; every item is recorded (evidence
        // items are whitespace-free tokens: the index column is space-joined).
        verify(VOk, vec!["conf/pg.toml", "pg_isready"])
            .await
            .unwrap();
        let m = k.get("facts/x").await.unwrap().meta;
        assert!(m.verified.is_some());
        assert_eq!(m.evidence, [cited_s.as_str(), "conf/pg.toml", "pg_isready"]);
        for good in [
            vec![dir.join("conf/pg.toml").to_string_lossy().into_owned()],
            vec!["https://example.com/doc".into()],
            vec!["https://example.com/doc".into(), cited_s.clone()],
        ] {
            verify(VOk, good.iter().map(String::as_str).collect())
                .await
                .unwrap();
        }
        // `stale` keeps the one-of-three rule: a bare id is enough, nothing is not.
        let e = verify(Stale, vec!["ftp://x"])
            .await
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("verify needs evidence from a read:"), "{e}");
        verify(Stale, vec!["meeting:06"]).await.unwrap();
        let m = k.get("facts/x").await.unwrap().meta;
        assert_eq!(m.evidence.len(), 6, "{:?}", m.evidence);
        assert_eq!(m.evidence[5], "meeting:06");
        assert_eq!(m.status, crate::types::Status::Stale);
    }

    /// Answers in order, or fails the whole call when `answers` is `None`.
    struct FakeJudge {
        answers: Option<Vec<BoolVerdict>>,
        seen: parking_lot::Mutex<Vec<BoolItem>>,
    }

    #[async_trait::async_trait]
    impl MemoryJudge for FakeJudge {
        async fn judge(&self, items: Vec<BoolItem>) -> anyhow::Result<Vec<BoolVerdict>> {
            self.seen.lock().extend(items);
            self.answers.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "decision model unavailable: checkpoint engine `gliner` is not running"
                )
            })
        }
    }

    fn said(choice: bool, p: f32) -> BoolVerdict {
        BoolVerdict {
            p_true: Some(p),
            choice: Some(choice),
            truncated: false,
            error: None,
        }
    }

    fn peek_tool(
        k: &Arc<Knowledge>,
        answers: Option<Vec<BoolVerdict>>,
    ) -> (PeekTool, Arc<FakeJudge>) {
        let judge = Arc::new(FakeJudge {
            answers,
            seen: parking_lot::Mutex::new(Vec::new()),
        });
        let tool = PeekTool {
            knowledge: k.clone(),
            judge: judge.clone(),
            threshold: 0.5,
        };
        (tool, judge)
    }

    fn peek_args(question: &str, ids: &[&str]) -> PeekArgs {
        PeekArgs {
            question: question.into(),
            ids: ids.iter().map(|&i| i.to_owned()).collect(),
        }
    }

    async fn peek_bundle(tag: &str) -> (tempfile::TempDir, Arc<Knowledge>) {
        let scratch = crate::test_dir(tag);
        let dir = scratch.path().to_path_buf();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.set_scoper(Arc::new(Main));
        for (id, title) in [("facts/a", "A"), ("facts/b", "B"), ("facts/c", "C")] {
            k.put(
                Put {
                    id: id.into(),
                    title: title.into(),
                    body: format!("body of {title}"),
                    ..put()
                },
                Source::Model,
            )
            .await
            .unwrap();
        }
        (scratch, k)
    }

    async fn opened(k: &Knowledge, ids: &[&str]) -> Vec<u64> {
        let ids: Vec<String> = ids.iter().map(|&i| i.to_owned()).collect();
        let stats = k.stats(&ids).await.unwrap();
        stats.iter().map(|s| s.opened).collect()
    }

    /// 212 §5.1: relevant rows in full (as `knowledge_get` renders them, in
    /// request order, each an open), the rest one line each; the loop's
    /// parser finds exactly the relevant ids.
    #[tokio::test]
    async fn peek_returns_relevant_in_full_and_the_rest_as_lines() {
        let (_scratch, k) = peek_bundle("kpeek").await;
        let answers = vec![said(true, 0.91), said(false, 0.12), said(true, 0.74)];
        let (tool, judge) = peek_tool(&k, Some(answers));
        let out = tool
            .call(
                peek_args(
                    "how is the build lock acquired",
                    &["facts/a", " facts/c", "facts/nope", "facts/b", "facts/a"],
                ),
                cx(false),
            )
            .await
            .unwrap();
        let a = render_concept(k.get("facts/a").await.unwrap());
        let b = render_concept(k.get("facts/b").await.unwrap());
        let want = format!(
            "peek_memory: 2 of 4 relevant to \"how is the build lock acquired\"\n\
             \n\
             ### `facts/a` — relevant (p=0.91)\n{a}\
             \n\
             ### `facts/b` — relevant (p=0.74)\n{b}\
             \n\
             Not pulled in:\n\
             - `facts/c` — C — not relevant (p=0.12)\n\
             - `facts/nope` — missing\n"
        );
        assert_eq!(out.content, want);
        assert!(out.structured.is_none());
        let seen = judge.seen.lock();
        assert_eq!(seen.len(), 3);
        assert_eq!(
            seen[0].statement,
            "This memory helps answer the question: how is the build lock acquired"
        );
        // No description given: the store fills in the title.
        assert_eq!(seen[0].state, "A\nA\n\nbody of A");
        drop(seen);
        assert_eq!(
            opened(&k, &["facts/a", "facts/b", "facts/c"]).await,
            [1, 1, 0]
        );
        assert_eq!(
            peek_relevant_ids(&out.content).collect::<Vec<_>>(),
            ["facts/a", "facts/b"]
        );
    }

    /// 212 §5.2: no model → every row undecided, nothing pulled in or opened.
    #[tokio::test]
    async fn peek_fails_closed_when_the_judge_is_unavailable() {
        let (_scratch, k) = peek_bundle("kpeek").await;
        let (tool, _) = peek_tool(&k, None);
        let out = tool
            .call(peek_args("q", &["facts/a", "facts/b"]), cx(false))
            .await
            .unwrap();
        let undecided = "undecided (decision model unavailable: checkpoint engine `gliner` is not running); knowledge_get(id) reads it anyway";
        assert_eq!(
            out.content,
            format!(
                "peek_memory: 0 of 2 relevant to \"q\"\n\
                 \n\
                 No memory was judged relevant.\n\
                 \n\
                 Not pulled in:\n\
                 - `facts/a` — A — {undecided}\n\
                 - `facts/b` — B — {undecided}\n\
                 The local decision model is unavailable; read with knowledge_get(id) instead.\n"
            )
        );
        assert!(peek_relevant_ids(&out.content).next().is_none());
        assert_eq!(opened(&k, &["facts/a", "facts/b"]).await, [0, 0]);
    }

    /// 212 §5.3: argument limits; dedupe keeps request order.
    #[tokio::test]
    async fn peek_validates_question_and_ids() {
        let (_scratch, k) = peek_bundle("kpeek").await;
        let (tool, judge) = peek_tool(&k, Some(vec![said(false, 0.1), said(false, 0.2)]));
        let tool = &tool;
        let err = |a| async move { tool.call(a, cx(false)).await.unwrap_err().to_string() };
        let q = "peek_memory: question must be 1–500 characters";
        assert_eq!(err(peek_args("  ", &["facts/a"])).await, q);
        assert_eq!(err(peek_args(&"x".repeat(501), &["facts/a"])).await, q);
        let n = "peek_memory: give 1–12 ids";
        assert_eq!(err(peek_args("q", &[])).await, n);
        assert_eq!(err(peek_args("q", &[" ", ""])).await, n);
        let thirteen: Vec<String> = (0..13).map(|i| format!("facts/x{i}")).collect();
        let thirteen: Vec<&str> = thirteen.iter().map(String::as_str).collect();
        assert_eq!(err(peek_args("q", &thirteen)).await, n);
        assert!(judge.seen.lock().is_empty());
        let out = tool
            .call(
                peek_args(&"é".repeat(500), &["facts/b", "facts/a", "facts/b "]),
                cx(false),
            )
            .await
            .unwrap();
        let seen = judge.seen.lock();
        let titles: Vec<&str> = seen.iter().map(|i| &i.state[..1]).collect();
        assert_eq!(titles, ["B", "A"]);
        // The headline quotes 80 chars of the question, then `…`.
        let head = out.content.lines().next().unwrap();
        assert_eq!(
            head,
            format!("peek_memory: 0 of 2 relevant to \"{}…\"", "é".repeat(80))
        );
    }

    /// Puts its sessions in fixed scopes; unknown sessions are the user's.
    struct Scopes(Vec<(runtime::SessionId, crate::AgentScope)>);

    #[async_trait::async_trait]
    impl crate::Scoper for Scopes {
        fn scope_for(&self, s: runtime::SessionId) -> Option<crate::AgentScope> {
            self.0
                .iter()
                .find(|(id, _)| *id == s)
                .map(|(_, a)| a.clone())
        }
        async fn bundle_for(
            &self,
            _: runtime::SessionId,
        ) -> std::result::Result<Option<String>, String> {
            Ok(None)
        }
    }

    /// 212 §5.4: another owner's private concept reads as `missing`, exactly
    /// like an absent one; the judge never sees it.
    #[tokio::test]
    async fn peek_hides_what_the_session_may_not_see() {
        let (_scratch, k) = peek_bundle("kpeek").await;
        let (bob, alice) = (cx(false), cx(false));
        let scope = |name: &str| crate::AgentScope {
            owner: format!("agent:{name}"),
            line: Default::default(),
        };
        k.set_scoper(Arc::new(Scopes(vec![
            (bob.session, scope("bob")),
            (alice.session, scope("alice")),
        ])));
        k.put_for(
            Put {
                id: "facts/secret".into(),
                title: "Secret".into(),
                ..put()
            },
            Source::Model,
            Some(bob.session),
        )
        .await
        .unwrap();
        assert_eq!(
            k.get("facts/secret").await.unwrap().meta.visibility,
            Visibility::Private
        );
        let (tool, judge) = peek_tool(&k, Some(vec![said(true, 0.9)]));
        let out = tool
            .call(peek_args("q", &["facts/secret", "facts/nope"]), alice)
            .await
            .unwrap();
        assert!(
            out.content.ends_with(
                "Not pulled in:\n- `facts/secret` — missing\n- `facts/nope` — missing\n"
            ),
            "{}",
            out.content
        );
        assert!(judge.seen.lock().is_empty());
        let out = tool
            .call(peek_args("q", &["facts/secret"]), bob)
            .await
            .unwrap();
        assert!(
            out.content
                .contains("### `facts/secret` — relevant (p=0.90)")
        );
    }

    /// 212 §5.5–6: an `@commit` id is refused per row; `choice=true` under the
    /// threshold is not relevant.
    #[tokio::test]
    async fn peek_refuses_revisions_and_applies_the_threshold() {
        let (_scratch, k) = peek_bundle("kpeek").await;
        let (tool, judge) = peek_tool(&k, Some(vec![said(true, 0.4)]));
        let out = tool
            .call(peek_args("q", &["facts/a@abc1234", "facts/a"]), cx(false))
            .await
            .unwrap();
        assert_eq!(
            out.content,
            "peek_memory: 0 of 2 relevant to \"q\"\n\
             \n\
             No memory was judged relevant.\n\
             \n\
             Not pulled in:\n\
             - `facts/a@abc1234` — not peekable: revisions are read with knowledge_get\n\
             - `facts/a` — A — not relevant (p=0.40)\n"
        );
        assert_eq!(judge.seen.lock().len(), 1);
        assert_eq!(opened(&k, &["facts/a"]).await, [0]);
    }
}
