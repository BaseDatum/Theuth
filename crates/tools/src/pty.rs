//! PTY jobs (design doc 22): `start` / `send` / `screen` / `wait` run a command
//! on a pseudo-terminal as a job on the [`Jobs`] actor (14) and show the model a
//! rendered 120×40 screen instead of the raw bytes. Same `j{n}` id, log file,
//! `jobs` / `cancel_job` and `<job_result>`; stdin, a screen and no deadline
//! are what differ (22 §1).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brush_core::Shell;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use nix::sys::signal::killpg;
use nix::unistd::Pid;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use regex::Regex;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::jobs::{self, Finished, Job, JobEvent, JobExit, JobId, Jobs, Log, lock, sep};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Screen height (22 §1.9).
const ROWS: u16 = 40;
/// Screen width (22 §1.9).
const COLS: u16 = 120;
/// `send` replies once no bytes arrived for this long (22 §4.5)…
const SETTLE_QUIET: Duration = Duration::from_millis(300);
/// …or after this long regardless.
const SETTLE_MAX: Duration = Duration::from_secs(2);
/// `wait` default / maximum seconds (22 §4.6).
const WAIT_DEFAULT: u64 = 30;
const WAIT_MAX: u64 = 120;
/// `ready.timeout` maximum seconds (22 §4.7).
const READY_MAX: u64 = 300;
/// TCP connect poll spacing for `ready.port`.
const PORT_POLL: Duration = Duration::from_millis(250);
/// Longest terminal query the responder carries across chunks (22 §4.4).
const CARRY: usize = 16;
/// Reader thread buffer (22 §4.2).
const READ_BUF: usize = 64 * 1024;

/// Arguments for [`StartTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StartArgs {
    /// Command line, run by `$SHELL -lc` on a 120×40 pseudo-terminal.
    pub command: String,
    /// Label; `send`/`screen`/`wait` accept it as `id` in place of `j{n}`.
    /// Unique among the session's running jobs.
    pub name: Option<String>,
    /// Working directory; default the session shell's.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: Option<BTreeMap<String, String>>,
    /// Block until the job is ready before replying (22 §1.7).
    pub ready: Option<Ready>,
}

/// `start`'s readiness condition: every field given must pass.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    /// Rust regex (no look-around) matched against the output so far.
    pub log: Option<String>,
    /// TCP port that must accept a connection.
    pub port: Option<u16>,
    /// Host for `port`; default `127.0.0.1`.
    pub host: Option<String>,
    /// Seconds to wait; default `tools.pty_ready_timeout_secs` (30), max 300.
    pub timeout: Option<u64>,
}

/// Arguments for [`SendTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendArgs {
    /// `j4` or the job's `name`.
    pub id: String,
    /// Typed first.
    pub text: Option<String>,
    /// Append Enter after `text`; default true.
    pub enter: Option<bool>,
    /// Keys sent after `text`.
    pub keys: Option<Vec<Key>>,
    /// Signal delivered to the process group after the write.
    pub signal: Option<Signal>,
}

/// Arguments for [`ScreenTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScreenArgs {
    /// `j4` or the job's `name`.
    pub id: String,
}

/// Arguments for [`WaitTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WaitArgs {
    /// `j4` or the job's `name`.
    pub id: String,
    /// Rust regex (no look-around) over output since this call started;
    /// absent → wait for exit.
    pub pattern: Option<String>,
    /// Seconds; default 30, max 120.
    pub timeout: Option<u64>,
}

/// A key `send` types (22 §4.5 key table).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Key {
    Enter,
    Tab,
    Escape,
    CtrlC,
    CtrlD,
    CtrlZ,
    CtrlL,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
}

impl Key {
    fn bytes(self) -> &'static [u8] {
        match self {
            Key::Enter => b"\r",
            Key::Tab => b"\t",
            Key::Escape => b"\x1b",
            Key::CtrlC => b"\x03",
            Key::CtrlD => b"\x04",
            Key::CtrlZ => b"\x1a",
            Key::CtrlL => b"\x0c",
            Key::Up => b"\x1b[A",
            Key::Down => b"\x1b[B",
            Key::Right => b"\x1b[C",
            Key::Left => b"\x1b[D",
            Key::Home => b"\x1b[H",
            Key::End => b"\x1b[F",
            Key::PageUp => b"\x1b[5~",
            Key::PageDown => b"\x1b[6~",
            Key::Backspace => b"\x7f",
            Key::Delete => b"\x1b[3~",
        }
    }
}

/// A signal `send` delivers to the job's process group.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Signal {
    Int,
    Term,
    Hup,
    Quit,
    Kill,
}

impl From<Signal> for nix::sys::signal::Signal {
    fn from(s: Signal) -> Self {
        use nix::sys::signal::Signal as S;
        match s {
            Signal::Int => S::SIGINT,
            Signal::Term => S::SIGTERM,
            Signal::Hup => S::SIGHUP,
            Signal::Quit => S::SIGQUIT,
            Signal::Kill => S::SIGKILL,
        }
    }
}

/// What a running PTY job shares with the tools (22 §4.8): the master's
/// writer, the rendered screen, an output wake-up and the process group.
pub(crate) struct PtyHandle {
    writer: Mutex<Box<dyn Write + Send>>,
    parser: Mutex<vt100::Parser>,
    output: Notify,
    /// `== pgid`: the child is a session leader.
    pid: u32,
    /// Set by the job task just before it finishes; `send`/`wait` loops read it.
    exit: Mutex<Option<JobExit>>,
}

impl PtyHandle {
    /// Current screen per 22 §4.3.
    pub(crate) fn screen(&self) -> (Vec<String>, (u16, u16)) {
        screen_text(lock(&self.parser).screen())
    }

    fn exit(&self) -> Option<JobExit> {
        lock(&self.exit).clone()
    }
}

/// `ask` → [`ToolOutput`]: spawn `args.command` on a pty as a job, wait for
/// grace or `ready`, reply per 22 §2.2.
pub struct StartPty {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id (carried on `JobEvent::Started`).
    pub call_id: String,
    /// The tool arguments.
    pub args: StartArgs,
    /// Resolved working directory.
    pub cwd: PathBuf,
    /// `ready` deadline when `args.ready.timeout` is absent.
    pub ready_timeout: Duration,
}

/// `ask` → `Result<ToolOutput, String>`: type into a PTY job, reply with the
/// settled screen (22 §4.5).
pub struct SendInput {
    /// Owning session.
    pub session: SessionId,
    /// The tool arguments (`id` inside).
    pub args: SendArgs,
}

/// `ask` → `Result<ToolOutput, String>`: the current screen.
pub struct Screen {
    /// Owning session.
    pub session: SessionId,
    /// `j4` or the job's `name`.
    pub id: String,
}

/// `ask` → `Result<ToolOutput, String>`: block for a pattern or exit (22 §4.6).
pub struct Wait {
    /// Owning session.
    pub session: SessionId,
    /// The tool arguments (`id` inside).
    pub args: WaitArgs,
}

/// A resolved PTY job as the tool paths see it, detached from the actor.
struct Live {
    id: JobId,
    name: Option<String>,
    log_path: PathBuf,
    started: Instant,
    pty: Arc<PtyHandle>,
}

impl Jobs {
    /// `id` resolution (22 §4.8): exact `j{n}`, else a running job named `id`,
    /// else a finished one for the error text.
    fn pty(&self, session: SessionId, id: &str) -> Result<Live, String> {
        let jobs = self
            .sessions
            .get(&session)
            .map_or(&[][..], |s| s.jobs.as_slice());
        let named = |j: &&Job| j.name.as_deref() == Some(id);
        let job = jobs
            .iter()
            .find(|j| j.id.0 == id)
            .or_else(|| jobs.iter().rev().filter(|j| j.exit.is_none()).find(named))
            .or_else(|| jobs.iter().rev().find(named))
            .ok_or_else(|| "no such job".to_owned())?;
        if let Some(exit) = &job.exit {
            return Err(format!(
                "{} {exit} — start it again",
                label(&job.id, job.name.as_deref())
            ));
        }
        let Some(pty) = &job.pty else {
            return Err(format!(
                "{} is a background `shell` job, not a PTY: `wait`/`send`/`screen` do not apply. \
                 Its result arrives as <job_result id=\"{}\"> when it exits — keep working or end the turn.",
                job.id, job.id
            ));
        };
        Ok(Live {
            id: job.id.clone(),
            name: job.name.clone(),
            log_path: job.log_path.clone(),
            started: job.started,
            pty: Arc::clone(pty),
        })
    }
}

impl Message<StartPty> for Jobs {
    type Reply = DelegatedReply<ToolOutput>;

    async fn handle(&mut self, m: StartPty, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let re = match m
            .args
            .ready
            .as_ref()
            .and_then(|r| r.log.as_deref())
            .map(Regex::new)
        {
            Some(Err(e)) => {
                tx.send(ToolOutput::err(format!("ready.log: {e}")));
                return reply;
            }
            Some(Ok(re)) => Some(re),
            None => None,
        };
        if let Some(name) = &m.args.name
            && let Some(j) = self.sessions.get(&m.session).and_then(|s| {
                s.jobs
                    .iter()
                    .find(|j| j.exit.is_none() && j.name.as_deref() == Some(name))
            })
        {
            tx.send(ToolOutput::err(format!(
                "a running job is already named `{name}` ({})",
                j.id
            )));
            return reply;
        }
        let sj = self.sessions.entry(m.session).or_default();
        sj.next += 1;
        let id = JobId(format!("j{}", sj.next));
        let dir = self.rt.data_dir.join("jobs").join(m.session.to_string());
        let log_path = dir.join(format!("{id}.log"));
        let s = match spawn(&dir, &log_path, &m.args, &m.cwd) {
            Ok(s) => s,
            Err(e) => {
                tx.send(ToolOutput::err(format!("cannot start job: {e:#}")));
                return reply;
            }
        };
        let pty = Arc::new(PtyHandle {
            writer: Mutex::new(s.writer),
            parser: Mutex::new(vt100::Parser::new(ROWS, COLS, 0)),
            output: Notify::new(),
            pid: s.pid,
            exit: Mutex::new(None),
        });
        let log = Arc::new(Mutex::new(Log::new(s.file)));
        let cancel = Arc::new(Notify::new());
        let (done_tx, mut done_rx) = oneshot::channel();
        // Same claim protocol as the pipes path (14): first swap owns the result.
        let claimable = Arc::new(AtomicBool::new(true));
        let started = Instant::now();
        let (chunk_tx, rx) = mpsc::channel(64);
        std::thread::spawn(move || read_loop(s.reader, &chunk_tx));
        let task = tokio::spawn(run(Run {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            command: m.args.command.clone(),
            call_id: m.call_id,
            log_path: log_path.clone(),
            log: log.clone(),
            pty: Arc::clone(&pty),
            master: s.master,
            child: s.child,
            rx,
            cancel: cancel.clone(),
            claimable: claimable.clone(),
            started,
            done: done_tx,
        }))
        .abort_handle();
        sj.push(Job {
            id: id.clone(),
            command: m.args.command,
            started,
            exit: None,
            secs: 0,
            log_path: log_path.clone(),
            log,
            pgids: Arc::new(Mutex::new(vec![s.pid as i32])),
            cancel,
            task,
            name: m.args.name.clone(),
            pty: Some(Arc::clone(&pty)),
        });
        let grace = self.grace;
        let ready_timeout = m.ready_timeout;
        tokio::spawn(async move {
            // `Some`: the task finished; `None`: grace / readiness ended first.
            let (note, within) = match m.args.ready {
                None if grace.is_zero() => (String::new(), None),
                None => (
                    String::new(),
                    tokio::time::timeout(grace, &mut done_rx).await.ok(),
                ),
                Some(r) => {
                    let secs = r.timeout.unwrap_or(ready_timeout.as_secs()).min(READY_MAX);
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
                    let host = r.host.as_deref().unwrap_or("127.0.0.1");
                    let port = r.port.map(|p| (host, p));
                    tokio::select! {
                        r = &mut done_rx => (String::new(), Some(r)),
                        o = await_output(&pty, &log_path, 0, re.as_ref(), port, deadline) => match o {
                            Outcome::Exited => (String::new(), Some((&mut done_rx).await)),
                            Outcome::Ready(matched) => {
                                let mut parts = Vec::new();
                                if let Some(m) = matched {
                                    parts.push(format!("log matched `{m}`"));
                                }
                                if let Some((_, p)) = port {
                                    parts.push(format!("port {p}"));
                                }
                                (format!(" · ready: {}", parts.join(", ")), None)
                            }
                            Outcome::TimedOut(pending) => (
                                format!(" · not ready after {secs}s (waiting on: {})", pending.join(", ")),
                                None,
                            ),
                        },
                    }
                }
            };
            let done = match within {
                Some(r) => r.ok(),
                None if claimable.swap(false, SeqCst) => None,
                None => done_rx.await.ok(),
            };
            tx.send(done.unwrap_or_else(|| {
                let (rows, cursor) = pty.screen();
                let mut content = format!(
                    "job {} · running · pid {}{note}\nlog: {}\n",
                    label(&id, m.args.name.as_deref()),
                    pty.pid,
                    log_path.display()
                );
                content.push_str(&screen_block(&rows, cursor));
                ToolOutput {
                    content,
                    is_error: false,
                    structured: Some(serde_json::json!({
                        "job": id, "log": log_path, "pid": pty.pid, "rows": rows, "cursor": cursor,
                    })),
                    files: Vec::new(),
                }
            }));
        });
        reply
    }
}

impl Message<SendInput> for Jobs {
    type Reply = DelegatedReply<Result<ToolOutput, String>>;

    async fn handle(&mut self, m: SendInput, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.pty(m.session, &m.args.id) {
            Ok(live) => {
                tokio::spawn(async move { tx.send(send(&live, m.args).await) });
            }
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl Message<Screen> for Jobs {
    type Reply = Result<ToolOutput, String>;

    async fn handle(&mut self, m: Screen, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pty(m.session, &m.id)
            .map(|live| reply(&live, None, None))
    }
}

impl Message<Wait> for Jobs {
    type Reply = DelegatedReply<Result<ToolOutput, String>>;

    async fn handle(&mut self, m: Wait, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.pty(m.session, &m.args.id) {
            Ok(live) => {
                tokio::spawn(async move { tx.send(wait(&live, m.args).await) });
            }
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

/// What the spawn hands the job task and the handle.
struct Spawned {
    file: std::fs::File,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    pid: u32,
}

/// 22 §4.1: `$SHELL -lc <command>` on a fresh 120×40 pty (`setsid` +
/// `TIOCSCTTY` in the child), log file created.
fn spawn(dir: &Path, log_path: &Path, args: &StartArgs, cwd: &Path) -> anyhow::Result<Spawned> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::File::create(log_path)?;
    let pair = native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(["-lc", &args.command]);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    for (k, v) in args.env.iter().flatten() {
        cmd.env(k, v);
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let pid = child
        .process_id()
        .ok_or_else(|| anyhow::anyhow!("pty child has no pid"))?;
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    Ok(Spawned {
        file,
        master: pair.master,
        child,
        reader,
        writer,
        pid,
    })
}

/// Reader thread (22 §4.2): master → bounded channel; back-pressure lands in
/// the kernel's pty buffer. Ends on EOF/EIO (every slave closed) or when the
/// job task is gone.
// ponytail: the dup'd master fd here keeps the pty alive until every slave
// holder exits, so a `setsid` escapee delays the drop-master SIGHUP (22 §2.4)
// and parks this thread; poll(2) with a stop flag if that ever matters.
fn read_loop(mut r: Box<dyn Read + Send>, tx: &mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Everything one PTY job task owns.
struct Run {
    rt: Runtime,
    me: ActorRef<Jobs>,
    session: SessionId,
    id: JobId,
    command: String,
    call_id: String,
    log_path: PathBuf,
    log: Arc<Mutex<Log>>,
    pty: Arc<PtyHandle>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: mpsc::Receiver<Vec<u8>>,
    cancel: Arc<Notify>,
    claimable: Arc<AtomicBool>,
    started: Instant,
    done: oneshot::Sender<ToolOutput>,
}

fn exit_of(
    r: Result<std::io::Result<portable_pty::ExitStatus>, tokio::task::JoinError>,
) -> JobExit {
    match r {
        Ok(Ok(st)) => JobExit::Exited(u8::try_from(st.exit_code()).unwrap_or(1)),
        _ => JobExit::Exited(1),
    }
}

/// The job task (22 §4.2): ingest chunks, coalesce `Screen` events, detect
/// exit (reaper or EOF, whichever first), then the 14 finish path with the
/// final screen as `output`.
async fn run(mut j: Run) {
    j.rt.publish(JobEvent::Started {
        session: j.session,
        id: j.id.clone(),
        command: j.command.clone(),
        call_id: j.call_id,
        log: j.log_path,
    })
    .await;

    let mut child = j.child;
    let mut waited = tokio::task::spawn_blocking(move || child.wait());
    let pid = Pid::from_raw(j.pty.pid as i32);
    let mut carry = Vec::new();
    let mut dirty = false;
    let mut tick = tokio::time::interval(jobs::OUTPUT_EVERY);
    let exit = loop {
        tokio::select! {
            biased;
            () = j.cancel.notified() => {
                let _ = killpg(pid, nix::sys::signal::Signal::SIGKILL);
                break JobExit::Killed;
            }
            r = &mut waited => break exit_of(r),
            chunk = j.rx.recv() => match chunk {
                Some(c) => {
                    ingest(&j.pty, &j.log, &mut carry, &c);
                    dirty = true;
                }
                // EOF: every slave closed. A child that closed its tty and
                // lingers gets a second, then SIGKILL.
                None => break match tokio::time::timeout(Duration::from_secs(1), &mut waited).await {
                    Ok(r) => exit_of(r),
                    Err(_) => {
                        let _ = killpg(pid, nix::sys::signal::Signal::SIGKILL);
                        exit_of(waited.await)
                    }
                },
            },
            _ = tick.tick(), if dirty => {
                let (rows, cursor) = j.pty.screen();
                j.rt.publish(JobEvent::Screen { session: j.session, id: j.id.clone(), rows, cursor }).await;
                dirty = false;
            }
        }
    };
    if !matches!(exit, JobExit::Killed) {
        // Trailing bytes still in flight from the reader.
        while let Ok(Some(c)) = tokio::time::timeout(Duration::from_millis(200), j.rx.recv()).await
        {
            ingest(&j.pty, &j.log, &mut carry, &c);
        }
    }
    drop(j.master); // closes the slave: anything that escaped the group gets SIGHUP

    let (rows, cursor) = j.pty.screen();
    let screen = rows.join("\n");
    let secs = j.started.elapsed().as_secs();
    let out = match &exit {
        JobExit::Exited(code) => ToolOutput {
            content: format!("{screen}{}exit code: {code}", sep(&screen)),
            is_error: *code != 0,
            structured: Some(serde_json::Value::String(screen.clone())),
            files: Vec::new(),
        },
        JobExit::Timeout | JobExit::Killed => {
            ToolOutput::err(format!("{screen}{}killed", sep(&screen)))
        }
    };
    *lock(&j.pty.exit) = Some(exit.clone());
    j.pty.output.notify_waiters();
    j.rt.publish(JobEvent::Screen {
        session: j.session,
        id: j.id.clone(),
        rows,
        cursor,
    })
    .await;
    let inline = j.claimable.swap(false, SeqCst);
    let _ =
        j.me.tell(Finished {
            session: j.session,
            id: j.id.clone(),
            exit: exit.clone(),
            secs,
        })
        .await;
    j.rt.publish(JobEvent::Finished {
        session: j.session,
        id: j.id,
        command: j.command,
        exit,
        secs,
        output: screen,
        inline,
        pty: true,
    })
    .await;
    let _ = j.done.send(out);
}

/// One chunk: log, render, answer terminal queries, wake waiters.
fn ingest(pty: &PtyHandle, log: &Mutex<Log>, carry: &mut Vec<u8>, chunk: &[u8]) {
    lock(log).push(chunk);
    let cursor = {
        let mut p = lock(&pty.parser);
        p.process(chunk);
        p.screen().cursor_position()
    };
    respond(pty, carry, chunk, cursor);
    pty.output.notify_waiters();
}

/// 22 §4.3.
fn screen_text(s: &vt100::Screen) -> (Vec<String>, (u16, u16)) {
    let mut rows: Vec<String> = s
        .rows(0, COLS)
        .map(|mut r| {
            r.truncate(r.trim_end().len());
            r
        })
        .collect();
    let last = rows
        .iter()
        .rposition(|r| !r.is_empty())
        .map_or(0, |i| i + 1);
    rows.truncate(last);
    (rows, s.cursor_position())
}

/// Fixed-reply terminal queries (22 §4.4); `ESC[6n` is answered from the cursor.
const QUERIES: &[(&[u8], &[u8])] = &[
    (b"\x1b[5n", b"\x1b[0n"),
    (b"\x1b[c", b"\x1b[?62;22c"),
    (b"\x1b[0c", b"\x1b[?62;22c"),
    (b"\x1b[>c", b"\x1b[>41;0;0c"),
    (b"\x1b[>0c", b"\x1b[>41;0;0c"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
];
const CPR: &[u8] = b"\x1b[6n";

enum Scan {
    /// A query of this many bytes; write the reply.
    Reply(usize, Cow<'static, [u8]>),
    /// The rest of the chunk is a strict prefix of a query: carry it.
    Partial,
    None,
}

/// Classify the bytes at an ESC.
fn query(s: &[u8], cursor: (u16, u16)) -> Scan {
    if s.starts_with(CPR) {
        let reply = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
        return Scan::Reply(CPR.len(), Cow::Owned(reply.into_bytes()));
    }
    for (q, r) in QUERIES {
        if s.starts_with(q) {
            return Scan::Reply(q.len(), Cow::Borrowed(r));
        }
    }
    let prefix = std::iter::once(CPR)
        .chain(QUERIES.iter().map(|(q, _)| *q))
        .any(|q| q.len() > s.len() && q.starts_with(s));
    if prefix { Scan::Partial } else { Scan::None }
}

/// The query responder (22 §4.4): replies go to the master, never the log.
fn respond(pty: &PtyHandle, carry: &mut Vec<u8>, chunk: &[u8], cursor: (u16, u16)) {
    let data: Cow<[u8]> = if carry.is_empty() {
        Cow::Borrowed(chunk)
    } else {
        carry.extend_from_slice(chunk);
        Cow::Owned(std::mem::take(carry))
    };
    let mut i = 0;
    while let Some(off) = data[i..].iter().position(|&b| b == 0x1b) {
        let at = i + off;
        match query(&data[at..], cursor) {
            Scan::Reply(len, reply) => {
                let mut w = lock(&pty.writer);
                let _ = w.write_all(&reply).and_then(|()| w.flush());
                i = at + len;
            }
            Scan::Partial => {
                if data.len() - at <= CARRY {
                    carry.extend_from_slice(&data[at..]);
                }
                break;
            }
            Scan::None => i = at + 1,
        }
    }
}

/// Drop ESC sequences (CSI, OSC, two-byte) so a pattern matches through colour
/// codes (22 §4.6); lossy UTF-8 of the rest.
fn strip(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes.get(i) {
            Some(b'[') => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(b']') => {
                while i < bytes.len()
                    && bytes[i] != 0x07
                    && !(bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\'))
                {
                    i += 1;
                }
                i += if bytes.get(i) == Some(&0x1b) { 2 } else { 1 };
            }
            // `ESC ( B` and friends: intermediates 0x20..=0x2f, then one final byte.
            Some(b) if (0x20..=0x2f).contains(b) => {
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(_) => i += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn log_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// First `re` match in the log bytes appended since offset `from`.
fn find_since(path: &Path, from: u64, re: &Regex) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    re.find(&strip(&buf)).map(|m| m.as_str().to_owned())
}

async fn connects(host: &str, port: u16) -> bool {
    tokio::time::timeout(PORT_POLL, tokio::net::TcpStream::connect((host, port)))
        .await
        .is_ok_and(|r| r.is_ok())
}

enum Outcome {
    /// Every condition passed; the log match if there was a pattern.
    Ready(Option<String>),
    Exited,
    /// What was still pending: `log \`re\``, `port N`.
    TimedOut(Vec<String>),
}

/// The shared loop of `wait` (22 §4.6) and `ready` (22 §4.7): wake on output
/// (or every [`PORT_POLL`] for a port), re-check, until every condition
/// passes, the job exits, or `deadline`. No pattern and no port → exit only.
async fn await_output(
    pty: &PtyHandle,
    log: &Path,
    from: u64,
    re: Option<&Regex>,
    port: Option<(&str, u16)>,
    deadline: tokio::time::Instant,
) -> Outcome {
    let exit_only = re.is_none() && port.is_none();
    let mut need_log = re.is_some();
    let mut need_port = port.is_some();
    let mut matched = None;
    loop {
        // Created before the checks so a wake between check and await counts.
        let notified = pty.output.notified();
        if pty.exit().is_some() {
            return Outcome::Exited;
        }
        if need_log && let Some(m) = re.and_then(|re| find_since(log, from, re)) {
            matched = Some(m);
            need_log = false;
        }
        if need_port && let Some((h, p)) = port {
            need_port = !connects(h, p).await;
        }
        if !exit_only && !need_log && !need_port {
            return Outcome::Ready(matched);
        }
        tokio::select! {
            () = notified, if need_log || exit_only => {}
            () = tokio::time::sleep(PORT_POLL), if need_port => {}
            () = tokio::time::sleep_until(deadline) => {
                let mut pending = Vec::new();
                if let (true, Some(re)) = (need_log, re) {
                    pending.push(format!("log `{}`", re.as_str()));
                }
                if let (true, Some((_, p))) = (need_port, port) {
                    pending.push(format!("port {p}"));
                }
                return Outcome::TimedOut(pending);
            }
        }
    }
}

/// 22 §4.5: write, signal, settle, screen.
async fn send(live: &Live, args: SendArgs) -> Result<ToolOutput, String> {
    let mut data = Vec::new();
    if let Some(text) = &args.text {
        data.extend_from_slice(text.as_bytes());
        if args.enter.unwrap_or(true) {
            data.push(b'\r');
        }
    }
    for k in args.keys.iter().flatten() {
        data.extend_from_slice(k.bytes());
    }
    if !data.is_empty() {
        let pty = Arc::clone(&live.pty);
        // A full kernel buffer blocks the write; keep it off the runtime.
        tokio::task::spawn_blocking(move || {
            let mut w = lock(&pty.writer);
            w.write_all(&data).and_then(|()| w.flush())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("write to pty: {e}"))?;
    }
    if let Some(sig) = args.signal {
        killpg(
            Pid::from_raw(live.pty.pid as i32),
            nix::sys::signal::Signal::from(sig),
        )
        .map_err(|e| format!("signal: {e}"))?;
    }
    settle(&live.pty).await;
    Ok(reply(live, None, None))
}

/// Quiet for [`SETTLE_QUIET`], or [`SETTLE_MAX`] total, or exit.
async fn settle(pty: &PtyHandle) {
    let end = tokio::time::Instant::now() + SETTLE_MAX;
    loop {
        let notified = pty.output.notified();
        if pty.exit().is_some() {
            return;
        }
        tokio::select! {
            () = notified => {}
            () = tokio::time::sleep(SETTLE_QUIET) => return,
            () = tokio::time::sleep_until(end) => return,
        }
    }
}

/// 22 §4.6.
async fn wait(live: &Live, args: WaitArgs) -> Result<ToolOutput, String> {
    let re = args
        .pattern
        .as_deref()
        .map(Regex::new)
        .transpose()
        .map_err(|e| format!("pattern: {e}"))?;
    let secs = args.timeout.unwrap_or(WAIT_DEFAULT).min(WAIT_MAX);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let from = log_len(&live.log_path);
    let (note, matched) =
        match await_output(&live.pty, &live.log_path, from, re.as_ref(), None, deadline).await {
            Outcome::Ready(m) => (m.as_deref().map(|m| format!("matched: {m:?}")), m),
            Outcome::TimedOut(_) => (Some(format!("timed out after {secs}s")), None),
            Outcome::Exited => (None, None),
        };
    Ok(reply(live, note.as_deref(), matched.as_deref()))
}

fn label(id: &JobId, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("{id} ({n})"),
        None => id.0.clone(),
    }
}

fn screen_block(rows: &[String], cursor: (u16, u16)) -> String {
    let mut s = String::from("screen:\n");
    for r in rows {
        s.push_str(r);
        s.push('\n');
    }
    let _ = write!(s, "cursor: row {} col {}", cursor.0 + 1, cursor.1 + 1);
    s
}

/// The `send` / `screen` / `wait` reply (22 §2.2): state line, optional note,
/// screen; `structured` mirrors it.
fn reply(live: &Live, note: Option<&str>, matched: Option<&str>) -> ToolOutput {
    let (rows, cursor) = live.pty.screen();
    let state = live
        .pty
        .exit()
        .map_or_else(|| "running".to_owned(), |e| e.to_string());
    let mut content = format!(
        "job {} · {state} · {}s\n",
        label(&live.id, live.name.as_deref()),
        live.started.elapsed().as_secs()
    );
    if let Some(n) = note {
        content.push_str(n);
        content.push('\n');
    }
    content.push_str(&screen_block(&rows, cursor));
    ToolOutput {
        content,
        is_error: false,
        structured: Some(serde_json::json!({
            "job": live.id, "rows": rows, "cursor": cursor, "matched": matched,
        })),
        files: Vec::new(),
    }
}

/// kameo lifts a `Result` reply: the handler's `Err(String)` arrives as
/// `SendError::HandlerError`; that is the model's error text.
fn lift<M>(
    r: Result<ToolOutput, kameo::error::SendError<M, String>>,
) -> anyhow::Result<ToolOutput> {
    match r {
        Ok(out) => Ok(out),
        Err(kameo::error::SendError::HandlerError(e)) => Ok(ToolOutput::err(e)),
        Err(e) => anyhow::bail!("jobs actor: {e}"),
    }
}

/// `start`: a command on a pty as a job with stdin and a screen.
pub struct StartTool {
    jobs: ActorRef<Jobs>,
    shells: Arc<Mutex<HashMap<SessionId, Shell>>>,
    ready_timeout: Duration,
}

impl StartTool {
    /// `shells` is [`crate::shell::ShellTool::shells`]: the default cwd is the
    /// session shell's (22 §1.5); `ready_timeout` is `tools.pty_ready_timeout_secs`.
    pub fn new(
        jobs: ActorRef<Jobs>,
        shells: Arc<Mutex<HashMap<SessionId, Shell>>>,
        ready_timeout: Duration,
    ) -> Self {
        Self {
            jobs,
            shells,
            ready_timeout,
        }
    }
}

#[async_trait::async_trait]
impl Tool for StartTool {
    const NAME: &'static str = "start";
    const RISK: Risk = Risk::Destructive;
    type Args = StartArgs;

    fn description(&self) -> String {
        "Run a command on a pseudo-terminal as a named job you can type into (REPLs, servers, TUIs, prompts). Returns the job id and the screen; with `ready`, waits for a log regex and/or a TCP port first. cwd defaults to the shell's; variables and functions do not carry over. Use `shell` for anything that just runs and exits.".into()
    }

    async fn call(&self, args: StartArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cwd = match &args.cwd {
            Some(c) => c.clone(),
            None => match lock(&self.shells).get(&cx.session) {
                Some(s) => s.working_dir().to_path_buf(),
                None => std::env::current_dir()?,
            },
        };
        self.jobs
            .ask(StartPty {
                session: cx.session,
                call_id: cx.call_id,
                args,
                cwd,
                ready_timeout: self.ready_timeout,
            })
            .await
            .map_err(|e| anyhow::anyhow!("jobs actor: {e}"))
    }
}

/// `send`: type into a PTY job.
pub struct SendTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for SendTool {
    const NAME: &'static str = "send";
    const RISK: Risk = Risk::Mutating;
    type Args = SendArgs;

    fn description(&self) -> String {
        "Type into a PTY job: `text` (+ Enter unless `enter: false`), then `keys`, or deliver a `signal`. Returns the screen once output settles.".into()
    }

    async fn call(&self, args: SendArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(SendInput {
                    session: cx.session,
                    args,
                })
                .await,
        )
    }
}

/// `screen`: the current screen of a PTY job.
pub struct ScreenTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for ScreenTool {
    const NAME: &'static str = "screen";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ScreenArgs;

    fn description(&self) -> String {
        "The current screen of a PTY job (120×40, blank rows dropped, cursor on the last line). For line output the log file under `read_file` is the full history.".into()
    }

    async fn call(&self, args: ScreenArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(Screen {
                    session: cx.session,
                    id: args.id,
                })
                .await,
        )
    }
}

/// `wait`: block for a pattern or exit.
pub struct WaitTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for WaitTool {
    const NAME: &'static str = "wait";
    const RISK: Risk = Risk::ReadOnly;
    type Args = WaitArgs;

    fn description(&self) -> String {
        "Block until a PTY job (started with `start`) prints something matching `pattern`, or exits; at most `timeout` seconds (default 30). Returns the match and the screen. Use it for prompts and readiness, not to babysit a build — end the turn and the `<job_result>` wakes you. Not for `shell` jobs: those cannot be waited on and report as `<job_result>` on their own.".into()
    }

    async fn call(&self, args: WaitArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(Wait {
                    session: cx.session,
                    args,
                })
                .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::CancelJob;
    use crate::jobs::tests::{Fx, finished, fixture, list, settled};

    fn cx(fx: &Fx) -> ToolCx {
        ToolCx {
            session: fx.session,
            call_id: String::new(),
        }
    }

    fn tools(fx: &Fx) -> (StartTool, SendTool, ScreenTool, WaitTool) {
        (
            StartTool::new(fx.jobs.clone(), Arc::default(), Duration::from_secs(30)),
            SendTool(fx.jobs.clone()),
            ScreenTool(fx.jobs.clone()),
            WaitTool(fx.jobs.clone()),
        )
    }

    fn start_args(command: &str) -> StartArgs {
        StartArgs {
            command: command.into(),
            name: None,
            cwd: None,
            env: None,
            ready: None,
        }
    }

    fn send_args(id: &str) -> SendArgs {
        SendArgs {
            id: id.into(),
            text: None,
            enter: None,
            keys: None,
            signal: None,
        }
    }

    fn rows(out: &ToolOutput) -> Vec<String> {
        serde_json::from_value(out.structured.as_ref().unwrap()["rows"].clone()).unwrap()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn cancel(fx: &Fx, id: &str) {
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId(id.into()),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_round_trip() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (start, send, ..) = tools(&fx);
        let mut args = start_args("python3 -q");
        args.ready = Some(Ready {
            log: Some(">>> ".into()),
            port: None,
            host: None,
            timeout: None,
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.starts_with("job j1 · running · pid ")
                && out.content.contains("· ready: log matched `>>> `"),
            "{}",
            out.content
        );
        assert_eq!(
            rows(&out).last().map(String::as_str),
            Some(">>>"),
            "{}",
            out.content
        );

        let mut args = send_args("j1");
        args.text = Some("1+1".into());
        let out = Tool::call(&send, args, cx(&fx)).await.unwrap();
        let rows = rows(&out);
        assert!(rows.iter().any(|r| r == "2"), "{}", out.content);
        assert!(
            rows.iter().filter(|r| r.starts_with(">>>")).count() >= 2,
            "{}",
            out.content
        );
        assert!(
            out.content.starts_with("job j1 · running · "),
            "{}",
            out.content
        );

        let mut args = send_args("j1");
        args.keys = Some(vec![Key::CtrlD]);
        let out = Tool::call(&send, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.starts_with("job j1 · exited 0 · "),
            "{}",
            out.content
        );
        assert_eq!(finished(&fx).await.0, JobExit::Exited(0));
        assert!(matches!(
            lock(&fx.events).iter().last(),
            Some(JobEvent::Finished {
                pty: true,
                inline: false,
                ..
            })
        ));
        let v = settled(&fx, Duration::from_secs(1)).await;
        assert!(v.pty && v.state == "exited 0", "{v:?}");
    }

    #[tokio::test]
    async fn screen_is_rendered_not_bytes() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(&start, start_args(r"printf 'abc\rX\n'"), cx(&fx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.ends_with("exit code: 0"), "{}", out.content);
        let (exit, screen, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Exited(0));
        assert!(inline);
        assert!(screen.lines().any(|l| l == "Xbc"), "{screen:?}");
        let log = std::fs::read(&list(&fx).await[0].log).unwrap();
        assert!(log.windows(5).any(|w| w == b"abc\rX"), "{log:?}");
    }

    #[tokio::test]
    async fn query_responder() {
        let fx = fixture(Duration::from_secs(5), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(
            &start,
            start_args(
                r#"stty -echo; printf '\033[H\033[2J\033[6n'; read -t 2 -rs -d R r; echo "${r#*[}""#,
            ),
            cx(&fx),
        )
        .await
        .unwrap();
        let (_, screen, _) = finished(&fx).await;
        assert!(screen.lines().any(|l| l == "1;1"), "{}", out.content);
        let log = std::fs::read_to_string(&list(&fx).await[0].log).unwrap();
        assert!(!log.contains("1;1R"), "{log:?}");
    }

    #[tokio::test]
    async fn wait_pattern_and_timeout() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, _, wait) = tools(&fx);
        let out = Tool::call(&start, start_args("sleep 1; echo READY; sleep 30"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let t = Instant::now();
        let out = Tool::call(
            &wait,
            WaitArgs {
                id: "j1".into(),
                pattern: Some("REA+DY".into()),
                timeout: None,
            },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(
            out.content.contains("matched: \"READY\""),
            "{}",
            out.content
        );
        assert_eq!(out.structured.as_ref().unwrap()["matched"], "READY");
        let out = Tool::call(
            &wait,
            WaitArgs {
                id: "j1".into(),
                pattern: Some("NEVER".into()),
                timeout: Some(1),
            },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("timed out after 1s"),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
    }

    #[tokio::test]
    async fn ready_port() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let port = free_port();
        let mut args = start_args(&format!("python3 -m http.server {port}"));
        args.ready = Some(Ready {
            log: None,
            port: Some(port),
            host: None,
            timeout: None,
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.contains(&format!("· ready: port {port}")),
            "{}",
            out.content
        );

        let wrong = free_port();
        let mut args = start_args("sleep 30");
        args.ready = Some(Ready {
            log: None,
            port: Some(wrong),
            host: None,
            timeout: Some(1),
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(!out.is_error);
        assert!(
            out.content
                .contains(&format!("not ready after 1s (waiting on: port {wrong})")),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
        cancel(&fx, "j2").await;
    }

    #[tokio::test]
    async fn name_resolves_and_cancel_kills() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, screen, _) = tools(&fx);
        let mut args = start_args("sleep 30");
        args.name = Some("srv".into());
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.starts_with("job j1 (srv) · running"),
            "{}",
            out.content
        );
        let pid = out.structured.as_ref().unwrap()["pid"].as_u64().unwrap() as i32;

        let out = Tool::call(&screen, ScreenArgs { id: "srv".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 (srv) · running"),
            "{}",
            out.content
        );
        let table = jobs::table(&list(&fx).await);
        assert!(table.contains("pty srv: sleep 30"), "{table}");

        // A second running job may not reuse the name.
        let mut args = start_args("sleep 30");
        args.name = Some("srv".into());
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.is_error && out.content.contains("already named `srv` (j1)"),
            "{}",
            out.content
        );

        cancel(&fx, "j1").await;
        assert_eq!(settled(&fx, Duration::from_secs(2)).await.state, "killed");
        let gone = Instant::now() + Duration::from_secs(1);
        while nix::sys::signal::kill(Pid::from_raw(pid), None).is_ok() {
            assert!(Instant::now() < gone, "pid {pid} still alive");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let out = Tool::call(&screen, ScreenArgs { id: "srv".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(out.is_error);
        assert_eq!(out.content, "j1 (srv) killed — start it again");
    }

    #[test]
    fn strip_and_queries() {
        assert_eq!(
            strip(b"\x1b[32m>>> \x1b[0m\x1b]0;title\x07x\x1b(B"),
            ">>> x"
        );
        assert!(matches!(query(b"\x1b[6n", (1, 4)), Scan::Reply(4, r) if &*r == b"\x1b[2;5R"));
        assert!(matches!(query(b"\x1b]11;", (0, 0)), Scan::Partial));
        assert!(matches!(query(b"\x1b[31m", (0, 0)), Scan::None));
    }
}
