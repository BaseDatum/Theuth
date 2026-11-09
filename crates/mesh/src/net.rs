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
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
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
    hub_pref: Mutex<HubPref>,
    pending: Mutex<Option<Pending>>,
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
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Every ALPN whose inbound streams are handed out through [`Transport::accept`].
const STREAM_ALPNS: [&str; 8] = [
    replica::ALPN_RECORDS,
    replica::ALPN_ACP,
    replica::ALPN_SESSIONS,
    replica::ALPN_KNOWLEDGE,
    replica::ALPN_RPC,
    replica::ALPN_ARTIFACTS,
    replica::ALPN_DOCS,
    replica::ALPN_DOUBLE,
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
                hub_pref: Mutex::new(hub_pref),
                pending: Mutex::new(None),
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
            }
        });
        if let Some(v) = opts.vault {
            net.set_vault(v);
        }
        if !net.paired_ids().is_empty() {
            net.join_network().await;
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
        if self.is_paired(peer) {
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
        for cfg in self.relays.relays::<Vec<_>>() {
            self.endpoint.insert_relay(cfg.url.clone(), cfg).await;
        }
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

    // --- small state -----------------------------------------------------

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
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
        let gone: Vec<EndpointId> = {
            let mut paired = self.paired.write().unwrap_or_else(|p| p.into_inner());
            let gone = paired.difference(&live).copied().collect();
            if *paired != live {
                paired.clone_from(&live);
                self.peers_gen.send_modify(|g| *g += 1);
            }
            gone
        };
        drop(refreshing);
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
        if last {
            tracing::info!("mesh: disconnected {}", peer.fmt_short());
            let _ = self
                .events
                .send(PeerEvent::Disconnected(PeerId(peer.to_string())));
        }
    }

    /// Inbound `ALPN_MESH` connection: a pairing handshake from a stranger, or
    /// a control connection from a paired peer.
    async fn accept_control(self: &Arc<Self>, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
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
            // sides pressed Pair.
            live || crate::nearby::take_token(self, &token, &peer)
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
        let _ = send.finish();
        tracing::info!("mesh: paired with {}", peer.fmt_short());
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
            if self.is_connected(&peer) {
                // Wait for our disconnect (or cancellation).
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
                    backoff = BACKOFF_MIN;
                    self.drive(conn).await;
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
            backoff = (backoff * 2).min(BACKOFF_MAX);
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
        if !net.is_paired(&peer) {
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
        if !net.is_paired(&connection.remote_id()) {
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
}
