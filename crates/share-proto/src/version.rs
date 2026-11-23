//! ALPN, protocol major and the client-version rule (§5.3 "Versioning").
//!
//! Within a major every change is additive: new optional fields carry `#[serde(default)]`, unknown fields are
//! ignored, new endpoints, events and error codes may appear (decoders here tolerate them). Renaming, removing or
//! changing the meaning of anything is a new major (`theuth/share/2`, `/v2/`); an edge serves the current and the
//! previous major for 6 months, and a client offers both ALPNs.

use std::fmt;
use std::str::FromStr;

/// The ALPN of the current major.
pub const ALPN: &[u8] = b"theuth/share/1";

/// Every ALPN this build speaks, newest first: an edge registers all of them on its `Router`, a client offers the
/// first and passes the rest as `ConnectOptions::additional_alpns`.
pub const ALPNS: &[&[u8]] = &[ALPN];

/// The protocol major, the `v` of `/v1/info`, `hello` and `welcome`.
pub const PROTOCOL_MAJOR: u32 = 1;

/// Every protocol major this build serves, newest first (the `supported` of `426 protocol_mismatch`); parallel
/// to [`ALPNS`].
pub const SUPPORTED_MAJORS: &[u32] = &[PROTOCOL_MAJOR];

/// Whether a `hello` with `v` is served; else `426 protocol_mismatch` (`client_too_old` stays the `min_client`
/// refusal).
pub fn protocol_ok(v: u32) -> bool {
    SUPPORTED_MAJORS.contains(&v)
}

/// Prefix of `/v1/info`'s `edge` field: `theuth-share-edge/<ver>`.
pub const EDGE_AGENT_PREFIX: &str = "theuth-share-edge/";

/// Prefix of `hello`'s `client` field: `theuth/<ver>`.
pub const CLIENT_AGENT_PREFIX: &str = "theuth/";

/// `theuth-share-edge/<ver>`.
pub fn edge_agent(version: &str) -> String {
    format!("{EDGE_AGENT_PREFIX}{version}")
}

/// `theuth/<ver>`.
pub fn client_agent(version: &str) -> String {
    format!("{CLIENT_AGENT_PREFIX}{version}")
}

/// A `major.minor.patch` release version; a pre-release or build suffix (`-rc.1`, `+abc`) is ignored, so
/// `2.26.0-rc.1` compares equal to `2.26.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major.
    pub major: u64,
    /// Minor.
    pub minor: u64,
    /// Patch.
    pub patch: u64,
}

/// A version string that is not `major.minor.patch`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a major.minor.patch version: {0:?}")]
pub struct VersionError(pub String);

impl FromStr for Version {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || VersionError(s.to_owned());
        let core = s.split(['-', '+']).next().unwrap_or_default();
        let mut parts = core.split('.').map(|p| p.parse::<u64>().map_err(|_| bad()));
        let v = Version {
            major: parts.next().ok_or_else(bad)??,
            minor: parts.next().ok_or_else(bad)??,
            patch: parts.next().ok_or_else(bad)??,
        };
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses `theuth/<ver>`, `theuth-share-edge/<ver>` or a bare `<ver>`.
pub fn parse_agent(agent: &str) -> Result<Version, VersionError> {
    let v = agent.rsplit_once('/').map_or(agent, |(_, v)| v);
    v.parse()
}

/// The compatibility rule: a `hello` whose `client` is older than the edge's `min_client` gets
/// `426 {code: client_too_old}`. An unparseable `client` is too old; an unparseable `min_client` admits everyone
/// (an operator typo must not lock every client out).
pub fn client_ok(client: &str, min_client: &str) -> bool {
    let Ok(min) = parse_agent(min_client) else {
        return true;
    };
    parse_agent(client).is_ok_and(|c| c >= min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_rule() {
        assert!(client_ok("theuth/2.26.0", "2.26.0"));
        assert!(client_ok("theuth/2.27.1", "theuth/2.26.3"));
        assert!(client_ok("theuth/2.26.0-rc.1", "2.26.0"));
        assert!(!client_ok("theuth/2.25.9", "2.26.0"));
        assert!(!client_ok("theuth/banana", "2.26.0"));
        assert!(client_ok("theuth/2.0.0", "nonsense"));
        assert!("1.2".parse::<Version>().is_err());
        assert!("1.2.3.4".parse::<Version>().is_err());
    }
}
