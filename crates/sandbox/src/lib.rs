//! Make an engine (design doc 67 §13, kept from 64 §4): bring `t` up on
//! compute that has no theuth yet — an SSH host or a Docker container on this
//! machine — and join it to the mesh. Two primitives, `exec` and `put`, carry
//! the whole installer; each backend is thin.
//!
//! Public surface consumed by other plugins:
//! - the [`Sandbox`] trait and its two backends ([`ssh::SshSandbox`],
//!   [`docker::DockerSandbox`])
//! - [`bringup::run`], the detect → binary → first run → daemon → pair →
//!   verify sequence, generic over the backend and over how the mesh is
//!   reached ([`MeshJoin`]: the mesh actor in-process, JSON-RPC from the CLI)
//! - commands on the [`SANDBOX_ACTOR`] actor ([`SandboxActor`], [`actor`]):
//!   [`MakeEngine`], [`Engines`], and the sandbox VM commands of design 79
//!   ([`Create`] … [`EnsureVm`]); [`tools`]: the `sandbox_*` tools and prompt
//!   blocks; [`config`]: the `sandbox:` section and what it switches on
//! - `<data_dir>/computers/<name>.yaml` ([`record`]), one per compute this
//!   crate created
//! - [`vm`]: the `VmRuntime` trait (79 §4.1); [`types`]: the wire types and
//!   the `sandbox:` config (79 §2.2, §4.7)

pub mod actor;
pub mod bringup;
pub mod config;
pub mod docker;
pub mod guest;
pub mod record;
pub mod release;
pub mod ssh;
pub mod takeover;
pub mod tools;
pub mod types;
pub mod vm;
pub mod rpc_support;
/// Sandbox RPC server and client traits.
pub mod rpc;
pub use rpc::rpc_module;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use kameo::actor::{ActorRef, Spawn};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub use actor::{
    Create, DesktopAccess, Destroy, Engines, EnsureVm, InstallRuntime, List, Logs, MakeEngine,
    Metrics, Pause, PreflightNow, Resize, Restore, Resume, Retry, SandboxActor, SetShares,
    Snapshot, Start, Stop, Takeover, Terminal,
};
pub use bringup::{MeshJoin, Spec};
pub use record::Computer;
// 79 wire types at the crate root; `types::DesktopAccess` stays qualified (the
// message struct of the same name lives here).
pub use types::{
    DesktopView, GuestConfig, PendingView, SandboxConfig, SandboxCreate, SandboxEvent, SandboxInfo,
    SandboxState, VmConfig, VmDefaults, VmView,
};
pub use vm::{
    Blocker, HostFacts, ImageRef, LogLine, Mount, NetPolicy, Pending, Port, Preflight,
    ResizeOutcome, ResizeStatus, ResourceChange, Resources, RuntimeState, SnapshotId, VirtState,
    VmMetrics, VmSnapshotInfo, VmStatus,
};

/// Registry name of the [`SandboxActor`].
pub const SANDBOX_ACTOR: &str = "sandbox";

/// Directory under `data_dir` holding one record per compute this crate created (64 §4.7).
pub const DIR: &str = "computers";

/// One progress sink: `(step, line)`.
type Sink = dyn Fn(&str, &str) + Send + Sync;

/// Where progress lines go: `(step, line)`. The CLI prints them; the actor logs them.
#[derive(Clone)]
pub struct Progress(Arc<Sink>);

impl Progress {
    /// `[step] line` on stdout.
    pub fn stdout() -> Self {
        Self(Arc::new(|step, line| println!("[{step}] {line}")))
    }

    /// `tracing::info!` per line.
    pub fn log() -> Self {
        Self(Arc::new(|step, line| {
            tracing::info!("sandbox {step}: {line}")
        }))
    }

    /// Drops every line (tests).
    pub fn silent() -> Self {
        Self(Arc::new(|_, _| {}))
    }

    /// Any sink.
    pub fn new(sink: impl Fn(&str, &str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(sink))
    }

    /// Emit one line.
    pub fn say(&self, step: &str, line: impl AsRef<str>) {
        (self.0)(step, line.as_ref());
    }
}

/// Backend-opaque identity of one compute: the container id (docker) or the
/// `user@host:port` target (ssh). Serialised into the computer record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    /// Backend-specific id.
    pub id: String,
}

/// One command for [`Sandbox::exec`]: a POSIX shell line (the target's login
/// shell runs it), optional stdin, a ceiling.
#[derive(Clone, Debug)]
pub struct Cmd {
    /// Shell line, run by `sh -lc` (docker) or the login shell sshd hands it to (ssh).
    pub line: String,
    /// Bytes written to the command's stdin, then EOF.
    pub stdin: Option<Bytes>,
    /// Ceiling; the command is abandoned and the step fails when it passes.
    pub timeout: Duration,
}

impl Cmd {
    /// The ceiling when none is set: `sandbox.step_timeout_secs`' default (600 s).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

    /// A command with no stdin and the default ceiling.
    pub fn new(line: impl Into<String>) -> Self {
        Self {
            line: line.into(),
            stdin: None,
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Set the ceiling.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Feed `bytes` on stdin.
    pub fn stdin(mut self, bytes: Bytes) -> Self {
        self.stdin = Some(bytes);
        self
    }
}

/// What a command produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// Exit status; `-1` when the transport lost it.
    pub status: i32,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    /// Exit status 0.
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// stdout as text, trimmed.
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_owned()
    }

    /// The last non-empty line of stderr, else of stdout, else the status — for failure messages.
    pub fn reason(&self) -> String {
        let last = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_owned())
        };
        last(&self.stderr)
            .or_else(|| last(&self.stdout))
            .unwrap_or_else(|| format!("exit status {}", self.status))
    }
}

/// How `t --headless` keeps running on the compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonKind {
    /// The container command is `t --headless`; the restart policy is "start on boot".
    Entrypoint,
    /// `t --install-service` writes a launchd / systemd user / Task Scheduler unit.
    Service,
}

/// What the compute is; the installer plans against it (64 §4.1, trimmed to what bring-up reads).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// `linux` | `macos` | `windows` (release asset naming).
    pub os: String,
    /// `x86_64` | `aarch64` (release asset naming).
    pub arch: String,
    /// Paths that survive `destroy(preserve_data: true)`.
    pub durable_roots: Vec<String>,
    /// How the daemon is kept alive.
    pub daemon: DaemonKind,
}

/// Compute the installer is written against (64 §4.1). `exec` and `put` are
/// the only primitives bring-up uses; `create` is idempotent (an existing
/// compute is adopted or, when it no longer matches, replaced).
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync {
    /// `"docker"` | `"ssh"`.
    fn kind(&self) -> &'static str;
    /// Create the container / verify the host answers. Idempotent.
    async fn create(&self, out: &Progress) -> anyhow::Result<Handle>;
    /// Run one shell line; `Err` only when the transport fails, a non-zero exit is an `Ok` output.
    async fn exec(&self, h: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput>;
    /// Write a file (parent directories created) with `mode`.
    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()>;
    /// Paths that survive `destroy(preserve_data: true)`.
    fn durable_roots(&self) -> Vec<String>;
    /// How the daemon is kept alive on this backend.
    fn daemon(&self) -> DaemonKind;
    /// OS and arch over `exec` (`uname -sm`, PowerShell fallback) plus the backend's constants.
    async fn capabilities(&self, h: &Handle) -> anyhow::Result<Capabilities> {
        let (os, arch) = detect(self, h).await?;
        Ok(Capabilities {
            os,
            arch,
            durable_roots: self.durable_roots(),
            daemon: self.daemon(),
        })
    }
    /// Remove the compute; `preserve_data` keeps [`Self::durable_roots`].
    async fn destroy(&self, h: &Handle, preserve_data: bool) -> anyhow::Result<()>;
}

/// `(os, arch)` in release-asset spelling: `uname -sm` first, PowerShell when
/// there is no `uname` (Windows OpenSSH).
pub async fn detect(sb: &(impl Sandbox + ?Sized), h: &Handle) -> anyhow::Result<(String, String)> {
    let out = sb.exec(h, Cmd::new("uname -sm")).await?;
    if out.ok()
        && let Some(p) = parse_uname(&out.stdout_str())
    {
        return Ok(p);
    }
    let ps = "powershell -NoProfile -Command \"[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture\"";
    let out = sb.exec(h, Cmd::new(ps)).await?;
    if out.ok()
        && let Some(arch) = parse_arch(&out.stdout_str())
    {
        return Ok(("windows".into(), arch));
    }
    anyhow::bail!("cannot detect os/arch: {}", out.reason())
}

/// `"Linux x86_64"` → `("linux", "x86_64")`, `"Darwin arm64"` → `("macos", "aarch64")`.
pub fn parse_uname(s: &str) -> Option<(String, String)> {
    let mut it = s.split_whitespace();
    let os = match it.next()? {
        "Linux" => "linux",
        "Darwin" => "macos",
        _ => return None,
    };
    Some((os.into(), parse_arch(it.next()?)?))
}

/// `x86_64|amd64|X64` → `x86_64`; `aarch64|arm64|Arm64` → `aarch64`.
pub fn parse_arch(s: &str) -> Option<String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => Some("x86_64".into()),
        "aarch64" | "arm64" => Some("aarch64".into()),
        _ => None,
    }
}

/// What `t mesh add --ssh|--docker` asks for; the actor's [`MakeEngine`] payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase")]
pub enum Target {
    /// An SSH host.
    Ssh {
        /// `[user@]host[:port]` (built-in) or an `~/.ssh/config` alias (system).
        target: String,
        /// Vault name of the private key (built-in transport); `None` → the system `ssh`.
        key: Option<String>,
        /// Force the system `ssh`/`scp` binaries.
        system: bool,
        /// Record and `link.name`; default: the host.
        name: Option<String>,
    },
    /// A container on this machine.
    Docker {
        /// Container/record name.
        name: String,
        /// A pre-built image ref instead of building `deploy/computer/Dockerfile`.
        image: Option<String>,
        /// A locally built Linux `t` → `Dockerfile.dev`.
        local_t: Option<PathBuf>,
    },
}

/// [`MeshJoin`] through the in-process mesh actor.
pub struct ActorJoin;

#[async_trait::async_trait]
impl MeshJoin for ActorJoin {
    async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer> {
        let actor = mesh::actor().ok_or_else(|| anyhow::anyhow!("mesh not running"))?;
        actor.ask(mesh::AddPeer(pairing)).await.map_err(mesh_err)
    }

    async fn status(&self) -> anyhow::Result<mesh::MeshStatus> {
        let actor = mesh::actor().ok_or_else(|| anyhow::anyhow!("mesh not running"))?;
        actor.ask(mesh::Status).await.map_err(mesh_err)
    }

    async fn remove_peer(&self, id: mesh::EngineId) -> anyhow::Result<()> {
        let actor = mesh::actor().ok_or_else(|| anyhow::anyhow!("mesh not running"))?;
        actor.ask(mesh::RemovePeer(id)).await.map_err(mesh_err)
    }
}

/// The handler's `Err(String)` as the message; a dead mailbox as `mesh: …`.
fn mesh_err<M: std::fmt::Debug>(e: kameo::error::SendError<M, String>) -> anyhow::Error {
    match e {
        kameo::error::SendError::HandlerError(s) => anyhow::anyhow!(s),
        e => anyhow::anyhow!("mesh: {e}"),
    }
}

/// Build the backend for `target` (reading the SSH key through `secret`, a
/// vault lookup by name that never leaves memory), run [`bringup::run`], write
/// the computer record. The one entry both the CLI and the actor use.
pub async fn make_engine<F, Fut>(
    data_dir: &std::path::Path,
    target: Target,
    secret: F,
    join: &dyn MeshJoin,
    out: &Progress,
    step_timeout: Duration,
) -> anyhow::Result<mesh::Peer>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<zeroize::Zeroizing<String>, String>>,
{
    let (sb, spec): (Box<dyn Sandbox>, Spec) = match target {
        Target::Ssh {
            target,
            key,
            system,
            name,
        } => {
            let t = ssh::Target::parse(&target)?;
            let name = name.unwrap_or_else(|| t.host.clone());
            let sb = match (system, key) {
                (false, Some(key)) => {
                    let pem = secret(key).await.map_err(anyhow::Error::msg)?;
                    ssh::SshSandbox::builtin(t, &pem)?
                }
                (false, None) => anyhow::bail!(
                    "--ssh needs --key <vault name> (built-in client) or --system-ssh"
                ),
                (true, _) => ssh::SshSandbox::system(t),
            };
            let spec = Spec {
                step_timeout,
                ..Spec::new(name)
            };
            (Box::new(sb), spec)
        }
        Target::Docker {
            name,
            image,
            local_t,
        } => {
            let sb = docker::DockerSandbox::connect(&name, image, local_t.clone()).await?;
            let spec = Spec {
                local_t,
                step_timeout,
                ..Spec::new(name)
            };
            (Box::new(sb), spec)
        }
    };
    let done = bringup::run(&*sb, &spec, join, out).await?;
    let rec = Computer {
        name: spec.name.clone(),
        backend: sb.kind().into(),
        handle: done.handle,
        engine: Some(done.peer.id.clone()),
        created_ms: record::now_ms(),
        vm: None,
    };
    record::write(data_dir, &rec)?;
    out.say("record", format!("{}/{}.yaml", DIR, rec.name));
    Ok(done.peer)
}

/// Declare wire contracts before starting producers, in every sandbox mode.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<SandboxEvent>().await
}

/// The plugin (79 §4.7): `enabled: false` → nothing; `guest:` set → the guest
/// prompt block and `sandbox_takeover` only; else the [`SandboxActor`] as
/// [`SANDBOX_ACTOR`], its tools, a reconcile at start (+ autostart) and every
/// `health_interval_secs`.
pub struct SandboxPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SandboxPlugin {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg = config::load(rt);
        match config::mode(&cfg) {
            config::Mode::Disabled => {
                tracing::info!("sandbox disabled (sandbox.enabled: false)");
                Ok(())
            }
            config::Mode::Guest(g) => {
                tracing::info!(vm = %g.name, host = %g.host, "sandbox: this engine is a guest");
                guest::start(rt, g);
                Ok(())
            }
            config::Mode::Host { vm } => {
                tracing::info!(vm, runtime = %cfg.vm.runtime, "sandbox config");
                let health = Duration::from_secs(cfg.health_interval_secs);
                let actor = SandboxActor::spawn(SandboxActor::new(rt.clone(), cfg.clone()));
                actor.register(SANDBOX_ACTOR)?;
                let _ = actor.tell(actor::Reconcile { autostart: true }).await;
                actor::spawn_health(actor.clone(), health);
                let rt2 = rt.clone();
                tokio::spawn(async move {
                    if let Some(reg) = rt2
                        .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                        .await
                    {
                        tools::register(&reg, rt2.clone(), cfg).await;
                    }
                });
                Ok(())
            }
        }
    }
}

/// The sandbox actor; `None` before the plugin started.
pub fn actor() -> Option<ActorRef<SandboxActor>> {
    ActorRef::<SandboxActor>::lookup(SANDBOX_ACTOR)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_and_powershell_spellings() {
        assert_eq!(
            parse_uname("Linux x86_64"),
            Some(("linux".into(), "x86_64".into()))
        );
        assert_eq!(
            parse_uname("Darwin arm64\n"),
            Some(("macos".into(), "aarch64".into()))
        );
        assert_eq!(
            parse_uname("Linux aarch64"),
            Some(("linux".into(), "aarch64".into()))
        );
        assert_eq!(parse_uname("FreeBSD amd64"), None);
        assert_eq!(parse_arch("X64"), Some("x86_64".into()));
        assert_eq!(parse_arch("Arm64"), Some("aarch64".into()));
        assert_eq!(parse_arch("X86"), None);
    }

    #[test]
    fn reason_prefers_stderr_last_line() {
        let out = ExecOutput {
            status: 2,
            stdout: b"ok\n".to_vec(),
            stderr: b"warn\nfatal: no\n\n".to_vec(),
        };
        assert_eq!(out.reason(), "fatal: no");
        let out = ExecOutput {
            status: 3,
            ..Default::default()
        };
        assert_eq!(out.reason(), "exit status 3");
    }
}
