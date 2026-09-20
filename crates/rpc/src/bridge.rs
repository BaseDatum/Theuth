//! Bus → `broadcast` bridge. One actor subscribed to every wire-visible event
//! type serializes each into a JSON value; every RPC subscription task owns a
//! `broadcast::Receiver` and filters by session.

use std::convert::Infallible;
use std::sync::Arc;

use a2a::A2aChanged;
use agent::{AgentEvent, CommunicationChanged, LoginFinished, PeerDelivered, PeerMessageChanged};
use app::AppCallChanged;
use artifact::ArtifactShown;
use browser::BrowserEvent;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use knowledge::KnowledgeChanged;
use link::LinkEvent;
use meeting::MeetingEvent;
use nodes::NodeEvent;
use runtime::Runtime;
use schedule::ScheduleEvent;
use secrets::SecretsEvent;
use serde::Serialize;
use signal::SignalEvent;
use slack::SlackEvent;
use tokio::sync::broadcast;
use tools::{
    ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, JobEvent, McpProgress,
};
use update::UpdateReady;
use voice::VoiceEvent;
use web::progress::ScrapeProgress;

/// One serialized event; shared across every subscription without copying.
pub type Event = Arc<serde_json::Value>;

/// Registry name of the one [`Bridge`] per process.
pub const BRIDGE_ACTOR: &str = "rpc.events";

/// Receivers that fall more than this many events behind see a `Lagged`.
const CAPACITY: usize = 1024;

/// `ask` → a clone of the bridge's `broadcast::Sender`, for a second plugin
/// (link) that wants the same feed the rpc server drains.
pub struct Feed;

impl Message<Feed> for Bridge {
    type Reply = broadcast::Sender<Event>;
    async fn handle(&mut self, _: Feed, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.tx.clone()
    }
}

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
    CommunicationChanged,
    PeerMessageChanged,
    PeerDelivered,
    A2aChanged,
    ApprovalRequested,
    ApprovalResolved,
    AskRequested,
    AskResolved,
    McpProgress,
    JobEvent,
    SignalEvent,
    SlackEvent,
    KnowledgeChanged,
    BrowserEvent,
    LinkEvent,
    NodeEvent,
    ScheduleEvent,
    ArtifactShown,
    SecretsEvent,
    ScrapeProgress,
    UpdateReady,
    VoiceEvent,
    MeetingEvent,
    AppCallChanged
);

/// The process-wide event feed. The first caller spawns the bridge,
/// registers it as [`BRIDGE_ACTOR`] and subscribes it to every event type;
/// later callers (a second plugin, a reconnecting server) get the same
/// sender, so each bus event is serialized once.
pub async fn start(rt: &Runtime) -> broadcast::Sender<Event> {
    if let Ok(Some(actor)) = ActorRef::<Bridge>::lookup(BRIDGE_ACTOR)
        && let Ok(tx) = actor.ask(Feed).await
    {
        return tx;
    }
    let (tx, _) = broadcast::channel(CAPACITY);
    let actor = Bridge::spawn_with_mailbox(tx.clone(), runtime::subscriber_mailbox());
    if let Err(e) = actor.register(BRIDGE_ACTOR) {
        tracing::warn!(error = %e, "rpc.events already registered; two bridges");
    }
    rt.subscribe::<Bridge, AgentEvent>(&actor).await;
    rt.subscribe::<Bridge, LoginFinished>(&actor).await;
    rt.subscribe::<Bridge, CommunicationChanged>(&actor).await;
    rt.subscribe::<Bridge, PeerMessageChanged>(&actor).await;
    rt.subscribe::<Bridge, PeerDelivered>(&actor).await;
    rt.subscribe::<Bridge, A2aChanged>(&actor).await;
    rt.subscribe::<Bridge, ApprovalRequested>(&actor).await;
    rt.subscribe::<Bridge, ApprovalResolved>(&actor).await;
    rt.subscribe::<Bridge, AskRequested>(&actor).await;
    rt.subscribe::<Bridge, AskResolved>(&actor).await;
    rt.subscribe::<Bridge, McpProgress>(&actor).await;
    rt.subscribe::<Bridge, JobEvent>(&actor).await;
    rt.subscribe::<Bridge, SignalEvent>(&actor).await;
    rt.subscribe::<Bridge, SlackEvent>(&actor).await;
    rt.subscribe::<Bridge, KnowledgeChanged>(&actor).await;
    rt.subscribe::<Bridge, BrowserEvent>(&actor).await;
    rt.subscribe::<Bridge, LinkEvent>(&actor).await;
    rt.subscribe::<Bridge, NodeEvent>(&actor).await;
    rt.subscribe::<Bridge, ScheduleEvent>(&actor).await;
    rt.subscribe::<Bridge, ArtifactShown>(&actor).await;
    rt.subscribe::<Bridge, SecretsEvent>(&actor).await;
    rt.subscribe::<Bridge, ScrapeProgress>(&actor).await;
    rt.subscribe::<Bridge, UpdateReady>(&actor).await;
    rt.subscribe::<Bridge, VoiceEvent>(&actor).await;
    rt.subscribe::<Bridge, MeetingEvent>(&actor).await;
    rt.subscribe::<Bridge, AppCallChanged>(&actor).await;
    tx
}
