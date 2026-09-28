//! `Jobs` actor (14 §2): every process the model starts is a background job.
//! Owns each job's process groups, streams output to
//! `<resources.root>/jobs/<session>/<id>.log`, publishes [`JobEvent`]s on the bus,
//! and kills everything still running on `rt.shutdown`. Registered as
//! [`JOBS_ACTOR`]; `shell` starts jobs with [`Start`], `jobs` / `cancel_job`
//! (also here) ask [`ListJobs`] / [`CancelJob`].
//!
//! Activity (47 §4.2 rule 3): each job is an admitted operation
//! (`Kind::Job` / `Kind::Pty`) from before its process spawns until the
//! producer settles it; a result not returned by the tool call travels with
//! that settlement and is the model's only `<job_result>` path. The
//! supervisor's sink arrives through [`InstallActivity`].

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
use platform::ProcessTree;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::activity::{self, Activity, ActivitySink, Kind, OperationId, Rejected};
use crate::resources::CloseAck;
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
/// How long a job's result waits for what the command left behind (open
/// pipes, `&` jobs, killed groups) before that becomes its own operation.
pub(crate) const SETTLE: Duration = Duration::from_secs(1);
/// After SIGKILL, a process group still answering signal 0 past this is
/// reported unknown rather than waited for.
pub(crate) const KILL_WAIT: Duration = Duration::from_secs(2);
/// Spacing of the signal-0 liveness poll on an owned process group.
pub(crate) const GROUP_POLL: Duration = Duration::from_millis(50);
/// Retained-terminal budget across every session (51 §7.4); each PTY is
/// charged [`crate::pty::RESERVE`] at `start` until its handle is evicted.
pub(crate) const PTY_BUDGET: usize = 64 << 20;

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
    /// The job's deadline (`tools.job_timeout_secs`, or the call's shorter
    /// `timeout_secs`) elapsed; the process group was killed.
    Timeout,
    /// [`CancelJob`] killed the process group; the [`CancelOrigin`] decides
    /// what the model hears about it.
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

/// Who asked [`CancelJob`] for the kill (14 §1.2 as-built). Decides the
/// `Finished` event: the model that called `cancel_job` already read the
/// screen it killed (61), so it gets no `<job_result>`; a human or a closing
/// session gets a one-line notice instead of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOrigin {
    /// The owning session's model called `cancel_job`.
    Tool,
    /// A human pressed stop in a UI (`/jobs kill`, the desktop job row).
    Ui,
    /// The session is closing ([`CloseSessions`]).
    Shutdown,
}

impl CancelOrigin {
    /// The one-line `Finished.output` for a job this origin killed.
    pub fn notice(self) -> &'static str {
        match self {
            CancelOrigin::Tool => "killed by cancel_job",
            CancelOrigin::Ui => "killed by the user",
            CancelOrigin::Shutdown => "killed at shutdown",
        }
    }
}

/// A job task's cancel signal, with who fired it: [`Cancel::fire`] records
/// the origin before it notifies, so the task's finish path can read it.
#[derive(Debug, Default)]
pub(crate) struct Cancel {
    origin: std::sync::OnceLock<CancelOrigin>,
    notify: Notify,
}

impl Cancel {
    /// The first origin sticks; the permit is stored when nobody waits yet.
    pub(crate) fn fire(&self, origin: CancelOrigin) {
        let _ = self.origin.set(origin);
        self.notify.notify_one();
    }

    pub(crate) async fn notified(&self) -> CancelOrigin {
        self.notify.notified().await;
        *self
            .origin
            .get()
            .expect("fire records the origin before it notifies")
    }
}

/// Bus events. `Output` is coalesced to one per [`OUTPUT_EVERY`] per job and
/// lossy under a flood; the log file is the truth. PTY jobs (22 §4.8) publish
/// `Screen` (coalesced the same way) instead of `Output`, and `Idle` when the
/// screen has stopped changing (61). Wire tags are `JobStarted` | `JobOutput`
/// | `JobScreen` | `JobIdle` | `JobFinished` so they read unambiguously on
/// the shared RPC event stream.
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
        /// `start`'s label (22 §2.1), PTY jobs only.
        #[serde(default)]
        name: Option<String>,
        /// Runs on a pseudo-terminal (22).
        #[serde(default)]
        pty: bool,
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
    /// A running PTY job's screen has not changed for `quiet_secs` (61 §1).
    /// Observation only on the bus: the model is woken through the activity
    /// guard (`Notice::JobIdle`), never from here.
    #[serde(rename = "JobIdle")]
    Idle {
        /// Owning session.
        session: SessionId,
        /// Job.
        id: JobId,
        /// Command line.
        command: String,
        /// `start`'s label.
        name: Option<String>,
        /// Seconds since the screen last changed.
        quiet_secs: u64,
        /// Job age in seconds.
        secs: u64,
        /// 1 for the first nudge of a backoff run, then 2, 3… (61 §1.4).
        nudge: u32,
        /// Seconds since a human typed into the job, within the 55 §2.3 window.
        user_typed_secs: Option<u64>,
        /// The screen text with its `cursor:` line: the `<job_idle>` body.
        output: String,
    },
    /// The job ended; the agent injects this as `<job_result>` unless `inline`.
    /// A [`CancelJob`] kill shapes it by [`CancelOrigin`] (14 §1.2 as-built):
    /// `Tool` → `inline: true` (the `cancel_job` reply was the acknowledgement,
    /// the model already read the screen it killed — 61); `Ui` / `Shutdown` →
    /// injected, `output` a one-line notice. Never the screen or the log.
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
        /// a PTY job the final screen text (22 §2.2). For a `Killed` job the
        /// origin's [`CancelOrigin::notice`].
        output: String,
        /// The tool call already returned this result (exited within the
        /// grace window), or the model killed it; UIs still count it, the
        /// agent must not inject it.
        inline: bool,
        /// Ran on a pseudo-terminal (22): `output` is a screen, not a log.
        pty: bool,
    },
}

/// One row of [`ListJobs`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobView {
    /// Owning session.
    pub session: SessionId,
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
    /// 37 §3.2: the caller consumes the output itself. No `JobEvent`, no log
    /// file, no `jobs` row, and the reply is always `Done` — the actor waits
    /// for the command (bounded by the job timeout) instead of returning a
    /// `Running` stub whose result would later be injected as `<job_result>`.
    pub quiet: bool,
    /// A clone of the session shell; the job owns it and hands it to
    /// `put_back` when it exits normally (not when killed or timed out).
    pub shell: Shell,
    /// Where the finished shell goes.
    pub put_back: PutBack,
    /// The session's [`crate::SessionResources::root`] (51 §4.5): the log
    /// is `<root>/jobs/<session>/<id>.log`, the data dir for a durable tree.
    pub root: PathBuf,
    /// [`activity::current`] at the caller. The job's own lifetime is admitted
    /// as a child of a *tracked* invocation (47 §4.2 rule 3); an untracked or
    /// absent parent (pre-registration invocation, RPC, tests) admits under
    /// the supervisor installed on the actor instead, so no job spawns
    /// untracked once one exists.
    pub activity: Option<Activity>,
    /// Per-call kill deadline (`shell.timeout_secs`); `None` = the actor's
    /// configured job timeout, which also caps any value given here. Values
    /// under one second become one second.
    pub timeout: Option<Duration>,
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

/// `ask` → `HashMap<SessionId, (u16, u16)>`: running `(pipe jobs, PTY jobs)`
/// per requested session (62 §0.5); a session with none may be absent.
#[derive(Clone, Debug)]
pub struct JobCounts(pub Vec<SessionId>);

/// `ask` → `Result<(), String>`: SIGKILL a running job's process groups.
/// Err `no such job` | `j3 already finished`.
#[derive(Clone, Debug)]
pub struct CancelJob {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub id: JobId,
    /// Who asked; shapes the `Finished` event.
    pub origin: CancelOrigin,
}

/// `ask` → [`CloseAck`] (owner `"jobs"`): the [`crate::CloseTree`] barrier's Jobs
/// half (51 §4.4). Every row of `sessions` goes at once (`ListJobs` is empty
/// on reply); running jobs are cancelled and their process groups killed;
/// the ack waits for the tasks to settle (bounded by `cancel`) and then
/// removes each session's `<root>/jobs/<session>` log directory. `ok` is
/// false when a task was still settling at `cancel` or a directory could
/// not be removed.
#[derive(Clone, Debug)]
pub struct CloseSessions {
    /// The closing subtree.
    pub sessions: Vec<SessionId>,
    /// The closer stopped waiting: reply with what is still unsettled.
    pub cancel: CancellationToken,
}

/// `tell` from a job task: the actual work settled.
pub(crate) struct Finished {
    /// The job's operation: its guard leaves [`Jobs::live`] here.
    pub(crate) op: OperationId,
    /// The row to close — `None` for a quiet job, which has none (and whose
    /// reused id may name someone else's row).
    pub(crate) row: Option<(SessionId, JobId, JobExit, u64)>,
    /// The `JobEvent::Finished` the model is still owed: neither returned
    /// inline by the tool call nor consumed by a quiet caller. Travels with
    /// the guard as [`Settle::FinishedWithResult`](crate::Settle) (47 §4.1).
    pub(crate) result: Option<JobEvent>,
}

/// `tell` on `rt.shutdown`: kill every running job's process groups.
struct KillAll;

/// `ask` → `()`: the activity supervisor (47 §4.2 rule 7), forwarded by the
/// registry's `RegisterActivity` before that is acknowledged. Every job still
/// running untracked is adopted (reported unknown until its own settlement)
/// synchronously, so the ack means the ledger holds every live job; from
/// then on nothing spawns untracked.
pub struct InstallActivity(pub ActivitySink);

/// `ask` from the admission task → `Result<R, String>`: spawn an admitted
/// job under the actor's state. Admission itself awaits the supervisor's
/// acknowledgement, so it happens off the mailbox (47 §4.1); this second hop
/// is what keeps "admitted" strictly before "spawned".
pub(crate) struct Launch<M> {
    pub(crate) m: M,
    pub(crate) id: JobId,
    pub(crate) activity: Activity,
}

/// What the pipe-job asker waits on after [`Launch`].
pub(crate) struct Running {
    pub(crate) done: oneshot::Receiver<ToolOutput>,
    pub(crate) claimable: Arc<AtomicBool>,
    pub(crate) log: Arc<Mutex<Log>>,
    pub(crate) log_path: PathBuf,
}

/// Admit a job's independent lifetime before anything spawns (47 §4.2
/// rule 3): as a child of a tracked parent invocation, else under the
/// installed supervisor (`None` before any exists: untracked, nothing
/// certified — those guards are adopted by [`InstallActivity`]).
pub(crate) async fn admit_job(
    parent: Option<Activity>,
    sink: Option<&ActivitySink>,
    owner: SessionId,
    kind: Kind,
) -> Result<Activity, Rejected> {
    match parent {
        Some(p) if p.is_tracked() => p.admit(kind).await,
        _ => activity::admit(sink, owner, kind).await,
    }
}

/// Output as it arrives: appended to the log file (none for a quiet job), the
/// last [`TAIL`] bytes kept, and the bytes since the last [`JobEvent::Output`]
/// (capped the same way).
pub(crate) struct Log {
    file: Option<std::fs::File>,
    tail: VecDeque<u8>,
    pending: VecDeque<u8>,
}

impl Log {
    pub(crate) fn new(file: Option<std::fs::File>) -> Self {
        Self {
            file,
            tail: VecDeque::with_capacity(TAIL),
            pending: VecDeque::new(),
        }
    }

    /// Every chunk from a pipe or a pty passes here before the log file, the
    /// tail and `JobEvent::Output`. Known secret values are scrubbed first —
    /// best effort, a value split across two chunks escapes; `run`'s pass
    /// over the whole render is the exact one (37 §3.4).
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        let text = String::from_utf8_lossy(chunk);
        let clean = crate::secrets::redact(&text);
        let chunk = match &clean {
            std::borrow::Cow::Owned(s) => s.as_bytes(),
            std::borrow::Cow::Borrowed(_) => chunk,
        };
        if let Some(file) = &mut self.file {
            let _ = file.write_all(chunk);
        }
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
    /// The job's admitted operation, keying [`Jobs::live`].
    pub(crate) op: OperationId,
    pub(crate) command: String,
    pub(crate) started: Instant,
    /// `None` while running.
    pub(crate) exit: Option<JobExit>,
    /// Total seconds once finished.
    pub(crate) secs: u64,
    pub(crate) log_path: PathBuf,
    pub(crate) log: Arc<Mutex<Log>>,
    pub(crate) pgids: Arc<Mutex<Vec<ProcessTree>>>,
    pub(crate) cancel: Arc<Cancel>,
    pub(crate) task: AbortHandle,
    /// `start`'s label (22 §2.1); PTY jobs only.
    pub(crate) name: Option<String>,
    /// Ran on a pseudo-terminal (22); stays true once `pty` is evicted.
    pub(crate) on_pty: bool,
    /// The pseudo-terminal, for PTY jobs (22 §4.8); `None` for a pipe job or
    /// once evicted under the budget (51 §7.4).
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

    /// `running` | `exited 0` | `timeout` | `killed`.
    pub(crate) fn state(&self) -> String {
        self.exit
            .as_ref()
            .map_or_else(|| "running".to_owned(), ToString::to_string)
    }

    fn view(&self, session: SessionId) -> JobView {
        JobView {
            session,
            id: self.id.clone(),
            command: self.command.clone(),
            state: self.state(),
            secs: self.secs(),
            log: self.log_path.clone(),
            tail: match &self.pty {
                Some(pty) => pty.screen().0.join("\n"),
                None => lock(&self.log).tail_text(TAIL),
            },
            name: self.name.clone(),
            pty: self.on_pty,
        }
    }
}

#[derive(Default)]
pub(crate) struct SessionJobs {
    pub(crate) next: u32,
    pub(crate) jobs: Vec<Job>,
}

impl SessionJobs {
    /// Append a row, dropping the oldest finished ones past [`MAX_ROWS`];
    /// returns how many retained terminals went with them.
    pub(crate) fn push(&mut self, job: Job) -> usize {
        self.jobs.push(job);
        let mut evicted = 0;
        while self.jobs.len() > MAX_ROWS {
            match self.jobs.iter().position(|j| j.exit.is_some()) {
                Some(i) => evicted += usize::from(self.jobs.remove(i).pty.is_some()),
                None => break,
            }
        }
        evicted
    }
}

/// The jobs actor. Look up with `ActorRef::<Jobs>::lookup(JOBS_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Jobs {
    pub(crate) rt: Runtime,
    pub(crate) grace: Duration,
    timeout: Duration,
    pub(crate) sessions: HashMap<SessionId, SessionJobs>,
    /// [`InstallActivity`]; `None` until the supervisor registers.
    pub(crate) activity: Option<ActivitySink>,
    /// Every unsettled job's guard (rows and quiet jobs alike), by operation:
    /// the actor's clone, dropped — carrying the owed result — on [`Finished`].
    pub(crate) live: HashMap<OperationId, Activity>,
    /// Bytes charged against [`PTY_BUDGET`]: one [`crate::pty::RESERVE`]
    /// per job whose `pty` is `Some`.
    pub(crate) pty_bytes: usize,
    /// Snapshot renders admitted at once (51 §7.4).
    pub(crate) snapshots: Arc<tokio::sync::Semaphore>,
    /// Pending reminder timers per session (`reminder.rs`); their guards
    /// live in `live` like any job's.
    pub(crate) reminders: HashMap<SessionId, crate::reminder::SessionReminders>,
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
            activity: None,
            live: HashMap::new(),
            pty_bytes: 0,
            snapshots: Arc::new(tokio::sync::Semaphore::new(crate::pty::SNAPSHOTS)),
            reminders: HashMap::new(),
        }
    }

    fn job_mut(&mut self, session: SessionId, id: &JobId) -> Option<&mut Job> {
        self.sessions
            .get_mut(&session)?
            .jobs
            .iter_mut()
            .find(|j| j.id == *id)
    }

    /// Record an admitted job's guard as it spawns. A supervisor installed
    /// between an untracked admission and this point adopts the guard here,
    /// in the same serialized step that spawns it: nothing runs untracked
    /// once one exists.
    pub(crate) fn track(&mut self, activity: &Activity) {
        if let Some(sink) = &self.activity {
            activity.adopt(sink);
        }
        self.live.insert(activity.id(), activity.clone());
    }

    /// Add a row, releasing the terminal charge of whatever it evicts.
    pub(crate) fn push(&mut self, session: SessionId, job: Job) {
        let evicted = self.sessions.entry(session).or_default().push(job);
        self.pty_bytes -= evicted * crate::pty::RESERVE;
    }

    /// Drop exited jobs' terminals, oldest first across every session, until
    /// `need` more bytes fit under [`PTY_BUDGET`] or none are left (51 §7.4).
    /// Live scrollback is never trimmed: vt100 exposes no way to.
    pub(crate) fn make_room(&mut self, need: usize) {
        while self.pty_bytes + need > PTY_BUDGET {
            let oldest = self
                .sessions
                .values_mut()
                .flat_map(|s| s.jobs.iter_mut())
                .filter(|j| j.exit.is_some() && j.pty.is_some())
                .min_by_key(|j| j.started);
            match oldest {
                Some(j) => {
                    j.pty = None;
                    self.pty_bytes -= crate::pty::RESERVE;
                }
                None => break,
            }
        }
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

    async fn handle(&mut self, mut m: Start, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let quiet = m.quiet;
        let sj = self.sessions.entry(m.session).or_default();
        // A quiet job takes no row and no id: it reuses the last number, which
        // nothing publishes or lists.
        if !quiet {
            sj.next += 1;
        }
        let id = JobId(format!("j{}", sj.next));
        let sink = self.activity.clone();
        let me = ctx.actor_ref().clone();
        let grace = self.grace;
        // Admission, then the grace wait, leave the mailbox so `ListJobs` /
        // `CancelJob` stay live. The caller's invocation stays admitted until
        // it gets this reply, so the job's admission opens no gap (47 §4.2 rule 3).
        tokio::spawn(async move {
            let fail =
                |why: String| StartReply::Done(ToolOutput::err(format!("cannot start job: {why}")));
            let activity = match admit_job(
                m.activity.take(),
                sink.as_ref(),
                m.session,
                Kind::Job(id.clone()),
            )
            .await
            {
                Ok(a) => a,
                Err(why) => return tx.send(fail(why.to_string())),
            };
            let Running {
                mut done,
                claimable,
                log,
                log_path,
            } = match me
                .ask(Launch {
                    m,
                    id: id.clone(),
                    activity,
                })
                .await
            {
                Ok(r) => r,
                Err(e) => return tx.send(fail(e.to_string())),
            };
            if quiet {
                // 37 §3.2: never a stub; the task's own `timeout` bounds the wait.
                let out = done
                    .await
                    .unwrap_or_else(|_| ToolOutput::err("job task died"));
                tx.send(StartReply::Done(out));
                return;
            }
            // `Some`: the task finished (or died) within grace; `None`: grace elapsed.
            let within = if grace.is_zero() {
                None
            } else {
                tokio::time::timeout(grace, &mut done).await.ok()
            };
            let done = match within {
                Some(r) => r.ok(),
                None if claimable.swap(false, SeqCst) => None,
                // The task claimed inline just as grace ran out: its result is ours.
                None => done.await.ok(),
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

impl Message<Launch<Start>> for Jobs {
    type Reply = Result<Running, String>;

    async fn handle(
        &mut self,
        l: Launch<Start>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Launch { m, id, activity } = l;
        let quiet = m.quiet;
        let dir = m.root.join("jobs").join(m.session.to_string());
        let log_path = dir.join(format!("{id}.log"));
        let pgids: Arc<Mutex<Vec<ProcessTree>>> = Arc::default();
        let log_file = (!quiet).then_some(log_path.as_path());
        // On `Err` the admitted guard drops here: settled `Finished`, no blocker.
        let (file, params, out_r, err_r) =
            setup(&dir, log_file, &m.shell, &pgids).map_err(|e| format!("{e:#}"))?;
        let log = Arc::new(Mutex::new(Log::new(file)));
        let cancel = Arc::new(Cancel::default());
        let (done_tx, done) = oneshot::channel();
        // Whoever swaps this first owns the result: the grace waiter (reply
        // `Running`, the result is owed to the model) or the task (reply
        // `Done`, the event is `inline` and the guard settles bare).
        let claimable = Arc::new(AtomicBool::new(true));
        // 37 §3.4: the executed string is real; everything published or stored
        // (`Started`/`Finished`, `JobView`, `<job_result command>`) is scrubbed.
        let shown = crate::secrets::redact(&m.command).into_owned();
        let started = Instant::now();
        self.track(&activity);
        let op = activity.id();
        let task = tokio::spawn(run(Run {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            command: m.command,
            shown: shown.clone(),
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
            quiet,
            timeout: m.timeout.map_or(self.timeout, |t| {
                t.clamp(Duration::from_secs(1), self.timeout)
            }),
            requested: m.timeout,
            started,
            done: done_tx,
            activity,
        }))
        .abort_handle();
        if !quiet {
            self.push(
                m.session,
                Job {
                    id,
                    op,
                    command: shown,
                    started,
                    exit: None,
                    secs: 0,
                    log_path: log_path.clone(),
                    log: log.clone(),
                    pgids,
                    cancel,
                    task,
                    name: None,
                    on_pty: false,
                    pty: None,
                },
            );
        }
        Ok(Running {
            done,
            claimable,
            log,
            log_path,
        })
    }
}

impl Message<ListJobs> for Jobs {
    type Reply = Vec<JobView>;

    async fn handle(&mut self, m: ListJobs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.sessions
            .get(&m.0)
            .map(|s| s.jobs.iter().map(|j| j.view(m.0)).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

impl Message<JobCounts> for Jobs {
    type Reply = HashMap<SessionId, (u16, u16)>;

    async fn handle(&mut self, m: JobCounts, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        m.0.into_iter()
            .filter_map(|id| {
                let s = self.sessions.get(&id)?;
                let (mut jobs, mut ptys) = (0u16, 0u16);
                for j in s.jobs.iter().filter(|j| j.exit.is_none()) {
                    if j.on_pty {
                        ptys = ptys.saturating_add(1);
                    } else {
                        jobs = jobs.saturating_add(1);
                    }
                }
                (jobs + ptys > 0).then_some((id, (jobs, ptys)))
            })
            .collect()
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
        job.cancel.fire(m.origin);
        Ok(())
    }
}

impl Message<CloseSessions> for Jobs {
    type Reply = DelegatedReply<CloseAck>;

    async fn handle(
        &mut self,
        m: CloseSessions,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let mut tasks = Vec::new();
        let mut dirs = Vec::new();
        for session in &m.sessions {
            let Some(sj) = self.sessions.remove(session) else {
                continue;
            };
            for job in sj.jobs {
                if job.exit.is_none() {
                    // As `CancelJob`: the task reports `Killed` and settles;
                    // the kill here only makes sure nothing survives a task
                    // that is never polled again.
                    job.cancel.fire(CancelOrigin::Shutdown);
                    kill(&job.pgids);
                    tasks.push((job.id, job.task));
                }
                if job.pty.is_some() {
                    self.pty_bytes -= crate::pty::RESERVE;
                }
                if let Some(dir) = job.log_path.parent()
                    && !dirs.iter().any(|d| d == dir)
                {
                    dirs.push(dir.to_path_buf());
                }
            }
        }
        for session in &m.sessions {
            self.close_reminders(*session).await;
        }
        let Some(tx) = tx else { return reply };
        tokio::spawn(async move {
            let mut doubts = Vec::new();
            let settled = async {
                while tasks.iter().any(|(_, t)| !t.is_finished()) {
                    tokio::time::sleep(GROUP_POLL).await;
                }
            };
            tokio::select! {
                biased;
                () = settled => {}
                () = m.cancel.cancelled() => {
                    for (id, t) in &tasks {
                        if !t.is_finished() {
                            doubts.push(format!("{id}: still settling at cancel"));
                        }
                    }
                }
            }
            for dir in dirs {
                match tokio::fs::remove_dir_all(&dir).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => doubts.push(format!("{}: {e}", dir.display())),
                }
            }
            tx.send(if doubts.is_empty() {
                CloseAck::ok("jobs")
            } else {
                CloseAck::failed("jobs", doubts.join("; "))
            });
        });
        reply
    }
}

impl Message<Finished> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: Finished, _: &mut Context<Self, Self::Reply>) {
        if let Some((session, id, exit, secs)) = m.row
            && let Some(job) = self.job_mut(session, &id)
        {
            job.exit = Some(exit);
            job.secs = secs;
        }
        // The actor's clone goes; the task's follows. Whichever is last
        // settles, with the result when one is owed (47 §4.1): the model's
        // only `<job_result>` path, the bus event stays observation.
        if let Some(activity) = self.live.remove(&m.op)
            && let Some(result) = m.result
        {
            activity.set_result(result);
        }
    }
}

impl Message<KillAll> for Jobs {
    type Reply = ();

    async fn handle(&mut self, _: KillAll, _: &mut Context<Self, Self::Reply>) {
        for job in self.sessions.values_mut().flat_map(|s| &mut s.jobs) {
            if job.exit.is_none() {
                kill(&job.pgids);
                // Aborting the task skips its settlement, and an escapee may
                // outlive the kill: say so before the guard can drop as `Finished`.
                if let Some(a) = self.live.get(&job.op) {
                    a.unknown(format!("{}: killed at shutdown before it settled", job.id));
                }
                job.task.abort();
            }
            // Nothing observes a terminal past shutdown: release every charge.
            job.pty = None;
        }
        self.pty_bytes = 0;
    }
}

impl Message<InstallActivity> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: InstallActivity, _: &mut Context<Self, Self::Reply>) {
        // Synchronous by design: the registry awaits this reply and the
        // supervisor may await the registry, so nothing here may ask either.
        for a in self.live.values() {
            a.adopt(&m.0);
        }
        self.activity = Some(m.0);
    }
}

/// Snapshot of the recorded trees, one per group (pipeline members share
/// the first member's; `setup`'s hook adds them to it).
fn groups(trees: &Mutex<Vec<ProcessTree>>) -> Vec<ProcessTree> {
    lock(trees).clone()
}

/// SIGKILL every recorded tree once and forget them; returns what was
/// signalled so the caller can verify it went.
pub(crate) fn kill(trees: &Mutex<Vec<ProcessTree>>) -> Vec<ProcessTree> {
    let killed = std::mem::take(&mut *lock(trees));
    for tree in &killed {
        tree.kill();
    }
    killed
}

/// Poll `tree` every [`GROUP_POLL`] until no member is left (an unreaped
/// zombie still counts, so this also waits out the reaper) or `deadline`;
/// `true` once the tree is gone. Actual process evidence, not output
/// idleness: only our own descendants are in a recorded tree.
pub(crate) async fn group_gone(tree: &ProcessTree, deadline: Option<tokio::time::Instant>) -> bool {
    loop {
        if !tree.alive() {
            return true;
        }
        if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
            return false;
        }
        tokio::time::sleep(GROUP_POLL).await;
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Log file (none for a quiet job: `log_path` absent), exec params and the
/// read ends of the output pipes for one job.
fn setup(
    dir: &std::path::Path,
    log_path: Option<&std::path::Path>,
    shell: &Shell,
    trees: &Arc<Mutex<Vec<ProcessTree>>>,
) -> anyhow::Result<(
    Option<std::fs::File>,
    ExecutionParameters,
    std::io::PipeReader,
    std::io::PipeReader,
)> {
    let file = match log_path {
        Some(p) => {
            std::fs::create_dir_all(dir)?;
            Some(std::fs::File::create(p)?)
        }
        None => None,
    };
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
    // brush reports `(pid, pgid)` per external process; the first of a
    // pipeline leads a new group, the rest join it (58 §0.4: a Job Object
    // on Windows, where the members are assigned as they appear).
    params.on_spawn = Some(Arc::new({
        let trees = trees.clone();
        move |pid, pgid| {
            let Some(pgid) = pgid else { return };
            let mut trees = lock(&trees);
            let r = match trees.iter().find(|t| t.id() == pgid as u32) {
                Some(t) => t.add(pid as u32),
                None => ProcessTree::adopt(pgid as u32).map(|t| trees.push(t)),
            };
            if let Err(e) = r {
                tracing::warn!(pid, pgid, "could not track the job's process: {e}");
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
    /// Executed verbatim.
    command: String,
    /// `command` with secrets scrubbed: the only copy that is published.
    shown: String,
    call_id: String,
    shell: Shell,
    put_back: PutBack,
    params: ExecutionParameters,
    out_r: std::io::PipeReader,
    err_r: std::io::PipeReader,
    log_path: PathBuf,
    log: Arc<Mutex<Log>>,
    pgids: Arc<Mutex<Vec<ProcessTree>>>,
    cancel: Arc<Cancel>,
    claimable: Arc<AtomicBool>,
    /// [`Start::quiet`]: publish nothing, take no row.
    quiet: bool,
    /// Deadline actually applied.
    timeout: Duration,
    /// [`Start::timeout`] as given, for the timeout message.
    requested: Option<Duration>,
    started: Instant,
    done: oneshot::Sender<ToolOutput>,
    /// The job's admitted lifetime (`Kind::Job`); the actor holds the other
    /// clone in [`Jobs::live`]. Dropped when this task returns.
    activity: Activity,
}

/// One output pipe being read: the first ≤ 64 KiB (+1 so `truncate` still
/// marks the cut) land in `head` as they arrive, so the finished output has
/// what was collected even when the reader outlives the command.
struct Drain {
    head: Arc<Mutex<Vec<u8>>>,
    task: JoinHandle<std::io::Result<()>>,
}

/// What the command left behind, awaited to actual settlement (47 §4.2
/// rule 4), never inferred from the command text:
/// - its `&` jobs: brush tracks each as a task that returns once its
///   processes are reaped, redirected output or not;
/// - every recorded process group, polled until no member is left — covers
///   a detached grandchild (`( sleep 60 & )`) whose spawn was recorded but
///   whose job table went with its subshell;
/// - the pipe ends still open: a holder outside every recorded group (a
///   `setsid` escapee) that inherited them, read to EOF.
///
/// Resolves to what could not be verified: a reader that died or failed
/// (output past that point is lost), a job that stopped rather than exited,
/// a group that survived SIGKILL past [`KILL_WAIT`]. A group alive after a
/// normal exit is legitimately ours and is waited for.
async fn settle(
    bg: Vec<brush_core::jobs::Job>,
    drains: Vec<JoinHandle<std::io::Result<()>>>,
    groups: Vec<ProcessTree>,
    killed: bool,
) -> Vec<String> {
    let mut why = Vec::new();
    for mut b in bg {
        match b.wait().await {
            Ok(_) if matches!(b.state, brush_core::jobs::JobState::Done) => {}
            Ok(_) => why.push("a background job stopped instead of exiting".to_owned()),
            Err(e) => why.push(format!("background job wait failed: {e}")),
        }
    }
    let bound = killed.then(|| tokio::time::Instant::now() + KILL_WAIT);
    for tree in groups {
        if !group_gone(&tree, bound).await {
            why.push(format!("process group {} survived SIGKILL", tree.id()));
        }
    }
    for r in futures::future::join_all(drains).await {
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => why.push(format!("output reader failed with its pipe open: {e}")),
            Err(e) => why.push(format!("output reader died with its pipe open: {e}")),
        }
    }
    why
}

/// [`settle`] did not finish within [`SETTLE`], or finished with doubts: the
/// rest is this session's work as a child operation of the job, settled by
/// the processes and readers themselves and marked unknown with each doubt.
/// The job's own result is never held up or overridden by it: a root marked
/// unknown would lose its `FinishedWithResult`, so uncertainty about the
/// tail lives on the child alone.
async fn linger(
    job: &Activity,
    id: &JobId,
    tail: impl Future<Output = Vec<String>> + Send + 'static,
) {
    match job.admit(Kind::Command("job background process")).await {
        Ok(a) => {
            let id = id.clone();
            let guard = a.clone();
            tokio::spawn(a.scope(async move {
                for why in tail.await {
                    guard.unknown(format!("{id}: {why}"));
                }
            }));
        }
        // The job itself completed and its result is owed regardless; the
        // supervisor that refused (or is gone) certifies nothing meanwhile.
        Err(e) => tracing::warn!("{id}: background work left untracked: {e}"),
    }
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

/// Pipe → log as bytes arrive, the head kept aside for the finished output.
/// Ends at EOF (`Ok`) or a read error (`Err`: the pipe may still be held,
/// what follows is lost) — never reported as one or the other by default.
fn drain(mut r: std::io::PipeReader, log: Arc<Mutex<Log>>) -> Drain {
    let head: Arc<Mutex<Vec<u8>>> = Arc::default();
    let task = tokio::task::spawn_blocking({
        let head = head.clone();
        move || {
            let mut buf = [0u8; 4096];
            loop {
                match r.read(&mut buf) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        lock(&log).push(&buf[..n]);
                        let mut head = lock(&head);
                        if head.len() <= MAX_OUTPUT {
                            head.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break Err(e),
                }
            }
        }
    });
    Drain { head, task }
}

async fn run(mut j: Run) {
    if !j.quiet {
        j.rt.publish(JobEvent::Started {
            session: j.session,
            id: j.id.clone(),
            command: j.shown.clone(),
            call_id: j.call_id,
            log: j.log_path,
            name: None,
            pty: false,
        })
        .await;
    }

    let out = drain(j.out_r, j.log.clone());
    let err = drain(j.err_r, j.log.clone());
    let source = SourceInfo::from("shell");
    let (exit, shell_err, killed, origin) = {
        let mut run = pin!(j.shell.run_string(j.command.as_str(), &source, &j.params));
        let mut deadline = pin!(tokio::time::sleep(j.timeout));
        let mut cancelled = pin!(j.cancel.notified());
        let mut tick = tokio::time::interval(OUTPUT_EVERY);
        loop {
            tokio::select! {
                biased;
                origin = &mut cancelled => {
                    break (JobExit::Killed, None, Some(kill(&j.pgids)), Some(origin));
                }
                () = &mut deadline => break (JobExit::Timeout, None, Some(kill(&j.pgids)), None),
                r = &mut run => break match r {
                    Ok(r) => (JobExit::Exited(u8::from(r.exit_code)), None, None, None),
                    Err(e) => (JobExit::Exited(1), Some(e.to_string()), None, None),
                },
                _ = tick.tick(), if !j.quiet => flush(&j.rt, j.session, &j.id, &j.log).await,
            }
        }
    };
    // The `&` jobs the command left running. Every `shell` call runs on a
    // fresh clone of the session shell, whose job table starts empty, so
    // taking them changes nothing a later `wait`/`jobs` builtin could see.
    let bg = std::mem::take(&mut j.shell.jobs_mut().jobs);
    // Killed: the groups just signalled, verified gone within `KILL_WAIT`.
    // Exited: every recorded group, waited for as long as it lives.
    let (groups, killed) = match killed {
        Some(g) => (g, true),
        None => (groups(&j.pgids), false),
    };
    drop(j.params); // closes our pipe writers so the drains can finish

    // The result waits `SETTLE` for the tail; past that the tail is its own
    // operation and the output is what the readers collected so far (the log
    // has the rest).
    let mut tail = Box::pin(settle(bg, vec![out.task, err.task], groups, killed));
    match tokio::time::timeout(SETTLE, tail.as_mut()).await {
        Ok(why) if why.is_empty() => {}
        Ok(why) => linger(&j.activity, &j.id, std::future::ready(why)).await,
        Err(_) => linger(&j.activity, &j.id, tail).await,
    }
    let stdout = std::mem::take(&mut *lock(&out.head));
    let stderr = std::mem::take(&mut *lock(&err.head));

    let secs = j.started.elapsed().as_secs();
    // Whole text, once: exact where the chunk pass above is best effort.
    let mut body = render(&stdout, &stderr);
    if let std::borrow::Cow::Owned(clean) = crate::secrets::redact(&body) {
        body = clean;
    }
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
            let applied = j.timeout.as_secs();
            let limit = match j.requested {
                Some(r) if r.as_secs() == applied => format!(" (timeout_secs={applied})"),
                Some(r) => format!(" (timeout_secs={} capped to {applied})", r.as_secs()),
                None => String::new(),
            };
            let out = ToolOutput::err(format!(
                "{body}{}timed out after {applied}s{limit}; process group killed",
                sep(&body)
            ));
            (body, out)
        }
        (JobExit::Killed, None) => {
            // The `start` reply (a kill inside the grace window) keeps the
            // body; the event carries only who killed it (14 §1.2 as-built).
            let body = truncate(body);
            let out = ToolOutput::err(format!("{body}{}killed", sep(&body)));
            let origin = origin.expect("Killed comes from the cancel arm");
            (origin.notice().to_owned(), out)
        }
    };

    // A killed shell was dropped mid-command; its state is not worth keeping.
    if !matches!(exit, JobExit::Timeout | JobExit::Killed) {
        (j.put_back)(j.shell);
    }
    // Claim the result (see `Start`); then the row update is queued before
    // anyone can react to the reply or the event. The model that called
    // `cancel_job` owes itself nothing: its kill is inline too.
    let inline = j.claimable.swap(false, SeqCst) || origin == Some(CancelOrigin::Tool);
    // 37 §3.2: a quiet job's output reaches only the `Start` asker.
    let event = (!j.quiet).then(|| JobEvent::Finished {
        session: j.session,
        id: j.id.clone(),
        command: j.shown,
        exit: exit.clone(),
        secs,
        output,
        inline,
        pty: false,
    });
    if !j.quiet {
        flush(&j.rt, j.session, &j.id, &j.log).await;
    }
    // The result rides the guard only when nobody else returns it: not
    // inline (the tool call has it), not quiet (the caller consumes it).
    let _ =
        j.me.tell(Finished {
            op: j.activity.id(),
            row: (!j.quiet).then(|| (j.session, j.id.clone(), exit, secs)),
            result: event.clone().filter(|_| !inline),
        })
        .await;
    if let Some(event) = event {
        j.rt.publish(event).await;
    }
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
        "Kill a running background job by id. This reply is the whole acknowledgement: no <job_result> follows a job you killed.".into()
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
                    origin: CancelOrigin::Tool,
                })
                .await
            {
                Ok(()) => ToolOutput::ok(format!("killed {id}; no <job_result> will follow")),
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
        let dir = std::env::temp_dir().join(format!("theuth-jobs-{}", uuid::Uuid::new_v4()));
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
        spawn_quiet(fx, command, false).await
    }

    async fn spawn_quiet(fx: &Fx, command: &str, quiet: bool) -> StartReply {
        fx.jobs
            .ask(Start {
                session: fx.session,
                command: command.into(),
                call_id: String::new(),
                quiet,
                shell: ShellTool::build_shell(false).await.unwrap(),
                put_back: Arc::new(|_| {}),
                root: fx.dir.clone(),
                activity: None,
                timeout: None,
            })
            .await
            .unwrap()
    }

    /// A fixture with a real supervisor recipient installed, as the
    /// registry's `RegisterActivity` does.
    pub(crate) async fn certified(
        grace: Duration,
        timeout: Duration,
    ) -> (
        Fx,
        ActorRef<crate::activity::test_sink::Ledger>,
        Arc<Mutex<Vec<crate::activity::test_sink::Seen>>>,
    ) {
        let fx = fixture(grace, timeout).await;
        let (ledger, sink, seen) = crate::activity::test_sink::spawn(None);
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        (fx, ledger, seen)
    }

    /// Polls the drained ledger until `pred` holds, within `within`.
    pub(crate) async fn ledger_until(
        ledger: &ActorRef<crate::activity::test_sink::Ledger>,
        seen: &Mutex<Vec<crate::activity::test_sink::Seen>>,
        within: Duration,
        pred: impl Fn(&[crate::activity::test_sink::Seen]) -> bool,
    ) -> Vec<crate::activity::test_sink::Seen> {
        let deadline = Instant::now() + within;
        loop {
            let v = crate::activity::test_sink::drained(ledger, seen).await;
            if pred(&v) {
                return v;
            }
            assert!(Instant::now() < deadline, "ledger never satisfied: {v:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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

    /// 37 §3.2: a quiet job past grace still replies `Done`, and nothing of
    /// it reaches the bus, the log dir or the `jobs` table.
    #[tokio::test]
    async fn quiet_job_is_invisible() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let reply = spawn_quiet(&fx, "sleep 0.3; echo s3cret", true).await;
        match reply {
            StartReply::Done(out) => assert_eq!(out.content, "s3cret\nexit code: 0"),
            r => panic!("expected Done, got {r:?}"),
        }
        assert!(lock(&fx.events).is_empty());
        assert!(list(&fx).await.is_empty());
        assert!(!fx.dir.join("jobs").exists());
    }

    /// 47 §4.2 rule 3 / §4.1: the job is admitted (`Kind::Job`) before it
    /// runs; a result the tool call did not return rides the guard as
    /// `FinishedWithResult` — and only then, never as a bare `Finished`.
    #[tokio::test]
    async fn stub_result_settles_with_result() {
        use crate::activity::test_sink::Seen;
        let (fx, ledger, seen) =
            certified(Duration::from_millis(100), Duration::from_secs(60)).await;
        let StartReply::Running { .. } = spawn_job(&fx, "sleep 0.5; echo late").await else {
            panic!("expected Running");
        };
        let v = crate::activity::test_sink::drained(&ledger, &seen).await;
        let [Seen::Admitted(op)] = v.as_slice() else {
            panic!("expected one admission, got {v:?}");
        };
        assert_eq!(op.owner, fx.session);
        assert_eq!(op.kind, Kind::Job(JobId("j1".into())));
        let op = op.id;
        let v = ledger_until(&ledger, &seen, Duration::from_secs(3), |v| v.len() >= 2).await;
        assert_eq!(v[1..], [Seen::Result(op)]);
        assert!(!finished(&fx).await.2);
    }

    /// Inline and quiet results are consumed by the tool call: the guard
    /// settles bare, so the supervisor owes no second wake (47 §3.2).
    #[tokio::test]
    async fn inline_and_quiet_settle_bare() {
        use crate::activity::test_sink::Seen;
        let (fx, ledger, seen) = certified(Duration::from_secs(3), Duration::from_secs(60)).await;
        assert!(matches!(
            spawn_job(&fx, "echo hi").await,
            StartReply::Done(_)
        ));
        assert!(matches!(
            spawn_quiet(&fx, "echo quiet", true).await,
            StartReply::Done(_)
        ));
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| v.len() >= 4).await;
        let ops: Vec<_> = v
            .iter()
            .filter_map(|s| match s {
                Seen::Admitted(op) => Some(op.id),
                _ => None,
            })
            .collect();
        assert_eq!(ops.len(), 2);
        for op in ops {
            assert!(v.contains(&Seen::Finished(op)), "{v:?}");
            assert!(!v.contains(&Seen::Result(op)), "{v:?}");
        }
    }

    /// `command` returns `Done` with `output` within `SETTLE`, leaves a
    /// `&` child behind as an open child operation of the job, and that
    /// operation finishes — never unknown — once the child actually exits.
    async fn background_child_stays_owned(command: &str, output: &str) {
        use crate::activity::test_sink::Seen;
        let (fx, ledger, seen) = certified(Duration::from_secs(5), Duration::from_secs(60)).await;
        let started = Instant::now();
        let StartReply::Done(out) = spawn_job(&fx, command).await else {
            panic!("expected Done");
        };
        assert_eq!(out.content, output);
        // The reply came after `SETTLE`, not after `sleep`.
        assert!(
            started.elapsed() < Duration::from_millis(1800),
            "{:?}",
            started.elapsed()
        );
        let v = crate::activity::test_sink::drained(&ledger, &seen).await;
        let job = match &v[0] {
            Seen::Admitted(op) => op.id,
            s => panic!("{s:?}"),
        };
        let child = v
            .iter()
            .find_map(|s| match s {
                Seen::Admitted(op) if op.kind == Kind::Command("job background process") => {
                    Some(op.id)
                }
                _ => None,
            })
            .expect("background operation admitted");
        assert!(v.contains(&Seen::Finished(job)), "{v:?}");
        assert!(!v.contains(&Seen::Finished(child)), "{v:?}");
        let v = ledger_until(&ledger, &seen, Duration::from_secs(3), |v| {
            v.contains(&Seen::Finished(child))
        })
        .await;
        assert!(!v.iter().any(|s| matches!(s, Seen::Unknown(..))), "{v:?}");
    }

    /// 47 §4.2 rule 4: a `&` child holding the pipe past the command's return
    /// is this session's work until it exits; the job's result is not held up
    /// and keeps what the reader had collected.
    #[tokio::test]
    async fn background_child_keeps_a_pipe_operation_open() {
        background_child_stays_owned("sleep 2 & echo bg", "bg\nexit code: 0").await;
    }

    /// A grandchild with its output redirected, left by a foreground `sh`
    /// that already exited: no pipe is held and the shell's job table is
    /// empty — the recorded process group is the evidence, never the
    /// command text.
    #[tokio::test]
    async fn redirected_background_process_stays_owned() {
        background_child_stays_owned(
            "sh -c 'sleep 2 >/dev/null 2>&1 &'; echo bg",
            "bg\nexit code: 0",
        )
        .await;
    }

    /// Doubt about the tail (a dead reader, a group that survived SIGKILL)
    /// blocks as unknown on the child operation only: the job still settles
    /// with the result it owes, so the model is woken with what there is.
    #[tokio::test]
    async fn uncertain_tail_still_delivers_the_result() {
        use crate::activity::test_sink::Seen;
        let (ledger, sink, seen) = crate::activity::test_sink::spawn(None);
        let id = JobId("j1".into());
        let job = activity::admit(Some(&sink), SessionId::new(), Kind::Job(id.clone()))
            .await
            .unwrap();
        linger(
            &job,
            &id,
            std::future::ready(vec!["output reader died".to_owned()]),
        )
        .await;
        job.set_result(JobEvent::Finished {
            session: job.owner(),
            id,
            command: String::new(),
            exit: JobExit::Exited(0),
            secs: 0,
            output: String::new(),
            inline: false,
            pty: false,
        });
        let root = job.id();
        drop(job);
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| v.len() >= 4).await;
        let Seen::Admitted(child) = &v[1] else {
            panic!("{v:?}")
        };
        assert!(
            v.iter().any(|s| matches!(s, Seen::Unknown(op, why) if *op == child.id && why.contains("reader died"))),
            "{v:?}"
        );
        assert!(v.contains(&Seen::Result(root)), "{v:?}");
        assert!(
            !v.iter()
                .any(|s| matches!(s, Seen::Unknown(op, _) | Seen::Finished(op) if *op == root)),
            "{v:?}"
        );
    }

    /// Setup failure after admission settles the guard: no leaked blocker.
    #[tokio::test]
    async fn setup_error_settles_admission() {
        use crate::activity::test_sink::Seen;
        let (fx, ledger, seen) = certified(Duration::from_secs(3), Duration::from_secs(60)).await;
        // The jobs dir is a file: `create_dir_all` fails before anything spawns.
        std::fs::write(fx.dir.join("jobs"), b"").unwrap();
        let StartReply::Done(out) = spawn_job(&fx, "echo never").await else {
            panic!("expected Done");
        };
        assert!(
            out.is_error && out.content.starts_with("cannot start job:"),
            "{}",
            out.content
        );
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| v.len() >= 2).await;
        let Seen::Admitted(op) = &v[0] else {
            panic!("{v:?}")
        };
        assert_eq!(v[1], Seen::Finished(op.id));
        assert!(list(&fx).await.is_empty());
    }

    /// A supervisor rejecting the admission is an ordinary tool error and the
    /// process never runs.
    #[tokio::test]
    async fn rejected_admission_runs_nothing() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (_ledger, sink, _seen) = crate::activity::test_sink::spawn(Some("paused"));
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        let marker = fx.dir.join("ran");
        let StartReply::Done(out) = spawn_job(&fx, &format!("touch {}", marker.display())).await
        else {
            panic!("expected Done");
        };
        assert_eq!(out.content, "cannot start job: paused");
        assert!(!marker.exists());
        assert!(list(&fx).await.is_empty());
    }

    /// 47 §4.2 rule 7: a job running before any supervisor existed is adopted
    /// on install — reported unknown, then settled by its own producer with
    /// the result it still owes; a job started after install is admitted.
    #[tokio::test]
    async fn install_adopts_running_jobs() {
        use crate::activity::test_sink::Seen;
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let StartReply::Running { .. } = spawn_job(&fx, "sleep 0.6; echo pre").await else {
            panic!("expected Running");
        };
        let (ledger, sink, seen) = crate::activity::test_sink::spawn(None);
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        let v = crate::activity::test_sink::drained(&ledger, &seen).await;
        let [Seen::Unknown(pre, why)] = v.as_slice() else {
            panic!("expected the running job reported unknown, got {v:?}");
        };
        assert!(
            why.contains("before the activity supervisor registered"),
            "{why}"
        );
        let pre = *pre;
        let StartReply::Running { .. } = spawn_job(&fx, "sleep 0.3; echo post").await else {
            panic!("expected Running");
        };
        let v = ledger_until(&ledger, &seen, Duration::from_secs(3), |v| {
            v.iter().filter(|s| matches!(s, Seen::Result(_))).count() == 2
        })
        .await;
        assert!(
            matches!(&v[1], Seen::Admitted(op) if op.kind == Kind::Job(JobId("j2".into()))),
            "{v:?}"
        );
        assert!(v.contains(&Seen::Result(pre)), "{v:?}");
        assert!(!v.contains(&Seen::Finished(pre)), "{v:?}");
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
        let pgid: u32 = list(&fx).await[0].tail.trim().parse().unwrap();
        assert!(platform::process::pid_alive(pgid));

        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: id.clone(),
                origin: CancelOrigin::Ui,
            })
            .await
            .unwrap();
        assert_eq!(settled(&fx, Duration::from_secs(1)).await.state, "killed");
        // A human's kill reaches the model as one line, not the output.
        assert_eq!(
            finished(&fx).await,
            (JobExit::Killed, "killed by the user".to_owned(), false)
        );
        // Reaped by tokio's orphan queue on the next SIGCHLD; allow it a moment.
        let gone = Instant::now() + Duration::from_secs(1);
        while platform::process::pid_alive(pgid) {
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
                origin: CancelOrigin::Ui,
            })
            .await;
        assert_eq!(refused(again), "j1 already finished");
        let missing = fx
            .jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId("j9".into()),
                origin: CancelOrigin::Ui,
            })
            .await;
        assert_eq!(refused(missing), "no such job");
    }

    /// 14 §1.2 as-built: the model's own `cancel_job` gets no `<job_result>`
    /// — the event is inline with the one-line notice, the output dropped.
    #[tokio::test]
    async fn tool_cancel_is_inline_and_quiet() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let StartReply::Running { id, .. } = spawn_job(&fx, "echo loud; sleep 30").await else {
            panic!("expected Running");
        };
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id,
                origin: CancelOrigin::Tool,
            })
            .await
            .unwrap();
        assert_eq!(settled(&fx, Duration::from_secs(1)).await.state, "killed");
        assert_eq!(
            finished(&fx).await,
            (JobExit::Killed, "killed by cancel_job".to_owned(), true)
        );
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
        let tool = ShellTool::new(fx.jobs.clone(), false);
        let cx = ToolCx {
            session: fx.session,
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&fx.dir),
        };
        let cd = Tool::call(
            &tool,
            crate::shell::ShellArgs {
                command: "cd /tmp".into(),
                cwd: None,
                timeout_secs: None,
                env: None,
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
                cwd: None,
                timeout_secs: None,
                env: None,
            },
            cx,
        )
        .await
        .unwrap();
        assert_eq!(pwd.content, "/tmp\nexit code: 0");
    }

    #[tokio::test]
    async fn close_sessions_kills_reaps_and_removes_logs() {
        let fx = fixture(Duration::from_millis(200), Duration::from_secs(60)).await;
        let StartReply::Running { log, .. } =
            spawn_job(&fx, "sh -c 'echo $$; exec sleep 30'").await
        else {
            panic!("expected Running");
        };
        let pid: u32 = list(&fx).await[0].tail.trim().parse().unwrap();
        assert!(platform::process::pid_alive(pid));
        assert_eq!(
            log,
            fx.dir
                .join("jobs")
                .join(fx.session.to_string())
                .join("j1.log")
        );
        assert!(log.is_file());

        let ack = fx
            .jobs
            .ask(CloseSessions {
                sessions: vec![fx.session, SessionId::new()],
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        assert_eq!(ack.owner, "jobs");
        assert!(ack.ok, "{ack:?}");
        assert!(!platform::process::pid_alive(pid));
        assert!(!log.parent().unwrap().exists());
        assert!(list(&fx).await.is_empty());
    }
}
