//! The pending pairing and its string (31 §4.4).

use crate::identity::{b64, now_ms, unb64};

/// Pairing TTL.
pub const TTL_MS: u64 = 300_000;

/// One pending pairing per harness.
#[derive(Clone)]
pub struct Pending {
    /// The PSK.
    pub secret: [u8; 32],
    /// The §4.4 string.
    pub key: String,
    /// Unix ms.
    pub expires_at_ms: u64,
}

impl Pending {
    /// Fresh secret, 5 min TTL.
    pub fn new(relay: &str, harness_pub: &[u8; 32], name: &str) -> Self {
        let secret: [u8; 32] = rand::random();
        Self {
            key: build(relay, harness_pub, &secret, name, None),
            secret,
            expires_at_ms: now_ms() + TTL_MS,
        }
    }

    /// Still usable.
    pub fn live(&self) -> bool {
        now_ms() < self.expires_at_ms
    }

    /// UI shape.
    pub fn pairing(&self) -> crate::types::Pairing {
        crate::types::Pairing {
            key: self.key.clone(),
            expires_at_ms: self.expires_at_ms,
        }
    }
}

/// `https://<host>[:port]/link#1.<b64url(pub‖secret)>.<b64url(name)>[.<b64url(pending_id)>]`;
/// scheme from `relay` (`wss`→`https`, else `http`). The 4th part (69
/// decision 4) names the replicated `pending` row a mesh pairing uses; the
/// engine identity's own pairing has none.
pub fn build(relay: &str, harness_pub: &[u8; 32], secret: &[u8; 32], name: &str, pending: Option<&str>) -> String {
    let (scheme, host) = match relay.split_once("://") {
        Some((s, rest)) => (s, rest.trim_end_matches('/')),
        None => ("wss", relay.trim_end_matches('/')),
    };
    let scheme = if scheme == "wss" { "https" } else { "http" };
    let mut both = [0u8; 64];
    both[..32].copy_from_slice(harness_pub);
    both[32..].copy_from_slice(secret);
    let mut s = format!("{scheme}://{host}/link#1.{}.{}", b64(&both), b64(name.as_bytes()));
    if let Some(p) = pending {
        s.push('.');
        s.push_str(&b64(p.as_bytes()));
    }
    s
}

/// Parsed pairing string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// `host[:port]`.
    pub relay_host: String,
    /// `https` (wss) vs `http` (ws).
    pub secure: bool,
    /// Harness static.
    pub harness_pub: [u8; 32],
    /// PSK.
    pub secret: [u8; 32],
    /// Harness display name.
    pub name: String,
    /// The `pending` row id (4th part); `None` on a 3-part string.
    pub pending: Option<String>,
}

/// Inverse of [`build`]; also accepts the bare `1.<…>.<…>[.<…>]` (then
/// `relay_host` is empty and `secure` true). 3 or 4 parts; anything else is `None`.
pub fn parse(s: &str) -> Option<Parsed> {
    let s = s.trim();
    let (relay_host, secure, frag) = match s.split_once('#') {
        Some((url, frag)) => {
            let (scheme, rest) = url.split_once("://")?;
            let host = rest.split('/').next()?;
            (host.to_owned(), scheme == "https", frag)
        }
        None => (String::new(), true, s),
    };
    let mut parts = frag.split('.');
    if parts.next()? != "1" {
        return None;
    }
    let both = unb64(parts.next()?)?;
    if both.len() != 64 {
        return None;
    }
    let name = String::from_utf8(unb64(parts.next()?)?).ok()?;
    let pending = match parts.next() {
        Some(p) => Some(String::from_utf8(unb64(p)?).ok()?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    let mut harness_pub = [0u8; 32];
    let mut secret = [0u8; 32];
    harness_pub.copy_from_slice(&both[..32]);
    secret.copy_from_slice(&both[32..]);
    Some(Parsed {
        relay_host,
        secure,
        harness_pub,
        secret,
        name,
        pending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_parse_round_trip() {
        let hp = [1u8; 32];
        let secret = [2u8; 32];
        let key = build("wss://relay.example:8443/", &hp, &secret, "Sky’s Mac", None);
        assert!(
            key.starts_with("https://relay.example:8443/link#1."),
            "{key}"
        );
        let p = parse(&key).unwrap();
        assert_eq!(p.relay_host, "relay.example:8443");
        assert!(p.secure);
        assert_eq!(p.harness_pub, hp);
        assert_eq!(p.secret, secret);
        assert_eq!(p.name, "Sky’s Mac");
        assert_eq!(p.pending, None);
        assert_eq!(key.matches('.').count() - "relay.example".matches('.').count(), 2, "3 parts");

        let key = build("ws://127.0.0.1:8787", &hp, &secret, "x", None);
        assert!(key.starts_with("http://127.0.0.1:8787/link#1."), "{key}");
        let bare = key.split_once('#').unwrap().1;
        let p = parse(bare).unwrap();
        assert_eq!(p.relay_host, "");
        assert_eq!(p.name, "x");

        // 69 decision 4: the 4th part names the pending row.
        let key = build("wss://relay.example", &hp, &secret, "work", Some("AbC-_123"));
        let p = parse(&key).unwrap();
        assert_eq!((p.name.as_str(), p.pending.as_deref()), ("work", Some("AbC-_123")));
        let bare = key.split_once('#').unwrap().1;
        assert_eq!(bare.matches('.').count(), 3, "4 parts");
        assert_eq!(parse(bare).unwrap().pending.as_deref(), Some("AbC-_123"));

        assert!(parse("2.abc.def").is_none());
        assert!(parse("https://h/link#1.short.eA").is_none());
        assert!(parse(&format!("{bare}.eA")).is_none(), "5 parts is not a pairing string");
        assert!(parse(&format!("{bare}.%%")).is_none(), "4th part must be base64url");
    }
}
