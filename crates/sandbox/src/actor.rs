//! The [`SANDBOX_ACTOR`](crate::SANDBOX_ACTOR) actor (79 §4.4): one record per
//! sandbox, the VM runtime behind `VmRuntime`, bring-up as spawned tasks with
//! `SandboxEvent::Progress`, reconcile against the runtime, and the docker/ssh
//! `MakeEngine` path that predates it.
//! Message fields are the 79 contract (§2.3) verbatim; each struct's doc line is the message's documentation.
#![allow(missing_docs)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::bringup::{self, HostSide, MeshJoin, VmCx};
use crate::record::{self, Computer, DesktopRecord, MeshPort, Setup, Store, VmRecord};
use crate::types::{
    self, PendingView, SandboxConfig, SandboxCreate, SandboxEvent, SandboxInfo, SandboxState,
};
use crate::vm::{
    Blocker, HostFacts, ImageRef, LogLine, Mount, Pending, Preflight, ResizeOutcome,
    ResourceChange, Resources, RuntimeState, SnapshotId, VirtState, VmHandle, VmMetrics, VmRuntime,
    VmSnapshotInfo, VmStatus,
};
use crate::{Progress, Target, make_engine, sq};

/// Vault prefix of a VM's rows: `sandbox/<name>/vnc`.
pub const VAULT_PREFIX: &str = "sandbox/";

/// The vault as the actor uses it (one impl over `secrets::handle`, one in-memory for tests).
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn set(&self, name: &str, value: &str) -> Result<(), String>;
    async fn get(&self, name: &str) -> Option<Zeroizing<String>>;
    async fn delete(&self, name: &str) -> Result<(), String>;
}

/// [`SecretStore`] over this engine's vault (`secrets::handle`).
pub struct VaultSecrets(pub Runtime);

#[async_trait::async_trait]
impl SecretStore for VaultSecrets {
    async fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let vault = secrets::handle(&self.0)
            .await
            .ok_or("secrets vault unavailable")?;
        let expected = vault.get_versioned(name).map(|(v, _)| v);
        // `set_if_version` is the writer for reserved `/` rows (37 §11).
        match vault
            .set_if_version(name, value, "sandbox", expected)
            .await?
        {
            Some(_) => Ok(()),
            None => Err(format!("{name}: concurrent write; retry")),
        }
    }

    async fn get(&self, name: &str) -> Option<Zeroizing<String>> {
        secrets::handle(&self.0).await?.get(name, 0)
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        let vault = secrets::handle(&self.0)
            .await
            .ok_or("secrets vault unavailable")?;
        vault.delete(name).await
    }
}

/// The vault row of a VM's VNC password.
pub fn vnc_row(name: &str) -> String {
    format!("{VAULT_PREFIX}{name}/vnc")
}

/// 8 alphanumeric characters (VNC auth truncates at 8).
pub fn vnc_password() -> Zeroizing<String> {
    use rand::Rng;
    const ALNUM: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut r = rand::rng();
    Zeroizing::new(
        (0..8)
            .map(|_| ALNUM[r.random_range(0..ALNUM.len())] as char)
            .collect(),
    )
}

/// `[a-z0-9-]{1,63}`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `ask` → `Result<mesh::Peer, String>`: run bring-up on `Target` and join the
/// mesh; the reply is the new peer. Progress goes to the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MakeEngine(pub Target);

/// `ask` → `Result<Vec<Computer>, String>`: the records under `computers/`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Engines;

// ---- 79 contract §2.3 messages.

/// `ask` → `Result<SandboxInfo, String>`: create a VM record and start bring-up; returns
/// at once with `state: creating`. `origin` = the tool call to stream progress into.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Create {
    /// What to create.
    pub spec: SandboxCreate,
    /// `(session, call_id)` of the asking tool call, for progress lines.
    pub origin: Option<(runtime::SessionId, String)>,
}

/// `ask` → `Result<SandboxInfo, String>`: resume a `failed` bring-up from its step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Retry {
    pub name: String,
}

/// `ask` → `Result<SandboxInfo, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub name: String,
}

/// `ask` → `Result<SandboxInfo, String>`: graceful for `grace_secs`, then kill; `force` skips the busy-sessions refusal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stop {
    pub name: String,
    pub grace_secs: u64,
    pub force: bool,
}

/// `ask` → `Result<SandboxInfo, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pause {
    pub name: String,
}

/// `ask` → `Result<SandboxInfo, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resume {
    pub name: String,
}

/// `ask` → `Result<ResizeOutcome, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resize {
    pub name: String,
    pub change: ResourceChange,
}

/// `ask` → `Result<Pending, String>`: the full share list; applied by the next start.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetShares {
    pub name: String,
    pub shares: Vec<Mount>,
}

/// `ask` → `Result<VmSnapshotInfo, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub label: String,
    pub full: bool,
}

/// `ask` → `Result<SandboxInfo, String>`: the current disk is discarded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restore {
    pub name: String,
    pub id: SnapshotId,
}

/// `ask` → `Result<(), String>`: peer, VM, vault row and record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Destroy {
    pub name: String,
    pub keep_snapshots: bool,
}

/// `ask` → `Result<types::DesktopAccess, String>`: the noVNC credential (RPC only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopAccess {
    pub name: String,
}

/// `ask` → [`Preflight`]: host readiness (cached ≤ 60 s).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PreflightNow;

/// `ask` → `Result<(), String>`: install msb + libkrunfw (progress as `SandboxEvent::Progress { name: "runtime" }`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct InstallRuntime;

/// `ask` → `Vec<SandboxInfo>`: every record.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct List;

/// `ask` → `Result<VmMetrics, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
    pub name: String,
}

/// `ask` → `Result<Vec<LogLine>, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Logs {
    pub name: String,
    pub tail: usize,
}

/// `ask` → `Result<u32, String>`: sessions steered (79 §6.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Takeover {
    pub name: String,
    pub on: bool,
}

/// `ask` → `Result<SandboxInfo, String>`: create with defaults if missing, start if
/// stopped; the pin's entry point (79 §2.8, C).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnsureVm {
    pub name: String,
    pub agent: Option<String>,
}

/// `ask` → `Result<String /* job id */, String>`: open an interactive shell in the VM as a PTY
/// job of `session` (`msb exec --tty <runtime_name> -- runuser -u engine -- bash -l`, root's
/// `bash -l` when `runuser` is absent); the UI attaches with `subscribe_pty(session, job)`.
#[derive(Clone, Debug)]
pub struct Terminal {
    pub name: String,
    pub session: runtime::SessionId,
    /// The session's file root (`Authorized.resources.root`): the job log lives under it.
    pub root: PathBuf,
}

/// `tell`: reconcile every record against the runtime (plugin start, every `health_interval_secs`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Reconcile {
    /// Start stopped records with `autostart` (engine start only).
    pub autostart: bool,
}

/// `tell`: a bring-up task finished (frees the name).
#[derive(Clone, Debug)]
pub struct BringupDone(pub String);

/// The [`SANDBOX_ACTOR`](crate::SANDBOX_ACTOR) actor.
#[derive(kameo::Actor)]
pub struct SandboxActor {
    rt: Runtime,
    cfg: SandboxConfig,
    store: Store,
    /// `Err` = why VMs cannot be made here (disabled, or `vm::runtime()` failed).
    vm: Result<Arc<dyn VmRuntime>, String>,
    join: Arc<dyn MeshJoin>,
    secrets: Arc<dyn SecretStore>,
    metrics: HashMap<String, VmMetrics>,
    inflight: HashSet<String>,
    bringups: Arc<tokio::sync::Semaphore>,
    preflight: Option<(Instant, Preflight)>,
    host_engine: Option<String>,
}

impl SandboxActor {
    /// The production actor: `vm::runtime(&cfg.vm)` when `vm.enabled`, the mesh actor, the vault.
    /// `cfg.vm.home` becomes the home theuth really uses (`install::effective_home`: shared when
    /// it matches the embedded runtime, private otherwise) so the backend, shares and preflight agree.
    pub fn new(rt: Runtime, mut cfg: SandboxConfig) -> Self {
        let vm = if cfg.vm.enabled {
            cfg.vm.home = crate::vm::install::effective_home(&cfg.vm.home, &rt.data_dir);
            crate::vm::runtime(&cfg.vm).map_err(|e| format!("{e:#}"))
        } else {
            Err("sandbox VMs are disabled (sandbox.vm.enabled: false)".into())
        };
        if let Err(e) = &vm {
            tracing::warn!("sandbox: {e}");
        }
        Self::with(
            rt.clone(),
            cfg,
            vm,
            Arc::new(crate::ActorJoin),
            Arc::new(VaultSecrets(rt)),
        )
    }

    /// Every dependency injected (tests: `MockVmRuntime`, a fake `MeshJoin`, an in-memory vault).
    pub fn with(
        rt: Runtime,
        cfg: SandboxConfig,
        vm: Result<Arc<dyn VmRuntime>, String>,
        join: Arc<dyn MeshJoin>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            store: Store::new(rt.data_dir.clone()),
            bringups: Arc::new(tokio::sync::Semaphore::new(
                cfg.max_concurrent_bringups.max(1),
            )),
            rt,
            cfg,
            vm,
            join,
            secrets,
            metrics: HashMap::new(),
            inflight: HashSet::new(),
            preflight: None,
            host_engine: None,
        }
    }

    fn vm(&self) -> Result<Arc<dyn VmRuntime>, String> {
        self.vm.clone()
    }

    async fn host_engine(&mut self) -> String {
        if let Some(e) = &self.host_engine {
            return e.clone();
        }
        let id = self
            .join
            .status()
            .await
            .map(|s| s.self_id.0)
            .unwrap_or_default();
        if !id.is_empty() {
            self.host_engine = Some(id.clone());
        }
        id
    }

    async fn info(&mut self, rec: &Computer) -> SandboxInfo {
        let host = self.host_engine().await;
        rec.info(&host, self.metrics.get(&rec.name).copied())
    }

    async fn publish_changed(&mut self, rec: &Computer) {
        let info = self.info(rec).await;
        self.rt
            .publish(SandboxEvent::Changed { sandbox: info })
            .await;
    }

    /// This engine's side of a VM (contract §2.10 inputs).
    fn host_side(&self) -> HostSide {
        let mesh: mesh::MeshConfig = self.rt.section("mesh").unwrap_or_default();
        let tools_policy = self
            .rt
            .config
            .extract_inner::<serde_json::Value>("tools.policy")
            .ok()
            .filter(|v| !v.is_null());
        HostSide {
            relay: mesh.relay,
            tools_policy,
            host_name: mesh::self_name(),
            loopback_mesh: self.cfg.vm.loopback_mesh,
            step_timeout: crate::config::step_timeout(&self.cfg),
            pair_tries: 6,
            verify_tries: 3,
            // 10 s, or a sixth of a short step ceiling (tests).
            verify_every: Duration::from_secs(10).min(crate::config::step_timeout(&self.cfg) / 6),
        }
    }

    /// A `Progress` that publishes `SandboxEvent::Progress` for `name`, joined to `origin`.
    fn progress(&self, name: &str, origin: Option<(runtime::SessionId, String)>) -> Progress {
        let rt = self.rt.clone();
        let name = name.to_owned();
        Progress::new(move |step, line| {
            tracing::info!("sandbox {name} {step}: {line}");
            let ev = SandboxEvent::Progress {
                name: name.clone(),
                step: step.to_owned(),
                line: line.to_owned(),
                pct: None,
                session: origin.as_ref().map(|o| o.0),
                call_id: origin.as_ref().map(|o| o.1.clone()),
            };
            let rt = rt.clone();
            tokio::spawn(async move { rt.publish(ev).await });
        })
    }

    /// Preflight, cached ≤ 60 s; without a runtime, one blocker saying why.
    async fn preflight(&mut self) -> Preflight {
        if let Some((at, p)) = &self.preflight
            && at.elapsed() < Duration::from_secs(60)
        {
            return p.clone();
        }
        let p = match self.vm() {
            Ok(rt) => rt.preflight().await,
            Err(why) => {
                let host = HostFacts {
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                    cores: std::thread::available_parallelism().map_or(0, |n| n.get() as u32),
                    mem_bytes: 0,
                    disk_free_bytes: 0,
                };
                let code = if self.cfg.vm.enabled {
                    "runtime_missing"
                } else {
                    "vm_disabled"
                };
                Preflight {
                    runtime: RuntimeState::Missing,
                    virt: VirtState::Unsupported { why: why.clone() },
                    defaults: Resources::derive(&host, &self.cfg.vm.defaults),
                    host,
                    blockers: vec![Blocker {
                        code: code.into(),
                        message: why,
                        fix: None,
                        installable: false,
                    }],
                    warnings: Vec::new(),
                    checked_at_ms: record::now_ms(),
                }
            }
        };
        let cache = self.rt.data_dir.join("sandbox").join("runtime.json");
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&cache, serde_json::to_vec_pretty(&p).unwrap_or_default());
        self.preflight = Some((Instant::now(), p.clone()));
        p
    }

    /// Spawn the bring-up task for `name` (bounded by `max_concurrent_bringups`).
    fn spawn_bringup(
        &mut self,
        name: &str,
        origin: Option<(runtime::SessionId, String)>,
        me: ActorRef<Self>,
    ) -> Result<(), String> {
        let runtime = self.vm()?;
        if !self.inflight.insert(name.to_owned()) {
            return Err(format!("{name}: bring-up already running"));
        }
        let store = self.store.clone();
        let join = self.join.clone();
        let host = self.host_side();
        let out = self.progress(name, origin);
        let secrets = self.secrets.clone();
        let sem = self.bringups.clone();
        let name = name.to_owned();
        let me2 = me.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let desktop = store
                .get(&name)
                .ok()
                .and_then(|r| r.vm.map(|v| v.desktop.enabled))
                .unwrap_or(false);
            let vnc_password = if desktop {
                secrets.get(&vnc_row(&name)).await
            } else {
                None
            };
            // Own task: a panic in bring-up (runtime I/O, a parser) lands here as a JoinError
            // and is logged; the name is freed either way (bug 7 hardening).
            let n = name.clone();
            let ran = tokio::spawn(async move {
                let changed = move |rec: &Computer| {
                    let _ = me2.tell(Changed(rec.clone())).try_send();
                };
                let cx = VmCx {
                    runtime: &*runtime,
                    store: &store,
                    join: &*join,
                    out: &out,
                    host: &host,
                    vnc_password,
                    changed: &changed,
                };
                if let Err(e) = bringup::run_vm(&cx, &n).await {
                    out.say("failed", format!("{e:#}"));
                }
            })
            .await;
            if let Err(e) = ran {
                tracing::error!("sandbox {name}: bring-up task panicked: {e}");
            }
            let _ = me.tell(BringupDone(name)).await;
        });
        Ok(())
    }

    /// One reconcile pass: runtime status (+ metrics when running) → record state.
    async fn reconcile(&mut self, autostart: bool, me: ActorRef<Self>) {
        let Ok(runtime) = self.vm() else {
            return;
        };
        for rec in self.store.load_all() {
            let Some(vm) = &rec.vm else {
                continue;
            };
            if self.inflight.contains(&rec.name)
                || matches!(
                    vm.state,
                    SandboxState::Creating { .. }
                        | SandboxState::Failed { .. }
                        | SandboxState::Destroying
                )
            {
                continue;
            }
            let Ok(h) = rec.vm_handle() else {
                continue;
            };
            let status = runtime.status(&h).await.unwrap_or(VmStatus::Gone);
            let state = state_of(status);
            if status == VmStatus::Running
                && let Ok(m) = runtime.metrics(&h).await
            {
                self.metrics.insert(rec.name.clone(), m);
            } else {
                self.metrics.remove(&rec.name);
            }
            // The runtime's snapshot list is the truth (bug 6: restore/rename left it stale).
            let snapshots = self
                .all_snapshots(&rec.name)
                .await
                .ok()
                .filter(|l| *l != vm.snapshots);
            // `ready` means "brought up, not yet seen running": running confirms it.
            let changed = vm.state != state
                && !(vm.state == SandboxState::Ready && status != VmStatus::Running);
            if (changed || snapshots.is_some())
                && let Ok((_, r)) = self.store.update(&rec.name, |r| {
                    if changed {
                        r.set_state(state.clone());
                    }
                    if let Some(l) = &snapshots {
                        r.vm_mut()?.snapshots = l.clone();
                    }
                    Ok(())
                })
            {
                self.publish_changed(&r).await;
            }
            if autostart && self.cfg.vm.autostart && vm.autostart && status == VmStatus::Stopped {
                let me = me.clone();
                let name = rec.name.clone();
                tokio::spawn(async move {
                    if let Err(e) = me.ask(Start { name: name.clone() }).await {
                        tracing::warn!("sandbox autostart {name}: {e}");
                    }
                });
            }
        }
    }

    /// Fresh status for one record → state (+ metrics), written and published when it changed.
    async fn refresh(&mut self, name: &str) -> Result<Computer, String> {
        let runtime = self.vm()?;
        let rec = self.store.get(name)?;
        let h = rec.vm_handle()?;
        let status = runtime.status(&h).await.map_err(|e| format!("{e:#}"))?;
        let state = state_of(status);
        if status == VmStatus::Running
            && let Ok(m) = runtime.metrics(&h).await
        {
            self.metrics.insert(name.to_owned(), m);
        }
        if rec.state() == state {
            return Ok(rec);
        }
        let (_, rec) = self.store.update(name, |r| {
            r.set_state(state.clone());
            Ok(())
        })?;
        self.publish_changed(&rec).await;
        Ok(rec)
    }

    /// Every snapshot of record `name` under either runtime name (`lab` and `lab-r`: a restore
    /// alternates them and a snapshot keeps the group it was created under), oldest first.
    async fn all_snapshots(&self, name: &str) -> Result<Vec<VmSnapshotInfo>, String> {
        let runtime = self.vm()?;
        let mut list = Vec::new();
        for n in [name.to_owned(), format!("{name}-r")] {
            let h = VmHandle {
                name: n,
                runtime_id: None,
            };
            list.extend(runtime.snapshots(&h).await.map_err(|e| format!("{e:#}"))?);
        }
        list.sort_by_key(|s| s.at_ms);
        list.dedup_by(|a, b| a.id == b.id);
        Ok(list)
    }

    /// Take `auto-<step>-<ts>` (disk) and prune auto-snapshots past `snapshots_keep`.
    async fn auto_snapshot(&mut self, name: &str, step: &str) -> Result<(), String> {
        let runtime = self.vm()?;
        let rec = self.store.get(name)?;
        let h = rec.vm_handle()?;
        if matches!(runtime.status(&h).await, Ok(VmStatus::Gone) | Err(_)) {
            return Ok(());
        }
        let label = format!("auto-{step}-{}", record::now_ms());
        let out = self.progress(name, None);
        let id = runtime
            .snapshot(&h, &label, false, &out)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let mut list = self.all_snapshots(name).await?;
        list.sort_by_key(|s| s.at_ms);
        let auto: Vec<SnapshotId> = list
            .iter()
            .filter(|s| s.auto)
            .map(|s| s.id.clone())
            .collect();
        if auto.len() > self.cfg.snapshots_keep {
            for old in &auto[..auto.len() - self.cfg.snapshots_keep] {
                if let Err(e) = runtime.delete_snapshot(old).await {
                    tracing::warn!("prune snapshot {}: {e:#}", old.0);
                }
                list.retain(|s| &s.id != old);
            }
        }
        tracing::debug!("auto snapshot {}", id.0);
        self.store.update(name, |r| {
            r.vm_mut()?.snapshots = list;
            Ok(())
        })?;
        Ok(())
    }

    /// Rewrite the guest's `/data/config.yaml` (shares → `mesh.workspaces`) on a running VM.
    async fn push_guest_config(&self, rec: &Computer) -> Result<(), String> {
        let runtime = self.vm()?;
        let h = rec.vm_handle()?;
        let text = bringup::guest_config(&rec.name, &self.host_side(), &rec.vm()?.shares);
        runtime
            .put(&h, bringup::GUEST_CONFIG, bytes::Bytes::from(text), 0o644)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Sessions with a running turn on `engine` that this engine can see.
    async fn busy_sessions(&self, engine: &str) -> usize {
        if engine.is_empty() {
            return 0;
        }
        crate::takeover::running_sessions(engine).await.len()
    }

    /// Build the record for a new VM and start its bring-up.
    async fn create(
        &mut self,
        spec: SandboxCreate,
        origin: Option<(runtime::SessionId, String)>,
        me: ActorRef<Self>,
    ) -> Result<SandboxInfo, String> {
        let name = spec.name.clone();
        self.vm()?;
        if !valid_name(&name) {
            return Err(format!(
                "name {name:?}: use 1–63 characters of a–z, 0–9 and -"
            ));
        }
        if self.store.get(&name).is_ok() {
            return Err(format!("a sandbox named {name} already exists"));
        }
        match self.join.status().await {
            Ok(s) if s.peers.iter().any(|p| p.name == name) || s.self_name == name => {
                return Err(format!("an engine named {name} is already in the mesh"));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("sandbox create: mesh status unavailable ({e:#}); name not checked")
            }
        }
        let vms = self
            .store
            .load_all()
            .iter()
            .filter(|r| r.vm.is_some())
            .count();
        if vms >= self.cfg.vm.max_vms {
            return Err(format!(
                "{vms} sandbox VMs already exist; sandbox.vm.max_vms is {}",
                self.cfg.vm.max_vms
            ));
        }
        let pf = self.preflight().await;
        if !pf.blockers.is_empty() {
            return Err(blockers_text(&pf.blockers));
        }
        let d = &self.cfg.vm.defaults;
        let base = pf.defaults;
        let resources = Resources {
            cpus: spec.cpus.unwrap_or(base.cpus).max(1),
            max_cpus: spec.max_cpus.unwrap_or(base.max_cpus),
            memory_mib: spec.memory_mib.unwrap_or(base.memory_mib),
            max_memory_mib: spec.max_memory_mib.unwrap_or(base.max_memory_mib),
            root_disk_mib: spec.root_disk_mib.unwrap_or(base.root_disk_mib),
        };
        let resources = Resources {
            max_cpus: resources.max_cpus.max(resources.cpus),
            max_memory_mib: resources.max_memory_mib.max(resources.memory_mib),
            ..resources
        };
        check_ceilings(&pf.host, resources.max_cpus, resources.max_memory_mib)?;
        let shares = validate_shares(&spec.shares, &self.rt.data_dir, &self.cfg.vm.home)?;
        let image = spec.image.clone().unwrap_or_else(|| ImageRef::Oci {
            r#ref: format!("{}:{}", self.cfg.vm.image, env!("CARGO_PKG_VERSION")),
        });
        // A reused name may still own snapshot groups from a destroyed record that kept them:
        // its `auto-*` ones are worthless now (they belong to a disk that is gone); named ones
        // stay so the user can still restore them.
        if let Ok(runtime) = self.vm() {
            for s in self.all_snapshots(&name).await.unwrap_or_default() {
                if s.auto
                    && let Err(e) = runtime.delete_snapshot(&s.id).await
                {
                    tracing::warn!("create {name}: stale snapshot {}: {e:#}", s.id.0);
                }
            }
        }
        let desktop = spec.desktop.unwrap_or(d.desktop);
        if desktop {
            self.secrets.set(&vnc_row(&name), &vnc_password()).await?;
        }
        let now = record::now_ms();
        let rec = Computer {
            name: name.clone(),
            backend: "vm".into(),
            handle: crate::Handle { id: name.clone() },
            engine: None,
            created_ms: now,
            vm: Some(VmRecord {
                runtime: self.vm().map(|r| r.kind().to_owned())?,
                image,
                resources,
                shares,
                desktop: DesktopRecord {
                    enabled: desktop,
                    geometry: spec.geometry.clone().unwrap_or_else(|| d.geometry.clone()),
                    host_port: None,
                },
                mesh_port: MeshPort {
                    guest: bringup::MESH_GUEST_PORT,
                    host_port: None,
                },
                network: spec.network.unwrap_or(d.network),
                autostart: spec.autostart.unwrap_or(true),
                state: SandboxState::Creating {
                    step: "create".into(),
                },
                pending: PendingView::default(),
                setup: Setup {
                    step: "create".into(),
                    at_ms: now,
                    error: None,
                },
                snapshots: Vec::new(),
                runtime_name: name.clone(),
                local_t: spec.local_t.clone(),
                purpose: spec.purpose.clone(),
                updated_ms: now,
            }),
        };
        self.store.save(&rec)?;
        self.spawn_bringup(&name, origin, me)?;
        let info = self.info(&rec).await;
        self.rt
            .publish(SandboxEvent::Changed {
                sandbox: info.clone(),
            })
            .await;
        Ok(info)
    }

    async fn start(&mut self, name: &str) -> Result<SandboxInfo, String> {
        let runtime = self.vm()?;
        let rec = self.store.get(name)?;
        let vm = rec.vm()?;
        if matches!(
            vm.state,
            SandboxState::Creating { .. } | SandboxState::Failed { .. }
        ) {
            return Err(format!(
                "{name} is not brought up yet ({}); use retry",
                state_str(&vm.state)
            ));
        }
        let h = rec.vm_handle()?;
        if vm.pending.shares || vm.pending.ports {
            let host = self.host_side();
            let spec = bringup::vm_spec(&rec, &host, &h.name).map_err(|e| format!("{e:#}"))?;
            runtime
                .set_mounts(&h, &spec.mounts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            runtime
                .set_ports(&h, &spec.ports)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        let (_, r) = self.store.update(name, |r| {
            r.set_state(SandboxState::Starting);
            Ok(())
        })?;
        self.publish_changed(&r).await;
        let out = self.progress(name, None);
        runtime
            .start(&h, &out)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let (_, r) = self.store.update(name, |r| {
            r.vm_mut()?.pending = PendingView::default();
            r.set_state(SandboxState::Running);
            Ok(())
        })?;
        self.publish_changed(&r).await;
        let rec = self.refresh(name).await?;
        Ok(self.info(&rec).await)
    }
}

/// `blockers`, one sentence per line, `fix` appended when there is one.
pub fn blockers_text(blockers: &[Blocker]) -> String {
    blockers
        .iter()
        .map(|b| match &b.fix {
            Some(f) => format!("{} — {f}", b.message),
            None => b.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runtime status → record state.
fn state_of(s: VmStatus) -> SandboxState {
    match s {
        VmStatus::Running => SandboxState::Running,
        VmStatus::Stopped => SandboxState::Stopped,
        VmStatus::Paused => SandboxState::Paused,
        VmStatus::Crashed => SandboxState::Crashed,
        VmStatus::Gone => SandboxState::Gone,
    }
}

/// The state's tag, for messages.
pub fn state_str(s: &SandboxState) -> String {
    match s {
        SandboxState::Creating { step } => format!("creating ({step})"),
        SandboxState::Failed { step, error } => format!("failed at {step}: {error}"),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
            .unwrap_or_default(),
    }
}

/// Ceilings the hypervisor cannot honour: libkrun reserves `max_memory` of
/// guest-physical memory at boot and HVF refuses a ceiling equal to the host's
/// RAM (`SetUserMemoryRegion(MemoryMap)` at 64 GiB on a 64 GiB Mac, smoke
/// 2026-09-30), leaving a VM that cannot start; more vCPUs than cores likewise.
fn check_ceilings(host: &HostFacts, max_cpus: u8, max_memory_mib: u32) -> Result<(), String> {
    let host_mib = (host.mem_bytes >> 20) as u32;
    if max_memory_mib >= host_mib {
        return Err(format!(
            "max_memory {max_memory_mib} MiB exceeds this host's {host_mib} MiB"
        ));
    }
    if u32::from(max_cpus) > host.cores {
        return Err(format!(
            "max_cpus {max_cpus} exceeds this host's {} cores",
            host.cores
        ));
    }
    Ok(())
}

/// Validate share host paths (79 §5.6, S1 finding 7: canonicalised) and fill
/// default guest paths `/shared/<basename>` with a numeric suffix on collision.
pub fn validate_shares(
    shares: &[Mount],
    data_dir: &Path,
    msb_home: &str,
) -> Result<Vec<Mount>, String> {
    let home = dirs::home_dir().unwrap_or_default();
    let msb = if msb_home.is_empty() {
        home.join(".microsandbox")
    } else {
        PathBuf::from(msb_home)
    };
    let mut out: Vec<Mount> = Vec::new();
    for m in shares {
        let raw = m
            .host
            .to_str()
            .ok_or_else(|| format!("{}: not UTF-8", m.host.display()))?;
        if raw.is_empty() || !m.host.is_absolute() {
            return Err(format!("{raw}: share paths must be absolute"));
        }
        if raw.contains([',', ':', ';']) {
            return Err(format!("{raw}: share paths may not contain , : or ;"));
        }
        let host = std::fs::canonicalize(&m.host).map_err(|e| format!("{raw}: {e}"))?;
        if !host.is_dir() {
            return Err(format!("{raw}: not a directory"));
        }
        if host == Path::new("/") {
            return Err("/ cannot be shared".into());
        }
        let data = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        if host == data {
            return Err(format!("{raw}: the data dir cannot be shared"));
        }
        if host == std::fs::canonicalize(&msb).unwrap_or(msb.clone()) {
            return Err(format!("{raw}: the runtime home cannot be shared"));
        }
        let mut guest = if m.guest.is_empty() {
            format!(
                "/shared/{}",
                host.file_name().and_then(|f| f.to_str()).unwrap_or("dir")
            )
        } else {
            m.guest.clone()
        };
        if !guest.starts_with('/') {
            return Err(format!("{guest}: guest paths must be absolute"));
        }
        let base = guest.clone();
        let mut n = 2;
        while out.iter().any(|o| o.guest == guest) {
            guest = format!("{base}-{n}");
            n += 1;
        }
        out.push(Mount {
            host,
            guest,
            readonly: m.readonly,
        });
    }
    Ok(out)
}

/// The record's alternate runtime name (79 §5.6 restore: `lab` ↔ `lab-r`; the design's `~` is
/// rejected by the SDK's name validator, S2 as-built).
fn alternate(runtime_name: &str, record_name: &str) -> String {
    if runtime_name == record_name {
        format!("{record_name}-r")
    } else {
        record_name.to_owned()
    }
}

/// `tell`: a record changed (from a bring-up task) → `Changed` event.
#[derive(Clone, Debug)]
pub struct Changed(pub Computer);

impl Message<Changed> for SandboxActor {
    type Reply = ();
    async fn handle(&mut self, m: Changed, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.publish_changed(&m.0).await;
    }
}

impl Message<BringupDone> for SandboxActor {
    type Reply = ();
    async fn handle(&mut self, m: BringupDone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.inflight.remove(&m.0);
    }
}

impl Message<Reconcile> for SandboxActor {
    type Reply = ();
    async fn handle(&mut self, m: Reconcile, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        self.reconcile(m.autostart, me).await;
    }
}

impl Message<MakeEngine> for SandboxActor {
    type Reply = Result<mesh::Peer, String>;

    // ponytail: bring-up runs inside the handler (minutes) so one engine is
    // made at a time; the VM path (`Create`) is the spawned one.
    async fn handle(
        &mut self,
        MakeEngine(target): MakeEngine,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let rt = self.rt.clone();
        make_engine(
            &self.rt.data_dir,
            target,
            move |name| async move {
                let vault = secrets::handle(&rt)
                    .await
                    .ok_or("secrets vault unavailable")?;
                vault
                    .get(&name, 0)
                    .ok_or_else(|| format!("no secret named {name}"))
            },
            &*self.join,
            &Progress::log(),
            crate::config::step_timeout(&self.cfg),
        )
        .await
        .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Engines> for SandboxActor {
    type Reply = Result<Vec<Computer>, String>;

    async fn handle(&mut self, _: Engines, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self.store.load_all())
    }
}

impl Message<Create> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Create, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        self.create(m.spec, m.origin, me).await
    }
}

impl Message<Retry> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Retry, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.store.get(&m.name)?;
        let SandboxState::Failed { step, .. } = rec.state() else {
            return Err(format!(
                "{} is not failed ({})",
                m.name,
                state_str(&rec.state())
            ));
        };
        if step != "create"
            && let Err(e) = self.auto_snapshot(&m.name, "retry").await
        {
            tracing::warn!("retry {}: auto snapshot: {e}", m.name);
        }
        // Mark first: the bring-up task reads the step it resumes from off the record.
        let (_, rec) = self.store.update(&m.name, |r| {
            r.set_state(SandboxState::Creating { step: step.clone() });
            Ok(())
        })?;
        let me = ctx.actor_ref().clone();
        self.spawn_bringup(&m.name, None, me)?;
        Ok(self.info(&rec).await)
    }
}

impl Message<Start> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Start, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.start(&m.name).await
    }
}

impl Message<Stop> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Stop, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        let h = rec.vm_handle()?;
        if !m.force {
            let engine = rec.engine.as_ref().map(|e| e.0.clone()).unwrap_or_default();
            let busy = self.busy_sessions(&engine).await;
            if busy > 0 {
                return Err(format!(
                    "{} has {busy} session(s) with a running turn; stop with force to interrupt them",
                    m.name
                ));
            }
        }
        runtime
            .stop(&h, Duration::from_secs(m.grace_secs))
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.metrics.remove(&m.name);
        let (_, rec) = self.store.update(&m.name, |r| {
            r.set_state(SandboxState::Stopped);
            Ok(())
        })?;
        self.publish_changed(&rec).await;
        Ok(self.info(&rec).await)
    }
}

impl Message<Pause> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Pause, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        runtime
            .pause(&rec.vm_handle()?)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let rec = self.refresh(&m.name).await?;
        Ok(self.info(&rec).await)
    }
}

impl Message<Resume> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Resume, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        runtime
            .resume(&rec.vm_handle()?)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let rec = self.refresh(&m.name).await?;
        Ok(self.info(&rec).await)
    }
}

impl Message<Resize> for SandboxActor {
    type Reply = Result<ResizeOutcome, String>;

    async fn handle(&mut self, m: Resize, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        let vm = rec.vm()?;
        let c = m.change;
        if c.cpus.is_some_and(|n| n < 1) {
            return Err("cpus must be at least 1".into());
        }
        if let Some(mem) = c.memory_mib {
            let floor_mib = self
                .metrics
                .get(&m.name)
                .map_or(0, |s| s.memory_bytes / (1024 * 1024) + 512);
            if u64::from(mem) < floor_mib {
                return Err(format!(
                    "memory {mem} MiB is below the guest's use + 512 MiB ({floor_mib} MiB)"
                ));
            }
        }
        if c.root_disk_mib
            .is_some_and(|d| d < vm.resources.root_disk_mib)
        {
            return Err(format!(
                "the root disk only grows ({} MiB now)",
                vm.resources.root_disk_mib
            ));
        }
        if c.max_cpus.is_some() || c.max_memory_mib.is_some() {
            let host = self.preflight().await.host;
            check_ceilings(
                &host,
                c.max_cpus.unwrap_or(vm.resources.max_cpus),
                c.max_memory_mib.unwrap_or(vm.resources.max_memory_mib),
            )?;
        }
        let h = rec.vm_handle()?;
        let outcome = runtime.resize(&h, c).await.map_err(|e| format!("{e:#}"))?;
        // Let the guest converge (S1 finding 1: ≤ 2 s); poll metrics, bounded by `resize_settle_secs`.
        let settle = Duration::from_secs(self.cfg.vm.resize_settle_secs);
        let deadline = Instant::now() + settle;
        while Instant::now() < deadline {
            if let Ok(met) = runtime.metrics(&h).await {
                self.metrics.insert(m.name.clone(), met);
                if outcome
                    .applied
                    .memory_mib
                    .is_none_or(|mib| met.memory_limit_bytes >= u64::from(mib) << 20)
                {
                    break;
                }
            }
            tokio::time::sleep(settle.min(Duration::from_secs(2))).await;
        }
        let (_, rec) = self.store.update(&m.name, |r| {
            let v = r.vm_mut()?;
            let a = outcome.applied;
            let n = outcome.next_start;
            v.resources = Resources {
                cpus: a.cpus.or(n.cpus).unwrap_or(v.resources.cpus),
                max_cpus: a.max_cpus.or(n.max_cpus).unwrap_or(v.resources.max_cpus),
                memory_mib: a
                    .memory_mib
                    .or(n.memory_mib)
                    .unwrap_or(v.resources.memory_mib),
                max_memory_mib: a
                    .max_memory_mib
                    .or(n.max_memory_mib)
                    .unwrap_or(v.resources.max_memory_mib),
                root_disk_mib: a
                    .root_disk_mib
                    .or(n.root_disk_mib)
                    .unwrap_or(v.resources.root_disk_mib),
            };
            v.pending.resources = outcome.restart_required;
            v.updated_ms = record::now_ms();
            Ok(())
        })?;
        self.publish_changed(&rec).await;
        Ok(outcome)
    }
}

impl Message<SetShares> for SandboxActor {
    type Reply = Result<Pending, String>;

    async fn handle(&mut self, m: SetShares, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let shares = validate_shares(&m.shares, &self.rt.data_dir, &self.cfg.vm.home)?;
        let rec = self.store.get(&m.name)?;
        let h = rec.vm_handle()?;
        let status = runtime.status(&h).await.map_err(|e| format!("{e:#}"))?;
        let pending = runtime
            .set_mounts(&h, &shares)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let (_, rec) = self.store.update(&m.name, |r| {
            let v = r.vm_mut()?;
            v.shares = shares.clone();
            v.pending.shares = true;
            v.updated_ms = record::now_ms();
            Ok(())
        })?;
        if status == VmStatus::Running
            && let Err(e) = self.push_guest_config(&rec).await
        {
            tracing::warn!("{}: guest config not rewritten: {e}", m.name);
        }
        self.publish_changed(&rec).await;
        Ok(Pending {
            restart_required: pending.restart_required || status == VmStatus::Running,
        })
    }
}

impl Message<Snapshot> for SandboxActor {
    type Reply = Result<VmSnapshotInfo, String>;

    async fn handle(&mut self, m: Snapshot, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        if m.full {
            return Err("full (memory) snapshots are not available for a systemd guest in v1; take a disk snapshot (full: false)".into());
        }
        if m.label.is_empty()
            || m.label.starts_with("auto-")
            || !m
                .label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return Err("label: use letters, digits, - _ . and not the auto- prefix".into());
        }
        let rec = self.store.get(&m.name)?;
        let h = rec.vm_handle()?;
        let out = self.progress(&m.name, None);
        let id = runtime
            .snapshot(&h, &m.label, false, &out)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let list = self.all_snapshots(&m.name).await?;
        let info = list
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .unwrap_or(VmSnapshotInfo {
                id: id.clone(),
                label: m.label.clone(),
                full: false,
                at_ms: record::now_ms(),
                bytes: None,
                auto: false,
            });
        let (_, rec) = self.store.update(&m.name, |r| {
            r.vm_mut()?.snapshots = list;
            Ok(())
        })?;
        self.publish_changed(&rec).await;
        Ok(info)
    }
}

impl Message<Restore> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: Restore, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        let old = rec.vm_handle()?;
        let status = runtime.status(&old).await.map_err(|e| format!("{e:#}"))?;
        if status == VmStatus::Running || status == VmStatus::Paused {
            runtime
                .stop(&old, Duration::from_secs(20))
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        if let Err(e) = self.auto_snapshot(&m.name, "restore").await {
            tracing::warn!("restore {}: auto snapshot: {e}", m.name);
        }
        let rec = self.store.get(&m.name)?;
        let alt = alternate(&rec.vm()?.runtime_name, &rec.name);
        let spec = bringup::vm_spec(&rec, &self.host_side(), &alt).map_err(|e| format!("{e:#}"))?;
        let out = self.progress(&m.name, None);
        runtime
            .restore(&m.id, &alt, &spec, &out)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Err(e) = runtime.destroy(&old).await {
            tracing::warn!(
                "restore {}: old vm {} not destroyed: {e:#}",
                m.name,
                old.name
            );
        }
        self.store.update(&m.name, |r| {
            let v = r.vm_mut()?;
            v.runtime_name = alt.clone();
            v.pending = PendingView::default();
            Ok(())
        })?;
        // Snapshots of the old runtime name stay listed under their ids (bug 6).
        if let Ok(list) = self.all_snapshots(&m.name).await {
            let _ = self.store.update(&m.name, |r| {
                r.vm_mut()?.snapshots = list;
                Ok(())
            });
        }
        let rec = self.refresh(&m.name).await?;
        Ok(self.info(&rec).await)
    }
}

impl Message<Destroy> for SandboxActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Destroy, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.store.get(&m.name)?;
        if rec.vm.is_none() {
            return Err(format!(
                "{} is a {} engine; remove it with `t mesh remove`",
                m.name, rec.backend
            ));
        }
        if self.inflight.contains(&m.name) {
            return Err(format!(
                "{}: bring-up in progress; wait for it to finish or fail",
                m.name
            ));
        }
        let (_, rec) = self.store.update(&m.name, |r| {
            r.set_state(SandboxState::Destroying);
            Ok(())
        })?;
        self.publish_changed(&rec).await;
        if let Some(id) = &rec.engine
            && let Err(e) = self.join.remove_peer(id.clone()).await
        {
            tracing::warn!("destroy {}: mesh remove: {e:#}", m.name);
        }
        if let Ok(runtime) = self.vm() {
            // Sweep both runtime names regardless of the record's state (smoke bug 4: a `gone`
            // record left its stopped msb row and snapshot groups behind).
            let handles = [m.name.clone(), format!("{}-r", m.name)].map(|n| VmHandle {
                name: n,
                runtime_id: None,
            });
            if m.keep_snapshots
                && runtime
                    .status(&rec.vm_handle()?)
                    .await
                    .is_ok_and(|s| s != VmStatus::Gone)
                && let Err(e) = self.auto_snapshot(&m.name, "destroy").await
            {
                tracing::warn!("destroy {}: auto snapshot: {e}", m.name);
            }
            for h in &handles {
                if runtime.status(h).await.is_ok_and(|s| s != VmStatus::Gone) {
                    let _ = runtime.stop(h, Duration::from_secs(20)).await;
                    runtime.destroy(h).await.map_err(|e| format!("{e:#}"))?;
                }
            }
            if !m.keep_snapshots {
                for s in self.all_snapshots(&m.name).await.unwrap_or_default() {
                    if let Err(e) = runtime.delete_snapshot(&s.id).await {
                        tracing::warn!("destroy {}: snapshot {}: {e:#}", m.name, s.id.0);
                    }
                }
            }
        }
        if rec.vm()?.desktop.enabled
            && let Err(e) = self.secrets.delete(&vnc_row(&m.name)).await
        {
            tracing::debug!("destroy {}: vault row: {e}", m.name);
        }
        self.metrics.remove(&m.name);
        self.store.remove(&m.name)?;
        self.rt
            .publish(SandboxEvent::Removed { name: m.name })
            .await;
        Ok(())
    }
}

impl Message<DesktopAccess> for SandboxActor {
    type Reply = Result<types::DesktopAccess, String>;

    async fn handle(
        &mut self,
        m: DesktopAccess,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let rec = self.store.get(&m.name)?;
        let vm = rec.vm()?;
        if !vm.desktop.enabled {
            return Err(format!("{} has no desktop", m.name));
        }
        let port = vm
            .desktop
            .host_port
            .ok_or_else(|| format!("{}: desktop port not published yet", m.name))?;
        let password = self
            .secrets
            .get(&vnc_row(&m.name))
            .await
            .ok_or_else(|| format!("{}: vnc password missing from the vault", m.name))?;
        Ok(types::DesktopAccess {
            ws_url: format!("ws://127.0.0.1:{port}/websockify"),
            http_url: format!("http://127.0.0.1:{port}/vnc.html"),
            password: password.as_str().to_owned(),
            geometry: vm.desktop.geometry.clone(),
            host_engine: self.host_engine().await,
        })
    }
}

impl Message<PreflightNow> for SandboxActor {
    type Reply = Preflight;

    async fn handle(&mut self, _: PreflightNow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.preflight().await
    }
}

impl Message<InstallRuntime> for SandboxActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        _: InstallRuntime,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let runtime = self.vm()?;
        let out = self.progress("runtime", None);
        runtime.install(&out).await.map_err(|e| format!("{e:#}"))?;
        self.preflight = None;
        Ok(())
    }
}

impl Message<List> for SandboxActor {
    type Reply = Vec<SandboxInfo>;

    async fn handle(&mut self, _: List, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let host = self.host_engine().await;
        self.store
            .load_all()
            .iter()
            .map(|r| r.info(&host, self.metrics.get(&r.name).copied()))
            .collect()
    }
}

impl Message<Metrics> for SandboxActor {
    type Reply = Result<VmMetrics, String>;

    async fn handle(&mut self, m: Metrics, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        let met = runtime
            .metrics(&rec.vm_handle()?)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.metrics.insert(m.name, met);
        Ok(met)
    }
}

impl Message<Logs> for SandboxActor {
    type Reply = Result<Vec<LogLine>, String>;

    async fn handle(&mut self, m: Logs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let runtime = self.vm()?;
        let rec = self.store.get(&m.name)?;
        runtime
            .logs(&rec.vm_handle()?, m.tail)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Takeover> for SandboxActor {
    type Reply = Result<u32, String>;

    async fn handle(&mut self, m: Takeover, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.store.get(&m.name)?;
        let engine = rec
            .engine
            .as_ref()
            .map(|e| e.0.clone())
            .ok_or_else(|| format!("{} has no engine yet", m.name))?;
        Ok(crate::takeover::steer(&self.rt, &m.name, &engine, m.on).await)
    }
}

impl Message<Terminal> for SandboxActor {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Terminal, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.store.get(&m.name)?;
        let vm = rec.vm()?;
        if !matches!(vm.state, SandboxState::Ready | SandboxState::Running) {
            return Err(format!(
                "{} is not running ({})",
                m.name,
                state_str(&vm.state)
            ));
        }
        let msb = crate::vm::install::msb_path(&self.cfg.vm.home)
            .ok_or("the microsandbox runtime is not installed")?;
        let command = format!(
            "{} exec --tty {} -- sh -c 'if command -v runuser >/dev/null 2>&1; then exec runuser -u engine -- bash -l; else exec bash -l; fi'",
            sq(&msb.display().to_string()),
            sq(&vm.runtime_name)
        );
        let jobs = ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)
            .ok()
            .flatten()
            .ok_or("jobs actor not running")?;
        let out = jobs
            .ask(tools::pty::StartPty {
                session: m.session,
                call_id: format!("sandbox_terminal:{}", m.name),
                args: tools::pty::StartArgs {
                    command,
                    name: Some(format!("sandbox {}", m.name)),
                    cwd: None,
                    env: None,
                    ready: None,
                    idle_secs: Some(0),
                },
                cwd: self.rt.data_dir.clone(),
                ready_timeout: Duration::from_secs(30),
                idle_after: None,
                root: m.root,
                activity: None,
                pty_shell: None,
            })
            .await
            .map_err(|e| format!("jobs actor: {e}"))?;
        if out.is_error {
            return Err(out.content);
        }
        out.structured
            .as_ref()
            .and_then(|s| s.get("job"))
            .and_then(|j| j.as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("the terminal exited at once:\n{}", out.content))
    }
}

impl Message<EnsureVm> for SandboxActor {
    type Reply = Result<SandboxInfo, String>;

    async fn handle(&mut self, m: EnsureVm, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match self.store.get(&m.name) {
            Ok(rec) => {
                let rec = match rec.state() {
                    SandboxState::Stopped | SandboxState::Crashed => {
                        return self.start(&m.name).await;
                    }
                    SandboxState::Ready
                    | SandboxState::Running
                    | SandboxState::Paused
                    | SandboxState::Starting => self.refresh(&m.name).await.unwrap_or(rec),
                    _ => rec,
                };
                Ok(self.info(&rec).await)
            }
            Err(_) => {
                let me = ctx.actor_ref().clone();
                let spec = SandboxCreate {
                    name: m.name,
                    purpose: m.agent.map(|a| format!("agent {a}")),
                    cpus: None,
                    memory_mib: None,
                    root_disk_mib: None,
                    max_cpus: None,
                    max_memory_mib: None,
                    shares: Vec::new(),
                    desktop: None,
                    geometry: None,
                    network: None,
                    image: None,
                    autostart: None,
                    local_t: None,
                };
                self.create(spec, None, me).await
            }
        }
    }
}

/// Every `health_interval_secs`, `tell` the actor to reconcile.
pub fn spawn_health(actor: ActorRef<SandboxActor>, every: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every.max(Duration::from_secs(5)));
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = actor.tell(Reconcile { autostart: false }).await {
                // A panic inside a handler stops the actor; every later sandbox_* call then
                // fails with "actor not running". Say so once instead of dying quietly.
                tracing::error!(
                    "sandbox actor is gone ({e}); health tick stopped — restart the engine to recover"
                );
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kameo::actor::Spawn;
    use parking_lot::Mutex;

    use super::*;
    use crate::ExecOutput;
    use crate::vm::mock::MockVmRuntime;

    /// In-memory vault.
    #[derive(Default)]
    struct MemSecrets(Mutex<HashMap<String, String>>);

    #[async_trait::async_trait]
    impl SecretStore for MemSecrets {
        async fn set(&self, name: &str, value: &str) -> Result<(), String> {
            self.0.lock().insert(name.into(), value.into());
            Ok(())
        }
        async fn get(&self, name: &str) -> Option<Zeroizing<String>> {
            self.0.lock().get(name).map(|v| Zeroizing::new(v.clone()))
        }
        async fn delete(&self, name: &str) -> Result<(), String> {
            self.0
                .lock()
                .remove(name)
                .map(|_| ())
                .ok_or_else(|| format!("unknown {name}"))
        }
    }

    /// Mesh double: every peer added is reachable at once; remembers pairings and removals.
    #[derive(Default)]
    struct Join {
        added: Mutex<Vec<String>>,
        removed: Mutex<Vec<String>>,
        peers: Mutex<Vec<mesh::Peer>>,
    }

    #[async_trait::async_trait]
    impl MeshJoin for Join {
        async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer> {
            let n = self.added.lock().len();
            self.added.lock().push(pairing);
            let p = mesh::Peer {
                id: mesh::EngineId(format!("vm-engine-{n}")),
                name: "lab".into(),
                addrs: vec![],
                reachable: true,
                last_seen_ms: 0,
                hub_pref: mesh::HubPref::Never,
            };
            self.peers.lock().push(p.clone());
            Ok(p)
        }
        async fn status(&self) -> anyhow::Result<mesh::MeshStatus> {
            Ok(mesh::MeshStatus {
                self_id: mesh::EngineId("host".into()),
                self_name: "mac".into(),
                mesh_name: None,
                peers: self.peers.lock().clone(),
                lease: mesh::LeaseState {
                    holder: None,
                    since_ms: 0,
                    reason: String::new(),
                },
                traffic: Vec::new(),
                replica: None,
            })
        }
        async fn remove_peer(&self, id: mesh::EngineId) -> anyhow::Result<()> {
            self.peers.lock().retain(|p| p.id != id);
            self.removed.lock().push(id.0);
            Ok(())
        }
    }

    /// Collects every `SandboxEvent`.
    #[derive(kameo::Actor)]
    struct Collector(Arc<Mutex<Vec<SandboxEvent>>>);

    impl Message<SandboxEvent> for Collector {
        type Reply = ();
        async fn handle(&mut self, ev: SandboxEvent, _: &mut Context<Self, Self::Reply>) {
            self.0.lock().push(ev);
        }
    }

    struct Rig {
        actor: ActorRef<SandboxActor>,
        mock: Arc<MockVmRuntime>,
        join: Arc<Join>,
        secrets: Arc<MemSecrets>,
        events: Arc<Mutex<Vec<SandboxEvent>>>,
        dir: PathBuf,
    }

    /// The handler's `Err(String)`.
    fn err<M: std::fmt::Debug>(e: kameo::error::SendError<M, String>) -> String {
        match e {
            kameo::error::SendError::HandlerError(s) => s,
            other => panic!("not a handler error: {other}"),
        }
    }

    fn ok(stdout: &str) -> ExecOutput {
        ExecOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn pairing(addr: &str) -> String {
        use data_encoding::BASE32_NOPAD;
        let v = serde_json::json!({
            "id": "0000000000000000000000000000000000000000000000000000000000000000",
            "name": "lab", "relay": "", "addrs": [{ "Ip": addr }], "token": "T"
        });
        format!(
            "1.{}",
            BASE32_NOPAD.encode(&serde_json::to_vec(&v).unwrap())
        )
    }

    async fn rig(tag: &str, mock: MockVmRuntime, mut cfg: SandboxConfig) -> Rig {
        let dir = std::env::temp_dir().join(format!("sandbox-actor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        cfg.vm.resize_settle_secs = 0;
        cfg.step_timeout_secs = 2;
        cfg.vm.loopback_mesh = true; // the tests assert the loopback pairing path; the shipped default is off (S1 #4b)
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        mock.canned("uname -sm", ok("Linux aarch64"));
        mock.canned("runuser -u engine -- env THEUTH_DATA_DIR=/data PATH=\"/usr/local/bin:$PATH\" t --version", ok("t 9.9"));
        mock.canned(
            "runuser -u engine -- env THEUTH_DATA_DIR=/data PATH=\"/usr/local/bin:$PATH\" t mesh pair",
            ok(&format!("\"{}\"", pairing("172.16.0.2:7800"))),
        );
        let mock = Arc::new(mock);
        let join = Arc::new(Join::default());
        let secrets = Arc::new(MemSecrets::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let col =
            Collector::spawn_with_mailbox(Collector(events.clone()), runtime::subscriber_mailbox());
        rt.subscribe::<Collector, SandboxEvent>(&col).await;
        let actor = SandboxActor::spawn(SandboxActor::with(
            rt,
            cfg,
            Ok(mock.clone()),
            join.clone(),
            secrets.clone(),
        ));
        Rig {
            actor,
            mock,
            join,
            secrets,
            events,
            dir,
        }
    }

    fn spec(name: &str) -> SandboxCreate {
        SandboxCreate {
            name: name.into(),
            purpose: None,
            cpus: None,
            memory_mib: None,
            root_disk_mib: None,
            max_cpus: None,
            max_memory_mib: None,
            shares: Vec::new(),
            desktop: Some(false),
            geometry: None,
            network: None,
            image: None,
            autostart: None,
            local_t: None,
        }
    }

    async fn wait_state(r: &Rig, name: &str, pred: impl Fn(&SandboxState) -> bool) -> SandboxInfo {
        for _ in 0..200 {
            let list = r.actor.ask(List).await.unwrap();
            if let Some(i) = list.iter().find(|i| i.name == name)
                && pred(&i.state)
            {
                return i.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "{name} never reached the state; last: {:?}",
            r.actor.ask(List).await.unwrap()
        );
    }

    #[tokio::test]
    async fn create_streams_progress_and_ends_ready() {
        let r = rig("create", MockVmRuntime::new(), SandboxConfig::default()).await;
        let origin = Some((runtime::SessionId::new(), "call-1".to_owned()));
        let first = r
            .actor
            .ask(Create {
                spec: spec("lab"),
                origin: origin.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            first.state,
            SandboxState::Creating {
                step: "create".into()
            }
        );
        let done = wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        assert_eq!(done.engine.as_deref(), Some("vm-engine-0"));
        let vm = done.vm.unwrap();
        assert_eq!(vm.host_engine, "host");
        assert!(vm.mesh_port.is_some(), "loopback udp port picked");
        assert!(vm.desktop.is_none());
        // Every step in order, then ready.
        let calls = r.mock.calls();
        let idx = |p: &str| {
            calls
                .iter()
                .position(|c| c.starts_with(p))
                .unwrap_or_else(|| panic!("{p} in {calls:?}"))
        };
        assert!(
            idx("create lab") < idx("exec lab: uname")
                && idx("exec lab: uname") < idx("exec lab: runuser")
        );
        // The pairing string handed to the mesh points at the host loopback port.
        let added = r.join.added.lock().clone();
        assert_eq!(added.len(), 1);
        let body = added[0].strip_prefix("1.").unwrap();
        let p: serde_json::Value =
            serde_json::from_slice(&data_encoding::BASE32_NOPAD.decode(body.as_bytes()).unwrap())
                .unwrap();
        assert_eq!(
            p["addrs"],
            serde_json::json!([{ "Ip": format!("127.0.0.1:{}", vm.mesh_port.unwrap()) }])
        );
        assert_eq!(p["name"], "lab");
        // Progress events carry the origin; the record file has the guest config bits.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let evs = r.events.lock().clone();
        let progress: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                SandboxEvent::Progress { step, call_id, .. } => {
                    Some((step.clone(), call_id.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            progress
                .iter()
                .any(|(s, c)| s == "pair" && c.as_deref() == Some("call-1")),
            "{progress:?}"
        );
        assert!(evs.iter().any(|e| matches!(e, SandboxEvent::Changed { sandbox } if sandbox.state == SandboxState::Ready)));
        let vmm = r.mock.vm("lab").unwrap();
        assert!(vmm.spec.files.iter().any(|f| f.path == "/data/config.yaml"
            && String::from_utf8_lossy(&f.bytes).contains("class: vm")));
        assert_eq!(
            vmm.spec.labels.get("io.theuth.sandbox").map(String::as_str),
            Some("lab")
        );
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn retry_resumes_from_the_failed_step() {
        let mock = MockVmRuntime::new();
        mock.canned(
            "runuser -u engine -- env THEUTH_DATA_DIR=/data PATH=\"/usr/local/bin:$PATH\" t --secret-list",
            ExecOutput { status: 1, stdout: vec![], stderr: b"boom\n".to_vec() },
        );
        let r = rig("retry", mock, SandboxConfig::default()).await;
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        let failed = wait_state(&r, "lab", |s| matches!(s, SandboxState::Failed { .. })).await;
        assert_eq!(
            failed.state,
            SandboxState::Failed {
                step: "first_run".into(),
                error: "first_run: boom".into()
            }
        );
        let before = r.mock.calls().len();
        // A second Create for the same name is refused.
        assert!(
            r.actor
                .ask(Create {
                    spec: spec("lab"),
                    origin: None
                })
                .await
                .map_err(err)
                .unwrap_err()
                .contains("already exists")
        );
        // Fix the guest (a later canned entry does not win: first match wins, so insert a new mock? no — the
        // mock returns the first prefix match; use a fresh canned list order by re-canning the exact line).
        r.mock.canned("runuser -u engine -- env THEUTH_DATA_DIR=/data PATH=\"/usr/local/bin:$PATH\" t --secret-list", ok(""));
        // The failing entry still wins (first match) — so retry fails again at the same step and never re-creates.
        r.actor.ask(Retry { name: "lab".into() }).await.unwrap();
        let again = wait_state(&r, "lab", |s| matches!(s, SandboxState::Failed { .. })).await;
        assert_eq!(again.state, failed.state);
        let after: Vec<String> = r.mock.calls()[before..].to_vec();
        assert!(
            !after.iter().any(|c| c.starts_with("create ")),
            "retry re-created: {after:?}"
        );
        assert!(
            after.iter().any(|c| c.contains("t --secret-list")),
            "{after:?}"
        );
        assert!(
            after
                .iter()
                .any(|c| c.starts_with("snapshot lab: auto-retry-")),
            "{after:?}"
        );
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn preflight_blockers_refuse_create_with_every_sentence() {
        let mut p = MockVmRuntime::blocked_preflight(
            "unsupported_arch",
            "microsandbox needs Apple Silicon",
        );
        p.blockers.push(Blocker {
            code: "disk_low".into(),
            message: "3 GiB free; a VM needs root_disk + image + 2 GiB".into(),
            fix: Some("free some disk".into()),
            installable: false,
        });
        let r = rig(
            "blocked",
            MockVmRuntime::new().with_preflight(p),
            SandboxConfig::default(),
        )
        .await;
        let e = r
            .actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert_eq!(
            e,
            "microsandbox needs Apple Silicon\n3 GiB free; a VM needs root_disk + image + 2 GiB — free some disk"
        );
        assert!(r.actor.ask(List).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn create_validates_name_and_mesh_uniqueness() {
        let r = rig("names", MockVmRuntime::new(), SandboxConfig::default()).await;
        assert!(
            r.actor
                .ask(Create {
                    spec: spec("Lab!"),
                    origin: None
                })
                .await
                .map_err(err)
                .unwrap_err()
                .contains("a–z")
        );
        assert!(
            r.actor
                .ask(Create {
                    spec: spec("mac"),
                    origin: None
                })
                .await
                .map_err(err)
                .unwrap_err()
                .contains("already in the mesh")
        );
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn resize_validation_and_outcome() {
        let r = rig("resize", MockVmRuntime::new(), SandboxConfig::default()).await;
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        let e = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    cpus: Some(0),
                    ..Default::default()
                },
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert!(e.contains("at least 1"));
        let e = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    root_disk_mib: Some(1),
                    ..Default::default()
                },
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert!(e.contains("only grows"));
        // Metrics sample: 512 MiB used → floor 1024 MiB.
        r.actor.ask(Metrics { name: "lab".into() }).await.unwrap();
        let e = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    memory_mib: Some(1000),
                    ..Default::default()
                },
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert!(e.contains("512 MiB"), "{e}");
        // Ceilings above the host (mock: 8 cores / 16 GiB) are refused; within it, recorded for the next start.
        let e = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    max_cpus: Some(12),
                    ..Default::default()
                },
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert!(e.contains("8 cores"), "{e}");
        let e = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    max_memory_mib: Some(65536),
                    ..Default::default()
                },
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert!(e.contains("16384 MiB"), "{e}");
        let o = r
            .actor
            .ask(Resize {
                name: "lab".into(),
                change: ResourceChange {
                    cpus: Some(3),
                    memory_mib: Some(3072),
                    max_cpus: Some(6),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert_eq!(
            (o.applied.cpus, o.applied.memory_mib),
            (Some(3), Some(3072))
        );
        assert!(o.restart_required);
        let i = wait_state(&r, "lab", |_| true).await;
        let vm = i.vm.unwrap();
        assert_eq!(
            (
                vm.resources.cpus,
                vm.resources.memory_mib,
                vm.resources.max_cpus
            ),
            (3, 3072, 6)
        );
        assert!(vm.pending.resources);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn share_path_validation() {
        let tmp = std::env::temp_dir().join(format!("sandbox-shares-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("Develop")).unwrap();
        std::fs::create_dir_all(tmp.join("other").join("Develop")).unwrap();
        std::fs::write(tmp.join("file"), b"x").unwrap();
        let data = tmp.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let m = |h: &Path, g: &str| Mount {
            host: h.to_path_buf(),
            guest: g.into(),
            readonly: false,
        };
        let bad = |h: &Path| validate_shares(&[m(h, "")], &data, "").unwrap_err();
        assert!(bad(Path::new("relative")).contains("absolute"));
        assert!(bad(Path::new("/")).contains("cannot be shared"));
        assert!(bad(&tmp.join("file")).contains("not a directory"));
        assert!(
            bad(&tmp.join("missing")).contains("No such file")
                || bad(&tmp.join("missing")).contains("not found")
        );
        assert!(bad(&data).contains("data dir"));
        assert!(
            validate_shares(&[m(Path::new("/a,b"), "")], &data, "")
                .unwrap_err()
                .contains(", : or ;")
        );
        let out = validate_shares(
            &[
                m(&tmp.join("Develop"), ""),
                m(&tmp.join("other").join("Develop"), ""),
                m(&tmp.join("Develop"), "/mnt/x"),
            ],
            &data,
            "",
        )
        .unwrap();
        let guests: Vec<&str> = out.iter().map(|m| m.guest.as_str()).collect();
        assert_eq!(guests, ["/shared/Develop", "/shared/Develop-2", "/mnt/x"]);
        assert!(out[0].host.is_absolute());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn restore_alternates_runtime_name_and_destroy_removes_everything() {
        let r = rig("restore", MockVmRuntime::new(), SandboxConfig::default()).await;
        let mut s = spec("lab");
        s.desktop = Some(true);
        r.actor
            .ask(Create {
                spec: s,
                origin: None,
            })
            .await
            .unwrap();
        // Desktop on: the verify step would GET noVNC on the loopback port, which nothing answers.
        let failed = wait_state(&r, "lab", |s| matches!(s, SandboxState::Failed { .. })).await;
        assert!(
            matches!(&failed.state, SandboxState::Failed { step, .. } if step == "verify"),
            "{:?}",
            failed.state
        );
        assert!(
            r.secrets.get(&vnc_row("lab")).await.is_some(),
            "vnc password stored at create"
        );
        assert!(
            failed
                .vm
                .as_ref()
                .unwrap()
                .desktop
                .as_ref()
                .unwrap()
                .host_port
                .is_some()
        );
        // Snapshot, then restore: the runtime name flips to `lab-r`, the old VM is destroyed, the record keeps its name.
        let snap = r
            .actor
            .ask(Snapshot {
                name: "lab".into(),
                label: "clean".into(),
                full: false,
            })
            .await
            .unwrap();
        assert_eq!(snap.id.0, "lab:clean");
        assert!(
            r.actor
                .ask(Snapshot {
                    name: "lab".into(),
                    label: "mem".into(),
                    full: true
                })
                .await
                .map_err(err)
                .unwrap_err()
                .contains("not available")
        );
        let i = r
            .actor
            .ask(Restore {
                name: "lab".into(),
                id: snap.id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(i.state, SandboxState::Running);
        let rec = Store::new(&r.dir).get("lab").unwrap();
        assert_eq!(rec.vm().unwrap().runtime_name, "lab-r");
        assert_eq!(r.mock.vm("lab").unwrap().status, VmStatus::Gone);
        assert_eq!(r.mock.vm("lab-r").unwrap().status, VmStatus::Running);
        assert_eq!(
            r.mock.vm("lab-r").unwrap().spec.ports.len(),
            2,
            "ports re-applied on restore"
        );
        let calls = r.mock.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("snapshot lab: auto-restore-"))
        );
        // Bug 6: the record's list spans both runtime names (`lab:clean`, `lab:auto-restore-*`).
        let listed: Vec<String> = rec
            .vm()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.id.0.clone())
            .collect();
        assert!(listed.contains(&"lab:clean".to_owned()), "{listed:?}");
        assert!(
            listed.iter().any(|s| s.starts_with("lab:auto-restore-")),
            "{listed:?}"
        );
        // Restore again flips back; the auto snapshot now lands in the `lab-r` group.
        let i = r
            .actor
            .ask(Restore {
                name: "lab".into(),
                id: snap.id,
            })
            .await
            .unwrap();
        assert_eq!(i.state, SandboxState::Running);
        let rec = Store::new(&r.dir).get("lab").unwrap();
        assert_eq!(rec.vm().unwrap().runtime_name, "lab");
        let listed: Vec<String> = rec
            .vm()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.id.0.clone())
            .collect();
        assert!(
            listed.iter().any(|s| s.starts_with("lab-r:auto-restore-")),
            "{listed:?}"
        );
        // Destroy without keeping snapshots: peer removed, both runtime names gone, every
        // snapshot of both groups deleted (bug 4), vault row gone, record gone, Removed published.
        r.actor
            .ask(Destroy {
                name: "lab".into(),
                keep_snapshots: false,
            })
            .await
            .unwrap();
        assert!(r.actor.ask(List).await.unwrap().is_empty());
        assert!(r.secrets.get(&vnc_row("lab")).await.is_none());
        assert_eq!(
            r.join.removed.lock().clone(),
            vec!["vm-engine-0".to_owned()]
        );
        assert_eq!(r.mock.vm("lab").unwrap().status, VmStatus::Gone);
        assert_eq!(r.mock.vm("lab-r").unwrap().status, VmStatus::Gone);
        for n in ["lab", "lab-r"] {
            let h = VmHandle {
                name: n.into(),
                runtime_id: None,
            };
            assert!(
                r.mock.snapshots(&h).await.unwrap().is_empty(),
                "{n} group swept"
            );
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            r.events
                .lock()
                .iter()
                .any(|e| matches!(e, SandboxEvent::Removed { name } if name == "lab"))
        );
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// Bug 4, the `gone` case: the runtime lost the VM but its snapshot groups remain; destroy
    /// still sweeps them, and a later create of the same name prunes stale `auto-*` snapshots
    /// while keeping named ones.
    #[tokio::test]
    async fn destroy_of_a_gone_record_sweeps_groups_and_create_prunes_auto() {
        let r = rig("gone", MockVmRuntime::new(), SandboxConfig::default()).await;
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        r.actor
            .ask(Snapshot {
                name: "lab".into(),
                label: "keep".into(),
                full: false,
            })
            .await
            .unwrap();
        let h = VmHandle {
            name: "lab".into(),
            runtime_id: None,
        };
        let out = Progress::silent();
        r.mock
            .snapshot(&h, "auto-retry-1", false, &out)
            .await
            .unwrap();
        // The VM vanishes underneath the record.
        r.mock.destroy(&h).await.unwrap();
        r.actor
            .ask(Destroy {
                name: "lab".into(),
                keep_snapshots: true,
            })
            .await
            .unwrap();
        assert_eq!(
            r.mock.snapshots(&h).await.unwrap().len(),
            2,
            "kept: named + auto"
        );
        // Same name again: the stale auto snapshot goes, `lab:keep` stays.
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        let left: Vec<String> = r
            .mock
            .snapshots(&h)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id.0.clone())
            .collect();
        assert_eq!(left, ["lab:keep"]);
        // Destroy without keeping: the named one goes too.
        r.actor
            .ask(Destroy {
                name: "lab".into(),
                keep_snapshots: false,
            })
            .await
            .unwrap();
        assert!(r.mock.snapshots(&h).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn reconcile_follows_the_runtime() {
        let r = rig("reconcile", MockVmRuntime::new(), SandboxConfig::default()).await;
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        r.actor.tell(Reconcile { autostart: false }).await.unwrap();
        let i = wait_state(&r, "lab", |s| *s == SandboxState::Running).await;
        assert!(
            i.vm.unwrap().metrics.is_some(),
            "metrics sampled while running"
        );
        let h = crate::vm::VmHandle {
            name: "lab".into(),
            runtime_id: None,
        };
        r.mock.stop(&h, Duration::ZERO).await.unwrap();
        r.actor.tell(Reconcile { autostart: false }).await.unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Stopped).await;
        // Autostart at engine start boots it again.
        r.actor.tell(Reconcile { autostart: true }).await.unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Running).await;
        // The VM vanishes from the runtime → gone.
        r.mock.destroy(&h).await.unwrap();
        r.actor.tell(Reconcile { autostart: false }).await.unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Gone).await;
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn shares_set_pending_and_stop_start_apply_them() {
        let r = rig("shares", MockVmRuntime::new(), SandboxConfig::default()).await;
        r.actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .unwrap();
        wait_state(&r, "lab", |s| *s == SandboxState::Ready).await;
        let dir = r.dir.join("Develop");
        std::fs::create_dir_all(&dir).unwrap();
        let p = r
            .actor
            .ask(SetShares {
                name: "lab".into(),
                shares: vec![Mount {
                    host: dir.clone(),
                    guest: String::new(),
                    readonly: true,
                }],
            })
            .await
            .unwrap();
        assert!(p.restart_required, "running vm → restart required");
        let i = wait_state(&r, "lab", |_| true).await;
        let vm = i.vm.unwrap();
        assert!(vm.pending.shares);
        assert_eq!(vm.shares[0].guest, "/shared/Develop");
        assert!(
            r.mock
                .calls()
                .iter()
                .any(|c| c == "put lab: /data/config.yaml"),
            "guest config rewritten"
        );
        let i = r
            .actor
            .ask(Stop {
                name: "lab".into(),
                grace_secs: 1,
                force: false,
            })
            .await
            .unwrap();
        assert_eq!(i.state, SandboxState::Stopped);
        let i = r.actor.ask(Start { name: "lab".into() }).await.unwrap();
        assert_eq!(i.state, SandboxState::Running);
        assert!(!i.vm.unwrap().pending.shares);
        assert!(r.mock.calls().iter().any(|c| c == "set_mounts lab: 1"));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn without_a_runtime_preflight_says_so_and_list_still_works() {
        let dir = std::env::temp_dir().join(format!("sandbox-nort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        record::write(
            &dir,
            &Computer {
                name: "box".into(),
                backend: "docker".into(),
                handle: crate::Handle { id: "c1".into() },
                engine: Some(mesh::EngineId("e1".into())),
                created_ms: 1,
                vm: None,
            },
        )
        .unwrap();
        let actor = SandboxActor::spawn(SandboxActor::with(
            rt,
            SandboxConfig::default(),
            Err("vm runtime not built".into()),
            Arc::new(Join::default()),
            Arc::new(MemSecrets::default()),
        ));
        let p = actor.ask(PreflightNow).await.unwrap();
        assert_eq!(p.blockers[0].code, "runtime_missing");
        assert_eq!(p.blockers[0].message, "vm runtime not built");
        assert!(dir.join("sandbox").join("runtime.json").exists());
        let e = actor
            .ask(Create {
                spec: spec("lab"),
                origin: None,
            })
            .await
            .map_err(err)
            .unwrap_err();
        assert_eq!(e, "vm runtime not built");
        let list = actor.ask(List).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].backend, "docker");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
