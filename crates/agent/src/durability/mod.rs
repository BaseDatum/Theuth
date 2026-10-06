//! Design doc 109: durability types. The journal tables live in
//! `store/durability.rs`; the `tools::Journal` impl over the store in
//! [`journal`]; the job reaper in [`jobs`] (W2); the ACP handoff in
//! [`handoff`] (W4). This file holds the config, the plain row/plan types
//! every wave shares and the crash-injection hook (§11.1).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use genai::chat::MessageContent;
use runtime::TurnId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod handoff;
pub mod jobs;
pub mod journal;

/// The top-level `durability:` config section (§10.1). Carried on
/// `AgentConfig::durability` the way `surface` is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DurabilityConfig {
    /// What boot does with open turns (§4.6).
    pub resume: ResumeMode,
    /// `ResumeTurn` concurrency; the rest queue, most recent first.
    pub resume_parallel: usize,
    /// Graceful-shutdown budget for in-flight tool calls (§9.2).
    pub drain_secs: u64,
    /// `assistant_partial` cadence (§8.1); never per token.
    pub partial_checkpoint_ms: u64,
    /// PTY screen snapshot timer (§5.3).
    pub pty_snapshot_secs: u64,
    /// Finished-turn journal retention (§3.6).
    pub journal_keep_days: u32,
    /// ACP delta handoff budget (§7.3).
    pub handoff_tokens: usize,
}

impl Default for DurabilityConfig {
    fn default() -> Self {
        Self {
            resume: ResumeMode::Auto,
            resume_parallel: 4,
            drain_secs: 10,
            partial_checkpoint_ms: 1000,
            pty_snapshot_secs: 5,
            journal_keep_days: 7,
            handoff_tokens: 4000,
        }
    }
}

/// `durability.resume` (§4.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeMode {
    /// Every open turn is resumed at boot.
    #[default]
    Auto,
    /// An Attention item per open turn; resume on the user's button.
    Ask,
    /// Nothing at boot; the next input resumes lazily.
    Off,
}

/// `turn_steps.kind` (§3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    InputAccepted,
    ModelCall,
    AssistantPartial,
    ToolBatch,
    TurnFinished,
    Suspended,
}

impl StepKind {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputAccepted => "input_accepted",
            Self::ModelCall => "model_call",
            Self::AssistantPartial => "assistant_partial",
            Self::ToolBatch => "tool_batch",
            Self::TurnFinished => "turn_finished",
            Self::Suspended => "suspended",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "input_accepted" => Self::InputAccepted,
            "model_call" => Self::ModelCall,
            "assistant_partial" => Self::AssistantPartial,
            "tool_batch" => Self::ToolBatch,
            "turn_finished" => Self::TurnFinished,
            "suspended" => Self::Suspended,
            _ => return None,
        })
    }
}

/// `tool_calls.state` (§3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Planned,
    Started,
    Finished,
    Interrupted,
    Skipped,
}

impl CallState {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Started => "started",
            Self::Finished => "finished",
            Self::Interrupted => "interrupted",
            Self::Skipped => "skipped",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => Self::Planned,
            "started" => Self::Started,
            "finished" => Self::Finished,
            "interrupted" => Self::Interrupted,
            "skipped" => Self::Skipped,
            _ => return None,
        })
    }

    /// `finished` / `interrupted` / `skipped`: nothing left to run.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Interrupted | Self::Skipped)
    }
}

/// One `tool_calls` row of the open batch (§4.2's per-call rule reads it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournaledCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub risk: tools::Risk,
    pub state: CallState,
    pub attempt: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub output: Option<String>,
    pub is_error: bool,
    pub files: Vec<PathBuf>,
}

/// What the newest journal step of an open turn says (§4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastStep {
    /// The input is in the frame; the turn never reached the model.
    InputAccepted,
    /// A model call was dispatched and never settled.
    ModelCall { request_id: String, attempt: u32 },
    /// A tool batch was persisted; `all_finished` when every row is terminal.
    ToolBatch { all_finished: bool },
    /// `turn_finished`: not open.
    Finished,
}

/// Crash (no `suspended` row) or drain (§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    Interrupted,
    Suspended,
}

/// The `assistant_partial` of an open model call (§8.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialContent {
    pub call_id: String,
    pub content: MessageContent,
    pub thinking_ms: Option<u64>,
    pub at: DateTime<Utc>,
}

/// What `ResumeTurn` acts on, computed from the journal by
/// `Store::resume_plan` (§4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct ResumePlan {
    pub turn: TurnId,
    pub kind: ResumeKind,
    /// Newest journal row's `at`.
    pub interrupted_at: DateTime<Utc>,
    pub last: LastStep,
    /// The open batch's `tool_calls` rows, in `tool_batch.payload.call_ids`
    /// order; empty unless `last` is a `ToolBatch`.
    pub calls: Vec<JournaledCall>,
    pub partial: Option<PartialContent>,
}

/// Crash-injection step names (§11.1), shared by the loop (W1) and the
/// crash suite (W6) so both spell them identically. Per-call points are
/// built with [`call_point`].
pub mod crash {
    pub const INPUT_BEFORE_PERSIST: &str = "input.before_persist";
    pub const INPUT_AFTER_PERSIST: &str = "input.after_persist";
    pub const MODEL_AFTER_RESERVE: &str = "model.after_reserve";
    pub const MODEL_AFTER_DISPATCH: &str = "model.after_dispatch";
    /// After the first text delta.
    pub const STREAM_MID: &str = "stream.mid";
    pub const STREAM_AFTER_PARTIAL: &str = "stream.after_partial";
    pub const STREAM_BEFORE_END_PERSIST: &str = "stream.before_end_persist";
    pub const BATCH_AFTER_TOOL_USE_PERSIST: &str = "batch.after_tool_use_persist";
    pub const BATCH_BEFORE_MESSAGE: &str = "batch.before_message";
    pub const BATCH_AFTER_MESSAGE: &str = "batch.after_message";
    pub const TURN_BEFORE_FINISHED: &str = "turn.before_finished";
    pub const TURN_AFTER_FINISHED: &str = "turn.after_finished";
    /// Suffixes of `call.<i>.<suffix>`.
    pub const CALL_BEFORE_STARTED: &str = "before_started";
    pub const CALL_AFTER_STARTED: &str = "after_started";
    /// Inside a fake tool.
    pub const CALL_MID: &str = "mid";
    pub const CALL_BEFORE_FINISHED: &str = "before_finished";
    pub const CALL_AFTER_FINISHED: &str = "after_finished";

    /// `call.<i>.<suffix>` for the i-th call of a batch.
    pub fn call_point(i: usize, suffix: &str) -> String {
        format!("call.{i}.{suffix}")
    }
}

/// Fault hook (§11.1): aborts the process when `THEUTH_CRASH_AT` equals
/// `step`; a no-op otherwise. Placed on both sides of every journal write.
pub fn crash_point(step: &str) {
    if std::env::var_os("THEUTH_CRASH_AT").is_some_and(|v| v == step) {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_partial_yaml() {
        let d = DurabilityConfig::default();
        assert_eq!(d.resume, ResumeMode::Auto);
        assert_eq!(
            (
                d.resume_parallel,
                d.drain_secs,
                d.partial_checkpoint_ms,
                d.pty_snapshot_secs,
                d.journal_keep_days,
                d.handoff_tokens
            ),
            (4, 10, 1000, 5, 7, 4000)
        );
        // A partial section keeps the other defaults.
        let c: DurabilityConfig = serde_yaml::from_str("resume: ask\ndrain_secs: 3\n").unwrap();
        assert_eq!(c.resume, ResumeMode::Ask);
        assert_eq!(c.drain_secs, 3);
        assert_eq!(c.resume_parallel, 4);
        // Empty `agent:` still parses with the durability default.
        let a: crate::AgentConfig = serde_yaml::from_str("model: x\n").unwrap();
        assert_eq!(a.durability, DurabilityConfig::default());
    }

    #[test]
    fn kinds_round_trip() {
        for k in [
            StepKind::InputAccepted,
            StepKind::ModelCall,
            StepKind::AssistantPartial,
            StepKind::ToolBatch,
            StepKind::TurnFinished,
            StepKind::Suspended,
        ] {
            assert_eq!(StepKind::parse(k.as_str()), Some(k));
        }
        for s in [
            CallState::Planned,
            CallState::Started,
            CallState::Finished,
            CallState::Interrupted,
            CallState::Skipped,
        ] {
            assert_eq!(CallState::parse(s.as_str()), Some(s));
            assert_eq!(
                s.is_terminal(),
                !matches!(s, CallState::Planned | CallState::Started)
            );
        }
        assert_eq!(crash::call_point(2, crash::CALL_MID), "call.2.mid");
    }

    #[test]
    fn crash_point_is_a_noop_unless_named() {
        // The env var is unset in the test process; a named step is still a no-op.
        crash_point(crash::INPUT_BEFORE_PERSIST);
    }
}
