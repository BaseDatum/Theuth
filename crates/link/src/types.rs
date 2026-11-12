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
    /// Relay WebSocket origin, e.g. `wss://relay.theuth.io` or `ws://127.0.0.1:8787`.
    pub relay: String,
    /// Display name in the pairing string and on devices; `""` → hostname.
    pub name: String,
    /// Refuse new pairings past this many devices.
    pub max_devices: usize,
    /// Guest (`/d`) handshakes admitted per minute (59 §8.3, §11); a guest handshake costs one DH.
    pub guest_handshakes_per_minute: u32,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay: "wss://relay.theuth.io".to_owned(),
            name: String::new(),
            max_devices: 16,
            guest_handshakes_per_minute: 10,
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

/// The mesh identity's uplink (69 §3): what this engine serves as the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MeshLink {
    /// `base64url(mesh static pub)`, 43 chars — what devices dial (`/h/<device_id>`).
    pub device_id: String,
    /// This engine holds the lease and its mesh uplink is registered.
    pub connected: bool,
}

/// One row of the replicated device registry (69 §3), revoked rows included
/// — `devices_list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DeviceInfo {
    /// First 8 chars of `base64url(static pub)`.
    pub id: String,
    /// User-visible name.
    pub name: String,
    /// `ios` | `macos` | `web` | `theuth` | …
    pub platform: String,
    /// Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub paired_ms: u64,
    /// Unix ms of the last session close; `0` while never seen.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_seen_ms: u64,
    /// `revoked_ms` is set: refused by every hub.
    pub revoked: bool,
}

/// `device_pair_start` reply (69 decision 4): what the QR encodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PairingPayload {
    /// `https://<relay-host>/link#1.<b64url(mesh pub‖psk)>.<b64url(name)>.<b64url(pending_id)>`.
    pub string: String,
    /// Unix ms; the `pending` row dies then (5 min).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expires_ms: u64,
    /// The `pending` row id (the string's 4th part).
    pub pending_id: String,
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
    /// The engine's link id (historically 'harness'); the field name is kept for wire compatibility.
    pub harness_id: Option<String>,
    /// The uplink is registered with the relay.
    pub connected: bool,
    /// Every paired device.
    pub devices: Vec<Device>,
    /// The pending pairing, if any.
    pub pairing: Option<Pairing>,
    /// The mesh identity (69): `None` until the mesh device key was loaded.
    pub mesh: Option<MeshLink>,
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
    /// The mesh uplink (69 §2) registered with / lost the relay — this engine
    /// started or stopped serving paired devices as the hub.
    MeshRelay {
        /// Registered right now.
        connected: bool,
    },
    /// A device completed pairing against the mesh identity (69); the row is
    /// in the replicated registry.
    DevicePaired {
        /// `Device.id`.
        id: String,
    },
    /// The device registry changed (pair, rename, revoke, a replicated row);
    /// `devices_list` again.
    DevicesChanged,
}

/// `ask` → [`LinkStatus`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// 184 §7.2 (W-v): `ask` → `Result<String, String>`: the mesh link id (the public id of
/// `mesh/device_key`), read only: never mints the key, never caches it in the Link state, needs no
/// Link. `Err` when the mesh has no key yet (the home then asks the holder's `link_mesh_id`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MeshLinkId;

/// 184 §7.2: the `doc_link_create` error (`ASK_FAILED`) when Link is off on the home, the mesh has
/// no `mesh/device_key` yet and no holder answers `link_mesh_id`; the Share dialog offers Retry.
pub const MESH_LINK_UNAVAILABLE: &str = "mesh_link_unavailable";

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

/// 183 D11: `ask` → `Result<u32, String>`: the mesh's `mesh/device_key` was just reset, so
/// every phone and iPad must re-pair: each active device is revoked (the row stays; scanning a
/// new QR reactivates it), the cached mesh identity is dropped and the mesh uplink restarts
/// under the new key. The count of devices revoked.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ResetDevices;

/// `ask` → `Vec<DeviceInfo>`: every registry row, revoked included (69 §3).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DevicesList;

/// `ask` → `Result<PairingPayload, String>`: a pairing for the MESH identity
/// (69 decision 4): loads `mesh/device_key`, writes a `pending` row (any
/// engine may; the hub accepts). `name: None` → the mesh's name.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DevicePairStart {
    /// The name the QR shows (3rd part).
    pub name: Option<String>,
}

/// `ask` → `()`: close every mode-2 conn of `key` (59 §8.4: `doc_member_revoke`); the static is not
/// a device, so nothing else changes. Its next `IK` completes and its first member call is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DropGuest {
    /// The guest's X25519 static.
    pub key: [u8; 32],
}

/// Serves one ACP conn (32 §4.3) over a record channel; ends when either half closes.
pub type AcpServe = std::sync::Arc<
    dyn Fn(runtime::Runtime, crate::RecordChannel) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// `tell` → `()`: rpc hands link what conns dispatch into (mode 0 → `methods`,
/// mode 1 → `acp`). Sent once by `rpc::RpcPlugin::start`; a second send is ignored.
pub struct SetDispatch {
    /// jsonrpsee methods a mode-0 conn's records go through (`raw_json_request`).
    pub methods: jsonrpsee::Methods,
    /// ACP agent half a mode-1 conn hands its [`crate::RecordChannel`] to.
    pub acp: AcpServe,
}

