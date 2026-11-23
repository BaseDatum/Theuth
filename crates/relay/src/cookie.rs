//! The `__Host-link` browser cookie (design doc 31 §7.6): `<32 random bytes hex>.<HMAC-SHA256 hex>`.
//!
//! HMAC-SHA256 comes from the `hmac` crate; the MAC compare is its constant-time `verify_slice`.

use hmac::{Hmac, KeyInit as _, Mac as _};
use rand::Rng as _;
use sha2::Sha256;

/// Cookie name.
pub const NAME: &str = "__Host-link";
const RANDOM_LEN: usize = 32;
const MAC_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// A keyed HMAC-SHA256 over `msg` (HMAC accepts any key length).
fn mac(key: &[u8], msg: &[u8]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("any key length");
    mac.update(msg);
    mac
}

/// HMAC-SHA256 of `msg` under `key` (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; MAC_LEN] {
    mac(key, msg).finalize().into_bytes().into()
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
        format!(
            "{}.{}",
            hex::encode(r),
            hex::encode(hmac_sha256(&self.secret, &r))
        )
    }

    /// `true` when `value` is well-formed and its MAC matches (constant-time compare).
    pub fn verify(&self, value: &str) -> bool {
        let Some((r, m)) = value.split_once('.') else {
            return false;
        };
        let (Ok(r), Ok(m)) = (hex::decode(r), hex::decode(m)) else {
            return false;
        };
        if r.len() != RANDOM_LEN || m.len() != MAC_LEN {
            return false;
        }
        mac(&self.secret, &r).verify_slice(&m).is_ok()
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
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // test case 6: 131-byte key (> block size)
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex::encode(mac),
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
