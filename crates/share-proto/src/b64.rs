//! base64url (no padding) serde helpers: signed bytes travel as strings so the verifier sees exactly the bytes
//! that were signed (§5.7).

use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

/// Encodes bytes as base64url without padding.
pub fn encode(bytes: &[u8]) -> String {
    BASE64URL_NOPAD.encode(bytes)
}

/// Decodes base64url without padding.
pub fn decode(s: &str) -> Result<Vec<u8>, data_encoding::DecodeError> {
    BASE64URL_NOPAD.decode(s.as_bytes())
}

/// `#[serde(with = "b64::bytes")]` for `Vec<u8>`.
pub mod bytes {
    use super::*;

    /// Serialises as a base64url string.
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(v))
    }

    /// Deserialises from a base64url string.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).map_err(D::Error::custom)
    }
}

/// `#[serde(with = "b64::array")]` for `[u8; N]`.
pub mod array {
    use super::*;

    /// Serialises as a base64url string.
    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(v))
    }

    /// Deserialises from a base64url string of exactly `N` bytes.
    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        let v = decode(&s).map_err(D::Error::custom)?;
        v.try_into()
            .map_err(|v: Vec<u8>| D::Error::custom(format!("expected {N} bytes, got {}", v.len())))
    }
}
