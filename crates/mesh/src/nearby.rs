//! 183 §6.8 (W-17): Nearby — advertise and browse `_theuth-mesh._udp` (TXT `v=1`, `id=`,
//! `name=`, `os=`) while Settings › Network is open or `t mesh pair` runs, and the
//! code-checked pairing ceremony on `theuth/mesh/1` (`PairRequest` → `PairChallenge` →
//! `PairReveal` → both `PairAnswer`s → the existing `PairHello` path with a fresh token).
//!
//! `lib.rs` (one message per RPC) and `net.rs` (the stranger's first frame) call in here, so
//! the Nearby work stays in this file. mDNS sits behind [`Mdns`]: [`Swarm`] on a real network,
//! an in-process fake in tests. Nothing is advertised or browsed unless a window is open and
//! `mesh.nearby` allows it; an advert is a hint only (§8.2): the dial is by endpoint id and
//! both humans compare a code derived from both real ids before anything is stored.
//!
//! Ceremony (commit-reveal): the dialer sends `PairRequest{commit = H(nonce_a)}`, the acceptor
//! answers `PairChallenge{nonce_b}`, the dialer opens `PairReveal{nonce_a}`, and the acceptor
//! checks it before it shows anything. Neither side picks its nonce after seeing the other's,
//! so an active LAN attacker in the middle cannot steer two ceremonies to one code; a dialer
//! that learns the code and walks away before revealing counts as a failed ceremony at once.
//! Both show `code(id_a, id_b, nonce_a, nonce_b)`. The dialer's Pair sends `PairAnswer{accept}`;
//! the acceptor sends its own only once both pressed, with a fresh token and its vault key,
//! and the dialer then runs [`Net::pair_with`] as a pasted string would. Cancel on either
//! side, or `mesh.pair_window_secs`, ends it.
//!
//! Bounds (§8.2): one ceremony per machine, `MAX_CEREMONIES` at once; `MAX_FAILURES` failed
//! ceremonies refuse that machine for the window, `MAX_WINDOW_FAILURES` from anyone close it.
//! `mesh_nearby_watch(true)` is a lease of `WATCH_TTL_MS` the open page renews.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use data_encoding::BASE32_NOPAD;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use sha2::Digest as _;
use tokio::sync::{broadcast, mpsc};

use crate::identity::now_ms;
use crate::net::{ALPN_MESH, CONNECT_TIMEOUT, Msg, Net, read_msg, write_msg};
use crate::pairing::Payload;
use crate::types::{EngineId, MeshEvent, NearbyEngine, PairCeremony};

/// DNS-SD service name: `_theuth-mesh._udp.local`.
pub const SERVICE: &str = "theuth-mesh";
/// §8.2: at most this many sightings, newest first.
const MAX_LISTED: usize = 32;
/// §8.2: this many failed ceremonies (refused, expired, or a request never revealed) from one
/// machine refuse it for the rest of the window.
const MAX_FAILURES: u32 = 3;
/// §8.2: this many failed ceremonies from any machines close the window. Each is at most one
/// blind guess at the 6-digit code (commit-reveal), so a window admits ≤ 10⁻⁵ of a match, and
/// three strangers (≤ 9 failures) cannot close an honest user's window.
const MAX_WINDOW_FAILURES: u32 = 10;
/// Ceremonies this engine accepts at once (a request still waiting for its reveal counts).
const MAX_CEREMONIES: usize = 4;
/// The dialer's `PairReveal` must follow the challenge within this.
const REVEAL_TIMEOUT: Duration = Duration::from_secs(10);
/// `mesh_nearby_watch(true)` holds the window this long; Settings › Network renews it.
pub(crate) const WATCH_TTL_MS: u64 = 30_000;
/// Advert addresses kept (and dialled) per sighting.
const MAX_ADDRS: usize = 8;
/// TXT `name` / `os` are cut to this many bytes (one TXT pair is at most 255 bytes).
const NAME_MAX: usize = 63;
/// The `PairReject` reason a side sends when its window ends, so the other reads `expired`.
const EXPIRED: &str = "the pairing window closed";

/// What one engine advertises, and what a browse reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    /// Endpoint id (TXT `id`).
    pub id: String,
    /// Display name (TXT `name`).
    pub name: String,
    /// Display OS (TXT `os`).
    pub os: String,
    /// Where it answers.
    pub addrs: Vec<SocketAddr>,
}

/// A browse callback: an advert seen (`false`) or gone (`true`).
pub type Seen = Box<dyn Fn(Advert, bool) + Send + Sync>;

/// mDNS advertise + browse. `start` advertises `me` and reports every other advert to
/// `seen` until the returned guard is dropped.
pub trait Mdns: Send + Sync {
    /// Start both; drop the guard to stop them.
    fn start(&self, me: Advert, seen: Seen) -> Result<Box<dyn Send>, String>;
}

/// Real mDNS (`swarm-discovery`, the crate iroh's own mDNS lookup uses).
pub struct Swarm;

impl Mdns for Swarm {
    fn start(&self, me: Advert, seen: Seen) -> Result<Box<dyn Send>, String> {
        // A DNS label holds 63 bytes: the 32-byte id in base32 (52), as iroh's lookup does.
        let label = |id: &str| {
            EndpointId::from_str(id)
                .map(|e| BASE32_NOPAD.encode(e.as_bytes()).to_ascii_lowercase())
                .map_err(|e| format!("endpoint id {id}: {e}"))
        };
        let txt = [
            ("v", "1".to_owned()),
            ("id", me.id.clone()),
            ("name", cut(&me.name, NAME_MAX)),
            ("os", cut(&me.os, NAME_MAX)),
        ]
        .map(|(k, v)| (k.to_owned(), Some(v)));
        let mut d =
            swarm_discovery::Discoverer::new_interactive(SERVICE.to_owned(), label(&me.id)?)
                .with_txt_attributes(txt)
                .map_err(|e| e.to_string())?
                .with_callback(move |from: &str, peer: &swarm_discovery::Peer| {
                    let txt = |k: &str| {
                        peer.txt_attribute(k)
                            .flatten()
                            .unwrap_or_default()
                            .to_owned()
                    };
                    if txt("v") != "1" {
                        return;
                    }
                    // An expired peer may carry no TXT: its id comes from the label.
                    let id = match txt("id") {
                        id if !id.is_empty() => id,
                        _ => match BASE32_NOPAD
                            .decode(from.to_ascii_uppercase().as_bytes())
                            .ok()
                            .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
                            .and_then(|b| EndpointId::from_bytes(&b).ok())
                        {
                            Some(e) => e.to_string(),
                            None => return,
                        },
                    };
                    let advert = Advert {
                        id,
                        name: txt("name"),
                        os: txt("os"),
                        addrs: peer
                            .addrs()
                            .iter()
                            .map(|&(ip, port)| SocketAddr::new(ip, port))
                            .collect(),
                    };
                    seen(advert, peer.is_expiry());
                });
        for a in &me.addrs {
            d = d.with_addrs(a.port(), [a.ip()]);
        }
        let guard = d
            .spawn(&tokio::runtime::Handle::current())
            .map_err(|e| format!("mDNS: {e}"))?;
        Ok(Box::new(guard))
    }
}

/// `Net`'s Nearby state.
pub(crate) struct State {
    mdns: Arc<dyn Mdns>,
    /// `MeshEvent::Nearby` / `MeshEvent::Pair`; the plugin puts them on the bus.
    pub(crate) events: broadcast::Sender<MeshEvent>,
    /// `mesh.nearby`.
    allowed: AtomicBool,
    /// `mesh.pair_window_secs`.
    window_secs: AtomicU64,
    /// The `mesh_nearby_watch(true)` lease ([`WATCH_TTL_MS`]; tests shorten it).
    ttl_ms: AtomicU64,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Settings › Network's lease (`mesh_nearby_watch`), ms since epoch.
    watch_until_ms: u64,
    /// `t mesh pair`'s window, ms since epoch.
    window_until_ms: u64,
    /// Failed ceremonies this window, by the other engine's id (§8.2).
    failures: HashMap<String, u32>,
    /// Failed ceremonies this window, all machines.
    failed: u32,
    /// Machines whose `PairRequest` is waiting for its `PairReveal`.
    pending: HashSet<String>,
    /// The running advertise + browse.
    guard: Option<Box<dyn Send>>,
    /// Sightings by endpoint id, with every address seen.
    seen: HashMap<String, (NearbyEngine, Vec<SocketAddr>)>,
    ceremonies: HashMap<uuid::Uuid, Ceremony>,
}

impl Inner {
    /// A fresh window: every count starts again.
    fn reset(&mut self) {
        self.failures.clear();
        self.failed = 0;
    }

    /// Count one failed ceremony with `peer`; `true` when that closes the window.
    fn fail(&mut self, peer: &str) -> bool {
        *self.failures.entry(peer.to_owned()).or_default() += 1;
        self.failed += 1;
        self.failed == MAX_WINDOW_FAILURES
    }
}

struct Ceremony {
    view: PairCeremony,
    /// This side sent the `PairRequest`.
    dialer: bool,
    /// This side's user pressed Pair.
    mine: bool,
    /// The other side's user pressed Pair.
    theirs: bool,
    /// Acceptor: the token it minted; dialer: the token it was given.
    token: Option<String>,
    /// Dialer: the acceptor's vault key (base32).
    vault_key: Option<crate::net::Secret>,
    /// Dialer: where the acceptor answered.
    addrs: Vec<SocketAddr>,
    /// Dialer: `Net::pair_with` is running; it settles the ceremony.
    pairing: bool,
    /// Frames for the ceremony stream.
    out: mpsc::UnboundedSender<Msg>,
}

/// What a ceremony does next, outside the lock.
enum Next {
    Wait,
    /// Acceptor, both pressed: send the token and the vault key.
    Grant(String, mpsc::UnboundedSender<Msg>),
    /// Dialer, both pressed and the token in: pair as a pasted string would.
    Pair(Payload),
}

impl State {
    pub(crate) fn new(mdns: Option<Arc<dyn Mdns>>) -> Self {
        Self {
            mdns: mdns.unwrap_or_else(default_mdns),
            events: broadcast::channel(64).0,
            allowed: AtomicBool::new(true),
            window_secs: AtomicU64::new(120),
            ttl_ms: AtomicU64::new(WATCH_TTL_MS),
            inner: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, ev: MeshEvent) {
        let _ = self.events.send(ev);
    }

    fn window_ms(&self) -> u64 {
        self.window_secs.load(Ordering::Relaxed).max(1) * 1000
    }

    /// Advertise + browse should run now.
    fn open(&self, i: &Inner) -> bool {
        let now = now_ms();
        self.allowed.load(Ordering::Relaxed)
            && i.failed < MAX_WINDOW_FAILURES
            && (i.watch_until_ms > now || i.window_until_ms > now)
    }
}

/// Real mDNS, or in a debug build with `THEUTH_DEV_NEARBY_DIR` set, [`dev::DirLan`]: the
/// scratch-engine rig's adverts as files, so Nearby is proven without any network interface.
fn default_mdns() -> Arc<dyn Mdns> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("THEUTH_DEV_NEARBY_DIR") {
        tracing::warn!("mesh: Nearby uses the dev directory {dir:?}, not mDNS");
        return Arc::new(dev::DirLan(PathBuf::from(dir)));
    }
    Arc::new(Swarm)
}

/// The plugin at start: `mesh.nearby` and `mesh.pair_window_secs`.
pub(crate) fn configure(net: &Arc<Net>, allowed: bool, window_secs: u64) {
    net.nearby.allowed.store(allowed, Ordering::Relaxed);
    net.nearby.window_secs.store(window_secs, Ordering::Relaxed);
}

/// `mesh_set_nearby(on)`: the switch; an open window starts or stops at once.
pub(crate) fn allow(net: &Arc<Net>, on: bool) -> Result<(), String> {
    net.nearby.allowed.store(on, Ordering::Relaxed);
    refresh(net)
}

/// `mesh_nearby_watch(open)`: start or stop advertise + browse for this window. Honours
/// `mesh.nearby`. A lone engine stays off the relay: the LAN dial needs only the advert's
/// addresses, and pairing joins the network as a pasted string does. `true` is a lease of
/// [`WATCH_TTL_MS`]: the open page renews it, so a killed window stops advertising and
/// accepting once it lapses. A renewal keeps the window's counts; a fresh open resets them.
pub(crate) async fn watch(net: &Arc<Net>, open: bool) -> Result<(), String> {
    let ttl = net.nearby.ttl_ms.load(Ordering::Relaxed);
    {
        let mut i = net.nearby.lock();
        let now = now_ms();
        if open {
            if i.watch_until_ms <= now {
                i.reset();
            }
            i.watch_until_ms = now + ttl;
        } else {
            i.watch_until_ms = 0;
        }
    }
    if open {
        refresh_after(net, ttl);
    }
    refresh(net)
}

/// `t mesh pair`: be findable (and see others) for `mesh.pair_window_secs`.
pub(crate) fn open_window(net: &Arc<Net>) {
    let ms = net.nearby.window_ms();
    {
        let mut i = net.nearby.lock();
        i.window_until_ms = now_ms() + ms;
        i.reset();
    }
    if let Err(e) = refresh(net) {
        tracing::warn!("mesh: Nearby: {e}");
    }
    refresh_after(net, ms);
}

/// Look again just after `ms`, when a window or lease may have lapsed.
fn refresh_after(net: &Arc<Net>, ms: u64) {
    let weak = Arc::downgrade(net);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms + 50)).await;
        if let Some(net) = weak.upgrade() {
            let _ = refresh(&net);
        }
    });
}

/// Start or stop advertise + browse to match the window.
fn refresh(net: &Arc<Net>) -> Result<(), String> {
    let st = &net.nearby;
    let (want, running) = {
        let i = st.lock();
        (st.open(&i), i.guard.is_some())
    };
    if want == running {
        return Ok(());
    }
    if !want {
        let old = {
            let mut i = st.lock();
            i.seen.clear();
            i.guard.take()
        };
        // Outside the lock: a fake LAN calls the other members back from the drop.
        drop(old);
        st.emit(MeshEvent::Nearby(Vec::new()));
        tracing::info!("mesh: Nearby closed");
        return Ok(());
    }
    let weak = Arc::downgrade(net);
    let guard = st.mdns.start(
        advert(net),
        Box::new(move |a, gone| {
            if let Some(net) = weak.upgrade() {
                sighted(&net, a, gone);
            }
        }),
    )?;
    let extra = {
        let mut i = st.lock();
        if i.guard.is_some() {
            Some(guard)
        } else {
            i.guard = Some(guard);
            None
        }
    };
    drop(extra);
    tracing::info!("mesh: Nearby open (advertising and browsing _{SERVICE}._udp)");
    Ok(())
}

/// This engine's advert: its id, name, display OS and local direct addresses.
fn advert(net: &Net) -> Advert {
    let addrs: Vec<SocketAddr> = net
        .endpoint
        .addr()
        .addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Ip(s) => Some(*s),
            _ => None,
        })
        .collect();
    Advert {
        id: net.id.0.clone(),
        name: net.name(),
        os: platform::process::machine().os_name.clone(),
        addrs: local_addrs(&addrs),
    }
}

/// §8.2: the advert addresses worth dialling: private, link-local or loopback (a forged advert
/// cannot point the dial at a public host), at most [`MAX_ADDRS`].
fn local_addrs(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    addrs
        .iter()
        .copied()
        .filter(|a| match a.ip() {
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
            IpAddr::V6(ip) => {
                ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local()
            }
        })
        .take(MAX_ADDRS)
        .collect()
}

/// `s` cut to at most `max` bytes, on a char boundary.
fn cut(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// A browse result: keep it (bounded, oldest out) or drop it, then publish the list. An advert
/// with no local address to dial is dropped.
fn sighted(net: &Arc<Net>, a: Advert, gone: bool) {
    if a.id == net.id.0 {
        return;
    }
    let addrs = local_addrs(&a.addrs);
    let list = {
        let mut i = net.nearby.lock();
        if gone || addrs.is_empty() {
            i.seen.remove(&a.id);
        } else {
            let engine = NearbyEngine {
                id: EngineId(a.id.clone()),
                name: cut(&a.name, NAME_MAX),
                os_name: cut(&a.os, NAME_MAX),
                addr: addrs[0].to_string(),
                seen_ms: now_ms(),
            };
            i.seen.insert(a.id, (engine, addrs));
            while i.seen.len() > MAX_LISTED {
                let Some(oldest) = i
                    .seen
                    .iter()
                    .min_by_key(|(_, (e, _))| e.seen_ms)
                    .map(|(id, _)| id.clone())
                else {
                    break;
                };
                i.seen.remove(&oldest);
            }
        }
        listing(net, &i)
    };
    net.nearby.emit(MeshEvent::Nearby(list));
}

fn listing(net: &Net, i: &Inner) -> Vec<NearbyEngine> {
    let mut out: Vec<NearbyEngine> = i
        .seen
        .values()
        .filter(|(e, _)| !EndpointId::from_str(&e.id.0).is_ok_and(|id| net.is_paired(&id)))
        .map(|(e, _)| e.clone())
        .collect();
    out.sort_by(|a, b| b.seen_ms.cmp(&a.seen_ms).then_with(|| a.id.0.cmp(&b.id.0)));
    out.truncate(MAX_LISTED);
    out
}

/// `mesh_nearby`: engines seen by the browse, not paired, newest first, at most 32 (§8.2).
pub(crate) fn list(net: &Net) -> Vec<NearbyEngine> {
    listing(net, &net.nearby.lock())
}

/// §6.8: the 6-digit check code both sides show, `"482 913"`.
fn code(id_a: &str, id_b: &str, nonce_a: &str, nonce_b: &str) -> String {
    let (lo, hi) = if id_a <= id_b {
        (id_a, id_b)
    } else {
        (id_b, id_a)
    };
    let h = sha2::Sha256::new()
        .chain_update(lo)
        .chain_update(hi)
        .chain_update(nonce_a)
        .chain_update(nonce_b)
        .finalize();
    let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

fn nonce() -> String {
    BASE32_NOPAD.encode(&rand::random::<[u8; 16]>())
}

/// The dialer's commitment to its nonce: base32 `SHA256(nonce)`.
fn commitment(nonce: &str) -> String {
    BASE32_NOPAD.encode(&sha2::Sha256::digest(nonce.as_bytes()))
}

/// 183 R2: what a dial to a machine this mesh unpaired reads while that machine still lists
/// this mesh (it drops it on its next connect: `Net::drive`'s "removed").
pub(crate) fn unpaired_text(name: &str) -> String {
    format!(
        "{name} was unpaired from this mesh; on {name}, remove this mesh first (Settings › Network › Other meshes)"
    )
}

/// `mesh_pair_nearby(id)`: dial `id` with `PairRequest`; the ceremony shows the code.
pub(crate) async fn pair(net: &Arc<Net>, id: &EngineId) -> Result<PairCeremony, String> {
    let (engine, addrs) = net
        .nearby
        .lock()
        .seen
        .get(&id.0)
        .cloned()
        .ok_or_else(|| format!("{id} is not nearby"))?;
    let eid = EndpointId::from_str(&id.0).map_err(|e| format!("{id}: {e}"))?;
    if net.is_paired(&eid) {
        return Err(format!("{} is already in this mesh", engine.name));
    }
    let addr = EndpointAddr {
        id: eid,
        addrs: addrs.iter().map(|a| TransportAddr::Ip(*a)).collect(),
    };
    let conn = tokio::time::timeout(
        CONNECT_TIMEOUT,
        net.endpoint.connect(addr, ALPN_MESH.as_bytes()),
    )
    .await
    .map_err(|_| format!("{} did not answer", engine.name))?
    .map_err(|e| format!("dial {}: {e}", engine.name))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    let nonce_a = nonce();
    write_msg(
        &mut send,
        &Msg::PairRequest {
            name: net.name(),
            os: platform::process::machine().os_name.clone(),
            commit: commitment(&nonce_a),
        },
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    let nonce_b = match read_msg(&mut recv).await {
        Ok(Msg::PairChallenge { nonce }) => nonce,
        Ok(Msg::PairReject { reason }) => return Err(format!("{} refused: {reason}", engine.name)),
        Ok(_) | Err(_) => {
            // 183 R2: a machine this mesh unpaired still lists us as paired (its tombstone never
            // reaches it), so it took the request for a control stream and dropped it.
            let unpaired = net
                .store
                .peer_entries()
                .await
                .unwrap_or_default()
                .into_iter()
                .any(|p| p.id == *id && p.removed());
            if unpaired {
                return Err(unpaired_text(&engine.name));
            }
            return Err(format!(
                "{} is not open for pairing (its Settings › Network page must be open)",
                engine.name
            ));
        }
    };
    // Only now, with the acceptor's nonce fixed, open ours.
    write_msg(
        &mut send,
        &Msg::PairReveal {
            nonce: nonce_a.clone(),
        },
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    let view = PairCeremony {
        id: uuid::Uuid::new_v4(),
        peer: id.clone(),
        name: engine.name,
        os_name: engine.os_name,
        addr: engine.addr,
        code: code(&net.id.0, &id.0, &nonce_a, &nonce_b),
        state: "waiting".into(),
        expires_ms: now_ms() + net.nearby.window_ms(),
    };
    start(net, view.clone(), true, addrs, conn, send, recv);
    Ok(view)
}

/// `mesh_pair_answer(ceremony, accept)`: this side's Pair or Cancel.
pub(crate) async fn answer(
    net: &Arc<Net>,
    ceremony: uuid::Uuid,
    accept: bool,
) -> Result<(), String> {
    let gone = || "that pairing has ended".to_string();
    if !accept {
        let out = net
            .nearby
            .lock()
            .ceremonies
            .get(&ceremony)
            .map(|c| c.out.clone())
            .ok_or_else(gone)?;
        let _ = out.send(Msg::PairAnswer {
            accept: false,
            token: None,
            vault_key: None,
        });
        settle(net, ceremony, "rejected");
        return Ok(());
    }
    let (view, next) = {
        let mut i = net.nearby.lock();
        let c = i.ceremonies.get_mut(&ceremony).ok_or_else(gone)?;
        if c.mine {
            return Ok(());
        }
        c.mine = true;
        c.view.state = "accepted".into();
        if c.dialer {
            let _ = c.out.send(Msg::PairAnswer {
                accept: true,
                token: None,
                vault_key: None,
            });
        }
        (c.view.clone(), next_step(c))
    };
    net.nearby.emit(MeshEvent::Pair(view));
    tokio::spawn(run(net.clone(), ceremony, next));
    Ok(())
}

/// Both pressed? The acceptor mints the token; the dialer, holding it, pairs.
fn next_step(c: &mut Ceremony) -> Next {
    if !(c.mine && c.theirs) || c.pairing {
        return Next::Wait;
    }
    match (c.dialer, &c.token) {
        (false, None) => {
            let token = crate::pairing::Pending::fresh().token;
            c.token = Some(token.clone());
            Next::Grant(token, c.out.clone())
        }
        (true, Some(token)) => {
            let token = token.clone();
            c.pairing = true;
            Next::Pair(Payload {
                id: EndpointId::from_str(&c.view.peer.0).expect("dialed by this id"),
                name: c.view.name.clone(),
                relay: String::new(),
                addrs: c.addrs.iter().map(|a| TransportAddr::Ip(*a)).collect(),
                token,
                vault_key: c.vault_key.take().map(|k| k.as_str().to_owned()),
                vault_key_id: None,
            })
        }
        _ => Next::Wait,
    }
}

/// The dialer's `pair_with` is under way for `ceremony` (it settles it, not the stream).
fn pairing(net: &Net, ceremony: uuid::Uuid) -> bool {
    net.nearby
        .lock()
        .ceremonies
        .get(&ceremony)
        .is_some_and(|c| c.pairing)
}

async fn run(net: Arc<Net>, ceremony: uuid::Uuid, next: Next) {
    match next {
        Next::Wait => {}
        Next::Grant(token, out) => {
            let vault_key = net
                .vault()
                .map(|v| crate::net::Secret::new(BASE32_NOPAD.encode(&*v.mesh_key())));
            let _ = out.send(Msg::PairAnswer {
                accept: true,
                token: Some(token),
                vault_key,
            });
        }
        Next::Pair(payload) => match net.pair_with(payload).await {
            Ok(peer) => {
                net.push_peers().await;
                tracing::info!("mesh: paired with {} ({}) over Nearby", peer.name, peer.id);
                settle(&net, ceremony, "paired");
            }
            Err(e) => {
                tracing::warn!("mesh: Nearby pairing: {e:#}");
                settle(&net, ceremony, "rejected");
            }
        },
    }
}

/// End a ceremony in `state` (`paired` | `rejected` | `expired`) and say so.
fn settle(net: &Arc<Net>, ceremony: uuid::Uuid, state: &str) {
    let (view, close) = {
        let mut i = net.nearby.lock();
        let Some(mut c) = i.ceremonies.remove(&ceremony) else {
            return;
        };
        c.view.state = state.to_owned();
        let close = !c.dialer && state != "paired" && i.fail(&c.view.peer.0);
        (c.view, close)
    };
    net.nearby.emit(MeshEvent::Pair(view));
    if close {
        closed_by_failures(net);
    }
}

fn closed_by_failures(net: &Arc<Net>) {
    tracing::info!("mesh: {MAX_WINDOW_FAILURES} Nearby pairings failed; window closed");
    let _ = refresh(net);
}

/// A stranger opened `theuth/mesh/1` with `PairRequest` instead of `PairHello`. While the
/// window is open and within the bounds, answer `PairChallenge`, wait for the reveal, check
/// it against the commitment, and only then show the code (`MeshEvent::Pair`). Otherwise refuse
/// and close, as for any stranger without a token. A request that never reveals, or reveals
/// wrong, is a failed ceremony at once.
pub(crate) async fn on_pair_request(
    net: &Arc<Net>,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    request: Msg,
) {
    let Msg::PairRequest { name, os, commit } = request else {
        conn.close(0u32.into(), b"not paired");
        return;
    };
    let peer = conn.remote_id();
    let peer_id = peer.to_string();
    let refused = {
        let mut i = net.nearby.lock();
        let accepting = i.pending.len() + i.ceremonies.values().filter(|c| !c.dialer).count();
        if !net.nearby.open(&i) {
            Some("pairing is not open on that machine")
        } else if i.failures.get(&peer_id).is_some_and(|n| *n >= MAX_FAILURES) {
            Some("too many failed pairings from this machine")
        } else if i.pending.contains(&peer_id)
            || i.ceremonies.values().any(|c| c.view.peer.0 == peer_id)
        {
            Some("a pairing with this machine is already open")
        } else if accepting >= MAX_CEREMONIES {
            Some("too many pairings at once")
        } else {
            i.pending.insert(peer_id.clone());
            None
        }
    };
    if let Some(reason) = refused {
        tracing::debug!(
            "mesh: {} sent a Nearby pairing request; refused: {reason}",
            peer.fmt_short()
        );
        refuse(conn, send, reason).await;
        return;
    }
    let nonce_b = self::nonce();
    let reveal = async {
        write_msg(
            &mut send,
            &Msg::PairChallenge {
                nonce: nonce_b.clone(),
            },
        )
        .await
        .ok()?;
        match tokio::time::timeout(REVEAL_TIMEOUT, read_msg(&mut recv)).await {
            Ok(Ok(Msg::PairReveal { nonce })) => Some(nonce),
            _ => None,
        }
    }
    .await
    .filter(|n| commitment(n) == commit);
    let close = {
        let mut i = net.nearby.lock();
        i.pending.remove(&peer_id);
        reveal.is_none() && i.fail(&peer_id)
    };
    let Some(nonce_a) = reveal else {
        tracing::info!(
            "mesh: {} sent no matching pairing reveal; refused, counted as a failed pairing",
            peer.fmt_short()
        );
        refuse(
            conn,
            send,
            "the pairing reveal did not match its commitment",
        )
        .await;
        if close {
            closed_by_failures(net);
        }
        return;
    };
    let view = PairCeremony {
        id: uuid::Uuid::new_v4(),
        peer: EngineId(peer_id.clone()),
        name: cut(&name, NAME_MAX),
        os_name: cut(&os, NAME_MAX),
        addr: remote_addr(&conn),
        code: code(&peer_id, &net.id.0, &nonce_a, &nonce_b),
        state: "waiting".into(),
        expires_ms: now_ms() + net.nearby.window_ms(),
    };
    tracing::info!(
        "mesh: {} ({}) asks to pair; code {}",
        view.name,
        peer.fmt_short(),
        view.code
    );
    start(net, view, false, Vec::new(), conn, send, recv);
}

/// `PairReject{reason}`, then close.
async fn refuse(conn: Connection, mut send: SendStream, reason: &str) {
    let _ = write_msg(
        &mut send,
        &Msg::PairReject {
            reason: reason.into(),
        },
    )
    .await;
    let _ = send.finish();
    // A moment for the frame before the connection goes.
    tokio::time::sleep(Duration::from_millis(100)).await;
    conn.close(0u32.into(), b"not paired");
}

/// Where the connection's other end is (its first direct path), `""` if none.
fn remote_addr(conn: &Connection) -> String {
    conn.paths()
        .iter()
        .find_map(|p| match p.remote_addr() {
            TransportAddr::Ip(s) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// `accept_control`: `token` is the one a ceremony with `peer` minted (single use); that
/// ceremony is paired. The same token from any other endpoint is refused.
pub(crate) fn take_token(net: &Arc<Net>, token: &str, peer: &EndpointId) -> bool {
    let peer = peer.to_string();
    let id = net
        .nearby
        .lock()
        .ceremonies
        .iter()
        .find(|(_, c)| !c.dialer && c.view.peer.0 == peer && c.token.as_deref() == Some(token))
        .map(|(id, _)| *id);
    match id {
        Some(id) => {
            settle(net, id, "paired");
            true
        }
        None => false,
    }
}

/// Register the ceremony and drive its stream until it ends.
fn start(
    net: &Arc<Net>,
    view: PairCeremony,
    dialer: bool,
    addrs: Vec<SocketAddr>,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let (out, mut outgoing) = mpsc::unbounded_channel();
    let (id, expires_ms) = (view.id, view.expires_ms);
    net.nearby.lock().ceremonies.insert(
        id,
        Ceremony {
            view: view.clone(),
            dialer,
            mine: false,
            theirs: false,
            token: None,
            vault_key: None,
            addrs,
            pairing: false,
            out,
        },
    );
    net.nearby.emit(MeshEvent::Pair(view));
    // Reads on their own task: a frame half-read when a write wins the select would be lost.
    let (tx, mut incoming) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(m) = read_msg(&mut recv).await {
            if tx.send(m).is_err() {
                return;
            }
        }
    });
    let net = net.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(expires_ms.saturating_sub(now_ms()));
        loop {
            tokio::select! {
                m = outgoing.recv() => match m {
                    Some(m) => if write_msg(&mut send, &m).await.is_err() { break },
                    // Settled: every queued frame went first.
                    None => break,
                },
                m = incoming.recv() => match m {
                    Some(Msg::PairAnswer { accept: true, token, vault_key }) => {
                        let next = {
                            let mut i = net.nearby.lock();
                            let Some(c) = i.ceremonies.get_mut(&id) else { break };
                            c.theirs = true;
                            if c.dialer && token.is_some() {
                                c.token = token;
                                c.vault_key = vault_key;
                            }
                            next_step(c)
                        };
                        tokio::spawn(run(net.clone(), id, next));
                    }
                    Some(Msg::PairAnswer { accept: false, .. }) => {
                        settle(&net, id, "rejected");
                    }
                    // The other side's window ended first (windows differ per machine).
                    Some(Msg::PairReject { reason }) if reason == EXPIRED => {
                        settle(&net, id, "expired");
                    }
                    // The other side closed the stream. Once our `PairHello` landed that is the
                    // acceptor's normal end, and `run` settles. Within a second of our own
                    // deadline it is the other side's window ending (theirs started one round
                    // trip earlier); otherwise it went away.
                    None => {
                        if !pairing(&net, id) {
                            let late = now_ms() + 1_000 >= expires_ms;
                            settle(&net, id, if late { "expired" } else { "rejected" });
                        }
                        break;
                    }
                    Some(_) => {}
                },
                () = tokio::time::sleep_until(deadline) => {
                    if !pairing(&net, id) {
                        let _ = write_msg(&mut send, &Msg::PairReject { reason: EXPIRED.into() }).await;
                        settle(&net, id, "expired");
                    }
                    break;
                }
            }
        }
        let _ = send.finish();
        // A moment for the last frame before the connection goes.
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(0u32.into(), b"pairing ended");
    });
}

/// Debug builds only: a LAN made of files, for the scratch-engine rig (`THEUTH_DEV_NEARBY_DIR`).
/// Each open window writes `<dir>/<id>` (name, OS, addresses, one per line) and reads the
/// others every 500 ms; no socket is opened, so nothing reaches a network interface.
#[cfg(debug_assertions)]
mod dev {
    use super::*;

    pub(super) struct DirLan(pub(super) PathBuf);

    struct Member {
        file: PathBuf,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Member {
        fn drop(&mut self) {
            self.task.abort();
            let _ = std::fs::remove_file(&self.file);
        }
    }

    fn read(id: String, text: &str) -> Option<Advert> {
        let mut lines = text.lines();
        Some(Advert {
            id,
            name: lines.next()?.to_owned(),
            os: lines.next()?.to_owned(),
            addrs: lines.filter_map(|l| l.parse().ok()).collect(),
        })
    }

    impl Mdns for DirLan {
        fn start(&self, me: Advert, seen: Seen) -> Result<Box<dyn Send>, String> {
            std::fs::create_dir_all(&self.0).map_err(|e| format!("{:?}: {e}", self.0))?;
            let file = self.0.join(&me.id);
            let mut text = format!("{}\n{}\n", me.name, me.os);
            for a in &me.addrs {
                text.push_str(&format!("{a}\n"));
            }
            std::fs::write(&file, text).map_err(|e| format!("{file:?}: {e}"))?;
            let dir = self.0.clone();
            let task = tokio::spawn(async move {
                let mut known: HashMap<String, Advert> = HashMap::new();
                loop {
                    let now: HashMap<String, Advert> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter_map(|e| {
                            let id = e.file_name().into_string().ok()?;
                            let text = std::fs::read_to_string(e.path()).ok()?;
                            (id != me.id).then(|| read(id.clone(), &text)).flatten()
                        })
                        .map(|a| (a.id.clone(), a))
                        .collect();
                    for (id, a) in &now {
                        if known.get(id) != Some(a) {
                            seen(a.clone(), false);
                        }
                    }
                    for (id, a) in &known {
                        if !now.contains_key(id) {
                            seen(a.clone(), true);
                        }
                    }
                    known = now;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            });
            Ok(Box::new(Member { file, task }))
        }
    }
}

/// An in-process LAN for tests: every member sees every other member's advert, and a
/// dropped guard is announced gone. No socket is opened.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    type Members = Vec<(u64, Advert, Arc<Seen>)>;

    #[derive(Clone, Default)]
    pub(crate) struct Lan(Arc<Mutex<Members>>, Arc<AtomicU64>);

    struct Member {
        lan: Lan,
        key: u64,
        advert: Advert,
    }

    impl Drop for Member {
        fn drop(&mut self) {
            let others: Vec<Arc<Seen>> = {
                let mut m = self.lan.0.lock().unwrap_or_else(|p| p.into_inner());
                m.retain(|x| x.0 != self.key);
                m.iter().map(|x| x.2.clone()).collect()
            };
            for s in others {
                (*s)(self.advert.clone(), true);
            }
        }
    }

    impl Mdns for Lan {
        fn start(&self, me: Advert, seen: Seen) -> Result<Box<dyn Send>, String> {
            let seen: Arc<Seen> = Arc::new(seen);
            let key = self.1.fetch_add(1, Ordering::Relaxed);
            let (existing, others): (Vec<Advert>, Vec<Arc<Seen>>) = {
                let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
                let existing = m.iter().map(|x| x.1.clone()).collect();
                let others = m.iter().map(|x| x.2.clone()).collect();
                m.push((key, me.clone(), seen.clone()));
                (existing, others)
            };
            for a in existing {
                (*seen)(a, false);
            }
            for s in others {
                (*s)(me.clone(), false);
            }
            Ok(Box::new(Member {
                lan: self.clone(),
                key,
                advert: me,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::node_with;

    async fn pair_event(rx: &mut broadcast::Receiver<MeshEvent>, state: &str) -> PairCeremony {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(MeshEvent::Pair(p)) = rx.recv().await
                    && p.state == state
                {
                    return p;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no `{state}` ceremony event"))
    }

    async fn eventually(what: &str, f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// The code is symmetric in the ids, depends on both nonces, and reads `ddd ddd`.
    #[test]
    fn the_code_is_six_digits_from_both_ids_and_nonces() {
        let c = code("aa", "bb", "n1", "n2");
        assert_eq!(c, code("bb", "aa", "n1", "n2"));
        assert_ne!(c, code("aa", "bb", "n1", "n3"));
        assert_ne!(c, code("aa", "cc", "n1", "n2"));
        assert_eq!(c.len(), 7);
        assert!(c.chars().enumerate().all(|(i, ch)| if i == 3 {
            ch == ' '
        } else {
            ch.is_ascii_digit()
        }));
    }

    /// Nothing is seen until a window opens; a watched page lists the other unpaired engine;
    /// closing one side's window makes it gone on the other.
    #[tokio::test]
    async fn nearby_lists_unpaired_engines_only_while_a_window_is_open() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        watch(&a, true).await.unwrap();
        assert!(list(&a).is_empty(), "b advertises nothing yet");
        watch(&b, true).await.unwrap();
        let seen = list(&a);
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].id.0.as_str(), seen[0].name.as_str()),
            (b.id.0.as_str(), "b")
        );
        assert_eq!(list(&b)[0].id, a.id);
        watch(&b, false).await.unwrap();
        assert!(list(&a).is_empty(), "b's window closed: gone");
        assert!(list(&b).is_empty());
        // `mesh.nearby: false`: an open page advertises nothing.
        allow(&b, false).unwrap();
        watch(&b, true).await.unwrap();
        assert!(list(&a).is_empty());
    }

    /// Both sides show one code; both press Pair; today's pairing runs (vault key included).
    #[tokio::test]
    async fn nearby_pairing_shows_one_code_on_both_sides_and_pairs() {
        let lan = fake::Lan::default();
        let (a, _da, ka) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, kb) = node_with("b", Some(Arc::new(lan.clone()))).await;
        let (mut ea, mut eb) = (a.nearby_events(), b.nearby_events());
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        let ca = pair(&a, &b.id).await.unwrap();
        let cb = pair_event(&mut eb, "waiting").await;
        assert_eq!(ca.code, cb.code, "both machines show the same code");
        assert_eq!((cb.peer.clone(), cb.name.as_str()), (a.id.clone(), "a"));
        assert!(
            !b.is_paired(&EndpointId::from_str(&a.id.0).unwrap()),
            "nothing before both press"
        );
        answer(&b, cb.id, true).await.unwrap();
        answer(&a, ca.id, true).await.unwrap();
        pair_event(&mut ea, "paired").await;
        pair_event(&mut eb, "paired").await;
        let (ia, ib) = (
            EndpointId::from_str(&a.id.0).unwrap(),
            EndpointId::from_str(&b.id.0).unwrap(),
        );
        eventually("both paired", || a.is_paired(&ib) && b.is_paired(&ia)).await;
        eventually("one vault key after pairing, as with a string", || {
            *ka.mesh_key() == *kb.mesh_key()
        })
        .await;
        assert!(list(&a).is_empty(), "a paired engine is not listed");
    }

    /// 183 W-35 on the Nearby path: pairing mid-rotation (a new key current, the old one still
    /// retained) converges as a pasted string does. The Grant carries only the ceremony's token
    /// and `vault_key`; the keyring rides `PairAck`, so the joiner holds both keys.
    #[tokio::test]
    async fn nearby_pairing_mid_rotation_carries_the_keyring() {
        let lan = fake::Lan::default();
        let (a, _da, ka) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, kb) = node_with("b", Some(Arc::new(lan.clone()))).await;
        // `a` is the established side (one peer), so `key_owner` makes `b` adopt.
        let (c, _dc, _kc) = node_with("c", Some(Arc::new(lan.clone()))).await;
        c.add_peer(&a.pairing_string().await).await.unwrap();
        let k0 = ka.current_key();
        let rotated = crate::keys::rotate(&a).await.unwrap();
        assert!(ka.held_keys().contains(&k0.id), "mid-rotation: K0 retained");

        let (mut ea, mut eb) = (a.nearby_events(), b.nearby_events());
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        let cb = pair(&b, &a.id).await.unwrap();
        let ca = pair_event(&mut ea, "waiting").await;
        answer(&a, ca.id, true).await.unwrap();
        answer(&b, cb.id, true).await.unwrap();
        pair_event(&mut ea, "paired").await;
        pair_event(&mut eb, "paired").await;
        eventually("b took a's current key", || {
            kb.current_key().id == rotated.key.id
        })
        .await;
        assert_eq!(
            ka.current_key().id,
            rotated.key.id,
            "the owner kept its key"
        );
        assert!(kb.held_keys().contains(&k0.id), "the retained key came too");
        assert!(kb.current_key().members.contains(&b.id.0));
        for n in [&a, &b, &c] {
            n.shutdown().await;
        }
    }

    /// A lookalike dials too: its code differs from the real machine's; Cancel refuses it and
    /// nothing is stored, while the real one still pairs.
    #[tokio::test]
    async fn a_mismatched_code_is_cancelled_and_nothing_is_stored() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        let (m, _dm, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (mut eb, mut em) = (b.nearby_events(), m.nearby_events());
        for n in [&a, &b, &m] {
            watch(n, true).await.unwrap();
        }
        let real = pair(&a, &b.id).await.unwrap();
        let on_b_real = pair_event(&mut eb, "waiting").await;
        let fake = pair(&m, &b.id).await.unwrap();
        let on_b_fake = pair_event(&mut eb, "waiting").await;
        assert_eq!(on_b_real.code, real.code);
        assert_ne!(
            on_b_fake.code, real.code,
            "the lookalike shows another code"
        );
        assert_eq!(on_b_fake.code, fake.code);
        // B's user compares with A's screen: Cancel the lookalike.
        answer(&b, on_b_fake.id, false).await.unwrap();
        pair_event(&mut em, "rejected").await;
        assert!(
            answer(&m, fake.id, true).await.is_err(),
            "the refused ceremony is over"
        );
        let im = EndpointId::from_str(&m.id.0).unwrap();
        assert!(!b.is_paired(&im) && b.store.peers().await.unwrap().is_empty());
        // The real one still pairs.
        answer(&b, on_b_real.id, true).await.unwrap();
        answer(&a, real.id, true).await.unwrap();
        pair_event(&mut eb, "paired").await;
        assert!(!b.is_paired(&im));
    }

    /// Past the acceptor's `mesh.pair_window_secs` the ceremony is expired on both sides, even
    /// when the dialer's own window is longer, and a late Pair is refused; nothing was paired.
    #[tokio::test]
    async fn an_expired_window_is_refused() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        configure(&b, true, 1);
        let (mut ea, mut eb) = (a.nearby_events(), b.nearby_events());
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        let ca = pair(&a, &b.id).await.unwrap();
        let cb = pair_event(&mut eb, "waiting").await;
        answer(&a, ca.id, true).await.unwrap();
        pair_event(&mut ea, "expired").await;
        pair_event(&mut eb, "expired").await;
        assert!(answer(&b, cb.id, true).await.is_err(), "too late");
        assert!(a.store.peers().await.unwrap().is_empty());
        assert!(b.store.peers().await.unwrap().is_empty());
        // A window that is not open refuses the request outright.
        watch(&a, false).await.unwrap();
        let (c, _dc, _) = node_with("c", Some(Arc::new(lan.clone()))).await;
        watch(&c, true).await.unwrap();
        // `b` sees `c`; `c` closes its page but `b` still holds the sighting for a moment.
        let stale = net_entry(&b, &c.id);
        watch(&c, false).await.unwrap();
        b.nearby.lock().seen.insert(c.id.0.clone(), stale);
        let e = pair(&b, &c.id).await.unwrap_err();
        assert!(e.contains("not open"), "{e}");
    }

    fn net_entry(net: &Net, id: &EngineId) -> (NearbyEngine, Vec<SocketAddr>) {
        net.nearby.lock().seen.get(&id.0).cloned().expect("seen")
    }

    /// A hand-driven dialer from `from` to `to`: `PairRequest` committing to `"n"`, then
    /// `reveal` (`None`: stop after the challenge). The refusal it read, if any, and the
    /// connection and streams (dropping them walks away).
    async fn hand_dial(
        from: &Net,
        to: &Net,
        reveal: Option<&str>,
    ) -> (Option<String>, (Connection, SendStream, RecvStream)) {
        let conn = from
            .endpoint
            .connect(to.endpoint.addr(), ALPN_MESH.as_bytes())
            .await
            .unwrap();
        let (mut s, mut r) = conn.open_bi().await.unwrap();
        let request = Msg::PairRequest {
            name: "m".into(),
            os: "x".into(),
            commit: commitment("n"),
        };
        write_msg(&mut s, &request).await.unwrap();
        let reason = match read_msg(&mut r).await {
            Ok(Msg::PairReject { reason }) => Some(reason),
            Ok(Msg::PairChallenge { .. }) => match reveal {
                None => None,
                Some(n) => {
                    let open = Msg::PairReveal { nonce: n.into() };
                    write_msg(&mut s, &open).await.unwrap();
                    match tokio::time::timeout(Duration::from_secs(2), read_msg(&mut r)).await {
                        Ok(Ok(Msg::PairReject { reason })) => Some(reason),
                        _ => None,
                    }
                }
            },
            other => panic!("{other:?}"),
        };
        (reason, (conn, s, r))
    }

    fn failures(net: &Net, peer: &Net) -> u32 {
        net.nearby
            .lock()
            .failures
            .get(&peer.id.0)
            .copied()
            .unwrap_or(0)
    }

    /// N-MUST (commit-reveal): an acceptor in the middle sees only `H(nonce_a)` before it
    /// picks `nonce_b`. Grinding `nonce_b` against what it sees (the old attack, when the
    /// request carried `nonce_a` itself) does not give the code the dialer shows.
    #[tokio::test]
    async fn a_grinding_acceptor_cannot_steer_the_code() {
        let (a, _da, _) = node_with("a", Some(Arc::new(fake::Lan::default()))).await;
        let m = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(iroh::SecretKey::generate())
            .alpns(vec![ALPN_MESH.as_bytes().to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let mid = m.id().to_string();
        let addrs = m
            .addr()
            .addrs
            .iter()
            .filter_map(|t| match t {
                TransportAddr::Ip(s) => Some(*s),
                _ => None,
            })
            .collect();
        let sighting = NearbyEngine {
            id: EngineId(mid.clone()),
            name: "b".into(),
            os_name: "macOS".into(),
            addr: String::new(),
            seen_ms: now_ms(),
        };
        a.nearby.lock().seen.insert(mid.clone(), (sighting, addrs));
        let (a_id, m_id) = (a.id.0.clone(), mid.clone());
        let grinder = tokio::spawn(async move {
            let conn = m.accept().await.unwrap().await.unwrap();
            let (mut s, mut r) = conn.accept_bi().await.unwrap();
            let Ok(Msg::PairRequest { commit, .. }) = read_msg(&mut r).await else {
                panic!("no PairRequest");
            };
            let nb = (0u64..)
                .map(|n| n.to_string())
                .find(|nb| code(&a_id, &m_id, &commit, nb).starts_with("00"))
                .unwrap();
            let chosen = code(&a_id, &m_id, &commit, &nb);
            write_msg(&mut s, &Msg::PairChallenge { nonce: nb.clone() })
                .await
                .unwrap();
            let Ok(Msg::PairReveal { nonce }) = read_msg(&mut r).await else {
                panic!("no PairReveal");
            };
            let opened = commitment(&nonce) == commit;
            (chosen, code(&a_id, &m_id, &nonce, &nb), opened, conn, m)
        });
        let shown = pair(&a, &EngineId(mid)).await.unwrap();
        let (chosen, real, opened, _conn, _m) = grinder.await.unwrap();
        assert!(opened, "the reveal opens the commitment");
        assert_eq!(shown.code, real);
        assert_ne!(
            shown.code, chosen,
            "the grinder's chosen code is not what A shows"
        );
    }

    /// N-MUST: a reveal that does not open the commitment is refused before any code shows and
    /// counts as a failure; so does a request that walks away after the challenge, at once
    /// (well inside the reveal timeout).
    #[tokio::test]
    async fn a_bad_reveal_is_refused_and_a_dropped_request_counts_at_once() {
        let lan = fake::Lan::default();
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        let (m, _dm, _) = node_with("m", Some(Arc::new(lan.clone()))).await;
        watch(&b, true).await.unwrap();
        let mut eb = b.nearby_events();
        let (reason, _held) = hand_dial(&m, &b, Some("not n")).await;
        assert_eq!(
            reason.as_deref(),
            Some("the pairing reveal did not match its commitment")
        );
        assert!(b.nearby.lock().ceremonies.is_empty(), "no code was shown");
        while let Ok(ev) = eb.try_recv() {
            assert!(
                !matches!(ev, MeshEvent::Pair(_)),
                "no ceremony event: {ev:?}"
            );
        }
        assert_eq!(failures(&b, &m), 1);
        let (reason, held) = hand_dial(&m, &b, None).await;
        assert_eq!(reason, None, "the challenge came");
        drop(held);
        eventually("the walked-away request counts", || failures(&b, &m) == 2).await;
    }

    /// N-S2: one ceremony per machine and `MAX_CEREMONIES` at once; a request over a cap is
    /// refused without a code and without counting.
    #[tokio::test]
    async fn ceremonies_are_capped_per_machine_and_per_window() {
        let lan = fake::Lan::default();
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        watch(&b, true).await.unwrap();
        let mut strangers = Vec::new();
        let mut held = Vec::new();
        for n in 0..MAX_CEREMONIES {
            let (s, d, _) = node_with(&format!("s{n}"), Some(Arc::new(lan.clone()))).await;
            let (reason, h) = hand_dial(&s, &b, None).await;
            assert_eq!(reason, None, "request {n} is taken");
            held.push(h);
            strangers.push((s, d));
        }
        let (again, _h) = hand_dial(&strangers[0].0, &b, None).await;
        assert_eq!(
            again.as_deref(),
            Some("a pairing with this machine is already open")
        );
        let (x, _dx, _) = node_with("x", Some(Arc::new(lan.clone()))).await;
        let (full, _h) = hand_dial(&x, &b, None).await;
        assert_eq!(full.as_deref(), Some("too many pairings at once"));
        assert_eq!(
            b.nearby.lock().failed,
            0,
            "refusals at a cap are not failures"
        );
    }

    /// N-S2: three failures refuse that machine for the window, but three strangers failing
    /// three times each cannot close the window: the honest pairing still runs.
    #[tokio::test]
    async fn three_strangers_cannot_close_an_honest_window() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        for n in 0..3 {
            let (s, _ds, _) = node_with(&format!("s{n}"), Some(Arc::new(lan.clone()))).await;
            for _ in 0..MAX_FAILURES {
                let (reason, _h) = hand_dial(&s, &b, Some("not n")).await;
                assert!(reason.is_some_and(|r| r.contains("did not match")));
            }
            let (blocked, _h) = hand_dial(&s, &b, Some("n")).await;
            assert_eq!(
                blocked.as_deref(),
                Some("too many failed pairings from this machine")
            );
        }
        assert_eq!(b.nearby.lock().failed, 9);
        let mut eb = b.nearby_events();
        let ca = pair(&a, &b.id).await.unwrap();
        let cb = pair_event(&mut eb, "waiting").await;
        answer(&b, cb.id, true).await.unwrap();
        answer(&a, ca.id, true).await.unwrap();
        pair_event(&mut eb, "paired").await;
    }

    /// N-S1: `mesh_nearby_watch(true)` is a lease. Renewed, the window stays (and keeps its
    /// counts); not renewed, as from a killed window, it stops advertising and accepting.
    #[tokio::test]
    async fn the_watch_lease_lapses_without_a_heartbeat() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        b.nearby.ttl_ms.store(400, Ordering::Relaxed);
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        b.nearby.lock().fail(&a.id.0);
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            watch(&b, true).await.unwrap();
        }
        assert_eq!(list(&a).len(), 1, "renewed past its TTL: still advertised");
        assert_eq!(
            b.nearby.lock().failed,
            1,
            "a renewal keeps the window's counts"
        );
        eventually("the lease lapsed", || list(&a).is_empty()).await;
        let (reason, _h) = hand_dial(&a, &b, Some("n")).await;
        assert_eq!(
            reason.as_deref(),
            Some("pairing is not open on that machine")
        );
    }

    /// N-S3: a ceremony's token pairs only the endpoint that ceremony is with.
    #[tokio::test]
    async fn a_ceremony_token_is_bound_to_its_endpoint() {
        let (b, _db, _) = node_with("b", Some(Arc::new(fake::Lan::default()))).await;
        let (x, y) = (
            iroh::SecretKey::generate().public(),
            iroh::SecretKey::generate().public(),
        );
        let id = uuid::Uuid::new_v4();
        let view = PairCeremony {
            id,
            peer: EngineId(x.to_string()),
            name: "x".into(),
            os_name: String::new(),
            addr: String::new(),
            code: String::new(),
            state: "accepted".into(),
            expires_ms: u64::MAX,
        };
        let ceremony = Ceremony {
            view,
            dialer: false,
            mine: true,
            theirs: true,
            token: Some("t".into()),
            vault_key: None,
            addrs: Vec::new(),
            pairing: false,
            out: mpsc::unbounded_channel().0,
        };
        b.nearby.lock().ceremonies.insert(id, ceremony);
        assert!(!take_token(&b, "t", &y), "another endpoint with the token");
        assert!(take_token(&b, "t", &x));
        assert!(!take_token(&b, "t", &x), "single use");
    }

    /// N-S4: only local advert addresses are kept, at most `MAX_ADDRS`; a long name is cut by
    /// bytes, so its TXT pair fits and Nearby starts.
    #[test]
    fn adverts_keep_local_addresses_and_names_fit_their_txt_pair() {
        let addrs: Vec<SocketAddr> = [
            "8.8.8.8:1",
            "192.168.1.2:1",
            "10.0.0.1:1",
            "169.254.1.1:1",
            "127.0.0.1:1",
            "[fe80::1]:1",
            "[fd00::1]:1",
            "[2001:db8::1]:1",
            "[::1]:1",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        let kept: Vec<String> = local_addrs(&addrs)
            .iter()
            .map(|a| a.ip().to_string())
            .collect();
        assert_eq!(
            kept,
            [
                "192.168.1.2",
                "10.0.0.1",
                "169.254.1.1",
                "127.0.0.1",
                "fe80::1",
                "fd00::1",
                "::1"
            ]
        );
        let many: Vec<SocketAddr> = (1..=20)
            .map(|p| SocketAddr::from(([10, 0, 0, 1], p)))
            .collect();
        assert_eq!(local_addrs(&many).len(), MAX_ADDRS);
        let name = cut(&"𝄞".repeat(100), NAME_MAX);
        assert!(name.len() <= NAME_MAX && format!("name={name}").len() <= 255);
        assert_eq!(cut("Sky's Mac", NAME_MAX), "Sky's Mac");
    }

    /// 183 R2: a machine this mesh unpaired, which never heard of it, still lists us as paired
    /// and drops our request; the dial says why and what to do, not "not open for pairing".
    #[tokio::test]
    async fn a_dial_to_a_machine_we_unpaired_says_why() {
        let lan = fake::Lan::default();
        let (a, _da, _) = node_with("a", Some(Arc::new(lan.clone()))).await;
        let (b, _db, _) = node_with("b", Some(Arc::new(lan.clone()))).await;
        b.add_peer(&a.pairing_string().await).await.unwrap();
        let mut row = a
            .store
            .peer_entries()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == b.id)
            .unwrap();
        row.removed_ms = Some(now_ms());
        a.store.put_peer(&row).await.unwrap();
        a.refresh_paired().await.unwrap();
        watch(&a, true).await.unwrap();
        watch(&b, true).await.unwrap();
        eventually("a lists b", || list(&a).iter().any(|e| e.id == b.id)).await;
        let e = pair(&a, &b.id).await.unwrap_err();
        assert!(
            e.starts_with("b was unpaired from this mesh; on b, remove this mesh first"),
            "{e}"
        );
    }
}
