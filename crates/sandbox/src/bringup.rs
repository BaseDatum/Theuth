//! Bring-up (64 §4.4, 67 §13): detect → binary → first run → daemon → pair →
//! verify. Written against [`Sandbox::exec`]/[`Sandbox::put`] only, and
//! against [`MeshJoin`] for the last two steps, so a mock of each runs the
//! whole sequence in a unit test. Every step prints one progress line and
//! fails with a one-line reason.

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;

use crate::{Cmd, DaemonKind, ExecOutput, Handle, Progress, Sandbox, release, sq};

/// How this engine's mesh is reached for the pair and verify steps: the mesh
/// actor in-process ([`crate::ActorJoin`]) or JSON-RPC from the CLI.
#[async_trait::async_trait]
pub trait MeshJoin: Send + Sync {
    /// `mesh::AddPeer(pairing)`.
    async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer>;
    /// `mesh::Status`.
    async fn status(&self) -> anyhow::Result<mesh::MeshStatus>;
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
}

impl Spec {
    /// Defaults: release binary, 6 pair tries, 3 verify tries 10 s apart (64 §4.4 step 6).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            local_t: None,
            pair_tries: 6,
            verify_tries: 3,
            verify_every: Duration::from_secs(10),
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
fn t(args: &str) -> Cmd {
    Cmd::new(format!("PATH=\"{BIN_DIR}:$PATH\" t {args}"))
}

fn fail(step: &str, out: &ExecOutput) -> anyhow::Error {
    anyhow::anyhow!("{step}: {}", out.reason())
}

/// The sequence. Idempotent per step: a present `t` is kept (unless
/// `spec.local_t` is set), an installed service is re-installed in place, an
/// already-paired engine is paired again (mutual pairing is idempotent).
pub async fn run(sb: &dyn Sandbox, spec: &Spec, join: &dyn MeshJoin, out: &Progress) -> anyhow::Result<Done> {
    out.say("create", sb.kind());
    let h = sb.create(out).await?;

    let caps = sb.capabilities(&h).await?;
    out.say("detect", format!("{} {}", caps.os, caps.arch));
    if caps.os == "windows" {
        anyhow::bail!("detect: windows targets are not brought up by this version — install t there and run `t mesh pair`");
    }

    match caps.daemon {
        DaemonKind::Entrypoint => out.say("binary", "in the image"),
        DaemonKind::Service => {
            let files: Vec<(String, Bytes)> = match &spec.local_t {
                Some(p) => {
                    out.say("binary", format!("local {}", p.display()));
                    let b = std::fs::read(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                    vec![("t".into(), Bytes::from(b))]
                }
                None => {
                    let have = sb.exec(&h, t("--version")).await?;
                    if have.ok() {
                        out.say("binary", format!("present: {}", have.stdout_str()));
                        Vec::new()
                    } else {
                        release::fetch(&caps.os, &caps.arch, out).await?
                    }
                }
            };
            for (name, bytes) in files {
                sb.put(&h, &format!("{BIN_DIR}/{name}"), bytes, 0o755).await?;
                out.say("binary", format!("installed {BIN_DIR}/{name}"));
            }
            // Any one-shot runs `first_run` (main's prologue) and proves the binary loads here.
            let fr = sb.exec(&h, t("--secret-list")).await?;
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
                .exec(&h, t(&format!("--install-service --no-pair --name {}", sq(&spec.name))))
                .await?;
            if !inst.ok() {
                return Err(fail("daemon", &inst));
            }
            out.say("daemon", inst.stdout_str());
        }
    }

    let mut pairing = None;
    for i in 0..spec.pair_tries {
        let p = sb.exec(&h, t("mesh pair")).await?;
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
    let peer = join.add_peer(pairing).await.map_err(|e| anyhow::anyhow!("pair: {e:#}"))?;
    out.say("pair", format!("{} ({})", peer.name, peer.id));

    for i in 0..spec.verify_tries {
        let status = join.status().await.map_err(|e| anyhow::anyhow!("verify: {e:#}"))?;
        if status.peers.iter().any(|p| p.id == peer.id && p.reachable) {
            out.say("verify", "connected");
            return Ok(Done { handle: h, peer });
        }
        if i + 1 < spec.verify_tries {
            tokio::time::sleep(spec.verify_every).await;
        }
    }
    anyhow::bail!("verify: {} paired but not reachable yet — check `t mesh status`", peer.name)
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
                if self.have_t { reply(0, "t 1.16.0") } else { reply(127, "") }
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
                peers: vec![peer(*p >= self.reachable_after)],
                lease: mesh::LeaseState {
                    holder: None,
                    since_ms: 0,
                    reason: String::new(),
                },
            })
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
        let tails: Vec<&str> = lines.iter().map(|l| l.rsplit(" t ").next().unwrap()).collect();
        assert_eq!(
            tails,
            ["uname -sm", "--secret-list", "--install-service --no-pair --name 'box'", "mesh pair"]
        );
        assert_eq!(*sb.puts.lock(), vec![(format!("{BIN_DIR}/t"), 0o755, 3)]);
        assert_eq!(*j.added.lock(), vec!["pair-me"]);
        assert_eq!(*j.polls.lock(), 2, "verify polls until reachable");
    }

    #[tokio::test]
    async fn present_binary_is_kept_and_container_skips_install() {
        let mut sb = Mock::new(DaemonKind::Service);
        sb.have_t = true;
        run(&sb, &spec(), &join(1), &Progress::silent()).await.unwrap();
        assert!(sb.puts.lock().is_empty());
        assert!(sb.lines().iter().any(|l| l.ends_with("t --version")));

        let sb = Mock::new(DaemonKind::Entrypoint);
        run(&sb, &spec(), &join(1), &Progress::silent()).await.unwrap();
        let lines = sb.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], "uname -sm");
        assert!(lines[1].ends_with("t mesh pair"));
    }

    #[tokio::test]
    async fn pair_retries_while_daemon_starts_and_windows_is_refused() {
        let mut sb = Mock::new(DaemonKind::Entrypoint);
        sb.pair_fail_first = true;
        run(&sb, &spec(), &join(1), &Progress::silent()).await.unwrap();
        assert_eq!(sb.lines().iter().filter(|l| l.ends_with("t mesh pair")).count(), 2);

        let mut sb = Mock::new(DaemonKind::Service);
        sb.os = "";
        let err = run(&sb, &spec(), &join(1), &Progress::silent()).await.unwrap_err();
        assert!(err.to_string().contains("detect"), "{err}");
    }

    #[tokio::test]
    async fn unreachable_after_polls_is_an_error() {
        let sb = Mock::new(DaemonKind::Entrypoint);
        let err = run(&sb, &spec(), &join(99), &Progress::silent()).await.unwrap_err();
        assert!(err.to_string().starts_with("verify:"), "{err}");
    }

    #[test]
    fn pairing_string_forms() {
        assert_eq!(parse_pairing("\"abc\"\n").unwrap(), "abc");
        assert_eq!(parse_pairing("abc").unwrap(), "abc");
        assert!(parse_pairing("\"\"").is_err());
    }
}
