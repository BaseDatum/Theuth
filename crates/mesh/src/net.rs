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

/// Stream ALPN: mesh control — pairing handshake, presence, peer-set exchange.
/// Not in the contract's five: gossip owns `ALPN_GOSSIP` end to end.
pub const ALPN_MESH: &str = "theuth/mesh/1";

/// `mesh.relay` value meaning n0's public relays.
pub const RELAY_PUBLIC: &str = "public";

/// Largest control or pairing message.
const MAX_MSG: u32 = 1 << 20;
/// Dial timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Reconnect backoff bounds.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// The higher id waits this long for the lower id's dial before dialing itself.
const DIAL_GRACE: Duration = Duration::from_secs(3);
/// Inbound stream queue per ALPN.
const INBOUND_QUEUE: usize = 64;

/// Messages on `ALPN_MESH` streams (`u32 LE len ‖ JSON`).
///
/// The vault key rides in `PairHello` only inside the already-authenticated
/// QUIC stream: `ALPN_MESH` is TLS keyed to both engines' identities (the
/// dialer checked `remote_id` against the pairing string, the acceptor checks
/// the one-time token before reading anything else), so the key is seen by
/// exactly the engine the string named. Unknown fields are ignored on read,
/// so a pre-F5 `t` on either end still pairs (it never adopts, never ranks).
#[derive(Debug, Serialize, Deserialize)]
enum Msg {
    /// Dialer of `t mesh add`: prove the one-time token, introduce yourself.
    PairHello {
        token: String,
        me: PeerEntry,
        peers: Vec<PeerEntry>,
        /// The dialer's mesh vault key (base32); the acceptor adopts it when
        /// [`pairing::key_owner`] says the dialer owns the key (79 F5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vault_key: Option<String>,
        /// The dialer's standing; `None` from a pre-F5 `t`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rank: Option<Rank>,
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
    },
    /// Inviter's refusal.
    PairReject { reason: String },
    /// Peer-set exchange between paired engines (on connect and on change).
    Peers { peers: Vec<PeerEntry> },
}

/// The vault's mesh data key as the handshake reads and replaces it (67 §5.4).
/// Production talks to the secrets actor ([`SecretsKeys`]); tests hand each
/// node its own cell, since the actor is one per process.
#[async_trait::async_trait]
pub trait VaultKeys: Send + Sync {
    /// The current key; `None` when secrets is not running.
    async fn get(&self) -> Option<[u8; 32]>;
    /// Re-seal every local row under `key` and keep it.
    async fn adopt(&self, key: [u8; 32]) -> Result<(), String>;
}

/// [`VaultKeys`] over [`secrets::SECRETS_ACTOR`].
pub struct SecretsKeys;

impl SecretsKeys {
    fn actor() -> Option<kameo::actor::ActorRef<secrets::SecretsActor>> {
        kameo::actor::ActorRef::<secrets::SecretsActor>::lookup(secrets::SECRETS_ACTOR)
            .ok()
            .flatten()
    }
}

#[async_trait::async_trait]
impl VaultKeys for SecretsKeys {
    async fn get(&self) -> Option<[u8; 32]> {
        Self::actor()?.ask(secrets::GetMeshKey).await.ok()
    }

    async fn adopt(&self, key: [u8; 32]) -> Result<(), String> {
        match Self::actor() {
            Some(a) => a
                .ask(secrets::AdoptMeshKey(key))
                .await
                .map_err(|e| e.to_string()),
            None => Err("secrets is not running".into()),
        }
    }
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
    /// Where the vault key lives; `None` = the secrets actor.
    pub keys: Option<Arc<dyn VaultKeys>>,
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
}

impl tokio::io::AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
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
    store: Store,
    /// This engine's display name (`mesh.name`, default the hostname).
    pub name: String,
    relay: String,
    /// Relays to insert on `join_network` (empty = `mesh.relay: none`).
    relays: RelayMap,
    /// `mesh.discovery`.
    discovery: bool,
    /// `join_network` ran.
    joined: AtomicBool,
    hub_pref: Mutex<HubPref>,
    pending: Mutex<Option<Pending>>,
    paired: RwLock<HashSet<EndpointId>>,
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
    keys: Arc<dyn VaultKeys>,
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
                name,
                relay: opts.relay,
                relays,
                discovery: opts.discovery,
                joined: AtomicBool::new(false),
                hub_pref: Mutex::new(hub_pref),
                pending: Mutex::new(None),
                paired: RwLock::new(paired),
                presence: Mutex::new(HashMap::new()),
                outbound: tokio::sync::Mutex::new(HashMap::new()),
                events,
                peers_gen,
                inbound,
                traffic: Mutex::new(HashMap::new()),
                storm_frames_per_sec: opts.storm_frames_per_sec,
                first_seen_ms: opts.first_seen_ms,
                keys: opts.keys.unwrap_or_else(|| Arc::new(SecretsKeys)),
            }
        });
        if !net.paired_ids().is_empty() {
            net.join_network().await;
        }
        Ok(net)
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

    /// Connected peers with when each connection run started (ms).
    pub fn connected(&self) -> Vec<(EngineId, u64)> {
        Self::lock(&self.presence)
            .iter()
            .filter(|(_, p)| !p.conns.is_empty())
            .map(|(id, p)| (EngineId(id.to_string()), p.since_ms))
            .collect()
    }

    /// This engine as a peer-set row.
    fn me(&self) -> PeerEntry {
        PeerEntry {
            id: self.id.clone(),
            name: self.name.clone(),
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
        let vault_key = self.keys.get().await.map(|k| BASE32_NOPAD.encode(&k));
        pairing::build(&Payload {
            id: self.endpoint.id(),
            name: self.name.clone(),
            relay: self.relay.clone(),
            addrs: self.endpoint.addr().addrs.into_iter().collect(),
            token,
            vault_key,
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

    /// Take `key` (base32) as the mesh vault key; a bad string or a secrets
    /// failure is logged, never fatal — the pairing itself stands.
    async fn adopt_key(&self, key: &str, from: &str) -> bool {
        let Some(key) = BASE32_NOPAD
            .decode(key.to_ascii_uppercase().as_bytes())
            .ok()
            .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
        else {
            tracing::warn!("mesh: {from} sent an unreadable vault key");
            return false;
        };
        match self.keys.adopt(key).await {
            Ok(()) => {
                tracing::info!("mesh: adopted the vault key from {from}");
                true
            }
            Err(e) => {
                tracing::warn!("mesh: adopting the vault key from {from}: {e}");
                false
            }
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
        let my_key = self.keys.get().await;
        let (mut send, mut recv) = conn.open_bi().await.context("open stream")?;
        write_msg(
            &mut send,
            &Msg::PairHello {
                token: payload.token.clone(),
                me: self.me(),
                peers: self.store.peer_entries().await?,
                vault_key: my_key.as_ref().map(|k| BASE32_NOPAD.encode(k)),
                rank: Some(mine.clone()),
            },
        )
        .await?;
        let (entry, adopted, theirs) = match read_msg(&mut recv).await? {
            Msg::PairAck {
                token,
                mut me,
                peers,
                adopted,
                rank,
            } => {
                if token != payload.token {
                    anyhow::bail!("pairing token mismatch in reply");
                }
                // The addresses we just dialed are known-good, and may be ones the
                // peer cannot see itself (a VM's loopback port, 79 smoke bug 9).
                me.addrs = pairing::union(&payload.addrs, &me.addrs);
                self.learn(me.clone(), &peers).await?;
                (me, adopted, rank)
            }
            Msg::PairReject { reason } => anyhow::bail!("pairing refused: {reason}"),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        };
        let net = self.clone();
        tokio::spawn(async move { net.drive(conn).await });
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
            // The inviter's vault key becomes ours before any of its rows arrive (67 §5.4).
            Side::Acceptor => {
                if let Some(key) = payload.vault_key.as_deref() {
                    self.adopt_key(key, &from).await;
                }
            }
            Side::Dialer => {
                if my_key.is_some() && !adopted {
                    tracing::warn!("mesh: {from} should have taken our vault key but did not");
                }
            }
        }
        Ok(self.peer_of(&entry))
    }

    /// Both sides after a handshake: `other` becomes a peer (fresh `added_ms`),
    /// their set is merged in.
    async fn learn(&self, mut other: PeerEntry, their_peers: &[PeerEntry]) -> anyhow::Result<()> {
        other.added_ms = now_ms();
        other.removed_ms = None;
        let mut incoming = their_peers.to_vec();
        incoming.push(other);
        self.merge_peers(&incoming).await
    }

    /// Merge a peer set into the store; refresh the paired set; notify.
    pub async fn merge_peers(&self, incoming: &[PeerEntry]) -> anyhow::Result<()> {
        let local = self.store.peer_entries().await?;
        let (_, changed) = pairing::merge(&local, incoming, &self.id);
        for p in &changed {
            self.store.put_peer(p).await?;
        }
        // The `peers` table also travels by the record replica, which writes the row behind this
        // merge's back: nothing "changed" here, yet the in-memory paired set never learned the id
        // and refused its control connection as a stranger (69 smoke: C, paired to A, could not
        // connect to B). Refresh whenever a live incoming id is unknown to the set.
        let unknown = {
            let paired = self.paired.read().unwrap_or_else(|p| p.into_inner());
            incoming.iter().any(|p| {
                p.removed_ms.is_none()
                    && p.id != self.id
                    && EndpointId::from_str(&p.id.0).is_ok_and(|e| !paired.contains(&e))
            })
        };
        if changed.is_empty() && !unknown {
            return Ok(());
        }
        self.refresh_paired().await?;
        Ok(())
    }

    /// Local removal: tombstone, drop connections, tell everyone.
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
        Ok(())
    }

    async fn refresh_paired(&self) -> anyhow::Result<()> {
        let live: HashSet<EndpointId> = self
            .store
            .peers()
            .await?
            .iter()
            .filter_map(|p| EndpointId::from_str(&p.id.0).ok())
            .collect();
        let some = !live.is_empty();
        *self.paired.write().unwrap_or_else(|p| p.into_inner()) = live;
        self.peers_gen.send_modify(|g| *g += 1);
        if some {
            self.join_network().await;
        }
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
        let (reachable, last_seen_ms) = eid.map_or((false, 0), |e| {
            let pr = Self::lock(&self.presence);
            pr.get(&e)
                .map_or((false, 0), |x| (!x.conns.is_empty(), x.since_ms))
        });
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
        loop {
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(_) => break,
            };
            match read_msg(&mut recv).await {
                Ok(Msg::Peers { peers }) => {
                    if let Err(e) = self.merge_peers(&peers).await {
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
        // Stranger: the first stream must be a valid PairHello.
        let (mut send, mut recv) = conn.accept_bi().await?;
        let Ok(Msg::PairHello {
            token,
            me,
            peers,
            vault_key,
            rank,
        }) = read_msg(&mut recv).await
        else {
            tracing::debug!(
                "mesh: {} is not in the paired set and sent no PairHello; closed",
                peer.fmt_short()
            );
            conn.close(0u32.into(), b"not paired");
            return Ok(());
        };
        let ok = {
            let mut pending = Self::lock(&self.pending);
            let live = pending
                .as_ref()
                .is_some_and(|p| p.live() && p.token == token);
            if live {
                *pending = None;
            }
            live
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
        let adopted = match (&rank, &vault_key) {
            (Some(theirs), Some(key)) if pairing::key_owner(theirs, &mine) == Side::Dialer => {
                self.adopt_key(key, &format!("{} ({})", me.name, me.id))
                    .await
            }
            _ => false,
        };
        let reply = Msg::PairAck {
            token,
            me: self.me(),
            peers: self.store.peer_entries().await.map_err(user_err)?,
            adopted,
            rank: Some(mine),
        };
        self.learn(me, &peers).await.map_err(user_err)?;
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
    fn count(&self, alpn: &str, stream: Box<dyn ByteStream>) -> Box<dyn ByteStream> {
        let counters = Self::lock(&self.traffic)
            .entry(alpn.to_owned())
            .or_default()
            .clone();
        Box::new(Counted {
            inner: stream,
            counters,
            alpn: alpn.to_owned(),
            limit: self.storm_frames_per_sec,
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
        Ok(self.count(alpn, Box::new(tokio::io::join(recv, send))))
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
        while let Ok((send, recv)) = connection.accept_bi().await {
            let stream = net.count(self.alpn, Box::new(tokio::io::join(recv, send)));
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
        self.inner.accept(connection).await
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

// --- framing -----------------------------------------------------------------

/// An application error as the router reports it.
fn user_err(e: anyhow::Error) -> AcceptError {
    AcceptError::from_boxed(e.into())
}

async fn write_msg(send: &mut SendStream, msg: &Msg) -> anyhow::Result<()> {
    let body = serde_json::to_vec(msg)?;
    anyhow::ensure!(body.len() < MAX_MSG as usize, "message too large");
    send.write_all(&(body.len() as u32).to_le_bytes()).await?;
    send.write_all(&body).await?;
    Ok(())
}

async fn read_msg(recv: &mut RecvStream) -> anyhow::Result<Msg> {
    let len = recv.read_u32_le().await.context("read length")?;
    anyhow::ensure!(len < MAX_MSG, "message too large ({len} bytes)");
    let mut body = vec![0u8; len as usize];
    recv.read_exact(&mut body).await.context("read body")?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// One node's vault key: a cell standing in for the secrets actor.
    struct MemKeys(Mutex<[u8; 32]>);

    #[async_trait::async_trait]
    impl VaultKeys for MemKeys {
        async fn get(&self) -> Option<[u8; 32]> {
            Some(*Net::lock(&self.0))
        }
        async fn adopt(&self, key: [u8; 32]) -> Result<(), String> {
            *Net::lock(&self.0) = key;
            Ok(())
        }
    }

    async fn node(name: &str) -> (Arc<Net>, tempfile::TempDir) {
        let (net, dir, _) = node_at(name, 1_000).await;
        (net, dir)
    }

    /// A node whose identity was created at `first_seen_ms`, with its own key.
    async fn node_at(
        name: &str,
        first_seen_ms: u64,
    ) -> (Arc<Net>, tempfile::TempDir, Arc<MemKeys>) {
        let dir = tempfile::tempdir().unwrap();
        let secret = SecretKey::generate();
        let store = Store::open(
            &dir.path().join("mesh.sqlite"),
            EngineId(secret.public().to_string()),
        )
        .await
        .unwrap();
        let keys = Arc::new(MemKeys(Mutex::new(rand::random())));
        let net = Net::bind(
            BindOpts {
                secret,
                relay: String::new(),
                discovery: false,
                bind_addr: Some("127.0.0.1:0".parse().unwrap()),
                storm_frames_per_sec: 50,
                first_seen_ms,
                keys: Some(keys.clone()),
            },
            store,
            name.into(),
            HubPref::Eligible,
        )
        .await
        .unwrap();
        (net, dir, keys)
    }

    /// 79 F5 (smoke bug 10): the established engine keeps its vault key in
    /// both dial directions. `c` (fresh, newer) dials `a` → `c` adopts; then
    /// `a` (one peer) dials fresh `b` → `b` adopts and `a`'s bytes never move.
    #[tokio::test]
    async fn vault_key_stays_with_the_established_engine() {
        let (a, _da, ka) = node_at("a", 1_000).await;
        let (b, _db, kb) = node_at("b", 3_000).await;
        let (c, _dc, kc) = node_at("c", 2_000).await;
        let key_a = ka.get().await.unwrap();
        assert_ne!(key_a, kb.get().await.unwrap());

        // Fresh dials established (a laptop pastes the mesh's string): as before.
        c.add_peer(&a.pairing_string().await).await.unwrap();
        assert_eq!(kc.get().await.unwrap(), key_a);
        assert_eq!(ka.get().await.unwrap(), key_a);

        // Established dials fresh (the host pairs a VM): the fresh side adopts.
        let peer = a.add_peer(&b.pairing_string().await).await.unwrap();
        assert_eq!(peer.id, b.id);
        assert!(a.is_paired(&b.endpoint.id()) && b.is_paired(&a.endpoint.id()));
        assert_eq!(kb.get().await.unwrap(), key_a, "the fresh acceptor adopted");
        assert_eq!(
            ka.get().await.unwrap(),
            key_a,
            "the established dialer kept its key"
        );
        assert_eq!(kc.get().await.unwrap(), key_a);
        a.shutdown().await;
        b.shutdown().await;
        c.shutdown().await;
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
        let mut cx = Counted {
            inner: Box::new(x),
            counters: c.clone(),
            alpn: "t".into(),
            limit: 50,
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
