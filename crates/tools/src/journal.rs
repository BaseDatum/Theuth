//! Design doc 109 §3.3 step 6: the journal seam. The agent installs a
//! [`Journal`] on the registry, jobs, approval and ask actors
//! ([`InstallJournal`], like `InstallActivity`); this crate keeps no `sqlx`
//! dependency and only hands over plain rows. Before any install (and in
//! tests) every actor holds a [`NoopJournal`].

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ToolOutput;
use crate::jobs::{JobExit, JobId};

/// What the agent's store records on behalf of this crate (§10.2). Every
/// method is fire-and-forget from the caller's point of view: a failed write
/// is logged by the implementation, never surfaced to the tool.
pub trait Journal: Send + Sync {
    /// `tool_calls → started` immediately before a call's body runs.
    fn call_started(&self, session: SessionId, call_id: &str) -> BoxFuture<'_, ()>;
    /// `tool_calls → finished` as the body returns.
    fn call_finished(
        &self,
        session: SessionId,
        call_id: &str,
        out: &ToolOutput,
    ) -> BoxFuture<'_, ()>;
    /// A human wait raised or resolved (§6.1): upsert by `q.id`.
    fn question(&self, q: QuestionRow) -> BoxFuture<'_, ()>;
    /// A `started` call began (`Some`) or stopped (`None`) blocking on
    /// something external (§4.4).
    fn wait(&self, w: Option<WaitRow>, session: SessionId, call_id: &str) -> BoxFuture<'_, ()>;
    /// A job spawned or settled (§5.1): upsert by `(j.session, j.id)`.
    fn job(&self, j: JobRow) -> BoxFuture<'_, ()>;
    /// The last rendered PTY screen (§5.3): upsert by `(s.session, s.job_id)`.
    fn pty_screen(&self, s: PtyScreenRow) -> BoxFuture<'_, ()>;
}

/// `ask` → `()`: install the agent's journal on the registry, which forwards
/// it to the jobs, approval and ask actors. Installing again replaces it.
pub struct InstallJournal(pub Arc<dyn Journal>);

/// The journal before any install: records nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopJournal;

impl Journal for NoopJournal {
    fn call_started(&self, _: SessionId, _: &str) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn call_finished(&self, _: SessionId, _: &str, _: &ToolOutput) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn question(&self, _: QuestionRow) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn wait(&self, _: Option<WaitRow>, _: SessionId, _: &str) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn job(&self, _: JobRow) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn pty_screen(&self, _: PtyScreenRow) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// `questions.kind` (§6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// The registry's approval gate.
    Approval,
    /// `ask_user`.
    Ask,
    /// `desktop_handoff`.
    Handoff,
    /// `secret_request` / `secret_request_form`.
    Secret,
}

impl QuestionKind {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Ask => "ask",
            Self::Handoff => "handoff",
            Self::Secret => "secret",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "approval" => Self::Approval,
            "ask" => Self::Ask,
            "handoff" => Self::Handoff,
            "secret" => Self::Secret,
            _ => return None,
        })
    }
}

/// `questions.state` (§6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    /// Waiting on a human.
    Pending,
    /// A human answered.
    Answered,
    /// Replaced by a newer question (§6.3).
    Superseded,
    /// The deadline passed.
    TimedOut,
}

impl QuestionState {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered => "answered",
            Self::Superseded => "superseded",
            Self::TimedOut => "timed_out",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "answered" => Self::Answered,
            "superseded" => Self::Superseded,
            "timed_out" => Self::TimedOut,
            _ => return None,
        })
    }
}

/// Who settled a question (126 §4.3): the owner, or the owner's double. Written to
/// `questions.answered_by` as `human` | `double`; `None` on a row still pending or
/// settled by a timeout. The `answer` JSON keeps its bare shape (109's replay reads it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Answerer {
    /// A person, through any frontend.
    Human,
    /// The `double` plugin acting at L2/L3.
    Double,
}

impl Answerer {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Double => "double",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "human" => Self::Human,
            "double" => Self::Double,
            _ => return None,
        })
    }
}

/// One `questions` row (§3.1, §6.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionRow {
    /// `ApprovalId` / `AskId` / `HandoffId` / `SecretRequestId` as text.
    pub id: String,
    /// The asking session.
    pub session: SessionId,
    /// The call that asked; `None` for an approval raised before the call is `started`.
    pub tool_call_id: Option<String>,
    /// Which actor raised it.
    pub kind: QuestionKind,
    /// `ApprovalRequested` / `AskRequested` / … as JSON.
    pub payload: Value,
    /// Lifecycle state.
    pub state: QuestionState,
    /// When it was raised.
    pub asked_at: DateTime<Utc>,
    /// `asked_at` + the kind's timeout; absolute, so downtime counts (§6.3).
    pub deadline: DateTime<Utc>,
    /// When it left `pending`.
    pub answered_at: Option<DateTime<Utc>>,
    /// Decision / answers JSON / outcome.
    pub answer: Option<Value>,
    /// Who answered (126 §4.3); `None` while pending or on a timeout.
    #[serde(default)]
    pub answered_by: Option<Answerer>,
}

/// `waits.kind` (§4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// `send_message{await_reply}` on an accepted exchange.
    PeerReply,
    /// An owned child's `result:` exchange.
    ChildResult,
    /// The `shell` call waiting out `job_grace_secs`.
    Job,
    /// `wait` on a PTY.
    PtyWait,
    /// A human-wait tool or the approval gate.
    Question,
    /// `wait_peer` on a child's state (155 §5.4); outlives its turn like `ChildResult`.
    PeerState,
}

impl WaitKind {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PeerReply => "peer_reply",
            Self::ChildResult => "child_result",
            Self::Job => "job",
            Self::PtyWait => "pty_wait",
            Self::Question => "question",
            Self::PeerState => "peer_state",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "peer_reply" => Self::PeerReply,
            "child_result" => Self::ChildResult,
            "job" => Self::Job,
            "pty_wait" => Self::PtyWait,
            "question" => Self::Question,
            "peer_state" => Self::PeerState,
            _ => return None,
        })
    }
}

/// One `waits` row (§3.1, §4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaitRow {
    /// The blocked session.
    pub session: SessionId,
    /// The `started` call that blocks.
    pub tool_call_id: String,
    /// What it blocks on.
    pub kind: WaitKind,
    /// Exchange id / child session / job id / question id.
    pub target: String,
    /// `None` = no deadline (a child result).
    pub deadline: Option<DateTime<Utc>>,
    /// `pty_wait`: `{pattern, cursor}`; `peer_reply`: `{request_id}`.
    pub payload: Value,
}

/// `jobs.kind` (§5.1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// `shell`.
    #[default]
    Shell,
    /// `start`.
    Pty,
    /// 145 §0.4: an inline tool call that outlived `promote_after`; `command`
    /// is the tool name, there is no process and no log.
    Promoted,
}

impl JobKind {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Pty => "pty",
            Self::Promoted => "promoted",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "shell" => Self::Shell,
            "pty" => Self::Pty,
            "promoted" => Self::Promoted,
            _ => return None,
        })
    }
}

/// `jobs.state` (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Spawned, not settled.
    Running,
    /// Exit / timeout / cancel.
    Settled,
    /// Died with the engine or reaped (§5.4).
    Lost,
}

impl JobState {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Settled => "settled",
            Self::Lost => "lost",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => Self::Running,
            "settled" => Self::Settled,
            "lost" => Self::Lost,
            _ => return None,
        })
    }
}

/// One `jobs` row (§3.1, §5.1); mirrors the in-memory `Job`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    /// Owning session.
    pub session: SessionId,
    /// `j<n>` per session.
    pub id: JobId,
    /// Shell or PTY.
    pub kind: JobKind,
    /// The `shell` / `start` call.
    pub call_id: String,
    /// The command line.
    pub command: String,
    /// `start`'s label.
    pub name: Option<String>,
    /// The group leader's id, for the reaper.
    pub pgid: Option<i64>,
    /// Log path; `None` for a quiet job.
    pub log: Option<PathBuf>,
    /// Spawn time, unix ms.
    pub started_ms: i64,
    /// Lifecycle state.
    pub state: JobState,
    /// How it ended; `None` while running.
    pub exit: Option<JobExit>,
    /// Settlement time, unix ms.
    pub settled_ms: Option<i64>,
    /// 139 D14: the engine that spawned the process (`sessions.owner`
    /// spelling, `''` on a lone engine). Left empty by the tools crate;
    /// the agent's `Journal` stamps it. The reaper signals only its own.
    #[serde(default)]
    pub engine: String,
}

impl JobRow {
    /// 171F3 R1: a user-cell kernel's row — `cell:` name, the kernel command
    /// marker, or a `.cell.log` log (any one: rows of older engines may lack
    /// a newer marker). Its end is never a `<job_result>` and its screen is
    /// never model input.
    pub fn is_cell_kernel(&self) -> bool {
        crate::jobs::is_cell(self.name.as_deref())
            || self.command.ends_with(crate::jobs::CELL_COMMAND)
            || self.log.as_deref().is_some_and(|l| {
                l.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(crate::jobs::CELL_LOG))
            })
    }
}

/// One `pty_screens` row (§3.1, §5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyScreenRow {
    /// Owning session.
    pub session: SessionId,
    /// The PTY job.
    pub job_id: JobId,
    /// `PtyHandle.revision` at capture.
    pub revision: u64,
    /// Screen height.
    pub rows: u16,
    /// Screen width.
    pub cols: u16,
    /// Cursor row.
    pub cursor_row: u16,
    /// Cursor column.
    pub cursor_col: u16,
    /// Rows joined by `'\n'`, scrubbed.
    pub text: String,
    /// Capture time, unix ms.
    pub at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip() {
        for k in [
            QuestionKind::Approval,
            QuestionKind::Ask,
            QuestionKind::Handoff,
            QuestionKind::Secret,
        ] {
            assert_eq!(QuestionKind::parse(k.as_str()), Some(k));
        }
        for s in [
            QuestionState::Pending,
            QuestionState::Answered,
            QuestionState::Superseded,
            QuestionState::TimedOut,
        ] {
            assert_eq!(QuestionState::parse(s.as_str()), Some(s));
        }
        for w in [
            WaitKind::PeerReply,
            WaitKind::ChildResult,
            WaitKind::Job,
            WaitKind::PtyWait,
            WaitKind::Question,
            WaitKind::PeerState,
        ] {
            assert_eq!(WaitKind::parse(w.as_str()), Some(w));
        }
        for j in [JobKind::Shell, JobKind::Pty, JobKind::Promoted] {
            assert_eq!(JobKind::parse(j.as_str()), Some(j));
        }
        for s in [JobState::Running, JobState::Settled, JobState::Lost] {
            assert_eq!(JobState::parse(s.as_str()), Some(s));
        }
        assert_eq!(QuestionKind::parse("nope"), None);
    }
}
