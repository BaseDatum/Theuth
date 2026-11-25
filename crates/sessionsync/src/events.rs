//! Live events of a session owned elsewhere (68 §5, worker F). Replication
//! delivers the rows a moment later; this stream exists for the liveness of
//! a UI attached to a front door that is not the owner.
//!
//! One actor per process, [`EventRelay`] (`"sessions.events"`), plays both
//! sides:
//!
//! - **Owner**: a peer opens `theuth/sessions/1` and sends
//!   [`Msg::Events`]`{session, from_seq}`; [`serve`] registers the stream
//!   and the actor, subscribed to [`AgentEvent`] on the bus, writes every
//!   event of that session as [`Msg::Event`]`{session, seq, bytes}` (`bytes`
//!   = the event as JSON, exactly what the RPC bridge serialises) until the
//!   peer closes the stream or ownership leaves this engine.
//! - **Subscriber**: [`follow`] refcounts local interest in a session (the RPC
//!   `subscribe_events` task holds a [`Following`] guard); the first follower
//!   spawns a task that resolves the owner from the replica row, opens the
//!   stream, re-publishes each event on the local bus (so `subscribe_events`
//!   filters work unchanged) and reconnects with backoff, woken early by
//!   `PeerConnected(owner)`. The last guard dropped aborts the task.
//!
//! `seq` is the owner's per-session counter for this process; `from_seq`
//! filters lower seqs on reconnect. There is no replay buffer: the stream is
//! live-only, the log carries history.
//!
//! 183 §5.1 (W-18, W-19): a subscriber opens with [`Msg::Watch`] instead and
//! also gets the session's approvals, asks, jobs and cells as [`Msg::Bus`]
//! (the owner's first frame is a `Watching` ack, then the prompts already
//! pending). An old owner cannot decode `Watch` and closes the stream before
//! any frame: the subscriber falls back to `Events` for that owner until it
//! reconnects. Bus frames are re-published locally as [`Relayed`] (the RPC
//! bridge's feed only, so no channel, double or attention actor on this
//! engine acts on another engine's prompt), and the pending approval and ask
//! cards are kept for [`relayed_prompts`] until their resolution or the end
//! of the stream.
//!
//! 209 §4.4: the owner also relays the [`WIRE_RELAYED`] topics of its own wire feed (`show_web`
//! tiles, page previews, tile calls, `browser_show_element` cards) as `Bus` frames of the event's
//! `type`, so a client on a front door can open a remote-owned session's tile, answer its tile
//! calls and fill its cards.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use agent::{AgentEvent, PeerMessageChanged};
use agent::store::Store;
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use replica::{ALPN_SESSIONS, ByteStream, PeerEvent, PeerId, Transport};
use runtime::{Runtime, SessionId};
use rpc_core::{Following, Unfollow};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tools::{ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, JobEvent};

use crate::types::{MAX_FRAME, Msg};

/// Registry name of the [`EventRelay`] actor.
pub const EVENTS_ACTOR: &str = "sessions.events";

/// Queued events per remote subscriber before the slowest one is dropped.
const QUEUE: usize = 256;
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// `tell`: the mesh transport is up.
pub struct SetTransport(pub Arc<dyn Transport>);

/// `tell`: a local client is subscribed to `session`; follow its owner.
#[derive(Clone, Copy, Debug)]
pub struct Follow(pub SessionId);


/// `tell`: a remote subscriber's stream for `session` is open; events go to `tx`.
pub struct Serve {
    /// The session.
    pub session: String,
    /// First seq wanted.
    pub from_seq: u64,
    /// The stream's writer.
    pub tx: mpsc::Sender<Msg>,
    /// 183 W-18: opened with [`Msg::Watch`]: [`Msg::Bus`] frames too.
    pub bus: bool,
}

/// 183 §5.1: one [`Msg::Bus`] frame of a followed session, re-published on
/// this engine's bus for the RPC bridge (`kind` as on the wire, `event` its JSON).
#[derive(Clone, Debug)]
pub struct Relayed {
    /// The session it belongs to.
    pub session: SessionId,
    /// `ApprovalRequested` | `ApprovalResolved` | `AskRequested` | `AskResolved` | `JobEvent` |
    /// `CellEvent`, or a [`WIRE_RELAYED`] kind (209 §4.4).
    pub kind: String,
    /// The event as the owner's bridge would serialise it.
    pub event: serde_json::Value,
}

/// Whether a followed owner's stream supports live Watch events.
#[derive(Clone, Debug, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct FollowState {
    /// Followed session, not the relay carrier's admission scope.
    pub session: SessionId,
    /// False for a disconnected stream or an old owner's Events fallback.
    pub live: bool,
}

impl wire::WireEvent for FollowState {
    const TOPIC: &'static str = "sessionsync.follow-state";
    const VERSION: u32 = 1;
    /// 216 §7.3: relay transport, not an event of the world.
    const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
}

/// 209 §4.4: wire events of a session that the owner relays from its own wire feed by topic,
/// as `(kind, topic)`: `show_web` tiles, page previews, tile calls and `browser_show_element`
/// cards raised on the owner reach a client attached to a front door. Their owners (`artifact`,
/// `browser`) declare them, so the relay needs no dependency on either. A frame without a
/// `session` (the `*Resolved` events) goes to the session its request named. Only topics every
/// engine declares belong here: a front door drops a relayed frame whose topic it has not declared.
pub const WIRE_RELAYED: &[(&str, &str)] = &[
    ("WebViewOpened", "artifact.web-view-opened"),
    ("PtyViewOpened", "artifact.pty-view-opened"),
    ("NoteViewOpened", "artifact.note-view-opened"),
    ("FileViewOpened", "artifact.file-view-opened"),
    ("DiffViewOpened", "artifact.diff-view-opened"),
    ("PagePreviewRequested", "artifact.page-preview-requested"),
    ("PagePreviewResolved", "artifact.page-preview-resolved"),
    ("WebViewCallRequested", "artifact.web-view-call-requested"),
    ("WebViewCallResolved", "artifact.web-view-call-resolved"),
    ("ElementFormRequested", "browser.element-form-requested"),
    ("ElementFormResolved", "browser.element-form-resolved"),
];

/// The topic a [`WIRE_RELAYED`] kind is declared under, for the front door's re-publication.
#[must_use]
pub fn wire_topic(kind: &str) -> Option<&'static str> {
    WIRE_RELAYED
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, t)| *t)
}

/// `tell`: one owner-side wire frame of a [`WIRE_RELAYED`] topic (its event JSON).
struct Wired(serde_json::Value);

/// Most request ids remembered for their resolution's session before the map is cleared.
const MAX_WIRED: usize = 4096;

/// `tell`: the owner of `session` (per the replica) reconnected; retry now.
#[derive(Clone, Debug)]
pub struct Reconnected(pub PeerId);


/// Follow `session`'s owner while the guard lives; `None` before the actor exists.
pub fn follow(session: SessionId) -> Option<Following> {
    let actor = actor()?;
    actor.tell(Follow(session)).try_send().ok()?;
    Some(Following::new(session, actor.recipient()))
}

/// The relay; `None` before the plugin started it.
pub fn actor() -> Option<ActorRef<EventRelay>> {
    ActorRef::<EventRelay>::lookup(EVENTS_ACTOR).ok().flatten()
}

/// 183 §5.1 (W-19): the approval and ask cards this viewer holds from
/// [`Msg::Bus`] frames of followed sessions, cleared when a stream ends.
#[derive(Clone, Debug, Default)]
pub struct RelayedPrompts {
    /// Relayed `ApprovalRequested` still pending on the owner.
    pub approvals: Vec<tools::ApprovalRequested>,
    /// Relayed `AskRequested` still pending on the owner.
    pub asks: Vec<tools::AskRequested>,
}

/// Pending relayed prompts per followed session (subscriber side).
static RELAYED: LazyLock<parking_lot::Mutex<HashMap<SessionId, RelayedPrompts>>> =
    LazyLock::new(Default::default);

/// Every relayed prompt still pending, for `PendingPrompts::collect` to merge.
pub fn relayed_prompts() -> RelayedPrompts {
    let mut out = RelayedPrompts::default();
    for p in RELAYED.lock().values() {
        out.approvals.extend(p.approvals.iter().cloned());
        out.asks.extend(p.asks.iter().cloned());
    }
    out
}

/// The session of a relayed prompt still pending (an answer forwards to its owner).
pub fn relayed_session(id: uuid::Uuid) -> Option<SessionId> {
    RELAYED.lock().iter().find_map(|(s, p)| {
        let hit = p.approvals.iter().any(|a| a.id.0 == id) || p.asks.iter().any(|a| a.id.0 == id);
        hit.then_some(*s)
    })
}

/// Keep or drop one relayed prompt from a `Bus` frame of `session`.
fn note_relayed(session: SessionId, kind: &str, event: &serde_json::Value) {
    let mut map = RELAYED.lock();
    match kind {
        "ApprovalRequested" => {
            if let Ok(a) = serde_json::from_value::<ApprovalRequested>(event.clone()) {
                let p = map.entry(session).or_default();
                p.approvals.retain(|x| x.id != a.id);
                p.approvals.push(a);
            }
        }
        "AskRequested" => {
            if let Ok(a) = serde_json::from_value::<AskRequested>(event.clone()) {
                let p = map.entry(session).or_default();
                p.asks.retain(|x| x.id != a.id);
                p.asks.push(a);
            }
        }
        "ApprovalResolved" => {
            if let (Ok(r), Some(p)) = (
                serde_json::from_value::<ApprovalResolved>(event.clone()),
                map.get_mut(&session),
            ) {
                p.approvals.retain(|x| x.id != r.id);
            }
        }
        "AskResolved" => {
            if let (Ok(r), Some(p)) = (
                serde_json::from_value::<AskResolved>(event.clone()),
                map.get_mut(&session),
            ) {
                p.asks.retain(|x| x.id != r.id);
            }
        }
        _ => {}
    }
}

/// The follow stream of `session` ended: its relayed cards are withdrawn (§4.4).
fn forget_relayed(session: SessionId) {
    RELAYED.lock().remove(&session);
}

/// Followed sessions whose stream is up and speaks `Watch` (subscriber side).
static LIVE: LazyLock<parking_lot::Mutex<std::collections::HashSet<SessionId>>> =
    LazyLock::new(Default::default);

/// 183 R1 (§12.1): tell this engine's clients whether `session`'s follow
/// stream is up and speaks `Watch` — the chip's `live` — as the client
/// event `{type: "FollowState", session, live}` (via the bridge, like a
/// relayed card). An old owner's `Events` stream is not live: rows only.
async fn follow_state(rt: &Runtime, session: SessionId, live: bool) {
    rt.publish(Relayed {
        session,
        kind: "FollowState".to_owned(),
        event: serde_json::json!(FollowState { session, live }),
    })
    .await;
}

/// One remote subscriber on the owner side. 183 R1: no `from_seq` filter —
/// nothing is buffered to replay, and the per-session counter restarts with
/// the `served` entry (or the owner process), so a reconnecting viewer's
/// `from_seq` only ever hid every frame until the counter caught up.
struct Remote {
    tx: mpsc::Sender<Msg>,
    /// Opened with `Watch`: gets `Bus` frames.
    bus: bool,
}

/// One followed session on the subscriber side.
struct Followed {
    refs: usize,
    task: Option<tokio::task::AbortHandle>,
}

/// The [`EVENTS_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct EventRelay {
    rt: Runtime,
    store: Store,
    transport: Option<Arc<dyn Transport>>,
    /// Owner side: open streams per session and the session's event counter.
    served: HashMap<String, (u64, Vec<Remote>)>,
    /// Owner side: pending approval / ask id → its session, because the
    /// `*Resolved` events carry no session (§5.1).
    prompts: HashMap<uuid::Uuid, SessionId>,
    /// Owner side: a [`WIRE_RELAYED`] request's id → its session, for its resolution (209 §4.4).
    wired: HashMap<uuid::Uuid, SessionId>,
    /// Owner side: live user-cell kernel jobs, kept off the relay (183 R1 S4).
    cells: std::collections::HashSet<(SessionId, tools::JobId)>,
    /// Subscriber side.
    following: HashMap<SessionId, Followed>,
}

impl EventRelay {
    /// Spawn on the subscriber mailbox, subscribe to [`AgentEvent`], register.
    pub async fn start(rt: &Runtime, store: Store) -> anyhow::Result<ActorRef<Self>> {
        let actor = Self::spawn_on(rt, store).await;
        actor.register(EVENTS_ACTOR)?;
        Ok(actor)
    }

    /// Spawn on the subscriber mailbox and subscribe to [`AgentEvent`] and the
    /// relayed bus types, unregistered (a second engine in one test process). The
    /// [`WIRE_RELAYED`] topics come from this engine's wire feed (209 §4.4).
    pub async fn spawn_on(rt: &Runtime, store: Store) -> ActorRef<Self> {
        let actor = Self::spawn_with_mailbox(
            Self {
                rt: rt.clone(),
                store,
                transport: None,
                served: HashMap::new(),
                prompts: HashMap::new(),
                wired: HashMap::new(),
                cells: std::collections::HashSet::new(),
                following: HashMap::new(),
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Self, AgentEvent>(&actor).await;
        rt.subscribe::<Self, ApprovalRequested>(&actor).await;
        rt.subscribe::<Self, ApprovalResolved>(&actor).await;
        rt.subscribe::<Self, AskRequested>(&actor).await;
        rt.subscribe::<Self, AskResolved>(&actor).await;
        rt.subscribe::<Self, JobEvent>(&actor).await;
        rt.subscribe::<Self, PeerMessageChanged>(&actor).await;
        rt.subscribe::<Self, cells::CellEvent>(&actor).await;
        watch_wire(rt, actor.clone());
        actor
    }

    fn here(&self, owner: &str) -> bool {
        owner.is_empty() || owner == self.store.engine()
    }

    /// Start the follower task for `session` unless one runs.
    fn spawn_follower(&mut self, session: SessionId) {
        let Some(t) = self.transport.clone() else {
            return;
        };
        let Some(f) = self.following.get_mut(&session) else {
            return;
        };
        if f.task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let (rt, store) = (self.rt.clone(), self.store.clone());
        f.task = Some(tokio::spawn(follower(rt, store, t, session)).abort_handle());
    }

    /// Owner side: one frame (`mk(seq)`) to the session's remote subscribers;
    /// `bus` frames only to those that opened with `Watch`.
    fn send(&mut self, session: SessionId, bus: bool, mk: impl FnOnce(String, u64) -> Msg) {
        let key = session.to_string();
        let Some((seq, remotes)) = self.served.get_mut(&key) else {
            return;
        };
        *seq += 1;
        let msg = mk(key.clone(), *seq);
        remotes.retain(|r| {
            if bus && !r.bus {
                return true;
            }
            match r.tx.try_send(msg.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::debug!(session = %key, "remote event subscriber too slow; dropped");
                    false
                }
            }
        });
        if remotes.is_empty() {
            self.served.remove(&key);
        }
    }

    /// Owner side: one non-agent bus event of `session` as [`Msg::Bus`].
    fn bus<T: Serialize>(&mut self, session: SessionId, kind: &str, ev: &T) {
        if self.served.is_empty() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(ev) else {
            return;
        };
        let kind = kind.to_owned();
        self.send(session, true, |session, seq| Msg::Bus {
            session,
            seq,
            kind,
            bytes,
        });
    }
}

/// The approval and ask cards of `session` pending on this engine, as
/// `(id, kind, bytes)`, for a `Watch` that opens after they were raised.
async fn pending_prompts(session: SessionId) -> Vec<(uuid::Uuid, &'static str, Vec<u8>)> {
    let mut out = Vec::new();
    if let Ok(Some(a)) = ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)
        && let Ok(v) = a.ask(tools::PendingApprovals).await
    {
        for r in v.into_iter().filter(|r| r.session == session) {
            if let Ok(b) = serde_json::to_vec(&r) {
                out.push((r.id.0, "ApprovalRequested", b));
            }
        }
    }
    if let Ok(Some(a)) = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)
        && let Ok(v) = a.ask(tools::PendingAsks).await
    {
        for r in v.into_iter().filter(|r| r.session == session) {
            if let Ok(b) = serde_json::to_vec(&r) {
                out.push((r.id.0, "AskRequested", b));
            }
        }
    }
    out
}

impl Message<SetTransport> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: SetTransport, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.transport = Some(m.0);
        for s in self.following.keys().copied().collect::<Vec<_>>() {
            self.spawn_follower(s);
        }
    }
}

impl Message<Follow> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: Follow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.following
            .entry(m.0)
            .or_insert(Followed {
                refs: 0,
                task: None,
            })
            .refs += 1;
        // A second client of an already-live follow learns it at once.
        if LIVE.lock().contains(&m.0) {
            follow_state(&self.rt, m.0, true).await;
        }
        self.spawn_follower(m.0);
    }
}

impl Message<Unfollow> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: Unfollow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(f) = self.following.get_mut(&m.0) else {
            return;
        };
        f.refs = f.refs.saturating_sub(1);
        if f.refs == 0 {
            if let Some(t) = f.task.take() {
                t.abort();
            }
            self.following.remove(&m.0);
            forget_relayed(m.0);
            if LIVE.lock().remove(&m.0) {
                follow_state(&self.rt, m.0, false).await;
            }
        }
    }
}

impl Message<Serve> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: Serve, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 183 W-18: a `Watch` is answered first, even when refused below, so
        // the subscriber knows this owner speaks it.
        if m.bus {
            let _ = m.tx.try_send(Msg::Bus {
                session: m.session.clone(),
                seq: 0,
                kind: "Watching".to_owned(),
                bytes: Vec::new(),
            });
        }
        // Only the owner streams; a stale request (the session moved) just closes.
        let session: SessionId = m.session.parse().unwrap_or_default();
        let owner = self.store.owner(session).await.ok().flatten();
        if !owner.is_some_and(|(o, _)| self.here(&o)) {
            return;
        }
        self.served
            .entry(m.session.clone())
            .or_insert((0, Vec::new()));
        if m.bus {
            // The cards already waiting: a focus after the ask still shows it.
            // 183 R2 (5a): a replay, not a live ask: seq 0 (with `Watching`),
            // so the viewer flags it and its UI states no deadline.
            for (id, kind, bytes) in pending_prompts(session).await {
                self.prompts.insert(id, session);
                let _ = m.tx.try_send(Msg::Bus {
                    session: m.session.clone(),
                    seq: 0,
                    kind: kind.to_owned(),
                    bytes,
                });
            }
        }
        if let Some((_, remotes)) = self.served.get_mut(&m.session) {
            remotes.push(Remote {
                tx: m.tx,
                bus: m.bus,
            });
        }
    }
}

impl Message<Reconnected> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, _: Reconnected, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // The follower tasks watch `PeerConnected` themselves; a finished one
        // (owner was here, or moved) is restarted here so a moved session is
        // picked up again.
        for s in self.following.keys().copied().collect::<Vec<_>>() {
            self.spawn_follower(s);
        }
    }
}

impl Message<AgentEvent> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(session) = ev.session() else { return };
        if let AgentEvent::SessionOwnerChanged { owner, .. } = &ev {
            let here = self.here(owner);
            if !here {
                // Ownership left: the streams end (senders dropped → writers return).
                self.served.remove(&session.to_string());
            }
            if self.following.contains_key(&session) {
                if here {
                    // Ours now: nothing to follow. 183 R2: the aborted stream's
                    // relayed cards and its live flag go with it, as at its end.
                    if let Some(t) = self.following.get_mut(&session).and_then(|f| f.task.take()) {
                        t.abort();
                    }
                    forget_relayed(session);
                    if LIVE.lock().remove(&session) {
                        follow_state(&self.rt, session, false).await;
                    }
                } else {
                    self.spawn_follower(session);
                }
            }
        }
        if self.served.is_empty() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&ev) else {
            return;
        };
        self.send(session, false, |session, seq| Msg::Event {
            session,
            seq,
            bytes,
        });
    }
}

impl Message<ApprovalRequested> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalRequested, _: &mut Context<Self, ()>) {
        self.prompts.insert(ev.id.0, ev.session);
        self.bus(ev.session, "ApprovalRequested", &ev);
    }
}

impl Message<AskRequested> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: AskRequested, _: &mut Context<Self, ()>) {
        self.prompts.insert(ev.id.0, ev.session);
        self.bus(ev.session, "AskRequested", &ev);
    }
}

impl Message<ApprovalResolved> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalResolved, _: &mut Context<Self, ()>) {
        if let Some(session) = self.prompts.remove(&ev.id.0) {
            self.bus(session, "ApprovalResolved", &ev);
        }
    }
}

impl Message<AskResolved> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: AskResolved, _: &mut Context<Self, ()>) {
        if let Some(session) = self.prompts.remove(&ev.id.0) {
            self.bus(session, "AskResolved", &ev);
        }
    }
}

impl Message<PeerMessageChanged> for EventRelay {
    type Reply = ();

    /// A receipt changed on the owner (answered, replied, failed): a viewer's
    /// "Expects a reply" chip refetches through the forwarded `list_peer_messages`.
    async fn handle(&mut self, ev: PeerMessageChanged, _: &mut Context<Self, ()>) {
        self.bus(ev.session, "PeerMessageChanged", &ev);
    }
}

impl Message<JobEvent> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: JobEvent, _: &mut Context<Self, ()>) {
        // 183 R1 (S4): a user-cell kernel is not the session's terminal
        // (171F B15). Only `Started` names it, and a viewer that follows
        // mid-run never saw that, so the owner drops the kernel's events.
        let (session, cell) = match &ev {
            JobEvent::Started {
                session, id, name, ..
            } => {
                let cell = name
                    .as_deref()
                    .is_some_and(|n| n.starts_with(tools::jobs::CELL_PREFIX));
                if cell {
                    self.cells.insert((*session, id.clone()));
                }
                (*session, cell)
            }
            JobEvent::Output { session, id, .. }
            | JobEvent::Screen { session, id, .. }
            | JobEvent::Idle { session, id, .. } => {
                (*session, self.cells.contains(&(*session, id.clone())))
            }
            JobEvent::Finished { session, id, .. } => {
                (*session, self.cells.remove(&(*session, id.clone())))
            }
        };
        if !cell {
            self.bus(session, "JobEvent", &ev);
        }
    }
}

impl Message<cells::CellEvent> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, ev: cells::CellEvent, _: &mut Context<Self, ()>) {
        use cells::CellEvent as C;
        let (C::RunQueued { scope, .. }
        | C::RunStarted { scope, .. }
        | C::RunFinished { scope, .. }
        | C::CellsChanged { scope }) = &ev
        else {
            return; // `KernelChanged` names no session.
        };
        if let cells::Scope::Session(session) = scope {
            self.bus(*session, "CellEvent", &ev);
        }
    }
}

impl Message<Wired> for EventRelay {
    type Reply = ();

    /// 209 §4.4: a [`WIRE_RELAYED`] event raised here goes to the session's remote viewers as a
    /// `Bus` frame of its own kind (the event's `type`); a request's id is kept so its
    /// resolution, which names no session, follows it.
    async fn handle(&mut self, Wired(event): Wired, _: &mut Context<Self, ()>) {
        let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
            return;
        };
        let kind = kind.to_owned();
        let id = event
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| s.parse::<uuid::Uuid>().ok());
        let session = event
            .get("session")
            .cloned()
            .and_then(|s| serde_json::from_value::<SessionId>(s).ok());
        let session = match (session, id) {
            (Some(s), Some(id)) if kind.ends_with("Requested") => {
                if self.wired.len() >= MAX_WIRED {
                    self.wired.clear();
                }
                self.wired.insert(id, s);
                s
            }
            (Some(s), _) => s,
            (None, Some(id)) => match self.wired.remove(&id) {
                Some(s) => s,
                None => return,
            },
            (None, None) => return,
        };
        self.bus(session, &kind, &event);
    }
}

/// Owner side (209 §4.4): hand every [`WIRE_RELAYED`] frame of this engine's wire feed to the
/// relay until shutdown or the relay is gone.
fn watch_wire(rt: &Runtime, relay: ActorRef<EventRelay>) {
    use tokio::sync::broadcast::error::RecvError;
    use wire::RuntimeWire as _;
    let mut feed = rt.wire_events();
    let shutdown = rt.shutdown.clone();
    tokio::spawn(async move {
        loop {
            let frame = tokio::select! {
                () = shutdown.cancelled() => return,
                f = feed.recv() => f,
            };
            match frame {
                Ok(f) => {
                    let relayed = f
                        .descriptor
                        .as_ref()
                        .is_some_and(|d| WIRE_RELAYED.iter().any(|(_, t)| *t == d.topic));
                    if relayed && relay.tell(Wired(f.event.clone())).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!(lost = n, "events relay: wire feed lagged");
                }
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// Owner side of one accepted `theuth/sessions/1` stream whose first frame
/// was [`Msg::Events`] (`bus: false`) or [`Msg::Watch`] (`bus: true`):
/// register with the relay, write frames until the peer closes or the relay
/// drops us (ownership moved).
pub async fn serve(
    relay: &ActorRef<EventRelay>,
    mut framed: Framed<Box<dyn ByteStream>, LengthDelimitedCodec>,
    session: String,
    from_seq: u64,
    bus: bool,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(QUEUE);
    relay
        .tell(Serve {
            session,
            from_seq,
            tx,
            bus,
        })
        .await
        .map_err(|_| anyhow::anyhow!("events relay gone"))?;
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(m) => framed.send(m.encode()?).await?,
                None => return Ok(()),
            },
            // The subscriber never writes after `Events`; a frame or EOF ends the stream.
            _ = framed.next() => return Ok(()),
        }
    }
}

/// The codec every `theuth/sessions/1` stream uses.
pub fn framed(stream: Box<dyn ByteStream>) -> Framed<Box<dyn ByteStream>, LengthDelimitedCodec> {
    Framed::new(
        stream,
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME)
            .new_codec(),
    )
}

/// What one stream attempt saw: the opening frame went out, and the owner
/// sent at least one frame back.
#[derive(Default)]
struct Attempt {
    sent: bool,
    spoke: bool,
}

/// Subscriber side: follow `session`'s owner until aborted; returns when the
/// owner is this engine (nothing to follow) or the session is unknown and not
/// [`crate::owner::expected`].
async fn follower(rt: Runtime, store: Store, t: Arc<dyn Transport>, session: SessionId) {
    let mut backoff = BACKOFF_MIN;
    let mut peers = t.subscribe();
    // 183 W-18: this owner refused `Watch`; `Events` until it reconnects.
    let mut legacy = false;
    loop {
        let owner = match store.owner(session).await {
            Ok(Some((o, _))) if !o.is_empty() && o != store.engine() => PeerId(o),
            // 183 §5.6: a chat another engine just created for us streams from
            // it before its row lands here.
            Ok(None) => match crate::owner::expected(session) {
                Some(engine) => engine,
                None => return,
            },
            _ => return,
        };
        // 183 R1: each stream starts from the owner's current frame; a
        // carried-over `last_seq` asked an old owner to skip everything.
        let mut last_seq = 0u64;
        let mut attempt = Attempt::default();
        let outcome = stream(
            &rt,
            &store,
            t.as_ref(),
            &owner,
            session,
            &mut last_seq,
            !legacy,
            &mut attempt,
        )
        .await;
        forget_relayed(session);
        if LIVE.lock().remove(&session) {
            follow_state(&rt, session, false).await;
        }
        match outcome {
            Ok(()) => backoff = BACKOFF_MIN,
            Err(e) => tracing::debug!(%session, %owner, "events stream: {e:#}"),
        }
        if !legacy && attempt.sent && !attempt.spoke {
            // An old owner cannot decode `Watch` and closed before any frame.
            tracing::debug!(%session, %owner, "owner refused Watch; following with Events");
            legacy = true;
            continue;
        }
        // Retry after backoff, or as soon as the owner reconnects.
        let wait = tokio::time::sleep(backoff);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                () = rt.shutdown.cancelled() => return,
                ev = peers.recv() => match ev {
                    Ok(PeerEvent::Connected(p)) if p == owner => {
                        legacy = false;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                },
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One stream to `owner`: `Watch` (or `Events` when `watch` is false), then
/// re-publish every `Event` locally and every `Bus` frame as [`Relayed`].
#[allow(clippy::too_many_arguments)]
async fn stream(
    rt: &Runtime,
    store: &Store,
    t: &dyn Transport,
    owner: &PeerId,
    session: SessionId,
    last_seq: &mut u64,
    watch: bool,
    attempt: &mut Attempt,
) -> anyhow::Result<()> {
    let mut framed = framed(t.open(owner, ALPN_SESSIONS).await?);
    let (key, from_seq) = (session.to_string(), *last_seq + 1);
    let open = if watch {
        Msg::Watch {
            session: key,
            from_seq,
        }
    } else {
        Msg::Events {
            session: key,
            from_seq,
        }
    };
    framed.send(open.encode()?).await?;
    attempt.sent = true;
    while let Some(frame) = framed.next().await {
        let msg = Msg::decode(&frame?)?;
        attempt.spoke = true;
        match msg {
            Msg::Event { seq, bytes, .. } => {
                // 183 §5.6: a chat in use keeps its by-id route to its creator.
                let _ = crate::owner::expected(session);
                *last_seq = seq;
                let mut ev: AgentEvent = serde_json::from_slice(&bytes)?;
                // `here` is relative to the publisher; recompute for this engine.
                if let AgentEvent::SessionOwnerChanged { owner, here, .. } = &mut ev {
                    *here = owner.is_empty() || *owner == store.engine();
                }
                rt.publish(ev).await;
            }
            Msg::Bus { kind, .. } if kind == "Watching" => {
                LIVE.lock().insert(session);
                follow_state(rt, session, true).await;
            }
            Msg::Bus {
                seq, kind, bytes, ..
            } => {
                *last_seq = (*last_seq).max(seq);
                let mut event: serde_json::Value = serde_json::from_slice(&bytes)?;
                // 183 R2 (5a): a seq-0 card is the owner's replay of what was
                // already pending when this stream opened: no "denies at".
                if seq == 0
                    && let Some(o) = event.as_object_mut()
                {
                    o.insert("replay".to_owned(), serde_json::Value::Bool(true));
                }
                note_relayed(session, &kind, &event);
                rt.publish(Relayed {
                    session,
                    kind,
                    event,
                })
                .await;
            }
            _ => anyhow::bail!("unexpected frame on an events stream"),
        }
    }
    Ok(())
}

/// Bytes of one frame as the wire carries it (for tests and L's dispatcher).
pub fn encode(m: &Msg) -> anyhow::Result<Bytes> {
    m.encode()
}

/// Test-only: the owner forgets every remote subscriber and its counters,
/// as a slow-viewer drop of the last remote or an owner restart does.
#[cfg(test)]
struct DropServed;

#[cfg(test)]
impl Message<DropServed> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, _: DropServed, _: &mut Context<Self, Self::Reply>) {
        self.served.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use tokio::sync::broadcast;

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Loopback pair, as `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let inboxes = Arc::new(Mutex::new(HashMap::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("owner"), mk("front"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    /// Bus tap: collects every `AgentEvent` published on `rt`.
    #[derive(kameo::Actor)]
    struct Tap(mpsc::UnboundedSender<AgentEvent>);

    impl Message<AgentEvent> for Tap {
        type Reply = ();
        async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(ev);
        }
    }

    async fn engine(dir: &std::path::Path, name: &str) -> (Runtime, Store) {
        let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let store = Store::open(
            &dir.join(format!("{name}.sqlite")),
            Some(mesh::EngineId(name.into())),
        )
        .await
        .unwrap();
        (rt, store)
    }

    /// An owner's `theuth/sessions/1` accept loop, reporting each opening
    /// frame (`"watch"` / `"events"`). `legacy`: refuse `Watch` by closing the
    /// stream before any frame, as a pre-183 owner does.
    fn accept_as_owner(
        relay: ActorRef<EventRelay>,
        t: Arc<Loop>,
        legacy: bool,
    ) -> mpsc::UnboundedReceiver<&'static str> {
        let (seen_tx, seen) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((_, stream)) = t.accept(ALPN_SESSIONS).await else {
                    return;
                };
                let mut framed = framed(stream);
                let Some(Ok(first)) = framed.next().await else {
                    continue;
                };
                let (session, from_seq, bus) = match Msg::decode(&first) {
                    Ok(Msg::Events { session, from_seq }) => (session, from_seq, false),
                    Ok(Msg::Watch { session, from_seq }) => (session, from_seq, true),
                    _ => continue,
                };
                let _ = seen_tx.send(if bus { "watch" } else { "events" });
                if bus && legacy {
                    continue; // dropped: the stream closes unanswered
                }
                let r = relay.clone();
                tokio::spawn(async move { serve(&r, framed, session, from_seq, bus).await });
            }
        });
        seen
    }

    /// Bus tap: collects every [`Relayed`] published on `rt`; `FollowState`
    /// goes to the second channel when there is one.
    #[derive(kameo::Actor)]
    struct RelayTap(
        mpsc::UnboundedSender<Relayed>,
        Option<mpsc::UnboundedSender<bool>>,
    );

    impl Message<Relayed> for RelayTap {
        type Reply = ();
        async fn handle(&mut self, ev: Relayed, _: &mut Context<Self, Self::Reply>) {
            if ev.kind == "FollowState" {
                if let Some(tx) = &self.1 {
                    let _ = tx.send(ev.event["live"].as_bool().unwrap_or_default());
                }
                return;
            }
            let _ = self.0.send(ev);
        }
    }

    /// An owner ("owner") and a front door ("front") whose replica row of a
    /// fresh session names "owner"; both relays spawned unregistered, the
    /// owner's accept loop running (`legacy` as in [`accept_as_owner`]).
    struct Two {
        rt_o: Runtime,
        rt_f: Runtime,
        stores: (Store, Store),
        front: ActorRef<EventRelay>,
        owner: ActorRef<EventRelay>,
        session: SessionId,
        seen: mpsc::UnboundedReceiver<&'static str>,
        _dir: tempfile::TempDir,
    }

    async fn two(legacy: bool) -> Two {
        let dir = tempfile::tempdir().unwrap();
        let (owner_t, front_t) = pair();
        let (rt_o, store_o) = engine(dir.path(), "owner").await;
        let session = SessionId::new();
        store_o
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        let theirs = Store::open(
            &dir.path().join("front.sqlite"),
            Some(mesh::EngineId("owner".into())),
        )
        .await
        .unwrap();
        theirs
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        theirs.close().await;
        let (rt_f, store_f) = engine(dir.path(), "front").await;
        let owner = EventRelay::spawn_on(&rt_o, store_o.clone()).await;
        owner.tell(SetTransport(owner_t.clone())).await.unwrap();
        let seen = accept_as_owner(owner.clone(), owner_t, legacy);
        let front = EventRelay::spawn_on(&rt_f, store_f.clone()).await;
        front.tell(SetTransport(front_t)).await.unwrap();
        Two {
            rt_o,
            rt_f,
            stores: (store_o, store_f),
            front,
            owner,
            session,
            seen,
            _dir: dir,
        }
    }

    /// Owner publishes; the front door follows over the loopback and
    /// re-publishes on its own bus; dropping the last guard closes the stream
    /// and the owner forgets the subscriber.
    #[tokio::test]
    async fn events_are_relayed_and_unsubscribed() {
        let dir = tempfile::tempdir().unwrap();
        let (owner_t, front_t) = pair();
        let (rt_o, store_o) = engine(dir.path(), "owner").await;
        let session = SessionId::new();
        store_o
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        // The front door's replica row says the owner is "owner": write it
        // under that identity, then reopen the file as "front".
        let front_path = dir.path().join("front.sqlite");
        let theirs = Store::open(&front_path, Some(mesh::EngineId("owner".into())))
            .await
            .unwrap();
        theirs
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        theirs.close().await;
        let (rt_f, store_f) = engine(dir.path(), "front").await;

        // Two engines in one process: only the front's relay takes the registry name.
        let owner = EventRelay::spawn_on(&rt_o, store_o.clone()).await;
        owner.tell(SetTransport(owner_t.clone())).await.unwrap();
        accept_as_owner(owner.clone(), owner_t.clone(), false);

        let front = EventRelay::start(&rt_f, store_f.clone()).await.unwrap();
        front.tell(SetTransport(front_t.clone())).await.unwrap();
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tap_tx), runtime::subscriber_mailbox());
        rt_f.subscribe::<Tap, AgentEvent>(&tap).await;

        let guard = follow(session).expect("relay registered");
        let follows = rpc_core::FollowMap::default();
        follows.lock().entry(1).or_default().insert(session, guard);
        // Give the follower time to open the stream.
        tokio::time::sleep(Duration::from_millis(200)).await;
        rt_o.publish(AgentEvent::TitleChanged {
            session,
            title: "hi".into(),
            tags: vec![],
        })
        .await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, AgentEvent::TitleChanged { session: s, ref title, .. } if s == session && title == "hi"),
            "{got:?}"
        );

        follows.lock().remove(&1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        rt_o.publish(AgentEvent::TitleChanged {
            session,
            title: "after".into(),
            tags: vec![],
        })
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(500), tapped.recv())
                .await
                .is_err(),
            "unsubscribed"
        );
        store_o.close().await;
        store_f.close().await;
    }

    /// 183 §5.6: a chat the owner just created for the front door streams its
    /// events there before its row replicates: the follower takes the owner
    /// from [`crate::owner::expect`] while the front's store has no row.
    #[tokio::test]
    async fn a_chat_created_elsewhere_is_followed_before_its_row_lands() {
        let dir = tempfile::tempdir().unwrap();
        let (owner_t, front_t) = pair();
        let (rt_o, store_o) = engine(dir.path(), "owner").await;
        let session = SessionId::new();
        store_o
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        let (rt_f, store_f) = engine(dir.path(), "front").await;
        let owner = EventRelay::spawn_on(&rt_o, store_o.clone()).await;
        owner.tell(SetTransport(owner_t.clone())).await.unwrap();
        accept_as_owner(owner.clone(), owner_t, false);
        let front = EventRelay::spawn_on(&rt_f, store_f.clone()).await;
        front.tell(SetTransport(front_t)).await.unwrap();
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tap_tx), runtime::subscriber_mailbox());
        rt_f.subscribe::<Tap, AgentEvent>(&tap).await;

        assert!(store_f.session(session).await.unwrap().is_none());
        crate::owner::expect(session, PeerId("owner".into()));
        front.tell(Follow(session)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        rt_o.publish(AgentEvent::TitleChanged {
            session,
            title: "first".into(),
            tags: vec![],
        })
        .await;
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(ev @ AgentEvent::TitleChanged { .. }) = tapped.recv().await {
                    return ev;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(got, AgentEvent::TitleChanged { session: s, ref title, .. } if s == session && title == "first"),
            "{got:?}"
        );
        store_o.close().await;
        store_f.close().await;
    }

    /// 183 W-19: an approval raised on the owner reaches a viewer that
    /// follows the session as a `Relayed` bus event and sits in
    /// `relayed_prompts()`; the owner's `ApprovalResolved` (no session of its
    /// own) is mapped to the session and clears it; unfollowing forgets.
    #[tokio::test]
    async fn relayed_approval_is_pending_on_the_viewer_until_resolved() {
        let mut t = two(false).await;
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap =
            RelayTap::spawn_with_mailbox(RelayTap(tap_tx, None), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<RelayTap, Relayed>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let id = tools::ApprovalId(uuid::Uuid::now_v7());
        t.rt_o
            .publish(ApprovalRequested {
                id,
                session: t.session,
                tool: "shell".into(),
                args: serde_json::json!({ "command": "cargo test" }),
                risk: tools::Risk::Mutating,
                human_only: false,
            })
            .await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "ApprovalRequested")
        );
        assert_eq!(got.event["tool"], "shell");
        let pending = |id: tools::ApprovalId| {
            relayed_prompts()
                .approvals
                .iter()
                .any(|a| a.id == id && a.session == t.session)
        };
        assert!(pending(id), "the viewer's snapshot keeps the relayed card");
        assert_eq!(relayed_session(id.0), Some(t.session));

        t.rt_o
            .publish(ApprovalResolved {
                id,
                decision: tools::Decision::Allow,
                timed_out: false,
                dismissed: false,
            })
            .await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "ApprovalResolved")
        );
        assert!(!pending(id), "the resolution cleared it by session");

        // A second card, then the follow ends: withdrawn.
        let id2 = tools::ApprovalId(uuid::Uuid::now_v7());
        t.rt_o
            .publish(ApprovalRequested {
                id: id2,
                session: t.session,
                tool: "shell".into(),
                args: serde_json::json!({}),
                risk: tools::Risk::Mutating,
                human_only: false,
            })
            .await;
        tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(pending(id2));
        t.front.tell(Unfollow(t.session)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !pending(id2),
            "the end of the follow withdraws relayed cards"
        );
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// 209 §4.4: the forward list names the tile-call pair, `show_web`'s event, the page-preview
    /// pair and the element-form pair (P5), each under its declared topic.
    #[test]
    fn the_relay_forwards_tile_calls_tiles_previews_and_forms() {
        for (kind, topic) in [
            ("WebViewCallRequested", "artifact.web-view-call-requested"),
            ("WebViewCallResolved", "artifact.web-view-call-resolved"),
            ("WebViewOpened", "artifact.web-view-opened"),
            ("PtyViewOpened", "artifact.pty-view-opened"),
            ("NoteViewOpened", "artifact.note-view-opened"),
            ("FileViewOpened", "artifact.file-view-opened"),
            ("DiffViewOpened", "artifact.diff-view-opened"),
            ("PagePreviewRequested", "artifact.page-preview-requested"),
            ("PagePreviewResolved", "artifact.page-preview-resolved"),
            ("ElementFormRequested", "browser.element-form-requested"),
            ("ElementFormResolved", "browser.element-form-resolved"),
        ] {
            assert_eq!(wire_topic(kind), Some(topic), "{kind}");
        }
        assert_eq!(wire_topic("ApprovalRequested"), None);
    }

    /// A `browser_show_element` card (as `browser` declares it) for the wire-relay test.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type", rename = "ElementFormRequested")]
    struct FormRequested {
        id: uuid::Uuid,
        session: SessionId,
        origin: String,
    }

    impl wire::WireEvent for FormRequested {
        const TOPIC: &'static str = "browser.element-form-requested";
        /// The production impl's 216 exposure (`wire` refuses twins that disagree).
        const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
    }

    /// Its resolution, which names no session.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type", rename = "ElementFormResolved")]
    struct FormResolved {
        id: uuid::Uuid,
        outcome: String,
    }

    impl wire::WireEvent for FormResolved {
        const TOPIC: &'static str = "browser.element-form-resolved";
        /// The production impl's 216 exposure (`wire` refuses twins that disagree).
        const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
    }

    /// 209 §4.4 (P5): a card raised on the owner reaches the viewer's bus, and its resolution
    /// follows the request's id to the same session.
    #[tokio::test]
    async fn an_element_form_on_the_owner_is_relayed_with_its_resolution() {
        use wire::RuntimeWire as _;
        let mut t = two(false).await;
        t.rt_o.register_wire_event::<FormRequested>().await.unwrap();
        t.rt_o.register_wire_event::<FormResolved>().await.unwrap();
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap =
            RelayTap::spawn_with_mailbox(RelayTap(tap_tx, None), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<RelayTap, Relayed>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let id = uuid::Uuid::now_v7();
        t.rt_o
            .publish(FormRequested {
                id,
                session: t.session,
                origin: "https://example.com".into(),
            })
            .await;
        t.rt_o
            .publish(FormResolved {
                id,
                outcome: "submitted".into(),
            })
            .await;
        let got = relayed(&mut tapped).await;
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "ElementFormRequested")
        );
        assert_eq!(got.event["origin"], "https://example.com");
        let got = relayed(&mut tapped).await;
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "ElementFormResolved")
        );
        assert_eq!(got.event["outcome"], "submitted");
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// A tile call's request (as `artifact` declares it) for the wire-relay test.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type", rename = "WebViewCallRequested")]
    struct CallRequested {
        id: uuid::Uuid,
        session: SessionId,
        view: String,
        op: String,
        args: serde_json::Value,
    }

    impl wire::WireEvent for CallRequested {
        const TOPIC: &'static str = "artifact.web-view-call-requested";
        /// The production impl's 216 exposure (`wire` refuses twins that disagree).
        const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
    }

    /// Its resolution, which names no session.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type", rename = "WebViewCallResolved")]
    struct CallResolved {
        id: uuid::Uuid,
    }

    impl wire::WireEvent for CallResolved {
        const TOPIC: &'static str = "artifact.web-view-call-resolved";
        /// The production impl's 216 exposure (`wire` refuses twins that disagree).
        const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
    }

    async fn relayed(rx: &mut mpsc::UnboundedReceiver<Relayed>) -> Relayed {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("relayed within 5 s")
            .expect("tap open")
    }

    /// 209 §4.4: a tile call raised on the owner reaches the viewer's bus with its kind, and so
    /// does its resolution, routed by the request's id to the same session.
    #[tokio::test]
    async fn a_tile_call_on_the_owner_is_relayed_with_its_resolution() {
        use wire::RuntimeWire as _;
        let mut t = two(false).await;
        t.rt_o.register_wire_event::<CallRequested>().await.unwrap();
        t.rt_o.register_wire_event::<CallResolved>().await.unwrap();
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap =
            RelayTap::spawn_with_mailbox(RelayTap(tap_tx, None), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<RelayTap, Relayed>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let id = uuid::Uuid::now_v7();
        // Another session's call stays on the owner.
        t.rt_o
            .publish(CallRequested {
                id: uuid::Uuid::now_v7(),
                session: SessionId::new(),
                view: "fedcba9876543210".into(),
                op: "snapshot".into(),
                args: serde_json::json!({}),
            })
            .await;
        t.rt_o
            .publish(CallRequested {
                id,
                session: t.session,
                view: "0123456789abcdef".into(),
                op: "snapshot".into(),
                args: serde_json::json!({}),
            })
            .await;
        t.rt_o.publish(CallResolved { id }).await;
        let got = relayed(&mut tapped).await;
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "WebViewCallRequested")
        );
        assert_eq!(got.event["view"], "0123456789abcdef");
        assert_eq!(got.event["id"], id.to_string());
        let got = relayed(&mut tapped).await;
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "WebViewCallResolved")
        );
        assert_eq!(got.event["id"], id.to_string());
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// An exchange's receipt changing on the owner reaches a viewer that
    /// follows the session as a `Relayed` `PeerMessageChanged`, so its
    /// pending-answer view refetches.
    #[tokio::test]
    async fn a_peer_message_change_on_the_owner_is_relayed_to_the_viewer() {
        let mut t = two(false).await;
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap =
            RelayTap::spawn_with_mailbox(RelayTap(tap_tx, None), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<RelayTap, Relayed>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let id = uuid::Uuid::now_v7();
        t.rt_o
            .publish(PeerMessageChanged {
                session: t.session,
                id,
                state: agent::PeerMessageState::Replied,
                from: String::new(),
                to: String::new(),
                result: false,
            })
            .await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.session, got.kind.as_str()),
            (t.session, "PeerMessageChanged")
        );
        let ev: PeerMessageChanged = serde_json::from_value(got.event).unwrap();
        assert_eq!((ev.session, ev.id), (t.session, id));
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// 183 W-18: an old owner closes a `Watch` stream unanswered; the viewer
    /// falls back to `Events` and still gets the session's agent events.
    #[tokio::test]
    async fn an_old_owner_refusing_watch_falls_back_to_events() {
        let mut t = two(true).await;
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tap_tx), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<Tap, AgentEvent>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), t.seen.recv())
                .await
                .unwrap(),
            Some("events"),
            "retried at once with Events"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        t.rt_o
            .publish(AgentEvent::TitleChanged {
                session: t.session,
                title: "legacy".into(),
                tags: vec![],
            })
            .await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, AgentEvent::TitleChanged { ref title, .. } if title == "legacy"),
            "{got:?}"
        );
        t.front.tell(Unfollow(t.session)).await.unwrap();
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// 183 R1 (E-M1): the owner drops the stream (a slow viewer, or a
    /// restart) and its per-session counter restarts at 0; the viewer
    /// reconnects and still gets new deltas and a new approval card. Before,
    /// it resumed at `last_seq + 1` and every frame below that was skipped.
    #[tokio::test]
    async fn a_reconnected_viewer_still_gets_events_and_cards() {
        let mut t = two(false).await;
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tap_tx), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<Tap, AgentEvent>(&tap).await;
        let (rtap_tx, mut rtapped) = mpsc::unbounded_channel();
        let (live_tx, mut live) = mpsc::unbounded_channel();
        let rtap = RelayTap::spawn_with_mailbox(
            RelayTap(rtap_tx, Some(live_tx)),
            runtime::subscriber_mailbox(),
        );
        t.rt_f.subscribe::<RelayTap, Relayed>(&rtap).await;
        let next_live = async |live: &mut mpsc::UnboundedReceiver<bool>| {
            tokio::time::timeout(Duration::from_secs(10), live.recv())
                .await
                .unwrap()
                .unwrap()
        };
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        assert!(
            next_live(&mut live).await,
            "FollowState live once the owner answers Watch"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let title = |s: &str| AgentEvent::TitleChanged {
            session: t.session,
            title: s.into(),
            tags: vec![],
        };
        for i in 0..5 {
            t.rt_o.publish(title(&format!("before {i}"))).await;
            tokio::time::timeout(Duration::from_secs(5), tapped.recv())
                .await
                .unwrap()
                .unwrap();
        }
        t.owner.tell(DropServed).await.unwrap();
        assert!(
            !next_live(&mut live).await,
            "the dropped stream is not live"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), t.seen.recv())
                .await
                .unwrap(),
            Some("watch"),
            "the viewer reconnects"
        );
        assert!(next_live(&mut live).await, "live again");
        tokio::time::sleep(Duration::from_millis(200)).await;
        t.rt_o.publish(title("after")).await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .expect("a delta after the reconnect")
            .unwrap();
        assert!(
            matches!(got, AgentEvent::TitleChanged { ref title, .. } if title == "after"),
            "{got:?}"
        );
        t.rt_o
            .publish(ApprovalRequested {
                id: tools::ApprovalId(uuid::Uuid::now_v7()),
                session: t.session,
                tool: "shell".into(),
                args: serde_json::json!({}),
                risk: tools::Risk::Mutating,
                human_only: false,
            })
            .await;
        let card = tokio::time::timeout(Duration::from_secs(5), rtapped.recv())
            .await
            .expect("a card after the reconnect")
            .unwrap();
        assert_eq!(card.kind, "ApprovalRequested");
        t.front.tell(Unfollow(t.session)).await.unwrap();
        t.stores.0.close().await;
        t.stores.1.close().await;
    }

    /// 183 R2 (5a): a card already pending on the owner when the viewer's
    /// stream opens arrives flagged `replay: true` (its UI states no
    /// deadline); one raised while followed does not.
    #[tokio::test]
    async fn a_card_pending_before_the_follow_is_flagged_as_a_replay() {
        let mut t = two(false).await;
        let approval = tools::Approval::spawn(tools::Approval::new(
            t.rt_o.clone(),
            Duration::from_secs(30),
        ));
        approval.register(tools::APPROVAL_ACTOR).unwrap();
        let parked = approval.clone();
        let session = t.session;
        tokio::spawn(async move {
            parked
                .ask(tools::approval::RequestApproval {
                    session,
                    tool: "shell".into(),
                    args: serde_json::json!({}),
                    risk: tools::Risk::Mutating,
                    call_id: None,
                })
                .await
        });
        while approval
            .ask(tools::PendingApprovals)
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap =
            RelayTap::spawn_with_mailbox(RelayTap(tap_tx, None), runtime::subscriber_mailbox());
        t.rt_f.subscribe::<RelayTap, Relayed>(&tap).await;
        t.front.tell(Follow(t.session)).await.unwrap();
        assert_eq!(t.seen.recv().await, Some("watch"));
        let replayed = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed.kind, "ApprovalRequested");
        assert_eq!(replayed.event["replay"], true, "{:?}", replayed.event);
        tokio::time::sleep(Duration::from_millis(200)).await;
        t.rt_o
            .publish(ApprovalRequested {
                id: tools::ApprovalId(uuid::Uuid::now_v7()),
                session: t.session,
                tool: "shell".into(),
                args: serde_json::json!({}),
                risk: tools::Risk::Mutating,
                human_only: false,
            })
            .await;
        let live = tokio::time::timeout(Duration::from_secs(5), tapped.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(live.event.get("replay").is_none(), "{:?}", live.event);
        t.front.tell(Unfollow(t.session)).await.unwrap();
        t.stores.0.close().await;
        t.stores.1.close().await;
    }
}
