//! Agent tools for Decks (design doc 229).
//!
//! Security boundary:
//! - Agents may propose scripts (`deck_script_propose`), which ALWAYS enter `pending_review`.
//! - Agents CANNOT self-approve scripts (`deck_script_approve` is owner RPC only).
//! - Normal session tool risk classes (`Risk::ReadOnly` / `Risk::Mutating`) are preserved.

use std::sync::Arc;

use artifact::layout::{self, LayoutOp, Outcome, PanePlacement};
use document::Principal;
use document::deck::{DeckDocError, SectionKind};
use kameo::actor::ActorRef;
use runtime::Runtime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::model::{DeclaredAction, RuntimeControlOp, ScriptMode};
use crate::owner::{
    CreateDeckMsg, DeckSupervisor, FocusSectionMsg, GetDeck, InteractMsg, ListDecks,
    ProposeScriptMsg, ReadScriptMsg, RuntimeControlMsg, SectionAddMsg, SectionArchiveMsg,
    SectionHistoryMsg, SectionMoveMsg, SectionPatchMsg,
};

/// Embedded onboarding & authoring guide for Decks (installed into `<data_dir>/skills/decks.md`).
pub const DECKS_SKILL: &str = include_str!("../decks.md");

const PROMPT_REMINDER: &str = "\
Decks are native collaborative live documents (sections + review-gated Monty scripts + full session/MCP tools). \
Use `deck_list`, `deck_create`, `deck_read`, `deck_section_*`, `deck_script_propose/read/run`, `deck_runtime`, `deck_interact`, and `show_deck`. \
Scripts proposed via `deck_script_propose` require explicit owner approval before streaming or running; never fabricate connected data (label example data clearly).";

#[derive(Clone)]
pub struct DecksTools {
    actor: ActorRef<DeckSupervisor>,
    rt: Runtime,
}

/// Register all Deck agent tools and concise system prompt note on `ToolRegistry`.
pub async fn register(reg: &ActorRef<ToolRegistry>, actor: ActorRef<DeckSupervisor>, rt: Runtime) {
    let t = DecksTools { actor, rt };
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "decks",
            text: PROMPT_REMINDER.into(),
        })
        .await;
    let tools: Vec<tools::DynTool> = vec![
        Arc::new(DeckListTool(t.clone())),
        Arc::new(DeckCreateTool(t.clone())),
        Arc::new(DeckReadTool(t.clone())),
        Arc::new(DeckSectionAddTool(t.clone())),
        Arc::new(DeckSectionPatchTool(t.clone())),
        Arc::new(DeckSectionMoveTool(t.clone())),
        Arc::new(DeckSectionArchiveTool(t.clone())),
        Arc::new(DeckSectionHistoryTool(t.clone())),
        Arc::new(DeckFocusTool(t.clone())),
        Arc::new(DeckScriptProposeTool(t.clone())),
        Arc::new(DeckScriptReadTool(t.clone())),
        Arc::new(DeckScriptRunTool(t.clone())),
        Arc::new(DeckRuntimeTool(t.clone())),
        Arc::new(DeckInteractTool(t.clone())),
        Arc::new(ShowDeckTool(t)),
    ];
    for tool in tools {
        let _ = reg.ask(RegisterTool(tool)).await;
    }
}

fn agent_principal(cx: &ToolCx) -> Principal {
    Principal::Agent {
        session: cx.session.0,
    }
}

fn structured<T: Serialize>(text: String, data: &T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn clean_deck_id(raw: &str) -> String {
    raw.trim()
        .strip_prefix("theuth://decks/")
        .or_else(|| raw.trim().strip_prefix("/decks/"))
        .or_else(|| raw.trim().strip_prefix("deck:"))
        .unwrap_or(raw.trim())
        .split('#')
        .next()
        .unwrap_or(raw.trim())
        .to_owned()
}

fn map_doc_send_err<M>(e: kameo::error::SendError<M, DeckDocError>) -> anyhow::Error {
    match e {
        kameo::error::SendError::HandlerError(doc_err) => anyhow::anyhow!("{doc_err}"),
        other => anyhow::anyhow!("{other}"),
    }
}

// ---------------------------------------------------------------------------
// deck_list
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DeckListArgs {
    /// Maximum number of Decks to return (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

pub struct DeckListTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckListTool {
    const NAME: &'static str = "deck_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list";
    type Args = DeckListArgs;

    fn description(&self) -> String {
        "List collaborative Decks with title, section count, runtime state, staleness, and active/pending script revisions.".into()
    }

    async fn call(&self, a: DeckListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let items = self
            .0
            .actor
            .ask(ListDecks {
                limit: Some(a.limit.unwrap_or(50)),
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = if items.is_empty() {
            "No Decks yet. Use deck_create(title=...) or deck_create(example=true) to create one."
                .into()
        } else {
            items
                .iter()
                .map(|d| {
                    format!(
                        "- {} ({}) — {} sections, state={:?}, active_rev={:?}, pending_rev={:?}",
                        d.title, d.id, d.section_count, d.state, d.active_rev, d.pending_rev
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(structured(text, &items))
    }
}

// ---------------------------------------------------------------------------
// deck_create
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DeckCreateArgs {
    /// Deck title (optional when `example` is true).
    #[serde(default)]
    pub title: Option<String>,
    /// Short description of the Deck's purpose.
    #[serde(default)]
    pub description: Option<String>,
    /// When true, scaffold the built-in `Workspace Pulse (Example)` Deck with hybrid sections and review-gated example script.
    #[serde(default)]
    pub example: bool,
}

pub struct DeckCreateTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckCreateTool {
    const NAME: &'static str = "deck_create";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckCreateArgs;

    fn description(&self) -> String {
        "Create a new collaborative Deck (DocKind::Deck) with its own isolated Monty SessionId. Set example=true to scaffold the Workspace Pulse (Example) Deck.".into()
    }

    async fn call(&self, a: DeckCreateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let detail = self
            .0
            .actor
            .ask(CreateDeckMsg {
                title: a.title,
                description: a.description,
                example: a.example,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = format!(
            "Created Deck {:?} ({}) with {} section(s). Open with show_deck(\"{}\").",
            detail.doc.title,
            detail.doc.doc_id,
            detail.doc.sections.len(),
            detail.doc.doc_id
        );
        Ok(structured(text, &detail))
    }
}

// ---------------------------------------------------------------------------
// deck_read
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckReadArgs {
    /// Deck ID (or `/decks/<id>` / `theuth://decks/<id>`).
    pub deck_id: String,
}

pub struct DeckReadTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckReadTool {
    const NAME: &'static str = "deck_read";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = DeckReadArgs;

    fn description(&self) -> String {
        "Read a Deck's collaborative sections (with stable IDs, aliases, and revs), runtime health, active/pending script metadata, and latest snapshot/live frame.".into()
    }

    async fn call(&self, a: DeckReadArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let deck_id = clean_deck_id(&a.deck_id);
        let detail = self
            .0
            .actor
            .ask(GetDeck {
                deck_id,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = format!(
            "Deck {:?} ({}): {} active section(s), state={:?}, stale={}, active_rev={:?}, pending_rev={:?}",
            detail.doc.title,
            detail.doc.doc_id,
            detail.doc.sections.len(),
            detail.runtime.state,
            detail.runtime.stale,
            detail.runtime.active_rev,
            detail.runtime.pending_rev
        );
        Ok(structured(text, &detail))
    }
}

// ---------------------------------------------------------------------------
// deck_section_add
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckSectionAddArgs {
    pub deck_id: String,
    /// Section kind: `json_render`, `note`, or `html`.
    pub kind: SectionKind,
    pub title: String,
    /// Section body (`json-render` JSON, markdown note, or strict-CSP HTML).
    pub content: String,
    /// Grid column span (1..=12).
    #[serde(default)]
    pub col_span: Option<u32>,
    /// Optional snapshot/frame key to bind into this section.
    #[serde(default)]
    pub data_key: Option<String>,
    /// Optional insertion position (0-based among active sections).
    #[serde(default)]
    pub index: Option<usize>,
}

pub struct DeckSectionAddTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckSectionAddTool {
    const NAME: &'static str = "deck_section_add";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckSectionAddArgs;

    fn description(&self) -> String {
        "Add one collaborative section (json_render, note, or strict-CSP html) to a Deck without rewriting other sections.".into()
    }

    async fn call(&self, a: DeckSectionAddArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let sec = self
            .0
            .actor
            .ask(SectionAddMsg {
                deck_id: clean_deck_id(&a.deck_id),
                kind: a.kind,
                title: a.title,
                content: a.content,
                col_span: a.col_span,
                data_key: a.data_key,
                index: a.index,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(
            format!("Added section {} ({}, rev {})", sec.id, sec.alias, sec.rev),
            &sec,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_section_patch
// ---------------------------------------------------------------------------

fn deserialize_optional_nullable<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckSectionPatchArgs {
    pub deck_id: String,
    /// Section ID or alias (e.g. `s1`).
    pub section_id: String,
    /// Optimistic CAS revision from `deck_read` to prevent overwriting concurrent user edits.
    #[serde(default)]
    pub expected_rev: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub col_span: Option<u32>,
    /// Omit to keep unchanged; pass `null` or `""` (or `clear_data_key: true`) to clear; pass a key name to set.
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    pub data_key: Option<Option<String>>,
    /// Explicitly clear `data_key` (cannot be combined with a non-empty `data_key`).
    #[serde(default)]
    pub clear_data_key: Option<bool>,
    #[serde(default)]
    pub summary: Option<String>,
}

pub struct DeckSectionPatchTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckSectionPatchTool {
    const NAME: &'static str = "deck_section_patch";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckSectionPatchArgs;

    fn description(&self) -> String {
        "Patch one Deck section locally (never whole-document rewrite). Pass expected_rev for optimistic concurrency.".into()
    }

    async fn call(&self, a: DeckSectionPatchArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let data_key = if a.clear_data_key == Some(true) {
            if a.data_key
                .as_ref()
                .and_then(|inner| inner.as_deref())
                .is_some_and(|s| !s.trim().is_empty())
            {
                anyhow::bail!("cannot set data_key and clear_data_key=true simultaneously");
            }
            Some(None)
        } else {
            a.data_key
        };
        let sec = self
            .0
            .actor
            .ask(SectionPatchMsg {
                deck_id: clean_deck_id(&a.deck_id),
                section_id: a.section_id,
                expected_rev: a.expected_rev,
                title: a.title,
                content: a.content,
                col_span: a.col_span,
                data_key,
                summary: a.summary,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(
            format!(
                "Patched section {} ({}) to rev {}",
                sec.id, sec.alias, sec.rev
            ),
            &sec,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_section_move
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckSectionMoveArgs {
    pub deck_id: String,
    pub section_id: String,
    pub to_index: usize,
}

pub struct DeckSectionMoveTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckSectionMoveTool {
    const NAME: &'static str = "deck_section_move";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "list";
    type Args = DeckSectionMoveArgs;

    fn description(&self) -> String {
        "Move a Deck section to a new display index using fractional CRDT ordering.".into()
    }

    async fn call(&self, a: DeckSectionMoveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let order = self
            .0
            .actor
            .ask(SectionMoveMsg {
                deck_id: clean_deck_id(&a.deck_id),
                section_id: a.section_id,
                to_index: a.to_index,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(
            format!("Section order updated ({} active)", order.len()),
            &order,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_section_archive
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckSectionArchiveArgs {
    pub deck_id: String,
    pub section_id: String,
    #[serde(default = "default_true")]
    pub archived: bool,
    #[serde(default)]
    pub expected_rev: Option<u64>,
}

fn default_true() -> bool {
    true
}

pub struct DeckSectionArchiveTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckSectionArchiveTool {
    const NAME: &'static str = "deck_section_archive";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckSectionArchiveArgs;

    fn description(&self) -> String {
        "Archive or unarchive a Deck section while preserving its revision history.".into()
    }

    async fn call(&self, a: DeckSectionArchiveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let sec = self
            .0
            .actor
            .ask(SectionArchiveMsg {
                deck_id: clean_deck_id(&a.deck_id),
                section_id: a.section_id,
                archived: a.archived,
                expected_rev: a.expected_rev,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(
            format!(
                "Section {} ({}) archived={} (rev {})",
                sec.id, sec.alias, sec.archived, sec.rev
            ),
            &sec,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_section_history
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckSectionHistoryArgs {
    pub deck_id: String,
    pub section_id: String,
}

pub struct DeckSectionHistoryTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckSectionHistoryTool {
    const NAME: &'static str = "deck_section_history";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list";
    type Args = DeckSectionHistoryArgs;

    fn description(&self) -> String {
        "Read per-section revision history and author attribution for a Deck section.".into()
    }

    async fn call(&self, a: DeckSectionHistoryArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let hist = self
            .0
            .actor
            .ask(SectionHistoryMsg {
                deck_id: clean_deck_id(&a.deck_id),
                section_id: a.section_id,
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(format!("{} revision(s)", hist.len()), &hist))
    }
}

// ---------------------------------------------------------------------------
// deck_focus
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckFocusArgs {
    pub deck_id: String,
    /// Section ID or alias to focus; null clears focus.
    #[serde(default)]
    pub section_id: Option<String>,
}

pub struct DeckFocusTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckFocusTool {
    const NAME: &'static str = "deck_focus";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = DeckFocusArgs;

    fn description(&self) -> String {
        "Highlight/focus a specific section inside a collaborative Deck for the viewer.".into()
    }

    async fn call(&self, a: DeckFocusArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let deck_id = clean_deck_id(&a.deck_id);
        self.0
            .actor
            .ask(FocusSectionMsg {
                deck_id: deck_id.clone(),
                section_id: a.section_id.clone(),
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(map_doc_send_err)?;
        Ok(structured(
            format!("Focused {:?} on deck {}", a.section_id, deck_id),
            &json!({ "deck_id": deck_id, "section_id": a.section_id }),
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_script_propose
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckScriptProposeArgs {
    pub deck_id: String,
    /// Portable Monty Python source defining async `on_start(last_state)`, `on_step()`/`collect()`, and optional `on_action(action, args, ctx)`.
    pub code: String,
    /// Declared interactive actions accepted by `on_action`, each with a JSON Schema for `args`.
    #[serde(default)]
    pub declared_actions: Vec<DeclaredAction>,
    /// Execution mode: `oneshot`, `streaming`, or `hybrid`.
    #[serde(default = "default_hybrid")]
    pub mode: ScriptMode,
    /// Optional cadence interval in seconds.
    #[serde(default)]
    pub interval_secs: Option<u64>,
    /// Optional 5-field cron schedule for scheduled collection.
    #[serde(default)]
    pub schedule_cron: Option<String>,
    /// Short summary of what changed in this revision.
    #[serde(default)]
    pub change_summary: Option<String>,
}

fn default_hybrid() -> ScriptMode {
    ScriptMode::Hybrid
}

pub struct DeckScriptProposeTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckScriptProposeTool {
    const NAME: &'static str = "deck_script_propose";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckScriptProposeArgs;

    fn description(&self) -> String {
        "Propose a new Monty script revision for a Deck. Always enters `pending_review` and cannot run recurring/streaming until the owner approves it in the Deck UI.".into()
    }

    async fn call(&self, a: DeckScriptProposeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let rev = self
            .0
            .actor
            .ask(ProposeScriptMsg {
                deck_id: clean_deck_id(&a.deck_id),
                code: a.code,
                declared_actions: a.declared_actions,
                mode: a.mode,
                interval_secs: a.interval_secs,
                schedule_cron: a.schedule_cron,
                proposed_by: format!("session:{}", cx.session.0),
                change_summary: a
                    .change_summary
                    .unwrap_or_else(|| "proposed by agent".into()),
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(
            format!(
                "Proposed script rev {} for deck {} (status: pending_review). Ask the owner to review and approve it in the Deck UI.",
                rev.rev, rev.deck_id
            ),
            &rev,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_script_read
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckScriptReadArgs {
    pub deck_id: String,
    /// Specific revision number; omit for pending/active/latest.
    #[serde(default)]
    pub rev: Option<u64>,
}

pub struct DeckScriptReadTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckScriptReadTool {
    const NAME: &'static str = "deck_script_read";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = DeckScriptReadArgs;

    fn description(&self) -> String {
        "Read a Deck's Monty script revision, declared actions, schedule, and review status.".into()
    }

    async fn call(&self, a: DeckScriptReadArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let script = self
            .0
            .actor
            .ask(ReadScriptMsg {
                deck_id: clean_deck_id(&a.deck_id),
                rev: a.rev,
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = match &script {
            Some(s) => format!(
                "Deck {} script rev {} (status={:?}, mode={:?}, {} declared action(s))",
                s.deck_id,
                s.rev,
                s.status,
                s.mode,
                s.declared_actions.len()
            ),
            None => "No script revision exists for this Deck yet.".into(),
        };
        Ok(structured(text, &script))
    }
}

// ---------------------------------------------------------------------------
// deck_script_run
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckScriptRunArgs {
    pub deck_id: String,
}

pub struct DeckScriptRunTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckScriptRunTool {
    const NAME: &'static str = "deck_script_run";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckScriptRunArgs;

    fn description(&self) -> String {
        "Run one bounded collection step immediately on a Deck's APPROVED script revision (refuses pending_review revisions).".into()
    }

    async fn call(&self, a: DeckScriptRunArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let status = self
            .0
            .actor
            .ask(RuntimeControlMsg {
                deck_id: clean_deck_id(&a.deck_id),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(
            format!(
                "Executed one step on deck {} (state={:?}, step_count={}, stale={})",
                status.deck_id, status.state, status.step_count, status.stale
            ),
            &status,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_runtime
// ---------------------------------------------------------------------------

/// Subset of runtime lifecycle operations permitted for agent tools (229 B3).
/// Agents may pause, stop, or request a single `run_once` step on an already
/// owner-approved revision, but NEVER `start` or `resume` recurring/streaming execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeOp {
    Pause,
    Stop,
    RunOnce,
}

impl From<AgentRuntimeOp> for RuntimeControlOp {
    fn from(op: AgentRuntimeOp) -> Self {
        match op {
            AgentRuntimeOp::Pause => RuntimeControlOp::Pause,
            AgentRuntimeOp::Stop => RuntimeControlOp::Stop,
            AgentRuntimeOp::RunOnce => RuntimeControlOp::RunOnce,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckRuntimeArgs {
    pub deck_id: String,
    /// Lifecycle operation: `pause`, `stop`, or `run_once`. Recurring `start`/`resume` require an explicit human owner action.
    pub op: AgentRuntimeOp,
}

pub struct DeckRuntimeTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckRuntimeTool {
    const NAME: &'static str = "deck_runtime";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckRuntimeArgs;

    fn description(&self) -> String {
        "Control a Deck's runtime lifecycle (`pause`, `stop`, `run_once`). Only owner-approved script revisions may run; recurring `start` and `resume` require explicit human owner control.".into()
    }

    async fn call(&self, a: DeckRuntimeArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let op: RuntimeControlOp = a.op.into();
        let status = self
            .0
            .actor
            .ask(RuntimeControlMsg {
                deck_id: clean_deck_id(&a.deck_id),
                op,
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(structured(
            format!(
                "Deck {} runtime op {:?} -> state={:?}, epoch={}, checkout_alive={}",
                status.deck_id, op, status.state, status.run_epoch, status.checkout_alive
            ),
            &status,
        ))
    }
}

// ---------------------------------------------------------------------------
// deck_interact
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckInteractArgs {
    pub deck_id: String,
    /// Declared action name.
    pub action: String,
    /// JSON object matching the action's declared JSON Schema.
    #[serde(default = "default_empty_obj")]
    pub args: Value,
    /// Optional idempotency key (auto-generated if omitted).
    #[serde(default)]
    pub request_id: Option<String>,
    /// Optional active script revision fence.
    #[serde(default)]
    pub expected_rev: Option<u64>,
}

fn default_empty_obj() -> Value {
    json!({})
}

pub struct DeckInteractTool(DecksTools);

#[async_trait::async_trait]
impl Tool for DeckInteractTool {
    const NAME: &'static str = "deck_interact";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DeckInteractArgs;

    fn description(&self) -> String {
        "Invoke a declared interactive action on a Deck's live Monty checkout. Validates args against the declared JSON Schema and returns the private result.".into()
    }

    async fn call(&self, a: DeckInteractArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let resp = self
            .0
            .actor
            .ask(InteractMsg {
                deck_id: clean_deck_id(&a.deck_id),
                request_id: a.request_id.unwrap_or_default(),
                caller_id: format!("agent:{}", cx.session.0),
                action: a.action,
                args: a.args,
                expected_rev: a.expected_rev,
                timeout_ms: None,
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = if resp.ok {
            format!(
                "Action {:?} succeeded (private={}, cached={})",
                resp.action, resp.private, resp.cached
            )
        } else {
            format!(
                "Action {:?} failed ({:?}): {}",
                resp.action,
                resp.error_code,
                resp.error.as_deref().unwrap_or("unknown error")
            )
        };
        Ok(structured(text, &resp))
    }
}

// ---------------------------------------------------------------------------
// show_deck
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShowDeckArgs {
    /// Deck ID to open in the desktop UI.
    pub deck_id: String,
    /// Optional pane placement hint (`beside`, `tab`, `replace`, `right`, `left`, `top`, `bottom`).
    #[serde(default)]
    pub placement: Option<PanePlacement>,
}

pub struct ShowDeckTool(DecksTools);

#[async_trait::async_trait]
impl Tool for ShowDeckTool {
    const NAME: &'static str = "show_deck";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    const RETURNS: &'static str = "dict";
    type Args = ShowDeckArgs;

    fn description(&self) -> String {
        "Open a Deck in the desktop workspace UI (`/decks/<id>`).".into()
    }

    async fn call(&self, a: ShowDeckArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let deck_id = clean_deck_id(&a.deck_id);
        let detail = self
            .0
            .actor
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: agent_principal(&cx),
            })
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let path = format!("/decks/{deck_id}");
        let outcome = layout::call(
            &self.0.rt,
            cx.session,
            LayoutOp::OpenView {
                path: path.clone(),
                title: Some(detail.doc.title.clone()),
                placement: a.placement.or(Some(PanePlacement::Beside)),
                target: None,
                focus: Some(true),
            },
        )
        .await;
        let status_msg = match outcome {
            Outcome::Done(_) => format!("Opened Deck {:?} at {path}", detail.doc.title),
            Outcome::NoClient => format!(
                "Deck {:?} is ready at {path} (no desktop client connected)",
                detail.doc.title
            ),
            Outcome::Failed(r) => format!(
                "Deck {:?} at {path} (UI could not open pane: {r})",
                detail.doc.title
            ),
            Outcome::TimedOut => {
                format!("Deck {:?} at {path} (UI open timed out)", detail.doc.title)
            }
        };
        Ok(structured(
            status_msg,
            &json!({
                "deck_id": deck_id,
                "title": detail.doc.title,
                "path": path,
            }),
        ))
    }
}
