//! Public types of tasks (67 §7). Everything that crosses the RPC surface
//! (`tasks_list`, `task_*`) is exported to TypeScript.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Task id: uuidv7 as text.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
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

/// How soon a task wants attention (132 D4). The user alone sets it; one
/// [`crate::rank::rank`] orders the inbox and an agent's wake dispatch by it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Its own group at the top of the rail.
    Urgent,
    /// Before the ordinary.
    High,
    /// The default.
    #[default]
    Medium,
    /// When nothing else waits.
    Low,
}

impl Priority {
    /// agentbox weights: 9 / 7 / 5 / 2.
    pub const fn weight(self) -> u32 {
        match self {
            Self::Urgent => 9,
            Self::High => 7,
            Self::Medium => 5,
            Self::Low => 2,
        }
    }

    /// Column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// The inverse of [`Self::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "urgent" => Some(Self::Urgent),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
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
    /// 214 A1: task ids that were in `blockers` until that task closed (done/cancelled) —
    /// the finished dependencies, kept for the graph. A manual removal does not add here.
    #[serde(default)]
    pub resolved_blockers: Vec<TaskId>,
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
    /// Acceptance criteria (70 DESK-PROTOCOL): what `done` is judged against
    /// first. Drafted from the brief by whoever creates the task when the user
    /// did not state it; `None` = judged on the user's evident intention, and
    /// then self-review is not allowed.
    #[serde(default)]
    pub done_when: Option<String>,
    /// Things attached to the task (70 TASK-ATTACHMENTS): the same references a chat
    /// message carries (design 66) — notes, knowledge, artifacts, chats, meetings. A desk
    /// sees them as `[attached …]` lines under the brief and reads them with the per-kind tools.
    #[serde(default)]
    pub attachments: Vec<agent::attach::AttachRef>,
    /// How soon it wants attention (132 D4); the user alone sets it.
    #[serde(default)]
    pub priority: Priority,
    /// 182 D1: the engine (mesh id) that works the task — only it dispatches,
    /// checks out and sweeps it; `None` = a legacy row, worked by the hub
    /// lease holder. Stamped at creation; changed only by `task_move_home`.
    #[serde(default)]
    pub home: Option<String>,
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
    /// Sent back from review to `doing`/`todo`; pick it up again.
    Resumed,
    /// `/wake`, the board, the CLI.
    Manual,
    /// A task was handed to it by its previous owner (70 DESK-PROTOCOL: the
    /// fit check); the thread says why.
    Reassigned,
    /// Every blocker of a task it owns is resolved: the work it delegated or
    /// waited on is in.
    BlockersResolved,
    /// Every child of a task it owns is done or cancelled.
    ChildrenCompleted,
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
            Self::Resumed => "resumed",
            Self::Manual => "manual",
            Self::Reassigned => "reassigned",
            Self::BlockersResolved => "blockers_resolved",
            Self::ChildrenCompleted => "children_completed",
        }
    }

    /// Every reason, for the store's "not applicable" filters.
    pub const ALL: [Self; 12] = [
        Self::Assigned,
        Self::Comment,
        Self::Mention,
        Self::Schedule,
        Self::Answer,
        Self::Review,
        Self::Resume,
        Self::Resumed,
        Self::Manual,
        Self::Reassigned,
        Self::BlockersResolved,
        Self::ChildrenCompleted,
    ];

    /// Whether a wake with this reason still has a purpose on a task in
    /// `state` (STALE-WAKE-DESKS). A closed task wants nobody — except an
    /// `answer` on a `done` one (132 D3: the reviewer's next move, `**next**`
    /// in the thread, is the owner's to pick up); `review` is the reviewer's
    /// move (and the manager's on a `blocked: no continuation`); the "pick it
    /// up" reasons need the task open for work; the rest (comment, mention,
    /// answer, manual, schedule) hold in any open state.
    pub const fn applies_to(self, state: TaskState) -> bool {
        use TaskState::*;
        match self {
            Self::Answer if matches!(state, Done) => true,
            _ if matches!(state, Done | Cancelled) => false,
            Self::Review => matches!(state, Review | Blocked),
            Self::Assigned
            | Self::Reassigned
            | Self::Resume
            | Self::Resumed
            | Self::BlockersResolved
            | Self::ChildrenCompleted => {
                matches!(state, Todo | Doing)
            }
            Self::Comment | Self::Mention | Self::Schedule | Self::Answer | Self::Manual => true,
        }
    }
}

/// One row of `wakes` (67 §7.3): coalesced on `key`.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
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

/// Why a task is `done` (70 DESK-PROTOCOL, Sky's ruling): the reviewer judges
/// against `done_when` first, the user's evident intention second, and third
/// the follow-up tasks that carry the work on. Written into the thread as
/// `done because: …`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DoneBecause {
    /// `done_when` is met.
    CriteriaMet,
    /// No explicit criteria, or they miss the point: the user's evident intention is met.
    IntentMet,
    /// The work continues in follow-up tasks that naturally follow this one.
    FollowUpsFiled {
        /// The follow-ups.
        tasks: Vec<TaskId>,
    },
}

impl std::fmt::Display for DoneBecause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CriteriaMet => f.write_str("criteria met"),
            Self::IntentMet => f.write_str("intent met"),
            Self::FollowUpsFiled { tasks } => {
                write!(
                    f,
                    "follow-ups filed ({})",
                    tasks
                        .iter()
                        .map(|t| t.0.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

/// One row of `task_events`: a field-level change (67 §7.2).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
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
    /// 182 D3: only tasks this engine works (homed here; a legacy row while
    /// it holds the lease) — `task_inbox`.
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub here: bool,
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
    /// Acceptance criteria; `None` = none stated (the creator should draft them from the brief).
    #[serde(default)]
    pub done_when: Option<String>,
    /// Attached references (design 66 refs), deduplicated by kind + id.
    #[serde(default)]
    pub attachments: Vec<agent::attach::AttachRef>,
    /// Priority; `None` = medium. The ledger refuses it from an agent.
    #[serde(default)]
    pub priority: Option<Priority>,
    /// Task ids or `decision:<id>` / `person:<name>` entries this task waits on — the
    /// dependency edges of a plan, set at create time. Checked as in `task_update`: duplicates
    /// dropped; an unknown task id or a list that would form a cycle refused. A `todo` task
    /// with an open blocker is created `blocked`.
    #[serde(default)]
    pub blockers: Vec<String>,
}

impl TaskCreate {
    const fn default_state() -> TaskState {
        TaskState::Backlog
    }
}

/// What a [`TaskChanged`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TaskChangeKind {
    /// A new task.
    Created,
    /// Any field (title, brief, priority, project, blockers, owner, …) but the state.
    Updated,
    /// The state.
    Moved,
    /// A comment or progress note in the thread.
    Commented,
    /// The row is gone.
    Deleted,
}

/// Bus event (214 D3): a task row changed — the UI's live board and graph refetch on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct TaskChanged {
    /// The task.
    pub id: TaskId,
    /// Its project label after the change.
    pub project: Option<String>,
    /// What changed.
    pub kind: TaskChangeKind,
}

impl wire::WireEvent for TaskChanged {
    const TOPIC: &'static str = "tasks.changed";
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
    /// Acceptance criteria.
    pub done_when: Option<String>,
    /// Attached references, replaced whole; each added/removed one is an `attached`/`detached` event.
    pub attachments: Option<Vec<agent::attach::AttachRef>>,
    /// Required from an agent moving a task to `done`; `user`/`system` default to intent met.
    pub done_because: Option<DoneBecause>,
    /// Priority; the user only (the ledger refuses agents).
    pub priority: Option<Priority>,
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
            done_when: None,
            attachments: vec![],
            priority: None,
            blockers: vec![],
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["where"], "build-A");
        assert!(v.get("where_").is_none());
    }

    #[test]
    fn create_state_defaults_to_backlog_on_the_wire() {
        let t: TaskCreate = serde_json::from_value(
            serde_json::json!({ "title": "t", "brief": "", "owner": { "type": "user" } }),
        )
        .unwrap();
        assert_eq!(t.state, TaskState::Backlog);
        assert_eq!(serde_json::to_value(TaskState::Backlog).unwrap(), "backlog");
    }
}
