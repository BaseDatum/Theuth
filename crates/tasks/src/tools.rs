//! The task tools (67 §7, §8, §11), registered by `TasksPlugin::start` once
//! the tool registry is up: `task_create`, `task_inbox`, `task_checkout`,
//! `task_update`, `task_move`, `task_note`, `task_place`, `escalate`, `decide`.
//! Each is one message to the [`Tasks`] actor; the caller's identity comes from
//! the roster through [`CallerOf`].

use std::sync::Arc;

use agent::attach::AttachRef;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::{Context, Message};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::packet::{self, Check, Item, PacketOption, Pointer, Recommendation};
use crate::place;
use crate::thread::Thread;
use crate::types::{
    Disposition, DoneBecause, Owner, Priority, TaskCreate, TaskFilter, TaskId, TaskPatch,
    TaskState,
};
use crate::{
    AnswerDecision, AwaitDecision, CheckBoard, CheckoutTask, CommentTask, CreateTask,
    EffectiveReviewer, Escalate, GetTask, ListTasks, MoveTask, NoteDisposition, PlaceTask,
    ReassignTask, ReviewPacket, Tasks, UpdateTask, WriteBudgetOf, escalate,
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
with pieces, \"we should…\", \"have ops…\", a plan), do not create tasks straight away: call `task_draft()` with \
the proposed tasks and wait for the user's approval, which arrives as their next message (`[app task-drafts …]` \
naming a draft id and a choice). Then `task_create()` with `state: backlog` for \"add to backlog\", `state: todo` \
for \"start now\" (the owner is woken and the work placed), nothing for \"discard\". Every task carries \
`done_when`, its acceptance criteria: take them from the user when stated, otherwise draft one to three \
checkable lines from the brief. A task created in `backlog` \
runs nothing until it is started. When the user asks to start, pull back, drop, approve or send back a task, \
use `task_move()` (it refuses illegal moves and lists what is allowed from the task's state); a move to `done` \
says why (`done_because`: criteria met, intent met, or follow-ups filed). Your own work on a \
task is `task_checkout()`; finishing it is `task_update()` with a disposition; handing it to the agent whose \
capabilities fit it better is `task_reassign()`.";

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
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()?;
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
    /// Things the task is about, as references the owner reads with the per-kind tools (design 66):
    /// `{kind: note|knowledge|artifact|session|meeting, id, title}`. Carry over the `[attached …]` lines of the
    /// user's message when the task is drafted from it.
    #[serde(default)]
    pub attachments: Vec<AttachRef>,
    /// `urgent` | `high` | `medium` | `low` (default medium). The user's to set — only when the user said so;
    /// an agent's call with it is refused.
    #[serde(default)]
    pub priority: Option<Priority>,
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
            attachments: a.attachments,
            priority: a.priority,
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
                t.reviewer
                    .as_ref()
                    .map_or("none".to_owned(), ToString::to_string),
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
            state: Some(
                a.state
                    .unwrap_or_else(|| vec![TaskState::Todo, TaskState::Doing, TaskState::Blocked]),
            ),
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
        for t in tasks
            .iter()
            .filter(|t| t.next_wake_after_ms.is_none_or(|w| w <= now))
        {
            if group.as_ref() != Some(&t.project) {
                group = Some(t.project.clone());
                text.push_str(&format!(
                    "## {}\n",
                    t.project.as_deref().unwrap_or("(no project)")
                ));
            }
            let place = match (&t.where_, &t.harness) {
                (None, None) => "unplaced".to_owned(),
                (w, h) => format!(
                    "{}/{}",
                    w.as_deref().unwrap_or("?"),
                    h.as_deref().unwrap_or("?")
                ),
            };
            text.push_str(&format!(
                "- {} [{}] {} — {}{}{} — moved {}\n",
                t.id,
                t.state.as_str(),
                t.title,
                place,
                if t.blockers.is_empty() {
                    String::new()
                } else {
                    format!(" — blocked on {}", t.blockers.join(", "))
                },
                if t.attachments.is_empty() {
                    String::new()
                } else {
                    format!(
                        " — attached: {}",
                        t.attachments
                            .iter()
                            .map(crate::ledger::attachment_label)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
                chrono::DateTime::from_timestamp_millis(t.updated_ms as i64)
                    .map(|d| d
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string())
                    .unwrap_or_default()
            ));
        }
        if text.is_empty() {
            text.push_str("inbox empty");
        }
        // 99 D6: a capped caller sees where it stands; uncapped callers and 0 see nothing.
        if let Ok(Some(b)) = actor.ask(WriteBudgetOf(cx.session.to_string())).await
            && b.used > 0
        {
            let when = if b.window_secs == 0 {
                "for this session's lifetime".to_owned()
            } else {
                format!("in the last {}", crate::ledger::window_text(b.window_secs))
            };
            text.push_str(&format!(
                "\n_cross-task writes: {}/{} {when}_",
                b.used, b.cap
            ));
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
         session holds it or you are not the owner. Returns the full record: brief, attachments as \
         `[attached <kind> <id> \"<title>\"]` lines (read them with the tool for their kind), thread (comments, \
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
        if !t.attachments.is_empty() {
            text.push_str("\n\n--- attachments ---\n");
            for r in &t.attachments {
                text.push_str(&agent::attach::marker(r));
                text.push('\n');
            }
        }
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
    /// Attached references, replaced whole (pass the current list plus or minus the change).
    #[serde(default)]
    pub attachments: Option<Vec<AttachRef>>,
    /// With `disposition: done`: why — `criteria_met` (against `done_when`), `intent_met` (the user's
    /// evident intention), or `follow_ups_filed {tasks}` (the work continues in those tasks).
    #[serde(default)]
    pub done_because: Option<DoneBecause>,
    /// `urgent` | `high` | `medium` | `low`. The user's to set: pass it only when the user said so; otherwise
    /// say why it should change in `task_note` and the user decides.
    #[serde(default)]
    pub priority: Option<Priority>,
    /// With `disposition: review`: the review packet's one-paragraph summary (≤ 16 KiB). Required when the
    /// reviewer is the user.
    #[serde(default)]
    pub summary: Option<String>,
    /// With `disposition: review`: the `doc` id of the review mini-board (`doc_create(kind="board")`, sections
    /// `## What was done` · `## Why it matters to you` · `## The picture` · `## Against the criteria` ·
    /// `## Walk through`). Required when the reviewer is the user.
    #[serde(default)]
    pub board: Option<String>,
    /// With `disposition: review`: exactly what to review (≤ 12), evidence inline:
    /// `code` = `ref: "path#L10-L24"` + `text` the lines verbatim (+ `root`); `diff` = `text` a unified diff;
    /// `artifact` / `note` / `knowledge` / `session` / `meeting` / `task` by id; `url` by `http(s)` URL.
    /// `title` and a one-line `note` each; `text` ≤ 32 KiB. Optional — the board's Excerpts are the primary evidence.
    #[serde(default)]
    pub pointers: Option<Vec<Pointer>>,
    /// With `disposition: review`: your status against each `done_when` line (`{text, done, note}`).
    #[serde(default)]
    pub checks: Option<Vec<Check>>,
    /// With `disposition: review`: the next moves after approval (≤ 4, labels ≤ 80 chars) — e.g. "Merge and
    /// continue to X" (recommended) / "Revise Y first" / "Park it". Never what the run already did; a thing read
    /// or a question answered ends with none. The reviewer's pick is written to the thread as `**next** (n) <label>`.
    #[serde(default)]
    pub options: Vec<OptionArg>,
    /// With `disposition: review`: index into `options` of the move you would make.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// With `disposition: review`: one honest line (≤ 200 chars) on the cost of approving wrongly; optional.
    #[serde(default)]
    pub if_wrong: Option<String>,
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
         only as `criteria_met`). A hand-over to review carries the review packet the reviewer reads instead \
         of the thread: `summary` (one short paragraph: the abstract the inbox row shows), `board` — the id of \
         a review mini-board you wrote for the reviewer (`doc_create(kind=\"board\", root=…)` then `doc_insert`, \
         see the `board` skill) with the sections `## What was done`, `## Why it matters to you`, `## The picture` \
         (one diagram, `ref` on every unit), `## Against the criteria` (each `done_when` line with its evidence \
         anchor), `## Walk through` (Excerpts in reading order), optionally `## Open questions`; write for the \
         person reviewing — what, why it matters to them, the whole picture; diagram and link heavy; short — \
         `checks` (your status against each `done_when` line) and optional `pointers` with the evidence inline \
         (`code` = `path#L10-L24` plus the lines verbatim, `diff` = a unified diff, artifacts / notes / sessions / \
         URLs by id or URL; a one-line `note` each). When the reviewer is the user the `summary` and the `board` \
         are required (the board must exist, be a board and open with those sections) and the tool refuses \
         without them, naming what to fix. Caps: summary 16 KiB, 12 pointers (text 32 KiB each). `state: doing` \
         sends a reviewed task back to its owner; `state: cancelled` drops it. A write by a session that does \
         not hold the checkout is refused once the checkout moved on (`superseded`). One session may write to \
         at most 20 tasks it does not own. Every change is logged to the thread with your session and \
         before/after."
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
            attachments: a.attachments,
            done_because: a.done_because,
            priority: a.priority,
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
        let mut packet = None;
        if let Some(d) = a.disposition {
            let current = match flat(actor.ask(GetTask(a.id.clone())).await) {
                Ok(t) => t,
                Err(e) => return Ok(ToolOutput::err(e)),
            };
            let mut blockers = patch.blockers.take().unwrap_or(current.blockers);
            match d {
                Disposition::Done => patch.state = Some(TaskState::Done),
                Disposition::Review { reviewer } => {
                    packet = match review_gate(
                        &actor,
                        &a.id,
                        Some(reviewer.clone()),
                        a.summary,
                        a.board,
                        a.pointers,
                        a.checks,
                        a.options,
                        a.recommended,
                        a.if_wrong,
                        &me,
                    )
                    .await
                    {
                        Ok(p) => p,
                        Err(e) => return Ok(ToolOutput::err(e)),
                    };
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
            && let Some(p) = packet
            && let Err(e) = flat(actor.ask(p).await)
        {
            return Ok(ToolOutput::err(format!(
                "task {} is in review but its packet was not stored: {e}",
                a.id
            )));
        }
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
    /// `backlog` (pull a queued task back), `review` (owner hands over), `done` (reviewer approves),
    /// `cancelled` (drop).
    pub state: TaskState,
    /// Why, for the thread (optional).
    #[serde(default)]
    pub note: Option<String>,
    /// With `state: done`: why — `criteria_met`, `intent_met`, or `follow_ups_filed {tasks}`. Required
    /// from an agent; the user's approval defaults to `intent_met`.
    #[serde(default)]
    pub done_because: Option<DoneBecause>,
    /// With `state: review`: the review packet's one-paragraph summary (≤ 16 KiB). Required when the reviewer is the user.
    #[serde(default)]
    pub summary: Option<String>,
    /// With `state: review`: the `doc` id of the review mini-board (see `task_update`). Required when the reviewer is the user.
    #[serde(default)]
    pub board: Option<String>,
    /// With `state: review`: exactly what to review (≤ 12), evidence inline (see `task_update`); optional.
    #[serde(default)]
    pub pointers: Option<Vec<Pointer>>,
    /// With `state: review`: your status against each `done_when` line.
    #[serde(default)]
    pub checks: Option<Vec<Check>>,
    /// With `state: done` on a review that offered next moves: the index of the move the reviewer picks.
    /// Written to the thread as `**next** (n) <label>`; the owner is woken with reason `answer`.
    #[serde(default)]
    pub next: Option<u32>,
    /// With `state: review`: the next moves after approval (≤ 4; see `task_update`).
    #[serde(default)]
    pub options: Vec<OptionArg>,
    /// With `state: review`: index into `options` of the move you would make.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// With `state: review`: one honest line on the cost of approving wrongly; optional.
    #[serde(default)]
    pub if_wrong: Option<String>,
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
         blocker its task), doing → review (owner). `todo` from backlog wakes the owner. Illegal moves are refused \
         with the list of states allowed from where the task is. A move to `review` whose reviewer is the user \
         needs the review packet — `summary` (one paragraph) and `board`, the id of a review mini-board \
         (`doc_create(kind=\"board\")`, sections `## What was done`, `## Why it matters to you`, `## The picture`, \
         `## Against the criteria`, `## Walk through`; write for the person reviewing — what, why it matters to \
         them, the whole picture; diagram and link heavy; short), plus `checks` against `done_when` — and is \
         refused without it. Working a task yourself is `task_checkout`; \
         finishing one is `task_update` with a disposition."
            .into()
    }

    async fn call(&self, a: TaskMoveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = actor()?;
        let me = caller(&actor, &cx).await;
        let packet = if a.state == TaskState::Review {
            match review_gate(
                &actor,
                &a.task,
                None,
                a.summary,
                a.board,
                a.pointers,
                a.checks,
                a.options,
                a.recommended,
                a.if_wrong,
                &me,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return Ok(ToolOutput::err(e)),
            }
        } else {
            None
        };
        let r = flat(
            actor
                .ask(MoveTask {
                    id: a.task,
                    state: a.state,
                    note: a.note,
                    done_because: a.done_because,
                    next: a.next,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        );
        Ok(match r {
            Ok(t) => {
                if let Some(p) = packet
                    && let Err(e) = flat(actor.ask(p).await)
                {
                    return Ok(ToolOutput::err(format!(
                        "task {} is in review but its packet was not stored: {e}",
                        t.id
                    )));
                }
                ToolOutput::ok(format!(
                    "task {} is now {} (owner {})",
                    t.id,
                    t.state.as_str(),
                    t.owner
                ))
            }
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// 101 D1/R1/R2, before any write: who reviews `id` if it goes to review now;
/// the user needs a `summary` and a `board` with the review skeleton, every
/// audience gets the caps checked. `Ok(Some(_))` is the packet to store once
/// the task is in review; `Ok(None)` = an agent audience with nothing given.
// Every argument is a distinct per-call input of the two `disposition: review` call sites; a struct would only be built there.
#[allow(clippy::too_many_arguments)]
async fn review_gate(
    actor: &ActorRef<Tasks>,
    id: &TaskId,
    reviewer: Option<Owner>,
    summary: Option<String>,
    board: Option<String>,
    pointers: Option<Vec<Pointer>>,
    checks: Option<Vec<Check>>,
    options: Vec<OptionArg>,
    recommended: Option<u32>,
    if_wrong: Option<String>,
    me: &str,
) -> Result<Option<ReviewPacket>, String> {
    let who = flat(
        actor
            .ask(EffectiveReviewer {
                id: id.clone(),
                reviewer,
            })
            .await,
    )?;
    let given = summary.is_some()
        || board.is_some()
        || pointers.is_some()
        || checks.is_some()
        || !options.is_empty();
    let p = packet::Packet {
        key: packet::review_key(id),
        task: id.clone(),
        kind: packet::PacketKind::Review,
        by: me.to_owned(),
        at_ms: 0,
        question: None,
        summary: summary.unwrap_or_default(),
        board,
        options: options.into_iter().map(Into::into).collect(),
        recommended,
        pointers: pointers.unwrap_or_default(),
        checks: checks.unwrap_or_default(),
        recommendation: None,
        items: vec![],
        to: None,
        if_wrong: if_wrong.map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()),
    };
    if who == Owner::User {
        packet::require_for_user(&p)?;
        flat(
            actor
                .ask(CheckBoard {
                    kind: p.kind,
                    board: p.board.clone().unwrap_or_default(),
                })
                .await,
        )?;
    }
    packet::validate(&p)?;
    Ok((who == Owner::User || given).then_some(ReviewPacket {
        id: id.clone(),
        summary: p.summary,
        board: p.board,
        pointers: p.pointers,
        checks: p.checks,
        options: p.options,
        recommended: p.recommended,
        if_wrong: p.if_wrong,
        actor: me.to_owned(),
    }))
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
            Ok(t) => ToolOutput::ok(format!(
                "task {} reassigned to {} ({})",
                t.id,
                t.owner,
                t.state.as_str()
            )),
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
            Ok(()) => ToolOutput::ok(if a.progress {
                "progress noted"
            } else {
                "comment added"
            }),
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
#[derive(Clone, Debug, kameo::Reply)]
pub struct PlaceContextReply {
    /// theuth's desk session, when open.
    pub desk: Option<String>,
    /// The owner's `tools` ceiling is `read_only`.
    pub read_only: bool,
    /// `tasks.place_prefer`.
    pub prefer: Vec<String>,
    /// The owner's pin (79 §4.9): `Err(blocker)` when pinned but not placeable now.
    pub pin: Result<Option<place::Pin>, String>,
}

impl Default for PlaceContextReply {
    fn default() -> Self {
        Self {
            desk: None,
            read_only: false,
            prefer: Vec::new(),
            pin: Ok(None),
        }
    }
}

impl Message<PlaceContext> for Tasks {
    type Reply = PlaceContextReply;

    async fn handle(&mut self, m: PlaceContext, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let router = self.ledger.router.as_ref();
        let pin = match router {
            Some(r) => r.pin_of(&m.owner).await,
            None => Ok(None),
        };
        PlaceContextReply {
            desk: router.and_then(|r| r.desk_for("theuth")),
            read_only: router
                .and_then(|r| r.tools_of(&m.owner))
                .is_some_and(|t| t == "read_only"),
            prefer: self.ledger.cfg.place_prefer.clone(),
            pin,
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
        .chain(
            status
                .peers
                .into_iter()
                .filter(|p| p.reachable)
                .map(|p| (p.name, p.id)),
        )
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
    let Some(agents) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
    else {
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
async fn ask_desk(
    desk: Option<String>,
    from: runtime::SessionId,
    task: &TaskId,
    prompt: String,
) -> Result<String, String> {
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
    let admitted =
        tools::communication::current().ok_or("peer messaging is off in this session")?;
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
        Ok(Ok(r)) => r.reply.ok_or_else(|| {
            r.error
                .unwrap_or_else(|| "theuth's desk answered nothing".into())
        }),
        Ok(Err(_)) => Err("theuth's desk: wait dropped".into()),
        Err(_) => {
            let _ = agents.tell(agent::supervisor::DropWait(d.receipt.id)).await;
            Err(format!(
                "theuth's desk did not answer within {}s",
                DESK_WAIT.as_secs()
            ))
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
         `blocked: engine:<what is missing>`. An owner whose file says `runs_in: vm` or `vm:<name>` is pinned: the \
         task goes to that sandbox VM's engine or waits `blocked: sandbox:<name>` until it is ready — never elsewhere."
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
        let placed = match ctx.pin {
            Err(blocker) => Err(blocker),
            Ok(pin) => {
                let (engines, cwds) =
                    tokio::join!(engines(), project_cwds(task.project.as_deref()));
                let inputs = place::Inputs {
                    task: &task,
                    needs: a.needs.as_deref(),
                    read_only: ctx.read_only,
                    project_cwds: &cwds,
                    engines: &engines,
                    prefer: &ctx.prefer,
                    pin,
                };
                let desk = ctx.desk.clone();
                place::place(&inputs, |prompt| ask_desk(desk, cx.session, &a.id, prompt)).await
            }
        };
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
            Err(missing) if missing.starts_with("engine:") || missing.starts_with("sandbox:") => {
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
                let what = if missing.starts_with("sandbox:") {
                    "the owner is pinned to a sandbox VM that is not ready"
                } else {
                    "no engine satisfies the needs"
                };
                Ok(ToolOutput::err(match r {
                    Ok(_) => format!("{what}; task set blocked on `{missing}`"),
                    Err(e) => format!("{what} (`{missing}`); could not block the task: {e}"),
                }))
            }
            Err(e) => Ok(ToolOutput::err(e)),
        }
    }
}

/// One `escalate` option (101 D2): a bare label or `{label, description}`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OptionArg {
    /// Just the label.
    Label(String),
    /// Label with its one-line trade-off.
    Full {
        /// A few words.
        label: String,
        /// One line: the trade-off.
        #[serde(default)]
        description: Option<String>,
    },
}

impl From<OptionArg> for PacketOption {
    fn from(o: OptionArg) -> Self {
        match o {
            OptionArg::Label(label) => Self {
                label,
                description: None,
            },
            OptionArg::Full { label, description } => Self { label, description },
        }
    }
}

/// Arguments for [`EscalateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EscalateArgs {
    /// Task id the decision is about.
    pub task: TaskId,
    /// The question, one or two sentences; the decider sees it first. Forwarding a report's decision
    /// (the task is already blocked on their `decision:<id>`): pass `""` to inherit their question,
    /// summary, board, options, pointers and items — never re-type them.
    pub question: String,
    /// Markdown (≤ 16 KiB): the abstract — at most 120 words answering what is being decided and why now;
    /// the inbox row, the thread and the channel card show it, the detail goes on the `board`. Required when
    /// your manager is the user.
    #[serde(default)]
    pub summary: Option<String>,
    /// The `doc` id of the mini-board you wrote for the decider (`doc_create(kind="board")`, sections
    /// `## What you're deciding` · `## Why it matters to you` · `## The picture` · `## Options` · `## Look at`).
    /// Required when your manager is the user.
    #[serde(default)]
    pub board: Option<String>,
    /// Concrete options (at most 4 unless a bundle; cap 6, labels ≤ 80 chars): `"label"` or
    /// `{label, description}` with a one-line trade-off each; the decider may still answer freely.
    #[serde(default)]
    pub options: Vec<OptionArg>,
    /// Index into `options` of the one you would pick — exactly one.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// Exactly what to look at (≤ 12), evidence inline: `code` = `ref: "path#L10-L24"` + `text` the lines verbatim
    /// (+ `root`); `diff` = `text` a unified diff; `artifact` / `note` / `knowledge` / `session` / `meeting` /
    /// `task` by id; `url` by `http(s)` URL. `title` and a one-line `note` each; `text` ≤ 32 KiB.
    #[serde(default)]
    pub pointers: Vec<Pointer>,
    /// Forwarding a report's decision: your view in one or two sentences (`text`, ≤ 1 KiB) and the index of
    /// the option you back (`option`). With an empty `question` this is the whole call.
    #[serde(default)]
    pub recommendation: Option<Recommendation>,
    /// A bundle (≤ 20): N related sub-decisions, each `{id, title, description?, options?, recommended?}` —
    /// `id` unique, the slug of the item's `###` heading under the board's `## Options`; options as
    /// `{label, description}` (≤ 6). The decider answers all at once (`1. <title>: <pick>` per line).
    #[serde(default)]
    pub items: Vec<Item>,
    /// One honest line (≤ 200 chars) on the cost of a wrong answer — "low, one-line revert" or "high, real users
    /// get this email". Required when your manager is the user; inherited on a forward.
    #[serde(default)]
    pub if_wrong: Option<String>,
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
         escalates further; the user gets it on their attention surface and as a card on their channels \
         with the options as buttons. Prepare the packet the decider reads instead of the thread: \
         `summary` — the abstract, at most 120 words answering what is being decided and why now (the \
         inbox row and the card show it); `board` — the id of a mini-board you wrote for the decider \
         (`doc_create(kind=\"board\", root=…)` then `doc_insert`, see the `board` skill) with the sections \
         `## What you're deciding`, `## Why it matters to you`, `## The picture` (one diagram, `ref` on \
         every unit), `## Options` (one `###` per option: consequence, cost/risk, reversibility, \
         recommendation), `## Look at` (Excerpts), optionally `## Open questions`; write for the person \
         deciding — what, why it matters to them, the whole picture; diagram and link heavy; short. Then \
         `options`: at most 4 unless it is a bundle, one line each, exactly one `recommended`; optional \
         `pointers` with the evidence inline (`code` = `path#L10-L24` plus the lines verbatim, `diff` = a \
         unified diff, artifacts / notes / sessions / URLs by id or URL), a one-line `note` each. \
         Forwarding a report's decision (the task is already blocked on their `decision:<id>`): pass \
         `question: \"\"` and your `recommendation` {text, option}; their question, summary, board, options \
         and pointers are inherited — do not re-type the report into `question`. N related decisions \
         (records, hosts, PRs): one call with `items`, one `{id, title, options, recommended}` each, and \
         one `###` per item under the board's `## Options` whose heading slug is the item `id`. When your \
         manager is the user the `summary` and the `board` are required (the board must exist, be a board \
         and open with those sections) and the tool refuses without them, naming what to fix. Caps: summary \
         16 KiB, 6 options (labels 80 chars), 20 items, 12 pointers (text 32 KiB each). \
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
                    summary: a.summary,
                    board: a.board,
                    options: a.options.into_iter().map(Into::into).collect(),
                    recommended: a.recommended,
                    pointers: a.pointers,
                    recommendation: a.recommendation,
                    items: a.items,
                    if_wrong: a.if_wrong,
                    actor: me,
                    session: Some(cx.session.to_string()),
                })
                .await,
        ) {
            Ok(d) => d,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let answer = actor
            .ask(AwaitDecision {
                decision: decision.clone(),
                wait: escalate::wait_secs(),
            })
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
        Ok(
            match flat(
                actor
                    .ask(AnswerDecision {
                        decision: a.decision.clone(),
                        answer: a.answer,
                        actor: me,
                        session: Some(cx.session.to_string()),
                    })
                    .await,
            ) {
                Ok(()) => ToolOutput::ok(format!(
                    "decided `{}` on task {}; the owner is re-woken.",
                    a.decision, a.task
                )),
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_create_defaults_to_backlog_and_refuses_other_states() {
        assert_eq!(create_state(None), Ok(TaskState::Backlog));
        assert_eq!(create_state(Some(TaskState::Todo)), Ok(TaskState::Todo));
        assert!(
            create_state(Some(TaskState::Doing))
                .unwrap_err()
                .contains("backlog or todo")
        );
        let a: TaskCreateArgs =
            serde_json::from_value(serde_json::json!({ "title": "t", "brief": "b" })).unwrap();
        assert_eq!(
            a.state, None,
            "the schema does not force a state; the default is backlog"
        );
    }
}

#[cfg(test)]
mod review_gate_tests {
    use kameo::actor::Spawn;

    use super::*;
    use crate::ledger::tests::{board, rig};
    use crate::{GetPacket, ListPackets};

    fn code_pointer() -> Pointer {
        Pointer {
            kind: packet::PointerKind::Code,
            ref_: "src/x.rs#L1-L2".into(),
            title: "the fix".into(),
            note: None,
            text: Some("a\nb".into()),
            root: None,
        }
    }

    /// 101 D1/R1/R2 at the tool boundary: a hand-over whose effective reviewer is
    /// the user needs a summary and a board with the review skeleton (refused
    /// before any write); an agent reviewer needs nothing; the packet is stored
    /// (and replaced) once the task is in review; `GetPacket` answers by key.
    #[tokio::test]
    async fn review_gate_enforces_the_user_audience() {
        let mut r = rig().await;
        let ops = r.task("agent:ops").await; // ops reports to lead
        r.ledger
            .checkout(&ops.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let lead = r.task("agent:lead").await; // lead reports to the user
        r.ledger
            .checkout(&lead.id, "s-lead", "agent:lead")
            .await
            .unwrap();
        let actor = Tasks::spawn(Tasks {
            ledger: r.ledger,
            dispatching: false,
            escalations: Default::default(),
        });

        // task_move {state: review}: the effective reviewer decides.
        let e = review_gate(&actor, &lead.id, None, None, None, None, None, vec![], None, None, "agent:lead")
            .await
            .unwrap_err();
        assert!(e.contains("needs a `board`"), "{e}");
        assert_eq!(
            flat(actor.ask(GetTask(lead.id.clone())).await)
                .unwrap()
                .state,
            TaskState::Doing,
            "nothing written"
        );
        let e = review_gate(
            &actor,
            &lead.id,
            None,
            Some("done".into()),
            None,
            None,
            None,
            vec![],
            None,
            None,
            "agent:lead",
        )
        .await
        .unwrap_err();
        assert!(
            e.contains("needs a `board`") && e.contains("`## What was done`"),
            "summary alone is not enough: {e}"
        );
        assert!(
            review_gate(&actor, &ops.id, None, None, None, None, None, vec![], None, None, "agent:ops")
                .await
                .unwrap()
                .is_none(),
            "agent reviewer, nothing given: no packet"
        );
        // task_update {disposition: review {reviewer: user}} names the user outright.
        let e = review_gate(
            &actor,
            &ops.id,
            Some(Owner::User),
            None,
            None,
            None,
            None,
            vec![],
            None,
            None,
            "agent:ops",
        )
        .await
        .unwrap_err();
        assert!(e.contains("`board`"), "{e}");
        // caps apply to every audience
        let e = review_gate(
            &actor,
            &ops.id,
            None,
            Some("x".repeat(packet::SUMMARY_MAX + 1)),
            None,
            None,
            None,
            vec![],
            None,
            None,
            "agent:ops",
        )
        .await
        .unwrap_err();
        assert!(e.contains("`summary`") && e.contains("16 KiB"), "{e}");
        // R2 through the actor: the board must exist, be a board, and open with the review skeleton
        board(&r.thread, "rb-bad", &["What was done", "The picture"]);
        for (b, want) in [
            ("nope", "board `nope` does not exist"),
            (
                "rb-bad",
                "board `rb-bad`: section 2 of a review board must be `## Why it matters to you`",
            ),
        ] {
            let e = review_gate(
                &actor,
                &lead.id,
                None,
                Some("done".into()),
                Some(b.into()),
                None,
                None,
                vec![],
                None,
                None,
                "agent:lead",
            )
            .await
            .unwrap_err();
            assert!(e.starts_with(want), "{e}");
        }
        assert_eq!(
            flat(actor.ask(GetTask(lead.id.clone())).await)
                .unwrap()
                .state,
            TaskState::Doing,
            "nothing written"
        );
        board(&r.thread, "rb", &packet::REVIEW_SECTIONS);

        // the happy path: gate, move, store; then replace
        let key = packet::review_key(&lead.id);
        for summary in ["first", "second"] {
            let rp = review_gate(
                &actor,
                &lead.id,
                None,
                Some(summary.into()),
                Some("rb".into()),
                Some(vec![code_pointer()]),
                None,
                vec![],
                None,
                None,
                "agent:lead",
            )
            .await
            .unwrap()
            .expect("a packet for the user");
            assert_eq!(rp.board.as_deref(), Some("rb"));
            if summary == "first" {
                let t = flat(
                    actor
                        .ask(MoveTask {
                            id: lead.id.clone(),
                            state: TaskState::Review,
                            note: None,
                            done_because: None,
                            next: None,
                            actor: "agent:lead".into(),
                            session: Some("s-lead".into()),
                        })
                        .await,
                )
                .unwrap();
                assert_eq!(
                    (t.state, t.reviewer),
                    (TaskState::Review, Some(Owner::User))
                );
            }
            flat(actor.ask(rp).await).unwrap();
            let p = flat(actor.ask(GetPacket(key.clone())).await)
                .unwrap()
                .expect("stored");
            assert_eq!(
                (p.summary.as_str(), p.board.as_deref(), p.pointers.len()),
                (summary, Some("rb"), 1)
            );
        }
        assert_eq!(
            flat(
                actor
                    .ask(ListPackets(vec![lead.id.clone(), ops.id.clone()]))
                    .await
            )
            .unwrap()
            .len(),
            1,
            "replaced, one row"
        );
        assert_eq!(
            flat(actor.ask(GetPacket("review:nope".into())).await).unwrap(),
            None
        );
        let doc = lead.thread.clone().unwrap();
        assert_eq!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .filter(|b| b.contains("**review requested**"))
                .count(),
            2
        );
    }

    /// 101 D2: `options` takes bare labels and `{label, description}`; the schema still generates.
    #[test]
    fn escalate_options_take_both_shapes() {
        let a: EscalateArgs = serde_json::from_value(serde_json::json!({
            "task": "t", "question": "q",
            "options": ["yes", {"label": "no", "description": "later"}],
            "recommended": 1
        }))
        .unwrap();
        let opts: Vec<PacketOption> = a.options.into_iter().map(Into::into).collect();
        assert_eq!(opts[0].label, "yes");
        assert_eq!(opts[0].description, None);
        assert_eq!(opts[1].description.as_deref(), Some("later"));
        assert!(a.pointers.is_empty() && a.summary.is_none());
        assert!(a.recommendation.is_none() && a.items.is_empty());
        let schema = serde_json::to_value(schemars::schema_for!(EscalateArgs)).unwrap();
        assert!(schema.to_string().contains("description"));
        // 103: a forward and a bundle parse; an item needs only `id` and `title`
        let a: EscalateArgs = serde_json::from_value(serde_json::json!({
            "task": "t", "question": "",
            "recommendation": {"text": "approve"},
            "items": [{"id": "a-rec", "title": "A record", "options": [{"label": "add"}], "recommended": 0},
                      {"id": "mx", "title": "MX"}]
        }))
        .unwrap();
        assert_eq!(a.recommendation.unwrap().option, None);
        assert_eq!(a.items[0].options[0].label, "add");
        assert_eq!(
            (a.items[1].recommended, a.items[1].options.len()),
            (None, 0)
        );
    }
}
