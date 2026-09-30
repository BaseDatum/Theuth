//! A microVM runtime (design 79 §4.1): knows images, resources, mounts, ports,
//! snapshots, metrics — nothing about engines or the mesh. `microsandbox.rs`
//! is the one impl; `MockVmRuntime` serves tests.
//!
//! Types are the 79 contract verbatim (`docs/design/79-contract.md` §2.1); the
//! contract is their documentation.
#![allow(missing_docs)]
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{Cmd, ExecOutput, Progress};

pub mod install; // S: runtime install/resolve (setup::* of the SDK)
pub mod microsandbox; // S
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod preflight; // S: host facts + blockers // K: MockVmRuntime — in-memory state machine, records calls

/// Build the configured runtime: `microsandbox` (installed or not — `preflight` says which);
/// any other `sandbox.vm.runtime` is an error.
pub fn runtime(cfg: &crate::types::VmConfig) -> anyhow::Result<Arc<dyn VmRuntime>> {
    match cfg.runtime.as_str() {
        "microsandbox" => Ok(Arc::new(microsandbox::Microsandbox::new(cfg.clone())?)),
        other => Err(anyhow::anyhow!(
            "sandbox.vm.runtime `{other}` is not supported; only `microsandbox` is"
        )),
    }
}

#[async_trait::async_trait]
pub trait VmRuntime: Send + Sync {
    /// `"microsandbox"` | `"mock"`.
    fn kind(&self) -> &'static str;
    async fn preflight(&self) -> Preflight;
    async fn install(&self, out: &Progress) -> anyhow::Result<()>;
    async fn create(&self, spec: &VmSpec, out: &Progress) -> anyhow::Result<VmHandle>;
    async fn status(&self, h: &VmHandle) -> anyhow::Result<VmStatus>;
    /// `out` streams the recreate steps (snapshot, remove, create) when boot-time changes are
    /// pending; a plain start says nothing. Deviates from the contract (`start(&self, h)`): smoke bug 8.
    async fn start(&self, h: &VmHandle, out: &Progress) -> anyhow::Result<()>;
    async fn stop(&self, h: &VmHandle, grace: Duration) -> anyhow::Result<()>;
    async fn pause(&self, h: &VmHandle) -> anyhow::Result<()>;
    async fn resume(&self, h: &VmHandle) -> anyhow::Result<()>;
    async fn destroy(&self, h: &VmHandle) -> anyhow::Result<()>;
    async fn exec(&self, h: &VmHandle, cmd: Cmd) -> anyhow::Result<ExecOutput>;
    async fn put(&self, h: &VmHandle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()>;
    async fn resize(&self, h: &VmHandle, change: ResourceChange) -> anyhow::Result<ResizeOutcome>;
    /// Boot-time properties: recorded now, applied by the next `start` (may recreate the sandbox keeping its disk).
    async fn set_mounts(&self, h: &VmHandle, mounts: &[Mount]) -> anyhow::Result<Pending>;
    async fn set_ports(&self, h: &VmHandle, ports: &[Port]) -> anyhow::Result<Pending>;
    async fn metrics(&self, h: &VmHandle) -> anyhow::Result<VmMetrics>;
    async fn logs(&self, h: &VmHandle, tail: usize) -> anyhow::Result<Vec<LogLine>>;
    async fn snapshot(
        &self,
        h: &VmHandle,
        name: &str,
        full: bool,
        out: &Progress,
    ) -> anyhow::Result<SnapshotId>;
    /// Always a new VM (upstream semantics); `spec` supplies ports and mounts again; `as_name` must be free.
    async fn restore(
        &self,
        id: &SnapshotId,
        as_name: &str,
        spec: &VmSpec,
        out: &Progress,
    ) -> anyhow::Result<VmHandle>;
    async fn snapshots(&self, h: &VmHandle) -> anyhow::Result<Vec<VmSnapshotInfo>>;
    async fn delete_snapshot(&self, id: &SnapshotId) -> anyhow::Result<()>;
    /// Ours only: label `io.theuth.sandbox`.
    async fn list(&self) -> anyhow::Result<Vec<VmHandle>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmHandle {
    pub name: String,
    pub runtime_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum VmStatus {
    Running,
    Stopped,
    Paused,
    Crashed,
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageRef {
    Oci { r#ref: String },
    Bootstrap { base: String, script: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Mount {
    pub host: PathBuf,
    pub guest: String,
    #[serde(default)]
    pub readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Port {
    /// `"127.0.0.1"`.
    pub host_ip: String,
    /// `0` = runtime picks; the chosen port is read back from `create`.
    pub host: u16,
    pub guest: u16,
    #[serde(default)]
    pub udp: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NetPolicy {
    #[default]
    PublicHost,
    PublicHostPrivate,
    PublicOnly,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Init {
    #[default]
    Auto,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrebootFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmSpec {
    pub name: String,
    pub image: ImageRef,
    pub cpus: u8,
    pub max_cpus: u8,
    pub memory_mib: u32,
    pub max_memory_mib: u32,
    pub root_disk_mib: u32,
    pub mounts: Vec<Mount>,
    pub ports: Vec<Port>,
    pub network: NetPolicy,
    pub init: Init,
    pub hostname: String,
    pub env: BTreeMap<String, String>,
    pub files: Vec<PrebootFile>,
    pub labels: BTreeMap<String, String>,
    pub ephemeral: bool,
    pub idle_timeout_secs: Option<u32>,
    pub max_duration_secs: Option<u32>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResourceChange {
    pub cpus: Option<u8>,
    pub max_cpus: Option<u8>,
    pub memory_mib: Option<u32>,
    pub max_memory_mib: Option<u32>,
    pub root_disk_mib: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ResizeStatus {
    Applied,
    Converging,
    GuestRefused { why: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResizeOutcome {
    pub applied: ResourceChange,
    pub next_start: ResourceChange,
    pub status: ResizeStatus,
    pub restart_required: bool,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Pending {
    pub restart_required: bool,
}

/// Live numbers from the hypervisor. `memory_limit_bytes` is the hypervisor's current
/// allocation (what a live resize changes); `memory_bytes` is the guest's use within it. The
/// guest's own `MemTotal` stays at the boot size by virtio-mem design and never tracks a
/// resize — not a bug; read the limit, not the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VmMetrics {
    pub cpu_percent: f32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub memory_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub memory_limit_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub disk_used_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub disk_free_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub net_rx_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub net_tx_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub uptime_secs: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LogLine {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// `stdout` | `stderr` | `system` | `kernel`.
    pub source: String,
    pub line: String,
}

/// `"<vm>:<label>"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SnapshotId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VmSnapshotInfo {
    pub id: SnapshotId,
    pub label: String,
    pub full: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub bytes: Option<u64>,
    pub auto: bool,
}

/// Host readiness. `blockers` empty ⇒ a VM can be created now. Every blocker is user-facing:
/// `code` stable for the UI, `message` one sentence, `fix` what to do (or None when nothing can be done on this host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Preflight {
    pub runtime: RuntimeState,
    pub virt: VirtState,
    pub host: HostFacts,
    pub defaults: Resources,
    pub blockers: Vec<Blocker>,
    pub warnings: Vec<Blocker>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub checked_at_ms: u64,
}

/// One reason a VM cannot (or should not) be created here.
///
/// `code` values: `unsupported_os` (Windows v1), `unsupported_arch` (x86_64 macOS: "microsandbox
/// needs Apple Silicon"), `no_kvm` ("/dev/kvm missing — enable virtualization in firmware, or nested
/// virtualization in the outer hypervisor"), `kvm_not_writable` ("add your user to the kvm group:
/// sudo usermod -aG kvm $USER, then log in again"), `glibc_too_old` ("needs glibc ≥ 2.28"),
/// `runtime_extract_failed` (the embedded pair could not be written into the home; retry),
/// `runtime_missing` (only when no runtime was built), `runtime_incomplete` ("msb X but libkrunfw Y"),
/// `disk_low` ("N GiB free; a VM needs root_disk + image + 2 GiB"), `memory_low` (warning: default
/// memory > host/2), `docker_missing` (warning, dev image path only), `runtime_home` (warning: theuth's
/// VMs live in a home the user's own `msb` does not look at), `vm_disabled` (`sandbox.vm.enabled: false`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Blocker {
    pub code: String,
    pub message: String,
    pub fix: Option<String>,
    /// `true` only for `runtime_extract_failed` / `runtime_missing` / `runtime_incomplete`: the
    /// **Set up runtime** button (`sandbox_install_runtime`) can clear it.
    pub installable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RuntimeState {
    /// The pair compiled into `t` (79 decision 2, embed ruling). `extracted: false` = the first
    /// `create` (or `sandbox_install_runtime`) writes it into `home`; `true` = `home` holds that
    /// version already (ours or the user's own install of the same version).
    Embedded {
        version: String,
        extracted: bool,
        home: String,
    },
    /// A user-provided pair of another version (`sandbox.vm.home`, `MSB_PATH`, or an `msb` on
    /// `PATH`); used as is.
    Installed {
        version: String,
        home: String,
        libkrunfw: String,
    },
    /// No runtime at all — only when `t` was built without one (`vm::runtime` failed).
    Missing,
    Incomplete {
        why: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VirtState {
    /// `via`: `"hvf"` | `"kvm"` | `"whp"`.
    Ok {
        via: String,
    },
    Unsupported {
        why: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HostFacts {
    pub os: String,
    pub arch: String,
    pub cores: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mem_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub disk_free_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Resources {
    pub cpus: u8,
    pub max_cpus: u8,
    pub memory_mib: u32,
    pub max_memory_mib: u32,
    pub root_disk_mib: u32,
}

impl Resources {
    /// Defaults from the host (79 decision 6): `cpus = clamp(cores/4, 2, 4)`,
    /// `memory_mib = clamp(mem/4, 2048, 8192)`, `root_disk_mib` from config,
    /// `max_cpus = cores`, `max_memory_mib = mem/2` — each overridden by a
    /// non-`auto` config value.
    pub fn derive(host: &HostFacts, cfg: &crate::types::VmDefaults) -> Self {
        let mem_mib = u32::try_from(host.mem_bytes / (1024 * 1024)).unwrap_or(u32::MAX);
        let cores = u8::try_from(host.cores).unwrap_or(u8::MAX);
        Self {
            cpus: cfg.cpus.value().unwrap_or((cores / 4).clamp(2, 4)),
            max_cpus: cfg.max_cpus.value().unwrap_or(cores),
            memory_mib: cfg
                .memory_mib
                .value()
                .unwrap_or((mem_mib / 4).clamp(2048, 8192)),
            max_memory_mib: cfg.max_memory_mib.value().unwrap_or(mem_mib / 2),
            root_disk_mib: cfg.root_disk_mib,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AutoOr, VmDefaults};

    fn host(cores: u32, gib: u64) -> HostFacts {
        HostFacts {
            os: "macos".into(),
            arch: "aarch64".into(),
            cores,
            mem_bytes: gib << 30,
            disk_free_bytes: 100 << 30,
        }
    }

    #[test]
    fn derive_clamps_from_the_host() {
        let r = Resources::derive(&host(8, 16), &VmDefaults::default());
        assert_eq!((r.cpus, r.memory_mib, r.root_disk_mib), (2, 4096, 20480));
        assert_eq!((r.max_cpus, r.max_memory_mib), (8, 8192));
        let r = Resources::derive(&host(16, 64), &VmDefaults::default());
        assert_eq!((r.cpus, r.memory_mib), (4, 8192));
        assert_eq!((r.max_cpus, r.max_memory_mib), (16, 32768));
        // Floors and ceilings.
        assert_eq!(
            Resources::derive(&host(2, 4), &VmDefaults::default()).cpus,
            2
        );
        assert_eq!(
            Resources::derive(&host(64, 512), &VmDefaults::default()).memory_mib,
            8192
        );
    }

    #[test]
    fn derive_honours_overrides() {
        let cfg = VmDefaults {
            cpus: AutoOr::Value(3),
            memory_mib: AutoOr::Value(1024),
            root_disk_mib: 4096,
            max_cpus: AutoOr::Value(6),
            max_memory_mib: AutoOr::Value(2048),
            ..VmDefaults::default()
        };
        let r = Resources::derive(&host(8, 16), &cfg);
        assert_eq!(
            (
                r.cpus,
                r.memory_mib,
                r.root_disk_mib,
                r.max_cpus,
                r.max_memory_mib
            ),
            (3, 1024, 4096, 6, 2048)
        );
    }
}
