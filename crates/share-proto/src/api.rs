//! Requests and answers of the control API on `theuth/share/1` (§5.3). Field names are the design's, exactly.
//! Times are Unix milliseconds (`*_ms`, and `not_after`), durations seconds (`*_s`, `secs`). Every struct ignores
//! unknown fields, and fields added later must be `#[serde(default)]` (the versioning rule, [`crate::version`]).

use std::collections::BTreeMap;

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
    /// A pending transfer offer of this name to another mesh (§5.2, D20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_to: Option<PublicKey>,
    /// When that offer lapses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_until_ms: Option<u64>,
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
    /// Per-tunnel bandwidth while not throttled, Mbit/s (`tunnel_mbit`, 50; §9.6).
    pub tunnel_mbit: u32,
    /// Per-tunnel bandwidth of non-invited meshes while monthly egress is above pace (`throttle_mbit`, 2; §9.6).
    pub throttle_mbit: u32,
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
            tunnel_mbit: 50,
            throttle_mbit: 2,
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
    /// This mesh's tunnels run at `limits.throttle_mbit` now (egress above the month's pace, §9.6); live changes
    /// arrive as `throttle` events.
    #[serde(default)]
    pub throttled: bool,
}

/// `GET /v1/names/{name}` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameAvailability {
    /// Whether this mesh could claim it now.
    pub available: bool,
    /// Why not (`taken`, `cooldown`, or a [`crate::NameError::reason`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// With `reason: cooldown`: when the previous owner's cooldown ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim_after_ms: Option<u64>,
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
    /// Per host, when its share went live on this engine (`shares.live_ms`). The edge refuses a host whose time is
    /// at or before the mesh's `stop_all_ms` with `409 stopped_all` (§7.4). A host absent here is checked only
    /// against the session's `hello` time, as before.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub published_ms: BTreeMap<String, u64>,
}

impl RoutesPut {
    /// The hosts the edge must refuse with `stopped_all`: those whose `published_ms` is at or before the mesh's
    /// last Stop all. Hosts without a time are not judged here.
    pub fn stopped_hosts(&self, stop_all_ms: u64) -> Vec<String> {
        self.hosts
            .iter()
            .filter(|h| self.published_ms.get(*h).is_some_and(|&t| t <= stop_all_ms))
            .cloned()
            .collect()
    }
}

/// Query of `POST /v1/names/{name}/csr` (body: CSR DER).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CsrQuery {
    /// A rotation to a new key (§6.3).
    #[serde(default)]
    pub rotate: bool,
    /// With `rotate`: the old key may be compromised (a removed engine held it), so the edge revokes the old
    /// certificate with `keyCompromise` instead of `superseded` (§6.3). Wire: `?rotate=true&compromised=true`.
    #[serde(default)]
    pub compromised: bool,
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
    /// The mesh's revoke list (replaced by each rekey, §5.7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked: Vec<PublicKey>,
    /// Rekeys the edge applied to this mesh, oldest first; the last `new` is `mesh`. A client in `confirming`
    /// polls until `mesh` equals its settled key (§5.7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rekeys: Vec<RekeyEntry>,
}

/// One applied rekey in `GET /v1/mesh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekeyEntry {
    /// The mesh id before.
    pub old: PublicKey,
    /// The mesh id after.
    pub new: PublicKey,
    /// Edge time the rekey was applied.
    pub ms: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn pk(b: u8) -> PublicKey {
        SecretKey::from_bytes(&[b; 32]).public()
    }

    #[test]
    fn routes_put_published_ms_and_old_json() {
        let old: RoutesPut = serde_json::from_str(r#"{"hosts":["a.sky.tun.zip"]}"#).unwrap();
        assert!(old.published_ms.is_empty());
        assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"hosts":["a.sky.tun.zip"]}"#);
        assert!(old.stopped_hosts(u64::MAX).is_empty());

        let r = RoutesPut {
            hosts: vec!["a.sky.tun.zip".into(), "b.sky.tun.zip".into(), "c.sky.tun.zip".into()],
            published_ms: [("a.sky.tun.zip".to_owned(), 100), ("b.sky.tun.zip".to_owned(), 200)].into(),
        };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<RoutesPut>(&j).unwrap(), r);
        // Stop all at 100: `a` went live at 100 (stopped), `b` after, `c` has no time.
        assert_eq!(r.stopped_hosts(100), vec!["a.sky.tun.zip".to_owned()]);
        assert!(r.stopped_hosts(99).is_empty());
    }

    #[test]
    fn mesh_status_rekeys_and_old_json() {
        let old = format!(
            r#"{{"mesh":"{}","engines":[{{"engine":"{}","attached":true,"last_ms":5}}],"routes":[]}}"#,
            pk(1),
            pk(2)
        );
        let m: MeshStatus = serde_json::from_str(&old).unwrap();
        assert!(m.revoked.is_empty() && m.rekeys.is_empty() && m.stop_all_ms.is_none());
        let m = MeshStatus {
            revoked: vec![pk(3)],
            rekeys: vec![RekeyEntry {
                old: pk(4),
                new: pk(1),
                ms: 9,
            }],
            ..m
        };
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<MeshStatus>(&j).unwrap(), m);
    }

    #[test]
    fn welcome_and_names_old_json() {
        let old = format!(
            r#"{{"v":1,"now_ms":1,"min_client":"2.0.0","mesh":"{}","names":[{{"name":"sky","kind":"reserved","state":"queued","queue":{{"position":2,"eta_s":60}}}}],
            "routes":[],"revoked":[],"domain":"tun.zip","limits":{{"interstitial":"required"}},"terms_url":"t","abuse_url":"a","terms_rev":"1","terms_ok":true,"full":false}}"#,
            pk(1)
        );
        let w: Welcome = serde_json::from_str(&old).unwrap();
        assert!(!w.throttled);
        assert_eq!((w.limits.tunnel_mbit, w.limits.throttle_mbit), (50, 2));
        assert_eq!(w.limits.interstitial, InterstitialRule::Required);
        let n = &w.names[0];
        assert_eq!(n.queue, Some(QueuePos { position: 2, eta_s: 60 }));
        assert_eq!((n.transfer_to, n.transfer_until_ms), (None, None));

        let mut w2 = w.clone();
        w2.throttled = true;
        w2.names[0].transfer_to = Some(pk(7));
        w2.names[0].transfer_until_ms = Some(77);
        let j = serde_json::to_string(&w2).unwrap();
        assert_eq!(serde_json::from_str::<Welcome>(&j).unwrap(), w2);

        let a: NameAvailability =
            serde_json::from_str(r#"{"available":false,"reason":"cooldown"}"#).unwrap();
        assert_eq!(a.reclaim_after_ms, None);
    }

    #[test]
    fn csr_query_and_paths() {
        let q: CsrQuery = serde_json::from_str(r#"{"rotate":true}"#).unwrap();
        assert!(q.rotate && !q.compromised);
        assert_eq!(
            crate::paths::name_csr_query("sky", &CsrQuery { rotate: true, compromised: true }),
            "/v1/names/sky/csr?rotate=true&compromised=true"
        );
        assert_eq!(
            crate::paths::name_csr_query("sky", &q),
            crate::paths::name_csr("sky", true)
        );
        assert_eq!(
            crate::paths::name_csr_query("sky", &CsrQuery::default()),
            "/v1/names/sky/csr"
        );
    }

    #[test]
    fn queue_eta_rule() {
        let r = 7 * 86_400 / 22; // open bucket refill interval
        // Position 1 with a token free: only its own order.
        assert_eq!(crate::queue_eta_s(1, 1, r, 500), 0);
        // Position 1, no token: the next token.
        assert_eq!(crate::queue_eta_s(1, 0, r, 500), 500);
        // Position 3, one token: two short → one full interval + next token + 2 orders ahead.
        assert_eq!(crate::queue_eta_s(3, 1, r, 500), r + 500 + 2 * 180);
        assert!(crate::version::protocol_ok(crate::PROTOCOL_MAJOR));
        assert!(!crate::version::protocol_ok(crate::PROTOCOL_MAJOR + 1));
    }
}
