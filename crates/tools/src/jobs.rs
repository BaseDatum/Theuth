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
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
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
use crate::journal::{JobKind, JobRow, JobState, PtyScreenRow};
use crate::pty::{Mark, MarkKind};
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
/// 512 MiB ≈ 80 live PTYs engine-wide (the estimate, not measured RSS).
pub(crate) const PTY_BUDGET: usize = 512 << 20;
/// 171 §12.1 inv. 3: the `name` prefix of a user-cell kernel job. Reserved:
/// `start` refuses it without `StartPty.cell`.
pub const CELL_PREFIX: &str = "cell:";
/// 171 §9.7: what `send` / `cancel_job` answer on a kernel job.
pub const CELL_REFUSED: &str = "this terminal belongs to a user cell — ask the user to press Run";
/// 171F B24: a kernel job's log is `<root>/jobs/<session>/<id>.cell.log`, so
/// the file tools can refuse it from the path alone, across engine restarts.
pub(crate) const CELL_LOG: &str = ".cell.log";
/// 171F3 R1: the `command` suffix of a user-cell kernel job (`cells::shell`),
/// the second marker a persisted row carries besides its `cell:` name.
pub const CELL_COMMAND: &str = " (user cell kernel)";
/// 171 §5.5: a mark sequence split across chunks is completed from this
/// much carried tail; OSC 7 carries a path, so it gets [`OSC7_CARRY`].
const MARK_CARRY: usize = 32;
const OSC7_CARRY: usize = 1024;

/// Is `name` a user-cell kernel job's (`cell:<label>`)?
pub(crate) fn is_cell(name: Option<&str>) -> bool {
    name.is_some_and(|n| n.starts_with(CELL_PREFIX))
}

/// 171F3 R4: why no program would run a user cell of language word `lang`
/// (`bash`, `pwsh`, …; a note's `shell:` default second) on this engine —
/// `Some("no pwsh on this machine")` — or `None` when one resolves or `lang`
/// is not a shell. The cells plugin installs it at start (it owns the login
/// PATH and the resolver); the agent's reply check and the note-write check
/// ask it ([`program_missing`]). Before an install nothing is flagged.
type ProgramCheck = Arc<
    dyn Fn(String, Option<String>) -> futures::future::BoxFuture<'static, Option<String>>
        + Send
        + Sync,
>;

static PROGRAM_CHECK: std::sync::RwLock<Option<ProgramCheck>> = std::sync::RwLock::new(None);

/// Install (replace) the program check (see [`program_missing`]).
pub fn set_program_check<F, Fut>(check: F)
where
    F: Fn(String, Option<String>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<String>> + Send + 'static,
{
    let check: ProgramCheck = Arc::new(move |lang, shell| Box::pin(check(lang, shell)));
    *PROGRAM_CHECK
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(check);
}

/// 171F3 R4: ask the installed program check why no program would run a `lang`
/// cell here; `None` when one resolves, `lang` is no shell, or none is installed.
pub async fn program_missing(lang: &str, note_shell: Option<&str>) -> Option<String> {
    let check = PROGRAM_CHECK
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    check(lang.to_owned(), note_shell.map(str::to_owned)).await
}

/// `j{n}`, `n` per session starting at 1.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, kameo::Reply)]
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
    /// 109 §5.4: the job died with the engine (or was reaped after a
    /// SIGKILL leaked it); its exit code is unknown. Never produced by a
    /// running job — written by the journal's reaper and the shutdown path.
    Lost,
}

impl std::fmt::Display for JobExit {
    /// The [`JobView::state`] wording: `exited 0`, `timeout`, `killed`, `lost`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobExit::Exited(code) => write!(f, "exited {code}"),
            JobExit::Timeout => f.write_str("timeout"),
            JobExit::Killed => f.write_str("killed"),
            JobExit::Lost => f.write_str("lost"),
        }
    }
}

impl std::str::FromStr for JobExit {
    type Err = String;

    /// Inverse of [`Display`](std::fmt::Display): the `jobs.exit` column (109 §5.1).
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "timeout" => Ok(Self::Timeout),
            "killed" => Ok(Self::Killed),
            "lost" => Ok(Self::Lost),
            _ => s
                .strip_prefix("exited ")
                .and_then(|n| n.parse().ok())
                .map(Self::Exited)
                .ok_or_else(|| format!("unknown job exit {s:?}")),
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
        /// 145 §0.4: `Promoted` for a tool call that outlived `promote_after`
        /// (`command` is the tool name); `Shell` / `Pty` otherwise.
        #[serde(default)]
        kind: JobKind,
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
        /// 155 §6.5: why it fired — `screen unchanged for N s, process tree
        /// quiet` or `no reaction to input`. Empty from older engines.
        #[serde(default)]
        why: String,
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
        /// 145 §0.4: `Promoted` → the agent wraps it as `<job_result tool="…">`.
        #[serde(default)]
        kind: JobKind,
    },
}

impl wire::WireEvent for JobEvent {
    const TOPIC: &'static str = "tools.jobs";
    const VERSION: u32 = 1;
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
    /// `running` | `exited 0` | `timeout` | `killed` | `lost` (109 §5.4: died
    /// with a previous engine; `tail` is the log tail or the stored screen).
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
    /// Process exit status, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub exit_code: Option<i32>,
    /// Spawn time, unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub started_ms: Option<i64>,
    /// Settlement time, unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ended_ms: Option<i64>,
    /// `shell`, `pty`, or the promoted tool's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub kind: Option<String>,
    /// The originating tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub call_id: Option<String>,
    /// Current size of the retained log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub log_bytes: Option<u64>,
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

/// `ask` → `Vec<JobView>` for the session, oldest first; user-cell kernel
/// jobs (`cell:` names) are omitted (171 §12.1 inv. 3).
#[derive(Clone, Copy, Debug)]
pub struct ListJobs(pub SessionId);

/// In-process TUI recovery: retain kernel visibility without exposing it on RPC.
pub struct LocalJobIds(pub SessionId);

impl Message<LocalJobIds> for Jobs {
    type Reply = Vec<JobId>;
    async fn handle(
        &mut self,
        query: LocalJobIds,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sessions
            .get(&query.0)
            .map_or_else(Vec::new, |session| {
                session
                    .jobs
                    .iter()
                    .filter(|job| job.exit.is_none())
                    .map(|job| job.id.clone())
                    .collect()
            })
    }
}

/// `ask` → `HashMap<SessionId, (u16, u16)>`: running `(pipe jobs, PTY jobs)`
/// per requested session (62 §0.5); a session with none may be absent.
#[derive(Clone, Debug)]
pub struct JobCounts(pub Vec<SessionId>);

/// `ask` → `Vec<LiveCountsRow>`: every session with a running job, a live
/// PTY or a pending reminder (the desktop's sidebar snapshot; live
/// `JobEvent`s / `ReminderEvent`s keep it current afterwards).
#[derive(Clone, Copy, Debug)]
pub struct LiveCounts;

/// One row of [`LiveCounts`]; at least one list is non-empty.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LiveCountsRow {
    /// Owning session.
    pub session: SessionId,
    /// Running non-PTY jobs.
    pub jobs: Vec<JobId>,
    /// Running PTY jobs.
    pub ptys: Vec<JobId>,
    /// Pending reminder ids (`r<n>`).
    pub reminders: Vec<String>,
}

/// `ask` → `Result<(), String>`: SIGKILL a running job's process groups.
/// Err `no such job` | `j3 already finished` | [`CELL_REFUSED`] for a kernel
/// job unless `origin` is `Shutdown` (171 §9.7).
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

/// `tell` on `rt.shutdown`: write every running job's row `exit = lost`,
/// then kill its process groups (109 §5.5). Public so a drain (§9.2 step 6)
/// can send it after its window instead of at the instant the token fires.
pub struct KillAll;

/// `ask` → `()`: the per-session form of [`KillAll`] (139 §6.1 step 4,
/// §7.1): the session's live rows `exit = lost` first, PTY screens
/// snapshotted, then `kill(pgids)`. Sent by the release drain
/// (`supervisor::transfer::drain_session`, W2). The rows stay in this
/// actor as `lost` so `jobs()` keeps listing them and their ids are not
/// reused if the session comes back.
pub struct KillSession {
    /// The session whose jobs die with its actor.
    pub session: SessionId,
}

/// Why a job is reported `lost` (139 §7.1): the banner / `lost_page` /
/// `<job_result exit="lost">` wording keys on it (W3 renders it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LostReason {
    /// "the engine restarted at hh:mm" — today's reaper (109 §5.4).
    #[default]
    Restart,
    /// "owner changed (ran on `engine`)" — the session moved to this engine
    /// while the process lived on `engine`.
    OwnerChanged {
        /// The machine that spawned it, as text: its mesh name, else
        /// "another machine" (183 R1 H2; never the `jobs.engine` id).
        engine: String,
    },
}

/// `tell` from the agent's reaper (109 §5.4): the `jobs` rows a previous
/// engine left `running`, settled `lost` in the store by the caller. Each
/// becomes a `lost` row here — `jobs()`, `screen()` and `pty_screen` answer
/// for it, its id is not reused — and, when [`LostJob::deliver`] is set,
/// the model's `<job_result exit="lost">` goes through the activity ledger,
/// the path every finished job's result takes (47 §4.1).
#[derive(Clone, Debug)]
pub struct Reaped {
    /// When this engine started: the banner's "restarted at".
    pub restarted_at: chrono::DateTime<chrono::Utc>,
    /// The rows, oldest first.
    pub jobs: Vec<LostJob>,
}

/// One lost job for [`Reaped`].
#[derive(Clone, Debug)]
pub struct LostJob {
    /// The settled row (`state = lost`).
    pub row: JobRow,
    /// The stored screen of a PTY job, if any (§5.3).
    pub screen: Option<PtyScreenRow>,
    /// Last ≤ 8 KiB of the log file: [`JobView::tail`] for the row.
    pub tail: String,
    /// `Some(body)` when the model already had the job's stub: the
    /// `<job_result exit="lost">` body to inject. `None` when the resumed
    /// batch reports the job itself (§4.3's job note).
    pub deliver: Option<String>,
    /// Why it is lost (139 §7.1); `Restart` from the boot reaper.
    pub reason: LostReason,
}

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
/// (capped the same way). The file is held only while something still
/// writes it: the row the actor retains after the exit keeps the tail and
/// the path, never the descriptor (`close`, `writer_done`).
pub(crate) struct Log {
    file: Option<std::fs::File>,
    tail: VecDeque<u8>,
    pending: VecDeque<u8>,
    /// Readers still feeding `push` (one per pipe drain); the file goes
    /// with the last one.
    writers: usize,
    /// Bytes appended so far, after redaction: the file offset of the next
    /// chunk (171 §5.5 `Mark::offset`).
    written: u64,
    /// 171 §5.5: set for a user-cell PTY job; `None` for every other log.
    marks: Option<MarkScan>,
}

/// 171 §5.5: where a cell job's shell-integration marks go, and the tail
/// of the last chunk that may begin one.
struct MarkScan {
    tx: tokio::sync::mpsc::Sender<Mark>,
    carry: Vec<u8>,
}

impl Log {
    pub(crate) fn new(file: Option<std::fs::File>) -> Self {
        Self {
            file,
            tail: VecDeque::with_capacity(TAIL),
            pending: VecDeque::new(),
            writers: 0,
            written: 0,
            marks: None,
        }
    }

    /// 171 §5.5: scan every chunk from now on for marks and send them to `tx`.
    pub(crate) fn scan_marks_to(&mut self, tx: tokio::sync::mpsc::Sender<Mark>) {
        self.marks = Some(MarkScan {
            tx,
            carry: Vec::new(),
        });
    }

    /// Drop the file: the output is drained, or nobody will read on.
    pub(crate) fn close(&mut self) {
        self.file = None;
    }

    /// One more reader feeds this log; paired with [`Log::writer_done`].
    fn writer(&mut self) {
        self.writers += 1;
    }

    /// A reader hit EOF (or died): the last one closes the file.
    fn writer_done(&mut self) {
        self.writers -= 1;
        if self.writers == 0 {
            self.close();
        }
    }

    /// A file-less log whose tail is `bytes` (109 §5.4: a lost job's row
    /// shows the last of what its log file holds).
    pub(crate) fn from_tail(bytes: &[u8]) -> Self {
        let mut log = Self::new(None);
        keep_last(&mut log.tail, bytes);
        log
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
        if let Some(scan) = &mut self.marks {
            for mark in scan_marks(&mut scan.carry, self.written, chunk) {
                if let Err(e) = scan.tx.try_send(mark) {
                    tracing::debug!("cell mark dropped: {e}");
                }
            }
        }
        self.written += chunk.len() as u64;
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

#[cfg(test)]
impl Log {
    pub(crate) fn is_open(&self) -> bool {
        self.file.is_some()
    }
}

/// `ask` → the finished rows of `session` that still hold a descriptor
/// (`"j3 log"`, `"j4 pty"`): the D1/D2 probe.
#[cfg(test)]
pub(crate) struct OpenHandles(pub(crate) SessionId);

#[cfg(test)]
impl Message<OpenHandles> for Jobs {
    type Reply = Vec<String>;

    async fn handle(&mut self, m: OpenHandles, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut held = Vec::new();
        for j in self.sessions.get(&m.0).into_iter().flat_map(|s| &s.jobs) {
            if j.exit.is_none() {
                continue;
            }
            if lock(&j.log).is_open() {
                held.push(format!("{} log", j.id));
            }
            if j.pty.as_ref().is_some_and(|p| p.holds_fds()) {
                held.push(format!("{} pty", j.id));
            }
        }
        held
    }
}

fn keep_last(buf: &mut VecDeque<u8>, chunk: &[u8]) {
    let over = (buf.len() + chunk.len()).saturating_sub(TAIL);
    buf.drain(..over.min(buf.len()));
    buf.extend(&chunk[chunk.len().saturating_sub(TAIL)..]);
}

/// 171 §5.5: the marks in `carry ++ chunk`, where `chunk` begins at log
/// offset `base` and `carry` is the unfinished tail of the previous chunk
/// (already in the file, just before `base`). Recognised: `OSC 133 ; A|C|D[;n]`
/// and `OSC 7 ; file://<host><path>`, each ended by BEL or ST; any other OSC
/// is skipped. On return `carry` holds the strict prefix of a mark that ended
/// the chunk, up to [`MARK_CARRY`] bytes ([`OSC7_CARRY`] for an OSC 7 path),
/// or nothing.
pub(crate) fn scan_marks(carry: &mut Vec<u8>, base: u64, chunk: &[u8]) -> Vec<Mark> {
    let start = base - carry.len() as u64;
    let mut buf = std::mem::take(carry);
    buf.extend_from_slice(chunk);
    let mut marks = Vec::new();
    let mut i = 0;
    while let Some(esc) = buf[i..].iter().position(|b| *b == 0x1b) {
        let at = i + esc;
        match parse_mark(&buf[at..]) {
            Parsed::Done(mark, len) => {
                if let Some((kind, exit, cwd)) = mark {
                    marks.push(Mark {
                        kind,
                        exit,
                        cwd,
                        offset: start + at as u64,
                    });
                }
                i = at + len;
            }
            Parsed::Not => i = at + 1,
            Parsed::Incomplete => {
                let tail = &buf[at..];
                let cap = if tail.get(2) == Some(&b'7') {
                    OSC7_CARRY
                } else {
                    MARK_CARRY
                };
                if tail.len() <= cap {
                    carry.extend_from_slice(tail);
                }
                break;
            }
        }
    }
    marks
}

/// [`parse_mark`]'s verdict on bytes starting at an ESC.
enum Parsed {
    /// A complete OSC of `len` bytes; `Some` when it is a mark.
    Done(Option<(MarkKind, Option<i32>, Option<String>)>, usize),
    /// A strict prefix of a mark: wait for more bytes.
    Incomplete,
    /// Not a mark (or an OSC that is not one and has no terminator yet).
    Not,
}

/// Parse one mark at `s[0] == ESC`.
fn parse_mark(s: &[u8]) -> Parsed {
    const P133: &[u8] = b"\x1b]133;";
    const P7: &[u8] = b"\x1b]7;file://";
    let Some(pat) = [P133, P7].into_iter().find(|p| s.starts_with(p)) else {
        // Shorter than a pattern it begins: wait; otherwise not a mark.
        return if [P133, P7].iter().any(|p| p.starts_with(s)) {
            Parsed::Incomplete
        } else {
            Parsed::Not
        };
    };
    // The payload ends at BEL or ST; a bare ESC inside aborts the OSC.
    let mut end = pat.len();
    let term = loop {
        match s.get(end) {
            None => return Parsed::Incomplete,
            Some(0x07) => break 1,
            Some(0x1b) => match s.get(end + 1) {
                None => return Parsed::Incomplete,
                Some(b'\\') => break 2,
                Some(_) => return Parsed::Not,
            },
            Some(_) => end += 1,
        }
    };
    let payload = &s[pat.len()..end];
    let len = end + term;
    let mark = if pat == P133 {
        let kind = match payload.first() {
            Some(b'A') => MarkKind::Ready,
            Some(b'C') => MarkKind::Start,
            Some(b'D') => MarkKind::End,
            _ => return Parsed::Done(None, len),
        };
        let exit = payload[1..]
            .strip_prefix(b";")
            .and_then(|n| std::str::from_utf8(n).ok()?.parse().ok());
        (kind, exit, None)
    } else {
        // `file://<host><path>`: the path starts at the first `/` after the host.
        let Some(slash) = payload.iter().position(|b| *b == b'/') else {
            return Parsed::Done(None, len);
        };
        (MarkKind::Cwd, None, Some(percent_decode(&payload[slash..])))
    };
    Parsed::Done(Some(mark), len)
}

/// `%XX` → the byte; anything else verbatim; lossy UTF-8.
fn percent_decode(s: &[u8]) -> String {
    percent_encoding::percent_decode(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// 109 §5.4: what a job that died with a previous engine keeps in memory
/// once the reaper reported it: when this engine started, why it is lost
/// (139 §7.1) and, for a PTY, the last stored screen.
pub(crate) struct Lost {
    pub(crate) restarted_at: chrono::DateTime<chrono::Utc>,
    pub(crate) reason: LostReason,
    pub(crate) screen: Option<PtyScreenRow>,
}

pub(crate) struct Job {
    pub(crate) id: JobId,
    /// The job's admitted operation, keying [`Jobs::live`].
    pub(crate) op: OperationId,
    pub(crate) command: String,
    /// The `shell` / `start` call (the `jobs.call_id` column).
    pub(crate) call_id: String,
    pub(crate) started: Instant,
    /// Spawn time, unix ms (the `jobs.started_ms` column).
    pub(crate) started_ms: i64,
    /// `None` while running.
    pub(crate) exit: Option<JobExit>,
    /// Total seconds once finished.
    pub(crate) secs: u64,
    /// Settlement time, unix ms.
    pub(crate) ended_ms: Option<i64>,
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
    /// `Some` for a job a previous engine lost (109 §5.4).
    pub(crate) lost: Option<Lost>,
    /// The `jobs.kind` column; `Promoted` for an adopted tool call (145 §0.4).
    pub(crate) kind: JobKind,
}

impl Job {
    fn secs(&self) -> u64 {
        if self.exit.is_some() {
            self.secs
        } else {
            self.started.elapsed().as_secs()
        }
    }

    /// `running` | `exited 0` | `timeout` | `killed` | `lost`.
    pub(crate) fn state(&self) -> String {
        self.exit
            .as_ref()
            .map_or_else(|| "running".to_owned(), ToString::to_string)
    }

    /// The `jobs` row this job mirrors (109 §5.1), in the given state.
    pub(crate) fn row(
        &self,
        session: SessionId,
        state: JobState,
        exit: Option<JobExit>,
        settled_ms: Option<i64>,
    ) -> JobRow {
        JobRow {
            session,
            id: self.id.clone(),
            kind: self.kind,
            call_id: self.call_id.clone(),
            command: self.command.clone(),
            name: self.name.clone(),
            pgid: lock(&self.pgids).first().map(|t| i64::from(t.id())),
            log: (self.kind != JobKind::Promoted).then(|| self.log_path.clone()),
            started_ms: self.started_ms,
            state,
            exit,
            settled_ms,
            engine: String::new(),
        }
    }

    fn view(&self, session: SessionId) -> JobView {
        JobView {
            session,
            id: self.id.clone(),
            command: self.command.clone(),
            state: self.state(),
            secs: self.secs(),
            log: self.log_path.clone(),
            tail: match (&self.pty, &self.lost) {
                (Some(pty), _) => pty.screen().0.join("\n"),
                (
                    None,
                    Some(Lost {
                        screen: Some(s), ..
                    }),
                ) => s.text.clone(),
                (None, _) => lock(&self.log).tail_text(TAIL),
            },
            name: self.name.clone(),
            pty: self.on_pty,
            exit_code: match self.exit {
                Some(JobExit::Exited(code)) => Some(i32::from(code)),
                _ => None,
            },
            started_ms: Some(self.started_ms),
            ended_ms: self.ended_ms,
            kind: Some(if self.kind == JobKind::Promoted {
                self.command.clone()
            } else {
                self.kind.as_str().to_owned()
            }),
            call_id: Some(self.call_id.clone()),
            log_bytes: std::fs::metadata(&self.log_path).ok().map(|m| m.len()),
        }
    }
}

#[derive(Default)]
pub(crate) struct SessionJobs {
    pub(crate) next: u32,
    /// 171F3 R2: `next` has been raised past the logs already on disk.
    seeded: bool,
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

    /// 171F3 R2: the next `j<n>`, never one the session used before. After an
    /// engine restart (or a closed tree) the counter starts past the highest
    /// `j<n>.log` / `j<n>.cell.log` in `dir` (`<root>/jobs/<session>`), and an
    /// id whose log still exists is skipped: a new job never truncates an old
    /// job's log, which cell run records and lost-job screens point at.
    // ponytail: a settled promoted job (no log file) as the session's last
    // job before a restart can still have its id reused; seed from the
    // `jobs` rows if that matters.
    pub(crate) fn alloc(&mut self, dir: &Path) -> JobId {
        if !self.seeded {
            self.seeded = true;
            self.next = self.next.max(highest_logged(dir));
        }
        loop {
            self.next += 1;
            let id = format!("j{}", self.next);
            if !dir.join(format!("{id}.log")).exists()
                && !dir.join(format!("{id}{CELL_LOG}")).exists()
            {
                return JobId(id);
            }
        }
    }
}

/// The highest `n` of a `j<n>.log` / `j<n>.cell.log` in `dir`; 0 when none.
fn highest_logged(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            let stem = name
                .strip_suffix(CELL_LOG)
                .or_else(|| name.strip_suffix(".log"))?;
            stem.strip_prefix('j')?.parse().ok()
        })
        .max()
        .unwrap_or(0)
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
    /// 109 §5.1: the journal seam ([`crate::InstallJournal`]); a
    /// [`crate::NoopJournal`] until the agent installs its store; the
    /// `jobs` / `pty_screens` rows are written through it.
    pub(crate) journal: Arc<dyn crate::Journal>,
    /// 109 §5.3: the PTY screen snapshot timer (`durability.pty_snapshot_secs`).
    pub(crate) pty_snapshot: Duration,
    /// 155 §6.6: the static `tools.pty_reaction_secs` / `pty_tree_cpu_pct`
    /// pair every PTY job task reads.
    pub(crate) pty_thresholds: crate::PtyThresholds,
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
            journal: Arc::new(crate::NoopJournal),
            pty_snapshot: Duration::from_secs(5),
            pty_thresholds: crate::PtyThresholds::from_config(&crate::ToolsConfig::default()),
        }
    }

    /// 109 §5.4: a job a previous engine lost, by exact id or name.
    pub(crate) fn lost(&self, session: SessionId, id: &str) -> Option<&Job> {
        let jobs = self.sessions.get(&session)?;
        jobs.jobs
            .iter()
            .filter(|j| j.lost.is_some())
            .find(|j| j.id.0 == id || j.name.as_deref() == Some(id))
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
    let mut actor = Jobs::new(rt.clone(), grace, timeout);
    if let Ok(secs) = rt
        .config
        .extract_inner::<u64>("durability.pty_snapshot_secs")
    {
        actor.pty_snapshot = Duration::from_secs(secs.max(1));
    }
    if let Ok(cfg) = crate::registry::config(rt) {
        actor.pty_thresholds = crate::PtyThresholds::from_config(&cfg);
    }
    let jobs = Jobs::spawn_with_mailbox(actor, runtime::subscriber_mailbox());
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
        let dir = m.root.join("jobs").join(m.session.to_string());
        let sj = self.sessions.entry(m.session).or_default();
        // A quiet job takes no row and no id: it reuses the last number, which
        // nothing publishes or lists.
        let id = if quiet {
            JobId(format!("j{}", sj.next))
        } else {
            sj.alloc(&dir)
        };
        let sink = self.activity.clone();
        let me = ctx.actor_ref().clone();
        let grace = self.grace;
        let journal = self.journal.clone();
        let (session, call_id) = (m.session, m.call_id.clone());
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
            // 109 §4.4: the grace wait is a `job` wait while it lasts.
            let within = if grace.is_zero() {
                None
            } else {
                let waits = !call_id.is_empty();
                if waits {
                    journal
                        .wait(
                            Some(crate::journal::WaitRow {
                                session,
                                tool_call_id: call_id.clone(),
                                kind: crate::journal::WaitKind::Job,
                                target: id.0.clone(),
                                deadline: Some(chrono::Utc::now() + grace),
                                payload: serde_json::json!({}),
                            }),
                            session,
                            &call_id,
                        )
                        .await;
                }
                let r = tokio::time::timeout(grace, &mut done).await.ok();
                if waits {
                    journal.wait(None, session, &call_id).await;
                }
                r
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
        let (pgid_tx, pgid_rx) = tokio::sync::mpsc::unbounded_channel();
        // On `Err` the admitted guard drops here: settled `Finished`, no blocker.
        let (file, params, out_r, err_r) =
            setup(&dir, log_file, &m.shell, &pgids, pgid_tx).map_err(|e| format!("{e:#}"))?;
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
        let started_ms = chrono::Utc::now().timestamp_millis();
        self.track(&activity);
        let op = activity.id();
        let call_id = m.call_id;
        let task = tokio::spawn(run(Run {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            command: m.command,
            shown: shown.clone(),
            call_id: call_id.clone(),
            shell: m.shell,
            put_back: m.put_back,
            params,
            out_r,
            err_r,
            log_path: log_path.clone(),
            log: log.clone(),
            pgids: pgids.clone(),
            pgid_rx,
            cancel: cancel.clone(),
            claimable: claimable.clone(),
            quiet,
            journal: self.journal.clone(),
            started_ms,
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
                    call_id,
                    started,
                    started_ms,
                    ended_ms: None,
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
                    lost: None,
                    kind: JobKind::Shell,
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

/// `ask` → `JobId` (145 §0.4): run a tool call that outlived `promote_after`
/// to completion as job `jN`. Publishes `JobEvent::Started { kind: Promoted, .. }`
/// and `JobEvent::Finished` whose body is the tool's output (`is_error` → exit 1);
/// `CancelJob` cancels `cancel` and aborts the task. Admission failure is the
/// `ask`'s `Err` (kameo lifts it): the caller then awaits the call inline.
pub struct Adopt {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id of the promoted call.
    pub call_id: String,
    /// Tool name (the job's `command`).
    pub tool: String,
    /// The call's [`ToolCx::cancel`]; cancelled by [`CancelJob`].
    pub cancel: CancellationToken,
    /// The running body; dropping it aborts the body's task.
    pub work: Pin<Box<dyn Future<Output = Result<ToolOutput, crate::ToolError>> + Send>>,
}

impl Message<Adopt> for Jobs {
    type Reply = DelegatedReply<Result<JobId, String>>;

    async fn handle(&mut self, m: Adopt, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let dir = self.rt.data_dir.join("jobs").join(m.session.to_string());
        let id = self.sessions.entry(m.session).or_default().alloc(&dir);
        let sink = self.activity.clone();
        let me = ctx.actor_ref().clone();
        // Admission awaits the supervisor: off the mailbox. The invocation
        // stays admitted until it gets this reply (47 §4.2 rule 3).
        tokio::spawn(async move {
            let activity =
                match admit_job(None, sink.as_ref(), m.session, Kind::Job(id.clone())).await {
                    Ok(a) => a,
                    Err(why) => return tx.send(Err(why.to_string())),
                };
            // A handler `Err` arrives lifted as `SendError::HandlerError`.
            tx.send(
                me.ask(Launch { m, id, activity })
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

impl Message<Launch<Adopt>> for Jobs {
    type Reply = Result<JobId, String>;

    async fn handle(
        &mut self,
        l: Launch<Adopt>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Launch { m, id, activity } = l;
        let cancel = Arc::new(Cancel::default());
        let started = Instant::now();
        let started_ms = chrono::Utc::now().timestamp_millis();
        self.track(&activity);
        let op = activity.id();
        let task = tokio::spawn(run_adopted(Adopted {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            tool: m.tool.clone(),
            call_id: m.call_id.clone(),
            token: m.cancel,
            work: m.work,
            cancel: cancel.clone(),
            journal: self.journal.clone(),
            started,
            started_ms,
            activity,
        }))
        .abort_handle();
        self.push(
            m.session,
            Job {
                id: id.clone(),
                op,
                command: m.tool,
                call_id: m.call_id,
                started,
                started_ms,
                ended_ms: None,
                exit: None,
                secs: 0,
                // No process, no log: the body's output is the result.
                log_path: PathBuf::new(),
                log: Arc::new(Mutex::new(Log::new(None))),
                pgids: Arc::default(),
                cancel,
                task,
                name: None,
                on_pty: false,
                pty: None,
                lost: None,
                kind: JobKind::Promoted,
            },
        );
        Ok(id)
    }
}

/// Everything one promoted job's task owns (145 §0.4).
struct Adopted {
    rt: Runtime,
    me: ActorRef<Jobs>,
    session: SessionId,
    id: JobId,
    tool: String,
    call_id: String,
    token: CancellationToken,
    work: Pin<Box<dyn Future<Output = Result<ToolOutput, crate::ToolError>> + Send>>,
    cancel: Arc<Cancel>,
    journal: Arc<dyn crate::Journal>,
    started: Instant,
    started_ms: i64,
    activity: Activity,
}

async fn run_adopted(j: Adopted) {
    let mut row = JobRow {
        session: j.session,
        id: j.id.clone(),
        kind: JobKind::Promoted,
        call_id: j.call_id.clone(),
        command: j.tool.clone(),
        name: None,
        pgid: None,
        log: None,
        started_ms: j.started_ms,
        state: JobState::Running,
        exit: None,
        settled_ms: None,
        engine: String::new(),
    };
    j.journal.job(row.clone()).await;
    j.rt.publish(JobEvent::Started {
        session: j.session,
        id: j.id.clone(),
        command: j.tool.clone(),
        call_id: j.call_id,
        log: PathBuf::new(),
        name: None,
        pty: false,
        kind: JobKind::Promoted,
    })
    .await;
    let (exit, output, origin) = tokio::select! {
        biased;
        origin = j.cancel.notified() => {
            // The token tells a body that listens; dropping `work` aborts
            // the body's task either way (`registry::invoke`).
            j.token.cancel();
            (JobExit::Killed, origin.notice().to_owned(), Some(origin))
        }
        r = j.work => match r {
            Ok(out) => (JobExit::Exited(u8::from(out.is_error)), out.content, None),
            Err(e) => (JobExit::Exited(1), format!("{}: {e:#}", j.tool), None),
        }
    };
    let output = truncate(crate::secrets::redact(&output).into_owned());
    let secs = j.started.elapsed().as_secs();
    // The model that called `cancel_job` owes itself nothing (14 §1.2 as-built).
    let inline = origin == Some(CancelOrigin::Tool);
    let event = JobEvent::Finished {
        session: j.session,
        id: j.id.clone(),
        command: j.tool,
        exit: exit.clone(),
        secs,
        output,
        inline,
        pty: false,
        kind: JobKind::Promoted,
    };
    row.state = JobState::Settled;
    row.exit = Some(exit.clone());
    row.settled_ms = Some(chrono::Utc::now().timestamp_millis());
    j.journal.job(row).await;
    let _ =
        j.me.tell(Finished {
            op: j.activity.id(),
            row: Some((j.session, j.id.clone(), exit, secs)),
            result: (!inline).then(|| event.clone()),
        })
        .await;
    j.rt.publish(event).await;
}

impl Message<ListJobs> for Jobs {
    type Reply = Vec<JobView>;

    async fn handle(&mut self, m: ListJobs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 171 §12.1 inv. 3: kernel jobs are the user's, not the model's.
        self.sessions
            .get(&m.0)
            .map(|s| {
                s.jobs
                    .iter()
                    .filter(|j| !is_cell(j.name.as_deref()))
                    .map(|j| j.view(m.0))
                    .collect::<Vec<_>>()
            })
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
                // 171F B15: a user-cell kernel is not the session's terminal.
                for j in s
                    .jobs
                    .iter()
                    .filter(|j| j.exit.is_none() && !is_cell(j.name.as_deref()))
                {
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

impl Message<LiveCounts> for Jobs {
    type Reply = Vec<LiveCountsRow>;

    async fn handle(&mut self, _: LiveCounts, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        fn row(
            rows: &mut HashMap<SessionId, LiveCountsRow>,
            session: SessionId,
        ) -> &mut LiveCountsRow {
            rows.entry(session).or_insert_with(|| LiveCountsRow {
                session,
                jobs: Vec::new(),
                ptys: Vec::new(),
                reminders: Vec::new(),
            })
        }
        let mut rows: HashMap<SessionId, LiveCountsRow> = HashMap::new();
        for (session, s) in &self.sessions {
            for j in s
                .jobs
                .iter()
                .filter(|j| j.exit.is_none() && !is_cell(j.name.as_deref()))
            {
                let r = row(&mut rows, *session);
                if j.on_pty {
                    r.ptys.push(j.id.clone());
                } else {
                    r.jobs.push(j.id.clone());
                }
            }
        }
        for (session, sr) in &self.reminders {
            for r in &sr.pending {
                row(&mut rows, *session).reminders.push(r.id.clone());
            }
        }
        rows.into_values().collect()
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
        // 171 §9.7: only the session's closing may kill a kernel.
        if m.origin != CancelOrigin::Shutdown && is_cell(job.name.as_deref()) {
            return Err(CELL_REFUSED.into());
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
            job.ended_ms = Some(chrono::Utc::now().timestamp_millis());
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
        self.kill_lost(None).await;
    }
}

impl Message<KillSession> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: KillSession, _: &mut Context<Self, Self::Reply>) {
        self.kill_lost(Some(m.session)).await;
    }
}

impl Jobs {
    /// [`KillAll`] (`None`) / [`KillSession`] (`Some`): 109 §5.5 — the
    /// truth goes to disk before the signal: a crash during the kill still
    /// leaves `exit = lost` on every running row, and the reaper (§5.4) /
    /// the taker's `settle_transferred` (139 §7.1) settles and reports it.
    /// The row stays `running` so `jobs_open` finds it: the store has no
    /// "reported" mark, and a `lost` state would be final.
    async fn kill_lost(&mut self, only: Option<SessionId>) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut rows = Vec::new();
        let mut screens = Vec::new();
        for (session, sj) in &self.sessions {
            if only.is_some_and(|s| s != *session) {
                continue;
            }
            for job in sj.jobs.iter().filter(|j| j.exit.is_none()) {
                rows.push(job.row(*session, JobState::Running, Some(JobExit::Lost), Some(now)));
                if let Some(pty) = &job.pty {
                    screens.push((Arc::clone(pty), *session, job.id.clone()));
                }
            }
        }
        for row in rows {
            self.journal.job(row).await;
        }
        for (pty, session, id) in screens {
            pty.snapshot_to_store(&*self.journal, session, &id).await;
        }
        for (session, sj) in &mut self.sessions {
            if only.is_some_and(|s| s != *session) {
                continue;
            }
            for job in &mut sj.jobs {
                if job.exit.is_none() {
                    kill(&job.pgids);
                    if only.is_some() {
                        // 139 §7.1: a transfer kill is a known outcome — the
                        // row is `lost` on disk and the taker reports it to
                        // the model — so the op retires as `Finished` (no
                        // "outcome unknown" card on the old owner). This
                        // actor lives on: list the job `lost` here.
                        self.live.remove(&job.op);
                        job.exit = Some(JobExit::Lost);
                        job.secs = job.started.elapsed().as_secs();
                        job.ended_ms = Some(chrono::Utc::now().timestamp_millis());
                    } else if let Some(a) = self.live.get(&job.op) {
                        // Aborting the task skips its settlement, and an escapee may
                        // outlive the kill: say so before the guard can drop as `Finished`.
                        a.unknown(format!("{}: killed at shutdown before it settled", job.id));
                    }
                    job.task.abort();
                    // The aborted task never reaches its own close (D3): the
                    // log file goes here; what an escapee writes past the
                    // kill is lost to the log, as it is to the row.
                    lock(&job.log).close();
                }
                // Nothing observes a terminal past shutdown / a transfer:
                // release every charge, and the pty's descriptors with it
                // (a `Live` in flight may still hold the `Arc`).
                if let Some(pty) = job.pty.take() {
                    pty.release();
                    self.pty_bytes = self.pty_bytes.saturating_sub(crate::pty::RESERVE);
                }
            }
        }
    }
}

impl Message<Reaped> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: Reaped, _: &mut Context<Self, Self::Reply>) {
        for LostJob {
            row,
            screen,
            tail,
            deliver,
            reason,
        } in m.jobs
        {
            let session = row.session;
            let sj = self.sessions.entry(session).or_default();
            if sj.jobs.iter().any(|j| j.id == row.id) {
                continue;
            }
            // Later jobs of the session number past the lost one, so `j7`
            // keeps naming the lost job for the rest of this engine's life.
            if let Some(n) = row.id.0.strip_prefix('j').and_then(|n| n.parse().ok()) {
                sj.next = sj.next.max(n);
            }
            let secs = row.settled_ms.map_or(0, |s| {
                u64::try_from((s - row.started_ms) / 1000).unwrap_or(0)
            });
            let pty = row.kind == JobKind::Pty;
            let event = JobEvent::Finished {
                session,
                id: row.id.clone(),
                command: row.command.clone(),
                exit: JobExit::Lost,
                secs,
                output: deliver.clone().unwrap_or_default(),
                // The resumed batch reports a job whose call never returned
                // its stub (§4.3): nothing to inject for it.
                inline: deliver.is_none(),
                pty,
                kind: row.kind,
            };
            self.push(
                session,
                Job {
                    id: row.id.clone(),
                    // Nothing is live for it: never in `live`.
                    op: OperationId(0),
                    command: row.command,
                    call_id: row.call_id,
                    started: Instant::now(),
                    started_ms: row.started_ms,
                    ended_ms: row.settled_ms,
                    exit: Some(JobExit::Lost),
                    secs,
                    log_path: row.log.unwrap_or_default(),
                    log: Arc::new(Mutex::new(Log::from_tail(tail.as_bytes()))),
                    pgids: Arc::default(),
                    cancel: Arc::default(),
                    task: tokio::spawn(async {}).abort_handle(),
                    name: row.name,
                    on_pty: pty,
                    pty: None,
                    lost: Some(Lost {
                        restarted_at: m.restarted_at,
                        reason,
                        screen,
                    }),
                    kind: row.kind,
                },
            );
            if deliver.is_some() {
                // The model had this job's stub: its `<job_result>` travels
                // the one delivery path, a settled operation with a result
                // (47 §4.1). Admission asks the supervisor, so off the mailbox.
                let sink = self.activity.clone();
                let (id, result) = (row.id, event.clone());
                tokio::spawn(async move {
                    match admit_job(None, sink.as_ref(), session, Kind::Job(id.clone())).await {
                        Ok(a) => a.set_result(result),
                        Err(e) => tracing::warn!("lost job {id} not reported: {e}"),
                    }
                });
            }
            self.rt.publish(event).await;
        }
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

impl Message<crate::InstallJournal> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: crate::InstallJournal, _: &mut Context<Self, Self::Reply>) {
        self.journal = m.0;
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
    pgid_tx: tokio::sync::mpsc::UnboundedSender<u32>,
) -> anyhow::Result<(
    Option<std::fs::File>,
    ExecutionParameters,
    std::io::PipeReader,
    std::io::PipeReader,
)> {
    let file = match log_path {
        Some(p) => {
            std::fs::create_dir_all(dir)?;
            // 171F3 R2: never truncate an existing log (the id skips them).
            Some(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(p)?,
            )
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
                None => ProcessTree::adopt(pgid as u32).map(|t| {
                    // 109 §5.1: the first group is the row's `pgid`.
                    if trees.is_empty() {
                        let _ = pgid_tx.send(pgid as u32);
                    }
                    trees.push(t);
                }),
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
    /// The first process group as it spawns (109 §5.1): the row's `pgid`.
    pgid_rx: tokio::sync::mpsc::UnboundedReceiver<u32>,
    cancel: Arc<Cancel>,
    claimable: Arc<AtomicBool>,
    /// [`Start::quiet`]: publish nothing, take no row.
    quiet: bool,
    /// 109 §5.1: where the `jobs` row goes; a quiet job writes none.
    journal: Arc<dyn crate::Journal>,
    /// Spawn time, unix ms (the row's `started_ms`).
    started_ms: i64,
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
/// Either way the reader signs off the log: the second one to end closes
/// the log file (D1), whether or not anything still awaits the task.
fn drain(mut r: std::io::PipeReader, log: Arc<Mutex<Log>>) -> Drain {
    let head: Arc<Mutex<Vec<u8>>> = Arc::default();
    lock(&log).writer();
    let task = tokio::task::spawn_blocking({
        let head = head.clone();
        move || {
            let mut buf = [0u8; 4096];
            let ended = loop {
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
            };
            lock(&log).writer_done();
            ended
        }
    });
    Drain { head, task }
}

async fn run(mut j: Run) {
    // 109 §5.1: the row exists from the spawn on; `pgid` follows with the
    // first group, the exit settles it. A quiet job has no row.
    let mut row = (!j.quiet).then(|| JobRow {
        session: j.session,
        id: j.id.clone(),
        kind: JobKind::Shell,
        call_id: j.call_id.clone(),
        command: j.shown.clone(),
        name: None,
        pgid: None,
        log: Some(j.log_path.clone()),
        started_ms: j.started_ms,
        state: JobState::Running,
        exit: None,
        settled_ms: None,
        engine: String::new(),
    });
    if let Some(r) = &row {
        j.journal.job(r.clone()).await;
        j.rt.publish(JobEvent::Started {
            session: j.session,
            id: j.id.clone(),
            command: j.shown.clone(),
            call_id: j.call_id,
            log: j.log_path,
            name: None,
            pty: false,
            kind: JobKind::Shell,
        })
        .await;
    }
    let mut pgid_pending = row.is_some();

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
                got = j.pgid_rx.recv(), if pgid_pending => {
                    pgid_pending = false;
                    if let (Some(pgid), Some(r)) = (got, row.as_mut()) {
                        r.pgid = Some(i64::from(pgid));
                        j.journal.job(r.clone()).await;
                    }
                }
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
        (JobExit::Lost, None) => {
            // 109: never reached by a running job (see `JobExit::Lost`);
            // kept total so the match stays exhaustive.
            let body = truncate(body);
            let out = ToolOutput::err(format!("{body}{}lost", sep(&body)));
            (body, out)
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
        kind: JobKind::Shell,
    });
    if !j.quiet {
        flush(&j.rt, j.session, &j.id, &j.log).await;
    }
    // The row settles before anyone can act on the result or the event.
    if let Some(mut r) = row {
        if r.pgid.is_none() {
            r.pgid = lock(&j.pgids).first().map(|t| i64::from(t.id()));
        }
        r.state = JobState::Settled;
        r.exit = Some(exit.clone());
        r.settled_ms = Some(chrono::Utc::now().timestamp_millis());
        j.journal.job(r).await;
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
        let cut = s.floor_char_boundary(MAX_OUTPUT);
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

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

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

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

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
        /// Every [`crate::ReminderEvent`] published on this fixture's bus.
        pub(crate) reminder_events: Arc<Mutex<Vec<crate::ReminderEvent>>>,
        /// Removes `dir` on drop; last field so it drops last.
        _guard: crate::TestDir,
    }

    #[derive(kameo::Actor)]
    struct Recorder(Arc<Mutex<Vec<JobEvent>>>);

    impl Message<JobEvent> for Recorder {
        type Reply = ();
        async fn handle(&mut self, e: JobEvent, _: &mut Context<Self, Self::Reply>) {
            lock(&self.0).push(e);
        }
    }

    #[derive(kameo::Actor)]
    struct ReminderRecorder(Arc<Mutex<Vec<crate::ReminderEvent>>>);

    impl Message<crate::ReminderEvent> for ReminderRecorder {
        type Reply = ();
        async fn handle(&mut self, e: crate::ReminderEvent, _: &mut Context<Self, Self::Reply>) {
            lock(&self.0).push(e);
        }
    }

    pub(crate) async fn fixture(grace: Duration, timeout: Duration) -> Fx {
        let guard = crate::TestDir::new("jobs");
        let dir = guard.0.clone();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let events = Arc::new(Mutex::new(Vec::new()));
        let rec = Recorder::spawn_with_mailbox(
            Recorder(Arc::clone(&events)),
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Recorder, JobEvent>(&rec).await;
        let reminder_events = Arc::new(Mutex::new(Vec::new()));
        let rrec = ReminderRecorder::spawn_with_mailbox(
            ReminderRecorder(Arc::clone(&reminder_events)),
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<ReminderRecorder, crate::ReminderEvent>(&rrec)
            .await;
        let jobs =
            Jobs::spawn_with_mailbox(Jobs::new(rt, grace, timeout), runtime::subscriber_mailbox());
        Fx {
            jobs,
            session: SessionId::new(),
            dir,
            events,
            reminder_events,
            _guard: guard,
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

    /// 145 T1 (the jobs half): an adopted body runs as `jN` — `Started
    /// { kind: Promoted }` naming the tool, then `Finished` carrying the
    /// tool's output, `is_error` → exit 1, never inline.
    #[tokio::test]
    async fn adopted_call_runs_as_a_promoted_job() {
        let fx = fixture(Duration::from_secs(1), Duration::from_secs(5)).await;
        let id = fx
            .jobs
            .ask(Adopt {
                session: fx.session,
                call_id: "c1".into(),
                tool: "search_web".into(),
                cancel: CancellationToken::new(),
                work: Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(ToolOutput::err("nothing found"))
                }),
            })
            .await
            .unwrap();
        assert_eq!(id.0, "j1");
        let (exit, output, inline) = finished(&fx).await;
        assert_eq!(
            (exit, output.as_str(), inline),
            (JobExit::Exited(1), "nothing found", false)
        );
        let events = lock(&fx.events).clone();
        assert!(
            matches!(
                &events[0],
                JobEvent::Started { kind: JobKind::Promoted, command, call_id, pty: false, .. }
                    if command == "search_web" && call_id == "c1"
            ),
            "{events:?}"
        );
        assert!(matches!(
            &events[1],
            JobEvent::Finished {
                kind: JobKind::Promoted,
                ..
            }
        ));
        let view = &fx.jobs.ask(ListJobs(fx.session)).await.unwrap()[0];
        assert_eq!(
            (view.command.as_str(), view.state.as_str(), view.pty),
            ("search_web", "exited 1", false)
        );
    }

    /// 145 T3: `cancel_job` on a promoted job cancels its token, drops the
    /// body — it never completes — and finishes `killed` with the origin's
    /// notice; a UI kill is injected, like any job's.
    #[tokio::test]
    async fn cancel_job_cancels_a_promoted_job() {
        struct OnDrop(Arc<AtomicBool>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, SeqCst);
            }
        }
        let fx = fixture(Duration::from_secs(1), Duration::from_secs(5)).await;
        let token = CancellationToken::new();
        let completed = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let work = {
            let (completed, dropped) = (completed.clone(), dropped.clone());
            Box::pin(async move {
                let _alive = OnDrop(dropped);
                std::future::pending::<()>().await;
                completed.store(true, SeqCst);
                Ok(ToolOutput::ok("never"))
            })
        };
        let id = fx
            .jobs
            .ask(Adopt {
                session: fx.session,
                call_id: "c2".into(),
                tool: "slow".into(),
                cancel: token.clone(),
                work,
            })
            .await
            .unwrap();
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: id.clone(),
                origin: CancelOrigin::Ui,
            })
            .await
            .unwrap();
        let (exit, output, inline) = finished(&fx).await;
        assert_eq!(
            (exit, output.as_str(), inline),
            (JobExit::Killed, CancelOrigin::Ui.notice(), false)
        );
        assert!(token.is_cancelled(), "the body's token fired");
        assert!(dropped.load(SeqCst) && !completed.load(SeqCst));
        assert_eq!(
            fx.jobs
                .ask(CancelJob {
                    session: fx.session,
                    id,
                    origin: CancelOrigin::Ui,
                })
                .await
                .unwrap_err()
                .to_string(),
            "j1 already finished"
        );
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
            kind: JobKind::Shell,
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

    /// 171F3 R2: a new jobs actor over the same root and session (an engine
    /// restart) numbers past every log already there — `j<n>.log` and a
    /// kernel's `j<n>.cell.log` alike — skips an id whose log appears later,
    /// and never rewrites an earlier log.
    #[tokio::test]
    async fn ids_are_never_reused_across_a_restart() {
        let mut fx = fixture(Duration::ZERO, Duration::from_secs(60)).await;
        // Each job outlives the zero grace, so the reply is the `Running` stub with its id.
        let StartReply::Running { id, log, .. } = spawn_job(&fx, "sleep 0.3; echo first").await
        else {
            panic!("expected Running");
        };
        assert_eq!(id.0, "j1");
        assert_eq!(settled(&fx, Duration::from_secs(5)).await.state, "exited 0");
        let kernel = log.with_file_name("j3.cell.log");
        std::fs::write(&kernel, "kernel output\n").unwrap();

        let rt = Runtime::new(figment::Figment::new(), fx.dir.clone());
        fx.jobs = Jobs::spawn_with_mailbox(
            Jobs::new(rt, Duration::ZERO, Duration::from_secs(60)),
            runtime::subscriber_mailbox(),
        );
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 0.3; echo second").await else {
            panic!("expected Running");
        };
        assert_eq!(id.0, "j4");
        let stray = log.with_file_name("j5.log");
        std::fs::write(&stray, "not mine\n").unwrap();
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 0.3; echo third").await else {
            panic!("expected Running");
        };
        assert_eq!(id.0, "j6");
        assert!(std::fs::read_to_string(&log).unwrap().contains("first"));
        assert_eq!(std::fs::read_to_string(&kernel).unwrap(), "kernel output\n");
        assert_eq!(std::fs::read_to_string(&stray).unwrap(), "not mine\n");
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

    /// LIVE-GLYPHS A3: a running job and a pending reminder make one row;
    /// once the job has exited and nothing else is live, no row.
    #[tokio::test]
    async fn live_counts_rows_running_jobs_and_pending_reminders() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 30").await else {
            panic!("expected Running");
        };
        let r = fx
            .jobs
            .ask(crate::SetReminder {
                session: fx.session,
                message: "x".into(),
                seconds: 600,
                activity: None,
            })
            .await
            .unwrap();
        let rows = fx.jobs.ask(LiveCounts).await.unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].session, fx.session);
        assert_eq!(rows[0].jobs, std::slice::from_ref(&id));
        assert!(rows[0].ptys.is_empty());
        assert_eq!(rows[0].reminders, std::slice::from_ref(&r.id));
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id,
                origin: CancelOrigin::Ui,
            })
            .await
            .unwrap();
        assert_eq!(settled(&fx, Duration::from_secs(1)).await.state, "killed");
        fx.jobs
            .ask(crate::CancelReminder {
                session: fx.session,
                id: r.id,
            })
            .await
            .unwrap();
        assert!(fx.jobs.ask(LiveCounts).await.unwrap().is_empty());
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
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
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
    /// 109: a journal that records every row, noting for each `jobs` row
    /// whether its process group answered signal 0 at write time.
    #[derive(Default)]
    pub(crate) struct RecJournal {
        pub(crate) jobs: Mutex<Vec<(JobRow, bool)>>,
        pub(crate) screens: Mutex<Vec<PtyScreenRow>>,
        pub(crate) waits: Mutex<Vec<Option<crate::journal::WaitRow>>>,
    }

    impl crate::Journal for RecJournal {
        fn call_started(&self, _: SessionId, _: &str) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn call_finished(
            &self,
            _: SessionId,
            _: &str,
            _: &ToolOutput,
        ) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn question(&self, _: crate::journal::QuestionRow) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn wait(
            &self,
            w: Option<crate::journal::WaitRow>,
            _: SessionId,
            _: &str,
        ) -> futures::future::BoxFuture<'_, ()> {
            lock(&self.waits).push(w);
            Box::pin(async {})
        }
        fn job(&self, j: JobRow) -> futures::future::BoxFuture<'_, ()> {
            let alive = j
                .pgid
                .and_then(|p| ProcessTree::adopt(p as u32).ok())
                .is_some_and(|t| t.alive());
            lock(&self.jobs).push((j, alive));
            Box::pin(async {})
        }
        fn pty_screen(&self, s: PtyScreenRow) -> futures::future::BoxFuture<'_, ()> {
            lock(&self.screens).push(s);
            Box::pin(async {})
        }
    }

    /// A fixture with a [`RecJournal`] installed.
    pub(crate) async fn journaled(grace: Duration, timeout: Duration) -> (Fx, Arc<RecJournal>) {
        let fx = fixture(grace, timeout).await;
        let journal = Arc::new(RecJournal::default());
        fx.jobs
            .ask(crate::InstallJournal(journal.clone()))
            .await
            .unwrap();
        (fx, journal)
    }

    /// Poll the journal until `pred` holds on the `jobs` rows.
    pub(crate) async fn rows_until(
        journal: &RecJournal,
        within: Duration,
        pred: impl Fn(&[(JobRow, bool)]) -> bool,
    ) -> Vec<(JobRow, bool)> {
        let deadline = Instant::now() + within;
        loop {
            let v: Vec<_> = lock(&journal.jobs).clone();
            if pred(&v) {
                return v;
            }
            assert!(Instant::now() < deadline, "journal never satisfied: {v:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 109 §5.1: a shell job writes `running` (then with its pgid), and
    /// `settled` with its exit; a quiet job writes nothing.
    #[tokio::test]
    async fn job_rows_follow_the_job() {
        let (fx, journal) = journaled(Duration::from_secs(3), Duration::from_secs(60)).await;
        let StartReply::Done(out) = spawn_job(&fx, "echo hi; exit 3").await else {
            panic!("expected Done");
        };
        assert!(out.is_error);
        let rows = rows_until(&journal, Duration::from_secs(2), |r| {
            r.iter().any(|(j, _)| j.state == JobState::Settled)
        })
        .await;
        let first = &rows[0].0;
        assert_eq!((first.state, first.exit.clone()), (JobState::Running, None));
        assert_eq!(first.kind, JobKind::Shell);
        assert_eq!(first.id.0, "j1");
        assert!(first.log.as_ref().unwrap().ends_with("j1.log"));
        let last = &rows.last().unwrap().0;
        assert_eq!(last.state, JobState::Settled);
        assert_eq!(last.exit, Some(JobExit::Exited(3)));
        assert!(last.settled_ms.unwrap() >= last.started_ms);
        // A quiet job: no row.
        let before = lock(&journal.jobs).len();
        let _ = spawn_quiet(&fx, "true", true).await;
        assert_eq!(lock(&journal.jobs).len(), before);
    }

    /// 109 §5.5: every running row is written `exit = lost` while its
    /// group is still alive, and only then is the group killed.
    #[tokio::test]
    async fn killall_writes_lost_before_kill() {
        let (fx, journal) = journaled(Duration::ZERO, Duration::from_secs(600)).await;
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 600").await else {
            panic!("expected Running");
        };
        let rows = rows_until(&journal, Duration::from_secs(5), |r| {
            r.iter().any(|(j, _)| j.pgid.is_some())
        })
        .await;
        let pgid = rows.last().unwrap().0.pgid.unwrap() as u32;
        let tree = ProcessTree::adopt(pgid).unwrap();
        assert!(tree.alive());
        fx.jobs.tell(KillAll).await.unwrap();
        let rows = rows_until(&journal, Duration::from_secs(5), |r| {
            r.iter().any(|(j, _)| j.exit == Some(JobExit::Lost))
        })
        .await;
        let (lost, alive_at_write) = rows
            .iter()
            .find(|(j, _)| j.exit == Some(JobExit::Lost))
            .unwrap();
        assert_eq!(lost.id, id);
        assert_eq!(lost.pgid, Some(i64::from(pgid)));
        assert!(alive_at_write, "the row went to disk after the kill");
        assert!(lost.settled_ms.is_some());
        // Still `running` for `jobs_open`: the reaper settles and reports it.
        assert_eq!(lost.state, JobState::Running);
        assert!(group_gone(&tree, Some(tokio::time::Instant::now() + KILL_WAIT)).await);
    }

    /// 139 §7.1: `KillSession` is `KillAll` for one session — its running
    /// row goes to disk `exit = lost` while the group is alive, then the
    /// group dies and the job lists `lost` here; another session's job is
    /// untouched.
    #[tokio::test]
    async fn kill_session_writes_lost_before_kill_and_spares_others() {
        let (fx, journal) = journaled(Duration::ZERO, Duration::from_secs(600)).await;
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 600").await else {
            panic!("expected Running");
        };
        let other = SessionId::new();
        let StartReply::Running { .. } = fx
            .jobs
            .ask(Start {
                session: other,
                command: "sleep 600".into(),
                call_id: String::new(),
                quiet: false,
                shell: ShellTool::build_shell(false).await.unwrap(),
                put_back: Arc::new(|_| {}),
                root: fx.dir.clone(),
                activity: None,
                timeout: None,
            })
            .await
            .unwrap()
        else {
            panic!("expected Running");
        };
        let rows = rows_until(&journal, Duration::from_secs(5), |r| {
            r.iter().filter(|(j, _)| j.pgid.is_some()).count() == 2
        })
        .await;
        let pgid_of = |s: SessionId| {
            rows.iter()
                .find(|(j, _)| j.session == s && j.pgid.is_some())
                .unwrap()
                .0
                .pgid
                .unwrap() as u32
        };
        let tree = ProcessTree::adopt(pgid_of(fx.session)).unwrap();
        let other_tree = ProcessTree::adopt(pgid_of(other)).unwrap();
        assert!(tree.alive() && other_tree.alive());
        fx.jobs
            .ask(KillSession {
                session: fx.session,
            })
            .await
            .unwrap();
        let rows = lock(&journal.jobs).clone();
        let lost: Vec<_> = rows
            .iter()
            .filter(|(j, _)| j.exit == Some(JobExit::Lost))
            .collect();
        assert_eq!(lost.len(), 1, "{rows:?}");
        let (lost, alive_at_write) = lost[0];
        assert_eq!((lost.session, &lost.id), (fx.session, &id));
        assert!(alive_at_write, "the row went to disk after the kill");
        assert_eq!(lost.state, JobState::Running);
        assert!(group_gone(&tree, Some(tokio::time::Instant::now() + KILL_WAIT)).await);
        assert!(
            other_tree.alive(),
            "the other session's group was signalled"
        );
        let view = list(&fx).await.pop().unwrap();
        assert_eq!((view.id, view.state.as_str()), (id, "lost"));
        assert_eq!(
            fx.jobs.ask(ListJobs(other)).await.unwrap()[0].state,
            "running"
        );
        // Awaited, and the group's death observed: a live `sleep 600` would
        // keep the runtime's pipe reader alive for the full ten minutes.
        fx.jobs.ask(KillAll).await.unwrap();
        assert!(group_gone(&other_tree, Some(tokio::time::Instant::now() + KILL_WAIT)).await);
    }

    /// 109 §4.4: the `shell` grace wait is a `job` wait while it lasts.
    #[tokio::test]
    async fn grace_wait_writes_and_clears_a_job_wait() {
        let (fx, journal) = journaled(Duration::from_millis(200), Duration::from_secs(60)).await;
        let reply = fx
            .jobs
            .ask(Start {
                session: fx.session,
                command: "sleep 1".into(),
                call_id: "c1".into(),
                quiet: false,
                shell: ShellTool::build_shell(false).await.unwrap(),
                put_back: Arc::new(|_| {}),
                root: fx.dir.clone(),
                activity: None,
                timeout: None,
            })
            .await
            .unwrap();
        assert!(matches!(reply, StartReply::Running { .. }));
        let waits = lock(&journal.waits).clone();
        assert_eq!(waits.len(), 2, "{waits:?}");
        let w = waits[0].as_ref().unwrap();
        assert_eq!(
            (w.kind, w.target.as_str(), w.tool_call_id.as_str()),
            (crate::journal::WaitKind::Job, "j1", "c1")
        );
        assert!(waits[1].is_none());
    }

    fn lost_row(session: SessionId, id: &str, kind: JobKind, log: &std::path::Path) -> JobRow {
        JobRow {
            session,
            id: JobId(id.into()),
            kind,
            call_id: "c0".into(),
            command: "cargo build".into(),
            name: None,
            pgid: None,
            log: Some(log.to_path_buf()),
            started_ms: 1_000,
            state: JobState::Lost,
            exit: Some(JobExit::Lost),
            settled_ms: Some(8_000),
            engine: String::new(),
        }
    }

    /// 109 §5.4: a reaped job lists as `lost` with its log tail, later jobs
    /// number past it, and — when the model had its stub — its
    /// `<job_result exit="lost">` settles through the ledger.
    #[tokio::test]
    async fn reaped_jobs_list_lost_and_deliver_through_the_ledger() {
        use crate::activity::test_sink::Seen;
        let (fx, ledger, seen) = certified(Duration::ZERO, Duration::from_secs(60)).await;
        let log = fx.dir.join("j7.log");
        std::fs::write(&log, "building…\nerror: boom\n").unwrap();
        fx.jobs
            .tell(Reaped {
                restarted_at: chrono::Utc::now(),
                jobs: vec![
                    LostJob {
                        row: lost_row(fx.session, "j7", JobKind::Shell, &log),
                        screen: None,
                        tail: "error: boom\n".into(),
                        deliver: Some("this job died with the engine".into()),
                        reason: LostReason::Restart,
                    },
                    LostJob {
                        row: lost_row(fx.session, "j3", JobKind::Shell, &log),
                        screen: None,
                        tail: String::new(),
                        deliver: None,
                        reason: LostReason::Restart,
                    },
                ],
            })
            .await
            .unwrap();
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| {
            v.iter().any(|s| matches!(s, Seen::Result(_)))
        })
        .await;
        // One delivery: j7's. j3 is the resumed batch's to report.
        assert_eq!(v.iter().filter(|s| matches!(s, Seen::Result(_))).count(), 1);
        let views = list(&fx).await;
        let j7 = views.iter().find(|v| v.id.0 == "j7").unwrap();
        assert_eq!(
            (j7.state.as_str(), j7.secs, j7.tail.as_str()),
            ("lost", 7, "error: boom\n")
        );
        assert_eq!(j7.log, log);
        assert!(table(&views).contains("j7  lost"), "{}", table(&views));
        let finished: Vec<_> = lock(&fx.events)
            .iter()
            .filter_map(|e| match e {
                JobEvent::Finished {
                    id, exit, inline, ..
                } => Some((id.0.clone(), exit.clone(), *inline)),
                _ => None,
            })
            .collect();
        assert!(
            finished.contains(&("j7".into(), JobExit::Lost, false)),
            "{finished:?}"
        );
        assert!(
            finished.contains(&("j3".into(), JobExit::Lost, true)),
            "{finished:?}"
        );
        // The next job of the session numbers past the lost ones.
        let StartReply::Running { id, .. } = spawn_job(&fx, "sleep 5").await else {
            panic!("expected Running");
        };
        assert_eq!(id.0, "j8");
    }

    // ---- 171 §5.5 mark scan ----

    fn mark(kind: MarkKind, exit: Option<i32>, cwd: Option<&str>, offset: u64) -> Mark {
        Mark {
            kind,
            exit,
            cwd: cwd.map(str::to_owned),
            offset,
        }
    }

    #[test]
    fn cell_mark_scan_every_split() {
        let bytes = b"out\x1b]133;C\x07hello\x1b]133;D;3\x07\x1b]7;file://host/tmp/a%20b\x07";
        let want = [
            mark(MarkKind::Start, None, None, 3),
            mark(MarkKind::End, Some(3), None, 16),
            mark(MarkKind::Cwd, None, Some("/tmp/a b"), 26),
        ];
        for split in 0..=bytes.len() {
            let mut carry = Vec::new();
            let mut got = scan_marks(&mut carry, 0, &bytes[..split]);
            got.extend(scan_marks(&mut carry, split as u64, &bytes[split..]));
            assert_eq!(got, want, "split at {split}");
            assert!(carry.is_empty(), "split at {split}: {carry:?}");
        }
    }

    #[test]
    fn cell_mark_scan_bel_and_st() {
        let mut carry = Vec::new();
        let got = scan_marks(
            &mut carry,
            100,
            b"\x1b]133;A\x1b\\\x1b]133;D;-1\x1b\\\x1b]7;file://h/x\x1b\\\x1b]0;title\x07\x1b]133;B\x07\x1b[31m",
        );
        assert_eq!(
            got,
            [
                mark(MarkKind::Ready, None, None, 100),
                mark(MarkKind::End, Some(-1), None, 109),
                mark(MarkKind::Cwd, None, Some("/x"), 121),
            ]
        );
        assert!(carry.is_empty());
        // A strict prefix at the end waits; an over-long one is dropped.
        assert!(scan_marks(&mut carry, 0, b"x\x1b]133;D;").is_empty());
        assert_eq!(carry, b"\x1b]133;D;");
        let long = [b"\x1b]7;file://h/".as_slice(), &[b'p'; 2000]].concat();
        assert!(scan_marks(&mut carry, 8, &long).is_empty());
        assert!(carry.is_empty());
    }

    /// Offsets index the log as written: the scan runs over the redacted
    /// bytes, so the placeholder's length, not the value's, precedes the mark.
    /// The vault hook is a process-wide `OnceLock` no test may install, so the
    /// already-redacted chunk stands in for `secrets::redact`'s output here and
    /// `Log::push`'s `written` counter is checked through a real file.
    #[test]
    fn cell_mark_inside_redacted_region() {
        let redacted = b"token={{secret:GH}}\x1b]133;D;0\x07";
        let mut carry = Vec::new();
        let got = scan_marks(&mut carry, 0, redacted);
        assert_eq!(got, [mark(MarkKind::End, Some(0), None, 19)]);

        let guard = crate::TestDir::new("cell-log");
        let path = guard.0.join("j1.log");
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut log = Log::new(Some(std::fs::File::create(&path).unwrap()));
        log.scan_marks_to(tx);
        log.push(b"first line\r\n");
        log.push(b"\x1b]133;");
        log.push(b"C\x07rest");
        drop(log);
        let on_disk = std::fs::read(&path).unwrap();
        let m = rx.try_recv().unwrap();
        assert_eq!(m.kind, MarkKind::Start);
        assert_eq!(&on_disk[m.offset as usize..][..8], b"\x1b]133;C\x07");
        assert!(rx.try_recv().is_err());
    }
}
