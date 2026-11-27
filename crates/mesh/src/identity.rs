//! This engine's iroh identity: `<data_dir>/mesh/identity` holds the 32-byte
//! Ed25519 secret (owner-only), created on first run (67 §4.1). The engine id
//! is the public key in iroh's text form.

use std::path::Path;

use anyhow::Context as _;
#[cfg(not(target_os = "ios"))]
use iroh::SecretKey;

use crate::types::EngineId;

/// Minimal 32-byte secret key stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
#[derive(Clone)]
pub struct SecretKey(zeroize::Zeroizing<[u8; 32]>);

#[cfg(target_os = "ios")]
impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

#[cfg(target_os = "ios")]
impl SecretKey {
    /// Construct a secret key from 32 raw bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(zeroize::Zeroizing::new(*bytes))
    }

    /// Generate 32 random secret bytes.
    pub fn generate() -> Self {
        Self(zeroize::Zeroizing::new(rand::random()))
    }

    /// Raw 32-byte representation.
    pub fn to_bytes(&self) -> [u8; 32] {
        *self.0
    }

    /// Deterministic public identifier derived via SHA-256 so the secret never leaves the process.
    pub fn public(&self) -> PublicKey {
        use sha2::Digest as _;
        let digest: [u8; 32] = sha2::Sha256::digest(*self.0).into();
        PublicKey(digest)
    }

    /// Stub signature on iOS (mesh signing is unavailable without `iroh`).
    pub fn sign(&self, _msg: &[u8]) -> Signature {
        Signature([0u8; 64])
    }
}

/// Minimal 32-byte public identifier stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey([u8; 32]);

#[cfg(target_os = "ios")]
impl PublicKey {
    /// Construct a public key from 32 raw bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, String> {
        Ok(Self(*bytes))
    }

    /// Raw 32-byte representation.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Short hex prefix for logging.
    pub fn fmt_short(&self) -> String {
        let s = self.to_string();
        s[..10.min(s.len())].to_owned()
    }

    /// Stub signature verification on iOS (always unsupported).
    pub fn verify(&self, _msg: &[u8], _sig: &Signature) -> Result<(), String> {
        Err("signature verification is unavailable on iOS".into())
    }
}

#[cfg(target_os = "ios")]
impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
    }
}

#[cfg(target_os = "ios")]
impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PublicKey").field(&self.to_string()).finish()
    }
}

#[cfg(target_os = "ios")]
impl std::str::FromStr for PublicKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = data_encoding::HEXLOWER_PERMISSIVE
            .decode(s.as_bytes())
            .map_err(|e| format!("invalid public key hex: {e}"))?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "public key must be 32 bytes".to_owned())?;
        Ok(Self(arr))
    }
}

#[cfg(target_os = "ios")]
impl serde::Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(target_os = "ios")]
impl<'de> serde::Deserialize<'de> for PublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Minimal 64-byte signature stub on iOS (where `iroh` is gated out).
#[cfg(target_os = "ios")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature([u8; 64]);

#[cfg(target_os = "ios")]
impl Signature {
    /// Raw 64-byte representation.
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }
}

/// The secret key and when it was first created (the engine's "first seen",
/// the probation clock of 67 §4.5).
pub struct Identity {
    /// Signs the endpoint's TLS certificate; never leaves the process.
    pub secret: SecretKey,
    /// Creation time of the identity file (ms since epoch).
    pub first_seen_ms: u64,
}

impl Identity {
    /// Load `<dir>/identity` or create it (32 random bytes, mode 0600).
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("identity");
        let secret = match std::fs::read(&path) {
            Ok(bytes) => {
                let bytes: [u8; 32] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("{}: expected 32 bytes", path.display()))?;
                SecretKey::from_bytes(&bytes)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let secret = SecretKey::generate();
                platform::files::private_dir(dir)?;
                platform::files::create_private(&path, &secret.to_bytes())
                    .with_context(|| format!("create {}", path.display()))?;
                secret
            }
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let first_seen_ms = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.created().or_else(|_| m.modified()).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or_else(now_ms, |d| d.as_millis() as u64);
        Ok(Self {
            secret,
            first_seen_ms,
        })
    }

    /// The public key as text.
    pub fn id(&self) -> EngineId {
        EngineId(self.secret.public().to_string())
    }
}

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    platform::time::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_load_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path()).unwrap();
        let b = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(
            std::fs::read(dir.path().join("identity")).unwrap().len(),
            32
        );
    }
}
