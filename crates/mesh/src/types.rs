//! Public types of the mesh (67 §4). Everything here crosses the RPC surface
//! (`mesh_*` methods) and so is exported to TypeScript.

use std::fmt;

use serde::{Deserialize, Serialize};

pub use replica::{PeerEvent, PeerId};

/// An engine's identity: its iroh endpoint id (an Ed25519 public key) as text.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct EngineId(pub String);

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<EngineId> for replica::PeerId {
    fn from(id: EngineId) -> Self {
        Self(id.0)
    }
}

impl From<replica::PeerId> for EngineId {
    fn from(id: replica::PeerId) -> Self {
        Self(id.0)
    }
}

/// How willing an engine is to hold the hub lease (67 §4.5, decision 5).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum HubPref {
    /// Wins the lease whenever connected.
    Prefer,
    /// Competes on stability (the default).
    #[default]
    Eligible,
    /// Never holds it.
    Never,
}

/// What kind of machine an engine runs on; feeds the stability score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum EngineClass {
    /// Always on, no battery, no sleep.
    Server,
    /// Mains-powered, may sleep.
    Desktop,
    /// Battery, sleeps, roams.
    Laptop,
    /// Container on a server (as `Server` for the score, but marked).
    Container,
    /// A VM on another engine's machine (79); ranks with Laptop.
    Vm,
}

/// One row of the peer set (`peers` table) with live reachability.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Peer {
    /// Endpoint id.
    pub id: EngineId,
    /// Display name (hostname unless renamed).
    pub name: String,
    /// Last known direct/relay addresses, as iroh prints them.
    pub addrs: Vec<String>,
    /// A connection is up right now.
    pub reachable: bool,
    /// Last time this engine heard from it (ms since epoch; 0 = never).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_seen_ms: u64,
    /// Its lease preference.
    pub hub_pref: HubPref,
    /// 183 §6.7: how it is reached now; `offline` from an older engine.
    #[serde(default)]
    pub path: PeerPath,
    /// 183 §6.7: round-trip time of the live connection, when known.
    #[serde(default)]
    pub rtt_ms: Option<u32>,
}

/// 183 §3.3: how a peer is reached right now.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum PeerPath {
    /// Direct (LAN or any direct address).
    Lan,
    /// Through the iroh relay.
    Relay,
    /// Not connected.
    #[default]
    Offline,
}

/// 183 §6.7: this engine's relay as `mesh_status` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RelayState {
    /// The relay URL in use.
    pub url: String,
    /// The endpoint holds a relay connection.
    pub connected: bool,
    /// Round-trip time to the relay, when known.
    pub rtt_ms: Option<u32>,
    /// Why it is not connected, if known.
    pub error: Option<String>,
}

/// 183 §6.8: an engine found by the Nearby mDNS browse, not paired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NearbyEngine {
    /// Its endpoint id (from the TXT `id`).
    pub id: EngineId,
    /// TXT `name`.
    pub name: String,
    /// TXT `os`.
    pub os_name: String,
    /// The address it was seen at.
    pub addr: String,
    /// Last seen (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seen_ms: u64,
}

/// 183 §6.8: one Nearby pairing ceremony, as both sides show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PairCeremony {
    /// The ceremony (`mesh_pair_answer`).
    pub id: uuid::Uuid,
    /// The other engine.
    pub peer: EngineId,
    /// Its display name.
    pub name: String,
    /// Its display OS.
    pub os_name: String,
    /// Where the other engine is: the advert's address (dialer) or the connection's remote
    /// address (acceptor); `""` when unknown. A contact ceremony (§7.4): the contact's
    /// machine, from its card.
    #[serde(default)]
    pub addr: String,
    /// The 6-digit check code, shown `482 913`.
    pub code: String,
    /// `waiting` | `accepted` | `paired` | `rejected` | `expired`.
    pub state: String,
    /// When the ceremony expires (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expires_ms: u64,
}

/// What an engine advertises (67 §8 placement, §4.5 election).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineStats {
    /// Endpoint id.
    pub id: EngineId,
    /// Process start (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub uptime_since_ms: u64,
    /// Machine class.
    pub class: EngineClass,
    /// Fraction of the last 30 days peers saw it reachable, 0..=1.
    pub availability_30d: f32,
    /// Harnesses installed (`native`, `omp`, `opencode`, `claude`, …).
    pub harnesses: Vec<String>,
    /// Repo paths it can open.
    pub repos: Vec<String>,
    /// Has a display (browser, screenshots).
    pub display: bool,
    /// Has a usable GPU. `true` on every Apple-silicon Mac and any Linux box
    /// with a render node — not a training-grade signal; see `accelerator`.
    pub gpu: bool,
    /// Which training-grade accelerator the GPU is (126 §8.4); `None` from an
    /// engine older than this field. Optional on the wire so older engines
    /// deserialise newer rows and vice versa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub accelerator: Option<Accelerator>,
    /// 183 §5.6 (W-11): `std::env::consts::OS` (`macos` | `linux` | `windows`);
    /// `""` from an older engine.
    #[serde(default)]
    pub os: String,
    /// Display OS (`macOS`, `Arch Linux`, `Ubuntu`, `Windows`); `""` from an older engine.
    #[serde(default)]
    pub os_name: String,
    /// `std::env::consts::ARCH` (`aarch64` | `x86_64`); `""` from an older engine.
    #[serde(default)]
    pub arch: String,
    /// Hostname, distinct from the mesh name; `""` from an older engine.
    #[serde(default)]
    pub host: String,
}

/// The training-grade accelerator an engine advertises (126 §8.4), detected
/// the cheap way: Apple silicon → `metal`; `/dev/nvidia0` → `cuda`; else
/// `cpu` (a probed machine with no training-grade GPU — an Intel iGPU still
/// says `gpu = true`). Spelled like `models.accelerator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Accelerator {
    /// Apple silicon (candle Metal).
    Metal,
    /// An NVIDIA device (`/dev/nvidia0`; candle CUDA with the `decision/cuda` build).
    Cuda,
    /// No training-grade GPU.
    Cpu,
}

impl Accelerator {
    /// The column / stats-line spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Cuda => "cuda",
            Self::Cpu => "cpu",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "metal" => Self::Metal,
            "cuda" => Self::Cuda,
            "cpu" => Self::Cpu,
            _ => return None,
        })
    }
}

/// Who holds the hub lease (67 §4.3).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LeaseState {
    /// The hub; `None` while no engine qualifies or during a handover.
    pub holder: Option<EngineId>,
    /// When `holder` took it (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub since_ms: u64,
    /// Why (`elected`, `prefer`, `release`, `lapse`, …), for `lease_log`.
    pub reason: String,
}

/// Bus events (67 §4). Also delivered to [`crate::Subscribe`] receivers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum MeshEvent {
    /// A peer connected or disconnected.
    Peer(PeerEvent),
    /// The lease changed hands (or was released).
    Lease(LeaseState),
    /// 183 §6.8: the Nearby list changed.
    Nearby(Vec<NearbyEngine>),
    /// 183 §6.8: a ceremony started, its code, or its outcome.
    Pair(PairCeremony),
}

impl wire::WireEvent for MeshEvent {
    const TOPIC: &'static str = "mesh.events";
    const VERSION: u32 = 1;
}

/// Frames and bytes one stream ALPN has moved since start (69 audit: the
/// number a storm would show). A frame is one read or write on a stream,
/// not a protocol message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AlpnTraffic {
    /// Stream ALPN (`theuth/records/1`, …).
    pub alpn: String,
    /// Reads that returned bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub frames_in: u64,
    /// Writes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub frames_out: u64,
    /// Bytes read.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes_in: u64,
    /// Bytes written.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes_out: u64,
}

/// Reply to [`crate::Status`] / `mesh_status`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MeshStatus {
    /// This engine.
    pub self_id: EngineId,
    /// This engine's display name (`mesh.name`, default the hostname).
    pub self_name: String,
    /// The mesh's name (`mesh_meta.name`, 69 decision 8); `None` until an
    /// engine wrote it.
    pub mesh_name: Option<String>,
    /// Every paired engine, this one excluded.
    pub peers: Vec<Peer>,
    /// The lease.
    pub lease: LeaseState,
    /// Per stream ALPN, what moved since start; sorted by `alpn`.
    pub traffic: Vec<AlpnTraffic>,
    /// Replication counters; `None` until the replica plugin syncs over this mesh.
    pub replica: Option<replica::Stats>,
    /// 183 §5.6 (W-11): every known engine's replicated `engine_stats` row.
    #[serde(default)]
    pub stats: Vec<EngineStats>,
    /// 183 §6.7: this engine's relay; `None` with `mesh.relay: none` or from an older engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub relay: Option<RelayState>,
    /// 183 §6.2 (W-14): `mesh.messaging_off`, the machines this engine refuses agent messages
    /// with (Settings › Network "Agents can message" off).
    #[serde(default)]
    pub messaging_off: Vec<EngineId>,
    /// 183 §6.8: `mesh.nearby` (the "Let nearby machines find this one while pairing" switch).
    #[serde(default = "yes")]
    pub nearby: bool,
    /// 183 R1 (S1): machines an unpaired machine introduced after its unpair, not paired here
    /// until a user confirms (`mesh_confirm_peer`).
    #[serde(default)]
    pub quarantined: Vec<QuarantinedPeer>,
    /// 184 §10.4: this engine's `homes_on` (from `DocsGate`): notes have homes in this mesh; the
    /// note's Move to ▸ item reads it. `false` from a pre-184 engine.
    #[serde(default)]
    pub docs_homes_on: bool,
}

/// 184 §8.1: reply to [`crate::DocsGate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct DocsGateState {
    /// Every counted paired engine (heard within `DOCS_STALE_MS`) advertises
    /// `docs_schema >= DOCS_SCHEMA` on its control connection.
    pub floor_ok: bool,
    /// Homes are on here: the holder says so (its own `floor_ok` on the holder) and `floor_ok`.
    pub homes_on: bool,
}

/// 183 R1 (S1), R4: a peer row a since-unpaired engine introduced, held back on this engine until confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QuarantinedPeer {
    /// The machine.
    pub id: EngineId,
    /// Its display name.
    pub name: String,
    /// The unpaired engine that introduced it.
    pub added_by: EngineId,
    /// That engine's display name.
    pub added_by_name: String,
}

/// Serde default `true`.
fn yes() -> bool {
    true
}

/// 183 §14.7 (D7, §12.9): what the Unpair dialog shows before anything happens
/// (`mesh_unpair_preview`).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnpairPreview {
    /// The machine to unpair.
    pub id: EngineId,
    /// Its display name.
    pub name: String,
    /// Connected now.
    pub connected: bool,
    /// Last time this engine heard from it (ms since epoch; 0 = never).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_seen_ms: u64,
    /// User secret names it had (W-36): replace these at their providers.
    pub secrets: Vec<String>,
    /// `oauth/<ns>` namespaces it had, shown "Sign in again".
    pub sign_ins: Vec<String>,
    /// Remaining machines not connected now: they get the new key when they reconnect.
    pub offline: Vec<String>,
    /// Remaining machines that never sent `KeyEpoch` (a pre-183 build, §6.9.10).
    pub old_build: Vec<String>,
    /// Paired phones and iPads (the W-37 line and the D11 checkbox).
    pub devices: u32,
}
