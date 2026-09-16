//! `wth --install-service` / `wth --uninstall-service`: run this harness as a
//! background service through the login shell and pair it to a parent as a
//! node (design doc 32 §5.1; the shell argument is 18 §2 with the pty
//! dropped). A launchd LaunchAgent on macOS, a systemd user unit on Linux;
//! anything else gets the `ExecStart` line to place by hand.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `--install-service` options.
pub struct Opts {
    /// `--relay` → `link.relay`; `None` keeps the configured or default relay.
    pub relay: Option<String>,
    /// `--name` → `link.name`; `None` keeps the configured name (`""` → hostname).
    pub name: Option<String>,
    /// Run the pairing step once the port answers (`false` with `--no-pair`).
    pub pair: bool,
}

/// Unit name on both platforms.
const UNIT: &str = "wth";
/// 18 §4: readiness is the port, not the log.
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Steps 2–5 of 32 §5.1 (step 1, `first_run`, ran in `main`).
pub async fn install(
    data_dir: &Path,
    config_path: &Path,
    config: &figment::Figment,
    opts: Opts,
) -> anyhow::Result<()> {
    runtime::write_config_value(config_path, "link", "enabled", "true")?;
    if let Some(relay) = &opts.relay {
        if !(relay.starts_with("wss://") || relay.starts_with("ws://")) {
            anyhow::bail!("--relay must start with ws:// or wss://");
        }
        runtime::write_config_value(config_path, "link", "relay", &format!("{relay:?}"))?;
    }
    if let Some(name) = &opts.name {
        runtime::write_config_value(config_path, "link", "name", &format!("{name:?}"))?;
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    let data_dir = data_dir.canonicalize()?;
    std::fs::create_dir_all(data_dir.join("logs"))?;
    let argv = shell_argv(&shell(std::env::var_os("SHELL")), &exe, &data_dir);
    let scope = install_unit(&argv, &data_dir)?;

    let listen = crate::link_cli::listen(config);
    let client = wait_ready(&data_dir, &listen).await;

    if !opts.pair || !std::io::stdout().is_terminal() {
        println!("service installed: {scope}");
        return Ok(());
    }
    let cfg = config
        .extract_inner::<link::LinkConfig>("link")
        .unwrap_or_default();
    let relay = opts.relay.unwrap_or(cfg.relay);
    let host = relay
        .split_once("://")
        .map_or(relay.as_str(), |(_, h)| h)
        .trim_end_matches('/');
    let name = match opts.name.unwrap_or(cfg.name) {
        n if n.is_empty() => hostname(),
        n => n,
    };
    println!("service installed: {scope} · relay {host} · name {name}");
    println!("pair this harness to your main one — on it, run  /nodes add <key>");
    println!("or paste the key in Settings › Nodes › Add node, or scan with the phone app:");
    crate::link_cli::pair(&client, true).await
}

/// Stop and remove the unit. `data_dir` (identity, `devices.yaml`) is not
/// touched: every pairing survives a re-install.
#[cfg(target_os = "macos")]
pub async fn uninstall() -> anyhow::Result<()> {
    let uid = uid()?;
    run("launchctl", &["bootout", &format!("gui/{uid}/{UNIT}")]);
    remove(&plist_path()?)?;
    println!(
        "removed launchd gui/{uid}/{UNIT} and {}",
        plist_path()?.display()
    );
    Ok(())
}

/// Stop and remove the unit. `data_dir` (identity, `devices.yaml`) is not
/// touched: every pairing survives a re-install.
#[cfg(target_os = "linux")]
pub async fn uninstall() -> anyhow::Result<()> {
    run("systemctl", &["--user", "disable", "--now", UNIT]);
    remove(&systemd_path()?)?;
    run("systemctl", &["--user", "daemon-reload"]);
    println!(
        "removed systemd --user {UNIT} and {}",
        systemd_path()?.display()
    );
    Ok(())
}

/// No unit on this OS: nothing was installed, nothing to remove.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("no service support on this OS; nothing to remove")
}

/// Poll the RPC port with the token until it answers or [`READY_TIMEOUT`]
/// passes; on timeout show the service log's tail and exit 1 (the unit stays).
async fn wait_ready(data_dir: &Path, listen: &str) -> jsonrpsee::ws_client::WsClient {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(c) = crate::link_cli::connect(data_dir, listen).await {
            return c;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!(
        "no answer on {listen} after {}s; the unit is installed, the last of {}:",
        READY_TIMEOUT.as_secs(),
        log_path(data_dir).display()
    );
    let log = std::fs::read_to_string(log_path(data_dir)).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    for l in &lines[lines.len().saturating_sub(20)..] {
        eprintln!("  {l}");
    }
    std::process::exit(1)
}

/// Write, load and start the LaunchAgent; returns the scope for the banner.
#[cfg(target_os = "macos")]
fn install_unit(argv: &[String], data_dir: &Path) -> anyhow::Result<String> {
    let uid = uid()?;
    let path = plist_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, plist(argv, data_dir))?;
    let plist = path.display().to_string();
    run("launchctl", &["bootout", &format!("gui/{uid}/{UNIT}")]);
    if !run("launchctl", &["bootstrap", &format!("gui/{uid}"), &plist]) {
        anyhow::bail!("launchctl bootstrap failed; see `launchctl print gui/{uid}/{UNIT}`");
    }
    println!(
        "a LaunchAgent runs while you are logged in; for a Mac nobody sits at, turn on \
         automatic login (System Settings › Users & Groups › Login Options)"
    );
    Ok(format!("launchd gui/{uid}/{UNIT}"))
}

/// Write, load and start the systemd user unit; returns the scope for the banner.
#[cfg(target_os = "linux")]
fn install_unit(argv: &[String], _data_dir: &Path) -> anyhow::Result<String> {
    let path = systemd_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, systemd_unit(argv))?;
    let user = std::env::var("USER").unwrap_or_default();
    if !run("loginctl", &["enable-linger", &user]) {
        println!(
            "could not enable lingering; without it the service stops at logout. Run: \
             sudo loginctl enable-linger {user}"
        );
    }
    run("systemctl", &["--user", "daemon-reload"]);
    // `enable` + `restart` rather than `enable --now`: a re-run on a running
    // service must pick up the unit and config it just rewrote.
    if !(run("systemctl", &["--user", "enable", UNIT])
        && run("systemctl", &["--user", "restart", UNIT]))
    {
        anyhow::bail!("systemctl --user failed; see `systemctl --user status {UNIT}`");
    }
    Ok(format!("systemd --user {UNIT}"))
}

/// No unit format for this OS (FreeBSD rc.d, OpenRC, Windows): hand over the
/// exec line; `link.enabled` is already set.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_unit(argv: &[String], _data_dir: &Path) -> anyhow::Result<String> {
    println!("no service support on this OS. link.enabled was set; run this at boot:");
    println!(
        "ExecStart={}",
        argv.iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    std::process::exit(2)
}

/// `<data_dir>/logs/service.log`: the unit's stdout and stderr.
fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("service.log")
}

/// `$SHELL` when it is an executable file, else the 18 §4 fallback (which
/// runs without `-i`: `/bin/sh` has no rc file worth sourcing).
fn shell(env_shell: Option<std::ffi::OsString>) -> (PathBuf, bool) {
    if let Some(s) = env_shell.map(PathBuf::from).filter(|p| is_executable(p)) {
        return (s, true);
    }
    let fallback = if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/sh"
    };
    (PathBuf::from(fallback), false)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// `[shell, -l, (-i,) -c, "exec '<exe>' --headless --data-dir '<data_dir>'"]`.
fn shell_argv((shell, interactive): &(PathBuf, bool), exe: &Path, data_dir: &Path) -> Vec<String> {
    let mut v = vec![shell.display().to_string(), "-l".to_owned()];
    if *interactive {
        v.push("-i".to_owned());
    }
    v.push("-c".to_owned());
    v.push(format!(
        "exec {} --headless --data-dir {}",
        sq(exe),
        sq(data_dir)
    ));
    v
}

/// Single-quote for `sh -c`; a literal `'` becomes `'\''`.
fn sq(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

#[cfg(any(target_os = "macos", test))]
fn plist(argv: &[String], data_dir: &Path) -> String {
    fn xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
    let args: String = argv
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml(a)))
        .collect();
    let log = xml(&log_path(data_dir).display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{UNIT}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>WTH_LAUNCH</key>
    <string>1</string>
    <key>TERM</key>
    <string>xterm-256color</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

#[cfg(any(target_os = "linux", test))]
fn systemd_unit(argv: &[String]) -> String {
    // systemd's double-quoted word: `"` and `\` escaped, `'` literal, so the
    // shell sees `'\''` for an apostrophe in a path.
    let (shell, cmd) = argv.split_at(argv.len() - 1);
    let cmd = cmd[0].replace('\\', r"\\").replace('"', r#"\""#);
    format!(
        "[Unit]\nDescription=wth harness\nAfter=network-online.target\n\n[Service]\nExecStart={} \"{cmd}\"\n\
         Environment=WTH_LAUNCH=1 TERM=xterm-256color\nStandardInput=null\nRestart=on-failure\nRestartSec=2\n\n\
         [Install]\nWantedBy=default.target\n",
        shell.join(" ")
    )
}

#[cfg(target_os = "macos")]
fn plist_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .join("Library/LaunchAgents")
        .join(format!("{UNIT}.plist")))
}

#[cfg(target_os = "linux")]
fn systemd_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config dir"))?
        .join("systemd/user")
        .join(format!("{UNIT}.service")))
}

/// The user's uid for `launchctl gui/<uid>`, from `id -u` (no libc dep).
#[cfg(target_os = "macos")]
fn uid() -> anyhow::Result<u32> {
    let out = std::process::Command::new("id").arg("-u").output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse()?)
}

/// Run to completion; `true` on exit 0. Output is the user's to read.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

/// Delete a file, treating "already gone" as done.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// This machine's hostname; `"wth"` when `hostname` fails.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "wth".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv() -> Vec<String> {
        shell_argv(
            &(PathBuf::from("/bin/zsh"), true),
            Path::new("/Applications/My Tools/wth"),
            Path::new("/Users/me/it's here/.wth"),
        )
    }

    #[test]
    fn exec_line_quotes_spaces_and_apostrophes() {
        let a = argv();
        assert_eq!(&a[..4], ["/bin/zsh", "-l", "-i", "-c"]);
        assert_eq!(
            a[4],
            r"exec '/Applications/My Tools/wth' --headless --data-dir '/Users/me/it'\''s here/.wth'"
        );
    }

    #[test]
    fn plist_carries_exec_and_log() {
        let p = plist(&argv(), Path::new("/Users/me/it's here/.wth"));
        assert!(p.contains(
            r"<string>exec '/Applications/My Tools/wth' --headless --data-dir '/Users/me/it'\''s here/.wth'</string>"
        ));
        assert!(p.contains("<string>/Users/me/it's here/.wth/logs/service.log</string>"));
        assert!(p.contains("<key>WTH_LAUNCH</key>\n    <string>1</string>"));
    }

    #[test]
    fn systemd_unit_carries_exec() {
        let u = systemd_unit(&argv());
        assert!(
            u.contains(
                r#"ExecStart=/bin/zsh -l -i -c "exec '/Applications/My Tools/wth' --headless --data-dir '/Users/me/it'\\''s here/.wth'""#
            ),
            "{u}"
        );
        assert!(u.contains("Environment=WTH_LAUNCH=1 TERM=xterm-256color\nStandardInput=null"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn shell_falls_back_without_interactive() {
        let (s, i) = shell(None);
        assert!(s.starts_with("/bin/"));
        assert!(!i);
        let (s, i) = shell(Some("/nonexistent/fish".into()));
        assert!(s.starts_with("/bin/"));
        assert!(!i);
        let (s, i) = shell(Some("/bin/sh".into()));
        assert_eq!(s, PathBuf::from("/bin/sh"));
        assert!(i);
    }
}
