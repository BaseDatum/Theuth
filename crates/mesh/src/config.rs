//! `mesh:` config section (67 §4).

use serde::{Deserialize, Serialize};

use crate::types::HubPref;

/// `mesh:` config section. Every key has a default; an absent section is the
/// defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshConfig {
    /// iroh relay URL used when hole punching fails (67 §4.2); `public` = n0's
    /// relays, `none`/`off`/empty = bind locally and never dial a relay.
    pub relay: String,
    /// DNS/pkarr + mDNS peer lookup (67 §4.2); `false` = dial remembered
    /// addresses only, no discovery traffic.
    pub discovery: bool,
    /// Display name other engines see; empty = hostname.
    pub name: String,
    /// This engine's lease preference (`prefer` | `eligible` | `never`).
    pub hub: HubPref,
    /// A newly joined engine cannot take the lease from an incumbent for this long (67 §4.5).
    pub probation_secs: u64,
    /// After a better engine appears, the incumbent keeps the lease this long before a graceful handover (67 §4.5).
    pub handover_secs: u64,
    /// Lease heartbeat over gossip (67 §4.3).
    pub heartbeat_secs: u64,
    /// Missed heartbeats for this long = the lease lapsed (67 §4.3, §4.6).
    pub lapse_secs: u64,
    /// How often engine stats (harnesses, repos, display, gpu) are re-advertised (67 §8).
    pub capabilities_secs: u64,
    /// Repo roots this engine advertises for placement (67 §8); empty = none.
    pub workspaces: Vec<String>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            relay: "https://iroh.relay.theuth.io".into(),
            discovery: true,
            name: String::new(),
            hub: HubPref::Eligible,
            probation_secs: 21_600,
            handover_secs: 600,
            heartbeat_secs: 10,
            lapse_secs: 30,
            capabilities_secs: 300,
            workspaces: Vec::new(),
        }
    }
}
