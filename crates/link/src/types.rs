//! Wire-visible types and actor messages of the link plugin (design doc 31 §6.2).
//!
//! This file is the contract `rpc` (methods), `tui` (`/link`) and the desktop
//! UI (bindings) code against. Shapes here are final; behaviour lives in the
//! sibling modules.

use serde::{Deserialize, Serialize};
use tokio_util::sync::{CancellationToken, DropGuard, WaitForCancellationFutureOwned};

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

/// One authenticated RPC connection — a Noise session (mode 0) or a local
/// WebSocket the bearer token admitted — as the method it invokes sees it:
/// the same peer for every request on that connection, a different one for
/// every other connection. Methods read it from their jsonrpsee
/// [`Extensions`](jsonrpsee::types::Extensions) (`#[method(with_extensions)]`,
/// `ext.get::<RpcPeer>()`); `None` there means no connection authenticated
/// the call — treat it as unauthenticated, never as local.
#[derive(Clone, Debug)]
pub struct RpcPeer {
    id: u64,
    closed: CancellationToken,
}

/// Held by the connection for as long as it is open; dropping it closes the
/// peer ([`RpcPeer::closed`] resolves). Never stored in extensions.
#[derive(Debug)]
pub struct RpcPeerGuard {
    _cancel_on_drop: DropGuard,
}

impl RpcPeer {
    /// A fresh peer with a random id. The connection keeps the guard until it
    /// is gone (disconnect, revocation, server stop).
    pub fn open() -> (Self, RpcPeerGuard) {
        let closed = CancellationToken::new();
        let guard = RpcPeerGuard {
            _cancel_on_drop: closed.clone().drop_guard(),
        };
        (
            Self {
                id: rand::random(),
                closed,
            },
            guard,
        )
    }

    /// A peer for a trusted in-process caller (the TUI, tests): authenticated
    /// by construction, never closes. Construct once and reuse it, or the
    /// caller looks like a new connection every time.
    pub fn internal() -> Self {
        Self {
            id: rand::random(),
            closed: CancellationToken::new(),
        }
    }

    /// Random per connection; collision-resistant across the Noise and local
    /// WebSocket domains.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Resolves once the connection is gone; `'static`, so it can outlive the
    /// request that read the peer (e.g. cleanup spawned per pending query).
    pub fn closed(&self) -> WaitForCancellationFutureOwned {
        self.closed.clone().cancelled_owned()
    }

    /// Whether the connection is already gone.
    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }
}
