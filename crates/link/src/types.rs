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

/// One authenticated RPC connection — a Noise session (mode 0), a guest conn
/// (mode 2, 59 §8.3) or a local WebSocket the bearer token admitted — as the
/// method it invokes sees it: the same peer for every request on that
/// connection, a different one for every other connection. Methods read it
/// from their jsonrpsee [`Extensions`](jsonrpsee::types::Extensions)
/// (`#[method(with_extensions)]`, `ext.get::<RpcPeer>()`); `None` there means
/// no connection authenticated the call — treat it as unauthenticated, never
/// as local. [`remote`](Self::remote) tells a relay conn (the caller's host is
/// not this machine, 54 §8) from a local socket or in-process caller;
/// [`guest`](Self::guest) tells a share-link guest from the owner's devices.
#[derive(Clone, Debug)]
pub struct RpcPeer {
    id: u64,
    closed: CancellationToken,
    remote: bool,
    guest: Option<std::sync::Arc<Guest>>,
    /// Set only by [`RpcPeer::internal`] (171 §12.1).
    internal: bool,
}

/// A mode-2 conn's identity (59 §8.3): the unknown static that completed `IK`
/// and that handshake's hash — what `doc_join`'s proof binds to. `joined` is
/// set by rpc once `document` admitted the static to some document; the conn
/// task closes the conn (`CLOSE 3`) when it is still unset 5 s after the
/// handshake, or after three refused member calls.
#[derive(Debug)]
pub struct Guest {
    /// The guest's X25519 static.
    pub key: [u8; 32],
    /// Noise handshake hash of this connection.
    pub handshake_hash: [u8; 32],
    joined: std::sync::atomic::AtomicBool,
    refusals: std::sync::atomic::AtomicU8,
}

impl Guest {
    /// `document` accepted a `doc_join` or a member call from this static.
    pub fn mark_joined(&self) {
        self.joined
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether some member call succeeded on this conn.
    pub fn joined(&self) -> bool {
        self.joined.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `document` refused a member call (not a member, viewer write); returns the running count (saturating).
    pub fn refuse(&self) -> u8 {
        use std::sync::atomic::Ordering::Relaxed;
        let _ = self
            .refusals
            .try_update(Relaxed, Relaxed, |n| Some(n.saturating_add(1)));
        self.refusals()
    }

    /// Refused member calls so far.
    pub fn refusals(&self) -> u8 {
        self.refusals.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Refused member calls that close a guest conn (59 §11: "three refusals close the conn").
    pub const MAX_REFUSALS: u8 = 3;

    /// Whether the conn has used up its refusals and must close (`CLOSE 3`).
    pub fn exhausted(&self) -> bool {
        self.refusals() >= Self::MAX_REFUSALS
    }
}

#[cfg(test)]
mod guest_tests {
    use super::Guest;

    #[test]
    fn three_refusals_exhaust_a_guest_and_a_join_stands_apart() {
        let g = Guest {
            key: [1; 32],
            handshake_hash: [2; 32],
            joined: Default::default(),
            refusals: Default::default(),
        };
        assert!(!g.joined() && !g.exhausted());
        assert_eq!(g.refuse(), 1);
        assert_eq!(g.refuse(), 2);
        assert!(!g.exhausted(), "two refusals keep the conn");
        assert_eq!(g.refuse(), 3);
        assert!(g.exhausted());
        // Joining does not forgive refusals; refusals do not unjoin.
        g.mark_joined();
        assert!(g.joined() && g.exhausted());
        for _ in 0..300 {
            g.refuse();
        }
        assert_eq!(g.refusals(), u8::MAX, "the counter saturates");
    }
}

/// Held by the connection for as long as it is open; dropping it closes the
/// peer ([`RpcPeer::closed`] resolves). Never stored in extensions.
#[derive(Debug)]
pub struct RpcPeerGuard {
    _cancel_on_drop: DropGuard,
}

impl RpcPeer {
    /// A fresh local peer with a random id. The connection keeps the guard
    /// until it is gone (disconnect, revocation, server stop).
    pub fn open() -> (Self, RpcPeerGuard) {
        Self::open_with(false, None)
    }

    /// A fresh peer for a connection that reached us through the relay: its
    /// host is elsewhere, so the microphone is not on it (54 §8). Guard as
    /// [`open`](Self::open).
    pub fn open_remote() -> (Self, RpcPeerGuard) {
        Self::open_with(true, None)
    }

    /// A fresh peer for a mode-2 guest conn (59 §8.3): remote, and carrying the
    /// static + handshake hash the join proof binds to.
    pub fn open_guest(key: [u8; 32], handshake_hash: [u8; 32]) -> (Self, RpcPeerGuard) {
        Self::open_with(
            true,
            Some(std::sync::Arc::new(Guest {
                key,
                handshake_hash,
                joined: Default::default(),
                refusals: Default::default(),
            })),
        )
    }

    fn open_with(remote: bool, guest: Option<std::sync::Arc<Guest>>) -> (Self, RpcPeerGuard) {
        let closed = CancellationToken::new();
        let guard = RpcPeerGuard {
            _cancel_on_drop: closed.clone().drop_guard(),
        };
        (
            Self {
                id: rand::random(),
                closed,
                remote,
                guest,
                internal: false,
            },
            guard,
        )
    }

    /// A peer for a trusted in-process caller (the TUI, tests): authenticated
    /// by construction, local, never closes. Construct once and reuse it, or
    /// the caller looks like a new connection every time.
    pub fn internal() -> Self {
        Self {
            id: rand::random(),
            closed: CancellationToken::new(),
            remote: false,
            guest: None,
            internal: true,
        }
    }

    /// Whether the connection came through the relay, i.e. the caller is not
    /// on this host (54 §8).
    pub fn remote(&self) -> bool {
        self.remote
    }

    /// The guest identity of a mode-2 conn; `None` for the owner's devices and local callers.
    pub fn guest(&self) -> Option<&Guest> {
        self.guest.as_deref()
    }

    /// Whether this is the in-process peer of [`RpcPeer::internal`]: the
    /// owner console behind the MCP server, the ACP agent half and the TUI,
    /// none of which is a human press (171 §12.1 invariant 1).
    pub fn is_internal(&self) -> bool {
        self.internal
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

    /// The connection's closed token itself, for owners that hold it beside
    /// the peer id (a private root's lease, 51 §4.3).
    pub fn closed_token(&self) -> CancellationToken {
        self.closed.clone()
    }
}

#[cfg(test)]
mod internal_tests {
    use super::RpcPeer;

    #[test]
    fn only_internal_is_internal() {
        assert!(RpcPeer::internal().is_internal());
        assert!(!RpcPeer::open().0.is_internal());
        assert!(!RpcPeer::open_remote().0.is_internal());
        assert!(!RpcPeer::open_guest([1; 32], [2; 32]).0.is_internal());
    }
}
