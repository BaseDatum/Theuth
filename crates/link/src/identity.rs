//! Harness X25519 identity (`<data_dir>/link/identity`, 31 §4.1) and the
//! Noise constants every handshake shares.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};

/// Noise suite of every pattern (contract §Noise).
pub const SUITE: &str = "25519_AESGCM_SHA256";
/// Subprotocol / prologue prefix.
pub const PROTOCOL: &str = "wth-link-1";

/// The harness static keypair and its derived id.
#[derive(Clone)]
pub struct Identity {
    /// X25519 private scalar as snow stores it.
    pub private: [u8; 32],
    /// X25519 public key.
    pub public: [u8; 32],
    /// `base64url_nopad(public)`, 43 chars.
    pub id: String,
}

impl Identity {
    /// Load `<dir>/identity` or create it (32 random bytes, mode 0600).
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("identity");
        let private: [u8; 32] = match std::fs::read(&path) {
            Ok(bytes) => bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("{}: expected 32 bytes", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let kp = snow::Builder::new(params("NK")?).generate_keypair()?;
                let private: [u8; 32] = kp
                    .private
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("snow keypair is not 32 bytes"))?;
                std::fs::create_dir_all(dir)?;
                write_private(&path, &private)?;
                private
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Self::from_private(private))
    }

    /// Derive public key and id from a private scalar.
    pub fn from_private(private: [u8; 32]) -> Self {
        let public = public_of(&private);
        Self {
            private,
            public,
            id: b64(&public),
        }
    }

    /// `"wth-link-1" ‖ harness_id` as bytes.
    pub fn prologue(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(PROTOCOL.len() + self.id.len());
        p.extend_from_slice(PROTOCOL.as_bytes());
        p.extend_from_slice(self.id.as_bytes());
        p
    }
}

/// X25519 public key for `private` via snow's DH implementation (clamping included).
pub fn public_of(private: &[u8; 32]) -> [u8; 32] {
    let mut dh = DefaultResolver
        .resolve_dh(&DHChoice::Curve25519)
        .expect("snow default resolver provides Curve25519");
    dh.set(private);
    let mut public = [0u8; 32];
    public.copy_from_slice(dh.pubkey());
    public
}

fn write_private(path: &Path, private: &[u8; 32]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
            .write_all(private)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, private)
    }
}

/// `Noise_<pattern>_25519_AESGCM_SHA256` params.
pub fn params(pattern: &str) -> anyhow::Result<snow::params::NoiseParams> {
    format!("Noise_{pattern}_{SUITE}")
        .parse()
        .map_err(|e| anyhow::anyhow!("noise params: {e}"))
}

/// base64url without padding.
pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Inverse of [`b64`].
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("wth-link-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Identity::load_or_create(&dir).unwrap();
        let b = Identity::load_or_create(&dir).unwrap();
        assert_eq!(a.public, b.public);
        assert_eq!(a.id.len(), 43);
        // snow's keypair public must equal our derivation from the stored scalar.
        let kp = snow::Builder::new(params("NK").unwrap())
            .generate_keypair()
            .unwrap();
        let private: [u8; 32] = kp.private.as_slice().try_into().unwrap();
        assert_eq!(public_of(&private).as_slice(), kp.public.as_slice());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
