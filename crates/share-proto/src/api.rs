//! Requests and answers of the control API on `theuth/share/1` (§5.3). Field names are the design's, exactly.
//! Times are Unix milliseconds (`*_ms`, and `not_after`), durations seconds (`*_s`, `secs`). Every struct ignores
//! unknown fields, and fields added later must be `#[serde(default)]` (the versioning rule, [`crate::version`]).

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::csr::SpkiHash;
use crate::names::NameKind;
use crate::signed::Signed;
use crate::ticket::Invite;

/// The edge's admission mode (§9.4, §9.5), live via `admin admission`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Admission {
    /// Any mesh may attach.
    Open,
    /// A new mesh needs an invite in its first `hello`; admitted meshes keep working.
    Invite,
    /// No new mesh.
    Closed,
}

/// `GET /v1/info`, the one request answered before `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    /// Protocol major ([`crate::PROTOCOL_MAJOR`]).
    pub v: u32,
    /// `theuth-share-edge/<ver>` ([`crate::version::edge_agent`]).
    pub edge: String,
    /// `D`, the edge's configured domain (D16).
    pub domain: String,
    /// The edge's iroh endpoint id; a delegation's `edge`.
    pub edge_id: PublicKey,
    /// The edge clock; the lease holder signs with `issued_ms = min(own clock, now_ms)` (§5.7).
    pub now_ms: u64,
    /// Oldest client version admitted (`426 client_too_old` below it).
    pub min_client: String,
    /// Admission mode.
    pub admission: Admission,
    /// Terms page, outside `D`.
    pub terms_url: String,
    /// Current terms revision, an explicit config value (§7.6).
    pub terms_rev: String,
    /// Abuse page, outside `D`.
    pub abuse_url: String,
    /// `names_max` reached: a first publish ends with "This edge is full" (§9.4).
    pub full: bool,
}

/// `POST /v1/hello`, repeated with each renewed delegation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol major, `1`.
    pub v: u32,
    /// `{body, sig}` of a [`crate::Delegation`].
    pub delegation: Signed,
    /// The ticket's invite, required in a mesh's first `hello` under `admission: invite`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<Invite>,
    /// `theuth/<ver>` ([`crate::version::client_agent`]).
    pub client: String,
    /// The terms revision the owner accepted on the first-run card (§7.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_rev: Option<String>,
}

/// Lifecycle of a name at the edge (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameState {
    /// Exists, no CSR yet (evicted after `allocated_ttl`).
    Allocated,
    /// Waiting for a certificate slot.
    Queued,
    /// Holds a slot; counted against `names_max`; ordering.
    Slotted,
    /// Has a valid certificate.
    Valid,
    /// Released (cooldown runs).
    Released,
    /// Banned by the operator.
    Banned,
}

/// A queue position with its ETA (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePos {
    /// 1-based position within the job's class.
    pub position: u32,
    /// Estimated seconds until the slot.
    pub eta_s: u64,
}

/// One of `welcome.names`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameInfo {
    /// The label under `D`.
    pub name: String,
    /// Random or reserved.
    pub kind: NameKind,
    /// Edge-side state.
    pub state: NameState,
    /// The issuing CA (`letsencrypt`, `gts`), once ordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    /// SHA-256 of the SPKI of the CSR the edge holds; an engine whose vault row differs waits or heals (§6.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spki: Option<SpkiHash>,
    /// The certificate's `notAfter`, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<u64>,
    /// Queue position while `queued`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueuePos>,
}

/// `{host, engine}`: a route and the engine holding it (`welcome.routes`, `route_conflict.conflicts`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteEntry {
    /// `<name>.D` or `<label>.<name>.D`.
    pub host: String,
    /// The engine that registered it.
    pub engine: PublicKey,
}

/// Whether the interstitial may be switched off (§9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterstitialRule {
    /// The owner may turn it off per share.
    #[default]
    Optional,
    /// An `open` edge: the client hides the off switch.
    Required,
}

/// `welcome.limits`: what the client needs to warn and refuse locally. The design names only `interstitial` and
/// "the expiry windows" (§5.2, §9.4); the rest are the edge config values of the same names. Every field defaults to
/// the design's default so the struct grows additively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WelcomeLimits {
    /// Interstitial switch rule.
    pub interstitial: InterstitialRule,
    /// A random name expires after this long without an attach by any engine of the mesh (30 days).
    pub random_idle_ms: u64,
    /// A reserved name expires after this long without an attach (90 days).
    pub reserved_idle_ms: u64,
    /// Warnings are due this long before expiry (14 and 3 days).
    pub expiry_warn_ms: Vec<u64>,
    /// Release cooldown floor (30 days); the cooldown is `max(released + this, not_after)`.
    pub reclaim_cooldown_ms: u64,
    /// Reserved names per mesh (1 in P1).
    pub reserved_per_mesh: u32,
    /// Engines attached per mesh (8).
    pub engines_per_mesh: u32,
    /// Routes per tunnel (64).
    pub routes_per_tunnel: u32,
    /// Visitor streams per tunnel (256).
    pub conns_per_tunnel: u32,
    /// `delegation_max_ttl` (25 h).
    pub delegation_max_ttl_ms: u64,
}

impl Default for WelcomeLimits {
    fn default() -> Self {
        const DAY: u64 = 86_400_000;
        WelcomeLimits {
            interstitial: InterstitialRule::Optional,
            random_idle_ms: 30 * DAY,
            reserved_idle_ms: 90 * DAY,
            expiry_warn_ms: vec![14 * DAY, 3 * DAY],
            reclaim_cooldown_ms: 30 * DAY,
            reserved_per_mesh: 1,
            engines_per_mesh: 8,
            routes_per_tunnel: 64,
            conns_per_tunnel: crate::limits::VISITOR_STREAMS_PER_TUNNEL,
            delegation_max_ttl_ms: crate::limits::DELEGATION_MAX_TTL_MS,
        }
    }
}

/// `200` answer to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// Protocol major.
    pub v: u32,
    /// Edge clock.
    pub now_ms: u64,
    /// Oldest client version admitted.
    pub min_client: String,
    /// The mesh id the connection is bound to.
    pub mesh: PublicKey,
    /// Every name the mesh holds here.
    pub names: Vec<NameInfo>,
    /// Every route of the mesh, all engines.
    pub routes: Vec<RouteEntry>,
    /// The mesh's revoke list.
    pub revoked: Vec<PublicKey>,
    /// Last Stop all; an engine stops every share with `live_ms < stop_all_ms` before sending routes (§7.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_all_ms: Option<u64>,
    /// `D`.
    pub domain: String,
    /// Limits the client applies locally.
    #[serde(default)]
    pub limits: WelcomeLimits,
    /// Terms page.
    pub terms_url: String,
    /// Abuse page.
    pub abuse_url: String,
    /// Current terms revision.
    pub terms_rev: String,
    /// This `hello` carried the current `terms_rev` (name and CSR requests need it).
    pub terms_ok: bool,
    /// `names_max` reached.
    pub full: bool,
    /// Sent by an edge running on its standby key: a signed [`crate::Retire`] of the old live key (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire: Option<Signed>,
}

/// `GET /v1/names/{name}` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameAvailability {
    /// Whether this mesh could claim it now.
    pub available: bool,
    /// Why not (`taken`, `cooldown`, or a [`crate::NameError::reason`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `POST /v1/names` body: a client-drawn random candidate ([`crate::names::random_name`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRequest {
    /// The candidate.
    pub name: String,
}

/// `201` of `POST /v1/names`, `200` of `PUT /v1/names/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameGranted {
    /// The name now held.
    pub name: String,
}

/// `200` of `DELETE /v1/names/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameReleased {
    /// The released name.
    pub name: String,
    /// Until then only this mesh may claim it again.
    pub reclaim_after_ms: u64,
}

/// `POST /v1/names/{name}/transfer` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRequest {
    /// The receiving mesh id.
    pub to_mesh: PublicKey,
}

/// `200` of a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferAccepted {
    /// The offer lapses then (7 days).
    pub transfer_until_ms: u64,
}

/// `PUT /v1/routes` body: this engine's full set; `204` or `409 route_conflict` with nothing applied.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoutesPut {
    /// Hosts, each `<name>.D` or `<label>.<name>.D` of a held name.
    pub hosts: Vec<String>,
}

/// Query of `POST /v1/names/{name}/csr` (body: CSR DER).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CsrQuery {
    /// A rotation to a new key (§6.3).
    #[serde(default)]
    pub rotate: bool,
}

/// `200` of the CSR post and of `GET /v1/names/{name}/cert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertChain {
    /// Leaf first, PEM.
    pub chain_pem: String,
    /// Leaf `notAfter`, Unix ms.
    pub not_after: u64,
}

/// `202` of the CSR post: ordering, or queued for a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertPending {
    /// Estimated seconds to a chain.
    pub eta_s: u64,
    /// Queue position while waiting for a slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_pos: Option<u32>,
}

/// One of `GET /v1/mesh`'s engines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshEngine {
    /// The engine's endpoint id.
    pub engine: PublicKey,
    /// Has a tunnel now.
    pub attached: bool,
    /// Last attach.
    pub last_ms: u64,
    /// On the revoke list since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_ms: Option<u64>,
}

/// `GET /v1/mesh` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshStatus {
    /// The edge's current mesh id for this mesh (after a rekey, the new one).
    pub mesh: PublicKey,
    /// Every engine seen.
    pub engines: Vec<MeshEngine>,
    /// Every route.
    pub routes: Vec<RouteEntry>,
    /// Last Stop all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_all_ms: Option<u64>,
}

/// `200` of `POST /v1/mesh/stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopAll {
    /// When the edge stopped every tunnel of the mesh.
    pub stop_all_ms: u64,
}

/// `POST /v1/mesh/revoke` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeRequest {
    /// Engines to append to the mesh's revoke list.
    pub engines: Vec<PublicKey>,
}

/// `200` of a revoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeResult {
    /// The mesh's revoke list after the append.
    pub revoked: Vec<PublicKey>,
}

/// `200` of `POST /v1/mesh/rekey` (body: [`crate::SignedRekey`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekeyAccepted {
    /// The new mesh id.
    pub mesh: PublicKey,
}
