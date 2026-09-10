//! Agent plugin: one supervisor actor (`"agent"`) routing to one actor per
//! session. Consumes tools through the `"tools"` registry; persists every
//! message to sqlite; publishes [`AgentEvent`] for any UI.
//!
//! - commands on the `"agent"` actor ([`Agents`]): [`SendInput`], [`Steer`],
//!   [`Cancel`], [`ListSessions`], [`LoadSession`], [`ListChildren`],
//!   [`ListModels`], [`SetModel`], [`ShowPrompt`], [`ListSkills`],
//!   [`LoadSkill`], [`AddSkill`], [`RemoveSkill`], [`AddSkillRepo`],
//!   [`RemoveSkillRepo`], [`SyncSkills`], [`ListLogins`], [`Login`], [`Logout`]
//! - events: [`AgentEvent`], [`LoginFinished`]

use std::sync::Arc;

use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};

pub mod compact;
pub mod frame;
pub mod knowledge;
pub mod loop_;
pub mod peers;
pub mod repl;
pub mod skills;
pub mod spill;
pub mod store;
pub mod supervisor;
pub mod todo;

pub use supervisor::Agents;

/// Registry name of the [`Agents`] supervisor actor.
pub const AGENT_ACTOR: &str = "agent";

/// `ask` → [`SessionId`]. `session: None` creates a new session. Returns as
/// soon as the turn is queued; progress arrives as [`AgentEvent`]s.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendInput {
    /// Existing session, or `None` for a new one.
    pub session: Option<SessionId>,
    /// User text.
    pub text: String,
}

/// `ask` → `()`. Injected as a user message after the current tool batch
/// completes (oh-my-pi steering); no-op if the session is idle (then it is
/// treated as [`SendInput`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Steer {
    /// Target session.
    pub session: SessionId,
    /// User text.
    pub text: String,
}

/// `ask` → `()`. Abort the in-flight turn; emits `TurnFinished { stop_reason: "cancelled" }`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Cancel {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<u32 /* epoch */, String>`: compact `session` now,
/// threshold ignored (design doc 07 §8.6). Refused while a turn is running.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Compact {
    /// Session to compact.
    pub session: SessionId,
}

/// `ask` → `Result<Vec<knowledge::Hit>, String>`: search the knowledge bundle (design doc 10 §8).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeSearch {
    /// Free text.
    pub query: String,
    /// Default 10, max 50.
    pub limit: Option<u32>,
}

/// `ask` → `Result<knowledge::Concept, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeGet {
    /// Concept id.
    pub id: String,
}

/// `ask` → `Result<Vec<knowledge::Meta>, String>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KnowledgeList {
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

/// `ask` → `Result<knowledge::Meta, String>`: create or update a concept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgePut {
    /// The document.
    pub put: knowledge::Put,
    /// Who is writing (`User` from RPC/TUI).
    pub source: knowledge::Source,
}

/// `ask` → `Result<(), String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeDelete {
    /// Concept id.
    pub id: String,
}

/// `ask` → `Result<knowledge::History, String>` (design doc 10 §2.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KnowledgeHistory {
    /// Concept id; `None` for the whole bundle.
    pub id: Option<String>,
    /// Default 20.
    pub limit: Option<u32>,
}

/// `ask` → `Result<knowledge::Change, String>`: rewrite a concept from a commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeRestore {
    /// Concept id.
    pub id: String,
    /// Abbreviated commit id; default: the commit before the latest change.
    pub commit: Option<String>,
}

/// `ask` → `Result<knowledge::HarvestResult, String>`: harvest a session's
/// finished frame now (design doc 10 §5.2). `Err("turn in flight")` while it runs.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Harvest {
    /// Session.
    pub session: SessionId,
}

/// `ask` → `Vec<SessionSummary>`, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSessions;

/// `ask` → [`Loaded`] for rendering a resumed session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LoadSession {
    /// Target session.
    pub session: SessionId,
}

/// Reply to [`LoadSession`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Loaded {
    /// Rendered messages so far (an in-flight round is not yet stored).
    pub history: Vec<HistoryEntry>,
    /// A turn is in flight; its events keep streaming.
    pub running: bool,
}

/// `ask` → `Vec<SessionSummary>`: subagents of `parent` (with `name` set), newest first.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ListChildren {
    /// Owning session.
    pub parent: SessionId,
}

/// `ask` → `Vec<ModelInfo>`: every catalog model whose provider is configured,
/// sorted by provider then id.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListModels;

/// `ask` → `Result<ModelInfo, String>`. Resolves `model` (design doc 02 §1);
/// switches `session` (if given) for its next model call, makes the id the
/// default for new sessions and writes it back to `<data_dir>/config.yaml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetModel {
    /// Session to switch; `None` changes only the default.
    pub session: Option<SessionId>,
    /// `provider/model`, or a bare model id resolved against configured providers.
    pub model: String,
}

/// One selectable model.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ModelInfo {
    /// `provider/model`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// models.dev provider id.
    pub provider: String,
    /// Context window in tokens; 0 when unknown.
    pub context: u32,
    /// Supports reasoning.
    pub reasoning: bool,
}

/// `ask` → `String`: the assembled system prompt (design doc 04 §1.1) as
/// `session` sees it; without a session the peers section is omitted.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ShowPrompt {
    /// Session whose peers block to include.
    pub session: Option<SessionId>,
}

/// `ask` → `Vec<SkillMeta>`, sorted by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSkills;

/// `ask` → `Result<SkillBody, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoadSkill {
    /// Skill name.
    pub name: String,
}

/// `ask` → `Result<SkillMeta, String>`: write `<skills>/<name>.md`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddSkill {
    /// `[a-z0-9][a-z0-9._-]*`.
    pub name: String,
    /// Index line.
    pub description: String,
    /// Markdown body.
    pub body: String,
    /// Inline into every system prompt instead of indexing.
    pub always: bool,
}

/// `ask` → `Result<(), String>`: delete a local skill.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveSkill {
    /// Skill name.
    pub name: String,
}

/// `ask` → `Result<Vec<SkillMeta>, String>`: clone a skill repo; returns its skills.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddSkillRepo {
    /// Git URL.
    pub url: String,
    /// Directory name; defaults to the repo basename.
    pub name: Option<String>,
}

/// `ask` → `Result<(), String>`: delete a cloned skill repo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveSkillRepo {
    /// Directory name.
    pub name: String,
}

/// `ask` → `Vec<SyncResult>`: `git pull` every skill repo.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SyncSkills;

/// `ask` → `Vec<LoginInfo>`: every provider a user can log in to (design doc 05 §2).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListLogins;

/// One OAuth provider row.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LoginInfo {
    /// genai namespace (`openai`).
    pub name: String,
    /// Tokens on disk.
    pub logged_in: bool,
    /// Account the tokens belong to, when the provider tells us.
    pub account: Option<String>,
    /// Access token expiry, unix seconds.
    pub expires_at: Option<u64>,
}

/// `ask` → `Result<String, String>`: the URL to open; the browser is also
/// opened. Completion arrives as [`LoginFinished`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Login {
    /// genai namespace (`openai`).
    pub provider: String,
}

/// `ask` → `Result<(), String>`: delete the provider's tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Logout {
    /// genai namespace (`openai`).
    pub provider: String,
}

/// Event: the login started by [`Login`] ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct LoginFinished {
    /// genai namespace (`openai`).
    pub provider: String,
    /// `None` on success.
    pub error: Option<String>,
}

/// One indexed skill.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkillMeta {
    /// Name (front matter, else file stem / directory name).
    pub name: String,
    /// Index line.
    pub description: String,
    /// Absolute path of the skill file.
    pub path: String,
    /// Inlined into the system prompt instead of indexed.
    pub always: bool,
    /// Clone directory name when the skill lives in a git repo under skills/.
    pub repo: Option<String>,
}

/// A loaded skill.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkillBody {
    /// Name.
    pub name: String,
    /// Markdown without front matter.
    pub body: String,
    /// Other files of a `SKILL.md` skill, relative to its directory; empty otherwise.
    pub files: Vec<String>,
}

/// One row of the session list.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionSummary {
    /// Id.
    pub id: SessionId,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// First user message, truncated.
    pub title: String,
    /// Model used.
    pub model: String,
    /// Parent session when this is a subagent.
    pub parent: Option<SessionId>,
    /// Peer name; `None` for top-level sessions.
    pub name: Option<String>,
}

/// Rendered history for UIs (tool traffic collapsed to summaries).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// User text.
    User {
        /// Text.
        text: String,
    },
    /// Assistant text.
    Assistant {
        /// Text.
        text: String,
    },
    /// A tool call and its result.
    Tool {
        /// Tool name.
        name: String,
        /// Raw arguments.
        args: serde_json::Value,
        /// Output, possibly truncated.
        output: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// A compaction boundary: everything before it was folded into `summary`
    /// (design doc 07 §8.4).
    Compacted {
        /// Epoch number the boundary opened.
        epoch: u32,
        /// The summary text the model works from.
        summary: String,
    },
}

/// Token usage for a turn.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Usage {
    /// Prompt tokens (cached + written + fresh).
    pub prompt: u32,
    /// Completion tokens.
    pub completion: u32,
    /// Prompt tokens read from the provider's prefix cache.
    pub cached: u32,
}

/// Everything a UI needs to render a session live. Every variant carries the
/// session and turn; UIs filter by session.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum AgentEvent {
    /// A user message was accepted and a model request is starting.
    TurnStarted {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// The user text (echo for UIs that did not send it).
        text: String,
        /// Canonical model id the turn runs on.
        model: String,
    },
    /// Streamed assistant text.
    TextDelta {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Fragment.
        text: Arc<str>,
    },
    /// Streamed reasoning text (providers that expose it).
    ReasoningDelta {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Fragment.
        text: Arc<str>,
    },
    /// The model has begun emitting a tool call (name known, arguments still
    /// streaming). Lets a UI show activity before `ToolCallStarted`.
    ToolCallPending {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id (matches `ToolCallStarted`).
        call_id: String,
        /// Tool name.
        name: String,
    },
    /// The model asked for a tool; the call is now routed to the registry.
    ToolCallStarted {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Raw arguments; UIs format these per tool.
        args: serde_json::Value,
    },
    /// The tool returned.
    ToolCallFinished {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id.
        call_id: String,
        /// Output text, possibly truncated for display.
        output: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// The turn ended (model finished, was cancelled, or errored).
    TurnFinished {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider stop reason, or `"cancelled"` / `"error"`.
        stop_reason: String,
        /// Token usage summed over the turn's model calls.
        usage: Usage,
        /// Prompt + completion tokens of the last model call: what the window holds now.
        context: u32,
    },
    /// Non-fatal or fatal error text for the UI.
    Error {
        /// Session.
        session: SessionId,
        /// Turn, if one was in flight.
        turn: Option<TurnId>,
        /// Message.
        message: String,
    },
    /// A subagent session was created for `parent`.
    SubagentSpawned {
        /// Owning session.
        parent: SessionId,
        /// New session.
        child: SessionId,
        /// Peer name, unique among the parent's children.
        name: String,
    },
    /// Text injected into `session` by the harness (subagent result or peer
    /// message), for transcripts.
    MessageDelivered {
        /// Receiving session.
        session: SessionId,
        /// Sender: a child's name or `"parent"`.
        from: String,
        /// Delivered text without its wrapper.
        text: String,
    },
    /// The session's model was switched (takes effect on its next model call).
    ModelChanged {
        /// Session.
        session: SessionId,
        /// Canonical model id.
        model: String,
    },
    /// Compaction started: the summariser call is in flight (design doc 07 §8).
    Compacting {
        /// Session.
        session: SessionId,
        /// Turn, when triggered mid-turn; `None` for `/compact`.
        turn: Option<TurnId>,
    },
    /// Compaction finished: a new epoch opened.
    Compacted {
        /// Session.
        session: SessionId,
        /// Turn, when triggered mid-turn; `None` for `/compact`.
        turn: Option<TurnId>,
        /// New epoch number.
        epoch: u32,
        /// Projected tokens before the cut.
        before: u32,
        /// Messages folded into the summary.
        folded: u32,
    },
    /// A concept was written or removed (design doc 10 §8). Global: not tied to a session.
    KnowledgeChanged {
        /// Concept id.
        id: String,
        /// Put or delete.
        op: knowledge::KnowledgeOp,
        /// Who wrote it.
        source: knowledge::Source,
    },
    /// The harvest continuation is in flight (design doc 10 §5.2); precedes `TurnFinished`.
    Harvesting {
        /// Session.
        session: SessionId,
    },
    /// A harvest finished for a session (design doc 10 §5.4).
    Harvested {
        /// Session.
        session: SessionId,
        /// Concepts created.
        added: u32,
        /// Concepts updated.
        updated: u32,
    },
}

impl AgentEvent {
    /// Session the event belongs to; `None` for global events.
    pub fn session(&self) -> Option<SessionId> {
        match self {
            Self::TurnStarted { session, .. }
            | Self::TextDelta { session, .. }
            | Self::ReasoningDelta { session, .. }
            | Self::ToolCallPending { session, .. }
            | Self::ToolCallStarted { session, .. }
            | Self::ToolCallFinished { session, .. }
            | Self::TurnFinished { session, .. }
            | Self::MessageDelivered { session, .. }
            | Self::ModelChanged { session, .. }
            | Self::Compacting { session, .. }
            | Self::Compacted { session, .. }
            | Self::Harvesting { session, .. }
            | Self::Harvested { session, .. }
            | Self::Error { session, .. } => Some(*session),
            Self::SubagentSpawned { child, .. } => Some(*child),
            Self::KnowledgeChanged { .. } => None,
        }
    }
}

/// `[agent]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Model id, `provider/model` (`anthropic/claude-sonnet-4-5`) or bare
    /// (`claude-sonnet-4-5`, resolved against configured providers).
    pub model: String,
    /// Max tool-call rounds per turn before the loop stops itself.
    pub max_tool_rounds: u32,
    /// Max characters of tool output sent back to the model.
    pub max_tool_output_chars: usize,
    /// Subagent limits and model.
    pub subagents: SubagentsConfig,
    /// Auto-compaction threshold as a fraction of the model's context window;
    /// `0` disables it (design doc 07 §8.1).
    pub compact_at: f32,
    /// Persistent Python runtime (design doc 08).
    pub repl: ReplConfig,
    /// Knowledge bundle (design doc 10).
    pub knowledge: knowledge::KnowledgeConfig,
}

/// `[agent.repl]` config section (design doc 08 §10).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplConfig {
    /// Path to the `monty` worker binary; default: `monty` on `PATH`, then
    /// the active toolchain's bin dir. Not found → the runtime is off.
    pub monty: Option<std::path::PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-sonnet-4-5".into(),
            max_tool_rounds: 32,
            max_tool_output_chars: 32_000,
            subagents: SubagentsConfig::default(),
            compact_at: 0.8,
            repl: ReplConfig::default(),
            knowledge: knowledge::KnowledgeConfig::default(),
        }
    }
}

/// `[agent.subagents]` config section (design doc 01 §2.6).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentsConfig {
    /// Child model; defaults to `agent.model`.
    pub model: Option<String>,
    /// Sessions at depth ≥ this cannot spawn.
    pub max_depth: u8,
    /// Concurrent child turns; further spawns queue.
    pub max_concurrent: usize,
    /// Wall clock per child turn; on expiry the turn is cancelled.
    pub max_runtime_secs: u64,
    /// How long `send_message{await_reply}` waits.
    pub reply_timeout_secs: u64,
}

impl Default for SubagentsConfig {
    fn default() -> Self {
        Self {
            model: None,
            max_depth: 1,
            max_concurrent: 8,
            max_runtime_secs: 600,
            reply_timeout_secs: 120,
        }
    }
}

/// The plugin.
pub struct AgentPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for AgentPlugin {
    fn name(&self) -> &'static str {
        "agent"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        supervisor::start(rt).await
    }
}
