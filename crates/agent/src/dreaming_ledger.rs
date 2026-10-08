//! The day ledger dreaming's Recall phase reads (design doc 136 §4 phase 1,
//! contract §2.2): read-only aggregates over the agent store for a time
//! window, plus knowledge usage counters for the ids shown in it.
//!
//! Wave C lands the shapes and an empty handler; `Agents::handle(LedgerSince)`
//! (136 W2) runs the queries: `sessions`, `messages`, `inner_calls`,
//! `tool_calls`, `turn_steps`, `questions`, `model_calls ⟕ cost_owners`, and
//! asks knowledge for `Stats`. Caps: 500 sessions, 5,000 user messages,
//! 2,000 tool errors; past a cap `truncated = true`. Private sessions and
//! `exclude` never appear; text passes the spill redactor.

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// Session cap of one ledger (136 §2.2).
pub const MAX_SESSIONS: usize = 500;
/// User-message cap of one ledger (136 §2.2).
pub const MAX_USER_MESSAGES: usize = 5_000;
/// Tool-error cap of one ledger (136 §2.2).
pub const MAX_TOOL_ERRORS: usize = 2_000;
/// Longest `text` / `dropped_assistant_text` kept, bytes (136 §2.2).
pub const MAX_TEXT_BYTES: usize = 2 * 1024;
/// Longest tool-error first line kept, chars (136 §2.2).
pub const MAX_ERROR_CHARS: usize = 300;

/// `ask` on the supervisor → `Result<Ledger, String>` (136 §2.2): everything
/// that happened between `since` and `until` (RFC 3339), minus `exclude` (the
/// dreaming session and its children) and, with `owner_only`, minus desks and
/// agents' sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerSince {
    /// Window start, RFC 3339.
    pub since: String,
    /// Window end, RFC 3339.
    pub until: String,
    /// Sessions left out (the asking routine's tree).
    #[serde(default)]
    pub exclude: Vec<SessionId>,
    /// Only the owner's own sessions (no desks, no agents' work sessions).
    #[serde(default)]
    pub owner_only: bool,
}

/// One session in the window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerSession {
    /// Session id.
    pub id: SessionId,
    /// `sessions.title`.
    pub title: String,
    /// `root | child | desk | routine`.
    pub kind: String,
    /// Parent session for a child.
    pub parent: Option<SessionId>,
    /// RFC 3339.
    pub created_at: String,
    /// Finished turns in the window.
    pub turns: u64,
    /// Tool calls in the window.
    pub tool_calls: u64,
    /// Tool calls that returned an error.
    pub errors: u64,
    /// Sum of `returned_nanos ?? estimated_nanos` of the session's calls.
    pub model_cost_nanos: u64,
}

/// A plain typed user message (no leading `<` or `[`), ≤ 2 KB, secrets scrubbed —
/// also the shape of an `[app …]` press row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerUserMessage {
    /// Session id.
    pub session: SessionId,
    /// `messages.seq`.
    pub seq: i64,
    /// RFC 3339.
    pub at: String,
    /// The text, clipped.
    pub text: String,
}

/// A name with counts: inner calls by tool name, skill loads by skill name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerCount {
    /// Tool or skill name.
    pub name: String,
    /// Calls (loads).
    pub calls: u64,
    /// Calls that errored.
    pub errors: u64,
    /// Distinct sessions.
    pub sessions: u64,
}

/// One failed tool call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerToolError {
    /// Session id.
    pub session: SessionId,
    /// The turn.
    pub turn_id: String,
    /// Tool name.
    pub name: String,
    /// `tool_calls.attempt`.
    pub attempt: u32,
    /// RFC 3339.
    pub at: String,
    /// First line of the output, ≤ 300 chars.
    pub output: String,
}

/// One approval / ask / handoff / secret question and its answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerQuestion {
    /// Session id.
    pub session: SessionId,
    /// `approval | ask | handoff | secret`.
    pub kind: String,
    /// `pending | answered | superseded | timed_out`.
    pub state: String,
    /// `allow | deny | …` (the answer text for an ask).
    pub answer: Option<String>,
    /// `human | double`.
    pub answered_by: Option<String>,
    /// RFC 3339.
    pub asked_at: String,
}

/// A turn that ended other than `end_turn`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerStop {
    /// Session id.
    pub session: SessionId,
    /// The turn.
    pub turn_id: String,
    /// `turn_steps(turn_finished).payload.stop_reason`.
    pub stop_reason: String,
    /// RFC 3339.
    pub at: String,
}

/// Impressions and opens of one hinted id (from knowledge `Stats`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerHintUse {
    /// Concept id.
    pub id: String,
    /// Times shown.
    pub shown: u64,
    /// Times opened.
    pub opened: u64,
}

/// A rewind: the corrected tail is gone from the store, so only what the
/// agent still holds (`resume_reports`, the durability journal) is returned;
/// dreaming's `Signals` actor snapshots the text at event time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerRewind {
    /// Session id.
    pub session: SessionId,
    /// RFC 3339.
    pub at: String,
    /// The dropped assistant text, ≤ 2 KB, scrubbed; may be empty.
    pub dropped_assistant_text: String,
}

/// The day ledger (136 §2.2) — one struct, serde + ts export.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Ledger {
    /// Window start, RFC 3339.
    pub since: String,
    /// Window end, RFC 3339.
    pub until: String,
    /// Sessions in the window (≤ 500).
    pub sessions: Vec<LedgerSession>,
    /// Plain typed user messages (≤ 5,000).
    pub user_messages: Vec<LedgerUserMessage>,
    /// Inner (python) tool calls by name.
    pub inner_calls: Vec<LedgerCount>,
    /// Failed tool calls (≤ 2,000).
    pub tool_errors: Vec<LedgerToolError>,
    /// Questions asked and how they were answered.
    pub questions: Vec<LedgerQuestion>,
    /// Turns that did not end normally.
    pub stops: Vec<LedgerStop>,
    /// User messages beginning `[app `.
    pub app_presses: Vec<LedgerUserMessage>,
    /// Usage of the ids shown in the window.
    pub hints: Vec<LedgerHintUse>,
    /// Skill loads by skill name.
    pub skill_loads: Vec<LedgerCount>,
    /// Rewinds the agent still knows about.
    pub rewinds: Vec<LedgerRewind>,
    /// A cap was hit; the ledger is a prefix.
    pub truncated: bool,
}

impl Ledger {
    /// The empty ledger of a window: what wave C's handler answers.
    pub fn empty(since: &str, until: &str) -> Self {
        Self {
            since: since.to_owned(),
            until: until.to_owned(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ledger_carries_the_window_and_is_not_truncated() {
        let l = Ledger::empty("2026-10-07T00:00:00Z", "2026-10-08T00:00:00Z");
        assert_eq!(l.since, "2026-10-07T00:00:00Z");
        assert!(!l.truncated);
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["sessions"], serde_json::json!([]));
        assert_eq!(v["truncated"], false);
        let back: Ledger = serde_json::from_value(v).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn ledger_since_defaults_exclude_and_owner_only() {
        let m: LedgerSince = serde_json::from_str(
            r#"{"since":"2026-10-07T00:00:00Z","until":"2026-10-08T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(m.exclude.is_empty());
        assert!(!m.owner_only);
    }
}
