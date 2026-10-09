//! `shell` tool backed by an embedded `brush_core::Shell` per session. Every
//! call is a background job on the [`Jobs`] actor (14 §0): the reply is the
//! output if it exits within the grace window, else a stub naming the job.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
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
    /// `tools.login_path`. With it on, the login-shell `PATH` probe starts
    /// now (on the current Tokio runtime) so it is usually done before the
    /// first `shell` call has to wait for it.
    pub fn new(jobs: ActorRef<Jobs>, login_path: bool) -> Self {
        if login_path {
            tokio::spawn(self::login_path());
        }
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
            export(&mut shell, "PATH", &path)?;
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
/// How long the login shell may take to print its `PATH`: rc files on a
/// loaded machine have been measured at 7 s.
const LOGIN_PATH_TIMEOUT: Duration = Duration::from_secs(10);
/// Failed probes per process before [`login_path`] gives up.
const LOGIN_PATH_ATTEMPTS: u32 = 3;

/// Process-wide probe state behind [`login_path`].
static PROBE: Mutex<Probe> = Mutex::new(Probe::Idle { attempts: 0 });

enum Probe {
    /// No probe running; `attempts` have failed so far.
    Idle { attempts: u32 },
    /// One probe in flight; every caller awaits the same cell.
    Pending {
        attempts: u32,
        cell: Arc<tokio::sync::OnceCell<Option<String>>>,
    },
    /// A successful probe: cached for the rest of the process.
    Ready(String),
    /// [`LOGIN_PATH_ATTEMPTS`] failures: the inherited `PATH` stays.
    GaveUp,
}

/// Observable form of the probe state (tests, diagnostics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginPathState {
    /// Never probed.
    Idle,
    /// A probe is running.
    Pending,
    /// The login shell's `PATH`.
    Ready(String),
    /// This many probes failed; `attempts == 3` means no more will run.
    Failed {
        /// Failed probes so far.
        attempts: u32,
    },
}

/// Current state of the process-wide probe.
pub fn login_path_state() -> LoginPathState {
    state_of(&PROBE)
}

fn state_of(probe: &Mutex<Probe>) -> LoginPathState {
    match &*lock(probe) {
        Probe::Idle { attempts: 0 } => LoginPathState::Idle,
        Probe::Idle { attempts } => LoginPathState::Failed {
            attempts: *attempts,
        },
        Probe::Pending { .. } => LoginPathState::Pending,
        Probe::Ready(p) => LoginPathState::Ready(p.clone()),
        Probe::GaveUp => LoginPathState::Failed {
            attempts: LOGIN_PATH_ATTEMPTS,
        },
    }
}

/// `PATH` as the user's login shell sets it ([`platform::env::login_path`]:
/// `$SHELL -l -i -c 'printf %s "$PATH"'`, fallback `/bin/zsh` on macOS,
/// `/bin/bash` elsewhere, with a [`LOGIN_PATH_TIMEOUT`], in its own session
/// so an interactive zsh cannot take the TUI's terminal; the current `PATH`
/// on Windows). A success is cached for the process; a failure or timeout
/// is retried by the next caller, up to [`LOGIN_PATH_ATTEMPTS`], then the
/// inherited `PATH` is kept for good (one warning, debug before).
/// Concurrent callers share one in-flight probe. `None` = keep the
/// inherited `PATH`; immediate when [`NO_LOGIN_PATH`] is `1`.
pub async fn login_path() -> Option<String> {
    if std::env::var_os(NO_LOGIN_PATH).is_some_and(|v| v == "1") {
        return None;
    }
    login_path_with(&PROBE, &login_shell(), LOGIN_PATH_TIMEOUT).await
}

/// The user's login shell.
fn login_shell() -> OsString {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
}

/// [`login_path`] over an explicit state, shell and timeout.
async fn login_path_with(probe: &Mutex<Probe>, shell: &OsStr, timeout: Duration) -> Option<String> {
    let (cell, attempt) = {
        let mut p = lock(probe);
        match &*p {
            Probe::Ready(path) => return Some(path.clone()),
            Probe::GaveUp => return None,
            Probe::Pending { cell, attempts } => (Arc::clone(cell), *attempts),
            Probe::Idle { attempts } => {
                let attempts = attempts + 1;
                let cell = Arc::new(tokio::sync::OnceCell::new());
                *p = Probe::Pending {
                    attempts,
                    cell: Arc::clone(&cell),
                };
                (cell, attempts)
            }
        }
    };
    // `get_or_init` runs the probe once; late joiners wait on the same cell.
    let result = cell
        .get_or_init(|| async {
            match probe_with(shell, timeout).await {
                Ok(path) => Some(path),
                Err(why) if attempt >= LOGIN_PATH_ATTEMPTS => {
                    tracing::warn!(shell = %shell.display(), attempt, %why, "login shell PATH probe failed; keeping the inherited PATH");
                    None
                }
                Err(why) => {
                    tracing::debug!(shell = %shell.display(), attempt, %why, "login shell PATH probe failed; will retry");
                    None
                }
            }
        })
        .await
        .clone();
    let mut p = lock(probe);
    // Only this attempt's own `Pending` advances; a later attempt's does not.
    if matches!(&*p, Probe::Pending { cell: c, .. } if Arc::ptr_eq(c, &cell)) {
        *p = match &result {
            Some(path) => Probe::Ready(path.clone()),
            None if attempt >= LOGIN_PATH_ATTEMPTS => Probe::GaveUp,
            None => Probe::Idle { attempts: attempt },
        };
    }
    result
}

/// One probe: `Err` says why it gave no usable `PATH`.
async fn probe_with(shell: &OsStr, timeout: Duration) -> Result<String, String> {
    platform::env::login_path(shell, timeout).await
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    const RISK: Risk = Risk::Destructive;
    type Args = ShellArgs;

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

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
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
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
        // With seeding off, a new shell keeps the inherited `PATH`.
        let shell = ShellTool::build_shell(false).await.unwrap();
        assert_eq!(
            shell.env_str("PATH").unwrap(),
            std::env::var("PATH").unwrap()
        );
    }

    /// An executable script standing in for the login shell.
    #[cfg(unix)]
    fn fake_shell(body: &str) -> (crate::TestDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::TestDir::new("shell");
        let p = dir.0.join("sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, p)
    }

    #[cfg(unix)]
    fn probe_state() -> Mutex<Probe> {
        Mutex::new(Probe::Idle { attempts: 0 })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_probe_is_retried_then_gives_up() {
        let state = probe_state();
        let never = OsStr::new("/usr/bin/false");
        let long = Duration::from_secs(5);
        for attempts in 1..=LOGIN_PATH_ATTEMPTS {
            assert_eq!(login_path_with(&state, never, long).await, None);
            assert_eq!(state_of(&state), LoginPathState::Failed { attempts });
        }
        // Given up: a shell that would now succeed is never run.
        let (_g, good) = fake_shell("printf %s /late/bin");
        assert_eq!(login_path_with(&state, good.as_os_str(), long).await, None);
        assert_eq!(
            state_of(&state),
            LoginPathState::Failed {
                attempts: LOGIN_PATH_ATTEMPTS
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_probe_counts_as_a_failure_and_success_is_cached() {
        let state = probe_state();
        let (_g, hang) = fake_shell("sleep 30");
        let started = Instant::now();
        assert_eq!(
            login_path_with(&state, hang.as_os_str(), Duration::from_millis(200)).await,
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(state_of(&state), LoginPathState::Failed { attempts: 1 });
        // Retry succeeds and is cached: a later failing shell is not consulted.
        let (_g, good) = fake_shell("printf %s /fake/bin:/bin");
        let path = login_path_with(&state, good.as_os_str(), Duration::from_secs(5)).await;
        assert_eq!(path.as_deref(), Some("/fake/bin:/bin"));
        assert_eq!(
            state_of(&state),
            LoginPathState::Ready("/fake/bin:/bin".into())
        );
        let path =
            login_path_with(&state, OsStr::new("/usr/bin/false"), Duration::from_secs(5)).await;
        assert_eq!(path.as_deref(), Some("/fake/bin:/bin"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_callers_share_one_probe() {
        let state = probe_state();
        let (_g, counted) = fake_shell("printf %s /once >> \"$0.calls\"; printf %s /once/bin");
        let long = Duration::from_secs(5);
        let (a, b) = tokio::join!(
            login_path_with(&state, counted.as_os_str(), long),
            login_path_with(&state, counted.as_os_str(), long)
        );
        assert_eq!(
            (a.as_deref(), b.as_deref()),
            (Some("/once/bin"), Some("/once/bin"))
        );
        let calls = std::fs::read_to_string(format!("{}.calls", counted.display())).unwrap();
        assert_eq!(calls, "/once");
    }

    /// The probe child is its own session: no controlling tty to steal, and
    /// a process group other than ours.
    #[cfg(unix)]
    #[tokio::test]
    async fn probe_child_has_no_controlling_terminal() {
        let (_g, shell) = fake_shell(
            r#"if (exec 3</dev/tty) 2>/dev/null; then printf HASTTY; else printf NOTTY; fi
printf ' %s' "$(ps -o pgid= -p $$)""#,
        );
        let out = probe_with(shell.as_os_str(), Duration::from_secs(5))
            .await
            .unwrap();
        let (tty, pgid) = out.split_once(' ').unwrap();
        assert_eq!(tty, "NOTTY");
        let ours = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        let ours = String::from_utf8_lossy(&ours.stdout).trim().to_owned();
        assert_ne!(pgid.trim(), ours, "{out}");
    }

    #[tokio::test]
    async fn new_with_login_path_starts_the_probe_eagerly() {
        if std::env::var_os(NO_LOGIN_PATH).is_some_and(|v| v == "1") {
            return;
        }
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let _tool = ShellTool::new(fx.jobs.clone(), true);
        // Let the spawned task take the lock; no shell call has happened.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            matches!(
                login_path_state(),
                LoginPathState::Pending | LoginPathState::Ready(_)
            ),
            "{:?}",
            login_path_state()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn login_shell_path_contains_bin() {
        let shell = login_shell();
        if std::env::var_os("SHELL").is_none_or(|s| s.is_empty())
            || !std::path::Path::new(&shell).exists()
        {
            return;
        }
        // The probe may exceed its budget on a loaded machine; then it
        // reports `Err` by design and there is nothing to check.
        if let Ok(path) = probe_with(&shell, LOGIN_PATH_TIMEOUT).await {
            assert!(
                path.split(':').any(|p| p == "/bin" || p == "/usr/bin"),
                "{path}"
            );
        }
    }
}
