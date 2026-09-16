//! Wire-visible types and actor messages of the link plugin (design doc 31 §6.2).
//!
//! This file is the contract `rpc` (methods), `tui` (`/link`) and the desktop
//! UI (bindings) code against. Shapes here are final; behaviour lives in the
//! sibling modules.

use serde::{Deserialize, Serialize};

/// Registry name of the [`crate::LinkActor`].
pub const LINK_ACTOR: &str = "link";

/// `link:` config section (31 §6.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkConfig {
    /// `false`: nothing dials out; `identity` and `devices.yaml` stay on disk.
    pub enabled: bool,
    /// Relay WebSocket origin, e.g. `wss://relay.wth.rwx.dev` or `ws://127.0.0.1:8787`.
    pub relay: String,
    /// Display name in the pairing string and on devices; `""` → hostname.
    pub name: String,
    /// Refuse new pairings past this many devices.
    pub max_devices: usize,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay: "wss://relay.wth.rwx.dev".to_owned(),
            name: String::new(),
            max_devices: 16,
        }
    }
}

/// A paired device as the UIs list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Device {
    /// First 8 chars of `base64url(static pub)`; stable, shown in the UI.
    pub id: String,
    /// User-visible name (from `PairHello.name`; renamable).
    pub name: String,
    /// `ios` | `macos` | `web` | … (from `PairHello.platform`).
    pub platform: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last connection close; `0` while never seen.
    pub last_seen_ms: u64,
    /// A session conn is open right now.
    pub online: bool,
}

/// A pending pairing: the string the QR encodes (31 §4.4) and when it dies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Pairing {
    /// `https://<relay-host>/link#1.<b64url(pub‖secret)>.<b64url(name)>`.
    pub key: String,
    /// Unix ms.
    pub expires_at_ms: u64,
}

/// `link_status` reply.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LinkStatus {
    /// `link.enabled`.
    pub enabled: bool,
    /// `link.relay`.
    pub relay: String,
    /// Effective display name (`link.name` or hostname).
    pub name: String,
    /// `base64url(static pub)`, 43 chars; `None` until an identity exists.
    pub harness_id: Option<String>,
    /// The uplink is registered with the relay.
    pub connected: bool,
    /// Every paired device.
    pub devices: Vec<Device>,
    /// The pending pairing, if any.
    pub pairing: Option<Pairing>,
}

/// Events the link plugin publishes (31 §6.2). One stream, one `match`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum LinkEvent {
    /// Uplink registered with / lost the relay.
    LinkRelay {
        /// Registered right now.
        connected: bool,
    },
    /// A pairing started (`Some`) or ended — expired, consumed or cancelled (`None`).
    LinkPairing {
        /// The pairing string.
        key: Option<String>,
        /// Unix ms.
        expires_at_ms: Option<u64>,
    },
    /// A device completed pairing.
    LinkPaired {
        /// The new row.
        device: Device,
    },
    /// A device's session conn opened or closed.
    LinkDevice {
        /// `Device.id`.
        id: String,
        /// Open right now.
        online: bool,
    },
    /// A device was revoked; its conns are closed.
    LinkRevoked {
        /// `Device.id`.
        id: String,
    },
}

/// `ask` → [`LinkStatus`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Result<(), String>`: turn the uplink on/off (persisted to
/// `config.yaml`); `relay: Some` also rewrites `link.relay` and reconnects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enable {
    /// New `link.enabled`.
    pub enabled: bool,
    /// New `link.relay`, if changing.
    pub relay: Option<String>,
}

/// `ask` → `Result<Pairing, String>`: start (or restart) the single pending
/// pairing; publishes `LinkPairing { key: Some }`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pair;

/// `ask` → `()`: drop the pending pairing; publishes `LinkPairing { key: None }`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CancelPair;

/// `ask` → `Result<(), String>`: set the harness display name (persisted as `link.name`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rename {
    /// New name; empty → hostname.
    pub name: String,
}

/// `ask` → `Result<(), String>`: rename a paired device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameDevice {
    /// `Device.id`.
    pub id: String,
    /// New name.
    pub name: String,
}

/// `ask` → `Result<(), String>`: revoke a device — close its conns, delete its
/// row; publishes `LinkRevoked`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revoke {
    /// `Device.id`.
    pub id: String,
}
