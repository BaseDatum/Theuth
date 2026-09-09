//! `Approval` actor, registered as [`crate::APPROVAL_ACTOR`]. Publishes
//! [`ApprovalRequested`] and waits for a [`Resolve`] from any UI.

use std::collections::HashMap;
use std::time::Duration;

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::Risk;

/// Correlates a request with its resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct ApprovalId(pub uuid::Uuid);

/// Human answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
}

/// Internal: drop a pending entry whose waiter gave up.
struct Expire(ApprovalId);

/// The approval actor.
#[derive(kameo::Actor)]
pub struct Approval {
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<ApprovalId, oneshot::Sender<Decision>>,
}

impl Approval {
    /// Build with the runtime (for publishing) and the human timeout.
    pub fn new(rt: Runtime, timeout: Duration) -> Self {
        Self {
            rt,
            timeout,
            pending: HashMap::new(),
        }
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
        let (dtx, drx) = oneshot::channel();
        self.pending.insert(id, dtx);
        let rt = self.rt.clone();
        let timeout = self.timeout;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(ApprovalRequested {
                id,
                session: m.session,
                tool: m.tool,
                args: m.args,
                risk: m.risk,
            })
            .await;
            let (decision, timed_out) = match tokio::time::timeout(timeout, drx).await {
                Ok(Ok(d)) => (d, false),
                // Sender dropped: actor died; treat as deny.
                Ok(Err(_)) => (Decision::Deny, false),
                Err(_) => {
                    let _ = me.tell(Expire(id)).await;
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
        match self.pending.remove(&m.id) {
            Some(tx) => tx.send(m.decision).is_ok(),
            None => false,
        }
    }
}

impl Message<Expire> for Approval {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        self.pending.remove(&m.0);
    }
}
