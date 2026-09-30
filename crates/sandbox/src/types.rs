//! Wire types (79 §2.2): what RPC, the tools and the desktop UI see, and the
//! `sandbox:` config section (79 §4.7). Every wire type is `JsonSchema` and,
//! under the `ts` feature, exported for `just ts`. Shapes are the 79 contract
//! verbatim (`docs/design/79-contract.md` §2.2); the contract is their documentation.
#![allow(missing_docs)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::vm::{ImageRef, Mount, NetPolicy, Resources, VmSnapshotInfo, VmMetrics};

/// One sandbox record as the UI and `sandbox_list` see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SandboxInfo {
    pub name: String,
    /// `docker` | `ssh` | `vm`.
    pub backend: String,
    pub state: SandboxState,
    /// Mesh `EngineId` once paired.
    pub engine: Option<String>,
    pub vm: Option<VmView>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_ms: u64,
}

/// The record's state machine (79 §4.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SandboxState {
    Creating { step: String },
    Ready,
    Starting,
    Running,
    Paused,
    Stopped,
    Crashed,
    Gone,
    Failed { step: String, error: String },
    Destroying,
}

/// The VM half of a [`SandboxInfo`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VmView {
    pub image: ImageRef,
    pub resources: Resources,
    pub shares: Vec<Mount>,
    pub desktop: Option<DesktopView>,
    pub network: NetPolicy,
    pub autostart: bool,
    pub pending: PendingView,
    pub metrics: Option<VmMetrics>,
    pub snapshots: Vec<VmSnapshotInfo>,
    /// Host loopback UDP port for iroh.
    pub mesh_port: Option<u16>,
    /// `EngineId` of the owner.
    pub host_engine: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DesktopView {
    pub enabled: bool,
    pub geometry: String,
    pub host_port: Option<u16>,
}

/// What waits for the next start.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PendingView {
    pub shares: bool,
    pub ports: bool,
    pub resources: bool,
}

/// RPC + tool argument for a new VM. Every field optional except `name`; defaults from Preflight.defaults / VmDefaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SandboxCreate {
    pub name: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub purpose: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cpus: Option<u8>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub memory_mib: Option<u32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub root_disk_mib: Option<u32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub max_cpus: Option<u8>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub max_memory_mib: Option<u32>,
    #[serde(default)]
    pub shares: Vec<Mount>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub desktop: Option<bool>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub geometry: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub network: Option<NetPolicy>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub image: Option<ImageRef>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub autostart: Option<bool>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub local_t: Option<PathBuf>,
}

/// What the pop-out needs to draw the desktop (RPC only, never a tool result).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DesktopAccess {
    /// `ws://127.0.0.1:<p>/websockify`.
    pub ws_url: String,
    /// `http://127.0.0.1:<p>/vnc.html`.
    pub http_url: String,
    pub password: String,
    pub geometry: String,
    pub host_engine: String,
}

/// Bus events (79 §4.4), bridged to RPC subscribers.
// `Changed` carries the whole record by contract (79-contract §2.2); events are rare.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SandboxEvent {
    Changed {
        sandbox: SandboxInfo,
    },
    Progress {
        name: String,
        step: String,
        line: String,
        pct: Option<u8>,
        session: Option<runtime::SessionId>,
        call_id: Option<String>,
    },
    Removed {
        name: String,
    },
    OpenDesktop {
        name: String,
        ws_url: String,
    },
    Takeover {
        name: String,
        engine: String,
        on: bool,
    },
}

/// `sandbox:` config section (79 §4.7). An absent section is the defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// `false`: no actor, no tools, no RPC.
    pub enabled: bool,
    pub vm: VmConfig,
    /// `sandbox_create` waits this long for `ready`.
    pub create_timeout_secs: u64,
    /// Per bring-up step.
    pub step_timeout_secs: u64,
    /// Status + metrics reconcile → `SandboxEvent::Changed` on change.
    pub health_interval_secs: u64,
    pub max_concurrent_bringups: usize,
    /// Oldest auto-snapshots pruned past this; named ones kept.
    pub snapshots_keep: usize,
    /// `desktop_handoff` waits this long for Done.
    pub handoff_timeout_secs: u64,
    /// Set inside a VM: this engine is a guest (79 §2.10).
    pub guest: Option<GuestConfig>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            vm: VmConfig::default(),
            create_timeout_secs: 900,
            step_timeout_secs: 600,
            health_interval_secs: 60,
            max_concurrent_bringups: 2,
            snapshots_keep: 5,
            handoff_timeout_secs: 1800,
            guest: None,
        }
    }
}

/// `sandbox.vm:`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VmConfig {
    /// `false`: docker/ssh engines only.
    pub enabled: bool,
    /// `microsandbox` (the only value in v1).
    pub runtime: String,
    /// `MSB_HOME`; `""` = the SDK default (`~/.microsandbox`).
    pub home: String,
    /// Image repository; tag = this binary's version.
    pub image: String,
    pub defaults: VmDefaults,
    /// Start recorded VMs with this engine.
    pub autostart: bool,
    pub max_vms: usize,
    pub resize_settle_secs: u64,
    /// Publish the guest's iroh UDP port on the host loopback and hand the mesh
    /// that address at pairing (79 §6.3; S1 finding 4 makes it a switch). Off by
    /// default: a host→guest datagram ≥ 1500 B wedges the guest NIC and the mesh
    /// does not pin its QUIC payload size (S1 finding 4b) — the relay path is used.
    pub loopback_mesh: bool,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            runtime: "microsandbox".into(),
            home: String::new(),
            image: "ghcr.io/basedatum/theuth-vm".into(),
            defaults: VmDefaults::default(),
            autostart: true,
            max_vms: 4,
            resize_settle_secs: 20,
            loopback_mesh: false,
        }
    }
}

/// `sandbox.vm.defaults:` — sizes for a new VM; `auto` derives from the host
/// ([`Resources::derive`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VmDefaults {
    pub cpus: AutoOr<u8>,
    pub memory_mib: AutoOr<u32>,
    pub root_disk_mib: u32,
    /// Live-resize ceiling.
    pub max_cpus: AutoOr<u8>,
    pub max_memory_mib: AutoOr<u32>,
    pub desktop: bool,
    pub geometry: String,
    pub network: NetPolicy,
}

impl Default for VmDefaults {
    fn default() -> Self {
        Self {
            cpus: AutoOr::default(),
            memory_mib: AutoOr::default(),
            root_disk_mib: 20480,
            max_cpus: AutoOr::default(),
            max_memory_mib: AutoOr::default(),
            desktop: true,
            geometry: "1440x900".into(),
            network: NetPolicy::PublicHost,
        }
    }
}

/// The literal `auto`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoWord {
    #[default]
    Auto,
}

/// `auto` or a number in YAML.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoOr<T> {
    Auto(AutoWord),
    Value(T),
}

impl<T> Default for AutoOr<T> {
    fn default() -> Self {
        Self::Auto(AutoWord::Auto)
    }
}

impl<T: Copy> AutoOr<T> {
    /// The configured number; `None` for `auto`.
    pub fn value(&self) -> Option<T> {
        match self {
            Self::Auto(_) => None,
            Self::Value(v) => Some(*v),
        }
    }
}

/// `sandbox.guest:` — written by the host at create (79 §2.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestConfig {
    pub name: String,
    /// Host engine name.
    pub host: String,
    pub shares: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_or_reads_auto_or_a_number() {
        assert_eq!(AutoOr::<u8>::default(), AutoOr::Auto(AutoWord::Auto));
        assert_eq!(AutoOr::<u8>::default().value(), None);
        let d: VmDefaults = serde_yaml::from_str("cpus: auto\nmemory_mib: 3072\n").unwrap();
        assert_eq!(d.cpus, AutoOr::Auto(AutoWord::Auto));
        assert_eq!(d.memory_mib.value(), Some(3072));
        assert_eq!(d.max_cpus.value(), None, "absent = auto");
        assert!(serde_yaml::from_str::<VmDefaults>("cpus: many\n").is_err());
        assert_eq!(serde_yaml::to_string(&d.cpus).unwrap().trim(), "auto");
        assert_eq!(serde_yaml::to_string(&d.memory_mib).unwrap().trim(), "3072");
    }

    #[test]
    fn config_defaults_are_the_documented_ones() {
        let c: SandboxConfig = serde_yaml::from_str("vm:\n  max_vms: 2\n").unwrap();
        assert!(c.enabled && c.vm.enabled && c.vm.autostart);
        assert_eq!(c.vm.max_vms, 2);
        assert_eq!(c.vm.runtime, "microsandbox");
        assert_eq!(c.vm.image, "ghcr.io/basedatum/theuth-vm");
        assert_eq!(c.vm.defaults.root_disk_mib, 20480);
        assert_eq!(c.vm.defaults.geometry, "1440x900");
        assert_eq!(c.vm.defaults.network, NetPolicy::PublicHost);
        assert_eq!(
            (
                c.create_timeout_secs,
                c.step_timeout_secs,
                c.health_interval_secs
            ),
            (900, 600, 60)
        );
        assert_eq!(
            (
                c.max_concurrent_bringups,
                c.snapshots_keep,
                c.handoff_timeout_secs
            ),
            (2, 5, 1800)
        );
        assert_eq!(c.guest, None);
    }

    #[test]
    fn sandbox_state_and_event_wire_shape() {
        let s = serde_json::to_value(SandboxState::Failed {
            step: "pair".into(),
            error: "x".into(),
        })
        .unwrap();
        assert_eq!(s["state"], "failed");
        let e = serde_json::to_value(SandboxEvent::Removed { name: "lab".into() }).unwrap();
        assert_eq!(e["type"], "Removed");
        let c: SandboxCreate = serde_json::from_str(r#"{"name":"lab"}"#).unwrap();
        assert_eq!(c.name, "lab");
        assert!(c.shares.is_empty() && c.cpus.is_none());
    }
}
