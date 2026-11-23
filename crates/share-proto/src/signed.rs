//! Signed records of §5.7: the mesh [`Delegation`], the [`Rekey`] and the edge's [`Retire`] notice (§5.5).
//!
//! Every record is postcard-encoded into `body`; the signature is Ed25519 (`iroh_base`) over a domain-separation
//! tag followed by `body`. On the wire `body` and the signatures are base64url, so the verifier checks exactly the
//! bytes that were signed and never re-encodes.

use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::b64;
use crate::error::ErrorCode;
use crate::limits::{DELEGATION_SKEW_MS, DELEGATION_TTL_MS};

/// Tag of [`Delegation`] signatures (§5.7).
pub const DELEGATION_TAG: &[u8] = b"theuth/share/delegation/1\0";
/// Tag of [`Rekey`] signatures (§5.7: "under their own tags").
pub const REKEY_TAG: &[u8] = b"theuth/share/rekey/1\0";
/// Tag of [`Retire`] signatures (§5.5; the design names no tag, this follows the scheme).
pub const RETIRE_TAG: &[u8] = b"theuth/share/retire/1\0";

/// `{body, sig}`: a postcard body and one Ed25519 signature, both base64url on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed {
    /// postcard bytes of the record.
    #[serde(with = "b64::bytes")]
    pub body: Vec<u8>,
    /// Ed25519 over `tag ‖ body`.
    #[serde(with = "b64::array")]
    pub sig: [u8; 64],
}

fn message(tag: &[u8], body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(tag.len() + body.len());
    m.extend_from_slice(tag);
    m.extend_from_slice(body);
    m
}

fn sign_raw(tag: &[u8], body: &[u8], key: &SecretKey) -> [u8; 64] {
    key.sign(&message(tag, body)).to_bytes()
}

fn verify_raw(tag: &[u8], body: &[u8], sig: &[u8; 64], key: &PublicKey) -> bool {
    key.verify(&message(tag, body), &Signature::from_bytes(sig))
        .is_ok()
}

fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_stdvec(v).expect("postcard encoding of an in-memory record cannot fail")
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    postcard::from_bytes(body).ok()
}

/// The edge's own identity, what a delegation or rekey must name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeIdentity {
    /// The edge's endpoint id.
    pub edge: PublicKey,
    /// `D`.
    pub domain: String,
}

/// A mesh delegation: the mesh share key lets one engine key act for the mesh at one edge for ≤ 24 h (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation {
    /// Record version, [`Delegation::VERSION`].
    pub v: u8,
    /// `D`, from `GET /v1/info`.
    pub domain: String,
    /// The edge's endpoint id: good at this edge only.
    pub edge: PublicKey,
    /// The mesh id (public half of `mesh/share_key`).
    pub mesh: PublicKey,
    /// The engine's iroh endpoint id; must equal the QUIC peer.
    pub engine: PublicKey,
    /// `min(holder clock, edge now_ms)`.
    pub issued_ms: u64,
    /// `issued_ms + 24 h`.
    pub expires_ms: u64,
}

/// Why a delegation is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegationError {
    /// `body` is not a postcard `Delegation`.
    #[error("delegation body does not decode")]
    Decode,
    /// `v` is not 1.
    #[error("delegation version {0} unsupported")]
    Version(u8),
    /// Signing key is not the delegation's `mesh`.
    #[error("signing key is not the delegation's mesh key")]
    KeyMismatch,
    /// Signature does not verify under `mesh`.
    #[error("delegation signature does not verify")]
    BadSignature,
    /// `engine` is not the QUIC peer.
    #[error("delegation engine is not the connected peer")]
    EngineNotPeer,
    /// `edge` is not this edge.
    #[error("delegation is for another edge")]
    WrongEdge,
    /// `domain` is not this edge's.
    #[error("delegation is for another domain")]
    WrongDomain,
    /// `expires_ms ≤ issued_ms` or the lifetime exceeds `delegation_max_ttl`.
    #[error("delegation lifetime invalid or above the edge's maximum")]
    Lifetime,
    /// `now < issued_ms − 5 min`.
    #[error("delegation not yet valid (edge now {now_ms})")]
    NotYetValid {
        /// Edge clock.
        now_ms: u64,
    },
    /// `now ≥ expires_ms`.
    #[error("delegation expired (edge now {now_ms})")]
    Expired {
        /// Edge clock.
        now_ms: u64,
    },
}

impl DelegationError {
    /// The `403` code the edge answers with (§5.3); clock errors also carry `now_ms` ([`Self::now_ms`]).
    pub fn code(&self) -> ErrorCode {
        match self {
            DelegationError::NotYetValid { .. } => ErrorCode::DelegationNotYetValid,
            DelegationError::Expired { .. } => ErrorCode::DelegationExpired,
            _ => ErrorCode::BadDelegation,
        }
    }

    /// The edge clock for the two clock codes.
    pub fn now_ms(&self) -> Option<u64> {
        match self {
            DelegationError::NotYetValid { now_ms } | DelegationError::Expired { now_ms } => {
                Some(*now_ms)
            }
            _ => None,
        }
    }
}

impl Delegation {
    /// The record version this build signs and accepts.
    pub const VERSION: u8 = 1;

    /// The lease holder's delegation: `issued_ms = min(holder_now_ms, edge_now_ms)`, lifetime 24 h (§5.7).
    pub fn new(
        domain: impl Into<String>,
        edge: PublicKey,
        mesh: PublicKey,
        engine: PublicKey,
        holder_now_ms: u64,
        edge_now_ms: u64,
    ) -> Self {
        let issued_ms = holder_now_ms.min(edge_now_ms);
        Delegation {
            v: Self::VERSION,
            domain: domain.into(),
            edge,
            mesh,
            engine,
            issued_ms,
            expires_ms: issued_ms + DELEGATION_TTL_MS,
        }
    }

    /// Signs with `mesh/share_key`, which must be this delegation's `mesh`.
    pub fn sign(&self, mesh_key: &SecretKey) -> Result<Signed, DelegationError> {
        if mesh_key.public() != self.mesh {
            return Err(DelegationError::KeyMismatch);
        }
        let body = encode(self);
        let sig = sign_raw(DELEGATION_TAG, &body, mesh_key);
        Ok(Signed { body, sig })
    }

    /// Decodes and checks the signature under the body's `mesh`, without the edge's context (the client reading
    /// its own delegation, or the first step of [`Self::verify`]).
    pub fn open(signed: &Signed) -> Result<Delegation, DelegationError> {
        let d: Delegation = decode(&signed.body).ok_or(DelegationError::Decode)?;
        if d.v != Self::VERSION {
            return Err(DelegationError::Version(d.v));
        }
        if !verify_raw(DELEGATION_TAG, &signed.body, &signed.sig, &d.mesh) {
            return Err(DelegationError::BadSignature);
        }
        Ok(d)
    }

    /// The edge's check on `hello` (§5.7): signature under `mesh`; `engine` equals the QUIC `peer`; `edge` and
    /// `domain` are the edge's own; `expires_ms − issued_ms ≤ max_ttl_ms`; `issued_ms − 5 min ≤ now < expires_ms`.
    /// Ban, rekey, revoke and engine-count checks need the edge's state and are the caller's.
    pub fn verify(
        signed: &Signed,
        peer: &PublicKey,
        edge: &EdgeIdentity,
        now_ms: u64,
        max_ttl_ms: u64,
    ) -> Result<Delegation, DelegationError> {
        let d = Self::open(signed)?;
        if d.engine != *peer {
            return Err(DelegationError::EngineNotPeer);
        }
        if d.edge != edge.edge {
            return Err(DelegationError::WrongEdge);
        }
        if d.domain != edge.domain {
            return Err(DelegationError::WrongDomain);
        }
        if d.expires_ms <= d.issued_ms || d.expires_ms - d.issued_ms > max_ttl_ms {
            return Err(DelegationError::Lifetime);
        }
        if now_ms < d.issued_ms.saturating_sub(DELEGATION_SKEW_MS) {
            return Err(DelegationError::NotYetValid { now_ms });
        }
        if now_ms >= d.expires_ms {
            return Err(DelegationError::Expired { now_ms });
        }
        Ok(d)
    }

    /// `holder − edge` in ms; the client shows "The clock on <machine> is off by N min" when
    /// [`Self::clock_off`] (§5.7).
    pub fn clock_skew_ms(holder_now_ms: u64, edge_now_ms: u64) -> i64 {
        holder_now_ms as i64 - edge_now_ms as i64
    }

    /// `|holder − edge| > 5 min`.
    pub fn clock_off(holder_now_ms: u64, edge_now_ms: u64) -> bool {
        holder_now_ms.abs_diff(edge_now_ms) > crate::limits::CLOCK_WARN_MS
    }
}

/// A mesh rekey (§5.7): every name, quota, invite and route of `old` moves to `new`; `revoke` *replaces* the mesh's
/// revoke list. Signed by both keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rekey {
    /// Record version, [`Rekey::VERSION`].
    pub v: u8,
    /// `D`.
    pub domain: String,
    /// The edge's endpoint id.
    pub edge: PublicKey,
    /// The current mesh id at the edge.
    pub old: PublicKey,
    /// The settled new mesh id.
    pub new: PublicKey,
    /// The mesh's revoke list after the rekey.
    pub revoke: Vec<PublicKey>,
    /// Holder clock.
    pub issued_ms: u64,
}

/// `POST /v1/mesh/rekey` body: `{body, sig_old, sig_new}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRekey {
    /// postcard bytes of the [`Rekey`].
    #[serde(with = "b64::bytes")]
    pub body: Vec<u8>,
    /// By `old`, over `REKEY_TAG ‖ body`.
    #[serde(with = "b64::array")]
    pub sig_old: [u8; 64],
    /// By `new`, over `REKEY_TAG ‖ body`.
    #[serde(with = "b64::array")]
    pub sig_new: [u8; 64],
}

/// Why a rekey is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RekeyError {
    /// `body` is not a postcard `Rekey`.
    #[error("rekey body does not decode")]
    Decode,
    /// `v` is not 1.
    #[error("rekey version {0} unsupported")]
    Version(u8),
    /// A signing key does not match `old` or `new`.
    #[error("signing keys do not match old and new")]
    KeyMismatch,
    /// `old == new`.
    #[error("rekey to the same key")]
    SameKey,
    /// For another edge.
    #[error("rekey is for another edge")]
    WrongEdge,
    /// For another domain.
    #[error("rekey is for another domain")]
    WrongDomain,
    /// `sig_old` does not verify under `old`.
    #[error("old-key signature does not verify")]
    BadOldSignature,
    /// `sig_new` does not verify under `new`.
    #[error("new-key signature does not verify")]
    BadNewSignature,
}

impl RekeyError {
    /// `403 bad_signature` for signature failures, `400 invalid` otherwise.
    pub fn code(&self) -> ErrorCode {
        match self {
            RekeyError::BadOldSignature | RekeyError::BadNewSignature => ErrorCode::BadSignature,
            _ => ErrorCode::Invalid,
        }
    }
}

impl Rekey {
    /// The record version this build signs and accepts.
    pub const VERSION: u8 = 1;

    /// Signs with the old and the new mesh share keys.
    pub fn sign(&self, old: &SecretKey, new: &SecretKey) -> Result<SignedRekey, RekeyError> {
        if old.public() != self.old || new.public() != self.new {
            return Err(RekeyError::KeyMismatch);
        }
        let body = encode(self);
        Ok(SignedRekey {
            sig_old: sign_raw(REKEY_TAG, &body, old),
            sig_new: sign_raw(REKEY_TAG, &body, new),
            body,
        })
    }

    /// The edge's stateless checks: decodes, names this edge and domain, `old ≠ new`, both signatures verify.
    /// Whether `old` is the mesh's current id (else `409 mesh_rekeyed {to}`) is the caller's state check.
    pub fn verify(signed: &SignedRekey, edge: &EdgeIdentity) -> Result<Rekey, RekeyError> {
        let r: Rekey = decode(&signed.body).ok_or(RekeyError::Decode)?;
        if r.v != Self::VERSION {
            return Err(RekeyError::Version(r.v));
        }
        if r.edge != edge.edge {
            return Err(RekeyError::WrongEdge);
        }
        if r.domain != edge.domain {
            return Err(RekeyError::WrongDomain);
        }
        if r.old == r.new {
            return Err(RekeyError::SameKey);
        }
        if !verify_raw(REKEY_TAG, &signed.body, &signed.sig_old, &r.old) {
            return Err(RekeyError::BadOldSignature);
        }
        if !verify_raw(REKEY_TAG, &signed.body, &signed.sig_new, &r.new) {
            return Err(RekeyError::BadNewSignature);
        }
        Ok(r)
    }
}

/// `welcome.retire`: an edge running on its standby key retires the old live key; clients never dial `retired`
/// again (§5.5). Signed by the standby key, which must be the connected edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retire {
    /// Record version, [`Retire::VERSION`].
    pub v: u8,
    /// The signing standby key, the edge now answering.
    pub edge: PublicKey,
    /// The retired live key.
    pub retired: PublicKey,
    /// Edge clock.
    pub issued_ms: u64,
}

/// Why a retire notice is ignored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RetireError {
    /// Not a postcard `Retire`, or an unknown version.
    #[error("retire notice does not decode")]
    Decode,
    /// Signing key is not `edge`.
    #[error("signing key is not the retire notice's edge")]
    KeyMismatch,
    /// Signed by another edge than the connected one, or the signature fails.
    #[error("retire notice is not signed by the connected edge")]
    BadSignature,
    /// Verified, but the signer is not the pinned standby or `retired` is not the pinned live key.
    #[error("retire notice does not match the pinned keys")]
    NotPinned,
}

impl Retire {
    /// The record version this build signs and accepts.
    pub const VERSION: u8 = 1;

    /// Signs with the standby key.
    pub fn sign(&self, standby: &SecretKey) -> Result<Signed, RetireError> {
        if standby.public() != self.edge {
            return Err(RetireError::KeyMismatch);
        }
        let body = encode(self);
        let sig = sign_raw(RETIRE_TAG, &body, standby);
        Ok(Signed { body, sig })
    }

    /// Accepts a notice only from the edge the client is connected to.
    pub fn verify(signed: &Signed, connected: &PublicKey) -> Result<Retire, RetireError> {
        let r: Retire = decode(&signed.body).ok_or(RetireError::Decode)?;
        if r.v != Self::VERSION {
            return Err(RetireError::Decode);
        }
        if r.edge != *connected || !verify_raw(RETIRE_TAG, &signed.body, &signed.sig, connected) {
            return Err(RetireError::BadSignature);
        }
        Ok(r)
    }
}

impl Rekey {
    /// A version-1 rekey of `old` to `new` at one edge; `revoke` becomes the mesh's whole revoke list there.
    pub fn new(
        domain: impl Into<String>,
        edge: PublicKey,
        old: PublicKey,
        new: PublicKey,
        revoke: Vec<PublicKey>,
        issued_ms: u64,
    ) -> Self {
        Rekey {
            v: Self::VERSION,
            domain: domain.into(),
            edge,
            old,
            new,
            revoke,
            issued_ms,
        }
    }

    /// The chained rekey of the heal rule (§5.7): the edge holds `to`, this engine's stash holds `to`'s secret, so
    /// it moves the mesh on from `to` to the settled key, keeping `revoke` and `issued_ms`.
    pub fn chain(&self, to: PublicKey) -> Rekey {
        Rekey {
            old: to,
            ..self.clone()
        }
    }
}

/// What a holder in `sending` does with `409 mesh_rekeyed {to}` (§5.7 state table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyConflict {
    /// `to` is the settled key: another holder already sent it. Go to `confirming`.
    Confirming,
    /// `to` is a key in this engine's stash: send [`Rekey::chain`] signed from the stash, then `confirming`.
    Chain {
        /// The edge's current mesh id, whose secret is in the stash.
        from: PublicKey,
    },
    /// Any other key: wait `rekey_heal_secs`, then operator.
    Contested,
}

/// Classifies a `409 mesh_rekeyed {to}` answer to a rekey.
pub fn rekey_conflict(to: &PublicKey, settled: &PublicKey, stash: &[PublicKey]) -> RekeyConflict {
    if to == settled {
        RekeyConflict::Confirming
    } else if stash.contains(to) {
        RekeyConflict::Chain { from: *to }
    } else {
        RekeyConflict::Contested
    }
}

/// The two endpoint keys a shipped default edge pins (§5.5, D15), plus the keys a [`Retire`] took out of use.
/// Clients dial `live`, and `standby` after `live` has been unreachable for
/// [`crate::limits::STANDBY_AFTER_MS`]; a valid retire notice from `standby` promotes it for good.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgePins {
    /// The key dialed first.
    pub live: PublicKey,
    /// The cold standby, if pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standby: Option<PublicKey>,
    /// Keys never dialed again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<PublicKey>,
}

impl EdgePins {
    /// Pins `live` and an optional `standby`.
    pub fn new(live: PublicKey, standby: Option<PublicKey>) -> Self {
        EdgePins {
            live,
            standby,
            retired: Vec::new(),
        }
    }

    /// Keys to try, in order: `live`, then `standby`; never a retired key.
    pub fn dial_order(&self) -> Vec<PublicKey> {
        std::iter::once(self.live)
            .chain(self.standby)
            .filter(|k| !self.retired.contains(k))
            .collect()
    }

    /// Applies `welcome.retire` from the `connected` edge: it must verify ([`Retire::verify`]), be signed by the
    /// pinned standby and retire the pinned live key. Then `standby` becomes `live` and the old key is retired.
    /// A notice already applied (`retired` already retired, signer already live) is accepted again unchanged.
    pub fn accept_retire(
        &mut self,
        signed: &Signed,
        connected: &PublicKey,
    ) -> Result<Retire, RetireError> {
        let r = Retire::verify(signed, connected)?;
        if self.live == r.edge && self.retired.contains(&r.retired) {
            return Ok(r);
        }
        if self.standby != Some(r.edge) || self.live != r.retired {
            return Err(RetireError::NotPinned);
        }
        self.retired.push(r.retired);
        self.live = r.edge;
        self.standby = None;
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 3_600_000;
    const T0: u64 = 1_800_000_000_000;

    fn key(b: u8) -> SecretKey {
        SecretKey::from_bytes(&[b; 32])
    }

    fn setup() -> (SecretKey, SecretKey, EdgeIdentity, Signed) {
        let mesh = key(1);
        let engine = key(2);
        let edge = EdgeIdentity {
            edge: key(3).public(),
            domain: "tun.zip".into(),
        };
        let d = Delegation::new("tun.zip", edge.edge, mesh.public(), engine.public(), T0, T0);
        let s = d.sign(&mesh).unwrap();
        (mesh, engine, edge, s)
    }

    #[test]
    fn delegation_good_and_wire_round_trip() {
        let (mesh, engine, edge, s) = setup();
        let json = serde_json::to_string(&s).unwrap();
        let back: Signed = serde_json::from_str(&json).unwrap();
        let d = Delegation::verify(&back, &engine.public(), &edge, T0 + H, 25 * H).unwrap();
        assert_eq!(d.mesh, mesh.public());
        assert_eq!(d.expires_ms, T0 + 24 * H);
    }

    #[test]
    fn delegation_tampered() {
        let (_, engine, edge, mut s) = setup();
        let last = s.body.len() - 1;
        s.body[last] ^= 1;
        let e = Delegation::verify(&s, &engine.public(), &edge, T0, 25 * H).unwrap_err();
        assert!(
            matches!(e, DelegationError::BadSignature | DelegationError::Decode),
            "{e:?}"
        );
        let (_, engine, edge, mut s) = setup();
        s.sig[0] ^= 1;
        assert_eq!(
            Delegation::verify(&s, &engine.public(), &edge, T0, 25 * H),
            Err(DelegationError::BadSignature)
        );
    }

    #[test]
    fn delegation_signed_by_another_key_is_refused() {
        let (_, engine, edge, _) = setup();
        let d = Delegation::new(
            "tun.zip",
            edge.edge,
            key(1).public(),
            engine.public(),
            T0,
            T0,
        );
        assert_eq!(d.sign(&key(9)), Err(DelegationError::KeyMismatch));
        let body = encode(&d);
        let forged = Signed {
            sig: sign_raw(DELEGATION_TAG, &body, &key(9)),
            body,
        };
        assert_eq!(
            Delegation::verify(&forged, &engine.public(), &edge, T0, 25 * H),
            Err(DelegationError::BadSignature)
        );
    }

    #[test]
    fn delegation_expired_and_skew() {
        let (_, engine, edge, s) = setup();
        let p = engine.public();
        let e = Delegation::verify(&s, &p, &edge, T0 + 24 * H, 25 * H).unwrap_err();
        assert_eq!(
            e,
            DelegationError::Expired {
                now_ms: T0 + 24 * H
            }
        );
        assert_eq!(e.code(), ErrorCode::DelegationExpired);
        // 5 min early is fine, one ms more is not.
        assert!(Delegation::verify(&s, &p, &edge, T0 - 5 * 60_000, 25 * H).is_ok());
        let e = Delegation::verify(&s, &p, &edge, T0 - 5 * 60_000 - 1, 25 * H).unwrap_err();
        assert_eq!(e.code(), ErrorCode::DelegationNotYetValid);
        assert_eq!(e.now_ms(), Some(T0 - 5 * 60_000 - 1));
    }

    #[test]
    fn delegation_issued_at_min_of_clocks() {
        let d = Delegation::new(
            "tun.zip",
            key(3).public(),
            key(1).public(),
            key(2).public(),
            T0 + H,
            T0,
        );
        assert_eq!(d.issued_ms, T0);
        assert!(Delegation::clock_off(T0 + H, T0));
        assert!(!Delegation::clock_off(T0 + 60_000, T0));
        assert_eq!(Delegation::clock_skew_ms(T0, T0 + 1000), -1000);
    }

    #[test]
    fn delegation_wrong_engine_edge_domain_ttl() {
        let (mesh, engine, edge, s) = setup();
        let e = Delegation::verify(&s, &key(7).public(), &edge, T0, 25 * H).unwrap_err();
        assert_eq!(e, DelegationError::EngineNotPeer);
        assert_eq!(e.code(), ErrorCode::BadDelegation);
        let other = EdgeIdentity {
            edge: key(8).public(),
            domain: "tun.zip".into(),
        };
        assert_eq!(
            Delegation::verify(&s, &engine.public(), &other, T0, 25 * H),
            Err(DelegationError::WrongEdge)
        );
        let other = EdgeIdentity {
            edge: edge.edge,
            domain: "example.org".into(),
        };
        assert_eq!(
            Delegation::verify(&s, &engine.public(), &other, T0, 25 * H),
            Err(DelegationError::WrongDomain)
        );
        let mut long = Delegation::open(&s).unwrap();
        long.expires_ms = long.issued_ms + 26 * H;
        let s = long.sign(&mesh).unwrap();
        assert_eq!(
            Delegation::verify(&s, &engine.public(), &edge, T0, 25 * H),
            Err(DelegationError::Lifetime)
        );
    }

    #[test]
    fn rekey_sign_verify() {
        let edge = EdgeIdentity {
            edge: key(3).public(),
            domain: "tun.zip".into(),
        };
        let (old, new) = (key(1), key(4));
        let r = Rekey {
            v: 1,
            domain: "tun.zip".into(),
            edge: edge.edge,
            old: old.public(),
            new: new.public(),
            revoke: vec![key(5).public()],
            issued_ms: T0,
        };
        let s = r.sign(&old, &new).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: SignedRekey = serde_json::from_str(&json).unwrap();
        assert_eq!(Rekey::verify(&back, &edge).unwrap(), r);

        let mut bad = s.clone();
        bad.sig_new[3] ^= 1;
        let e = Rekey::verify(&bad, &edge).unwrap_err();
        assert_eq!(e, RekeyError::BadNewSignature);
        assert_eq!(e.code(), ErrorCode::BadSignature);
        let mut bad = s.clone();
        bad.sig_old = bad.sig_new;
        assert_eq!(Rekey::verify(&bad, &edge), Err(RekeyError::BadOldSignature));
        assert_eq!(r.sign(&new, &old), Err(RekeyError::KeyMismatch));
        let other = EdgeIdentity {
            edge: key(8).public(),
            domain: "tun.zip".into(),
        };
        assert_eq!(Rekey::verify(&s, &other), Err(RekeyError::WrongEdge));
        // Tags separate record kinds: a rekey body signed as a delegation never verifies.
        let cross = SignedRekey {
            sig_old: sign_raw(DELEGATION_TAG, &s.body, &old),
            sig_new: sign_raw(DELEGATION_TAG, &s.body, &new),
            body: s.body.clone(),
        };
        assert_eq!(
            Rekey::verify(&cross, &edge),
            Err(RekeyError::BadOldSignature)
        );
    }

    #[test]
    fn retire_only_from_connected_edge() {
        let standby = key(6);
        let r = Retire {
            v: 1,
            edge: standby.public(),
            retired: key(3).public(),
            issued_ms: T0,
        };
        let s = r.sign(&standby).unwrap();
        assert_eq!(Retire::verify(&s, &standby.public()).unwrap(), r);
        assert_eq!(
            Retire::verify(&s, &key(3).public()),
            Err(RetireError::BadSignature)
        );
    }

    #[test]
    fn rekey_chain_heal_and_conflict_rule() {
        let edge = EdgeIdentity {
            edge: key(3).public(),
            domain: "tun.zip".into(),
        };
        // K → S was beaten by K → T (T's secret is in our stash): chain T → S.
        let (k, s, t) = (key(1), key(4), key(9));
        let first = Rekey::new("tun.zip", edge.edge, k.public(), s.public(), vec![key(5).public()], T0);
        assert_eq!(
            rekey_conflict(&t.public(), &s.public(), &[k.public(), t.public()]),
            RekeyConflict::Chain { from: t.public() }
        );
        assert_eq!(
            rekey_conflict(&s.public(), &s.public(), &[]),
            RekeyConflict::Confirming
        );
        assert_eq!(
            rekey_conflict(&key(11).public(), &s.public(), &[k.public()]),
            RekeyConflict::Contested
        );
        let chained = first.chain(t.public());
        assert_eq!((chained.old, chained.new), (t.public(), s.public()));
        assert_eq!(chained.revoke, first.revoke);
        let signed = chained.sign(&t, &s).unwrap();
        assert_eq!(Rekey::verify(&signed, &edge).unwrap(), chained);
        // The first rekey's signatures do not carry over to the chained body.
        let s1 = first.sign(&k, &s).unwrap();
        let forged = SignedRekey {
            body: signed.body.clone(),
            ..s1
        };
        assert_eq!(Rekey::verify(&forged, &edge), Err(RekeyError::BadOldSignature));
        // Tampered body.
        let mut bad = signed.clone();
        let last = bad.body.len() - 1;
        bad.body[last] ^= 1;
        assert!(Rekey::verify(&bad, &edge).is_err());
    }

    #[test]
    fn edge_pins_accept_only_the_standby_retiring_live() {
        let (live, standby, stranger) = (key(3), key(6), key(12));
        let notice = |signer: &SecretKey, retired: PublicKey| {
            Retire {
                v: Retire::VERSION,
                edge: signer.public(),
                retired,
                issued_ms: T0,
            }
            .sign(signer)
            .unwrap()
        };
        let mut pins = EdgePins::new(live.public(), Some(standby.public()));
        assert_eq!(pins.dial_order(), vec![live.public(), standby.public()]);

        // A stranger edge retiring live: verifies (it is the connected edge) but is not pinned.
        let s = notice(&stranger, live.public());
        assert_eq!(
            pins.accept_retire(&s, &stranger.public()),
            Err(RetireError::NotPinned)
        );
        // The standby retiring some other key.
        let s = notice(&standby, stranger.public());
        assert_eq!(
            pins.accept_retire(&s, &standby.public()),
            Err(RetireError::NotPinned)
        );
        // Tampered signature.
        let mut s = notice(&standby, live.public());
        s.sig[0] ^= 1;
        assert_eq!(
            pins.accept_retire(&s, &standby.public()),
            Err(RetireError::BadSignature)
        );
        assert_eq!(pins, EdgePins::new(live.public(), Some(standby.public())));

        let s = notice(&standby, live.public());
        pins.accept_retire(&s, &standby.public()).unwrap();
        assert_eq!(pins.live, standby.public());
        assert_eq!(pins.standby, None);
        assert_eq!(pins.dial_order(), vec![standby.public()]);
        // Re-delivered in every later welcome: idempotent.
        pins.accept_retire(&s, &standby.public()).unwrap();
        assert_eq!(pins.retired, vec![live.public()]);

        let j = serde_json::to_string(&pins).unwrap();
        assert_eq!(serde_json::from_str::<EdgePins>(&j).unwrap(), pins);
        let old: EdgePins =
            serde_json::from_str(&format!(r#"{{"live":"{}"}}"#, live.public())).unwrap();
        assert_eq!(old, EdgePins::new(live.public(), None));
    }
}
