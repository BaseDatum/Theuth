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
            key: build(relay, harness_pub, &secret, name),
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

/// `https://<host>[:port]/link#1.<b64url(pub‖secret)>.<b64url(name)>`;
/// scheme from `relay` (`wss`→`https`, else `http`).
pub fn build(relay: &str, harness_pub: &[u8; 32], secret: &[u8; 32], name: &str) -> String {
    let (scheme, host) = match relay.split_once("://") {
        Some((s, rest)) => (s, rest.trim_end_matches('/')),
        None => ("wss", relay.trim_end_matches('/')),
    };
    let scheme = if scheme == "wss" { "https" } else { "http" };
    let mut both = [0u8; 64];
    both[..32].copy_from_slice(harness_pub);
    both[32..].copy_from_slice(secret);
    format!(
        "{scheme}://{host}/link#1.{}.{}",
        b64(&both),
        b64(name.as_bytes())
    )
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
}

/// Inverse of [`build`]; also accepts the bare `1.<…>.<…>` (then `relay_host`
/// is empty and `secure` true).
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
    let mut parts = frag.splitn(3, '.');
    if parts.next()? != "1" {
        return None;
    }
    let both = unb64(parts.next()?)?;
    if both.len() != 64 {
        return None;
    }
    let name = String::from_utf8(unb64(parts.next()?)?).ok()?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_parse_round_trip() {
        let hp = [1u8; 32];
        let secret = [2u8; 32];
        let key = build("wss://relay.example:8443/", &hp, &secret, "Sky’s Mac");
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

        let key = build("ws://127.0.0.1:8787", &hp, &secret, "x");
        assert!(key.starts_with("http://127.0.0.1:8787/link#1."), "{key}");
        let bare = key.split_once('#').unwrap().1;
        let p = parse(bare).unwrap();
        assert_eq!(p.relay_host, "");
        assert_eq!(p.name, "x");

        assert!(parse("2.abc.def").is_none());
        assert!(parse("https://h/link#1.short.eA").is_none());
    }
}
