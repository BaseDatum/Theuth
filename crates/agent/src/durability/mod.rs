//! Design doc 109: durability types. The journal tables live in
//! `store/durability.rs`; the `tools::Journal` impl over the store in
//! [`journal`]; the job reaper in [`jobs`] (W2); the ACP handoff in
//! [`handoff`] (W4). This file holds the config, the plain row/plan types
//! every wave shares and the crash-injection hook (§11.1).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use genai::chat::MessageContent;
use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResumeReport;

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
    /// 146 D1: how far back the boot pass looks for mid-prompt ACP children
    /// (`sessions.updated_at`), in days.
    pub acp_respawn_window_days: u32,
    /// 146 D2: ACP children respawning at once; the rest queue until one
    /// comes back (its harness accepted the prompt) or fails.
    pub acp_respawn_parallel: usize,
}

impl Default for DurabilityConfig {
    fn default() -> Self {
        Self {
            resume: ResumeMode::Auto,
            resume_parallel: 4,
            drain_secs: 10,
            partial_checkpoint_ms: 5000,
            pty_snapshot_secs: 5,
            journal_keep_days: 7,
            handoff_tokens: 4000,
            acp_respawn_window_days: 7,
            acp_respawn_parallel: 4,
        }
    }
}

/// 178 W1/W2: the floor of a resumed turn's remaining run time — an
/// over-cap turn gets this long to land before it is cancelled.
pub const RESUME_GRACE: std::time::Duration = std::time::Duration::from_secs(60);

/// 178 W1/W2: what is left of a run-time `cap` after `ran` (the turn's run
/// time before a restart, [`run_clock`] / `store::durability::run_ms`:
/// downtime never counts), floored at [`RESUME_GRACE`] but never above `cap`.
pub fn remaining_run(cap: std::time::Duration, ran: chrono::Duration) -> std::time::Duration {
    let ran = ran.to_std().unwrap_or_default();
    cap.saturating_sub(ran).max(RESUME_GRACE.min(cap))
}

/// 178 F1: the run clock of every turn running in this process, read by the
/// drain, which writes the turn's accrued run time as `ran_ms` on its
/// `suspended` row. Cumulative across resumes: a turn starts from the
/// `ran_ms` its journal already holds. `std::time::Instant` is
/// `clock_gettime(CLOCK_UPTIME_RAW)` on Darwin and `CLOCK_MONOTONIC` on
/// Linux (std docs) — neither advances while the machine sleeps, so a laptop
/// asleep mid-turn adds nothing (Windows' `QueryPerformanceCounter` may).
pub mod run_clock {
    use std::collections::HashMap;
    use std::sync::LazyLock;
    use std::time::Instant;

    use runtime::{SessionId, TurnId};

    type Clocks = HashMap<TurnId, (SessionId, Instant, u64)>;
    static CLOCKS: LazyLock<parking_lot::Mutex<Clocks>> = LazyLock::new(Default::default);

    /// `turn` of `session` runs from now, `prior_ms` already behind it. A
    /// clock already running for `turn` is kept.
    pub fn start(session: SessionId, turn: TurnId, prior_ms: u64) {
        CLOCKS
            .lock()
            .entry(turn)
            .or_insert((session, Instant::now(), prior_ms));
    }

    /// `turn` stopped running here.
    pub fn stop(turn: TurnId) {
        CLOCKS.lock().remove(&turn);
    }

    /// Every clock of `session` stops (an ACP child's prompt returned).
    pub fn stop_session(session: SessionId) {
        CLOCKS.lock().retain(|_, (s, _, _)| *s != session);
    }

    /// `turn`'s run time so far, prior runs included; `None` when it does
    /// not run here.
    pub fn ran_ms(turn: TurnId) -> Option<u64> {
        CLOCKS
            .lock()
            .get(&turn)
            .map(|(_, at, prior)| prior + at.elapsed().as_millis() as u64)
    }
}

/// 146 D4: the restart-recovery counts. Published on the bus by the boot
/// pass (`respawn_acp_children`) and again whenever a respawn settles; the
/// Attention actor renders one `resume:boot:<at_ms>` item from the latest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BootPass {
    /// When the pass ran (ms since epoch): the item's id and `at_ms`.
    pub at_ms: u64,
    /// Open turns `resume_scan` handed back to their actors.
    pub resuming: u32,
    /// ACP children whose harness accepted the respawn prompt.
    pub respawned: u32,
    /// Of those, the ones that came back on a fresh harness session because
    /// `session/load` failed (D11: "restarted fresh").
    pub fresh: u32,
    /// ACP children queued or connecting; `0` = the pass is settled.
    pub waiting: u32,
    /// `(session, name)` of every child that was not respawned (D3).
    pub lost: Vec<(SessionId, String)>,
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
    /// Beside the user row `take_input` persisted.
    InputAccepted,
    /// Beside `reserve_call`; `call_id` set.
    ModelCall,
    /// Upserted in place while streaming (§8.1).
    AssistantPartial,
    /// With the assistant `tool_use` row and its `tool_calls` in `planned`.
    ToolBatch,
    /// With the final assistant row; closes the turn.
    TurnFinished,
    /// By the drain (§9); open, only changes the resume wording.
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
    /// Row written with the batch; the body has not run.
    Planned,
    /// The body is running; its effect is unknown to a restart.
    Started,
    /// Returned; `output` is the result.
    Finished,
    /// A restart found it `started` and mutating; not re-run (§4.2).
    Interrupted,
    /// Never run: cancelled or landed (`NOT_RUN`).
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
    /// Provider call id.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as sent.
    pub args: Value,
    /// The registry's risk for the tool (`read_only` for loop-local tools).
    pub risk: tools::Risk,
    /// Lifecycle state.
    pub state: CallState,
    /// 1 on the first run; a replay bumps it.
    pub attempt: u32,
    /// When the body began.
    pub started_at: Option<DateTime<Utc>>,
    /// Spill-capped content; `None` until terminal.
    pub output: Option<String>,
    /// The output is an error.
    pub is_error: bool,
    /// Files the tool produced.
    pub files: Vec<PathBuf>,
}

/// What the newest journal step of an open turn says (§4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastStep {
    /// The input is in the frame; the turn never reached the model.
    InputAccepted,
    /// A model call was dispatched and never settled.
    ModelCall {
        /// The accounting request id; the re-issued call reuses it.
        request_id: String,
        /// The lost attempt; the re-issue is `attempt + 1`.
        attempt: u32,
    },
    /// A tool batch was persisted.
    ToolBatch {
        /// Every row is terminal: only the batch message is missing.
        all_finished: bool,
    },
    /// `turn_finished`: not open.
    Finished,
}

/// Crash (no `suspended` row) or drain (§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    /// No `suspended` row: a crash.
    Interrupted,
    /// A `suspended` row: a drain (§9).
    Suspended,
}

impl ResumeKind {
    /// The `resume_reports.kind` spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::Suspended => "suspended",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "interrupted" => Some(Self::Interrupted),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

/// Why a turn is being resumed (139 §3, §6.3): what the *engine* knows,
/// beside [`ResumeKind`], which is what the *journal* says. Drives wording
/// and the D5 policy; stored in `resume_reports.cause` (0038).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ResumeCause {
    /// No `suspended` row: the engine died mid-turn.
    Crash,
    /// `suspended{shutdown | parent_gone | drain}` (Revision 1 Q2 folds them).
    Shutdown,
    /// `suspended{update}`.
    Update,
    /// Taken from a reachable owner that drained (`suspended{release}`).
    Transfer,
    /// Taken from an unreachable owner, which may still be executing.
    Takeover,
}

impl ResumeCause {
    /// The `resume_reports.cause` / `<engine_restart kind>` spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::Shutdown => "shutdown",
            Self::Update => "update",
            Self::Transfer => "transfer",
            Self::Takeover => "takeover",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "crash" => Self::Crash,
            "shutdown" => Self::Shutdown,
            "update" => Self::Update,
            "transfer" => Self::Transfer,
            "takeover" => Self::Takeover,
            _ => return None,
        })
    }

    /// §6.3's boot rule for a turn found in the journal (`resume_scan`, the
    /// `resume_turn` RPC): the newest `suspended{reason}` → `shutdown` for
    /// `shutdown` / `parent_gone` / `drain`, `update` for `update`,
    /// `transfer` for `release`; no `suspended` row → `crash`.
    pub fn from_journal(kind: ResumeKind, suspended_reason: Option<&str>) -> Self {
        match (kind, suspended_reason) {
            (ResumeKind::Interrupted, _) => Self::Crash,
            (ResumeKind::Suspended, Some("update")) => Self::Update,
            (ResumeKind::Suspended, Some("release")) => Self::Transfer,
            (ResumeKind::Suspended, _) => Self::Shutdown,
        }
    }
}

/// The `assistant_partial` of an open model call (§8.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialContent {
    /// The model call.
    pub call_id: String,
    /// What streamed so far, as `persist` would write it.
    pub content: MessageContent,
    /// Thinking time so far.
    pub thinking_ms: Option<u64>,
    /// When the row was last upserted.
    pub at: DateTime<Utc>,
}

/// What `ResumeTurn` acts on, computed from the journal by
/// `Store::resume_plan` (§4.1).
#[derive(Clone, Debug)]
pub struct ResumePlan {
    /// The open turn.
    pub turn: TurnId,
    /// Crash or drain: what the journal says.
    pub kind: ResumeKind,
    /// §6.3's boot rule from the journal (`ResumeCause::from_journal`);
    /// the `Resume` message overrides it for `transfer` / `takeover`.
    pub cause: ResumeCause,
    /// Newest journal row's `at`.
    pub interrupted_at: DateTime<Utc>,
    /// 178 F1: the turn's run time before the interruption
    /// (`store::durability::run_ms`): the drain's cumulative `ran_ms`, plus
    /// what ran after it; downtime never counts.
    pub ran_ms: u64,
    /// 178 W2: the turn's opener was bounded (a peer input, or a scoped
    /// input at depth 0: journaled `source` `peer` / `scoped`).
    pub bounded: bool,
    /// What to act on (§4.2).
    pub last: LastStep,
    /// The open batch's `tool_calls` rows, in `tool_batch.payload.call_ids`
    /// order; empty unless `last` is a `ToolBatch`.
    pub calls: Vec<JournaledCall>,
    /// The open model call's partial, for the UI (§8.1).
    pub partial: Option<PartialContent>,
}

/// One `resume_reports` row (§8.2, migration 0035): the [`ResumeReport`] a
/// resumed turn settled with, plus the plan it ran under, so the banner's
/// counts survive a reload. One row per turn; a second resume overwrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeReportRow {
    /// Session.
    pub session: SessionId,
    /// The resumed turn.
    pub turn: TurnId,
    /// The plan's crash-or-drain.
    pub kind: ResumeKind,
    /// Why the engine resumed it (139 §6.3); the boot rule for a `''` row.
    pub cause: ResumeCause,
    /// The plan's newest journal row time.
    pub interrupted_at: DateTime<Utc>,
    /// When the resumed step settled.
    pub at: DateTime<Utc>,
    /// The counts.
    pub report: ResumeReport,
}

/// Crash-injection step names (§11.1), shared by the loop (W1) and the
/// crash suite (W6) so both spell them identically. Per-call points are
/// built with [`call_point`].
pub mod crash {
    /// Before `take_input` persists.
    pub const INPUT_BEFORE_PERSIST: &str = "input.before_persist";
    /// After `take_input` committed.
    pub const INPUT_AFTER_PERSIST: &str = "input.after_persist";
    /// After `reserve_call`.
    pub const MODEL_AFTER_RESERVE: &str = "model.after_reserve";
    /// After the model call was dispatched.
    pub const MODEL_AFTER_DISPATCH: &str = "model.after_dispatch";
    /// After the first text delta.
    pub const STREAM_MID: &str = "stream.mid";
    /// After a partial upsert.
    pub const STREAM_AFTER_PARTIAL: &str = "stream.after_partial";
    /// Before the final assistant row persists.
    pub const STREAM_BEFORE_END_PERSIST: &str = "stream.before_end_persist";
    /// After the `tool_use` row and planned rows committed.
    pub const BATCH_AFTER_TOOL_USE_PERSIST: &str = "batch.after_tool_use_persist";
    /// Before the batch message persists.
    pub const BATCH_BEFORE_MESSAGE: &str = "batch.before_message";
    /// After the batch message committed.
    pub const BATCH_AFTER_MESSAGE: &str = "batch.after_message";
    /// Before the `turn_finished` step.
    pub const TURN_BEFORE_FINISHED: &str = "turn.before_finished";
    /// After the `turn_finished` step committed.
    pub const TURN_AFTER_FINISHED: &str = "turn.after_finished";
    /// Suffixes of `call.<i>.<suffix>`.
    pub const CALL_BEFORE_STARTED: &str = "before_started";
    /// After `tool_calls → started` committed.
    pub const CALL_AFTER_STARTED: &str = "after_started";
    /// Inside a fake tool.
    pub const CALL_MID: &str = "mid";
    /// Before `tool_calls → finished`.
    pub const CALL_BEFORE_FINISHED: &str = "before_finished";
    /// After `tool_calls → finished` committed.
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
                d.handoff_tokens,
                d.acp_respawn_window_days,
                d.acp_respawn_parallel
            ),
            (4, 10, 5000, 5, 7, 4000, 7, 4)
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
