//! `Approval` actor, registered as [`crate::APPROVAL_ACTOR`]. Publishes
//! [`ApprovalRequested`] and waits for a [`Resolve`] from any UI.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::Risk;
use crate::journal::{
    Answerer, Journal, QuestionKind, QuestionRow, QuestionState, WaitKind, WaitRow,
};

/// Correlates a request with its resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct ApprovalId(pub uuid::Uuid);

/// Human answer. A `kameo::Reply` so [`crate::ExternalApproval`] can answer with it directly.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Run it.
    Allow,
    /// Refuse; the model sees an error.
    Deny,
}

/// Event: a tool call is waiting on a human. Every UI subscribes.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ApprovalRequested {
    /// Use in [`Resolve`].
    pub id: ApprovalId,
    /// Owning session.
    pub session: SessionId,
    /// Tool name.
    pub tool: String,
    /// Raw arguments; UIs format these per tool (shell: `command`).
    pub args: serde_json::Value,
    /// Risk class that triggered the ask.
    pub risk: Risk,
}

impl wire::WireEvent for ApprovalRequested {
    const TOPIC: &'static str = "tools.approval-requested";
    const VERSION: u32 = 1;
}

/// Event: a request was answered (by a human or by timeout). UIs dismiss
/// their prompt on this.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ApprovalResolved {
    /// The request.
    pub id: ApprovalId,
    /// Outcome.
    pub decision: Decision,
    /// True when nobody answered in time.
    pub timed_out: bool,
    /// 207: the user left the call to the agent (Attention dismiss): not run; `decision` is `deny`, `timed_out` false.
    #[serde(default)]
    pub dismissed: bool,
}

impl wire::WireEvent for ApprovalResolved {
    const TOPIC: &'static str = "tools.approval-resolved";
    const VERSION: u32 = 1;
}

/// `ask` → `bool`: true if the id was pending and is now resolved. A human
/// answer; the double answers with [`ResolveBy`] (126 §4.3).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Resolve {
    /// The request.
    pub id: ApprovalId,
    /// The answer.
    pub decision: Decision,
}

/// [`Resolve`] naming who answers (126 §4.3): the double's L2/L3 answer
/// settles the row with `answered_by = double`; everything else is as for
/// a human `Resolve`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ResolveBy {
    /// The request.
    pub id: ApprovalId,
    /// The answer.
    pub decision: Decision,
    /// Who answers.
    pub by: Answerer,
}

/// 207: `ask` → `bool` (true when it was pending): the user leaves this call to the agent — it is not run.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Dismiss {
    /// The request.
    pub id: ApprovalId,
}

/// Registry → approval: publish [`ApprovalRequested`] and wait for a
/// [`Resolve`] or the timeout. `ask` → [`ApprovalResolved`] (timeout = `Deny`).
#[derive(Clone, Debug)]
pub struct RequestApproval {
    /// Owning session.
    pub session: SessionId,
    /// Tool name.
    pub tool: String,
    /// Raw arguments.
    pub args: serde_json::Value,
    /// Risk class.
    pub risk: Risk,
    /// The gated tool call (109 §6.1: the `questions.tool_call_id`); `None`
    /// for an external approval that belongs to no call.
    pub call_id: Option<String>,
}

/// 109 §6.2 (D-call): a stored `answered` row for a `planned` call the
/// engine died before starting. `tell` before the call replays; the next
/// [`RequestApproval`] for `(session, call_id)` resolves with `decision`
/// without asking again.
#[derive(Clone, Debug)]
pub struct HonourAnswer {
    /// The asking session.
    pub session: SessionId,
    /// The call the answer was given for.
    pub call_id: String,
    /// The stored answer.
    pub decision: Decision,
    /// 207: the stored answer was a dismissal (`decision` is `Deny`): the
    /// replay resolves as dismissed.
    pub dismissed: bool,
}

/// Internal: drop a pending entry whose waiter gave up.
struct Expire(ApprovalId);

/// 207: what the parked call is told: a decision, or that the user left it
/// to the agent.
enum Verdict {
    Decided(Decision),
    Dismissed,
}

/// One unanswered request: the card, its journal row and the waiter.
struct Pending {
    requested: ApprovalRequested,
    row: QuestionRow,
    tx: oneshot::Sender<Verdict>,
}

/// The approval actor.
#[derive(kameo::Actor)]
pub struct Approval {
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<ApprovalId, Pending>,
    /// [`HonourAnswer`]s not yet consumed by their call's replay: the
    /// decision and whether it was a dismissal (207).
    honoured: HashMap<(SessionId, String), (Decision, bool)>,
    /// 109 §6.1: the journal seam the `questions` rows go through.
    pub(crate) journal: Arc<dyn Journal>,
}

impl Approval {
    /// Build with the runtime (for publishing) and the human timeout.
    pub fn new(rt: Runtime, timeout: Duration) -> Self {
        Self {
            rt,
            timeout,
            pending: HashMap::new(),
            honoured: HashMap::new(),
            journal: Arc::new(crate::NoopJournal),
        }
    }

    /// Journal a pending row's end: the `questions` row leaves `pending`
    /// and the call's `waits(question)` row goes (§4.4). `answer` is the
    /// stored JSON (the bare `Decision`, or 207's `"dismissed"`); `by` names
    /// who answered (126 §4.3); `None` on a timeout.
    async fn settle(
        &self,
        mut row: QuestionRow,
        state: QuestionState,
        answer: Option<serde_json::Value>,
        by: Option<Answerer>,
    ) {
        row.state = state;
        row.answered_at = Some(Utc::now());
        row.answer = answer;
        row.answered_by = by;
        if let Some(call) = &row.tool_call_id {
            self.journal.wait(None, row.session, call).await;
        }
        self.journal.question(row).await;
    }

    /// Deliver `decision` to the parked call and settle the row; `false` when
    /// `id` was not pending.
    async fn resolve(&mut self, id: ApprovalId, decision: Decision, by: Answerer) -> bool {
        let Some(p) = self.pending.remove(&id) else {
            return false;
        };
        let delivered = p.tx.send(Verdict::Decided(decision)).is_ok();
        self.settle(
            p.row,
            QuestionState::Answered,
            Some(serde_json::json!(decision)),
            Some(by),
        )
        .await;
        delivered
    }
}

impl Message<crate::InstallJournal> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: crate::InstallJournal, _: &mut Context<Self, Self::Reply>) {
        self.journal = m.0;
    }
}

impl Message<HonourAnswer> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: HonourAnswer, _: &mut Context<Self, Self::Reply>) {
        self.honoured
            .insert((m.session, m.call_id), (m.decision, m.dismissed));
    }
}

impl Message<RequestApproval> for Approval {
    type Reply = DelegatedReply<ApprovalResolved>;

    async fn handle(
        &mut self,
        m: RequestApproval,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let id = ApprovalId(uuid::Uuid::now_v7());
        if let Some((decision, dismissed)) = m
            .call_id
            .as_ref()
            .and_then(|c| self.honoured.remove(&(m.session, c.clone())))
        {
            // 109 §6.2: the user already answered this call; no second card.
            if let Some(tx) = tx {
                tx.send(ApprovalResolved {
                    id,
                    decision,
                    timed_out: false,
                    dismissed,
                });
            }
            return reply;
        }
        let (dtx, drx) = oneshot::channel();
        let requested = ApprovalRequested {
            id,
            session: m.session,
            tool: m.tool,
            args: m.args,
            risk: m.risk,
        };
        let asked_at = Utc::now();
        let deadline = asked_at + self.timeout;
        let row = QuestionRow {
            id: id.0.to_string(),
            session: m.session,
            tool_call_id: m.call_id,
            kind: QuestionKind::Approval,
            payload: serde_json::json!(requested),
            state: QuestionState::Pending,
            asked_at,
            deadline,
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        self.journal.question(row.clone()).await;
        if let Some(call) = &row.tool_call_id {
            self.journal
                .wait(
                    Some(WaitRow {
                        session: m.session,
                        tool_call_id: call.clone(),
                        kind: WaitKind::Question,
                        target: row.id.clone(),
                        deadline: Some(deadline),
                        payload: serde_json::json!({}),
                    }),
                    m.session,
                    call,
                )
                .await;
        }
        self.pending.insert(
            id,
            Pending {
                requested: requested.clone(),
                row,
                tx: dtx,
            },
        );
        let rt = self.rt.clone();
        let timeout = self.timeout;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(requested).await;
            let (decision, timed_out, dismissed) = match tokio::time::timeout(timeout, drx).await {
                Ok(Ok(Verdict::Decided(d))) => (d, false, false),
                // 207: left to the agent: not run.
                Ok(Ok(Verdict::Dismissed)) => (Decision::Deny, false, true),
                // Sender dropped: actor died; treat as deny.
                Ok(Err(_)) => (Decision::Deny, false, false),
                Err(_) => {
                    let _ = me.ask(Expire(id)).await;
                    (Decision::Deny, true, false)
                }
            };
            let resolved = ApprovalResolved {
                id,
                decision,
                timed_out,
                dismissed,
            };
            rt.publish(resolved.clone()).await;
            if let Some(tx) = tx {
                tx.send(resolved);
            }
        });
        reply
    }
}

impl Message<Resolve> for Approval {
    type Reply = bool;

    async fn handle(&mut self, m: Resolve, _: &mut Context<Self, Self::Reply>) -> bool {
        self.resolve(m.id, m.decision, Answerer::Human).await
    }
}

impl Message<ResolveBy> for Approval {
    type Reply = bool;

    async fn handle(&mut self, m: ResolveBy, _: &mut Context<Self, Self::Reply>) -> bool {
        self.resolve(m.id, m.decision, m.by).await
    }
}

impl Message<Dismiss> for Approval {
    type Reply = bool;

    /// 207: the parked call is not run; the row settles `answered` with
    /// `"dismissed"` by the human.
    async fn handle(&mut self, m: Dismiss, _: &mut Context<Self, Self::Reply>) -> bool {
        let Some(p) = self.pending.remove(&m.id) else {
            return false;
        };
        let delivered = p.tx.send(Verdict::Dismissed).is_ok();
        self.settle(
            p.row,
            QuestionState::Answered,
            Some(serde_json::json!("dismissed")),
            Some(Answerer::Human),
        )
        .await;
        delivered
    }
}

impl Message<Expire> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        if let Some(p) = self.pending.remove(&m.0) {
            self.settle(p.row, QuestionState::TimedOut, None, None)
                .await;
        }
    }
}

/// 183 R1 (H1): `ask` — `session` left this engine (released to a taker),
/// so its approvals stop waiting here: the waiter sees its sender dropped
/// and publishes `ApprovalResolved` (deny, not timed out), which clears the
/// card on every client. The `questions` row is left `pending`, untouched:
/// the taker's resume re-asks the call. Shared with the ask actor.
#[derive(Clone, Copy, Debug)]
pub struct ForgetSession(pub SessionId);

impl Message<ForgetSession> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: ForgetSession, _: &mut Context<Self, Self::Reply>) {
        self.pending.retain(|_, p| p.requested.session != m.0);
    }
}

/// `ask` → the still-pending approvals, for the `subscribe_events` snapshot a
/// client gets on (re)subscribe (31 §7.3).
pub struct PendingApprovals;

impl Message<PendingApprovals> for Approval {
    type Reply = Vec<ApprovalRequested>;

    async fn handle(
        &mut self,
        _: PendingApprovals,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.pending.values().map(|p| p.requested.clone()).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use futures::future::BoxFuture;
    use kameo::actor::Spawn;

    use super::*;
    use crate::jobs::lock;

    /// A journal that keeps every row it was handed (109 W3 tests).
    #[derive(Default)]
    pub(crate) struct RecJournal {
        pub(crate) questions: Mutex<Vec<QuestionRow>>,
        /// `(call_id, Some(row))` on put, `(call_id, None)` on clear.
        pub(crate) waits: Mutex<Vec<(String, Option<WaitRow>)>>,
    }

    impl RecJournal {
        pub(crate) fn questions(&self) -> Vec<QuestionRow> {
            lock(&self.questions).clone()
        }

        pub(crate) fn waits(&self) -> Vec<(String, Option<WaitRow>)> {
            lock(&self.waits).clone()
        }
    }

    impl Journal for RecJournal {
        fn call_started(&self, _: SessionId, _: &str) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn call_finished(&self, _: SessionId, _: &str, _: &crate::ToolOutput) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn question(&self, q: QuestionRow) -> BoxFuture<'_, ()> {
            lock(&self.questions).push(q);
            Box::pin(async {})
        }
        fn wait(&self, w: Option<WaitRow>, _: SessionId, call_id: &str) -> BoxFuture<'_, ()> {
            lock(&self.waits).push((call_id.to_owned(), w));
            Box::pin(async {})
        }
        fn job(&self, _: crate::JobRow) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn pty_screen(&self, _: crate::PtyScreenRow) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    /// Bus subscriber counting the cards it sees.
    #[derive(kameo::Actor)]
    pub(crate) struct Seen(pub(crate) Arc<Mutex<Vec<ApprovalId>>>);

    impl Message<ApprovalRequested> for Seen {
        type Reply = ();
        async fn handle(&mut self, m: ApprovalRequested, _: &mut Context<Self, ()>) {
            lock(&self.0).push(m.id);
        }
    }

    async fn actor(
        timeout: Duration,
    ) -> (Runtime, kameo::actor::ActorRef<Approval>, Arc<RecJournal>) {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let journal = Arc::new(RecJournal::default());
        let actor = Approval::spawn(Approval::new(rt.clone(), timeout));
        actor
            .ask(crate::InstallJournal(journal.clone()))
            .await
            .unwrap();
        (rt, actor, journal)
    }

    fn request(session: SessionId, call_id: Option<&str>) -> RequestApproval {
        RequestApproval {
            session,
            tool: "shell".into(),
            args: serde_json::json!({ "command": "rm x" }),
            risk: Risk::Destructive,
            call_id: call_id.map(Into::into),
        }
    }

    /// 183 R1 (H1): a released session's approval stops waiting on the old
    /// owner — no longer pending (a reload's snapshot drops it), its card is
    /// resolved for the clients, and its row stays `pending` for the taker.
    #[tokio::test]
    async fn forget_session_drops_the_card_and_keeps_the_row_pending() {
        let (_rt, actor, journal) = actor(Duration::from_secs(30)).await;
        let (session, other) = (SessionId::new(), SessionId::new());
        for s in [session, other] {
            let a = actor.clone();
            tokio::spawn(async move { a.ask(request(s, Some("c1"))).await });
        }
        while actor.ask(PendingApprovals).await.unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        actor.ask(ForgetSession(session)).await.unwrap();
        let left = actor.ask(PendingApprovals).await.unwrap();
        assert_eq!(left.iter().map(|r| r.session).collect::<Vec<_>>(), [other]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            journal
                .questions()
                .iter()
                .all(|r| r.state == QuestionState::Pending),
            "no row is settled by the forget"
        );
    }

    /// §6.1: a raised approval is a `pending` row with the card as payload and
    /// a `waits(question)` row for its call; the answer makes it `answered`
    /// with the decision and clears the wait.
    #[tokio::test]
    async fn question_row_pending_then_answered() {
        let (_rt, actor, journal) = actor(Duration::from_secs(5)).await;
        let session = SessionId::new();
        let pending = tokio::spawn({
            let actor = actor.clone();
            async move { actor.ask(request(session, Some("c1"))).await.unwrap() }
        });
        let id = loop {
            if let Some(r) = actor.ask(PendingApprovals).await.unwrap().first() {
                break r.id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let rows = journal.questions();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id, id.0.to_string());
        assert_eq!(
            (row.kind, row.state),
            (QuestionKind::Approval, QuestionState::Pending)
        );
        assert_eq!(row.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(row.payload["tool"], "shell");
        assert_eq!(row.deadline - row.asked_at, chrono::Duration::seconds(5));
        let waits = journal.waits();
        assert_eq!(waits.len(), 1);
        let w = waits[0].1.as_ref().unwrap();
        assert_eq!(
            (w.kind, w.target.as_str()),
            (WaitKind::Question, row.id.as_str())
        );

        assert!(
            actor
                .ask(Resolve {
                    id,
                    decision: Decision::Allow
                })
                .await
                .unwrap()
        );
        let resolved = pending.await.unwrap();
        assert_eq!(
            (resolved.decision, resolved.timed_out),
            (Decision::Allow, false)
        );
        let rows = journal.questions();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].state, QuestionState::Answered);
        assert_eq!(rows[1].answer, Some(serde_json::json!("allow")));
        assert!(rows[1].answered_at.is_some());
        assert_eq!(rows[1].answered_by, Some(Answerer::Human));
        assert_eq!(
            journal
                .waits()
                .last()
                .map(|(c, w)| (c.as_str(), w.is_none())),
            Some(("c1", true))
        );
        // §6.3: an answer for an id no longer pending is refused.
        assert!(
            !actor
                .ask(Resolve {
                    id,
                    decision: Decision::Deny
                })
                .await
                .unwrap()
        );
    }

    /// 126 §4.3: the double's answer settles the row with `answered_by =
    /// double`; the `answer` JSON keeps the bare `Decision` shape.
    #[tokio::test]
    async fn resolve_by_double_names_the_answerer() {
        let (_rt, actor, journal) = actor(Duration::from_secs(5)).await;
        let session = SessionId::new();
        let pending = tokio::spawn({
            let actor = actor.clone();
            async move { actor.ask(request(session, Some("c1"))).await.unwrap() }
        });
        let id = loop {
            if let Some(r) = actor.ask(PendingApprovals).await.unwrap().first() {
                break r.id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(
            actor
                .ask(ResolveBy {
                    id,
                    decision: Decision::Allow,
                    by: Answerer::Double,
                })
                .await
                .unwrap()
        );
        let resolved = pending.await.unwrap();
        assert_eq!(
            (resolved.decision, resolved.timed_out),
            (Decision::Allow, false)
        );
        let rows = journal.questions();
        assert_eq!(rows[1].state, QuestionState::Answered);
        assert_eq!(rows[1].answer, Some(serde_json::json!("allow")));
        assert_eq!(rows[1].answered_by, Some(Answerer::Double));
    }

    /// §6.3: nobody answers → `Deny`, `timed_out`, and the row is `timed_out`.
    #[tokio::test]
    async fn deadline_elapsed_is_timeout() {
        let (_rt, actor, journal) = actor(Duration::from_millis(20)).await;
        let r = actor
            .ask(request(SessionId::new(), Some("c1")))
            .await
            .unwrap();
        assert_eq!((r.decision, r.timed_out), (Decision::Deny, true));
        let rows = journal.questions();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].state, QuestionState::TimedOut);
        assert!(rows[1].answer.is_none());
        assert!(rows[1].answered_by.is_none());
        assert!(actor.ask(PendingApprovals).await.unwrap().is_empty());
    }

    /// §6.2 (D-call): a stored answer for the call is honoured — no card is
    /// published, no row is written, the decision comes back at once; it is
    /// consumed by that one replay, and another call still asks.
    #[tokio::test]
    async fn answered_row_honoured_on_replay() {
        let (rt, actor, journal) = actor(Duration::from_millis(50)).await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sub = Seen::spawn(Seen(seen.clone()));
        rt.subscribe::<Seen, ApprovalRequested>(&sub).await;
        let session = SessionId::new();
        actor
            .tell(HonourAnswer {
                session,
                call_id: "c1".into(),
                decision: Decision::Deny,
                dismissed: false,
            })
            .await
            .unwrap();
        let r = actor.ask(request(session, Some("c1"))).await.unwrap();
        assert_eq!(
            (r.decision, r.timed_out, r.dismissed),
            (Decision::Deny, false, false)
        );
        assert!(journal.questions().is_empty(), "no second question");
        // Consumed: the same call asks again normally (and times out here).
        let r = actor.ask(request(session, Some("c1"))).await.unwrap();
        assert!(r.timed_out);
        assert_eq!(lock(&seen).len(), 1, "one card for the real ask");
        assert_eq!(journal.questions().len(), 2);
    }

    /// Bus subscriber keeping every resolution it sees.
    #[derive(kameo::Actor)]
    struct SeenResolved(Arc<Mutex<Vec<ApprovalResolved>>>);

    impl Message<ApprovalResolved> for SeenResolved {
        type Reply = ();
        async fn handle(&mut self, m: ApprovalResolved, _: &mut Context<Self, ()>) {
            lock(&self.0).push(m);
        }
    }

    /// 207: `Dismiss` leaves a pending call to the agent: the waiter and the
    /// bus see a dismissed deny (not a timeout), the row is `answered` with
    /// `"dismissed"` by the human, and a second `Dismiss` (or an unknown id)
    /// is refused.
    #[tokio::test]
    async fn dismiss_leaves_the_call_to_the_agent() {
        let (rt, actor, journal) = actor(Duration::from_secs(5)).await;
        let published = Arc::new(Mutex::new(Vec::new()));
        let sub = SeenResolved::spawn(SeenResolved(published.clone()));
        rt.subscribe::<SeenResolved, ApprovalResolved>(&sub).await;
        let session = SessionId::new();
        let pending = tokio::spawn({
            let actor = actor.clone();
            async move { actor.ask(request(session, Some("c1"))).await.unwrap() }
        });
        let id = loop {
            if let Some(r) = actor.ask(PendingApprovals).await.unwrap().first() {
                break r.id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(actor.ask(Dismiss { id }).await.unwrap());
        let r = pending.await.unwrap();
        assert_eq!(
            (r.id, r.decision, r.timed_out, r.dismissed),
            (id, Decision::Deny, false, true)
        );
        let event = loop {
            if let Some(e) = lock(&published).first().cloned() {
                break e;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            (event.id, event.decision, event.timed_out, event.dismissed),
            (id, Decision::Deny, false, true)
        );
        let rows = journal.questions();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].state, QuestionState::Answered);
        assert_eq!(rows[1].answer, Some(serde_json::json!("dismissed")));
        assert_eq!(rows[1].answered_by, Some(Answerer::Human));
        assert!(actor.ask(PendingApprovals).await.unwrap().is_empty());
        assert!(!actor.ask(Dismiss { id }).await.unwrap(), "already gone");
        assert!(
            !actor
                .ask(Dismiss {
                    id: ApprovalId(uuid::Uuid::now_v7())
                })
                .await
                .unwrap(),
            "unknown id"
        );
    }

    /// 207: a stored dismissal replays as dismissed — no card, no row.
    #[tokio::test]
    async fn dismissed_answer_honoured_on_replay() {
        let (_rt, actor, journal) = actor(Duration::from_millis(50)).await;
        let session = SessionId::new();
        actor
            .tell(HonourAnswer {
                session,
                call_id: "c1".into(),
                decision: Decision::Deny,
                dismissed: true,
            })
            .await
            .unwrap();
        let r = actor.ask(request(session, Some("c1"))).await.unwrap();
        assert_eq!(
            (r.decision, r.timed_out, r.dismissed),
            (Decision::Deny, false, true)
        );
        assert!(journal.questions().is_empty(), "no second question");
    }
}
