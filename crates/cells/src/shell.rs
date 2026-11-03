//! Owner: E1 — shell kernels (design 171 §5.2, §5.4–5.7): a `cell` PTY job started with an
//! explicit `program`, the binary-owned rc files, the run file + run line, marks → events.
//!
//! One kernel = one interactive shell on a PTY job named `cell:<kind> <id>` (lead pin L1).
//! A run writes `kernels/<id>/run.sh` (0600: the CWD-rule prologue + the exact code), types
//! the one run line of §5.5 and waits for the marks the run line itself prints: `133;C`
//! starts the run (its log offset is the output's start), `133;D;n` ends it with exit `n`,
//! OSC 7 updates the live cwd. Stop is Ctrl-C; an interactive shell drops the rest of the
//! run line on SIGINT, so after a Stop/Kill the kernel probes: once the shell is back in the
//! terminal's foreground it types a line that prints the missing `D` mark itself.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use kameo::actor::ActorRef;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tools::jobs::{JobExit, JobId, Jobs};
use tools::pty::{CellForeground, CellKill, Key, PtyAttach, PtyInput, PtyOut, StartArgs, StartPty};

use crate::error::{CellsError, Result};
use crate::kernel::{ExecuteRequest, Kernel, KernelCtx, KernelEvent, KernelSink};
use crate::types::{
    ExitStatus, Interpreter, InterpreterKind, KernelPhase, KernelSpec, Mark, MarkKind, RunId,
    SessionId, ShellKind,
};

/// E1's keys of the `cells:` section (flattened into [`crate::CellsConfig`]). None yet: every
/// shell knob of §15.4 lives in the frozen section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {}

/// How long a fresh shell may take to source the user's rc and print its ready mark.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Stop/Kill probe cadence: the shell must hold the foreground this many ticks in a row.
const PROBE_EVERY: Duration = Duration::from_millis(150);
const PROBE_TICKS: u32 = 3;

/// Start a shell kernel for `spec` (`KernelSpec::Shell`).
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    Ok(launch(spec, ctx).await?.0)
}

/// [`start`] plus the PTY job's log path (the actor copies the run's `[C, D)` range from it,
/// §8.3) — the actor's local adapter around the frozen `Kernel` trait, which has no log
/// accessor.
pub(crate) async fn launch(
    spec: &KernelSpec,
    ctx: KernelCtx,
) -> Result<(Box<dyn Kernel>, PathBuf), CellsError> {
    let KernelSpec::Shell { kind, path } = spec else {
        return Err(CellsError::Failed("not a shell kernel".into()));
    };
    let jobs = ctx
        .jobs
        .clone()
        .ok_or_else(|| CellsError::Failed("the jobs plugin is not running".into()))?;
    let kid = kernel_dir_name(&ctx.id);
    let rc = ctx.paths.rc_dir();
    write_rc(&rc)?;
    let kernel_dir = ctx.paths.kernel_dir(&kid);
    std::fs::create_dir_all(&kernel_dir)?;
    let run_file = kernel_dir.join(if *kind == ShellKind::Pwsh {
        "run.ps1"
    } else {
        "run.sh"
    });
    let (args, extra) = invocation(*kind, &rc);
    let mut env: BTreeMap<String, String> = (*ctx.login_env).clone();
    env.extend(extra);
    let cwd = env
        .get("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"));
    let (mark_tx, mut marks) = mpsc::channel::<Mark>(256);
    let out = jobs
        .ask(StartPty {
            session: ctx.owner,
            call_id: String::new(),
            args: StartArgs {
                command: format!("{} (user cell kernel)", path.display()),
                name: Some(format!(
                    "{}{} {kid}",
                    tools::jobs::CELL_PREFIX,
                    kind_word(*kind)
                )),
                cwd: None,
                env: Some(env),
                ready: None,
                idle_secs: Some(0),
            },
            cwd,
            ready_timeout: READY_TIMEOUT,
            idle_after: None,
            root: ctx.root.clone(),
            activity: None,
            pty_shell: None,
            program: Some((path.clone(), args)),
            cell: Some(mark_tx),
        })
        .await
        .map_err(|e| CellsError::Failed(format!("jobs actor: {e}")))?;
    if out.is_error {
        return Err(CellsError::Failed(out.content));
    }
    let structured = out.structured.unwrap_or_default();
    let job = structured
        .get("job")
        .and_then(|v| v.as_str())
        .map(|s| JobId(s.to_owned()))
        .ok_or_else(|| CellsError::Failed("the kernel job has no id".into()))?;
    let log = structured
        .get("log")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| CellsError::Failed("the kernel job has no log".into()))?;
    let session = ctx.owner;
    // Attaching with a size fixes the grid at `pty_cols × pty_rows` (§5.10).
    let size = (ctx.config.pty_rows, ctx.config.pty_cols);
    let attached = jobs
        .ask(PtyAttach {
            session,
            id: job.clone(),
            size: Some(size),
        })
        .await
        .map_err(|e| CellsError::Failed(format!("attach to the kernel: {e}")));
    let mut stream = match attached {
        Ok(a) => a.rx,
        Err(e) => {
            close_job(&jobs, session, &job).await;
            return Err(e);
        }
    };
    // Ready on mark A (§5.5); an exit first is a spawn failure (`dead`).
    let ready = tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            tokio::select! {
                m = marks.recv() => match m {
                    Some(m) if m.kind == MarkKind::Ready => return Ok(()),
                    Some(_) => {}
                    None => return Err("the kernel's mark channel closed".to_owned()),
                },
                o = stream.recv() => match o {
                    Ok(PtyOut::Exit(e)) => return Err(format!("the shell exited before it was ready ({e})")),
                    Ok(PtyOut::Data(_)) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Err("the shell exited before it was ready".to_owned()),
                },
            }
        }
    })
    .await;
    match ready {
        Ok(Ok(())) => {}
        Ok(Err(why)) => {
            close_job(&jobs, session, &job).await;
            return Err(CellsError::Failed(why));
        }
        Err(_) => {
            close_job(&jobs, session, &job).await;
            return Err(CellsError::Failed(format!(
                "the shell did not become ready within {} s — check your rc files",
                READY_TIMEOUT.as_secs()
            )));
        }
    }
    let shared = Arc::new(Shared {
        phase: Mutex::new(KernelPhase::Ready),
        cwd: Mutex::new(None),
        run: Mutex::new(None),
    });
    let kernel = ShellKernel {
        spec: spec.clone(),
        kind: *kind,
        jobs: jobs.clone(),
        session,
        job: job.clone(),
        run_file,
        shared: Arc::clone(&shared),
    };
    tokio::spawn(watch(Watch {
        shared,
        jobs,
        session,
        job,
        kind: *kind,
        marks,
        stream,
    }));
    Ok((Box::new(kernel), log))
}

async fn close_job(jobs: &ActorRef<Jobs>, session: SessionId, job: &JobId) {
    let _ = jobs
        .ask(CellKill {
            session,
            id: job.clone(),
            close: true,
        })
        .await;
}

/// `k` + 12 hex of the kernel id's digest: the job name and `kernels/<id>/` (stable per key).
pub(crate) fn kernel_dir_name(id: &crate::types::KernelId) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_string(id).unwrap_or_default();
    let d = hex::encode(Sha256::digest(json.as_bytes()));
    format!("k{}", &d[..12])
}

fn kind_word(kind: ShellKind) -> &'static str {
    match kind {
        ShellKind::Bash => "bash",
        ShellKind::Zsh => "zsh",
        ShellKind::Sh => "sh",
        ShellKind::Pwsh => "pwsh",
    }
}

/// The program's arguments and extra environment per the §5.5 table.
fn invocation(kind: ShellKind, rc: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    match kind {
        ShellKind::Bash => (
            vec![
                "--noprofile".into(),
                "--rcfile".into(),
                s(rc.join("bash.rc")),
                "-i".into(),
            ],
            Vec::new(),
        ),
        ShellKind::Zsh => (
            vec!["-i".into()],
            vec![("ZDOTDIR".into(), s(rc.join("zsh")))],
        ),
        ShellKind::Sh => (vec!["-i".into()], vec![("ENV".into(), s(rc.join("sh.rc")))]),
        ShellKind::Pwsh => (
            vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                format!(". {}", pwsh_quote(&s(rc.join("rc.ps1")))),
            ],
            Vec::new(),
        ),
    }
}

const READY: &str = r"printf '\033]133;A\007'";

/// bash: the user's `~/.bashrc`, an empty prompt, the ready mark (§5.5). History stays off so
/// run lines never land in the user's history file.
const BASH_RC: &str = r#"# theuth user-cell kernel (binary-owned; rewritten at every kernel start)
[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"
PS1= PS2= PS0=
unset PROMPT_COMMAND HISTFILE
printf '\033]133;A\007'
"#;

/// zsh `.zshenv`: the user's `~/.zshenv`, remembering where it points `ZDOTDIR`.
const ZSH_ENV: &str = r#"# theuth user-cell kernel (binary-owned)
__theuth_zd=$ZDOTDIR
ZDOTDIR=$HOME
[ -r "$HOME/.zshenv" ] && . "$HOME/.zshenv"
__theuth_user_zd=${ZDOTDIR:-$HOME}
ZDOTDIR=$__theuth_zd
"#;

/// zsh `.zshrc`: the user's `.zshrc`, comments allowed at the prompt (the zsh trap of
/// proof step 2), no prompt, no hooks, the ready mark.
const ZSH_RC: &str = r#"# theuth user-cell kernel (binary-owned)
if [ -r "$__theuth_user_zd/.zshrc" ]; then
  ZDOTDIR=$__theuth_user_zd
  . "$__theuth_user_zd/.zshrc"
fi
setopt interactive_comments
PROMPT= RPROMPT= PS2=
precmd_functions=()
preexec_functions=()
unset HISTFILE
printf '\033]133;A\007'
"#;

const SH_RC: &str = r#"# theuth user-cell kernel (binary-owned)
PS1= PS2=
printf '\033]133;A\007'
"#;

const PWSH_RC: &str = r#"# theuth user-cell kernel (binary-owned)
if (Test-Path $PROFILE) { . $PROFILE }
function prompt { '' }
[Console]::Write("`e]133;A`a")
"#;

/// Write the binary-owned rc files under `rc/` (§5.5, §15.5).
fn write_rc(rc: &Path) -> Result<()> {
    debug_assert!(BASH_RC.contains(READY));
    std::fs::create_dir_all(rc.join("zsh"))?;
    std::fs::write(rc.join("bash.rc"), BASH_RC)?;
    std::fs::write(rc.join("zsh").join(".zshenv"), ZSH_ENV)?;
    std::fs::write(rc.join("zsh").join(".zshrc"), ZSH_RC)?;
    std::fs::write(rc.join("sh.rc"), SH_RC)?;
    std::fs::write(rc.join("rc.ps1"), PWSH_RC)?;
    Ok(())
}

/// POSIX single-quoted.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// PowerShell single-quoted.
fn pwsh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The run file's text: the prologue when `req.cwd` is set (§5.6 step 4), then the code.
fn run_file_text(kind: ShellKind, req: &ExecuteRequest) -> String {
    let mut out = String::new();
    if let Some(cwd) = &req.cwd {
        let cwd = cwd.to_string_lossy();
        if kind == ShellKind::Pwsh {
            out.push_str(&format!("Set-Location -LiteralPath {}\n", pwsh_quote(&cwd)));
            for (k, v) in &req.env {
                out.push_str(&format!("$env:{k} = {}\n", pwsh_quote(v)));
            }
        } else {
            out.push_str(&format!("cd -- {}\n", sh_quote(&cwd)));
            for (k, v) in &req.env {
                out.push_str(&format!("export {k}={}\n", sh_quote(v)));
            }
        }
    }
    out.push_str(&req.code);
    out.push('\n');
    out
}

/// The one run line (§5.5).
fn run_line(kind: ShellKind, file: &Path) -> String {
    let f = file.to_string_lossy();
    if kind == ShellKind::Pwsh {
        format!(
            "[Console]::Write(\"`e]133;C`a\"); . {}; $c = if ($?) {{ 0 }} elseif ($LASTEXITCODE) {{ $LASTEXITCODE }} else {{ 1 }}; [Console]::Write(\"`e]133;D;$c`a`e]7;file://localhost/$($PWD.Path)`a\")",
            pwsh_quote(&f)
        )
    } else {
        format!(
            r#"printf '\033]133;C\007'; . {}; printf '\033]133;D;%s\007\033]7;file://%s%s\007' "$?" "$(hostname)" "$PWD""#,
            sh_quote(&f)
        )
    }
}

/// After a Stop/Kill, a line that prints the `D` mark the aborted run line never reached.
fn probe_line(kind: ShellKind, exit: i32) -> String {
    if kind == ShellKind::Pwsh {
        format!("[Console]::Write(\"`e]133;D;{exit}`a`e]7;file://localhost/$($PWD.Path)`a\")")
    } else {
        format!(r#"printf '\033]133;D;{exit}\007\033]7;file://%s%s\007' "$(hostname)" "$PWD""#)
    }
}

/// Write `0600` (the run file can hold expanded secrets, invariant 9).
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let _ = std::fs::remove_file(path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(text.as_bytes())
}

/// The run in flight.
struct Current {
    id: RunId,
    sink: KernelSink,
    started: bool,
    /// Stop (130) / Kill (137) was sent: the exit the probe line reports.
    stopping: Option<i32>,
    run_file: PathBuf,
}

struct Shared {
    phase: Mutex<KernelPhase>,
    cwd: Mutex<Option<String>>,
    run: Mutex<Option<Current>>,
}

struct ShellKernel {
    spec: KernelSpec,
    kind: ShellKind,
    jobs: ActorRef<Jobs>,
    session: SessionId,
    job: JobId,
    run_file: PathBuf,
    shared: Arc<Shared>,
}

impl ShellKernel {
    async fn type_text(&self, text: String, enter: bool) -> Result<()> {
        type_into(&self.jobs, self.session, &self.job, text, enter).await
    }
}

async fn type_into(
    jobs: &ActorRef<Jobs>,
    session: SessionId,
    job: &JobId,
    text: String,
    enter: bool,
) -> Result<()> {
    jobs.ask(PtyInput {
        session,
        id: job.clone(),
        text: Some(text),
        keys: if enter { vec![Key::Enter] } else { Vec::new() },
    })
    .await
    .map_err(|e| CellsError::Failed(format!("write to the kernel: {e}")))
}

#[async_trait::async_trait]
impl Kernel for ShellKernel {
    fn spec(&self) -> &KernelSpec {
        &self.spec
    }

    fn phase(&self) -> KernelPhase {
        *self.shared.phase.lock()
    }

    async fn execute(&self, req: ExecuteRequest, sink: KernelSink) -> Result<()> {
        {
            let mut phase = self.shared.phase.lock();
            if *phase != KernelPhase::Ready {
                return Err(CellsError::Failed(format!("the kernel is {phase:?}")));
            }
            *phase = KernelPhase::Busy;
        }
        let text = run_file_text(self.kind, &req);
        if let Err(e) = write_private(&self.run_file, &text) {
            *self.shared.phase.lock() = KernelPhase::Ready;
            return Err(e.into());
        }
        *self.shared.run.lock() = Some(Current {
            id: req.run.clone(),
            sink,
            started: false,
            stopping: None,
            run_file: self.run_file.clone(),
        });
        if let Err(e) = self
            .type_text(run_line(self.kind, &self.run_file), true)
            .await
        {
            self.shared.run.lock().take();
            let _ = std::fs::remove_file(&self.run_file);
            *self.shared.phase.lock() = KernelPhase::Ready;
            return Err(e);
        }
        Ok(())
    }

    async fn input(&self, run: &RunId, bytes: Vec<u8>) -> Result<()> {
        let live = self
            .shared
            .run
            .lock()
            .as_ref()
            .is_some_and(|c| c.id == *run);
        if !live {
            return Err(CellsError::NotFound(format!("{run} is not running")));
        }
        self.type_text(String::from_utf8_lossy(&bytes).into_owned(), false)
            .await
    }

    async fn interrupt(&self) -> Result<()> {
        if let Some(c) = self.shared.run.lock().as_mut() {
            c.stopping.get_or_insert(130);
        }
        self.type_text("\x03".into(), false).await
    }

    async fn kill(&self) -> Result<()> {
        if let Some(c) = self.shared.run.lock().as_mut() {
            c.stopping = Some(137);
        }
        self.jobs
            .ask(CellKill {
                session: self.session,
                id: self.job.clone(),
                close: false,
            })
            .await
            .map_err(|e| CellsError::Failed(format!("kill: {e}")))
    }

    async fn shutdown(&self) -> Result<()> {
        *self.shared.phase.lock() = KernelPhase::Closed;
        close_job(&self.jobs, self.session, &self.job).await;
        Ok(())
    }

    fn live_cwd(&self) -> Option<String> {
        self.shared.cwd.lock().clone()
    }
}

/// The kernel's watcher: marks → run events, live bytes between C and D, the exit.
struct Watch {
    shared: Arc<Shared>,
    jobs: ActorRef<Jobs>,
    session: SessionId,
    job: JobId,
    kind: ShellKind,
    marks: mpsc::Receiver<Mark>,
    stream: broadcast::Receiver<PtyOut>,
}

async fn watch(mut w: Watch) {
    let mut cut = LiveCut::default();
    let mut probe = tokio::time::interval(PROBE_EVERY);
    probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut fg_ticks = 0u32;
    let mut probed: Option<tokio::time::Instant> = None;
    loop {
        let stopping = w.shared.run.lock().as_ref().and_then(|c| c.stopping);
        tokio::select! {
            m = w.marks.recv() => match m {
                Some(m) => on_mark(&w.shared, m, &mut cut).await,
                None => break,
            },
            o = w.stream.recv() => match o {
                Ok(PtyOut::Data(bytes)) => {
                    if let Some(live) = cut.feed(&bytes) {
                        let sink = w.shared.run.lock().as_ref().map(|c| c.sink.clone());
                        if let Some(sink) = sink {
                            let _ = sink.try_send(KernelEvent::Bytes(live));
                        }
                    }
                }
                Ok(PtyOut::Exit(exit)) => {
                    died(&w.shared, &exit).await;
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    died(&w.shared, &JobExit::Lost).await;
                    break;
                }
            },
            _ = probe.tick(), if stopping.is_some() => {
                let fg = w.jobs.ask(CellForeground { session: w.session, id: w.job.clone() }).await;
                if matches!(fg, Ok(true)) { fg_ticks += 1 } else { fg_ticks = 0 }
                let due = probed.is_none_or(|t| t.elapsed() > Duration::from_secs(2));
                if fg_ticks >= PROBE_TICKS && due && let Some(exit) = stopping {
                    probed = Some(tokio::time::Instant::now());
                    let _ = type_into(&w.jobs, w.session, &w.job, probe_line(w.kind, exit), true).await;
                }
            },
        }
        if stopping.is_none() {
            fg_ticks = 0;
            probed = None;
        }
    }
}

async fn on_mark(shared: &Shared, m: Mark, cut: &mut LiveCut) {
    match m.kind {
        MarkKind::Ready => {}
        MarkKind::Cwd => *shared.cwd.lock() = m.cwd,
        MarkKind::Start => {
            let sink = {
                let mut run = shared.run.lock();
                match run.as_mut() {
                    Some(c) if !c.started => {
                        c.started = true;
                        Some(c.sink.clone())
                    }
                    _ => None,
                }
            };
            if let Some(sink) = sink {
                cut.start();
                let _ = sink
                    .send(KernelEvent::Started {
                        log_from: Some(m.offset),
                    })
                    .await;
            }
        }
        MarkKind::End => {
            let Some(c) = shared.run.lock().take() else {
                return; // a probe's D after the real one, or a stray mark
            };
            cut.end();
            let _ = std::fs::remove_file(&c.run_file);
            *shared.phase.lock() = KernelPhase::Ready;
            if !c.started {
                let _ = c
                    .sink
                    .send(KernelEvent::Started {
                        log_from: Some(m.offset),
                    })
                    .await;
            }
            let exit = m.exit.or(c.stopping).unwrap_or(0);
            let _ = c
                .sink
                .send(KernelEvent::Finished {
                    exit: Some(ExitStatus::Code(exit)),
                    log_to: Some(m.offset),
                    kernel_died: false,
                })
                .await;
        }
    }
}

async fn died(shared: &Shared, exit: &JobExit) {
    *shared.phase.lock() = KernelPhase::Dead;
    let Some(c) = shared.run.lock().take() else {
        return;
    };
    let _ = std::fs::remove_file(&c.run_file);
    let status = match exit {
        JobExit::Exited(n) => ExitStatus::Code(i32::from(*n)),
        JobExit::Killed => ExitStatus::Signal("KILL".into()),
        JobExit::Timeout | JobExit::Lost => ExitStatus::Signal("HUP".into()),
    };
    let _ = c
        .sink
        .send(KernelEvent::Finished {
            exit: Some(status),
            log_to: None,
            kernel_died: true,
        })
        .await;
}

/// Cuts the live byte stream to the run's `[C, D)` for `cell_attach` (the record itself is
/// rendered from the log range, §8.3). The marks arrive on their own channel; this only
/// trims the C/D sequences out of the chunks that carry them.
#[derive(Default)]
struct LiveCut {
    on: bool,
    carry: Vec<u8>,
}

const C_SEQ: &[u8] = b"\x1b]133;C\x07";
const D_SEQ: &[u8] = b"\x1b]133;D";

impl LiveCut {
    fn start(&mut self) {
        self.on = true;
    }

    fn end(&mut self) {
        self.on = false;
        self.carry.clear();
    }

    /// The part of `chunk` that belongs to the running output, if any.
    fn feed(&mut self, chunk: &[u8]) -> Option<Vec<u8>> {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);
        if let Some(at) = find(&data, C_SEQ) {
            self.on = true;
            data.drain(..at + C_SEQ.len());
        } else if !self.on {
            let keep = data.len().min(C_SEQ.len() - 1);
            self.carry = data[data.len() - keep..].to_vec();
            return None;
        }
        if let Some(at) = find(&data, D_SEQ) {
            self.on = false;
            data.truncate(at);
        }
        (!data.is_empty()).then_some(data)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---- resolution (§6.5) ----

/// §6.5: which binary runs a `kind` cell — the note default `shell:`, else the first `kind` on
/// the login PATH, else the system one — and its chip. E2's `interp::interpreters` lists the
/// shells through this.
pub async fn resolve(
    kind: ShellKind,
    note_shell: Option<&str>,
    login_env: &BTreeMap<String, String>,
) -> Result<(PathBuf, Interpreter), CellsError> {
    let path_var = login_env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let home = login_env.get("HOME").map(String::as_str);
    let (path, why) = match note_shell.filter(|s| !s.trim().is_empty()) {
        Some(s) => {
            let p = expand_home(s.trim(), home);
            let p = if p.is_absolute() {
                p
            } else {
                search_path(&p.to_string_lossy(), &path_var).unwrap_or(p)
            };
            if !p.is_file() {
                return Err(CellsError::NoInterpreter(format!(
                    "the note's shell `{s}` was not found"
                )));
            }
            (p, "the note's `shell:` default".to_owned())
        }
        None => {
            let names: &[&str] = match kind {
                ShellKind::Bash => &["bash"],
                ShellKind::Zsh => &["zsh"],
                ShellKind::Sh => &["sh"],
                ShellKind::Pwsh => &["pwsh", "powershell"],
            };
            let found = names.iter().find_map(|n| search_path(n, &path_var));
            match found {
                Some(p) => (p, "first on your login PATH".to_owned()),
                None => {
                    let system = match kind {
                        ShellKind::Bash => Some("/bin/bash"),
                        ShellKind::Zsh => Some("/bin/zsh"),
                        ShellKind::Sh => Some("/bin/sh"),
                        ShellKind::Pwsh => None,
                    }
                    .map(PathBuf::from)
                    .filter(|p| p.is_file());
                    match system {
                        Some(p) => (p, "the system shell".to_owned()),
                        None => {
                            return Err(CellsError::NoInterpreter(if cfg!(windows) {
                                format!(
                                    "no {} on this machine — install Git for Windows or WSL",
                                    kind_word(kind)
                                )
                            } else {
                                format!("no {} on this machine", kind_word(kind))
                            }));
                        }
                    }
                }
            }
        }
    };
    let version = version_of(kind, &path).await;
    let source = source_of(&path);
    let mut why = why;
    if kind == ShellKind::Bash && version.as_deref().is_some_and(|v| v.starts_with("3.")) {
        why.push_str(" · bash 3.2 — no associative arrays, no `${x,,}`");
    }
    Ok((
        path.clone(),
        Interpreter {
            kind: interpreter_kind(kind),
            source,
            path: Some(path),
            version,
            why,
            auto: true,
        },
    ))
}

/// The chip text (§6.3): `bash 5.2 (Homebrew)`, `bash 3.2 (macOS)`, `zsh 5.9`, `sh (dash)`.
pub(crate) fn chip(i: &Interpreter) -> String {
    let word = match i.kind {
        InterpreterKind::Bash => "bash",
        InterpreterKind::Zsh => "zsh",
        InterpreterKind::Sh => "sh",
        InterpreterKind::Pwsh => "pwsh",
        InterpreterKind::Python => "Python",
        InterpreterKind::Monty => "Monty",
        InterpreterKind::Node => "node",
        InterpreterKind::Deno => "deno",
        InterpreterKind::Bun => "bun",
    };
    let short = i.version.as_deref().map(|v| {
        let mut parts = v.split('.');
        match (parts.next(), parts.next()) {
            (Some(a), Some(b)) => format!("{a}.{b}"),
            _ => v.to_owned(),
        }
    });
    let mut out = word.to_owned();
    if let Some(v) = short {
        out.push(' ');
        out.push_str(&v);
    }
    match i.source.as_str() {
        "homebrew" => out.push_str(" (Homebrew)"),
        "macOS" => out.push_str(" (macOS)"),
        "dash" => out.push_str(" (dash)"),
        _ => {}
    }
    out
}

fn interpreter_kind(kind: ShellKind) -> InterpreterKind {
    match kind {
        ShellKind::Bash => InterpreterKind::Bash,
        ShellKind::Zsh => InterpreterKind::Zsh,
        ShellKind::Sh => InterpreterKind::Sh,
        ShellKind::Pwsh => InterpreterKind::Pwsh,
    }
}

fn expand_home(s: &str, home: Option<&str>) -> PathBuf {
    match (s.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => Path::new(h).join(rest),
        _ if s == "~" => home.map_or_else(|| PathBuf::from(s), PathBuf::from),
        _ => PathBuf::from(s),
    }
}

fn search_path(name: &str, path_var: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ""] } else { &[""] };
    std::env::split_paths(path_var).find_map(|dir| {
        exts.iter()
            .map(|e| dir.join(format!("{name}{e}")))
            .find(|p| p.is_file())
    })
}

fn source_of(path: &Path) -> String {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let r = real.to_string_lossy();
    if r.starts_with("/opt/homebrew")
        || r.starts_with("/usr/local/Cellar")
        || r.starts_with("/home/linuxbrew")
    {
        "homebrew".into()
    } else if r.ends_with("/dash") {
        "dash".into()
    } else if cfg!(target_os = "macos") && (r.starts_with("/bin/") || r.starts_with("/usr/bin/")) {
        "macOS".into()
    } else {
        "system".into()
    }
}

/// `<shell> --version`'s number, probed once per path (2 s cap).
async fn version_of(kind: ShellKind, path: &Path) -> Option<String> {
    static CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<String>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    if let Some(v) = CACHE.lock().get(path) {
        return v.clone();
    }
    let v = if kind == ShellKind::Sh {
        None
    } else {
        let mut cmd = tokio::process::Command::new(path);
        cmd.arg("--version")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(2), cmd.output()).await {
            Ok(Ok(out)) => parse_version(&String::from_utf8_lossy(&out.stdout)),
            _ => None,
        }
    };
    CACHE.lock().insert(path.to_path_buf(), v.clone());
    v
}

/// First `N.N[.N]` in `--version` output (`GNU bash, version 5.2.37(1)-release`, `zsh 5.9`).
fn parse_version(text: &str) -> Option<String> {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\d+\.\d+(?:\.\d+)?").expect("version re"));
    RE.find(text.lines().next()?).map(|m| m.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_file_has_prologue_only_when_asked() {
        let mut req = ExecuteRequest {
            run: RunId("r_1".into()),
            code: "ls".into(),
            mode: crate::types::RunMode::Cell,
            cwd: Some(PathBuf::from("/tmp/it's")),
            env: vec![("P".into(), "a b".into())],
            timeout: None,
        };
        assert_eq!(
            run_file_text(ShellKind::Bash, &req),
            "cd -- '/tmp/it'\\''s'\nexport P='a b'\nls\n"
        );
        req.cwd = None;
        assert_eq!(run_file_text(ShellKind::Bash, &req), "ls\n");
    }

    #[test]
    fn run_line_is_the_design_line() {
        assert_eq!(
            run_line(ShellKind::Bash, Path::new("/k/run.sh")),
            r#"printf '\033]133;C\007'; . '/k/run.sh'; printf '\033]133;D;%s\007\033]7;file://%s%s\007' "$?" "$(hostname)" "$PWD""#
        );
    }

    #[test]
    fn live_cut_keeps_only_the_run_bytes_across_splits() {
        let all = b"echo-of-run-line\x1b]133;C\x07hello\n\x1b]133;D;0\x07\x1b]7;file://h/x\x07";
        for split in 0..all.len() {
            let mut cut = LiveCut::default();
            let mut got = Vec::new();
            for part in [&all[..split], &all[split..]] {
                if let Some(b) = cut.feed(part) {
                    got.extend(b);
                }
            }
            // A split inside the D sequence leaks at most its first bytes.
            let got = String::from_utf8_lossy(&got).into_owned();
            assert!(got.starts_with("hello\n"), "split {split}: {got:?}");
            assert!(
                got.len() <= "hello\n".len() + D_SEQ.len(),
                "split {split}: {got:?}"
            );
        }
    }

    #[test]
    fn versions_and_chips() {
        assert_eq!(
            parse_version("GNU bash, version 5.2.37(1)-release (aarch64-apple-darwin24.0.0)"),
            Some("5.2.37".into())
        );
        assert_eq!(
            parse_version("zsh 5.9 (arm64-apple-darwin24.0)"),
            Some("5.9".into())
        );
        let i = |kind, source: &str, version: Option<&str>| Interpreter {
            kind,
            source: source.into(),
            path: None,
            version: version.map(Into::into),
            why: String::new(),
            auto: true,
        };
        assert_eq!(
            chip(&i(InterpreterKind::Bash, "homebrew", Some("5.2.37"))),
            "bash 5.2 (Homebrew)"
        );
        assert_eq!(
            chip(&i(InterpreterKind::Bash, "macOS", Some("3.2.57"))),
            "bash 3.2 (macOS)"
        );
        assert_eq!(
            chip(&i(InterpreterKind::Zsh, "system", Some("5.9"))),
            "zsh 5.9"
        );
        assert_eq!(chip(&i(InterpreterKind::Sh, "dash", None)), "sh (dash)");
    }

    #[tokio::test]
    async fn resolve_prefers_the_login_path_then_the_system_shell() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("bash");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'GNU bash, version 9.9.1(1)-release'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = BTreeMap::new();
        env.insert(
            "PATH".to_owned(),
            format!("{}:/usr/bin:/bin", dir.path().display()),
        );
        let (p, i) = resolve(ShellKind::Bash, None, &env).await.unwrap();
        assert_eq!(p, fake);
        assert_eq!(i.version.as_deref(), Some("9.9.1"));
        env.insert("PATH".into(), "/nonexistent".into());
        if Path::new("/bin/bash").is_file() {
            let (p, _) = resolve(ShellKind::Bash, None, &env).await.unwrap();
            assert_eq!(p, PathBuf::from("/bin/bash"));
        }
        let (p, _) = resolve(ShellKind::Bash, Some(fake.to_str().unwrap()), &env)
            .await
            .unwrap();
        assert_eq!(p, fake);
    }
}
