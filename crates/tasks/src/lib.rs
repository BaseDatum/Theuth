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
//!   [`AwaitDecision`], [`AnswerDecision`], [`LeaveDecision`], [`Mention`], [`MarkAnswered`],
//!   [`GetPacket`], [`ListPackets`], [`EffectiveReviewer`], [`ReviewPacket`],
//!   [`NoteDisposition`], [`EpisodeClosing`], [`FinishWake`], [`ReleaseClaims`],
//!   [`WriteBudgetOf`], [`MoveHome`], [`HomesOffline`], [`EnginesWith`], [`RegisterDeskRouter`]
//! - trait [`DeskRouter`] (the roster implements it)
//!
//! The rules live in [`ledger`]; this file is the actor around them.

/// Hub RPC methods and typed clients.
pub mod rpc_hub;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_hub::rpc_module(server),
    ]).expect("tasks RPC composition")
}

pub mod rpc_support;
pub mod activity;
pub mod config;
pub mod escalate;
pub mod ledger;
pub mod packet;
pub mod place;
pub mod rank;
pub mod store;
pub mod thread;
pub mod tools;
pub mod types;

use std::collections::HashSet;
use std::convert::Infallible;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{Actor, ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, PanicError};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub use config::TasksConfig;
pub use types::{
    Checkout, Disposition, DoneBecause, Owner, Priority, Task, TaskChangeKind, TaskChanged,
    TaskCreate, TaskEvent, TaskFilter, TaskId, TaskPatch, TaskState, Wake, WakeReason,
};

/// Registry name of the [`Tasks`] actor.
pub const TASKS_ACTOR: &str = "tasks";

/// Sqlite file under `data_dir` (67 §7).
pub const DB: &str = "tasks.sqlite";

/// Wake dispatch cadence (67 §7.3; a hub duty, 67 §4.4).
pub const DISPATCH_EVERY: Duration = Duration::from_secs(5);
/// Orphan sweep cadence (67 §7.5).
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// 207: blocker prefix of a decision the user left to the agent: `left:decision:<id>`.
pub const LEFT_PREFIX: &str = "left:";
/// 207: what a live `escalate` returns when the user leaves its decision (`decision: <this>`).
pub const LEFT_ANSWER: &str = "left to you — the user chose not to decide. Pick the option you would recommend, record it with task_note, and continue.";

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
    /// The engine `agent`'s work must run on (79 §4.9, its `runs_in`):
    /// `Ok(None)` = unpinned; `Ok(Some(pin))` = place there or nowhere;
    /// `Err(blocker)` = pinned but the engine cannot take work now — the
    /// task is set `blocked` on `blocker` (`sandbox:<name>`) and the roster
    /// clears it when the sandbox is ready.
    async fn pin_of(&self, agent: &str) -> Result<Option<place::Pin>, String> {
        let _ = agent;
        Ok(None)
    }
    /// 179 D3: `agent`'s file says `paused: true`. Its wakes are raised but
    /// never claimed or attempted while it is (no back-off accrues), the
    /// sweep leaves its checkouts alone, and a routine's fire is skipped.
    fn is_paused(&self, agent: &str) -> bool {
        let _ = agent;
        false
    }
    /// 182 D8: the engines (mesh ids) whose roster has `agent`, from the
    /// replicated per-engine agent list; empty when unknown.
    async fn engines_with(&self, agent: &str) -> Vec<String> {
        let _ = agent;
        Vec::new()
    }
    /// 182 S1: this engine's roster has `agent` (theuth is everywhere):
    /// `Some(false)` — no file for it here, a wake for it is not delivered
    /// here; `None` — its file here is momentarily invalid (an edit in flight).
    fn has_agent(&self, agent: &str) -> Option<bool> {
        let _ = agent;
        Some(true)
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

/// `ask` → `Result<Vec<ledger::Waiting>, String>`: 132 D5 — decisions and
/// reviews addressed to the user on tasks in the actor's reporting line
/// (every task for the user), oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaitingOnUser(pub String);

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
/// "done needs a reviewer…" | "tasks.cross_task_write_cap: …" (a hired agent's
/// session past its cap outside its reporting line, 99) | "superseded: …" |
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
    /// 132 D3: with `done` on a review that offered next moves, the index of the pick.
    #[serde(default)]
    pub next: Option<u32>,
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

/// `ask` → `Result<Option<WriteBudget>, String>`: where a session stands
/// against the cross-task write cap (99 D6); `None` = uncapped (theuth's line
/// or the user's own sessions).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteBudgetOf(pub String);

/// A capped session's standing against the cross-task write cap (99 D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteBudget {
    /// Distinct tasks outside the agent's line written to within the window.
    pub used: u32,
    /// `tasks.cross_task_write_cap`.
    pub cap: u32,
    /// `tasks.cross_task_write_window_secs`; `0` = the session's lifetime.
    pub window_secs: u64,
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
/// With a `summary` and a `board` (both required when the manager is the user,
/// 101 D1/R1) the request is stored as a `decision:<id>` packet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Escalate {
    /// Which.
    pub task: TaskId,
    /// The question.
    pub question: String,
    /// Markdown: what the decision is about (101 §3).
    pub summary: Option<String>,
    /// The decision mini-board's document id (101 R1).
    pub board: Option<String>,
    /// Options.
    pub options: Vec<packet::PacketOption>,
    /// Index into `options`.
    pub recommended: Option<u32>,
    /// Where to look.
    pub pointers: Vec<packet::Pointer>,
    /// Forwarding (103 F1/F2): the manager's view.
    pub recommendation: Option<packet::Recommendation>,
    /// A bundle (103 F3).
    pub items: Vec<packet::Item>,
    /// The cost of a wrong answer (132 D2); required when the user decides.
    pub if_wrong: Option<String>,
    /// Who.
    pub actor: String,
    /// Session.
    pub session: Option<String>,
}

/// `ask` → `Result<Option<Packet>, String>`: one packet by key (`decision:<id>` | `review:<task>`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetPacket(pub String);

/// `ask` → `Result<Vec<Packet>, String>`: every packet of these tasks, newest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListPackets(pub Vec<TaskId>);

/// `ask` → `Result<Owner, String>`: who would review `id` if it went to review
/// now — `reviewer` when given, else the task's, else the owner's manager
/// (101 D1: the tools refuse a hand-over to the user without a packet).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectiveReviewer {
    /// Which.
    pub id: TaskId,
    /// The reviewer the call names, if any.
    pub reviewer: Option<Owner>,
}

/// `ask` → `Result<Packet, String>`: store (replace) the `review:<task>` packet
/// and write the `**review requested**` thread block (101 D3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewPacket {
    /// Which.
    pub id: TaskId,
    /// Markdown.
    pub summary: String,
    /// The review mini-board's document id (101 R1).
    pub board: Option<String>,
    /// Where to look.
    pub pointers: Vec<packet::Pointer>,
    /// Claimed status against `done_when`.
    pub checks: Vec<packet::Check>,
    /// 132 D3: the next moves after approval (≤ 4); `recommended` indexes them.
    #[serde(default)]
    pub options: Vec<packet::PacketOption>,
    /// Index into `options`.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// The cost of a wrong call (132 D2), optional on a review.
    #[serde(default)]
    pub if_wrong: Option<String>,
    /// Who.
    pub actor: String,
}

/// `ask` → `Result<(), String>`: 101 R2 — `board` exists, is a board document
/// and carries the skeleton of `kind` ([`packet::check_board`]); the tools run
/// it for the user audience before any write.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckBoard {
    /// Decision or review skeleton.
    pub kind: packet::PacketKind,
    /// The board document id.
    pub board: String,
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

/// 207: `ask` → `Result<LeftDecision, String>`: the user leaves `decision` to its task's owner. Never wakes anyone.
/// `Err("no task is blocked on …")` when no task carries the blocker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeaveDecision {
    /// `decision:<id>`.
    pub decision: String,
    /// Who (`rpc_core::Client::actor`).
    pub actor: String,
}

/// 207: whether an `escalate` was still waiting live for the answer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct LeftDecision {
    /// An `escalate` was waiting live and now returns [`LEFT_ANSWER`].
    pub live: bool,
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

/// `tell` (179 D3): `agent` was un-paused — its wakes' back-off is cleared
/// and a dispatch pass runs now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unpaused(pub String);

/// `ask` → `u32` (179 D9): `agent`'s unclaimed wakes — `N wakes held`
/// while it is paused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeldWakes(pub String);

/// `ask` → `Option<Wake>` (179 D2): the wake this desk session holds
/// claimed — the one its suspended turn serves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedWake(pub String);

/// `ask` → `bool` (179 D5): the router says `agent` is paused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IsPaused(pub String);

/// `ask` → `Vec<String>` (XTICK): the router's [`DeskRouter::engines_with`];
/// empty before the roster registered it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnginesWith(pub String);

/// `tell`: the desk turn that took this wake did not yield (a cancel, a
/// timeout, a provider error — WAKE-LIVE-CHILD D2): the claim is released so
/// the wake is delivered again, not before `min_backoff_ms`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseWake {
    /// The wake, as delivered.
    pub wake: Wake,
    /// Floor on the re-delivery back-off (ms).
    pub min_backoff_ms: u64,
}

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

/// `ask` → `Result<Task, String>`: 182 D10 `task_move_home` — the task lives
/// on engine `to` (a mesh id) from now on ([`ledger::Ledger::move_home`]).
/// The RPC checks first (`check_only`), then moves a live checkout session
/// (181 `session_move`), then moves the task (`session_moved`: that session
/// continues the checkout there — 182 S4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveHome {
    /// The task.
    pub id: TaskId,
    /// Target engine id.
    pub to: String,
    /// `user` | `agent:<name>`.
    pub actor: String,
    /// The calling session (an agent's: the cross-task write cap).
    #[serde(default)]
    pub session: Option<String>,
    /// Only check the move would be accepted; change nothing.
    #[serde(default)]
    pub check_only: bool,
    /// The checkout's session was moved to `to` first: it keeps the checkout.
    #[serde(default)]
    pub session_moved: bool,
}

/// `ask` → `Result<Vec<HomeOffline>, String>`: 182 D9 — paired engines
/// unreachable for `tasks.home_offline_ask_secs` with tasks waiting on them.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct HomesOffline;

/// 182 D9: one offline engine and the tasks that wait for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeOffline {
    /// Engine id.
    pub engine: String,
    /// Its display name.
    pub name: String,
    /// ms since epoch: first seen unreachable by this engine.
    pub since_ms: u64,
    /// Agent-owned tasks homed there in todo / doing, or blocked with a wake pending.
    pub tasks: Vec<Task>,
}

/// `ask` → `Result<MovedHome, String>`: 182 D9 "Move them to <this
/// engine>" — every task waiting on engine `from` moves to `to`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveWaiting {
    /// The offline home (engine id).
    pub from: String,
    /// Target engine id.
    pub to: String,
    /// `user` | `agent:<name>`.
    pub actor: String,
}

/// 182 D9: what [`MoveWaiting`] did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MovedHome {
    /// The tasks moved.
    pub moved: Vec<TaskId>,
    /// `<title>: <why>` for each one that could not move (its agent is not there).
    pub refused: Vec<String>,
}

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

/// Internal `tell`: a sweep's probe answers came back; apply them.
struct SweepProbed {
    reachable: Option<HashSet<String>>,
    peers: Vec<(String, String, bool)>,
    probed: std::collections::HashMap<TaskId, ledger::Probed>,
}

/// Internal `ask`: [`EpisodeClosing`] with the desk's children resolved.
struct EpisodeSessionsClosing {
    agent: String,
    sessions: Vec<String>,
}

/// Internal `tell`: a delivery returned.
struct Delivered {
    wake: Wake,
    ok: bool,
}

/// Internal `tell`: the mesh told us who we are.
struct SetEngine(String);

/// The [`TASKS_ACTOR`] actor.
pub struct Tasks {
    ledger: ledger::Ledger,
    escalations: escalate::Escalations,
}

impl Actor for Tasks {
    type Args = Self;
    type Error = Infallible;

    fn name() -> &'static str {
        "Tasks"
    }

    async fn on_start(args: Self, _: ActorRef<Self>) -> Result<Self, Infallible> {
        Ok(args)
    }

    /// An `Err` reply to a `tell` (or a panic in a handler) must not stop the
    /// ledger: every task RPC would fail with "tasks not running" until a
    /// restart (101 live proof F-4). Log it and keep going.
    async fn on_panic(
        &mut self,
        _: WeakActorRef<Self>,
        err: PanicError,
    ) -> Result<ControlFlow<ActorStopReason>, Infallible> {
        tracing::error!(%err, "tasks actor: handler failed; continuing");
        Ok(ControlFlow::Continue(()))
    }
}

impl Tasks {
    /// The mesh's view: the connected engine set (ids and names, self
    /// included) and every paired peer as `(id, name, reachable)`; `None`
    /// when the mesh is not up (the sweep then triggers on staleness alone).
    async fn mesh_view() -> Option<(HashSet<String>, Vec<(String, String, bool)>)> {
        let status = mesh::actor()?.ask(mesh::Status).await.ok()?;
        let mut set: HashSet<String> = HashSet::from([status.self_id.0]);
        let mut peers = Vec::with_capacity(status.peers.len());
        for p in status.peers {
            if p.reachable {
                set.insert(p.name.clone());
                set.insert(p.id.0.clone());
            }
            peers.push((p.id.0, p.name, p.reachable));
        }
        Some((set, peers))
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
        // 182 S3: the backfill homes `doing` rows only on engines it knows.
        if let Some((_, peers)) = Self::mesh_view().await {
            self.ledger.note_peers(&peers);
        }
        if let Err(e) = self.ledger.set_engine(m.0).await {
            tracing::error!("182: home backfill: {e}");
        }
    }
}

impl Message<MoveHome> for Tasks {
    type Reply = Result<Task, String>;

    /// The mesh is asked here: it never asks tasks back.
    async fn handle(&mut self, m: MoveHome, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let view = Self::mesh_view().await;
        if let Some((_, peers)) = &view {
            self.ledger.note_peers(peers);
        }
        let reachable = view.as_ref().map(|(r, _)| r);
        let how = ledger::MoveHow {
            session: m.session.as_deref(),
            check_only: m.check_only,
            session_moved: m.session_moved,
        };
        self.ledger
            .move_home(&m.id, &m.to, &m.actor, reachable, how)
            .await
    }
}

impl Message<HomesOffline> for Tasks {
    type Reply = Result<Vec<HomeOffline>, String>;

    async fn handle(&mut self, _: HomesOffline, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some((_, peers)) = Self::mesh_view().await else {
            return Ok(Vec::new());
        };
        self.ledger.homes_offline(&peers).await
    }
}

impl Message<MoveWaiting> for Tasks {
    type Reply = Result<MovedHome, String>;

    async fn handle(&mut self, m: MoveWaiting, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let view = Self::mesh_view().await;
        if let Some((_, peers)) = &view {
            self.ledger.note_peers(peers);
        }
        self.ledger
            .move_waiting(&m.from, &m.to, &m.actor, view.as_ref().map(|(r, _)| r))
            .await
    }
}

impl Message<ListTasks> for Tasks {
    type Reply = Result<Vec<Task>, String>;

    async fn handle(&mut self, m: ListTasks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.list(&m.0).await
    }
}

impl Message<WaitingOnUser> for Tasks {
    type Reply = Result<Vec<ledger::Waiting>, String>;

    async fn handle(
        &mut self,
        m: WaitingOnUser,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger.waiting_on_user(&m.0).await
    }
}

impl Message<GetTask> for Tasks {
    type Reply = Result<Task, String>;

    async fn handle(&mut self, m: GetTask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.get(&m.0).await
    }
}

impl Message<GetPacket> for Tasks {
    type Reply = Result<Option<packet::Packet>, String>;

    async fn handle(&mut self, m: GetPacket, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.packet(&m.0).await
    }
}

impl Message<ListPackets> for Tasks {
    type Reply = Result<Vec<packet::Packet>, String>;

    async fn handle(&mut self, m: ListPackets, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.packets_for(&m.0).await
    }
}

impl Message<EffectiveReviewer> for Tasks {
    type Reply = Result<Owner, String>;

    async fn handle(
        &mut self,
        m: EffectiveReviewer,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let t = self.ledger.get(&m.id).await?;
        Ok(self.ledger.effective_reviewer(&t, m.reviewer.as_ref()))
    }
}

impl Message<ReviewPacket> for Tasks {
    type Reply = Result<packet::Packet, String>;

    async fn handle(&mut self, m: ReviewPacket, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.review_packet(m).await
    }
}

impl Message<CheckBoard> for Tasks {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CheckBoard, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.check_board(m.kind, &m.board).await
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
                m.next,
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

impl Message<WriteBudgetOf> for Tasks {
    type Reply = Result<Option<WriteBudget>, String>;

    async fn handle(
        &mut self,
        m: WriteBudgetOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger.write_budget(&m.0).await
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
        let ask = packet::Ask {
            question: m.question,
            summary: m.summary,
            board: m.board,
            options: m.options,
            recommended: m.recommended,
            pointers: m.pointers,
            recommendation: m.recommendation,
            items: m.items,
            if_wrong: m.if_wrong,
        };
        let (decision, manager) = self
            .ledger
            .escalate(&m.task, ask, &m.actor, m.session.as_deref())
            .await?;
        if manager == Owner::User {
            // The stored packet, not the message: a forward (103 F1) inherits
            // its question and options from the parent.
            let Ok(Some(p)) = self.ledger.packet(&decision).await else {
                tracing::warn!(%decision, "no packet stored for the user's decision; parked");
                return Ok(decision);
            };
            let title = self
                .ledger
                .get(&m.task)
                .await
                .map(|t| t.title)
                .unwrap_or_default();
            tokio::spawn(ask_user(ctx.actor_ref().clone(), p, m.session, title));
        }
        Ok(decision)
    }
}

/// The user is the manager (67 §11): the decision as an ask card on the
/// calling session — desktop, slack and signal already render `AskRequested`
/// — and the answer back through [`AnswerDecision`]. Without a session there
/// is no card; the task waits parked for a `decide`.
async fn ask_user(me: ActorRef<Tasks>, p: packet::Packet, session: Option<String>, title: String) {
    let decision = p.key.clone();
    let Some(session) = session
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
        question: card_question(&p, &title),
        options: p
            .options
            .iter()
            .map(|o| ::tools::Choice {
                label: o.label.clone(),
                description: o.description.clone(),
            })
            .collect(),
        // 103 F2: the forwarding manager's pick wins over the report's.
        recommended: p
            .recommendation
            .as_ref()
            .and_then(|r| r.option)
            .or(p.recommended)
            .map(|r| r as usize),
    };
    let Ok(out) = ask
        .ask(::tools::ask::Request {
            session,
            questions: vec![question],
            call_id: None,
        })
        .await
    else {
        return;
    };
    let answer = match out.answers.and_then(|v| v.into_iter().next()) {
        Some(::tools::Answer::Choice { index }) => p.options.get(index).map(|o| o.label.clone()),
        Some(::tools::Answer::Text { text }) => Some(text),
        _ => None,
    };
    if let Some(answer) = answer {
        // `ask`, not `tell`: an `Err` reply to a tell stops the actor (F-4).
        match me
            .ask(AnswerDecision {
                decision: decision.clone(),
                answer,
                actor: "user".into(),
                session: None,
            })
            .await
        {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(%decision, error = %e, "stale ask card: decision already answered elsewhere")
            }
        }
    }
}

/// The ask card [`ask_user`] raised for `decision` is dismissed once the
/// decision is answered by any path (`decide`, `task_decide`, the card
/// itself): the card's question id is the decision key, so the pending list
/// is the record. A dismissed card resolves `ask_user` with no answer.
async fn drop_card(decision: String) {
    let Some(ask) = ActorRef::<::tools::AskUser>::lookup(::tools::ASK_ACTOR)
        .ok()
        .flatten()
    else {
        return;
    };
    let Ok(pending) = ask.ask(::tools::PendingAsks).await else {
        return;
    };
    for req in pending {
        if req.questions.iter().any(|q| q.id == decision) {
            let _ = ask
                .ask(::tools::AnswerAsk {
                    id: req.id,
                    answers: None,
                })
                .await;
        }
    }
}

/// The ask card's text (101 D4, 103 item 5): the question, the bundle as
/// numbered lines (`packet::items_summary`), the task's short id and title.
fn card_question(p: &packet::Packet, title: &str) -> String {
    let mut s = p.question.clone().unwrap_or_default();
    if !p.items.is_empty() {
        s.push_str("\n\n");
        s.push_str(&packet::items_summary(&p.items));
    }
    s.push_str(&format!(
        "\n\n(task {} — {title})",
        p.task.0.get(..8).unwrap_or(&p.task.0)
    ));
    s
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
        tokio::spawn(drop_card(m.decision));
        Ok(())
    }
}

impl Message<LeaveDecision> for Tasks {
    type Reply = Result<LeftDecision, String>;

    async fn handle(
        &mut self,
        m: LeaveDecision,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let live = self.escalations.waiting(&m.decision);
        self.ledger
            .leave_decision(&m.decision, &m.actor, live)
            .await?;
        if live {
            self.escalations.answer(&m.decision, LEFT_ANSWER);
        }
        tokio::spawn(drop_card(m.decision));
        Ok(LeftDecision { live })
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
        // 207 E3: the user left the decision between `Escalate` and this wait
        // (recorded as parked): the escalate is live after all — answer now.
        let left = self
            .ledger
            .take_left_decision(&m.decision)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(decision = %m.decision, "take_left_decision: {e}");
                false
            });
        if left {
            if let Some(tx) = tx {
                tx.send(Some(LEFT_ANSWER.to_owned()));
            }
            return reply;
        }
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
    type Reply = DelegatedReply<Result<Vec<TaskId>, String>>;

    /// Off the mailbox: the agent actor asked here may be inside its own
    /// `Identity` handler asking tasks `ListTasks` (`roster::Identities`);
    /// holding this mailbox while asking it back deadlocks both. The children
    /// are resolved in a task and the ledger work comes back as an `ask`.
    async fn handle(
        &mut self,
        m: EpisodeClosing,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
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
            let r = me
                .ask(EpisodeSessionsClosing {
                    agent: m.agent,
                    sessions,
                })
                .await
                .map_err(|e| format!("tasks: {e}"));
            match tx {
                Some(tx) => tx.send(r),
                None => {
                    if let Err(e) = r {
                        tracing::error!("episode closing: {e}");
                    }
                }
            }
        });
        reply
    }
}

impl Message<EpisodeSessionsClosing> for Tasks {
    type Reply = Result<Vec<TaskId>, String>;

    async fn handle(
        &mut self,
        m: EpisodeSessionsClosing,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ledger.episode_closing(&m.agent, &m.sessions).await
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

impl Message<Unpaused> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: Unpaused, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Err(e) = self.ledger.unpaused(&m.0).await {
            tracing::error!(agent = %m.0, "back-off not cleared: {e}");
        }
        let _ = ctx.actor_ref().tell(Dispatch).await;
    }
}

impl Message<HeldWakes> for Tasks {
    type Reply = u32;

    async fn handle(&mut self, m: HeldWakes, _: &mut Context<Self, Self::Reply>) -> u32 {
        self.ledger.held_wakes(&m.0).await.unwrap_or_else(|e| {
            tracing::error!(agent = %m.0, "held wakes not counted: {e}");
            0
        })
    }
}

impl Message<ClaimedWake> for Tasks {
    type Reply = Option<Wake>;

    async fn handle(&mut self, m: ClaimedWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.claimed_by(&m.0).await.unwrap_or_else(|e| {
            tracing::error!(session = %m.0, "claimed wake not read: {e}");
            None
        })
    }
}

impl Message<IsPaused> for Tasks {
    type Reply = bool;

    async fn handle(&mut self, m: IsPaused, _: &mut Context<Self, Self::Reply>) -> bool {
        self.ledger
            .router
            .as_ref()
            .is_some_and(|r| r.is_paused(&m.0))
    }
}

impl Message<EnginesWith> for Tasks {
    type Reply = Vec<String>;

    async fn handle(&mut self, m: EnginesWith, _: &mut Context<Self, Self::Reply>) -> Vec<String> {
        match &self.ledger.router {
            Some(r) => r.engines_with(&m.0).await,
            None => Vec::new(),
        }
    }
}

impl Message<ReleaseWake> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: ReleaseWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Err(e) = self.ledger.release_wake(&m.wake, m.min_backoff_ms).await {
            tracing::error!(wake = %m.wake.id, "wake not released: {e}");
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

    /// 182 D4: every engine dispatches — the wakes of the tasks it works.
    /// M2a: not before the first sync with the mesh (a task may have moved).
    async fn handle(&mut self, _: Dispatch, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(router) = self.ledger.router.clone() else {
            return;
        };
        if !(self.ledger.synced)() {
            return;
        }
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
                    Ok(()) => {
                        tracing::info!(agent = %d.wake.agent, wake = %d.wake.id, task = ?d.wake.task, reason = d.wake.reason.as_str(), "wake delivered");
                        true
                    }
                    // 179 D3: the roster's gate (`held: …`) is the backstop
                    // for a pause this pass did not see yet.
                    Err(e) if e.to_string().starts_with("held: ") => {
                        tracing::debug!(agent = %d.wake.agent, "wake not delivered: {e:#}");
                        false
                    }
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

    /// Off the mailbox: `mesh_view()` asks the mesh and the liveness probe
    /// asks the agent actor, which may be asking tasks back (`ListTasks` from
    /// `roster::Identities` inside its `Identity` handler). The answers
    /// return as [`SweepProbed`] and are applied against the rows as they
    /// are then. 182 D4: every engine sweeps the tasks it works — M2a: once
    /// the first sync with the mesh is done.
    async fn handle(&mut self, _: Sweep, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !(self.ledger.synced)() {
            return;
        }
        let prober = self.ledger.prober();
        let now = (self.ledger.clock)();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let (reachable, peers) = match Self::mesh_view().await {
                Some((r, p)) => (Some(r), p),
                None => (None, Vec::new()),
            };
            match prober.probe(now, reachable.as_ref()).await {
                Ok(probed) => {
                    let _ = me
                        .tell(SweepProbed {
                            reachable,
                            peers,
                            probed,
                        })
                        .await;
                }
                Err(e) => tracing::error!("orphan sweep: {e}"),
            }
        });
    }
}

impl Message<SweepProbed> for Tasks {
    type Reply = ();

    async fn handle(&mut self, m: SweepProbed, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ledger.note_peers(&m.peers);
        match self
            .ledger
            .sweep_apply(m.reachable.as_ref(), &m.probed)
            .await
        {
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

/// A session is at work: a turn is in flight, it waits on a peer or an
/// approval, or it runs a job or PTY.
fn session_active(a: &agent::SessionActivity) -> bool {
    a.turn_started_at.is_some()
        || matches!(
            a.state,
            agent::AgentState::Running
                | agent::AgentState::Awaiting { .. }
                | agent::AgentState::Approval { .. }
        )
        || a.jobs + a.ptys > 0
}

/// [`ledger::LivenessProbe`] over the agent actor (WAKE-LIVE-CHILD D4): the
/// session's own [`agent::ActivityOf`] plus every descendant from
/// [`agent::ListAgents`]. `None` without an agent actor or for an id this
/// engine has no row for.
pub struct AgentProbe;

#[async_trait::async_trait]
impl ledger::LivenessProbe for AgentProbe {
    async fn liveness(&self, session: &str) -> Option<ledger::Liveness> {
        let id: runtime::SessionId = session.parse().ok()?;
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()?;
        let own = agents
            .ask(agent::ActivityOf(vec![id]))
            .await
            .ok()?
            .remove(&id)?;
        let kids = agents.ask(agent::ListAgents { session: id }).await.ok()?;
        let ms = |t: Option<chrono::DateTime<chrono::Utc>>| t.map(|t| t.timestamp_millis() as u64);
        Some(ledger::Liveness {
            active_now: session_active(&own) || kids.iter().any(|k| session_active(&k.activity)),
            last_active_ms: kids
                .iter()
                .filter_map(|k| ms(k.activity.last_active))
                .chain(ms(own.last_active))
                .max(),
        })
    }
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

    async fn agent_paused(&self, agent: &str) -> bool {
        match actor() {
            Some(t) => t.ask(IsPaused(agent.to_owned())).await.unwrap_or(false),
            None => false,
        }
    }

    async fn engines_with(&self, agent: &str) -> Vec<String> {
        match actor() {
            Some(t) => t
                .ask(EnginesWith(agent.to_owned()))
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

/// Declare wire contracts before starting producers.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<activity::ActivityAppended>().await?;
    rt.register_wire_event::<TaskChanged>().await
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

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
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
        match replica::attach(store.pool(), &["tasks", "wakes", "answered", "packets"]).await {
            Ok(handle) => {
                tracing::info!("tasks: tasks, wakes, answered and packets replicate");
                // 182 M1: homes of open tasks reach a peer that attached the
                // column late (a staggered upgrade) — under their own clocks.
                mesh::reannounce_on_connect(
                    rt,
                    handle,
                    "tasks",
                    "home IS NOT NULL AND state NOT IN ('done', 'cancelled') AND id <> ?",
                    Some(&["home"]),
                );
            }
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
        let mut ledger = ledger::Ledger::new(
            store,
            cfg,
            Arc::new(thread::DocThread),
            Arc::new(platform::time::now_ms),
            platform::process::hostname().to_owned(),
        );
        // 182 D2: creates before `SetEngine` leave `home` unset; `set_engine` stamps them.
        ledger.homed = false;
        // 214 D3: the ledger's committed changes, published as `tasks.changed`.
        let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel::<TaskChanged>();
        ledger.changed = Some(changed);
        let rt2 = rt.clone();
        tokio::spawn(async move {
            while let Some(c) = changes.recv().await {
                rt2.publish(c).await;
            }
        });
        // A `dispatching` placeholder claim only lives between one process's
        // claim and its delivery; one found at start never returned.
        if let Err(e) = ledger.release_claims(ledger::DISPATCHING).await {
            tracing::warn!("orphan wake claims not released: {e}");
        }
        let actor = Tasks::spawn(Tasks {
            ledger,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 103 item 5: the ask card lists a bundle's items with their recommended pick.
    #[test]
    fn card_question_lists_the_bundle() {
        let opt = |l: &str| packet::PacketOption {
            label: l.into(),
            description: None,
        };
        let mut p = packet::Packet {
            key: "decision:d1".into(),
            task: TaskId("0123456789abcdef".into()),
            kind: packet::PacketKind::Decision,
            by: "agent:ops".into(),
            at_ms: 1,
            question: Some("DNS records?".into()),
            summary: String::new(),
            board: None,
            options: vec![],
            recommended: None,
            pointers: vec![],
            checks: vec![],
            recommendation: None,
            items: vec![],
            to: None,
            if_wrong: None,
            left_to_agent_ms: None,
        };
        assert_eq!(
            card_question(&p, "portal"),
            "DNS records?\n\n(task 01234567 — portal)"
        );
        p.items = vec![
            packet::Item {
                id: "a".into(),
                title: "A record".into(),
                description: None,
                options: vec![opt("add"), opt("skip")],
                recommended: Some(0),
            },
            packet::Item {
                id: "mx".into(),
                title: "MX".into(),
                description: None,
                options: vec![],
                recommended: None,
            },
        ];
        assert_eq!(
            card_question(&p, "portal"),
            "DNS records?\n\n1. A record — recommended: add\n2. MX\n\n(task 01234567 — portal)"
        );
    }

    /// 106 F-4: answering a decision through `decide` dismisses its ask card;
    /// a stale card answered afterwards (or any `Err` reply to a `tell`) is
    /// logged and leaves the actor alive.
    #[tokio::test]
    async fn stale_card_and_fallible_tell_leave_the_actor_alive() {
        let mut r = ledger::tests::rig().await;
        let t = r.task("agent:lead").await; // lead reports to the user
        ledger::tests::board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let ask = ::tools::AskUser::spawn(::tools::AskUser::new(rt, Duration::from_secs(30)));
        ask.register(::tools::ASK_ACTOR).unwrap();
        let actor = Tasks::spawn(Tasks {
            ledger: r.ledger,
            escalations: Default::default(),
        });
        let session = runtime::SessionId::new().to_string();
        let opt = |l: &str| packet::PacketOption {
            label: l.into(),
            description: None,
        };
        let decision = actor
            .ask(Escalate {
                task: t.id.clone(),
                question: "ship?".into(),
                summary: Some("Ship the release.".into()),
                board: Some("b1".into()),
                options: vec![opt("yes"), opt("no")],
                recommended: Some(0),
                pointers: vec![],
                recommendation: None,
                items: vec![],
                if_wrong: Some("low, the release is a tag".into()),
                actor: "agent:lead".into(),
                session: Some(session.clone()),
            })
            .await
            .unwrap();
        let pending = |n: usize| {
            let ask = ask.clone();
            async move {
                for _ in 0..200 {
                    let p = ask.ask(::tools::PendingAsks).await.unwrap();
                    if p.len() == n {
                        return p;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!("pending asks never reached {n}");
            }
        };
        let card = pending(1).await.remove(0);
        assert_eq!(card.questions[0].id, decision);

        // the decision is answered elsewhere: the card goes
        actor
            .ask(AnswerDecision {
                decision: decision.clone(),
                answer: "yes".into(),
                actor: "user".into(),
                session: None,
            })
            .await
            .unwrap();
        pending(0).await;
        assert!(
            actor
                .ask(GetTask(t.id.clone()))
                .await
                .unwrap()
                .blockers
                .is_empty()
        );

        // a stale card for the answered decision (the F-4 race): answered → Err logged, actor alive
        let p = actor
            .ask(GetPacket(decision.clone()))
            .await
            .unwrap()
            .unwrap();
        let stale = tokio::spawn(ask_user(actor.clone(), p, Some(session), "t".into()));
        let card = pending(1).await.remove(0);
        ask.ask(::tools::AnswerAsk {
            id: card.id,
            answers: Some(vec![::tools::Answer::Choice { index: 1 }]),
        })
        .await
        .unwrap();
        stale.await.unwrap();
        assert!(
            actor.ask(GetTask(t.id.clone())).await.is_ok(),
            "actor stopped"
        );

        // on_panic: an Err reply to a tell no longer stops the ledger
        actor
            .tell(AnswerDecision {
                decision: "decision:nope".into(),
                answer: "x".into(),
                actor: "user".into(),
                session: None,
            })
            .await
            .unwrap();
        assert!(actor.ask(GetTask(t.id)).await.is_ok(), "actor stopped");
    }

    /// 207: `LeaveDecision` answers a live `escalate` with [`LEFT_ANSWER`] (`live: true`),
    /// reports a parked one as `live: false`, and refuses a decision nothing is blocked on.
    #[tokio::test]
    async fn leave_decision_reports_live_and_answers_the_waiter() {
        let mut r = ledger::tests::rig().await;
        let live_task = r.task("agent:lead").await;
        let parked_task = r.task("agent:lead").await;
        ledger::tests::board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let actor = Tasks::spawn(Tasks {
            ledger: r.ledger,
            escalations: Default::default(),
        });
        let escalate = |task: &TaskId| Escalate {
            task: task.clone(),
            question: "ship?".into(),
            summary: Some("Ship the release.".into()),
            board: Some("b1".into()),
            options: vec![],
            recommended: None,
            pointers: vec![],
            recommendation: None,
            items: vec![],
            if_wrong: Some("low, the release is a tag".into()),
            actor: "agent:lead".into(),
            session: None,
        };
        let live = actor.ask(escalate(&live_task.id)).await.unwrap();
        let parked = actor.ask(escalate(&parked_task.id)).await.unwrap();
        // the escalate tool's live wait is in the mailbox before the leave
        let waiter = actor
            .ask(AwaitDecision {
                decision: live.clone(),
                wait: Duration::from_secs(30),
            })
            .enqueue()
            .await
            .unwrap();
        let left = actor
            .ask(LeaveDecision {
                decision: live.clone(),
                actor: "user".into(),
            })
            .await
            .unwrap();
        assert!(left.live);
        assert_eq!(waiter.await.unwrap(), Some(LEFT_ANSWER.to_owned()));
        assert!(
            actor
                .ask(GetTask(live_task.id.clone()))
                .await
                .unwrap()
                .blockers
                .is_empty()
        );

        let left = actor
            .ask(LeaveDecision {
                decision: parked.clone(),
                actor: "user".into(),
            })
            .await
            .unwrap();
        assert!(!left.live);
        assert_eq!(
            actor
                .ask(GetTask(parked_task.id.clone()))
                .await
                .unwrap()
                .blockers,
            vec![format!("{LEFT_PREFIX}{parked}")]
        );
        // 207 E3: the escalate's live wait only arrives now (the leave landed
        // between Escalate and AwaitDecision): it gets the left answer at
        // once, and the task is unblocked as by a live leave.
        let late = tokio::time::timeout(
            Duration::from_secs(5),
            actor.ask(AwaitDecision {
                decision: parked.clone(),
                wait: Duration::from_secs(30),
            }),
        )
        .await
        .expect("answered at once, not after the wait")
        .unwrap();
        assert_eq!(late, Some(LEFT_ANSWER.to_owned()));
        let t = actor.ask(GetTask(parked_task.id.clone())).await.unwrap();
        assert!(t.blockers.is_empty(), "{:?}", t.blockers);
        assert_ne!(t.state, TaskState::Blocked);

        let err = actor
            .ask(LeaveDecision {
                decision: "decision:nope".into(),
                actor: "user".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no task is blocked on decision:nope"), "{err}");
    }

    /// A liveness probe shaped like the real one (`AgentProbe` asks the agent
    /// actor, whose `Identity` handler asks tasks back): it asks the tasks
    /// actor `ListTasks` before answering, then waits for `gate`.
    struct AskingProbe {
        tasks: std::sync::OnceLock<ActorRef<Tasks>>,
        gate: tokio::sync::Notify,
        asked: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl ledger::LivenessProbe for AskingProbe {
        async fn liveness(&self, _: &str) -> Option<ledger::Liveness> {
            let tasks = self.tasks.get()?.clone();
            tasks.ask(ListTasks(TaskFilter::default())).await.ok()?;
            self.asked.notify_one();
            self.gate.notified().await;
            Some(ledger::Liveness {
                active_now: false,
                last_active_ms: None,
            })
        }
    }

    /// A stale `doing` checkout with a probe that asks back into the actor.
    async fn asking_rig() -> (ActorRef<Tasks>, Arc<AskingProbe>, TaskId) {
        let mut r = ledger::tests::rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.tick(3_600_000);
        let probe = Arc::new(AskingProbe {
            tasks: Default::default(),
            gate: Default::default(),
            asked: Default::default(),
        });
        r.ledger.probe = probe.clone();
        let actor = Tasks::spawn(Tasks {
            ledger: r.ledger,
            escalations: Default::default(),
        });
        probe.tasks.set(actor.clone()).ok().unwrap();
        (actor, probe, t.id)
    }

    async fn orphaned(actor: &ActorRef<Tasks>, id: &TaskId) -> bool {
        for _ in 0..500 {
            let t = actor.ask(GetTask(id.clone())).await.unwrap();
            if t.checkout.is_some_and(|c| c.orphaned) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// Audit #1: the sweep's probe asks the agent actor, which asks tasks
    /// back. With the probe on the mailbox neither ask ever answered; now
    /// the probe runs off it, the inbound ask answers and the sweep applies.
    #[tokio::test]
    async fn sweep_probe_may_ask_tasks_back() {
        let (actor, probe, id) = asking_rig().await;
        probe.gate.notify_one();
        actor.tell(Sweep).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), probe.asked.notified())
            .await
            .expect("the probe's ask into tasks never answered");
        tokio::time::timeout(
            Duration::from_secs(5),
            actor.ask(ListTasks(TaskFilter::default())),
        )
        .await
        .expect("tasks mailbox held by the sweep")
        .unwrap();
        assert!(orphaned(&actor, &id).await, "the sweep did not apply");
    }

    /// Audit #1, the other edge: `EpisodeClosing` answers while a sweep's
    /// probe is still out (its ask into tasks pending behind the gate).
    #[tokio::test]
    async fn episode_closing_answers_while_probe_is_out() {
        let (actor, probe, id) = asking_rig().await;
        actor.tell(Sweep).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), probe.asked.notified())
            .await
            .expect("the probe's ask into tasks never answered");
        let blocked = tokio::time::timeout(
            Duration::from_secs(5),
            actor.ask(EpisodeClosing {
                agent: "lead".into(),
                session: "s-lead".into(),
            }),
        )
        .await
        .expect("tasks mailbox held by the sweep")
        .unwrap();
        assert!(blocked.is_empty());
        probe.gate.notify_one();
        assert!(orphaned(&actor, &id).await, "the sweep did not apply");
    }
}
