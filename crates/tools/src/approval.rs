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
use crate::journal::{Journal, QuestionKind, QuestionRow, QuestionState, WaitKind, WaitRow};

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

/// Event: a request was answered (by a human or by timeout). UIs dismiss
/// their prompt on this.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct ApprovalResolved {
    /// The request.
    pub id: ApprovalId,
    /// Outcome.
    pub decision: Decision,
    /// True when nobody answered in time.
    pub timed_out: bool,
}

/// `ask` → `bool`: true if the id was pending and is now resolved.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Resolve {
    /// The request.
    pub id: ApprovalId,
    /// The answer.
    pub decision: Decision,
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
}

/// Internal: drop a pending entry whose waiter gave up.
struct Expire(ApprovalId);

/// One unanswered request: the card, its journal row and the waiter.
struct Pending {
    requested: ApprovalRequested,
    row: QuestionRow,
    tx: oneshot::Sender<Decision>,
}

/// The approval actor.
#[derive(kameo::Actor)]
pub struct Approval {
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<ApprovalId, Pending>,
    /// [`HonourAnswer`]s not yet consumed by their call's replay.
    honoured: HashMap<(SessionId, String), Decision>,
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
    /// and the call's `waits(question)` row goes (§4.4).
    async fn settle(&self, mut row: QuestionRow, state: QuestionState, answer: Option<Decision>) {
        row.state = state;
        row.answered_at = Some(Utc::now());
        row.answer = answer.map(|d| serde_json::json!(d));
        if let Some(call) = &row.tool_call_id {
            self.journal.wait(None, row.session, call).await;
        }
        self.journal.question(row).await;
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
        self.honoured.insert((m.session, m.call_id), m.decision);
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
        if let Some(decision) = m
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
            let (decision, timed_out) = match tokio::time::timeout(timeout, drx).await {
                Ok(Ok(d)) => (d, false),
                // Sender dropped: actor died; treat as deny.
                Ok(Err(_)) => (Decision::Deny, false),
                Err(_) => {
                    let _ = me.ask(Expire(id)).await;
                    (Decision::Deny, true)
                }
            };
            let resolved = ApprovalResolved {
                id,
                decision,
                timed_out,
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
        let Some(p) = self.pending.remove(&m.id) else {
            return false;
        };
        let delivered = p.tx.send(m.decision).is_ok();
        self.settle(p.row, QuestionState::Answered, Some(m.decision))
            .await;
        delivered
    }
}

impl Message<Expire> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        if let Some(p) = self.pending.remove(&m.0) {
            self.settle(p.row, QuestionState::TimedOut, None).await;
        }
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
