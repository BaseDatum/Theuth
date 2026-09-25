//! The `ssh` backend (64 §4.2): an existing host the user administers, reached
//! by one of two transports chosen per call site —
//!
//! - **built-in** (`russh`): the private key is a vault secret read in-process
//!   and decoded in memory; works from machines without an `ssh` binary.
//!   Host keys: the user's `~/.ssh/known_hosts` — unknown hosts are learned
//!   on first use (fingerprint printed), a changed key is a hard error.
//! - **system**: the `ssh` binary with `BatchMode=yes`, so `~/.ssh/config`,
//!   the agent and `ProxyJump` apply as they do in the user's terminal.
//!
//! `put` is `cat > path` over `exec` with the bytes on stdin, so both
//! transports share one code path and no SFTP subsystem is required.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate, known_hosts};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{Cmd, DaemonKind, ExecOutput, Handle, Progress, Sandbox, sq};

/// `[user@]host[:port]`. What was not given stays `None`: the system `ssh`
/// then takes `User`/`Port` from `~/.ssh/config`; the built-in client uses
/// the local user name and 22.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Login user as given.
    pub user: Option<String>,
    /// Host name or address (or an `~/.ssh/config` alias with the system transport).
    pub host: String,
    /// Port as given.
    pub port: Option<u16>,
}

impl Target {
    /// Parse `[user@]host[:port]`; an IPv6 literal goes in brackets.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();
        let (user, rest) = match s.split_once('@') {
            Some((u, r)) if !u.is_empty() => (Some(u.to_owned()), r),
            _ => (None, s),
        };
        let (host, port) = if let Some(r) = rest.strip_prefix('[') {
            let (h, p) = r.split_once(']').ok_or_else(|| anyhow::anyhow!("bad target {s}"))?;
            (h.to_owned(), p.strip_prefix(':'))
        } else {
            match rest.rsplit_once(':') {
                Some((h, p)) => (h.to_owned(), Some(p)),
                None => (rest.to_owned(), None),
            }
        };
        if host.is_empty() {
            anyhow::bail!("bad target {s}: no host");
        }
        let port = port
            .map(|p| p.parse::<u16>().map_err(|_| anyhow::anyhow!("bad port in {s}")))
            .transpose()?;
        Ok(Self { user, host, port })
    }

    /// The user for the built-in client: as given, else the local user.
    pub fn user(&self) -> String {
        self.user.clone().unwrap_or_else(|| {
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "root".into())
        })
    }

    /// The port for the built-in client: as given, else 22.
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(22)
    }

    /// `user@host:port` with the defaults filled in.
    pub fn id(&self) -> String {
        format!("{}@{}:{}", self.user(), self.host, self.port())
    }
}

enum Transport {
    Builtin {
        key: Arc<PrivateKey>,
        session: tokio::sync::Mutex<Option<client::Handle<HostKeyCheck>>>,
    },
    System,
}

/// One SSH host.
pub struct SshSandbox {
    target: Target,
    transport: Transport,
}

impl SshSandbox {
    /// Built-in transport with a private key in OpenSSH / PKCS#8 / PKCS#1 PEM
    /// text (the vault value). Decoded here; the text is the caller's to drop.
    pub fn builtin(target: Target, key_pem: &str) -> anyhow::Result<Self> {
        let key = russh::keys::decode_secret_key(key_pem, None).map_err(|e| anyhow::anyhow!("private key: {e}"))?;
        Ok(Self {
            target,
            transport: Transport::Builtin {
                key: Arc::new(key),
                session: tokio::sync::Mutex::new(None),
            },
        })
    }

    /// The system `ssh` binary.
    pub fn system(target: Target) -> Self {
        Self {
            target,
            transport: Transport::System,
        }
    }

    /// The target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    async fn connect(&self, key: &Arc<PrivateKey>) -> anyhow::Result<client::Handle<HostKeyCheck>> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(120)),
            keepalive_interval: Some(Duration::from_secs(15)),
            ..Default::default()
        });
        let check = HostKeyCheck {
            host: self.target.host.clone(),
            port: self.target.port(),
        };
        let mut h = client::connect(config, (self.target.host.as_str(), self.target.port()), check)
            .await
            .map_err(|e| anyhow::anyhow!("connect {}: {e}", self.target.id()))?;
        let hash = h.best_supported_rsa_hash().await?.flatten();
        let res = h
            .authenticate_publickey(self.target.user(), PrivateKeyWithHashAlg::new(key.clone(), hash))
            .await?;
        if !res.success() {
            anyhow::bail!("{}: the key was not accepted", self.target.id());
        }
        Ok(h)
    }

    async fn exec_builtin(
        &self,
        key: &Arc<PrivateKey>,
        session: &tokio::sync::Mutex<Option<client::Handle<HostKeyCheck>>>,
        cmd: Cmd,
    ) -> anyhow::Result<ExecOutput> {
        let mut guard = session.lock().await;
        if guard.as_ref().is_none_or(client::Handle::is_closed) {
            *guard = Some(self.connect(key).await?);
        }
        let h = guard.as_ref().expect("connected above");
        let channel = h.channel_open_session().await?;
        channel.exec(true, cmd.line.as_bytes()).await?;
        let (mut rx, tx) = channel.split();
        // Writer on its own task: `data` waits on the SSH window, which only
        // refills while the read loop below drains the channel.
        let stdin = cmd.stdin.unwrap_or_default();
        let writer = tokio::spawn(async move {
            tx.data_bytes(stdin).await?;
            tx.eof().await
        });
        let mut out = ExecOutput {
            status: -1,
            ..Default::default()
        };
        loop {
            let Some(msg) = rx.wait().await else { break };
            match msg {
                russh::ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                russh::ChannelMsg::ExtendedData { data, ext: 1 } => out.stderr.extend_from_slice(&data),
                russh::ChannelMsg::ExitStatus { exit_status } => out.status = exit_status as i32,
                russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
        writer.await??;
        Ok(out)
    }

    async fn exec_system(&self, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let mut c = Command::new("ssh");
        c.arg("-o")
            .arg("BatchMode=yes")
            .args(self.target.port.iter().flat_map(|p| ["-p".to_owned(), p.to_string()]))
            .arg(match &self.target.user {
                Some(u) => format!("{u}@{}", self.target.host),
                None => self.target.host.clone(),
            })
            .arg("--")
            .arg(&cmd.line)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = c.spawn().map_err(|e| anyhow::anyhow!("ssh: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped");
        let bytes = cmd.stdin.unwrap_or_default();
        let feed = tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
        let out = child.wait_with_output().await?;
        feed.await?;
        Ok(ExecOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

/// Host-key policy for the built-in transport: the user's `known_hosts`,
/// learned on first contact, a changed key refused.
struct HostKeyCheck {
    host: String,
    port: u16,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = server_key.public_key();
        if known_hosts::check_known_hosts(&self.host, self.port, &key)? {
            return Ok(true);
        }
        tracing::info!(
            "sandbox ssh: learning host key for {}:{} ({})",
            self.host,
            self.port,
            key.fingerprint(HashAlg::Sha256)
        );
        known_hosts::learn_known_hosts(&self.host, self.port, &key)?;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl Sandbox for SshSandbox {
    fn kind(&self) -> &'static str {
        "ssh"
    }

    async fn create(&self, out: &Progress) -> anyhow::Result<Handle> {
        let h = Handle { id: self.target.id() };
        let probe = self.exec(&h, Cmd::new("echo ok")).await?;
        if !probe.ok() {
            anyhow::bail!("{}: {}", self.target.id(), probe.reason());
        }
        out.say(
            "create",
            format!(
                "{} answers ({})",
                self.target.id(),
                match self.transport {
                    Transport::Builtin { .. } => "built-in ssh",
                    Transport::System => "system ssh",
                }
            ),
        );
        Ok(h)
    }

    async fn exec(&self, _: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let timeout = cmd.timeout;
        let line = cmd.line.clone();
        let fut = async {
            match &self.transport {
                Transport::Builtin { key, session } => self.exec_builtin(key, session, cmd).await,
                Transport::System => self.exec_system(cmd).await,
            }
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| anyhow::anyhow!("timed out after {}s: {line}", timeout.as_secs()))?
    }

    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
        let dir = path.rsplit_once('/').map_or(".", |(d, _)| d);
        // Paths are used unquoted where they start with `$HOME` so the shell expands them.
        let q = |p: &str| if p.starts_with('$') { p.to_owned() } else { sq(p) };
        let line = format!("mkdir -p {} && cat > {} && chmod {mode:o} {}", q(dir), q(path), q(path));
        let out = self.exec(h, Cmd::new(line).stdin(bytes)).await?;
        if !out.ok() {
            anyhow::bail!("put {path}: {}", out.reason());
        }
        Ok(())
    }

    fn durable_roots(&self) -> Vec<String> {
        vec!["$HOME/.theuth".into()]
    }

    fn daemon(&self) -> DaemonKind {
        DaemonKind::Service
    }

    /// `preserve_data: false` also removes the data dir; the service unit goes either way.
    async fn destroy(&self, h: &Handle, preserve_data: bool) -> anyhow::Result<()> {
        let un = self
            .exec(h, Cmd::new(format!("PATH=\"{}:$PATH\" t --uninstall-service", crate::bringup::BIN_DIR)))
            .await?;
        if !un.ok() {
            tracing::warn!("uninstall-service on {}: {}", h.id, un.reason());
        }
        if !preserve_data {
            let rm = self.exec(h, Cmd::new("rm -rf \"$HOME/.theuth\"")).await?;
            if !rm.ok() {
                anyhow::bail!("destroy {}: {}", h.id, rm.reason());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_forms() {
        let t = Target::parse("me@box.example:2222").unwrap();
        assert_eq!((t.user.as_deref(), t.host.as_str(), t.port), (Some("me"), "box.example", Some(2222)));
        assert_eq!(t.id(), "me@box.example:2222");
        let t = Target::parse("me@[fe80::1]:22").unwrap();
        assert_eq!(t.host, "fe80::1");
        let t = Target::parse("alias").unwrap();
        assert_eq!((t.user.as_deref(), t.port()), (None, 22));
        assert!(Target::parse("me@").is_err());
        assert!(Target::parse("me@box:x").is_err());
    }
}
