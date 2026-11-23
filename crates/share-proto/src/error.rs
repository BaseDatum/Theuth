//! The error envelope `4xx {code, …}` of the control API (§5.3) and every code the design names.

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::api::RouteEntry;
use crate::csr::SpkiHash;

/// Every error code of §5.3 (and §9.4, §6.2), snake_case on the wire. Codes are additive within a major, so an
/// unknown code decodes as [`ErrorCode::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The mesh, name, engine key or network is banned (§9.3).
    Banned,
    /// A per-network limit (§9.4).
    NetworkQuota,
    /// `admission: closed`.
    AdmissionClosed,
    /// `admission: invite` and no valid invite in the mesh's first `hello`.
    InviteRequired,
    /// The delegation does not decode, verify, or match this edge, domain or peer (§5.7).
    BadDelegation,
    /// The engine is on the mesh's revoke list.
    Revoked,
    /// `engines_per_mesh` engines are already attached, or too many engine keys first seen today.
    EnginesQuota,
    /// `now < issued_ms − 5 min`; carries `now_ms`.
    DelegationNotYetValid,
    /// `now ≥ expires_ms`; carries `now_ms`.
    DelegationExpired,
    /// The mesh was rekeyed; carries `to`.
    MeshRekeyed,
    /// `client` is older than `min_client` (HTTP 426).
    ClientTooOld,
    /// First claim won by another mesh.
    NameTaken,
    /// The name is in its release cooldown; may carry `reclaim_after_ms`.
    Cooldown,
    /// A per-mesh quota.
    Quota,
    /// The last `hello` did not carry the current `terms_rev` (§7.6).
    TermsRequired,
    /// The global paced bucket is empty; carries `retry_after_s`.
    Paced,
    /// `names_max` or `queue_max` reached (§9.4).
    EdgeFull,
    /// A malformed name, host, path or body; carries `reason`.
    Invalid,
    /// The engine's mesh does not hold the name.
    NotOwner,
    /// A host is routed by another engine; carries `conflicts`, nothing was applied (§5.4).
    RouteConflict,
    /// A different CSR without `rotate`; carries the `spki` the edge holds (§6.2).
    CsrMismatch,
    /// The CSR's SANs are not `{<name>.D, *.<name>.D}` of a held name (§6.2).
    CsrNames,
    /// A `Rekey` signature does not verify (§5.7).
    BadSignature,
    /// Not in the design's tables: a request other than `GET /v1/info` before a valid `hello` (§5.3).
    HelloRequired,
    /// Not in the design's tables: the `404` of `GET /v1/names/{name}/cert` and the admin socket's `not_found`.
    NotFound,
    /// Not in the design's tables: a per-IP or per-network rate limit (`/v1/info`, name checks).
    RateLimited,
    /// Not in the design's tables: a JSON body above 64 KiB.
    TooLarge,
    /// Not in the design's tables: an edge-side failure.
    Internal,
    /// A code this build does not know (added by a later edge).
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// The HTTP status the edge answers this code with (§5.3).
    pub fn status(self) -> u16 {
        use ErrorCode::*;
        match self {
            Invalid => 400,
            Banned
            | NetworkQuota
            | AdmissionClosed
            | InviteRequired
            | BadDelegation
            | Revoked
            | EnginesQuota
            | DelegationNotYetValid
            | DelegationExpired
            | Quota
            | TermsRequired
            | NotOwner
            | CsrNames
            | BadSignature
            | HelloRequired => 403,
            NotFound => 404,
            MeshRekeyed | NameTaken | Cooldown | RouteConflict | CsrMismatch => 409,
            TooLarge => 413,
            ClientTooOld => 426,
            Paced | RateLimited => 429,
            Internal | Unknown => 500,
            EdgeFull => 503,
        }
    }

    /// The wire string (`name_taken`, …).
    pub fn as_str(self) -> &'static str {
        use ErrorCode::*;
        match self {
            Banned => "banned",
            NetworkQuota => "network_quota",
            AdmissionClosed => "admission_closed",
            InviteRequired => "invite_required",
            BadDelegation => "bad_delegation",
            Revoked => "revoked",
            EnginesQuota => "engines_quota",
            DelegationNotYetValid => "delegation_not_yet_valid",
            DelegationExpired => "delegation_expired",
            MeshRekeyed => "mesh_rekeyed",
            ClientTooOld => "client_too_old",
            NameTaken => "name_taken",
            Cooldown => "cooldown",
            Quota => "quota",
            TermsRequired => "terms_required",
            Paced => "paced",
            EdgeFull => "edge_full",
            Invalid => "invalid",
            NotOwner => "not_owner",
            RouteConflict => "route_conflict",
            CsrMismatch => "csr_mismatch",
            CsrNames => "csr_names",
            BadSignature => "bad_signature",
            HelloRequired => "hello_required",
            NotFound => "not_found",
            RateLimited => "rate_limited",
            TooLarge => "too_large",
            Internal => "internal",
            Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The body of every non-2xx answer: `{code, …}` plus the code's own fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The code.
    pub code: ErrorCode,
    /// The edge clock, for `delegation_not_yet_valid` / `delegation_expired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<u64>,
    /// The mesh id a rekeyed mesh moved to (`mesh_rekeyed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<PublicKey>,
    /// Seconds until the paced bucket refills (`paced`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
    /// Why (`invalid`: `brand`, `reserved`, `length`, …; see [`crate::NameError::reason`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The hosts held by other engines (`route_conflict`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<RouteEntry>,
    /// The SPKI hash of the CSR the edge holds (`csr_mismatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spki: Option<SpkiHash>,
    /// When the previous owner's cooldown ends (`cooldown`; additive, not in the design's table).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim_after_ms: Option<u64>,
    /// Human-readable detail for logs; clients show their own text per code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ErrorBody {
    /// A body with only `code`.
    pub fn new(code: ErrorCode) -> Self {
        ErrorBody {
            code,
            now_ms: None,
            to: None,
            retry_after_s: None,
            reason: None,
            conflicts: Vec::new(),
            spki: None,
            reclaim_after_ms: None,
            message: None,
        }
    }

    /// `delegation_not_yet_valid` / `delegation_expired` with the edge clock.
    pub fn with_now(code: ErrorCode, now_ms: u64) -> Self {
        ErrorBody {
            now_ms: Some(now_ms),
            ..Self::new(code)
        }
    }

    /// `invalid` with a reason.
    pub fn invalid(reason: impl Into<String>) -> Self {
        ErrorBody {
            reason: Some(reason.into()),
            ..Self::new(ErrorCode::Invalid)
        }
    }

    /// `mesh_rekeyed` naming the new mesh id.
    pub fn rekeyed(to: PublicKey) -> Self {
        ErrorBody {
            to: Some(to),
            ..Self::new(ErrorCode::MeshRekeyed)
        }
    }

    /// `paced` with a retry delay.
    pub fn paced(retry_after_s: u64) -> Self {
        ErrorBody {
            retry_after_s: Some(retry_after_s),
            ..Self::new(ErrorCode::Paced)
        }
    }

    /// `route_conflict` with the holders.
    pub fn route_conflict(conflicts: Vec<RouteEntry>) -> Self {
        ErrorBody {
            conflicts,
            ..Self::new(ErrorCode::RouteConflict)
        }
    }

    /// `csr_mismatch` with the SPKI the edge holds.
    pub fn csr_mismatch(spki: SpkiHash) -> Self {
        ErrorBody {
            spki: Some(spki),
            ..Self::new(ErrorCode::CsrMismatch)
        }
    }

    /// The HTTP status for this body.
    pub fn status(&self) -> u16 {
        self.code.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_unknown_tolerated() {
        let b = ErrorBody::with_now(ErrorCode::DelegationNotYetValid, 42);
        let j = serde_json::to_string(&b).unwrap();
        assert_eq!(j, r#"{"code":"delegation_not_yet_valid","now_ms":42}"#);
        assert_eq!(serde_json::from_str::<ErrorBody>(&j).unwrap(), b);
        let u: ErrorBody = serde_json::from_str(r#"{"code":"from_the_future","extra":1}"#).unwrap();
        assert_eq!(u.code, ErrorCode::Unknown);
        for c in [
            ErrorCode::NameTaken,
            ErrorCode::CsrMismatch,
            ErrorCode::EdgeFull,
            ErrorCode::ClientTooOld,
        ] {
            let s = serde_json::to_string(&c).unwrap();
            assert_eq!(s, format!("\"{}\"", c.as_str()));
        }
        assert_eq!(ErrorCode::ClientTooOld.status(), 426);
        assert_eq!(ErrorCode::EdgeFull.status(), 503);
        assert_eq!(ErrorCode::RouteConflict.status(), 409);
    }
}
