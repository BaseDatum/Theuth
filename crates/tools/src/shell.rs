//! `shell` tool backed by an embedded `brush_core::Shell` per session. Every
//! call is a background job on the [`Jobs`] actor (14 §0): the reply is the
//! output if it exits within the grace window, else a stub naming the job.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brush_core::{ProfileLoadBehavior, RcLoadBehavior, Shell, ShellVariable};
use kameo::actor::ActorRef;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::activity;
use crate::jobs::{Jobs, PutBack, Start, StartReply, lock};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Interactive-only builtins we never register.
const SKIPPED_BUILTINS: &[&str] = &["bind", "history", "fc", "complete", "compgen", "compopt"];

/// Arguments for [`ShellTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// Bash command line. State (cwd, variables, functions) persists across
    /// calls within a session.
    pub command: String,
    /// Working directory for this call only (absolute, or relative to the
    /// session cwd); use `cd` in `command` to change the persistent cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Kill deadline in seconds for this call only; default and maximum:
    /// the configured job timeout (1800).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Exported variables for this call only; not persisted.
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

/// Runs bash in-process; one persistent shell per session.
///
/// Shell state is last-finished-wins (14 §0.5): each job runs on a clone of
/// the session shell and, when it exits, that clone replaces the stored one.
/// Sequential `cd` / `export` persist; concurrent jobs are independent
/// snapshots and whichever exits later wins. A call with `cwd` or `env` runs
/// on a clone that is never put back, so nothing from it persists.
pub struct ShellTool {
    jobs: ActorRef<Jobs>,
    sessions: Arc<Mutex<HashMap<SessionId, Shell>>>,
    /// `tools.login_path`: seed new shells' `PATH` from [`login_path`].
    login_path: bool,
}

impl ShellTool {
    /// Construct with the actor that runs the jobs; `login_path` is
    /// `tools.login_path`.
    pub fn new(jobs: ActorRef<Jobs>, login_path: bool) -> Self {
        Self {
            jobs,
            sessions: Arc::default(),
            login_path,
        }
    }

    /// The per-session shells, shared so `start` (22 §1.5) can default a PTY
    /// job's cwd to the session shell's.
    pub fn shells(&self) -> Arc<Mutex<HashMap<SessionId, Shell>>> {
        Arc::clone(&self.sessions)
    }

    /// A fresh non-interactive shell at the process cwd. Profile and rc files
    /// are skipped (speed, determinism); with `login_path`, `PATH` comes from
    /// [`login_path`] instead so tools installed by the user's rc are found.
    pub(crate) async fn build_shell(login_path: bool) -> anyhow::Result<Shell> {
        let builtins = brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode)
            .into_iter()
            .filter(|(name, _)| !SKIPPED_BUILTINS.contains(&name.as_str()));
        let mut shell = Shell::builder()
            .builtins(builtins)
            .profile(ProfileLoadBehavior::Skip)
            .rc(RcLoadBehavior::Skip)
            .interactive(false)
            .working_dir(std::env::current_dir()?)
            .build()
            .await?;
        if login_path && let Some(path) = self::login_path().await {
            export(&mut shell, "PATH", path)?;
        }
        Ok(shell)
    }

    /// A clone of the session shell, built on first use.
    async fn session(&self, id: SessionId) -> anyhow::Result<Shell> {
        if let Some(shell) = lock(&self.sessions).get(&id) {
            return Ok(shell.clone());
        }
        let shell = Self::build_shell(self.login_path).await?;
        // Two concurrent first calls race here; both jobs put their shell
        // back and the later exit wins, like any concurrent pair.
        Ok(lock(&self.sessions).entry(id).or_insert(shell).clone())
    }
}

/// Set an exported variable on `shell`.
fn export(shell: &mut Shell, name: &str, value: &str) -> anyhow::Result<()> {
    let mut var = ShellVariable::new(value);
    var.export();
    shell
        .set_env_global(name, var)
        .map_err(|e| anyhow::anyhow!("env {name}: {e}"))
}

/// Env var that turns [`login_path`] off (tests, CI).
pub const NO_LOGIN_PATH: &str = "THEUTH_NO_LOGIN_PATH";
/// How long the login shell may take to print its `PATH`.
const LOGIN_PATH_TIMEOUT: Duration = Duration::from_secs(3);

/// `PATH` as the user's login shell sets it, computed once per process:
/// `$SHELL -l -i -c 'printf %s "$PATH"'` (fallback `/bin/zsh` on macOS,
/// `/bin/bash` elsewhere) with a [`LOGIN_PATH_TIMEOUT`], stdin from
/// `/dev/null`, stderr discarded. `None` (keep the inherited `PATH`) when
/// [`NO_LOGIN_PATH`] is `1`, or the probe fails, times out, or prints
/// nothing — warned once.
pub async fn login_path() -> Option<&'static str> {
    static PATH: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();
    PATH.get_or_init(|| {
        resolve_login_path(std::env::var_os(NO_LOGIN_PATH).is_some_and(|v| v == "1"))
    })
    .await
    .as_deref()
}

/// [`login_path`] before caching: `None` when `skip`, else the probe.
async fn resolve_login_path(skip: bool) -> Option<String> {
    if skip {
        return None;
    }
    probe_login_path().await
}

/// Spawn the login shell and read its `PATH`.
async fn probe_login_path() -> Option<String> {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        });
    let run = tokio::process::Command::new(&shell)
        .args(["-l", "-i", "-c", r#"printf %s "$PATH""#])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let shell = shell.to_string_lossy();
    match tokio::time::timeout(LOGIN_PATH_TIMEOUT, run).await {
        Ok(Ok(out)) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if path.is_empty() {
                tracing::warn!(%shell, "login shell printed an empty PATH; keeping the inherited PATH");
                return None;
            }
            Some(path)
        }
        Ok(Ok(out)) => {
            tracing::warn!(%shell, status = %out.status, "login shell failed; keeping the inherited PATH");
            None
        }
        Ok(Err(e)) => {
            tracing::warn!(%shell, error = %e, "login shell could not start; keeping the inherited PATH");
            None
        }
        Err(_) => {
            tracing::warn!(%shell, timeout = ?LOGIN_PATH_TIMEOUT, "login shell timed out; keeping the inherited PATH");
            None
        }
    }
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    const RISK: Risk = Risk::Destructive;
    type Args = ShellArgs;

    fn description(&self) -> String {
        "Run a bash command as a background job. Returns the output if it finishes within a few seconds, otherwise a job id and log path; the result is delivered later as <job_result>. cwd, variables and functions persist between calls; `cwd`, `env` and `timeout_secs` apply to one call only.".into()
    }

    async fn call(&self, args: ShellArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let mut shell = self.session(cx.session).await?;
        let per_call = args.cwd.is_some() || args.env.is_some();
        if let Some(cwd) = &args.cwd {
            let target = if cwd.is_absolute() {
                cwd.clone()
            } else {
                shell.working_dir().join(cwd)
            };
            let meta = tokio::fs::metadata(&target).await.map_err(|e| {
                anyhow::anyhow!(
                    "cwd {}: {e}; pass an existing directory (relative paths resolve against the session cwd {})",
                    target.display(),
                    shell.working_dir().display()
                )
            })?;
            anyhow::ensure!(
                meta.is_dir(),
                "cwd {}: not a directory; pass a directory, not a file",
                target.display()
            );
            shell
                .set_working_dir(&target)
                .map_err(|e| anyhow::anyhow!("cwd {}: {e}", target.display()))?;
        }
        for (name, value) in args.env.iter().flatten() {
            export(&mut shell, name, value)?;
        }
        let sessions = Arc::clone(&self.sessions);
        let session = cx.session;
        // A clone carrying per-call overrides is discarded, so neither the
        // overrides nor a `cd` inside the command persist.
        let put_back: PutBack = if per_call {
            Arc::new(|_| {})
        } else {
            Arc::new(move |shell| {
                lock(&sessions).insert(session, shell);
            })
        };
        let started = Instant::now();
        let reply = self
            .jobs
            .ask(Start {
                session,
                command: args.command,
                call_id: cx.call_id,
                quiet: cx.quiet,
                shell,
                put_back,
                root: cx.resources.root.clone(),
                // This invocation stays admitted until the reply: the job's
                // own admission happens under it, with no gap (47 §4.2 rule 3).
                activity: activity::current(),
                timeout: args.timeout_secs.map(Duration::from_secs),
            })
            .await
            .map_err(|e| anyhow::anyhow!("jobs actor: {e}"))?;
        Ok(match reply {
            StartReply::Done(out) => out,
            StartReply::Running { id, log, tail } => ToolOutput {
                content: format!(
                    "job {id} started {}s ago, still running.\nlog: {}\noutput so far:\n{}\nResult arrives as <job_result id=\"{id}\"> when it exits — keep working, or end the turn and it will start a new one. `wait` is for PTY jobs only; do not call it on {id}.",
                    started.elapsed().as_secs(),
                    log.display(),
                    tail.trim_end()
                ),
                is_error: false,
                structured: Some(serde_json::json!({ "job": id, "log": log })),
                files: Vec::new(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::tests::{Fx, fixture};

    fn args(command: &str) -> ShellArgs {
        ShellArgs {
            command: command.into(),
            cwd: None,
            timeout_secs: None,
            env: None,
        }
    }

    async fn harness(grace: u64, timeout: u64) -> (Fx, ShellTool, ToolCx) {
        let fx = fixture(Duration::from_secs(grace), Duration::from_secs(timeout)).await;
        let tool = ShellTool::new(fx.jobs.clone(), false);
        let cx = ToolCx {
            session: fx.session,
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&fx.dir),
        };
        (fx, tool, cx)
    }

    async fn run(tool: &ShellTool, cx: &ToolCx, a: ShellArgs) -> anyhow::Result<ToolOutput> {
        Tool::call(tool, a, cx.clone()).await
    }

    fn session_cwd() -> String {
        std::env::current_dir().unwrap().display().to_string()
    }

    #[tokio::test]
    async fn cwd_override_applies_to_one_call_only() {
        let (_fx, tool, cx) = harness(10, 60).await;
        let mut a = args("pwd; cd /");
        a.cwd = Some("/tmp".into());
        let out = run(&tool, &cx, a).await.unwrap();
        assert_eq!(out.content, "/tmp\nexit code: 0");
        let out = run(&tool, &cx, args("pwd")).await.unwrap();
        assert_eq!(out.content, format!("{}\nexit code: 0", session_cwd()));
    }

    #[tokio::test]
    async fn relative_cwd_resolves_against_session_cwd_and_missing_errors() {
        let (fx, tool, cx) = harness(10, 60).await;
        let sub = fx.dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let out = run(&tool, &cx, args(&format!("cd {}", fx.dir.display())))
            .await
            .unwrap();
        assert_eq!(out.content, "exit code: 0");
        let mut a = args("pwd");
        a.cwd = Some("sub".into());
        let out = run(&tool, &cx, a).await.unwrap();
        assert_eq!(out.content, format!("{}\nexit code: 0", sub.display()));
        let mut a = args("pwd");
        a.cwd = Some("nope".into());
        let e = run(&tool, &cx, a).await.unwrap_err().to_string();
        assert!(
            e.starts_with(&format!("cwd {}: ", sub.with_file_name("nope").display())),
            "{e}"
        );
        assert!(e.contains("session cwd"), "{e}");
        let mut a = args("pwd");
        std::fs::write(sub.join("file"), "").unwrap();
        a.cwd = Some(sub.join("file"));
        let e = run(&tool, &cx, a).await.unwrap_err().to_string();
        assert!(e.contains("not a directory"), "{e}");
    }

    #[tokio::test]
    async fn env_override_is_exported_and_not_persisted() {
        let (_fx, tool, cx) = harness(10, 60).await;
        let mut a = args("echo \"$THEUTH_T=$(sh -c 'printf %s \"$THEUTH_T\"')\"; export STICKY=1");
        a.env = Some([("THEUTH_T".to_owned(), "v".to_owned())].into());
        let out = run(&tool, &cx, a).await.unwrap();
        assert_eq!(out.content, "v=v\nexit code: 0");
        let out = run(&tool, &cx, args("echo \"[$THEUTH_T][$STICKY]\""))
            .await
            .unwrap();
        assert_eq!(out.content, "[][]\nexit code: 0");
    }

    #[tokio::test]
    async fn timeout_secs_kills_and_names_the_limit() {
        let (_fx, tool, cx) = harness(10, 60).await;
        let mut a = args("sleep 5");
        a.timeout_secs = Some(1);
        let started = Instant::now();
        let out = run(&tool, &cx, a).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "timed out after 1s (timeout_secs=1); process group killed"
        );
    }

    #[tokio::test]
    async fn timeout_secs_is_capped_at_the_configured_maximum() {
        let (fx, tool, cx) = harness(10, 2).await;
        let mut a = args("sleep 10");
        a.timeout_secs = Some(100);
        let out = run(&tool, &cx, a).await.unwrap();
        assert_eq!(
            out.content,
            "timed out after 2s (timeout_secs=100 capped to 2); process group killed"
        );
        assert_eq!(crate::jobs::tests::list(&fx).await[0].state, "timeout");
    }

    #[tokio::test]
    async fn login_path_off_keeps_the_inherited_path() {
        // `THEUTH_NO_LOGIN_PATH=1` short-circuits before the probe: nothing
        // to seed, so a new shell keeps the inherited `PATH`.
        assert_eq!(resolve_login_path(true).await, None);
        let shell = ShellTool::build_shell(false).await.unwrap();
        assert_eq!(
            shell.env_str("PATH").unwrap(),
            std::env::var("PATH").unwrap()
        );
    }

    #[tokio::test]
    async fn login_shell_path_contains_bin() {
        let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) else {
            return;
        };
        if !std::path::Path::new(&shell).exists() {
            return;
        }
        // The probe may exceed its 3 s budget on a loaded machine; then it
        // reports `None` by design and there is nothing to check.
        if let Some(path) = probe_login_path().await {
            assert!(
                path.split(':').any(|p| p == "/bin" || p == "/usr/bin"),
                "{path}"
            );
        }
    }
}
