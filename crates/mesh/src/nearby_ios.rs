//! iOS stub for [`crate::nearby`]: Nearby mDNS advertise/browse and pairing
//! ceremonies are disabled on iOS where `iroh` and `swarm-discovery` are gated out.

use std::net::SocketAddr;

/// DNS-SD service name: `_theuth-mesh._udp.local`.
pub const SERVICE: &str = "theuth-mesh";

/// What one engine advertises, and what a browse reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    /// Endpoint id (TXT `id`).
    pub id: String,
    /// Display name (TXT `name`).
    pub name: String,
    /// Display OS (TXT `os`).
    pub os: String,
    /// Where it answers.
    pub addrs: Vec<SocketAddr>,
}

/// A browse callback: an advert seen (`false`) or gone (`true`).
pub type Seen = Box<dyn Fn(Advert, bool) + Send + Sync>;

/// mDNS advertise + browse. `start` advertises `me` and reports every other advert to
/// `seen` until the returned guard is dropped.
pub trait Mdns: Send + Sync {
    /// Start both; drop the guard to stop them.
    fn start(&self, me: Advert, seen: Seen) -> Result<Box<dyn Send>, String>;
}

/// Stub mDNS implementation on iOS (always reports unsupported).
pub struct Swarm;

impl Mdns for Swarm {
    fn start(&self, _me: Advert, _seen: Seen) -> Result<Box<dyn Send>, String> {
        Err("Nearby mDNS discovery is unavailable on iOS".into())
    }
}
