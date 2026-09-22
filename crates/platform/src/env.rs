//! The login shell's environment (58 §3.1 `env.rs`). Only `PATH` is
//! consumed today (`tools.login_path`), so only `PATH` is probed.

use std::ffi::OsStr;
use std::time::Duration;

/// Run `shell -l -i -c 'printf %s "$PATH"'` once, stdin from `/dev/null`,
/// stderr discarded, in its own session (a `zsh -i` that can reach the tty
/// takes its foreground away from the TUI; a new process group alone still
/// shares the session's tty). `Err` says why it gave no usable `PATH`.
/// Windows has no login-shell environment problem: the current `PATH`.
pub async fn login_path(shell: &OsStr, timeout: Duration) -> Result<String, String> {
    if cfg!(windows) {
        return std::env::var("PATH").map_err(|e| e.to_string());
    }
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", r#"printf %s "$PATH""#])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::process::own_session(&mut cmd);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if path.is_empty() {
                return Err("printed an empty PATH".into());
            }
            Ok(path)
        }
        Ok(Ok(out)) => Err(format!("exited with {}", out.status)),
        Ok(Err(e)) => Err(format!("could not start: {e}")),
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
    }
}
