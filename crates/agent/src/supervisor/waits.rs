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
//!   [`HonourAnswer`] so the replay does not ask again. It ends by queueing
//!   [`DeliverSparedResults`] behind the handler, i.e. after `resume_scan`.
//! - [`DeliverSparedResults`]: the boot pass of §7.2 — every child result
//!   `fail_interrupted` spared is enqueued into its parent through the same
//!   intake as a fresh one; the dedup key makes a double delivery impossible.
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
use crate::AGENT_ACTOR;
use crate::communication::{PeerAddress, PeerMessageState, PeerReceipt, PeerRecord};
use crate::durability::CallState;

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

/// `tell` (internal): the §7.2 boot pass over spared `result:` exchanges.
pub(super) struct DeliverSparedResults;

/// `ask` → `Result<(SessionId, Option<QuestionRow>), String>`: the session of
/// open turn `turn` and the newest `questions` row of `call_id` (§6.2).
pub(crate) struct ResumeQuestion {
    /// The open turn being resumed.
    pub turn: TurnId,
    /// The `started` human-wait call.
    pub call_id: String,
}

/// `ask` → `Result<ResumedWait, String>`: what a `started`
/// `send_message{await_reply}` call blocks on after a restart (§4.4).
pub(crate) struct ResumeWait {
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

impl Agents {
    /// Boot (§4.6 step 10): rebuild the live waiters and settle the dead
    /// approvals of every open turn from their rows; see the module doc.
    pub(super) async fn rebuild_waits(&mut self) {
        let open = match self.store.open_turns().await {
            Ok(o) => o,
            Err(e) => return tracing::error!("109: open turns unreadable: {e:#}"),
        };
        let approval = ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)
            .ok()
            .flatten();
        let (mut rearmed, mut honoured, mut superseded) = (0usize, 0usize, 0usize);
        for (session, turn, _) in &open {
            let calls = self.store.calls_of(*session, *turn).await.unwrap_or_default();
            for call in calls.iter().filter(|c| c.state == CallState::Planned) {
                let rows = self
                    .store
                    .questions_of_call(*session, &call.call_id)
                    .await
                    .unwrap_or_default();
                for row in rows.into_iter().filter(|r| r.kind == QuestionKind::Approval) {
                    match row.state {
                        QuestionState::Pending => {
                            // §6.2: the gate's invocation died with the process.
                            match self
                                .store
                                .question_resolve(&row.id, QuestionState::Superseded, None)
                                .await
                            {
                                Ok(true) => superseded += 1,
                                Ok(false) => {}
                                Err(e) => tracing::error!(id = %row.id, "109: supersede: {e:#}"),
                            }
                        }
                        QuestionState::Answered => {
                            let decision: Option<Decision> = row
                                .answer
                                .and_then(|a| serde_json::from_value(a).ok());
                            if let (Some(a), Some(decision)) = (&approval, decision) {
                                let _ = a
                                    .tell(HonourAnswer {
                                        session: *session,
                                        call_id: call.call_id.clone(),
                                        decision,
                                    })
                                    .await;
                                honoured += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let waits = self.store.waits_of(*session, *turn).await.unwrap_or_default();
            for w in waits.iter().filter(|w| w.kind == WaitKind::PeerReply) {
                for id in request_ids(w) {
                    rearmed += usize::from(self.rearm_peer_wait(*session, id).await);
                }
            }
        }
        if rearmed + honoured + superseded > 0 {
            tracing::info!(
                rearmed,
                honoured,
                superseded,
                "109: waits and approvals rebuilt from the journal"
            );
        }
        // §7.2: behind this handler's remaining work (`resume_scan`), so a
        // resumed parent is queued before its spared results arrive.
        if let Ok(Some(me)) = ActorRef::<Agents>::lookup(AGENT_ACTOR)
            && let Err(e) = me.tell(DeliverSparedResults).try_send()
        {
            tracing::error!("109: spared child results not scheduled: {e}");
        }
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
            None => self.communication.0.write().remote(session),
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

    /// The session whose open turn is `turn`, from the journal.
    async fn session_of_turn(&self, turn: TurnId) -> Result<SessionId, String> {
        self.store
            .open_turns()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|(_, t, _)| *t == turn)
            .map(|(s, _, _)| s)
            .ok_or_else(|| format!("turn {turn} is not open"))
    }

    /// Deliver one spared child result into its parent (§7.2), the tail of
    /// `hand_off` without the live-child bookkeeping.
    async fn deliver_spared(&mut self, record: PeerRecord) {
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
        let generation = match self.communication.0.write().route(child, parent) {
            Ok(g) => g,
            Err(e) => {
                tracing::info!(%child, %parent, "109: spared child result not delivered: {e}");
                self.enqueued.remove(&id);
                return;
            }
        };
        if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
            self.enqueued.remove(&id);
            match self.store.fail_exchange(id, &reason).await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::error!(%id, "fail_exchange: {e:#}"),
            }
        }
    }
}

impl Agents {
    /// [`DeliverSparedResults`]'s body.
    async fn deliver_spared_results(&mut self) {
        let spared = match self.store.spared_child_results().await {
            Ok(s) => s,
            Err(e) => return tracing::error!("109: spared child results unreadable: {e:#}"),
        };
        if spared.is_empty() {
            return;
        }
        // A parent whose open turn waits on this child takes the result as
        // that wait's answer ([`ResumeWait`]), not as a new input.
        let mut awaited: Vec<(SessionId, String)> = Vec::new();
        if let Ok(open) = self.store.open_turns().await {
            for (session, turn, _) in open {
                let waits = self.store.waits_of(session, turn).await.unwrap_or_default();
                awaited.extend(
                    waits
                        .into_iter()
                        .filter(|w| w.kind == WaitKind::ChildResult)
                        .map(|w| (session, w.target)),
                );
            }
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
            self.deliver_spared(record).await;
        }
        tracing::info!(n, "109: spared child results delivered");
    }

    /// [`ResumeQuestion`]'s body.
    async fn resume_question(
        &mut self,
        turn: TurnId,
        call_id: &str,
    ) -> Result<(SessionId, Option<QuestionRow>), String> {
        let session = self.session_of_turn(turn).await?;
        let row = self
            .store
            .questions_of_call(session, call_id)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        Ok((session, row))
    }

    /// [`ResumeWait`]'s body.
    async fn resume_wait(&mut self, turn: TurnId, call_id: &str) -> Result<ResumedWait, String> {
        let session = self.session_of_turn(turn).await?;
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
                    if open && self.rearm_peer_wait(session, id).await
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

impl Message<DeliverSparedResults> for Agents {
    type Reply = ();

    async fn handle(&mut self, _: DeliverSparedResults, _: &mut Context<Self, Self::Reply>) {
        self.deliver_spared_results().await;
    }
}

impl Message<ResumeQuestion> for Agents {
    type Reply = Result<(SessionId, Option<QuestionRow>), String>;

    async fn handle(&mut self, m: ResumeQuestion, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.resume_question(m.turn, &m.call_id).await
    }
}

impl Message<ResumeWait> for Agents {
    type Reply = Result<ResumedWait, String>;

    async fn handle(&mut self, m: ResumeWait, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.resume_wait(m.turn, &m.call_id).await
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

    use super::super::tests::agents_with;
    use super::*;
    use crate::durability::StepKind;
    use crate::store::communication::AcceptExchange;
    use crate::store::durability::{calls_planned_in, step_in};

    /// An open turn of `s`: `input_accepted`, `model_call`, a `tool_batch` of
    /// `calls` (planned), with `started` marked `started`.
    async fn open_batch(
        a: &Agents,
        s: SessionId,
        calls: &[(&str, &str)],
        started: &[&str],
    ) -> TurnId {
        let t = TurnId::new();
        a.store.append(s, &ChatMessage::user("hi")).await.unwrap();
        a.store
            .step(s, t, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        a.store
            .step(s, t, StepKind::ModelCall, Some("mc1"), &json!({}))
            .await
            .unwrap();
        let seq = a
            .store
            .append(s, &ChatMessage::assistant("tools"))
            .await
            .unwrap();
        let ids: Vec<&str> = calls.iter().map(|(id, _)| *id).collect();
        let mut tx = a.store.begin_write().await.unwrap();
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
        calls_planned_in(&mut tx, s, t, seq, &planned).await.unwrap();
        tx.commit().await.unwrap();
        for id in started {
            assert!(a.store.call_started(s, id).await.unwrap());
        }
        t
    }

    fn approval_row(s: SessionId, call: &str, state: QuestionState, answer: Option<&str>) -> QuestionRow {
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
        }
    }

    /// §6.2 at boot: a `pending` approval of a `planned` call is `superseded`
    /// (its gate invocation died with the process); an `answered` one is
    /// handed to the approval actor, which honours it on the replay without
    /// publishing a card; rows of `started` calls and other kinds are left.
    #[tokio::test]
    async fn superseded_on_reask_and_answered_row_honoured() {
        let mut a = agents_with(true).await;
        let (s, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let _t = open_batch(
            &a,
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
        // The approval actor as the engine registers it; another test may
        // already have one up — the registered one serves either way.
        let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let spawned = tools::Approval::spawn(tools::Approval::new(rt, Duration::from_millis(50)));
        let approval = match spawned.register(tools::APPROVAL_ACTOR) {
            Ok(()) => spawned,
            Err(_) => ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)
                .unwrap()
                .unwrap(),
        };

        a.rebuild_waits().await;

        let state = |id: &str| {
            let rows = futures::executor::block_on(a.store.questions_pending(s)).unwrap();
            rows.iter().any(|r| r.id == id)
        };
        assert!(!state(&pending.id), "c1's pending approval is no longer pending");
        assert!(state(&ask_row.id), "c3's ask row (a started call) is untouched");
        let c1 = a.store.questions_of_call(s, "c1").await.unwrap();
        assert_eq!(c1[0].state, QuestionState::Superseded);
        let c2 = a.store.questions_of_call(s, "c2").await.unwrap();
        assert_eq!(c2[0].state, QuestionState::Answered, "the answered row stands");
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
        // The session of an open turn resolves; a closed one does not.
        let (session, row) = a.resume_question(_t, "c3").await.unwrap();
        assert_eq!(session, s);
        assert_eq!(row.unwrap().id, ask_row.id);
        assert!(a.resume_question(TurnId::new(), "c3").await.is_err());
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
        let t = open_batch(a, root, &[("c1", "send_message")], &["c1"]).await;
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
        let mut a = agents_with(true).await;
        let (root, child, t, receipt) = awaiting(&mut a, chrono::Duration::minutes(10)).await;
        assert!(a.waits.is_empty());

        a.rebuild_waits().await;
        assert!(a.waits.contains_key(&receipt.id), "the waiter is back");
        let (received, sent) = a.pending_requests(root);
        assert!(received.is_empty() && sent == [receipt.id]);
        let (received, _) = a.pending_requests(child);
        assert_eq!(received, [receipt.id]);

        let mut w = a.resume_wait(t, "c1").await.unwrap();
        let row = w.row.unwrap();
        let left = remaining(row.deadline.unwrap()).unwrap();
        assert!(left > Duration::from_secs(9 * 60) && left <= Duration::from_secs(10 * 60));
        assert_eq!(w.receipts.len(), 1);
        assert_eq!(w.receipts[0].state, PeerMessageState::Accepted);
        let rx = w.pending.remove(&receipt.id).expect("live receiver for the open exchange");
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
        let (_, _, t2, r2) = awaiting(&mut a, -chrono::Duration::seconds(30)).await;
        let w = a.resume_wait(t2, "c1").await.unwrap();
        assert!(remaining(w.row.unwrap().deadline.unwrap()).is_none());
        assert_eq!(w.receipts[0].id, r2.id);
        // No row at all: the call wrote nothing before the kill.
        let w = a.resume_wait(t2, "zz").await.unwrap();
        assert!(w.row.is_none() && w.receipts.is_empty());
    }

    /// §7.1: a reply that landed while the engine was down is delivered as the
    /// call's result on resume — nothing is re-armed for a `replied` exchange.
    #[tokio::test]
    async fn replied_exchange_delivered_on_resume() {
        let mut a = agents_with(true).await;
        let (_root, child, t, receipt) = awaiting(&mut a, chrono::Duration::minutes(10)).await;
        a.store
            .owner_answer(receipt.id, child, "while you were out")
            .await
            .unwrap();

        a.rebuild_waits().await;
        assert!(a.waits.is_empty(), "a replied exchange has no waiter");
        let w = a.resume_wait(t, "c1").await.unwrap();
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
        let mut a = agents_with(true).await;
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((p, "kid")), None, (None, None))
            .await
            .unwrap();
        a.ledger.certify();
        let generation = a.communication.0.write().route(child, p).unwrap();
        let result = |turn: TurnId| AcceptExchange {
            sender: &format!("session:{child}"),
            recipient: &format!("session:{p}"),
            sender_session: Some(child),
            recipient_session: Some(p),
            request_id: None,
            reply_to: None,
            request_scope: None,
            generation: generation.revision() as i64,
            body: "done",
        };
        let ct = TurnId::new();
        let rid = format!("result:{ct}");
        let mut ex = result(ct);
        ex.request_id = Some(&rid);
        let receipt = a.store.accept_exchange(&ex).await.unwrap();
        assert_eq!(a.store.spared_child_results().await.unwrap().len(), 1);
        assert_eq!(a.ledger.activity(p), None);

        a.deliver_spared_results().await;
        assert!(a.enqueued.contains(&receipt.id), "handed to the parent's intake");
        assert!(a.ledger.activity(p).is_some(), "the parent is admitted a ticket");
        // Again: the dedup set refuses a second delivery.
        a.deliver_spared_results().await;
        assert_eq!(a.enqueued.len(), 1);

        // A parent whose open turn waits on this child: left for the resume.
        let (q, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let kid2 = SessionId::new();
        a.session(Some(kid2), Some((q, "kid2")), None, (None, None))
            .await
            .unwrap();
        let t = open_batch(&a, q, &[("c1", "send_message")], &["c1"]).await;
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
        a.deliver_spared_results().await;
        assert!(!a.enqueued.contains(&r2.id), "awaited by an open turn: not injected");
        let w = a.resume_wait(t, "c1").await.unwrap();
        assert_eq!(w.child_results, ["kid2 finished"]);
        assert_eq!(
            a.store.exchange_receipt(r2.id).await.unwrap().unwrap().state,
            PeerMessageState::Consumed
        );
        assert!(a.store.spared_child_results().await.unwrap().iter().all(|r| r.receipt.id != r2.id));
    }
}
