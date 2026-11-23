// 144-E1 owns this file.
//! Insights (144 §4.3): the default `Dense summary` at the end of an ingest, `insight_run` (a builtin or
//! a `notebook_insight: true` skill), `insight_promote` (an insight section → a note). Every insight is a
//! `### <name> · <model> · <date>` section under `## Insights` at the end of the source document, written
//! as `Principal::Helper` — the one post-ingest write a source accepts (`share.rs::source_writable`).
//!
//! The model call and the skill lookup live above this crate (the agent actor's typed registry, the skills
//! index): the crate that knows both ends (`rpc`) installs [`Hooks`] once at start, the way
//! `codeindex::map::generator::install_side_caller` works.

use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::SessionId;

use crate::actor::{CreateNote, DocumentActor, Edit, Markdown};
use crate::helper::Edit as BlockEdit;
use crate::ingest::{Ctx, IngestActor, flat_err, ingest};
use crate::meta::{DocId, Principal};
use crate::notebook::{
    InsightInstruction, InsightRun, ItemKind, NotebookItemAdd, SourceEvent, SourceStatus,
};

/// The default insight's instruction (§4.3, verbatim).
pub const DEFAULT_PROMPT: &str =
    "Dense summary (≤ 200 words), then `Key points` (≤ 8 bullets); cite nothing; plain markdown";
/// The default insight's section name.
pub const DEFAULT_NAME: &str = "Dense summary";
/// The three builtins of `insight_run`: `(name, section heading, instruction)`.
pub const BUILTINS: [(&str, &str, &str); 3] = [
    (
        "key_insights",
        "Key insights",
        "Key insights: ≤ 10 bullets, each one sentence",
    ),
    (
        "toc",
        "Table of contents",
        "Table of contents: the document's structure as a nested list with one line per section",
    ),
    (
        "open_questions",
        "Open questions",
        "Open questions: ≤ 8 questions the source raises but does not answer",
    ),
];
/// Wall-clock budget of one insight call.
const BUDGET: Duration = Duration::from_secs(120);

/// One model call the agent plugin runs for an insight: `session` is the caller's chat when there is
/// one; `notebooks` are the live notebooks holding the source (the agent charges the call to one of
/// their chats); `model` is `notebooks.insight_model` (`None` = `agent.title_model` → the session model).
#[derive(Clone, Debug)]
pub struct InsightCall {
    /// The source being summarised.
    pub source: DocId,
    /// Notebooks holding it, oldest membership first.
    pub notebooks: Vec<DocId>,
    /// The calling chat, if any.
    pub session: Option<SessionId>,
    /// `notebooks.insight_model`.
    pub model: Option<String>,
    /// System prompt: the instruction, verbatim.
    pub system: String,
    /// The source markdown (cut to `insight_input_tokens`).
    pub user: String,
    /// Wall-clock budget.
    pub budget: Duration,
}

/// What came back.
#[derive(Clone, Debug)]
pub struct InsightReply {
    /// The model's markdown.
    pub text: String,
    /// Canonical id of the model that answered (goes into the section heading).
    pub model: String,
}

/// Runs one [`InsightCall`].
pub type SideCaller = Arc<
    dyn Fn(InsightCall) -> Pin<Box<dyn Future<Output = Result<InsightReply, String>> + Send>>
        + Send
        + Sync,
>;
/// A skill name → `(section heading, instruction)` for skills whose front matter has
/// `notebook_insight: true`; `Err` names the problem.
pub type SkillResolver = Arc<dyn Fn(&str) -> Result<(String, String), String> + Send + Sync>;

/// The two seams above this crate.
pub struct Hooks {
    /// The model call.
    pub side_call: SideCaller,
    /// The skill lookup.
    pub skill: SkillResolver,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Install the process-wide hooks once; `false` when already installed.
pub fn install(hooks: Hooks) -> bool {
    HOOKS.set(hooks).is_ok()
}

fn hooks() -> Result<&'static Hooks, String> {
    HOOKS
        .get()
        .ok_or_else(|| "insights are unavailable: the agent plugin is not wired up".to_owned())
}

/// `(section heading, instruction)` for an instruction: a builtin by name, else the skill.
pub fn resolve(i: &InsightInstruction) -> Result<(String, String), String> {
    match (i.builtin.as_deref(), i.skill.as_deref()) {
        (Some(b), None) => BUILTINS
            .iter()
            .find(|(name, _, _)| *name == b)
            .map(|(_, heading, text)| ((*heading).to_owned(), (*text).to_owned()))
            .ok_or_else(|| {
                format!(
                    "unknown builtin `{b}`; builtins are {}",
                    BUILTINS.map(|(n, _, _)| n).join(", ")
                )
            }),
        (None, Some(s)) => (hooks()?.skill)(s),
        _ => Err("give exactly one of builtin or skill".to_owned()),
    }
}

/// Builtin names, in order (the first three of `insight_skills()`).
pub fn builtin_names() -> Vec<String> {
    BUILTINS.map(|(n, _, _)| n.to_owned()).to_vec()
}

// ---- The hook at the end of an ingest -----------------------------------------------------------

/// Called by the ingest job once a source is `completed` (spawned, so the card never shows `queued`
/// again while the model answers). One `Dense summary` per source: a refresh that kept the blocks
/// (same text) does not add a second one. A failed call never fails the ingest: it lands as
/// `insight_error` in the source's `meta`.
pub(crate) async fn after_ingest(ctx: Ctx, source: DocId) {
    if !ctx.cfg.insight_default {
        return;
    }
    let md = match markdown(&ctx, &source, false).await {
        Ok(md) => md,
        Err(e) => {
            tracing::debug!(%source, "insight skipped: {e}");
            return;
        }
    };
    if has_section(&md, DEFAULT_NAME) {
        return;
    }
    if let Err(e) = generate(&ctx, &source, DEFAULT_NAME, DEFAULT_PROMPT, None).await {
        tracing::info!(%source, "default insight not written: {e}");
        record_error(&ctx, &source, &e).await;
    }
}

/// The model call and the append: the source's own text (never its earlier insights) cut to
/// `insight_input_tokens`, then [`append`].
async fn generate(
    ctx: &Ctx,
    source: &DocId,
    heading: &str,
    instruction: &str,
    session: Option<SessionId>,
) -> Result<String, String> {
    let md = markdown(ctx, source, false).await?;
    let own = strip_insights(&md);
    let input = platform::text::head_chars(own, ctx.cfg.insight_input_tokens as usize * 4);
    if input.trim().is_empty() {
        return Err("the source has no text".to_owned());
    }
    let notebooks = ctx
        .store
        .notebooks_of(ItemKind::Source, source.as_str(), true)
        .await
        .map_err(|e| e.to_string())?;
    let reply = (hooks()?.side_call)(InsightCall {
        source: source.clone(),
        notebooks,
        session,
        model: ctx.cfg.insight_model.clone(),
        system: instruction.to_owned(),
        user: input.to_owned(),
        budget: BUDGET,
    })
    .await?;
    let text = reply.text.trim();
    if text.is_empty() {
        return Err("the model answered nothing".to_owned());
    }
    append(ctx, source, heading, &reply.model, text).await
}

/// The model's own `#`/`##`/`###` headings inside an insight (`# Summary`, `# Key points`) become `####`, so the
/// `###` section stays one section for `section_of` and the UI's reader (both end a section at `#`–`###`).
fn demote_headings(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if !fenced && (1..=3).contains(&hashes) && line[hashes..].starts_with(' ') {
            out.push_str("####");
            out.push_str(&line[hashes..]);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// Append `### <heading> · <model> · <date>` + `text` under `## Insights` (created when absent) as
/// `Principal::Helper`; bump `sources.insights`, clear `insight_error`, emit `Changed{Completed}`.
/// Returns the `###` heading's block id.
pub(crate) async fn append(
    ctx: &Ctx,
    source: &DocId,
    heading: &str,
    model: &str,
    text: &str,
) -> Result<String, String> {
    let annotated = markdown(ctx, source, true).await?;
    let has = has_insights(&annotated);
    let section = format!(
        "### {heading} · {model} · {}\n\n{}\n",
        today(),
        demote_headings(text)
    );
    let markdown = if has {
        section
    } else {
        format!("## Insights\n\n{section}")
    };
    let out = ctx
        .docs
        .ask(Edit {
            doc: source.clone(),
            edits: vec![BlockEdit::Insert {
                markdown,
                after: None,
                before: None,
                at: Some("end".into()),
            }],
            by: Principal::Helper,
        })
        .await
        .map_err(|e| e.to_string())?;
    let block = out
        .inserted
        .get(usize::from(!has))
        .cloned()
        .ok_or_else(|| "the insert returned no block ids".to_owned())?;
    match ctx.store.source_row(source).await {
        Ok(Some(mut row)) => {
            row.insights += 1;
            if let Some(o) = row.meta.as_object_mut() {
                o.remove("insight_error");
            }
            if let Err(e) = ctx
                .store
                .source_set_insight(source, row.insights, &row.meta)
                .await
            {
                tracing::warn!(%source, "insights count not written: {e:#}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(%source, "source row not read: {e:#}"),
    }
    ctx.rt
        .publish(SourceEvent::Changed {
            source: source.clone(),
            status: SourceStatus::Completed,
        })
        .await;
    Ok(block)
}

async fn record_error(ctx: &Ctx, source: &DocId, error: &str) {
    if let Ok(Some(mut row)) = ctx.store.source_row(source).await {
        if !row.meta.is_object() {
            row.meta = serde_json::json!({});
        }
        if let Some(o) = row.meta.as_object_mut() {
            o.insert(
                "insight_error".into(),
                serde_json::Value::String(error.to_owned()),
            );
        }
        if let Err(e) = ctx
            .store
            .source_set_insight(source, row.insights, &row.meta)
            .await
        {
            tracing::warn!(%source, "insight_error not written: {e:#}");
        }
        ctx.rt
            .publish(SourceEvent::Changed {
                source: source.clone(),
                status: SourceStatus::Completed,
            })
            .await;
    }
}

async fn markdown(ctx: &Ctx, source: &DocId, annotate: bool) -> Result<String, String> {
    ctx.docs
        .ask(Markdown {
            doc: source.clone(),
            blocks: None,
            annotate,
        })
        .await
        .map_err(flat_err)
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

// ---- insight_run / insight_promote ---------------------------------------------------------------

/// `ask` → `Result<InsightRun, String>`: one more insight section (§4.3 "more on demand"). Hub only.
#[derive(Clone, Debug)]
pub struct RunInsight {
    /// The source.
    pub source: DocId,
    /// What to produce.
    pub instruction: InsightInstruction,
    /// The calling chat, when a tool asks (charged there); `None` from the UI.
    pub session: Option<SessionId>,
}

impl Message<RunInsight> for IngestActor {
    type Reply = DelegatedReply<Result<InsightRun, String>>;

    async fn handle(&mut self, m: RunInsight, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if let Err(e) = self.gate().await {
            tx.send(Err(e));
            return reply;
        }
        let c = self.ctx.clone();
        tokio::spawn(async move {
            let out = async {
                let (heading, instruction) = resolve(&m.instruction)?;
                generate(&c, &m.source, &heading, &instruction, m.session)
                    .await
                    .map(|block| InsightRun { block })
            }
            .await;
            tx.send(out);
        });
        reply
    }
}

/// `ask` → `Result<DocId, String>`: an insight section as a new note (§4.3 "promote to note"). Hub only.
#[derive(Clone, Debug)]
pub struct PromoteInsight {
    /// The source.
    pub source: DocId,
    /// The `###` heading block, or any block inside the section.
    pub block: String,
    /// The notebook to file the note in; `None` = every live notebook holding the source.
    pub notebook: Option<DocId>,
    /// Who creates the note.
    pub by: Principal,
}

impl Message<PromoteInsight> for IngestActor {
    type Reply = DelegatedReply<Result<DocId, String>>;

    async fn handle(
        &mut self,
        m: PromoteInsight,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if let Err(e) = self.gate().await {
            tx.send(Err(e));
            return reply;
        }
        let c = self.ctx.clone();
        tokio::spawn(async move {
            tx.send(promote(&c, &m.source, &m.block, m.notebook, m.by).await);
        });
        reply
    }
}

/// `doc_create(title = "<insight> — <source title>", markdown = the section's blocks)` + the trailing
/// `*From [<source title>](theuth://sources/<id>#<block>) · <model> · <date>*`, filed in `notebook` (or in
/// every live notebook holding the source).
async fn promote(
    ctx: &Ctx,
    source: &DocId,
    block: &str,
    notebook: Option<DocId>,
    by: Principal,
) -> Result<DocId, String> {
    let annotated = markdown(ctx, source, true).await?;
    let (heading, body) = section_of(&annotated, block)
        .ok_or_else(|| format!("block {block} is not inside an insight section of {source}"))?;
    let (name, model, date) = split_heading(&heading);
    let source_title = ctx
        .store
        .summary(source)
        .await
        .map_err(|e| e.to_string())?
        .map(|s| s.title)
        .unwrap_or_else(|| source.to_string());
    let markdown = format!(
        "{body}\n\n*From [{source_title}](theuth://sources/{source}#{block}) · {model} · {date}*"
    );
    let doc = ctx
        .docs
        .ask(CreateNote {
            title: format!("{name} — {source_title}"),
            markdown,
            by: by.clone(),
            home: crate::actor::Homing::Hub,
        })
        .await
        .map_err(flat_err)?;
    let targets = match notebook {
        Some(n) => vec![n],
        None => ctx
            .store
            .notebooks_of(ItemKind::Source, source.as_str(), false)
            .await
            .map_err(|e| e.to_string())?,
    };
    for nb in targets {
        ctx.docs
            .ask(NotebookItemAdd {
                notebook: nb,
                kind: ItemKind::Note,
                item: doc.id.to_string(),
                by: by.clone(),
            })
            .await
            .map_err(flat_err)?;
    }
    Ok(doc.id)
}

/// `insight_run` entry point (the RPC and the tool). Hub only (`NotHub` off the hub).
pub async fn insight_run(
    _actor: &ActorRef<DocumentActor>,
    source: DocId,
    instruction: InsightInstruction,
    session: Option<SessionId>,
) -> anyhow::Result<InsightRun> {
    ingest()?
        .ask(RunInsight {
            source,
            instruction,
            session,
        })
        .await
        .map_err(|e| anyhow::anyhow!(flat_err(e)))
}

/// `insight_promote` entry point (the RPC and the tool). Hub only.
pub async fn insight_promote(
    _actor: &ActorRef<DocumentActor>,
    source: DocId,
    block: String,
    notebook: Option<DocId>,
    by: Principal,
) -> anyhow::Result<DocId> {
    ingest()?
        .ask(PromoteInsight {
            source,
            block,
            notebook,
            by,
        })
        .await
        .map_err(|e| anyhow::anyhow!(flat_err(e)))
}

// ---- Markdown helpers (pure) -----------------------------------------------------------------------

/// Whether the (plain or annotated) markdown has a `## Insights` section.
pub fn has_insights(markdown: &str) -> bool {
    markdown.lines().any(|l| l.trim_end() == "## Insights")
}

/// Whether an insight section named `heading` exists (`### <heading> · …`).
pub fn has_section(markdown: &str, heading: &str) -> bool {
    let prefix = format!("### {heading} · ");
    markdown.lines().any(|l| l.starts_with(&prefix))
}

/// The source's own text: everything before its `## Insights` heading (and the heading's `<!-- id -->` line
/// when the markdown is annotated).
pub fn strip_insights(markdown: &str) -> &str {
    let mut at = 0;
    let mut prev_comment: Option<usize> = None;
    for line in markdown.split_inclusive('\n') {
        let t = line.trim_end();
        if t == "## Insights" {
            return markdown[..prev_comment.unwrap_or(at)].trim_end();
        }
        prev_comment = t.starts_with("<!-- ").then_some(at);
        at += line.len();
    }
    markdown
}

/// Annotated markdown → `(block id, block markdown)` in order.
fn blocks(annotated: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in annotated.lines() {
        if let Some(id) = line
            .strip_prefix("<!-- ")
            .and_then(|r| r.strip_suffix(" -->"))
            .filter(|id| !id.contains(' '))
        {
            out.push((id.to_owned(), String::new()));
        } else if let Some((_, text)) = out.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    for (_, text) in &mut out {
        let t = text.trim().to_owned();
        *text = t;
    }
    out
}

/// The `###` section of `## Insights` that holds `block` (its heading or any block inside): the heading
/// text (after `### `) and the body blocks joined, ids stripped.
pub fn section_of(annotated: &str, block: &str) -> Option<(String, String)> {
    let blocks = blocks(annotated);
    let at = blocks.iter().position(|(id, _)| id == block)?;
    let insights = blocks[..=at]
        .iter()
        .rposition(|(_, t)| t.trim_end() == "## Insights")?;
    let head = blocks[insights..=at]
        .iter()
        .rposition(|(_, t)| t.starts_with("### "))
        .map(|i| i + insights)?;
    let heading = blocks[head].1.trim_start_matches("### ").trim().to_owned();
    let body: Vec<&str> = blocks[head + 1..]
        .iter()
        .map(|(_, t)| t.as_str())
        .take_while(|t| !(t.starts_with("# ") || t.starts_with("## ") || t.starts_with("### ")))
        .collect();
    Some((heading, body.join("\n\n")))
}

/// `<name> · <model> · <date>` → its three parts (`unknown` / today when the heading is not ours).
pub fn split_heading(heading: &str) -> (String, String, String) {
    let parts: Vec<&str> = heading.rsplitn(3, " · ").collect();
    match parts.as_slice() {
        [date, model, name] => ((*name).to_owned(), (*model).to_owned(), (*date).to_owned()),
        _ => (heading.to_owned(), "unknown".to_owned(), today()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<!-- a -->\n# Runbook\n\n<!-- b -->\nStep one.\n\n<!-- c -->\n## Insights\n\n<!-- d -->\n### Dense summary · anthropic/claude-haiku · 2026-10-09\n\n<!-- e -->\nA tight summary.\n\n<!-- f -->\n- point one\n- point two\n\n<!-- g -->\n### Open questions · m · 2026-10-10\n\n<!-- h -->\n- why?\n";

    /// Z2 live-proof defect: Sonnet's default insight opened with `# Summary` / `# Key points`, which `section_of`
    /// and the UI read as the end of the `###` section (an empty insight, an empty promoted note).
    #[test]
    fn model_headings_inside_an_insight_are_demoted_below_the_section() {
        let demoted = demote_headings(
            "# Summary\n\nText.\n\n## Key points\n\n- one\n\n### deep\n\n```\n# not a heading\n```\n#hashtag\n",
        );
        assert_eq!(
            demoted,
            "#### Summary\n\nText.\n\n#### Key points\n\n- one\n\n#### deep\n\n```\n# not a heading\n```\n#hashtag"
        );
        let doc = format!(
            "<!-- c -->\n## Insights\n\n<!-- d -->\n### Dense summary · m · 2026-10-09\n\n<!-- e -->\n{}\n",
            demote_headings("# Summary\n\nText.")
        );
        assert_eq!(section_of(&doc, "d").unwrap().1, "#### Summary\n\nText.");
    }

    #[test]
    fn section_lookup_from_heading_or_inner_block_and_the_heading_split() {
        let (h, body) = section_of(DOC, "d").unwrap();
        assert_eq!(h, "Dense summary · anthropic/claude-haiku · 2026-10-09");
        assert_eq!(body, "A tight summary.\n\n- point one\n- point two");
        assert_eq!(section_of(DOC, "f").unwrap().0, h);
        assert_eq!(
            section_of(DOC, "h").unwrap(),
            ("Open questions · m · 2026-10-10".into(), "- why?".into())
        );
        assert!(
            section_of(DOC, "b").is_none(),
            "a source block is not an insight"
        );
        assert!(section_of(DOC, "zz").is_none());
        assert_eq!(
            split_heading(&h),
            (
                "Dense summary".into(),
                "anthropic/claude-haiku".into(),
                "2026-10-09".into()
            )
        );
        assert_eq!(split_heading("Plain").0, "Plain");
    }

    #[test]
    fn insights_detection_and_strip() {
        assert!(has_insights(DOC));
        assert!(has_section(DOC, "Dense summary"));
        assert!(!has_section(DOC, "Table of contents"));
        assert_eq!(
            strip_insights(DOC),
            "<!-- a -->\n# Runbook\n\n<!-- b -->\nStep one."
        );
        assert!(!has_insights("# Runbook\n\nStep one.\n"));
        assert_eq!(strip_insights("plain"), "plain");
    }

    #[test]
    fn resolve_builtins_and_refuse_the_rest() {
        let (h, i) = resolve(&InsightInstruction {
            builtin: Some("toc".into()),
            skill: None,
        })
        .unwrap();
        assert_eq!(h, "Table of contents");
        assert!(i.starts_with("Table of contents:"));
        assert!(
            resolve(&InsightInstruction {
                builtin: Some("nope".into()),
                skill: None
            })
            .unwrap_err()
            .contains("key_insights, toc, open_questions")
        );
        assert!(resolve(&InsightInstruction::default()).is_err());
        assert_eq!(builtin_names(), ["key_insights", "toc", "open_questions"]);
    }
}
