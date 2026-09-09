//! Agent plugin: one supervisor actor (`"agent"`) routing to one actor per
//! session. Consumes tools through the `"tools"` registry; persists every
//! message to sqlite; publishes [`AgentEvent`] for any UI.
//!
//! Public surface:
//! - commands on the `"agent"` actor ([`Agents`]): [`SendInput`], [`Steer`],
//!   [`Cancel`], [`ListSessions`], [`LoadSession`]
//! - event: [`AgentEvent`]

use std::sync::Arc;

use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};

pub mod loop_;
pub mod skills;
pub mod store;
pub mod subagent;
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
        /// Human summary of the call.
        summary: String,
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
        /// Human summary of the arguments.
        summary: String,
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
            | Self::Error { session, .. } => *session,
        }
    }
}

/// `[agent]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// genai model name (`claude-sonnet-4-5`, `gpt-5-mini`, `ollama::llama3`, …).
    pub model: String,
    /// Prepended to the built-in system prompt.
    pub system_prompt: String,
    /// Max tool-call rounds per turn before the loop stops itself.
    pub max_tool_rounds: u32,
    /// Max characters of tool output sent back to the model.
    pub max_tool_output_chars: usize,
    /// Model for subagents; defaults to `model`.
    pub subagent_model: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-5".into(),
            system_prompt: String::new(),
            max_tool_rounds: 32,
            max_tool_output_chars: 32_000,
            subagent_model: None,
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
