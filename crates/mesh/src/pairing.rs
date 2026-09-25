//! The pairing string (`t mesh pair` → `t mesh add`) and the pure peer-set
//! merge both sides run after the exchange (67 §4.1).
//!
//! String form mirrors 31's convention (`crates/link/src/pairing.rs`): a
//! version prefix, a dot, one opaque payload — here `1.<base32(json)>` with
//! `{ id, name, relay, addrs, token }`. Base32 (no padding, case-insensitive
//! on parse) because the string is typed or pasted between machines.

use std::collections::BTreeMap;

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use serde::{Deserialize, Serialize};

use crate::identity::now_ms;
use crate::types::{EngineId, HubPref};

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
    /// Fresh token.
    pub fn new() -> Self {
        let bytes: [u8; 16] = rand::random();
        Self {
            token: BASE32_NOPAD.encode(&bytes),
            expires_at_ms: now_ms() + TTL_MS,
        }
    }

    /// Still usable.
    pub fn live(&self) -> bool {
        now_ms() < self.expires_at_ms
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
    let bytes = BASE32_NOPAD.decode(body.to_ascii_uppercase().as_bytes()).ok()?;
    serde_json::from_slice(&bytes).ok()
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
        self.removed_ms.map_or(self.added_ms, |r| r.max(self.added_ms))
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
/// engine is not its own peer). Returns the merged set and which rows changed.
pub fn merge(local: &[PeerEntry], incoming: &[PeerEntry], me: &EngineId) -> (Vec<PeerEntry>, Vec<PeerEntry>) {
    let mut by_id: BTreeMap<EngineId, PeerEntry> = local.iter().map(|p| (p.id.clone(), p.clone())).collect();
    let mut changed = Vec::new();
    for p in incoming {
        if &p.id == me {
            continue;
        }
        let replace = by_id.get(&p.id).is_none_or(|cur| newer(p, cur));
        if replace {
            by_id.insert(p.id.clone(), p.clone());
            changed.push(p.clone());
        }
    }
    (by_id.into_values().collect(), changed)
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
        let secret = iroh::SecretKey::generate();
        let p = Payload {
            id: secret.public(),
            name: "Sky’s Mac".into(),
            relay: "https://iroh.relay.theuth.io".into(),
            addrs: vec![TransportAddr::Ip("192.168.1.5:11204".parse().unwrap())],
            token: Pending::new().token,
            vault_key: Some(BASE32_NOPAD.encode(&[9u8; 32])),
        };
        let s = build(&p);
        assert!(s.starts_with("1."), "{s}");
        assert_eq!(parse(&s).unwrap(), p);
        assert_eq!(parse(&format!("  {}  ", s.to_ascii_lowercase())).unwrap(), p);
        assert!(parse("2.abc").is_none());
        assert!(parse("1.!!!").is_none());
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
}
