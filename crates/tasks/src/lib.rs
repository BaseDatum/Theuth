//! Tasks plugin (design doc 67 §7): the task record, its states, wakes with
//! coalescing and no-progress back-off, checkout and disposition, the
//! field-level event log. Schedules that name an agent create tasks through
//! [`schedule::TaskCreator`] (67 §7.4); wakes reach an agent's desk through a
//! [`DeskRouter`] the roster registers.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`TASKS_ACTOR`] actor ([`Tasks`]): [`ListTasks`],
//!   [`GetTask`], [`TaskBlocks`], [`CreateTask`], [`UpdateTask`], [`MoveTask`], [`CommentTask`],
//!   [`TaskEvents`], [`CheckoutTask`], [`PlaceTask`], [`Escalate`],
//!   [`AwaitDecision`], [`AnswerDecision`], [`Mention`], [`MarkAnswered`],
//!   [`NoteDisposition`], [`EpisodeClosing`], [`FinishWake`], [`ReleaseClaims`],
//!   [`SetDispatching`], [`RegisterDeskRouter`]
//! - trait [`DeskRouter`] (the roster implements it)
//!
//! The rules live in [`ledger`]; this file is the actor around them.

pub mod activity;
pub mod config;
pub mod escalate;
pub mod ledger;
pub mod place;
pub mod store;
pub mod thread;
pub mod tools;
pub mod types;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub use config::TasksConfig;
pub use types::{
    Checkout, Disposition, DoneBecause, Owner, Task, TaskCreate, TaskEvent, TaskFilter, TaskId,
    TaskPatch, TaskState, Wake, WakeReason,
};

/// Registry name of the [`Tasks`] actor.
pub const TASKS_ACTOR: &str = "tasks";

/// Sqlite file under `data_dir` (67 §7).
pub const DB: &str = "tasks.sqlite";

/// Wake dispatch cadence (67 §7.3; a hub duty, 67 §4.4).
pub const DISPATCH_EVERY: Duration = Duration::from_secs(5);
/// Orphan sweep cadence (67 §7.5).
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// How a wake reaches an agent: the roster owns desks, the tasks plugin owns
/// wakes, so the roster registers one of these with [`RegisterDeskRouter`].
/// The three defaulted methods are the roster's directory as tasks needs it;
/// their defaults (no manager, no agent, no names) make every manager the
/// user and every session the user's.
#[async_trait::async_trait]
pub trait DeskRouter: Send + Sync {
    /// The desk session of `agent` (name without `agent:`), if it has one open.
    fn desk_for(&self, agent: &str) -> Option<String>;
    /// Deliver `wake` (with its task, when it names one) to `agent`'s desk as
    /// one input; opens the desk when needed. The desk reads the thread
    /// (`task.thread`) for what is new.
    async fn deliver(&self, agent: &str, wake: &Wake, task: Option<&Task>) -> anyhow::Result<()>;
    /// `agent`'s `reports_to` as an agent name; `None` = the user (67 §11).
    fn manager_of(&self, agent: &str) -> Option<String> {
        let _ = agent;
        None
    }
    /// The agent whose desk or work session `session` is; `None` = the user's own.
    fn agent_for(&self, session: &str) -> Option<String> {
        let _ = session;
        None
    }
    /// Every agent name, for `@`-less mentions in comments (67 §7.3).
    fn agents(&self) -> Vec<String> {
        Vec::new()
    }
    /// `agent`'s `tools` ceiling from its file (`read_only` | `mutating` |
    /// `destructive`); `None` when unknown. Placement (67 §8): a `read_only`
    /// agent's tasks run on the native harness, which enforces it.
    fn tools_of(&self, agent: &str) -> Option<String> {
        let _ = agent;
        None
    }
}

/// `ask` → `()`: the roster hands over its router once it is up. Replaces any earlier one.
pub struct RegisterDeskRouter(pub Arc<dyn DeskRouter>);

/// `ask` → `Result<Vec<Task>, String>`: tasks matching `filter`, by project then oldest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListTasks(pub TaskFilter);

/// `ask` → `Result<Task, String>` with `blocks` filled. `Err("no such task")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetTask(pub TaskId);

/// `ask` → `Result<Vec<TaskId>, String>`: tasks whose `blockers` name this one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskBlocks(pub TaskId);

/// `ask` → `Result<Task, String>`: create, log, wake the owner (`assigned`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateTask {
    /// The input.
    pub input: TaskCreate,
    /// `agent:<name>` | `user` | `system`: who is creating.
    pub created_by: String,
    /// Session doing it, for the event log and the cross-task write cap.
    pub session: Option<String>,
}

/// `ask` → `Result<Task, String>`: apply `patch`, guard transitions, log, wake
/// whoever the change concerns. `Err("no such task" | "conflict: …" |
/// "done needs a reviewer…" | "tasks.cross_task_write_cap…" | "superseded: …" |
/// "blockers would form a cycle: …")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateTask {
    /// Which.
    pub id: TaskId,
    /// What.
    pub patch: TaskPatch,
    /// `agent:<name>` | `user` | `system`.
    pub actor: String,
    /// Session doing it.
    pub session: Option<String>,
}

/// `ask` → `Result<Task, String>`: `task_move` — a state change on the caller's
/// intent (backlog ↔ todo, → cancelled, review → done | todo, blocked → todo …),
/// guarded like [`UpdateTask`]; `Err` names the allowed targets. `note` is
/// appended to the thread as a comment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveTask {
    /// Which.
    pub id: TaskId,
    /// Where to.
    pub state: TaskState,
    /// Why, for the thread.
    pub note: Option<String>,
    /// The verdict a move to `done` records (70 DESK-PROTOCOL); `None` from an agent is refused.
    #[serde(default)]
    pub done_because: Option<DoneBecause>,
    /// `agent:<name>` | `user` | `system`.
    pub actor: String,
    /// Session doing it.
    pub session: Option<String>,
}

/// `ask` → `Result<Task, String>`: `task_reassign` (70 DESK-PROTOCOL 1) — the
/// owner hands the task to `to` with a reason; the thread says
/// `reassigned to <to>: <why>` and `to` is woken `reassigned`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReassignTask {
    /// Which.
    pub id: TaskId,
    /// New owner.
    pub to: Owner,
    /// The fit: why `to` is the one.
    pub why: String,
    /// `agent:<name>` | `user` | `system`.
    pub actor: String,
    /// Session doing it.
    pub session: Option<String>,
}

/// `ask` → `Result<(), String>`: append to the thread; a comment wakes the
/// owner (`comment`), a progress note wakes nobody and refreshes the checkout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommentTask {
    /// Which.
    pub id: TaskId,
    /// Markdown.
    pub text: String,
    /// `true` = progress note (67 §7.5).
    pub progress: bool,
    /// `agent:<name>` | `user` | `system`.
    pub actor: String,
    /// Session doing it.
    pub session: Option<String>,
}

/// `ask` → `Result<Vec<TaskEvent>, String>`: the field-level log, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskEvents(pub TaskId);

/// `ask` → `Result<Task, String>`: `task_checkout` (67 §7.2, §7.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckoutTask {
    /// Which.
    pub id: TaskId,
    /// The work session taking it.
    pub session: String,
    /// `agent:<name>` | `user`.
    pub actor: String,
}

/// `ask` → `Result<Task, String>`: `task_place` — record `where`/`harness` and the reason (67 §8).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaceTask {
    /// Which.
    pub id: TaskId,
    /// Engine.
    pub where_: Option<String>,
    /// Harness.
    pub harness: Option<String>,
    /// One line, into the thread.
    pub reason: String,
    /// Who.
    pub actor: String,
    /// Session.
    pub session: Option<String>,
}

/// `ask` → `Result<String, String>`: `escalate` (67 §11); the `decision:<id>` blocker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Escalate {
    /// Which.
    pub task: TaskId,
    /// The question.
    pub question: String,
    /// Options.
    pub options: Vec<String>,
    /// Who.
    pub actor: String,
    /// Session.
    pub session: Option<String>,
}

/// `ask` → `Result<(), String>`: a chat message into the task's thread
/// (67 §4.6) — a comment by `actor` and a `mention` wake for the owner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mention {
    /// Which.
    pub id: TaskId,
    /// The message.
    pub text: String,
    /// Who (`user` for a chat).
    pub actor: String,
    /// Session, when one.
    pub session: Option<String>,
}

/// `ask` → `Result<bool, String>`: mark chat message `(sender, timestamp)`
/// answered (67 §4.7); `true` when this engine was first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkAnswered {
    /// Channel-side sender key.
    pub sender: String,
    /// Channel-side message timestamp.
    pub timestamp: u64,
}

/// `ask` → `Result<(), String>`: `decide` (67 §11) — answer the
/// `decision:<id>` some task is blocked on; the owner is re-woken.
/// `Err("no task is blocked on …")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerDecision {
    /// `decision:<id>`.
    pub decision: String,
    /// The answer.
    pub answer: String,
    /// Who.
    pub actor: String,
    /// Session.
    pub session: Option<String>,
}

/// `ask` → `Option<String>`: the answer to `decision` within `wait`, else
/// `None` (the task stays parked; the answer re-wakes the owner).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AwaitDecision {
    /// `decision:<id>`.
    pub decision: String,
    /// Live wait.
    pub wait: Duration,
}

/// `ask` → `Result<(), String>`: a disposition that changes no field
/// (`continuing:<by>`, `delegated:<child>`), recorded for [`EpisodeClosing`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteDisposition {
    /// Which.
    pub id: TaskId,
    /// `continuing:<by>` | `delegated:<child>`.
    pub text: String,
    /// Who.
    pub actor: String,
    /// Session.
    pub session: Option<String>,
}

/// `ask` → `Result<Vec<TaskId>, String>`: the roster is about to close
/// `agent`'s desk `session` (67 §7.3 last rule): its claimed wakes are
/// released for the next episode; every `doing` task checked out by it or
/// its children with no `continuing` disposition is `blocked: no
/// continuation` and the manager is woken. Replies with the tasks it blocked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpisodeClosing {
    /// Agent name.
    pub agent: String,
    /// Desk session id.
    pub session: String,
}

/// `tell`: the desk turn that took this wake is over — the roster judged it
/// handled, or blocked the task (STALE-WAKE-DESKS). The row is deleted
/// whether or not the agent wrote the task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinishWake(pub Wake);

/// `tell`: a desk (re)opened on this session (STALE-WAKE-DESKS: desks are
/// reused across restarts); wakes still claimed by it belong to a turn that
/// no longer runs and are released for dispatch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseClaims(pub String);

/// `ask` → `Result<Task, String>`: the 67 §7.3 rule on one task (70
/// DESK-PROTOCOL: a desk turn nudged once and still not moving it) —
/// `blocked: no continuation`, `why` in the thread, the agent's manager woken.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoContinuation {
    /// Which.
    pub id: TaskId,
    /// Whose desk failed to move it.
    pub agent: String,
    /// The thread line.
    pub why: String,
}

/// `ask` → `()`: run (or stop running) the hub duties — wake dispatch and
/// the orphan sweep — on this engine. Default on; the mesh lease flips it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetDispatching(pub bool);

/// `ask` → `Result<String, String>`: [`schedule::TaskCreator`] behind the actor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateForSchedule {
    /// Schedule name.
    pub schedule: String,
    /// Fire time.
    pub fire_ms: u64,
    /// Owner agent.
    pub agent: String,
    /// Brief.
    pub prompt: String,
    /// Project label.
    pub project: Option<String>,
}

/// `ask` → `Result<(), String>`: the schedule's `until` passed; its open tasks are done (67 §7.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloseForSchedule {
    /// Schedule name.
    pub schedule: String,
    /// Why.
    pub why: String,
}

/// Internal `tell`: one dispatch pass.
#[derive(Clone, Copy, Debug)]
struct Dispatch;

/// Internal `tell`: one orphan sweep.
#[derive(Clone, Copy, Debug)]
struct Sweep;

/// Internal `tell`: a delivery returned.
struct Delivered {
    wake: Wake,
    ok: bool,
}

/// Internal `tell`: the mesh told us who we are.
struct SetEngine(String);

/// The [`TASKS_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Tasks {
    ledger: ledger::Ledger,
    dispatching: bool,
    escalations: escalate::Escalations,
}

impl Tasks {
    /// The connected engine set from the mesh; `None` when the mesh is not up
    /// (the sweep then triggers on staleness alone).
    async fn reachable() -> Option<HashSet<String>> {
        let status = mesh::actor()?.ask(mesh::Status).await.ok()?;
        let mut set: HashSet<String> = HashSet::from([status.self_id.0]);
        for p in status.peers.into_iter().filter(|p| p.reachable) {
            set.insert(p.name);
            set.insert(p.id.0);
        }
        Some(set)
    }
}

impl Message<RegisterDeskRouter> for Tasks {
    type Reply = ();

    async fn handle(
        &mut self,
        m: RegisterDeskRouter,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger.router = Some(m.0);
    }
}

impl Message<SetEngine> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: SetEngine, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        store::set_hlc_origin(&m.0);
        self.ledger.engine = m.0;
    }
}

impl Message<SetDispatching> for Tasks {
    type Reply = ();

    async fn handle(
        &mut self,
        m: SetDispatching,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.dispatching = m.0;
    }
}

impl Message<ListTasks> for Tasks {
    type Reply = Result<Vec<Task>, String>;

    async fn handle(&mut self, m: ListTasks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.list(&m.0).await
    }
}

impl Message<GetTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: GetTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.get(&m.0).await
    }
}

impl Message<TaskBlocks> for Tasks {
    type Reply = Result<Vec<TaskId>, String>;

    async fn handle(&mut self, m: TaskBlocks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.blocks_of(&m.0).await
    }
}

impl Message<CreateTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: CreateTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .create(
                m.input,
                &m.created_by,
                m.session.as_deref(),
                WakeReason::Assigned,
            )
            .await
    }
}

impl Message<UpdateTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: UpdateTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .update(&m.id, m.patch, &m.actor, m.session.as_deref())
            .await
    }
}

impl Message<MoveTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: MoveTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .move_to(
                &m.id,
                m.state,
                m.note.as_deref(),
                m.done_because,
                &m.actor,
                m.session.as_deref(),
            )
            .await
    }
}

impl Message<ReassignTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: ReassignTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .reassign(&m.id, m.to, &m.why, &m.actor, m.session.as_deref())
            .await
    }
}

impl Message<CommentTask> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CommentTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .comment(&m.id, &m.text, m.progress, &m.actor, m.session.as_deref())
            .await
    }
}

impl Message<TaskEvents> for Tasks {
    type Reply = Result<Vec<TaskEvent>, String>;

    async fn handle(&mut self, m: TaskEvents, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.events(&m.0).await
    }
}

impl Message<CheckoutTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: CheckoutTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.checkout(&m.id, &m.session, &m.actor).await
    }
}

impl Message<PlaceTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: PlaceTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger
            .place(
                &m.id,
                m.where_,
                m.harness,
                &m.reason,
                &m.actor,
                m.session.as_deref(),
            )
            .await
    }
}

impl Message<Escalate> for Tasks {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Escalate, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (decision, manager) = self
            .ledger
            .escalate(
                &m.task,
                &m.question,
                &m.options,
                &m.actor,
                m.session.as_deref(),
            )
            .await?;
        if manager == Owner::User {
            let title = self
                .ledger
                .get(&m.task)
                .await
                .map(|t| t.title)
                .unwrap_or_default();
            tokio::spawn(ask_user(
                ctx.actor_ref().clone(),
                decision.clone(),
                m,
                title,
            ));
        }
        Ok(decision)
    }
}

/// The user is the manager (67 §11): the decision as an ask card on the
/// calling session — desktop, slack and signal already render `AskRequested`
/// — and the answer back through [`AnswerDecision`]. Without a session there
/// is no card; the task waits parked for a `decide`.
async fn ask_user(me: ActorRef<Tasks>, decision: String, m: Escalate, title: String) {
    let Some(session) = m
        .session
        .as_deref()
        .and_then(|s| s.parse::<runtime::SessionId>().ok())
    else {
        tracing::info!(%decision, "escalated to the user without a session; parked");
        return;
    };
    let Some(ask) = ActorRef::<::tools::AskUser>::lookup(::tools::ASK_ACTOR)
        .ok()
        .flatten()
    else {
        tracing::warn!(%decision, "ask actor is not running; parked");
        return;
    };
    let question = ::tools::Question {
        id: decision.clone(),
        question: format!(
            "{}\n\n(task {} — {title})",
            m.question,
            m.task.0.get(..8).unwrap_or(&m.task.0)
        ),
        options: m
            .options
            .iter()
            .map(|o| ::tools::Choice {
                label: o.clone(),
                description: None,
            })
            .collect(),
        recommended: None,
    };
    let Ok(out) = ask
        .ask(::tools::ask::Request {
            session,
            questions: vec![question],
        })
        .await
    else {
        return;
    };
    let answer = match out.answers.and_then(|v| v.into_iter().next()) {
        Some(::tools::Answer::Choice { index }) => m.options.get(index).cloned(),
        Some(::tools::Answer::Text { text }) => Some(text),
        _ => None,
    };
    if let Some(answer) = answer {
        let _ = me
            .tell(AnswerDecision {
                decision,
                answer,
                actor: "user".into(),
                session: None,
            })
            .await;
    }
}

impl Message<AnswerDecision> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: AnswerDecision,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger
            .answer_decision(&m.decision, &m.answer, &m.actor, m.session.as_deref())
            .await?;
        self.escalations.answer(&m.decision, &m.answer);
        Ok(())
    }
}

impl Message<AwaitDecision> for Tasks {
    type Reply = DelegatedReply<Option<String>>;

    async fn handle(
        &mut self,
        m: AwaitDecision,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let rx = self.escalations.subscribe(&m.decision);
        tokio::spawn(async move {
            let answer = escalate::wait(rx, m.wait).await;
            if let Some(tx) = tx {
                tx.send(answer);
            }
        });
        reply
    }
}

impl Message<Mention> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Mention, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.mention(&m.id, &m.text, &m.actor).await
    }
}

impl Message<MarkAnswered> for Tasks {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: MarkAnswered, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = (self.ledger.clock)();
        self.ledger
            .store
            .mark_answered(&m.sender, m.timestamp, now)
            .await
            .map_err(|e| format!("tasks store: {e:#}"))
    }
}

impl Message<NoteDisposition> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: NoteDisposition,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger
            .note_disposition(&m.id, &m.text, &m.actor, m.session.as_deref())
            .await
    }
}

impl Message<EpisodeClosing> for Tasks {
    type Reply = Result<Vec<TaskId>, String>;

    async fn handle(
        &mut self,
        m: EpisodeClosing,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // The desk's children (work sessions) go with it.
        let mut sessions = vec![m.session.clone()];
        if let Ok(id) = m.session.parse::<runtime::SessionId>()
            && let Some(agents) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten()
            && let Ok(kids) = agents.ask(agent::ListAgents { session: id }).await
        {
            sessions.extend(kids.into_iter().map(|k| k.id.to_string()));
        }
        self.ledger.episode_closing(&m.agent, &sessions).await
    }
}

impl Message<FinishWake> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: FinishWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Err(e) = self.ledger.finish_wake(&m.0).await {
            tracing::error!(wake = %m.0.id, "wake not finished: {e}");
        }
    }
}

impl Message<ReleaseClaims> for Tasks {
    type Reply = ();

    async fn handle(
        &mut self,
        m: ReleaseClaims,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(e) = self.ledger.release_claims(&m.0).await {
            tracing::error!(session = %m.0, "wake claims not released: {e}");
        }
    }
}

impl Message<NoContinuation> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(
        &mut self,
        m: NoContinuation,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger
            .no_continuation_by_id(&m.id, &m.agent, &m.why)
            .await
    }
}

impl Message<CreateForSchedule> for Tasks {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        m: CreateForSchedule,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger
            .create_for_schedule(&m.schedule, m.fire_ms, &m.agent, &m.prompt, m.project)
            .await
            .map(|t| t.id.0)
    }
}

impl Message<CloseForSchedule> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: CloseForSchedule,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger.close_for_schedule(&m.schedule, &m.why).await
    }
}

impl Message<Dispatch> for Tasks {
    type Reply = ();

    async fn handle(&mut self, _: Dispatch, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.dispatching {
            return;
        }
        let Some(router) = self.ledger.router.clone() else {
            return;
        };
        let deliveries = match self.ledger.dispatch().await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("wake dispatch: {e}");
                return;
            }
        };
        // Off the mailbox: opening a desk may be slow.
        for d in deliveries {
            let me = ctx.actor_ref().clone();
            let router = router.clone();
            tokio::spawn(async move {
                let ok = match router
                    .deliver(&d.wake.agent, &d.wake, d.task.as_ref())
                    .await
                {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(agent = %d.wake.agent, "wake not delivered: {e:#}");
                        false
                    }
                };
                let _ = me.tell(Delivered { wake: d.wake, ok }).await;
            });
        }
    }
}

impl Message<Delivered> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: Delivered, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let r = if m.ok {
            self.ledger.delivered(&m.wake).await
        } else {
            self.ledger.delivery_failed(&m.wake).await
        };
        if let Err(e) = r {
            tracing::error!("wake bookkeeping: {e}");
        }
    }
}

impl Message<Sweep> for Tasks {
    type Reply = ();

    async fn handle(&mut self, _: Sweep, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.dispatching {
            return;
        }
        let reachable = Self::reachable().await;
        match self.ledger.sweep(reachable.as_ref()).await {
            Ok(ids) if !ids.is_empty() => tracing::info!(n = ids.len(), "checkouts orphaned"),
            Ok(_) => {}
            Err(e) => tracing::error!("orphan sweep: {e}"),
        }
    }
}

/// [`schedule::TaskCreator`] over the tasks actor (67 §7.4).
pub struct ScheduleTasks;

/// The project of the schedule: its `project:` key, else the project of the
/// newest session whose `cwd` is the schedule's, else none.
async fn project_of_cwd(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()?;
    agents
        .ask(agent::ListSessions)
        .await
        .ok()?
        .into_iter()
        .find(|s| s.cwd.as_deref() == Some(cwd) && s.project.is_some())
        .and_then(|s| s.project)
}

#[async_trait::async_trait]
impl schedule::TaskCreator for ScheduleTasks {
    async fn create_for_schedule(
        &self,
        schedule: &str,
        fire_ms: u64,
        agent: &str,
        prompt: &str,
        project: Option<&str>,
        cwd: Option<&str>,
    ) -> anyhow::Result<String> {
        let project = match project {
            Some(p) => Some(p.to_owned()),
            None => project_of_cwd(cwd).await,
        };
        actor()
            .ok_or_else(|| anyhow::anyhow!("tasks is not running"))?
            .ask(CreateForSchedule {
                schedule: schedule.to_owned(),
                fire_ms,
                agent: agent.to_owned(),
                prompt: prompt.to_owned(),
                project,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    async fn close_for_schedule(&self, schedule: &str, why: &str) -> anyhow::Result<()> {
        actor()
            .ok_or_else(|| anyhow::anyhow!("tasks is not running"))?
            .ask(CloseForSchedule {
                schedule: schedule.to_owned(),
                why: why.to_owned(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// The plugin: reads `tasks`, opens the store, spawns [`Tasks`] as
/// [`TASKS_ACTOR`], starts the dispatch and sweep ticks, registers
/// [`ScheduleTasks`] with the schedule actor and the seven tools with the
/// registry once each is up.
pub struct TasksPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for TasksPlugin {
    fn name(&self) -> &'static str {
        "tasks"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: TasksConfig = rt.section("tasks").unwrap_or_else(|e| {
            tracing::debug!("no [tasks] config section, using defaults: {e}");
            TasksConfig::default()
        });
        let store = match store::Store::open(&rt.data_dir.join(DB)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("tasks unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor_store = store.clone();
        match replica::attach(store.pool(), &["tasks", "wakes", "answered"]).await {
            Ok(_) => tracing::info!("tasks: tasks, wakes and answered replicate"),
            Err(e) => tracing::debug!("tasks: local only ({e})"),
        }
        // `roster.decision_wait_secs` (67 §11) without a roster dependency.
        if let Some(secs) = rt
            .section::<serde_json::Value>("roster")
            .ok()
            .and_then(|v| v.get("decision_wait_secs")?.as_u64())
        {
            escalate::set_wait(Duration::from_secs(secs));
        }
        let ledger = ledger::Ledger::new(
            store,
            cfg,
            Arc::new(thread::DocThread),
            Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64)
            }),
            platform::process::hostname().to_owned(),
        );
        // A `dispatching` placeholder claim only lives between one process's
        // claim and its delivery; one found at start never returned.
        if let Err(e) = ledger.release_claims(ledger::DISPATCHING).await {
            tracing::warn!("orphan wake claims not released: {e}");
        }
        let actor = Tasks::spawn(Tasks {
            ledger,
            dispatching: true,
            escalations: escalate::Escalations::default(),
        });
        actor.register(TASKS_ACTOR)?;
        let pool = actor_store.pool().clone();
        tokio::spawn(async move { thread::adopt_threads(&pool).await });
        if let Err(e) = activity::Activity::start(rt, &actor_store).await {
            tracing::warn!("activity feed not started: {e:#}");
        }

        let me = actor.clone();
        tokio::spawn(async move {
            let mut dispatch = tokio::time::interval(DISPATCH_EVERY);
            let mut sweep = tokio::time::interval(SWEEP_EVERY);
            loop {
                tokio::select! {
                    _ = dispatch.tick() => {
                        if me.tell(Dispatch).await.is_err() { break }
                    }
                    _ = sweep.tick() => {
                        if me.tell(Sweep).await.is_err() { break }
                    }
                }
            }
        });
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(mesh) = rt2.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await
                && let Ok(s) = mesh.ask(mesh::Status).await
            {
                let _ = me.tell(SetEngine(s.self_id.0)).await;
            }
        });
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(sched) = rt2
                .lookup_when::<schedule::Schedule>(schedule::SCHEDULE_ACTOR)
                .await
            {
                let _ = sched
                    .ask(schedule::RegisterTaskCreator(Arc::new(ScheduleTasks)))
                    .await;
            }
        });
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg).await;
                let _ = reg
                    .ask(::tools::RegisterPrompt {
                        plugin: "tasks",
                        text: tools::PROMPT.into(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}

/// The tasks actor; `None` before the plugin started or when its store failed.
pub fn actor() -> Option<ActorRef<Tasks>> {
    ActorRef::<Tasks>::lookup(TASKS_ACTOR).ok().flatten()
}
