//! The edge's live state: config, store, the route table (§5.4) and the attached tunnels (§5.3).

use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use dashmap::DashMap;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use iroh::endpoint::Connection;
use share_proto::events::{KillEvent, RoutesEvent};
use share_proto::{Admission, Event, KillReason, PublicKey, RouteEntry};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

use crate::config::{Config, Secrets};
use crate::store::Store;

/// The edge's version, sent as `theuth-share-edge/<ver>`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Visitor bytes both ways.
    pub bytes: AtomicU64,
    /// Events for this connection's `GET /v1/events`.
    pub events: broadcast::Sender<Event>,
}

impl Tunnel {
    /// A connection before `hello`.
    pub fn new(conn: Connection, pre_hello: Option<OwnedSemaphorePermit>) -> Arc<Tunnel> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Tunnel {
            engine: conn.remote_id(),
            net: conn_net(&conn),
            conn,
            session: Mutex::new(None),
            pre_hello: Mutex::new(pre_hello),
            streams: AtomicU32::new(0),
            conns: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
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
    /// Brand blocklist lines (§9.4).
    pub brands: Vec<String>,
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
    /// Certificates (EDGE2); unset in an edge built without backends.
    pub certs: OnceLock<crate::certs::Certs>,
    /// Per-name Kubernetes objects (EDGE-K8S); unset unless `k8s.enabled`.
    pub k8s: OnceLock<Arc<crate::k8s::K8s>>,
    next_conn: AtomicU64,
}

impl Edge {
    /// Build the state.
    pub fn new(cfg: Config, secrets: Secrets, store: Store, id: PublicKey) -> anyhow::Result<Edge> {
        let brands = match &cfg.brand_blocklist_file {
            Some(p) if p.exists() => std::fs::read_to_string(p)?
                .lines()
                .map(|l| l.trim().to_ascii_lowercase())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect(),
            _ => Vec::new(),
        };
        let per_sec = NonZeroU32::new(cfg.limits.new_conns_per_ip_sec.max(1)).expect("nonzero");
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
            next_conn: AtomicU64::new(1),
            certs: OnceLock::new(),
            k8s: OnceLock::new(),
            brands,
            cfg,
            secrets,
            store,
            id,
        })
    }

    /// Live admission mode.
    pub fn admission(&self) -> Admission {
        *self.admission.read().expect("admission lock")
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

    /// A connection ended: drop its routes and its tunnel entry (if still the current one).
    pub fn detach(&self, tunnel: &Arc<Tunnel>) {
        self.drop_routes(|_, r| Arc::ptr_eq(&r.tunnel, tunnel));
        self.tunnels
            .remove_if(&tunnel.engine, |_, t| Arc::ptr_eq(t, tunnel));
    }
}
