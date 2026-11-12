//! Wire-visible types and actor messages of the nodes plugin (design doc 32 §6.1–§6.2).
//!
//! This file is the contract `rpc` (methods), `tui` (`/nodes`) and the desktop
//! UI (bindings) code against. Shapes here are final; behaviour lives in the
//! sibling modules.

use serde::{Deserialize, Serialize};

/// Registry name of the [`crate::NodesActor`].
pub const NODES_ACTOR: &str = "nodes";

/// `nodes:` config section (32 §6.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NodesConfig {
    /// `false`: no dials, nodes invisible to the model and the UI; `nodes.yaml` kept.
    pub enabled: bool,
}

impl Default for NodesConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A paired node (32 §6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Node {
    /// `base64url(pub)`, 43 chars: the node's harness id.
    pub id: String,
    /// Display name (local alias).
    pub name: String,
    /// `ws(s)://host[:port]` of the relay the node registers at.
    pub relay: String,
    /// Host OS as `info` reports it; `""` until the first connect.
    pub platform: String,
    /// `theuth` version as `info` reports it; `""` until the first connect.
    pub version: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last control-conn close; `0` never.
    pub last_seen_ms: u64,
    /// Control conn up.
    pub online: bool,
    /// The node deleted us from its `devices.yaml` (32 §6.3).
    pub revoked: bool,
    /// Live ACP conns (remote children).
    pub children: u32,
}

/// Events the nodes plugin publishes (32 §6.2), forwarded by `rpc::bridge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum NodeEvent {
    /// A node was paired.
    NodeAdded {
        /// The new row.
        node: Node,
    },
    /// Presence changed.
    NodeOnline {
        /// Node id.
        id: String,
        /// Up or down.
        online: bool,
    },
    /// A node was removed.
    NodeRemoved {
        /// Node id.
        id: String,
    },
    /// The node deleted us from its `devices.yaml`.
    NodeRevoked {
        /// Node id.
        id: String,
    },
}

impl wire::WireEvent for NodeEvent {
    const TOPIC: &'static str = "nodes.events";
    const VERSION: u32 = 1;
}

/// `ask` → `Vec<Node>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Result<Node, String>`: pair with a node from its 31 §4.4 string
/// (this harness is the `IKpsk2` initiator).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Add {
    /// The pairing string printed by `t --install-service` / `t --link`.
    pub key: String,
    /// Local alias; `None` → the name the node announces.
    pub name: Option<String>,
}

/// `ask` → `Result<(), String>`: close conns, delete the row, best-effort
/// `link_revoke_device(<our id>)` on the node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Remove {
    /// Node id.
    pub id: String,
}

/// `ask` → `Result<(), String>`: local alias.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rename {
    /// Node id.
    pub id: String,
    /// New alias.
    pub name: String,
}

/// `ask` → `Result<link::Pairing, String>`: `link_pair` on the node over the
/// control conn (32 §4.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    /// Node id.
    pub id: String,
}
