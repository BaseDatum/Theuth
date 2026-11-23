//! The edge's live state: config, store, the route table (§5.4) and the attached tunnels (§5.3).

use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use dashmap::DashMap;
use governor::{DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter};
use iroh::endpoint::Connection;
use share_proto::events::{KillEvent, RoutesEvent};
use share_proto::{Admission, Event, KillReason, PublicKey, RouteEntry};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

use crate::config::{Config, Limits, Secrets};
use crate::store::Store;

/// The edge's version, sent as `theuth-share-edge/<ver>`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Control requests one tunnel may open per second; beyond the burst, `429 rate_limited` (FIX4-EDGE #7).
pub const CONTROL_PER_SEC: u32 = 20;
/// Burst of [`CONTROL_PER_SEC`].
pub const CONTROL_BURST: u32 = 100;
/// Most engines one mesh's revoke list holds (`POST /v1/mesh/revoke` past it is `429 quota`).
pub const REVOKED_PER_MESH: u64 = 256;
/// `conn_log` rows queued for the writer; a full queue drops the row and counts it (FIX4-EDGE #9).
pub const CONN_LOG_QUEUE: usize = 4096;
/// Rows the `conn_log` writer inserts per transaction at most.
const CONN_LOG_BATCH: usize = 256;

/// The `conn_log` writer's queue: visitors hand their row over without waiting on SQLite; one task inserts in
/// batches ([`ConnLog::run`]).
#[derive(Debug)]
pub struct ConnLog {
    tx: tokio::sync::mpsc::Sender<crate::store::ConnLogRow>,
    rx: Mutex<Option<tokio::sync::mpsc::Receiver<crate::store::ConnLogRow>>>,
    /// Rows dropped because the queue was full (`share_edge_conn_log_dropped_total`).
    pub dropped: AtomicU64,
}

impl ConnLog {
    fn new(cap: usize) -> ConnLog {
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        ConnLog {
            tx,
            rx: Mutex::new(Some(rx)),
            dropped: AtomicU64::new(0),
        }
    }

    /// Queue one row; never waits. A full queue (or no writer) drops it and counts the drop.
    pub fn push(&self, row: crate::store::ConnLogRow) {
        if self.tx.try_send(row).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The writer: batches of up to 256 rows, one transaction each. Runs once per edge (takes the receiver).
    pub async fn run(edge: Arc<Edge>) {
        let Some(mut rx) = edge.conn_log.rx.lock().expect("conn_log lock").take() else {
            return;
        };
        let mut batch = Vec::with_capacity(CONN_LOG_BATCH);
        while rx.recv_many(&mut batch, CONN_LOG_BATCH).await > 0 {
            if let Err(e) = edge.store.conn_log_batch(&batch).await {
                tracing::warn!(rows = batch.len(), "conn_log: {e:#}");
            }
            batch.clear();
        }
    }
}

/// Unix ms now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The per-IP key: the IPv4 address or the IPv6 /64 (§4.2, §9.4). A v4-mapped v6 address is its v4 address.
pub fn net_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// The client network of a tunnel: its selected path's IP key, or `relay:<url>` for a relayed path (§9.4).
pub fn conn_net(conn: &Connection) -> String {
    let paths = conn.paths();
    let path = paths
        .iter()
        .find(|p| p.is_selected())
        .or_else(|| paths.iter().next());
    match path.map(|p| p.remote_addr().clone()) {
        Some(iroh::TransportAddr::Ip(s)) => net_key(s.ip()).to_string(),
        Some(iroh::TransportAddr::Relay(u)) => format!("relay:{u}"),
        _ => "unknown".into(),
    }
}

/// The delegation a tunnel's current `hello` carried.
#[derive(Debug, Clone)]
pub struct Session {
    /// The mesh the engine acts for.
    pub mesh: PublicKey,
    /// Delegation expiry.
    pub expires_ms: u64,
    /// The `hello` carried the current terms revision.
    pub terms_ok: bool,
    /// The terms revision the `hello` carried; checked against the live `terms_rev` at every name and CSR
    /// request, so a reloaded revision needs a new `hello` (§7.6).
    pub terms_rev: Option<String>,
    /// Edge time of the `hello` that set this session; routes from a session older than the mesh's last Stop
    /// all are refused (§7.4).
    pub hello_ms: u64,
}

/// One engine's connection on `theuth/share/1`, before and after `hello`.
#[derive(Debug)]
pub struct Tunnel {
    /// The QUIC peer, the engine key.
    pub engine: PublicKey,
    /// The connection the engine dialed.
    pub conn: Connection,
    /// Client network.
    pub net: String,
    /// Set by a valid `hello`; `None` before it.
    pub session: Mutex<Option<Session>>,
    /// Pre-`hello` slot, dropped at the first valid `hello`.
    pub pre_hello: Mutex<Option<OwnedSemaphorePermit>>,
    /// Open visitor streams.
    pub streams: AtomicU32,
    /// Visitor connections ever routed.
    pub conns: AtomicU64,
    /// Visitor bytes both ways, counted live by the splice.
    pub bytes: AtomicU64,
    /// Of `bytes`, already folded into the mesh and network totals (§9.6).
    pub folded: AtomicU64,
    /// The mesh entered by invite (set at `hello`): exempt from the egress throttle.
    pub invited: AtomicBool,
    /// Per-tunnel byte shaping (`tunnel_mbit`, or `throttle_mbit` while throttled); unshaped before `hello`.
    pub shaper: crate::meter::Shaper,
    /// New visitor connections per second (`new_conns_per_tunnel_sec`, burst `new_conns_burst`).
    pub conn_rate: DefaultDirectRateLimiter,
    /// Control requests (bi-streams) per second ([`CONTROL_PER_SEC`], burst [`CONTROL_BURST`]).
    pub control_rate: DefaultDirectRateLimiter,
    /// Events for this connection's `GET /v1/events`.
    pub events: broadcast::Sender<Event>,
}

impl Tunnel {
    /// A connection before `hello`.
    pub fn new(conn: Connection, pre_hello: Option<OwnedSemaphorePermit>, limits: &Limits) -> Arc<Tunnel> {
        let (events, _) = broadcast::channel(64);
        let per_sec = NonZeroU32::new(limits.new_conns_per_tunnel_sec.max(1)).expect("nonzero");
        let burst = NonZeroU32::new(limits.new_conns_burst.max(1)).expect("nonzero");
        Arc::new(Tunnel {
            engine: conn.remote_id(),
            net: conn_net(&conn),
            conn,
            session: Mutex::new(None),
            pre_hello: Mutex::new(pre_hello),
            streams: AtomicU32::new(0),
            conns: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            folded: AtomicU64::new(0),
            invited: AtomicBool::new(false),
            shaper: crate::meter::Shaper::new(0),
            conn_rate: RateLimiter::direct(Quota::per_second(per_sec).allow_burst(burst)),
            control_rate: RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(CONTROL_PER_SEC).expect("nonzero"))
                    .allow_burst(NonZeroU32::new(CONTROL_BURST).expect("nonzero")),
            ),
            events,
        })
    }

    /// The live session at `now_ms`.
    pub fn session(&self, now_ms: u64) -> Option<Session> {
        self.session
            .lock()
            .expect("session lock")
            .clone()
            .filter(|s| now_ms < s.expires_ms)
    }

    /// The mesh of the last `hello`, expired or not.
    pub fn mesh(&self) -> Option<PublicKey> {
        self.session
            .lock()
            .expect("session lock")
            .as_ref()
            .map(|s| s.mesh)
    }

    /// Send an event to this connection's event stream (dropped when nobody listens).
    pub fn send(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// `kill {host?, reason}`; without a host the connection closes after a moment so the event can leave.
    pub fn kill(self: &Arc<Self>, host: Option<String>, reason: KillReason) {
        let whole = host.is_none();
        self.send(Event::Kill(KillEvent { host, reason }));
        if whole {
            let conn = self.conn.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                conn.close(1u32.into(), reason_bytes(reason));
            });
        }
    }
}

fn reason_bytes(r: KillReason) -> &'static [u8] {
    match r {
        KillReason::EdgeBan => b"edge_ban",
        KillReason::Budget => b"budget",
        KillReason::StopAll => b"stop_all",
        KillReason::Revoked => b"revoked",
        KillReason::MeshRekeyed => b"mesh_rekeyed",
        KillReason::DelegationExpired => b"delegation_expired",
        _ => b"kill",
    }
}

/// One live route: `host → tunnel` (§5.4).
#[derive(Debug, Clone)]
pub struct Route {
    /// The name the host is under.
    pub name: String,
    /// The mesh holding the name.
    pub mesh: PublicKey,
    /// The tunnel that registered the host.
    pub tunnel: Arc<Tunnel>,
    /// The host's counters for the `stats` event (§9.6).
    pub stats: Arc<crate::meter::HostStats>,
}

/// Shared edge state.
#[derive(Debug)]
pub struct Edge {
    /// Config as loaded.
    pub cfg: Config,
    /// Secrets' sources.
    pub secrets: Secrets,
    /// `share-edge.sqlite`.
    pub store: Store,
    /// The edge's endpoint id.
    pub id: PublicKey,
    /// Live admission mode.
    pub admission: RwLock<Admission>,
    /// `admin pause`: every tunnel closed, `hello` refused.
    pub paused: AtomicBool,
    /// The reloadable part of the config (§8.1, `admin reload`).
    live: RwLock<Arc<Live>>,
    /// Metric counters and the brake's windows (§9.5).
    pub ops: crate::ops::Ops,
    /// The standby endpoint key's public half (§5.5), when one is stored.
    pub standby: OnceLock<PublicKey>,
    /// The signed retire notice `welcome` carries while this edge runs on its former standby key (§5.5).
    pub retire: OnceLock<share_proto::Signed>,
    /// SIGTERM seen: `hello` is refused while the tunnels drain.
    pub draining: AtomicBool,
    /// The config file `admin reload` re-reads; unset for an edge built in-process (tests).
    pub config_path: OnceLock<std::path::PathBuf>,
    /// `host → route`, exact match.
    pub routes: DashMap<String, Route>,
    /// Attached tunnels by engine key (one per engine; a new one replaces the old after `hello`).
    pub tunnels: DashMap<PublicKey, Arc<Tunnel>>,
    /// Pre-`hello` connections.
    pub pre_hello: Arc<Semaphore>,
    /// Visitor connections.
    pub visitors: Arc<Semaphore>,
    /// Visitor connections per IP key.
    pub per_ip: DashMap<IpAddr, u32>,
    /// New visitor connections per IP key.
    pub ip_rate: DefaultKeyedRateLimiter<IpAddr>,
    /// `GET /v1/info` per client network ([`Tunnel::net`]).
    pub info_rate: DefaultKeyedRateLimiter<String>,
    /// The `conn_log` writer's queue.
    pub conn_log: ConnLog,
    /// Certificates (EDGE2); unset in an edge built without backends.
    pub certs: OnceLock<crate::certs::Certs>,
    /// Per-name Kubernetes objects (EDGE-K8S); unset unless `k8s.enabled`.
    pub k8s: OnceLock<Arc<crate::k8s::K8s>>,
    /// Origin-ASN table (`limits.asn_db`); empty = ASN tier off.
    pub asn: crate::limits::AsnDb,
    /// Byte totals, budgets and the egress throttle (§9.6).
    pub meter: crate::meter::Meter,
    /// `names_max` reached and nothing evictable: `edge_full`, `full: true` in `/v1/info` and `welcome`.
    full: AtomicBool,
    next_conn: AtomicU64,
}

impl Edge {
    /// Build the state.
    pub fn new(cfg: Config, secrets: Secrets, store: Store, id: PublicKey) -> anyhow::Result<Edge> {
        let live = Live::from_config(&cfg)?;
        let per_sec = NonZeroU32::new(cfg.limits.new_conns_per_ip_sec.max(1)).expect("nonzero");
        let asn = crate::limits::AsnDb::load(cfg.limits.asn_db.as_deref())?;
        Ok(Edge {
            admission: RwLock::new(cfg.admission),
            paused: AtomicBool::new(false),
            pre_hello: Arc::new(Semaphore::new(cfg.limits.pre_hello_conns as usize)),
            visitors: Arc::new(Semaphore::new(cfg.limits.global_conns as usize)),
            per_ip: DashMap::new(),
            ip_rate: RateLimiter::keyed(Quota::per_second(per_sec)),
            info_rate: RateLimiter::keyed(Quota::per_minute(
                NonZeroU32::new(cfg.limits.info_per_ip_min.max(1)).expect("nonzero"),
            )),
            routes: DashMap::new(),
            tunnels: DashMap::new(),
            conn_log: ConnLog::new(CONN_LOG_QUEUE),
            next_conn: AtomicU64::new(1),
            certs: OnceLock::new(),
            k8s: OnceLock::new(),
            asn,
            meter: crate::meter::Meter::default(),
            full: AtomicBool::new(false),
            live: RwLock::new(Arc::new(live)),
            ops: crate::ops::Ops::default(),
            standby: OnceLock::new(),
            retire: OnceLock::new(),
            draining: AtomicBool::new(false),
            config_path: OnceLock::new(),
            cfg,
            secrets,
            store,
            id,
        })
    }

    /// The reloadable config now in force.
    pub fn live(&self) -> Arc<Live> {
        self.live.read().expect("live lock").clone()
    }

    /// Swap in a reloaded config.
    pub fn set_live(&self, live: Live) {
        *self.live.write().expect("live lock") = Arc::new(live);
    }

    /// Live admission mode.
    pub fn admission(&self) -> Admission {
        *self.admission.read().expect("admission lock")
    }

    /// The edge is full (§9.4): `names_max` reached and no never-published name to evict.
    pub fn full(&self) -> bool {
        self.full.load(Ordering::Relaxed)
    }

    /// Set by the slot queue.
    pub fn set_full(&self, on: bool) {
        if self.full.swap(on, Ordering::Relaxed) != on {
            tracing::warn!(full = on, "edge full state");
        }
    }

    /// A fresh visitor connection id (PROXY TLV `0xE0`).
    pub fn conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    /// Attached tunnels of `mesh`.
    pub fn mesh_tunnels(&self, mesh: &PublicKey) -> Vec<Arc<Tunnel>> {
        self.tunnels
            .iter()
            .filter(|t| t.mesh().as_ref() == Some(mesh))
            .map(|t| t.value().clone())
            .collect()
    }

    /// The routes of `mesh` as wire entries.
    pub fn mesh_routes(&self, mesh: &PublicKey) -> Vec<RouteEntry> {
        let mut v: Vec<RouteEntry> = self
            .routes
            .iter()
            .filter(|r| r.mesh == *mesh)
            .map(|r| RouteEntry {
                host: r.key().clone(),
                engine: r.tunnel.engine,
            })
            .collect();
        v.sort_by(|a, b| a.host.cmp(&b.host));
        v
    }

    /// Announce a route change to every tunnel of `mesh`.
    pub fn announce(&self, mesh: &PublicKey, host: &str, engine: Option<PublicKey>) {
        for t in self.mesh_tunnels(mesh) {
            t.send(Event::Routes(RoutesEvent {
                host: host.to_string(),
                engine,
            }));
        }
    }

    /// Remove every route matching `pred`, announcing each; returns the removed `(host, route)`.
    pub fn drop_routes(&self, pred: impl Fn(&str, &Route) -> bool) -> Vec<(String, Route)> {
        let hosts: Vec<String> = self
            .routes
            .iter()
            .filter(|r| pred(r.key(), r.value()))
            .map(|r| r.key().clone())
            .collect();
        let mut out = Vec::new();
        for h in hosts {
            if let Some((h, r)) = self.routes.remove_if(&h, |k, v| pred(k, v)) {
                self.announce(&r.mesh, &h, None);
                out.push((h, r));
            }
        }
        out
    }

    /// Forget idle keys of the keyed rate limiters and give the memory back (FIX4-EDGE #8; every minute from
    /// [`sweep_limiters`]).
    pub fn sweep_limiters(&self) {
        self.ip_rate.retain_recent();
        self.ip_rate.shrink_to_fit();
        self.info_rate.retain_recent();
        self.info_rate.shrink_to_fit();
    }

    /// A connection ended: drop its routes and its tunnel entry (if still the current one).
    pub fn detach(&self, tunnel: &Arc<Tunnel>) {
        // Bytes since the last fold still count against the mesh and network (§9.6).
        self.meter.fold_tunnel(self, tunnel);
        self.drop_routes(|_, r| Arc::ptr_eq(&r.tunnel, tunnel));
        self.tunnels
            .remove_if(&tunnel.engine, |_, t| Arc::ptr_eq(t, tunnel));
    }
}

/// The config keys `admin reload` applies live (§8.1): `limits` (except the ones sized at start), the brand file,
/// `reserved_words`, terms and abuse URLs, `acme.queue_max`/`invite_share`, `brake`.
#[derive(Debug, Clone)]
pub struct Live {
    /// `limits`.
    pub limits: Limits,
    /// Brand blocklist lines (§9.4).
    pub brands: Vec<String>,
    /// Operator reserved words.
    pub reserved_words: Vec<String>,
    /// Terms page.
    pub terms_url: String,
    /// Current terms revision.
    pub terms_rev: String,
    /// Abuse page.
    pub abuse_url: String,
    /// `acme.queue_max`.
    pub queue_max: u32,
    /// `acme.invite_share`.
    pub invite_share: f64,
    /// `brake`.
    pub brake: crate::config::BrakeConfig,
    /// `admission` as the file says (a change in the file is applied live by `reload`).
    pub admission: Admission,
}

impl Live {
    /// The reloadable keys of `cfg`, reading the brand file.
    pub fn from_config(cfg: &Config) -> anyhow::Result<Live> {
        let brands = match &cfg.brand_blocklist_file {
            Some(p) if p.exists() => std::fs::read_to_string(p)?
                .lines()
                .map(|l| l.trim().to_ascii_lowercase())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect(),
            _ => Vec::new(),
        };
        Ok(Live {
            limits: cfg.limits.clone(),
            brands,
            reserved_words: cfg.reserved_words.clone(),
            terms_url: cfg.terms_url.clone(),
            terms_rev: cfg.terms_rev.clone(),
            abuse_url: cfg.abuse_url.clone(),
            queue_max: cfg.acme.queue_max,
            invite_share: cfg.acme.invite_share,
            brake: cfg.brake.clone(),
            admission: cfg.admission,
        })
    }
}

/// [`Edge::sweep_limiters`] every minute.
pub async fn sweep_limiters(edge: Arc<Edge>) {
    let mut every = tokio::time::interval(Duration::from_secs(60));
    loop {
        every.tick().await;
        edge.sweep_limiters();
    }
}
