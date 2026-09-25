//! Mesh plugin (design doc 67 §4): this engine's iroh endpoint, the paired
//! peer set, the stability-elected hub lease, and what every engine
//! advertises. It implements [`replica::Transport`] on the endpoint and hands
//! it to the replica plugin; every other plugin reaches peers through that.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`MESH_ACTOR`] actor ([`Mesh`]): [`Status`], [`AddPeer`],
//!   [`RemovePeer`], [`SetHubPref`], [`PairingString`], [`ListStats`], [`Subscribe`]
//! - events: [`MeshEvent`] (bus + [`Subscribe`])

pub mod config;
pub mod identity;
pub mod lease;
pub mod net;
pub mod pairing;
pub mod stats;
pub mod store;
pub mod tools;
pub mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use iroh_gossip::TopicId;
use iroh_gossip::api::Event as GossipEvent;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use config::MeshConfig;
pub use net::ALPN_MESH;
pub use types::{
    EngineClass, EngineId, EngineStats, HubPref, LeaseState, MeshEvent, MeshStatus, Peer, PeerEvent,
    PeerId,
};

use crate::identity::now_ms;
use crate::lease::Candidate;
use crate::net::Net;

/// Registry name of the [`Mesh`] actor.
pub const MESH_ACTOR: &str = "mesh";

/// Sqlite file under `data_dir` (67 §4).
pub const DB: &str = "mesh.sqlite";

/// Directory under `data_dir` holding the endpoint identity (67 §4.1).
pub const DIR: &str = "mesh";

/// This engine holds the hub lease (67 §4.4). `true` until the mesh actor
/// says otherwise, so a single engine without the mesh plugin does every hub
/// duty. Set on every [`MeshEvent::Lease`]; read by plugins whose work is a
/// hub duty (schedule ticks).
static HUB: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The mesh actor calls this on every lease change.
pub fn set_hub(hub: bool) {
    HUB.store(hub, std::sync::atomic::Ordering::Relaxed);
}

/// Is this engine the hub? `true` when no mesh is running.
pub fn is_hub() -> bool {
    HUB.load(std::sync::atomic::Ordering::Relaxed)
}

/// `ask` → `Result<MeshStatus, String>`: this engine, its peers, the lease.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Result<Peer, String>`: pair with the engine behind `pairing`
/// (`t mesh add <string>`, 67 §4.1). Mutual and transitive: both sides learn
/// each other's peer set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddPeer(pub String);

/// `ask` → `Result<(), String>`: leave an engine out of the mesh (`t mesh remove`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovePeer(pub EngineId);

/// `ask` → `Result<(), String>`: change this engine's lease preference; written
/// to `mesh.hub` in config and advertised.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetHubPref(pub HubPref);

/// `ask` → `Result<String, String>`: a pairing string another engine can
/// `t mesh add` (`t mesh pair`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PairingString;

/// `ask` → `Result<Vec<EngineStats>, String>`: what every known engine
/// advertises (this one included), for placement (`mesh_peers`, 67 §8).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListStats;

/// `ask` → `broadcast::Receiver<MeshEvent>`: live peer and lease events.
#[derive(Clone, Copy, Debug, Default)]
pub struct Subscribe;

/// What rides on the gossip topic (67 §4.3, §4.6). A heartbeat only confirms
/// liveness: everything the election needs is the replicated `engine_stats`
/// row, and `stats_hlc` names the version of that row the sender has written.
#[derive(Clone, Debug, Serialize, Deserialize)]
enum GossipMsg {
    Heartbeat {
        id: EngineId,
        is_hub: bool,
        stats_hlc: String,
    },
    LeaseRelease {
        id: EngineId,
    },
}

/// What this engine last heard from a peer.
#[derive(Clone, Debug)]
struct Seen {
    last_hb_ms: u64,
    stats_hlc: String,
}

struct Live {
    lease: LeaseState,
    seen: HashMap<EngineId, Seen>,
    self_stats: EngineStats,
    /// `hlc` of our last `engine_stats` write (what heartbeats carry).
    stats_hlc: String,
}

/// Shared by the actor and its background loops.
struct Ctx {
    rt: Runtime,
    cfg: Mutex<MeshConfig>,
    config_path: PathBuf,
    store: store::Store,
    net: Arc<Net>,
    events: broadcast::Sender<MeshEvent>,
    first_seen_ms: u64,
    started_ms: u64,
    live: Mutex<Live>,
}

impl Ctx {
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn cfg(&self) -> MeshConfig {
        Self::lock(&self.cfg).clone()
    }

    fn lease(&self) -> LeaseState {
        Self::lock(&self.live).lease.clone()
    }

    fn is_hub(&self) -> bool {
        Self::lock(&self.live).lease.holder.as_ref() == Some(&self.net.id)
    }

    async fn status(&self) -> Result<MeshStatus, String> {
        let peers = self.store.peers().await.map_err(|e| e.to_string())?;
        Ok(MeshStatus {
            self_id: self.net.id.clone(),
            peers: peers.iter().map(|p| self.net.peer_of(p)).collect(),
            lease: self.lease(),
        })
    }

    async fn emit(&self, ev: MeshEvent) {
        if let MeshEvent::Lease(l) = &ev {
            // Hub duties (67 §4.4): OAuth refresh, schedule ticks.
            let hub = l.holder.as_ref() == Some(&self.net.id);
            oauth::set_hub(hub);
            set_hub(hub);
        }
        let _ = self.events.send(ev.clone());
        self.rt.publish(ev).await;
    }

    /// Probe and store this engine's stats. Unchanged stats are not
    /// rewritten: every write is a replicated batch, and heartbeats already
    /// carry liveness.
    async fn refresh_stats(&self) {
        let (id, started, data_dir, workspaces) =
            (self.net.id.clone(), self.started_ms, self.rt.data_dir.clone(), self.cfg().workspaces);
        let Ok(s) = tokio::task::spawn_blocking(move || stats::probe(id, started, &data_dir, &workspaces)).await else {
            return;
        };
        if Self::lock(&self.live).self_stats == s {
            return;
        }
        self.write_stats(s).await;
    }

    /// Write `s` as our `engine_stats` row (current `hub_pref`), remember it.
    async fn write_stats(&self, s: EngineStats) {
        match self.store.put_stats(&s, self.cfg().hub, self.first_seen_ms, now_ms()).await {
            Ok(hlc) => {
                let mut live = Self::lock(&self.live);
                live.self_stats = s;
                live.stats_hlc = hlc;
            }
            Err(e) => tracing::warn!("mesh: engine_stats: {e:#}"),
        }
    }

    /// Election over the connected, heartbeating set (67 §4.3–§4.5). A peer
    /// whose `engine_stats` row has not replicated yet is not a candidate:
    /// nothing is requested, the change feed delivers it. Emits
    /// `MeshEvent::Lease` and appends to `lease_log` on change.
    async fn recompute(&self, hint: &str) {
        let now = now_ms();
        let cfg = self.cfg();
        let lapse_ms = cfg.lapse_secs * 1000;
        let mut cands = Vec::new();
        let (incumbent, self_stats, seen) = {
            let live = Self::lock(&self.live);
            (live.lease.holder.clone(), live.self_stats.clone(), live.seen.clone())
        };
        cands.push(Candidate {
            id: self.net.id.clone(),
            hub_pref: cfg.hub,
            availability_30d: self.store.availability_30d(&self.net.id, now).await.unwrap_or(1.0),
            class: self_stats.class,
            uptime_since_ms: self.started_ms,
            first_seen_ms: self.first_seen_ms,
            connected_since_ms: self.started_ms,
        });
        for (id, since_ms) in self.net.connected() {
            let Some(s) = seen.get(&id) else {
                continue;
            };
            if now.saturating_sub(s.last_hb_ms) > lapse_ms {
                continue;
            }
            match self.store.candidate(&id, since_ms, now).await {
                Ok(Some(c)) => cands.push(c),
                Ok(None) => tracing::debug!("mesh: no engine_stats row for {id} yet (heartbeat {}); waiting", s.stats_hlc),
                Err(e) => tracing::warn!("mesh: engine_stats for {id}: {e:#}"),
            }
        }
        let holder = lease::elect(&cands, incumbent.as_ref(), now, cfg.probation_secs * 1000, cfg.handover_secs * 1000);
        if holder == incumbent {
            return;
        }
        let reason = match (&holder, &incumbent) {
            (None, _) => "none",
            (Some(h), _) if cands.iter().any(|c| &c.id == h && c.hub_pref == HubPref::Prefer) => "prefer",
            (Some(_), None) => "elected",
            (Some(_), Some(inc)) if cands.iter().any(|c| &c.id == inc) => "handover",
            (Some(_), Some(_)) => hint,
        };
        let lease = LeaseState {
            holder: holder.clone(),
            since_ms: now,
            reason: reason.to_owned(),
        };
        Self::lock(&self.live).lease = lease.clone();
        tracing::info!("mesh: lease → {:?} ({reason})", holder.as_ref().map(|h| &h.0[..8.min(h.0.len())]));
        if let Err(e) = self.store.lease_log(now, holder.as_ref(), reason).await {
            tracing::warn!("mesh: lease_log: {e:#}");
        }
        self.emit(MeshEvent::Lease(lease)).await;
    }

    /// Record a heartbeat. The stats themselves arrive by replication.
    fn on_heartbeat(&self, hb: GossipMsg) {
        let GossipMsg::Heartbeat { id, stats_hlc, .. } = hb else {
            return;
        };
        if id == self.net.id {
            return;
        }
        Self::lock(&self.live).seen.insert(id, Seen { last_hb_ms: now_ms(), stats_hlc });
    }

    /// Everything that keeps the lease honest: heartbeats out and in,
    /// availability intervals, re-election on every change, release on stop.
    async fn run_lease(self: Arc<Self>, cancel: CancellationToken) {
        let topic = TopicId::from_bytes(sha2::Sha256::digest(b"theuth/mesh/lease/1").into());
        let Ok(topic) = self.net.gossip.subscribe(topic, self.net.paired_ids()).await else {
            tracing::warn!("mesh: gossip subscribe failed; lease disabled");
            return;
        };
        let (sender, mut receiver) = topic.split();
        let mut peer_events = self.net.subscribe_peers();
        let mut peers_gen = self.net.peers_gen.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(self.cfg().heartbeat_secs.max(1)));
        let mut known = self.net.paired_ids();
        // Every peer starts an "unreachable" interval; connects flip it.
        if let Ok(peers) = self.store.peers().await {
            for p in &peers {
                let _ = self.store.availability_open(&self.net.id, &p.id, false, now_ms()).await;
            }
        }
        self.recompute("start").await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => {
                    let now = now_ms();
                    let connected = self.net.connected();
                    for (id, _) in &connected {
                        let _ = self.store.availability_touch(&self.net.id, id, now).await;
                    }
                    let (is_hub, stats_hlc) = {
                        let live = Self::lock(&self.live);
                        (live.lease.holder.as_ref() == Some(&self.net.id), live.stats_hlc.clone())
                    };
                    let hb = GossipMsg::Heartbeat { id: self.net.id.clone(), is_hub, stats_hlc };
                    if let Ok(bytes) = serde_json::to_vec(&hb) {
                        let _ = sender.broadcast(bytes.into()).await;
                    }
                    self.recompute("lapse").await;
                }
                ev = receiver.next() => match ev {
                    Some(Ok(GossipEvent::Received(msg))) => match serde_json::from_slice::<GossipMsg>(&msg.content) {
                        Ok(hb @ GossipMsg::Heartbeat { .. }) => {
                            self.on_heartbeat(hb);
                            self.recompute("heartbeat").await;
                        }
                        Ok(GossipMsg::LeaseRelease { id }) => {
                            Self::lock(&self.live).seen.remove(&id);
                            self.recompute("release").await;
                        }
                        Err(e) => tracing::debug!("mesh: gossip payload: {e}"),
                    },
                    Some(Ok(_)) => {}
                    Some(Err(e)) => tracing::debug!("mesh: gossip: {e}"),
                    None => break,
                },
                ev = peer_events.recv() => match ev {
                    Ok(ev) => {
                        let (p, up) = match &ev {
                            PeerEvent::Connected(p) => (p, true),
                            PeerEvent::Disconnected(p) => (p, false),
                        };
                        let id = EngineId(p.0.clone());
                        let _ = self.store.availability_open(&self.net.id, &id, up, now_ms()).await;
                        self.emit(MeshEvent::Peer(ev)).await;
                        if !up {
                            Self::lock(&self.live).seen.remove(&id);
                            self.recompute("disconnect").await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                },
                r = peers_gen.changed() => {
                    if r.is_err() { break; }
                    let now = self.net.paired_ids();
                    let fresh: Vec<_> = now.iter().filter(|id| !known.contains(id)).copied().collect();
                    if !fresh.is_empty() {
                        let _ = sender.join_peers(fresh).await;
                    }
                    known = now;
                }
            }
        }
        if self.is_hub()
            && let Ok(bytes) = serde_json::to_vec(&GossipMsg::LeaseRelease { id: self.net.id.clone() })
        {
            let _ = sender.broadcast(bytes.into()).await;
            let lease = LeaseState { holder: None, since_ms: now_ms(), reason: "release".into() };
            Self::lock(&self.live).lease = lease.clone();
            let _ = self.store.lease_log(now_ms(), None, "release").await;
            self.emit(MeshEvent::Lease(lease)).await;
        }
    }
}

/// The [`MESH_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Mesh {
    ctx: Arc<Ctx>,
}

impl Message<Status> for Mesh {
    type Reply = Result<MeshStatus, String>;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.status().await
    }
}

impl Message<AddPeer> for Mesh {
    type Reply = DelegatedReply<Result<Peer, String>>;

    async fn handle(&mut self, m: AddPeer, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let c = self.ctx.clone();
        // Network round trips: off the mailbox.
        tokio::spawn(async move {
            let r = c.net.add_peer(&m.0).await.map_err(|e| format!("{e:#}"));
            if r.is_ok() {
                c.net.push_peers().await;
                c.recompute("pair").await;
            }
            tx.send(r);
        });
        reply
    }
}

impl Message<RemovePeer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RemovePeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.net.remove_peer(&m.0).await.map_err(|e| format!("{e:#}"))?;
        self.ctx.net.push_peers().await;
        Ctx::lock(&self.ctx.live).seen.remove(&m.0);
        self.ctx.recompute("remove").await;
        Ok(())
    }
}

impl Message<SetHubPref> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetHubPref, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        runtime::write_config_value(&self.ctx.config_path, "mesh", "hub", store::hub_pref_str(m.0))
            .map_err(|e| format!("write {}: {e}", self.ctx.config_path.display()))?;
        Ctx::lock(&self.ctx.cfg).hub = m.0;
        self.ctx.net.set_hub_pref(m.0);
        let s = Ctx::lock(&self.ctx.live).self_stats.clone();
        self.ctx.write_stats(s).await;
        self.ctx.recompute("hub_pref").await;
        Ok(())
    }
}

impl Message<PairingString> for Mesh {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: PairingString, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self.ctx.net.pairing_string().await)
    }
}

impl Message<ListStats> for Mesh {
    type Reply = Result<Vec<EngineStats>, String>;

    async fn handle(&mut self, _: ListStats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.store.stats().await.map_err(|e| e.to_string())
    }
}

impl Message<Subscribe> for Mesh {
    type Reply = broadcast::Receiver<MeshEvent>;

    async fn handle(&mut self, _: Subscribe, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.events.subscribe()
    }
}

/// The plugin: reads `mesh`, opens the store, binds the endpoint, spawns
/// [`Mesh`] as [`MESH_ACTOR`], hands the transport to the replica plugin,
/// registers the `mesh_peers` tool once the registry is up.
pub struct MeshPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for MeshPlugin {
    fn name(&self) -> &'static str {
        "mesh"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        // SIGTERM: release the lease gracefully (67 §4.6), then stop — armed
        // before anything below can bail out, so a mesh without its store
        // still stops cleanly. macOS `willSleep` has no platform hook yet.
        let stop = rt.shutdown.clone();
        tokio::spawn(async move {
            if platform::process::stop_signal().await.is_ok() {
                stop.cancel();
            }
        });
        let cfg: MeshConfig = rt.section("mesh").unwrap_or_else(|e| {
            tracing::debug!("no [mesh] config section, using defaults: {e}");
            MeshConfig::default()
        });
        let dir = rt.data_dir.join(DIR);
        let identity = match identity::Identity::load_or_create(&dir) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("mesh unavailable: {e:#}");
                return Ok(());
            }
        };
        let store = match store::Store::open(&rt.data_dir.join(DB), identity.id()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("mesh unavailable: {e:#}");
                return Ok(());
            }
        };
        let name = match cfg.name.trim() {
            "" => platform::process::hostname().to_owned(),
            n => n.to_owned(),
        };
        let net = match Net::bind(
            net::BindOpts {
                secret: identity.secret,
                relay: cfg.relay.clone(),
                discovery: cfg.discovery,
                bind_addr: cfg.bind,
            },
            store.clone(),
            name,
            cfg.hub,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("mesh unavailable: {e:#}");
                return Ok(());
            }
        };
        tracing::info!("mesh: engine {} on relay {} (discovery {})", net.id, cfg.relay, cfg.discovery);

        // The replica plugin starts after this one: attach when it is up.
        let (rt2, pool) = (rt.clone(), store.pool().clone());
        tokio::spawn(async move {
            if rt2.lookup_when::<replica::Replica>(replica::REPLICA_ACTOR).await.is_none() {
                return;
            }
            match replica::attach(&pool, &["peers", "engine_stats", "availability"]).await {
                Ok(_) => tracing::info!("mesh: peers/engine_stats/availability replicated"),
                Err(e) => tracing::info!("mesh: replication unavailable ({e}); local only"),
            }
        });

        let (events, _) = broadcast::channel(256);
        let started_ms = now_ms();
        let self_stats = {
            let (id, data_dir, workspaces) = (net.id.clone(), rt.data_dir.clone(), cfg.workspaces.clone());
            tokio::task::spawn_blocking(move || stats::probe(id, started_ms, &data_dir, &workspaces))
                .await
                .unwrap_or_else(|_| EngineStats {
                    id: net.id.clone(),
                    uptime_since_ms: started_ms,
                    class: EngineClass::Desktop,
                    availability_30d: 1.0,
                    harnesses: vec!["native".into()],
                    repos: vec![],
                    display: false,
                    gpu: false,
                })
        };
        let stats_hlc = match store.put_stats(&self_stats, cfg.hub, identity.first_seen_ms, started_ms).await {
            Ok(hlc) => hlc,
            Err(e) => {
                tracing::warn!("mesh: engine_stats: {e:#}");
                String::new()
            }
        };
        let ctx = Arc::new(Ctx {
            rt: rt.clone(),
            cfg: Mutex::new(cfg.clone()),
            config_path: rt.data_dir.join("config.yaml"),
            store,
            net: net.clone(),
            events,
            first_seen_ms: identity.first_seen_ms,
            started_ms,
            live: Mutex::new(Live {
                lease: LeaseState {
                    holder: None,
                    since_ms: started_ms,
                    reason: "start".into(),
                },
                seen: HashMap::new(),
                self_stats,
                stats_hlc,
            }),
        });

        let actor = Mesh::spawn(Mesh { ctx: ctx.clone() });
        actor.register(MESH_ACTOR)?;

        // The transport, to the replica plugin whenever it comes up.
        let (rt2, transport) = (rt.clone(), net.clone());
        tokio::spawn(async move {
            if let Some(replica) = rt2.lookup_when::<replica::Replica>(replica::REPLICA_ACTOR).await {
                let t: Arc<dyn replica::Transport> = transport;
                let _ = replica.ask(replica::RegisterTransport(t)).await;
            }
        });

        // Background loops, all ending on shutdown; the lease loop releases first.
        let cancel = rt.shutdown.child_token();
        tokio::spawn(net.clone().run_connector(cancel.clone()));
        let c = ctx.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(cfg.capabilities_secs.max(30)));
            every.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = every.tick() => c.refresh_stats().await,
                }
            }
        });
        let (c, n, stop) = (ctx.clone(), net.clone(), rt.shutdown.clone());
        tokio::spawn(async move {
            c.run_lease(stop.child_token()).await;
            n.shutdown().await;
        });
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg).await;
            }
        });
        Ok(())
    }
}

/// The mesh actor; `None` before the plugin started or when its store failed.
pub fn actor() -> Option<ActorRef<Mesh>> {
    ActorRef::<Mesh>::lookup(MESH_ACTOR).ok().flatten()
}
