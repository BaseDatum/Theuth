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
//! - commands on the [`SANDBOX_ACTOR`] actor ([`SandboxActor`]):
//!   [`MakeEngine`], [`Engines`]
//! - `<data_dir>/computers/<name>.yaml` ([`record`]), one per compute this
//!   crate created

pub mod bringup;
pub mod docker;
pub mod record;
pub mod release;
pub mod ssh;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub use bringup::{MeshJoin, Spec};
pub use record::Computer;

/// Registry name of the [`SandboxActor`].
pub const SANDBOX_ACTOR: &str = "sandbox";

/// Directory under `data_dir` holding one record per compute this crate created (64 §4.7).
pub const DIR: &str = "computers";

/// Per-command ceiling (`sandbox.step_timeout_secs` in 64 §4.7; a constant here).
pub const STEP_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// A command with no stdin and the default ceiling.
    pub fn new(line: impl Into<String>) -> Self {
        Self {
            line: line.into(),
            stdin: None,
            timeout: STEP_TIMEOUT,
        }
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

/// Single-quote for `sh`; a literal `'` becomes `'\''`.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

/// `ask` → `Result<mesh::Peer, String>`: run bring-up on `Target` and join the
/// mesh; the reply is the new peer. Progress goes to the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MakeEngine(pub Target);

/// `ask` → `Result<Vec<Computer>, String>`: the records under `computers/`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Engines;

/// The [`SANDBOX_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct SandboxActor {
    rt: Runtime,
}

impl Message<MakeEngine> for SandboxActor {
    type Reply = Result<mesh::Peer, String>;

    // ponytail: bring-up runs inside the handler (minutes) so one engine is
    // made at a time; spawn + progress events if the UI ever drives this.
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
            &ActorJoin,
            &Progress::log(),
        )
        .await
        .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Engines> for SandboxActor {
    type Reply = Result<Vec<Computer>, String>;

    async fn handle(&mut self, _: Engines, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        record::list(&self.rt.data_dir).map_err(|e| format!("{e:#}"))
    }
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
            (Box::new(sb), Spec::new(name))
        }
        Target::Docker {
            name,
            image,
            local_t,
        } => {
            let sb = docker::DockerSandbox::connect(&name, image, local_t.clone()).await?;
            let spec = Spec {
                local_t,
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
        engine: done.peer.id.clone(),
        created_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
    };
    record::write(data_dir, &rec)?;
    out.say("record", format!("{}/{}.yaml", DIR, rec.name));
    Ok(done.peer)
}

/// The plugin: spawns [`SandboxActor`] as [`SANDBOX_ACTOR`].
pub struct SandboxPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SandboxPlugin {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let actor = SandboxActor::spawn(SandboxActor { rt: rt.clone() });
        actor.register(SANDBOX_ACTOR)?;
        Ok(())
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
