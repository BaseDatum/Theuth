//! The pairing string (`t mesh pair` → `t mesh add`) and the pure peer-set
//! merge both sides run after the exchange (67 §4.1).
//!
//! String form mirrors 31's convention (`crates/link/src/pairing.rs`): a
//! version prefix, a dot, one opaque payload — here `1.<base32(json)>` with
//! `{ id, name, relay, addrs, token }`. Base32 (no padding, case-insensitive
//! on parse) because the string is typed or pasted between machines.

use std::collections::BTreeMap;

use data_encoding::BASE32_NOPAD;
#[cfg(not(target_os = "ios"))]
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use serde::{Deserialize, Serialize};

use crate::identity::now_ms;
use crate::types::{EngineId, HubPref};

/// Endpoint identifier stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
pub type EndpointId = crate::identity::PublicKey;

/// Transport address stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TransportAddr {
    /// Direct IP socket address.
    Ip(std::net::SocketAddr),
    /// Relay URL string.
    Relay(String),
}

#[cfg(target_os = "ios")]
impl std::fmt::Display for TransportAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip(addr) => write!(f, "{addr}"),
            Self::Relay(url) => f.write_str(url),
        }
    }
}

/// Endpoint address stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointAddr {
    /// Target endpoint id.
    pub id: EndpointId,
    /// Known transport addresses.
    pub addrs: std::collections::BTreeSet<TransportAddr>,
}

/// Pairing TTL (as 31).
pub const TTL_MS: u64 = 300_000;

/// What `t mesh pair` prints, decoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    /// The inviting engine.
    pub id: EndpointId,
    /// Its display name.
    pub name: String,
    /// Its relay URL (`mesh.relay` as configured; `public` for n0's).
    pub relay: String,
    /// Direct and relay addresses at the time of printing.
    pub addrs: Vec<TransportAddr>,
    /// One-time token the inviting engine checks (base32).
    pub token: String,
    /// The vault's mesh data key, base32 (67 §5.4): the joiner adopts it so
    /// replicated secrets open on both sides. Absent when the secrets plugin
    /// is not running on the inviter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_key: Option<String>,
    /// 183 §6.9.9: the id of `vault_key` (`secrets::KeyInfo::id`); absent from a pre-183 engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_key_id: Option<String>,
}

impl Payload {
    /// The engine's address for dialing.
    pub fn addr(&self) -> EndpointAddr {
        EndpointAddr {
            id: self.id,
            addrs: self.addrs.iter().cloned().collect(),
        }
    }
}

/// One pending pairing per engine: a fresh token with a 5-minute TTL.
#[derive(Clone, Debug)]
pub struct Pending {
    /// The token, base32.
    pub token: String,
    /// Unix ms.
    pub expires_at_ms: u64,
}

impl Pending {
    /// Fresh random token.
    pub fn fresh() -> Self {
        Self::fresh_for(TTL_MS)
    }

    /// Fresh random token live for `ttl_ms` (224: a cloud node's remote ticket lives longer
    /// than the Settings pairing string, because the node installs before it dials).
    pub fn fresh_for(ttl_ms: u64) -> Self {
        let bytes: [u8; 16] = rand::random();
        Self {
            token: BASE32_NOPAD.encode(&bytes),
            expires_at_ms: now_ms().saturating_add(ttl_ms),
        }
    }

    /// Still usable.
    pub fn live(&self) -> bool {
        now_ms() < self.expires_at_ms
    }
}

/// 224: the longest a remote ticket may live.
pub const REMOTE_TTL_MAX_MS: u64 = 3_600_000;

/// Consume a remote ticket (224 §4): drops the expired ones, then removes and accepts `token`
/// when it is one of the live ones. Single use: a second presentation fails.
pub fn take_remote(remote: &mut Vec<Pending>, token: &str) -> bool {
    remote.retain(Pending::live);
    match remote.iter().position(|p| p.token == token) {
        Some(i) => {
            remote.swap_remove(i);
            true
        }
        None => false,
    }
}

/// `1.<base32(json)>`.
pub fn build(p: &Payload) -> String {
    let json = serde_json::to_vec(p).unwrap_or_default();
    format!("1.{}", BASE32_NOPAD.encode(&json))
}

/// Inverse of [`build`]; tolerant of surrounding whitespace and lowercase.
pub fn parse(s: &str) -> Option<Payload> {
    let (version, body) = s.trim().split_once('.')?;
    if version != "1" {
        return None;
    }
    let bytes = BASE32_NOPAD
        .decode(body.to_ascii_uppercase().as_bytes())
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `t mesh pair --remote` (224 §4): the same one-time token, id and addresses without
/// `vault_key`/`vault_key_id`, for an engine whose ticket travels through a cloud provider's
/// metadata or a Kubernetes Secret. The joiner needs neither: a 183 inviter hands it the
/// keyring in `PairAck`, inside the token-checked connection.
pub fn without_vault_key(s: &str) -> Option<String> {
    let mut p = parse(s)?;
    p.vault_key = None;
    p.vault_key_id = None;
    Some(build(&p))
}

/// One row of the peer set as it travels between engines: the `peers` table
/// row plus its tombstone. LWW on [`PeerEntry::stamp`]; a removal wins a tie.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerEntry {
    /// Endpoint id.
    pub id: EngineId,
    /// Display name.
    pub name: String,
    /// Last known addresses.
    #[serde(default)]
    pub addrs: Vec<TransportAddr>,
    /// Lease preference as last advertised.
    #[serde(default)]
    pub hub_pref: HubPref,
    /// When it was (re-)added.
    pub added_ms: u64,
    /// When it was removed; `Some` = tombstone.
    #[serde(default)]
    pub removed_ms: Option<u64>,
}

impl PeerEntry {
    /// The LWW clock of this row.
    pub fn stamp(&self) -> u64 {
        self.removed_ms
            .map_or(self.added_ms, |r| r.max(self.added_ms))
    }

    /// Tombstone?
    pub fn removed(&self) -> bool {
        self.removed_ms.is_some()
    }
}

/// Does `incoming` replace `local`? Newer stamp wins; on a tie a removal wins.
pub fn newer(incoming: &PeerEntry, local: &PeerEntry) -> bool {
    match incoming.stamp().cmp(&local.stamp()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => incoming.removed() && !local.removed(),
        std::cmp::Ordering::Less => false,
    }
}

/// Union of two peer sets under [`newer`]; `me` is never merged in (an
/// engine is not its own peer). A replacing live row keeps the addresses the
/// local row already had (union, first-seen order): an address injected by
/// hand — the host's `127.0.0.1:<port>` into a loopback-published VM — must
/// survive the peer's own advertisement (79 smoke bug 9). Returns the merged
/// set and which rows changed.
// ponytail: addresses only ever accumulate; prune stale ones if a peer that
// changes networks often makes iroh's dial list long.
pub fn merge(
    local: &[PeerEntry],
    incoming: &[PeerEntry],
    me: &EngineId,
) -> (Vec<PeerEntry>, Vec<PeerEntry>) {
    let mut by_id: BTreeMap<EngineId, PeerEntry> =
        local.iter().map(|p| (p.id.clone(), p.clone())).collect();
    let mut changed = Vec::new();
    for p in incoming {
        if &p.id == me {
            continue;
        }
        let Some(cur) = by_id.get(&p.id) else {
            by_id.insert(p.id.clone(), p.clone());
            changed.push(p.clone());
            continue;
        };
        if !newer(p, cur) {
            continue;
        }
        let mut p = p.clone();
        if !p.removed() {
            p.addrs = union(&cur.addrs, &p.addrs);
        }
        by_id.insert(p.id.clone(), p.clone());
        changed.push(p);
    }
    (by_id.into_values().collect(), changed)
}

/// `a` then whatever of `b` is not already in it.
pub fn union(a: &[TransportAddr], b: &[TransportAddr]) -> Vec<TransportAddr> {
    let mut out = a.to_vec();
    out.extend(b.iter().filter(|x| !a.contains(x)).cloned());
    out
}

/// An engine's standing in the mesh when a handshake decides who keeps the
/// vault key (79 F5, smoke bug 10). Counted before the new peer is learned,
/// on both sides, so both compute the same answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rank {
    /// Live (not removed) paired peers.
    pub paired_peers: u32,
    /// Identity creation (`identity.rs`), the same age signal as the mesh name.
    pub first_seen_ms: u64,
    /// Final tie-break.
    pub id: EngineId,
}

/// Which end of a pairing handshake keeps its mesh vault key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The engine running `t mesh add`.
    Dialer,
    /// The engine whose pairing string was pasted.
    Acceptor,
}

/// The established engine keeps the key and the fresh one adopts it,
/// whichever way the handshake was dialed: more paired peers wins; tie → the
/// older identity; tie → the lower id. A laptop pasting an existing mesh's
/// string (dialer fresh) adopts as before; a host bringing up a VM (acceptor
/// fresh) keeps its key and the VM adopts.
pub fn key_owner(dialer: &Rank, acceptor: &Rank) -> Side {
    let key = |r: &Rank| {
        (
            std::cmp::Reverse(r.paired_peers),
            r.first_seen_ms,
            r.id.clone(),
        )
    };
    if key(dialer) < key(acceptor) {
        Side::Dialer
    } else {
        Side::Acceptor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, added: u64, removed: Option<u64>) -> PeerEntry {
        PeerEntry {
            id: EngineId(id.into()),
            name: id.into(),
            addrs: vec![],
            hub_pref: HubPref::Eligible,
            added_ms: added,
            removed_ms: removed,
        }
    }

    #[test]
    fn pairing_string_round_trip() {
        #[cfg(not(target_os = "ios"))]
        let secret = iroh::SecretKey::generate();
        #[cfg(target_os = "ios")]
        let secret = crate::identity::SecretKey::generate();
        let p = Payload {
            id: secret.public(),
            name: "Sky’s Mac".into(),
            relay: "https://iroh.relay.theuth.io".into(),
            addrs: vec![TransportAddr::Ip("192.168.1.5:11204".parse().unwrap())],
            token: Pending::fresh().token,
            vault_key: Some(BASE32_NOPAD.encode(&[9u8; 32])),
            vault_key_id: Some("0123456789abcdef".into()),
        };
        let s = build(&p);
        assert!(s.starts_with("1."), "{s}");
        assert_eq!(parse(&s).unwrap(), p);
        // A pre-183 string (the key absent: `None` is never serialized) still parses.
        let old = Payload {
            vault_key_id: None,
            ..p.clone()
        };
        assert_eq!(parse(&build(&old)).unwrap(), old);
        assert_eq!(
            parse(&format!("  {}  ", s.to_ascii_lowercase())).unwrap(),
            p
        );
        assert!(parse("2.abc").is_none());
        assert!(parse("1.!!!").is_none());
        let remote = parse(&without_vault_key(&s).unwrap()).unwrap();
        assert_eq!((remote.vault_key, remote.vault_key_id), (None, None));
        assert_eq!((remote.id, remote.token), (p.id, p.token.clone()));
        assert!(without_vault_key("2.abc").is_none());
    }

    /// 224: a remote ticket opens the door once, and not after its TTL.
    #[test]
    fn remote_ticket_is_single_use_and_expires() {
        let a = Pending::fresh_for(900_000);
        let b = Pending::fresh_for(900_000);
        assert!(
            a.expires_at_ms > now_ms() + TTL_MS,
            "longer than the pairing TTL"
        );
        let mut stale = Pending::fresh_for(900_000);
        stale.expires_at_ms = now_ms() - 1;
        let mut remote = vec![a.clone(), b.clone(), stale.clone()];
        assert!(!take_remote(&mut remote, "nope"));
        assert_eq!(remote.len(), 2, "the expired one was pruned");
        assert!(!take_remote(&mut remote, &stale.token), "expired");
        assert!(take_remote(&mut remote, &a.token));
        assert!(!take_remote(&mut remote, &a.token), "single use");
        assert!(
            take_remote(&mut remote, &b.token),
            "the other is still live"
        );
        assert!(remote.is_empty());
    }

    #[test]
    fn merge_is_union_and_removal_wins_when_newer() {
        let me = EngineId("me".into());
        let local = vec![entry("a", 10, None), entry("b", 10, None)];
        // c is new; a was removed later elsewhere; b was re-added later elsewhere.
        let incoming = vec![
            entry("c", 5, None),
            entry("a", 20, Some(20)),
            entry("b", 30, None),
            entry("me", 1, None),
        ];
        let (merged, changed) = merge(&local, &incoming, &me);
        let ids: Vec<&str> = merged.iter().map(|p| p.id.0.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(merged.iter().find(|p| p.id.0 == "a").unwrap().removed());
        assert_eq!(merged.iter().find(|p| p.id.0 == "b").unwrap().added_ms, 30);
        assert_eq!(changed.len(), 3);

        // Older removal loses to a newer add; equal stamps → removal wins.
        let (m, _) = merge(&[entry("x", 50, None)], &[entry("x", 40, Some(40))], &me);
        assert!(!m[0].removed());
        let (m, _) = merge(&[entry("x", 50, None)], &[entry("x", 50, Some(50))], &me);
        assert!(m[0].removed());
    }

    /// 79 smoke bug 9: the host adds a VM by its injected `127.0.0.1:<port>`;
    /// the VM's own advertisement (relay + NAT address) must add to, not
    /// replace, that address. A tombstone carries no addresses.
    #[test]
    fn merge_keeps_the_local_addrs_of_a_replaced_live_row() {
        let me = EngineId("me".into());
        let ip = |s: &str| TransportAddr::Ip(s.parse().unwrap());
        let mut local = entry("vm", 10, None);
        local.addrs = vec![ip("127.0.0.1:56082")];
        let mut adv = entry("vm", 20, None);
        adv.addrs = vec![ip("172.16.0.6:7800"), ip("127.0.0.1:56082")];
        let (m, changed) = merge(std::slice::from_ref(&local), &[adv.clone()], &me);
        assert_eq!(m[0].addrs, [ip("127.0.0.1:56082"), ip("172.16.0.6:7800")]);
        assert_eq!(changed[0].addrs, m[0].addrs);
        // An older advertisement changes nothing; a removal keeps its (empty) list.
        let (m, changed) = merge(&[adv.clone()], &[local], &me);
        assert_eq!(m[0].addrs, adv.addrs);
        assert!(changed.is_empty());
        let (m, _) = merge(&[adv], &[entry("vm", 30, Some(30))], &me);
        assert!(m[0].removed() && m[0].addrs.is_empty());
    }

    /// 79 F5: peers, then age, then id; the direction of the dial never matters.
    #[test]
    fn key_owner_is_peers_then_age_then_id() {
        let rank = |peers: u32, first_seen_ms: u64, id: &str| Rank {
            paired_peers: peers,
            first_seen_ms,
            id: EngineId(id.into()),
        };
        let table = [
            // (dialer, acceptor, owner)
            (rank(2, 900, "z"), rank(0, 100, "a"), Side::Dialer), // host with VMs dials a fresh VM
            (rank(0, 900, "a"), rank(1, 100, "z"), Side::Acceptor), // laptop pastes an established mesh's string
            (rank(0, 100, "z"), rank(0, 900, "a"), Side::Dialer), // no peers on either side: the older identity
            (rank(0, 900, "z"), rank(0, 100, "a"), Side::Acceptor),
            (rank(1, 500, "a"), rank(1, 500, "b"), Side::Dialer), // full tie: the lower id
            (rank(1, 500, "b"), rank(1, 500, "a"), Side::Acceptor),
        ];
        for (d, a, want) in &table {
            assert_eq!(key_owner(d, a), *want, "{d:?} dials {a:?}");
            // Swapping the roles swaps the answer.
            let flipped = match want {
                Side::Dialer => Side::Acceptor,
                Side::Acceptor => Side::Dialer,
            };
            assert_eq!(key_owner(a, d), flipped, "{a:?} dials {d:?}");
        }
    }
}
