//! The task tools (67 §7, §8, §11), registered by `TasksPlugin::start` once
//! the tool registry is up: `task_create`, `task_inbox`, `task_checkout`,
//! `task_update`, `task_move`, `task_note`, `task_place`, `escalate`, `decide`.
//! Each is one message to the [`Tasks`] actor; the caller's identity comes from
//! the roster through [`CallerOf`].

use std::sync::Arc;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::{Context, Message};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::place;
use crate::thread::Thread;
use crate::types::{DoneBecause, Disposition, Owner, TaskCreate, TaskFilter, TaskId, TaskPatch, TaskState};
use crate::{
    AnswerDecision, AwaitDecision, CheckoutTask, CommentTask, CreateTask, Escalate, GetTask,
    ListTasks, MoveTask, NoteDisposition, PlaceTask, ReassignTask, Tasks, UpdateTask, escalate,
};

/// `ask` → `String`: the actor text of `session`'s caller — `agent:<name>`
/// when the roster knows the session, else `user`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallerOf(pub String);

impl Message<CallerOf> for Tasks {
    type Reply = String;

    async fn handle(&mut self, m: CallerOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .agent_for(&m.0)
            .map_or_else(|| "user".to_owned(), |a| format!("agent:{a}"))
    }
}

/// The tasks plugin's system-prompt paragraph (`RegisterPrompt { plugin: "tasks" }`,
/// 69 TASKS): the board is primarily for observation; tasks are made in chat.
pub const PROMPT: &str = "## Tasks\n\
The board is for observation; work is filed from chat. When the user describes work to be done (a request \
with pieces, \"we should…\", \"have ops…\", a plan), do not create tasks straight away: call `task_draft` with \
the proposed tasks and wait for the user's approval, which arrives as their next message (`[app task-drafts …]` \
naming a draft id and a choice). Then `task_create` with `state: backlog` for \"add to backlog\", `state: todo` \
for \"start now\" (the owner is woken and the work placed), nothing for \"discard\". Every task carries \
`done_when`, its acceptance criteria: take them from the user when stated, otherwise draft one to three \
checkable lines from the brief. A task created in `backlog` \
runs nothing until it is started. When the user asks to start, pull back, drop, approve or send back a task, \
use `task_move` (it refuses illegal moves and lists what is allowed from the task's state); a move to `done` \
says why (`done_because`: criteria met, intent met, or follow-ups filed). Your own work on a \
task is `task_checkout`; finishing it is `task_update` with a disposition; handing it to the agent whose \
capabilities fit it better is `task_reassign`.";

/// Register the ten tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>) {
    let _ = reg.ask(RegisterTool(Arc::new(TaskCreateTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskInboxTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskCheckoutTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskUpdateTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskMoveTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskReassignTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskNoteTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TaskPlaceTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(EscalateTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DecideTool))).await;
}

fn actor() -> anyhow::Result<ActorRef<Tasks>> {
    crate::actor().ok_or_else(|| anyhow::anyhow!("tasks is not running"))
}

/// The actor's own `Err` and a mailbox failure, as one string for the model.
fn flat<T, M>(r: Result<T, SendError<M, String>>) -> Result<T, String> {
    r.map_err(|e| match e {
        SendError::HandlerError(msg) => msg,
        other => format!("tasks actor: {other}"),
    })
}

async fn caller(a: &ActorRef<Tasks>, cx: &ToolCx) -> String {
    a.ask(CallerOf(cx.session.to_string()))
        .await
        .unwrap_or_else(|_| "user".to_owned())
}

/// The calling session's project label, from the agent's session list.
async fn session_project(cx: &ToolCx) -> Option<String> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR).ok().flatten()?;
    agents
        .ask(agent::ListSessions)
        .await
        .ok()?
        .into_iter()
        .find(|s| s.id == cx.session)
        .and_then(|s| s.project)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn out<T: Serialize>(r: Result<T, String>) -> anyhow::Result<ToolOutput> {
    Ok(match r {
        Ok(v) => ToolOutput::ok(serde_json::to_string_pretty(&v)?),
        Err(e) => ToolOutput::err(e),
    })
}

fn parse_owner(s: Option<&str>) -> anyhow::Result<Option<Owner>> {
    s.map(|s| s.parse::<Owner>().map_err(anyhow::Error::msg))
        .transpose()
}

/// `task_create`'s `state`: `backlog` unless the caller passed `todo`; anything else is refused.
fn create_state(s: Option<TaskState>) -> Result<TaskState, String> {
    match s.unwrap_or(TaskState::Backlog) {
        s @ (TaskState::Backlog | TaskState::Todo) => Ok(s),
        s => Err(format!("state is backlog or todo, not {}", s.as_str())),
    }
}

/// Arguments for [`TaskCreateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreateArgs {
    /// One line, imperative: what done looks like.
    pub title: String,
    /// Markdown brief: context, constraints, acceptance. The whole of what the owner gets.
    pub brief: String,
    /// `backlog` (default: on the board, not dispatched — nobody is woken) or `todo` (the owner is
    /// woken and the work is placed). Pass `todo` only when the user asked to start it now.
    #[serde(default)]
    pub state: Option<TaskState>,
    /// `agent:<name>` or `user`. Default: the calling agent.
    #[serde(default)]
    pub owner: Option<String>,
    /// `agent:<name>` or `user`; default: the owner's `reports_to`.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Parent task id when this is a delegation.
    #[serde(default)]
    pub parent: Option<String>,
    /// Project the task belongs to (a project name as sessions carry it); default: the calling session's.
    #[serde(default)]
    pub project: Option<String>,
    /// Engine name the work must run on (from `mesh_peers`); leave unset to let theuth place it.
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// `native` | `omp` | `opencode` | `claude`; leave unset to let theuth choose.
    #[serde(default)]
    pub harness: Option<String>,
    /// Linked external tickets (URLs).
    #[serde(default)]
    pub external: Vec<String>,
    /// Acceptance criteria: the checkable conditions under which this task is done. Required in
    /// spirit — when the user did not state them, draft them from the brief (one to three
    /// checkable lines); the reviewer judges `done` against them first.
    #[serde(default)]
    pub done_when: Option<String>,
}

/// `task_create`: a new task record; owner and reviewer default per the line.
pub struct TaskCreateTool;

#[async_trait::async_trait]
impl Tool for TaskCreateTool {
    const NAME: &'static str = "task_create";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskCreateArgs;

    fn description(&self) -> String {
        "Create a task: a first-class record on the board with one owner (an agent or the user), a state \
         (`backlog` by default — not dispatched; `todo` when the user wants it started now: the owner is woken \
         with reason `assigned`), a project (the same project labels sessions carry; defaults to your session's), \
         an optional reviewer, blockers, a parent for delegation, and a placement (`where` engine, \
         `harness`). Use it to delegate work to another agent (set `parent` to your current task so the \
         tree shows it), to hand something to the user, or to put your own future work on the board. \
         When the user describes work in chat, prefer `task_draft` first and create only after approval. \
         External tickets (Jira, GitHub) are links on the task, never the source of truth. Returns the task id \
         and its line."
            .into()
    }

    async fn call(&self, a: TaskCreateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let owner = match parse_owner(a.owner.as_deref())? {
            Some(o) => o,
            None => me.parse().unwrap_or(Owner::User),
        };
        let project = match a.project {
            Some(p) => Some(p),
            None => session_project(&cx).await,
        };
        let state = match create_state(a.state) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let input = TaskCreate {
            state,
            title: a.title,
            brief: a.brief,
            owner,
            reviewer: parse_owner(a.reviewer.as_deref())?,
            parent: a.parent.map(TaskId),
            project,
            where_: a.where_,
            harness: a.harness,
            external: a.external,
            done_when: a.done_when,
        };
        let r = flat(
            actor
                .ask(CreateTask {
                    input,
                    created_by: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        );
        Ok(match r {
            Ok(t) => ToolOutput::ok(format!(
                "created task {} — owner {}, reviewer {}, project {}, state {}",
                t.id,
                t.owner,
                t.reviewer.as_ref().map_or("none".to_owned(), ToString::to_string),
                t.project.as_deref().unwrap_or("none"),
                t.state.as_str()
            )),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Arguments for [`TaskInboxTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TaskInboxArgs {
    /// States to show; default `todo`, `doing`, `blocked`.
    #[serde(default)]
    pub state: Option<Vec<TaskState>>,
    /// Only tasks in this project; default: every project.
    #[serde(default)]
    pub project: Option<String>,
}

/// `task_inbox`: the calling agent's own open tasks.
pub struct TaskInboxTool;

#[async_trait::async_trait]
impl Tool for TaskInboxTool {
    const NAME: &'static str = "task_inbox";
    const RISK: Risk = Risk::ReadOnly;
    type Args = TaskInboxArgs;

    fn description(&self) -> String {
        "Your inbox: the tasks you own in `todo`, `doing` or `blocked` (or the states you pass), grouped by \
         project (tasks without one last), oldest first within a group, one line each — id, state, title, \
         placement, blockers, when it last moved. Pass `project` to see one project only. It shows only \
         your own tasks: do not go looking for other agents' work. Read it at the start of a wake, then \
         `task_checkout` the one the wake named."
            .into()
    }

    async fn call(&self, a: TaskInboxArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let filter = TaskFilter {
            owner: Some(me),
            state: Some(a.state.unwrap_or_else(|| {
                vec![TaskState::Todo, TaskState::Doing, TaskState::Blocked]
            })),
            engine: None,
            project: a.project,
        };
        let tasks = match flat(actor.ask(ListTasks(filter)).await) {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let now = now_ms();
        let mut text = String::new();
        let mut group: Option<Option<String>> = None;
        for t in tasks.iter().filter(|t| t.next_wake_after_ms.is_none_or(|w| w <= now)) {
            if group.as_ref() != Some(&t.project) {
                group = Some(t.project.clone());
                text.push_str(&format!("## {}\n", t.project.as_deref().unwrap_or("(no project)")));
            }
            let place = match (&t.where_, &t.harness) {
                (None, None) => "unplaced".to_owned(),
                (w, h) => format!("{}/{}", w.as_deref().unwrap_or("?"), h.as_deref().unwrap_or("?")),
            };
            text.push_str(&format!(
                "- {} [{}] {} — {}{} — moved {}\n",
                t.id,
                t.state.as_str(),
                t.title,
                place,
                if t.blockers.is_empty() { String::new() } else { format!(" — blocked on {}", t.blockers.join(", ")) },
                chrono::DateTime::from_timestamp_millis(t.updated_ms as i64)
                    .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default()
            ));
        }
        if text.is_empty() {
            text.push_str("inbox empty");
        }
        Ok(ToolOutput::ok(text))
    }
}

/// Arguments for [`TaskCheckoutTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskCheckoutArgs {
    /// Task id.
    pub id: TaskId,
}

/// `task_checkout`: take a task into `doing` under the calling session.
pub struct TaskCheckoutTool;

#[async_trait::async_trait]
impl Tool for TaskCheckoutTool {
    const NAME: &'static str = "task_checkout";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskCheckoutArgs;

    fn description(&self) -> String {
        "Check a task out: moves it `todo`/`blocked` → `doing` with this session as its checkout (session, \
         engine, epoch), so no other session can move it until you dispose of it. Refused if another live \
         session holds it or you are not the owner. Returns the full record: brief, thread (comments, \
         answers, the last progress note), placement (`where`, `harness`) — open the work session exactly \
         as the placement says."
            .into()
    }

    async fn call(&self, a: TaskCheckoutArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let t = match flat(
            actor
                .ask(CheckoutTask {
                    id: a.id,
                    session: cx.session.to_string(),
                    actor: me,
                })
                .await,
        ) {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut text = serde_json::to_string_pretty(&t)?;
        if let Some(doc) = &t.thread
            && let Some(md) = crate::thread::DocThread.read(doc).await
        {
            text.push_str("\n\n--- thread ---\n");
            text.push_str(&md);
        }
        Ok(ToolOutput::ok(text))
    }
}

/// Arguments for [`TaskUpdateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdateArgs {
    /// Task id.
    pub id: TaskId,
    /// How you leave a task you worked on: `done` (reviewers only), `review {reviewer}`, `blocked {on}`, `delegated {to}` (a child task exists; the task waits on it), `continuing {by}` (a schedule, reminder or live child will wake it). State changes other than `cancelled` and send-back go through this.
    #[serde(default)]
    pub disposition: Option<Disposition>,
    /// `cancelled`, or `doing` to send a reviewed task back to its owner. Other states: use `disposition`.
    #[serde(default)]
    pub state: Option<TaskState>,
    /// New owner (`agent:<name>` | `user`); the new owner is woken.
    #[serde(default)]
    pub owner: Option<String>,
    /// New reviewer (`agent:<name>` | `user`).
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Blockers, replaced whole: task ids, `decision:<id>`, `person:<name>`.
    #[serde(default)]
    pub blockers: Option<Vec<String>>,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New brief.
    #[serde(default)]
    pub brief: Option<String>,
    /// External ticket URLs, replaced whole.
    #[serde(default)]
    pub external: Option<Vec<String>>,
    /// Acceptance criteria (replaces `done_when`).
    #[serde(default)]
    pub done_when: Option<String>,
    /// With `disposition: done`: why — `criteria_met` (against `done_when`), `intent_met` (the user's
    /// evident intention), or `follow_ups_filed {tasks}` (the work continues in those tasks).
    #[serde(default)]
    pub done_because: Option<DoneBecause>,
}

/// `task_update`: change fields and state; every change is a thread event.
pub struct TaskUpdateTool;

#[async_trait::async_trait]
impl Tool for TaskUpdateTool {
    const NAME: &'static str = "task_update";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskUpdateArgs;

    fn description(&self) -> String {
        "Update a task's fields or dispose of it. Dispositions: `review` when you finished (the reviewer \
         is woken; without a `done_when` on the task the reviewer is your manager, never yourself), \
         `blocked` naming what you wait on, `delegated` naming the child task that now carries \
         the work, `continuing` naming what will wake the task again, `done` (reviewers only, with \
         `done_because`; a desk that is its own reviewer marks done only against an explicit `done_when` and \
         only as `criteria_met`). `state: doing` sends a reviewed task \
         back to its owner; `state: cancelled` drops it. A write by a session that does not hold the \
         checkout is refused once the checkout moved on (`superseded`). One session may write to at most 20 \
         tasks it does not own. Every change is logged to the thread with your session and before/after."
            .into()
    }

    async fn call(&self, a: TaskUpdateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let session = Some(cx.session.to_string());
        let mut patch = TaskPatch {
            title: a.title,
            brief: a.brief,
            owner: parse_owner(a.owner.as_deref())?,
            reviewer: parse_owner(a.reviewer.as_deref())?,
            state: None,
            blockers: a.blockers,
            external: a.external,
            done_when: a.done_when,
            done_because: a.done_because,
            ..Default::default()
        };
        match a.state {
            None => {}
            Some(TaskState::Cancelled | TaskState::Doing) => patch.state = a.state,
            Some(s) => {
                return Ok(ToolOutput::err(format!(
                    "state {} goes through `disposition`",
                    s.as_str()
                )));
            }
        }
        let mut note = None;
        if let Some(d) = a.disposition {
            let current = match flat(actor.ask(GetTask(a.id.clone())).await) {
                Ok(t) => t,
                Err(e) => return Ok(ToolOutput::err(e)),
            };
            let mut blockers = patch.blockers.take().unwrap_or(current.blockers);
            match d {
                Disposition::Done => patch.state = Some(TaskState::Done),
                Disposition::Review { reviewer } => {
                    patch.state = Some(TaskState::Review);
                    patch.reviewer = Some(reviewer);
                }
                Disposition::Blocked { on } => {
                    patch.state = Some(TaskState::Blocked);
                    if !blockers.contains(&on) {
                        blockers.push(on);
                    }
                }
                Disposition::Delegated { to } => {
                    if let Err(e) = flat(actor.ask(GetTask(to.clone())).await) {
                        return Ok(ToolOutput::err(format!("delegated to {to}: {e}")));
                    }
                    patch.state = Some(TaskState::Blocked);
                    if !blockers.contains(&to.0) {
                        blockers.push(to.0.clone());
                    }
                    note = Some(format!("delegated:{to}"));
                }
                Disposition::Continuing { by } => note = Some(format!("continuing:{by}")),
            }
            patch.blockers = Some(blockers);
        }
        let r = flat(
            actor
                .ask(UpdateTask {
                    id: a.id.clone(),
                    patch,
                    actor: me.clone(),
                    session: session.clone(),
                })
                .await,
        );
        if r.is_ok()
            && let Some(text) = note
        {
            let _ = actor
                .ask(NoteDisposition {
                    id: a.id,
                    text,
                    actor: me,
                    session,
                })
                .await;
        }
        out(r)
    }
}

/// Arguments for [`TaskMoveTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskMoveArgs {
    /// Task id.
    pub task: TaskId,
    /// Target state: `todo` (start a backlog task; resume a blocked one; send a reviewed one back),
    /// `backlog` (pull a queued task back), `done` (reviewer approves), `cancelled` (drop).
    pub state: TaskState,
    /// Why, for the thread (optional).
    #[serde(default)]
    pub note: Option<String>,
    /// With `state: done`: why — `criteria_met`, `intent_met`, or `follow_ups_filed {tasks}`. Required
    /// from an agent; the user's approval defaults to `intent_met`.
    #[serde(default)]
    pub done_because: Option<DoneBecause>,
}

/// `task_move`: a state change on the user's (or the caller's) intent.
pub struct TaskMoveTool;

#[async_trait::async_trait]
impl Tool for TaskMoveTool {
    const NAME: &'static str = "task_move";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskMoveArgs;

    fn description(&self) -> String {
        "Move a task between states when the user asks (\"start that\", \"put it back\", \"drop it\", \
         \"approve it\"): backlog ↔ todo, anything → cancelled, review → done | todo (reviewer), blocked → todo \
         (the user's move clears system blockers such as `no continuation`; a decision needs its answer, a task \
         blocker its task). `todo` from backlog wakes the owner. Illegal moves are refused \
         with the list of states allowed from where the task is. Working a task yourself is `task_checkout`; \
         finishing one is `task_update` with a disposition."
            .into()
    }

    async fn call(&self, a: TaskMoveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let r = flat(
            actor
                .ask(MoveTask {
                    id: a.task,
                    state: a.state,
                    note: a.note,
                    done_because: a.done_because,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        );
        Ok(match r {
            Ok(t) => ToolOutput::ok(format!("task {} is now {} (owner {})", t.id, t.state.as_str(), t.owner)),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Arguments for [`TaskReassignTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskReassignArgs {
    /// Task id.
    pub task: TaskId,
    /// New owner: `agent:<name>` (an active roster agent) or `user`.
    pub to: String,
    /// The fit: which of their capabilities match the task better than yours.
    pub why: String,
}

/// `task_reassign`: the fit check's hand-off (70 DESK-PROTOCOL 1).
pub struct TaskReassignTool;

#[async_trait::async_trait]
impl Tool for TaskReassignTool {
    const NAME: &'static str = "task_reassign";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskReassignArgs;

    fn description(&self) -> String {
        "Hand a task you own to the agent whose capabilities fit it better (see `agents_list`). The task \
         returns to `todo`, its checkout is released, the thread records `reassigned to <agent>: <why>` and \
         the new owner is woken with reason `reassigned`. Do this before any work when the fit check says \
         someone else is the specialist; do not do it to get rid of a task nobody fits — work that yourself."
            .into()
    }

    async fn call(&self, a: TaskReassignArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let Some(to) = parse_owner(Some(&a.to))? else {
            return Ok(ToolOutput::err("`to` names the new owner"));
        };
        let r = flat(
            actor
                .ask(ReassignTask {
                    id: a.task,
                    to,
                    why: a.why,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        );
        Ok(match r {
            Ok(t) => ToolOutput::ok(format!("task {} reassigned to {} ({})", t.id, t.owner, t.state.as_str())),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Arguments for [`TaskNoteTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskNoteArgs {
    /// Task id.
    pub id: TaskId,
    /// Markdown; a comment, a question to the owner or reviewer, an answer, or a progress note.
    pub text: String,
    /// `true`: this is a progress note (where you are, what is left) that a resumed session starts from; default `false` = a comment.
    #[serde(default)]
    pub progress: bool,
}

/// `task_note`: write to the task thread.
pub struct TaskNoteTool;

#[async_trait::async_trait]
impl Tool for TaskNoteTool {
    const NAME: &'static str = "task_note";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskNoteArgs;

    fn description(&self) -> String {
        "Write to a task's thread. A comment wakes the owner (and the reviewer when the task is in review) \
         with reason `comment`; a question to the user reaches them through their channels. With \
         `progress: true` it is a progress note: a few lines on what is done and what is next, written at \
         natural boundaries (after a batch of tool results, every few minutes, and before you dispose of \
         the task) so that if this engine is lost the task can be resumed elsewhere from your note. \
         Progress notes wake nobody."
            .into()
    }

    async fn call(&self, a: TaskNoteArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let r = flat(
            actor
                .ask(CommentTask {
                    id: a.id,
                    text: a.text,
                    progress: a.progress,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        );
        Ok(match r {
            Ok(()) => ToolOutput::ok(if a.progress { "progress noted" } else { "comment added" }),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Arguments for [`TaskPlaceTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskPlaceArgs {
    /// Task id.
    pub id: TaskId,
    /// What the work needs, in plain words: a repo path, a display, a signed-in browser, a GPU, a harness.
    #[serde(default)]
    pub needs: Option<String>,
    /// The engine theuth's desk chose.
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// The harness theuth's desk chose: `native` | `omp` | `opencode` | `claude`.
    #[serde(default)]
    pub harness: Option<String>,
    /// One line: why this placement.
    #[serde(default)]
    pub reason: Option<String>,
}

/// `ask` → [`PlaceContextReply`]: what `task_place` needs from the ledger.
#[derive(Clone, Debug)]
pub struct PlaceContext {
    /// The task owner's bare agent name (`user` for the user).
    pub owner: String,
}

/// Reply to [`PlaceContext`].
#[derive(Clone, Debug, Default, kameo::Reply)]
pub struct PlaceContextReply {
    /// theuth's desk session, when open.
    pub desk: Option<String>,
    /// The owner's `tools` ceiling is `read_only`.
    pub read_only: bool,
    /// `tasks.place_prefer`.
    pub prefer: Vec<String>,
}

impl Message<PlaceContext> for Tasks {
    type Reply = PlaceContextReply;

    async fn handle(&mut self, m: PlaceContext, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let router = self.ledger.router.as_ref();
        PlaceContextReply {
            desk: router.and_then(|r| r.desk_for("theuth")),
            read_only: router
                .and_then(|r| r.tools_of(&m.owner))
                .is_some_and(|t| t == "read_only"),
            prefer: self.ledger.cfg.place_prefer.clone(),
        }
    }
}

/// How long one desk turn may take before the placement gives up (the
/// question stands; the desk's late answer arrives as an ordinary message).
const DESK_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// The engines that can start work now: this one (by its `mesh.name`, hostname
/// by default — as the mesh's peers know it) and every reachable peer, with
/// what each advertises.
async fn engines() -> Vec<place::Engine> {
    let Some(mesh) = mesh::actor() else {
        return Vec::new();
    };
    let Ok(status) = mesh.ask(mesh::Status).await else {
        return Vec::new();
    };
    let stats = mesh.ask(mesh::ListStats).await.unwrap_or_default();
    let me = (status.self_name, status.self_id);
    std::iter::once(me)
        .chain(status.peers.into_iter().filter(|p| p.reachable).map(|p| (p.name, p.id)))
        .filter_map(|(name, id)| {
            let stats = stats.iter().find(|s| s.id == id)?.clone();
            Some(place::Engine { name, stats })
        })
        .collect()
}

/// Working directories of the sessions in `project`.
async fn project_cwds(project: Option<&str>) -> Vec<String> {
    let Some(project) = project else {
        return Vec::new();
    };
    let Some(agents) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR).ok().flatten() else {
        return Vec::new();
    };
    let mut cwds: Vec<String> = agents
        .ask(agent::ListSessions)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.project.as_deref() == Some(project))
        .filter_map(|s| s.cwd)
        .collect();
    cwds.sort();
    cwds.dedup();
    cwds
}

/// One turn of theuth's desk over the 50 peer-message path: `prompt` as a
/// `<message>` from the calling session with `await_reply`, the reply text
/// back. The desk asking about its own task answers itself instead.
async fn ask_desk(desk: Option<String>, from: runtime::SessionId, task: &TaskId, prompt: String) -> Result<String, String> {
    let desk = desk.ok_or("theuth's desk is not open")?;
    if desk == from.to_string() {
        return Err(format!(
            "you are theuth's desk: decide and call task_place again with `where`, `harness` and `reason`.\n{prompt}"
        ));
    }
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent supervisor not running")?;
    let admitted = tools::communication::current().ok_or("peer messaging is off in this session")?;
    let d = agents
        .ask(agent::supervisor::Deliver {
            from,
            to: format!("session:{desk}"),
            text: prompt,
            await_reply: true,
            reply_to: None,
            request_id: Some(format!("place:{task}:{}", now_ms())),
            admitted: admitted.token().clone(),
        })
        .await
        .map_err(|e| match e {
            SendError::HandlerError(m) => format!("theuth's desk: {m}"),
            other => format!("agent supervisor: {other}"),
        })?;
    let rx = d.reply.ok_or("theuth's desk did not take the question")?;
    match tokio::time::timeout(DESK_WAIT, rx).await {
        Ok(Ok(r)) => r.reply.ok_or_else(|| r.error.unwrap_or_else(|| "theuth's desk answered nothing".into())),
        Ok(Err(_)) => Err("theuth's desk: wait dropped".into()),
        Err(_) => {
            let _ = agents.tell(agent::supervisor::DropWait(d.receipt.id)).await;
            Err(format!("theuth's desk did not answer within {}s", DESK_WAIT.as_secs()))
        }
    }
}

/// `task_place`: where and in what a task runs (67 §8, decision 3) — policy
/// first ([`place::decide`]), theuth's desk when several engines fit.
pub struct TaskPlaceTool;

#[async_trait::async_trait]
impl Tool for TaskPlaceTool {
    const NAME: &'static str = "task_place";
    const RISK: Risk = Risk::Mutating;
    type Args = TaskPlaceArgs;

    fn description(&self) -> String {
        "Place a task that reached you without a placement: policy reads the task, what it needs \
         (`needs`, plus the brief) and what engines advertise (`mesh_peers`) — a repo on one engine, a display, \
         a GPU, the owner's `tools` ceiling — and writes `where` (an engine) and `harness` (native / omp / opencode / \
         claude) onto the task; when several engines fit, theuth's desk is asked for the pick. Call it before \
         opening a work session for a task whose `where` is empty; never choose an engine yourself. Pass `where` \
         and `harness` only when you are theuth's desk answering. When no engine satisfies the needs the task is set \
         `blocked: engine:<what is missing>`."
            .into()
    }

    async fn call(&self, a: TaskPlaceArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let session = Some(cx.session.to_string());
        let mut task = match flat(actor.ask(GetTask(a.id.clone())).await) {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        // An explicit answer (theuth's desk, or a human) overrides what the task says.
        if a.where_.is_some() {
            task.where_ = a.where_;
        }
        if a.harness.is_some() {
            task.harness = a.harness;
        }
        let owner = match &task.owner {
            Owner::Agent(n) => n.clone(),
            _ => "user".to_owned(),
        };
        let ctx = actor.ask(PlaceContext { owner }).await.unwrap_or_default();
        let (engines, cwds) = tokio::join!(engines(), project_cwds(task.project.as_deref()));
        let inputs = place::Inputs {
            task: &task,
            needs: a.needs.as_deref(),
            read_only: ctx.read_only,
            project_cwds: &cwds,
            engines: &engines,
            prefer: &ctx.prefer,
        };
        let desk = ctx.desk.clone();
        let placed = place::place(&inputs, |prompt| ask_desk(desk, cx.session, &a.id, prompt)).await;
        match placed {
            Ok(p) => {
                let reason = match a.reason {
                    Some(r) if p.reason == "set on the task" => r,
                    _ => p.reason,
                };
                out(flat(
                    actor
                        .ask(PlaceTask {
                            id: a.id,
                            where_: Some(p.where_),
                            harness: Some(p.harness),
                            reason,
                            actor: me,
                            session,
                        })
                        .await,
                ))
            }
            Err(missing) if missing.starts_with("engine:") => {
                let blocked = TaskPatch {
                    state: Some(TaskState::Blocked),
                    blockers: Some(vec![missing.clone()]),
                    ..Default::default()
                };
                let r = flat(
                    actor
                        .ask(UpdateTask {
                            id: a.id,
                            patch: blocked,
                            actor: me,
                            session,
                        })
                        .await,
                );
                Ok(ToolOutput::err(match r {
                    Ok(_) => format!("no engine satisfies the needs; task set blocked on `{missing}`"),
                    Err(e) => format!("no engine satisfies the needs (`{missing}`); could not block the task: {e}"),
                }))
            }
            Err(e) => Ok(ToolOutput::err(e)),
        }
    }
}

/// Arguments for [`EscalateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EscalateArgs {
    /// Task id the decision is about.
    pub task: TaskId,
    /// The question, with the context the decider needs to answer without reading the whole thread.
    pub question: String,
    /// Concrete options, when there are some; the decider may still answer freely.
    #[serde(default)]
    pub options: Vec<String>,
}

/// `escalate`: a decision request up the reporting line (67 §11, decision 8).
pub struct EscalateTool;

#[async_trait::async_trait]
impl Tool for EscalateTool {
    const NAME: &'static str = "escalate";
    const RISK: Risk = Risk::Mutating;
    type Args = EscalateArgs;

    fn description(&self) -> String {
        "Ask your manager for a decision you cannot make yourself. The request goes up your reporting \
         line as a wake with reason `answer`: a manager that is an agent answers with `decide` or \
         escalates further; the user gets it as a card on their channels with the options as buttons. \
         You wait up to `roster.decision_wait_secs` (two minutes) for a live answer; if it comes it is \
         in this tool's output and the task is unblocked. If none comes the task stays \
         `blocked: decision:<id>`, your episode ends with its journal, and the answer re-wakes you. \
         Do not poll: either the reply is in this tool's output or you should end your turn."
            .into()
    }

    async fn call(&self, a: EscalateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let decision = match flat(
            actor
                .ask(Escalate {
                    task: a.task.clone(),
                    question: a.question,
                    options: a.options,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        ) {
            Ok(d) => d,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let answer = actor
            .ask(AwaitDecision { decision: decision.clone(), wait: escalate::wait_secs() })
            .await
            .ok()
            .flatten();
        Ok(match answer {
            Some(answer) => ToolOutput::ok(format!("decision: {answer}")),
            None => ToolOutput::ok(format!(
                "parked: task {} is blocked on `{decision}`; the answer re-wakes you. End your turn.",
                a.task
            )),
        })
    }
}

/// Arguments for [`DecideTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DecideArgs {
    /// The task the decision is about (from the wake).
    pub task: TaskId,
    /// The `decision:<id>` named in the task thread.
    pub decision: String,
    /// Your answer: one of the options, or your own words.
    pub answer: String,
}

/// `decide`: a manager answers an escalation (67 §11).
pub struct DecideTool;

#[async_trait::async_trait]
impl Tool for DecideTool {
    const NAME: &'static str = "decide";
    const RISK: Risk = Risk::Mutating;
    type Args = DecideArgs;

    fn description(&self) -> String {
        "Answer a decision a report escalated to you (a wake with reason `answer`; the question and its \
         `decision:<id>` are in the task thread). The answer goes into the thread, the blocker is \
         cleared and the asking agent is re-woken. If you cannot decide it either, `escalate` the same \
         task further instead."
            .into()
    }

    async fn call(&self, a: DecideArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        Ok(match flat(
            actor
                .ask(AnswerDecision {
                    decision: a.decision.clone(),
                    answer: a.answer,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        ) {
            Ok(()) => ToolOutput::ok(format!("decided `{}` on task {}; the owner is re-woken.", a.decision, a.task)),
            Err(e) => ToolOutput::err(e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_create_defaults_to_backlog_and_refuses_other_states() {
        assert_eq!(create_state(None), Ok(TaskState::Backlog));
        assert_eq!(create_state(Some(TaskState::Todo)), Ok(TaskState::Todo));
        assert!(create_state(Some(TaskState::Doing)).unwrap_err().contains("backlog or todo"));
        let a: TaskCreateArgs = serde_json::from_value(serde_json::json!({ "title": "t", "brief": "b" })).unwrap();
        assert_eq!(a.state, None, "the schema does not force a state; the default is backlog");
    }
}
