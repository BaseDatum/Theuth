//! Wire contract of public web sharing (design doc 210): the ALPN and versioning rule (§5.3), every request,
//! answer and event of the control API (§5.3), the error envelope, the mesh delegation and rekey signatures
//! (§5.7), the edge ticket (§5.5), the name, host and path rules (§5.2, §5.4), the CSR rule (§6.2), the PROXY v2
//! TLVs of the visitor stream (§5.3) and the admin socket protocol (§8.5).
//!
//! No theuth crate is a dependency (D14): `theuth-share-edge` and the `share` plugin both build on this alone.

pub mod admin;
pub mod api;
pub mod b64;
pub mod csr;
pub mod error;
pub mod events;
pub mod names;
pub mod proxy;
pub mod signed;
pub mod ticket;
pub mod version;

pub use api::*;
pub use csr::{CsrError, CsrInfo, SpkiHash, check_csr, expected_sans};
pub use admin::{AdminCommand, AdminRequest, AdminResponse, RevokeReason};
pub use error::{ErrorBody, ErrorCode, LimitKind, LimitScope};
pub use events::{Event, KillReason, ThrottleEvent};
pub use iroh_base::{EndpointAddr, PublicKey, SecretKey};
pub use names::{HostParts, NameError, NameKind};
pub use signed::{
    Delegation, DelegationError, EdgeIdentity, EdgePins, Rekey, RekeyConflict, RekeyError, Retire,
    RetireError, Signed, SignedRekey, rekey_conflict,
};
pub use ticket::{Invite, ShareEdgeTicket};
pub use version::{ALPN, ALPNS, PROTOCOL_MAJOR, SUPPORTED_MAJORS, Version};

/// Path constants of the control API (§5.3). `{name}` is the axum path-parameter syntax.
pub mod paths {
    /// `GET`: answered before `hello`.
    pub const INFO: &str = "/v1/info";
    /// `POST`: delegation, invite, client version, terms revision.
    pub const HELLO: &str = "/v1/hello";
    /// `POST`: the mesh's names, answered without a session: `hello`'s checks, no attach (§5.2).
    pub const NAMES_STATUS: &str = "/v1/names/status";
    /// `POST`: allocate a client-drawn random name.
    pub const NAMES: &str = "/v1/names";
    /// `GET` availability · `PUT` claim a reserved name · `DELETE` release.
    pub const NAME: &str = "/v1/names/{name}";
    /// `POST`: transfer to another mesh.
    pub const NAME_TRANSFER: &str = "/v1/names/{name}/transfer";
    /// `POST`: CSR DER body, `?rotate=true` for a rotation.
    pub const NAME_CSR: &str = "/v1/names/{name}/csr";
    /// `GET`: the current chain.
    pub const NAME_CERT: &str = "/v1/names/{name}/cert";
    /// `PUT`: this engine's full route set.
    pub const ROUTES: &str = "/v1/routes";
    /// `GET`: the SSE stream, held for the connection.
    pub const EVENTS: &str = "/v1/events";
    /// `GET`: the mesh's engines, routes and Stop all time.
    pub const MESH: &str = "/v1/mesh";
    /// `POST`: Stop all.
    pub const MESH_STOP: &str = "/v1/mesh/stop";
    /// `POST`: revoke engines.
    pub const MESH_REVOKE: &str = "/v1/mesh/revoke";
    /// `POST`: a signed `Rekey`.
    pub const MESH_REKEY: &str = "/v1/mesh/rekey";

    /// `/v1/names/<name>`.
    pub fn name(name: &str) -> String {
        format!("/v1/names/{name}")
    }
    /// `/v1/names/<name>/transfer`.
    pub fn name_transfer(name: &str) -> String {
        format!("/v1/names/{name}/transfer")
    }
    /// `/v1/names/<name>/csr`, with `?rotate=true` when `rotate`.
    pub fn name_csr(name: &str, rotate: bool) -> String {
        if rotate {
            format!("/v1/names/{name}/csr?rotate=true")
        } else {
            format!("/v1/names/{name}/csr")
        }
    }
    /// `/v1/names/<name>/csr` with the query of `q` (`rotate`, `compromised`).
    pub fn name_csr_query(name: &str, q: &crate::CsrQuery) -> String {
        match (q.rotate, q.compromised) {
            (false, _) => format!("/v1/names/{name}/csr"),
            (true, false) => format!("/v1/names/{name}/csr?rotate=true"),
            (true, true) => format!("/v1/names/{name}/csr?rotate=true&compromised=true"),
        }
    }
    /// `/v1/names/<name>/cert`.
    pub fn name_cert(name: &str) -> String {
        format!("/v1/names/{name}/cert")
    }
}

/// Protocol constants that both sides must agree on (§4.2, §5.3, §5.7).
pub mod limits {
    /// JSON request and answer bodies are at most this many bytes (§5.3).
    pub const MAX_JSON_BODY: usize = 64 * 1024;
    /// Visitor streams per tunnel: the edge's cap and the client's `max_concurrent_bidi_streams` (§5.3).
    pub const VISITOR_STREAMS_PER_TUNNEL: u32 = 256;
    /// The edge gives `open_bi` this long (§5.3).
    pub const OPEN_BI_TIMEOUT_MS: u64 = 5_000;
    /// Delegation lifetime the lease holder signs (§5.7).
    pub const DELEGATION_TTL_MS: u64 = 24 * 3_600_000;
    /// Default `delegation_max_ttl` on the edge (§5.7).
    pub const DELEGATION_MAX_TTL_MS: u64 = 25 * 3_600_000;
    /// A delegation is accepted from `issued_ms − DELEGATION_SKEW_MS` (§5.7).
    pub const DELEGATION_SKEW_MS: u64 = 5 * 60_000;
    /// Clients renew at this age, with jitter (§5.7).
    pub const DELEGATION_RENEW_MS: u64 = 12 * 3_600_000;
    /// Above this holder-versus-edge clock difference the client shows "clock is off" (§5.7).
    pub const CLOCK_WARN_MS: u64 = 5 * 60_000;
    /// `rekey_settle_secs`: after writing `mesh/share_key`, wait for `synced()` and this long, then re-read the
    /// row; its value is the settled key (§5.7, P0-6).
    pub const REKEY_SETTLE_SECS: u64 = 60;
    /// `rekey_heal_secs`: a `contested` rekey waits this long for a chain heal before the operator (§5.7).
    pub const REKEY_HEAL_SECS: u64 = 3_600;
    /// Rekey retry backoff against an unreachable edge: from this, doubling (§5.7).
    pub const REKEY_BACKOFF_MIN_MS: u64 = 60_000;
    /// … up to this.
    pub const REKEY_BACKOFF_MAX_MS: u64 = 3_600_000;
    /// A transfer offer lapses after this (§5.2, D20).
    pub const TRANSFER_TTL_MS: u64 = 7 * 86_400_000;
    /// An allocated name without a CSR is evicted after this (`allocated_ttl`, §9.4).
    pub const ALLOCATED_TTL_MS: u64 = 3_600_000;
    /// The client dials the pinned standby after the live key was unreachable this long (§5.5).
    pub const STANDBY_AFTER_MS: u64 = 10 * 60_000;
    /// `stats` events are sent every this many seconds (§5.3).
    pub const STATS_PERIOD_S: u64 = 30;
    /// Per-order time added to a queue ETA for every job ahead (§6.5).
    pub const ORDER_ETA_S: u64 = 180;
}

/// The ETA of §6.5 for queue position `position` (1-based) with `tokens` whole tokens available, a bucket refill
/// interval of `refill_s` and `next_token_s` until the next token. With `short = max(0, p − t)`:
/// `(short == 0 ? 0 : (short − 1) × r + next_token) + 3 min × (p − 1)`. The design writes `short × r + next_token`,
/// which counts the next token twice; this is the same rule without that.
pub fn queue_eta_s(position: u32, tokens: u32, refill_s: u64, next_token_s: u64) -> u64 {
    let p = u64::from(position.max(1));
    let short = p.saturating_sub(u64::from(tokens));
    let wait = if short == 0 {
        0
    } else {
        (short - 1) * refill_s + next_token_s
    };
    wait + (p - 1) * limits::ORDER_ETA_S
}
