//! This engine's iroh identity: `<data_dir>/mesh/identity` holds the 32-byte
//! Ed25519 secret (owner-only), created on first run (67 §4.1). The engine id
//! is the public key in iroh's text form.

use std::path::Path;

use anyhow::Context as _;
use iroh::SecretKey;

use crate::types::EngineId;

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
