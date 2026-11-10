//! Design doc 109 §4.4 / §6.2 / §7.2 (W3): what the `waits` and `questions`
//! rows of open turns become at boot, and what a resumed call asks the
//! supervisor for.
//!
//! - [`Agents::rebuild_waits`] (boot, after the reaper, before `resume_scan`):
//!   every `peer_reply` wait of an open turn is re-created in `Agents.waits`
//!   with its route's generation — a reply that lands after the restart
//!   settles it as before — and its receiver is parked in [`REARMED`] for the
//!   resumed call; the `approval` rows of `planned` calls are settled: a
//!   `pending` one is `superseded` (its registry invocation died with the
//!   process, §6.2), an `answered` one is handed to the approval actor as a
//!   [`HonourAnswer`] so the replay does not ask again.
//! - [`Agents::deliver_spared_results`]: the boot pass of §7.2, run once by
//!   the `ActivityRegistered` handler after `resume_scan` and the ACP
//!   respawn — every child result `fail_interrupted` spared is enqueued into
//!   its parent through the same intake as a fresh one; the dedup key makes
//!   a double delivery impossible.
//! - [`ResumeQuestion`] / [`ResumeWait`]: what `loop_/resume_calls.rs` asks
//!   for a `started` human-wait / `send_message{await_reply}` call.

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::{SessionId, TurnId};
use tokio::sync::oneshot;
use tools::approval::HonourAnswer;
use tools::{Decision, QuestionKind, QuestionRow, QuestionState, WaitKind, WaitRow};
use uuid::Uuid;

use super::Agents;
use crate::communication::{PeerAddress, PeerMessageState, PeerReceipt, PeerRecord};
use crate::durability::CallState;
use crate::store::durability::OpenTurnRow;

/// Receivers of the waits re-armed at boot, by exchange id, until the
/// resumed call takes them ([`ResumeWait`]). A reply that arrives in between
/// is buffered in the oneshot.
// ponytail: process-global; exchange ids are uuids, so several `Agents` in one test binary cannot collide.
static REARMED: LazyLock<Mutex<HashMap<Uuid, oneshot::Receiver<PeerReceipt>>>> =
    LazyLock::new(Default::default);

/// The map.
fn rearmed() -> parking_lot::MutexGuard<'static, HashMap<Uuid, oneshot::Receiver<PeerReceipt>>> {
    REARMED.lock()
}

/// `ask` → `Result<Option<QuestionRow>, String>`: the newest `questions`
/// row of `call_id` in `session` (§6.2).
pub(crate) struct ResumeQuestion {
    /// The session whose turn is being resumed.
    pub session: SessionId,
    /// The `started` human-wait call.
    pub call_id: String,
}

/// `ask` → `Result<ResumedWait, String>`: what a `started`
/// `send_message{await_reply}` call blocks on after a restart (§4.4).
pub(crate) struct ResumeWait {
    /// The session whose turn is being resumed.
    pub session: SessionId,
    /// The open turn being resumed.
    pub turn: TurnId,
    /// The waiting call.
    pub call_id: String,
}

/// Reply to [`ResumeWait`].
pub(crate) struct ResumedWait {
    /// The `waits` row, if the call had written one.
    pub row: Option<WaitRow>,
    /// Every awaited exchange's current receipt (`replied` carries the text).
    pub receipts: Vec<PeerReceipt>,
    /// Live waiters for the receipts still open, by exchange id.
    pub pending: HashMap<Uuid, oneshot::Receiver<PeerReceipt>>,
    /// Spared `result:` texts of a `child_result` wait, consumed here.
    pub child_results: Vec<String>,
}

/// The exchange ids a `peer_reply` row names: `payload.request_ids`, else
/// its `target`.
fn request_ids(w: &WaitRow) -> Vec<Uuid> {
    let listed: Vec<Uuid> = w
        .payload
        .get("request_ids")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if listed.is_empty() {
        w.target.parse().into_iter().collect()
    } else {
        listed
    }
}

/// The live approval actor, if any (the honoured answers' destination).
fn approval_actor() -> Option<ActorRef<tools::Approval>> {
    ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)
        .ok()
        .flatten()
        .filter(ActorRef::is_alive)
}

/// What a rebuild did, for the one log line.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    rearmed: usize,
    honoured: usize,
    superseded: usize,
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, o: Self) {
        self.rearmed += o.rearmed;
        self.honoured += o.honoured;
        self.superseded += o.superseded;
    }
}

impl Counts {
    fn log(self) {
        if self.rearmed + self.honoured + self.superseded > 0 {
            tracing::info!(
                rearmed = self.rearmed,
                honoured = self.honoured,
                superseded = self.superseded,
                "109: waits and approvals rebuilt from the journal"
            );
        }
    }
}

impl Agents {
    /// Boot (§4.6 step 10): rebuild the live waiters and settle the dead
    /// approvals of every open turn `here` from their rows; see the module doc.
    /// `open` is the boot pass's one `open_turns(Scope::Here)` (146 D5).
    pub(super) async fn rebuild_waits(&mut self, open: &[OpenTurnRow]) {
        let approval = approval_actor();
        let mut n = Counts::default();
        for (session, turn, _) in open {
            let (session, turn) = (*session, *turn);
            n += self
                .rebuild_waits_of(session, turn, approval.as_ref())
                .await;
        }
        n.log();
    }

    /// 139 §7.3: [`Self::rebuild_waits`] for one session's open turn — the
    /// first step of a resume after a transfer / takeover, on the new owner,
    /// from the replicated `questions` / `waits` rows.
    pub(super) async fn rebuild_waits_for(&mut self, session: SessionId) {
        let turn = match self.store.open_turn(session).await {
            Ok(Some((turn, _))) => turn,
            Ok(None) => return,
            Err(e) => return tracing::error!(%session, "109: open_turn: {e:#}"),
        };
        let approval = approval_actor();
        self.rebuild_waits_of(session, turn, approval.as_ref())
            .await
            .log();
    }

    /// One open turn's rows: approvals of `planned` calls settled, `peer_reply`
    /// waits re-armed. `approval` is the actor the honoured answers go to.
    async fn rebuild_waits_of(
        &mut self,
        session: SessionId,
        turn: TurnId,
        approval: Option<&ActorRef<tools::Approval>>,
    ) -> Counts {
        let mut n = Counts::default();
        let calls = self.store.calls_of(session, turn).await.unwrap_or_default();
        for call in calls.iter().filter(|c| c.state == CallState::Planned) {
            let rows = self
                .store
                .questions_of_call(session, &call.call_id)
                .await
                .unwrap_or_default();
            for row in rows
                .into_iter()
                .filter(|r| r.kind == QuestionKind::Approval)
            {
                match row.state {
                    QuestionState::Pending => {
                        // §6.2: the gate's invocation died with the process.
                        match self
                            .store
                            .question_resolve(&row.id, QuestionState::Superseded, None)
                            .await
                        {
                            Ok(true) => n.superseded += 1,
                            Ok(false) => {}
                            Err(e) => tracing::error!(id = %row.id, "109: supersede: {e:#}"),
                        }
                    }
                    QuestionState::Answered => {
                        let decision: Option<Decision> =
                            row.answer.and_then(|a| serde_json::from_value(a).ok());
                        if let (Some(a), Some(decision)) = (approval, decision) {
                            let _ = a
                                .tell(HonourAnswer {
                                    session,
                                    call_id: call.call_id.clone(),
                                    decision,
                                })
                                .await;
                            n.honoured += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        let waits = self.store.waits_of(session, turn).await.unwrap_or_default();
        for w in waits.iter().filter(|w| w.kind == WaitKind::PeerReply) {
            for id in request_ids(w) {
                n.rearmed += usize::from(self.rearm_peer_wait(session, id).await);
            }
        }
        n
    }

    /// Re-create the live [`super::Wait`] of exchange `id` for `session`
    /// (§7.1): only while the exchange is still open (`accepted` /
    /// `consumed`); a terminal receipt is the resumed call's result as it
    /// stands. `true` when a waiter now exists.
    async fn rearm_peer_wait(&mut self, session: SessionId, id: Uuid) -> bool {
        if self.waits.contains_key(&id) {
            return true;
        }
        let receipt = match self.store.exchange_receipt(id).await {
            Ok(Some(r)) => r,
            Ok(None) => return false,
            Err(e) => {
                tracing::error!(%id, "109: exchange unreadable: {e:#}");
                return false;
            }
        };
        if !matches!(
            receipt.state,
            PeerMessageState::Accepted | PeerMessageState::Consumed
        ) {
            return false;
        }
        let generation = match PeerAddress::local(&receipt.to) {
            Some(to) => self.communication.0.write().route(session, to),
            None => {
                let contact = self.network.is_contact(&receipt.to);
                self.remote_generation(session, contact)
            }
        };
        let generation = match generation {
            Ok(g) => g,
            Err(e) => {
                tracing::info!(%session, %id, "109: wait not re-armed: {e}");
                return false;
            }
        };
        match self.wait(session, receipt.to.clone(), id, generation) {
            Ok(rx) => {
                rearmed().insert(id, rx);
                true
            }
            Err(e) => {
                tracing::warn!(%session, %id, "109: wait not re-armed: {e}");
                false
            }
        }
    }

    /// Deliver one spared child result into its parent (§7.2), the tail of
    /// `hand_off` without the live-child bookkeeping — 179: also any local
    /// peer's `accepted` exchange a paused tree kept. 139 §5.1 item 11: a
    /// parent owned elsewhere (taken since the child finished) is never
    /// woken here — the result goes to its owner as `Agents::deliver` does.
    pub(super) async fn deliver_spared(&mut self, record: PeerRecord) {
        let id = record.receipt.id;
        let (Some(child), Some(parent)) = (
            PeerAddress::local(&record.from),
            PeerAddress::local(&record.receipt.to),
        ) else {
            return;
        };
        if !self.enqueued.insert(id) {
            return;
        }
        // The rows' lineage, as `resolve` makes known before any route.
        self.mirror(child).await;
        let generation = match self.communication.0.write().route(child, parent) {
            Ok(g) => g,
            Err(e) => {
                tracing::info!(%child, %parent, "109: spared child result not delivered: {e}");
                self.enqueued.remove(&id);
                return;
            }
        };
        if let Some(owner) = self.remote_owner(parent).await {
            // ponytail: a forwarded row stays `accepted` here and is re-forwarded at
            // the next boot; the owner's intake dedups on the id (`accept_exchange_as`).
            let m = super::Deliver {
                from: child,
                to: record.receipt.to.clone(),
                text: record.text.clone(),
                await_reply: false,
                reply_to: record.receipt.reply_to,
                request_id: None,
                admitted: tokio_util::sync::CancellationToken::new(),
            };
            if let Err(e) = self
                .forward_peer(owner, parent, &m, record.text, record.receipt, generation)
                .await
            {
                tracing::warn!(%child, %parent, "139: spared child result not forwarded: {e}");
            }
            return;
        }
        if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
            self.enqueued.remove(&id);
            match self.store.fail_exchange(id, &reason).await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::error!(%id, "fail_exchange: {e:#}"),
            }
            self.error(parent, format!("subagent result not delivered: {reason}"))
                .await;
        }
    }
}

impl Agents {
    /// The §7.2 boot pass (module doc); `open` as [`Self::rebuild_waits`]'s.
    pub(super) async fn deliver_spared_results(&mut self, open: &[OpenTurnRow]) {
        let spared = match self.store.spared_child_results().await {
            Ok(s) => s,
            Err(e) => return tracing::error!("109: spared child results unreadable: {e:#}"),
        };
        if spared.is_empty() {
            return;
        }
        // A parent whose open turn waits on this child takes the result as
        // that wait's answer ([`ResumeWait`]), not as a new input.
        // 139 §5.1 item 11: the "awaited" subtraction over sessions here.
        let mut awaited: Vec<(SessionId, String)> = Vec::new();
        for (session, turn, _) in open {
            let waits = self
                .store
                .waits_of(*session, *turn)
                .await
                .unwrap_or_default();
            awaited.extend(
                waits
                    .into_iter()
                    .filter(|w| w.kind == WaitKind::ChildResult)
                    .map(|w| (*session, w.target)),
            );
        }
        let n = spared.len();
        for record in spared {
            let child = PeerAddress::local(&record.from).map(|c| c.to_string());
            let parent = PeerAddress::local(&record.receipt.to);
            if let (Some(c), Some(p)) = (child, parent)
                && awaited.contains(&(p, c))
            {
                continue;
            }
            // 179 D6: a paused agent's result stays `accepted` for its un-pause.
            if let Some(p) = parent
                && self.paused_root(p).await.is_some()
            {
                continue;
            }
            self.deliver_spared(record).await;
        }
        tracing::info!(n, "109: spared child results delivered");
    }

    /// [`ResumeQuestion`]'s body.
    async fn resume_question(
        &mut self,
        session: SessionId,
        call_id: &str,
    ) -> Result<Option<QuestionRow>, String> {
        Ok(self
            .store
            .questions_of_call(session, call_id)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .next())
    }

    /// [`ResumeWait`]'s body.
    async fn resume_wait(
        &mut self,
        session: SessionId,
        turn: TurnId,
        call_id: &str,
    ) -> Result<ResumedWait, String> {
        let row = self
            .store
            .waits_of(session, turn)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|w| w.tool_call_id == call_id);
        let mut out = ResumedWait {
            row: row.clone(),
            receipts: Vec::new(),
            pending: HashMap::new(),
            child_results: Vec::new(),
        };
        let Some(row) = row else { return Ok(out) };
        match row.kind {
            WaitKind::PeerReply => {
                for id in request_ids(&row) {
                    let Ok(Some(receipt)) = self.store.exchange_receipt(id).await else {
                        continue;
                    };
                    let open = matches!(
                        receipt.state,
                        PeerMessageState::Accepted | PeerMessageState::Consumed
                    );
                    if open
                        && self.rearm_peer_wait(session, id).await
                        && let Some(rx) = rearmed().remove(&id)
                    {
                        out.pending.insert(id, rx);
                    }
                    out.receipts.push(receipt);
                }
            }
            WaitKind::ChildResult => {
                let Ok(child) = row.target.parse::<SessionId>() else {
                    return Ok(out);
                };
                let from = format!("session:{child}");
                let spared = self
                    .store
                    .pending_exchanges(session, 64)
                    .await
                    .map_err(|e| e.to_string())?;
                for r in spared.into_iter().filter(|r| r.from == from) {
                    let Ok(generation) = self.communication.0.write().route(child, session) else {
                        continue;
                    };
                    let revision = generation.revision() as i64;
                    if let Ok(Some(settled)) = self
                        .store
                        .consume_exchange_direct(r.receipt.id, session, revision)
                        .await
                    {
                        self.enqueued.remove(&r.receipt.id);
                        self.peer_changed(&settled).await;
                        out.child_results.push(r.text);
                    }
                }
            }
            _ => {}
        }
        Ok(out)
    }
}

impl Message<ResumeQuestion> for Agents {
    type Reply = Result<Option<QuestionRow>, String>;

    async fn handle(
        &mut self,
        m: ResumeQuestion,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.resume_question(m.session, &m.call_id).await
    }
}

impl Message<ResumeWait> for Agents {
    type Reply = Result<ResumedWait, String>;

    async fn handle(&mut self, m: ResumeWait, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.resume_wait(m.session, m.turn, &m.call_id).await
    }
}

/// `deadline - now`, `None` once passed.
pub(crate) fn remaining(deadline: DateTime<Utc>) -> Option<std::time::Duration> {
    (deadline - Utc::now())
        .to_std()
        .ok()
        .filter(|d| !d.is_zero())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use genai::chat::{ChatMessage, ToolCall};
    use kameo::actor::Spawn;
    use serde_json::json;
    use tools::approval::RequestApproval;
    use tools::{Decision, Risk};

    use super::super::resume::tests::b_holding_a_turn;
    use super::super::tests::{agents_on, agents_with};
    use super::*;
    use crate::durability::StepKind;
    use crate::store::communication::AcceptExchange;
    use crate::store::durability::{calls_planned_in, step_in};

    /// An open turn of `s`: `input_accepted`, `model_call`, a `tool_batch` of
    /// `calls` (planned), with `started` marked `started`.
    async fn open_batch(
        a: &crate::store::Store,
        s: SessionId,
        calls: &[(&str, &str)],
        started: &[&str],
    ) -> TurnId {
        let t = TurnId::new();
        a.append(s, &ChatMessage::user("hi")).await.unwrap();
        a.step(s, t, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        a.step(s, t, StepKind::ModelCall, Some("mc1"), &json!({}))
            .await
            .unwrap();
        let seq = a.append(s, &ChatMessage::assistant("tools")).await.unwrap();
        let ids: Vec<&str> = calls.iter().map(|(id, _)| *id).collect();
        let mut tx = a.begin_write().await.unwrap();
        step_in(
            &mut tx,
            s,
            t,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ids, "round": 1}),
        )
        .await
        .unwrap();
        let planned: Vec<(ToolCall, Risk)> = calls
            .iter()
            .map(|(id, name)| {
                (
                    ToolCall {
                        call_id: (*id).into(),
                        fn_name: (*name).into(),
                        fn_arguments: json!({}),
                        thought_signatures: None,
                    },
                    Risk::Mutating,
                )
            })
            .collect();
        calls_planned_in(&mut tx, s, t, seq, &planned)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for id in started {
            assert!(a.call_started(s, id).await.unwrap());
        }
        t
    }

    fn approval_row(
        s: SessionId,
        call: &str,
        state: QuestionState,
        answer: Option<&str>,
    ) -> QuestionRow {
        let asked_at = Utc::now();
        QuestionRow {
            id: Uuid::now_v7().to_string(),
            session: s,
            tool_call_id: Some(call.into()),
            kind: QuestionKind::Approval,
            payload: json!({"tool": "shell"}),
            state,
            asked_at,
            deadline: asked_at + chrono::Duration::minutes(5),
            answered_at: answer.map(|_| asked_at),
            answer: answer.map(|a| json!(a)),
            answered_by: answer.map(|_| tools::Answerer::Human),
        }
    }

    /// §6.2 at boot: a `pending` approval of a `planned` call is `superseded`
    /// (its gate invocation died with the process); an `answered` one is
    /// handed to the approval actor, which honours it on the replay without
    /// publishing a card; rows of `started` calls and other kinds are left.
    #[tokio::test]
    async fn superseded_on_reask_and_answered_row_honoured() {
        let (_scratch, mut a) = agents_with(true).await;
        let (s, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let t = open_batch(
            &a.store,
            s,
            &[("c1", "shell"), ("c2", "shell"), ("c3", "ask_user")],
            &["c3"],
        )
        .await;
        let pending = approval_row(s, "c1", QuestionState::Pending, None);
        let answered = approval_row(s, "c2", QuestionState::Answered, Some("allow"));
        let mut ask_row = approval_row(s, "c3", QuestionState::Pending, None);
        ask_row.kind = QuestionKind::Ask;
        for r in [&pending, &answered, &ask_row] {
            a.store.question_put(r).await.unwrap();
        }
        // The approval actor, as the engine registers it (handed in directly:
        // the process-wide registry name belongs to whichever test is up).
        let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let approval = tools::Approval::spawn(tools::Approval::new(rt, Duration::from_millis(50)));

        a.rebuild_waits_of(s, t, Some(&approval)).await;

        let state = |id: &str| {
            let rows = futures::executor::block_on(a.store.questions_pending(s)).unwrap();
            rows.iter().any(|r| r.id == id)
        };
        assert!(
            !state(&pending.id),
            "c1's pending approval is no longer pending"
        );
        assert!(
            state(&ask_row.id),
            "c3's ask row (a started call) is untouched"
        );
        let c1 = a.store.questions_of_call(s, "c1").await.unwrap();
        assert_eq!(c1[0].state, QuestionState::Superseded);
        let c2 = a.store.questions_of_call(s, "c2").await.unwrap();
        assert_eq!(
            c2[0].state,
            QuestionState::Answered,
            "the answered row stands"
        );
        // The replay of c2 asks the gate again: honoured, no card, no timeout.
        let r = approval
            .ask(RequestApproval {
                session: s,
                tool: "shell".into(),
                args: json!({}),
                risk: Risk::Destructive,
                call_id: Some("c2".into()),
            })
            .await
            .unwrap();
        assert_eq!((r.decision, r.timed_out), (Decision::Allow, false));
        // c1 was superseded, not answered: its replay asks for real.
        let r = approval
            .ask(RequestApproval {
                session: s,
                tool: "shell".into(),
                args: json!({}),
                risk: Risk::Destructive,
                call_id: Some("c1".into()),
            })
            .await
            .unwrap();
        assert!(r.timed_out);
        // The newest row of the call; an unknown call has none.
        let row = a.resume_question(s, "c3").await.unwrap();
        assert_eq!(row.unwrap().id, ask_row.id);
        assert!(a.resume_question(s, "zz").await.unwrap().is_none());
    }

    /// `root` awaiting `child`'s answer to one accepted exchange, journaled
    /// as the `started` call `c1` of an open turn with its `waits(peer_reply)`
    /// row whose deadline is `left` from now.
    async fn awaiting(
        a: &mut Agents,
        left: chrono::Duration,
    ) -> (SessionId, SessionId, TurnId, PeerReceipt) {
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let t = open_batch(&a.store, root, &[("c1", "send_message")], &["c1"]).await;
        let generation = a.communication.0.write().route(root, child).unwrap();
        let receipt = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &format!("session:{root}"),
                recipient: &format!("session:{child}"),
                sender_session: Some(root),
                recipient_session: Some(child),
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "question",
            })
            .await
            .unwrap();
        a.store
            .wait_put(&WaitRow {
                session: root,
                tool_call_id: "c1".into(),
                kind: WaitKind::PeerReply,
                target: receipt.id.to_string(),
                deadline: Some(Utc::now() + left),
                payload: json!({ "request_id": receipt.id, "request_ids": [receipt.id] }),
            })
            .await
            .unwrap();
        (root, child, t, receipt)
    }

    /// §4.4 / §7.1: at boot the wait is re-created in `Agents.waits` (so the
    /// pending sets of `PeerMessagePage` list it again and a reply that
    /// arrives before the turn resumes still settles it); the resumed call
    /// takes the live receiver with the row's remaining deadline, and the
    /// reply resolves it with the `replied` receipt.
    #[tokio::test]
    async fn wait_rearmed_with_remaining_deadline() {
        let (_scratch, mut a) = agents_with(true).await;
        let (root, child, t, receipt) = awaiting(&mut a, chrono::Duration::minutes(10)).await;
        assert!(a.waits.is_empty());

        let open = a.open_here().await;
        a.rebuild_waits(&open).await;
        assert!(a.waits.contains_key(&receipt.id), "the waiter is back");
        let (received, sent) = a.pending_requests(root);
        assert!(received.is_empty() && sent == [receipt.id]);
        let (received, _) = a.pending_requests(child);
        assert_eq!(received, [receipt.id]);

        let mut w = a.resume_wait(root, t, "c1").await.unwrap();
        let row = w.row.unwrap();
        let left = remaining(row.deadline.unwrap()).unwrap();
        assert!(left > Duration::from_secs(9 * 60) && left <= Duration::from_secs(10 * 60));
        assert_eq!(w.receipts.len(), 1);
        assert_eq!(w.receipts[0].state, PeerMessageState::Accepted);
        let rx = w
            .pending
            .remove(&receipt.id)
            .expect("live receiver for the open exchange");
        assert!(w.child_results.is_empty());

        // The child answers after the restart: the same path as before it.
        let replied = a
            .store
            .owner_answer(receipt.id, child, "the answer")
            .await
            .unwrap();
        assert!(a.settle_wait(&replied));
        let fin = rx.await.unwrap();
        assert_eq!(fin.state, PeerMessageState::Replied);
        assert_eq!(fin.reply.as_deref(), Some("the answer"));
        assert!(a.waits.is_empty());

        // A row whose deadline passed while down: nothing left to wait.
        let (root2, _, t2, r2) = awaiting(&mut a, -chrono::Duration::seconds(30)).await;
        let w = a.resume_wait(root2, t2, "c1").await.unwrap();
        assert!(remaining(w.row.unwrap().deadline.unwrap()).is_none());
        assert_eq!(w.receipts[0].id, r2.id);
        // No row at all: the call wrote nothing before the kill.
        let w = a.resume_wait(root2, t2, "zz").await.unwrap();
        assert!(w.row.is_none() && w.receipts.is_empty());
    }

    /// §7.1: a reply that landed while the engine was down is delivered as the
    /// call's result on resume — nothing is re-armed for a `replied` exchange.
    #[tokio::test]
    async fn replied_exchange_delivered_on_resume() {
        let (_scratch, mut a) = agents_with(true).await;
        let (root, child, t, receipt) = awaiting(&mut a, chrono::Duration::minutes(10)).await;
        a.store
            .owner_answer(receipt.id, child, "while you were out")
            .await
            .unwrap();

        let open = a.open_here().await;
        a.rebuild_waits(&open).await;
        assert!(a.waits.is_empty(), "a replied exchange has no waiter");
        let w = a.resume_wait(root, t, "c1").await.unwrap();
        assert!(w.pending.is_empty());
        assert_eq!(w.receipts.len(), 1);
        assert_eq!(w.receipts[0].state, PeerMessageState::Replied);
        assert_eq!(w.receipts[0].reply.as_deref(), Some("while you were out"));
        let out = crate::peers::receipts_output(&w.receipts);
        assert!(!out.is_error);
        assert!(out.content.contains("replied") && out.content.ends_with("while you were out"));
    }

    /// §7.2: a child's `result:` exchange `fail_interrupted` spared is
    /// enqueued into its parent at boot through the ordinary intake (the
    /// parent is admitted a ticket); a parent whose open turn waits on that
    /// child (`child_result`) keeps it for the resumed call instead, which
    /// consumes it.
    #[tokio::test]
    async fn spared_child_results_delivered_once() {
        let (_scratch, mut a) = agents_with(true).await;
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((p, "kid")), None, (None, None))
            .await
            .unwrap();
        a.ledger.certify();
        let generation = a.communication.0.write().route(child, p).unwrap();
        let ct = TurnId::new();
        let (from, to, rid) = (
            format!("session:{child}"),
            format!("session:{p}"),
            format!("result:{ct}"),
        );
        let receipt = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(child),
                recipient_session: Some(p),
                request_id: Some(&rid),
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "done",
            })
            .await
            .unwrap();
        assert_eq!(a.store.spared_child_results().await.unwrap().len(), 1);
        assert_eq!(a.ledger.activity(p), None);

        let open = a.open_here().await;
        a.deliver_spared_results(&open).await;
        assert!(
            a.enqueued.contains(&receipt.id),
            "handed to the parent's intake"
        );
        assert!(
            a.ledger.activity(p).is_some(),
            "the parent is admitted a ticket"
        );
        // Again: the dedup set refuses a second delivery.
        let open = a.open_here().await;
        a.deliver_spared_results(&open).await;
        assert_eq!(a.enqueued.len(), 1);

        // A parent whose open turn waits on this child: left for the resume.
        let (q, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let kid2 = SessionId::new();
        a.session(Some(kid2), Some((q, "kid2")), None, (None, None))
            .await
            .unwrap();
        let t = open_batch(&a.store, q, &[("c1", "send_message")], &["c1"]).await;
        a.store
            .wait_put(&WaitRow {
                session: q,
                tool_call_id: "c1".into(),
                kind: WaitKind::ChildResult,
                target: kid2.to_string(),
                deadline: None,
                payload: json!({}),
            })
            .await
            .unwrap();
        let g2 = a.communication.0.write().route(kid2, q).unwrap();
        let ct2 = TurnId::new();
        let rid2 = format!("result:{ct2}");
        let r2 = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &format!("session:{kid2}"),
                recipient: &format!("session:{q}"),
                sender_session: Some(kid2),
                recipient_session: Some(q),
                request_id: Some(&rid2),
                reply_to: None,
                request_scope: None,
                generation: g2.revision() as i64,
                body: "kid2 finished",
            })
            .await
            .unwrap();
        let open = a.open_here().await;
        a.deliver_spared_results(&open).await;
        assert!(
            !a.enqueued.contains(&r2.id),
            "awaited by an open turn: not injected"
        );
        let w = a.resume_wait(q, t, "c1").await.unwrap();
        assert_eq!(w.child_results, ["kid2 finished"]);
        assert_eq!(
            a.store
                .exchange_receipt(r2.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            PeerMessageState::Consumed
        );
        assert!(
            a.store
                .spared_child_results()
                .await
                .unwrap()
                .iter()
                .all(|r| r.receipt.id != r2.id)
        );
    }

    /// 139 §7.3 / I1: `rebuild_waits_for(s1)` settles s1's pending approval
    /// and leaves s2's; the boot `rebuild_waits` settles s2's and never `a`'s
    /// (a replica re-arms nothing of the owner's — `Scope::Here`).
    #[tokio::test]
    async fn rebuild_waits_for_one_session_leaves_others() {
        let scratch = crate::TestDir::new("139");
        let dir = scratch.0.clone();
        let a = crate::store::Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let foreign = SessionId::new();
        a.create_session(foreign, "m", None, None, None)
            .await
            .unwrap();
        open_batch(&a, foreign, &[("c1", "shell")], &[]).await;
        let theirs = approval_row(foreign, "c1", QuestionState::Pending, None);
        a.question_put(&theirs).await.unwrap();
        a.close().await;

        let mut b = agents_on(dir, Some("b"), true).await;
        let (s1, _) = b.session(None, None, None, (None, None)).await.unwrap();
        let (s2, _) = b.session(None, None, None, (None, None)).await.unwrap();
        open_batch(&b.store, s1, &[("c1", "shell")], &[]).await;
        open_batch(&b.store, s2, &[("c1", "shell")], &[]).await;
        let (p1, p2) = (
            approval_row(s1, "c1", QuestionState::Pending, None),
            approval_row(s2, "c1", QuestionState::Pending, None),
        );
        b.store.question_put(&p1).await.unwrap();
        b.store.question_put(&p2).await.unwrap();
        let store = b.store.clone();
        let state = |s: SessionId| {
            let store = store.clone();
            async move { store.questions_of_call(s, "c1").await.unwrap()[0].state }
        };

        b.rebuild_waits_for(s1).await;
        assert_eq!(state(s1).await, QuestionState::Superseded);
        assert_eq!(state(s2).await, QuestionState::Pending, "s2 untouched");
        assert_eq!(state(foreign).await, QuestionState::Pending);

        let open = b.open_here().await;
        b.rebuild_waits(&open).await;
        assert_eq!(state(s2).await, QuestionState::Superseded);
        assert_eq!(
            state(foreign).await,
            QuestionState::Pending,
            "a's approval is not b's"
        );
    }

    /// 139 §5.1 item 11: a spared result for a parent owned elsewhere is
    /// forwarded, never injected here — no actor, no ticket; with no mesh the
    /// forward fails as `OwnerUnreachable` and the receipt says so.
    #[tokio::test]
    async fn spared_result_for_a_foreign_parent_is_forwarded_not_injected() {
        let (_dir, mut b, foreign, _) = b_holding_a_turn(true).await;
        let child = SessionId::new();
        b.store
            .create_session(child, "m", Some(foreign), Some("kid"), None)
            .await
            .unwrap();
        b.ledger.certify();
        b.mirror(child).await;
        let generation = b.communication.0.write().route(child, foreign).unwrap();
        let rid = format!("result:{}", TurnId::new());
        let receipt = b
            .store
            .accept_exchange(&AcceptExchange {
                sender: &format!("session:{child}"),
                recipient: &format!("session:{foreign}"),
                sender_session: Some(child),
                recipient_session: Some(foreign),
                request_id: Some(&rid),
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "done on b",
            })
            .await
            .unwrap();
        assert_eq!(b.store.spared_child_results().await.unwrap().len(), 1);

        let open = b.open_here().await;
        b.deliver_spared_results(&open).await;
        assert!(b.sessions.is_empty(), "no actor for a's session");
        assert_eq!(b.ledger.activity(foreign), None, "no ticket admitted");
        assert!(!b.enqueued.contains(&receipt.id));
        let r = b.store.exchange_receipt(receipt.id).await.unwrap().unwrap();
        assert_eq!(r.state, PeerMessageState::Failed);
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.contains("OwnerUnreachable") && e.contains("\"a\"")),
            "{r:?}"
        );
    }
}
