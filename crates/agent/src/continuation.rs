//! Continuation contract (47 §§3–4): the typed transitions a session's runner
//! (the native loop, an ACP child) exchanges with [`Agents`], and the private
//! activity ledger behind the single yield decision.
//!
//! Every message here is handled by `Agents` **synchronously** — no await
//! between the last check and the logical commit — so a decision is a
//! linearization point over everything already admitted:
//!
//! - `Agents` admits an [`InputTicket`] for every routed input *before* the
//!   actor `tell` (the queued-child wait included), and the runner
//!   acknowledges consumption with [`InputConsumed`] once the input is
//!   persisted into a turn. Until then it is pending and blocks any nudge.
//! - The runner reports its own busy state with [`TurnEntered`] /
//!   [`TurnReturned`]: `turn: Some` for a model turn, `None` for a
//!   state-owning mini-turn (manual compaction, harvest). A child's model turn
//!   stays counted after it returns until `Agents` has handed its result to
//!   the parent (the parent's ticket is admitted first; no zero-work gap).
//! - Tool/job/browser work arrives through the tools-owned activity protocol
//!   ([`tools::Admit`] / [`tools::Settle`]); the ledger keeps one entry per
//!   admitted operation until its producer settles it, and keeps an
//!   `Unknown` entry forever (its owner reconciles by settling `Finished`).
//! - At a natural stop the loop asks [`DecideYield`]; the reply is a
//!   committed same-turn reservation, never a reusable idle flag.
//!
//! The runner reaches `Agents` by registry name ([`notify`], [`decide`]); a
//! missing supervisor means no decision (finish), never a nudge.

use std::collections::{BTreeMap, HashMap};

use kameo::actor::ActorRef;
use kameo::message::Message;
use runtime::{InputId, SessionId, TurnId};
use tokio_util::sync::CancellationToken;
use tools::{Operation, OperationId};

use crate::AGENT_ACTOR;
use crate::supervisor::Agents;
use crate::todo::{self, MAX_CONTINUE_REMINDERS, TodoSnapshot};

/// Who initiated an input (47 §4.5). Decided by the ingress path, never by
/// content: a human message or a child's initial assignment is `External`
/// and starts a fresh reminder budget; results, peer messages and other
/// harness-made inputs are `Automatic` and keep the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOrigin {
    /// A human message or a child's initial assignment.
    External,
    /// A result, peer message or other harness-made input.
    Automatic,
}

/// One admitted input, carried on `UserInput`/`Input`/`RestartTurn` into the
/// runner. `id` is unique per process; `epoch` is the owning session's control
/// epoch at admission (see [`DecideYield::epoch`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputTicket {
    /// Process-unique admission id; acknowledged by [`InputConsumed::id`].
    pub id: u64,
    /// The session's control epoch when the input was admitted.
    pub epoch: u64,
}

/// `tell` (runner → `Agents`): the session claimed its state. `turn: Some`
/// for a model turn (native `start_turn`, ACP `open`), `None` for a manual
/// compaction/harvest mini-turn. Sent from the actor before the task spawns.
#[derive(Clone, Copy, Debug)]
pub struct TurnEntered {
    /// The session whose state was claimed.
    pub session: SessionId,
    /// The model turn, or `None` for a maintenance mini-turn.
    pub turn: Option<TurnId>,
}

/// `tell` (runner → `Agents`): ticket `id` was persisted into `turn`. Sent
/// right after the store write; pending until then.
#[derive(Clone, Copy, Debug)]
pub struct InputConsumed {
    /// The session the ticket was admitted for.
    pub session: SessionId,
    /// [`InputTicket::id`] of the persisted input.
    pub id: u64,
    /// The turn the input was persisted into.
    pub turn: TurnId,
}

/// `tell` (runner → `Agents`): a queued harness input holding `ticket` was
/// dropped without entering a turn (its A2A request scope was cancelled,
/// 50 §8). Nothing is owed for it: the ticket is released and the scope's
/// work for it is over. Peer exchanges report through
/// [`crate::communication::PeerIntakeFailed`] instead, which also settles the row.
#[derive(Clone, Copy, Debug)]
pub struct InputDropped {
    /// The session the ticket was admitted for.
    pub session: SessionId,
    /// The admission that will never be consumed.
    pub ticket: InputTicket,
}

/// `tell` (runner → `Agents`): the state came back to the actor (`TurnDone`
/// in the loop, `finish` in an ACP child). `turn` matches the
/// [`TurnEntered`]. Pending inputs are **not** retired by this: a queued
/// automatic input still blocks until it is consumed by the next turn.
#[derive(Clone, Copy, Debug)]
pub struct TurnReturned {
    /// The session whose state came back.
    pub session: SessionId,
    /// The turn that returned; `None` for a maintenance mini-turn.
    pub turn: Option<TurnId>,
}

/// `ask` (turn task → `Agents`) → [`YieldDecision`]. Proposed once per
/// natural stop, before finalization and `TurnFinished`; the task waits for
/// the reply from its spawned turn, never from a session handler.
///
/// - `checkpoint`: per-turn counter of proposals (1, 2, …); each value is
///   decided at most once, a duplicate is `Finish`.
/// - `epoch`: the control epoch of the ticket that opened this turn. Cancel,
///   rewind, manual compaction and context edits bump the session's epoch, so
///   a mismatch is a stale candidate. A turn with no ticket must not propose.
/// - `snapshot`: the committed todo state the loop would remind about.
/// - `cancel`: the turn's token; a cancelled turn never continues.
#[derive(Clone, Debug)]
pub struct DecideYield {
    /// The proposing session.
    pub session: SessionId,
    /// The turn at its natural stop.
    pub turn: TurnId,
    /// Per-turn proposal counter, decided at most once.
    pub checkpoint: u64,
    /// Control epoch of the ticket that opened `turn`.
    pub epoch: u64,
    /// The committed todo state a reminder would be rendered from.
    pub snapshot: TodoSnapshot,
    /// The turn's cancellation token.
    pub cancel: CancellationToken,
}

/// `tell` (runner → `Agents`): work under `session` (a remote ACP/node
/// child's tools, a lost connection mid-turn) cannot be proven stopped.
/// Blocks the session's subtree, synchronously on receipt, for the rest of
/// this process (47 §4.2 rule 6): no producer can positively reconcile it,
/// so nothing short of deleting the session clears it. The first reason is
/// kept; duplicates are idempotent.
#[derive(Clone, Debug)]
pub struct SessionUnknown {
    /// The session whose work is in doubt.
    pub session: SessionId,
    /// Why; surfaced to the user and kept as the blocker's reason.
    pub failure: String,
}

/// Reply to [`DecideYield`]. `Continue` is a committed reservation for the
/// same turn: the loop appends `todo::render_reminder(items, attempt)` through
/// its harness-note path and calls the model again, publishing no
/// `TurnFinished`. It must still check `cancel` and its own steering queue
/// before persisting the note: input admitted after the decision finishes
/// the turn instead, so a reminder never lands behind it (the attempt stays
/// counted; a discarded reservation is never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, kameo::Reply)]
pub enum YieldDecision {
    /// End the turn.
    Finish,
    /// Append reminder `attempt` (1-based within the request) and call the model again.
    Continue {
        /// Reminders committed for this request so far, this one included.
        attempt: u8,
    },
}

/// Fire-and-forget to the supervisor by registry name; a missing supervisor
/// is logged (its coverage is gone anyway, so nothing can be certified).
pub async fn notify<M>(m: M)
where
    M: Send + 'static,
    Agents: Message<M, Reply = ()>,
{
    match ActorRef::<Agents>::lookup(AGENT_ACTOR) {
        Ok(Some(a)) => {
            let _ = a.tell(m).await;
        }
        _ => tracing::warn!("continuation: agent supervisor not registered"),
    }
}

/// Ask the supervisor for a yield decision; unreachable is `Finish`.
pub async fn decide(c: DecideYield) -> YieldDecision {
    match ActorRef::<Agents>::lookup(AGENT_ACTOR) {
        Ok(Some(a)) => a.ask(c).await.unwrap_or(YieldDecision::Finish),
        _ => YieldDecision::Finish,
    }
}

/// What owns a session's state right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Busy {
    #[default]
    Idle,
    /// A model turn is running (or waiting for a decision).
    Turn(TurnId),
    /// Manual compaction / harvest owns the state.
    Maintenance,
    /// A child's model turn returned but its result has not yet been handed
    /// to the parent (or the parent's handoff ran first: see `handed_off`).
    Delivering(TurnId),
}

/// Reminder budget of the current externally initiated request (47 §4.5) and
/// its automatic communication chain (50 §8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Request {
    /// Reminders appended so far.
    reminders: u8,
    /// Todo version at the last reminder; the next needs a newer one.
    latch: Option<u64>,
    /// Automatic (peer/result) inputs consumed since the last external one.
    chain: u32,
}

#[derive(Debug, Default)]
struct SessionEntry {
    parent: Option<SessionId>,
    epoch: u64,
    busy: Busy,
    /// A child turn whose result reached the parent before the child's own
    /// `TurnReturned`.
    handed_off: Option<TurnId>,
    /// Admitted inputs not yet consumed: ticket → (origin, caller's input id,
    /// chain length inherited from the sender at admission).
    pending: BTreeMap<u64, (InputOrigin, Option<InputId>, u32)>,
    /// Last decided `(turn, checkpoint)`.
    checkpoint: Option<(TurnId, u64)>,
    request: Request,
    /// Resumed after an unclean owner exit (47 §4.5): permanently ineligible
    /// in this process.
    resumed: bool,
    /// A runner reported work under this session it cannot prove stopped
    /// ([`SessionUnknown`]); blocks the subtree until the session is deleted.
    unknown: Option<String>,
}

#[derive(Debug)]
enum OpState {
    Running,
    Unknown(String),
}

/// The supervisor-owned activity ledger. Pure state; every method is
/// synchronous so the caller's actor handler is the serialization point.
#[derive(Debug, Default)]
pub struct Ledger {
    sessions: HashMap<SessionId, SessionEntry>,
    /// Admitted (or reported-unknown) operations: owner and state. An owner
    /// the ledger has never seen is unresolved ancestry and blocks every
    /// decision; `Agents` resolves owners from the store on admission.
    ops: HashMap<OperationId, (SessionId, OpState)>,
    next_ticket: u64,
    /// An `ActivitySink` for this supervisor is registered with the tool
    /// registry: tool/job/browser work is admitted here before it runs.
    certified: bool,
}

impl Ledger {
    /// A session is known with its parent link. `resumed` = the row already
    /// had history when this process first touched it: an earlier owner's
    /// jobs, readers and children cannot be proven stopped (47 §4.5) and no
    /// producer reconciliation exists to prove it, so the session never
    /// continues automatically in this process. Nothing clears it — not a
    /// human request, not a shutdown signal. New sessions use fresh accounting.
    pub fn register(&mut self, session: SessionId, parent: Option<SessionId>, resumed: bool) {
        let e = self.sessions.entry(session).or_default();
        e.parent = parent;
        e.resumed |= resumed;
    }

    /// Drop everything about a deleted session (its actor is killed with it).
    pub fn forget(&mut self, session: SessionId) {
        self.sessions.remove(&session);
        self.ops.retain(|_, (owner, _)| *owner != session);
    }

    /// The activity recipient is registered; nothing is certified before this.
    pub fn certify(&mut self) {
        self.certified = true;
    }

    /// Current control epoch of `session`.
    pub fn epoch(&self, session: SessionId) -> u64 {
        self.sessions.get(&session).map_or(0, |e| e.epoch)
    }

    /// Cancel / rewind / manual compaction / context edit: every candidate
    /// and ticket admitted before this is stale for `session`.
    pub fn bump(&mut self, session: SessionId) {
        self.sessions.entry(session).or_default().epoch += 1;
    }

    /// Admit an input for `session` before it is routed. Pending until
    /// [`Self::consumed`]; released with [`Self::release`] when routing failed
    /// or [`Self::withdrawn`] when the user pulled it back by `input` id. An
    /// automatic input continues the session's own chain (a job result, a
    /// self-addressed continuation); one carried from another session goes
    /// through [`Self::admit_chained`].
    pub fn admit(
        &mut self,
        session: SessionId,
        origin: InputOrigin,
        input: Option<InputId>,
    ) -> InputTicket {
        let inherited = self.chain(session);
        self.admit_with(session, origin, input, inherited)
    }

    /// 50 §8: admit an automatic input into `to` that continues the chain of
    /// `from` (a peer message or child result; `from: None` for a remote
    /// sender). The recipient's chain becomes the longer of its own and the
    /// inherited one, plus this link, when the input is consumed. `carried`:
    /// the count another engine's sender sent with it (183 §6.5; 0 locally).
    pub fn admit_chained(
        &mut self,
        to: SessionId,
        input: Option<InputId>,
        from: Option<SessionId>,
        carried: u32,
    ) -> InputTicket {
        let inherited = from.map_or(0, |f| self.chain(f)).max(carried);
        self.admit_with(to, InputOrigin::Automatic, input, inherited)
    }

    fn admit_with(
        &mut self,
        session: SessionId,
        origin: InputOrigin,
        input: Option<InputId>,
        inherited: u32,
    ) -> InputTicket {
        self.next_ticket += 1;
        let e = self.sessions.entry(session).or_default();
        e.pending
            .insert(self.next_ticket, (origin, input, inherited));
        InputTicket {
            id: self.next_ticket,
            epoch: e.epoch,
        }
    }

    /// The input never reached a runner (routing error): nothing is owed.
    pub fn release(&mut self, session: SessionId, ticket: InputTicket) {
        if let Some(e) = self.sessions.get_mut(&session) {
            e.pending.remove(&ticket.id);
        }
    }

    /// A queued steer was withdrawn (`Withdraw` → `Ok`): its ticket is gone
    /// with it. Unknown ids are a no-op (already consumed or never ticketed).
    pub fn withdrawn(&mut self, session: SessionId, input: InputId) {
        if let Some(e) = self.sessions.get_mut(&session) {
            e.pending.retain(|_, (_, id, _)| *id != Some(input));
        }
    }

    /// [`InputConsumed`]: an external input opens a fresh reminder budget
    /// and communication chain; an automatic one continues the longer of the
    /// session's own chain and the one it inherited at admission, one link
    /// longer. It does not clear `resumed`: a human asking again proves
    /// nothing about processes an earlier owner lost (47 §4.5).
    pub fn consumed(&mut self, m: InputConsumed) {
        let Some(e) = self.sessions.get_mut(&m.session) else {
            return;
        };
        match e.pending.remove(&m.id) {
            Some((InputOrigin::External, _, _)) => e.request = Request::default(),
            Some((InputOrigin::Automatic, _, inherited)) => {
                e.request.chain = e.request.chain.max(inherited).saturating_add(1);
            }
            None => {}
        }
    }

    /// 50 §8: links in the automatic communication chain behind `session`'s
    /// current request (peer messages, replies, child/job results), inherited
    /// across sessions through [`Self::admit_chained`]; 0 after a fresh
    /// external input.
    pub fn chain(&self, session: SessionId) -> u32 {
        self.sessions.get(&session).map_or(0, |e| e.request.chain)
    }

    /// 50 §8: an automatic input from `from` into `to` would exceed
    /// [`crate::communication::CHAIN_MAX`] links; the supervisor refuses it
    /// visibly instead of admitting it. `from: None` is a non-session sender
    /// (remote peer), which contributes no chain of its own; `carried` is the
    /// count a sender on another engine sent (183 §6.5).
    pub fn chain_exhausted_for(
        &self,
        from: Option<SessionId>,
        to: SessionId,
        carried: u32,
    ) -> bool {
        let inherited = from.map_or(0, |f| self.chain(f)).max(carried);
        inherited.max(self.chain(to)) >= crate::communication::CHAIN_MAX
    }

    /// [`Self::chain_exhausted_for`] for a session's own continuation.
    pub fn chain_exhausted(&self, session: SessionId) -> bool {
        self.chain_exhausted_for(None, session, 0)
    }

    /// The model turn owning `session`'s state, or still delivering its result.
    pub fn turn_of(&self, session: SessionId) -> Option<TurnId> {
        match self.sessions.get(&session)?.busy {
            Busy::Turn(t) | Busy::Delivering(t) => Some(t),
            Busy::Idle | Busy::Maintenance => None,
        }
    }

    /// Nothing owns `session`'s state.
    pub fn is_idle(&self, session: SessionId) -> bool {
        self.sessions
            .get(&session)
            .is_none_or(|e| e.busy == Busy::Idle)
    }

    /// Admitted inputs of `session` not yet consumed.
    pub fn pending_count(&self, session: SessionId) -> usize {
        self.sessions.get(&session).map_or(0, |e| e.pending.len())
    }

    /// Admitted (running or unknown) operations owned by `session` itself.
    pub fn operations_of(&self, session: SessionId) -> Vec<OperationId> {
        self.ops
            .iter()
            .filter(|(_, (owner, _))| *owner == session)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Owner of an admitted or unknown operation.
    pub fn op_owner(&self, op: OperationId) -> Option<SessionId> {
        self.ops.get(&op).map(|(owner, _)| *owner)
    }

    /// [`TurnEntered`].
    pub fn entered(&mut self, m: TurnEntered) {
        let e = self.sessions.entry(m.session).or_default();
        e.busy = match m.turn {
            Some(t) => Busy::Turn(t),
            None => Busy::Maintenance,
        };
    }

    /// [`TurnReturned`]. Matched against the current owner: a stale return
    /// (an older turn's, after a newer one entered) changes nothing. A
    /// child's model turn waits for [`Self::handed_off`].
    pub fn returned(&mut self, m: TurnReturned) {
        let Some(e) = self.sessions.get_mut(&m.session) else {
            return;
        };
        e.busy = match (m.turn, e.busy) {
            (Some(t), Busy::Turn(cur)) if t == cur => {
                if e.parent.is_some() && e.handed_off.take() != Some(t) {
                    Busy::Delivering(t)
                } else {
                    Busy::Idle
                }
            }
            (None, Busy::Maintenance) => Busy::Idle,
            (_, cur) => {
                tracing::debug!(session = %m.session, "stale TurnReturned {:?} while {cur:?}", m.turn);
                cur
            }
        };
    }

    /// The parent's ticket (or waiter) now owns the result of `child`'s
    /// `turn`. Called from the `TurnFinished` handoff, before or after the
    /// child's own return; matched by turn so a later turn started from
    /// queued input cannot inherit an earlier turn's handoff.
    pub fn handed_off(&mut self, child: SessionId, turn: TurnId) {
        let Some(e) = self.sessions.get_mut(&child) else {
            return;
        };
        match e.busy {
            Busy::Delivering(t) if t == turn => e.busy = Busy::Idle,
            Busy::Turn(t) if t == turn => e.handed_off = Some(turn),
            _ => {}
        }
    }

    /// [`tools::Admit`]. Idempotent by identity: a re-admitted id keeps its
    /// state (an `Unknown` is not downgraded to running).
    pub fn admit_op(&mut self, op: &Operation) {
        self.ops
            .entry(op.id)
            .or_insert((op.owner, OpState::Running));
    }

    /// `Settle::Finished` (also `FinishedWithResult` once the result's ticket
    /// is admitted). Idempotent; clears an `Unknown` too — a settlement from
    /// the producer is the positive reconciliation 47 §4.2 rule 8 asks for.
    pub fn retire_op(&mut self, op: OperationId) {
        self.ops.remove(&op);
    }

    /// `Settle::Unknown`: keep the blocker, remember why. An id that was never
    /// admitted (work adopted at registration) is inserted with the owner the
    /// producer reports.
    pub fn unknown_op(&mut self, op: OperationId, owner: SessionId, failure: String) {
        self.ops.insert(op, (owner, OpState::Unknown(failure)));
    }

    /// The ledger knows `session` (its parent link is resolved).
    pub fn knows(&self, session: SessionId) -> bool {
        self.sessions.contains_key(&session)
    }

    /// [`SessionUnknown`]: the first reason is kept.
    pub fn session_unknown(&mut self, m: SessionUnknown) {
        self.sessions
            .entry(m.session)
            .or_default()
            .unknown
            .get_or_insert(m.failure);
    }

    /// `session` and every descendant known to the ledger.
    pub fn subtree(&self, session: SessionId) -> Vec<SessionId> {
        let mut out = vec![session];
        let mut i = 0;
        while i < out.len() {
            let p = out[i];
            out.extend(
                self.sessions
                    .iter()
                    .filter(|(_, e)| e.parent == Some(p))
                    .map(|(id, _)| *id),
            );
            i += 1;
        }
        out
    }

    /// Owned or unresolved work under `session`: pending inputs, busy
    /// descendants, admitted or unknown operations. `None` = nothing.
    pub fn activity(&self, session: SessionId) -> Option<String> {
        for d in self.subtree(session) {
            let Some(e) = self.sessions.get(&d) else {
                continue;
            };
            if let Some(why) = &e.unknown {
                return Some(format!("{d} unknown: {why}"));
            }
            if let Some((id, (origin, _, _))) = e.pending.iter().next() {
                return Some(format!("input {id} ({origin:?}) pending on {d}"));
            }
            if d != session && e.busy != Busy::Idle {
                return Some(format!("{d} busy: {:?}", e.busy));
            }
        }
        let mine = self.subtree(session);
        self.ops.iter().find_map(|(id, (owner, state))| {
            // An owner the ledger has never seen (no row resolved for it) is
            // unresolved ancestry: it blocks every session until it settles.
            let blocks = mine.contains(owner) || !self.sessions.contains_key(owner);
            blocks.then(|| match state {
                OpState::Running => format!("operation {id} of {owner} running"),
                OpState::Unknown(why) => format!("operation {id} of {owner} unknown: {why}"),
            })
        })
    }

    /// The decision (47 §4.3). `blocked` is the caller's own evidence
    /// (approvals, peer waiters, event-projected running descendants).
    /// Consumes the checkpoint exactly once; on `Continue` the reminder count
    /// and progress latch are committed before returning.
    pub fn decide(&mut self, c: &DecideYield, blocked: bool) -> YieldDecision {
        let certified = self.certified;
        let Some(e) = self.sessions.get_mut(&c.session) else {
            return YieldDecision::Finish;
        };
        if e.busy != Busy::Turn(c.turn) || e.epoch != c.epoch || c.cancel.is_cancelled() {
            return YieldDecision::Finish;
        }
        if e.checkpoint
            .is_some_and(|(t, cp)| t == c.turn && cp >= c.checkpoint)
        {
            return YieldDecision::Finish;
        }
        e.checkpoint = Some((c.turn, c.checkpoint));
        if !certified
            || e.resumed
            || blocked
            || !todo::actionable(&c.snapshot.items)
            || e.request.reminders >= MAX_CONTINUE_REMINDERS
            || e.request.latch.is_some_and(|v| c.snapshot.version <= v)
        {
            return YieldDecision::Finish;
        }
        if let Some(why) = self.activity(c.session) {
            tracing::debug!(session = %c.session, "yield: {why}");
            return YieldDecision::Finish;
        }
        let e = self.sessions.get_mut(&c.session).expect("checked above");
        e.request.reminders += 1;
        e.request.latch = Some(c.snapshot.version);
        YieldDecision::Continue {
            attempt: e.request.reminders,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todo::{Status, Todo};
    use tools::Kind;

    fn plan(version: u64) -> TodoSnapshot {
        TodoSnapshot {
            version,
            items: vec![Todo {
                text: "x".into(),
                status: Status::Pending,
                reason: None,
            }],
            ..TodoSnapshot::default()
        }
    }

    fn candidate(session: SessionId, turn: TurnId, checkpoint: u64, epoch: u64) -> DecideYield {
        DecideYield {
            session,
            turn,
            checkpoint,
            epoch,
            snapshot: plan(1),
            cancel: CancellationToken::new(),
        }
    }

    /// A certified root session with one consumed external input in turn `t`.
    fn quiescent() -> (Ledger, SessionId, TurnId) {
        let mut l = Ledger::default();
        l.certify();
        let s = SessionId::new();
        l.register(s, None, false);
        let t = TurnId::new();
        let ticket = l.admit(s, InputOrigin::External, None);
        l.entered(TurnEntered {
            session: s,
            turn: Some(t),
        });
        l.consumed(InputConsumed {
            session: s,
            id: ticket.id,
            turn: t,
        });
        (l, s, t)
    }

    fn op(owner: SessionId) -> Operation {
        Operation {
            id: tools::Activity::untracked(owner, Kind::Command("t")).id(),
            owner,
            kind: Kind::Command("t"),
        }
    }

    #[test]
    fn continues_only_at_verified_quiescence_and_once_per_checkpoint() {
        let (mut l, s, t) = quiescent();
        let c = candidate(s, t, 1, 0);
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 1 });
        // Duplicate decision for the same checkpoint: finish, no second reminder.
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        // Next checkpoint without todo progress: latched.
        let mut c2 = candidate(s, t, 2, 0);
        assert_eq!(l.decide(&c2, false), YieldDecision::Finish);
        // Progress unlocks, the ceiling holds across automatic turns.
        c2.checkpoint = 3;
        c2.snapshot = plan(2);
        assert_eq!(l.decide(&c2, false), YieldDecision::Continue { attempt: 2 });
        let r = l.admit(s, InputOrigin::Automatic, None);
        l.returned(TurnReturned {
            session: s,
            turn: Some(t),
        });
        let t2 = TurnId::new();
        l.entered(TurnEntered {
            session: s,
            turn: Some(t2),
        });
        l.consumed(InputConsumed {
            session: s,
            id: r.id,
            turn: t2,
        });
        let mut c3 = candidate(s, t2, 1, 0);
        c3.snapshot = plan(3);
        assert_eq!(l.decide(&c3, false), YieldDecision::Continue { attempt: 3 });
        let mut c4 = candidate(s, t2, 2, 0);
        c4.snapshot = plan(4);
        assert_eq!(l.decide(&c4, false), YieldDecision::Finish, "ceiling");
        // A fresh human request resets the budget.
        let h = l.admit(s, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: s,
            id: h.id,
            turn: t2,
        });
        let mut c5 = candidate(s, t2, 3, 0);
        c5.snapshot = plan(5);
        assert_eq!(l.decide(&c5, false), YieldDecision::Continue { attempt: 1 });
    }

    #[test]
    fn pending_input_and_stale_candidates_finish() {
        let (mut l, s, t) = quiescent();
        // Admitted but not consumed input wins over the reminder.
        let p = l.admit(s, InputOrigin::Automatic, None);
        assert_eq!(
            l.decide(&candidate(s, t, 1, 0), false),
            YieldDecision::Finish
        );
        l.consumed(InputConsumed {
            session: s,
            id: p.id,
            turn: t,
        });
        // Cancel bumps the epoch: the old candidate is stale even though the
        // turn is still the current one.
        l.bump(s);
        assert_eq!(
            l.decide(&candidate(s, t, 2, 0), false),
            YieldDecision::Finish
        );
        assert_eq!(
            l.decide(&candidate(s, t, 3, 1), false),
            YieldDecision::Continue { attempt: 1 }
        );
        // Cancelled token, wrong turn, released ticket, maintenance owner.
        let mut c = candidate(s, t, 4, 1);
        c.snapshot = plan(9);
        c.cancel.cancel();
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        assert_eq!(
            l.decide(&candidate(s, TurnId::new(), 5, 1), false),
            YieldDecision::Finish
        );
        let lost = l.admit(s, InputOrigin::Automatic, None);
        l.release(s, lost);
        let mut c = candidate(s, t, 6, 1);
        c.snapshot = plan(10);
        assert_eq!(l.decide(&c, true), YieldDecision::Finish, "caller evidence");
        assert_eq!(
            l.decide(&candidate(s, t, 6, 1), false),
            YieldDecision::Finish,
            "checkpoint 6 was consumed by the blocked decision"
        );
    }

    #[test]
    fn queued_children_grandchildren_and_delivery_block_the_parent() {
        let (mut l, s, t) = quiescent();
        let child = SessionId::new();
        l.register(child, Some(s), false);
        // Waiting for a permit: the first-turn ticket is pending.
        let first = l.admit(child, InputOrigin::External, None);
        assert_eq!(
            l.decide(&candidate(s, t, 1, 0), false),
            YieldDecision::Finish
        );
        let ct = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(ct),
        });
        l.consumed(InputConsumed {
            session: child,
            id: first.id,
            turn: ct,
        });
        // Running child.
        assert_eq!(
            l.decide(&candidate(s, t, 2, 0), false),
            YieldDecision::Finish
        );
        // Idle child with a busy grandchild.
        let grand = SessionId::new();
        l.register(grand, Some(child), false);
        l.entered(TurnEntered {
            session: grand,
            turn: None,
        });
        l.returned(TurnReturned {
            session: child,
            turn: Some(ct),
        });
        l.handed_off(child, ct);
        assert_eq!(
            l.decide(&candidate(s, t, 3, 0), false),
            YieldDecision::Finish
        );
        l.returned(TurnReturned {
            session: grand,
            turn: None,
        });
        assert_eq!(
            l.decide(&candidate(s, t, 4, 0), false),
            YieldDecision::Continue { attempt: 1 }
        );

        // Child returned, result not yet handed to the parent: still counted,
        // in either order of return and handoff.
        let ct2 = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(ct2),
        });
        l.returned(TurnReturned {
            session: child,
            turn: Some(ct2),
        });
        let mut c = candidate(s, t, 5, 0);
        c.snapshot = plan(2);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        let r = l.admit(s, InputOrigin::Automatic, None);
        l.handed_off(child, ct2);
        c.checkpoint = 6;
        assert_eq!(l.decide(&c, false), YieldDecision::Finish, "result pending");
        l.consumed(InputConsumed {
            session: s,
            id: r.id,
            turn: t,
        });
        c.checkpoint = 7;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 2 });
        let ct3 = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(ct3),
        });
        // A late handoff for the *previous* turn must not release this one.
        l.handed_off(child, ct2);
        l.handed_off(child, ct3);
        c.checkpoint = 8;
        c.snapshot = plan(3);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish, "still running");
        l.returned(TurnReturned {
            session: child,
            turn: Some(ct3),
        });
        c.checkpoint = 9;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 3 });
        // Handoff-before-return for an older turn does not pre-release a newer one.
        let ct4 = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(ct4),
        });
        l.handed_off(child, ct3);
        l.returned(TurnReturned {
            session: child,
            turn: Some(ct4),
        });
        let h = l.admit(s, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: s,
            id: h.id,
            turn: t,
        });
        c.checkpoint = 10;
        assert_eq!(
            l.decide(&c, false),
            YieldDecision::Finish,
            "ct4 undelivered"
        );
        l.handed_off(child, ct4);
        c.checkpoint = 11;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 1 });
    }

    #[test]
    fn operations_unknown_and_orphans_block_but_other_roots_do_not() {
        let (mut l, s, t) = quiescent();
        let other = SessionId::new();
        l.register(other, None, false);
        l.entered(TurnEntered {
            session: other,
            turn: Some(TurnId::new()),
        });
        let theirs = op(other);
        l.admit_op(&theirs);
        assert_eq!(
            l.decide(&candidate(s, t, 1, 0), false),
            YieldDecision::Continue { attempt: 1 }
        );
        let mine = op(s);
        l.admit_op(&mine);
        let mut c = candidate(s, t, 2, 0);
        c.snapshot = plan(2);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.retire_op(mine.id);
        l.retire_op(mine.id); // duplicate settlement is idempotent
        c.checkpoint = 3;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 2 });
        // Unknown outcome stays until its owner settles; fresh input does not clear it.
        l.unknown_op(mine.id, s, "reader lost".into());
        let h = l.admit(s, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: s,
            id: h.id,
            turn: t,
        });
        c.checkpoint = 4;
        c.snapshot = plan(3);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.retire_op(mine.id);
        c.checkpoint = 5;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 1 });
        // An owner the ledger never saw has unresolved ancestry: blocks everyone.
        let orphan = op(SessionId::new());
        l.admit_op(&orphan);
        c.checkpoint = 6;
        c.snapshot = plan(4);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.forget(orphan.owner);
        l.retire_op(orphan.id);
        c.checkpoint = 7;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 2 });
        // Reported unknown without ever being admitted (a call adopted at
        // registration): scoped to its owner, so another root's stays
        // theirs while ours blocks until its producer settles.
        let stray = OperationId(u64::MAX);
        l.unknown_op(stray, other, "started before registration".into());
        c.checkpoint = 8;
        c.snapshot = plan(5);
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 3 });
        let h = l.admit(s, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: s,
            id: h.id,
            turn: t,
        });
        l.unknown_op(
            OperationId(u64::MAX - 1),
            s,
            "started before registration".into(),
        );
        c.checkpoint = 9;
        c.snapshot = plan(6);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.retire_op(OperationId(u64::MAX - 1));
        c.checkpoint = 10;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 1 });
    }

    #[test]
    fn uncertified_resumed_or_planless_sessions_never_continue() {
        let (mut l, s, t) = quiescent();
        l.certified = false;
        assert_eq!(
            l.decide(&candidate(s, t, 1, 0), false),
            YieldDecision::Finish
        );
        l.certify();
        let mut c = candidate(s, t, 2, 0);
        c.snapshot.items[0].status = Status::Blocked;
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        // Resumed after an unclean exit: a human asking again proves nothing.
        let r = SessionId::new();
        l.register(r, None, true);
        let rt = TurnId::new();
        let h = l.admit(r, InputOrigin::External, None);
        l.entered(TurnEntered {
            session: r,
            turn: Some(rt),
        });
        l.consumed(InputConsumed {
            session: r,
            id: h.id,
            turn: rt,
        });
        assert_eq!(
            l.decide(&candidate(r, rt, 1, 0), false),
            YieldDecision::Finish
        );
        // A fresh session in the same process is unaffected.
        let (mut l2, s2, t2) = quiescent();
        assert_eq!(
            l2.decide(&candidate(s2, t2, 1, 0), false),
            YieldDecision::Continue { attempt: 1 }
        );
    }

    #[test]
    fn stale_return_and_session_unknown() {
        let (mut l, s, t) = quiescent();
        let child = SessionId::new();
        l.register(child, Some(s), false);
        let c1 = TurnId::new();
        let c2 = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(c1),
        });
        l.entered(TurnEntered {
            session: child,
            turn: Some(c2),
        });
        // An older turn's return (or a maintenance return) cannot idle the newer turn.
        l.returned(TurnReturned {
            session: child,
            turn: Some(c1),
        });
        l.returned(TurnReturned {
            session: child,
            turn: None,
        });
        assert_eq!(
            l.decide(&candidate(s, t, 1, 0), false),
            YieldDecision::Finish
        );
        l.returned(TurnReturned {
            session: child,
            turn: Some(c2),
        });
        l.handed_off(child, c2);
        assert_eq!(
            l.decide(&candidate(s, t, 2, 0), false),
            YieldDecision::Continue { attempt: 1 }
        );
        // A runner-reported unknown under a descendant blocks the root for
        // good: neither fresh input nor the child idling clears it; only
        // deleting the child does.
        l.session_unknown(SessionUnknown {
            session: child,
            failure: "remote closed".into(),
        });
        let mut c = candidate(s, t, 3, 0);
        c.snapshot = plan(2);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        let h = l.admit(s, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: s,
            id: h.id,
            turn: t,
        });
        let c3 = TurnId::new();
        l.entered(TurnEntered {
            session: child,
            turn: Some(c3),
        });
        l.returned(TurnReturned {
            session: child,
            turn: Some(c3),
        });
        l.handed_off(child, c3);
        c.checkpoint = 4;
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.forget(child);
        c.checkpoint = 5;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 1 });
        // Re-admitting an unknown id does not downgrade it.
        let o = op(s);
        l.unknown_op(o.id, s, "lost".into());
        l.admit_op(&o);
        c.checkpoint = 6;
        c.snapshot = plan(3);
        assert_eq!(l.decide(&c, false), YieldDecision::Finish);
        l.retire_op(o.id);
        c.checkpoint = 7;
        assert_eq!(l.decide(&c, false), YieldDecision::Continue { attempt: 2 });
    }

    /// 50 §8: automatic (peer/result) consumptions lengthen the request's
    /// chain up to `CHAIN_MAX`; only a consumed external input resets it.
    /// Admission alone counts nothing.
    #[test]
    fn chain_counts_consumed_automatic_inputs_until_external_reset() {
        let (mut l, s, t) = quiescent();
        let consume = |l: &mut Ledger, origin: InputOrigin| {
            let ticket = l.admit(s, origin, None);
            l.consumed(InputConsumed {
                session: s,
                id: ticket.id,
                turn: t,
            });
        };
        for _ in 0..crate::communication::CHAIN_MAX - 1 {
            consume(&mut l, InputOrigin::Automatic);
        }
        assert!(!l.chain_exhausted(s));
        // Pending, not consumed: no effect.
        l.admit(s, InputOrigin::Automatic, None);
        assert!(!l.chain_exhausted(s));
        consume(&mut l, InputOrigin::Automatic);
        assert!(l.chain_exhausted(s));
        consume(&mut l, InputOrigin::External);
        assert!(!l.chain_exhausted(s));
        assert!(!l.chain_exhausted(SessionId::new()));
    }

    /// 50 §8: the chain belongs to the initiating request, not to one
    /// session: a message from a session deep in a chain carries that depth
    /// to its recipient, a bounce back carries it further, and the bound is
    /// checked against the longer of sender and recipient before admission.
    /// A fresh external input in the recipient resets only the recipient.
    #[test]
    fn chain_is_inherited_across_sessions() {
        let max = crate::communication::CHAIN_MAX;
        let mut l = Ledger::default();
        let (a, b) = (SessionId::new(), SessionId::new());
        l.register(a, None, false);
        l.register(b, None, false);
        let t = TurnId::new();
        let relay = |l: &mut Ledger, from: SessionId, to: SessionId| {
            let ticket = l.admit_chained(to, None, Some(from), 0);
            l.consumed(InputConsumed {
                session: to,
                id: ticket.id,
                turn: t,
            });
        };
        // a self-continues max-2 times, then messages b once: b inherits.
        for _ in 0..max - 2 {
            let ticket = l.admit(a, InputOrigin::Automatic, None);
            l.consumed(InputConsumed {
                session: a,
                id: ticket.id,
                turn: t,
            });
        }
        assert_eq!(l.chain(a), max - 2);
        relay(&mut l, a, b);
        assert_eq!(l.chain(b), max - 1);
        assert!(!l.chain_exhausted_for(Some(b), a, 0));
        // b answers a: a is now at the bound; nothing more may be relayed either way.
        relay(&mut l, b, a);
        assert_eq!(l.chain(a), max);
        assert!(l.chain_exhausted_for(Some(a), b, 0) && l.chain_exhausted_for(Some(b), a, 0));
        // A remote sender contributes nothing, but the exhausted recipient still refuses.
        assert!(l.chain_exhausted_for(None, a, 0) && !l.chain_exhausted_for(None, b, 0));
        // 183 §6.5: unless it carries its own engine's count.
        assert!(l.chain_exhausted_for(None, b, max));
        // A human input in a resets a alone; b keeps its inherited depth.
        let fresh = l.admit(a, InputOrigin::External, None);
        l.consumed(InputConsumed {
            session: a,
            id: fresh.id,
            turn: t,
        });
        assert_eq!((l.chain(a), l.chain(b)), (0, max - 1));
        assert!(!l.chain_exhausted_for(Some(a), b, 0));
    }

    /// The read API the network state binds request scopes with: the owning
    /// turn survives the return of a child turn until its handoff, operations
    /// list by owner, and pending counts drop only on consumption.
    #[test]
    fn busy_and_operation_queries_follow_the_ledger() {
        let mut l = Ledger::default();
        let (root, child) = (SessionId::new(), SessionId::new());
        l.register(root, None, false);
        l.register(child, Some(root), false);
        assert!(l.is_idle(child) && l.turn_of(child).is_none());
        let t = TurnId::new();
        let ticket = l.admit(child, InputOrigin::External, None);
        assert_eq!(l.pending_count(child), 1);
        l.entered(TurnEntered {
            session: child,
            turn: Some(t),
        });
        assert_eq!(l.turn_of(child), Some(t));
        assert!(!l.is_idle(child));
        let o = op(child);
        l.admit_op(&o);
        assert_eq!(l.operations_of(child), vec![o.id]);
        assert!(l.operations_of(root).is_empty());
        assert_eq!(l.op_owner(o.id), Some(child));
        l.consumed(InputConsumed {
            session: child,
            id: ticket.id,
            turn: t,
        });
        assert_eq!(l.pending_count(child), 0);
        // A child's return keeps the turn owned until the parent holds the result.
        l.returned(TurnReturned {
            session: child,
            turn: Some(t),
        });
        assert_eq!(l.turn_of(child), Some(t));
        l.handed_off(child, t);
        assert!(l.turn_of(child).is_none() && l.is_idle(child));
        l.retire_op(o.id);
        assert!(l.operations_of(child).is_empty() && l.op_owner(o.id).is_none());
    }
}
