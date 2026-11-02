//! The `__Host-link` browser cookie (design doc 31 §7.6): `<32 random bytes hex>.<HMAC-SHA256 hex>`.
//!
//! HMAC-SHA256 is the two-pass RFC 2104 construction over `sha2` — the
//! workspace has no `hmac` crate and one function is not worth a dependency.

use rand::Rng as _;
use sha2::{Digest, Sha256};

/// Cookie name.
pub const NAME: &str = "__Host-link";
const RANDOM_LEN: usize = 32;
const MAC_LEN: usize = 32;
const BLOCK: usize = 64;

/// HMAC-SHA256 of `msg` under `key` (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; MAC_LEN] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..MAC_LEN].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(T[(b >> 4) as usize] as char);
        s.push(T[(b & 15) as usize] as char);
    }
    s
}

/// Decode hex; `None` on odd length or a non-hex digit.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
}

/// Signs and verifies cookie values.
pub struct Signer {
    secret: Vec<u8>,
}

impl Signer {
    /// Use `secret` (from `--cookie-secret`), or 32 random bytes when absent.
    pub fn new(secret: Option<Vec<u8>>) -> Self {
        let secret = secret.unwrap_or_else(|| {
            let mut s = vec![0u8; 32];
            rand::rng().fill_bytes(&mut s);
            s
        });
        Self { secret }
    }

    /// A fresh cookie value: `<random hex>.<mac hex>`.
    pub fn issue(&self) -> String {
        let mut r = [0u8; RANDOM_LEN];
        rand::rng().fill_bytes(&mut r);
        format!("{}.{}", hex(&r), hex(&hmac_sha256(&self.secret, &r)))
    }

    /// `true` when `value` is well-formed and its MAC matches (constant-time compare).
    pub fn verify(&self, value: &str) -> bool {
        let Some((r, m)) = value.split_once('.') else {
            return false;
        };
        let (Some(r), Some(m)) = (unhex(r), unhex(m)) else {
            return false;
        };
        if r.len() != RANDOM_LEN || m.len() != MAC_LEN {
            return false;
        }
        let want = hmac_sha256(&self.secret, &r);
        m.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }

    /// The `Set-Cookie` header value for a freshly issued cookie.
    pub fn set_cookie(&self) -> String {
        format!(
            "{NAME}={}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=34560000",
            self.issue()
        )
    }
}

/// The `__Host-link` value inside a `Cookie` request header, if present.
pub fn from_header(cookie: &str) -> Option<&str> {
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(NAME)?.strip_prefix('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc4231_vector() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?"
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // test case 6: 131-byte key (> block size)
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn sign_verify_roundtrip() {
        let s = Signer::new(Some(b"secret".to_vec()));
        let v = s.issue();
        assert!(s.verify(&v));
        assert!(!Signer::new(Some(b"other".to_vec())).verify(&v));
        let mut tampered = v.clone();
        tampered.replace_range(0..1, if v.starts_with('0') { "1" } else { "0" });
        assert!(!s.verify(&tampered));
        assert!(!s.verify("garbage"));
        assert!(!s.verify("abc.def"));
        assert!(!s.verify(""));
    }

    #[test]
    fn header_extraction() {
        let s = Signer::new(None);
        let v = s.issue();
        let hdr = format!("a=b; {NAME}={v}; c=d");
        assert_eq!(from_header(&hdr), Some(v.as_str()));
        assert_eq!(from_header("a=b"), None);
        assert!(s.set_cookie().starts_with(&format!("{NAME}=")));
        assert!(s.set_cookie().contains("SameSite=Strict"));
    }
}
