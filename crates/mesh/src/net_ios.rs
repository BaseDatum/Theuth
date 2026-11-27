//! iOS stub for [`crate::net`]: mesh QUIC networking (`iroh`, `iroh-gossip`,
//! `iroh-mdns-address-lookup`) is gated out on iOS; the standalone engine never
//! binds a mesh endpoint.

use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use replica::{ByteStream, PeerEvent, PeerId, Transport};
use secrets::Vault;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

use crate::identity::SecretKey;
use crate::pairing::{EndpointId, PeerEntry};
use crate::store::Store;
use crate::types::{AlpnTraffic, EngineId, HubPref, Peer};

/// Stream ALPN: mesh control — pairing handshake, presence, peer-set exchange.
pub const ALPN_MESH: &str = "theuth/mesh/1";

/// 183 §7.2 (W-21): A2A between contacts (separate meshes).
pub const ALPN_A2A: &str = "theuth/a2a/1";

/// The close reason of a `theuth/a2a/1` connection the gate refused (not a contact).
pub const A2A_NOT_A_CONTACT: &[u8] = b"not a contact";

/// `mesh.relay` value meaning n0's public relays.
pub const RELAY_PUBLIC: &str = "public";

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

    /// Last known direct addresses for `remote`.
    fn hints(&self, _remote: &str) -> Vec<std::net::SocketAddr> {
        Vec::new()
    }
}

/// How to bind (stub on iOS where mesh networking is disabled).
pub struct BindOpts {
    /// This engine's key.
    pub secret: SecretKey,
    /// `mesh.relay`: URL, `public`, or empty for none.
    pub relay: String,
    /// DNS/pkarr + mDNS lookup.
    pub discovery: bool,
    /// Explicit socket to bind.
    pub bind_addr: Option<std::net::SocketAddr>,
    /// `mesh.storm_frames_per_sec`: one ALPN above this for 10 s is warned about.
    pub storm_frames_per_sec: u64,
    /// Identity creation (`identity.rs`).
    pub first_seen_ms: u64,
    /// The vault whose mesh keys pairing and rotation move.
    pub vault: Option<Arc<Vault>>,
    /// 183 §6.8: the Nearby advertise + browse.
    pub mdns: Option<Arc<dyn crate::nearby::Mdns>>,
}

/// Standalone iOS stub for the mesh network handle (never bound on iOS).
pub struct Net {
    /// This engine.
    pub id: EngineId,
    /// Bumped on every peer-set change.
    pub peers_gen: watch::Sender<u64>,
    name: RwLock<String>,
    removals: broadcast::Sender<EngineId>,
    events: broadcast::Sender<PeerEvent>,
    nearby_events: broadcast::Sender<crate::MeshEvent>,
    vault: OnceLock<Arc<Vault>>,
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Net {
    /// Bind the endpoint (always unsupported on iOS).
    pub async fn bind(
        _opts: BindOpts,
        _store: Store,
        _name: String,
        _hub_pref: HubPref,
    ) -> anyhow::Result<Arc<Self>> {
        anyhow::bail!("mesh networking is unavailable on iOS")
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

    /// Close everything (no-op on iOS).
    pub async fn shutdown(&self) {}

    /// Connect to the relay and start discovery (no-op on iOS).
    pub async fn join_network(&self) {}

    /// 210 §5.7: tombstoned engines as this engine's paired set drops them.
    pub fn subscribe_removals(&self) -> broadcast::Receiver<EngineId> {
        self.removals.subscribe()
    }

    /// Connect/disconnect events (the [`Transport::subscribe`] channel).
    pub fn subscribe_peers(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }

    /// Advertised lease preference (no-op on iOS).
    pub fn set_hub_pref(&self, _p: HubPref) {}

    /// Is `id` a live (not removed) peer? Always `false` on iOS.
    pub fn is_paired(&self, _id: &EndpointId) -> bool {
        false
    }

    /// Paired ids (always empty on iOS).
    pub fn paired_ids(&self) -> Vec<EndpointId> {
        Vec::new()
    }

    /// 191 (D1): this mesh's current membership (lone engine on iOS).
    pub async fn members(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(vec![(self.id.0.clone(), 0)])
    }

    /// 183 §7.2: install the a2a plugin's `theuth/a2a/1` handler (no-op on iOS).
    pub fn set_contacts(&self, _handler: Arc<dyn ContactAlpn>) {}

    /// 183 §7.2: one bidirectional `theuth/a2a/1` stream (unavailable on iOS).
    pub async fn a2a_stream(
        &self,
        _eid: &str,
        _addrs: &[std::net::SocketAddr],
        _relay: Option<&str>,
    ) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// 183 §7.3: how the contact `eid` is reached right now (always offline on iOS).
    pub fn a2a_path(&self, _eid: &str) -> crate::PeerPath {
        crate::PeerPath::Offline
    }

    /// 183 §7.2: direct addresses for `eid` (always empty on iOS).
    pub fn a2a_direct_addrs(&self, _eid: &str) -> Vec<std::net::SocketAddr> {
        Vec::new()
    }

    /// 183 §7.4 Revocation: close every `theuth/a2a/1` connection of `eid` (no-op on iOS).
    pub fn close_a2a(&self, _eid: &str) {}

    /// This engine's direct addresses and home relay (always empty on iOS).
    pub fn a2a_reach(&self) -> (Vec<String>, Option<String>) {
        (Vec::new(), None)
    }

    /// 183 §7.0 (W-22): sign `msg` with this engine's key (empty stub on iOS).
    pub fn sign(&self, _msg: &[u8]) -> Vec<u8> {
        Vec::new()
    }

    /// Connected right now? Always `false` on iOS.
    pub fn is_connected(&self, _id: &EndpointId) -> bool {
        false
    }

    /// Connected paired peers (always empty on iOS).
    pub fn connected(&self) -> Vec<(EngineId, u64)> {
        Vec::new()
    }

    /// This engine's display name.
    pub fn name(&self) -> String {
        self.name.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// 183 §6.3: when `id` was last heard from (always `None` on iOS).
    pub fn last_seen(&self, _id: &str) -> Option<u64> {
        None
    }

    /// 183 §6.7: the relay state (always `None` on iOS).
    pub fn relay_state(&self) -> Option<crate::RelayState> {
        None
    }

    /// 183 §6.8: `MeshEvent::Nearby` / `MeshEvent::Pair` stream.
    pub fn nearby_events(&self) -> broadcast::Receiver<crate::MeshEvent> {
        self.nearby_events.subscribe()
    }

    /// 183 §6.7: update this engine's display name locally.
    pub async fn rename(&self, name: &str) -> anyhow::Result<()> {
        *self.name.write().unwrap_or_else(|p| p.into_inner()) = name.to_owned();
        Ok(())
    }

    /// 183 §6.7: rename another machine (unavailable on iOS).
    pub async fn rename_peer(&self, _id: &EngineId, _name: &str) -> anyhow::Result<()> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// The string `t mesh pair` prints (empty on iOS).
    pub async fn pairing_string(&self) -> String {
        String::new()
    }

    /// `t mesh add <string>` (unavailable on iOS).
    pub async fn add_peer(self: &Arc<Self>, _s: &str) -> anyhow::Result<Peer> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// Merge a peer set into the store (unavailable on iOS).
    pub async fn merge_peers(
        &self,
        _incoming: &[PeerEntry],
        _from: Option<&EngineId>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// Local removal of a peer (unavailable on iOS).
    pub async fn remove_peer(&self, _id: &EngineId) -> anyhow::Result<()> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// 183 R1 (S1): quarantined rows (always empty on iOS).
    pub async fn quarantined(&self) -> anyhow::Result<Vec<crate::types::QuarantinedPeer>> {
        Ok(Vec::new())
    }

    /// 183 R1 (S1): confirm a quarantined peer (unavailable on iOS).
    pub async fn confirm_peer(&self, _id: &EngineId) -> anyhow::Result<()> {
        anyhow::bail!("mesh networking is unavailable on iOS")
    }

    /// Push the full peer set to connected peers (no-op on iOS).
    pub async fn push_peers(&self) {}

    /// The UI/RPC shape of a stored row (offline on iOS).
    pub fn peer_of(&self, p: &PeerEntry) -> Peer {
        Peer {
            id: p.id.clone(),
            name: p.name.clone(),
            addrs: p.addrs.iter().map(ToString::to_string).collect(),
            reachable: false,
            last_seen_ms: p.added_ms,
            hub_pref: p.hub_pref,
            path: crate::PeerPath::Offline,
            rtt_ms: None,
        }
    }

    /// Connector loop (waits for cancellation on iOS).
    pub async fn run_connector(self: Arc<Self>, cancel: CancellationToken) {
        cancel.cancelled().await;
    }

    /// What every ALPN's streams moved since start (always empty on iOS).
    pub fn traffic(&self) -> Vec<AlpnTraffic> {
        Vec::new()
    }
}

#[async_trait::async_trait]
impl Transport for Net {
    fn peers(&self) -> Vec<PeerId> {
        Vec::new()
    }

    async fn open(&self, _peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::bail!("mesh transport is unavailable on iOS")
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }

    async fn accept(&self, _alpn: &str) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)> {
        anyhow::bail!("mesh transport is unavailable on iOS")
    }
}
