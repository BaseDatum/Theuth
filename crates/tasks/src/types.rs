//! Public types of tasks (67 §7). Everything that crosses the RPC surface
//! (`tasks_list`, `task_*`) is exported to TypeScript.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Task id: uuidv7 as text.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// A fresh uuidv7 id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a task is in its life (67 §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Created, not dispatched: no wake, no placement, not in any desk inbox.
    Backlog,
    /// Queued for the owner: woken, placed, in the desk inbox; not checked out.
    Todo,
    /// Checked out by the owner's session.
    Doing,
    /// Owner finished; waits for the reviewer.
    Review,
    /// Waiting on a decision, a person or another task.
    Blocked,
    /// Reviewer approved.
    Done,
    /// Dropped.
    Cancelled,
}

impl TaskState {
    /// The serde name (`todo`, `doing`, …), also the `tasks.state` column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Review => "review",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Who owns (or reviews) a task. JSON: `{ "type": "agent", "name": "ops" }` |
/// `{ "type": "user" }`; text (columns, filters, prompts): `agent:ops` | `user`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum Owner {
    /// A roster agent by name.
    Agent(String),
    /// The user.
    User,
    /// The harness itself: reviewer of a schedule task its `until` closed (67 §7.4).
    System,
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(name) => write!(f, "agent:{name}"),
            Self::User => f.write_str("user"),
            Self::System => f.write_str("system"),
        }
    }
}

impl FromStr for Owner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            _ => s
                .strip_prefix("agent:")
                .filter(|n| !n.is_empty())
                .map(|n| Self::Agent(n.to_owned()))
                .ok_or_else(|| format!("owner: expected agent:<name> or user, got {s:?}")),
        }
    }
}

/// The session working a task right now (67 §7.5, decision 21).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Checkout {
    /// Work session id.
    pub session: String,
    /// Engine the session runs on.
    pub engine: String,
    /// Bumped on every checkout, so a resumed task's stale writer loses.
    pub epoch: u32,
    /// Last progress note (ms since epoch); the orphan timer runs from here.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub progress_at_ms: u64,
    /// Set by the orphan sweep (67 §7.5): the session is presumed lost; the
    /// next checkout may take the task and bumps `epoch`.
    #[serde(default)]
    pub orphaned: bool,
    /// Set with `orphaned` when the sweep took the session onto `engine`
    /// (68 §4.4): the `Resume` wake continues that session.
    #[serde(default)]
    pub taken: bool,
}

/// The task record (67 §7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Task {
    /// uuidv7.
    pub id: TaskId,
    /// One line.
    pub title: String,
    /// Markdown.
    pub brief: String,
    /// Exactly one owner.
    pub owner: Owner,
    /// State.
    pub state: TaskState,
    /// Who marks it done; `None` = the owner's `reports_to`.
    pub reviewer: Option<Owner>,
    /// Task ids, `decision:<approval id>`, `person:<name>`.
    pub blockers: Vec<String>,
    /// Tasks whose `blockers` name this one (the inverse, computed on read, never stored).
    #[serde(default)]
    pub blocks: Vec<TaskId>,
    /// Delegation tree.
    pub parent: Option<TaskId>,
    /// Project label (`agent`'s `projects.name`, the session project); `None` = no project.
    pub project: Option<String>,
    /// Engine name, written by theuth at delegation (67 §8).
    #[serde(rename = "where")]
    pub where_: Option<String>,
    /// `native` | `omp` | `opencode` | `claude`, written by theuth (67 §8).
    pub harness: Option<String>,
    /// Linked external tickets (URLs).
    pub external: Vec<String>,
    /// Thread document id (59): comments, questions, answers, notes.
    pub thread: Option<String>,
    /// Current checkout, while `doing`.
    pub checkout: Option<Checkout>,
    /// Routine provenance: `(schedule name, fire time ms)`; the coalescing key (67 §7.4).
    #[cfg_attr(feature = "ts", ts(type = "[string, number] | null"))]
    pub schedule: Option<(String, u64)>,
    /// `agent:<name>` | `user` | `system`.
    pub created_by: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_ms: u64,
    /// Consecutive wakes that changed no field (67 §7.3 back-off).
    pub no_progress_count: u32,
    /// Back-off: no wake is delivered before this (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub next_wake_after_ms: Option<u64>,
}

/// Why an agent is woken (67 §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum WakeReason {
    /// A task was assigned to it.
    Assigned,
    /// A thread entry on one of its tasks.
    Comment,
    /// Named in a chat message or a thread.
    Mention,
    /// A schedule fired (67 §7.4).
    Schedule,
    /// A decision or question it waited on was answered.
    Answer,
    /// A task it reviews entered `review`.
    Review,
    /// Its checkout's engine was lost; resume elsewhere (67 §7.5).
    Resume,
    /// `/wake`, the board, the CLI.
    Manual,
}

impl WakeReason {
    /// The serde name, also the `wakes.reason` column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Comment => "comment",
            Self::Mention => "mention",
            Self::Schedule => "schedule",
            Self::Answer => "answer",
            Self::Review => "review",
            Self::Resume => "resume",
            Self::Manual => "manual",
        }
    }
}

/// One row of `wakes` (67 §7.3): coalesced on `key`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Wake {
    /// uuidv7.
    pub id: String,
    /// Agent name (no `agent:` prefix).
    pub agent: String,
    /// The task, when the wake is about one.
    pub task: Option<TaskId>,
    /// Why.
    pub reason: WakeReason,
    /// Idempotency key `(agent, task, reason)` as text.
    pub key: String,
    /// Times it was raised before being claimed.
    pub count: u32,
    /// Desk session that took it; `None` while queued.
    pub claimed_by: Option<String>,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
}

impl Wake {
    /// The coalescing key for `(agent, task, reason)` (67 §7.3).
    pub fn key_of(agent: &str, task: Option<&TaskId>, reason: WakeReason) -> String {
        format!(
            "{agent}|{}|{}",
            task.map_or("", |t| t.0.as_str()),
            reason.as_str()
        )
    }
}

/// How an episode leaves a task it checked out (67 §7.3, decision 12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Disposition {
    /// Marked done by its reviewer.
    Done,
    /// Handed to a reviewer.
    Review {
        /// Who reviews.
        reviewer: Owner,
    },
    /// Waiting on something named.
    Blocked {
        /// `decision:<id>`, `person:<name>`, a task id, or free text.
        on: String,
    },
    /// A child task exists with another owner.
    Delegated {
        /// The child.
        to: TaskId,
    },
    /// Something will wake it: a schedule, a reminder, a live child.
    Continuing {
        /// What will.
        by: String,
    },
}

/// One row of `task_events`: a field-level change (67 §7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TaskEvent {
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// `agent:<name>` | `user` | `system`.
    pub actor: String,
    /// Session that wrote it, when one did.
    pub session: Option<String>,
    /// Field name (`state`, `owner`, `comment`, …).
    pub field: String,
    /// Previous value as text.
    pub before: Option<String>,
    /// New value as text.
    pub after: Option<String>,
}

/// `tasks_list` filter; every field narrows, all `None` = every task.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct TaskFilter {
    /// `agent:<name>` | `user`.
    pub owner: Option<String>,
    /// Any of these states.
    pub state: Option<Vec<TaskState>>,
    /// `where` equals this engine name.
    pub engine: Option<String>,
    /// `project` equals this label.
    pub project: Option<String>,
}

/// `task_create` input. Missing `owner` = `user`; `reviewer` defaults per 67 §7.1;
/// `state` (`backlog` | `todo`) defaults to `backlog` on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TaskCreate {
    /// `backlog` (default: created, not dispatched) or `todo` (the owner is woken).
    #[serde(default = "TaskCreate::default_state")]
    pub state: TaskState,
    /// One line.
    pub title: String,
    /// Markdown.
    pub brief: String,
    /// Owner.
    pub owner: Owner,
    /// Reviewer; `None` = the owner's `reports_to`.
    #[serde(default)]
    pub reviewer: Option<Owner>,
    /// Parent task for delegation.
    #[serde(default)]
    pub parent: Option<TaskId>,
    /// Project label.
    #[serde(default)]
    pub project: Option<String>,
    /// Engine name.
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// Harness.
    #[serde(default)]
    pub harness: Option<String>,
    /// External ticket URLs.
    #[serde(default)]
    pub external: Vec<String>,
}

impl TaskCreate {
    const fn default_state() -> TaskState {
        TaskState::Backlog
    }
}

/// `task_update` patch: `Some` = set, `None` = leave. Guarded transitions
/// (`state` under a checkout, `done` needing a reviewer) are checked by the actor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields = nullable))]
#[serde(default)]
pub struct TaskPatch {
    /// Title.
    pub title: Option<String>,
    /// Brief.
    pub brief: Option<String>,
    /// Owner.
    pub owner: Option<Owner>,
    /// Reviewer.
    pub reviewer: Option<Owner>,
    /// State.
    pub state: Option<TaskState>,
    /// Blockers, replaced whole.
    pub blockers: Option<Vec<String>>,
    /// Engine name.
    #[serde(rename = "where")]
    pub where_: Option<String>,
    /// Project label.
    pub project: Option<String>,
    /// Harness.
    pub harness: Option<String>,
    /// External URLs, replaced whole.
    pub external: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_text_round_trip() {
        assert_eq!("agent:ops".parse::<Owner>(), Ok(Owner::Agent("ops".into())));
        assert_eq!("user".parse::<Owner>(), Ok(Owner::User));
        assert!("agent:".parse::<Owner>().is_err());
        assert!("bob".parse::<Owner>().is_err());
        assert_eq!(Owner::Agent("ops".into()).to_string(), "agent:ops");
        assert_eq!(Owner::User.to_string(), "user");
    }

    #[test]
    fn owner_json_is_tagged() {
        let v = serde_json::to_value(Owner::Agent("ops".into())).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "agent", "name": "ops" }));
        let v = serde_json::to_value(Owner::User).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "user" }));
    }

    #[test]
    fn where_is_renamed_on_the_wire() {
        let t = TaskCreate {
            state: TaskState::Todo,
            title: "t".into(),
            brief: String::new(),
            owner: Owner::User,
            reviewer: None,
            parent: None,
            project: None,
            where_: Some("build-A".into()),
            harness: None,
            external: vec![],
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["where"], "build-A");
        assert!(v.get("where_").is_none());
    }

    #[test]
    fn create_state_defaults_to_backlog_on_the_wire() {
        let t: TaskCreate =
            serde_json::from_value(serde_json::json!({ "title": "t", "brief": "", "owner": { "type": "user" } })).unwrap();
        assert_eq!(t.state, TaskState::Backlog);
        assert_eq!(serde_json::to_value(TaskState::Backlog).unwrap(), "backlog");
    }
}
