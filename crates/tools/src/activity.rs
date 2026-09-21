//! Activity protocol (47 §4): acknowledged admission and honest settlement of
//! session-owned work, so the agent supervisor can positively certify "nothing
//! is running" before continuing a yielded turn.
//!
//! The tools crate owns the *types*; the `agent` crate owns the ledger and the
//! decision. `Agents` implements [`Admit`], [`Settle`] and [`Notice`], hands
//! the registry an [`ActivitySink`] through [`crate::RegisterActivity`], and
//! from then on every [`crate::Invoke`] is admitted before anything runs and
//! settled when the real work stops. Nothing here reads tool output or job
//! rows: authority flows through these messages only; `JobEvent`,
//! `ToolCallFinished` and the like stay observation.
//!
//! # Lifetimes
//!
//! An admitted operation is an [`Activity`]: an `Arc`-shared guard. Cloning
//! shares the lifetime; the entry is retired when the *last* clone drops, from
//! `Drop`, so a panicking or aborted task still settles, and a task that
//! outlives its caller keeps the entry alive by holding a clone. The rule for
//! detached work is therefore: clone the `Activity` **before** spawning and
//! move the clone into the task. Never settle from an observer.
//!
//! Work that must outlive the invocation as an independent obligation (a
//! background job) admits a *child* operation with [`Activity::admit`] and
//! settles it with [`Activity::set_result`] when the producer knows the
//! outcome. Work whose termination cannot be established calls
//! [`Activity::unknown`]: the supervisor retains a blocker until its owner
//! reconciles; dropping after `unknown` sends nothing more. A still-running
//! operation with something the model should hear now (a quiet PTY, 61)
//! sends it with [`Activity::notify`]; the guard is unchanged.
//!
//! # Scope
//!
//! [`Activity::scope`] installs the guard as the tokio task-local *current*
//! activity; [`current`] reads it. Task-locals do not cross `tokio::spawn` or
//! an actor mailbox: spawned futures are wrapped with `scope` again, and
//! messages to other actors carry ownership in a [`Tracked`] envelope.
//!
//! # Uncertified mode
//!
//! Until `Agents` registers a sink there is nothing to certify: `admit` with
//! `None` yields an untracked `Activity`, tools run as before and the
//! supervisor must treat coverage as unknown. Once a sink *is* registered a
//! dead recipient is a rejection: fail closed rather than run untracked work
//! that a later nudge decision would not see. An untracked guard still running
//! when a sink arrives is *adopted* by the registry: reported `Unknown` (the
//! supervisor never admitted it) and settled by its last clone like any other.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use kameo::actor::{ActorRef, Recipient, ReplyRecipient};
use kameo::message::Message;
use runtime::SessionId;

use crate::jobs::{JobEvent, JobId, lock};

/// Process-local identity of one admitted operation. Never reused within a
/// process; not a stable identifier across restarts (47 §4.5: an unclean
/// restart loses the live proof anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub u64);

impl OperationId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// What an operation is, for the ledger and for the warning that names an
/// unknown outcome. Carries identities only: never arguments, command lines
/// or output, so a quiet invocation (37 §3.2) discloses nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A registry invocation: approval, nested calls, tool body, result handoff.
    Invoke {
        /// Tool name.
        tool: String,
        /// Provider call id; not unique across retries, hence [`OperationId`].
        call_id: String,
        /// The caller consumes the output itself; UIs saw no card.
        quiet: bool,
    },
    /// A pipe job's independent lifetime (spawn → exit → result accepted).
    Job(JobId),
    /// A PTY job's independent lifetime.
    Pty(JobId),
    /// A harness-managed command with a completion the model is owed
    /// (browser command, render, readiness wait). Static label only.
    Command(&'static str),
}

/// One admitted operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    /// Unique per process.
    pub id: OperationId,
    /// The conversation that owns the work. Resource keys such as a scrape
    /// render slot's `SessionId` are *not* owners; the originating invocation's
    /// owner propagates through the nested path (47 §4.2 rule 5).
    pub owner: SessionId,
    /// What it is.
    pub kind: Kind,
}

/// `ask` → [`Admission`]. Sent **before** execution or detached scheduling.
/// The handler inserts the ledger entry, then replies; it must not await
/// anything that can ask back into the tools actor.
#[derive(Clone, Debug)]
pub struct Admit(pub Operation);

/// Reply to [`Admit`]. `Rejected` is an ordinary tool error to the caller and
/// the operation never runs (47 §4.1).
#[derive(Clone, Debug, PartialEq, Eq, kameo::Reply)]
pub enum Admission {
    /// Entry recorded; the operation may run.
    Admitted,
    /// Refused, with a reason safe to show the model.
    Rejected(String),
}

/// `tell`. How an admitted operation left the ledger. Duplicates and
/// settlement of unknown ids are idempotent by identity (47 §4.4).
#[derive(Clone, Debug)]
pub enum Settle {
    /// Actual work settled and owes no separate wake. Sent when the last
    /// [`Activity`] clone drops; late is safe, early never happens.
    Finished {
        /// The operation.
        op: OperationId,
    },
    /// Actual work settled **and** its result is owed to the model: the entry
    /// atomically becomes a pending-delivery obligation the supervisor formats,
    /// routes and retires only once the receiving session has accepted the
    /// input. `result` is always [`JobEvent::Finished`]; the public bus event
    /// stays for UIs, delivery never happens on both paths (47 §4.1).
    FinishedWithResult {
        /// The operation.
        op: OperationId,
        /// The `JobEvent::Finished` payload.
        result: JobEvent,
    },
    /// Termination or result ownership could not be established. The
    /// supervisor retains a blocker and surfaces `failure`; no `Finished`
    /// follows. Cleared only by its owner's positive reconciliation
    /// (47 §4.2 rule 8).
    Unknown {
        /// The operation.
        op: OperationId,
        /// The owning session, so an operation the supervisor never admitted
        /// (adopted at registration) blocks that session's subtree only.
        owner: SessionId,
        /// Names the operation; never carries payloads.
        failure: String,
    },
}

/// `tell`. Something about an admitted operation the model should hear
/// without the operation settling. The supervisor delivers it like a result
/// (wrapped, injected under the op's scope) only while the op is still
/// admitted; a notice for a retired op is dropped.
#[derive(Clone, Debug)]
pub enum Notice {
    /// A running PTY job's screen has been unchanged for `quiet_secs` (61).
    JobIdle {
        /// The job's operation.
        op: OperationId,
        /// Always [`JobEvent::Idle`].
        event: JobEvent,
    },
}

/// Admission rejected or the supervisor is unreachable. Rendered to the
/// model as an ordinary tool error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected(pub String);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

/// The supervisor's typed recipients, registered through
/// [`crate::RegisterActivity`]. Cloning shares the underlying actor refs.
#[derive(Clone)]
pub struct ActivitySink {
    admit: ReplyRecipient<Admit, Admission>,
    settle: Recipient<Settle>,
    notice: Recipient<Notice>,
}

impl fmt::Debug for ActivitySink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivitySink")
            .field("actor", &self.settle.id())
            .field("alive", &self.settle.is_alive())
            .finish()
    }
}

impl ActivitySink {
    /// All recipients of one actor, normally `Agents`.
    pub fn new<A>(actor: ActorRef<A>) -> Self
    where
        A: kameo::Actor
            + Message<Admit, Reply = Admission>
            + Message<Settle, Reply = ()>
            + Message<Notice, Reply = ()>,
    {
        Self {
            admit: actor.clone().reply_recipient(),
            settle: actor.clone().recipient(),
            notice: actor.recipient(),
        }
    }

    /// The supervisor still exists. A dead sink means coverage is lost, not empty.
    pub fn is_alive(&self) -> bool {
        self.settle.is_alive()
    }

    /// Admit a root operation for `owner`. Awaits the supervisor's
    /// acknowledgement, so call it from a task, not an actor handler. A
    /// dead or unresponsive recipient is `Err`: fail closed.
    pub async fn admit(&self, owner: SessionId, kind: Kind) -> Result<Activity, Rejected> {
        let op = Operation {
            id: OperationId::next(),
            owner,
            kind,
        };
        match self.admit.ask(Admit(op.clone())).await {
            Ok(Admission::Admitted) => Ok(Activity::new(op, Some(self.clone()))),
            Ok(Admission::Rejected(why)) => Err(Rejected(why)),
            Err(e) => Err(Rejected(format!("activity supervisor unavailable: {e}"))),
        }
    }

    /// Fire-and-forget on the supervisor's unbounded mailbox; the only
    /// failure is a closed mailbox, which is logged: nothing else can be
    /// done from `Drop`, and the supervisor that would have cared is gone.
    fn settle(&self, msg: Settle) {
        if let Err(e) = self.settle.tell(msg).try_send() {
            tracing::warn!(error = %e, "activity settlement lost: supervisor gone");
        }
    }

    /// Same delivery as [`Self::settle`], for a [`Notice`].
    fn notify(&self, msg: Notice) {
        if let Err(e) = self.notice.tell(msg).try_send() {
            tracing::warn!(error = %e, "activity notice lost: supervisor gone");
        }
    }
}

/// Admit under an optional sink: `None` (nothing registered yet) yields an
/// untracked guard so tools keep working before integration; `Some` is
/// [`ActivitySink::admit`]. Coverage is certified only in the second case.
pub async fn admit(
    sink: Option<&ActivitySink>,
    owner: SessionId,
    kind: Kind,
) -> Result<Activity, Rejected> {
    match sink {
        Some(sink) => sink.admit(owner, kind).await,
        None => Ok(Activity::untracked(owner, kind)),
    }
}

enum Outcome {
    Finished,
    Result(JobEvent),
    Unknown,
}

struct Inner {
    op: Operation,
    /// Set at admission, or later by [`Activity::adopt`]; never replaced.
    sink: OnceLock<ActivitySink>,
    outcome: Mutex<Outcome>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let Some(sink) = self.sink.get() else { return };
        let outcome = std::mem::replace(
            self.outcome
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner),
            Outcome::Finished,
        );
        let op = self.op.id;
        match outcome {
            Outcome::Finished => sink.settle(Settle::Finished { op }),
            Outcome::Result(result) => sink.settle(Settle::FinishedWithResult { op, result }),
            // Already reported; the blocker stays until its owner reconciles.
            Outcome::Unknown => {}
        }
    }
}

/// An admitted operation's lifetime guard, shared by every clone; see the
/// module docs for the clone-before-spawn rule. Independent of [`crate::ToolCx`]
/// so existing tools and struct literals are untouched; read it with
/// [`current`] inside an invocation.
#[derive(Clone)]
pub struct Activity(Arc<Inner>);

impl fmt::Debug for Activity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Activity")
            .field("op", &self.0.op)
            .field("tracked", &self.is_tracked())
            .finish()
    }
}

impl Activity {
    fn new(op: Operation, sink: Option<ActivitySink>) -> Self {
        Self(Arc::new(Inner {
            op,
            sink: sink.map(OnceLock::from).unwrap_or_default(),
            outcome: Mutex::new(Outcome::Finished),
        }))
    }

    /// A guard with no supervisor behind it: tools run, nothing is certified.
    /// Only for the pre-registration window and standalone tests.
    pub fn untracked(owner: SessionId, kind: Kind) -> Self {
        Self::new(
            Operation {
                id: OperationId::next(),
                owner,
                kind,
            },
            None,
        )
    }

    /// The operation's identity.
    pub fn id(&self) -> OperationId {
        self.0.op.id
    }

    /// The owning session.
    pub fn owner(&self) -> SessionId {
        self.0.op.owner
    }

    /// What it is.
    pub fn kind(&self) -> &Kind {
        &self.0.op.kind
    }

    /// Settlement will reach a supervisor.
    pub fn is_tracked(&self) -> bool {
        self.0.sink.get().is_some()
    }

    /// A supervisor registered while this untracked guard was running: its
    /// settlement now reaches `sink`. The supervisor never admitted the
    /// operation, so it is reported [`Settle::Unknown`] first, a blocker the
    /// last clone's `Finished` retires (the producer's settlement is the
    /// positive reconciliation) and that stays if the outcome is already
    /// unknown. No-op once a sink is attached.
    pub(crate) fn adopt(&self, sink: &ActivitySink) {
        if self.0.sink.set(sink.clone()).is_err() {
            return;
        }
        sink.settle(Settle::Unknown {
            op: self.id(),
            owner: self.owner(),
            failure: format!(
                "{} ({:?}) started before the activity supervisor registered",
                self.id(),
                self.kind()
            ),
        });
    }

    /// Admit a child operation with the same owner and sink but its own
    /// lifetime: for work that legitimately outlives this guard (a background
    /// job) and settles on its own. Untracked parents yield untracked
    /// children, and are themselves marked unknown: the child is invisible to
    /// any supervisor that adopts the parent later, so the parent's `Finished`
    /// must not vouch for it.
    pub async fn admit(&self, kind: Kind) -> Result<Activity, Rejected> {
        let sink = self.0.sink.get();
        if sink.is_none() {
            self.unknown(format!("{} handed out untracked work", self.id()));
        }
        admit(sink, self.owner(), kind).await
    }

    /// The last clone will settle with [`Settle::FinishedWithResult`] carrying
    /// `result` instead of a bare `Finished`. Later calls replace the payload;
    /// a prior [`Self::unknown`] wins.
    pub fn set_result(&self, result: JobEvent) {
        let mut o = lock(&self.0.outcome);
        if !matches!(*o, Outcome::Unknown) {
            *o = Outcome::Result(result);
        }
    }

    /// Termination cannot be established: report now, retain the blocker.
    /// Dropping afterwards sends nothing. Idempotent; the first reason wins.
    pub fn unknown(&self, failure: impl Into<String>) {
        let mut o = lock(&self.0.outcome);
        if matches!(*o, Outcome::Unknown) {
            return;
        }
        *o = Outcome::Unknown;
        drop(o);
        if let Some(sink) = self.0.sink.get() {
            sink.settle(Settle::Unknown {
                op: self.id(),
                owner: self.owner(),
                failure: failure.into(),
            });
        }
    }

    /// Tell the supervisor something about this still-running operation
    /// (61 §1.8): delivered to the model under the op's scope. A no-op when
    /// untracked (nothing certified, as with results) or already
    /// [`Self::unknown`] (the owner is reconciling; a wake would race it).
    pub fn notify(&self, event: JobEvent) {
        let Some(sink) = self.0.sink.get() else {
            return;
        };
        if matches!(*lock(&self.0.outcome), Outcome::Unknown) {
            return;
        }
        sink.notify(Notice::JobIdle {
            op: self.id(),
            event,
        });
    }

    /// Run `fut` with this guard as the task-local [`current`] activity. The
    /// guard lives at least as long as the future. A panic inside `fut` marks
    /// the operation [`Self::unknown`] before resuming the unwind: whatever
    /// the panicking work started without a clone is unaccounted for, so the
    /// last clone must not report `Finished`. An *aborted* task is
    /// indistinguishable from a finished one; callers that abort work they
    /// own call `unknown` first.
    pub fn scope<F: Future>(self, fut: F) -> impl Future<Output = F::Output> {
        use futures::FutureExt as _;
        let guard = self.clone();
        CURRENT.scope(self, async move {
            match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
                Ok(out) => out,
                Err(payload) => {
                    guard.unknown(format!("{} panicked ({:?})", guard.id(), guard.kind()));
                    std::panic::resume_unwind(payload)
                }
            }
        })
    }
}

tokio::task_local! {
    static CURRENT: Activity;
}

/// The activity the current task runs under, if it was entered with
/// [`Activity::scope`]. `None` outside any invocation (RPC paths, tests):
/// such callers admit through [`ActivitySink`] directly.
pub fn current() -> Option<Activity> {
    CURRENT.try_with(Clone::clone).ok()
}

/// A message to another actor carrying the sender's [`current`] activity, so
/// ownership crosses a mailbox the task-local cannot. The handler runs the
/// work under [`Tracked::scope`]; the guard lives until that future settles,
/// which is what keeps a browser/web command counted after its caller timed
/// out (47 §4.2 rule 5). The wrapped message type is unchanged.
#[derive(Clone, Debug)]
pub struct Tracked<M> {
    /// The original message.
    pub message: M,
    /// [`current`] at the sender; `None` from untracked callers.
    pub activity: Option<Activity>,
}

impl<M> Tracked<M> {
    /// Capture [`current`] at the call site.
    pub fn new(message: M) -> Self {
        Self {
            message,
            activity: current(),
        }
    }

    /// Handler side: run `f(message)` under the carried activity (or plainly
    /// when there is none).
    pub async fn scope<F, Fut>(self, f: F) -> Fut::Output
    where
        F: FnOnce(M) -> Fut,
        Fut: Future,
    {
        match self.activity {
            Some(a) => a.scope(f(self.message)).await,
            None => f(self.message).await,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_sink {
    //! A real recipient actor for lifetime/admission tests: records the
    //! ledger transitions it receives, admits or rejects by policy.

    use std::sync::{Arc, Mutex};

    use kameo::actor::{ActorRef, Spawn};
    use kameo::message::{Context, Message};

    use super::*;

    /// Everything the supervisor saw, in order.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Seen {
        Admitted(Operation),
        Finished(OperationId),
        Result(OperationId),
        Unknown(OperationId, String),
        Idle(OperationId),
    }

    #[derive(kameo::Actor)]
    pub(crate) struct Ledger {
        pub(crate) seen: Arc<Mutex<Vec<Seen>>>,
        pub(crate) reject: Option<String>,
    }

    impl Message<Admit> for Ledger {
        type Reply = Admission;
        async fn handle(
            &mut self,
            Admit(op): Admit,
            _: &mut Context<Self, Self::Reply>,
        ) -> Admission {
            if let Some(why) = &self.reject {
                return Admission::Rejected(why.clone());
            }
            lock(&self.seen).push(Seen::Admitted(op));
            Admission::Admitted
        }
    }

    impl Message<Settle> for Ledger {
        type Reply = ();
        async fn handle(&mut self, msg: Settle, _: &mut Context<Self, Self::Reply>) {
            let seen = match msg {
                Settle::Finished { op } => Seen::Finished(op),
                Settle::FinishedWithResult { op, .. } => Seen::Result(op),
                Settle::Unknown { op, failure, .. } => Seen::Unknown(op, failure),
            };
            lock(&self.seen).push(seen);
        }
    }

    impl Message<Notice> for Ledger {
        type Reply = ();
        async fn handle(&mut self, msg: Notice, _: &mut Context<Self, Self::Reply>) {
            let Notice::JobIdle { op, .. } = msg;
            lock(&self.seen).push(Seen::Idle(op));
        }
    }

    /// Spawn a ledger; `(actor, sink, seen)`.
    pub(crate) fn spawn(
        reject: Option<&str>,
    ) -> (ActorRef<Ledger>, ActivitySink, Arc<Mutex<Vec<Seen>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let actor = Ledger::spawn(Ledger {
            seen: seen.clone(),
            reject: reject.map(str::to_owned),
        });
        (actor.clone(), ActivitySink::new(actor), seen)
    }

    /// Snapshot after the actor has drained its mailbox (a round trip orders us after prior tells).
    pub(crate) async fn drained(actor: &ActorRef<Ledger>, seen: &Mutex<Vec<Seen>>) -> Vec<Seen> {
        let _ = actor
            .ask(Admit(Operation {
                id: OperationId(0),
                owner: SessionId::new(),
                kind: Kind::Command("drain"),
            }))
            .await;
        let mut v = lock(seen).clone();
        v.retain(|s| {
            !matches!(
                s,
                Seen::Admitted(Operation {
                    id: OperationId(0),
                    ..
                })
            )
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::test_sink::{Seen, drained, spawn};
    use super::*;

    #[tokio::test]
    async fn clone_outliving_caller_delays_settlement_until_last_drop() {
        let (actor, sink, seen) = spawn(None);
        let owner = SessionId::new();
        let act = sink.admit(owner, Kind::Command("c")).await.unwrap();
        let op = act.id();
        assert_eq!(
            drained(&actor, &seen).await,
            [Seen::Admitted(Operation {
                id: op,
                owner,
                kind: Kind::Command("c")
            })]
        );

        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let detached = act.clone();
        let task = tokio::spawn(detached.scope(async move {
            assert_eq!(current().map(|a| a.id()), Some(op));
            go_rx.await.ok();
        }));
        drop(act);
        assert_eq!(
            drained(&actor, &seen).await.len(),
            1,
            "settled while detached work runs"
        );

        go_tx.send(()).unwrap();
        task.await.unwrap();
        assert_eq!(drained(&actor, &seen).await[1], Seen::Finished(op));
    }

    #[tokio::test]
    async fn result_and_unknown_shape_the_final_settlement() {
        let (actor, sink, seen) = spawn(None);
        let owner = SessionId::new();
        let finished = JobEvent::Finished {
            session: owner,
            id: JobId("j".into()),
            command: String::new(),
            exit: crate::JobExit::Exited(0),
            secs: 0,
            output: String::new(),
            inline: false,
            pty: false,
        };

        let a = sink
            .admit(owner, Kind::Job(JobId("j".into())))
            .await
            .unwrap();
        let (ida, ob) = (a.id(), a.clone());
        a.set_result(finished);
        drop(a);
        drop(ob);

        let u = sink.admit(owner, Kind::Command("u")).await.unwrap();
        let idu = u.id();
        u.unknown("reader lost");
        u.unknown("second reason ignored");
        drop(u);

        let seen = drained(&actor, &seen).await;
        assert_eq!(seen[1], Seen::Result(ida));
        assert_eq!(seen[3], Seen::Unknown(idu, "reader lost".into()));
        assert_eq!(seen.len(), 4, "no Finished after Unknown, no duplicates");
    }

    /// 61 §5 item 8: a notice reaches the ledger only from a tracked guard
    /// whose outcome is not already unknown.
    #[tokio::test]
    async fn notify_reaches_a_tracked_guard_only() {
        let (actor, sink, seen) = spawn(None);
        let owner = SessionId::new();
        let idle = || JobEvent::Idle {
            session: owner,
            id: JobId("j".into()),
            command: String::new(),
            name: None,
            quiet_secs: 60,
            secs: 61,
            nudge: 1,
            user_typed_secs: None,
            output: ">>> ".into(),
        };

        Activity::untracked(owner, Kind::Pty(JobId("j".into()))).notify(idle());

        let a = sink
            .admit(owner, Kind::Pty(JobId("j".into())))
            .await
            .unwrap();
        let op = a.id();
        a.notify(idle());
        a.notify(idle());
        a.unknown("reader lost");
        a.notify(idle());
        drop(a);

        let seen = drained(&actor, &seen).await;
        assert_eq!(&seen[1..3], [Seen::Idle(op), Seen::Idle(op)]);
        assert!(matches!(&seen[3], Seen::Unknown(id, _) if *id == op));
        assert_eq!(
            seen.len(),
            4,
            "nothing after Unknown; untracked sent nothing"
        );
    }

    #[tokio::test]
    async fn rejection_and_dead_sink_fail_closed_but_none_is_uncertified() {
        let (_actor, sink, _) = spawn(Some("paused"));
        let owner = SessionId::new();
        assert_eq!(
            sink.admit(owner, Kind::Command("x")).await.unwrap_err(),
            Rejected("paused".into())
        );

        let (actor, sink, _) = spawn(None);
        actor.stop_gracefully().await.unwrap();
        actor.wait_for_shutdown().await;
        assert!(sink.admit(owner, Kind::Command("x")).await.is_err());

        let a = admit(None, owner, Kind::Command("x")).await.unwrap();
        assert!(!a.is_tracked());
        assert!(
            a.admit(Kind::Command("y"))
                .await
                .is_ok_and(|c| !c.is_tracked())
        );
    }

    #[tokio::test]
    async fn panicking_holder_leaves_unknown_not_finished() {
        let (actor, sink, seen) = spawn(None);
        let a = sink
            .admit(SessionId::new(), Kind::Command("p"))
            .await
            .unwrap();
        let op = a.id();
        let held = a.clone();
        let task = tokio::spawn(held.scope(async { panic!("boom") }));
        assert!(task.await.is_err());
        drop(a);
        let seen = drained(&actor, &seen).await;
        assert!(
            matches!(&seen[1], Seen::Unknown(id, why) if *id == op && why.contains("panicked"))
        );
        assert_eq!(seen.len(), 2, "no Finished after Unknown");
    }

    #[tokio::test]
    async fn tracked_envelope_carries_ownership_across_a_spawn() {
        let (actor, sink, seen) = spawn(None);
        let owner = SessionId::new();
        let a = sink.admit(owner, Kind::Command("web")).await.unwrap();
        let op = a.id();
        let env = a.clone().scope(async { Tracked::new(42u8) }).await;
        drop(a);
        assert_eq!(env.activity.as_ref().map(Activity::id), Some(op));
        assert!(Tracked::new(()).activity.is_none());

        let handler = tokio::spawn(env.scope(|m| async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            (m, current().map(|a| a.id()))
        }));
        assert_eq!(
            drained(&actor, &seen).await.len(),
            1,
            "alive while the handler runs"
        );
        assert_eq!(handler.await.unwrap(), (42, Some(op)));
        assert_eq!(drained(&actor, &seen).await[1], Seen::Finished(op));
    }
}
