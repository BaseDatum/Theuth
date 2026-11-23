//! The CSR rule of §6.2: the request parses, its self-signature verifies, and its SANs are exactly
//! `{<name>.D, *.<name>.D}` — DNS names only. Whether the mesh holds `name` is the edge's state check.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::FromDer;

use crate::b64;
use crate::error::ErrorCode;

/// SHA-256 of a DER `SubjectPublicKeyInfo` (`names.csr_spki`, `welcome.names[].spki`, `csr_mismatch.spki`).
/// JSON: base64url.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpkiHash(pub [u8; 32]);

impl SpkiHash {
    /// Hashes a DER SPKI (rcgen's `KeyPair::public_key_der`, a certificate's or CSR's `subject_pki.raw`).
    pub fn of(spki_der: &[u8]) -> Self {
        SpkiHash(Sha256::digest(spki_der).into())
    }
}

impl fmt::Debug for SpkiHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpkiHash({})", b64::encode(&self.0))
    }
}

impl Serialize for SpkiHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&b64::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for SpkiHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        b64::array::deserialize(d).map(SpkiHash)
    }
}

/// What the edge stores of an accepted CSR besides its DER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsrInfo {
    /// Hash of the requested key.
    pub spki: SpkiHash,
}

/// Why a CSR is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsrError {
    /// Not a DER PKCS#10 request (or trailing bytes).
    #[error("not a DER certification request")]
    Parse,
    /// The self-signature does not verify.
    #[error("CSR signature does not verify")]
    Signature,
    /// A SAN that is not a DNS name.
    #[error("CSR carries a non-DNS subject alternative name")]
    NonDnsSan,
    /// The DNS SANs are not exactly the expected pair.
    #[error("CSR names {found:?}, expected exactly {expected:?}")]
    Names {
        /// What the CSR asked for.
        found: Vec<String>,
        /// `{<name>.D, *.<name>.D}`.
        expected: [String; 2],
    },
}

impl CsrError {
    /// `400 invalid` for unparsable or unsigned requests, `403 csr_names` for the wrong names (§5.3).
    pub fn code(&self) -> ErrorCode {
        match self {
            CsrError::Parse | CsrError::Signature => ErrorCode::Invalid,
            CsrError::NonDnsSan | CsrError::Names { .. } => ErrorCode::CsrNames,
        }
    }
}

/// `[<name>.D, *.<name>.D]`, the one identifier set of a name's certificate.
pub fn expected_sans(name: &str, domain: &str) -> [String; 2] {
    [format!("{name}.{domain}"), format!("*.{name}.{domain}")]
}

/// Checks a CSR against the rule for `name` under `domain`.
pub fn check_csr(der: &[u8], name: &str, domain: &str) -> Result<CsrInfo, CsrError> {
    let (rest, csr) = X509CertificationRequest::from_der(der).map_err(|_| CsrError::Parse)?;
    if !rest.is_empty() {
        return Err(CsrError::Parse);
    }
    csr.verify_signature().map_err(|_| CsrError::Signature)?;
    let mut found = BTreeSet::new();
    for ext in csr.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext {
            for g in &san.general_names {
                match g {
                    GeneralName::DNSName(n) => {
                        found.insert(n.to_ascii_lowercase());
                    }
                    _ => return Err(CsrError::NonDnsSan),
                }
            }
        }
    }
    let expected = expected_sans(name, domain);
    let want: BTreeSet<String> = expected.iter().cloned().collect();
    if found != want {
        return Err(CsrError::Names {
            found: found.into_iter().collect(),
            expected,
        });
    }
    Ok(CsrInfo {
        spki: SpkiHash::of(csr.certification_request_info.subject_pki.raw),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair, PublicKeyData};

    fn csr(names: &[&str], key: &KeyPair) -> Vec<u8> {
        let params =
            CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap();
        params.serialize_request(key).unwrap().der().to_vec()
    }

    #[test]
    fn good_csr() {
        let key = KeyPair::generate().unwrap();
        let der = csr(&["sky.tun.zip", "*.sky.tun.zip"], &key);
        let info = check_csr(&der, "sky", "tun.zip").unwrap();
        assert_eq!(info.spki, SpkiHash::of(&key.subject_public_key_info()));
        let j = serde_json::to_string(&info.spki).unwrap();
        assert_eq!(serde_json::from_str::<SpkiHash>(&j).unwrap(), info.spki);
    }

    #[test]
    fn extra_san() {
        let key = KeyPair::generate().unwrap();
        let der = csr(&["sky.tun.zip", "*.sky.tun.zip", "evil.tun.zip"], &key);
        let e = check_csr(&der, "sky", "tun.zip").unwrap_err();
        assert!(matches!(e, CsrError::Names { .. }), "{e:?}");
        assert_eq!(e.code(), ErrorCode::CsrNames);
    }

    #[test]
    fn missing_wildcard_and_wrong_name() {
        let key = KeyPair::generate().unwrap();
        let der = csr(&["sky.tun.zip"], &key);
        assert!(matches!(
            check_csr(&der, "sky", "tun.zip"),
            Err(CsrError::Names { .. })
        ));
        let der = csr(&["bob.tun.zip", "*.bob.tun.zip"], &key);
        assert!(matches!(
            check_csr(&der, "sky", "tun.zip"),
            Err(CsrError::Names { .. })
        ));
        let der = csr(&["sky.tun.zip", "*.sky.tun.zip"], &key);
        assert!(matches!(
            check_csr(&der, "sky", "example.org"),
            Err(CsrError::Names { .. })
        ));
    }

    #[test]
    fn garbage_and_tampered() {
        assert_eq!(check_csr(b"nope", "sky", "tun.zip"), Err(CsrError::Parse));
        let key = KeyPair::generate().unwrap();
        let mut der = csr(&["sky.tun.zip", "*.sky.tun.zip"], &key);
        let last = der.len() - 1;
        der[last] ^= 0xff;
        let e = check_csr(&der, "sky", "tun.zip").unwrap_err();
        assert!(matches!(e, CsrError::Signature | CsrError::Parse), "{e:?}");
        assert_eq!(e.code(), ErrorCode::Invalid);
    }
}
