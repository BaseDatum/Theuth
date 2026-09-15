//! Bus → `broadcast` bridge. One actor subscribed to every wire-visible event
//! type serializes each into a JSON value; every RPC subscription task owns a
//! `broadcast::Receiver` and filters by session.

use std::convert::Infallible;
use std::sync::Arc;

use agent::{AgentEvent, LoginFinished};
use browser::BrowserEvent;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use knowledge::KnowledgeChanged;
use runtime::Runtime;
use serde::Serialize;
use signal::SignalEvent;
use slack::SlackEvent;
use tokio::sync::broadcast;
use tools::{ApprovalRequested, ApprovalResolved, JobEvent, McpProgress};

/// One serialized event; shared across every subscription without copying.
pub type Event = Arc<serde_json::Value>;

/// Receivers that fall more than this many events behind see a `Lagged`.
const CAPACITY: usize = 1024;

/// The bridge actor.
pub struct Bridge {
    tx: broadcast::Sender<Event>,
}

impl Actor for Bridge {
    type Args = broadcast::Sender<Event>;
    type Error = Infallible;

    async fn on_start(tx: Self::Args, _: ActorRef<Self>) -> Result<Self, Infallible> {
        Ok(Self { tx })
    }
}

impl Bridge {
    fn forward<T: Serialize>(&self, ev: &T) {
        match serde_json::to_value(ev) {
            // `Err` only means no receiver is listening right now.
            Ok(v) => drop(self.tx.send(Arc::new(v))),
            Err(e) => tracing::warn!(error = %e, "event not serializable"),
        }
    }
}

macro_rules! forward {
    ($($t:ty),*) => {$(
        impl Message<$t> for Bridge {
            type Reply = ();
            async fn handle(&mut self, ev: $t, _: &mut Context<Self, ()>) {
                self.forward(&ev);
            }
        }
    )*};
}
forward!(
    AgentEvent,
    LoginFinished,
    ApprovalRequested,
    ApprovalResolved,
    McpProgress,
    JobEvent,
    SignalEvent,
    SlackEvent,
    KnowledgeChanged,
    BrowserEvent
);

/// Spawn the bridge, subscribe it to every event type, return the sender to
/// `subscribe()` from.
pub async fn start(rt: &Runtime) -> broadcast::Sender<Event> {
    let (tx, _) = broadcast::channel(CAPACITY);
    let actor = Bridge::spawn_with_mailbox(tx.clone(), runtime::subscriber_mailbox());
    rt.subscribe::<Bridge, AgentEvent>(&actor).await;
    rt.subscribe::<Bridge, LoginFinished>(&actor).await;
    rt.subscribe::<Bridge, ApprovalRequested>(&actor).await;
    rt.subscribe::<Bridge, ApprovalResolved>(&actor).await;
    rt.subscribe::<Bridge, McpProgress>(&actor).await;
    rt.subscribe::<Bridge, JobEvent>(&actor).await;
    rt.subscribe::<Bridge, SignalEvent>(&actor).await;
    rt.subscribe::<Bridge, SlackEvent>(&actor).await;
    rt.subscribe::<Bridge, KnowledgeChanged>(&actor).await;
    rt.subscribe::<Bridge, BrowserEvent>(&actor).await;
    tx
}
