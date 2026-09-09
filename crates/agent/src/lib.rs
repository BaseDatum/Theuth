//! Agent plugin: one supervisor actor (`"agent"`) routing to one actor per
//! session. Consumes tools through the `"tools"` registry; persists every
//! message to sqlite; publishes [`AgentEvent`] for any UI.
//!
//! Public surface:
//! - commands on the `"agent"` actor ([`Agents`]): [`SendInput`], [`Steer`],
//!   [`Cancel`], [`ListSessions`], [`LoadSession`], [`ListChildren`],
//!   [`ListModels`], [`SetModel`]
//! - event: [`AgentEvent`]

use std::sync::Arc;

use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};

pub mod loop_;
pub mod peers;
pub mod skills;
pub mod store;
pub mod supervisor;

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

/// `ask` → `Vec<SessionSummary>`, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSessions;

/// `ask` → `Vec<HistoryEntry>` for rendering a resumed session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LoadSession {
    /// Target session.
    pub session: SessionId,
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
}

/// Token usage for a turn.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Usage {
    /// Prompt tokens.
    pub prompt: u32,
    /// Completion tokens.
    pub completion: u32,
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
}

impl AgentEvent {
    /// Session the event belongs to.
    pub fn session(&self) -> SessionId {
        match self {
            Self::TurnStarted { session, .. }
            | Self::TextDelta { session, .. }
            | Self::ReasoningDelta { session, .. }
            | Self::ToolCallStarted { session, .. }
            | Self::ToolCallFinished { session, .. }
            | Self::TurnFinished { session, .. }
            | Self::MessageDelivered { session, .. }
            | Self::ModelChanged { session, .. }
            | Self::Error { session, .. } => *session,
            Self::SubagentSpawned { child, .. } => *child,
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
    /// Prepended to the built-in system prompt.
    pub system_prompt: String,
    /// Max tool-call rounds per turn before the loop stops itself.
    pub max_tool_rounds: u32,
    /// Max characters of tool output sent back to the model.
    pub max_tool_output_chars: usize,
    /// Subagent limits and model.
    pub subagents: SubagentsConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-sonnet-4-5".into(),
            system_prompt: String::new(),
            max_tool_rounds: 32,
            max_tool_output_chars: 32_000,
            subagents: SubagentsConfig::default(),
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
