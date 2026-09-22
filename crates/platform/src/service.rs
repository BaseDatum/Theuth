//! Per-user service units (58 §0.6): a launchd LaunchAgent, a systemd user
//! unit, a Task Scheduler logon task. Moved from `theuth::service` (32 §5.1).

use std::path::{Path, PathBuf};

/// Unit name on every platform.
pub const UNIT: &str = "theuth";

/// The unit format this OS uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceUnit {
    /// `~/Library/LaunchAgents/theuth.plist`, `launchctl bootstrap gui/<uid>`.
    LaunchAgent,
    /// `~/.config/systemd/user/theuth.service`, `systemctl --user`.
    SystemdUser,
    /// `schtasks /create /sc onlogon /tn theuth`, running as the current user.
    TaskScheduler,
    /// FreeBSD rc.d, OpenRC, …: the caller hands over the exec line.
    None,
}

/// What an install reports: the scope for the banner and a note for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// `launchd gui/501/theuth`, `systemd --user theuth`, `Task Scheduler theuth`.
    pub scope: String,
    /// Something the user should know now (login-only, no lingering).
    pub note: Option<String>,
}

impl ServiceUnit {
    /// This OS's format.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::LaunchAgent
        } else if cfg!(target_os = "linux") {
            Self::SystemdUser
        } else if cfg!(windows) {
            Self::TaskScheduler
        } else {
            Self::None
        }
    }

    /// Write, (re)load and start the unit running `argv`; the unit's stdout
    /// and stderr go to `<data_dir>/logs/service.log` where the format
    /// allows. `Err` for [`ServiceUnit::None`].
    pub fn install(self, argv: &[String], data_dir: &Path) -> anyhow::Result<Installed> {
        match self {
            Self::LaunchAgent => {
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
                Ok(Installed {
                    scope: format!("launchd gui/{uid}/{UNIT}"),
                    note: Some(
                        "a LaunchAgent runs while you are logged in; for a Mac nobody sits at, turn on \
                         automatic login (System Settings › Users & Groups › Login Options)"
                            .to_owned(),
                    ),
                })
            }
            Self::SystemdUser => {
                let path = systemd_path()?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&path, systemd_unit(argv))?;
                let user = std::env::var("USER").unwrap_or_default();
                let note = (!run("loginctl", &["enable-linger", &user])).then(|| {
                    format!(
                        "could not enable lingering; without it the service stops at logout. Run: \
                         sudo loginctl enable-linger {user}"
                    )
                });
                run("systemctl", &["--user", "daemon-reload"]);
                // `enable` + `restart` rather than `enable --now`: a re-run on a running
                // service must pick up the unit and config it just rewrote.
                if !(run("systemctl", &["--user", "enable", UNIT])
                    && run("systemctl", &["--user", "restart", UNIT]))
                {
                    anyhow::bail!("systemctl --user failed; see `systemctl --user status {UNIT}`");
                }
                Ok(Installed {
                    scope: format!("systemd --user {UNIT}"),
                    note,
                })
            }
            // ponytail: unverified on Windows — step 4's check: `t --install-service`
            // creates a logon task and the daemon answers RPC after logoff/logon.
            Self::TaskScheduler => {
                let tr = argv.iter().map(|a| quote_win(a)).collect::<Vec<_>>().join(" ");
                run("schtasks", &["/end", "/tn", UNIT]);
                if !run(
                    "schtasks",
                    &["/create", "/f", "/sc", "onlogon", "/rl", "limited", "/tn", UNIT, "/tr", &tr],
                ) {
                    anyhow::bail!("schtasks /create failed; see `schtasks /query /tn {UNIT} /v`");
                }
                if !run("schtasks", &["/run", "/tn", UNIT]) {
                    anyhow::bail!("schtasks /run failed; see `schtasks /query /tn {UNIT} /v`");
                }
                Ok(Installed {
                    scope: format!("Task Scheduler {UNIT}"),
                    note: Some(
                        "a logon task runs while you are signed in; the service log is \
                         where the daemon writes it, not in Task Scheduler"
                            .to_owned(),
                    ),
                })
            }
            Self::None => anyhow::bail!("no service support on this OS"),
        }
    }

    /// Stop and remove the unit; returns what was removed. Data is untouched.
    pub fn uninstall(self) -> anyhow::Result<String> {
        match self {
            Self::LaunchAgent => {
                let uid = uid()?;
                run("launchctl", &["bootout", &format!("gui/{uid}/{UNIT}")]);
                remove(&plist_path()?)?;
                Ok(format!("launchd gui/{uid}/{UNIT} and {}", plist_path()?.display()))
            }
            Self::SystemdUser => {
                run("systemctl", &["--user", "disable", "--now", UNIT]);
                remove(&systemd_path()?)?;
                run("systemctl", &["--user", "daemon-reload"]);
                Ok(format!("systemd --user {UNIT} and {}", systemd_path()?.display()))
            }
            Self::TaskScheduler => {
                run("schtasks", &["/end", "/tn", UNIT]);
                if !run("schtasks", &["/delete", "/f", "/tn", UNIT]) {
                    anyhow::bail!("schtasks /delete failed; see `schtasks /query /tn {UNIT}`");
                }
                Ok(format!("Task Scheduler {UNIT}"))
            }
            Self::None => anyhow::bail!("no service support on this OS; nothing to remove"),
        }
    }
}

/// Whether a service manager started this process (39 §3.4: it may exit for
/// a restart): parent pid 1 or systemd's `INVOCATION_ID` on Unix. Windows:
/// never, until the Task Scheduler path is verified (58 step 4).
pub fn managed() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: getppid takes no arguments and cannot fail.
        let init = unsafe { libc::getppid() } == 1;
        init || std::env::var_os("INVOCATION_ID").is_some()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// `<data_dir>/logs/service.log`: the unit's stdout and stderr.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("service.log")
}

/// The LaunchAgent for `argv` (32 §5.1).
pub fn plist(argv: &[String], data_dir: &Path) -> String {
    fn xml(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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
    <key>THEUTH_LAUNCH</key>
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

/// The systemd user unit for `argv`, whose last element is the `-c` script.
pub fn systemd_unit(argv: &[String]) -> String {
    // systemd's double-quoted word: `"` and `\` escaped, `'` literal, so the
    // shell sees `'\''` for an apostrophe in a path.
    let (shell, cmd) = argv.split_at(argv.len() - 1);
    let cmd = cmd[0].replace('\\', r"\\").replace('"', r#"\""#);
    format!(
        "[Unit]\nDescription=theuth harness\nAfter=network-online.target\n\n[Service]\nExecStart={} \"{cmd}\"\n\
         Environment=THEUTH_LAUNCH=1 TERM=xterm-256color\nStandardInput=null\nRestart=on-failure\nRestartSec=2\n\n\
         [Install]\nWantedBy=default.target\n",
        shell.join(" ")
    )
}

/// One `schtasks /tr` word: double-quoted when it has a space or quote.
fn quote_win(a: &str) -> String {
    if a.contains([' ', '"']) {
        format!("\"{}\"", a.replace('"', "\\\""))
    } else {
        a.to_owned()
    }
}

fn plist_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .join("Library/LaunchAgents")
        .join(format!("{UNIT}.plist")))
}

fn systemd_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config dir"))?
        .join("systemd/user")
        .join(format!("{UNIT}.service")))
}

/// The user's uid for `launchctl gui/<uid>`, from `id -u`.
fn uid() -> anyhow::Result<u32> {
    let out = std::process::Command::new("id").arg("-u").output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse()?)
}

/// Run to completion; `true` on exit 0. Output is the user's to read.
fn run(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

/// Delete a file, treating "already gone" as done.
fn remove(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv() -> Vec<String> {
        [
            "/bin/zsh",
            "-l",
            "-i",
            "-c",
            r"exec '/Applications/My Tools/t' --headless --data-dir '/Users/me/it'\''s here/.theuth'",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn plist_carries_exec_and_log() {
        let p = plist(&argv(), Path::new("/Users/me/it's here/.theuth"));
        assert!(p.contains(
            r"<string>exec '/Applications/My Tools/t' --headless --data-dir '/Users/me/it'\''s here/.theuth'</string>"
        ));
        assert!(p.contains("<string>/Users/me/it's here/.theuth/logs/service.log</string>"));
        assert!(p.contains("<key>THEUTH_LAUNCH</key>\n    <string>1</string>"));
    }

    #[test]
    fn systemd_unit_carries_exec() {
        let u = systemd_unit(&argv());
        assert!(
            u.contains(
                r#"ExecStart=/bin/zsh -l -i -c "exec '/Applications/My Tools/t' --headless --data-dir '/Users/me/it'\\''s here/.theuth'""#
            ),
            "{u}"
        );
        assert!(u.contains("Environment=THEUTH_LAUNCH=1 TERM=xterm-256color\nStandardInput=null"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn schtasks_command_line_quotes_paths_with_spaces() {
        let words = [r"C:\Program Files\theuth\t.exe", "--headless", "--data-dir", r"C:\Users\me\.theuth"]
            .map(quote_win);
        assert_eq!(
            words.join(" "),
            r#""C:\Program Files\theuth\t.exe" --headless --data-dir C:\Users\me\.theuth"#
        );
    }
}
