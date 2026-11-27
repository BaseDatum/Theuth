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
use russh::keys::{
    HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate, known_hosts,
};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{Cmd, DaemonKind, ExecOutput, Handle, Progress, Sandbox};
use platform::shell::single_quote as quote;

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
            let (h, p) = r
                .split_once(']')
                .ok_or_else(|| anyhow::anyhow!("bad target {s}"))?;
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
            .map(|p| {
                p.parse::<u16>()
                    .map_err(|_| anyhow::anyhow!("bad port in {s}"))
            })
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
    identity_file: Option<String>,
    connect_timeout: Option<u64>,
}

impl SshSandbox {
    /// Built-in transport with a private key in OpenSSH / PKCS#8 / PKCS#1 PEM
    /// text (the vault value). Decoded here; the text is the caller's to drop.
    pub fn builtin(target: Target, key_pem: &str) -> anyhow::Result<Self> {
        let key = russh::keys::decode_secret_key(key_pem, None)
            .map_err(|e| anyhow::anyhow!("private key: {e}"))?;
        Ok(Self {
            target,
            transport: Transport::Builtin {
                key: Arc::new(key),
                session: tokio::sync::Mutex::new(None),
            },
            identity_file: None,
            connect_timeout: None,
        })
    }

    /// The system `ssh` binary.
    pub fn system(target: Target) -> Self {
        Self {
            target,
            transport: Transport::System,
            identity_file: None,
            connect_timeout: None,
        }
    }

    /// Pass `-i <path>` to system `ssh`. Path string only — never opened or read here.
    pub fn with_identity_file(mut self, path: impl Into<String>) -> Self {
        let p = path.into();
        if !p.trim().is_empty() {
            self.identity_file = Some(p);
        }
        self
    }

    /// Pass `-o ConnectTimeout=<secs>` to system `ssh`.
    pub fn with_connect_timeout(mut self, secs: u64) -> Self {
        self.connect_timeout = Some(secs);
        self
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
        let mut h = client::connect(
            config,
            (self.target.host.as_str(), self.target.port()),
            check,
        )
        .await
        .map_err(|e| anyhow::anyhow!("connect {}: {e}", self.target.id()))?;
        let hash = h.best_supported_rsa_hash().await?.flatten();
        let res = h
            .authenticate_publickey(
                self.target.user(),
                PrivateKeyWithHashAlg::new(key.clone(), hash),
            )
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
                russh::ChannelMsg::ExtendedData { data, ext: 1 } => {
                    out.stderr.extend_from_slice(&data)
                }
                russh::ChannelMsg::ExitStatus { exit_status } => out.status = exit_status as i32,
                russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
        writer.await??;
        Ok(out)
    }

    async fn exec_system(&self, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let argv = system_ssh_argv(
            &self.target,
            self.identity_file.as_deref(),
            self.connect_timeout,
            &cmd.line,
        );
        let mut c = Command::new("ssh");
        c.args(&argv)
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

/// Build the argv passed to the system `ssh` binary. Pure so tests can assert
/// `BatchMode=yes`, optional `-p` / `-i` / `ConnectTimeout`, and `[user@]host`.
pub fn system_ssh_argv(
    target: &Target,
    identity_file: Option<&str>,
    connect_timeout: Option<u64>,
    cmd_line: &str,
) -> Vec<String> {
    let mut args = vec!["-o".to_owned(), "BatchMode=yes".to_owned()];
    if let Some(secs) = connect_timeout {
        args.push("-o".to_owned());
        args.push(format!("ConnectTimeout={secs}"));
    }
    if let Some(p) = target.port {
        args.push("-p".to_owned());
        args.push(p.to_string());
    }
    if let Some(id) = identity_file.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("-i".to_owned());
        args.push(id.to_owned());
    }
    args.push(match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    });
    args.push("--".to_owned());
    args.push(cmd_line.to_owned());
    args
}

/// Single-quote a string for PowerShell (`'` doubled).
pub fn ps_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Encode a PowerShell script as UTF-16LE base64 for `powershell -NoProfile -EncodedCommand`.
/// Works identically regardless of whether Windows OpenSSH uses `cmd.exe`, `powershell.exe`, or `pwsh.exe`.
pub fn encode_powershell_command(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let encoded = base64::prelude::BASE64_STANDARD.encode(&utf16);
    format!("powershell -NoProfile -NonInteractive -EncodedCommand {encoded}")
}

/// Decode a `powershell ... -EncodedCommand <b64>` command line back into its script (useful in tests).
pub fn decode_powershell_command(cmd_line: &str) -> Option<String> {
    use base64::Engine;
    let (_, b64) = cmd_line.rsplit_once("-EncodedCommand ")?;
    let bytes = base64::prelude::BASE64_STANDARD.decode(b64.trim()).ok()?;
    let (pairs, rem) = bytes.as_chunks::<2>();
    if !rem.is_empty() {
        return None;
    }
    let units: Vec<u16> = pairs.iter().map(|&c| u16::from_le_bytes(c)).collect();
    String::from_utf16(&units).ok()
}

/// True when `path` uses Windows path syntax (`\`, `%ENV%`, or a drive letter like `C:`).
pub fn is_windows_path(path: &str) -> bool {
    path.contains('\\')
        || path.contains('%')
        || (path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':')
}

/// Build the POSIX atomic `put` command (writes `<path>.new`, `chmod`, `mv -f` to avoid `ETXTBSY`).
pub fn posix_put_cmd(path: &str, bytes: Bytes, mode: u32) -> Cmd {
    let dir = path.rsplit_once('/').map_or(".", |(d, _)| d);
    let q = |p: &str| {
        if p.starts_with('$') {
            p.to_owned()
        } else {
            quote(p)
        }
    };
    let tmp = format!("{path}.new");
    let line = format!(
        "mkdir -p {} && cat > {} && chmod {mode:o} {} && mv -f {} {}",
        q(dir),
        q(&tmp),
        q(&tmp),
        q(&tmp),
        q(path)
    );
    Cmd::new(line).stdin(bytes)
}

/// Build a Windows `put` command that decodes base64 from stdin via PowerShell
/// `[IO.File]::WriteAllBytes` and moves it into place (moving a running `.exe` aside to `.old` if needed).
pub fn windows_put_cmd(path: &str, bytes: Bytes) -> Cmd {
    use base64::Engine;
    let b64 = base64::prelude::BASE64_STANDARD.encode(&bytes);
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $p=[Environment]::ExpandEnvironmentVariables({}); \
         $d=[IO.Path]::GetDirectoryName($p); \
         if($d){{[IO.Directory]::CreateDirectory($d)|Out-Null}}; \
         $b64=([Console]::In.ReadToEnd()).Trim(); \
         $tmp=$p+'.new'; \
         [IO.File]::WriteAllBytes($tmp,[Convert]::FromBase64String($b64)); \
         try {{ Move-Item -Force $tmp $p }} catch {{ \
           $old=$p+'.old'; \
           Remove-Item -Force -ErrorAction SilentlyContinue $old; \
           Move-Item -Force $p $old; \
           Move-Item -Force $tmp $p \
         }}",
        ps_single_quote(path)
    );
    Cmd::new(encode_powershell_command(&script))
        .stdin(Bytes::from(b64))
        .redact(format!("put {path}"))
}

/// Host-key policy for the built-in transport: the user's `known_hosts`,
/// learned on first contact, a changed key refused.
struct HostKeyCheck {
    host: String,
    port: u16,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
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
        let h = Handle {
            id: self.target.id(),
        };
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
        let label = cmd.display().to_owned();
        let fut = async {
            match &self.transport {
                Transport::Builtin { key, session } => self.exec_builtin(key, session, cmd).await,
                Transport::System => self.exec_system(cmd).await,
            }
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| anyhow::anyhow!("timed out after {}s: {label}", timeout.as_secs()))?
    }

    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
        let cmd = if is_windows_path(path) {
            windows_put_cmd(path, bytes)
        } else {
            posix_put_cmd(path, bytes, mode)
        };
        let out = self.exec(h, cmd).await?;
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
            .exec(
                h,
                Cmd::new(format!(
                    "PATH=\"{}:$PATH\" t --uninstall-service",
                    crate::bringup::BIN_DIR
                )),
            )
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
        assert_eq!(
            (t.user.as_deref(), t.host.as_str(), t.port),
            (Some("me"), "box.example", Some(2222))
        );
        assert_eq!(t.id(), "me@box.example:2222");
        let t = Target::parse("me@[fe80::1]:22").unwrap();
        assert_eq!(t.host, "fe80::1");
        let t = Target::parse("alias").unwrap();
        assert_eq!((t.user.as_deref(), t.port()), (None, 22));
        assert!(Target::parse("me@").is_err());
        assert!(Target::parse("me@box:x").is_err());
    }

    #[test]
    fn system_ssh_argv_includes_batch_mode_port_identity_and_timeout() {
        let t = Target {
            user: Some("alice".into()),
            host: "box.example".into(),
            port: Some(2222),
        };
        let argv = system_ssh_argv(&t, Some("/home/a/.ssh/id_ed25519"), Some(8), "uname -sm");
        assert_eq!(
            argv,
            vec![
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "-p",
                "2222",
                "-i",
                "/home/a/.ssh/id_ed25519",
                "alice@box.example",
                "--",
                "uname -sm",
            ]
        );

        let alias = Target {
            user: None,
            host: "prod".into(),
            port: None,
        };
        let argv = system_ssh_argv(&alias, None, None, "echo ok");
        assert_eq!(argv, vec!["-o", "BatchMode=yes", "prod", "--", "echo ok"]);
    }

    #[test]
    fn put_commands_handle_posix_and_windows() {
        let p = posix_put_cmd("$HOME/.local/bin/t", Bytes::from_static(b"ELF"), 0o755);
        assert!(p.line.contains("mkdir -p $HOME/.local/bin"));
        assert!(p.line.contains(".new"));
        assert!(p.line.contains("mv -f"));
        assert!(!p.line.contains("sudo"));

        let w = windows_put_cmd(r"%USERPROFILE%\.local\bin\t.exe", Bytes::from_static(b"MZ"));
        assert!(w.line.starts_with("powershell -NoProfile -NonInteractive -EncodedCommand "));
        let decoded = decode_powershell_command(&w.line).unwrap();
        assert!(decoded.contains("WriteAllBytes"));
        assert!(decoded.contains(r"%USERPROFILE%\.local\bin\t.exe"));
        assert_eq!(w.stdin.as_deref(), Some(b"TVo=".as_slice()));
    }
}
