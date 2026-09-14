//! `Jobs` actor (14 §2): every process the model starts is a background job.
//! Owns each job's process groups, streams output to
//! `<data_dir>/jobs/<session>/<id>.log`, publishes [`JobEvent`]s on the bus,
//! and kills everything still running on `rt.shutdown`. Registered as
//! [`JOBS_ACTOR`]; `shell` starts jobs with [`Start`], `jobs` / `cancel_job`
//! (also here) ask [`ListJobs`] / [`CancelJob`].

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use brush_core::{ExecutionParameters, ProcessGroupPolicy, Shell, SourceInfo};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, oneshot};
use tokio::task::AbortHandle;

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Registry name of the [`Jobs`] actor.
pub const JOBS_ACTOR: &str = "jobs";
/// Output longer than this is truncated before it reaches the model.
const MAX_OUTPUT: usize = 64 * 1024;
/// Bytes of recent output kept per job for stubs, `jobs` and [`JobView`].
const TAIL: usize = 8 * 1024;
/// Bytes of tail quoted in the still-running `shell` reply.
const STUB_TAIL: usize = 2 * 1024;
/// Finished rows kept per session before the oldest go.
const MAX_ROWS: usize = 32;
/// Minimum spacing of [`JobEvent::Output`] / [`JobEvent::Screen`] per job.
pub(crate) const OUTPUT_EVERY: Duration = Duration::from_millis(500);

/// `j{n}`, `n` per session starting at 1.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct JobId(pub String);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(&self.0)
    }
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum JobExit {
    /// The command returned this code (a shell error is `1`).
    Exited(u8),
    /// `tools.job_timeout_secs` elapsed; the process group was killed.
    Timeout,
    /// [`CancelJob`] killed the process group.
    Killed,
}

impl std::fmt::Display for JobExit {
    /// The [`JobView::state`] wording: `exited 0`, `timeout`, `killed`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobExit::Exited(code) => write!(f, "exited {code}"),
            JobExit::Timeout => f.write_str("timeout"),
            JobExit::Killed => f.write_str("killed"),
        }
    }
}

/// Bus events. `Output` is coalesced to one per [`OUTPUT_EVERY`] per job and
/// lossy under a flood; the log file is the truth. PTY jobs (22 §4.8) publish
/// `Screen` (coalesced the same way) instead of `Output`. Wire tags are
/// `JobStarted` | `JobOutput` | `JobScreen` | `JobFinished` so they read
/// unambiguously on the shared RPC event stream.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum JobEvent {
    /// A job was spawned.
    #[serde(rename = "JobStarted")]
    Started {
        /// Owning session.
        session: SessionId,
        /// Job.
        id: JobId,
        /// Command line.
        command: String,
        /// The `shell` call that started it: UIs attach live output to its card.
        call_id: String,
        /// Log file receiving stdout+stderr as they arrive.
        log: PathBuf,
    },
    /// Bytes since the previous `Output`.
    #[serde(rename = "JobOutput")]
    Output {
        /// Owning session.
        session: SessionId,
        /// Job.
        id: JobId,
        /// Lossy UTF-8 of the new bytes.
        chunk: String,
    },
    /// A PTY job's rendered screen (22 §4.3): blank trailing rows dropped,
    /// rows right-trimmed. Not published for pipe jobs.
    #[serde(rename = "JobScreen")]
    Screen {
        /// Owning session.
        session: SessionId,
        /// Job.
        id: JobId,
        /// Non-blank prefix of the 40 screen rows.
        rows: Vec<String>,
        /// `(row, col)`, zero-based.
        cursor: (u16, u16),
    },
    /// The job ended; the agent injects this as `<job_result>` unless `inline`.
    #[serde(rename = "JobFinished")]
    Finished {
        /// Owning session.
        session: SessionId,
        /// Job.
        id: JobId,
        /// Command line.
        command: String,
        /// How it ended.
        exit: JobExit,
        /// Wall time from spawn to exit.
        secs: u64,
        /// stdout+stderr, truncated to 64 KiB; the agent applies spill. For
        /// a PTY job the final screen text (22 §2.2).
        output: String,
        /// The tool call already returned this result (exited within the
        /// grace window); UIs still count it, the agent must not inject it.
        inline: bool,
        /// Ran on a pseudo-terminal (22): `output` is a screen, not a log.
        pty: bool,
    },
}

/// One row of [`ListJobs`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobView {
    /// Job.
    pub id: JobId,
    /// Command line.
    pub command: String,
    /// `running` | `exited 0` | `timeout` | `killed`.
    pub state: String,
    /// Seconds running so far, or total when finished.
    pub secs: u64,
    /// Log file.
    pub log: PathBuf,
    /// Last ≤ 8 KiB of output; for a PTY job the current screen text.
    pub tail: String,
    /// `start`'s `name` label (22 §2.1), PTY jobs only.
    pub name: Option<String>,
    /// Runs on a pseudo-terminal (22).
    pub pty: bool,
}

/// Receives a finished job's shell (14 §0.5: last finished wins).
pub type PutBack = Arc<dyn Fn(Shell) + Send + Sync>;

/// `ask` → [`StartReply`]: spawn `command` on `shell` as a job and wait up to
/// the actor's grace for it to exit.
pub struct Start {
    /// Owning session.
    pub session: SessionId,
    /// Bash command line.
    pub command: String,
    /// Provider call id of the `shell` call (carried on `JobEvent::Started`).
    pub call_id: String,
    /// A clone of the session shell; the job owns it and hands it to
    /// `put_back` when it exits normally (not when killed or timed out).
    pub shell: Shell,
    /// Where the finished shell goes.
    pub put_back: PutBack,
}

/// Reply to [`Start`].
#[derive(Clone, Debug, kameo::Reply)]
pub enum StartReply {
    /// Exited within grace: the `shell` tool output, today's shape.
    Done(ToolOutput),
    /// Still running after grace.
    Running {
        /// Job.
        id: JobId,
        /// Log file.
        log: PathBuf,
        /// Last ≤ 2 KiB of output so far.
        tail: String,
    },
}

/// `ask` → `Vec<JobView>` for the session, oldest first.
#[derive(Clone, Copy, Debug)]
pub struct ListJobs(pub SessionId);

/// `ask` → `Result<(), String>`: SIGKILL a running job's process groups.
/// Err `no such job` | `j3 already finished`.
#[derive(Clone, Debug)]
pub struct CancelJob {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub id: JobId,
}

/// `tell` from a job task: record how it ended.
pub(crate) struct Finished {
    pub(crate) session: SessionId,
    pub(crate) id: JobId,
    pub(crate) exit: JobExit,
    pub(crate) secs: u64,
}

/// `tell` on `rt.shutdown`: kill every running job's process groups.
struct KillAll;

/// Output as it arrives: appended to the log file, the last [`TAIL`] bytes
/// kept, and the bytes since the last [`JobEvent::Output`] (capped the same way).
pub(crate) struct Log {
    file: std::fs::File,
    tail: VecDeque<u8>,
    pending: VecDeque<u8>,
}

impl Log {
    pub(crate) fn new(file: std::fs::File) -> Self {
        Self {
            file,
            tail: VecDeque::with_capacity(TAIL),
            pending: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        let _ = self.file.write_all(chunk);
        keep_last(&mut self.tail, chunk);
        keep_last(&mut self.pending, chunk);
    }

    /// Last ≤ `max` bytes of the tail, cut on a char boundary.
    fn tail_text(&self, max: usize) -> String {
        let bytes: Vec<u8> = self.tail.iter().copied().collect();
        let mut start = bytes.len().saturating_sub(max);
        while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
            start += 1;
        }
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

fn keep_last(buf: &mut VecDeque<u8>, chunk: &[u8]) {
    let over = (buf.len() + chunk.len()).saturating_sub(TAIL);
    buf.drain(..over.min(buf.len()));
    buf.extend(&chunk[chunk.len().saturating_sub(TAIL)..]);
}

pub(crate) struct Job {
    pub(crate) id: JobId,
    pub(crate) command: String,
    pub(crate) started: Instant,
    /// `None` while running.
    pub(crate) exit: Option<JobExit>,
    /// Total seconds once finished.
    pub(crate) secs: u64,
    pub(crate) log_path: PathBuf,
    pub(crate) log: Arc<Mutex<Log>>,
    pub(crate) pgids: Arc<Mutex<Vec<i32>>>,
    pub(crate) cancel: Arc<Notify>,
    pub(crate) task: AbortHandle,
    /// `start`'s label (22 §2.1); PTY jobs only.
    pub(crate) name: Option<String>,
    /// The pseudo-terminal, for PTY jobs (22 §4.8).
    pub(crate) pty: Option<Arc<crate::pty::PtyHandle>>,
}

impl Job {
    fn secs(&self) -> u64 {
        if self.exit.is_some() {
            self.secs
        } else {
            self.started.elapsed().as_secs()
        }
    }

    fn view(&self) -> JobView {
        JobView {
            id: self.id.clone(),
            command: self.command.clone(),
            state: self
                .exit
                .as_ref()
                .map_or_else(|| "running".to_owned(), ToString::to_string),
            secs: self.secs(),
            log: self.log_path.clone(),
            tail: match &self.pty {
                Some(pty) => pty.screen().0.join("\n"),
                None => lock(&self.log).tail_text(TAIL),
            },
            name: self.name.clone(),
            pty: self.pty.is_some(),
        }
    }
}

#[derive(Default)]
pub(crate) struct SessionJobs {
    pub(crate) next: u32,
    pub(crate) jobs: Vec<Job>,
}

impl SessionJobs {
    /// Append a row, dropping the oldest finished ones past [`MAX_ROWS`].
    pub(crate) fn push(&mut self, job: Job) {
        self.jobs.push(job);
        while self.jobs.len() > MAX_ROWS {
            match self.jobs.iter().position(|j| j.exit.is_some()) {
                Some(i) => {
                    self.jobs.remove(i);
                }
                None => break,
            }
        }
    }
}

/// The jobs actor. Look up with `ActorRef::<Jobs>::lookup(JOBS_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Jobs {
    pub(crate) rt: Runtime,
    pub(crate) grace: Duration,
    timeout: Duration,
    pub(crate) sessions: HashMap<SessionId, SessionJobs>,
}

impl Jobs {
    /// Build with the runtime (log dir, bus), the inline-reply grace and the
    /// per-job deadline.
    pub fn new(rt: Runtime, grace: Duration, timeout: Duration) -> Self {
        Self {
            rt,
            grace,
            timeout,
            sessions: HashMap::new(),
        }
    }

    fn job_mut(&mut self, session: SessionId, id: &JobId) -> Option<&mut Job> {
        self.sessions
            .get_mut(&session)?
            .jobs
            .iter_mut()
            .find(|j| j.id == *id)
    }
}

/// Spawn and register the actor, and kill every running job when `rt.shutdown`
/// fires (00 §12: the `setsid` escape stays).
pub fn start(rt: &Runtime, grace: Duration, timeout: Duration) -> anyhow::Result<ActorRef<Jobs>> {
    let jobs = Jobs::spawn_with_mailbox(
        Jobs::new(rt.clone(), grace, timeout),
        runtime::subscriber_mailbox(),
    );
    jobs.register(JOBS_ACTOR)?;
    let shutdown = rt.shutdown.clone();
    let me = jobs.clone();
    tokio::spawn(async move {
        shutdown.cancelled().await;
        let _ = me.tell(KillAll).await;
    });
    Ok(jobs)
}

impl Message<Start> for Jobs {
    type Reply = DelegatedReply<StartReply>;

    async fn handle(&mut self, m: Start, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let sj = self.sessions.entry(m.session).or_default();
        sj.next += 1;
        let id = JobId(format!("j{}", sj.next));
        let dir = self.rt.data_dir.join("jobs").join(m.session.to_string());
        let log_path = dir.join(format!("{id}.log"));
        let pgids: Arc<Mutex<Vec<i32>>> = Arc::default();
        let (file, params, out_r, err_r) = match setup(&dir, &log_path, &m.shell, &pgids) {
            Ok(s) => s,
            Err(e) => {
                tx.send(StartReply::Done(ToolOutput::err(format!(
                    "cannot start job: {e:#}"
                ))));
                return reply;
            }
        };
        let log = Arc::new(Mutex::new(Log::new(file)));
        let cancel = Arc::new(Notify::new());
        let (done_tx, mut done_rx) = oneshot::channel();
        // Whoever swaps this first owns the result: the grace waiter (reply
        // `Running`, the event is injected later) or the task (reply `Done`,
        // the event is `inline` and not injected).
        let claimable = Arc::new(AtomicBool::new(true));
        let started = Instant::now();
        let task = tokio::spawn(run(Run {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            command: m.command.clone(),
            call_id: m.call_id,
            shell: m.shell,
            put_back: m.put_back,
            params,
            out_r,
            err_r,
            log_path: log_path.clone(),
            log: log.clone(),
            pgids: pgids.clone(),
            cancel: cancel.clone(),
            claimable: claimable.clone(),
            timeout: self.timeout,
            started,
            done: done_tx,
        }))
        .abort_handle();
        sj.push(Job {
            id: id.clone(),
            command: m.command,
            started,
            exit: None,
            secs: 0,
            log_path: log_path.clone(),
            log: log.clone(),
            pgids,
            cancel,
            task,
            name: None,
            pty: None,
        });
        // The grace wait leaves the mailbox so `ListJobs` / `CancelJob` stay live.
        let grace = self.grace;
        tokio::spawn(async move {
            // `Some`: the task finished (or died) within grace; `None`: grace elapsed.
            let within = if grace.is_zero() {
                None
            } else {
                tokio::time::timeout(grace, &mut done_rx).await.ok()
            };
            let done = match within {
                Some(r) => r.ok(),
                None if claimable.swap(false, SeqCst) => None,
                // The task claimed inline just as grace ran out: its result is ours.
                None => done_rx.await.ok(),
            };
            tx.send(match done {
                Some(out) => StartReply::Done(out),
                None => StartReply::Running {
                    id,
                    log: log_path,
                    tail: lock(&log).tail_text(STUB_TAIL),
                },
            });
        });
        reply
    }
}

impl Message<ListJobs> for Jobs {
    type Reply = Vec<JobView>;

    async fn handle(&mut self, m: ListJobs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.sessions
            .get(&m.0)
            .map(|s| s.jobs.iter().map(Job::view).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

impl Message<CancelJob> for Jobs {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CancelJob, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(job) = self.job_mut(m.session, &m.id) else {
            return Err("no such job".into());
        };
        if job.exit.is_some() {
            return Err(format!("{} already finished", m.id));
        }
        // The task kills the groups and finishes as `Killed`; killing here
        // would race its normal-exit arm into `Exited(137)`.
        job.cancel.notify_one();
        Ok(())
    }
}

impl Message<Finished> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: Finished, _: &mut Context<Self, Self::Reply>) {
        if let Some(job) = self.job_mut(m.session, &m.id) {
            job.exit = Some(m.exit);
            job.secs = m.secs;
        }
    }
}

impl Message<KillAll> for Jobs {
    type Reply = ();

    async fn handle(&mut self, _: KillAll, _: &mut Context<Self, Self::Reply>) {
        for job in self.sessions.values().flat_map(|s| &s.jobs) {
            if job.exit.is_none() {
                kill(&job.pgids);
                job.task.abort();
            }
        }
    }
}

/// SIGKILL every recorded group once and forget them.
pub(crate) fn kill(pgids: &Mutex<Vec<i32>>) {
    let mut pgids = std::mem::take(&mut *lock(pgids));
    pgids.sort_unstable();
    pgids.dedup(); // pipeline members share the first member's group
    for pgid in pgids {
        let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Log file, exec params and the read ends of the output pipes for one job.
fn setup(
    dir: &std::path::Path,
    log_path: &std::path::Path,
    shell: &Shell,
    pgids: &Arc<Mutex<Vec<i32>>>,
) -> anyhow::Result<(
    std::fs::File,
    ExecutionParameters,
    std::io::PipeReader,
    std::io::PipeReader,
)> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::File::create(log_path)?;
    let (out_r, out_w) = std::io::pipe()?;
    let (err_r, err_w) = std::io::pipe()?;
    let mut params = shell.default_exec_params();
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
                lock(&pgids).push(pgid);
            }
        }
    }));
    Ok((file, params, out_r, err_r))
}

/// Everything one job task owns.
struct Run {
    rt: Runtime,
    me: ActorRef<Jobs>,
    session: SessionId,
    id: JobId,
    command: String,
    call_id: String,
    shell: Shell,
    put_back: PutBack,
    params: ExecutionParameters,
    out_r: std::io::PipeReader,
    err_r: std::io::PipeReader,
    log_path: PathBuf,
    log: Arc<Mutex<Log>>,
    pgids: Arc<Mutex<Vec<i32>>>,
    cancel: Arc<Notify>,
    claimable: Arc<AtomicBool>,
    timeout: Duration,
    started: Instant,
    done: oneshot::Sender<ToolOutput>,
}

/// Publish the bytes since the last `Output`, if any.
async fn flush(rt: &Runtime, session: SessionId, id: &JobId, log: &Mutex<Log>) {
    let pending: Vec<u8> = std::mem::take(&mut lock(log).pending).into();
    if !pending.is_empty() {
        rt.publish(JobEvent::Output {
            session,
            id: id.clone(),
            chunk: String::from_utf8_lossy(&pending).into_owned(),
        })
        .await;
    }
}

/// Pipe → log as bytes arrive; returns the first ≤ 64 KiB (+1 so `truncate`
/// still marks the cut) for the finished output.
fn drain(mut r: std::io::PipeReader, log: Arc<Mutex<Log>>) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    lock(&log).push(&buf[..n]);
                    if all.len() <= MAX_OUTPUT {
                        all.extend_from_slice(&buf[..n]);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        all
    })
}

async fn run(mut j: Run) {
    j.rt.publish(JobEvent::Started {
        session: j.session,
        id: j.id.clone(),
        command: j.command.clone(),
        call_id: j.call_id,
        log: j.log_path,
    })
    .await;

    let out_task = drain(j.out_r, j.log.clone());
    let err_task = drain(j.err_r, j.log.clone());
    let source = SourceInfo::from("shell");
    let (exit, shell_err) = {
        let mut run = pin!(j.shell.run_string(j.command.as_str(), &source, &j.params));
        let mut deadline = pin!(tokio::time::sleep(j.timeout));
        let mut cancelled = pin!(j.cancel.notified());
        let mut tick = tokio::time::interval(OUTPUT_EVERY);
        loop {
            tokio::select! {
                biased;
                () = &mut cancelled => { kill(&j.pgids); break (JobExit::Killed, None) }
                () = &mut deadline => { kill(&j.pgids); break (JobExit::Timeout, None) }
                r = &mut run => break match r {
                    Ok(r) => (JobExit::Exited(u8::from(r.exit_code)), None),
                    Err(e) => (JobExit::Exited(1), Some(e.to_string())),
                },
                _ = tick.tick() => flush(&j.rt, j.session, &j.id, &j.log).await,
            }
        }
    };
    drop(j.params); // closes our pipe writers so the drains can finish

    // ponytail: a child that survived the kill (setsid'd itself) keeps the pipe
    // open; give up on its output after a second rather than hang the job.
    let (stdout, stderr) = match tokio::time::timeout(
        Duration::from_secs(1),
        futures::future::join(out_task, err_task),
    )
    .await
    {
        Ok((o, e)) => (o.unwrap_or_default(), e.unwrap_or_default()),
        Err(_) => (Vec::new(), Vec::new()),
    };

    let secs = j.started.elapsed().as_secs();
    let body = render(&stdout, &stderr);
    let (output, out) = match (&exit, shell_err) {
        (_, Some(e)) => {
            let body = truncate(format!("{body}\nshell error: {e}"));
            (body.clone(), ToolOutput::err(body))
        }
        (JobExit::Exited(code), None) => {
            // The runtime hands Python the output alone: a non-zero exit
            // raises, so the trailer only ever says `0` there.
            let body = truncate(body);
            let out = ToolOutput {
                content: format!("{body}{}exit code: {code}", sep(&body)),
                is_error: *code != 0,
                structured: Some(serde_json::Value::String(body.clone())),
                files: Vec::new(),
            };
            (body, out)
        }
        (JobExit::Timeout, None) => {
            let body = truncate(body);
            let out = ToolOutput::err(format!(
                "{body}{}timed out after {}s; process group killed",
                sep(&body),
                j.timeout.as_secs()
            ));
            (body, out)
        }
        (JobExit::Killed, None) => {
            let body = truncate(body);
            let out = ToolOutput::err(format!("{body}{}killed", sep(&body)));
            (body, out)
        }
    };

    flush(&j.rt, j.session, &j.id, &j.log).await;
    // A killed shell was dropped mid-command; its state is not worth keeping.
    if !matches!(exit, JobExit::Timeout | JobExit::Killed) {
        (j.put_back)(j.shell);
    }
    // Claim the result (see `Start`); then the row update is queued before
    // anyone can react to the reply or the event.
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
        output,
        inline,
        pty: false,
    })
    .await;
    let _ = j.done.send(out);
}

pub(crate) fn sep(body: &str) -> &'static str {
    if body.is_empty() || body.ends_with('\n') {
        ""
    } else {
        "\n"
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

/// The `/jobs` listing shared by the `jobs` tool, the TUI and Signal: one
/// line per job — `id  state  secs  command (≤60 chars)  | last output line`
/// — or `no jobs`. A PTY job's command is prefixed `pty ` (and `name: ` when
/// named) and its last line is the last non-blank screen row (22 §2.3).
pub fn table(jobs: &[JobView]) -> String {
    if jobs.is_empty() {
        return "no jobs".into();
    }
    let rows: Vec<String> = jobs
        .iter()
        .map(|j| {
            let last = j
                .tail
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            let mut command = j.command.chars().take(60).collect::<String>();
            if j.command.chars().count() > 60 {
                command.push('…');
            }
            if j.pty {
                let name = j
                    .name
                    .as_deref()
                    .map_or(String::new(), |n| format!("{n}: "));
                command = format!("pty {name}{command}");
            }
            format!(
                "{}  {:<9} {:>5}s  {command}  | {last}",
                j.id, j.state, j.secs
            )
        })
        .collect();
    rows.join("\n")
}

/// Arguments for [`JobsTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct JobsArgs {}

/// `jobs`: this session's jobs as a text table.
pub struct JobsTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for JobsTool {
    const NAME: &'static str = "jobs";
    const RISK: Risk = Risk::ReadOnly;
    type Args = JobsArgs;

    fn description(&self) -> String {
        "List this session's background jobs: id, state, seconds, command, last output line.".into()
    }

    async fn call(&self, _: JobsArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let jobs = self
            .0
            .ask(ListJobs(cx.session))
            .await
            .map_err(|e| anyhow::anyhow!("jobs actor: {e}"))?;
        Ok(ToolOutput::ok(table(&jobs)))
    }
}

/// Arguments for [`CancelJobTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CancelJobArgs {
    /// Job id from the `shell` stub or `jobs`, e.g. `j3`.
    pub id: String,
}

/// `cancel_job`: SIGKILL a running job.
pub struct CancelJobTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for CancelJobTool {
    const NAME: &'static str = "cancel_job";
    const RISK: Risk = Risk::Mutating;
    type Args = CancelJobArgs;

    fn description(&self) -> String {
        "Kill a running background job by id.".into()
    }

    async fn call(&self, args: CancelJobArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let id = JobId(args.id);
        // kameo lifts a `Result` reply: the handler's `Err(String)` arrives as
        // `SendError::HandlerError`.
        Ok(
            match self
                .0
                .ask(CancelJob {
                    session: cx.session,
                    id: id.clone(),
                })
                .await
            {
                Ok(()) => ToolOutput::ok(format!("killed {id}")),
                Err(kameo::error::SendError::HandlerError(e)) => ToolOutput::err(e),
                Err(e) => anyhow::bail!("jobs actor: {e}"),
            },
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::shell::ShellTool;

    pub(crate) struct Fx {
        pub(crate) jobs: ActorRef<Jobs>,
        pub(crate) session: SessionId,
        pub(crate) dir: PathBuf,
        /// Every [`JobEvent`] published on this fixture's bus.
        pub(crate) events: Arc<Mutex<Vec<JobEvent>>>,
    }

    #[derive(kameo::Actor)]
    struct Recorder(Arc<Mutex<Vec<JobEvent>>>);

    impl Message<JobEvent> for Recorder {
        type Reply = ();
        async fn handle(&mut self, e: JobEvent, _: &mut Context<Self, Self::Reply>) {
            lock(&self.0).push(e);
        }
    }

    pub(crate) async fn fixture(grace: Duration, timeout: Duration) -> Fx {
        let dir = std::env::temp_dir().join(format!("wth-jobs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let events = Arc::new(Mutex::new(Vec::new()));
        let rec = Recorder::spawn_with_mailbox(
            Recorder(Arc::clone(&events)),
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Recorder, JobEvent>(&rec).await;
        let jobs =
            Jobs::spawn_with_mailbox(Jobs::new(rt, grace, timeout), runtime::subscriber_mailbox());
        Fx {
            jobs,
            session: SessionId::new(),
            dir,
            events,
        }
    }

    /// `(exit, output, inline)` of the first `Finished` event, once published.
    pub(crate) async fn finished(fx: &Fx) -> (JobExit, String, bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let hit = lock(&fx.events).iter().find_map(|e| match e {
                JobEvent::Finished {
                    exit,
                    output,
                    inline,
                    ..
                } => Some((exit.clone(), output.clone(), *inline)),
                _ => None,
            });
            if let Some(hit) = hit {
                return hit;
            }
            assert!(Instant::now() < deadline, "no Finished event");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn spawn_job(fx: &Fx, command: &str) -> StartReply {
        fx.jobs
            .ask(Start {
                session: fx.session,
                command: command.into(),
                call_id: String::new(),
                shell: ShellTool::build_shell().await.unwrap(),
                put_back: Arc::new(|_| {}),
            })
            .await
            .unwrap()
    }

    pub(crate) async fn list(fx: &Fx) -> Vec<JobView> {
        fx.jobs.ask(ListJobs(fx.session)).await.unwrap()
    }

    /// Polls until the only job's state leaves `running`.
    pub(crate) async fn settled(fx: &Fx, within: Duration) -> JobView {
        let deadline = Instant::now() + within;
        loop {
            let v = list(fx).await.pop().unwrap();
            if v.state != "running" {
                return v;
            }
            assert!(Instant::now() < deadline, "still running: {v:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn inline_when_fast() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        match spawn_job(&fx, "echo hi").await {
            StartReply::Done(out) => {
                assert_eq!(out.content, "hi\nexit code: 0");
                assert!(!out.is_error);
            }
            r => panic!("expected Done, got {r:?}"),
        }
        let rows = list(&fx).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, JobId("j1".into()));
        assert_eq!(rows[0].state, "exited 0");
        assert_eq!(rows[0].tail, "hi\n");
        assert_eq!(
            finished(&fx).await,
            (JobExit::Exited(0), "hi\n".to_owned(), true)
        );
    }

    #[tokio::test]
    async fn stub_then_finished_event() {
        let fx = fixture(Duration::from_millis(200), Duration::from_secs(60)).await;
        let StartReply::Running { id, log, .. } = spawn_job(&fx, "sleep 2; echo late").await else {
            panic!("expected Running");
        };
        assert_eq!(id, JobId("j1".into()));
        assert!(log.starts_with(fx.dir.join("jobs")));
        assert_eq!(list(&fx).await[0].state, "running");

        let v = settled(&fx, Duration::from_secs(5)).await;
        assert_eq!(v.state, "exited 0");
        assert!(std::fs::read_to_string(&log).unwrap().contains("late"));

        assert!(matches!(
            lock(&fx.events).first(),
            Some(JobEvent::Started { .. })
        ));
        assert_eq!(
            finished(&fx).await,
            (JobExit::Exited(0), "late\n".to_owned(), false)
        );
    }

    #[tokio::test]
    async fn cancel_kills_group() {
        let fx = fixture(Duration::from_millis(200), Duration::from_secs(60)).await;
        let StartReply::Running { id, .. } = spawn_job(&fx, "sh -c 'echo $$; exec sleep 30'").await
        else {
            panic!("expected Running");
        };
        let pgid: i32 = list(&fx).await[0].tail.trim().parse().unwrap();
        assert!(nix::sys::signal::kill(Pid::from_raw(pgid), None).is_ok());

        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(settled(&fx, Duration::from_secs(1)).await.state, "killed");
        // Reaped by tokio's orphan queue on the next SIGCHLD; allow it a moment.
        let gone = Instant::now() + Duration::from_secs(1);
        while nix::sys::signal::kill(Pid::from_raw(pgid), None).is_ok() {
            assert!(Instant::now() < gone, "pgid {pgid} still alive");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let refused = |r: Result<(), kameo::error::SendError<CancelJob, String>>| match r {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            other => panic!("expected a handler error, got {other:?}"),
        };
        let again = fx
            .jobs
            .ask(CancelJob {
                session: fx.session,
                id,
            })
            .await;
        assert_eq!(refused(again), "j1 already finished");
        let missing = fx
            .jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId("j9".into()),
            })
            .await;
        assert_eq!(refused(missing), "no such job");
    }

    #[tokio::test]
    async fn deadline() {
        let fx = fixture(Duration::from_millis(100), Duration::from_millis(500)).await;
        assert!(matches!(
            spawn_job(&fx, "sleep 5").await,
            StartReply::Running { .. }
        ));
        assert_eq!(settled(&fx, Duration::from_secs(3)).await.state, "timeout");
    }

    #[tokio::test]
    async fn state_last_finished_wins() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let tool = ShellTool::new(fx.jobs.clone());
        let cx = ToolCx {
            session: fx.session,
            call_id: String::new(),
        };
        let cd = Tool::call(
            &tool,
            crate::shell::ShellArgs {
                command: "cd /tmp".into(),
            },
            cx.clone(),
        )
        .await
        .unwrap();
        assert_eq!(cd.content, "exit code: 0");
        let pwd = Tool::call(
            &tool,
            crate::shell::ShellArgs {
                command: "pwd".into(),
            },
            cx,
        )
        .await
        .unwrap();
        assert_eq!(pwd.content, "/tmp\nexit code: 0");
    }
}
