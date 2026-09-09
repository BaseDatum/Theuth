//! `shell` tool backed by an embedded `brush_core::Shell` per session.
//! See design doc §6 and §11.3 for the capture, timeout and orphan-kill rules.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brush_core::{
    ExecutionParameters, ProcessGroupPolicy, ProfileLoadBehavior, RcLoadBehavior, Shell, SourceInfo,
};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Output longer than this is truncated before it reaches the model.
const MAX_OUTPUT: usize = 64 * 1024;
/// Interactive-only builtins we never register.
const SKIPPED_BUILTINS: &[&str] = &["bind", "history", "fc", "complete", "compgen", "compopt"];

/// Arguments for [`ShellTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// Bash command line. State (cwd, variables, functions) persists across
    /// calls within a session.
    pub command: String,
}

/// Runs bash in-process; one persistent shell per session.
pub struct ShellTool {
    /// Kill the command's process group after this long.
    pub timeout: Duration,
    /// Session shells. The inner lock serializes calls within a session.
    sessions: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<Shell>>>>,
}

impl ShellTool {
    /// Construct with the configured timeout.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    async fn build_shell() -> anyhow::Result<Shell> {
        let builtins = brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode)
            .into_iter()
            .filter(|(name, _)| !SKIPPED_BUILTINS.contains(&name.as_str()));
        Ok(Shell::builder()
            .builtins(builtins)
            .profile(ProfileLoadBehavior::Skip)
            .rc(RcLoadBehavior::Skip)
            .interactive(false)
            .working_dir(std::env::current_dir()?)
            .build()
            .await?)
    }

    async fn session(&self, id: SessionId) -> anyhow::Result<Arc<tokio::sync::Mutex<Shell>>> {
        if let Some(shell) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
        {
            return Ok(shell.clone());
        }
        let shell = Arc::new(tokio::sync::Mutex::new(Self::build_shell().await?));
        // Two concurrent first calls race here; the second one wins and the first
        // shell is dropped after its call, at worst losing that call's cwd/env.
        Ok(self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id)
            .or_insert(shell)
            .clone())
    }
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n[output truncated at 64 KiB]");
    }
    s
}

fn render(stdout: &[u8], stderr: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(stdout).into_owned();
    if !stderr.is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&String::from_utf8_lossy(stderr));
    }
    s
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    const RISK: Risk = Risk::Destructive;
    type Args = ShellArgs;

    fn description(&self) -> String {
        "Run a bash command. cwd, variables and functions persist between calls. Output is stdout+stderr with the exit code.".into()
    }

    async fn call(&self, args: ShellArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let shell = self.session(cx.session).await?;
        let mut shell = shell.lock().await;

        let (out_r, out_w) = std::io::pipe()?;
        let (err_r, err_w) = std::io::pipe()?;
        let pgids: Arc<Mutex<Vec<i32>>> = Arc::default();
        let mut params: ExecutionParameters = shell.default_exec_params();
        // Never hand the TUI's terminal to a child: with a tty on fd 0 brush
        // has the (new-group) child `tcsetpgrp` itself in the foreground, which
        // gets it SIGTTOU-stopped under the TUI and hangs the turn.
        params.set_fd(0, brush_core::openfiles::null()?);
        params.set_fd(1, out_w.into());
        params.set_fd(2, err_w.into());
        params.process_group_policy = ProcessGroupPolicy::NewProcessGroup;
        params.on_spawn = Some(Arc::new({
            let pgids = pgids.clone();
            move |_pid, pgid| {
                if let Some(pgid) = pgid {
                    pgids.lock().unwrap_or_else(|e| e.into_inner()).push(pgid);
                }
            }
        }));

        let drain = |mut r: std::io::PipeReader| {
            tokio::task::spawn_blocking(move || {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                buf
            })
        };
        let out_task = drain(out_r);
        let err_task = drain(err_r);

        let source = SourceInfo::from("shell");
        let result = tokio::time::timeout(
            self.timeout,
            shell.run_string(args.command.as_str(), &source, &params),
        )
        .await;
        drop(params); // closes our pipe writers so the drains can finish
        drop(shell);

        let (result, killed) = match result {
            Ok(r) => (Some(r), Vec::new()),
            Err(_) => {
                let mut pgids =
                    std::mem::take(&mut *pgids.lock().unwrap_or_else(|e| e.into_inner()));
                pgids.sort_unstable();
                pgids.dedup(); // pipeline members share the first member's group
                for &pgid in &pgids {
                    let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
                }
                (None, pgids)
            }
        };

        // ponytail: a child that survived the kill (setsid'd itself) keeps the pipe
        // open; give up on its output after a second rather than hang the turn.
        let (stdout, stderr) = match tokio::time::timeout(
            Duration::from_secs(1),
            futures::future::join(out_task, err_task),
        )
        .await
        {
            Ok((o, e)) => (o.unwrap_or_default(), e.unwrap_or_default()),
            Err(_) => (Vec::new(), Vec::new()),
        };
        let body = render(&stdout, &stderr);

        Ok(match result {
            None => {
                let groups = killed
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let sep = if body.is_empty() { "" } else { "\n" };
                ToolOutput::err(truncate(format!(
                    "timed out after {}s; killed process group(s) [{groups}]{sep}{body}",
                    self.timeout.as_secs()
                )))
            }
            Some(Err(e)) => ToolOutput::err(truncate(format!("{body}\nshell error: {e}"))),
            Some(Ok(r)) => {
                let code = u8::from(r.exit_code);
                let sep = if body.is_empty() || body.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                ToolOutput {
                    content: truncate(format!("{body}{sep}exit code: {code}")),
                    is_error: code != 0,
                }
            }
        })
    }
}
