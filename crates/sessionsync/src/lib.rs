//! Sessions everywhere (design doc 68): every durable session is written on
//! one engine — its owner — and copied to every other engine as a
//! commit-ordered log of SQLite changesets over `theuth/sessions/1`.
//!
//! The agent store captures and logs (`agent::store`, 68 §3); this crate
//! ships the log, bootstraps peers with snapshots, applies what arrives, and
//! moves ownership. The contract commit (worker C) holds the actor, the wire
//! types, the config and the apply entry points; L fills shipping/snapshot/
//! pruning (`ship.rs`, `sync.rs`, `snapshot.rs`, `prune.rs`), O ownership
//! (`owner.rs`), F remote events (`events.rs`).

use std::collections::HashMap;
use std::sync::Arc;

use agent::store::Store;
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use replica::{ALPN_SESSIONS, PeerEvent, PeerId, Transport};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// Provider-owned RPC extension contracts.
pub mod rpc_support;
pub mod apply;
pub mod config;
pub mod events;
pub mod owner;
pub mod prune;
pub mod ship;
pub mod snapshot;
pub mod sync;
pub mod types;

pub use config::SessionsConfig;

/// Registry name of the [`SessionSync`] actor.
pub const SESSIONS_ACTOR: &str = "sessions";

/// Why a session is being taken (68 §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeReason {
    /// The user said "continue here": may take at once, accepting §4.3's tail loss.
    User,
    /// A task checkout was resumed here (67 §7.5): the desk follows.
    Orphan,
    /// This engine needs to write and the owner has been unreachable ≥ `takeover_secs`.
    Write,
    /// 181 D2: the owner asked this engine to take the session (Move to ▸):
    /// the owner must be connected and release; the open turn always resumes here.
    Moved,
}

impl TakeReason {
    /// Parse the RPC spelling (`user` | `orphan` | `write` | `moved`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "orphan" => Some(Self::Orphan),
            "write" => Some(Self::Write),
            "moved" => Some(Self::Moved),
            _ => None,
        }
    }
}

/// `tell`: the mesh transport is up (sent once by the plugin's waiter).
pub struct TransportReady(pub Arc<dyn Transport>);

/// `tell`: a peer connected; the sync worker dials it (L).
#[derive(Clone, Debug)]
pub struct PeerConnected(pub PeerId);

/// `tell`: a peer disconnected.
#[derive(Clone, Debug)]
pub struct PeerGone(pub PeerId);

/// `tell`: ship every unshipped log row now (L); a no-op until L.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShipNow;

/// `ask` → `Result<(owner, epoch), String>`: take `id` here (68 §4.2; [`owner::Take`]).
#[derive(Clone, Debug)]
pub struct TakeSession {
    /// The session.
    pub id: runtime::SessionId,
    /// Why.
    pub reason: TakeReason,
    /// 139 Revision 1 Q10: a `Write` take made for a client's write on this
    /// engine's front door (a human typed) resumes the taken open turn like
    /// `User`; system takes (desk wakes, hub-failover plumbing) leave it.
    /// Only the resume policy reads it — 68's `unseen_long_enough` gate
    /// still keys on `reason`.
    pub by_user: bool,
    /// 181: the engine that forwarded this take (`rpc::forward::Forwarded`);
    /// `Moved` is honoured only from the session's owner, else it is `User`.
    pub caller: Option<PeerId>,
}

/// `ask` → the connected peers, as the transport reports them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Peers;

/// The [`SESSIONS_ACTOR`] actor: the durable store, the transport once the
/// mesh is up, and the peers currently connected.
#[derive(kameo::Actor)]
pub struct SessionSync {
    rt: Runtime,
    /// `sessions:` config.
    pub config: SessionsConfig,
    /// The agent's durable store (never a private one).
    pub store: Store,
    transport: Option<Arc<dyn Transport>>,
    /// Connected peers and whether we dial a `theuth/sessions/1` stream to them.
    peers: HashMap<PeerId, bool>,
    /// The sync worker: streams, relay feed, prune timer.
    shipper: Arc<ship::Shipper>,
}

impl SessionSync {
    /// The transport, once the mesh registered one.
    pub fn transport(&self) -> Option<Arc<dyn Transport>> {
        self.transport.clone()
    }

    /// Accept `theuth/sessions/1` streams and forward connect/disconnect
    /// events to the actor; each accepted stream is handed to [`Self::serve`].
    fn start_transport(&mut self, t: Arc<dyn Transport>, me: ActorRef<Self>) {
        self.transport = Some(t.clone());
        if let Some(ev) = events::actor() {
            let _ = ev.tell(events::SetTransport(t.clone())).try_send();
        }
        for p in t.peers() {
            self.shipper.dial(t.clone(), p.clone());
            self.peers.insert(p, true);
        }
        let accept = t.clone();
        let shipper = self.shipper.clone();
        tokio::spawn(async move {
            loop {
                match accept.accept(ALPN_SESSIONS).await {
                    Ok((peer, stream)) => {
                        let shipper = shipper.clone();
                        tokio::spawn(async move {
                            if let Err(e) = Self::serve(shipper, peer.clone(), stream).await {
                                tracing::debug!("sessions: stream from {peer} ended: {e:#}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("sessions: accept: {e:#}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });
        let mut events = t.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(PeerEvent::Connected(p)) => {
                        let _ = me.tell(PeerConnected(p)).await;
                    }
                    Ok(PeerEvent::Disconnected(p)) => {
                        let _ = me.tell(PeerGone(p)).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// One accepted stream. An [`types::Msg::Events`] first frame is a remote
    /// event subscription (F, `events.rs`); a [`types::Msg::Release`] is a
    /// taker asking us to let go (O, `owner.rs`); anything else is the Hello/
    /// log/snapshot protocol of [`sync`], which takes over with that frame read.
    async fn serve(
        shipper: Arc<ship::Shipper>,
        peer: PeerId,
        stream: Box<dyn replica::ByteStream>,
    ) -> anyhow::Result<()> {
        let mut framed = events::framed(stream);
        let first = futures::StreamExt::next(&mut framed)
            .await
            .ok_or_else(|| anyhow::anyhow!("closed before the first frame"))??;
        match types::Msg::decode(&first)? {
            types::Msg::Events { session, from_seq } => {
                let relay =
                    events::actor().ok_or_else(|| anyhow::anyhow!("events relay not running"))?;
                events::serve(&relay, framed, session, from_seq, false).await
            }
            // 183 §5.1 (W-18): Events plus the session's approvals, asks, jobs and cells.
            types::Msg::Watch { session, from_seq } => {
                let relay =
                    events::actor().ok_or_else(|| anyhow::anyhow!("events relay not running"))?;
                events::serve(&relay, framed, session, from_seq, true).await
            }
            types::Msg::Release { session, schema } => {
                owner::answer_release(&shipper.node().store, &peer, schema, session, &mut framed)
                    .await
            }
            other => shipper.serve_with_first(peer, framed, other).await,
        }
    }
}

impl Message<TransportReady> for SessionSync {
    type Reply = ();

    async fn handle(
        &mut self,
        m: TransportReady,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        self.start_transport(m.0, me);
    }
}

impl Message<PeerConnected> for SessionSync {
    type Reply = ();

    async fn handle(
        &mut self,
        m: PeerConnected,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(ev) = events::actor() {
            let _ = ev.tell(events::Reconnected(m.0.clone())).try_send();
        }
        let dialing = match &self.transport {
            Some(t) => {
                self.shipper.dial(t.clone(), m.0.clone());
                true
            }
            None => false,
        };
        self.peers.insert(m.0, dialing);
    }
}

impl Message<PeerGone> for SessionSync {
    type Reply = ();

    async fn handle(&mut self, m: PeerGone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.peers.remove(&m.0);
        self.shipper.hangup(&m.0);
    }
}

impl Message<ShipNow> for SessionSync {
    type Reply = ();

    async fn handle(&mut self, _: ShipNow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.shipper.ship_now();
    }
}

impl Message<TakeSession> for SessionSync {
    type Reply = kameo::reply::DelegatedReply<Result<(String, u32), String>>;

    async fn handle(
        &mut self,
        m: TakeSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // The mesh's last-seen view, for an unreachable owner (68 §4.2 (c)).
        let peers = match mesh::actor() {
            Some(mesh) => mesh
                .ask(mesh::Status)
                .await
                .map(|s| s.peers)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let take = owner::Take {
            rt: self.rt.clone(),
            config: self.config.clone(),
            store: self.store.clone(),
            transport: self.transport.clone(),
            connected: self.peers.keys().cloned().collect(),
            peers,
            by_user: m.by_user,
            caller: m.caller,
        };
        // Off the mailbox: waits on the owner's release and on the tail replicating.
        tokio::spawn(async move { tx.send(take.run(m.id, m.reason).await) });
        reply
    }
}

impl Message<owner::Demote> for SessionSync {
    type Reply = ();

    async fn handle(
        &mut self,
        m: owner::Demote,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (session, by) = (m.session, m.by.clone());
        match owner::demote(&self.rt, &self.store, m).await {
            Ok(n) => tracing::info!("sessions: {session} taken by {by}; {n} rows orphaned here"),
            Err(e) => tracing::warn!("sessions: demote {session}: {e:#}"),
        }
    }
}

impl Message<Peers> for SessionSync {
    type Reply = Vec<PeerId>;

    async fn handle(&mut self, _: Peers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.peers.keys().cloned().collect()
    }
}

/// The plugin: reads `sessions:`, takes the agent's durable store, spawns
/// [`SessionSync`] as [`SESSIONS_ACTOR`] and hands it the transport once
/// the replica plugin has one. Starts after `agent` and `replica`.
pub struct SessionSyncPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SessionSyncPlugin {
    fn name(&self) -> &'static str {
        "sessions"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let config: SessionsConfig = rt.section("sessions").unwrap_or_else(|e| {
            tracing::debug!("no [sessions] config section, using defaults: {e}");
            SessionsConfig::default()
        });
        let Some(agents) = rt.lookup_when::<agent::Agents>(agent::AGENT_ACTOR).await else {
            return Ok(());
        };
        let store = match agents.ask(agent::DurableStore).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("sessions unavailable: no durable store: {e}");
                return Ok(());
            }
        };
        if let Err(e) = events::EventRelay::start(rt, store.clone()).await {
            tracing::warn!("sessions: remote events unavailable: {e:#}");
        }
        let shipper = ship::Shipper::new(store.clone(), Some(rt.clone()), &config);
        tokio::spawn(owner::sweep_orphans(
            store.clone(),
            config.orphan_keep_days,
            rt.shutdown.clone(),
        ));
        let actor = SessionSync::spawn(SessionSync {
            rt: rt.clone(),
            config,
            store,
            transport: None,
            peers: HashMap::new(),
            shipper,
        });
        actor.register(SESSIONS_ACTOR)?;
        let rt = rt.clone();
        let me = actor.clone();
        tokio::spawn(async move {
            if let Some(t) = replica::transport(&rt).await {
                let _ = me.tell(TransportReady(t)).await;
            }
        });
        Ok(())
    }
}

/// `session_orphans(id)`: this engine's orphaned log rows of `id`, one entry
/// per superseded epoch, oldest first.
pub async fn orphans(
    store: &Store,
    id: runtime::SessionId,
) -> anyhow::Result<Vec<types::SessionOrphan>> {
    let mut out: Vec<types::SessionOrphan> = Vec::new();
    for row in store.log_orphans(id).await? {
        let epoch = u32::try_from(row.epoch).unwrap_or_default();
        let seq = u64::try_from(row.seq).unwrap_or_default();
        match out.last_mut() {
            Some(o) if o.epoch == epoch => o.to_seq = seq,
            _ => out.push(types::SessionOrphan {
                epoch,
                from_seq: seq,
                to_seq: seq,
                at: replica::Hlc::parse(&row.hlc)
                    .and_then(|h| {
                        chrono::DateTime::from_timestamp_millis(i64::try_from(h.wall_ms).ok()?)
                    })
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
            }),
        }
    }
    Ok(out)
}

/// `session_owner(id)`: the session row's owner and whether it is this engine.
pub async fn owner(
    store: &Store,
    id: runtime::SessionId,
) -> anyhow::Result<Option<types::SessionOwner>> {
    Ok(store
        .owner(id)
        .await?
        .map(|(owner, epoch)| types::SessionOwner {
            here: owner.is_empty() || owner == store.engine(),
            owner,
            epoch,
        }))
}

/// The sessions actor; `None` before the plugin started.
pub fn actor() -> Option<ActorRef<SessionSync>> {
    ActorRef::<SessionSync>::lookup(SESSIONS_ACTOR)
        .ok()
        .flatten()
}

impl SessionSync {
    /// The runtime (shutdown token, data dir), for L/O workers.
    pub fn runtime(&self) -> &Runtime {
        &self.rt
    }
}
