//! omp adapter (141 §6.4, D7): the two engine-side feeds for omp's `task`
//! subagents — *live* progress rows riding the parent's `tool_call_update`
//! frames (`progress_rows`) and the *tail* of the child's on-disk transcript
//! (`Tailer`, `to_updates`). The acp crate exposes the adapter; the
//! `AcpChild` actor calls it (141 §7.3).
//!
//! 141 C stub: every body here is the contract only — W5 (OMP) replaces this
//! module. Signatures and field names are final; `progress_rows` returns
//! `None` (no frame is a task frame), `Tailer::locate` returns `None` (no
//! transcript is found) and `to_updates` returns nothing, so the actor's
//! omp branches are reachable but inert until W5 lands.

use std::io;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::SessionUpdate;
use serde::Deserialize;

use crate::harness::{ChildAnnounce, ChildSource, ChildState, CostRule};

/// One `task` frame's worth of subagent rows (141 §6.4 feed 1): the
/// `tool_call_update` (or `tool_call`) whose `rawOutput.details.progress[]`
/// is an array of rows. Each frame is a full snapshot; the latest wins per id.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskFrame {
    /// `toolCallId` of the frame — the spawning tool call (`observed_tool_call`).
    pub tool_call_id: String,
    /// `details.async` present: background spawn, excluded from the parent's
    /// `usage_update` → `CostRule::Counted` (F23). Absent: sync → `InsideParent`.
    pub background: bool,
    /// `details.async.state` (`running` / `completed`), the roster footer.
    pub batch_state: Option<String>,
    /// The rows, in `index` order as sent.
    pub rows: Vec<Progress>,
}

/// A recent tool of a subagent (`progress[].recentTools[]`, F19).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentTool {
    /// Tool name.
    pub tool: String,
    /// Rendered arguments.
    #[serde(default)]
    pub args: String,
    /// Unix ms when it ended; `None` while running.
    #[serde(default)]
    pub end_ms: Option<u64>,
}

/// Retry bookkeeping of a subagent (`retryState` / `retryFailure`, F19).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryState {
    /// Attempt number.
    #[serde(default)]
    pub attempt: Option<u32>,
    /// Attempts allowed.
    #[serde(default)]
    pub max_attempts: Option<u32>,
    /// Backoff before the next attempt.
    #[serde(default)]
    pub delay_ms: Option<u64>,
    /// The error that caused the retry / the final failure.
    #[serde(default)]
    pub error_message: Option<String>,
}

/// One omp `AgentProgress` row (F19; capture `omp-18.3.1-task.jsonl` line 27).
/// `id` is the child's external id (= job id = transcript stem; nested
/// `a.b`); `status` is `pending | running | completed | failed | aborted`.
/// Every other field is optional: omp fills them in as the child runs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Position in the batch.
    #[serde(default)]
    pub index: Option<u32>,
    /// External id / job id / transcript stem.
    pub id: String,
    /// `pending | running | completed | failed | aborted`.
    pub status: String,
    /// Agent type (`scout`, `task`, …).
    #[serde(default)]
    pub agent: Option<String>,
    /// `bundled | project | user`.
    #[serde(default)]
    pub agent_source: Option<String>,
    /// Model role the agent was configured with.
    #[serde(default)]
    pub model_role: Option<String>,
    /// The task text (prompt).
    #[serde(default)]
    pub task: Option<String>,
    /// The rendered assignment (preferred for `observed_description`).
    #[serde(default)]
    pub assignment: Option<String>,
    /// Recent tools, newest first.
    #[serde(default)]
    pub recent_tools: Vec<RecentTool>,
    /// Recent output lines.
    #[serde(default)]
    pub recent_output: Vec<String>,
    /// Tools called so far.
    #[serde(default)]
    pub tool_count: Option<u32>,
    /// Model requests so far.
    #[serde(default)]
    pub requests: Option<u32>,
    /// input + output + cacheWrite tokens (billing volume).
    #[serde(default)]
    pub tokens: Option<u64>,
    /// USD so far.
    #[serde(default)]
    pub cost: Option<f64>,
    /// Wall time so far.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// `provider/model:thinking` the child resolved to → `harness_model`.
    #[serde(default)]
    pub resolved_model: Option<String>,
    /// `provider/model` without the thinking suffix.
    #[serde(default)]
    pub resolved_model_identity: Option<String>,
    /// Thinking level in force.
    #[serde(default)]
    pub resolved_thinking_level: Option<String>,
    /// Latest context size.
    #[serde(default)]
    pub context_tokens: Option<u64>,
    /// Context window.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// What the child says it is doing.
    #[serde(default)]
    pub last_intent: Option<String>,
    /// The tool running now.
    #[serde(default)]
    pub current_tool: Option<String>,
    /// Its rendered arguments.
    #[serde(default)]
    pub current_tool_args: Option<String>,
    /// `extractedToolData.yield[0].data` and friends, kept raw → `observed_detail.result`.
    #[serde(default)]
    pub extracted_tool_data: Option<serde_json::Value>,
    /// Retry in progress.
    #[serde(default)]
    pub retry_state: Option<RetryState>,
    /// Retries exhausted.
    #[serde(default)]
    pub retry_failure: Option<RetryState>,
}

impl Progress {
    /// The announce for this row (141 §6.4 feed 1 table): identity from `id`
    /// (nested `a.b` → parent `a`), the frame's tool call and cost rule, the
    /// state from `status`, `resolvedModel` as the model, `assignment` (else
    /// `task`) head as the description; live numbers go to `detail`.
    ///
    /// 141 C stub: W5 fills `detail` per the §6.4 table; this body maps the
    /// identity, state, model and tool call only.
    #[must_use]
    pub fn announce(&self, frame: &TaskFrame) -> ChildAnnounce {
        let mut a = ChildAnnounce::new(ChildSource::OmpTask, self.id.clone());
        a.parent = self.id.rsplit_once('.').map(|(p, _)| p.to_owned());
        a.title = Some(self.id.clone());
        a.tool_call = Some(frame.tool_call_id.clone());
        a.model = self.resolved_model.clone();
        a.cost_rule = Some(if frame.background {
            CostRule::Counted
        } else {
            CostRule::InsideParent
        });
        a.state = Some(match self.status.as_str() {
            "pending" => ChildState::Pending,
            "running" => ChildState::Running,
            "completed" => ChildState::Completed,
            "failed" => ChildState::Failed {
                error: self
                    .retry_failure
                    .as_ref()
                    .and_then(|r| r.error_message.clone()),
            },
            "aborted" => ChildState::Cancelled,
            _ => ChildState::Unknown,
        });
        a
    }
}

/// The task frame in `update`, if it is one: any `tool_call` /
/// `tool_call_update` whose `rawOutput.details.progress` is an array of
/// objects with string `id` and `status` — regardless of the call's own
/// `status` (omp keeps updating a `completed` call, F18). `None` otherwise.
///
/// 141 C stub: always `None`; W5 implements the structural recogniser.
#[must_use]
pub fn progress_rows(update: &SessionUpdate) -> Option<TaskFrame> {
    let _ = update;
    None
}

/// Where omp keeps one ACP session's transcripts on disk (141 §6.4 feed 2,
/// F21): the parent file `~/.omp/agent/sessions/<encoded-cwd>/<ts>_<sid>.jsonl`
/// and, beside it, the artifacts dir `<same path minus .jsonl>/` holding
/// `<id>.jsonl` / `<id>.md` / `<id>.json` per child and `<a>/<a>.<b>.jsonl`
/// for nested ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tailer {
    /// The engine's home (`~`); omp writes under `<home>/.omp`.
    pub home: PathBuf,
    /// The parent session's `.jsonl`.
    pub parent_file: PathBuf,
    /// The artifacts dir (parent file minus `.jsonl`).
    pub artifacts: PathBuf,
}

impl Tailer {
    /// Find the parent transcript for `acp_session` (the ACP session id ==
    /// omp session id) under `home`: glob `.omp/agent/sessions/*/*_<sid>.jsonl`.
    /// `None` until omp has written it (retried on the first task frame).
    ///
    /// 141 C stub: always `None`; W5 implements the glob.
    #[must_use]
    pub fn locate(home: &Path, acp_session: &str) -> Option<Tailer> {
        let _ = (home, acp_session);
        None
    }

    /// The child transcript for external id `id`: `<artifacts>/<id>.jsonl`;
    /// nested `a.b` → `<artifacts>/a/a.b.jsonl`.
    #[must_use]
    pub fn child_path(&self, id: &str) -> PathBuf {
        match id.rsplit_once('.') {
            Some((parent, _)) => self.artifacts.join(parent).join(format!("{id}.jsonl")),
            None => self.artifacts.join(format!("{id}.jsonl")),
        }
    }

    /// Whole lines of `id`'s transcript from byte `cursor`, parsed into
    /// [`Entry`]s, and the new cursor (a partial trailing line is left unread).
    ///
    /// 141 C stub: reads nothing; W5 implements.
    pub fn read(&self, id: &str, cursor: u64) -> io::Result<(Vec<Entry>, u64)> {
        let _ = id;
        Ok((Vec::new(), cursor))
    }

    /// `<id>.md` exists: the child finished and omp wrote its final output.
    ///
    /// 141 C stub: always `false`; W5 implements.
    #[must_use]
    pub fn sidecar_done(&self, id: &str) -> bool {
        let _ = id;
        false
    }

    /// Job ids the parent transcript settled since `parent_cursor`
    /// (`custom_message{customType:"async-result"}.details.jobs[].jobId`, F22)
    /// and the new cursor.
    ///
    /// 141 C stub: reads nothing; W5 implements.
    pub fn settled_jobs(&self, parent_cursor: u64) -> io::Result<(Vec<String>, u64)> {
        Ok((Vec::new(), parent_cursor))
    }
}

/// A tool call inside a child transcript entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Tool call id (shared by the `toolCall` part, `tool_execution_start` and `toolResult`).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as sent (JSON text).
    pub arguments: String,
    /// The intent the model gave, if any.
    pub intent: Option<String>,
}

/// Usage of one assistant entry in a child transcript (`message.usage`, F21).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Cache read tokens.
    pub cache_read: u64,
    /// Cache write tokens.
    pub cache_write: u64,
    /// Total tokens as omp reports them.
    pub total_tokens: u64,
    /// `cost.total` in USD.
    pub cost: f64,
}

/// One parsed child-transcript jsonl entry (141 §6.4 feed 2 table, F21).
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// `session {id, cwd, parentSession}`.
    Session {
        /// omp's own session id for the child.
        id: String,
        /// Working directory.
        cwd: Option<String>,
        /// The parent transcript path omp recorded.
        parent_session: Option<String>,
    },
    /// `model_change {model}`.
    Model(String),
    /// `message role:user` text.
    User(String),
    /// `message role:assistant`: text parts, tool-call parts, usage, model.
    Assistant {
        /// Concatenated text parts.
        text: String,
        /// Tool-call parts in order.
        calls: Vec<Call>,
        /// Usage of this response.
        usage: Option<Usage>,
        /// `provider/model` of this response.
        model: Option<String>,
    },
    /// `custom {customType:"tool_execution_start"}`.
    ToolStart(Call),
    /// `message role:toolResult`.
    ToolResult {
        /// Tool call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Result text.
        text: String,
        /// The tool failed.
        is_error: bool,
    },
    /// `title`, `session_init`, `thinking_level_change`, anything else.
    Other,
}

/// The SDK `SessionUpdate`s that replay `entries` on the child so
/// `acp_child`'s `translate()` is reused verbatim (141 D3): assistant text →
/// `agent_message_chunk`, a call → `tool_call`, a result →
/// `tool_call_update completed|failed`; user entries produce nothing (the
/// actor opens the turn itself).
///
/// 141 C stub: always empty; W5 implements the mapping.
#[must_use]
pub fn to_updates(entries: &[Entry]) -> Vec<SessionUpdate> {
    let _ = entries;
    Vec::new()
}
