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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::AgentEvent;
use agent::store::Store;
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use replica::{ALPN_SESSIONS, ByteStream, PeerEvent, PeerId, Transport};
use runtime::{Runtime, SessionId};
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

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

/// `tell`: one local client of `session` is gone.
#[derive(Clone, Copy, Debug)]
pub struct Unfollow(pub SessionId);

/// `tell`: a remote subscriber's stream for `session` is open; events go to `tx`.
pub struct Serve {
    /// The session.
    pub session: String,
    /// First seq wanted.
    pub from_seq: u64,
    /// The stream's writer.
    pub tx: mpsc::Sender<Msg>,
}

/// `tell`: the owner of `session` (per the replica) reconnected; retry now.
#[derive(Clone, Debug)]
pub struct Reconnected(pub PeerId);

/// Refcount guard from [`follow`]: dropping it is the [`Unfollow`].
pub struct Following {
    session: SessionId,
    actor: ActorRef<EventRelay>,
}

impl Drop for Following {
    fn drop(&mut self) {
        let _ = self.actor.tell(Unfollow(self.session)).try_send();
    }
}

/// Follow `session`'s owner while the guard lives; `None` before the actor exists.
pub fn follow(session: SessionId) -> Option<Following> {
    let actor = actor()?;
    actor.tell(Follow(session)).try_send().ok()?;
    Some(Following { session, actor })
}

/// The relay; `None` before the plugin started it.
pub fn actor() -> Option<ActorRef<EventRelay>> {
    ActorRef::<EventRelay>::lookup(EVENTS_ACTOR).ok().flatten()
}

/// One remote subscriber on the owner side.
struct Remote {
    from_seq: u64,
    tx: mpsc::Sender<Msg>,
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

    /// Spawn on the subscriber mailbox and subscribe to [`AgentEvent`], unregistered
    /// (a second engine in one test process).
    pub async fn spawn_on(rt: &Runtime, store: Store) -> ActorRef<Self> {
        let actor = Self::spawn_with_mailbox(
            Self {
                rt: rt.clone(),
                store,
                transport: None,
                served: HashMap::new(),
                following: HashMap::new(),
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Self, AgentEvent>(&actor).await;
        actor
    }

    fn here(&self, owner: &str) -> bool {
        owner.is_empty() || owner == self.store.engine()
    }

    /// Start the follower task for `session` unless one runs.
    fn spawn_follower(&mut self, session: SessionId) {
        let Some(t) = self.transport.clone() else { return };
        let Some(f) = self.following.get_mut(&session) else { return };
        if f.task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let (rt, store) = (self.rt.clone(), self.store.clone());
        f.task = Some(tokio::spawn(follower(rt, store, t, session)).abort_handle());
    }
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
        self.following.entry(m.0).or_insert(Followed { refs: 0, task: None }).refs += 1;
        self.spawn_follower(m.0);
    }
}

impl Message<Unfollow> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: Unfollow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(f) = self.following.get_mut(&m.0) else { return };
        f.refs = f.refs.saturating_sub(1);
        if f.refs == 0 {
            if let Some(t) = f.task.take() {
                t.abort();
            }
            self.following.remove(&m.0);
        }
    }
}

impl Message<Serve> for EventRelay {
    type Reply = ();

    async fn handle(&mut self, m: Serve, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // Only the owner streams; a stale request (the session moved) just closes.
        let owner = self.store.owner(m.session.parse().unwrap_or_default()).await.ok().flatten();
        if !owner.is_some_and(|(o, _)| self.here(&o)) {
            return;
        }
        self.served.entry(m.session).or_insert((0, Vec::new())).1.push(Remote {
            from_seq: m.from_seq,
            tx: m.tx,
        });
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
                    // Ours now: nothing to follow.
                    if let Some(t) = self.following.get_mut(&session).and_then(|f| f.task.take()) {
                        t.abort();
                    }
                } else {
                    self.spawn_follower(session);
                }
            }
        }
        if self.served.is_empty() {
            return;
        }
        let key = session.to_string();
        let Some((seq, remotes)) = self.served.get_mut(&key) else { return };
        let Ok(bytes) = serde_json::to_vec(&ev) else { return };
        *seq += 1;
        let msg = Msg::Event {
            session: key.clone(),
            seq: *seq,
            bytes,
        };
        remotes.retain(|r| {
            if *seq < r.from_seq {
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
}

/// Owner side of one accepted `theuth/sessions/1` stream whose first frame
/// was [`Msg::Events`]: register with the relay, write events until the peer
/// closes or the relay drops us (ownership moved).
pub async fn serve(
    relay: &ActorRef<EventRelay>,
    mut framed: Framed<Box<dyn ByteStream>, LengthDelimitedCodec>,
    session: String,
    from_seq: u64,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(QUEUE);
    relay
        .tell(Serve {
            session,
            from_seq,
            tx,
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
    Framed::new(stream, LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME).new_codec())
}

/// Subscriber side: follow `session`'s owner until aborted; returns when the
/// owner is this engine (nothing to follow) or the session is unknown.
async fn follower(rt: Runtime, store: Store, t: Arc<dyn Transport>, session: SessionId) {
    let mut backoff = BACKOFF_MIN;
    let mut last_seq = 0u64;
    let mut peers = t.subscribe();
    loop {
        let owner = match store.owner(session).await {
            Ok(Some((o, _))) if !o.is_empty() && o != store.engine() => PeerId(o),
            _ => return,
        };
        let outcome = stream(&rt, &store, t.as_ref(), &owner, session, &mut last_seq).await;
        match outcome {
            Ok(()) => backoff = BACKOFF_MIN,
            Err(e) => tracing::debug!(%session, %owner, "events stream: {e:#}"),
        }
        // Retry after backoff, or as soon as the owner reconnects.
        let wait = tokio::time::sleep(backoff);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                () = rt.shutdown.cancelled() => return,
                ev = peers.recv() => match ev {
                    Ok(PeerEvent::Connected(p)) if p == owner => break,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                },
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One stream to `owner`: `Events`, then re-publish every `Event` locally.
async fn stream(
    rt: &Runtime,
    store: &Store,
    t: &dyn Transport,
    owner: &PeerId,
    session: SessionId,
    last_seq: &mut u64,
) -> anyhow::Result<()> {
    let mut framed = framed(t.open(owner, ALPN_SESSIONS).await?);
    framed
        .send(
            Msg::Events {
                session: session.to_string(),
                from_seq: *last_seq + 1,
            }
            .encode()?,
        )
        .await?;
    while let Some(frame) = framed.next().await {
        let Msg::Event { seq, bytes, .. } = Msg::decode(&frame?)? else {
            anyhow::bail!("unexpected frame on an events stream");
        };
        *last_seq = seq;
        let mut ev: AgentEvent = serde_json::from_slice(&bytes)?;
        // `here` is relative to the publisher; recompute for this engine.
        if let AgentEvent::SessionOwnerChanged { owner, here, .. } = &mut ev {
            *here = owner.is_empty() || *owner == store.engine();
        }
        rt.publish(ev).await;
    }
    Ok(())
}

/// Bytes of one frame as the wire carries it (for tests and L's dispatcher).
pub fn encode(m: &Msg) -> anyhow::Result<Bytes> {
    m.encode()
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
            self.inboxes.lock().keys().filter(|p| **p != self.me).cloned().collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self.inboxes.lock().get(peer).cloned().ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs))).map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("closed"))
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
        let store = Store::open(&dir.join(format!("{name}.sqlite")), Some(mesh::EngineId(name.into()))).await.unwrap();
        (rt, store)
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
        store_o.create_session(session, "m", None, None, None).await.unwrap();
        // The front door's replica row says the owner is "owner": write it
        // under that identity, then reopen the file as "front".
        let front_path = dir.path().join("front.sqlite");
        let theirs = Store::open(&front_path, Some(mesh::EngineId("owner".into()))).await.unwrap();
        theirs.create_session(session, "m", None, None, None).await.unwrap();
        theirs.close().await;
        let (rt_f, store_f) = engine(dir.path(), "front").await;

        // Two engines in one process: only the front's relay takes the registry name.
        let owner = EventRelay::spawn_on(&rt_o, store_o.clone()).await;
        owner.tell(SetTransport(owner_t.clone())).await.unwrap();
        // The owner's accept loop: first frame `Events` → `serve`.
        let (o2, acc) = (owner.clone(), owner_t.clone());
        tokio::spawn(async move {
            loop {
                let Ok((_, stream)) = acc.accept(ALPN_SESSIONS).await else { return };
                let mut framed = framed(stream);
                let Some(Ok(first)) = framed.next().await else { continue };
                let Ok(Msg::Events { session, from_seq }) = Msg::decode(&first) else { continue };
                let o = o2.clone();
                tokio::spawn(async move { serve(&o, framed, session, from_seq).await });
            }
        });

        let front = EventRelay::start(&rt_f, store_f.clone()).await.unwrap();
        front.tell(SetTransport(front_t.clone())).await.unwrap();
        let (tap_tx, mut tapped) = mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tap_tx), runtime::subscriber_mailbox());
        rt_f.subscribe::<Tap, AgentEvent>(&tap).await;

        let guard = follow(session).expect("relay registered");
        // Give the follower time to open the stream.
        tokio::time::sleep(Duration::from_millis(200)).await;
        rt_o.publish(AgentEvent::TitleChanged { session, title: "hi".into(), tags: vec![] }).await;
        let got = tokio::time::timeout(Duration::from_secs(5), tapped.recv()).await.unwrap().unwrap();
        assert!(matches!(got, AgentEvent::TitleChanged { session: s, ref title, .. } if s == session && title == "hi"), "{got:?}");

        drop(guard);
        tokio::time::sleep(Duration::from_millis(200)).await;
        rt_o.publish(AgentEvent::TitleChanged { session, title: "after".into(), tags: vec![] }).await;
        assert!(tokio::time::timeout(Duration::from_millis(500), tapped.recv()).await.is_err(), "unsubscribed");
        store_o.close().await;
        store_f.close().await;
    }
}
