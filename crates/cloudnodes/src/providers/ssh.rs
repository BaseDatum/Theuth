//! SSH provider: install a headless theuth engine natively on a machine the user already has
//! SSH access to, over the system `ssh` binary (`BatchMode=yes`).
//!
//! - Parses `~/.ssh/config` (with depth-limited `Include` strictly under `~/.ssh/`) without ever
//!   opening private key files (`IdentityFile` is a path string only).
//! - Downloads and minisign-verifies the signed release on THIS engine (`sandbox::release`),
//!   uploads `t` (or `t.exe`) over SSH, runs `t --install-service --no-pair` as the SSH login
//!   user, and pairs it with `t mesh add <ticket>` (redacted in all logs and errors).
//! - Never uses `sudo`, never creates an `engine` user, never installs Docker, never opens
//!   inbound ports, never touches `sshd`, and never runs `userdel`.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use platform::shell::single_quote as quote;
use sandbox::bringup::{BIN_DIR, WIN_BIN_DIR, windows_t_cmd};
use sandbox::ssh::{SshSandbox, Target, encode_powershell_command, ps_single_quote};
use sandbox::{Cmd, Handle, Progress, Sandbox};

use crate::types::{ProviderKind, SshHostEntry, SshHosts, SshOptions, SshProbe, Stage};

use super::{CloudError, CreateReq, Created, Listed, Pace, Provider, ProviderStatus, Result, Say};

/// Maximum recursion depth for `Include` directives in `~/.ssh/config`.
const MAX_INCLUDE_DEPTH: usize = 4;

/// Short timeout for `cloud_ssh_probe` and `status` checks so the UI never hangs.
const PROBE_CONNECT_TIMEOUT_SECS: u64 = 8;
const PROBE_CMD_TIMEOUT: Duration = Duration::from_secs(12);
const STATUS_CONNECT_TIMEOUT_SECS: u64 = 5;
const STATUS_CMD_TIMEOUT: Duration = Duration::from_secs(8);
const INSTALL_CMD_TIMEOUT: Duration = Duration::from_secs(120);
const ACTION_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Injectable release fetcher for unit tests without hitting the network.
pub type ReleaseFetcher = Arc<
    dyn Fn(&str, &str) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<(String, Bytes)>>> + Send>>
        + Send
        + Sync,
>;

/// True when a system `ssh` executable is found on `PATH`.
pub fn ssh_available() -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let names: &[&str] = if cfg!(windows) {
        &["ssh.exe", "ssh"]
    } else {
        &["ssh"]
    };
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let p = dir.join(name);
            if is_executable_file(&p) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// System `ssh` availability and concrete hosts parsed from `<home>/.ssh/config`.
pub fn hosts(home: Option<&Path>) -> SshHosts {
    SshHosts {
        ssh_available: ssh_available(),
        hosts: home.map(parse_ssh_config).unwrap_or_default(),
    }
}

/// Parse `<home>/.ssh/config` (and depth-limited `Include` directives strictly under
/// `<home>/.ssh/`). Skips wildcard (`*`, `?`) and negated (`!`) `Host` patterns. Never opens
/// private key files (`IdentityFile` is returned as a path string only).
pub fn parse_ssh_config(home: &Path) -> Vec<SshHostEntry> {
    let ssh_dir = home.join(".ssh");
    let Ok(canon_ssh_dir) = ssh_dir.canonicalize() else {
        return Vec::new();
    };
    let config_path = ssh_dir.join("config");
    let mut visited = HashSet::new();
    let mut entries: Vec<SshHostEntry> = Vec::new();
    parse_config_file(
        &config_path,
        home,
        &canon_ssh_dir,
        0,
        &mut visited,
        &mut entries,
    );
    entries
}

fn parse_config_file(
    path: &Path,
    home: &Path,
    canon_ssh_dir: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    entries: &mut Vec<SshHostEntry>,
) {
    let Ok(canon) = path.canonicalize() else {
        return;
    };
    if !canon.starts_with(canon_ssh_dir) || !canon.is_file() {
        return;
    }
    if !visited.insert(canon.clone()) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&canon) else {
        return;
    };
    let mut active: Vec<usize> = Vec::new();

    for raw_line in text.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, rest)) = split_directive(line) else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "include" => {
                if depth < MAX_INCLUDE_DEPTH {
                    for pattern in split_tokens(rest) {
                        for inc in resolve_include_pattern(&pattern, home, canon_ssh_dir) {
                            parse_config_file(
                                &inc,
                                home,
                                canon_ssh_dir,
                                depth + 1,
                                visited,
                                entries,
                            );
                        }
                    }
                }
            }
            "host" => {
                active.clear();
                for pat in split_tokens(rest) {
                    if pat.is_empty()
                        || pat.contains('*')
                        || pat.contains('?')
                        || pat.starts_with('!')
                    {
                        continue;
                    }
                    let idx = if let Some(pos) = entries.iter().position(|e| e.alias == pat) {
                        pos
                    } else {
                        entries.push(SshHostEntry {
                            alias: pat,
                            hostname: None,
                            user: None,
                            port: None,
                            identity: None,
                            proxy_jump: None,
                        });
                        entries.len() - 1
                    };
                    if !active.contains(&idx) {
                        active.push(idx);
                    }
                }
            }
            "match" => {
                active.clear();
            }
            "hostname" => {
                if let Some(val) = first_token(rest) {
                    for &i in &active {
                        if entries[i].hostname.is_none() {
                            entries[i].hostname = Some(val.clone());
                        }
                    }
                }
            }
            "user" => {
                if let Some(val) = first_token(rest) {
                    for &i in &active {
                        if entries[i].user.is_none() {
                            entries[i].user = Some(val.clone());
                        }
                    }
                }
            }
            "port" => {
                if let Some(val) = first_token(rest).and_then(|s| s.parse::<u16>().ok()) {
                    for &i in &active {
                        if entries[i].port.is_none() {
                            entries[i].port = Some(val);
                        }
                    }
                }
            }
            "identityfile" => {
                // Path string only — never open or read the key file.
                if let Some(val) = first_token(rest) {
                    for &i in &active {
                        if entries[i].identity.is_none() {
                            entries[i].identity = Some(val.clone());
                        }
                    }
                }
            }
            "proxyjump" => {
                if let Some(val) = first_token(rest) {
                    for &i in &active {
                        if entries[i].proxy_jump.is_none() {
                            entries[i].proxy_jump = Some(val.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// Split `Key Value` or `Key=Value` or `Key = Value`.
fn split_directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut split_at = None;
    for (idx, ch) in line.char_indices() {
        if ch.is_whitespace() || ch == '=' {
            split_at = Some(idx);
            break;
        }
    }
    let idx = split_at?;
    let key = &line[..idx];
    let rest = line[idx..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    if key.is_empty() || rest.is_empty() {
        return None;
    }
    Some((key, rest))
}

fn split_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn first_token(s: &str) -> Option<String> {
    split_tokens(s).into_iter().next()
}

/// Resolve an `Include` pattern strictly under `canon_ssh_dir`.
/// Rejects any `..` traversal component and any file outside `~/.ssh/`.
fn resolve_include_pattern(pattern: &str, home: &Path, canon_ssh_dir: &Path) -> Vec<PathBuf> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Vec::new();
    }
    if Path::new(pat)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Vec::new();
    }
    let expanded = if let Some(rest) = pat.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(pat).is_absolute() {
        PathBuf::from(pat)
    } else {
        canon_ssh_dir.join(pat)
    };
    if expanded
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Vec::new();
    }

    let expanded_str = expanded.to_string_lossy();
    if !expanded_str.contains('*') && !expanded_str.contains('?') {
        return match expanded.canonicalize() {
            Ok(c) if c.starts_with(canon_ssh_dir) && c.is_file() => vec![c],
            _ => Vec::new(),
        };
    }

    // Simple single-level glob on the filename component (e.g. `config.d/*` or `config.d/*.conf`).
    let Some(parent) = expanded.parent() else {
        return Vec::new();
    };
    let Some(file_pat) = expanded.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let Ok(canon_parent) = parent.canonicalize() else {
        return Vec::new();
    };
    if !canon_parent.starts_with(canon_ssh_dir) {
        return Vec::new();
    }
    let Ok(read_dir) = std::fs::read_dir(&canon_parent) else {
        return Vec::new();
    };
    let mut matches: Vec<PathBuf> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name_str = name.to_str()?;
            if !glob_match(file_pat, name_str) {
                return None;
            }
            let c = e.path().canonicalize().ok()?;
            (c.starts_with(canon_ssh_dir) && c.is_file()).then_some(c)
        })
        .collect();
    matches.sort();
    matches
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Validate and convert [`SshOptions`] into a [`sandbox::ssh::Target`].
pub fn build_target(opts: &SshOptions) -> Result<Target> {
    let host = opts.host.trim();
    if host.is_empty() {
        return Err(CloudError("SSH host is required".into()));
    }
    if host.starts_with('-') || host.chars().any(char::is_whitespace) {
        return Err(CloudError(format!("invalid SSH host {host:?}")));
    }
    let user = opts
        .user
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(u) = &user
        && (u.starts_with('-') || u.chars().any(char::is_whitespace))
    {
        return Err(CloudError(format!("invalid SSH user {u:?}")));
    }
    if let Some(id) = opts
        .identity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && id.starts_with('-')
    {
        return Err(CloudError("invalid SSH identity path".into()));
    }
    Ok(Target {
        user,
        host: host.to_owned(),
        port: opts.port,
    })
}

/// Build a system-`ssh` [`SshSandbox`] from [`SshOptions`]. Never reads `identity`.
pub fn build_sandbox(opts: &SshOptions, connect_timeout: Option<u64>) -> Result<SshSandbox> {
    let target = build_target(opts)?;
    let mut sb = SshSandbox::system(target);
    if let Some(id) = opts
        .identity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sb = sb.with_identity_file(id);
    }
    if let Some(secs) = connect_timeout {
        sb = sb.with_connect_timeout(secs);
    }
    Ok(sb)
}

/// Human-readable OS/arch string for progress lines (e.g. `"Linux x86_64"`).
pub fn friendly_os_label(os: &str, arch: &str) -> String {
    let os_label = match os {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    };
    format!("{os_label} {arch}")
}

/// Build the `t --install-service --no-pair --name <name>` command (POSIX or Windows, no `sudo`).
pub fn install_service_cmd(os: &str, name: &str, timeout: Duration) -> Cmd {
    if os == "windows" {
        windows_t_cmd(&["--install-service", "--no-pair", "--name", name], timeout)
    } else {
        Cmd::new(format!(
            "PATH=\"{BIN_DIR}:$PATH\" t --install-service --no-pair --name {}",
            quote(name)
        ))
        .timeout(timeout)
    }
}

/// Build the `t mesh add <ticket>` command (POSIX or Windows, always redacted).
pub fn mesh_add_cmd(os: &str, ticket: &str, timeout: Duration) -> Cmd {
    let cmd = if os == "windows" {
        windows_t_cmd(&["mesh", "add", ticket], timeout)
    } else {
        Cmd::new(format!(
            "PATH=\"{BIN_DIR}:$PATH\" t mesh add {}",
            quote(ticket)
        ))
        .timeout(timeout)
    };
    cmd.redact("t mesh add <redacted>")
}

/// Build the `t mesh status` command (best-effort to read remote `self_id`).
pub fn mesh_status_cmd(os: &str, timeout: Duration) -> Cmd {
    if os == "windows" {
        windows_t_cmd(&["mesh", "status"], timeout)
    } else {
        Cmd::new(format!("PATH=\"{BIN_DIR}:$PATH\" t mesh status")).timeout(timeout)
    }
}

/// Build the service restart command matching the unit created by `t --install-service`.
pub fn restart_cmd(os: &str, timeout: Duration) -> Cmd {
    match os {
        "macos" => Cmd::new("launchctl kickstart -k \"gui/$(id -u)/theuth\"").timeout(timeout),
        "windows" => {
            let script = "$ErrorActionPreference='Continue'; \
                          schtasks /end /tn theuth 2>$null | Out-Null; \
                          schtasks /run /tn theuth; \
                          exit $LASTEXITCODE";
            Cmd::new(encode_powershell_command(script))
                .timeout(timeout)
                .redact("schtasks /run /tn theuth")
        }
        _ => Cmd::new("systemctl --user restart theuth").timeout(timeout),
    }
}

/// Build a cheap service health check command.
pub fn status_cmd(os: &str, timeout: Duration) -> Cmd {
    match os {
        "macos" => Cmd::new("launchctl print \"gui/$(id -u)/theuth\" >/dev/null 2>&1")
            .timeout(timeout),
        "windows" => {
            let script = "schtasks /query /tn theuth /fo LIST 2>$null | Out-Null; exit $LASTEXITCODE";
            Cmd::new(encode_powershell_command(script))
                .timeout(timeout)
                .redact("schtasks /query /tn theuth")
        }
        _ => Cmd::new("systemctl --user is-active --quiet theuth").timeout(timeout),
    }
}

/// Build the `t --uninstall-service` command (POSIX or Windows, no `sudo`).
pub fn uninstall_service_cmd(os: &str, timeout: Duration) -> Cmd {
    if os == "windows" {
        windows_t_cmd(&["--uninstall-service"], timeout)
    } else {
        Cmd::new(format!(
            "PATH=\"{BIN_DIR}:$PATH\" t --uninstall-service"
        ))
        .timeout(timeout)
    }
}

/// Build the command that removes the installed `t` binary (and nothing else).
pub fn remove_binary_cmd(os: &str, timeout: Duration) -> Cmd {
    if os == "windows" {
        let script = format!(
            "$p=[Environment]::ExpandEnvironmentVariables({}); \
             Remove-Item -Force -ErrorAction SilentlyContinue $p, ($p+'.old'), ($p+'.new'); \
             exit 0",
            ps_single_quote(&format!(r"{WIN_BIN_DIR}\t.exe"))
        );
        Cmd::new(encode_powershell_command(&script))
            .timeout(timeout)
            .redact(r"remove %USERPROFILE%\.local\bin\t.exe")
    } else {
        Cmd::new(format!("rm -f \"{BIN_DIR}/t\"")).timeout(timeout)
    }
}

/// Build the command that removes the theuth data directory (`~/.theuth` or `%USERPROFILE%\.theuth`).
pub fn remove_data_dir_cmd(os: &str, timeout: Duration) -> Cmd {
    if os == "windows" {
        let script = "$d=[Environment]::ExpandEnvironmentVariables('%USERPROFILE%\\.theuth'); \
                      Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $d; \
                      exit 0";
        Cmd::new(encode_powershell_command(script))
            .timeout(timeout)
            .redact(r"remove %USERPROFILE%\.theuth")
    } else {
        Cmd::new("rm -rf \"$HOME/.theuth\"").timeout(timeout)
    }
}

/// Detect macOS `launchctl bootstrap gui/<uid>` failures caused by a missing GUI login session
/// and return an actionable message (never falling back to a root LaunchDaemon).
pub fn macos_gui_session_error(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let is_gui_err = lower.contains("gui/")
        || lower.contains("bootstrap failed")
        || lower.contains("domain does not support")
        || lower.contains("could not find domain")
        || (lower.contains("launchctl") && lower.contains("input/output error"));
    is_gui_err.then(|| {
        const CANNED: &str = "macOS background service install failed because this user has no active GUI login session \
             (launchctl bootstrap gui/<uid>). Sign in to the macOS desktop once as this user and try again.";
        match output.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
            Some(last) => format!("{CANNED} ({last})"),
            None => CANNED.to_owned(),
        }
    })
}

/// Parse `self_id` from `t mesh status` JSON output (best-effort).
fn parse_mesh_self_id(stdout: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("self_id")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn redact_ticket(msg: &str, ticket: &str) -> String {
    if ticket.is_empty() {
        msg.to_owned()
    } else {
        msg.replace(ticket, "<redacted>")
    }
}

/// Probe an SSH target without installing anything: connects with a short timeout, detects
/// OS/arch, and checks that a signed release is published for it.
pub async fn probe(opts: &SshOptions) -> SshProbe {
    let sb = match build_sandbox(opts, Some(PROBE_CONNECT_TIMEOUT_SECS)) {
        Ok(sb) => sb,
        Err(e) => {
            return SshProbe {
                os: None,
                arch: None,
                ok: false,
                error: Some(e.0),
            };
        }
    };
    probe_with(&sb, PROBE_CMD_TIMEOUT).await
}

/// Testable core of [`probe`] against any [`Sandbox`].
pub async fn probe_with(sb: &dyn Sandbox, timeout: Duration) -> SshProbe {
    let h = Handle { id: "probe".into() };
    let (os, arch) = match sandbox::detect_with_timeout(sb, &h, timeout).await {
        Ok(pair) => pair,
        Err(e) => {
            return SshProbe {
                os: None,
                arch: None,
                ok: false,
                error: Some(format!("{e:#}")),
            };
        }
    };
    if let Err(e) = sandbox::release::validate_target(&os, &arch) {
        return SshProbe {
            os: Some(os),
            arch: Some(arch),
            ok: false,
            error: Some(format!("{e:#}")),
        };
    }
    SshProbe {
        os: Some(os),
        arch: Some(arch),
        ok: true,
        error: None,
    }
}

/// Cloud-node provider that installs `t` natively on an existing machine over system SSH.
pub struct SshProvider {
    opts: SshOptions,
    sandbox: Option<Arc<dyn Sandbox>>,
    fetcher: Option<ReleaseFetcher>,
    pace: Pace,
}

impl SshProvider {
    /// Production provider using the system `ssh` binary and signed release downloads.
    pub fn new(opts: SshOptions) -> Self {
        Self {
            opts,
            sandbox: None,
            fetcher: None,
            pace: Pace {
                every: Duration::from_secs(2),
                tries: 6,
            },
        }
    }

    /// Test constructor with injected sandbox, release fetcher, and retry pace.
    pub fn with_mocks(
        opts: SshOptions,
        sandbox: Arc<dyn Sandbox>,
        fetcher: ReleaseFetcher,
        pace: Pace,
    ) -> Self {
        Self {
            opts,
            sandbox: Some(sandbox),
            fetcher: Some(fetcher),
            pace,
        }
    }

    fn target_label(&self) -> String {
        let host = self.opts.host.trim();
        let mut s = match self
            .opts
            .user
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            Some(u) => format!("{u}@{host}"),
            None => host.to_owned(),
        };
        if let Some(p) = self.opts.port
            && p != 22
        {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    fn open_sandbox(&self, connect_timeout: Option<u64>) -> Result<Arc<dyn Sandbox>> {
        if let Some(sb) = &self.sandbox {
            return Ok(sb.clone());
        }
        Ok(Arc::new(build_sandbox(&self.opts, connect_timeout)?))
    }

    async fn fetch_release(&self, os: &str, arch: &str) -> Result<Vec<(String, Bytes)>> {
        if let Some(f) = &self.fetcher {
            return (f)(os, arch).await.map_err(CloudError::from);
        }
        sandbox::release::fetch(os, arch, &Progress::silent())
            .await
            .map_err(CloudError::from)
    }
}

#[async_trait]
impl Provider for SshProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ssh
    }

    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        let target_label = self.target_label();
        let sb = self.open_sandbox(Some(15))?;
        let h = Handle {
            id: target_label.clone(),
        };

        say(Stage::Server, &format!("Connecting to {target_label}"));
        let (os, arch) = sandbox::detect_with_timeout(&*sb, &h, PROBE_CMD_TIMEOUT)
            .await
            .map_err(CloudError::from)?;
        sandbox::release::validate_target(&os, &arch).map_err(CloudError::from)?;
        say(
            Stage::Server,
            &format!("Detected {}", friendly_os_label(&os, &arch)),
        );

        say(Stage::Install, "Downloading signed release");
        let files = self.fetch_release(&os, &arch).await?;

        say(Stage::Install, "Uploading engine");
        let is_windows = os == "windows";
        let bin_dir = if is_windows { WIN_BIN_DIR } else { BIN_DIR };
        let sep = if is_windows { "\\" } else { "/" };
        for (name, bytes) in files {
            let dest = format!("{bin_dir}{sep}{name}");
            sb.put(&h, &dest, bytes, 0o755)
                .await
                .map_err(CloudError::from)?;
        }

        say(Stage::Install, "Installing background service");
        let inst = sb
            .exec(&h, install_service_cmd(&os, &req.name, INSTALL_CMD_TIMEOUT))
            .await
            .map_err(CloudError::from)?;
        if !inst.ok() {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&inst.stderr),
                String::from_utf8_lossy(&inst.stdout)
            );
            if os == "macos"
                && let Some(msg) = macos_gui_session_error(&combined)
            {
                return Err(CloudError(msg));
            }
            return Err(CloudError(format!(
                "installing background service failed: {}",
                inst.reason()
            )));
        }

        say(Stage::Join, "Joining mesh");
        if !req.ticket.is_empty() {
            let tries = self.pace.tries.max(1);
            let mut last_err = String::new();
            let mut joined_ok = false;
            for attempt in 0..tries {
                let res = sb
                    .exec(&h, mesh_add_cmd(&os, &req.ticket, ACTION_CMD_TIMEOUT))
                    .await
                    .map_err(|e| CloudError(redact_ticket(&format!("{e:#}"), &req.ticket)))?;
                if res.ok() {
                    joined_ok = true;
                    break;
                }
                let reason = redact_ticket(&res.reason(), &req.ticket);
                let lower = reason.to_ascii_lowercase();
                if lower.contains("already paired") || lower.contains("already a member") {
                    joined_ok = true;
                    break;
                }
                last_err = reason;
                if attempt + 1 < tries {
                    self.pace.sleep().await;
                }
            }
            if !joined_ok {
                return Err(CloudError(format!("mesh add failed: {last_err}")));
            }
        }

        // Best-effort: read the remote engine's mesh id so `wait_join` matches even if its
        // hostname differs from `req.name`.
        let self_id = sb
            .exec(&h, mesh_status_cmd(&os, STATUS_CMD_TIMEOUT))
            .await
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| parse_mesh_self_id(&o.stdout_str()));

        Ok(Created {
            server_id: Some(target_label),
            volume_id: None,
            extra: self_id,
            ip: None,
            size_slug: format!("{os}-{arch}"),
        })
    }

    async fn status(&self, _name: &str) -> Result<Option<ProviderStatus>> {
        let sb = self.open_sandbox(Some(STATUS_CONNECT_TIMEOUT_SECS))?;
        let h = Handle {
            id: self.target_label(),
        };
        let Ok((os, arch)) = sandbox::detect_with_timeout(&*sb, &h, STATUS_CMD_TIMEOUT).await
        else {
            return Ok(Some(ProviderStatus {
                server_state: Some("unreachable".into()),
                ..Default::default()
            }));
        };
        let state = match sb.exec(&h, status_cmd(&os, STATUS_CMD_TIMEOUT)).await {
            Ok(out) if out.ok() => "running",
            Ok(_) => "stopped",
            Err(_) => "unreachable",
        };
        Ok(Some(ProviderStatus {
            server_state: Some(state.into()),
            size_slug: Some(format!("{os}-{arch}")),
            ..Default::default()
        }))
    }

    async fn list(&self) -> Result<Vec<Listed>> {
        Ok(Vec::new())
    }

    async fn restart(&self, _name: &str) -> Result<()> {
        let sb = self.open_sandbox(Some(10))?;
        let h = Handle {
            id: self.target_label(),
        };
        let (os, _) = sandbox::detect_with_timeout(&*sb, &h, PROBE_CMD_TIMEOUT)
            .await
            .map_err(CloudError::from)?;
        let out = sb
            .exec(&h, restart_cmd(&os, ACTION_CMD_TIMEOUT))
            .await
            .map_err(CloudError::from)?;
        if !out.ok() {
            return Err(CloudError(format!("restart failed: {}", out.reason())));
        }
        Ok(())
    }

    async fn resize(&self, _name: &str, _memory_gb: u8) -> Result<String> {
        Err(CloudError("Resize is not supported for SSH machines".into()))
    }

    async fn destroy(&self, _name: &str, keep_volume: bool) -> Result<Vec<String>> {
        let sb = self.open_sandbox(Some(10))?;
        let h = Handle {
            id: self.target_label(),
        };
        let (os, _) = sandbox::detect_with_timeout(&*sb, &h, PROBE_CMD_TIMEOUT)
            .await
            .map_err(CloudError::from)?;

        let mut lines = Vec::new();
        let un = sb
            .exec(&h, uninstall_service_cmd(&os, ACTION_CMD_TIMEOUT))
            .await
            .map_err(CloudError::from)?;
        if un.ok() {
            lines.push("Uninstalled theuth service".into());
        } else {
            tracing::warn!(
                "cloudnodes ssh: uninstall-service on {}: {}",
                h.id,
                un.reason()
            );
            lines.push("Service was not installed or already removed".into());
        }

        let rm_bin = sb
            .exec(&h, remove_binary_cmd(&os, ACTION_CMD_TIMEOUT))
            .await
            .map_err(CloudError::from)?;
        if !rm_bin.ok() {
            return Err(CloudError(format!(
                "removing installed binary failed: {}",
                rm_bin.reason()
            )));
        }
        lines.push("Removed installed t binary".into());

        if !keep_volume {
            let rm_data = sb
                .exec(&h, remove_data_dir_cmd(&os, ACTION_CMD_TIMEOUT))
                .await
                .map_err(CloudError::from)?;
            if !rm_data.ok() {
                return Err(CloudError(format!(
                    "removing theuth data directory failed: {}",
                    rm_data.reason()
                )));
            }
            lines.push("Removed theuth data directory".into());
        } else {
            lines.push("Kept theuth data directory".into());
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use sandbox::ssh::{decode_powershell_command, system_ssh_argv};
    use sandbox::{DaemonKind, ExecOutput};

    struct TempHome(PathBuf);
    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl TempHome {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "cloudnodes-ssh-test-{:016x}",
                rand::random::<u64>()
            ));
            std::fs::create_dir_all(p.join(".ssh")).unwrap();
            Self(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }

    #[test]
    fn ssh_config_parser_handles_entries_wildcards_includes_and_never_opens_keys() {
        let h = TempHome::new();
        // Outside file that an Include traversal must NOT read.
        h.write("secret.conf", "Host escaped\n  HostName 10.0.0.9\n");
        // Valid included file under ~/.ssh/config.d/
        h.write(
            ".ssh/config.d/work.conf",
            "Host build-arm\n  HostName 192.168.1.50\n  User builder\n  Port=2200\n  ProxyJump bastion\n",
        );
        // Main config with comments, wildcards, negated host, non-existent key file, and bad includes.
        h.write(
            ".ssh/config",
            r#"
# Top comment
Include config.d/*.conf
Include ../secret.conf
Include /etc/ssh/ssh_config

Host *
  User defaultuser

Host !blocked *.wildcard prod-box web-box
  HostName 203.0.113.10
  User deploy
  Port 2222
  IdentityFile ~/.ssh/does_not_exist_never_opened
  ProxyJump jump.example.com

Match host something
  User ignored
"#,
        );

        let entries = parse_ssh_config(&h.0);
        let aliases: Vec<&str> = entries.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, vec!["build-arm", "prod-box", "web-box"]);

        assert_eq!(entries[0].hostname.as_deref(), Some("192.168.1.50"));
        assert_eq!(entries[0].user.as_deref(), Some("builder"));
        assert_eq!(entries[0].port, Some(2200));
        assert_eq!(entries[0].proxy_jump.as_deref(), Some("bastion"));

        assert_eq!(entries[1].hostname.as_deref(), Some("203.0.113.10"));
        assert_eq!(entries[1].user.as_deref(), Some("deploy"));
        assert_eq!(entries[1].port, Some(2222));
        assert_eq!(
            entries[1].identity.as_deref(),
            Some("~/.ssh/does_not_exist_never_opened")
        );
        assert_eq!(entries[1].proxy_jump.as_deref(), Some("jump.example.com"));
        // Key file does not exist and was never opened.
        assert!(!h.0.join(".ssh/does_not_exist_never_opened").exists());
    }

    #[cfg(unix)]
    #[test]
    fn ssh_config_parser_rejects_symlink_escape_in_include() {
        let h = TempHome::new();
        h.write("outside.conf", "Host escaped\n  HostName 10.9.9.9\n");
        std::os::unix::fs::symlink(h.0.join("outside.conf"), h.0.join(".ssh/link.conf")).unwrap();
        h.write(".ssh/config", "Include link.conf\nHost ok\n  HostName 1.2.3.4\n");
        let entries = parse_ssh_config(&h.0);
        let aliases: Vec<&str> = entries.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, vec!["ok"]);
    }

    #[test]
    fn command_builders_quote_properly_never_use_sudo_and_redact_ticket() {
        let ticket = "1.SECRET_TICKET_VALUE";

        // POSIX commands
        let inst = install_service_cmd("linux", "my-box", Duration::from_secs(30));
        assert!(inst.line.contains("--install-service --no-pair --name 'my-box'"));
        assert!(!inst.line.contains("sudo"));

        let add = mesh_add_cmd("linux", ticket, Duration::from_secs(30));
        assert!(add.line.contains("t mesh add"));
        assert!(add.line.contains(ticket));
        assert!(!add.display().contains(ticket));
        assert!(!add.line.contains("sudo"));

        let un = uninstall_service_cmd("linux", Duration::from_secs(30));
        assert!(un.line.contains("--uninstall-service"));
        assert!(!un.line.contains("sudo"));
        assert!(!un.line.contains("userdel"));
        assert!(!un.line.contains("sshd"));

        let rs_linux = restart_cmd("linux", Duration::from_secs(30));
        assert_eq!(rs_linux.line, "systemctl --user restart theuth");
        let rs_mac = restart_cmd("macos", Duration::from_secs(30));
        assert!(rs_mac.line.contains("launchctl kickstart -k"));
        assert!(!rs_mac.line.contains("sudo"));

        // Windows commands
        let w_inst = install_service_cmd("windows", "win'box", Duration::from_secs(30));
        let w_inst_dec = decode_powershell_command(&w_inst.line).unwrap();
        assert!(w_inst_dec.contains("'--install-service'"));
        assert!(w_inst_dec.contains("'--no-pair'"));
        assert!(w_inst_dec.contains("'win''box'"));

        let w_add = mesh_add_cmd("windows", ticket, Duration::from_secs(30));
        let w_add_dec = decode_powershell_command(&w_add.line).unwrap();
        assert!(w_add_dec.contains("'mesh' 'add'"));
        assert!(!w_add.display().contains(ticket));

        let w_rs = restart_cmd("windows", Duration::from_secs(30));
        let w_rs_dec = decode_powershell_command(&w_rs.line).unwrap();
        assert!(w_rs_dec.contains("schtasks /run /tn theuth"));

        let w_un = uninstall_service_cmd("windows", Duration::from_secs(30));
        let w_un_dec = decode_powershell_command(&w_un.line).unwrap();
        assert!(w_un_dec.contains("'--uninstall-service'"));

        let w_rm = remove_data_dir_cmd("windows", Duration::from_secs(30));
        let w_rm_dec = decode_powershell_command(&w_rm.line).unwrap();
        assert!(w_rm_dec.contains(r"%USERPROFILE%\.theuth"));

        // System ssh argv uses BatchMode=yes
        let t = build_target(&SshOptions {
            host: "prod-box".into(),
            user: Some("alice".into()),
            port: Some(2222),
            identity: Some("~/.ssh/id_ed25519".into()),
            name: None,
        })
        .unwrap();
        let argv = system_ssh_argv(&t, Some("~/.ssh/id_ed25519"), Some(8), "uname -sm");
        assert!(argv.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(argv.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(argv.windows(2).any(|w| w == ["-i", "~/.ssh/id_ed25519"]));
    }

    struct MockSandbox {
        uname_out: ExecOutput,
        ps_out: ExecOutput,
        cmd_out: ExecOutput,
        install_out: ExecOutput,
        mesh_add_out: ExecOutput,
        mesh_status_out: ExecOutput,
        execs: Mutex<Vec<String>>,
        puts: Mutex<Vec<(String, usize)>>,
    }

    impl MockSandbox {
        fn linux() -> Self {
            Self {
                uname_out: ExecOutput {
                    status: 0,
                    stdout: b"Linux x86_64\n".to_vec(),
                    stderr: Vec::new(),
                },
                ps_out: ExecOutput {
                    status: 1,
                    ..Default::default()
                },
                cmd_out: ExecOutput {
                    status: 1,
                    ..Default::default()
                },
                install_out: ExecOutput {
                    status: 0,
                    stdout: b"service installed: user\n".to_vec(),
                    stderr: Vec::new(),
                },
                mesh_add_out: ExecOutput {
                    status: 0,
                    stdout: b"joined\n".to_vec(),
                    stderr: Vec::new(),
                },
                mesh_status_out: ExecOutput {
                    status: 0,
                    stdout: br#"{"self_id":"peer-ssh-1"}"#.to_vec(),
                    stderr: Vec::new(),
                },
                execs: Mutex::new(Vec::new()),
                puts: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Sandbox for MockSandbox {
        fn kind(&self) -> &'static str {
            "mock-ssh"
        }
        async fn create(&self, _: &Progress) -> anyhow::Result<Handle> {
            Ok(Handle { id: "m".into() })
        }
        async fn exec(&self, _: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
            self.execs.lock().push(cmd.line.clone());
            let decoded = decode_powershell_command(&cmd.line);
            let check = decoded.as_deref().unwrap_or(&cmd.line);
            if cmd.line == "uname -sm" {
                Ok(self.uname_out.clone())
            } else if cmd.line.starts_with("powershell ") && cmd.line.contains("OSArchitecture") {
                Ok(self.ps_out.clone())
            } else if cmd.line.starts_with("cmd /c echo %PROCESSOR_ARCHITECTURE%") {
                Ok(self.cmd_out.clone())
            } else if check.contains("--install-service") {
                Ok(self.install_out.clone())
            } else if check.contains("mesh add") || (check.contains("'mesh'") && check.contains("'add'")) {
                Ok(self.mesh_add_out.clone())
            } else if check.contains("mesh status") || (check.contains("'mesh'") && check.contains("'status'")) {
                Ok(self.mesh_status_out.clone())
            } else {
                Ok(ExecOutput {
                    status: 0,
                    ..Default::default()
                })
            }
        }
        async fn put(&self, _: &Handle, path: &str, bytes: Bytes, _: u32) -> anyhow::Result<()> {
            self.puts.lock().push((path.to_owned(), bytes.len()));
            Ok(())
        }
        fn durable_roots(&self) -> Vec<String> {
            vec!["$HOME/.theuth".into()]
        }
        fn daemon(&self) -> DaemonKind {
            DaemonKind::Service
        }
        async fn destroy(&self, _: &Handle, _: bool) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn probe_rejects_darwin_x86_64_and_handles_windows_fallbacks() {
        let mut darwin_intel = MockSandbox::linux();
        darwin_intel.uname_out = ExecOutput {
            status: 0,
            stdout: b"Darwin x86_64\n".to_vec(),
            stderr: Vec::new(),
        };
        let p = probe_with(&darwin_intel, Duration::from_secs(5)).await;
        assert!(!p.ok);
        assert_eq!(p.os.as_deref(), Some("macos"));
        assert_eq!(p.arch.as_deref(), Some("x86_64"));
        assert!(p.error.as_deref().unwrap_or("").contains("macOS Intel"));

        let mut win_ps = MockSandbox::linux();
        win_ps.uname_out = ExecOutput {
            status: 1,
            ..Default::default()
        };
        win_ps.ps_out = ExecOutput {
            status: 0,
            stdout: b"X64\r\n".to_vec(),
            stderr: Vec::new(),
        };
        let p = probe_with(&win_ps, Duration::from_secs(5)).await;
        assert!(p.ok);
        assert_eq!(p.os.as_deref(), Some("windows"));
        assert_eq!(p.arch.as_deref(), Some("x86_64"));

        let mut win_cmd = MockSandbox::linux();
        win_cmd.uname_out = ExecOutput {
            status: 1,
            ..Default::default()
        };
        win_cmd.cmd_out = ExecOutput {
            status: 0,
            stdout: b"ARM64\r\n".to_vec(),
            stderr: Vec::new(),
        };
        let p = probe_with(&win_cmd, Duration::from_secs(5)).await;
        assert!(p.ok);
        assert_eq!(p.os.as_deref(), Some("windows"));
        assert_eq!(p.arch.as_deref(), Some("aarch64"));
    }

    #[tokio::test]
    async fn create_uploads_installs_pairs_and_destroy_respects_keep_volume() {
        let sb = Arc::new(MockSandbox::linux());
        let fetcher: ReleaseFetcher = Arc::new(|os, arch| {
            let os = os.to_owned();
            let arch = arch.to_owned();
            Box::pin(async move {
                assert_eq!((os.as_str(), arch.as_str()), ("linux", "x86_64"));
                Ok(vec![("t".into(), Bytes::from_static(b"ELF-BIN"))])
            })
        });
        let provider = SshProvider::with_mocks(
            SshOptions {
                host: "myserver".into(),
                user: Some("alice".into()),
                port: Some(22),
                identity: None,
                name: None,
            },
            sb.clone(),
            fetcher,
            Pace::NOW,
        );
        let stages = Mutex::new(Vec::new());
        let say = |s: Stage, line: &str| {
            stages.lock().push((s, line.to_owned()));
        };
        let req = CreateReq {
            name: "workbox".into(),
            region: String::new(),
            memory_gb: 0,
            volume_gb: 0,
            ticket: "1.TICKET123".into(),
            manifest: String::new(),
            image: String::new(),
            k8s: None,
            desktop: false,
        };
        let created = provider.create(&req, &say).await.unwrap();
        assert_eq!(created.server_id.as_deref(), Some("alice@myserver"));
        assert_eq!(created.extra.as_deref(), Some("peer-ssh-1"));
        assert_eq!(created.size_slug, "linux-x86_64");
        assert_eq!(*sb.puts.lock(), vec![("$HOME/.local/bin/t".into(), 7)]);
        assert!(
            stages
                .lock()
                .iter()
                .any(|(s, l)| *s == Stage::Server && l == "Detected Linux x86_64")
        );
        // Never logged ticket in progress lines.
        assert!(
            !stages
                .lock()
                .iter()
                .any(|(_, l)| l.contains("1.TICKET123"))
        );

        // Resize unsupported.
        assert!(provider.resize("workbox", 4).await.is_err());

        // Destroy with keep_volume = true must NOT delete ~/.theuth.
        sb.execs.lock().clear();
        let kept = provider.destroy("workbox", true).await.unwrap();
        assert!(kept.iter().any(|l| l.contains("Kept")));
        assert!(!sb.execs.lock().iter().any(|l| l.contains(".theuth")));

        // Destroy with keep_volume = false removes ~/.theuth, never userdel or sshd.
        sb.execs.lock().clear();
        let wiped = provider.destroy("workbox", false).await.unwrap();
        assert!(wiped.iter().any(|l| l.contains("Removed theuth data")));
        let cmds = sb.execs.lock().clone();
        assert!(cmds.iter().any(|l| l.contains("--uninstall-service")));
        assert!(cmds.iter().any(|l| l.contains("rm -f \"$HOME/.local/bin/t\"")));
        assert!(cmds.iter().any(|l| l.contains("rm -rf \"$HOME/.theuth\"")));
        assert!(!cmds.iter().any(|l| l.contains("userdel") || l.contains("sshd") || l.contains("sudo")));
    }

    #[tokio::test]
    async fn macos_launchctl_no_gui_session_returns_clear_error() {
        let mut sb = MockSandbox::linux();
        sb.uname_out = ExecOutput {
            status: 0,
            stdout: b"Darwin arm64\n".to_vec(),
            stderr: Vec::new(),
        };
        sb.install_out = ExecOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: b"launchctl bootstrap gui/501 failed: Bootstrap failed: 5: Input/output error"
                .to_vec(),
        };
        let fetcher: ReleaseFetcher = Arc::new(|_, _| {
            Box::pin(async { Ok(vec![("t".into(), Bytes::from_static(b"MACHO"))]) })
        });
        let provider = SshProvider::with_mocks(
            SshOptions {
                host: "mac-mini".into(),
                ..Default::default()
            },
            Arc::new(sb),
            fetcher,
            Pace::NOW,
        );
        let req = CreateReq {
            name: "macmini".into(),
            region: String::new(),
            memory_gb: 0,
            volume_gb: 0,
            ticket: "1.T".into(),
            manifest: String::new(),
            image: String::new(),
            k8s: None,
            desktop: false,
        };
        let err = provider.create(&req, &|_, _| {}).await.unwrap_err();
        assert!(
            err.0.contains("GUI login session"),
            "expected clear GUI session error, got: {}",
            err.0
        );
        assert!(
            err.0.contains("Bootstrap failed: 5: Input/output error"),
            "expected raw last line appended, got: {}",
            err.0
        );
    }
}
