//! Bus → `broadcast` bridge. One actor subscribed to every wire-visible event
//! type serializes each into a JSON value; every RPC subscription task owns a
//! `broadcast::Receiver` and filters by session.

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use a2a::A2aChanged;
use agent::ask::NotebookAsk;
use agent::{
    AgentEvent, CommunicationChanged, CostCubeChanged, EndlessProgress, LoginFinished,
    PeerDelivered, PeerMessageChanged,
};
use app::AppCallChanged;
use artifact::ArtifactShown;
use artifact::preview::{PagePreviewRequested, PagePreviewResolved};
use artifact::snapshot::{FenceSnapshotRequested, FenceSnapshotResolved};
use autoresearch::StudyEvent;
use browser::BrowserEvent;
use cells::CellEvent; // 171:C
use codeindex::CodeindexEvent;
use document::{DocumentChanged, SourceEvent};
use double::{DoubleAnswered, DoublePredicted, DoubleTrainingState, DoubleVersionActivated};
use dreaming::{DreamNightFinished, DreamNightStarted, DreamUndone};
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use knowledge::KnowledgeChanged;
use link::LinkEvent;
use meeting::{BriefEvent, CalendarEvent, MeetingEvent};
use mesh::MeshEvent;
use nodes::NodeEvent;
use roster::attention::AttentionChanged;
use runtime::Runtime;
use sandbox::SandboxEvent;
use schedule::ScheduleEvent;
use secrets::SecretsEvent;
use serde::Serialize;
use signal::SignalEvent;
use slack::SlackEvent;
use talk::TalkEvent;
use tasks::activity::ActivityAppended;
use telegram::TelegramEvent;
use tokio::sync::broadcast;
use tools::{
    ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, JobEvent, McpProgress,
    ReminderEvent,
};
use update::UpdateReady;
use voice::VoiceEvent;
use web::progress::ScrapeProgress;

/// One serialized event; shared across every subscription without copying.
pub type Event = Arc<serde_json::Value>;

/// Registry name of the one [`Bridge`] per process.
pub const BRIDGE_ACTOR: &str = "rpc.events";

/// Receivers that fall more than this many events behind see a `Lagged`.
/// Since 129 every subscription drains its receiver on a task of its own, so
/// a slow client no longer lags the ring; only a stalled drain does.
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
    /// 171F B15: live user-cell kernel jobs. Their `JobEvent`s stay off the
    /// client feed (a kernel is not the session's terminal); the bus still
    /// carries them to the supervisor's cell watch.
    cells: HashSet<(runtime::SessionId, tools::JobId)>,
}

impl Actor for Bridge {
    type Args = broadcast::Sender<Event>;
    type Error = Infallible;

    async fn on_start(tx: Self::Args, _: ActorRef<Self>) -> Result<Self, Infallible> {
        Ok(Self {
            tx,
            cells: HashSet::new(),
        })
    }
}

impl Message<JobEvent> for Bridge {
    type Reply = ();
    async fn handle(&mut self, ev: JobEvent, _: &mut Context<Self, ()>) {
        let cell = match &ev {
            JobEvent::Started {
                session, id, name, ..
            } => {
                let cell = name
                    .as_deref()
                    .is_some_and(|n| n.starts_with(tools::jobs::CELL_PREFIX));
                if cell {
                    self.cells.insert((*session, id.clone()));
                }
                cell
            }
            JobEvent::Output { session, id, .. }
            | JobEvent::Screen { session, id, .. }
            | JobEvent::Idle { session, id, .. } => self.cells.contains(&(*session, id.clone())),
            JobEvent::Finished { session, id, .. } => self.cells.remove(&(*session, id.clone())),
        };
        if !cell {
            self.forward(&ev);
        }
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
    SignalEvent,
    SlackEvent,
    TelegramEvent,
    KnowledgeChanged,
    DocumentChanged,
    SourceEvent,
    NotebookAsk,
    BrowserEvent,
    LinkEvent,
    NodeEvent,
    ScheduleEvent,
    ArtifactShown,
    PagePreviewRequested,
    PagePreviewResolved,
    FenceSnapshotRequested,
    FenceSnapshotResolved,
    SecretsEvent,
    ScrapeProgress,
    UpdateReady,
    VoiceEvent,
    TalkEvent,
    MeetingEvent,
    BriefEvent,
    CalendarEvent,
    AppCallChanged,
    ActivityAppended,
    CodeindexEvent,
    MeshEvent,
    SandboxEvent,
    DoublePredicted,
    DoubleAnswered,
    DoubleTrainingState,
    DoubleVersionActivated,
    AttentionChanged,
    ReminderEvent,
    DreamNightStarted,
    DreamNightFinished,
    DreamUndone,
    StudyEvent,
    CellEvent,
    EndlessProgress,
    CostCubeChanged
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
    // 120 §11.4: `page_preview` short-circuits to "no renderer" when nobody subscribes.
    artifact::preview::watch_feed(tx.clone());
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
    rt.subscribe::<Bridge, TelegramEvent>(&actor).await;
    rt.subscribe::<Bridge, KnowledgeChanged>(&actor).await;
    rt.subscribe::<Bridge, DocumentChanged>(&actor).await;
    rt.subscribe::<Bridge, SourceEvent>(&actor).await;
    rt.subscribe::<Bridge, NotebookAsk>(&actor).await;
    rt.subscribe::<Bridge, BrowserEvent>(&actor).await;
    rt.subscribe::<Bridge, LinkEvent>(&actor).await;
    rt.subscribe::<Bridge, NodeEvent>(&actor).await;
    rt.subscribe::<Bridge, ScheduleEvent>(&actor).await;
    rt.subscribe::<Bridge, ArtifactShown>(&actor).await;
    rt.subscribe::<Bridge, PagePreviewRequested>(&actor).await;
    rt.subscribe::<Bridge, PagePreviewResolved>(&actor).await;
    rt.subscribe::<Bridge, FenceSnapshotRequested>(&actor).await;
    rt.subscribe::<Bridge, FenceSnapshotResolved>(&actor).await;
    rt.subscribe::<Bridge, SecretsEvent>(&actor).await;
    rt.subscribe::<Bridge, ScrapeProgress>(&actor).await;
    rt.subscribe::<Bridge, UpdateReady>(&actor).await;
    rt.subscribe::<Bridge, VoiceEvent>(&actor).await;
    rt.subscribe::<Bridge, TalkEvent>(&actor).await;
    rt.subscribe::<Bridge, MeetingEvent>(&actor).await;
    rt.subscribe::<Bridge, BriefEvent>(&actor).await;
    rt.subscribe::<Bridge, CalendarEvent>(&actor).await;
    rt.subscribe::<Bridge, AppCallChanged>(&actor).await;
    rt.subscribe::<Bridge, ActivityAppended>(&actor).await;
    rt.subscribe::<Bridge, CodeindexEvent>(&actor).await;
    rt.subscribe::<Bridge, MeshEvent>(&actor).await;
    rt.subscribe::<Bridge, SandboxEvent>(&actor).await;
    rt.subscribe::<Bridge, DoublePredicted>(&actor).await;
    rt.subscribe::<Bridge, DoubleAnswered>(&actor).await;
    rt.subscribe::<Bridge, DoubleTrainingState>(&actor).await;
    rt.subscribe::<Bridge, DoubleVersionActivated>(&actor).await;
    rt.subscribe::<Bridge, AttentionChanged>(&actor).await;
    rt.subscribe::<Bridge, DreamNightStarted>(&actor).await;
    rt.subscribe::<Bridge, DreamNightFinished>(&actor).await;
    rt.subscribe::<Bridge, DreamUndone>(&actor).await;
    rt.subscribe::<Bridge, StudyEvent>(&actor).await;
    rt.subscribe::<Bridge, CellEvent>(&actor).await; // 171:C
    rt.subscribe::<Bridge, ReminderEvent>(&actor).await;
    rt.subscribe::<Bridge, EndlessProgress>(&actor).await;
    rt.subscribe::<Bridge, CostCubeChanged>(&actor).await;
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::{JobExit, JobId, JobKind};

    /// 171F B15: a user-cell kernel's `JobEvent`s never reach clients (no
    /// terminal glyph); an agent PTY's pass, and a reused id after the
    /// kernel's `Finished` passes again.
    #[tokio::test]
    async fn cell_kernel_job_events_stay_off_the_client_feed() {
        let (tx, mut rx) = broadcast::channel(16);
        let b = Bridge::spawn(tx);
        let s = runtime::SessionId::new();
        let job = |id: &str| JobId(id.into());
        let started = |id: &str, name: &str| JobEvent::Started {
            session: s,
            id: job(id),
            command: String::new(),
            call_id: String::new(),
            log: "x.log".into(),
            name: Some(name.into()),
            pty: true,
            kind: JobKind::default(),
        };
        for (id, name) in [("j1", "cell:bash 5.2"), ("j2", "dev")] {
            b.ask(started(id, name)).await.unwrap();
            b.ask(JobEvent::Screen {
                session: s,
                id: job(id),
                rows: Vec::new(),
                cursor: (0, 0),
            })
            .await
            .unwrap();
            b.ask(JobEvent::Output {
                session: s,
                id: job(id),
                chunk: "x".into(),
            })
            .await
            .unwrap();
            b.ask(JobEvent::Finished {
                session: s,
                id: job(id),
                command: String::new(),
                exit: JobExit::Killed,
                secs: 0,
                output: String::new(),
                inline: true,
                pty: true,
                kind: JobKind::default(),
            })
            .await
            .unwrap();
        }
        b.ask(started("j1", "later")).await.unwrap();
        let mut seen = Vec::new();
        while let Ok(v) = rx.try_recv() {
            seen.push(format!(
                "{} {}",
                v["type"].as_str().unwrap(),
                v["id"].as_str().unwrap()
            ));
        }
        assert_eq!(
            seen,
            [
                "JobStarted j2",
                "JobScreen j2",
                "JobOutput j2",
                "JobFinished j2",
                "JobStarted j1"
            ]
        );
    }
}
