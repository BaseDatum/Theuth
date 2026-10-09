// 144-E1 owns this file.
//! The context dial (design 144 §5.1–5.2): the per-thread `session_context` rows, the default rule, the
//! one planner both the `<notebook>` block and the pre-turn estimate run through ([`plan`]), the
//! `notebook_context_set` / `notebook_context_estimate` tools, the insight skills and the side call the
//! insight job charges to a notebook chat. Ids, kinds and levels are strings here: the agent crate does
//! not depend on `document` (`rpc` maps them onto the contract's types).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::side_call::CallKind;
use runtime::{ModelRole, Runtime, SessionId, SideCall, SideReply};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::{RegisterTool, ToolRegistry};
use tools::{REGISTRY_ACTOR, Risk, Tool, ToolCx, ToolOutput};

use crate::lifecycle::{CreateSession, CreateSessionOptions};
use crate::notebooks::{self, Budget, Item, cut, cut_blocks, invoke, tokens};
use crate::supervisor::Agents;
use crate::{AGENT_ACTOR, skills};

/// The four dial positions, least to most (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Not in the block at all.
    Off,
    /// One line.
    Mention,
    /// The mention + the source's insights (a note's first heading + 400 chars).
    Summary,
    /// The whole annotated markdown.
    Full,
}

impl Level {
    /// `off | mention | summary | full`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "mention" => Some(Self::Mention),
            "summary" => Some(Self::Summary),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// The column / attribute text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mention => "mention",
            Self::Summary => "summary",
            Self::Full => "full",
        }
    }
}

/// What the budget did to one item (§5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Injected whole at its level.
    Available,
    /// Cut to fit, with the §5.3 notice.
    Truncated,
    /// Left out: `off`, or beyond the budget.
    Omitted,
}

impl Status {
    /// The attribute text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Truncated => "truncated",
            Self::Omitted => "omitted",
        }
    }
}

const KINDS: [&str; 4] = ["source", "note", "session", "artifact"];

fn check_kind(kind: &str) -> Result<(), String> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(format!("kind must be one of {}", KINDS.join(", ")))
    }
}

fn check_level(level: &str) -> Result<Level, String> {
    Level::parse(level).ok_or_else(|| "level must be off, mention, summary or full".to_owned())
}

/// Sessions and artifacts are `mention`-only (§5.1): anything above is stored as `mention`.
fn clamp(kind: &str, level: Level) -> Level {
    if matches!(kind, "session" | "artifact") {
        level.min(Level::Mention)
    } else {
        level
    }
}

// ---- Storage: `Agents` messages over `session_context` -------------------------------------------

/// `ask` → `Result<(), String>`: pin `item` at `level` for `session`.
#[derive(Clone, Debug)]
pub struct ContextSet {
    /// The thread.
    pub session: SessionId,
    /// The item id.
    pub item: String,
    /// `source | note | session | artifact`.
    pub kind: String,
    /// `off | mention | summary | full`.
    pub level: String,
}

impl Message<ContextSet> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ContextSet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        check_kind(&m.kind)?;
        let level = clamp(&m.kind, check_level(&m.level)?);
        self.store_for(m.session)
            .context_set(m.session, &m.item, &m.kind, level.as_str())
            .await
            .map_err(|e| e.to_string())
    }
}

/// `ask` → `Result<(), String>`: back to the default rule for `item`.
#[derive(Clone, Debug)]
pub struct ContextClear {
    /// The thread.
    pub session: SessionId,
    /// The item id.
    pub item: String,
}

impl Message<ContextClear> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ContextClear, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store_for(m.session)
            .context_clear(m.session, &m.item)
            .await
            .map_err(|e| e.to_string())
    }
}

/// `ask` → `Result<(), String>`: pin every `(item, kind)` of a notebook at `level` (the bulk toggles);
/// [`context_set_all`] resolves the notebook's items first, outside the actor.
#[derive(Clone, Debug)]
pub struct ContextSetAll {
    /// The thread.
    pub session: SessionId,
    /// `(item, kind)` rows.
    pub items: Vec<(String, String)>,
    /// `off | mention | summary | full`.
    pub level: String,
}

impl Message<ContextSetAll> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ContextSetAll,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let level = check_level(&m.level)?;
        let store = self.store_for(m.session);
        for (item, kind) in &m.items {
            check_kind(kind)?;
            store
                .context_set(m.session, item, kind, clamp(kind, level).as_str())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// `ask` → `Result<Vec<(item, kind, level)>, String>`: the pins of `session`.
#[derive(Clone, Copy, Debug)]
pub struct ContextGet {
    /// The thread.
    pub session: SessionId,
}

impl Message<ContextGet> for Agents {
    type Reply = Result<Vec<(String, String, String)>, String>;

    async fn handle(&mut self, m: ContextGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store_for(m.session)
            .context_get(m.session)
            .await
            .map_err(|e| e.to_string())
    }
}

/// `ask` → `Vec<Option<String>>`: display titles of sessions (the block's `session` items), positionally.
#[derive(Clone, Debug)]
pub struct SessionTitles(pub Vec<SessionId>);

impl Message<SessionTitles> for Agents {
    type Reply = Vec<Option<String>>;

    async fn handle(
        &mut self,
        m: SessionTitles,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut out = Vec::with_capacity(m.0.len());
        for id in m.0 {
            let title = match self.store_for(id).session(id).await {
                Ok(Some(s)) => Some(crate::attach::session_title(
                    &s.title,
                    None,
                    Some(s.created_at),
                )),
                _ => None,
            };
            out.push(title);
        }
        out
    }
}

fn err<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Pin `item` at `level` for `session` (the RPC body).
pub async fn context_set(
    agents: &ActorRef<Agents>,
    session: SessionId,
    item: String,
    kind: String,
    level: String,
) -> anyhow::Result<()> {
    agents
        .ask(ContextSet {
            session,
            item,
            kind,
            level,
        })
        .await
        .map_err(err)
}

/// Forget the pin (the RPC body).
pub async fn context_clear(
    agents: &ActorRef<Agents>,
    session: SessionId,
    item: String,
) -> anyhow::Result<()> {
    agents
        .ask(ContextClear { session, item })
        .await
        .map_err(err)
}

/// Pin every live item of `notebook` (of `kind` when given) at `level` (the RPC body): `notebook_get`
/// through the registry as `session`, then one [`ContextSetAll`].
pub async fn context_set_all(
    agents: &ActorRef<Agents>,
    session: SessionId,
    notebook: String,
    kind: Option<String>,
    level: String,
) -> anyhow::Result<()> {
    let detail = invoke(
        session,
        "notebook_get",
        serde_json::json!({ "id": notebook }),
    )
    .await?
    .structured
    .unwrap_or_default();
    let items: Vec<(String, String)> = detail["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let k = row["kind"].as_str()?;
            let item = row["item"].as_str()?;
            (kind.as_deref().is_none_or(|want| want == k)).then(|| (item.to_owned(), k.to_owned()))
        })
        .collect();
    agents
        .ask(ContextSetAll {
            session,
            items,
            level,
        })
        .await
        .map_err(err)
}

/// The pins of `session` as `item → level` (the planner's input).
pub async fn pins(agents: &ActorRef<Agents>, session: SessionId) -> anyhow::Result<Pins> {
    let rows = agents.ask(ContextGet { session }).await.map_err(err)?;
    Ok(pins_of(rows))
}

/// `item → level` from `(item, kind, level)` rows; an unreadable level row is ignored.
pub fn pins_of(rows: Vec<(String, String, String)>) -> Pins {
    rows.into_iter()
        .filter_map(|(item, kind, level)| Level::parse(&level).map(|l| (item, clamp(&kind, l))))
        .collect()
}

/// `item → pinned level`.
pub type Pins = HashMap<String, Level>;

// ---- The planner: one source of truth for the block and the estimate -----------------------------

/// One item after the default rule and the budget: what the block injects and what the estimate counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed<'a> {
    /// The item.
    pub item: &'a Item,
    /// The effective level.
    pub level: Level,
    /// A `session_context` row decided it.
    pub pinned: bool,
    /// What the budget did.
    pub status: Status,
    /// Tokens of `text` (chars / 4).
    pub tokens: u32,
    /// What goes inside `<item>…</item>` (the §5.3 notice included when truncated); empty when `off`.
    pub text: String,
}

/// §5.1: the effective level per item — pinned if present, else `summary` for a source with a
/// `## Insights` section, `full` for other sources and notes, `mention` for sessions and artifacts —
/// taken newest first per level in the order full → summary → mention while the running total stays
/// under `budget` tokens; the first overflow is `truncated` at a `<!-- id -->` boundary, the rest fall
/// to `mention` / `omitted`; `off` items are omitted. The result is in injection order.
pub fn plan<'a>(items: &'a [Item], pins: &Pins, budget: u32) -> Vec<Placed<'a>> {
    let mut order: Vec<&Item> = items.iter().collect();
    order.sort_by_key(|i| std::cmp::Reverse(i.added_ms));
    let requested: Vec<(&Item, Level, bool)> = order
        .into_iter()
        .map(|it| match pins.get(&it.id) {
            Some(&l) => (it, clamp(&it.kind, l), true),
            None => (it, default_level(it), false),
        })
        .collect();
    let mut left = budget;
    let mut spent_truncation = false;
    let mut out: Vec<Placed<'a>> = Vec::with_capacity(requested.len());
    for want in [Level::Full, Level::Summary, Level::Mention, Level::Off] {
        for &(it, level, pinned) in requested.iter().filter(|(_, l, _)| *l == want) {
            let placed = match level {
                Level::Off => Placed {
                    item: it,
                    level,
                    pinned,
                    status: Status::Omitted,
                    tokens: 0,
                    text: String::new(),
                },
                Level::Mention => {
                    let text = mention_line(it);
                    let tokens = tokens(&text);
                    left = left.saturating_sub(tokens);
                    Placed {
                        item: it,
                        level,
                        pinned,
                        status: Status::Available,
                        tokens,
                        text,
                    }
                }
                Level::Summary | Level::Full => {
                    let text = if level == Level::Full {
                        it.body.clone()
                    } else {
                        it.body.as_deref().map(|b| summary_text(it, b))
                    };
                    match text {
                        Some(text) => {
                            let total = tokens(&text);
                            if total <= left {
                                left -= total;
                                Placed {
                                    item: it,
                                    level,
                                    pinned,
                                    status: Status::Available,
                                    tokens: total,
                                    text,
                                }
                            } else if !spent_truncation
                                && let Some((kept, first_omitted)) =
                                    cut_blocks(&text, left as usize * 4)
                            {
                                spent_truncation = true;
                                let shown = tokens(kept);
                                left = left.saturating_sub(shown);
                                let omitted = first_omitted
                                    .map(|b| format!("; first omitted block {b}"))
                                    .unwrap_or_default();
                                Placed {
                                    item: it,
                                    level,
                                    pinned,
                                    status: Status::Truncated,
                                    tokens: shown,
                                    text: format!(
                                        "{}\n[truncated: {shown} of {total} tokens shown; the rest with doc_read(blocks=[…]){omitted}]",
                                        kept.trim_end()
                                    ),
                                }
                            } else {
                                spent_truncation = true;
                                demoted(it, pinned, &mut left)
                            }
                        }
                        // The read failed: nothing to inject beyond the line.
                        None => {
                            let text = mention_line(it);
                            let tokens = tokens(&text);
                            left = left.saturating_sub(tokens);
                            Placed {
                                item: it,
                                level: Level::Mention,
                                pinned,
                                status: Status::Available,
                                tokens,
                                text,
                            }
                        }
                    }
                }
            };
            out.push(placed);
        }
    }
    // A demoted item renders among the mentions: full/summary first, then mention, then off.
    out.sort_by_key(|p| std::cmp::Reverse(p.level));
    out
}

/// Beyond the budget: the mention line, `omitted`.
fn demoted<'a>(it: &'a Item, pinned: bool, left: &mut u32) -> Placed<'a> {
    let text = mention_line(it);
    let tokens = tokens(&text);
    *left = left.saturating_sub(tokens);
    Placed {
        item: it,
        level: Level::Mention,
        pinned,
        status: Status::Omitted,
        tokens,
        text,
    }
}

/// The default rule's level for an unpinned item.
pub fn default_level(it: &Item) -> Level {
    match it.kind.as_str() {
        "source" => {
            if it.body.as_deref().is_some_and(has_insights) {
                Level::Summary
            } else {
                Level::Full
            }
        }
        "note" => Level::Full,
        _ => Level::Mention,
    }
}

/// The one-line form (§5.1 `mention`).
pub fn mention_line(it: &Item) -> String {
    match it.kind.as_str() {
        "source" => format!(
            "mention only: {} words; read with doc_read(\"source:{}\") if needed",
            it.words, it.id
        ),
        "note" => format!(
            "mention only: {} words; read with doc_read(\"{}\") if needed",
            it.words, it.id
        ),
        "session" => format!("mention only: read with session_read(\"{}\")", it.id),
        _ => format!("mention only: read with artifact_read(\"{}\")", it.id),
    }
}

/// §5.1 `summary`: the mention line + the `## Insights` section's blocks (ids kept), or — a note, or a
/// source without insights — its first heading and the first 400 chars after it.
pub fn summary_text(it: &Item, body: &str) -> String {
    let rest = match insights_section(body) {
        Some(s) => s.trim_end().to_owned(),
        None => teaser(body),
    };
    format!("{}\n{rest}", mention_line(it))
}

/// Whether the annotated markdown has a `## Insights` heading.
pub fn has_insights(body: &str) -> bool {
    body.lines().any(|l| l.trim_end() == "## Insights")
}

/// The `## Insights` section (its `<!-- id -->` line included) up to the next level-1/2 heading.
pub fn insights_section(body: &str) -> Option<&str> {
    let mut at = 0;
    let mut start = None;
    let mut prev_comment: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let t = line.trim_end();
        match start {
            None if t == "## Insights" => start = Some(prev_comment.unwrap_or(at)),
            Some(s) if t.starts_with("# ") || t.starts_with("## ") => {
                return Some(&body[s..prev_comment.unwrap_or(at)]);
            }
            _ => {}
        }
        prev_comment = t.starts_with("<!-- ").then_some(at);
        at += line.len();
    }
    start.map(|s| &body[s..])
}

/// A note's first heading (with its id line) and the first 400 chars after it.
fn teaser(body: &str) -> String {
    let mut at = 0;
    let mut prev_comment: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let t = line.trim_end();
        if t.starts_with('#') {
            let head_start = prev_comment.unwrap_or(at);
            let head_end = at + line.len();
            let rest = &body[head_end..];
            return format!("{}{}", &body[head_start..head_end], cut(rest, 400))
                .trim_end()
                .to_owned();
        }
        prev_comment = t.starts_with("<!-- ").then_some(at);
        at += line.len();
    }
    cut(body, 400).trim_end().to_owned()
}

// ---- The estimate ----------------------------------------------------------------------------------

/// One row of the estimate (the contract's `ContextItem`, strings for kind).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstimateItem {
    /// The item id.
    pub id: String,
    /// `source | note | session | artifact`.
    pub kind: String,
    /// Display title.
    pub title: String,
    /// The effective level.
    pub level: Level,
    /// A pin decided it.
    pub pinned: bool,
    /// Tokens at `level`.
    pub tokens: u32,
    /// What the budget did.
    pub status: Status,
}

/// The pre-turn estimate (the contract's `ContextEstimate`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    /// Every item, in injection order.
    pub items: Vec<EstimateItem>,
    /// Σ `tokens`.
    pub total_tokens: u32,
    /// `notebooks.context_budget_tokens`.
    pub budget_tokens: u32,
    /// `off` or beyond the budget.
    pub omitted: u32,
    /// Nothing injected: the block carries the "nothing in context" sentence.
    pub empty: bool,
}

/// The estimate of a plan.
pub fn estimate_of(placed: &[Placed<'_>], budget_tokens: u32) -> Estimate {
    let items: Vec<EstimateItem> = placed
        .iter()
        .map(|p| EstimateItem {
            id: p.item.id.clone(),
            kind: p.item.kind.clone(),
            title: p.item.title.clone(),
            level: p.level,
            pinned: p.pinned,
            tokens: p.tokens,
            status: p.status,
        })
        .collect();
    Estimate {
        total_tokens: items.iter().map(|i| i.tokens).sum(),
        budget_tokens,
        omitted: items.iter().filter(|i| i.status == Status::Omitted).count() as u32,
        empty: placed.iter().all(|p| p.level == Level::Off),
        items,
    }
}

/// `notebook_context_estimate(notebook, session)`: read-only, no model call, nothing cached. The items
/// come through the registry as `session` (a fresh id when `None`: the default rule with no pins).
pub async fn estimate(
    agents: &ActorRef<Agents>,
    session: Option<SessionId>,
    notebook: &str,
    budget: Budget,
) -> anyhow::Result<Estimate> {
    let pins = match session {
        Some(s) => pins(agents, s).await?,
        None => Pins::new(),
    };
    let (_, _, mut items) = notebooks::fetch(None, session.unwrap_or_default(), notebook).await?;
    let sessions: Vec<(usize, SessionId)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.kind == "session")
        .filter_map(|(n, i)| i.id.parse().ok().map(|s| (n, s)))
        .collect();
    if !sessions.is_empty()
        && let Ok(titles) = agents
            .ask(SessionTitles(sessions.iter().map(|(_, s)| *s).collect()))
            .await
    {
        for ((n, _), title) in sessions.iter().zip(titles) {
            if let Some(t) = title {
                items[*n].title = t;
            }
        }
    }
    let placed = plan(&items, &pins, budget.context_budget_tokens);
    Ok(estimate_of(&placed, budget.context_budget_tokens))
}

// ---- Tools -----------------------------------------------------------------------------------------

fn agents() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("agent is not running"))
}

/// Arguments for [`ContextSetTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextSetArgs {
    /// The item id (a source or note document id, a session id, an artifact call id).
    pub item: String,
    /// `source | note | session | artifact`.
    pub kind: String,
    /// `off | mention | summary | full` (sessions and artifacts are `mention` at most).
    pub level: String,
    /// The thread; default: this one.
    #[serde(default)]
    pub session: Option<String>,
}

/// `notebook_context_set` (ReadOnly: it changes what the model sees, nothing the user keeps).
pub struct ContextSetTool;

#[async_trait::async_trait]
impl Tool for ContextSetTool {
    const NAME: &'static str = "notebook_context_set";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ContextSetArgs;

    fn description(&self) -> String {
        "Set how much of a notebook item this thread's `<notebook>` block carries from the next turn on: \
         `off` (not mentioned), `mention` (one line), `summary` (the source's insights), `full` (the whole \
         text). Sessions and artifacts are `mention` at most. Pinned until cleared; the default rule picks \
         for the rest."
            .into()
    }

    async fn call(&self, a: ContextSetArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let session = match a.session.as_deref() {
            Some(s) => s.parse::<SessionId>()?,
            None => cx.session,
        };
        context_set(
            &agents()?,
            session,
            a.item.clone(),
            a.kind.clone(),
            a.level.clone(),
        )
        .await?;
        Ok(ToolOutput::ok(format!(
            "{} {} → {} for thread {session} (applies from the next turn)",
            a.kind, a.item, a.level
        )))
    }
}

/// Arguments for [`ContextEstimateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextEstimateArgs {
    /// The notebook id.
    pub notebook: String,
    /// The thread whose pins apply; default: this one.
    #[serde(default)]
    pub session: Option<String>,
}

/// `notebook_context_estimate` (ReadOnly): what the next turn's block will carry.
pub struct ContextEstimateTool {
    /// For `notebooks.context_budget_tokens`.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ContextEstimateTool {
    const NAME: &'static str = "notebook_context_estimate";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = ContextEstimateArgs;

    fn description(&self) -> String {
        "What the `<notebook>` block of this thread's next turn will carry for a notebook: per item its \
         effective level, whether it is pinned, its tokens and status (available | truncated | omitted), \
         the total against the budget. No model call."
            .into()
    }

    async fn call(&self, a: ContextEstimateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let session = match a.session.as_deref() {
            Some(s) => s.parse::<SessionId>()?,
            None => cx.session,
        };
        let budget = Budget::from_runtime(&self.rt);
        let est = estimate(&agents()?, Some(session), &a.notebook, budget).await?;
        let mut text = format!(
            "{} of {} tokens · {} omitted{}\n",
            est.total_tokens,
            est.budget_tokens,
            est.omitted,
            if est.empty {
                " · nothing in context"
            } else {
                ""
            }
        );
        for i in &est.items {
            text.push_str(&format!(
                "- {} {} {:?} {}{} {} ({} tokens)\n",
                i.kind,
                i.id,
                i.title,
                i.level.as_str(),
                if i.pinned { " (pinned)" } else { "" },
                i.status.as_str(),
                i.tokens
            ));
        }
        Ok(ToolOutput {
            structured: serde_json::to_value(&est).ok(),
            ..ToolOutput::ok(text)
        })
    }
}

/// Register the two tools once the registry is up (the agent plugin's start).
pub fn register(rt: &Runtime) {
    let rt = rt.clone();
    tokio::spawn(async move {
        let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await else {
            return;
        };
        let _ = reg.ask(RegisterTool(Arc::new(ContextSetTool))).await;
        let _ = reg
            .ask(RegisterTool(Arc::new(ContextEstimateTool { rt })))
            .await;
    });
}

// ---- Insight skills and the side call -------------------------------------------------------------

/// Skills whose front matter has `notebook_insight: true`, by name (after the builtins in
/// `insight_skills()`).
pub fn insight_skills(dir: &Path) -> Vec<String> {
    skills::index(dir)
        .into_iter()
        .filter(|s| notebook_insight_flag(Path::new(&s.path)))
        .map(|s| s.name)
        .collect()
}

/// `(section heading, instruction)` of the insight skill `name`: its description (else its name) and its body.
pub fn insight_skill(dir: &Path, name: &str) -> Result<(String, String), String> {
    let meta = skills::find(dir, name)?;
    if !notebook_insight_flag(Path::new(&meta.path)) {
        return Err(format!(
            "skill `{name}` is not an insight skill (front matter `notebook_insight: true`)"
        ));
    }
    let body = skills::body(dir, &meta).map_err(|e| format!("{name}: {e}"))?;
    let heading = if meta.description.trim().is_empty() {
        meta.name
    } else {
        meta.description
    };
    Ok((heading, body.body.trim().to_owned()))
}

#[derive(Deserialize, Default)]
struct InsightMatter {
    #[serde(default)]
    notebook_insight: bool,
}

/// `notebook_insight: true` in a skill file's front matter.
fn notebook_insight_flag(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    gray_matter::Matter::<gray_matter::engine::YAML>::new()
        .parse::<InsightMatter>(&text)
        .ok()
        .and_then(|p| p.data)
        .is_some_and(|m| m.notebook_insight)
}

/// `ask` → `Result<(), String>`: make sure `session`'s actor is live (resumed from the store when idle),
/// so a [`SideCall`] on it is answered.
#[derive(Clone, Copy, Debug)]
pub struct WakeSession(pub SessionId);

impl Message<WakeSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: WakeSession, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.actor(m.0)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    }
}

/// The insight job's model call (§4.3): `ModelRole::Named(model)` when `notebooks.insight_model` is set,
/// else the title chain; charged to `session` when given, else to the newest chat bound to one of
/// `notebooks` (woken when idle), else to a new chat bound to the first notebook — a side call is answered
/// by a live session actor only.
pub async fn insight_side_call(
    agents: &ActorRef<Agents>,
    session: Option<SessionId>,
    notebooks: &[String],
    model: Option<String>,
    system: String,
    user: String,
    budget: Duration,
) -> Result<SideReply, String> {
    let role = model.map_or(ModelRole::Title, ModelRole::Named);
    let mut candidates: Vec<SessionId> = session.into_iter().collect();
    for nb in notebooks {
        if let Ok(chats) = notebooks::notebook_chats(agents, nb.clone()).await {
            candidates.extend(chats.into_iter().map(|c| c.id));
        }
    }
    if candidates.is_empty() {
        let Some(nb) = notebooks.first() else {
            return Err("no chat to run the insight on: the source is in no notebook".to_owned());
        };
        let opened = agents
            .ask(CreateSession {
                options: CreateSessionOptions {
                    notebook: Some(nb.clone()),
                    ..Default::default()
                },
                peer: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        candidates.push(opened.session);
    }
    let mut last = String::new();
    for id in candidates {
        if let Err(e) = agents.ask(WakeSession(id)).await {
            last = e.to_string();
            continue;
        }
        match agents
            .ask(SideCall {
                session: id,
                role: role.clone(),
                system: system.clone(),
                user: user.clone(),
                budget,
                kind: CallKind::Title,
            })
            .await
        {
            Ok(r) => return Ok(r),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: &str, body: Option<&str>, added_ms: i64) -> Item {
        Item {
            kind: kind.into(),
            id: id.into(),
            title: id.into(),
            origin: String::new(),
            body: body.map(str::to_owned),
            words: 3,
            added_ms,
        }
    }

    #[test]
    fn insights_section_and_teaser() {
        let body = "<!-- a -->\n# T\n\n<!-- b -->\nbody\n\n<!-- c -->\n## Insights\n\n<!-- d -->\n### Dense summary · m · d\n\n<!-- e -->\nsum\n";
        assert_eq!(
            insights_section(body),
            Some(
                "<!-- c -->\n## Insights\n\n<!-- d -->\n### Dense summary · m · d\n\n<!-- e -->\nsum\n"
            )
        );
        let more = format!("{body}\n<!-- f -->\n## Appendix\n\n<!-- g -->\nx\n");
        assert!(insights_section(&more).unwrap().ends_with("sum\n\n"));
        assert_eq!(insights_section("no insights"), None);
        assert_eq!(
            teaser("<!-- a -->\n# T\n\n<!-- b -->\nbody"),
            "<!-- a -->\n# T\n\n<!-- b -->\nbody"
        );
        assert_eq!(
            teaser(&format!("<!-- a -->\n# T\n\n{}", "x".repeat(500))).len(),
            415
        );
    }

    #[test]
    fn default_rule_pins_and_clamps() {
        let s_ins = item(
            "source",
            "S",
            Some("<!-- a -->\n## Insights\n\n<!-- b -->\nsum"),
            2,
        );
        let s_raw = item("source", "R", Some("<!-- a -->\nraw"), 1);
        let n = item("note", "N", Some("<!-- a -->\n# N\n\n<!-- b -->\nnote"), 3);
        let c = item("session", "C", None, 4);
        let items = vec![s_ins.clone(), s_raw.clone(), n.clone(), c.clone()];
        let p = plan(&items, &Pins::new(), 40_000);
        let got: Vec<(&str, Level, Status, bool)> = p
            .iter()
            .map(|p| (p.item.id.as_str(), p.level, p.status, p.pinned))
            .collect();
        assert_eq!(
            got,
            [
                ("N", Level::Full, Status::Available, false),
                ("R", Level::Full, Status::Available, false),
                ("S", Level::Summary, Status::Available, false),
                ("C", Level::Mention, Status::Available, false),
            ]
        );
        let mut pins = Pins::new();
        pins.insert("S".into(), Level::Off);
        pins.insert("C".into(), Level::Full);
        pins.insert("R".into(), Level::Summary);
        let p = plan(&items, &pins, 40_000);
        let got: Vec<(&str, Level, Status, bool)> = p
            .iter()
            .map(|p| (p.item.id.as_str(), p.level, p.status, p.pinned))
            .collect();
        assert_eq!(
            got,
            [
                ("N", Level::Full, Status::Available, false),
                ("R", Level::Summary, Status::Available, true),
                ("C", Level::Mention, Status::Available, true),
                ("S", Level::Off, Status::Omitted, true),
            ]
        );
        let e = estimate_of(&p, 40_000);
        assert_eq!((e.omitted, e.empty, e.budget_tokens), (1, false, 40_000));
        assert_eq!(e.total_tokens, p.iter().map(|p| p.tokens).sum::<u32>());
        let all_off: Pins = ["S", "R", "N", "C"]
            .into_iter()
            .map(|id| (id.to_owned(), Level::Off))
            .collect();
        let e = estimate_of(&plan(&items, &all_off, 40_000), 40_000);
        assert!(e.empty && e.omitted == 4 && e.total_tokens == 0);
        assert!(estimate_of(&plan(&[], &Pins::new(), 40_000), 40_000).empty);
    }

    #[test]
    fn budget_cuts_newest_first_then_demotes() {
        let blk = |id: &str| format!("<!-- {id} -->\n{}\n", "x".repeat(27));
        let items = vec![
            item("note", "OLD", Some(&blk("o1")), 1),
            item(
                "note",
                "MID",
                Some(&format!("{}{}{}", blk("m1"), blk("m2"), blk("m3"))),
                2,
            ),
            item("note", "NEW", Some(&blk("n1")), 3),
        ];
        let p = plan(&items, &Pins::new(), 32);
        assert_eq!(
            p.iter()
                .map(|p| (p.item.id.as_str(), p.status))
                .collect::<Vec<_>>(),
            [
                ("NEW", Status::Available),
                ("MID", Status::Truncated),
                ("OLD", Status::Omitted)
            ]
        );
        assert_eq!(p[1].tokens, 20);
        assert!(p[1].text.ends_with("[truncated: 20 of 30 tokens shown; the rest with doc_read(blocks=[…]); first omitted block m3]"));
        assert_eq!(p[2].level, Level::Mention);
        assert_eq!(
            p[2].text,
            "mention only: 3 words; read with doc_read(\"OLD\") if needed"
        );
        let e = estimate_of(&p, 32);
        assert_eq!(e.omitted, 1);
        assert_eq!(e.total_tokens, 10 + 20 + tokens(&p[2].text));
    }

    #[test]
    fn pins_rows_parse_and_clamp() {
        let p = pins_of(vec![
            ("A".into(), "source".into(), "full".into()),
            ("B".into(), "session".into(), "full".into()),
            ("C".into(), "note".into(), "bogus".into()),
        ]);
        assert_eq!(p.get("A"), Some(&Level::Full));
        assert_eq!(p.get("B"), Some(&Level::Mention));
        assert!(!p.contains_key("C"));
        assert_eq!(clamp("artifact", Level::Summary), Level::Mention);
        assert_eq!(check_level("summary"), Ok(Level::Summary));
        assert!(check_level("loud").is_err() && check_kind("blob").is_err());
    }
}
