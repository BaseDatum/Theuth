//! The login shell's environment (58 §3.1 `env.rs`): [`login_path`] probes
//! `PATH` for `tools.login_path`; [`login_env`] captures the whole
//! environment for user-cell kernels (171 §5.4).

use std::collections::BTreeMap;
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
    let out = login_output(shell, r#"printf %s "$PATH""#, timeout).await?;
    let path = String::from_utf8_lossy(&out).trim().to_owned();
    if path.is_empty() {
        return Err("printed an empty PATH".into());
    }
    Ok(path)
}

/// 171 §5.4: run `shell -l -i -c 'env -0'` under the same harness as
/// [`login_path`] and parse the NUL-separated `KEY=VALUE` entries (first
/// `=` splits; entries without one or with an empty key are skipped).
/// Windows has no login shell: the process's own environment.
pub async fn login_env(
    shell: &OsStr,
    timeout: Duration,
) -> Result<BTreeMap<String, String>, String> {
    if cfg!(windows) {
        return Ok(std::env::vars().collect());
    }
    let out = login_output(shell, "env -0", timeout).await?;
    let env: BTreeMap<String, String> = out
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (k, v) = entry.split_once('=')?;
            (!k.is_empty()).then(|| (k.to_owned(), v.to_owned()))
        })
        .collect();
    if env.is_empty() {
        return Err("printed no environment".into());
    }
    Ok(env)
}

/// `shell -l -i -c <script>`'s stdout under the harness both probes share.
async fn login_output(shell: &OsStr, script: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::process::own_session(&mut cmd);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(out.stdout),
        Ok(Ok(out)) => Err(format!("exited with {}", out.status)),
        Ok(Err(e)) => Err(format!("could not start: {e}")),
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A fake shell at `<tmp>/<name>/sh` whose body is `script`; removed on drop.
    struct FakeShell(std::path::PathBuf);

    impl FakeShell {
        fn new(name: &str, script: &str) -> Self {
            use std::os::unix::fs::PermissionsExt as _;
            let d =
                std::env::temp_dir().join(format!("platform-env-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join("sh");
            std::fs::write(&p, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self(d)
        }

        fn path(&self) -> std::path::PathBuf {
            self.0.join("sh")
        }
    }

    impl Drop for FakeShell {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn login_env_parses_nul_separated_entries() {
        let sh = FakeShell::new(
            "parse",
            r#"printf 'A=1\0B=two words\0C=x=y\0novalue\0=empty\0'"#,
        );
        // A cap, not a wait: generous so a loaded machine (shared builders) cannot trip it.
        let env = login_env(sh.path().as_os_str(), Duration::from_secs(30))
            .await
            .unwrap();
        let want: BTreeMap<String, String> = [("A", "1"), ("B", "two words"), ("C", "x=y")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(env, want);
    }

    #[tokio::test]
    async fn login_env_times_out() {
        let sh = FakeShell::new("slow", "sleep 5");
        let err = login_env(sh.path().as_os_str(), Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }
}
