//! `t --install-service` / `t --uninstall-service`: run this harness as a
//! background service and pair it to a parent as a node (design doc 32 §5.1).
//! The unit is [`platform::service::ServiceUnit`]'s (58 §0.6): a launchd
//! LaunchAgent, a systemd user unit or a Task Scheduler logon task, running
//! `exec t --headless` through the login shell on Unix (18 §2 with the pty
//! dropped) and `t.exe --headless` directly on Windows; anything else gets
//! the `ExecStart` line to place by hand.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use platform::service::{ServiceUnit, log_path};

/// `--install-service` options.
pub struct Opts {
    /// `--relay` → `link.relay`; `None` keeps the configured or default relay.
    pub relay: Option<String>,
    /// `--name` → `link.name`; `None` keeps the configured name (`""` → hostname).
    pub name: Option<String>,
    /// Run the pairing step once the port answers (`false` with `--no-pair`).
    pub pair: bool,
}

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
    let argv = argv(&exe, &data_dir);
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
        n if n.is_empty() => platform::process::hostname().to_owned(),
        n => n,
    };
    println!("service installed: {scope} · relay {host} · name {name}");
    println!("pair this harness to your main one — on it, run  /nodes add <key>");
    println!("or paste the key in Settings › Nodes › Add node, or scan with the phone app:");
    crate::link_cli::pair(&client, true).await
}

/// Stop and remove the unit. `data_dir` (identity, `devices.yaml`) is not
/// touched: every pairing survives a re-install.
pub async fn uninstall() -> anyhow::Result<()> {
    println!("removed {}", ServiceUnit::current().uninstall()?);
    Ok(())
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

/// Write, load and start the unit; returns the scope for the banner. With
/// no unit format for this OS (FreeBSD rc.d, OpenRC): hand over the exec
/// line; `link.enabled` is already set.
fn install_unit(argv: &[String], data_dir: &Path) -> anyhow::Result<String> {
    let unit = ServiceUnit::current();
    if unit == ServiceUnit::None {
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
    let installed = unit.install(argv, data_dir)?;
    if let Some(note) = installed.note {
        println!("{note}");
    }
    Ok(installed.scope)
}

/// The unit's command line: `exec t --headless` through the login shell on
/// Unix so the daemon inherits the login environment; `t.exe --headless`
/// directly on Windows, which has no login-shell environment (58 §0.3).
fn argv(exe: &Path, data_dir: &Path) -> Vec<String> {
    if cfg!(windows) {
        return vec![
            exe.display().to_string(),
            "--headless".to_owned(),
            "--data-dir".to_owned(),
            data_dir.display().to_string(),
        ];
    }
    shell_argv(&shell(std::env::var_os("SHELL")), exe, data_dir)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv() -> Vec<String> {
        shell_argv(
            &(PathBuf::from("/bin/zsh"), true),
            Path::new("/Applications/My Tools/t"),
            Path::new("/Users/me/it's here/.theuth"),
        )
    }

    #[test]
    fn exec_line_quotes_spaces_and_apostrophes() {
        let a = argv();
        assert_eq!(&a[..4], ["/bin/zsh", "-l", "-i", "-c"]);
        assert_eq!(
            a[4],
            r"exec '/Applications/My Tools/t' --headless --data-dir '/Users/me/it'\''s here/.theuth'"
        );
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
