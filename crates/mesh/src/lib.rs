//! Mesh plugin (design doc 67 §4): this engine's iroh endpoint, the paired
//! peer set, the stability-elected hub lease, and what every engine
//! advertises. It implements [`replica::Transport`] on the endpoint and hands
//! it to the replica plugin; every other plugin reaches peers through that.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`MESH_ACTOR`] actor ([`Mesh`]): [`Status`], [`AddPeer`],
//!   [`RemovePeer`], [`SetHubPref`], [`PairingString`], [`ListStats`], [`Subscribe`];
//!   183 Phase 2: [`Nearby`], [`NearbyWatch`], [`PairNearby`], [`PairAnswer`],
//!   [`SetMessaging`], [`SetNearby`], [`Rename`], [`PreviewUnpair`], [`RotateVault`]
//! - events: [`MeshEvent`] (bus + [`Subscribe`])
//!
//! 183 Phase 2 module split: Nearby and the pairing ceremony live in `nearby.rs`, the
//! vault-key frames, keyring and rotation hooks in `keys.rs`; `net.rs` and this file only
//! call into them.

pub mod config;
pub mod identity;
mod keys;
pub mod lease;
pub mod nearby;
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
    Accelerator, AlpnTraffic, DocsGateState, EngineClass, EngineId, EngineStats, HubPref,
    LeaseState, MeshEvent, MeshStatus, NearbyEngine, PairCeremony, Peer, PeerEvent, PeerId,
    PeerPath, QuarantinedPeer, RelayState, UnpairPreview,
};

use crate::identity::now_ms;
use crate::lease::Candidate;
use crate::net::Net;

/// Registry name of the [`Mesh`] actor.
pub const MESH_ACTOR: &str = "mesh";

/// Sqlite file under `data_dir` (67 §4).
pub const DB: &str = "mesh.sqlite";

/// How long the start-up rows wait for the replica plugin before they are written local-only.
const ATTACH_WAIT: Duration = Duration::from_secs(10);

/// 182 S9: how long past `lapse` a just-started engine follows an elder holder.
const RESTART_HOLD_MS: u64 = 30_000;

/// Directory under `data_dir` holding the endpoint identity (67 §4.1).
pub const DIR: &str = "mesh";

/// 184 §8.1: the docs schema this build speaks on `theuth/docs/1` (1 = the 69 docs as built; 2 =
/// homes, tags 10-14), advertised in the control connection's `DocsSchema` frame.
pub const DOCS_SCHEMA: u32 = 2;

/// 184 §8.1 (Sky Q5): a paired engine last heard this long ago or more stops counting for the
/// docs gate's floor.
pub const DOCS_STALE_MS: u64 = 7 * 86_400_000;

/// 184 §8.1: the floor over this engine's paired set: every peer heard (or, never heard, first
/// listed) less than [`DOCS_STALE_MS`] ago advertises `docs_schema >= DOCS_SCHEMA`. A counted peer
/// never heard counts as 0 (off until heard); one with no row at all fails.
pub(crate) fn docs_floor(peers: &[EngineId], seen: &[store::DocsSeen], now: u64) -> bool {
    peers
        .iter()
        .all(|p| match seen.iter().find(|s| &s.id == p) {
            Some(s) => {
                s.docs_schema >= DOCS_SCHEMA
                    || now.saturating_sub(s.heard_ms.unwrap_or(s.first_ms)) >= DOCS_STALE_MS
            }
            None => false,
        })
}

/// 184 §8.1, fail-closed: homes are on only when this engine's floor agrees and the holder says
/// on — this engine itself when it holds the lease, else the holder's frame on its own
/// authenticated control connection (`None`: no holder, no connection or no frame yet).
pub(crate) fn docs_homes_on(
    floor_ok: bool,
    holder_is_me: bool,
    holder_frame: Option<bool>,
) -> bool {
    floor_ok && (holder_is_me || holder_frame == Some(true))
}

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

/// 182 M2a: the first sync with the mesh is done (or there is nothing to sync
/// with). `true` without a mesh or without paired peers; an engine with peers
/// starts `false` until every connected peer's first replica exchange landed.
static SYNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The mesh plugin flips this at start and once the first sync is done.
pub fn set_synced(done: bool) {
    SYNCED.store(done, std::sync::atomic::Ordering::Relaxed);
}

/// Task work (dispatch, sweeps, owner writes, desk-turn resumes) may run on this
/// engine's rows: the first sync is done (182 M2a).
pub fn synced() -> bool {
    SYNCED.load(std::sync::atomic::Ordering::Relaxed)
}

/// 182 M1: re-announce `table`'s rows matching `filter` (one `?`, bound to
/// `""`) — only `columns` when given — when a peer connects, at most once per
/// peer a minute (a flapping link). A peer that attached a column after our
/// writes went by recorded them unapplied; this sends them again under their
/// own clocks (LWW-safe, a no-op elsewhere).
/// ponytail: the whole matching set per connect; a per-peer digest if tables grow large.
pub fn reannounce_on_connect(
    rt: &Runtime,
    handle: replica::ReplicaHandle,
    table: &'static str,
    filter: &'static str,
    columns: Option<&'static [&'static str]>,
) {
    let rt = rt.clone();
    tokio::spawn(async move {
        let Some(mesh) = rt.lookup_when::<Mesh>(MESH_ACTOR).await else {
            return;
        };
        let Ok(mut events) = mesh.ask(Subscribe).await else {
            return;
        };
        let mut last: HashMap<String, tokio::time::Instant> = HashMap::new();
        loop {
            match events.recv().await {
                Ok(MeshEvent::Peer(replica::PeerEvent::Connected(p))) => {
                    let now = tokio::time::Instant::now();
                    if last
                        .get(&p.0)
                        .is_some_and(|t| now - *t < Duration::from_secs(60))
                    {
                        continue;
                    }
                    last.insert(p.0, now);
                    match handle.reannounce(table, filter, "", columns).await {
                        Ok(n) => tracing::debug!(table, n, "182: rows re-announced on connect"),
                        Err(e) => tracing::warn!(table, "182: re-announce: {e:#}"),
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

/// This engine's display name once the plugin read `mesh.name` (67 §8); empty
/// until then. Read by plugins naming the local engine in text (`agent:
/// "<harness>@<engine>"`, `tasks::place`) without a round trip to the actor.
static SELF_NAME: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// The mesh plugin sets this at start (and a test may, to name the engine).
pub fn set_self_name(name: &str) {
    *SELF_NAME.write().unwrap_or_else(|p| p.into_inner()) = name.to_owned();
}

/// This engine's name as its peers see it: `mesh.name`, else the hostname.
pub fn self_name() -> String {
    let n = SELF_NAME.read().unwrap_or_else(|p| p.into_inner());
    if n.is_empty() {
        platform::process::hostname().to_owned()
    } else {
        n.clone()
    }
}

/// This engine's mesh id once the plugin read its identity (182: the
/// `engine:` key a new schedule is stamped with); `None` before or without a mesh.
static SELF_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The mesh plugin sets this at start.
pub fn set_self_id(id: &str) {
    let _ = SELF_ID.set(id.to_owned());
}

/// This engine's mesh id; `None` before the mesh plugin started or without one.
pub fn self_id() -> Option<String> {
    SELF_ID.get().cloned()
}

/// XTICK: this engine's mesh id from `data_dir`'s identity file — readable
/// before the mesh plugin starts; `None` without one (never creates it).
pub fn stored_self_id(data_dir: &std::path::Path) -> Option<String> {
    let dir = data_dir.join(DIR);
    if !dir.join("identity").exists() {
        return None;
    }
    identity::Identity::load_or_create(&dir)
        .ok()
        .map(|i| i.id().0)
}

/// The bound endpoint, for [`is_paired`]; set once at plugin start.
static NET: std::sync::OnceLock<Arc<Net>> = std::sync::OnceLock::new();

/// 183 §7.2: the bound endpoint, for the a2a plugin's `theuth/a2a/1` reach; `None` before the
/// mesh plugin bound it (or without a mesh).
pub fn net() -> Option<Arc<Net>> {
    NET.get().cloned()
}

/// 182 S3/S8: `id` (a mesh engine id) is a paired peer of this engine;
/// `None` (unknown) before the mesh plugin bound its endpoint.
pub fn is_paired(id: &str) -> Option<bool> {
    NET.get()
        .map(|n| n.paired_ids().iter().any(|p| p.to_string() == id))
}

/// XTICK: this engine has a paired peer; `None` (unknown) before the mesh
/// plugin bound its endpoint.
pub fn has_peers() -> Option<bool> {
    NET.get().map(|n| !n.paired_ids().is_empty())
}

/// 183 R1 (S8): the mesh engines connected right now (ids as text); `None`
/// before the mesh plugin bound its endpoint (or without a mesh).
pub fn connected_ids() -> Option<Vec<String>> {
    NET.get()
        .map(|n| n.connected().into_iter().map(|(id, _)| id.0).collect())
}

/// 183 §6.1: how many machines this engine is paired with; `None` before the mesh plugin bound
/// its endpoint (or without a mesh). The agent's absent-`agent.communication` default reads it.
pub fn paired_count() -> Option<usize> {
    NET.get().map(|n| n.paired_ids().len())
}

/// 183 §6.3: when this engine last heard from `id` (ms since epoch), from the connection
/// bookkeeping peer events keep, without asking the actor; `None` when never seen this run.
pub fn last_seen_ms(id: &str) -> Option<u64> {
    NET.get()?.last_seen(id)
}

/// 183 §6.2 (W-14): `mesh.messaging_off`, kept current by the plugin and [`SetMessaging`].
static MESSAGING_OFF: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Replace the per-machine deny list (the plugin at start, [`SetMessaging`], tests).
pub fn set_messaging_off(ids: Vec<String>) {
    *MESSAGING_OFF.write().unwrap_or_else(|p| p.into_inner()) = ids;
}

/// 183 §6.2: agent messages between this engine and `id` are off here (`mesh.messaging_off`).
pub fn messaging_off(id: &str) -> bool {
    MESSAGING_OFF
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|x| x == id)
}

/// `data_dir`'s mesh store opened read-only (before the mesh plugin starts);
/// `None` without one.
async fn stored_pool(data_dir: &std::path::Path) -> Option<sqlx::SqlitePool> {
    let path = data_dir.join(DB);
    if !path.exists() {
        return None;
    }
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .read_only(true);
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .ok()
}

/// 182 M2a/S11: `data_dir`'s mesh store lists a paired peer — readable before
/// the mesh plugin starts (read-only; `false` without a store).
pub async fn has_stored_peers(data_dir: &std::path::Path) -> bool {
    let Some(pool) = stored_pool(data_dir).await else {
        return false;
    };
    let n: Result<(i64,), _> = sqlx::query_as("SELECT COUNT(*) FROM peers")
        .fetch_one(&pool)
        .await;
    pool.close().await;
    n.is_ok_and(|(n,)| n > 0)
}

/// XTICK: [`has_peers`] before the mesh plugin starts — the stored paired set
/// as [`store::Store::peers`] reads it (183 R4): not this engine, not
/// tombstoned, not quarantined. A store from before 183 (no `peer_origin`
/// yet) has no quarantine to leave out.
pub async fn has_stored_paired(data_dir: &std::path::Path) -> bool {
    let Some(pool) = stored_pool(data_dir).await else {
        return false;
    };
    let me = stored_self_id(data_dir).unwrap_or_default();
    let mut n: Result<(i64,), _> = sqlx::query_as(
        "SELECT COUNT(*) FROM peers p WHERE p.id != ?1 AND p.removed_ms IS NULL \
         AND NOT EXISTS (SELECT 1 FROM peer_origin o \
             WHERE o.id = p.id AND o.quarantined AND NOT o.confirmed)",
    )
    .bind(&me)
    .fetch_one(&pool)
    .await;
    if n.is_err() {
        n = sqlx::query_as(
            "SELECT COUNT(*) FROM peers p WHERE p.id != ?1 AND p.removed_ms IS NULL",
        )
        .bind(&me)
        .fetch_one(&pool)
        .await;
    }
    pool.close().await;
    n.is_ok_and(|(n,)| n > 0)
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

/// 183 R1 (D11, S6): `ask` → `Result<(), String>`: [`RemovePeer`], with the rotation's record
/// carrying `device_reset_pending` (written with the mint, so a crash before the device-key
/// reset retries it) when `reset_devices` (`mesh_remove(id, true)`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unpair {
    /// The engine to leave out.
    pub id: EngineId,
    /// Also reset phone and iPad pairing.
    pub reset_devices: bool,
}

/// 183 R1 (S1): `ask` → `Result<(), String>`: keep (`true`) or drop a quarantined machine
/// (`mesh_confirm_peer`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfirmPeer {
    /// The quarantined engine.
    pub id: EngineId,
    /// Pair it again.
    pub keep: bool,
}

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

/// 183 §10.1: `ask` → `Result<Vec<NearbyEngine>, String>`: engines the Nearby browse sees,
/// not paired (`mesh_nearby`); empty while no window is open.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Nearby;

/// 183 §10.1: `ask` → `Result<(), String>`: open (`true`, a 30 s lease the caller renews) or
/// close the Nearby window: advertise + browse `_theuth-mesh._udp` (`mesh_nearby_watch`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct NearbyWatch(pub bool);

/// 183 §10.1: `ask` → `Result<PairCeremony, String>`: start a code-checked pairing with a
/// Nearby engine (`mesh_pair_nearby`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairNearby(pub EngineId);

/// 183 §10.1: `ask` → `Result<(), String>`: this side's Pair (`accept`) or Cancel for a
/// ceremony (`mesh_pair_answer`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PairAnswer {
    /// [`PairCeremony::id`].
    pub ceremony: uuid::Uuid,
    /// Pair (`true`) or Cancel.
    pub accept: bool,
}

/// 183 §6.2 (W-14): `ask` → `Result<(), String>`: let agents on this engine and `id`
/// message each other (`on`), or not; writes `mesh.messaging_off` (`mesh_set_messaging`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetMessaging {
    /// The other engine.
    pub id: EngineId,
    /// Agents can message.
    pub on: bool,
}

/// 183 §6.7: `ask` → `Result<(), String>`: the "Let nearby machines find this one while
/// pairing" switch; writes `mesh.nearby` (`mesh_set_nearby`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetNearby(pub bool);

/// 183 §6.7: `ask` → `Result<(), String>`: rename a machine (`mesh_rename`): `mesh.name` for
/// this engine, the replicated `peers.name` otherwise.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rename {
    /// The machine.
    pub id: EngineId,
    /// Its new display name.
    pub name: String,
}

/// 183 §12.9: `ask` → `Result<UnpairPreview, String>`: what unpairing `id` means
/// (`mesh_unpair_preview`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreviewUnpair(pub EngineId);

/// 183 §6.9.12: `ask` → `Result<secrets::RotationState, String>`: rotate the vault key
/// without an unpair, or retry a failed rotation (`vault_rotate`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RotateVault;

/// 184 §8.1: `ask` → [`DocsGateState`]: this engine's docs floor and `homes_on`. The document
/// actor asks on every `MeshEvent` and every 30 s.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct DocsGate;

/// 184 §4.9: `ask` → `Vec<EngineId>`: the ids of tombstoned (unpaired) rows in the peer table;
/// quarantined rows are not removed. The holder re-homes documents homed on them.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RemovedPeers;

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
    /// 182 D5: its last heartbeat said it holds the lease.
    is_hub: bool,
}

struct Live {
    lease: LeaseState,
    seen: HashMap<EngineId, Seen>,
    self_stats: EngineStats,
    /// `hlc` of our last `engine_stats` write (what heartbeats carry).
    stats_hlc: String,
    /// When each connected peer's open `availability` interval last hit its row.
    touches: store::Touches,
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

    /// Set `mesh.<key>: <value>` (a YAML scalar or flow value) in the config file.
    fn write_config(&self, key: &str, value: &str) -> Result<(), String> {
        runtime::write_config_value(&self.config_path, "mesh", key, value)
            .map_err(|e| format!("write {}: {e}", self.config_path.display()))
    }

    fn lease(&self) -> LeaseState {
        Self::lock(&self.live).lease.clone()
    }

    fn is_hub(&self) -> bool {
        Self::lock(&self.live).lease.holder.as_ref() == Some(&self.net.id)
    }

    async fn status(&self) -> Result<MeshStatus, String> {
        let peers = self.store.peers().await.map_err(|e| e.to_string())?;
        let mesh_name = self
            .store
            .meta_get("name")
            .await
            .map_err(|e| e.to_string())?;
        let cfg = self.cfg();
        Ok(MeshStatus {
            self_id: self.net.id.clone(),
            self_name: self.net.name(),
            mesh_name,
            peers: peers.iter().map(|p| self.net.peer_of(p)).collect(),
            lease: self.lease(),
            traffic: self.net.traffic(),
            replica: replica::stats().await,
            // 183 §9.2: the replicated `engine_stats` rows.
            stats: self.store.stats().await.map_err(|e| e.to_string())?,
            relay: self.net.relay_state(),
            messaging_off: cfg.messaging_off.into_iter().map(EngineId).collect(),
            nearby: cfg.nearby,
            quarantined: self.net.quarantined().await.map_err(|e| e.to_string())?,
            docs_homes_on: self.docs_gate().await.homes_on,
        })
    }

    /// 184 §8.1: the docs gate under the current lease ([`Net::docs_gate`]).
    async fn docs_gate(&self) -> DocsGateState {
        self.net
            .docs_gate(self.lease().holder.as_ref(), now_ms())
            .await
    }

    async fn emit(&self, ev: MeshEvent) {
        if let MeshEvent::Lease(l) = &ev {
            // Hub duties (67 §4.4): OAuth refresh, schedule ticks.
            let hub = l.holder.as_ref() == Some(&self.net.id);
            oauth::set_hub(hub);
            set_hub(hub);
            // 184 §8.1: the holder decides the docs gate.
            self.net.docs.changed.notify_one();
        }
        let _ = self.events.send(ev.clone());
        self.rt.publish(ev).await;
    }

    /// Probe and store this engine's stats. Unchanged stats are not
    /// rewritten: every write is a replicated batch, and heartbeats already
    /// carry liveness.
    async fn refresh_stats(&self) {
        let cfg = self.cfg();
        let (id, started, data_dir) = (
            self.net.id.clone(),
            self.started_ms,
            self.rt.data_dir.clone(),
        );
        let Ok(s) = tokio::task::spawn_blocking(move || {
            stats::probe(id, started, &data_dir, &cfg.workspaces, cfg.class)
        })
        .await
        else {
            return;
        };
        if Self::lock(&self.live).self_stats == s {
            return;
        }
        self.write_stats(s).await;
    }

    /// Write `s` as our `engine_stats` row (current `hub_pref`), remember it.
    async fn write_stats(&self, s: EngineStats) {
        match self
            .store
            .put_stats(&s, self.cfg().hub, self.first_seen_ms, now_ms())
            .await
        {
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
            (
                live.lease.holder.clone(),
                live.self_stats.clone(),
                live.seen.clone(),
            )
        };
        cands.push(Candidate {
            id: self.net.id.clone(),
            hub_pref: cfg.hub,
            availability_30d: self
                .store
                .availability_30d(&self.net.id, now)
                .await
                .unwrap_or(1.0),
            class: self_stats.class,
            uptime_since_ms: self.started_ms,
            first_seen_ms: self.first_seen_ms,
            connected_since_ms: self.started_ms,
        });
        // A connected peer is a candidate: the QUIC control connection is the liveness signal
        // (a dead peer drops out through `Disconnected`). Heartbeat age used to gate this, but
        // iroh-gossip delivery between two peers proved to pause for 30–60 s at a time (69 smoke),
        // which split the lease — two hubs, the mesh id flapping at the relay every few seconds.
        // A heartbeat still carries `is_hub`/`stats_hlc`; its absence is logged, not acted on.
        let mut claimers = Vec::new();
        for (id, since_ms) in self.net.connected() {
            if let Some(s) = seen.get(&id) {
                if now.saturating_sub(s.last_hb_ms) > lapse_ms {
                    tracing::debug!(
                        "mesh: no heartbeat from {id} for {} s while connected (last stats {}); gossip lagging",
                        (now - s.last_hb_ms) / 1000,
                        s.stats_hlc
                    );
                }
                if s.is_hub {
                    claimers.push(id.clone());
                }
            }
            match self.store.candidate(&id, since_ms, now).await {
                Ok(Some(c)) => cands.push(c),
                Ok(None) => tracing::debug!("mesh: no engine_stats row for {id} yet; waiting"),
                Err(e) => tracing::warn!("mesh: engine_stats for {id}: {e:#}"),
            }
        }
        let dual = incumbent.as_ref() == Some(&self.net.id) && !claimers.is_empty();
        // 182 S9: for its first `lapse` + 30 s a (re)started engine does not take
        // the lease from a connected holder that has been up longer — it follows
        // it; rank decides later, under the handover grace (no churn on restart).
        let elder = (now.saturating_sub(self.started_ms) < lapse_ms + RESTART_HOLD_MS)
            .then(|| {
                cands
                    .iter()
                    .filter(|c| claimers.contains(&c.id) && c.uptime_since_ms < self.started_ms)
                    .min_by_key(|c| (c.uptime_since_ms, c.id.0.clone()))
                    .map(|c| c.id.clone())
            })
            .flatten();
        let holder = match elder {
            Some(h) => Some(h),
            None => lease::resolve(
                &cands,
                incumbent.as_ref(),
                &self.net.id,
                &claimers,
                now,
                cfg.probation_secs * 1000,
                cfg.handover_secs * 1000,
            ),
        };
        if holder == incumbent {
            return;
        }
        let reason = match (&holder, &incumbent) {
            (None, _) => "none",
            // 182 D5: two holders met; the loser releases at once.
            _ if dual => "two holders",
            (Some(h), _)
                if cands
                    .iter()
                    .any(|c| &c.id == h && c.hub_pref == HubPref::Prefer) =>
            {
                "prefer"
            }
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
        tracing::info!(
            "mesh: lease → {:?} ({reason})",
            holder.as_ref().map(|h| &h.0[..8.min(h.0.len())])
        );
        if let Err(e) = self.store.lease_log(now, holder.as_ref(), reason).await {
            tracing::warn!("mesh: lease_log: {e:#}");
        }
        self.emit(MeshEvent::Lease(lease)).await;
    }

    /// Record a heartbeat. The stats themselves arrive by replication.
    fn on_heartbeat(&self, hb: GossipMsg) {
        let GossipMsg::Heartbeat {
            id,
            stats_hlc,
            is_hub,
        } = hb
        else {
            return;
        };
        if id == self.net.id {
            return;
        }
        Self::lock(&self.live).seen.insert(
            id,
            Seen {
                last_hb_ms: now_ms(),
                stats_hlc,
                is_hub,
            },
        );
    }

    /// Everything that keeps the lease honest: heartbeats out and in,
    /// availability intervals, re-election on every change, release on stop.
    async fn run_lease(self: Arc<Self>, cancel: CancellationToken) {
        let topic = TopicId::from_bytes(sha2::Sha256::digest(b"theuth/mesh/lease/1").into());
        let Ok(topic) = self
            .net
            .gossip
            .subscribe(topic, self.net.paired_ids())
            .await
        else {
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
                let _ = self
                    .store
                    .availability_open(&self.net.id, &p.id, false, now_ms())
                    .await;
            }
        }
        // An engine with stored peers does not elect itself at start: it says "no holder" (every
        // follower stops serving) until a peer's first heartbeat arrives or `lapse_secs` pass. Before (69 smoke) a
        // restarted engine elected itself over the candidate set {self} in 0.3 s, registered the
        // mesh id at the relay and bounced every device five times before the incumbent's view
        // reached it; a lone engine (no peers) still elects itself at once.
        let mut warming = !known.is_empty();
        let warm_deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.cfg().lapse_secs.max(1));
        if warming {
            let lease = LeaseState {
                holder: None,
                since_ms: now_ms(),
                reason: "start".into(),
            };
            Self::lock(&self.live).lease = lease.clone();
            tracing::info!(
                "mesh: lease → None (start: {} stored peer(s), waiting for one to connect)",
                known.len()
            );
            self.emit(MeshEvent::Lease(lease)).await;
        } else {
            self.recompute("start").await;
        }
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep_until(warm_deadline), if warming => {
                    warming = false;
                    self.recompute("start").await;
                }
                _ = tick.tick() => {
                    let now = now_ms();
                    let connected = self.net.connected();
                    // Idle-writer guard (69 audit): the row is written on open,
                    // close and every `availability_flush_secs`, not per heartbeat.
                    let due: Vec<EngineId> = {
                        let mut live = Self::lock(&self.live);
                        connected.iter().filter(|(id, _)| live.touches.due(id, now)).map(|(id, _)| id.clone()).collect()
                    };
                    for id in &due {
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
                    if !warming { self.recompute("lapse").await; }
                }
                ev = receiver.next() => match ev {
                    Some(Ok(GossipEvent::Received(msg))) => match serde_json::from_slice::<GossipMsg>(&msg.content) {
                        // P2-F1: the neighbour that delivered it and the engine it names must both
                        // be paired (a removed engine's frames, and its own id, count for nothing).
                        Ok(GossipMsg::Heartbeat { id, .. } | GossipMsg::LeaseRelease { id })
                            if !self.net.gossip_admits(&msg.delivered_from, &id) =>
                        {
                            tracing::debug!("mesh: gossip about {id} via {} dropped: not paired", msg.delivered_from.fmt_short());
                        }
                        Ok(hb @ GossipMsg::Heartbeat { .. }) => {
                            self.on_heartbeat(hb);
                            // 182 D5: a starting engine elects on the first heartbeat (not the
                            // connect): a holder's `is_hub` makes it the incumbent, so the
                            // handover grace keeps it instead of two holders meeting.
                            warming = false;
                            self.recompute("heartbeat").await;
                        }
                        Ok(GossipMsg::LeaseRelease { id }) => {
                            Self::lock(&self.live).seen.remove(&id);
                            self.recompute("release").await;
                        }
                        Err(e) => tracing::debug!("mesh: gossip payload: {e}"),
                    },
                    Some(Ok(other)) => tracing::debug!("mesh: gossip {other:?}"),
                    Some(Err(e)) => tracing::debug!("mesh: gossip: {e}"),
                    None => break,
                },
                ev = peer_events.recv() => match ev {
                    Ok(ev) => {
                        let (p, up) = match &ev {
                            PeerEvent::Connected(p) => (p, true),
                            PeerEvent::Disconnected(p) => (p, false),
                        };
                        let (id, now) = (EngineId(p.0.clone()), now_ms());
                        let _ = self.store.availability_open(&self.net.id, &id, up, now).await;
                        Self::lock(&self.live).touches.wrote(&id, now);
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
        // Close every open interval at its true end, not at the last flush.
        let now = now_ms();
        for (id, _) in self.net.connected() {
            let _ = self.store.availability_touch(&self.net.id, &id, now).await;
        }
        if self.is_hub()
            && let Ok(bytes) = serde_json::to_vec(&GossipMsg::LeaseRelease {
                id: self.net.id.clone(),
            })
        {
            let _ = sender.broadcast(bytes.into()).await;
            let lease = LeaseState {
                holder: None,
                since_ms: now_ms(),
                reason: "release".into(),
            };
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
        self.unpair(&m.0, false).await
    }
}

impl Message<Unpair> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Unpair, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.unpair(&m.id, m.reset_devices).await
    }
}

impl Mesh {
    /// [`RemovePeer`] / [`Unpair`]: tombstone, push, recompute, then the rotation.
    async fn unpair(&mut self, id: &EngineId, reset_devices: bool) -> Result<(), String> {
        self.ctx
            .net
            .remove_peer(id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ctx.net.push_peers().await;
        Ctx::lock(&self.ctx.live).seen.remove(id);
        self.ctx.recompute("remove").await;
        // 183 §6.9.3 (W-28): the rotation after the tombstone; its failure leaves the unpair.
        if let Err(e) = keys::after_remove(&self.ctx.net, id, reset_devices).await {
            tracing::warn!("mesh: vault-key rotation after unpairing {id}: {e}");
        }
        Ok(())
    }
}

impl Message<ConfirmPeer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ConfirmPeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !m.keep {
            // 183 R4, P2-F1: Remove is the user's unpair (tombstone, push, rotation); the list
            // entry clears inside the tombstone step, so the machine is never paired between.
            let listed = self
                .ctx
                .net
                .quarantined()
                .await
                .map_err(|e| format!("{e:#}"))?;
            if !listed.iter().any(|q| q.id == m.id) {
                return Err(format!("{} is not waiting for a confirmation", m.id));
            }
            return self.unpair(&m.id, false).await;
        }
        self.ctx
            .net
            .confirm_peer(&m.id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.ctx.recompute("confirm").await;
        Ok(())
    }
}

impl Message<SetHubPref> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetHubPref, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        runtime::write_config_value(
            &self.ctx.config_path,
            "mesh",
            "hub",
            store::hub_pref_str(m.0),
        )
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

    async fn handle(
        &mut self,
        _: PairingString,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let s = self.ctx.net.pairing_string().await;
        // 183 §6.8: `t mesh pair` also lets nearby machines find this one for the pair window.
        nearby::open_window(&self.ctx.net);
        Ok(s)
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

impl Message<Nearby> for Mesh {
    type Reply = Result<Vec<NearbyEngine>, String>;

    async fn handle(&mut self, _: Nearby, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(nearby::list(&self.ctx.net))
    }
}

impl Message<NearbyWatch> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: NearbyWatch, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        nearby::watch(&self.ctx.net, m.0).await
    }
}

impl Message<PairNearby> for Mesh {
    type Reply = DelegatedReply<Result<PairCeremony, String>>;

    async fn handle(&mut self, m: PairNearby, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let net = self.ctx.net.clone();
        // Network round trips: off the mailbox.
        tokio::spawn(async move { tx.send(nearby::pair(&net, &m.0).await) });
        reply
    }
}

impl Message<PairAnswer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PairAnswer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        nearby::answer(&self.ctx.net, m.ceremony, m.accept).await
    }
}

impl Message<SetMessaging> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetMessaging, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut off = self.ctx.cfg().messaging_off;
        off.retain(|id| *id != m.id.0);
        if !m.on {
            off.push(m.id.0);
        }
        self.ctx
            .write_config("messaging_off", &serde_json::json!(off).to_string())?;
        Ctx::lock(&self.ctx.cfg).messaging_off = off.clone();
        set_messaging_off(off);
        Ok(())
    }
}

impl Message<SetNearby> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetNearby, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx
            .write_config("nearby", if m.0 { "true" } else { "false" })?;
        Ctx::lock(&self.ctx.cfg).nearby = m.0;
        nearby::allow(&self.ctx.net, m.0)
    }
}

impl Message<Rename> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Rename, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let name = m.name.trim();
        if name.is_empty() {
            return Err("a machine needs a name".into());
        }
        if m.id == self.ctx.net.id {
            self.ctx
                .write_config("name", &serde_json::json!(name).to_string())?;
            Ctx::lock(&self.ctx.cfg).name = name.to_owned();
            set_self_name(name);
            // 187: MCP homes show this name.
            ::tools::set_machine(&m.id.0, name);
            self.ctx.net.rename(name).await
        } else {
            self.ctx.net.rename_peer(&m.id, name).await
        }
        .map_err(|e| format!("{e:#}"))
    }
}

impl Message<PreviewUnpair> for Mesh {
    type Reply = Result<UnpairPreview, String>;

    async fn handle(
        &mut self,
        m: PreviewUnpair,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        keys::unpair_preview(&self.ctx.net, &m.0)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<RotateVault> for Mesh {
    type Reply = Result<secrets::RotationState, String>;

    async fn handle(&mut self, _: RotateVault, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        keys::rotate(&self.ctx.net).await
    }
}

impl Message<DocsGate> for Mesh {
    type Reply = DocsGateState;

    async fn handle(&mut self, _: DocsGate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.docs_gate().await
    }
}

impl Message<RemovedPeers> for Mesh {
    type Reply = Vec<EngineId>;

    async fn handle(&mut self, _: RemovedPeers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // A failed read re-homes nothing this pass; the next pass retries.
        self.ctx.store.removed_peers().await.unwrap_or_else(|e| {
            tracing::warn!("mesh: removed peers: {e:#}");
            Vec::new()
        })
    }
}

/// The plugin: reads `mesh`, opens the store, binds the endpoint, spawns
/// [`Mesh`] as [`MESH_ACTOR`], hands the transport to the replica plugin,
/// registers the `mesh_peers` tool once the registry is up.
pub struct MeshPlugin;

/// Declare wire contracts before starting producers, including disabled services.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<MeshEvent>().await
}

#[async_trait::async_trait]
impl runtime::Plugin for MeshPlugin {
    fn name(&self) -> &'static str {
        "mesh"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        // SIGTERM: release the lease gracefully (67 §4.6), then stop — armed
        // before anything below can bail out, so a mesh without its store
        // still stops cleanly. macOS `willSleep` has no platform hook yet.
        let stop = rt.shutdown.clone();
        tokio::spawn(async move {
            if platform::process::stop_signal().await.is_ok() {
                tracing::warn!(
                    "exiting: SIGTERM (mesh plugin releases the lease and cancels shutdown)"
                );
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
        set_self_name(&name);
        set_self_id(&identity.id().0);
        // 187: MCP servers start only on their home engine.
        ::tools::set_machine(&identity.id().0, &name);
        ::tools::set_paired_check(|| {
            NET.get()
                .map(|n| n.paired_ids().iter().map(ToString::to_string).collect())
        });
        let net = match Net::bind(
            net::BindOpts {
                secret: identity.secret,
                relay: cfg.relay.clone(),
                discovery: cfg.discovery,
                bind_addr: cfg.bind,
                storm_frames_per_sec: cfg.storm_frames_per_sec,
                first_seen_ms: identity.first_seen_ms,
                vault: None,
                mdns: None,
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
        tracing::info!(
            "mesh: engine {} on relay {} (discovery {})",
            net.id,
            cfg.relay,
            cfg.discovery
        );
        let _ = NET.set(net.clone());
        // 187: the paired set is known from here on: place servers homed on an engine that left.
        ::tools::peers_changed();
        // 183 §6.2, §6.8: the deny list and the Nearby switch, from config.
        set_messaging_off(cfg.messaging_off.clone());
        nearby::configure(&net, cfg.nearby, cfg.pair_window_secs);
        // 183 §5.6: the machine facts carry this engine's mesh name.
        platform::process::set_machine_name(&net.name());
        // 79 §6.6: secrets stamps its replicated rows with the engine id; it starts
        // before us and cannot ask the mesh (mesh depends on secrets), so we tell it.
        tokio::spawn(tell_secrets_engine(rt.clone(), net.id.clone()));
        // 183 §6.9: the vault the key frames and the rotation move keys in, and the key tick
        // (member rule, missing-key requests, the rotation's peer list) until shutdown.
        // P2-F1: the tick (and its paired-set refresh: unpair enforcement) runs from the start,
        // vault or not; the vault joins when it opens, `keys::start` before any tick uses it.
        let (n, rt2) = (net.clone(), rt.clone());
        tokio::spawn(async move {
            let vault = secrets::handle(&rt2);
            tokio::pin!(vault);
            let mut waiting = true;
            let mut every = tokio::time::interval(keys::TICK);
            loop {
                tokio::select! {
                    () = rt2.shutdown.cancelled() => return,
                    v = &mut vault, if waiting => {
                        waiting = false;
                        if let Some(v) = v {
                            n.set_vault(v);
                            keys::start(&n).await;
                        }
                    }
                    _ = every.tick() => keys::tick(&n, now_ms()).await,
                }
            }
        });

        // 69 decision 8: the mesh's name defaults to the first engine's own.
        // Written after the attach (below) so the trigger captures it, and only
        // by an engine that starts alone: a joiner (peers at start) takes the
        // row replication brings. Two defaults meeting at pairing time are
        // ranked by identity age in `Store::meta_default` (79 smoke bug 5).
        let alone = store.peers().await.map(|p| p.is_empty()).unwrap_or(false);

        let (events, _) = broadcast::channel(256);
        let started_ms = now_ms();
        let self_stats = {
            let (id, data_dir, workspaces, class) = (
                net.id.clone(),
                rt.data_dir.clone(),
                cfg.workspaces.clone(),
                cfg.class,
            );
            tokio::task::spawn_blocking(move || {
                stats::probe(id, started_ms, &data_dir, &workspaces, class)
            })
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
                accelerator: None,
                os: platform::process::machine().os.into(),
                os_name: platform::process::machine().os_name.clone(),
                arch: platform::process::machine().arch.into(),
                host: platform::process::machine().host.clone(),
            })
        };
        // Written after the attach (below): a row that lands before the replica
        // is up never fires a trigger and only its later column writes ship —
        // a partial row every peer refuses (69 smoke: no peer could elect us).
        let stats_hlc = String::new();
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
                self_stats: self_stats.clone(),
                stats_hlc,
                touches: store::Touches::new(cfg.availability_flush_secs * 1000),
            }),
        });

        let actor = Mesh::spawn(Mesh { ctx: ctx.clone() });
        actor.register(MESH_ACTOR)?;

        // 183 §6.8: Nearby list and pairing ceremony changes onto the bus.
        let (c, mut nearby_events) = (ctx.clone(), net.nearby_events());
        tokio::spawn(async move {
            loop {
                match nearby_events.recv().await {
                    Ok(ev) => c.emit(ev).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        // The replica plugin starts after this one: attach when it is up (bounded — a slimmer
        // plugin set may never start it), then write the start-up rows.
        let (c, name, stats) = (ctx.clone(), net.name(), self_stats);
        tokio::spawn(async move {
            let replica = tokio::time::timeout(
                ATTACH_WAIT,
                c.rt.lookup_when::<replica::Replica>(replica::REPLICA_ACTOR),
            )
            .await;
            if matches!(replica, Ok(Some(_))) {
                match replica::attach(
                    c.store.pool(),
                    &["peers", "engine_stats", "availability", "mesh_meta"],
                )
                .await
                {
                    Ok(_) => {
                        tracing::info!("mesh: peers/engine_stats/availability/mesh_meta replicated")
                    }
                    Err(e) => tracing::info!("mesh: replication unavailable ({e}); local only"),
                }
            }
            if alone && let Err(e) = c.store.meta_default("name", &name, c.first_seen_ms).await {
                tracing::warn!("mesh: mesh_meta name: {e:#}");
            }
            c.write_stats(stats).await;
        });

        // The transport, to the replica plugin whenever it comes up.
        let (rt2, transport) = (rt.clone(), net.clone());
        tokio::spawn(async move {
            if let Some(replica) = rt2
                .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
                .await
            {
                let t: Arc<dyn replica::Transport> = transport;
                let _ = replica.ask(replica::RegisterTransport(t)).await;
            }
        });

        // Background loops, all ending on shutdown; the lease loop releases first.
        let cancel = rt.shutdown.child_token();
        tokio::spawn(net.clone().run_connector(cancel.clone()));
        let c = ctx.clone();
        tokio::spawn(async move {
            let mut every =
                tokio::time::interval(Duration::from_secs(cfg.capabilities_secs.max(30)));
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
        // 184 §8.1: the docs gate pass on every frame, connect, disconnect and lease change, and
        // every tick (heard_ms, staleness); a flip is sent to every connected peer, and published
        // on the engine bus (a Rust-only `DocsGateState`) so the document actor reroutes at once.
        // A lone engine has nobody to tell (its `DocsGate` still answers, computed per ask).
        let (c, stop) = (ctx.clone(), rt.shutdown.clone());
        tokio::spawn(async move {
            let mut every = tokio::time::interval(keys::TICK);
            let mut last = None;
            loop {
                tokio::select! {
                    () = stop.cancelled() => break,
                    _ = every.tick() => {}
                    () = c.net.docs.changed.notified() => {}
                }
                if !c.net.paired_ids().is_empty() {
                    let s = c.docs_gate().await;
                    if last.replace(s.homes_on) != Some(s.homes_on) {
                        c.rt.publish(s).await;
                    }
                }
            }
        });
        // 182 M2a: with paired peers, the first sync gates task work here (no dispatch,
        // sweep, owner write or desk-turn resume on stale rows) until every connected
        // peer's first replica exchange completed, or `lapse_secs` passed with none.
        if !net.paired_ids().is_empty() {
            set_synced(false);
            let (n, lapse, stop) = (
                net.clone(),
                ctx.cfg().lapse_secs.max(1),
                rt.shutdown.clone(),
            );
            tokio::spawn(async move {
                let start = tokio::time::Instant::now();
                let mut every = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = every.tick() => {}
                    }
                    let connected = n.connected();
                    let waited = start.elapsed().as_secs();
                    let why = if connected.is_empty() {
                        (waited >= lapse).then_some("no peer connected")
                    } else if connected
                        .iter()
                        .all(|(id, _)| replica::caught_up_with(&id.0))
                    {
                        Some("caught up with every connected peer")
                    } else {
                        // ponytail: a peer that never completes its first exchange (an
                        // incompatible build) stops gating after 5 × lapse_secs.
                        (waited >= 5 * lapse)
                            .then_some("timed out waiting for a peer's first exchange")
                    };
                    if let Some(why) = why {
                        tracing::info!(
                            "182: first sync done after {waited} s ({why}); task work starts"
                        );
                        set_synced(true);
                        return;
                    }
                }
            });
        }
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

/// Hand this engine's id to the secrets actor ([`secrets::SetEngine`]) once it is registered.
async fn tell_secrets_engine(rt: Runtime, id: EngineId) {
    if let Some(s) = rt
        .lookup_when::<secrets::SecretsActor>(secrets::SECRETS_ACTOR)
        .await
    {
        let _ = s.tell(secrets::SetEngine(id.0)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XTICK: before the plugin starts, "peered" is the stored paired set —
    /// this engine's own row, a tombstone and a quarantined row are no peer.
    #[tokio::test]
    async fn stored_paired_leaves_out_self_tombstones_and_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let me = identity::Identity::load_or_create(&dir.path().join(DIR))
            .unwrap()
            .id();
        assert_eq!(stored_self_id(dir.path()).as_deref(), Some(me.0.as_str()));
        let store = store::Store::open(&dir.path().join(DB), me.clone())
            .await
            .unwrap();
        let pool = store.pool();
        for (id, removed) in [(me.0.as_str(), None), ("gone", Some(2)), ("held", None)] {
            sqlx::query(
                "INSERT OR REPLACE INTO peers (id, name, added_ms, removed_ms) VALUES (?, ?, 1, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(removed)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO peer_origin (id, by_engine, at_ms, quarantined) VALUES ('held', 'x', 1, 1)")
            .execute(pool)
            .await
            .unwrap();
        assert!(has_stored_peers(dir.path()).await, "raw rows");
        assert!(!has_stored_paired(dir.path()).await);
        sqlx::query("INSERT INTO peers (id, name, added_ms) VALUES ('live', 'live', 1)")
            .execute(pool)
            .await
            .unwrap();
        assert!(has_stored_paired(dir.path()).await);
    }

    /// 79 §6.6 (was roster→secrets, now mesh→secrets): once the mesh has its id,
    /// the secrets actor stamps the rows it writes with it, not the hostname.
    #[tokio::test]
    async fn secrets_learns_the_engine_id_from_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(Default::default(), dir.path().to_path_buf());
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let a = secrets::SecretsActor::spawn(secrets::SecretsActor::new(
            vault,
            rt.clone(),
            Duration::from_secs(10),
        ));
        a.register(secrets::SECRETS_ACTOR).unwrap();
        tell_secrets_engine(rt, EngineId("engine-f3".into())).await;

        let asker = a.clone();
        tokio::spawn(async move {
            asker
                .ask(secrets::Request {
                    session: runtime::SessionId::new(),
                    title: None,
                    fields: vec![secrets::SecretRequestField {
                        name: "TOKEN".into(),
                        label: "a token".into(),
                        masked: true,
                        current: None,
                    }],
                    id: None,
                })
                .await
        });
        let row = loop {
            let rows = a
                .ask(secrets::ListRequests {
                    unanswered_only: true,
                })
                .await
                .unwrap();
            if let Some(r) = rows.into_iter().next() {
                break r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(row.engine, "engine-f3");
    }

    fn seen(id: &str, schema: u32, first_ms: u64, heard_ms: Option<u64>) -> store::DocsSeen {
        store::DocsSeen {
            id: EngineId(id.into()),
            docs_schema: schema,
            homes_on: false,
            first_ms,
            heard_ms,
        }
    }

    /// 184 §8.1, Sky Q5: a peer counts unless last heard (never heard: first listed) 7 days ago or
    /// more; a counted peer below `DOCS_SCHEMA`, or never heard, keeps the floor down.
    #[test]
    fn the_floor_counts_every_peer_heard_within_seven_days() {
        let now = 100 * 86_400_000;
        let day = 86_400_000;
        let ids = |v: &[&str]| v.iter().map(|s| EngineId((*s).into())).collect::<Vec<_>>();
        assert!(docs_floor(&[], &[], now), "a lone engine: on");
        let new = seen("new", DOCS_SCHEMA, now - day, Some(now - 1));
        assert!(docs_floor(&ids(&["new"]), std::slice::from_ref(&new), now));
        // Never heard, listed an hour ago: counted as 0, off until heard.
        let quiet = seen("quiet", 0, now - 3_600_000, None);
        assert!(!docs_floor(
            &ids(&["new", "quiet"]),
            &[new.clone(), quiet],
            now
        ));
        // An old build last heard 6 days ago counts; at exactly 7 days it stops counting.
        let old = |heard| seen("old", 0, now - 30 * day, Some(heard));
        assert!(!docs_floor(
            &ids(&["new", "old"]),
            &[new.clone(), old(now - 6 * day)],
            now
        ));
        assert!(docs_floor(
            &ids(&["new", "old"]),
            &[new.clone(), old(now - DOCS_STALE_MS)],
            now
        ));
        // Never heard and listed 7 days ago: stale too.
        let gone = seen("gone", 0, now - 8 * day, None);
        assert!(docs_floor(
            &ids(&["new", "gone"]),
            &[new.clone(), gone],
            now
        ));
        // A paired engine with no row yet fails closed.
        assert!(!docs_floor(&ids(&["new", "unlisted"]), &[new], now));
        // A schema-1 engine (the 69 docs) is below the floor.
        assert!(!docs_floor(
            &ids(&["one"]),
            &[seen("one", 1, now, Some(now))],
            now
        ));
    }

    /// 184 §8.1, fail-closed: the holder's own floor decides; a follower needs the holder's frame
    /// saying on and its own floor; no holder or no frame is off.
    #[test]
    fn homes_are_on_only_when_the_holder_and_the_floor_agree() {
        assert!(docs_homes_on(true, true, None), "the holder: its floor");
        assert!(!docs_homes_on(false, true, None));
        assert!(docs_homes_on(true, false, Some(true)));
        assert!(
            !docs_homes_on(true, false, Some(false)),
            "the holder says off"
        );
        assert!(
            !docs_homes_on(true, false, None),
            "no frame from the holder yet"
        );
        assert!(!docs_homes_on(false, false, Some(true)), "own floor fails");
    }
}
