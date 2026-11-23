//! `ShareEdgeTicket` (§5.5): the edge's endpoint address and an optional invite, as an `iroh_tickets::Ticket` with
//! `KIND = "theuthedge"` (the prefix plus lowercase base32 of postcard bytes, as `EndpointTicket`).
//!
//! The shipped default (two pinned keys, live and standby) is `crates/share`'s config default, not part of the wire
//! contract (D15, appendix m2).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use iroh_base::{EndpointAddr, PublicKey, TransportAddr};
use iroh_tickets::{ParseError, Ticket};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};

use crate::b64;

/// A 128-bit invite secret. The edge stores only [`Invite::hash`] with a use count and expiry; under
/// `admission: invite` it is required in a mesh's first `hello`. JSON: base64url; postcard: 16 raw bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Invite(pub [u8; 16]);

impl Invite {
    /// SHA-256 of the secret, the edge's `invites.hash`.
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }

    /// Lowercase hex of [`Invite::hash`]: the operator-facing invite id (`admin invite list` / `invite revoke`).
    pub fn hash_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.hash())
    }
}

impl fmt::Debug for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invite(…)")
    }
}

impl Serialize for Invite {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&b64::encode(&self.0))
        } else {
            self.0.serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for Invite {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            let v = b64::decode(&s).map_err(D::Error::custom)?;
            let a: [u8; 16] = v
                .try_into()
                .map_err(|_| D::Error::custom("invite must be 16 bytes"))?;
            Ok(Invite(a))
        } else {
            Ok(Invite(<[u8; 16]>::deserialize(d)?))
        }
    }
}

/// How an engine finds an edge (§5.5): the edge's endpoint id, relay URL and public addresses, so the client
/// dials without discovery; and an invite for `admission: invite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareEdgeTicket {
    /// The edge's endpoint id and addresses.
    pub addr: EndpointAddr,
    /// Invite secret, if the operator issued one.
    pub invite: Option<Invite>,
}

/// Frozen wire format; a new shape is a new variant.
#[derive(Serialize, Deserialize)]
enum Wire {
    V1 {
        id: PublicKey,
        addrs: BTreeSet<TransportAddr>,
        invite: Option<Invite>,
    },
}

impl ShareEdgeTicket {
    /// The edge's endpoint id, what the ticket pins.
    pub fn edge_id(&self) -> PublicKey {
        self.addr.id
    }
}

impl Ticket for ShareEdgeTicket {
    const KIND: &'static str = "theuthedge";

    fn encode_bytes(&self) -> Vec<u8> {
        let w = Wire::V1 {
            id: self.addr.id,
            addrs: self.addr.addrs.clone(),
            invite: self.invite,
        };
        postcard::to_stdvec(&w).expect("postcard encoding of a ticket cannot fail")
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Self, ParseError> {
        let Wire::V1 { id, addrs, invite } = postcard::from_bytes(bytes)?;
        Ok(ShareEdgeTicket {
            addr: EndpointAddr { id, addrs },
            invite,
        })
    }
}

impl fmt::Display for ShareEdgeTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode_string())
    }
}

impl FromStr for ShareEdgeTicket {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::decode_string(s.trim())
    }
}

impl Serialize for ShareEdgeTicket {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.encode_string())
    }
}

impl<'de> Deserialize<'de> for ShareEdgeTicket {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    #[test]
    fn ticket_round_trip() {
        let id = SecretKey::from_bytes(&[5; 32]).public();
        let addr = EndpointAddr::new(id)
            .with_relay_url("https://iroh.relay.theuth.io".parse().unwrap())
            .with_ip_addr("203.0.113.7:7850".parse().unwrap());
        for invite in [None, Some(Invite([9; 16]))] {
            let t = ShareEdgeTicket {
                addr: addr.clone(),
                invite,
            };
            let s = t.to_string();
            assert!(s.starts_with("theuthedge"), "{s}");
            assert_eq!(s, s.to_ascii_lowercase());
            let back: ShareEdgeTicket = s.parse().unwrap();
            assert_eq!(back, t);
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<ShareEdgeTicket>(&json).unwrap(), t);
        }
        assert!("endpointabc".parse::<ShareEdgeTicket>().is_err());
        assert!("theuthedge!!".parse::<ShareEdgeTicket>().is_err());
    }

    #[test]
    fn invite_json_is_base64url() {
        let i = Invite([0xff; 16]);
        let j = serde_json::to_string(&i).unwrap();
        assert_eq!(j, "\"_____________________w\"");
        assert_eq!(serde_json::from_str::<Invite>(&j).unwrap(), i);
        assert!(serde_json::from_str::<Invite>("\"AAAA\"").is_err());
    }
}
