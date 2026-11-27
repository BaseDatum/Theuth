//! The iroh side of the mesh (67 §4.1–§4.2): endpoint + router, the
//! `theuth/mesh/1` control protocol (pairing handshake, presence, peer-set
//! exchange), one connection per paired peer kept up with backoff, inbound
//! streams per ALPN, and the [`replica::Transport`] every other plugin uses.
//!
//! Connection direction: the lower endpoint id dials; the higher waits a
//! short grace and dials only if nothing arrived (both directions are
//! accepted, so a wrong guess costs one extra connection, never a gap).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Context as _;
use data_encoding::BASE32_NOPAD;
use iroh::endpoint::{
    ConnectOptions, Connection, QuicTransportConfig, RecvStream, SendStream, presets,
};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
};
use iroh_gossip::net::Gossip;
use replica::{ByteStream, PeerEvent, PeerId, Transport};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::identity::now_ms;
use crate::pairing::{self, Payload, PeerEntry, Pending, Rank, Side};
use crate::store::Store;
use crate::types::{AlpnTraffic, EngineId, HubPref, Peer};

use secrets::Vault;

/// Stream ALPN: mesh control — pairing handshake, presence, peer-set exchange.
/// Not in the contract's five: gossip owns `ALPN_GOSSIP` end to end.
pub const ALPN_MESH: &str = "theuth/mesh/1";
/// 183 §7.2 (W-21): A2A between contacts (separate meshes). Not a stream ALPN: its own
/// handler ([`A2aGate`]) refuses family peers, then asks the installed [`ContactAlpn`] (the
/// a2a plugin) whether the remote is a live contact or a stranger inside an open window.
pub const ALPN_A2A: &str = "theuth/a2a/1";
/// The close reason of a `theuth/a2a/1` connection the gate refused (not a contact, or no
/// longer one): the request never reached the a2a router, so the dialer may try elsewhere.
pub const A2A_NOT_A_CONTACT: &[u8] = b"not a contact";
/// Live inbound `theuth/a2a/1` connections of one remote id.
const A2A_CONNS_MAX: usize = 4;
/// Concurrent `theuth/a2a/1` streams of one remote id (all its connections together).
const A2A_STREAMS_MAX: usize = 16;
/// Direct addresses remembered for one contact.
const A2A_ADDRS_MAX: usize = 8;

/// 183 §7.2: what the a2a plugin plugs into `theuth/a2a/1`. Without one every connection
/// is refused.
pub trait ContactAlpn: Send + Sync + 'static {
    /// Admit a connection (and, again, each stream) from `remote` (endpoint id text): a
    /// live contact, or a stranger while an Add contact window is open.
    fn admit(&self, remote: &str) -> bool;
    /// Serve one bidirectional stream of an admitted `remote`.
    fn stream(
        &self,
        remote: String,
        stream: Box<dyn ByteStream>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
    /// 183 §7.2: the contact's last known direct addresses (from its last live connection,
    /// kept in `peers.yaml`), dialled beside any hints the caller has: a LAN-only pair is
    /// reachable after a restart without discovery or a relay.
    fn hints(&self, _remote: &str) -> Vec<std::net::SocketAddr> {
        Vec::new()
    }
}

/// `mesh.relay` value meaning n0's public relays.
pub const RELAY_PUBLIC: &str = "public";

/// Largest control or pairing message.
const MAX_MSG: u32 = 1 << 20;
/// Dial timeout.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 183 R1 (S1): a row an engine introduced this long before its unpair, or any time after,
/// is quarantined (clock skew between the remover and this engine).
pub(crate) const QUARANTINE_SKEW_MS: u64 = 5 * 60_000;
/// Reconnect backoff bounds.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Minimum duration a control connection must stay alive before its disconnect
/// resets reconnect backoff; an immediate close (e.g. `not paired`) escalates backoff.
const MIN_HEALTHY_CONN: Duration = Duration::from_secs(10);
/// The higher id waits this long for the lower id's dial before dialing itself.
const DIAL_GRACE: Duration = Duration::from_secs(3);
/// Inbound stream queue per ALPN.
const INBOUND_QUEUE: usize = 64;

/// 183 R1 (S7): key material in a frame: the plain string on the wire, `<redacted>` in
/// `Debug` (the `{other:?}` logs), zeroized on drop.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Secret(zeroize::Zeroizing<String>);

impl Secret {
    pub(crate) fn new(s: String) -> Self {
        Self(zeroize::Zeroizing::new(s))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Messages on `ALPN_MESH` streams (`u32 LE len ‖ JSON`).
///
/// The vault key rides in `PairHello` only inside the already-authenticated
/// QUIC stream: `ALPN_MESH` is TLS keyed to both engines' identities (the
/// dialer checked `remote_id` against the pairing string, the acceptor checks
/// the one-time token before reading anything else), so the key is seen by
/// exactly the engine the string named. Unknown fields are ignored on read,
/// so a pre-F5 `t` on either end still pairs (it never adopts, never ranks).
/// A frame an older engine cannot decode is logged and ignored there
/// ([`Net::drive`]), so 183's variants are appended and safe toward old peers.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Msg {
    /// Dialer of `t mesh add`: prove the one-time token, introduce yourself.
    PairHello {
        token: String,
        me: PeerEntry,
        peers: Vec<PeerEntry>,
        /// The dialer's mesh vault key (base32); the acceptor adopts it when
        /// [`pairing::key_owner`] says the dialer owns the key (79 F5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vault_key: Option<Secret>,
        /// The dialer's standing; `None` from a pre-F5 `t`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rank: Option<Rank>,
        /// 183 §6.9.9: the current key plus every unretired one; empty from a pre-183 `t`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        keyring: Vec<secrets::WireKey>,
    },
    /// Inviter's answer: token echoed, its own row and peer set.
    PairAck {
        token: String,
        me: PeerEntry,
        peers: Vec<PeerEntry>,
        /// The acceptor took the dialer's key.
        #[serde(default)]
        adopted: bool,
        /// The acceptor's standing; `None` from a pre-F5 `t`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rank: Option<Rank>,
        /// 183 §6.9.9: as `PairHello.keyring`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        keyring: Vec<secrets::WireKey>,
    },
    /// Inviter's refusal.
    PairReject { reason: String },
    /// Peer-set exchange between paired engines (on connect and on change).
    Peers { peers: Vec<PeerEntry> },
    /// 183 §6.8: a stranger's Nearby pairing request (first frame, instead of `PairHello`),
    /// accepted only while this engine's pairing window is open. `commit`: base32
    /// `SHA256(nonce_a)`, the dialer's nonce committed before it sees the acceptor's.
    PairRequest {
        name: String,
        os: String,
        commit: String,
    },
    /// 183 §6.8: the acceptor's nonce (16 random bytes, base32).
    PairChallenge { nonce: String },
    /// 183 §6.8: the dialer opens its commitment. The acceptor checks it before it shows a code
    /// or counts a press, so neither side can pick its nonce after seeing the other's.
    PairReveal { nonce: String },
    /// 183 §6.8: one user pressed Pair (`accept`) or Cancel. The acceptor sends its accept only
    /// once both sides pressed Pair, carrying the freshly minted token the dialer's `PairHello`
    /// then presents and its vault key (what a pairing string carries).
    PairAnswer {
        accept: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vault_key: Option<Secret>,
    },
    /// 183 §6.9.7: right after `Peers` on every connect, both ways: my current key and every key id I hold.
    KeyEpoch {
        current: secrets::KeyInfo,
        held: Vec<String>,
    },
    /// 183 §6.9.7: send me key `id`.
    KeyRequest { id: String },
    /// 183 §6.9.5: a key, with the rotation's tombstones, applied before the key is stored.
    KeyGrant {
        key: secrets::WireKey,
        removed: Vec<PeerEntry>,
    },
    /// 183 §6.9.5: key `id` is stored here (display only).
    KeyAck { id: String },
    /// 183 §6.9.5: `KeyRequest` refused (a tombstoned asker, an unknown id).
    KeyRefused { id: String, reason: String },
    /// 184 §8.1: this engine's docs schema ([`crate::DOCS_SCHEMA`]) and its `homes_on`, sent on
    /// connect (beside `keys::on_connect`) and whenever `homes_on` changes; the receiver records it
    /// under the connection's authenticated `remote_id`. Appended: an older engine ignores it.
    DocsSchema { schema: u32, homes_on: bool },
}

/// How to bind (production from `MeshConfig`; tests on loopback, no relay).
pub struct BindOpts {
    /// This engine's key.
    pub secret: SecretKey,
    /// `mesh.relay`: URL, `public`, or empty for none.
    pub relay: String,
    /// DNS/pkarr + mDNS lookup (off in loopback tests). Started once there is
    /// a paired peer, never for a lone engine ([`Net::join_network`]).
    pub discovery: bool,
    /// Explicit socket to bind (tests: `127.0.0.1:0`).
    pub bind_addr: Option<std::net::SocketAddr>,
    /// `mesh.storm_frames_per_sec`: one ALPN above this for 10 s is warned about.
    pub storm_frames_per_sec: u64,
    /// Identity creation (`identity.rs`); the age half of [`Rank`].
    pub first_seen_ms: u64,
    /// The vault whose mesh keys pairing and rotation move; `None` = the secrets plugin's,
    /// handed over by [`Net::set_vault`] once it is up (tests give each node its own).
    pub vault: Option<Arc<Vault>>,
    /// 183 §6.8: the Nearby advertise + browse; `None` = real mDNS (tests pass a fake).
    pub mdns: Option<Arc<dyn crate::nearby::Mdns>>,
}

/// Frames = one `poll_read` that returned bytes, or one `poll_write`. Not
/// protocol messages: a sampler on top of these needs no knowledge of what
/// rides on the ALPN.
#[derive(Default)]
struct Counters {
    frames_in: AtomicU64,
    frames_out: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    sampler: Mutex<Sampler>,
}

/// Rate over 1 s windows, driven by the traffic itself (a storm keeps it
/// running; silence needs no timer). Warns once the rate has stayed above
/// the limit for [`STORM_SUSTAIN`], at most every [`STORM_WARN_EVERY`].
#[derive(Default)]
struct Sampler {
    window_start_ms: u64,
    frames_at_start: u64,
    over_since_ms: Option<u64>,
    last_warn_ms: u64,
}

/// A rate above the limit for this long is a storm.
const STORM_SUSTAIN: Duration = Duration::from_secs(10);
/// Storm warnings are logged at most this often per ALPN.
const STORM_WARN_EVERY: Duration = Duration::from_secs(60);

impl Sampler {
    /// One more frame at `now_ms` with `total` frames so far; `Some(rate)` when
    /// this crossed into a warning.
    fn sample(&mut self, now_ms: u64, total: u64, limit: u64) -> Option<u64> {
        if self.window_start_ms == 0 {
            (self.window_start_ms, self.frames_at_start) = (now_ms, total);
            return None;
        }
        let elapsed = now_ms.saturating_sub(self.window_start_ms);
        if elapsed < 1000 {
            return None;
        }
        let rate = total.saturating_sub(self.frames_at_start) * 1000 / elapsed;
        let window_start = self.window_start_ms;
        (self.window_start_ms, self.frames_at_start) = (now_ms, total);
        if rate <= limit {
            self.over_since_ms = None;
            return None;
        }
        let since = *self.over_since_ms.get_or_insert(window_start);
        if now_ms.saturating_sub(since) >= STORM_SUSTAIN.as_millis() as u64
            && (self.last_warn_ms == 0
                || now_ms.saturating_sub(self.last_warn_ms) >= STORM_WARN_EVERY.as_millis() as u64)
        {
            self.last_warn_ms = now_ms;
            return Some(rate);
        }
        None
    }
}

impl Counters {
    fn frame(&self, alpn: &str, limit: u64) {
        let total =
            self.frames_in.load(Ordering::Relaxed) + self.frames_out.load(Ordering::Relaxed);
        let warn = self
            .sampler
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .sample(now_ms(), total, limit);
        if let Some(rate) = warn {
            tracing::warn!("mesh: {alpn} at {rate} frames/s for 10 s (limit {limit}): storm?");
        }
    }
}

/// A stream whose reads and writes feed one ALPN's [`Counters`].
struct Counted {
    inner: Box<dyn ByteStream>,
    counters: Arc<Counters>,
    alpn: String,
    limit: u64,
    /// 183 R3: the other end, and the live paired set: once it leaves, every read and write
    /// on this stream fails, so no shipper's in-flight batch reaches (or comes from) it.
    peer: EndpointId,
    paired: Arc<RwLock<HashSet<EndpointId>>>,
}

impl Counted {
    fn unpaired(&self) -> Option<std::io::Error> {
        let gone = !self
            .paired
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&self.peer);
        gone.then(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "peer is no longer paired")
        })
    }
}

impl tokio::io::AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(e) = self.unpaired() {
            return Poll::Ready(Err(e));
        }
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            let n = buf.filled().len() - before;
            if n > 0 {
                self.counters.frames_in.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .bytes_in
                    .fetch_add(n as u64, Ordering::Relaxed);
                self.counters.frame(&self.alpn, self.limit);
            }
        }
        r
    }
}

impl tokio::io::AsyncWrite for Counted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(e) = self.unpaired() {
            return Poll::Ready(Err(e));
        }
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &r {
            self.counters.frames_out.fetch_add(1, Ordering::Relaxed);
            self.counters
                .bytes_out
                .fetch_add(*n as u64, Ordering::Relaxed);
            self.counters.frame(&self.alpn, self.limit);
        }
        r
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

type InboundItem = (PeerId, Box<dyn ByteStream>);

/// One paired peer's live control connections.
#[derive(Default)]
struct Presence {
    conns: Vec<Connection>,
    since_ms: u64,
}

/// 184 §8.1: a control connection silent this long after connect is a pre-184 build (`schema` 0).
const DOCS_SILENT: Duration = Duration::from_secs(5);
/// 184 §8.1: `peer_docs_seen.heard_ms` of a connected peer is refreshed at most this often.
const DOCS_TOUCH_MS: u64 = 60_000;

/// 184 §8.1: the docs gate's live half (`Net.docs`); the floor itself is in `peer_docs_seen`.
pub(crate) struct DocsLive {
    /// What this engine advertises in its `DocsSchema` frame.
    homes_on: AtomicBool,
    /// Per peer, its last frame's `homes_on` and the live control connections (`stable_id`) that
    /// sent a frame; a peer drops out with the last of them.
    frames: Mutex<HashMap<EndpointId, Frames>>,
    touches: Mutex<crate::store::Touches>,
    /// Poked on a frame, a silence, a connect or a disconnect: the gate pass runs at once.
    pub(crate) changed: tokio::sync::Notify,
    /// Serialises [`Net::docs_gate`].
    pass: tokio::sync::Mutex<()>,
    /// Z184-R2: a frame's record and a silence's check-then-record run one at a time, so a
    /// frame landing during the silence check is never overwritten by its `schema 0`.
    record: tokio::sync::Mutex<()>,
}

/// [`DocsLive::frames`] of one peer.
#[derive(Default)]
struct Frames {
    conns: HashSet<usize>,
    homes_on: bool,
}

impl Default for DocsLive {
    fn default() -> Self {
        Self {
            homes_on: AtomicBool::new(false),
            frames: Mutex::new(HashMap::new()),
            touches: Mutex::new(crate::store::Touches::new(DOCS_TOUCH_MS)),
            changed: tokio::sync::Notify::new(),
            pass: tokio::sync::Mutex::new(()),
            record: tokio::sync::Mutex::new(()),
        }
    }
}

/// Everything iroh: shared by the actor, the connector tasks and the handlers.
pub struct Net {
    /// The endpoint (one per process, 67 §4.2).
    pub endpoint: Endpoint,
    router: Router,
    /// Lease heartbeats ride on this (67 §4.3).
    pub gossip: Gossip,
    /// This engine.
    pub id: EngineId,
    pub(crate) store: Store,
    /// This engine's display name (`mesh.name`, default the hostname); `mesh_rename` changes it.
    name: RwLock<String>,
    relay: String,
    /// Relays to insert on `join_network` (empty = `mesh.relay: none`).
    relays: RelayMap,
    /// `mesh.discovery`.
    discovery: bool,
    /// `join_network` ran.
    joined: AtomicBool,
    /// `relays` are in the endpoint's relay map (`join_network`, or 210 `dial_alpn` without it).
    relays_in: AtomicBool,
    /// 210 §5.7: tombstoned engines as this engine's paired set drops them ([`Net::subscribe_removals`]).
    removals: broadcast::Sender<EngineId>,
    hub_pref: Mutex<HubPref>,
    pending: Mutex<Option<Pending>>,
    /// 224: single-use remote tickets (cloud nodes), each with its own TTL; separate from the
    /// Settings pairing slot. Pruned when expired.
    remote: Mutex<Vec<Pending>>,
    paired: Arc<RwLock<HashSet<EndpointId>>>,
    /// 183 R3: inbound connections on the stream ALPNs and gossip, by peer (closed on unpair).
    inbound_conns: Mutex<HashMap<EndpointId, Vec<Connection>>>,
    /// P2-F1: serialises `refresh_paired` (store read → set write).
    refreshing: tokio::sync::Mutex<()>,
    presence: Mutex<HashMap<EndpointId, Presence>>,
    outbound: tokio::sync::Mutex<HashMap<(EndpointId, String), Connection>>,
    events: broadcast::Sender<PeerEvent>,
    /// Bumped on every peer-set change; the connector and the lease loop watch it.
    pub peers_gen: watch::Sender<u64>,
    inbound: HashMap<
        &'static str,
        (
            mpsc::Sender<InboundItem>,
            tokio::sync::Mutex<mpsc::Receiver<InboundItem>>,
        ),
    >,
    /// Per ALPN, what its streams moved (69 audit).
    traffic: Mutex<HashMap<String, Arc<Counters>>>,
    storm_frames_per_sec: u64,
    first_seen_ms: u64,
    /// 183 §6.9: the vault ([`Net::vault`]); set at bind in tests, by the plugin otherwise.
    vault: std::sync::OnceLock<Arc<Vault>>,
    /// 183 §6.9: what each peer said of its keys, the member rule, the asks.
    pub(crate) keyring: crate::keys::State,
    /// 183 §6.8: the Nearby window, its sightings and the pairing ceremonies.
    pub(crate) nearby: crate::nearby::State,
    /// 183 §7.2: the a2a plugin's `theuth/a2a/1` handler, once installed.
    contacts: std::sync::OnceLock<Arc<dyn ContactAlpn>>,
    /// 183 §7.2: `theuth/a2a/1` connections by contact, both directions (dialed ones are
    /// reused; Remove or Block closes them all).
    a2a_conns: Mutex<HashMap<EndpointId, Vec<Connection>>>,
    /// 183 §7.2: each remote's shared `theuth/a2a/1` stream budget.
    a2a_stream_budgets: Mutex<HashMap<EndpointId, Arc<tokio::sync::Semaphore>>>,
    /// 184 §8.1: the docs gate's frames and what this engine advertises.
    pub(crate) docs: DocsLive,
    /// Dev only (debug builds on a loopback bind; `None` in release): the file
    /// `THEUTH_DEV_MESH_DROP_FILE` names, whose lines are endpoint ids this engine drops
    /// ([`Net::dropped`]) — a scratch-rig partition without a firewall rule (184 §15.3 check 12).
    dev_drop: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Every ALPN whose inbound streams are handed out through [`Transport::accept`].
const STREAM_ALPNS: [&str; 10] = [
    replica::ALPN_RECORDS,
    replica::ALPN_ACP,
    replica::ALPN_SESSIONS,
    replica::ALPN_KNOWLEDGE,
    replica::ALPN_RPC,
    replica::ALPN_ARTIFACTS,
    replica::ALPN_DOCS,
    replica::ALPN_DOUBLE,
    replica::ALPN_PTY,
    replica::ALPN_DESKTOP,
];

impl Net {
    /// Bind the endpoint, spawn the router and gossip, load the paired set.
    pub async fn bind(
        opts: BindOpts,
        store: Store,
        name: String,
        hub_pref: HubPref,
    ) -> anyhow::Result<Arc<Self>> {
        // The relays this engine will use once it has a peer (`join_network`);
        // until then the endpoint binds with an empty relay map so a lone
        // engine dials nothing. `Disabled` would drop the relay transport for good.
        let relays = match opts.relay.trim() {
            "" | "none" | "off" => RelayMap::empty(),
            RELAY_PUBLIC => RelayMode::Default.relay_map(),
            url => RelayMap::from(
                RelayUrl::from_str(url).with_context(|| format!("mesh.relay: {url}"))?,
            ),
        };
        let relay_mode = if relays.is_empty() {
            RelayMode::Disabled
        } else {
            RelayMode::Custom(RelayMap::empty())
        };
        let mut alpns: Vec<Vec<u8>> = STREAM_ALPNS.iter().map(|a| a.as_bytes().to_vec()).collect();
        alpns.push(ALPN_MESH.as_bytes().to_vec());
        alpns.push(replica::ALPN_GOSSIP.as_bytes().to_vec());
        // 183 §7.2: always registered; [`A2aGate`] refuses everyone until the a2a plugin
        // installs its handler, and then everyone but contacts and an open window's strangers.
        alpns.push(ALPN_A2A.as_bytes().to_vec());
        let builder = Endpoint::builder(presets::Minimal)
            .secret_key(opts.secret)
            .alpns(alpns)
            .relay_mode(relay_mode);
        let builder = match opts.bind_addr {
            Some(addr) => builder.bind_addr(addr)?,
            None => builder,
        };
        let endpoint = builder.bind().await.context("bind iroh endpoint")?;
        let id = EngineId(endpoint.id().to_string());
        let paired: HashSet<EndpointId> = store
            .peers()
            .await?
            .iter()
            .filter_map(|p| EndpointId::from_str(&p.id.0).ok())
            .collect();

        let gossip = Gossip::builder()
            .alpn(replica::ALPN_GOSSIP)
            .spawn(endpoint.clone());
        let (events, _) = broadcast::channel(256);
        let (peers_gen, _) = watch::channel(0);
        let mut inbound = HashMap::new();
        for alpn in STREAM_ALPNS {
            let (tx, rx) = mpsc::channel(INBOUND_QUEUE);
            inbound.insert(alpn, (tx, tokio::sync::Mutex::new(rx)));
        }

        // The router needs handlers that hold the Net; build with a placeholder
        // router, then swap in the real one (Router is cheap and clonable).
        let net = Arc::new_cyclic(|weak: &std::sync::Weak<Net>| {
            let mut rb = Router::builder(endpoint.clone())
                .accept(ALPN_MESH, Control(weak.clone()))
                .accept(ALPN_A2A, A2aGate { net: weak.clone() })
                .accept(
                    replica::ALPN_GOSSIP,
                    Gated {
                        net: weak.clone(),
                        inner: gossip.clone(),
                    },
                );
            for alpn in STREAM_ALPNS {
                rb = rb.accept(
                    alpn,
                    Inbound {
                        net: weak.clone(),
                        alpn,
                    },
                );
            }
            Net {
                endpoint: endpoint.clone(),
                router: rb.spawn(),
                gossip: gossip.clone(),
                id,
                store,
                name: RwLock::new(name),
                relay: opts.relay,
                relays,
                discovery: opts.discovery,
                joined: AtomicBool::new(false),
                relays_in: AtomicBool::new(false),
                removals: broadcast::channel(64).0,
                hub_pref: Mutex::new(hub_pref),
                pending: Mutex::new(None),
                remote: Mutex::new(Vec::new()),
                paired: Arc::new(RwLock::new(paired)),
                inbound_conns: Mutex::new(HashMap::new()),
                refreshing: tokio::sync::Mutex::new(()),
                presence: Mutex::new(HashMap::new()),
                outbound: tokio::sync::Mutex::new(HashMap::new()),
                events,
                peers_gen,
                inbound,
                traffic: Mutex::new(HashMap::new()),
                storm_frames_per_sec: opts.storm_frames_per_sec,
                first_seen_ms: opts.first_seen_ms,
                vault: std::sync::OnceLock::new(),
                keyring: crate::keys::State::default(),
                nearby: crate::nearby::State::new(opts.mdns),
                contacts: std::sync::OnceLock::new(),
                a2a_conns: Mutex::new(HashMap::new()),
                a2a_stream_budgets: Mutex::new(HashMap::new()),
                docs: DocsLive::default(),
                dev_drop: dev_drop_file(opts.bind_addr),
            }
        });
        if let Some(v) = opts.vault {
            net.set_vault(v);
        }
        // 183 §7.2: an engine with contacts but no peers joins once the a2a plugin asks
        // ([`Net::join_network`] is idempotent).
        if !net.paired_ids().is_empty() {
            net.join_network().await;
        }
        // Dev only ([`Net::dropped`]): live connections of a dropped peer close within a second,
        // whichever side dialed (a connector inside `drive` never reaches its wait loop).
        if net.dev_drop.is_some() {
            let weak = Arc::downgrade(&net);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let Some(net) = weak.upgrade() else { return };
                    for peer in net.paired_ids() {
                        if net.dropped(&peer) && net.is_connected(&peer) {
                            net.close_all(&peer, || false).await;
                        }
                    }
                }
            });
        }
        Ok(net)
    }

    /// 183 §6.9: the vault pairing and rotation move keys in; the first call wins.
    pub fn set_vault(&self, vault: Arc<Vault>) {
        vault.set_engine(&self.id.0);
        let _ = self.vault.set(vault);
    }

    /// The vault, once [`Net::set_vault`] ran; `None` without the secrets plugin.
    pub fn vault(&self) -> Option<Arc<Vault>> {
        self.vault.get().cloned()
    }

    /// 183 §6.9: one live control connection per connected peer.
    pub(crate) fn control_conns(&self) -> Vec<(EndpointId, Connection)> {
        Self::lock(&self.presence)
            .iter()
            .filter_map(|(id, p)| Some((*id, p.conns.first()?.clone())))
            .collect()
    }

    /// 183 §6.9: a live control connection to `peer`.
    pub(crate) fn control_conn(&self, peer: &EndpointId) -> Option<Connection> {
        Self::lock(&self.presence)
            .get(peer)
            .and_then(|p| p.conns.first().cloned())
    }

    /// 183 §6.9.5, R3: close every connection of an engine that left the paired set (an
    /// unpair here, a merged tombstone, one the replica wrote).
    pub(crate) async fn close_conns(&self, peer: &EndpointId) {
        if self.is_paired(peer) {
            return;
        }
        self.close_all(peer, || self.is_paired(peer)).await;
    }

    /// Close every connection of `peer`; `keep()` (checked once the outbound cache is held)
    /// keeps the dials made since.
    async fn close_all(&self, peer: &EndpointId, keep: impl Fn() -> bool) {
        // 183 R3: every ALPN, both directions: the control connection, every connection it
        // opened to us, and our cached dials (records, sessions, docs, acp, rpc, …).
        // P2-F1: presence and inbound first; `outbound` is held across a dial (up to 30 s).
        let mut conns = Self::lock(&self.presence)
            .get(peer)
            .map(|p| p.conns.clone())
            .unwrap_or_default();
        conns.extend(
            Self::lock(&self.inbound_conns)
                .remove(peer)
                .unwrap_or_default(),
        );
        for c in conns.drain(..) {
            c.close(0u32.into(), b"removed");
        }
        let mut out = self.outbound.lock().await;
        // A re-pair during the wait keeps the connections it dialed since.
        if keep() {
            return;
        }
        out.retain(|(id, _), c| {
            let keep = id != peer;
            if !keep {
                c.close(0u32.into(), b"removed");
            }
            keep
        });
    }

    /// Dev only: this engine drops `peer`'s traffic (`THEUTH_DEV_MESH_DROP_FILE` lists it):
    /// no dial, every accept closed, live connections closed. Always `false` in release.
    pub(crate) fn dropped(&self, peer: &EndpointId) -> bool {
        self.dev_drop.as_ref().is_some_and(|f| {
            let id = peer.to_string();
            std::fs::read_to_string(f).is_ok_and(|s| s.lines().any(|l| l.trim() == id))
        })
    }

    /// 183 R3: remember an inbound connection (stream ALPNs, gossip) so an unpair closes it.
    fn track_inbound(&self, peer: EndpointId, conn: &Connection) {
        let mut m = Self::lock(&self.inbound_conns);
        let list = m.entry(peer).or_default();
        list.retain(|c| c.close_reason().is_none());
        list.push(conn.clone());
    }

    /// Close everything (lease release is the caller's job).
    pub async fn shutdown(&self) {
        let _ = self.gossip.shutdown().await;
        let _ = self.router.shutdown().await;
        self.endpoint.close().await;
    }

    /// Connect to the relay and start DNS/pkarr + mDNS lookup (67 §4.2) —
    /// once, and only when there is a peer to find or be found by: on bind
    /// with a stored peer set, on pairing (either side). A lone engine has
    /// neither, and offline the relay probes and publishers would only log
    /// a failure every few seconds. Pairing carries direct addresses, so a
    /// LAN pair needs no relay; `pairing_string` joins first so the string
    /// carries a relay address for the cross-network case.
    pub async fn join_network(&self) {
        if self.joined.swap(true, Ordering::Relaxed) {
            return;
        }
        self.insert_relays().await;
        if self.discovery
            && let Ok(services) = self.endpoint.address_lookup()
        {
            use iroh::address_lookup::{
                AddressLookupBuilder, DnsAddressLookup, PkarrPublisher, PkarrResolver,
            };
            match PkarrPublisher::n0_dns().into_address_lookup(&self.endpoint) {
                Ok(s) => services.add(s),
                Err(e) => tracing::warn!("mesh: pkarr publisher unavailable: {e}"),
            }
            match PkarrResolver::n0_dns().into_address_lookup(&self.endpoint) {
                Ok(s) => services.add(s),
                Err(e) => tracing::warn!("mesh: pkarr resolver unavailable: {e}"),
            }
            match DnsAddressLookup::n0_dns().into_address_lookup(&self.endpoint) {
                Ok(s) => services.add(s),
                Err(e) => tracing::warn!("mesh: DNS lookup unavailable: {e}"),
            }
            match iroh_mdns_address_lookup::MdnsAddressLookup::builder().build(self.endpoint.id()) {
                Ok(mdns) => services.add(mdns),
                Err(e) => tracing::warn!("mesh: mDNS discovery unavailable: {e}"),
            }
        }
        tracing::info!(
            "mesh: joined the network (relay {}, discovery {})",
            self.relay,
            self.discovery
        );
    }

    /// `mesh.relay`'s relays into the endpoint's relay map, once ([`Net::join_network`],
    /// [`Net::dial_alpn`]).
    async fn insert_relays(&self) {
        if self.relays_in.swap(true, Ordering::Relaxed) {
            return;
        }
        for cfg in self.relays.relays::<Vec<_>>() {
            self.endpoint.insert_relay(cfg.url.clone(), cfg).await;
        }
    }

    /// 210 D18 (§5.3): one connection to `addr` on a non-mesh ALPN (`theuth/share/1`) from this
    /// engine's endpoint, with a per-connection `transport` config (the share client allows the
    /// streams the edge opens; the endpoint's own config is untouched). Unlike
    /// [`Net::join_network`] it publishes nothing (no pkarr, n0 DNS or mDNS): it inserts only
    /// `mesh.relay`'s relays and dials `addr`'s own relay URL and direct addresses (a relay URL
    /// outside the map is dialed as a non-home relay). The caller owns the connection: no
    /// tracking, reuse, inbound handler or paired-peer check.
    // TODO(210-M4): offer the previous major as well (`ConnectOptions::with_additional_alpns`)
    // once `share_proto::ALPNS` holds two.
    pub async fn dial_alpn(
        &self,
        addr: EndpointAddr,
        alpn: &[u8],
        transport: QuicTransportConfig,
    ) -> anyhow::Result<Connection> {
        self.insert_relays().await;
        let id = addr.id;
        let opts = ConnectOptions::new().with_transport_config(transport);
        let dial = async {
            let connecting = self.endpoint.connect_with_opts(addr, alpn, opts).await?;
            anyhow::Ok(connecting.await?)
        };
        tokio::time::timeout(CONNECT_TIMEOUT, dial)
            .await
            .context("dial timed out")?
            .with_context(|| {
                format!(
                    "dial {} on {}",
                    id.fmt_short(),
                    String::from_utf8_lossy(alpn)
                )
            })
    }

    // --- small state -----------------------------------------------------

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 210 §5.7: engines that left this engine's paired set because they were tombstoned (an
    /// unpair here or a replicated one), not quarantined. Only drops seen while running: a
    /// subscriber reconciles against [`crate::RemovedPeers`] at start.
    pub fn subscribe_removals(&self) -> broadcast::Receiver<EngineId> {
        self.removals.subscribe()
    }

    /// Connect/disconnect events (the [`Transport::subscribe`] channel).
    pub fn subscribe_peers(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }

    /// Advertised lease preference (goes into this engine's `PeerEntry`).
    pub fn set_hub_pref(&self, p: HubPref) {
        *Self::lock(&self.hub_pref) = p;
    }

    /// Is `id` a live (not removed) peer?
    pub fn is_paired(&self, id: &EndpointId) -> bool {
        self.paired
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(id)
    }

    /// Paired ids (for gossip bootstrap).
    pub fn paired_ids(&self) -> Vec<EndpointId> {
        self.paired
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .copied()
            .collect()
    }

    /// 191 (D1): this mesh's current membership as a contact roster lists it: the live peers
    /// (neither removed nor quarantined, as [`Self::paired_ids`]) plus this engine, each with
    /// its `added_ms` (this engine's from the row other machines wrote about it, else 0).
    pub async fn members(&self) -> anyhow::Result<Vec<(String, u64)>> {
        let me = self.store.self_added_ms().await?;
        let mut out: Vec<(String, u64)> = self
            .store
            .peers()
            .await?
            .into_iter()
            .map(|p| (p.id.0, p.added_ms))
            .collect();
        out.push((self.id.0.clone(), me.unwrap_or(0)));
        Ok(out)
    }

    /// 183 §7.2: install the a2a plugin's `theuth/a2a/1` handler (the first call wins).
    pub fn set_contacts(&self, handler: Arc<dyn ContactAlpn>) {
        let _ = self.contacts.set(handler);
    }

    /// 183 §7.2: one bidirectional `theuth/a2a/1` stream to `eid` (a contact, or the inviter
    /// of a ceremony), over a cached connection when one is open; `addrs` and `relay` are
    /// dial hints (a ticket's, a sighting's). A machine of this mesh is refused: family
    /// talks over `theuth/rpc/1`.
    pub async fn a2a_stream(
        &self,
        eid: &str,
        addrs: &[std::net::SocketAddr],
        relay: Option<&str>,
    ) -> anyhow::Result<Box<dyn ByteStream>> {
        let id = EndpointId::from_str(eid).with_context(|| format!("endpoint id {eid}"))?;
        if self.is_paired(&id) {
            anyhow::bail!(
                "{} is a machine of this mesh, not a contact",
                id.fmt_short()
            );
        }
        let cached = Self::lock(&self.a2a_conns)
            .get(&id)
            .and_then(|l| l.iter().find(|c| c.close_reason().is_none()).cloned());
        if let Some(conn) = cached
            && let Ok((send, recv)) = conn.open_bi().await
        {
            return Ok(Box::new(tokio::io::join(recv, send)));
        }
        let stored = self
            .contacts
            .get()
            .map(|h| h.hints(eid))
            .unwrap_or_default();
        let mut set: std::collections::BTreeSet<TransportAddr> = addrs
            .iter()
            .chain(&stored)
            .map(|a| TransportAddr::Ip(*a))
            .collect();
        if let Some(url) = relay.and_then(|r| RelayUrl::from_str(r).ok()) {
            set.insert(TransportAddr::Relay(url));
        }
        let addr = EndpointAddr { id, addrs: set };
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(addr, ALPN_A2A.as_bytes()),
        )
        .await
        .context("dial timed out")?
        .with_context(|| format!("dial {} on {ALPN_A2A}", id.fmt_short()))?;
        // A dialed connection is ours: it is tracked (for reuse and Remove) whatever the cap.
        let _ = self.track_a2a(id, &conn);
        if let Some(handler) = self.contacts.get().cloned() {
            tokio::spawn(serve_a2a(
                self.paired.clone(),
                self.a2a_streams(id),
                handler,
                conn.clone(),
            ));
        }
        let (send, recv) = conn.open_bi().await.context("open stream")?;
        Ok(Box::new(tokio::io::join(recv, send)))
    }

    /// 183 §7.3: how the contact `eid` is reached right now (its live `theuth/a2a/1`
    /// connections, either direction).
    pub fn a2a_path(&self, eid: &str) -> crate::PeerPath {
        let Ok(id) = EndpointId::from_str(eid) else {
            return crate::PeerPath::Offline;
        };
        Self::lock(&self.a2a_conns)
            .get(&id)
            .and_then(|l| l.iter().find(|c| c.close_reason().is_none()))
            .map_or(crate::PeerPath::Offline, |c| path_of(c).0)
    }

    /// 183 §7.2: the direct addresses `eid` answers on over its live `theuth/a2a/1`
    /// connections (either direction; relay paths left out), to remember for the next dial.
    pub fn a2a_direct_addrs(&self, eid: &str) -> Vec<std::net::SocketAddr> {
        let Ok(id) = EndpointId::from_str(eid) else {
            return Vec::new();
        };
        let mut out = std::collections::BTreeSet::new();
        for c in Self::lock(&self.a2a_conns).get(&id).into_iter().flatten() {
            if c.close_reason().is_some() {
                continue;
            }
            for p in c.paths().iter() {
                if let TransportAddr::Ip(s) = p.remote_addr() {
                    out.insert(*s);
                }
            }
        }
        out.into_iter().take(A2A_ADDRS_MAX).collect()
    }

    /// 183 §7.4 Revocation: close every `theuth/a2a/1` connection of `eid` (Remove, Block).
    pub fn close_a2a(&self, eid: &str) {
        let Ok(id) = EndpointId::from_str(eid) else {
            return;
        };
        for c in Self::lock(&self.a2a_conns).remove(&id).unwrap_or_default() {
            c.close(0u32.into(), A2A_NOT_A_CONTACT);
        }
    }

    /// This engine's direct addresses and home relay, for an invite ticket.
    pub fn a2a_reach(&self) -> (Vec<String>, Option<String>) {
        let mut addrs = Vec::new();
        let mut relay = None;
        for a in self.endpoint.addr().addrs {
            match a {
                TransportAddr::Ip(s) => addrs.push(s.to_string()),
                TransportAddr::Relay(u) => relay = Some(u.to_string()),
                _ => {}
            }
        }
        (addrs, relay)
    }

    /// 183 §7.0 (W-22): sign `msg` with this engine's key (the contact card's JWS).
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.endpoint.secret_key().sign(msg).to_bytes().to_vec()
    }

    /// Track an inbound connection; `false` when `peer` already has [`A2A_CONNS_MAX`] live
    /// ones (a flood from one id).
    fn track_a2a(&self, peer: EndpointId, conn: &Connection) -> bool {
        let mut m = Self::lock(&self.a2a_conns);
        let list = m.entry(peer).or_default();
        list.retain(|c| c.close_reason().is_none());
        if list.len() >= A2A_CONNS_MAX {
            return false;
        }
        list.push(conn.clone());
        true
    }

    /// `peer`'s shared `theuth/a2a/1` stream budget ([`A2A_STREAMS_MAX`]); spent budgets of
    /// remotes no stream holds any more are dropped here.
    fn a2a_streams(&self, peer: EndpointId) -> Arc<tokio::sync::Semaphore> {
        let mut m = Self::lock(&self.a2a_stream_budgets);
        m.retain(|_, s| Arc::strong_count(s) > 1);
        m.entry(peer)
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(A2A_STREAMS_MAX)))
            .clone()
    }
    /// Connected right now?
    pub fn is_connected(&self, id: &EndpointId) -> bool {
        Self::lock(&self.presence)
            .get(id)
            .is_some_and(|p| !p.conns.is_empty())
    }

    /// Connected paired peers with when each connection run started (ms). P2-F1: a connection
    /// of an engine that just left the paired set is not a peer, even before it closes.
    pub fn connected(&self) -> Vec<(EngineId, u64)> {
        Self::lock(&self.presence)
            .iter()
            .filter(|(id, p)| !p.conns.is_empty() && self.is_paired(id))
            .map(|(id, p)| (EngineId(id.to_string()), p.since_ms))
            .collect()
    }

    /// P2-F1: a gossip frame counts only when the neighbour that delivered it and the engine
    /// it names are both paired. Not "the same engine": gossip forwards frames through peers.
    pub(crate) fn gossip_admits(&self, delivered_from: &EndpointId, id: &EngineId) -> bool {
        self.is_paired(delivered_from)
            && EndpointId::from_str(&id.0).is_ok_and(|e| self.is_paired(&e))
    }

    /// This engine's display name.
    pub fn name(&self) -> String {
        self.name.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// 183 §6.3: when `id` was last heard from (ms): now while connected, else when its last
    /// connection closed; `None` when it never connected in this process.
    pub fn last_seen(&self, id: &str) -> Option<u64> {
        let eid = EndpointId::from_str(id).ok()?;
        let pr = Self::lock(&self.presence);
        let p = pr.get(&eid)?;
        Some(if p.conns.is_empty() {
            p.since_ms
        } else {
            now_ms()
        })
        .filter(|&ms| ms > 0)
    }

    /// 183 §6.7: the relay as Settings › Network shows it; `None` with `mesh.relay: none`.
    /// `rtt_ms` stays `None`: iroh 1.4 measures relay latency only in its unstable net report.
    pub fn relay_state(&self) -> Option<crate::RelayState> {
        use iroh::Watcher as _;
        let configured = self.relays.relays::<Vec<_>>().first()?.url.to_string();
        Some(match self.endpoint.home_relay_status().get().first() {
            Some(s) => crate::RelayState {
                url: s.url().to_string(),
                connected: s.is_connected(),
                rtt_ms: None,
                error: s.last_error().map(|e| e.to_string()),
            },
            // Not joined yet (a lone engine dials no relay).
            None => crate::RelayState {
                url: configured,
                connected: false,
                rtt_ms: None,
                error: None,
            },
        })
    }

    /// 183 §6.8: `MeshEvent::Nearby` / `MeshEvent::Pair` as the Nearby code raises them.
    pub fn nearby_events(&self) -> broadcast::Receiver<crate::MeshEvent> {
        self.nearby.events.subscribe()
    }

    /// 183 §6.7: this engine's new name, in its own replicated `peers` row so the other
    /// machines show it (a lone engine writes none: a `peers` row means "has a peer").
    pub async fn rename(&self, name: &str) -> anyhow::Result<()> {
        *self.name.write().unwrap_or_else(|p| p.into_inner()) = name.to_owned();
        if !self.paired_ids().is_empty() {
            self.store.put_peer(&self.me()).await?;
        }
        Ok(())
    }

    /// 183 §6.7: another machine's display name (`peers.name`, replicated and pushed).
    pub async fn rename_peer(&self, id: &EngineId, name: &str) -> anyhow::Result<()> {
        let Some(mut row) = self.store.peers().await?.into_iter().find(|p| &p.id == id) else {
            anyhow::bail!("unknown machine {id}");
        };
        row.name = name.to_owned();
        self.store.put_peer(&row).await?;
        self.push_peers().await;
        Ok(())
    }

    /// This engine as a peer-set row.
    fn me(&self) -> PeerEntry {
        PeerEntry {
            id: self.id.clone(),
            name: self.name(),
            addrs: self.endpoint.addr().addrs.into_iter().collect(),
            hub_pref: *Self::lock(&self.hub_pref),
            added_ms: now_ms(),
            removed_ms: None,
        }
    }

    /// Address hints for dialing `id`: what the peer set remembers.
    async fn addr_for(&self, id: EndpointId) -> EndpointAddr {
        let addrs: BTreeSet<TransportAddr> = self
            .store
            .peers()
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id.0 == id.to_string())
            .map(|p| p.addrs.into_iter().collect())
            .unwrap_or_default();
        EndpointAddr { id, addrs }
    }

    // --- pairing ---------------------------------------------------------

    /// The string `t mesh pair` prints; reuses a live token.
    pub async fn pairing_string(&self) -> String {
        let token = {
            let mut pending = Self::lock(&self.pending);
            match pending.as_ref().filter(|p| p.live()) {
                Some(p) => p.token.clone(),
                None => {
                    let p = Pending::fresh();
                    let token = p.token.clone();
                    *pending = Some(p);
                    token
                }
            }
        };
        // Join now so the string carries a relay address (a moment for the home relay).
        self.join_network().await;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.endpoint.online()).await;
        let vault = self.vault();
        pairing::build(&Payload {
            id: self.endpoint.id(),
            name: self.name(),
            relay: self.relay.clone(),
            addrs: self.endpoint.addr().addrs.into_iter().collect(),
            token,
            vault_key: vault.as_ref().map(|v| BASE32_NOPAD.encode(&*v.mesh_key())),
            vault_key_id: vault.map(|v| v.current_key().id),
        })
    }

    /// 224: a key-less single-use ticket that lives `ttl_ms` (capped at
    /// [`pairing::REMOTE_TTL_MAX_MS`]), minted for a cloud node that dials in after installing.
    /// Independent of [`Net::pairing_string`]'s slot: neither replaces the other.
    pub async fn remote_ticket(&self, ttl_ms: u64) -> String {
        let p = Pending::fresh_for(ttl_ms.min(pairing::REMOTE_TTL_MAX_MS));
        let token = p.token.clone();
        {
            let mut remote = Self::lock(&self.remote);
            remote.retain(Pending::live);
            remote.push(p);
        }
        self.join_network().await;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.endpoint.online()).await;
        pairing::build(&Payload {
            id: self.endpoint.id(),
            name: self.name(),
            relay: self.relay.clone(),
            addrs: self.endpoint.addr().addrs.into_iter().collect(),
            token,
            vault_key: None,
            vault_key_id: None,
        })
    }

    /// This engine's standing for [`pairing::key_owner`], before the new peer is learned.
    fn rank(&self) -> Rank {
        Rank {
            paired_peers: u32::try_from(self.paired_ids().len()).unwrap_or(u32::MAX),
            first_seen_ms: self.first_seen_ms,
            id: self.id.clone(),
        }
    }

    /// `t mesh add <string>`: dial, prove the token, store each other, merge
    /// peer sets (67 §4.1). Returns the new peer.
    ///
    /// The vault key follows [`pairing::key_owner`], not the dial direction
    /// (79 F5): the established side keeps its key and the fresh side adopts,
    /// whether the fresh side dialed (a laptop pasting a mesh's string) or was
    /// dialed (a host bringing up a VM). A pre-F5 acceptor sends no rank; then
    /// the dialer adopts the string's key as before, with a warning.
    pub async fn add_peer(self: &Arc<Self>, s: &str) -> anyhow::Result<Peer> {
        let payload = pairing::parse(s).context("not a mesh pairing string")?;
        if payload.id == self.endpoint.id() {
            anyhow::bail!("that is this engine's own pairing string");
        }
        self.join_network().await;
        self.pair_with(payload).await
    }

    /// [`Self::add_peer`] once the string is read; a Nearby ceremony (183 §6.8) arrives here
    /// with the acceptor's fresh token and vault key in place of a pasted string.
    pub(crate) async fn pair_with(self: &Arc<Self>, payload: Payload) -> anyhow::Result<Peer> {
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(payload.addr(), ALPN_MESH.as_bytes()),
        )
        .await
        .context("dial timed out")?
        .context("dial")?;
        if conn.remote_id() != payload.id {
            anyhow::bail!("dialed engine has a different id");
        }
        let mine = self.rank();
        let my_key = self.vault().map(|v| v.mesh_key());
        let (mut send, mut recv) = conn.open_bi().await.context("open stream")?;
        let my_keyring = crate::keys::pairing_keyring(self, &EngineId(payload.id.to_string()));
        write_msg(
            &mut send,
            &Msg::PairHello {
                token: payload.token.clone(),
                me: self.me(),
                peers: self.store.peer_entries().await?,
                vault_key: my_key
                    .as_ref()
                    .map(|k| Secret::new(BASE32_NOPAD.encode(&**k))),
                rank: Some(mine.clone()),
                keyring: my_keyring,
            },
        )
        .await?;
        let (entry, adopted, theirs, their_keyring) = match read_msg(&mut recv).await? {
            Msg::PairAck {
                token,
                mut me,
                peers,
                adopted,
                rank,
                keyring,
            } => {
                if token != payload.token {
                    anyhow::bail!("pairing token mismatch in reply");
                }
                // The addresses we just dialed are known-good, and may be ones the
                // peer cannot see itself (a VM's loopback port, 79 smoke bug 9).
                me.addrs = pairing::union(&payload.addrs, &me.addrs);
                self.learn(me.clone(), &peers).await?;
                (me, adopted, rank, keyring)
            }
            Msg::PairReject { reason } => anyhow::bail!("pairing refused: {reason}"),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        };
        let from = format!("{} ({})", entry.name, entry.id);
        let owner = match &theirs {
            Some(theirs) => pairing::key_owner(&mine, theirs),
            None => {
                tracing::warn!(
                    "mesh: {from} runs a pre-79-F5 t (no key rank in its PairAck): \
                     taking its vault key as before"
                );
                Side::Acceptor
            }
        };
        match owner {
            // The inviter's vault key becomes ours before any of its rows arrive (67 §5.4),
            // and before the control connection's `KeyEpoch` names our key (183 §6.9.9).
            Side::Acceptor => {
                crate::keys::adopt_pairing(
                    self,
                    &from,
                    payload.vault_key.as_deref(),
                    their_keyring,
                )
                .await;
            }
            Side::Dialer => {
                if my_key.is_some() && !adopted {
                    tracing::warn!("mesh: {from} should have taken our vault key but did not");
                }
                crate::keys::after_pairing(self, theirs.as_ref(), &their_keyring);
            }
        }
        vault_paired(&entry.id.0);
        let net = self.clone();
        tokio::spawn(async move { net.drive(conn).await });
        Ok(self.peer_of(&entry))
    }

    /// Both sides after a handshake: `other` becomes a peer (fresh `added_ms`),
    /// their set is merged in. 183 R4: no origin is recorded: a pairing partner hands a fresh
    /// engine the whole mesh, and quarantining all of it when that partner is retired would be
    /// wrong; only a `Peers` frame from a member can bring in an accomplice.
    async fn learn(&self, mut other: PeerEntry, their_peers: &[PeerEntry]) -> anyhow::Result<()> {
        other.added_ms = now_ms();
        other.removed_ms = None;
        let mut incoming = their_peers.to_vec();
        incoming.push(other);
        self.merge_peers(&incoming, None).await
    }

    /// Merge a peer set from `from` (`None`: not from a peer) into the store; refresh the
    /// paired set; notify. 183 R1 (S1): a live row `from` introduces is recorded as its
    /// origin, so it can be quarantined if `from` turns out to be unpaired already.
    pub async fn merge_peers(
        &self,
        incoming: &[PeerEntry],
        from: Option<&EngineId>,
    ) -> anyhow::Result<()> {
        let local = self.store.peer_entries().await?;
        let (_, changed) = pairing::merge(&local, incoming, &self.id);
        let now = now_ms();
        for p in &changed {
            self.store.put_peer(p).await?;
            if let Some(by) = from
                && &p.id != by
                && !p.removed()
                && !local.iter().any(|l| l.id == p.id)
            {
                self.store.record_origin(&p.id, by, now).await?;
            }
        }
        // 183 R1 (V-M1): always from the store. The `peers` table also travels by the record
        // replica, which writes rows (tombstones included) behind this merge's back, so a merge
        // that changed nothing may still find the in-memory set stale.
        self.refresh_paired().await?;
        Ok(())
    }

    /// Local removal: tombstone, refresh the paired set, drop connections, tell everyone.
    /// P2-F1: a quarantined row leaves the list last, so Remove never pairs it in between, a
    /// failed removal leaves it quarantined, and a failure clearing the list leaves no stale set.
    pub async fn remove_peer(&self, id: &EngineId) -> anyhow::Result<()> {
        let entries = self.store.peer_entries().await?;
        let Some(mut row) = entries.into_iter().find(|p| &p.id == id) else {
            anyhow::bail!("unknown peer {id}");
        };
        row.removed_ms = Some(now_ms());
        self.store.put_peer(&row).await?;
        self.refresh_paired().await?;
        if let Ok(eid) = EndpointId::from_str(&id.0) {
            let conns = Self::lock(&self.presence)
                .remove(&eid)
                .map(|p| p.conns)
                .unwrap_or_default();
            for c in conns {
                c.close(0u32.into(), b"removed");
            }
            let _ = self
                .events
                .send(PeerEvent::Disconnected(PeerId(id.0.clone())));
        }
        self.store.confirm_origin(id).await?;
        Ok(())
    }

    /// The paired set, recomputed from the store (183 R1, V-M1: the key path reads this, never
    /// the in-memory copy alone). Quarantine first (S1); an engine that left the set has its
    /// connections closed.
    pub(crate) async fn refresh_paired(&self) -> anyhow::Result<HashSet<EndpointId>> {
        // P2-F1: one refresh at a time from the store read to the set write, so a slower one
        // never writes an older set over a newer one (the connections are closed after).
        let refreshing = self.refreshing.lock().await;
        self.quarantine().await?;
        // 183 R4: `Store::peers` leaves quarantined rows out (they stay live in the replicated
        // table): unpaired on this engine only, until a user keeps or removes them.
        let live: HashSet<EndpointId> = self
            .store
            .peers()
            .await?
            .iter()
            .filter_map(|p| EndpointId::from_str(&p.id.0).ok())
            .collect();
        let (gone, changed): (Vec<EndpointId>, bool) = {
            let mut paired = self.paired.write().unwrap_or_else(|p| p.into_inner());
            let gone = paired.difference(&live).copied().collect();
            let changed = *paired != live;
            if changed {
                paired.clone_from(&live);
                self.peers_gen.send_modify(|g| *g += 1);
            }
            (gone, changed)
        };
        drop(refreshing);
        if changed {
            // 187: a server homed on an engine that left runs on one still here; a re-pair gives it back.
            ::tools::peers_changed();
        }
        if !gone.is_empty() && self.removals.receiver_count() > 0 {
            match self.store.removed_peers().await {
                Ok(removed) => {
                    for id in removed
                        .into_iter()
                        .filter(|r| gone.iter().any(|g| g.to_string() == r.0))
                    {
                        let _ = self.removals.send(id);
                    }
                }
                Err(e) => tracing::warn!("mesh: removal events: {e:#}"),
            }
        }
        for id in gone {
            self.close_conns(&id).await;
        }
        if !live.is_empty() {
            self.join_network().await;
        }
        Ok(live)
    }

    /// 183 R1 (S1), R4: a live row that an engine introduced (by `Peers`) after, or within
    /// [`QUARANTINE_SKEW_MS`] before, that engine was unpaired is quarantined here: marked
    /// locally and left out of this engine's paired set, never tombstoned in the replicated
    /// table, so one engine's suspicion never unpairs a machine for the whole mesh. A user
    /// keeps it or removes it (`confirm_peer`; Remove is a real unpair).
    async fn quarantine(&self) -> anyhow::Result<()> {
        let origins = self.store.origins().await?;
        if origins.is_empty() {
            return Ok(());
        }
        let entries = self.store.peer_entries().await?;
        for o in origins.iter().filter(|o| !o.confirmed && !o.quarantined) {
            let Some(removed) = entries
                .iter()
                .find(|p| p.id == o.by)
                .and_then(|p| p.removed_ms)
            else {
                continue;
            };
            if o.at_ms.saturating_add(QUARANTINE_SKEW_MS) < removed {
                continue;
            }
            let Some(row) = entries.iter().find(|p| p.id == o.id && !p.removed()) else {
                continue;
            };
            self.store.set_quarantined(&o.id).await?;
            tracing::warn!(
                "mesh: {} ({}) was added by {} after it was unpaired; not paired here until confirmed",
                row.name,
                o.id,
                o.by
            );
        }
        Ok(())
    }

    /// 183 R1 (S1): the quarantined rows, for `MeshStatus.quarantined`.
    pub async fn quarantined(&self) -> anyhow::Result<Vec<crate::types::QuarantinedPeer>> {
        let entries = self.store.peer_entries().await?;
        let name = |id: &EngineId| {
            entries
                .iter()
                .find(|p| &p.id == id)
                .map_or_else(|| id.0.chars().take(8).collect(), |p| p.name.clone())
        };
        Ok(self
            .store
            .origins()
            .await?
            .into_iter()
            .filter(|o| o.quarantined && !o.confirmed)
            .map(|o| crate::types::QuarantinedPeer {
                name: name(&o.id),
                added_by_name: name(&o.by),
                id: o.id,
                added_by: o.by,
            })
            .collect())
    }

    /// 183 R1 (S1, `mesh_confirm_peer` keep), R4: the row leaves the list and is paired here
    /// again (it was never tombstoned). Remove is `remove_peer`, which clears the list entry.
    pub async fn confirm_peer(&self, id: &EngineId) -> anyhow::Result<()> {
        if !self.store.confirm_origin(id).await? {
            anyhow::bail!("{id} is not waiting for a confirmation");
        }
        self.refresh_paired().await?;
        Ok(())
    }

    /// Push the full peer set to every connected peer (after a local change).
    pub async fn push_peers(&self) {
        let Ok(peers) = self.store.peer_entries().await else {
            return;
        };
        let conns: Vec<Connection> = Self::lock(&self.presence)
            .values()
            .filter_map(|p| p.conns.first().cloned())
            .collect();
        for c in conns {
            if let Ok((mut send, _recv)) = c.open_bi().await {
                let _ = write_msg(
                    &mut send,
                    &Msg::Peers {
                        peers: peers.clone(),
                    },
                )
                .await;
            }
        }
    }

    /// The UI/RPC shape of a stored row.
    pub fn peer_of(&self, p: &PeerEntry) -> Peer {
        let eid = EndpointId::from_str(&p.id.0).ok();
        let (conn, last_seen_ms) = eid.map_or((None, 0), |e| {
            let pr = Self::lock(&self.presence);
            pr.get(&e)
                .map_or((None, 0), |x| (x.conns.first().cloned(), x.since_ms))
        });
        let reachable = conn.is_some();
        let (path, rtt_ms) = conn
            .as_ref()
            .map_or((crate::PeerPath::Offline, None), path_of);
        Peer {
            id: p.id.clone(),
            name: p.name.clone(),
            addrs: p.addrs.iter().map(ToString::to_string).collect(),
            reachable,
            last_seen_ms: if reachable {
                now_ms()
            } else {
                last_seen_ms.max(p.added_ms)
            },
            hub_pref: p.hub_pref,
            path,
            rtt_ms,
        }
    }

    // --- presence and control connections --------------------------------

    /// Own a control connection to a paired peer until it closes: presence
    /// bookkeeping, peer-set exchange, `PeerEvent`s.
    async fn drive(self: &Arc<Self>, conn: Connection) {
        let peer = conn.remote_id();
        if !self.is_paired(&peer) {
            conn.close(0u32.into(), b"not paired");
            return;
        }
        self.presence_add(peer, conn.clone());
        // Dialer or acceptor, either side may push its set; the dialer starts.
        if conn.side().is_client()
            && let Ok(peers) = self.store.peer_entries().await
            && let Ok((mut send, _recv)) = conn.open_bi().await
        {
            let _ = write_msg(&mut send, &Msg::Peers { peers }).await;
        }
        crate::keys::on_connect(self, &conn).await;
        self.docs_on_connect(&conn).await;
        loop {
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(_) => break,
            };
            match read_msg(&mut recv).await {
                Ok(Msg::Peers { peers }) => {
                    let from = EngineId(peer.to_string());
                    if let Err(e) = self.merge_peers(&peers, Some(&from)).await {
                        tracing::warn!("mesh: peer set from {}: {e:#}", peer.fmt_short());
                    }
                    let _ = send.finish();
                }
                Ok(Msg::PairHello { .. }) => {
                    let _ = write_msg(
                        &mut send,
                        &Msg::PairReject {
                            reason: "already paired".into(),
                        },
                    )
                    .await;
                }
                Ok(
                    m @ (Msg::KeyEpoch { .. }
                    | Msg::KeyRequest { .. }
                    | Msg::KeyGrant { .. }
                    | Msg::KeyAck { .. }
                    | Msg::KeyRefused { .. }),
                ) => {
                    // Off the loop: a grant re-seals rows, a request waits for its ack.
                    tokio::spawn(crate::keys::on_frame(self.clone(), peer, m, send, recv));
                }
                Ok(Msg::DocsSchema { schema, homes_on }) => {
                    let _ = send.finish();
                    self.docs_frame(peer, conn.stable_id(), schema, homes_on)
                        .await;
                }
                Ok(other) => tracing::debug!("mesh: ignoring {other:?} from {}", peer.fmt_short()),
                Err(e) => tracing::debug!("mesh: control stream from {}: {e:#}", peer.fmt_short()),
            }
        }
        self.presence_remove(peer, &conn);
    }

    fn presence_add(&self, peer: EndpointId, conn: Connection) {
        let first = {
            let mut pr = Self::lock(&self.presence);
            let p = pr.entry(peer).or_default();
            if p.conns.is_empty() {
                p.since_ms = now_ms();
            }
            p.conns.push(conn);
            p.conns.len() == 1
        };
        if first {
            tracing::info!("mesh: connected {}", peer.fmt_short());
            let _ = self
                .events
                .send(PeerEvent::Connected(PeerId(peer.to_string())));
        }
        self.docs.changed.notify_one();
    }

    fn presence_remove(&self, peer: EndpointId, conn: &Connection) {
        let last = {
            let mut pr = Self::lock(&self.presence);
            let Some(p) = pr.get_mut(&peer) else {
                return;
            };
            p.conns.retain(|c| c.stable_id() != conn.stable_id());
            // 182: a disconnected peer was last seen now (`Peer::last_seen_ms`, the
            // UI's "offline since"); the next connect restarts the clock.
            if p.conns.is_empty() {
                p.since_ms = now_ms();
            }
            p.conns.is_empty()
        };
        {
            // 184 §8.1: a frame counts while a connection that carried one lives.
            let mut frames = Self::lock(&self.docs.frames);
            if let Some(f) = frames.get_mut(&peer) {
                f.conns.remove(&conn.stable_id());
                if last || f.conns.is_empty() {
                    frames.remove(&peer);
                }
            }
        }
        if last {
            tracing::info!("mesh: disconnected {}", peer.fmt_short());
            let _ = self
                .events
                .send(PeerEvent::Disconnected(PeerId(peer.to_string())));
        }
        self.docs.changed.notify_one();
    }

    // --- 184 §8.1: the docs gate ------------------------------------------

    /// On every control connection, beside `keys::on_connect`: our `DocsSchema` frame, and a peer
    /// from which no live control connection carried one within [`DOCS_SILENT`] is recorded as
    /// `schema` 0. (Two control connections can be up at once, a pairing's and the connector's,
    /// and the peer's frame may ride either.)
    async fn docs_on_connect(self: &Arc<Self>, conn: &Connection) {
        self.send_docs(conn).await;
        let (net, conn) = (self.clone(), conn.clone());
        tokio::spawn(async move {
            tokio::time::sleep(DOCS_SILENT).await;
            let peer = conn.remote_id();
            let _record = net.docs.record.lock().await;
            let spoke = Self::lock(&net.docs.frames).contains_key(&peer);
            if conn.close_reason().is_none() && !spoke && net.is_paired(&peer) {
                tracing::info!(
                    "mesh: {} sent no docs schema: a pre-184 build, notes stay on the hub",
                    peer.fmt_short()
                );
                net.docs_record(peer, 0, false).await;
            }
        });
    }

    async fn send_docs(&self, conn: &Connection) {
        let msg = Msg::DocsSchema {
            schema: crate::DOCS_SCHEMA,
            homes_on: self.docs.homes_on.load(Ordering::Relaxed),
        };
        if let Ok((mut send, _recv)) = conn.open_bi().await
            && write_msg(&mut send, &msg).await.is_ok()
        {
            let _ = send.finish();
        }
    }

    /// A `DocsSchema` frame from `peer` (the connection's `remote_id`, never a claimed id).
    async fn docs_frame(&self, peer: EndpointId, conn: usize, schema: u32, homes_on: bool) {
        let _record = self.docs.record.lock().await;
        {
            let mut frames = Self::lock(&self.docs.frames);
            let f = frames.entry(peer).or_default();
            f.conns.insert(conn);
            f.homes_on = homes_on;
        }
        self.docs_record(peer, schema, homes_on).await;
    }

    async fn docs_record(&self, peer: EndpointId, schema: u32, homes_on: bool) {
        let (id, now) = (EngineId(peer.to_string()), now_ms());
        if let Err(e) = self.store.docs_heard(&id, schema, homes_on, now).await {
            tracing::warn!("mesh: peer_docs_seen {}: {e:#}", peer.fmt_short());
        }
        Self::lock(&self.docs.touches).wrote(&id, now);
        self.docs.changed.notify_one();
    }

    /// The gate (§8.1) as of `now`, `holder` the lease holder: the floor over the paired set and
    /// `peer_docs_seen`, `homes_on` from the holder's own frame; a change is sent to every
    /// connected peer. Fail-closed: a store error is homes off.
    pub(crate) async fn docs_gate(
        &self,
        holder: Option<&EngineId>,
        now: u64,
    ) -> crate::DocsGateState {
        // One pass at a time: two racing passes could leave the older answer advertised.
        let _pass = self.docs.pass.lock().await;
        let state = match self.docs_floor(now).await {
            Ok(floor_ok) => {
                let mine = holder == Some(&self.id);
                let frame = holder
                    .filter(|_| !mine)
                    .and_then(|h| EndpointId::from_str(&h.0).ok())
                    .filter(|h| self.is_connected(h))
                    .and_then(|h| Self::lock(&self.docs.frames).get(&h).map(|f| f.homes_on));
                crate::DocsGateState {
                    floor_ok,
                    homes_on: crate::docs_homes_on(floor_ok, mine, frame),
                }
            }
            Err(e) => {
                tracing::warn!("mesh: docs gate: {e:#}");
                crate::DocsGateState::default()
            }
        };
        if self.docs.homes_on.swap(state.homes_on, Ordering::Relaxed) != state.homes_on {
            tracing::info!(
                "mesh: notes homes {}",
                if state.homes_on { "on" } else { "off" }
            );
            for (_, c) in self.control_conns() {
                self.send_docs(&c).await;
            }
        }
        state
    }

    async fn docs_floor(&self, now: u64) -> anyhow::Result<bool> {
        let peers: Vec<EngineId> = self
            .store
            .peers()
            .await?
            .into_iter()
            .map(|p| p.id)
            .collect();
        self.store.docs_listed(&peers, now).await?;
        let due: Vec<EngineId> = {
            let mut t = Self::lock(&self.docs.touches);
            self.connected()
                .into_iter()
                .filter(|(id, _)| t.due(id, now))
                .map(|(id, _)| id)
                .collect()
        };
        for id in &due {
            self.store.docs_touch(id, now).await?;
        }
        Ok(crate::docs_floor(
            &peers,
            &self.store.docs_seen().await?,
            now,
        ))
    }

    /// Inbound `ALPN_MESH` connection: a pairing handshake from a stranger, or
    /// a control connection from a paired peer.
    async fn accept_control(self: &Arc<Self>, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        if self.dropped(&peer) {
            conn.close(0u32.into(), b"dropped");
            return Ok(());
        }
        if self.is_paired(&peer) {
            self.drive(conn).await;
            return Ok(());
        }
        // Stranger: the first stream must be a valid PairHello, or a Nearby PairRequest (§6.8).
        let (mut send, mut recv) = conn.accept_bi().await?;
        let (token, me, peers, vault_key, rank, keyring) = match read_msg(&mut recv).await {
            Ok(Msg::PairHello {
                token,
                me,
                peers,
                vault_key,
                rank,
                keyring,
            }) => (token, me, peers, vault_key, rank, keyring),
            Ok(m @ Msg::PairRequest { .. }) => {
                crate::nearby::on_pair_request(self, conn, send, recv, m).await;
                return Ok(());
            }
            _ => {
                tracing::debug!(
                    "mesh: {} is not in the paired set and sent no PairHello; closed",
                    peer.fmt_short()
                );
                conn.close(0u32.into(), b"not paired");
                return Ok(());
            }
        };
        let ok = {
            let mut pending = Self::lock(&self.pending);
            let live = pending
                .as_ref()
                .is_some_and(|p| p.live() && p.token == token);
            if live {
                *pending = None;
            }
            // 183 §6.8: or the token a Nearby ceremony with this very endpoint minted once both
            // sides pressed Pair. 224: or a remote ticket (consumed on use).
            live || pairing::take_remote(&mut Self::lock(&self.remote), &token)
                || crate::nearby::take_token(self, &token, &peer)
        };
        if !ok {
            let _ = write_msg(
                &mut send,
                &Msg::PairReject {
                    reason: "invalid or expired token".into(),
                },
            )
            .await;
            conn.close(0u32.into(), b"bad token");
            return Ok(());
        }
        if me.id.0 != peer.to_string() {
            let _ = write_msg(
                &mut send,
                &Msg::PairReject {
                    reason: "id mismatch".into(),
                },
            )
            .await;
            return Ok(());
        }
        // 79 F5: the fresh side takes the established side's key. Decided here
        // with the pre-pairing counts on both ends; a pre-F5 dialer sends no
        // rank and adopts our key from the string as before.
        let mine = self.rank();
        let from = format!("{} ({})", me.name, me.id);
        let dialer_owns = rank
            .as_ref()
            .is_some_and(|theirs| pairing::key_owner(theirs, &mine) == Side::Dialer);
        let adopted = match &vault_key {
            Some(key) if dialer_owns => {
                crate::keys::adopt_pairing(self, &from, Some(key.as_str()), keyring.clone()).await
            }
            _ => false,
        };
        let reply = Msg::PairAck {
            token,
            me: self.me(),
            peers: self.store.peer_entries().await.map_err(user_err)?,
            adopted,
            rank: Some(mine),
            keyring: crate::keys::pairing_keyring(self, &me.id),
        };
        self.learn(me, &peers).await.map_err(user_err)?;
        if !dialer_owns {
            crate::keys::after_pairing(self, rank.as_ref(), &keyring);
        }
        write_msg(&mut send, &reply).await.map_err(user_err)?;
        vault_paired(&peer.to_string());
        let _ = send.finish();
        tracing::info!("mesh: paired with {}", peer.fmt_short());
        // Push the updated peer set to already-connected peers: when a new node joins via a
        // ticket it is the dialer (with 0 peers) and we are the acceptor, so existing peers
        // only learn about it if we broadcast here. Spawned off the accept path so a slow or
        // unreachable third peer cannot delay driving this connection.
        let net = self.clone();
        tokio::spawn(async move {
            net.push_peers().await;
        });
        self.drive(conn).await;
        Ok(())
    }

    // --- connector -------------------------------------------------------

    /// Keep one control connection to every paired peer; reconcile on every
    /// peer-set change. Runs until `cancel`.
    pub async fn run_connector(self: Arc<Self>, cancel: CancellationToken) {
        let mut tasks: HashMap<EndpointId, CancellationToken> = HashMap::new();
        let mut generation = self.peers_gen.subscribe();
        loop {
            let want = self.paired_ids();
            tasks.retain(|id, c| {
                let keep = want.contains(id);
                if !keep {
                    c.cancel();
                }
                keep
            });
            for id in want {
                if let std::collections::hash_map::Entry::Vacant(e) = tasks.entry(id) {
                    let c = cancel.child_token();
                    e.insert(c.clone());
                    tokio::spawn(self.clone().connect_loop(id, c));
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                r = generation.changed() => if r.is_err() { break },
            }
        }
        for c in tasks.values() {
            c.cancel();
        }
    }

    async fn connect_loop(self: Arc<Self>, peer: EndpointId, cancel: CancellationToken) {
        let mut backoff = BACKOFF_MIN;
        let mut events = self.events.subscribe();
        let me_lower = self.endpoint.id() < peer;
        while !cancel.is_cancelled() {
            if self.dropped(&peer) {
                backoff = BACKOFF_MIN;
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                continue;
            }
            if self.is_connected(&peer) {
                // Wait for our disconnect (or cancellation).
                let started = tokio::time::Instant::now();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        ev = events.recv() => match ev {
                            Ok(PeerEvent::Disconnected(p)) if p.0 == peer.to_string() => break,
                            Ok(_) => {}
                            Err(broadcast::error::RecvError::Lagged(_)) => break,
                            Err(_) => return,
                        }
                    }
                }
                backoff = backoff_after_conn(backoff, started.elapsed());
                continue;
            }
            if !me_lower {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(DIAL_GRACE) => {}
                }
                if self.is_connected(&peer) {
                    continue;
                }
            }
            let addr = self.addr_for(peer).await;
            let dial = tokio::time::timeout(
                CONNECT_TIMEOUT,
                self.endpoint.connect(addr, ALPN_MESH.as_bytes()),
            );
            let conn = tokio::select! {
                _ = cancel.cancelled() => return,
                r = dial => r,
            };
            match conn {
                Ok(Ok(conn)) => {
                    let started = tokio::time::Instant::now();
                    self.drive(conn).await;
                    backoff = backoff_after_conn(backoff, started.elapsed());
                }
                Ok(Err(e)) => tracing::debug!("mesh: dial {}: {e}", peer.fmt_short()),
                Err(_) => tracing::debug!("mesh: dial {}: timed out", peer.fmt_short()),
            }
            if self.is_connected(&peer) {
                continue;
            }
            let jitter = backoff.mul_f64(rand::random_range(0.8..1.2));
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(jitter) => {}
            }
            backoff = next_backoff(backoff);
        }
    }

    // --- streams -----------------------------------------------------------

    /// A connection to `peer` on `alpn`, cached per (peer, alpn).
    async fn connection(&self, peer: EndpointId, alpn: &str) -> anyhow::Result<Connection> {
        let key = (peer, alpn.to_owned());
        let mut cache = self.outbound.lock().await;
        if let Some(c) = cache.get(&key)
            && c.close_reason().is_none()
        {
            return Ok(c.clone());
        }
        let addr = self.addr_for(peer).await;
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(addr, alpn.as_bytes()),
        )
        .await
        .context("dial timed out")?
        .with_context(|| format!("dial {} on {alpn}", peer.fmt_short()))?;
        cache.insert(key, conn.clone());
        Ok(conn)
    }

    /// Inbound streams on `alpn` arrive here from [`Inbound`].
    async fn deliver_inbound(&self, alpn: &str, item: InboundItem) {
        if let Some((tx, _)) = self.inbound.get(alpn) {
            let _ = tx.send(item).await;
        }
    }

    /// Wrap `stream` so its reads and writes count toward `alpn`.
    fn count(
        &self,
        alpn: &str,
        peer: EndpointId,
        stream: Box<dyn ByteStream>,
    ) -> Box<dyn ByteStream> {
        let counters = Self::lock(&self.traffic)
            .entry(alpn.to_owned())
            .or_default()
            .clone();
        Box::new(Counted {
            inner: stream,
            counters,
            alpn: alpn.to_owned(),
            limit: self.storm_frames_per_sec,
            peer,
            paired: self.paired.clone(),
        })
    }

    /// What every ALPN's streams moved since start, sorted by ALPN.
    pub fn traffic(&self) -> Vec<AlpnTraffic> {
        let mut out: Vec<AlpnTraffic> = Self::lock(&self.traffic)
            .iter()
            .map(|(alpn, c)| AlpnTraffic {
                alpn: alpn.clone(),
                frames_in: c.frames_in.load(Ordering::Relaxed),
                frames_out: c.frames_out.load(Ordering::Relaxed),
                bytes_in: c.bytes_in.load(Ordering::Relaxed),
                bytes_out: c.bytes_out.load(Ordering::Relaxed),
            })
            .collect();
        out.sort_by(|a, b| a.alpn.cmp(&b.alpn));
        out
    }
}

#[async_trait::async_trait]
impl Transport for Net {
    fn peers(&self) -> Vec<PeerId> {
        self.connected()
            .into_iter()
            .map(|(id, _)| PeerId(id.0))
            .collect()
    }

    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        let id = EndpointId::from_str(&peer.0).with_context(|| format!("bad peer id {peer}"))?;
        anyhow::ensure!(self.is_paired(&id), "{peer} is not a paired engine");
        anyhow::ensure!(!self.dropped(&id), "{peer}: traffic dropped (dev)");
        let conn = self.connection(id, alpn).await?;
        let (send, recv) = conn.open_bi().await.context("open stream")?;
        Ok(self.count(alpn, id, Box::new(tokio::io::join(recv, send))))
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }

    async fn accept(&self, alpn: &str) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)> {
        let (_, rx) = self
            .inbound
            .get(alpn)
            .with_context(|| format!("no inbound queue for {alpn}"))?;
        rx.lock().await.recv().await.context("mesh endpoint closed")
    }
}

// --- protocol handlers -------------------------------------------------------

/// `ALPN_MESH` handler.
#[derive(Debug, Clone)]
struct Control(std::sync::Weak<Net>);

impl ProtocolHandler for Control {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(net) = self.0.upgrade() else {
            return Ok(());
        };
        net.accept_control(connection).await
    }
}

/// 183 §7.2: `theuth/a2a/1`. A family peer is refused (it uses `peer_deliver`); anyone else
/// passes only when the a2a plugin's [`ContactAlpn::admit`] says so, rechecked per stream
/// so a removed or blocked contact's open connection stops at its next request.
#[derive(Debug, Clone)]
struct A2aGate {
    net: std::sync::Weak<Net>,
}

impl ProtocolHandler for A2aGate {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(net) = self.net.upgrade() else {
            return Ok(());
        };
        let peer = connection.remote_id();
        if net.is_paired(&peer) {
            connection.close(0u32.into(), b"family uses peer_deliver");
            return Ok(());
        }
        let Some(handler) = net.contacts.get().cloned() else {
            connection.close(0u32.into(), A2A_NOT_A_CONTACT);
            return Ok(());
        };
        let remote = peer.to_string();
        if !handler.admit(&remote) {
            connection.close(0u32.into(), A2A_NOT_A_CONTACT);
            return Ok(());
        }
        if !net.track_a2a(peer, &connection) {
            connection.close(0u32.into(), b"too many connections");
            return Ok(());
        }
        serve_a2a(
            net.paired.clone(),
            net.a2a_streams(peer),
            handler,
            connection,
        )
        .await;
        Ok(())
    }
}

/// 183 §7.2: serve a contact's requests on `connection`, whichever side dialed it: each
/// stream is re-admitted, so a removed or blocked contact stops at its next request. The
/// dialer serves too, because either side reuses a live connection (`Net::a2a_stream`).
/// At most [`A2A_STREAMS_MAX`] streams of one remote run at once (`streams` is shared by
/// all its connections); a stream beyond that is dropped unanswered.
async fn serve_a2a(
    paired: Arc<RwLock<HashSet<EndpointId>>>,
    streams: Arc<tokio::sync::Semaphore>,
    handler: Arc<dyn ContactAlpn>,
    connection: Connection,
) {
    let peer = connection.remote_id();
    let remote = peer.to_string();
    while let Ok((send, recv)) = connection.accept_bi().await {
        let family = paired
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&peer);
        if family || !handler.admit(&remote) {
            connection.close(0u32.into(), A2A_NOT_A_CONTACT);
            break;
        }
        let Ok(permit) = streams.clone().try_acquire_owned() else {
            continue;
        };
        let run = handler.stream(remote.clone(), Box::new(tokio::io::join(recv, send)));
        tokio::spawn(async move {
            run.await;
            drop(permit);
        });
    }
}

/// Stream ALPNs: paired peers only; every accepted bi stream is queued for
/// [`Transport::accept`].
#[derive(Debug, Clone)]
struct Inbound {
    net: std::sync::Weak<Net>,
    alpn: &'static str,
}

impl ProtocolHandler for Inbound {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(net) = self.net.upgrade() else {
            return Ok(());
        };
        let peer = connection.remote_id();
        if !net.is_paired(&peer) || net.dropped(&peer) {
            connection.close(0u32.into(), b"not paired");
            return Ok(());
        }
        let pid = PeerId(peer.to_string());
        net.track_inbound(peer, &connection);
        while let Ok((send, recv)) = connection.accept_bi().await {
            // 183 R3: per stream, not only per connection: an unpair lands between streams.
            if !net.is_paired(&peer) {
                connection.close(0u32.into(), b"not paired");
                break;
            }
            let stream = net.count(self.alpn, peer, Box::new(tokio::io::join(recv, send)));
            net.deliver_inbound(self.alpn, (pid.clone(), stream)).await;
        }
        Ok(())
    }
}

/// Paired-peers-only gate in front of another handler (gossip).
#[derive(Debug, Clone)]
struct Gated<H> {
    net: std::sync::Weak<Net>,
    inner: H,
}

impl<H: ProtocolHandler + Clone> ProtocolHandler for Gated<H> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(net) = self.net.upgrade() else {
            return Ok(());
        };
        if !net.is_paired(&connection.remote_id()) || net.dropped(&connection.remote_id()) {
            connection.close(0u32.into(), b"not paired");
            return Ok(());
        }
        net.track_inbound(connection.remote_id(), &connection);
        self.inner.accept(connection).await
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

/// [`Net::dev_drop`]: `THEUTH_DEV_MESH_DROP_FILE`, honoured only by a debug build bound to a
/// loopback address; `None` otherwise (release builds never read it).
fn dev_drop_file(bind: Option<std::net::SocketAddr>) -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) || !bind.is_some_and(|a| a.ip().is_loopback()) {
        return None;
    }
    std::env::var_os("THEUTH_DEV_MESH_DROP_FILE").map(std::path::PathBuf::from)
}

// --- framing -----------------------------------------------------------------

/// 183 §6.7: how a live connection reaches its peer: the selected path (else the first open
/// one), direct = `lan`, through the relay = `relay`, with its RTT estimate.
fn path_of(conn: &Connection) -> (crate::PeerPath, Option<u32>) {
    let paths = conn.paths();
    let Some(p) = paths
        .iter()
        .find(|p| p.is_selected())
        .or_else(|| paths.iter().next())
    else {
        return (crate::PeerPath::Lan, None);
    };
    let path = if p.is_relay() {
        crate::PeerPath::Relay
    } else {
        crate::PeerPath::Lan
    };
    (
        path,
        Some(u32::try_from(p.rtt().as_millis()).unwrap_or(u32::MAX)),
    )
}

/// An application error as the router reports it.
fn user_err(e: anyhow::Error) -> AcceptError {
    AcceptError::from_boxed(e.into())
}

/// 225: tell the seal gate a pairing with `id` just completed: an `enroll` from it is
/// pre-approved for a while, and an engine without a keychain asks it to become an unlock device.
fn vault_paired(id: &str) {
    if let Some(g) = secrets::seal::installed() {
        g.after_pairing(id);
    }
}

pub(crate) async fn write_msg(send: &mut SendStream, msg: &Msg) -> anyhow::Result<()> {
    let body = serde_json::to_vec(msg)?;
    anyhow::ensure!(body.len() < MAX_MSG as usize, "message too large");
    send.write_all(&(body.len() as u32).to_le_bytes()).await?;
    send.write_all(&body).await?;
    Ok(())
}

pub(crate) async fn read_msg(recv: &mut RecvStream) -> anyhow::Result<Msg> {
    let len = recv.read_u32_le().await.context("read length")?;
    anyhow::ensure!(len < MAX_MSG, "message too large ({len} bytes)");
    let mut body = vec![0u8; len as usize];
    recv.read_exact(&mut body).await.context("read body")?;
    Ok(serde_json::from_slice(&body)?)
}

/// Reconnect backoff after a control connection closes: reset to [`BACKOFF_MIN`]
/// only when the connection stayed alive for at least [`MIN_HEALTHY_CONN`]; an
/// immediate close (e.g. `not paired`) keeps the current backoff so it escalates.
fn backoff_after_conn(current: Duration, lived: Duration) -> Duration {
    if lived >= MIN_HEALTHY_CONN {
        BACKOFF_MIN
    } else {
        current
    }
}

/// Double `current` up to [`BACKOFF_MAX`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(BACKOFF_MAX)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// 183 §10.2: a pre-183 `PairHello` (no `keyring`) still decodes, an empty keyring is
    /// not sent (the frame stays byte-identical for old peers), and the appended key frames
    /// round-trip without key bytes in `Debug`.
    #[test]
    fn old_pair_hello_decodes_and_key_frames_round_trip() {
        let entry = PeerEntry {
            id: EngineId("a".into()),
            name: "a".into(),
            addrs: Vec::new(),
            hub_pref: HubPref::Eligible,
            added_ms: 1,
            removed_ms: None,
        };
        let old = serde_json::json!({"PairHello": {"token": "t", "me": entry, "peers": []}});
        let Msg::PairHello { keyring, .. } = serde_json::from_value(old).unwrap() else {
            panic!("not a PairHello")
        };
        assert!(keyring.is_empty());
        let hello = Msg::PairHello {
            token: "t".into(),
            me: entry.clone(),
            peers: Vec::new(),
            vault_key: None,
            rank: None,
            keyring: Vec::new(),
        };
        assert!(!serde_json::to_string(&hello).unwrap().contains("keyring"));
        let grant = Msg::KeyGrant {
            key: secrets::WireKey {
                info: secrets::KeyInfo {
                    id: "0123456789abcdef".into(),
                    epoch: 1,
                    initiator: "a".into(),
                    members: vec!["a".into(), "b".into()],
                    minted_ms: 5,
                },
                key: zeroize::Zeroizing::new("KEYBYTES".into()),
            },
            removed: vec![entry],
        };
        assert!(!format!("{grant:?}").contains("KEYBYTES"));
        let back: Msg = serde_json::from_str(&serde_json::to_string(&grant).unwrap()).unwrap();
        let Msg::KeyGrant { key, removed } = back else {
            panic!("not a KeyGrant")
        };
        assert_eq!(
            (key.info.epoch, key.key.as_str(), removed.len()),
            (1, "KEYBYTES", 1)
        );
    }

    /// 183 R1 (S7): the vault key in `PairHello` and in a Nearby `PairAnswer` never reaches a
    /// `Debug` line (the `{other:?}` logs), and still crosses the wire as the plain string.
    #[test]
    fn pairing_frames_redact_the_vault_key_in_debug() {
        let me = PeerEntry {
            id: EngineId("a".into()),
            name: "a".into(),
            addrs: Vec::new(),
            hub_pref: HubPref::Eligible,
            added_ms: 1,
            removed_ms: None,
        };
        let frames = [
            Msg::PairHello {
                token: "t".into(),
                me,
                peers: Vec::new(),
                vault_key: Some(Secret::new("VAULTKEYBYTES".into())),
                rank: None,
                keyring: Vec::new(),
            },
            Msg::PairAnswer {
                accept: true,
                token: Some("t".into()),
                vault_key: Some(Secret::new("VAULTKEYBYTES".into())),
            },
        ];
        for f in &frames {
            assert!(!format!("{f:?}").contains("VAULTKEYBYTES"), "{f:?}");
            assert!(
                serde_json::to_string(f)
                    .unwrap()
                    .contains("\"VAULTKEYBYTES\"")
            );
        }
    }

    async fn node(name: &str) -> (Arc<Net>, tempfile::TempDir) {
        let (net, dir, _) = node_at(name, 1_000).await;
        (net, dir)
    }

    /// A node whose identity was created at `first_seen_ms`, with its own vault.
    pub(crate) async fn node_at(
        name: &str,
        first_seen_ms: u64,
    ) -> (Arc<Net>, tempfile::TempDir, Arc<Vault>) {
        bind_node(name, first_seen_ms, None).await
    }

    /// 183 §6.8: a loopback node whose Nearby runs on `mdns` (a fake LAN in tests).
    pub(crate) async fn node_with(
        name: &str,
        mdns: Option<Arc<dyn crate::nearby::Mdns>>,
    ) -> (Arc<Net>, tempfile::TempDir, Arc<Vault>) {
        bind_node(name, 1_000, mdns).await
    }

    async fn bind_node(
        name: &str,
        first_seen_ms: u64,
        mdns: Option<Arc<dyn crate::nearby::Mdns>>,
    ) -> (Arc<Net>, tempfile::TempDir, Arc<Vault>) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let net = bind_at(
            name,
            first_seen_ms,
            mdns,
            dir.path(),
            SecretKey::generate(),
            Some(vault.clone()),
        )
        .await;
        (net, dir, vault)
    }

    /// Bind a loopback node over `dir`'s mesh store (a restart reuses `secret` and `dir`).
    pub(crate) async fn bind_at(
        name: &str,
        first_seen_ms: u64,
        mdns: Option<Arc<dyn crate::nearby::Mdns>>,
        dir: &std::path::Path,
        secret: SecretKey,
        vault: Option<Arc<Vault>>,
    ) -> Arc<Net> {
        let store = Store::open(
            &dir.join("mesh.sqlite"),
            EngineId(secret.public().to_string()),
        )
        .await
        .unwrap();
        Net::bind(
            BindOpts {
                secret,
                relay: String::new(),
                discovery: false,
                bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                storm_frames_per_sec: 50,
                first_seen_ms,
                vault,
                mdns,
            },
            store,
            name.into(),
            HubPref::Eligible,
        )
        .await
        .unwrap()
    }

    /// Two paired nodes, `a` holding a control connection to `c`.
    async fn connected_pair() -> (Arc<Net>, Arc<Net>, tempfile::TempDir, tempfile::TempDir) {
        let (a, da) = node("a").await;
        let (c, dc) = node("c").await;
        c.add_peer(&a.pairing_string().await).await.unwrap();
        for _ in 0..400 {
            if a.control_conn(&c.endpoint.id()).is_some() && c.is_paired(&a.endpoint.id()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(a.control_conn(&c.endpoint.id()).is_some());
        (a, c, da, dc)
    }

    /// P2-F1: `close_conns` closes the control connection at once, even while a dial holds
    /// `outbound`; an engine re-paired during that wait keeps the connections dialed for it.
    #[tokio::test]
    async fn closing_never_waits_for_a_dial_and_spares_a_re_pair() {
        let (a, c, _da, _dc) = connected_pair().await;
        let ec = c.endpoint.id();
        let _s = Transport::open(&*a, &PeerId(c.id.0.clone()), replica::ALPN_RECORDS)
            .await
            .unwrap();
        let control = a.control_conn(&ec).unwrap();
        let dialing = a.outbound.lock().await;
        a.paired.write().unwrap().remove(&ec);
        let a2 = a.clone();
        let closing = tokio::spawn(async move { a2.close_conns(&ec).await });
        for _ in 0..80 {
            if control.close_reason().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            control.close_reason().is_some(),
            "closed while the dial lock is held"
        );
        a.paired.write().unwrap().insert(ec);
        drop(dialing);
        closing.await.unwrap();
        assert!(
            a.outbound
                .lock()
                .await
                .contains_key(&(ec, replica::ALPN_RECORDS.to_string())),
            "re-paired meanwhile: its dial stays"
        );
        a.shutdown().await;
        c.shutdown().await;
    }

    /// P2-F1: an engine that left the paired set is no peer (lease, `Transport::peers`) and its
    /// gossip counts for nothing, even while its connection is still open.
    #[tokio::test]
    async fn an_engine_out_of_the_paired_set_is_no_peer_and_no_gossip_source() {
        let (a, c, _da, _dc) = connected_pair().await;
        let ec = c.endpoint.id();
        let stranger = SecretKey::generate().public();
        assert_eq!(Transport::peers(&*a), vec![PeerId(c.id.0.clone())]);
        assert!(a.gossip_admits(&ec, &c.id));
        assert!(
            !a.gossip_admits(&stranger, &c.id),
            "relayed by an unpaired engine"
        );
        assert!(
            !a.gossip_admits(&ec, &EngineId(stranger.to_string())),
            "names an unpaired engine"
        );
        a.paired.write().unwrap().remove(&ec);
        assert!(a.is_connected(&ec), "its connection is not closed yet");
        assert!(a.connected().is_empty());
        assert!(Transport::peers(&*a).is_empty());
        assert!(!a.gossip_admits(&ec, &c.id));
        a.shutdown().await;
        c.shutdown().await;
    }

    /// 183 R3: an unpair closes every connection to the removed engine, on every ALPN and in
    /// both directions, and a stream already open carries nothing more: C receives no further
    /// byte of records, and its own sessions stream delivers nothing here; new dials fail.
    #[tokio::test]
    async fn an_unpair_closes_every_stream_to_the_removed_engine() {
        let (a, _da) = node("a").await;
        let (c, _dc) = node("c").await;
        c.add_peer(&a.pairing_string().await).await.unwrap();
        for _ in 0..400 {
            if a.is_paired(&c.endpoint.id()) && c.is_paired(&a.endpoint.id()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let (pa, pc) = (PeerId(a.id.0.clone()), PeerId(c.id.0.clone()));
        let mut out = Transport::open(&*a, &pc, replica::ALPN_RECORDS)
            .await
            .unwrap();
        out.write_all(b"row1").await.unwrap();
        out.flush().await.unwrap();
        let (_, mut at_c) = Transport::accept(&*c, replica::ALPN_RECORDS).await.unwrap();
        let mut buf = [0u8; 4];
        at_c.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"row1");
        let mut from_c = Transport::open(&*c, &pa, replica::ALPN_SESSIONS)
            .await
            .unwrap();
        from_c.write_all(b"s1").await.unwrap();
        from_c.flush().await.unwrap();
        let (_, mut at_a) = Transport::accept(&*a, replica::ALPN_SESSIONS)
            .await
            .unwrap();
        let mut two = [0u8; 2];
        at_a.read_exact(&mut two).await.unwrap();

        a.remove_peer(&c.id).await.unwrap();
        let sent = out.write_all(b"row2").await.and(out.flush().await);
        assert!(sent.is_err(), "an in-flight batch to c fails at once");
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), at_c.read_to_end(&mut rest))
            .await
            .expect("c's stream ends, it does not hang");
        assert!(
            rest.is_empty(),
            "c received nothing after the unpair: {rest:?}"
        );
        let _ = from_c.write_all(b"s2").await;
        let _ = from_c.flush().await;
        assert!(
            at_a.read(&mut two).await.map_or(true, |n| n == 0),
            "nothing from c is read here either"
        );
        assert!(Transport::open(&*a, &pc, replica::ALPN_DOCS).await.is_err());
        // C still thinks it is paired; its new stream is refused, never delivered here.
        if let Ok(mut s) = Transport::open(&*c, &pa, replica::ALPN_DOCS).await {
            let _ = s.write_all(b"doc").await;
            let _ = s.flush().await;
        }
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                Transport::accept(&*a, replica::ALPN_DOCS)
            )
            .await
            .is_err(),
            "no docs stream from c reaches a"
        );
        a.shutdown().await;
        c.shutdown().await;
    }

    /// 79 F5 (smoke bug 10): the established engine keeps its vault key in
    /// both dial directions. `c` (fresh, newer) dials `a` → `c` adopts; then
    /// `a` (one peer) dials fresh `b` → `b` adopts and `a`'s bytes never move.
    #[tokio::test]
    async fn vault_key_stays_with_the_established_engine() {
        let (a, _da, ka) = node_at("a", 1_000).await;
        let (b, _db, kb) = node_at("b", 3_000).await;
        let (c, _dc, kc) = node_at("c", 2_000).await;
        let key = |v: &Vault| *v.mesh_key();
        let key_a = key(&ka);
        assert_ne!(key_a, key(&kb));

        // Fresh dials established (a laptop pastes the mesh's string): as before.
        c.add_peer(&a.pairing_string().await).await.unwrap();
        assert_eq!(key(&kc), key_a);
        assert_eq!(key(&ka), key_a);

        // Established dials fresh (the host pairs a VM): the fresh side adopts.
        let peer = a.add_peer(&b.pairing_string().await).await.unwrap();
        assert_eq!(peer.id, b.id);
        assert!(a.is_paired(&b.endpoint.id()) && b.is_paired(&a.endpoint.id()));
        assert_eq!(key(&kb), key_a, "the fresh acceptor adopted");
        assert_eq!(key(&ka), key_a, "the established dialer kept its key");
        assert_eq!(key(&kc), key_a);
        a.shutdown().await;
        b.shutdown().await;
        c.shutdown().await;
    }

    /// 183 §6.7: a connected peer reports its path (`lan` on loopback) with an RTT and a live
    /// `last_seen`; after a removal it is `offline` with none. No relay configured: no relay
    /// state. A rename of the other machine lands in its row.
    #[tokio::test]
    async fn mesh_status_paths_follow_the_connection() {
        let (a, _da) = node("a").await;
        let (b, _db) = node("b").await;
        let mut a_events = a.subscribe_peers();
        b.add_peer(&a.pairing_string().await).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), a_events.recv())
            .await
            .unwrap()
            .unwrap();
        let row = |n: &Net, id: &EngineId| {
            let n = n.store.clone();
            let id = id.clone();
            async move {
                n.peer_entries()
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|p| p.id == id)
                    .unwrap()
            }
        };
        let p = a.peer_of(&row(&a, &b.id).await);
        assert!(p.reachable);
        assert_eq!(p.path, crate::PeerPath::Lan);
        assert!(p.rtt_ms.is_some(), "the live connection's RTT");
        assert!(a.last_seen(&b.id.0).is_some());
        assert!(a.relay_state().is_none(), "`mesh.relay` empty: no relay");

        a.rename_peer(&b.id, "build box").await.unwrap();
        assert_eq!(row(&a, &b.id).await.name, "build box");

        a.remove_peer(&b.id).await.unwrap();
        let p = a.peer_of(&row(&a, &b.id).await);
        assert_eq!(
            (p.reachable, p.path, p.rtt_ms),
            (false, crate::PeerPath::Offline, None)
        );
        a.shutdown().await;
        b.shutdown().await;
    }

    /// Two endpoints on loopback, no relay: pair, connect, round-trip bytes on
    /// `ALPN_RECORDS` both ways through the `Transport`.
    #[tokio::test]
    async fn pair_connect_and_round_trip_records() {
        let (a, _da) = node("a").await;
        let (b, _db) = node("b").await;
        let mut a_events = a.subscribe_peers();

        let s = a.pairing_string().await;
        let peer = b.add_peer(&s).await.unwrap();
        assert_eq!(peer.id, a.id);
        assert_eq!(peer.name, "a");
        assert!(a.is_paired(&b.endpoint.id()) && b.is_paired(&a.endpoint.id()));
        assert!(a.store.peers().await.unwrap().iter().any(|p| p.id == b.id));
        assert!(b.store.peers().await.unwrap().iter().any(|p| p.id == a.id));
        // The token is single use.
        assert!(b.add_peer(&s).await.is_err());

        let ev = tokio::time::timeout(Duration::from_secs(10), a_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev, PeerEvent::Connected(PeerId(b.id.0.clone())));
        assert_eq!(a.peers(), vec![PeerId(b.id.0.clone())]);

        // b → a on ALPN_RECORDS; a echoes.
        let echo = {
            let a = a.clone();
            tokio::spawn(async move {
                let (from, mut stream) = a.accept(replica::ALPN_RECORDS).await.unwrap();
                let mut buf = [0u8; 5];
                stream.read_exact(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
                stream.flush().await.unwrap();
                (from, buf)
            })
        };
        let mut s = b
            .open(&PeerId(a.id.0.clone()), replica::ALPN_RECORDS)
            .await
            .unwrap();
        s.write_all(b"hello").await.unwrap();
        s.flush().await.unwrap();
        let mut back = [0u8; 5];
        tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut back))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&back, b"hello");
        let (from, got) = echo.await.unwrap();
        assert_eq!(from, PeerId(b.id.0.clone()));
        assert_eq!(&got, b"hello");

        // 69 audit: the round trip is on the books, per ALPN, both ends.
        let per = |n: &Net| {
            n.traffic()
                .into_iter()
                .find(|t| t.alpn == replica::ALPN_RECORDS)
                .unwrap()
        };
        let (ta, tb) = (per(&a), per(&b));
        assert_eq!(
            (ta.bytes_in, ta.bytes_out, tb.bytes_in, tb.bytes_out),
            (5, 5, 5, 5),
            "{ta:?} {tb:?}"
        );
        assert!(ta.frames_in >= 1 && ta.frames_out >= 1 && tb.frames_in >= 1 && tb.frames_out >= 1);

        // Removal tombstones on b and closes the connection.
        b.remove_peer(&a.id).await.unwrap();
        assert!(!b.is_paired(&a.endpoint.id()));
        assert!(b.store.peer_entries().await.unwrap()[0].removed());
        // 183 §4.8.1: an engine b no longer pairs with cannot reach b's
        // forwarded-RPC ALPN: b answers any stream it accepts, yet a's never
        // yields an answer.
        let answer = {
            let b = b.clone();
            tokio::spawn(async move {
                while let Ok((_, mut stream)) = b.accept(replica::ALPN_RPC).await {
                    let _ = stream.write_all(b"x\n").await;
                    let _ = stream.flush().await;
                }
            })
        };
        let refused = async {
            let mut s = a.open(&PeerId(b.id.0.clone()), replica::ALPN_RPC).await?;
            s.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
                .await?;
            s.flush().await?;
            let mut buf = [0u8; 1];
            anyhow::ensure!(s.read(&mut buf).await? > 0, "closed");
            anyhow::Ok(())
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(10), refused)
                .await
                .is_ok_and(|r| r.is_err()),
            "an unpaired engine's rpc stream is refused"
        );
        answer.abort();
        a.shutdown().await;
        b.shutdown().await;
    }

    /// The sampler warns once the rate stays above the limit for 10 s, then
    /// at most once a minute; a quiet second resets the sustain clock.
    #[test]
    fn sampler_warns_on_sustained_rate_once_a_minute() {
        let mut s = Sampler::default();
        let (mut total, mut warned) = (0, Vec::new());
        // 100 frames/s for 12 s (clock starts at 1 s; 0 is "no window yet"):
        // the first warning lands once 10 s are over.
        for sec in 1..=12u64 {
            total += 100;
            if let Some(rate) = s.sample(sec * 1000, total, 50) {
                warned.push((sec, rate));
            }
        }
        assert_eq!(
            warned,
            vec![(11, 100)],
            "10 s above the limit, measured from the first hot window"
        );
        // Still hot for the rest of the minute: nothing more until 60 s after the warning.
        for sec in 13..=75u64 {
            total += 100;
            if let Some(rate) = s.sample(sec * 1000, total, 50) {
                warned.push((sec, rate));
            }
        }
        assert_eq!(warned, vec![(11, 100), (71, 100)]);
        // A quiet second resets: 5 hot seconds after it warn nothing.
        assert!(s.sample(76_000, total + 10, 50).is_none());
        for sec in 77..=81u64 {
            total += 100;
            assert!(
                s.sample(sec * 1000, total, 50).is_none(),
                "sustain restarted at {sec}"
            );
        }
    }

    /// `Counted` books bytes and frames on both directions of a duplex pipe.
    #[tokio::test]
    async fn counted_stream_books_both_directions() {
        let (x, y) = tokio::io::duplex(64);
        let c = Arc::new(Counters::default());
        let peer = SecretKey::generate().public();
        let mut cx = Counted {
            inner: Box::new(x),
            counters: c.clone(),
            alpn: "t".into(),
            limit: 50,
            peer,
            paired: Arc::new(RwLock::new(HashSet::from([peer]))),
        };
        let mut y = y;
        cx.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        y.read_exact(&mut buf).await.unwrap();
        y.write_all(b"hi").await.unwrap();
        let mut back = [0u8; 2];
        cx.read_exact(&mut back).await.unwrap();
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        assert_eq!((load(&c.frames_out), load(&c.bytes_out)), (1, 5));
        assert_eq!((load(&c.frames_in), load(&c.bytes_in)), (1, 2));
    }

    /// Poll `f` every 25 ms for up to `secs` seconds.
    async fn eventually(secs: u64, mut f: impl AsyncFnMut() -> bool) -> bool {
        for _ in 0..secs * 40 {
            if f().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    async fn docs_row(net: &Net, id: &EngineId) -> Option<crate::store::DocsSeen> {
        let rows = net.store.docs_seen().await.unwrap();
        rows.into_iter().find(|s| &s.id == id)
    }

    /// 184 §8.1: two control connections up at once (a pairing's and the connector's dial, as on
    /// a fresh pairing) each carry a `DocsSchema` frame; the peer's frame on either counts, so
    /// neither side is taken for a pre-184 build once `DOCS_SILENT` has passed (that turned homes
    /// off after every fresh pairing, until a reconnect).
    #[tokio::test]
    async fn two_live_control_connections_are_not_a_silent_peer() {
        let (a, c, _da, _dc) = connected_pair().await;
        let second = c
            .endpoint
            .connect(a.endpoint.addr(), ALPN_MESH.as_bytes())
            .await
            .unwrap();
        let c2 = c.clone();
        tokio::spawn(async move { c2.drive(second).await });
        tokio::time::sleep(DOCS_SILENT + Duration::from_secs(2)).await;
        for (x, y) in [(&a, &c), (&c, &a)] {
            let row = docs_row(x, &y.id).await;
            assert_eq!(
                row.map(|s| s.docs_schema),
                Some(crate::DOCS_SCHEMA),
                "{} heard {}",
                x.id.0,
                y.id.0
            );
        }
    }

    /// 184 §8.1 (W-g): both ends send `DocsSchema` on connect and record the other under its
    /// authenticated id; the holder's flip reaches the follower in one frame; the follower is on
    /// only while the holder says so; a peer rolled back to silence (0) turns the holder off, and
    /// the follower with it.
    #[tokio::test]
    async fn docs_schema_rides_the_control_connection_on_connect_and_on_change() {
        use crate::DocsGateState;
        let (a, c, _da, _dc) = connected_pair().await;
        assert!(
            eventually(10, async || docs_row(&a, &c.id)
                .await
                .is_some_and(
                    |s| s.docs_schema == crate::DOCS_SCHEMA && s.heard_ms.is_some()
                ))
            .await,
            "a heard c on connect"
        );
        assert!(
            eventually(10, async || docs_row(&c, &a.id)
                .await
                .is_some_and(|s| s.docs_schema == crate::DOCS_SCHEMA))
            .await,
            "c heard a on connect"
        );
        let on = DocsGateState {
            floor_ok: true,
            homes_on: true,
        };
        // c follows a, which has said nothing but off yet.
        assert_eq!(
            c.docs_gate(Some(&a.id), now_ms()).await,
            DocsGateState {
                floor_ok: true,
                homes_on: false
            }
        );
        // a holds the lease: its own floor decides, and the flip is sent.
        assert_eq!(a.docs_gate(Some(&a.id), now_ms()).await, on);
        assert!(
            eventually(10, async || c.docs_gate(Some(&a.id), now_ms()).await == on).await,
            "a's flip reached c"
        );
        // c's own flip reached a too (c sends its `homes_on`; only the holder's counts).
        assert!(
            eventually(10, async || docs_row(&a, &c.id)
                .await
                .is_some_and(|s| s.homes_on))
            .await
        );
        assert!(
            !c.docs_gate(None, now_ms()).await.homes_on,
            "no holder: off"
        );
        assert!(
            eventually(10, async || docs_row(&a, &c.id)
                .await
                .is_some_and(|s| !s.homes_on))
            .await
        );
        // Rollback: c falls silent on a (recorded 0): a turns off, and c with it.
        a.docs_record(c.endpoint.id(), 0, false).await;
        assert_eq!(
            a.docs_gate(Some(&a.id), now_ms()).await,
            DocsGateState::default()
        );
        assert!(
            eventually(10, async || !c
                .docs_gate(Some(&a.id), now_ms())
                .await
                .homes_on)
            .await,
            "a's off reached c"
        );
        a.shutdown().await;
        c.shutdown().await;
    }

    /// A bare iroh endpoint on loopback: an engine that speaks no 184 (or nothing at all).
    async fn bare(key: SecretKey) -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    /// 184 §8.1, §8.3: a paired engine whose control connection says nothing for 5 s is a
    /// pre-184 build (`schema` 0), even one that said 2 before (rollback new→old), and the floor
    /// drops; a stranger's `DocsSchema` is never recorded.
    #[tokio::test]
    async fn a_silent_peer_is_old_and_a_strangers_frame_counts_for_nothing() {
        let (a, _da) = node("a").await;
        let old_key = SecretKey::generate();
        let old = EngineId(old_key.public().to_string());
        a.store
            .put_peer(&PeerEntry {
                id: old.clone(),
                name: "old".into(),
                addrs: vec![],
                hub_pref: HubPref::Eligible,
                added_ms: now_ms(),
                removed_ms: None,
            })
            .await
            .unwrap();
        a.refresh_paired().await.unwrap();
        a.store
            .docs_heard(&old, crate::DOCS_SCHEMA, false, now_ms())
            .await
            .unwrap();
        assert!(
            a.docs_gate(Some(&a.id), now_ms()).await.floor_ok,
            "it ran 184 before"
        );
        let old_ep = bare(old_key).await;
        let _conn = old_ep
            .connect(a.endpoint.addr(), ALPN_MESH.as_bytes())
            .await
            .unwrap();
        assert!(
            eventually(10, async || docs_row(&a, &old)
                .await
                .is_some_and(|s| s.docs_schema == 0))
            .await,
            "silent 5 s: recorded 0"
        );
        assert!(!a.docs_gate(Some(&a.id), now_ms()).await.floor_ok);

        let stranger_key = SecretKey::generate();
        let stranger = EngineId(stranger_key.public().to_string());
        let stranger_ep = bare(stranger_key).await;
        let conn = stranger_ep
            .connect(a.endpoint.addr(), ALPN_MESH.as_bytes())
            .await
            .unwrap();
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        write_msg(
            &mut send,
            &Msg::DocsSchema {
                schema: crate::DOCS_SCHEMA,
                homes_on: true,
            },
        )
        .await
        .unwrap();
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        assert!(docs_row(&a, &stranger).await.is_none(), "not paired");
        old_ep.close().await;
        stranger_ep.close().await;
        a.shutdown().await;
    }

    /// 210 D18: `dial_alpn` reaches a plain endpoint on a non-mesh ALPN with the caller's
    /// per-connection transport config (here at most two bidi streams the remote opens), and
    /// never joins the network: with discovery on, no address-lookup service (pkarr, n0 DNS,
    /// mDNS) is installed after the dial.
    #[tokio::test]
    async fn dial_alpn_uses_its_config_and_publishes_nothing() {
        const TEST_ALPN: &[u8] = b"theuth/test-share/1";
        let dir = tempfile::tempdir().unwrap();
        let secret = SecretKey::generate();
        let store = Store::open(
            &dir.path().join("mesh.sqlite"),
            EngineId(secret.public().to_string()),
        )
        .await
        .unwrap();
        let net = Net::bind(
            BindOpts {
                secret,
                relay: String::new(),
                discovery: true,
                bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                storm_frames_per_sec: 50,
                first_seen_ms: 1_000,
                vault: None,
                mdns: None,
            },
            store,
            "a".into(),
            HubPref::Eligible,
        )
        .await
        .unwrap();
        let edge = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::generate())
            .alpns(vec![TEST_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let accepting = edge.clone();
        let accepted =
            tokio::spawn(async move { accepting.accept().await.unwrap().await.unwrap() });

        // Gossip's in-memory lookup is the one service a bound endpoint has; the dial adds none.
        let lookups = net.endpoint.address_lookup().unwrap().len();
        let transport = QuicTransportConfig::builder()
            .max_concurrent_bidi_streams(2u32.into())
            .build();
        let conn = net
            .dial_alpn(edge.addr(), TEST_ALPN, transport)
            .await
            .unwrap();
        assert_eq!(conn.alpn(), TEST_ALPN);
        assert_eq!(conn.remote_id(), edge.id());
        let theirs = accepted.await.unwrap();

        // The edge opens visitor streams: two fit under this connection's limit, a third waits.
        let mut open = Vec::new();
        for _ in 0..2 {
            let (mut send, recv) = theirs.open_bi().await.unwrap();
            send.write_all(b"v").await.unwrap();
            open.push((send, recv));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(500), theirs.open_bi())
                .await
                .is_err(),
            "the per-connection stream limit applies"
        );
        let (_send, mut recv) = conn.accept_bi().await.unwrap();
        let mut one = [0u8; 1];
        recv.read_exact(&mut one).await.unwrap();
        assert_eq!(&one, b"v");

        assert!(
            !net.joined.load(Ordering::Relaxed),
            "join_network never ran"
        );
        assert_eq!(
            net.endpoint.address_lookup().unwrap().len(),
            lookups,
            "no pkarr, DNS or mDNS service: nothing publishes this engine"
        );
        conn.close(0u32.into(), b"done");
        edge.close().await;
        net.shutdown().await;
    }

    /// Bug 1: when a new node C (dialer, 0 peers) pairs with A via a remote ticket while
    /// A is already connected to B, A (the pairing acceptor) pushes the updated peer set
    /// so B learns C without waiting for a reconnect.
    #[tokio::test]
    async fn pairing_acceptor_pushes_updated_peers_to_existing_peers() {
        let (a, b, _da, _db) = connected_pair().await;
        let (c, _dc) = node("c").await;
        assert!(!b.is_paired(&c.endpoint.id()));

        let ticket = a.remote_ticket(60_000).await;
        c.add_peer(&ticket).await.unwrap();

        assert!(a.is_paired(&c.endpoint.id()));
        assert!(c.is_paired(&a.endpoint.id()));
        assert!(
            c.is_paired(&b.endpoint.id()),
            "dialer c learned existing peer b from PairAck"
        );
        assert!(
            eventually(10, async || b.is_paired(&c.endpoint.id())).await,
            "existing peer b should learn newly paired node c from acceptor push_peers"
        );
        assert!(
            b.store
                .peers()
                .await
                .unwrap()
                .iter()
                .any(|p| p.id == c.id),
            "b persisted c in its peer store"
        );

        a.shutdown().await;
        b.shutdown().await;
        c.shutdown().await;
    }

    /// Bug 2 (unit): reconnect backoff escalates up to `BACKOFF_MAX` on immediate closes
    /// (`lived < MIN_HEALTHY_CONN`) and resets to `BACKOFF_MIN` once a connection stays
    /// healthy for at least `MIN_HEALTHY_CONN`.
    #[test]
    fn connect_backoff_escalates_on_immediate_close_and_resets_after_healthy_conn() {
        let mut backoff = BACKOFF_MIN;
        let mut sleeps = Vec::new();
        for _ in 0..8 {
            backoff = backoff_after_conn(backoff, Duration::from_millis(2));
            sleeps.push(backoff);
            backoff = next_backoff(backoff);
        }
        assert_eq!(
            sleeps,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(32),
                BACKOFF_MAX,
                BACKOFF_MAX,
            ]
        );
        assert_eq!(
            backoff_after_conn(
                BACKOFF_MAX,
                MIN_HEALTHY_CONN.saturating_sub(Duration::from_millis(1))
            ),
            BACKOFF_MAX,
            "just under MIN_HEALTHY_CONN does not reset backoff"
        );
        assert_eq!(
            backoff_after_conn(BACKOFF_MAX, MIN_HEALTHY_CONN),
            BACKOFF_MIN,
            "at MIN_HEALTHY_CONN resets backoff"
        );
        assert_eq!(
            backoff_after_conn(BACKOFF_MAX, MIN_HEALTHY_CONN + Duration::from_secs(30)),
            BACKOFF_MIN,
            "above MIN_HEALTHY_CONN resets backoff"
        );
    }

    /// Bug 2 (live): when a peer closes `ALPN_MESH` immediately after QUIC connect
    /// (`not paired`), `connect_loop` escalates its reconnect interval instead of resetting
    /// to `BACKOFF_MIN` on every attempt.
    #[tokio::test]
    async fn connect_loop_escalates_backoff_when_peer_closes_immediately() {
        let (a_key, b_key) = loop {
            let x = SecretKey::generate();
            let y = SecretKey::generate();
            if x.public() < y.public() {
                break (x, y);
            }
        };
        let dir = tempfile::tempdir().unwrap();
        let a = bind_at("a", 1_000, None, dir.path(), a_key, None).await;
        let rejector = Endpoint::builder(presets::Minimal)
            .secret_key(b_key)
            .alpns(vec![ALPN_MESH.as_bytes().to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let ep = rejector.clone();
        let acceptor = tokio::spawn(async move {
            for _ in 0..3 {
                let Some(incoming) = ep.accept().await else {
                    break;
                };
                if let Ok(conn) = incoming.await {
                    let _ = tx.send(tokio::time::Instant::now());
                    conn.close(0u32.into(), b"not paired");
                }
            }
        });

        a.store
            .put_peer(&PeerEntry {
                id: EngineId(rejector.id().to_string()),
                name: "rejector".into(),
                addrs: rejector.addr().addrs.into_iter().collect(),
                hub_pref: HubPref::Eligible,
                added_ms: now_ms(),
                removed_ms: None,
            })
            .await
            .unwrap();
        a.refresh_paired().await.unwrap();

        let cancel = CancellationToken::new();
        let connector = tokio::spawn(a.clone().run_connector(cancel.clone()));

        let mut times = Vec::with_capacity(3);
        for _ in 0..3 {
            let t = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("timed out waiting for reconnect dial")
                .expect("acceptor closed early");
            times.push(t);
        }

        cancel.cancel();
        let _ = connector.await;
        let _ = acceptor.await;
        rejector.close().await;
        a.shutdown().await;

        let gap1 = times[1].duration_since(times[0]);
        let gap2 = times[2].duration_since(times[1]);
        // First retry waits ~1 s (0.8..1.2 s); second retry must wait ~2 s (1.6..2.4 s),
        // proving backoff was not reset to BACKOFF_MIN after the immediate close.
        assert!(
            gap2 > Duration::from_millis(1400) && gap2 > gap1,
            "expected escalating backoff after immediate close: gap1={gap1:?}, gap2={gap2:?}"
        );
    }
}
