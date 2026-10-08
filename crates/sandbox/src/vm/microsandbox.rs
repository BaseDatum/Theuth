//! `impl VmRuntime for Microsandbox` over the microsandbox Rust SDK (79 §4.1).
//!
//! Measured behaviour this file is built on (S1, `79-as-built-S2.md`):
//! - mounts and ports cannot change in place and `Sandbox::restore` drops `init`; the sequence
//!   that keeps the disk *and* systemd is stop → disk snapshot → remove → recreate the full spec
//!   with `override_snapshot` ([`Microsandbox::recreate`]);
//! - `host_port: 0` is bound but never reported, so theuth picks the host port itself;
//! - full snapshots and default-flush disk snapshots are refused for `init: auto` guests.
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::Once,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use microsandbox::{
    CreationProgress, LocalBackend, MicrosandboxError, ModificationDisposition, NetworkPolicy,
    NetworkProfile, PlannedChange, ResourceConvergenceState, Sandbox,
    config::GlobalConfig,
    logs::LogOptions,
    protocol::fs::FsSetAttrs,
    sandbox::{RootfsSource, SandboxBuilder, SandboxHandle, SandboxStatus},
    snapshot::GuestFlush,
};
use parking_lot::Mutex;

use super::{
    ImageRef, Init, LogLine, Mount, NetPolicy, Pending, Port, Preflight, ResizeOutcome,
    ResizeStatus, ResourceChange, SnapshotId, VmHandle, VmMetrics, VmRuntime, VmSnapshotInfo,
    VmSpec, VmStatus, install, preflight,
};
use crate::{Cmd, ExecOutput, Progress, types::VmConfig};

/// The label every theuth VM carries (value = its name).
pub const LABEL: &str = "io.theuth.sandbox";
const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const LIST_PAGE: u32 = 100;

static BACKEND: Once = Once::new();
/// The `home` the backend was built for, so `reset_backend` rebuilds the same one.
static HOME: Mutex<String> = Mutex::new(String::new());

/// Mounts/ports recorded by `set_mounts`/`set_ports`, applied by the next `start`.
#[derive(Default)]
struct PendingSpec {
    mounts: Option<Vec<Mount>>,
    ports: Option<Vec<Port>>,
}

pub struct Microsandbox {
    cfg: VmConfig,
    sdk: GlobalConfig,
    // ponytail: process-local; after an engine restart the actor re-issues set_mounts/set_ports
    // from its record before `start` (the SDK config keeps the *old* mounts until then).
    pending: Mutex<HashMap<String, PendingSpec>>,
    /// Why the last extraction of the embedded runtime failed; preflight shows it as
    /// `runtime_extract_failed` until one succeeds.
    extract_failed: Mutex<Option<String>>,
}

/// Build the SDK backend for `home` (`""` = the SDK default) and install it process-wide.
/// Repeatable: `swap_default_backend` is a `RwLock`, the previous backend drops with its pools.
fn reset_backend(home: &str) {
    let mut b = LocalBackend::builder();
    if !home.is_empty() {
        b = b.home(home);
    }
    match b.build_lazy() {
        Ok(backend) => {
            microsandbox::swap_default_backend(backend);
        }
        Err(e) => tracing::error!("microsandbox backend for {home:?} could not be rebuilt: {e}"),
    }
}

impl Microsandbox {
    /// Installs the process-wide `LocalBackend` for `cfg.home` once.
    pub fn new(cfg: VmConfig) -> anyhow::Result<Self> {
        let sdk = install::config(&cfg.home);
        let mut err = None;
        BACKEND.call_once(|| {
            let mut b = LocalBackend::builder();
            if !cfg.home.is_empty() {
                b = b.home(&cfg.home);
            }
            match b.build_lazy() {
                Ok(backend) => microsandbox::set_default_backend(backend),
                Err(e) => err = Some(e),
            }
            *HOME.lock() = cfg.home.clone();
        });
        if let Some(e) = err {
            bail!("microsandbox backend for {}: {e}", sdk.home().display());
        }
        Ok(Self {
            cfg,
            sdk,
            pending: Mutex::default(),
            extract_failed: Mutex::default(),
        })
    }

    /// Extract the embedded runtime into the home when it is not there yet (first use; seconds,
    /// no network). Remembers a failure for preflight.
    async fn setup_runtime(&self, out: &Progress) -> anyhow::Result<()> {
        match install::install(&self.sdk, out).await {
            Ok(_) => {
                *self.extract_failed.lock() = None;
                Ok(())
            }
            Err(e) => {
                *self.extract_failed.lock() = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    async fn handle(&self, name: &str) -> anyhow::Result<SandboxHandle> {
        Sandbox::get(name)
            .await
            .with_context(|| ctx(name, "lookup"))
    }

    async fn connect(&self, name: &str) -> anyhow::Result<Sandbox> {
        self.handle(name)
            .await?
            .connect()
            .await
            .with_context(|| ctx(name, "connect"))
    }

    /// Boot `builder` (detached), streaming progress; on failure remove the stopped record the
    /// SDK leaves behind. With `init: auto` the agent is ready ~1 s before `init.krun` hands PID 1
    /// to systemd; wait for that so "created" means systemd is up (79 §4.2).
    async fn boot(
        name: &str,
        builder: SandboxBuilder,
        out: &Progress,
        step: &str,
    ) -> anyhow::Result<Sandbox> {
        let (mut progress, task) = builder
            .detached(true)
            .create_with_progress()
            .with_context(|| ctx(name, "create"))?;
        while let Some(ev) = progress.recv().await {
            match ev {
                CreationProgress::Pull(p) => out.say(step, format!("pull: {p:?}")),
                CreationProgress::Startup(s) => out.say(step, format!("startup: {:?}", s.phase)),
            }
        }
        let sb = match task.await.with_context(|| ctx(name, "create task"))? {
            Ok(sb) => sb,
            Err(e) => {
                let _ = Sandbox::remove(name).await;
                // Smoke bug 2: one boot that died in the runtime child left the SDK's single write
                // connection wedged (every later `insert run` hit a FK error until the engine was
                // restarted). A fresh `LocalBackend` = fresh sqlite pools; cheap, so do it on every
                // boot failure rather than only on the FK text.
                reset_backend(&HOME.lock());
                let e = anyhow!(e);
                let e = if format!("{e:#}").contains("FOREIGN KEY constraint failed") {
                    e.context("the runtime database rejected the new sandbox row; the SDK connection was rebuilt — retry, and if it persists restart the engine to recover")
                } else {
                    e
                };
                return Err(e).with_context(|| ctx(name, "boot"));
            }
        };
        if sb.config().spec.init.is_some() {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if let Ok(o) = sb.shell("cat /proc/1/comm").await
                    && o.stdout().is_ok_and(|s| s.trim() == "systemd")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        Ok(sb)
    }

    /// The whole `VmSpec` as a builder (ports with `host == 0` already resolved by the caller).
    fn builder(&self, spec: &VmSpec, ports: &[Port]) -> anyhow::Result<SandboxBuilder> {
        let image = match &spec.image {
            ImageRef::Oci { r#ref } => r#ref.clone(),
            ImageRef::Bootstrap { base, .. } => base.clone(),
        };
        let mut b = Sandbox::builder(&spec.name)
            .image_with(|i| i.oci(image).root_disk(spec.root_disk_mib))
            .cpus(spec.cpus)
            .max_cpus(spec.max_cpus)
            .memory(spec.memory_mib)
            .max_memory(spec.max_memory_mib)
            .hostname(&spec.hostname)
            .label(LABEL, &spec.name)
            .ephemeral(spec.ephemeral);
        if spec.init == Init::Auto {
            b = b.init("auto");
        }
        for (k, v) in &spec.labels {
            b = b.label(k, v);
        }
        for (k, v) in &spec.env {
            b = b.env(k, v);
        }
        if let Some(s) = spec.idle_timeout_secs {
            b = b.idle_timeout(u64::from(s));
        }
        if let Some(s) = spec.max_duration_secs {
            b = b.max_duration(u64::from(s));
        }
        if !spec.files.is_empty() {
            let files = spec.files.clone();
            b = b.patch(|p| {
                files
                    .into_iter()
                    .fold(p, |p, f| p.file(f.path, f.bytes, Some(f.mode), true))
            });
        }
        b = b.network(|n| n.policy(policy(spec.network)));
        b = with_mounts(b, &spec.mounts)?;
        Ok(with_ports(b, ports))
    }

    /// Stop → disk snapshot → remove → recreate the persisted spec with `mounts`/`ports`
    /// swapped in, booted on the snapshot's disk (S1 sequence 6). Keeps `upper.ext4`, systemd
    /// and the labels; the helper snapshot stays as `auto-reconfigure-<ts>` for the actor to prune.
    async fn recreate(
        &self,
        h: &SandboxHandle,
        mounts: Option<Vec<Mount>>,
        ports: Option<Vec<Port>>,
        out: &Progress,
    ) -> anyhow::Result<()> {
        let name = h.name().to_owned();
        let mut config = h.config().with_context(|| ctx(&name, "read config"))?;
        if matches!(
            h.status_snapshot(),
            SandboxStatus::Running | SandboxStatus::Paused
        ) {
            h.stop_with_timeout(Duration::from_secs(30))
                .await
                .with_context(|| ctx(&name, "stop before reconfigure"))?;
        }
        let label = format!("auto-reconfigure-{}", now_ms() / 1000);
        h.snapshot(&label)
            .await
            .with_context(|| ctx(&name, "snapshot before reconfigure"))?;
        out.say("start", format!("disk snapshot {name}:{label}"));
        Sandbox::remove(&name)
            .await
            .with_context(|| ctx(&name, "remove before reconfigure"))?;
        out.say(
            "start",
            format!("removed {name}; recreating with the new shares/ports"),
        );
        // Patches were baked into the disk we keep; mounts/ports are replaced below.
        config.spec.patches.clear();
        if mounts.is_some() {
            config.spec.mounts.clear();
        }
        if ports.is_some() {
            config.spec.network.ports.clear();
        }
        let mut b = SandboxBuilder::from(config).override_snapshot(format!("{name}:{label}"));
        if let Some(m) = &mounts {
            b = with_mounts(b, m)?;
        }
        if let Some(p) = &ports {
            b = with_ports(b, p);
        }
        Self::boot(&name, b, out, "start").await?;
        Ok(())
    }
}

fn ctx(name: &str, stage: &str) -> String {
    format!("sandbox {name}: {stage}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn policy(p: NetPolicy) -> NetworkPolicy {
    match p {
        NetPolicy::PublicHost => {
            NetworkPolicy::from_profiles([NetworkProfile::Public, NetworkProfile::Host])
        }
        NetPolicy::PublicHostPrivate => NetworkPolicy::from_profiles([
            NetworkProfile::Public,
            NetworkProfile::Host,
            NetworkProfile::Private,
        ]),
        NetPolicy::PublicOnly => NetworkPolicy::from_profiles([NetworkProfile::Public]),
        NetPolicy::None => NetworkPolicy::none(),
    }
}

fn with_mounts(mut b: SandboxBuilder, mounts: &[Mount]) -> anyhow::Result<SandboxBuilder> {
    for m in mounts {
        // `/tmp` is a symlink on macOS and virtiofs refuses the un-canonical path at boot.
        let host = std::fs::canonicalize(&m.host)
            .with_context(|| format!("share {}: host path", m.host.display()))?;
        let ro = m.readonly;
        b = b.volume(&m.guest, move |v| {
            let v = v.bind(host);
            if ro { v.readonly() } else { v }
        });
    }
    Ok(b)
}

fn with_ports(mut b: SandboxBuilder, ports: &[Port]) -> SandboxBuilder {
    for p in ports {
        let ip: IpAddr = p.host_ip.parse().unwrap_or(LOCALHOST);
        b = if p.udp {
            b.port_udp_bind(ip, p.host, p.guest)
        } else {
            b.port_bind(ip, p.host, p.guest)
        };
    }
    b
}

/// A free loopback port (bind-and-release). The runtime never reports the port it picks for
/// `host: 0`, so the caller picks; the race window is microseconds and `create` retries once.
pub fn free_port(udp: bool) -> anyhow::Result<u16> {
    let port = if udp {
        std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port()
    } else {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port()
    };
    Ok(port)
}

/// Replace every `host == 0` with a free loopback port. Callers persist the result: a `0` left
/// in a record would bind a different port on every restart.
pub fn resolve_ports(ports: &[Port]) -> anyhow::Result<Vec<Port>> {
    ports
        .iter()
        .map(|p| {
            Ok(Port {
                host: if p.host == 0 {
                    free_port(p.udp)?
                } else {
                    p.host
                },
                ..p.clone()
            })
        })
        .collect()
}

fn map_status(s: SandboxStatus) -> VmStatus {
    match s {
        SandboxStatus::Running | SandboxStatus::Starting | SandboxStatus::Draining => {
            VmStatus::Running
        }
        SandboxStatus::Paused => VmStatus::Paused,
        SandboxStatus::Created | SandboxStatus::Stopped => VmStatus::Stopped,
        SandboxStatus::Crashed => VmStatus::Crashed,
    }
}

fn map_exec(o: microsandbox::ExecOutput) -> ExecOutput {
    ExecOutput {
        status: o.status().code,
        stdout: o.stdout_bytes().to_vec(),
        stderr: o.stderr_bytes().to_vec(),
    }
}

fn is_systemd(h: &SandboxHandle) -> bool {
    h.config().is_ok_and(|c| c.spec.init.is_some())
}

/// `nproc` and `MemTotal` (MiB) as the guest sees them.
async fn guest_size(sb: &Sandbox) -> Option<(u8, u32)> {
    let o = sb
        .shell("nproc; awk '/MemTotal/{print $2}' /proc/meminfo")
        .await
        .ok()?;
    let s = o.stdout().ok()?;
    let mut it = s.lines();
    let cpus = it.next()?.trim().parse().ok()?;
    let kib: u32 = it.next()?.trim().parse().ok()?;
    Some((cpus, kib / 1024))
}

#[async_trait::async_trait]
impl VmRuntime for Microsandbox {
    fn kind(&self) -> &'static str {
        "microsandbox"
    }

    async fn preflight(&self) -> Preflight {
        let cfg = self.cfg.clone();
        let cfg2 = cfg.clone();
        let extract_failed = self.extract_failed.lock().clone();
        let probe = move || {
            let mut facts = preflight::probe(&cfg);
            facts.extract_failed = extract_failed;
            preflight::evaluate(&facts, &cfg)
        };
        match tokio::task::spawn_blocking(probe).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("sandbox preflight probe panicked: {e}");
                let host = super::HostFacts {
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                    cores: 0,
                    mem_bytes: 0,
                    disk_free_bytes: 0,
                };
                Preflight {
                    runtime: super::RuntimeState::Missing,
                    virt: super::VirtState::Unsupported {
                        why: format!("preflight probe failed: {e}"),
                    },
                    defaults: super::Resources::derive(&host, &cfg2.defaults),
                    host,
                    blockers: vec![super::Blocker {
                        code: "preflight_failed".into(),
                        message: format!("the host probe failed: {e}"),
                        fix: Some("check the engine log; retry".into()),
                        installable: false,
                    }],
                    warnings: Vec::new(),
                    checked_at_ms: now_ms(),
                }
            }
        }
    }

    async fn install(&self, out: &Progress) -> anyhow::Result<()> {
        self.setup_runtime(out).await
    }

    async fn create(&self, spec: &VmSpec, out: &Progress) -> anyhow::Result<VmHandle> {
        let name = spec.name.as_str();
        if !matches!(
            install::resolve(&self.sdk),
            super::RuntimeState::Embedded {
                extracted: true,
                ..
            } | super::RuntimeState::Installed { .. }
        ) {
            self.setup_runtime(out).await?;
        }
        if let Ok(h) = Sandbox::get(name).await {
            let cfg = h.config().with_context(|| ctx(name, "read config"))?;
            if cfg.spec.labels.get(LABEL).is_none_or(|v| v != name) {
                bail!("sandbox {name}: the name is taken by a sandbox theuth did not create");
            }
            let same_image = match (&cfg.spec.image, &spec.image) {
                (RootfsSource::Oci(o), ImageRef::Oci { r#ref }) => o.reference == *r#ref,
                (RootfsSource::Oci(o), ImageRef::Bootstrap { base, .. }) => o.reference == *base,
                _ => false,
            };
            if same_image {
                out.say("create", format!("adopting existing sandbox {name}"));
                if map_status(h.status_snapshot()) == VmStatus::Stopped {
                    h.start_detached()
                        .await
                        .with_context(|| ctx(name, "start"))?;
                }
                return Ok(VmHandle {
                    name: name.into(),
                    runtime_id: Some(h.id().to_string()),
                });
            }
            out.say(
                "create",
                format!("replacing sandbox {name} (image changed)"),
            );
            h.destroy()
                .await
                .with_context(|| ctx(name, "destroy stale"))?;
        }
        let auto_ports = spec.ports.iter().any(|p| p.host == 0);
        let mut ports = resolve_ports(&spec.ports)?;
        let mut tries = if auto_ports { 3 } else { 1 };
        let sb = loop {
            tries -= 1;
            match Self::boot(name, self.builder(spec, &ports)?, out, "create").await {
                Ok(sb) => break sb,
                Err(e) if tries > 0 && format!("{e:#}").contains("Address already in use") => {
                    out.say("create", "published port taken; picking another");
                    ports = resolve_ports(&spec.ports)?;
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        };
        if auto_ports {
            let picked: Vec<String> = ports
                .iter()
                .map(|p| {
                    format!(
                        "{}:{}->{}/{}",
                        p.host_ip,
                        p.host,
                        p.guest,
                        if p.udp { "udp" } else { "tcp" }
                    )
                })
                .collect();
            out.say("create", format!("published {}", picked.join(", ")));
        }
        Ok(VmHandle {
            name: name.into(),
            runtime_id: Some(sb.id().to_string()),
        })
    }

    async fn status(&self, h: &VmHandle) -> anyhow::Result<VmStatus> {
        match Sandbox::get(&h.name).await {
            Ok(sh) => Ok(map_status(sh.status_snapshot())),
            Err(MicrosandboxError::SandboxNotFound(_)) => Ok(VmStatus::Gone),
            Err(e) => Err(anyhow!(e)).with_context(|| ctx(&h.name, "status")),
        }
    }

    async fn start(&self, h: &VmHandle, out: &Progress) -> anyhow::Result<()> {
        let sh = self.handle(&h.name).await?;
        let pending = self.pending.lock().remove(&h.name);
        match pending {
            Some(p) if p.mounts.is_some() || p.ports.is_some() => {
                self.recreate(&sh, p.mounts, p.ports, out).await
            }
            _ => sh
                .start_detached()
                .await
                .map(|_| ())
                .with_context(|| ctx(&h.name, "start")),
        }
    }

    async fn stop(&self, h: &VmHandle, grace: Duration) -> anyhow::Result<()> {
        let sh = self.handle(&h.name).await?;
        match sh.stop_with_timeout(grace).await {
            Ok(()) => Ok(()),
            Err(MicrosandboxError::StopTimeout { .. }) => sh
                .kill()
                .await
                .with_context(|| ctx(&h.name, "kill after grace")),
            Err(e) => Err(anyhow!(e)).with_context(|| ctx(&h.name, "stop")),
        }
    }

    // Host control, not the agent: `connect()` refuses a Paused sandbox.
    async fn pause(&self, h: &VmHandle) -> anyhow::Result<()> {
        self.handle(&h.name)
            .await?
            .pause()
            .await
            .with_context(|| ctx(&h.name, "pause"))
    }

    async fn resume(&self, h: &VmHandle) -> anyhow::Result<()> {
        self.handle(&h.name)
            .await?
            .resume()
            .await
            .with_context(|| ctx(&h.name, "resume"))
    }

    async fn destroy(&self, h: &VmHandle) -> anyhow::Result<()> {
        self.pending.lock().remove(&h.name);
        match Sandbox::get(&h.name).await {
            Ok(sh) => {
                // `destroy` (like `remove`) refuses a paused VM: "still running".
                if sh.status_snapshot() == SandboxStatus::Paused {
                    sh.resume()
                        .await
                        .with_context(|| ctx(&h.name, "resume before destroy"))?;
                }
                sh.destroy().await.with_context(|| ctx(&h.name, "destroy"))
            }
            Err(MicrosandboxError::SandboxNotFound(_)) => Ok(()),
            Err(e) => Err(anyhow!(e)).with_context(|| ctx(&h.name, "destroy")),
        }
    }

    async fn exec(&self, h: &VmHandle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let sb = self.connect(&h.name).await?;
        let stdin = cmd.stdin.clone();
        sb.shell_with(cmd.line.clone(), move |o| {
            let o = o.timeout(cmd.timeout);
            match stdin {
                Some(b) => o.stdin_bytes(b.to_vec()),
                None => o.stdin_null(),
            }
        })
        .await
        .map(map_exec)
        .with_context(|| ctx(&h.name, &format!("exec `{}`", cmd.line)))
    }

    async fn put(&self, h: &VmHandle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
        let sb = self.connect(&h.name).await?;
        let stage = |s: &str| ctx(&h.name, &format!("put {path}: {s}"));
        if let Some((dir, _)) = path.rsplit_once('/')
            && !dir.is_empty()
        {
            let o = sb
                .shell(format!("mkdir -p '{dir}'"))
                .await
                .with_context(|| stage("mkdir"))?;
            if o.status().code != 0 {
                bail!(
                    "{}",
                    stage(&format!("mkdir failed: {}", o.stderr().unwrap_or_default()))
                );
            }
        }
        let tmp = format!("{path}.theuth-tmp");
        let fs = sb.fs();
        fs.write(&tmp, &bytes)
            .await
            .with_context(|| stage("write"))?;
        fs.set_stat(
            &tmp,
            false,
            FsSetAttrs {
                mode: Some(mode),
                ..Default::default()
            },
        )
        .await
        .with_context(|| stage("chmod"))?;
        fs.rename(&tmp, path).await.with_context(|| stage("rename"))
    }

    async fn resize(&self, h: &VmHandle, change: ResourceChange) -> anyhow::Result<ResizeOutcome> {
        let sh = self.handle(&h.name).await?;
        // The SDK silently ignores a smaller root disk; design §5.5 says refuse it.
        if let Some(want) = change.root_disk_mib {
            let current = match sh.config().ok().map(|c| c.spec.image) {
                Some(RootfsSource::Oci(o)) => match o.root_disk {
                    Some(microsandbox::sandbox::RootDisk::Managed { size_mib: Some(m) }) => Some(m),
                    _ => None,
                },
                _ => None,
            };
            // A snapshot-restored disk has no configured size: take the filesystem's, rounded
            // up to 256 MiB (ext4 metadata eats ~3 %).
            let current = match current {
                Some(m) => m,
                None => {
                    let m = sh
                        .metrics()
                        .await
                        .with_context(|| ctx(&h.name, "metrics"))?;
                    let bytes = m.upper_used_bytes.unwrap_or(0) + m.upper_free_bytes.unwrap_or(0);
                    u32::try_from(bytes.div_ceil(256 << 20) * 256).unwrap_or(u32::MAX)
                }
            };
            if want < current {
                bail!(
                    "sandbox {}: the root disk can only grow ({current} MiB now, {want} MiB asked)",
                    h.name
                );
            }
        }
        let with = |mut m: microsandbox::SandboxModificationBuilder, c: &ResourceChange| {
            if let Some(v) = c.cpus {
                m = m.cpus(v);
            }
            if let Some(v) = c.max_cpus {
                m = m.max_cpus(v);
            }
            if let Some(v) = c.memory_mib {
                m = m.memory(v);
            }
            if let Some(v) = c.max_memory_mib {
                m = m.max_memory(v);
            }
            if let Some(v) = c.root_disk_mib {
                m = m.root_disk_size(v);
            }
            m
        };
        let plan = with(sh.modify(), &change)
            .dry_run()
            .await
            .with_context(|| ctx(&h.name, "resize plan"))?;
        let mut live = ResourceChange::default();
        let mut later = ResourceChange::default();
        for c in &plan.changes {
            let PlannedChange::Config(c) = c else {
                continue;
            };
            let (dst, field) = match c.disposition {
                ModificationDisposition::Live => (&mut live, c.field.as_str()),
                ModificationDisposition::NextStart | ModificationDisposition::RequiresRestart => {
                    (&mut later, c.field.as_str())
                }
                ModificationDisposition::Unsupported => bail!(
                    "sandbox {}: resize {} {}→{} is not supported: {}",
                    h.name,
                    c.field,
                    c.before.as_deref().unwrap_or("?"),
                    c.after.as_deref().unwrap_or("?"),
                    c.reason.as_deref().unwrap_or("no reason given")
                ),
            };
            match field {
                "cpus" => dst.cpus = change.cpus,
                "max_cpus" => dst.max_cpus = change.max_cpus,
                "memory" => dst.memory_mib = change.memory_mib,
                "max_memory" => dst.max_memory_mib = change.max_memory_mib,
                "root_disk_size" | "root_disk" => dst.root_disk_mib = change.root_disk_mib,
                _ => {}
            }
        }
        let mut status = ResizeStatus::Applied;
        if live != ResourceChange::default() {
            let applied = with(sh.modify(), &live)
                .apply()
                .await
                .with_context(|| ctx(&h.name, "resize apply"))?;
            for r in &applied.resize_status {
                match r.state {
                    ResourceConvergenceState::GuestRefused | ResourceConvergenceState::Failed => {
                        status = ResizeStatus::GuestRefused {
                            why: format!("{:?} {}→{}", r.resource, r.requested, r.actual),
                        }
                    }
                    ResourceConvergenceState::Accepted | ResourceConvergenceState::Converging => {
                        if status == ResizeStatus::Applied {
                            status = ResizeStatus::Converging;
                        }
                    }
                    ResourceConvergenceState::Applied => {}
                }
            }
        }
        if later != ResourceChange::default() {
            with(sh.modify(), &later)
                .next_start()
                .apply()
                .await
                .with_context(|| ctx(&h.name, "resize next_start"))?;
        }
        // No completion event (upstream #1578): poll the guest until it reports the new size.
        if status == ResizeStatus::Converging
            && (live.cpus.is_some() || live.memory_mib.is_some())
            && let Ok(sb) = self.connect(&h.name).await
        {
            let deadline = Instant::now() + Duration::from_secs(self.cfg.resize_settle_secs);
            while Instant::now() < deadline {
                if let Some((cpus, mem)) = guest_size(&sb).await {
                    let cpu_ok = live.cpus.is_none_or(|c| c == cpus);
                    // MemTotal sits a few % under the plugged size (kernel reservations).
                    let mem_ok = live
                        .memory_mib
                        .is_none_or(|m| mem >= m * 92 / 100 && mem <= m + m / 50);
                    if cpu_ok && mem_ok {
                        status = ResizeStatus::Applied;
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Ok(ResizeOutcome {
            applied: live,
            next_start: later,
            status,
            restart_required: later != ResourceChange::default(),
        })
    }

    async fn set_mounts(&self, h: &VmHandle, mounts: &[Mount]) -> anyhow::Result<Pending> {
        let sh = self.handle(&h.name).await?;
        self.pending
            .lock()
            .entry(h.name.clone())
            .or_default()
            .mounts = Some(mounts.to_vec());
        Ok(Pending {
            restart_required: map_status(sh.status_snapshot()) != VmStatus::Stopped,
        })
    }

    async fn set_ports(&self, h: &VmHandle, ports: &[Port]) -> anyhow::Result<Pending> {
        let sh = self.handle(&h.name).await?;
        self.pending.lock().entry(h.name.clone()).or_default().ports = Some(resolve_ports(ports)?);
        Ok(Pending {
            restart_required: map_status(sh.status_snapshot()) != VmStatus::Stopped,
        })
    }

    async fn metrics(&self, h: &VmHandle) -> anyhow::Result<VmMetrics> {
        let m = self
            .handle(&h.name)
            .await?
            .metrics()
            .await
            .with_context(|| ctx(&h.name, "metrics"))?;
        // `memory_bytes` reads 0 right after a resize; fall back to limit − available.
        let memory_bytes = if m.memory_bytes > 0 {
            m.memory_bytes
        } else {
            m.memory_limit_bytes
                .saturating_sub(m.memory_available_bytes.unwrap_or(m.memory_limit_bytes))
        };
        Ok(VmMetrics {
            cpu_percent: m.cpu_percent,
            memory_bytes,
            memory_limit_bytes: m.memory_limit_bytes,
            disk_used_bytes: m.upper_used_bytes.unwrap_or(0),
            disk_free_bytes: m.upper_free_bytes.unwrap_or(0),
            net_rx_bytes: m.net_rx_bytes,
            net_tx_bytes: m.net_tx_bytes,
            uptime_secs: m.uptime.as_secs(),
            at_ms: m.timestamp.timestamp_millis().max(0) as u64,
        })
    }

    /// The SDK's log store holds exec output and the host-side runtime log only; the guest
    /// journal comes from `journalctl` when systemd is PID 1 and the VM is running.
    async fn logs(&self, h: &VmHandle, tail: usize) -> anyhow::Result<Vec<LogLine>> {
        let sh = self.handle(&h.name).await?;
        let opts = LogOptions {
            tail: Some(tail),
            ..Default::default()
        };
        let mut lines: Vec<LogLine> = sh
            .logs(&opts)
            .await
            .with_context(|| ctx(&h.name, "logs"))?
            .into_iter()
            .map(|e| LogLine {
                at_ms: e.timestamp.timestamp_millis().max(0) as u64,
                source: format!("{:?}", e.source).to_lowercase(),
                line: String::from_utf8_lossy(&e.data).trim_end().to_owned(),
            })
            .collect();
        if is_systemd(&sh)
            && map_status(sh.status_snapshot()) == VmStatus::Running
            && let Ok(sb) = sh.connect().await
            && let Ok(o) = sb
                .shell(format!(
                    "journalctl -n {tail} --no-pager -o short-unix 2>/dev/null"
                ))
                .await
        {
            for l in o.stdout().unwrap_or_default().lines() {
                let (ts, rest) = l.split_once(' ').unwrap_or(("0", l));
                lines.push(LogLine {
                    at_ms: (ts.parse::<f64>().unwrap_or(0.0) * 1000.0) as u64,
                    source: "system".into(),
                    line: rest.to_owned(),
                });
            }
        }
        lines.sort_by_key(|l| l.at_ms);
        let skip = lines.len().saturating_sub(tail);
        Ok(lines.split_off(skip))
    }

    async fn snapshot(
        &self,
        h: &VmHandle,
        name: &str,
        full: bool,
        out: &Progress,
    ) -> anyhow::Result<SnapshotId> {
        let sh = self.handle(&h.name).await?;
        let systemd = is_systemd(&sh);
        if full && systemd {
            bail!(
                "sandbox {}: full snapshots are not available for systemd guests (init auto) in microsandbox 0.7.4 — the workload freezer is unavailable; take a disk snapshot instead",
                h.name
            );
        }
        let running = map_status(sh.status_snapshot()) == VmStatus::Running;
        let mut b = microsandbox::Snapshot::builder(name).from_sandbox(&h.name);
        if full {
            b = b.full();
        } else if running {
            // Default flush needs the freezer; `sync` in the guest + Skip is the working route.
            if let Ok(sb) = sh.connect().await {
                let _ = sb.shell("sync").await;
            }
            b = b.guest_flush(GuestFlush::Skip);
        }
        let t = Instant::now();
        let snap = b
            .create()
            .await
            .with_context(|| ctx(&h.name, &format!("snapshot {name}")))?;
        out.say(
            "snapshot",
            format!(
                "{}:{name} ({}, virtual {} MiB) in {} ms",
                h.name,
                if full { "full" } else { "disk" },
                snap.size_bytes().unwrap_or(0) / (1024 * 1024),
                t.elapsed().as_millis()
            ),
        );
        let id = SnapshotId(format!("{}:{name}", h.name));
        // The SDK indexes on create but only warns when that write fails (smoke bug 2: `msb
        // snapshot ls` empty until `reindex`); fall back to its reindex when the id is missing.
        if !self
            .snapshots(h)
            .await
            .is_ok_and(|l| l.iter().any(|s| s.id == id))
        {
            match microsandbox::Snapshot::reindex_default().await {
                Ok(n) => out.say(
                    "snapshot",
                    format!("{} was not indexed; reindexed {n} snapshots", id.0),
                ),
                Err(e) => tracing::warn!("snapshot {}: reindex: {e}", id.0),
            }
        }
        Ok(id)
    }

    /// `Sandbox::restore` drops `init`, cpus/memory, ports and mounts (upstream #1676); the
    /// full `spec` is booted on the snapshot's disk with `override_snapshot` instead.
    async fn restore(
        &self,
        id: &SnapshotId,
        as_name: &str,
        spec: &VmSpec,
        out: &Progress,
    ) -> anyhow::Result<VmHandle> {
        if Sandbox::get(as_name).await.is_ok() {
            bail!("sandbox {as_name}: already exists; restore needs a free name");
        }
        let mut spec = spec.clone();
        spec.name = as_name.into();
        // The disk already holds the pre-boot files; patching again would only rewrite them.
        spec.files.clear();
        let ports = resolve_ports(&spec.ports)?;
        let b = self.builder(&spec, &ports)?.override_snapshot(&id.0);
        let sb = Self::boot(as_name, b, out, "restore").await?;
        Ok(VmHandle {
            name: as_name.into(),
            runtime_id: Some(sb.id().to_string()),
        })
    }

    async fn snapshots(&self, h: &VmHandle) -> anyhow::Result<Vec<VmSnapshotInfo>> {
        let prefix = format!("{}:", h.name);
        let all = microsandbox::Snapshot::list()
            .await
            .with_context(|| ctx(&h.name, "list snapshots"))?;
        Ok(all
            .into_iter()
            .filter_map(|s| {
                let name = s.name()?;
                let label = match (s.group(), name.strip_prefix(&prefix)) {
                    (_, Some(l)) => l,
                    (Some(g), None) if g == h.name => name,
                    _ => return None,
                };
                Some(VmSnapshotInfo {
                    id: SnapshotId(format!("{prefix}{label}")),
                    label: label.to_owned(),
                    full: s.scope() == microsandbox::SnapshotScope::Full,
                    at_ms: s.created_at().and_utc().timestamp_millis().max(0) as u64,
                    bytes: s.size_bytes(),
                    auto: label.starts_with("auto-"),
                })
            })
            .collect())
    }

    /// The SDK refuses to remove a group's head while other members exist; move the head to
    /// another member of the same VM first, then retry.
    async fn delete_snapshot(&self, id: &SnapshotId) -> anyhow::Result<()> {
        let stage = || format!("snapshot {}: delete", id.0);
        match microsandbox::Snapshot::remove(&id.0, true).await {
            Ok(()) => return Ok(()),
            Err(e) if e.to_string().contains("cannot remove current head") => {}
            Err(e) => return Err(anyhow!(e)).with_context(stage),
        }
        let (vm, _) = id.0.split_once(':').unwrap_or((&id.0, ""));
        let h = VmHandle {
            name: vm.into(),
            runtime_id: None,
        };
        let other = self
            .snapshots(&h)
            .await?
            .into_iter()
            .find(|s| s.id != *id)
            .ok_or_else(|| {
                anyhow!(
                    "{}: it is the group head and no other member was found",
                    stage()
                )
            })?;
        microsandbox::Snapshot::group_head(&other.id.0)
            .await
            .with_context(|| format!("{}: move head to {}", stage(), other.id.0))?;
        microsandbox::Snapshot::remove(&id.0, true)
            .await
            .with_context(stage)
    }

    async fn list(&self) -> anyhow::Result<Vec<VmHandle>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = Sandbox::list_with(|l| {
                let l = l.limit(LIST_PAGE);
                match &cursor {
                    Some(c) => l.cursor(c),
                    None => l,
                }
            })
            .await
            .context("sandbox list")?;
            for sh in page.sandboxes {
                let ours = sh.config().is_ok_and(|c| c.spec.labels.contains_key(LABEL));
                if ours {
                    out.push(VmHandle {
                        name: sh.name().to_owned(),
                        runtime_id: Some(sh.id().to_string()),
                    });
                }
            }
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
    }
}
