//! Bring-up (64 §4.4, 67 §13): detect → binary → first run → daemon → pair →
//! verify. Written against [`Sandbox::exec`]/[`Sandbox::put`] only, and
//! against [`MeshJoin`] for the last two steps, so a mock of each runs the
//! whole sequence in a unit test. Every step prints one progress line and
//! fails with a one-line reason.

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;

use crate::record::{Computer, Setup, Store, VM_STEPS};
use crate::types::SandboxState;
use crate::vm::{ImageRef, Init, Mount, Port, PrebootFile, VmRuntime, VmSpec};
use crate::{Cmd, DaemonKind, ExecOutput, Handle, Progress, Sandbox, release};
use platform::shell::single_quote as quote;

/// How this engine's mesh is reached for the pair and verify steps: the mesh
/// actor in-process ([`crate::ActorJoin`]) or JSON-RPC from the CLI.
#[async_trait::async_trait]
pub trait MeshJoin: Send + Sync {
    /// `mesh::AddPeer(pairing)`.
    async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer>;
    /// `mesh::Status`.
    async fn status(&self) -> anyhow::Result<mesh::MeshStatus>;
    /// `mesh::RemovePeer` (a VM's `Destroy` removes its engine, 79 §5.6).
    async fn remove_peer(&self, id: mesh::EngineId) -> anyhow::Result<()>;
}

/// What to bring up.
#[derive(Clone, Debug)]
pub struct Spec {
    /// Record name and the engine's `link.name`.
    pub name: String,
    /// A `t` built here for the target (`--local-t`); `None` → the latest release for the target's triple.
    pub local_t: Option<PathBuf>,
    /// `t mesh pair` retries while the daemon comes up (each after `verify_every`).
    pub pair_tries: u32,
    /// `Status` polls before giving up on "connected".
    pub verify_tries: u32,
    /// Pause between polls.
    pub verify_every: Duration,
    /// Per-command ceiling (`sandbox.step_timeout_secs`).
    pub step_timeout: Duration,
}

impl Spec {
    /// Defaults: release binary, 6 pair tries, 3 verify tries 10 s apart (64 §4.4 step 6), 600 s per step.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            local_t: None,
            pair_tries: 6,
            verify_tries: 3,
            verify_every: Duration::from_secs(10),
            step_timeout: Cmd::DEFAULT_TIMEOUT,
        }
    }
}

/// What bring-up leaves behind.
#[derive(Clone, Debug)]
pub struct Done {
    /// The compute.
    pub handle: Handle,
    /// The new engine as this engine's mesh sees it.
    pub peer: mesh::Peer,
}

/// Where the installer puts `t` on an ssh host (in the unit's login-shell PATH on most distros).
pub const BIN_DIR: &str = "$HOME/.local/bin";

/// `t <args>` with the install dir on PATH (the docker image has `t` on PATH already).
fn t(args: &str, timeout: Duration) -> Cmd {
    Cmd::new(format!("PATH=\"{BIN_DIR}:$PATH\" t {args}")).timeout(timeout)
}

fn fail(step: &str, out: &ExecOutput) -> anyhow::Error {
    anyhow::anyhow!("{step}: {}", out.reason())
}

/// The sequence. Idempotent per step: a present `t` is kept (unless
/// `spec.local_t` is set), an installed service is re-installed in place, an
/// already-paired engine is paired again (mutual pairing is idempotent).
pub async fn run(
    sb: &dyn Sandbox,
    spec: &Spec,
    join: &dyn MeshJoin,
    out: &Progress,
) -> anyhow::Result<Done> {
    out.say("create", sb.kind());
    let h = sb.create(out).await?;

    let caps = sb.capabilities(&h).await?;
    out.say("detect", format!("{} {}", caps.os, caps.arch));
    if caps.os == "windows" {
        anyhow::bail!(
            "detect: windows targets are not brought up by this version — install t there and run `t mesh pair`"
        );
    }

    match caps.daemon {
        DaemonKind::Entrypoint => out.say("binary", "in the image"),
        DaemonKind::Service => {
            let files: Vec<(String, Bytes)> = match &spec.local_t {
                Some(p) => {
                    out.say("binary", format!("local {}", p.display()));
                    let b =
                        std::fs::read(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                    vec![("t".into(), Bytes::from(b))]
                }
                None => {
                    let have = sb.exec(&h, t("--version", spec.step_timeout)).await?;
                    if have.ok() {
                        out.say("binary", format!("present: {}", have.stdout_str()));
                        Vec::new()
                    } else {
                        release::fetch(&caps.os, &caps.arch, out).await?
                    }
                }
            };
            for (name, bytes) in files {
                sb.put(&h, &format!("{BIN_DIR}/{name}"), bytes, 0o755)
                    .await?;
                out.say("binary", format!("installed {BIN_DIR}/{name}"));
            }
            // Any one-shot runs `first_run` (main's prologue) and proves the binary loads here.
            let fr = sb.exec(&h, t("--secret-list", spec.step_timeout)).await?;
            if !fr.ok() {
                return Err(fail("first_run", &fr));
            }
            out.say("first_run", "ok");
        }
    }

    match caps.daemon {
        DaemonKind::Entrypoint => out.say("daemon", "container entrypoint"),
        DaemonKind::Service => {
            let inst = sb
                .exec(
                    &h,
                    t(
                        &format!("--install-service --no-pair --name {}", quote(&spec.name)),
                        spec.step_timeout,
                    ),
                )
                .await?;
            if !inst.ok() {
                return Err(fail("daemon", &inst));
            }
            out.say("daemon", inst.stdout_str());
        }
    }

    let mut pairing = None;
    for i in 0..spec.pair_tries {
        let p = sb.exec(&h, t("mesh pair", spec.step_timeout)).await?;
        if p.ok() {
            pairing = Some(parse_pairing(&p.stdout_str())?);
            break;
        }
        if i + 1 == spec.pair_tries {
            return Err(fail("pair", &p));
        }
        tokio::time::sleep(spec.verify_every).await;
    }
    let pairing = pairing.ok_or_else(|| anyhow::anyhow!("pair: no pairing string"))?;
    let peer = join
        .add_peer(pairing)
        .await
        .map_err(|e| anyhow::anyhow!("pair: {e:#}"))?;
    out.say("pair", format!("{} ({})", peer.name, peer.id));

    for i in 0..spec.verify_tries {
        let status = join
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("verify: {e:#}"))?;
        if status.peers.iter().any(|p| p.id == peer.id && p.reachable) {
            out.say("verify", "connected");
            return Ok(Done { handle: h, peer });
        }
        if i + 1 < spec.verify_tries {
            tokio::time::sleep(spec.verify_every).await;
        }
    }
    anyhow::bail!(
        "verify: {} paired but not reachable yet — check `t mesh status`",
        peer.name
    )
}

/// `t mesh pair` prints its JSON-RPC reply pretty-printed: a JSON string. A
/// bare string is accepted too.
pub fn parse_pairing(stdout: &str) -> anyhow::Result<String> {
    let s = stdout.trim();
    let p = match serde_json::from_str::<String>(s) {
        Ok(p) => p,
        Err(_) => s.to_owned(),
    };
    if p.is_empty() {
        anyhow::bail!("pair: empty pairing string");
    }
    Ok(p)
}

// ---- 79 §5.3: bring-up for a sandbox VM, against `VmRuntime` and the record.

/// What the host contributes to a VM's bring-up: its own mesh relay and tools
/// policy (copied into the guest config, contract §2.10), its name, and the knobs.
#[derive(Clone, Debug)]
pub struct HostSide {
    /// `mesh.relay` of this engine.
    pub relay: String,
    /// `tools.policy` of this engine, verbatim; `None` → the guest keeps its default policy.
    pub tools_policy: Option<serde_json::Value>,
    /// This engine's mesh name (the guest's `sandbox.guest.host`).
    pub host_name: String,
    /// Publish the iroh UDP port on the loopback and hand the mesh that address (S1 finding 4).
    pub loopback_mesh: bool,
    /// Per-command and per-step ceiling.
    pub step_timeout: Duration,
    /// `t mesh pair` retries.
    pub pair_tries: u32,
    /// Reachability polls.
    pub verify_tries: u32,
    /// Pause between polls.
    pub verify_every: Duration,
}

/// The guest's `/data/config.yaml` (contract §2.10, verbatim keys).
pub fn guest_config(name: &str, host: &HostSide, shares: &[Mount]) -> String {
    let workspaces: Vec<&str> = shares.iter().map(|m| m.guest.as_str()).collect();
    let mut cfg = serde_json::json!({
        "mesh": { "name": name, "hub": "never", "class": "vm", "bind": format!("0.0.0.0:{MESH_GUEST_PORT}"),
                  "relay": host.relay, "workspaces": workspaces },
        "secrets": { "key_store": "file" },
        "link": { "enabled": false },
        "computer": { "enabled": true },
        "sandbox": { "enabled": false, "guest": { "name": name, "host": host.host_name, "shares": workspaces } },
    });
    if let Some(policy) = &host.tools_policy {
        cfg["tools"] = serde_json::json!({ "policy": policy });
    }
    serde_yaml::to_string(&cfg).unwrap_or_default()
}

/// Guest port websockify/noVNC listens on.
pub const VNC_GUEST_PORT: u16 = 6080;
/// Guest UDP port the VM's iroh endpoint binds (`mesh.bind`).
pub const MESH_GUEST_PORT: u16 = 7800;
/// Where bring-up puts `t` when the image has none (`bootstrap`, `--local-t`).
pub const GUEST_BIN: &str = "/usr/local/bin/t";
/// The guest config path (pre-boot file, confirmed by the `config` step).
pub const GUEST_CONFIG: &str = "/data/config.yaml";

/// `t <args>` inside the guest, as the `engine` user against `/data`.
fn gt(args: &str, timeout: Duration) -> Cmd {
    Cmd::new(format!(
        "runuser -u engine -- env THEUTH_DATA_DIR=/data PATH=\"/usr/local/bin:$PATH\" t {args}"
    ))
    .timeout(timeout)
}

/// Bind-and-release a free loopback port (S1 finding 4: the runtime never
/// reports a `0` host port, so theuth picks it).
pub fn free_port(udp: bool) -> anyhow::Result<u16> {
    if udp {
        Ok(std::net::UdpSocket::bind("127.0.0.1:0")?
            .local_addr()?
            .port())
    } else {
        Ok(std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port())
    }
}

/// The `VmSpec` a record asks for (create and restore build the same one).
pub fn vm_spec(rec: &Computer, host: &HostSide, runtime_name: &str) -> anyhow::Result<VmSpec> {
    let vm = rec.vm().map_err(anyhow::Error::msg)?;
    let mut ports = Vec::new();
    if vm.desktop.enabled {
        let p = vm
            .desktop
            .host_port
            .ok_or_else(|| anyhow::anyhow!("desktop port not picked"))?;
        ports.push(Port {
            host_ip: "127.0.0.1".into(),
            host: p,
            guest: VNC_GUEST_PORT,
            udp: false,
        });
    }
    if let Some(p) = vm.mesh_port.host_port {
        ports.push(Port {
            host_ip: "127.0.0.1".into(),
            host: p,
            guest: vm.mesh_port.guest,
            udp: true,
        });
    }
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("io.theuth.sandbox".to_owned(), rec.name.clone());
    Ok(VmSpec {
        name: runtime_name.to_owned(),
        image: vm.image.clone(),
        cpus: vm.resources.cpus,
        max_cpus: vm.resources.max_cpus,
        memory_mib: vm.resources.memory_mib,
        max_memory_mib: vm.resources.max_memory_mib,
        root_disk_mib: vm.resources.root_disk_mib,
        mounts: vm.shares.clone(),
        ports,
        network: vm.network,
        init: Init::Auto,
        hostname: rec.name.clone(),
        env: std::collections::BTreeMap::new(),
        files: vec![PrebootFile {
            path: GUEST_CONFIG.into(),
            bytes: guest_config(&rec.name, host, &vm.shares).into_bytes(),
            mode: 0o644,
        }],
        labels,
        ephemeral: false,
        idle_timeout_secs: None,
        max_duration_secs: None,
    })
}

/// Decode `1.<base32 json>`, replace `addrs` with the one loopback UDP address
/// the host can reach the VM's iroh endpoint on, re-encode (79 §5.3 step 7).
pub fn loopback_pairing(pairing: &str, port: u16) -> anyhow::Result<String> {
    use data_encoding::BASE32_NOPAD;
    let (version, body) = pairing
        .trim()
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("pair: malformed pairing string"))?;
    let bytes = BASE32_NOPAD.decode(body.to_ascii_uppercase().as_bytes())?;
    let mut v: serde_json::Value = serde_json::from_slice(&bytes)?;
    v["addrs"] = serde_json::json!([{ "Ip": format!("127.0.0.1:{port}") }]);
    Ok(format!(
        "{version}.{}",
        BASE32_NOPAD.encode(&serde_json::to_vec(&v)?)
    ))
}

/// Everything a VM bring-up talks to.
pub struct VmCx<'a> {
    /// The VM runtime.
    pub runtime: &'a dyn VmRuntime,
    /// The records.
    pub store: &'a Store,
    /// The mesh, for pair and verify.
    pub join: &'a dyn MeshJoin,
    /// Progress lines.
    pub out: &'a Progress,
    /// This engine's inputs to the guest.
    pub host: &'a HostSide,
    /// `sandbox/<name>/vnc`, read from the vault by the actor; `None` with no desktop.
    pub vnc_password: Option<zeroize::Zeroizing<String>>,
    /// Called after every record write (the actor publishes `Changed`).
    pub changed: &'a (dyn Fn(&Computer) + Send + Sync),
}

impl VmCx<'_> {
    fn mark(&self, name: &str, step: &str, error: Option<String>) -> anyhow::Result<Computer> {
        let (_, rec) = self
            .store
            .update(name, |rec| {
                let vm = rec.vm_mut()?;
                vm.setup = Setup {
                    step: step.to_owned(),
                    at_ms: crate::record::now_ms(),
                    error: error.clone(),
                };
                vm.state = match &error {
                    None => SandboxState::Creating {
                        step: step.to_owned(),
                    },
                    Some(e) => SandboxState::Failed {
                        step: step.to_owned(),
                        error: e.clone(),
                    },
                };
                vm.updated_ms = vm.setup.at_ms;
                Ok(())
            })
            .map_err(anyhow::Error::msg)?;
        (self.changed)(&rec);
        Ok(rec)
    }
}

/// The sequence for a VM record (79 §5.3), resumable: a `failed{step}` record
/// restarts at that step. On success the record is `ready` with its engine id;
/// on error it is `failed{step, error}` and the error is returned.
pub async fn run_vm(cx: &VmCx<'_>, name: &str) -> anyhow::Result<()> {
    let rec = cx.store.get(name).map_err(anyhow::Error::msg)?;
    let start = match &rec.vm().map_err(anyhow::Error::msg)?.state {
        SandboxState::Failed { step, .. } | SandboxState::Creating { step } => {
            VM_STEPS.iter().position(|s| s == step).unwrap_or(0)
        }
        _ => 0,
    };
    for step in &VM_STEPS[start..] {
        let rec = cx.mark(name, step, None)?;
        let r = tokio::time::timeout(cx.host.step_timeout, vm_step(cx, step, rec))
            .await
            .unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "timed out after {:?}",
                    cx.host.step_timeout
                ))
            });
        if let Err(e) = r {
            let msg = format!("{e:#}");
            cx.mark(name, step, Some(msg.clone()))?;
            anyhow::bail!("{step}: {msg}");
        }
    }
    let (_, rec) = cx
        .store
        .update(name, |rec| {
            rec.set_state(SandboxState::Ready);
            Ok(())
        })
        .map_err(anyhow::Error::msg)?;
    (cx.changed)(&rec);
    cx.out.say("ready", &rec.name);
    Ok(())
}

async fn vm_step(cx: &VmCx<'_>, step: &str, mut rec: Computer) -> anyhow::Result<()> {
    let name = rec.name.clone();
    let h = rec.vm_handle().map_err(anyhow::Error::msg)?;
    let t_o = cx.host.step_timeout;
    match step {
        "create" => {
            // Ports are ours to pick (S1 finding 4); picked once, kept across retries.
            let (_, r) = cx
                .store
                .update(&name, |rec| {
                    let vm = rec.vm_mut()?;
                    if vm.desktop.enabled && vm.desktop.host_port.is_none() {
                        vm.desktop.host_port = Some(free_port(false).map_err(|e| e.to_string())?);
                    }
                    if cx.host.loopback_mesh && vm.mesh_port.host_port.is_none() {
                        vm.mesh_port.host_port = Some(free_port(true).map_err(|e| e.to_string())?);
                    }
                    Ok(())
                })
                .map_err(anyhow::Error::msg)?;
            rec = r;
            let spec = vm_spec(&rec, cx.host, &h.name)?;
            cx.out.say(
                "create",
                format!("{} {}", cx.runtime.kind(), image_str(&spec.image)),
            );
            cx.runtime.create(&spec, cx.out).await?;
        }
        "detect" => {
            let out = cx
                .runtime
                .exec(&h, Cmd::new("uname -sm").timeout(t_o))
                .await?;
            let (os, arch) = crate::parse_uname(&out.stdout_str())
                .ok_or_else(|| anyhow::anyhow!("unrecognised uname: {}", out.stdout_str()))?;
            if os != "linux" {
                anyhow::bail!("a sandbox VM guest must be Linux, got {os}");
            }
            cx.out.say("detect", format!("{os} {arch}"));
        }
        "config" => {
            if vmr(&rec)?.desktop.enabled {
                let pw = cx.vnc_password.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("vnc password missing from the vault (sandbox/{name}/vnc)")
                })?;
                // systemd is PID 1 within ~0.5 s of boot but its D-Bus socket comes later;
                // `systemctl` fails with "Failed to connect to bus" until then, so retry.
                let cmd = Cmd::new(
                    "mkdir -p /home/engine/.vnc && vncpasswd -f > /home/engine/.vnc/passwd \
                     && chmod 600 /home/engine/.vnc/passwd && chown -R engine:engine /home/engine/.vnc \
                     && for i in $(seq 100); do systemctl restart xvnc && exit 0; sleep 0.2; done; exit 1",
                )
                .stdin(Bytes::from(format!("{}\n", pw.as_str())))
                .timeout(t_o);
                let out = cx.runtime.exec(&h, cmd).await?;
                if !out.ok() {
                    return Err(fail("vncpasswd", &out));
                }
            }
            // The SDK's pre-boot file leaves `/data` root-owned (the image's chown is
            // shadowed), and `t` as `engine` then fails with EACCES at `first_run`.
            cx.runtime
                .exec(
                    &h,
                    Cmd::new("mkdir -p /data && chown -R engine:engine /data").timeout(t_o),
                )
                .await?;
            let probe = cx
                .runtime
                .exec(&h, Cmd::new(format!("test -s {GUEST_CONFIG}")).timeout(t_o))
                .await?;
            if !probe.ok() {
                let text = guest_config(&name, cx.host, &vmr(&rec)?.shares);
                cx.runtime
                    .put(&h, GUEST_CONFIG, Bytes::from(text), 0o644)
                    .await?;
                cx.out.say("config", format!("{GUEST_CONFIG} written"));
            } else {
                cx.out.say("config", format!("{GUEST_CONFIG} present"));
            }
        }
        "binary" => {
            let vm = vmr(&rec)?;
            let bytes: Option<Bytes> = match &vm.local_t {
                Some(p) => {
                    cx.out.say("binary", format!("local {}", p.display()));
                    Some(Bytes::from(
                        std::fs::read(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?,
                    ))
                }
                None => {
                    let have = cx.runtime.exec(&h, gt("--version", t_o)).await?;
                    if have.ok() {
                        cx.out
                            .say("binary", format!("present: {}", have.stdout_str()));
                        None
                    } else {
                        let arch = cx
                            .runtime
                            .exec(&h, Cmd::new("uname -m").timeout(t_o))
                            .await?;
                        let arch = crate::parse_arch(&arch.stdout_str())
                            .ok_or_else(|| anyhow::anyhow!("unrecognised arch"))?;
                        release::fetch("linux", &arch, cx.out)
                            .await?
                            .into_iter()
                            .find(|(n, _)| n == "t")
                            .map(|(_, b)| b)
                    }
                }
            };
            if let Some(b) = bytes {
                cx.runtime.put(&h, GUEST_BIN, b, 0o755).await?;
                cx.out.say("binary", format!("installed {GUEST_BIN}"));
            }
        }
        "first_run" => {
            let fr = cx.runtime.exec(&h, gt("--secret-list", t_o)).await?;
            if !fr.ok() {
                return Err(fail("first_run", &fr));
            }
            cx.out.say("first_run", "ok");
        }
        "daemon" => {
            // The image enables `theuth.service` at boot, so it was already running
            // while `config` re-owned `/data`, `binary` may have replaced `t` and
            // `first_run` opened the vault — two processes migrating one fresh
            // `secrets.sqlite` ("table … already exists", 79 F5). One restart here
            // puts the engine on the prepared data dir, alone.
            let rs = cx
                .runtime
                .exec(&h, Cmd::new("systemctl restart theuth").timeout(t_o))
                .await?;
            if !rs.ok() {
                return Err(fail("daemon restart", &rs));
            }
            let units = if vmr(&rec)?.desktop.enabled {
                "theuth xvnc desktop novnc"
            } else {
                "theuth"
            };
            let deadline = tokio::time::Instant::now() + t_o;
            loop {
                let out = cx
                    .runtime
                    .exec(
                        &h,
                        Cmd::new(format!("systemctl is-active {units}")).timeout(t_o),
                    )
                    .await?;
                if out.ok() {
                    cx.out.say("daemon", format!("active: {units}"));
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    let j = cx
                        .runtime
                        .exec(&h, Cmd::new("journalctl -n 30 --no-pager").timeout(t_o))
                        .await
                        .map(|o| o.stdout_str())
                        .unwrap_or_default();
                    anyhow::bail!(
                        "units not active ({}); journal:\n{j}",
                        out.stdout_str().replace('\n', " ")
                    );
                }
                tokio::time::sleep(cx.host.verify_every.min(Duration::from_secs(5))).await;
            }
        }
        "pair" => {
            let mut pairing = None;
            for i in 0..cx.host.pair_tries {
                let p = cx.runtime.exec(&h, gt("mesh pair", t_o)).await?;
                if p.ok() {
                    pairing = Some(parse_pairing(&p.stdout_str())?);
                    break;
                }
                if i + 1 == cx.host.pair_tries {
                    return Err(fail("pair", &p));
                }
                tokio::time::sleep(cx.host.verify_every).await;
            }
            let mut pairing = pairing.ok_or_else(|| anyhow::anyhow!("no pairing string"))?;
            if let (true, Some(port)) = (cx.host.loopback_mesh, vmr(&rec)?.mesh_port.host_port) {
                pairing = loopback_pairing(&pairing, port)?;
            }
            let peer = cx.join.add_peer(pairing).await?;
            cx.out.say("pair", format!("{} ({})", peer.name, peer.id));
            let (_, r) = cx
                .store
                .update(&name, |rec| {
                    rec.engine = Some(peer.id.clone());
                    Ok(())
                })
                .map_err(anyhow::Error::msg)?;
            (cx.changed)(&r);
        }
        "verify" => {
            let id = rec
                .engine
                .clone()
                .ok_or_else(|| anyhow::anyhow!("not paired"))?;
            let mut reachable = false;
            for i in 0..cx.host.verify_tries {
                let status = cx.join.status().await?;
                if status.peers.iter().any(|p| p.id == id && p.reachable) {
                    reachable = true;
                    break;
                }
                if i + 1 < cx.host.verify_tries {
                    tokio::time::sleep(cx.host.verify_every).await;
                }
            }
            if !reachable {
                anyhow::bail!("{name} paired but not reachable yet — check `t mesh status`");
            }
            cx.out.say("verify", "connected");
            if let (true, Some(port)) = (vmr(&rec)?.desktop.enabled, vmr(&rec)?.desktop.host_port) {
                let url = format!("http://127.0.0.1:{port}/vnc.html");
                let mut ok = false;
                for _ in 0..cx.host.verify_tries.max(1) {
                    if let Ok(r) = reqwest::get(&url).await
                        && r.status().is_success()
                    {
                        ok = true;
                        break;
                    }
                    tokio::time::sleep(cx.host.verify_every.min(Duration::from_secs(5))).await;
                }
                if !ok {
                    anyhow::bail!("noVNC not answering on {url}");
                }
                cx.out.say("verify", format!("desktop {url}"));
            }
        }
        other => anyhow::bail!("unknown step {other}"),
    }
    Ok(())
}

fn vmr(rec: &Computer) -> anyhow::Result<&crate::record::VmRecord> {
    rec.vm().map_err(anyhow::Error::msg)
}

/// One-line image name for progress.
fn image_str(i: &ImageRef) -> String {
    match i {
        ImageRef::Oci { r#ref } => r#ref.clone(),
        ImageRef::Bootstrap { base, .. } => format!("bootstrap on {base}"),
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::Capabilities;

    /// Scripted compute: every exec line is recorded; replies come from a
    /// match on the line; `puts` records paths and modes.
    struct Mock {
        daemon: DaemonKind,
        os: &'static str,
        have_t: bool,
        pair_fail_first: bool,
        log: Mutex<Vec<String>>,
        puts: Mutex<Vec<(String, u32, usize)>>,
    }

    impl Mock {
        fn new(daemon: DaemonKind) -> Self {
            Self {
                daemon,
                os: "Linux x86_64",
                have_t: false,
                pair_fail_first: false,
                log: Mutex::new(Vec::new()),
                puts: Mutex::new(Vec::new()),
            }
        }
        fn lines(&self) -> Vec<String> {
            self.log.lock().clone()
        }
    }

    fn reply(status: i32, stdout: &str) -> ExecOutput {
        ExecOutput {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[async_trait::async_trait]
    impl Sandbox for Mock {
        fn kind(&self) -> &'static str {
            "mock"
        }
        async fn create(&self, _: &Progress) -> anyhow::Result<Handle> {
            Ok(Handle { id: "m".into() })
        }
        async fn exec(&self, _: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
            let n = {
                let mut log = self.log.lock();
                log.push(cmd.line.clone());
                log.iter().filter(|l| l.ends_with("t mesh pair")).count()
            };
            let l = cmd.line.as_str();
            Ok(if l == "uname -sm" {
                reply(0, self.os)
            } else if l.ends_with("t --version") {
                if self.have_t {
                    reply(0, "t 1.16.0")
                } else {
                    reply(127, "")
                }
            } else if l.ends_with("t --secret-list") {
                reply(0, "key: file\n")
            } else if l.contains("--install-service") {
                reply(0, "service installed: user")
            } else if l.ends_with("t mesh pair") {
                if self.pair_fail_first && n == 1 {
                    reply(1, "no harness on 127.0.0.1:7777")
                } else {
                    reply(0, "\"pair-me\"\n")
                }
            } else {
                reply(1, "")
            })
        }
        async fn put(&self, _: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
            self.puts.lock().push((path.into(), mode, bytes.len()));
            Ok(())
        }
        fn durable_roots(&self) -> Vec<String> {
            vec!["/data".into()]
        }
        fn daemon(&self) -> DaemonKind {
            self.daemon
        }
        async fn capabilities(&self, h: &Handle) -> anyhow::Result<Capabilities> {
            let (os, arch) = crate::detect(self, h).await?;
            Ok(Capabilities {
                os,
                arch,
                durable_roots: self.durable_roots(),
                daemon: self.daemon,
            })
        }
        async fn destroy(&self, _: &Handle, _: bool) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Mesh double: remembers the pairing string, reports the peer reachable from the `reachable_after`-th poll.
    struct Join {
        added: Mutex<Vec<String>>,
        polls: Mutex<u32>,
        reachable_after: u32,
    }

    fn peer(reachable: bool) -> mesh::Peer {
        mesh::Peer {
            id: mesh::EngineId("e1".into()),
            name: "box".into(),
            addrs: vec![],
            reachable,
            last_seen_ms: 0,
            hub_pref: mesh::HubPref::Eligible,
            path: mesh::PeerPath::Offline,
            rtt_ms: None,
        }
    }

    #[async_trait::async_trait]
    impl MeshJoin for Join {
        async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer> {
            self.added.lock().push(pairing);
            Ok(peer(false))
        }
        async fn status(&self) -> anyhow::Result<mesh::MeshStatus> {
            let mut p = self.polls.lock();
            *p += 1;
            Ok(mesh::MeshStatus {
                self_id: mesh::EngineId("me".into()),
                self_name: "me".into(),
                mesh_name: None,
                peers: vec![peer(*p >= self.reachable_after)],
                lease: mesh::LeaseState {
                    holder: None,
                    since_ms: 0,
                    reason: String::new(),
                },
                traffic: Vec::new(),
                replica: None,
                stats: Vec::new(),
                relay: None,
                messaging_off: Vec::new(),
                nearby: true,
                quarantined: Vec::new(),
                docs_homes_on: false,
            })
        }
        async fn remove_peer(&self, _: mesh::EngineId) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn join(reachable_after: u32) -> Join {
        Join {
            added: Mutex::new(Vec::new()),
            polls: Mutex::new(0),
            reachable_after,
        }
    }

    fn spec() -> Spec {
        Spec {
            verify_every: Duration::from_millis(1),
            ..Spec::new("box")
        }
    }

    #[tokio::test]
    async fn service_host_with_local_binary_runs_every_step_in_order() {
        let sb = Mock::new(DaemonKind::Service);
        let bin = std::env::temp_dir().join(format!("sandbox-t-{}", std::process::id()));
        std::fs::write(&bin, b"ELF").unwrap();
        let spec = Spec {
            local_t: Some(bin.clone()),
            ..spec()
        };
        let j = join(2);
        let done = run(&sb, &spec, &j, &Progress::silent()).await.unwrap();
        std::fs::remove_file(bin).ok();

        assert_eq!(done.peer.id.0, "e1");
        let lines = sb.lines();
        let tails: Vec<&str> = lines
            .iter()
            .map(|l| l.rsplit(" t ").next().unwrap())
            .collect();
        assert_eq!(
            tails,
            [
                "uname -sm",
                "--secret-list",
                "--install-service --no-pair --name 'box'",
                "mesh pair"
            ]
        );
        assert_eq!(*sb.puts.lock(), vec![(format!("{BIN_DIR}/t"), 0o755, 3)]);
        assert_eq!(*j.added.lock(), vec!["pair-me"]);
        assert_eq!(*j.polls.lock(), 2, "verify polls until reachable");
    }

    #[tokio::test]
    async fn present_binary_is_kept_and_container_skips_install() {
        let mut sb = Mock::new(DaemonKind::Service);
        sb.have_t = true;
        run(&sb, &spec(), &join(1), &Progress::silent())
            .await
            .unwrap();
        assert!(sb.puts.lock().is_empty());
        assert!(sb.lines().iter().any(|l| l.ends_with("t --version")));

        let sb = Mock::new(DaemonKind::Entrypoint);
        run(&sb, &spec(), &join(1), &Progress::silent())
            .await
            .unwrap();
        let lines = sb.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], "uname -sm");
        assert!(lines[1].ends_with("t mesh pair"));
    }

    #[tokio::test]
    async fn pair_retries_while_daemon_starts_and_windows_is_refused() {
        let mut sb = Mock::new(DaemonKind::Entrypoint);
        sb.pair_fail_first = true;
        run(&sb, &spec(), &join(1), &Progress::silent())
            .await
            .unwrap();
        assert_eq!(
            sb.lines()
                .iter()
                .filter(|l| l.ends_with("t mesh pair"))
                .count(),
            2
        );

        let mut sb = Mock::new(DaemonKind::Service);
        sb.os = "";
        let err = run(&sb, &spec(), &join(1), &Progress::silent())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("detect"), "{err}");
    }

    #[tokio::test]
    async fn unreachable_after_polls_is_an_error() {
        let sb = Mock::new(DaemonKind::Entrypoint);
        let err = run(&sb, &spec(), &join(99), &Progress::silent())
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("verify:"), "{err}");
    }

    #[test]
    fn pairing_string_forms() {
        assert_eq!(parse_pairing("\"abc\"\n").unwrap(), "abc");
        assert_eq!(parse_pairing("abc").unwrap(), "abc");
        assert!(parse_pairing("\"\"").is_err());
    }
}

#[cfg(test)]
mod vm_tests {
    use super::*;

    fn host() -> HostSide {
        HostSide {
            relay: "https://relay.example".into(),
            tools_policy: Some(serde_json::json!({ "mutating": "ask" })),
            host_name: "mac".into(),
            loopback_mesh: true,
            step_timeout: Duration::from_secs(1),
            pair_tries: 1,
            verify_tries: 1,
            verify_every: Duration::from_millis(1),
        }
    }

    #[test]
    fn guest_config_has_the_contract_keys() {
        let shares = vec![Mount {
            host: "/Users/sky/Develop".into(),
            guest: "/shared/Develop".into(),
            readonly: false,
        }];
        let y = guest_config("lab", &host(), &shares);
        let v: serde_yaml::Value = serde_yaml::from_str(&y).unwrap();
        assert_eq!(v["mesh"]["name"], "lab");
        assert_eq!(v["mesh"]["hub"], "never");
        assert_eq!(v["mesh"]["class"], "vm");
        assert_eq!(v["mesh"]["bind"], "0.0.0.0:7800");
        assert_eq!(v["mesh"]["relay"], "https://relay.example");
        assert_eq!(v["mesh"]["workspaces"][0], "/shared/Develop");
        assert_eq!(v["secrets"]["key_store"], "file");
        assert_eq!(v["link"]["enabled"], false);
        assert_eq!(v["tools"]["policy"]["mutating"], "ask");
        assert_eq!(v["computer"]["enabled"], true);
        assert_eq!(v["sandbox"]["enabled"], false);
        assert_eq!(v["sandbox"]["guest"]["host"], "mac");
        assert_eq!(v["sandbox"]["guest"]["shares"][0], "/shared/Develop");
        let mut h = host();
        h.tools_policy = None;
        let v: serde_yaml::Value = serde_yaml::from_str(&guest_config("lab", &h, &[])).unwrap();
        assert!(v.get("tools").is_none());
    }

    #[test]
    fn loopback_pairing_replaces_addrs_only() {
        use data_encoding::BASE32_NOPAD;
        let v = serde_json::json!({
            "id": "aa", "name": "lab", "relay": "public",
            "addrs": [{ "Ip": "172.16.0.2:7800" }, { "Relay": "https://r/" }],
            "token": "TOK", "vault_key": "KEY"
        });
        let s = format!(
            "1.{}",
            BASE32_NOPAD.encode(&serde_json::to_vec(&v).unwrap())
        );
        let out = loopback_pairing(&s, 61235).unwrap();
        let (ver, body) = out.split_once('.').unwrap();
        assert_eq!(ver, "1");
        let q: serde_json::Value =
            serde_json::from_slice(&BASE32_NOPAD.decode(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(q["addrs"], serde_json::json!([{ "Ip": "127.0.0.1:61235" }]));
        for k in ["id", "name", "relay", "token", "vault_key"] {
            assert_eq!(q[k], v[k], "{k} kept");
        }
        assert!(loopback_pairing("garbage", 1).is_err());
    }
}
