# 79 — Contract: names every worker codes against

*Base: `main` @ `eb9bd242` (design 79 v0.3 committed). Integration branch `feature/79`. K writes this contract into code first (one commit on `feature/79-K`, merged fast-forward into `feature/79`); every other worker branches from that merge. Where this file and `79-sandbox-vm.md` disagree, this file wins for names and signatures, the design wins for behaviour.*

## 0. Workers, worktrees, ownership, merge order

| Tag | Worktree · branch | Owns (may edit) | Must not touch |
|---|---|---|---|
| **K** contract | `../theuth-79-K` · `feature/79-K` | everything in §2 exactly as written: `crates/sandbox/src/{vm/mod.rs,types.rs}`, `crates/sandbox/Cargo.toml` (+`ts` feature, `schemars`, `ts-rs`, `agent` dep), stub RPC methods + `forward!`/subscribe + `export.rs` in `crates/rpc`, `crates/rpc/Cargo.toml` (`sandbox` dep, `sandbox/ts`), `EngineClass::Vm` + `mesh.class` in `crates/mesh/src/{types.rs,config.rs}`, `runs_in` in `crates/roster/src/{file.rs,types.rs}`, attention kinds `Handoff`/`Secret` in `crates/roster/src/attention.rs` (variants + id formats only), actor message structs + stub handlers in `crates/sandbox/src/lib.rs`, regenerated `bindings/` | behaviour of anything; the desktop UI; `deploy/` |
| **S** vm runtime | phase 1: `/tmp/msb-spike-79` (standalone cargo project); phase 2: `../theuth-79-S` · `feature/79-S` | `crates/sandbox/src/vm/{microsandbox.rs,install.rs,preflight.rs}`, `vm::runtime()` body in `vm/mod.rs`, `crates/sandbox/examples/vm_spike.rs`, `crates/sandbox/tests/vm.rs`, workspace `Cargo.toml` `[workspace.dependencies] microsandbox = …` + `Cargo.lock` | `crates/sandbox/src/lib.rs` beyond the `pub mod vm;` line K added; `main.rs` |
| **A** engine core | `../theuth-79-A` · `feature/79-A` | `crates/sandbox/src/{lib.rs,actor.rs,record.rs,bringup.rs,tools.rs,config.rs,guest.rs,takeover.rs}`, the sandbox RPC method bodies in `crates/rpc/src/api.rs`, `crates/theuth/src/main.rs` (`t mesh add --vm`, `t sandbox …`, `DEFAULT_CONFIG`), `crates/tui` `/sandbox`, README `sandbox:` block | `crates/sandbox/src/vm/**`, `deploy/`, UI |
| **C** mesh + pin | `../theuth-79-C` · `feature/79-C` | `crates/mesh/src/{stats.rs,lease.rs,tools.rs,lib.rs}` (class override, rank, `mesh_peers` line), `crates/rpc/src/acp.rs` (describe string), `crates/tasks/src/place.rs` (+ `Inputs.pin`), `crates/roster/src/{desk.rs,lib.rs}` (pass the pin; `EnsureVm` ask), tests | `crates/sandbox/**` except reading its types; UI |
| **H** hand-off + secrets | `../theuth-79-H` · `feature/79-H` | `crates/roster/src/{handoff.rs,attention.rs (derive),lib.rs (register)}`, `crates/roster/migrations/0004_handoffs.sql`, `crates/secrets/src/{lib.rs,tools.rs,store.rs,requests.rs}`, `crates/secrets/migrations/0002_requests.sql`, RPC bodies `handoffs_list`, `handoff_resolve`, `answer_secret` for replicated requests | `crates/sandbox/**`, `crates/mesh/**`, UI |
| **E** desktop UI | `../theuth-79-E` · `feature/79-E` | `crates/desktop/ui/**` (incl. `package.json`/lockfile for `@novnc/novnc`), `crates/desktop/src/commands.rs` only if the pop‑out needs a new label prefix | any Rust crate under `crates/` other than `desktop` |
| **B** image | `../theuth-79-B` · `feature/79-B` | `deploy/vm/**`, `justfile` (one new recipe block `vm-image` / `vm-image-dev`), `scripts/vm-*.sh` | `crates/**` |
| **I** integration | `../theuth-79` · `feature/79` | merges in the order K → S → A → C → H → E → B, runs the gates, the live smoke, writes `79-as-built-INTEGRATE.md` | new features |

**Working rules (every brief).** Work only in your worktree. No questions — decide, note the decision in your as‑built. Commit early and often on your branch with conventional messages (`feat(sandbox): …`); never amend, squash or rebase. Sky's untracked docs in the main checkout are read‑only. Stop new work at 35 % context left and write the as‑built. Gate before DONE: `cargo check -p <your crates> --tests`, `cargo clippy -p <your crates> -- -D warnings`, `cargo test -p <your crates> --lib`, plus what your brief adds; **never `cargo check --workspace`** (an hour under load). UI worker: `bun run tsc`, `bun run test`, `bun run build` in `crates/desktop/ui`. As‑built: `docs/design/79-as-built-<TAG>.md` — built per commit, deviations with reasons, not built, tests, check results, edits outside your files (flag each). Last line of your final message: `DONE <TAG>`. A secret placeholder is written in prose as "the 37 placeholder"; never the literal form.

## 1. Vocabulary

*sandbox VM* (user word: "sandbox VM"; record `backend: vm`; mesh `class: vm`), *runtime* (microsandbox), *record* (`<data_dir>/computers/<name>.yaml`), *guest* (the engine inside), *host engine* (the engine that owns the record), *share* (a bind mount), *hand‑off* (a human at the desktop), *pin* (`runs_in`).

## 2. Contract code (K writes these verbatim; owners fill bodies)

### 2.1 `crates/sandbox/src/vm/mod.rs`

```rust
//! A microVM runtime (design 79 §4.1): knows images, resources, mounts, ports,
//! snapshots, metrics — nothing about engines or the mesh. `microsandbox.rs`
//! is the one impl; `MockVmRuntime` serves tests.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use crate::{Cmd, ExecOutput, Progress};

pub mod install;      // S: runtime install/resolve (setup::* of the SDK)
pub mod preflight;    // S: host facts + blockers
pub mod microsandbox; // S
#[cfg(any(test, feature = "mock"))]
pub mod mock;         // K: MockVmRuntime — in-memory state machine, records calls

/// Build the configured runtime. K: `Err(anyhow!("vm runtime not built"))`; S replaces.
pub fn runtime(cfg: &crate::types::VmConfig) -> anyhow::Result<Arc<dyn VmRuntime>>;

#[async_trait::async_trait]
pub trait VmRuntime: Send + Sync {
    fn kind(&self) -> &'static str;                                              // "microsandbox" | "mock"
    async fn preflight(&self) -> Preflight;
    async fn install(&self, out: &Progress) -> anyhow::Result<()>;
    async fn create(&self, spec: &VmSpec, out: &Progress) -> anyhow::Result<VmHandle>;
    async fn status(&self, h: &VmHandle) -> anyhow::Result<VmStatus>;
    async fn start(&self, h: &VmHandle) -> anyhow::Result<()>;
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
    async fn snapshot(&self, h: &VmHandle, name: &str, full: bool, out: &Progress) -> anyhow::Result<SnapshotId>;
    /// Always a new VM (upstream semantics); `spec` supplies ports and mounts again; `as_name` must be free.
    async fn restore(&self, id: &SnapshotId, as_name: &str, spec: &VmSpec, out: &Progress) -> anyhow::Result<VmHandle>;
    async fn snapshots(&self, h: &VmHandle) -> anyhow::Result<Vec<SnapshotInfo>>;
    async fn delete_snapshot(&self, id: &SnapshotId) -> anyhow::Result<()>;
    /// Ours only: label `io.theuth.sandbox`.
    async fn list(&self) -> anyhow::Result<Vec<VmHandle>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmHandle { pub name: String, pub runtime_id: Option<String> }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum VmStatus { Running, Stopped, Paused, Crashed, Gone }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageRef { Oci { r#ref: String }, Bootstrap { base: String, script: String } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Mount { pub host: PathBuf, pub guest: String, #[serde(default)] pub readonly: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Port { pub host_ip: String /* "127.0.0.1" */, pub host: u16 /* 0 = runtime picks; the chosen port is read back from `create` */, pub guest: u16, #[serde(default)] pub udp: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NetPolicy { #[default] PublicHost, PublicHostPrivate, PublicOnly, None }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Init { #[default] Auto, Agent }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrebootFile { pub path: String, pub bytes: Vec<u8>, pub mode: u32 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmSpec {
    pub name: String, pub image: ImageRef,
    pub cpus: u8, pub max_cpus: u8, pub memory_mib: u32, pub max_memory_mib: u32, pub root_disk_mib: u32,
    pub mounts: Vec<Mount>, pub ports: Vec<Port>, pub network: NetPolicy, pub init: Init,
    pub hostname: String, pub env: BTreeMap<String, String>, pub files: Vec<PrebootFile>,
    pub labels: BTreeMap<String, String>, pub ephemeral: bool,
    pub idle_timeout_secs: Option<u32>, pub max_duration_secs: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResourceChange { pub cpus: Option<u8>, pub max_cpus: Option<u8>, pub memory_mib: Option<u32>, pub max_memory_mib: Option<u32>, pub root_disk_mib: Option<u32> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ResizeStatus { Applied, Converging, GuestRefused { why: String } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResizeOutcome { pub applied: ResourceChange, pub next_start: ResourceChange, pub status: ResizeStatus, pub restart_required: bool }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Pending { pub restart_required: bool }

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VmMetrics { pub cpu_percent: f32, pub memory_bytes: u64, pub memory_limit_bytes: u64, pub disk_used_bytes: u64, pub disk_free_bytes: u64, pub net_rx_bytes: u64, pub net_tx_bytes: u64, pub uptime_secs: u64, pub at_ms: u64 }
// ts-rs: every u64 field carries #[cfg_attr(feature = "ts", ts(type = "number"))] (the repo's convention, see roster AttentionItem.at_ms).

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LogLine { pub at_ms: u64, pub source: String /* stdout|stderr|system|kernel */, pub line: String }

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SnapshotId(pub String);   // "<vm>:<label>"

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SnapshotInfo { pub id: SnapshotId, pub label: String, pub full: bool, pub at_ms: u64, pub bytes: Option<u64>, pub auto: bool }

/// Host readiness. `blockers` empty ⇒ a VM can be created now. Every blocker is user-facing:
/// `code` stable for the UI, `message` one sentence, `fix` what to do (or None when nothing can be done on this host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Preflight { pub runtime: RuntimeState, pub virt: VirtState, pub host: HostFacts, pub defaults: Resources, pub blockers: Vec<Blocker>, pub warnings: Vec<Blocker>, pub checked_at_ms: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Blocker { pub code: String, pub message: String, pub fix: Option<String>, pub installable: bool /* true only for runtime_missing / runtime_incomplete */ }
// codes: unsupported_os (Windows v1), unsupported_arch (x86_64 macOS: "microsandbox needs Apple Silicon"), no_kvm ("/dev/kvm missing — enable virtualization in firmware, or nested virtualization in the outer hypervisor"),
// kvm_not_writable ("add your user to the kvm group: sudo usermod -aG kvm $USER, then log in again"), glibc_too_old ("needs glibc ≥ 2.28"), runtime_missing, runtime_incomplete ("msb X but libkrunfw Y"),
// disk_low ("N GiB free; a VM needs root_disk + image + 2 GiB"), memory_low (warning: default memory > host/2), docker_missing (warning, dev image path only)

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RuntimeState { Installed { version: String, home: String, libkrunfw: String }, Missing, Incomplete { why: String } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VirtState { Ok { via: String /* "hvf" | "kvm" | "whp" */ }, Unsupported { why: String } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HostFacts { pub os: String, pub arch: String, pub cores: u32, pub mem_bytes: u64, pub disk_free_bytes: u64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Resources { pub cpus: u8, pub max_cpus: u8, pub memory_mib: u32, pub max_memory_mib: u32, pub root_disk_mib: u32 }
```
`Resources::derive(host: &HostFacts, cfg: &VmDefaults) -> Resources` (K, pure, tested): `cpus = clamp(cores/4, 2, 4)`, `memory_mib = clamp(mem/4, 2048, 8192)`, `root_disk_mib = 20480`, `max_cpus = cores`, `max_memory_mib = mem/2` — each overridden by a non‑`auto` config value.

### 2.2 `crates/sandbox/src/types.rs` — wire types (all `Serialize, Deserialize, schemars::JsonSchema`, `ts(export)` under `ts`)

```rust
pub struct SandboxInfo { pub name: String, pub backend: String /* docker|ssh|vm */, pub state: SandboxState, pub engine: Option<String>,
    pub vm: Option<VmView>, pub created_ms: u64, pub updated_ms: u64 }

#[serde(tag = "state", rename_all = "snake_case")]
pub enum SandboxState { Creating { step: String }, Ready, Starting, Running, Paused, Stopped, Crashed, Gone, Failed { step: String, error: String }, Destroying }

pub struct VmView { pub image: ImageRef, pub resources: Resources, pub shares: Vec<Mount>, pub desktop: Option<DesktopView>,
    pub network: NetPolicy, pub autostart: bool, pub pending: PendingView, pub metrics: Option<VmMetrics>, pub snapshots: Vec<SnapshotInfo>,
    pub mesh_port: Option<u16> /* host loopback UDP port for iroh */, pub host_engine: String /* EngineId of the owner */ }
pub struct DesktopView { pub enabled: bool, pub geometry: String, pub host_port: Option<u16> }
#[derive(Default)] pub struct PendingView { pub shares: bool, pub ports: bool, pub resources: bool }

/// RPC + tool argument for a new VM. Every field optional except `name`; defaults from Preflight.defaults / VmDefaults.
pub struct SandboxCreate { pub name: String, pub purpose: Option<String>, pub cpus: Option<u8>, pub memory_mib: Option<u32>, pub root_disk_mib: Option<u32>,
    pub max_cpus: Option<u8>, pub max_memory_mib: Option<u32>, pub shares: Vec<Mount>, pub desktop: Option<bool>, pub geometry: Option<String>,
    pub network: Option<NetPolicy>, pub image: Option<ImageRef>, pub autostart: Option<bool>, pub local_t: Option<PathBuf> }

pub struct DesktopAccess { pub ws_url: String /* ws://127.0.0.1:<p>/websockify */, pub http_url: String /* http://127.0.0.1:<p>/vnc.html */, pub password: String, pub geometry: String, pub host_engine: String }

#[serde(tag = "type")]
pub enum SandboxEvent {
    Changed  { sandbox: SandboxInfo },
    Progress { name: String, step: String, line: String, pct: Option<u8>, session: Option<runtime::SessionId>, call_id: Option<String> },
    Removed  { name: String },
    OpenDesktop { name: String, ws_url: String },
    Takeover { name: String, engine: String, on: bool },
}

#[serde(default)] pub struct SandboxConfig { pub enabled: bool /* true */, pub vm: VmConfig, pub create_timeout_secs: u64 /* 900 */, pub step_timeout_secs: u64 /* 600 */,
    pub health_interval_secs: u64 /* 60 */, pub max_concurrent_bringups: usize /* 2 */, pub snapshots_keep: usize /* 5 */, pub handoff_timeout_secs: u64 /* 1800 */,
    pub guest: Option<GuestConfig> }
#[serde(default)] pub struct VmConfig { pub enabled: bool /* true */, pub runtime: String /* "microsandbox" */, pub home: String /* "" */, pub image: String /* "ghcr.io/basedatum/theuth-vm" */,
    pub defaults: VmDefaults, pub autostart: bool /* true */, pub max_vms: usize /* 4 */, pub resize_settle_secs: u64 /* 20 */ }
#[serde(default)] pub struct VmDefaults { pub cpus: AutoOr<u8>, pub memory_mib: AutoOr<u32>, pub root_disk_mib: u32 /* 20480 */, pub max_cpus: AutoOr<u8>, pub max_memory_mib: AutoOr<u32>,
    pub desktop: bool /* true */, pub geometry: String /* "1440x900" */, pub network: NetPolicy /* PublicHost */ }
/// `auto` or a number in YAML.  #[serde(untagged)] enum AutoOr<T> { Auto(AutoWord), Value(T) } — K provides with tests.
pub struct GuestConfig { pub name: String, pub host: String /* host engine name */, pub shares: Vec<String> }
```

### 2.3 Actor messages (`crates/sandbox/src/lib.rs`; K adds structs + stub handlers replying `Err("not built")`; A implements)

`Create { spec: SandboxCreate, origin: Option<(runtime::SessionId, String)> } → Result<SandboxInfo, String>` (returns at once, state `creating`) · `Retry { name }` · `Start { name }` · `Stop { name, grace_secs: u64, force: bool }` · `Pause { name }` · `Resume { name }` · `Resize { name, change: ResourceChange } → Result<ResizeOutcome, String>` · `SetShares { name, shares: Vec<Mount> } → Result<Pending, String>` · `Snapshot { name, label: String, full: bool } → Result<SnapshotInfo, String>` · `Restore { name, id: SnapshotId }` · `Destroy { name, keep_snapshots: bool }` · `DesktopAccess { name } → Result<DesktopAccess, String>` · `PreflightNow → Preflight` · `InstallRuntime → Result<(), String>` · `List → Vec<SandboxInfo>` · `Metrics { name } → Result<VmMetrics, String>` · `Logs { name, tail: usize } → Result<Vec<LogLine>, String>` · `Takeover { name, on: bool } → Result<u32 /* sessions steered */, String>` · `EnsureVm { name, agent: Option<String> } → Result<SandboxInfo, String>` (create with defaults if missing, start if stopped; used by the pin, C) · existing `MakeEngine`, `Engines` stay.

### 2.4 RPC (`crates/rpc/src/api.rs`; K adds methods that lookup+ask; bodies are the actor's)

| method | params | returns |
|---|---|---|
| `sandboxes_list` | — | `Vec<SandboxInfo>` |
| `sandbox_preflight` | — | `Preflight` |
| `sandbox_install_runtime` | — | `()` (progress as `SandboxEvent::Progress { name: "runtime", … }`) |
| `sandbox_create` | `spec: SandboxCreate` | `SandboxInfo` |
| `sandbox_retry` / `sandbox_start` / `sandbox_pause` / `sandbox_resume` | `name` | `SandboxInfo` |
| `sandbox_stop` | `name, grace_secs?: u64, force?: bool` | `SandboxInfo` |
| `sandbox_resize` | `name, change: ResourceChange` | `ResizeOutcome` |
| `sandbox_set_shares` | `name, shares: Vec<Mount>` | `Pending` |
| `sandbox_snapshot` | `name, label, full?: bool` | `SnapshotInfo` |
| `sandbox_restore` | `name, id: SnapshotId` | `SandboxInfo` |
| `sandbox_destroy` | `name, keep_snapshots?: bool` | `()` |
| `sandbox_desktop_access` | `name` | `DesktopAccess` |
| `sandbox_metrics` | `name` | `VmMetrics` |
| `sandbox_logs` | `name, tail?: usize` | `Vec<LogLine>` |
| `sandbox_takeover` | `name, on: bool` | `u32` |
| `handoffs_list` | — | `Vec<Handoff>` (H) |
| `handoff_resolve` | `id, outcome: "done" \| "cancelled"` | `()` (H) |

Events on the bridge: `SandboxEvent` (new) and `MeshEvent` (existing type, newly forwarded). `export.rs`: every type in §2.1/§2.2 with `ts(export)`, `Handoff`, `SecretRequest`. `just ts && just ts-desktop` regenerated in K's commit; E adds the methods to `ui/src/rpc/api.ts` + the method list.

### 2.5 Tools (A in `crates/sandbox/src/tools.rs` unless noted; `CONSUMES_SECRETS = false` on all sandbox tools)

`sandbox_create` (Mutating; args = `SandboxCreate` + `wait: Option<bool>` default true) · `sandbox_install_runtime` (Mutating) · `sandbox_list` (ReadOnly) · `sandbox_start` / `sandbox_stop {name, grace_secs?, force?}` / `sandbox_pause` / `sandbox_resume` (Mutating) · `sandbox_resize {name, cpus?, memory_mib?, root_disk_mib?, max_cpus?, max_memory_mib?}` (Mutating) · `sandbox_share {name, host, guest?, readonly?}` / `sandbox_unshare {name, guest}` (Mutating) · `sandbox_snapshot {name, label, full?}` (Mutating) · `sandbox_restore {name, id}` (Destructive) · `sandbox_destroy {name, keep_snapshots?}` (Destructive) · `sandbox_desktop {name}` (ReadOnly) · `sandbox_takeover {name, on}` (Mutating) · **H:** `desktop_handoff {reason, url?, timeout_secs?}` (ReadOnly, `crates/roster`) · `secret_request_form {title, fields: [{name, label, masked?}]}` (ReadOnly, `crates/secrets`). Text returns are one line per sandbox / one line per outcome; `sandbox_create` streams `Progress` with the caller's `(session, call_id)`.

Prompt blocks (A, `RegisterPrompt`): host block when `sandbox.enabled && vm.enabled` (one paragraph, §4.6 of the design); guest block when `sandbox.guest` is set (this engine is sandbox VM `<name>` on `<host>`, display `:1`, shares, the user may be watching or driving; after `<desktop_takeover by="user"/>` stop `computer_*` until `<desktop_handback/>`).

### 2.6 Record (`crates/sandbox/src/record.rs`, A) — `Computer` gains `#[serde(default)] pub vm: Option<VmRecord>`

```yaml
vm:
  runtime: microsandbox
  image: { kind: oci, ref: ghcr.io/basedatum/theuth-vm:2.1.2 }
  resources: { cpus: 4, max_cpus: 16, memory_mib: 4096, max_memory_mib: 32768, root_disk_mib: 20480 }
  shares: [ { host: /Users/sky/Develop, guest: /shared/Develop, readonly: false } ]
  desktop: { enabled: true, geometry: 1440x900, host_port: 61234 }
  mesh_port: { guest: 7800, host_port: 61235 }
  network: public_host
  autostart: true
  state: { state: ready }
  pending: { shares: false, ports: false, resources: false }
  setup: { step: verify, at_ms: 0, error: null }
  snapshots: []
  runtime_name: lab           # the microsandbox sandbox name currently backing the record (`lab` or `lab~` after a restore)
```

### 2.7 Replicated rows (H; K writes nothing here)

`crates/roster/migrations/0004_handoffs.sql`: `handoffs(id TEXT PRIMARY KEY, engine TEXT NOT NULL, session TEXT NOT NULL, agent TEXT, reason TEXT NOT NULL, url TEXT, created_ms INTEGER NOT NULL, resolved_ms INTEGER, outcome TEXT, hlc TEXT)` attached to the replica (single PK, nullable columns, LWW — the stage‑1 rules). Rust `Handoff` mirrors it (wire type). Tool `desktop_handoff` inserts and polls `resolved_ms` every 1 s until `timeout_secs`; `handoff_resolve` sets `resolved_ms`, `outcome`. Attention derives `AttentionKind::Handoff` items `handoff:<id>` from unresolved rows (title `"<agent or session> needs you: <reason>"`, `detail` = VM name when the engine is a VM record on this hub, else the engine name).
`crates/secrets/migrations/0002_requests.sql`: `secret_requests(id TEXT PRIMARY KEY, engine TEXT NOT NULL, session TEXT NOT NULL, title TEXT, fields TEXT NOT NULL /* json [{name,label,masked,current}] */, created_ms INTEGER NOT NULL, answered_ms INTEGER, outcome TEXT, hlc TEXT)` attached to the replica. `secret_request` and `secret_request_form` write a row *and* publish the local `SecretsEvent::Requested` as today (so a local UI answers instantly); `answer_secret { id, values }` on any engine writes the vault rows and `answered_ms`; the requesting tool returns when the row is answered or its vault names appear. Attention derives `AttentionKind::Secret` items `secret:<id>`. The existing masked card gains multi‑field rendering (E) from `fields`.

### 2.8 Pin (K: parse; C: behaviour)

`agents/<name>.md` front matter `runs_in: any | vm | vm:<name>` → `AgentFile.runs_in: RunsIn` with `enum RunsIn { Any, OwnVm, Vm(String) }` (`Default = Any`; serialised back only when not `Any`). `tasks::place::Inputs` gains `pub pin: Option<Pin>` with `enum Pin { Engine(String) }` — the roster resolves `OwnVm` → engine of record `vm-<agent>` and `Vm(n)` → engine of record `n` by asking `SandboxActor::EnsureVm` (which creates/starts and returns the record; the engine id may be `None` while `creating` → the task is placed `blocked` on `sandbox:<name>` and re‑placed on `SandboxEvent::Changed { state: ready }`). With a pin, `place` returns `Placed { where_: <pinned engine>, harness: <as policy> }` or `Blocked(reason)`; never a fallback engine.

### 2.9 Mesh (K: variant + config key; C: behaviour)

`EngineClass::Vm` (`serde` "vm"); `MeshConfig.class: Option<EngineClass>` (`None` = detect). `stats::probe` returns the declared class when set. `lease` ranks `Vm` with `Laptop`. `mesh_peers` prints `class=vm`. `rpc::acp` describe string unchanged (`mesh · vm · harnesses: …`).

### 2.10 Guest config written at create (A, §5.3 of the design) — verbatim keys

`mesh: { name, hub: never, class: vm, bind: "0.0.0.0:7800", relay: <host's>, workspaces: [<guest share paths>] }`, `secrets: { key_store: file }`, `link: { enabled: false }`, `tools: { policy: <host's> }`, `computer: { enabled: true }`, `sandbox: { enabled: false, guest: { name, host, shares } }`.

### 2.11 Desktop UI names (E)

Settings › Mesh section **Sandbox VMs** (`components/settings/SandboxVms.tsx`, mounted from `Mesh.tsx`); sheets `SandboxCreateSheet`, `SandboxResizeSheet`, `SandboxSharesSheet`, `SandboxSnapshots`; card `SandboxPreflightCard` (renders `Preflight.blockers` as a red list with `fix` lines, an **Install runtime** button when a blocker is `installable`, and *disables Add* while blockers exist — this is the "your system does not meet the requirements" surface, one sentence per blocker, never a generic error); row `SandboxRow` (gauges from `vm.metrics`); pop‑out route `routes/vm.$name.tsx` → `components/vm/VmDesktop.tsx` (`@novnc/novnc` `RFB`, `resizeSession`, `viewOnly`, Watch / Take over, hand‑off banner); `open_window("popout-vm-<name>", "/vm/<name>")`; session header button **Desktop** when the session's owner engine matches a `SandboxInfo.engine`; attention cards for kinds `handoff` (opens the pop‑out) and `secret` (masked fields from `fields`); Agents editor **Runs in** select (`any` / `own VM` / named). Events: `SandboxEvent` reduced into a `sandboxes` query cache (`['sandboxes']`), `Progress` appended to a per‑name log (cap 500 lines).

### 2.12 Image (B) — `deploy/vm/`

`Dockerfile` (`FROM ghcr.io/basedatum/theuth-vm-base:trixie`), `Dockerfile.dev` (copies `./t` — a Linux `t` for the host's arch), `bootstrap.sh` (the stock‑image first‑boot script producing the same end state), `units/{xvnc,desktop,novnc,theuth}.service`, `README.md` (how to build, load, run, verify). User `engine` uid 1000; `/data`; `t` at `/usr/local/bin/t`; `theuth.service` env `DISPLAY=:1 THEUTH_DATA_DIR=/data THEUTH_LAUNCH=1`, `Restart=always`; `xvnc.service` `Xtigervnc :1 -rfbauth /home/engine/.vnc/passwd -geometry 1440x900 -depth 24 -localhost -AcceptSetDesktopSize` (a placeholder `passwd` is baked so the unit starts before bring‑up replaces it); `novnc.service` `websockify --web=/usr/share/novnc 0.0.0.0:6080 localhost:5901`; `desktop.service` `dbus-launch --exit-with-session startxfce4` as `engine`. `just vm-image [tag]` (buildx amd64+arm64 → `ghcr.io/basedatum/theuth-vm:<tag>`), `just vm-image-dev` (local arch, `docker save | msb load`). Image label `io.theuth.vm.schema=1`.

## 3. Definition of done per worker (the gate lines that go in each brief are in the brief; this is what I merges against)

K: bindings regenerated, every stub compiles, `Resources::derive` + `AutoOr` + `runs_in` parse tests green. S: `cargo run -p sandbox --example vm_spike` on the Mac prints the measurement table (install, create bootstrap VM, exec, resize live ×2, disk grow, mounts‑on‑stopped, UDP publish, stop/start persistence, detach survival, snapshot/restore, destroy). A: RPC smoke with the mock runtime (`create` → `Progress` → `ready` → `destroy`) and, when S is merged, the real one. C: `tests` for rank/class/pin. H: two‑engine test of a replicated request row is *not* required — a single‑engine round trip through the replica tables is. E: tsc/vitest/build green with the regenerated bindings; a storybook‑style `?sim=` fixture for the preflight card in the blocked state. B: image boots to `/vnc.html` 200 under `msb run` on the Mac; `computer_screenshot` from a `t` session inside returns XFCE.
