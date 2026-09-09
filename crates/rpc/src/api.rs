//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; the subscription drains a `broadcast::Receiver` fed by
//! [`crate::bridge`].

use std::fmt::Display;

use agent::{
    Agents, Cancel, HistoryEntry, ListSessions, LoadSession, SendInput, SessionSummary, Steer,
};
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::Message;
use kameo::reply::Reply;
use runtime::{Runtime, SessionId};
use tokio::sync::broadcast;
use tools::{Approval, ApprovalId, Decision, ListTools, Resolve, ToolRegistry, ToolSpec};

use crate::bridge::Event;

/// `ask` failed (handler error, actor stopped mid-call).
const ASK_FAILED: i32 = -32000;
/// The named actor is not registered.
const NOT_RUNNING: i32 = -32001;

/// The wire API. `#[rpc(server)]` generates `ApiServer` (+ `into_rpc()`).
#[rpc(server)]
pub trait Api {
    /// Start a turn; `session: None` creates a session. Returns once queued.
    #[method(name = "send_input")]
    async fn send_input(&self, session: Option<SessionId>, text: String) -> RpcResult<SessionId>;
    /// Queue a user message after the current tool batch.
    #[method(name = "steer")]
    async fn steer(&self, session: SessionId, text: String) -> RpcResult<()>;
    /// Abort the in-flight turn.
    #[method(name = "cancel")]
    async fn cancel(&self, session: SessionId) -> RpcResult<()>;
    /// Every session, newest first.
    #[method(name = "list_sessions")]
    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>>;
    /// Subagent sessions of `parent`, newest first, with their peer names.
    #[method(name = "list_children")]
    async fn list_children(&self, parent: SessionId) -> RpcResult<Vec<SessionSummary>>;
    /// Rendered history of one session.
    #[method(name = "load_session")]
    async fn load_session(&self, session: SessionId) -> RpcResult<Vec<HistoryEntry>>;
    /// Every registered tool.
    #[method(name = "list_tools")]
    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>>;
    /// Answer a pending approval; `true` if it was still pending.
    #[method(name = "resolve_approval")]
    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool>;
    /// Stop the whole process.
    #[method(name = "shutdown")]
    async fn shutdown(&self) -> RpcResult<()>;
    /// Stream every wire-visible event as `{"type": ..., ...}`, optionally
    /// only those carrying `session`. A slow client sees
    /// `{"type":"Lagged","count":n}` where `n` events were dropped.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value)]
    async fn subscribe_events(&self, session: Option<SessionId>) -> SubscriptionResult;
}

/// Server state: the event feed and the runtime (for `shutdown`).
pub struct Server {
    /// Feed from [`crate::bridge`].
    pub events: broadcast::Sender<Event>,
    /// `rt.shutdown` is cancelled by `shutdown`.
    pub rt: Runtime,
}

fn lookup<A: Actor>(name: &str) -> RpcResult<ActorRef<A>> {
    match ActorRef::<A>::lookup(name) {
        Ok(Some(a)) => Ok(a),
        Ok(None) => Err(ErrorObjectOwned::owned(
            NOT_RUNNING,
            format!("{name} not running"),
            None::<()>,
        )),
        Err(e) => Err(ErrorObjectOwned::owned(
            ASK_FAILED,
            e.to_string(),
            None::<()>,
        )),
    }
}

/// Look up `name`, `ask` it `msg`, map every failure to a JSON-RPC error.
async fn ask<A, M>(name: &str, msg: M) -> RpcResult<<A::Reply as Reply>::Ok>
where
    A: Actor + Message<M>,
    M: Send + 'static,
    <A::Reply as Reply>::Error: Display,
{
    let actor = lookup::<A>(name)?;
    actor.ask(msg).await.map_err(|e: SendError<M, _>| {
        ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>)
    })
}

#[async_trait]
impl ApiServer for Server {
    async fn send_input(&self, session: Option<SessionId>, text: String) -> RpcResult<SessionId> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SendInput { session, text }).await
    }

    async fn steer(&self, session: SessionId, text: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Steer { session, text }).await
    }

    async fn cancel(&self, session: SessionId) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Cancel { session }).await
    }

    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await
    }

    async fn list_children(&self, parent: SessionId) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::ListChildren { parent }).await
    }

    async fn load_session(&self, session: SessionId) -> RpcResult<Vec<HistoryEntry>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, LoadSession { session }).await
    }

    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, ListTools).await
    }

    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool> {
        ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision }).await
    }

    async fn shutdown(&self) -> RpcResult<()> {
        self.rt.shutdown.cancel();
        Ok(())
    }

    async fn subscribe_events(
        &self,
        pending: PendingSubscriptionSink,
        session: Option<SessionId>,
    ) -> SubscriptionResult {
        // Subscribe before accept so nothing published in between is missed.
        let mut rx = self.events.subscribe();
        let sink = pending.accept().await?;
        let session = session.map(|s| s.to_string());
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    _ = sink.closed() => return,
                    r = rx.recv() => match r {
                        Ok(ev) => {
                            let mine = match (&session, ev.get("session").and_then(|s| s.as_str())) {
                                (Some(want), Some(have)) => want == have,
                                _ => true,
                            };
                            if !mine {
                                continue;
                            }
                            serde_json::value::to_raw_value(&*ev)
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            serde_json::value::to_raw_value(&serde_json::json!({ "type": "Lagged", "count": n }))
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                };
                let Ok(raw) = msg else { continue };
                if sink.send(SubscriptionMessage::from(raw)).await.is_err() {
                    return;
                }
            }
        });
        Ok(())
    }
}
