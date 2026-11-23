//! Name keys and CSRs (210 §6.1, §6.2): a P-256 key and a CSR for `{<name>.D, *.<name>.D}` made
//! with `rcgen`. The key lives in the mesh vault row `share/<domain>/<name>/tls` as
//! `{gen, key_pem, csr_der}`; the chain never does.

use serde::{Deserialize, Serialize};

/// `share/<domain>/<name>/tls` (§8.3).
pub fn vault_row(domain: &str, name: &str) -> String {
    format!("share/{domain}/{name}/tls")
}

/// The vault row's value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameKey {
    /// Generation, bumped on rotation.
    pub r#gen: u32,
    /// PKCS#8 PEM private key.
    pub key_pem: String,
    /// CSR DER, base64url in JSON.
    #[serde(with = "share_proto::b64::bytes")]
    pub csr_der: Vec<u8>,
}

/// Where name keys live. M1: the local vault ([`crate::wiring::VaultNameKeys`]); M2: through
/// `share.name_key.ensure` on the lease holder with the settle rule (§6.2 step 1).
// TODO(210-M2): `share.name_key.ensure` on the lease holder.
#[async_trait::async_trait]
pub trait NameKeyStore: Send + Sync {
    /// The current row, if any.
    async fn get(&self, domain: &str, name: &str) -> anyhow::Result<Option<NameKey>>;
    /// Write `key` only if no row exists (`set_if_version(…, None)`); returns the settled row.
    async fn create(&self, domain: &str, name: &str, key: NameKey) -> anyhow::Result<NameKey>;
}

/// The two SANs.
pub fn sans(name: &str, domain: &str) -> Vec<String> {
    share_proto::expected_sans(name, domain).to_vec()
}

/// A fresh P-256 key (PEM) and its CSR (DER) for `name` under `domain`.
pub fn name_key(name: &str, domain: &str) -> anyhow::Result<(String, Vec<u8>)> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::new(sans(name, domain))?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    let csr = params.serialize_request(&key)?;
    Ok((key.serialize_pem(), csr.der().to_vec()))
}

/// Get or create the row for `name` (generation 1).
pub async fn ensure(store: &dyn NameKeyStore, domain: &str, name: &str) -> anyhow::Result<NameKey> {
    if let Some(k) = store.get(domain, name).await? {
        return Ok(k);
    }
    let (key_pem, csr_der) = name_key(name, domain)?;
    store
        .create(
            domain,
            name,
            NameKey {
                r#gen: 1,
                key_pem,
                csr_der,
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csr_passes_the_edge_check() {
        let (_, der) = name_key("sky", "tun.zip").unwrap();
        let info = share_proto::check_csr(&der, "sky", "tun.zip").unwrap();
        assert_eq!(info.spki.0.len(), 32);
        assert!(share_proto::check_csr(&der, "other", "tun.zip").is_err());
        assert_eq!(vault_row("tun.zip", "sky"), "share/tun.zip/sky/tls");
    }
}
