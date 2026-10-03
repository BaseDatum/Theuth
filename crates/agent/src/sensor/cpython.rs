//! The CPython path (40 §4.4): `python3 <sensors>/.shim.py <sensor.py>` as a
//! child in its own process group / Job Object at low priority, the request
//! on stdin, the result JSON on stdout's last line, killed as a tree at the
//! deadline. No tool bridge and no secrets: a CPython sensor talks to the
//! world with its own libraries.

use std::path::Path;
use std::process::Stdio;

use platform::process::ProcessTree;
use serde_json::Value;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

use super::{Call, SensorError};

/// Characters of the child's output kept as the strike tail (`exit 3: <tail>`).
const TAIL_CHARS: usize = 300;

pub(super) async fn run(python: &Path, shim: &Path, call: &Call<'_>) -> Result<Value, SensorError> {
    let Call {
        meta,
        fn_name,
        state,
        args,
        cwd,
        timeout,
    } = *call;
    let input = serde_json::json!({
        "fn": fn_name,
        "state": state.cloned().unwrap_or(Value::Null),
        "args": args,
    });
    // ponytail: `nice(1)` in front instead of `setpriority` — no libc here;
    // it execs python in place, so pid, group and session are unchanged.
    let mut cmd = if cfg!(unix) {
        let mut c = Command::new("nice");
        c.arg("-n").arg("10").arg(python);
        c
    } else {
        Command::new(python)
    };
    if cfg!(windows) && python.file_stem().is_some_and(|s| s == "py") {
        cmd.arg("-3");
    }
    cmd.arg(shim)
        .arg(&meta.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let cwd = cwd.map_or_else(std::env::temp_dir, Path::to_path_buf);
    cmd.current_dir(&cwd);
    for (k, v) in args {
        cmd.env(format!("THEUTH_SENSOR_{}", k.to_ascii_uppercase()), v);
    }
    cmd.env("THEUTH_SENSOR_CWD", &cwd)
        .env("THEUTH_SENSOR_TIMEOUT", timeout.as_secs().to_string())
        .env("THEUTH_SENSOR_HOSTS", meta.hosts.join(","));
    if fn_name == "probe" {
        cmd.env("THEUTH_SENSOR_PROBE", "1");
    }
    // Unix: `setsid`, so the child leads its own group and `killpg` reaches
    // every descendant. Windows: a Job Object with kill-on-close, adopted
    // below; `BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW`.
    platform::process::own_session(&mut cmd);
    #[cfg(windows)]
    cmd.creation_flags(0x4000 | 0x0800_0000);
    let mut child = cmd
        .spawn()
        .map_err(|e| SensorError::Failed(format!("{}: {e}", python.display())))?;
    let tree = child.id().and_then(|pid| ProcessTree::adopt(pid).ok());
    if let Some(mut stdin) = child.stdin.take() {
        // A shim that exits before reading is reported by its status, not here.
        let _ = stdin.write_all(input.to_string().as_bytes()).await;
    }
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(SensorError::Failed(format!("{}: {e}", meta.name))),
        Err(_) => {
            // The leader dies with the dropped child (`kill_on_drop`); the
            // group / job takes its descendants (40 §5: descendants included).
            if let Some(t) = &tree {
                t.kill();
            }
            return Err(SensorError::Timeout(timeout));
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let text = if stderr.trim().is_empty() {
            &stdout
        } else {
            &stderr
        };
        let tail: String = text
            .trim_end()
            .chars()
            .rev()
            .take(TAIL_CHARS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let code = out
            .status
            .code()
            .map_or_else(|| "signal".to_owned(), |c| c.to_string());
        return Err(SensorError::Failed(format!(
            "exit {code}: {}",
            tools::secrets::redact(&tail)
        )));
    }
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(last)
        .map_err(|e| SensorError::Malformed(format!("shim output is not JSON ({e}): {last:.200}")))
}
