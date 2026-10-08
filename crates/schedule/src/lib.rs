//! Schedule plugin (design doc 34): a markdown file in `<data_dir>/schedules/`
//! whose front matter says *when* and whose body is the prompt. One actor
//! ticks, fires each due schedule as a wrapped input into that schedule's own
//! session, and delivers the run's reply to the user through
//! [`agent::Deliver`]. History lives in `<data_dir>/schedule.sqlite`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SCHEDULE_ACTOR`] actor ([`Schedule`]): [`ListSchedules`],
//!   [`Fire`], [`History`], [`SetEnabled`], [`Done`]; watchers (40):
//!   [`TestSchedule`], [`BindSession`]
//! - events: [`ScheduleEvent`]
//!
//! Loops (design doc 33) are schedule files with a stop (`until:` /
//! `max_runs:`); the same actor runs them.

pub mod file;
pub mod store;
pub mod tools;
pub mod validate;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use agent::sensor::{SensorError, SensorRun};
use chrono::{DateTime, Local};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use notify::Watcher;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

/// Registry name of the [`Schedule`] actor.
pub const SCHEDULE_ACTOR: &str = "schedule";

/// Directory under `data_dir` holding the schedule files (34 §2.1).
pub const DIR: &str = "schedules";

/// Sqlite file under `data_dir` holding run history (34 §7).
pub const DB: &str = "schedule.sqlite";

/// `schedule:` config section (34 §9).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleConfig {
    /// `false`: no watcher, no actor, no tool, no prompt block.
    pub enabled: bool,
    /// A run still going after this many seconds is cancelled and reported as `timeout`.
    pub run_timeout_secs: u64,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            run_timeout_secs: 1800,
        }
    }
}

/// `ask` → `Vec<ScheduleInfo>`: every file in the directory, parsed or not.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSchedules;

/// `ask` → `Vec<SensorInfo>`: every `<data_dir>/sensors/*.py`, resolved (108 §9.2).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSensors;

/// `ask` → `Result<Verdict, String>`: the 40 §1.12 dry run on an unsaved file
/// (108 §9.4) — nothing written, bound or reconciled. `Err` with the parse
/// error or `not a watcher`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestFile(pub file::ScheduleFile);

/// One installed sensor as the form's Sensor select and the `routines` skill
/// see it (108 §9.2).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SensorInfo {
    /// File name: `pr_watch.py`.
    pub name: String,
    /// First line of the module docstring.
    pub description: Option<String>,
    /// The docstring's `when:` line.
    pub usage: Option<String>,
    /// `# arg:` lines in header order.
    pub args: Vec<agent::sensor::SensorArg>,
    /// `# theuth-sensor:` version; `None` for a user file.
    pub version: Option<String>,
    /// The name is one of the shipped built-ins.
    pub builtin: bool,
    /// A built-in whose text differs from the shipped and the last-installed copy.
    pub edited: bool,
    /// The binary's version of a built-in (differs from `version` when `edited` blocks an update).
    pub shipped_version: Option<String>,
    /// `# needs: cpython`.
    pub needs_cpython: bool,
    /// A CPython was found on `PATH`.
    pub cpython_available: bool,
    /// `# hosts:`.
    pub hosts: Vec<String>,
    /// The file defines `probe`.
    pub has_probe: bool,
    /// Header error; the other fields are best-effort.
    pub error: Option<String>,
}

/// `ask` → `Result<SessionId, String>`: fire now (`/schedules run`), ignoring `next`.
/// `Err("no such schedule" | "busy" | "disabled" | "<parse error>")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fire {
    /// File stem.
    pub name: String,
}

/// `ask` → `u64`: the live `schedule.run_timeout_secs` — seconds one run may
/// take unless the file says `run_timeout:` (88).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetRunTimeout;

/// `ask` → `Result<(), String>`: set and persist `schedule.run_timeout_secs`
/// (88). Runs fired from then on use it; in-flight runs keep their deadline.
/// `Err("run_timeout: below 30s")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetRunTimeout {
    /// New value, seconds (≥ 30).
    pub seconds: u64,
}

/// `ask` → `Vec<RunInfo>`: the last `limit` fires and skips of `name`, newest first (34 §7).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    /// File stem.
    pub name: String,
    /// Row cap.
    pub limit: u32,
}

/// `ask` → `Result<ScheduleInfo, String>`: rewrite the file's `enabled:` line
/// (33 §3.5; `/schedules pause|resume`, the desktop toggle). The watcher
/// reconciles and publishes [`ScheduleEvent::Changed`]. `Err("no such schedule")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEnabled {
    /// File stem.
    pub name: String,
    /// New value of the `enabled:` line.
    pub enabled: bool,
}

/// `ask` → `Result<ScheduleFile, String>`: a file's front matter and body for
/// the desktop editor. `Err("no such schedule" | "front matter: …")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Get {
    /// File stem.
    pub name: String,
}

/// `ask` → `Result<ScheduleInfo, String>`: validate and write `<dir>/<name>.md`
/// (create or replace), then reconcile so the reply is current. `Err(<name
/// rule> | <parse error> | <io error>)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Put(pub file::ScheduleFile);

/// `ask` → `Result<(), String>`: remove `<dir>/<name>.md`; the history gets a
/// `removed` row like any other deletion. `Err("no such schedule" | <io error>)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delete {
    /// File stem.
    pub name: String,
}

/// `ask` → `Result<String, String>`: the model in a run declares its loop done
/// (33 §3.5, the `schedule_done` tool). `session` is the run's session or one
/// of its descendants. Ends the loop (`done (model): <why>` row, file removed,
/// delivered); replies `"ended <name>"`. `Err("not a schedule run")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Done {
    /// The calling session (`ToolCx.session`).
    pub session: SessionId,
    /// The model's reason; recorded and delivered.
    pub why: String,
}

/// `ask` → `Result<Verdict, String>`: the 40 §1.12 verdict on demand
/// (`/schedules test <name>`, `schedule_status <name>`): dry-run the sensor
/// twice, run `probe`, report. `Err("no such schedule" | "not a watcher" | …)`.
/// Runs through `validate.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestSchedule {
    /// File stem.
    pub name: String,
}

/// `ask` → `Result<SessionId, String>`: create the watcher's session now, before
/// its first fire (40 §1.12: titled after `subject=`, id kept in `watch.session_id`),
/// or return the one already bound. `Err("no such schedule" | "not a watcher" | …)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindSession {
    /// File stem.
    pub name: String,
    /// `subject=` from the creation dry run; the title becomes `<name> · <subject>`.
    #[serde(default)]
    pub subject: Option<String>,
}

/// How one tick of a `when:` file ended (40 §3.1). `Display` is the row's
/// `result` / the reason string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The sensor failed; the text after `check broken: ` (a [`agent::sensor::SensorError`] display).
    Broken(String),
    /// The runtime had no sensor slot within 1 s (40 §5): retried at `every`, no strike.
    Deferred,
    /// `fire: false`.
    Quiet,
    /// `fire: true`, same hash as the last fired payload (`fire: on_change`).
    Unchanged,
    /// `for:` not met yet.
    Holding {
        /// Time since the first true tick.
        held: Duration,
        /// `for:`.
        need: Duration,
    },
    /// A fire is due, `cooldown:` forbids it.
    Cooldown {
        /// Until the cooldown ends.
        left: Duration,
    },
    /// `mode: skip`, run in flight.
    Busy,
    /// `mode: queue`; pending payload replaced.
    Queued {
        /// Fires folded into the pending payload so far (0 = first).
        coalesced: u32,
    },
    /// The turn was started.
    Fired,
    /// `mode: restart`: the in-flight run was cancelled, then fired.
    Restarted,
}

impl std::fmt::Display for CheckOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fmt_d = |d: Duration| humantime::format_duration(Duration::from_secs(d.as_secs()));
        match self {
            Self::Broken(why) => write!(f, "check broken: {why}"),
            Self::Deferred => f.write_str("deferred"),
            Self::Quiet => f.write_str("quiet"),
            Self::Unchanged => f.write_str("unchanged"),
            Self::Holding { held, need } => {
                write!(f, "holding: {} of {}", fmt_d(*held), fmt_d(*need))
            }
            Self::Cooldown { left } => write!(f, "cooldown: {} left", fmt_d(*left)),
            Self::Busy => f.write_str("skipped: busy"),
            Self::Queued { coalesced: 0 } => f.write_str("queued"),
            Self::Queued { coalesced } => write!(f, "queued ({coalesced} coalesced)"),
            Self::Fired => f.write_str("fired"),
            Self::Restarted => f.write_str("restarted"),
        }
    }
}

/// The last sensor check of a watcher (40 §3.1, §6): `ScheduleInfo.last_check`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LastCheck {
    /// When the sensor ran.
    pub at: DateTime<Local>,
    /// The [`CheckOutcome`] string.
    pub outcome: String,
    /// Sensor wall clock.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub elapsed_ms: u64,
    /// `sha256(payload)` when the check produced one.
    pub hash: Option<String>,
}

/// The 40 §1.12 verdict: what `schedule` replies with for a `when:` file,
/// `/schedules test` shows, and the desktop's Test button expands.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Verdict {
    /// Sensor file name.
    pub sensor: String,
    /// Wall clock of the (first) dry run.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub elapsed_ms: u64,
    /// The sensor said `fire`.
    pub fire: bool,
    /// `sha256(payload)` of the dry run.
    pub hash: String,
    /// The two dry runs hashed differently: `payload flaps — include only stable fields`.
    pub flaps: bool,
    /// `probe()`'s note, when the sensor has one and it said something.
    pub probe: Option<String>,
    /// The gate would fire now (fire, and not `unchanged` against the last hash).
    pub would_fire: bool,
    /// The body mutates and the file lacks `approvals: false`.
    pub needs_approvals_off: bool,
    /// The dry run failed; the reason (`SensorError` display). Other fields are then defaults.
    pub error: Option<String>,
}

/// How a fire with `agent:` becomes a task (67 §7.4): the tasks plugin
/// implements this and registers it with [`RegisterTaskCreator`];
/// [`Schedule::fire`] consults it when `Spec.agent` is set.
#[async_trait::async_trait]
pub trait TaskCreator: Send + Sync {
    /// Upsert the task for `(schedule, fire_ms)` owned by `agent` with
    /// `prompt` as its brief, wake the agent (`schedule`), return the task id.
    /// `project` is the file's `project:` key; `cwd` its `cwd:`, from which
    /// the tasks side may infer a project when `project` is `None`.
    async fn create_for_schedule(
        &self,
        schedule: &str,
        fire_ms: u64,
        agent: &str,
        prompt: &str,
        project: Option<&str>,
        cwd: Option<&str>,
    ) -> anyhow::Result<String>;
    /// The schedule's `until` passed (`why` = the end row text): its open
    /// tasks are `done` with reviewer `system`.
    async fn close_for_schedule(&self, schedule: &str, why: &str) -> anyhow::Result<()>;
}

/// `ask` → `()`: the tasks plugin hands over its [`TaskCreator`]. Replaces any earlier one.
pub struct RegisterTaskCreator(pub Arc<dyn TaskCreator>);

/// One schedule as `/schedules` and Settings › Schedules show it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScheduleInfo {
    /// File stem.
    pub name: String,
    /// The front-matter clause as written: `cron 0 9 * * 1-5` / `every 30m` / `at 2026-09-16T15:00`.
    pub when: String,
    /// `croner::Cron::describe()` for cron; the clause itself otherwise.
    pub describe: String,
    /// `enabled:` in the front matter (default true).
    pub enabled: bool,
    /// `quiet:` in the front matter (default false): the reply is not delivered.
    pub quiet: bool,
    /// Next fire; `None` when disabled, broken, or a one-shot in the past.
    pub next: Option<DateTime<Local>>,
    /// Last real fire (skips excluded).
    pub last: Option<DateTime<Local>>,
    /// `result` of the last completed run (34 §7).
    pub last_result: Option<String>,
    /// The schedule's session, once it has fired.
    pub session: Option<SessionId>,
    /// A run is in flight.
    pub running: bool,
    /// Parse error; such a schedule never fires.
    pub error: Option<String>,
    /// `until:` — the stop check (33 §3.1).
    pub until: Option<String>,
    /// `max_runs:` — the cap (33 §3.2).
    pub max_runs: Option<u32>,
    /// Fires since the loop's last end row (33 §3.2); 0 for a plain schedule.
    pub runs: u32,
    /// `approvals:` in the front matter (default true); false = the run never prompts.
    pub approvals: bool,
    /// `cwd:` — where `until` runs and the session starts.
    pub cwd: Option<PathBuf>,
    /// `agent:` — the roster agent whose task each fire creates (67 §7.4).
    pub agent: Option<String>,
    /// `project:` — the project label of that task (67 §7.1).
    pub project: Option<String>,
    /// `when:` as written for a watcher (40 §2): `pr_watch.py repo=o/r pr=482`; `None` for a clock-only file.
    pub when_sensor: Option<String>,
    /// The sensor file alone (`pr_watch.py`); `None` for a clock-only file.
    pub sensor: Option<String>,
    /// `fire:` (`on_change` / `while_true` / `once`); `None` without `when`.
    pub fire: Option<String>,
    /// `for:` as a duration string (`2m`, `0s`); `None` without `when`.
    #[serde(rename = "for")]
    pub for_: Option<String>,
    /// `cooldown:` as a duration string; `None` without `when`.
    pub cooldown: Option<String>,
    /// `mode:` (`skip` / `queue` / `restart`); `None` without `when`.
    pub mode: Option<String>,
    /// `memory:` (`continue` / `clear`); `None` without `when`.
    pub memory: Option<String>,
    /// `expires:` as a duration string (`14d`); `None` when unset.
    pub expires: Option<String>,
    /// `timeout:` — the sensor wall clock as a duration string; `None` without `when`.
    pub timeout: Option<String>,
    /// `model:` as written.
    pub model: Option<String>,
    /// `run_timeout:` as a duration string; `None` = the setting.
    pub run_timeout: Option<String>,
    /// `created:` (40 §2); what `expires` counts from.
    pub created: Option<DateTime<Local>>,
    /// The last sensor check (40 §3.1); `None` before the first or for a clock-only file.
    pub last_check: Option<LastCheck>,
}

/// What a history row is (108 §9.3), computed from the row on read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// A fire: the prompt entered a session.
    Run,
    /// A gate outcome (`quiet`, `unchanged`, `holding…`, `cooldown…`, `deferred`, `queued…`, `check broken: …`).
    Check,
    /// The loop or watcher ended (`done…`, `max_runs N reached`, `until broken…`, `expired`, `removed`, `check broken ×3, disabled`).
    End,
    /// `skipped: …`.
    Skip,
}

impl RunKind {
    /// The kind of a `runs` row from its `session` and `result` (108 §9.3).
    pub fn of(session: bool, result: Option<&str>) -> Self {
        match result {
            Some(r) if r.starts_with("skipped:") => Self::Skip,
            Some(r)
                if r.starts_with("done")
                    || r.starts_with("max_runs ") && r.ends_with(" reached")
                    || r.starts_with("until broken")
                    || r == "expired"
                    || r == "removed"
                    || r.starts_with("check broken ×") =>
            {
                Self::End
            }
            // An `agent:` fire is a task, not a session (`task <id>`, 67 §7.4).
            Some(r) if r.starts_with("task ") => Self::Run,
            _ if session => Self::Run,
            _ => Self::Check,
        }
    }
}

/// One row of `runs` (34 §7).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RunInfo {
    /// The schedule's session at the time; `None` for a skip before the first fire.
    pub session: Option<SessionId>,
    /// When it fired (or was skipped).
    pub fired_at: DateTime<Local>,
    /// When the turn ended; `None` while running.
    pub finished_at: Option<DateTime<Local>>,
    /// `end_turn` / `error: …` / `timeout` / `cancelled` / `skipped: busy` / `lost`;
    /// loop ends (33 §3.3): `done: …` / `done (model): …` / `max_runs N reached` /
    /// `until broken: …` / `removed`; `None` while running.
    pub result: Option<String>,
    /// The run's final text, ≤ 2 KiB (33 §0.7); `None` for skips, ends, `lost`.
    pub report: Option<String>,
    /// The payload's first line on a `when:` fire (40 §1.5); `None` otherwise.
    pub envelope: Option<String>,
    /// What the row is (108 §9.3).
    pub kind: RunKind,
}

/// Bus events (34 §5).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum ScheduleEvent {
    /// A schedule's prompt entered its session.
    Fired {
        /// File stem.
        name: String,
        /// The schedule's session.
        session: SessionId,
        /// Wall-clock time of the fire.
        at: DateTime<Local>,
    },
    /// A due fire did not happen.
    Skipped {
        /// File stem.
        name: String,
        /// Wall-clock time of the skip.
        at: DateTime<Local>,
        /// `busy`.
        reason: String,
    },
    /// The directory was reconciled; `list_schedules` may differ.
    Changed,
}

/// Ceiling on the tick sleep (34 §1.3): the suspended-clock case.
const TICK: Duration = Duration::from_secs(60);

/// Ceiling on an `until` check (33 §3.1); no exit by then ends the loop as broken.
const UNTIL_TIMEOUT: Duration = Duration::from_secs(120);

/// The last 2 KiB of `s`, cut at a char boundary (33 §3.1, §0.7).
fn tail_2k(s: &str) -> &str {
    let mut cut = s.len().saturating_sub(2048);
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    &s[cut..]
}

/// Run `until` through the login shell ([`platform::shell`], `-lc`) in `cwd`,
/// no stdin (33 §3.1). `Ok((exit code, output tail))`; `Err(why)` when it did
/// not exit in [`UNTIL_TIMEOUT`], could not start, or this host has no login shell.
async fn until_check(cmd: &str, cwd: Option<&Path>) -> Result<(Option<i32>, String), String> {
    // ponytail: ShellKind::Brush is 58 §11 step 3
    let Some(mut c) = platform::shell().command(cmd, true) else {
        return Err(
            "until: no login shell on this platform (brush in-process is 58 step 3)".into(),
        );
    };
    c.kill_on_drop(true);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = tokio::time::timeout(UNTIL_TIMEOUT, c.output())
        .await
        .map_err(|_| format!("no exit in {}s", UNTIL_TIMEOUT.as_secs()))?
        .map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), tail_2k(&text).to_owned()))
}

/// `every:` of a watcher (parse guarantees `when` + `every`, 40 §2).
fn every_of(spec: &file::Spec) -> Duration {
    match spec.when {
        file::When::Every(d) => d,
        _ => Duration::from_secs(60),
    }
}

/// `now - t` as a std duration; zero when `t` is ahead.
fn since(now: DateTime<Local>, t: DateTime<Local>) -> Duration {
    (now - t).to_std().unwrap_or_default()
}

/// `t + d` for a std duration.
fn plus(t: DateTime<Local>, d: Duration) -> DateTime<Local> {
    t + chrono::TimeDelta::from_std(d).unwrap_or(chrono::TimeDelta::MAX)
}

/// `SensorRunner::check` found no free slot within 1 s (40 §5).
fn deferred(r: &Result<SensorRun, SensorError>) -> bool {
    matches!(r, Err(SensorError::Budget(m)) if m == "deferred")
}

/// 40 §3.2 as a pure function: the outcome of one check and the watcher's
/// next tick, given the file, its `watch` row, the sensor's answer, the clock
/// and whether its run is in flight. `queued` is the pending payload's
/// `coalesced` count when `mode: queue` already holds one. Mutates only
/// `w.holding_since` (`first_true_at`: set on the first true tick, cleared by
/// a false one). Writes nothing; [`Schedule::check_done`] does.
fn gate(
    spec: &file::Spec,
    w: &mut store::WatchRow,
    r: &Result<SensorRun, SensorError>,
    now: DateTime<Local>,
    busy: bool,
    queued: Option<u32>,
) -> (CheckOutcome, DateTime<Local>) {
    let every = every_of(spec);
    if deferred(r) {
        return (CheckOutcome::Deferred, plus(now, every));
    }
    let r = match r {
        Ok(r) => r,
        Err(e) => return (CheckOutcome::Broken(e.to_string()), plus(now, every)),
    };
    // §1.8: the sensor's `next` clamped to [every, 10 × every].
    let next = plus(
        now,
        r.next
            .map_or(every, Duration::from_secs)
            .clamp(every, every * 10),
    );
    if !r.fire {
        w.holding_since = None;
        return (CheckOutcome::Quiet, next);
    }
    let hash = agent::sensor::payload_hash(&r.payload);
    if spec.fire == file::Fire::OnChange && w.last_hash.as_deref() == Some(hash.as_str()) {
        return (CheckOutcome::Unchanged, next);
    }
    let held = since(now, *w.holding_since.get_or_insert(now));
    if held < spec.for_ {
        return (
            CheckOutcome::Holding {
                held,
                need: spec.for_,
            },
            next,
        );
    }
    if let Some(t) = w.last_fired_at
        && since(now, t) < spec.cooldown
    {
        let end = plus(t, spec.cooldown);
        return (
            CheckOutcome::Cooldown {
                left: since(end, now),
            },
            next.max(end),
        );
    }
    // §3.1: a fire's next is `last + every`; the row is the run row.
    let fired_next = plus(now, every);
    if busy {
        return match spec.mode {
            file::Mode::Skip => (CheckOutcome::Busy, next),
            file::Mode::Queue => (
                CheckOutcome::Queued {
                    coalesced: queued.map_or(0, |c| c + 1),
                },
                next,
            ),
            file::Mode::Restart => (CheckOutcome::Restarted, fired_next),
        };
    }
    (CheckOutcome::Fired, fired_next)
}

/// A run's end reason is a normal stop (40 §1.7). The reason is the
/// provider's raw string (`agent` `TurnFinished.stop_reason`: `end_turn`
/// Anthropic, `stop` OpenAI chat, `completed` OpenAI Responses, …), so the
/// classification is genai's, not a string list of our own.
fn run_succeeded(kind: &str) -> bool {
    matches!(
        genai::chat::StopReason::from(kind.to_owned()),
        genai::chat::StopReason::Completed(_)
    )
}

/// `"`, `<`, `&` escaped for a `<when …="…">` attribute: envelope tokens are
/// untrusted text (40 §7).
fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// What a `when:` fire carries into the run (40 §3.3) and commits when the
/// run ends successfully (§1.7). Also the `mode: queue` pending payload.
#[derive(Clone, Debug)]
struct WhenFire {
    payload: String,
    hash: String,
    /// Sensor state as JSON text.
    state: String,
    coalesced: u32,
}

impl WhenFire {
    fn new(r: &SensorRun, coalesced: u32) -> Self {
        Self {
            payload: r.payload.clone(),
            hash: agent::sensor::payload_hash(&r.payload),
            state: r.state.to_string(),
            coalesced,
        }
    }

    /// The payload's first line (40 §1.5).
    fn envelope(&self) -> &str {
        self.payload.lines().next().unwrap_or_default()
    }

    /// The `<when …>` block of the wrapper (40 §3.3): `subject`, `stage`,
    /// `events` copied from the envelope when present.
    fn block(&self, sensor: &str) -> String {
        let env = agent::sensor::parse_envelope(&self.payload);
        let mut s = format!("<when name=\"{}\"", attr(sensor));
        for k in ["subject", "stage", "events"] {
            if let Some(v) = env.get(k) {
                let _ = write!(s, " {k}=\"{}\"", attr(v));
            }
        }
        let _ = write!(
            s,
            " coalesced=\"{}\">\n{}\n</when>",
            self.coalesced,
            self.payload.trim_end()
        );
        s
    }
}

/// Internal `tell`: a spawned sensor check returned (40 §3.2); the gate runs
/// in [`Schedule::check_done`]. The check itself runs off the mailbox so a
/// 20 s sensor does not stall `ListSchedules`; `sensors.check` holds one of
/// the harness-wide slots, so a tick past `max_concurrent` waits rather
/// than being dropped (§1.9).
struct CheckDone {
    name: String,
    result: Result<SensorRun, SensorError>,
}

/// A sensor check in flight: what [`Schedule::tick`] hands back for the
/// `Tick` handler to spawn (tests await it inline).
type Check = Pin<Box<dyn Future<Output = CheckDone> + Send>>;

/// Test double for the sensor runtime: `(args, state) -> check result`.
#[cfg(test)]
type FakeSensor = Arc<
    dyn Fn(&BTreeMap<String, String>, &serde_json::Value) -> Result<SensorRun, SensorError>
        + Send
        + Sync,
>;

/// Consecutive strikes that disable a watcher (40 §5).
const MAX_STRIKES: u32 = 3;

/// One file in the directory, parsed or not.
struct Entry {
    spec: Result<file::Spec, String>,
    /// File text as last read; a reconcile re-parses only when it differs.
    raw: String,
    next: Option<DateTime<Local>>,
    session: Option<SessionId>,
    last: Option<DateTime<Local>>,
    last_result: Option<String>,
}

impl Entry {
    fn new(raw: String, spec: Result<file::Spec, String>, now: DateTime<Local>) -> Self {
        let mut e = Self {
            spec,
            raw,
            next: None,
            session: None,
            last: None,
            last_result: None,
        };
        e.arm(now);
        e
    }

    /// `next` from `last` as at boot (34 §3): a `missed: run` cron occurrence
    /// in `(last, now]` is due now; otherwise the next occurrence after `now`.
    /// Also the handover path: a non-hub does not tick, so regaining the hub
    /// re-arms every entry through here.
    fn arm(&mut self, now: DateTime<Local>) {
        self.next = next_of(&self.spec, now, self.last);
        if let (Ok(s), Some(last)) = (&self.spec, self.last)
            && s.enabled
            && s.missed_run
            && s.when.missed(last, now)
        {
            self.next = Some(now);
        }
    }
}

/// `next` for an entry: `None` when broken or disabled (34 §3).
fn next_of(
    spec: &Result<file::Spec, String>,
    now: DateTime<Local>,
    last: Option<DateTime<Local>>,
) -> Option<DateTime<Local>> {
    match spec {
        Ok(s) if s.enabled => s.when.next(now, last),
        _ => None,
    }
}

/// A `when:` file as read: a hand-edited one without `created:` gains the
/// line on first load (40 §2; `expires` counts from it) and is re-read from
/// what was written. On a failed write the text stands and `created_of`
/// falls back to the mtime.
fn stamp_created(
    dir: &Path,
    name: &str,
    raw: String,
    spec: Result<file::Spec, String>,
    now: DateTime<Local>,
) -> (String, Result<file::Spec, String>) {
    if !spec
        .as_ref()
        .is_ok_and(|s| s.sensor.is_some() && s.created.is_none())
    {
        return (raw, spec);
    }
    let text = file::set_created(&raw, now);
    match file::write_atomic(dir, name, &text) {
        Ok(_) => {
            let spec = file::parse(name, &text);
            (text, spec)
        }
        Err(e) => {
            tracing::warn!(%name, "created: not written: {e}");
            (raw, spec)
        }
    }
}

/// A fire whose turn has not finished (34 §4).
struct Run {
    id: i64,
    name: String,
    quiet: bool,
    text: String,
    messaged: bool,
    last_error: Option<String>,
    deadline: Instant,
    timed_out: bool,
    /// Subagent sessions spawned under the run, grandchildren included (33 §3.4).
    children: HashSet<SessionId>,
    /// Direct children whose result has not reached the run's session yet
    /// (a `PeerDelivered` from `session:<child>`, 50 §10): a just-spawned
    /// child is `Idle` before its first turn, so liveness alone would close
    /// the run early (33 §3.4).
    pending: HashSet<SessionId>,
    /// A turn is in flight on the run's session (`TurnStarted`/`TurnFinished`).
    turn_open: bool,
    /// `stop_reason` of the last `TurnFinished`; what [`Settle`] closes with.
    last_stop: String,
    /// A `when:` fire's payload, hash and state: committed when the run
    /// [`run_succeeded`], a strike on `error` / `timeout` (40 §1.7, §5).
    watch: Option<WhenFire>,
    /// The fire's wall clock (`watch.last_fired_at` on commit).
    fired_at: DateTime<Local>,
}

/// What a fire produced.
#[derive(Debug)]
enum Fired {
    /// The schedule's session took the prompt (34 §4).
    Session(SessionId),
    /// A task was created for the schedule's agent (67 §7.4); its id.
    Task(String),
}

/// Internal `tell`: the directory changed (watcher) — re-read it (34 §2.3).
#[derive(Clone, Copy, Debug)]
struct Reconcile;

/// Grace between a fanned-out run's `TurnFinished` and its [`Settle`]: long
/// enough for the agent's `TurnDone` to start a queued result turn and for
/// its `TurnStarted` to reach this actor. `ponytail:` fixed; a `TurnStarted`
/// that arrives later than this closes nothing, it just runs untracked.
const SETTLE: Duration = Duration::from_millis(500);

/// Internal `tell`: fire what is due, cancel what is past its deadline (34 §3–4).
#[derive(Clone, Copy, Debug)]
struct Tick;

/// Internal `ask` → `Option<Duration>`: time until the earliest `next` or run
/// deadline; `None` when nothing is pending.
#[derive(Clone, Copy, Debug)]
struct NextWake;

/// The [`SCHEDULE_ACTOR`] actor (34 §5): the live set of schedule files, the
/// runs in flight, and the run history.
#[derive(kameo::Actor)]
pub struct Schedule {
    rt: Runtime,
    dir: PathBuf,
    store: store::Store,
    timeout: Duration,
    /// The sensor runtime (40 §4); `tick_when` and the creation dry run go through it.
    sensors: Arc<agent::sensor::SensorRunner>,
    entries: BTreeMap<String, Entry>,
    /// Keyed by session: that is what [`agent::AgentEvent`] carries.
    running: HashMap<SessionId, Run>,
    /// Files the actor removed itself by ending a loop (33 §3.3); the
    /// reconcile that notices writes no `removed` row for these.
    consumed: HashSet<String>,
    /// Turns `agent:` fires into tasks (67 §7.4); `None` until the tasks plugin registers.
    task_creator: Option<Arc<dyn TaskCreator>>,
    /// [`mesh::is_hub`] as of the last tick; ticks are a hub duty (67 §4.4).
    hub: bool,
    /// Watchers whose sensor check is in flight (40 §1.9: no self-overlap); memory only.
    checking: HashSet<String>,
    /// `mode: queue` pending payloads, one per watcher, newest wins (40 §1.9); memory only.
    pending: HashMap<String, WhenFire>,
    /// Watchers loaded or changed since their last check (40 §1.12): the first
    /// check is the file's dry run — a failing one is `check broken`, not a strike.
    unvalidated: HashSet<String>,
    /// Stands in for `sensors.check` in tests: `(args, state) -> result`.
    #[cfg(test)]
    fake_sensor: Option<FakeSensor>,
}

impl Message<RegisterTaskCreator> for Schedule {
    type Reply = ();

    async fn handle(
        &mut self,
        m: RegisterTaskCreator,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.task_creator = Some(m.0);
    }
}

impl Schedule {
    /// Read the directory, join the store's latest row per name (34 §7) and
    /// arm `missed: run` cron schedules (34 §3).
    async fn load(
        rt: Runtime,
        dir: PathBuf,
        store: store::Store,
        timeout: Duration,
        sensors: Arc<agent::sensor::SensorRunner>,
    ) -> Self {
        let now = Local::now();
        let mut latest = match store.latest().await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("schedule history unreadable: {e:#}");
                HashMap::new()
            }
        };
        let mut entries = BTreeMap::new();
        let mut unvalidated = HashSet::new();
        for (name, raw, spec) in file::load_dir(&dir) {
            let (raw, spec) = stamp_created(&dir, &name, raw, spec, now);
            if spec.as_ref().is_ok_and(|s| s.sensor.is_some()) {
                unvalidated.insert(name.clone());
            }
            let mut e = Entry::new(raw, spec, now);
            if let Some(l) = latest.remove(&name) {
                e.session = l.session;
                e.last = l.last;
                e.last_result = l.last_result;
                e.arm(now);
            }
            // 40 §1.12: a session bound at creation precedes any run row.
            if e.session.is_none()
                && e.spec.as_ref().is_ok_and(|s| s.sensor.is_some())
                && let Ok(Some(w)) = store.watch_get(&name).await
            {
                e.session = w.session_id;
            }
            entries.insert(name, e);
        }
        Self {
            rt,
            dir,
            store,
            timeout,
            sensors,
            entries,
            running: HashMap::new(),
            consumed: HashSet::new(),
            task_creator: None,
            hub: mesh::is_hub(),
            checking: HashSet::new(),
            pending: HashMap::new(),
            unvalidated,
            #[cfg(test)]
            fake_sensor: None,
        }
    }

    /// 109 §4.6 step 12: the fires the store spared at open become runs
    /// again, turn open, so the resumed turn's `TurnFinished` closes them
    /// with its real stop reason; a fresh deadline guards the resume itself.
    fn reattach(&mut self, spared: Vec<store::OpenRun>) {
        for r in spared {
            let quiet = self
                .entries
                .get(&r.name)
                .and_then(|e| e.spec.as_ref().ok())
                .is_some_and(|s| s.quiet);
            tracing::info!(name = %r.name, session = %r.session, "run re-attached to a resumed turn");
            self.running.insert(
                r.session,
                Run {
                    id: r.id,
                    name: r.name,
                    quiet,
                    text: String::new(),
                    messaged: false,
                    last_error: None,
                    deadline: Instant::now() + self.timeout,
                    timed_out: false,
                    children: HashSet::new(),
                    pending: HashSet::new(),
                    turn_open: true,
                    last_stop: String::new(),
                    watch: None,
                    fired_at: r.fired_at,
                },
            );
        }
    }

    fn is_running(&self, e: &Entry) -> bool {
        e.session.is_some_and(|s| self.running.contains_key(&s))
    }

    /// When a run fired now is cancelled (88): the file's own `run_timeout:`,
    /// else the live `schedule.run_timeout_secs`.
    fn deadline_for(&self, run_timeout: Option<Duration>) -> Instant {
        Instant::now() + run_timeout.unwrap_or(self.timeout)
    }

    /// Fires since the loop's last end row (33 §3.2); 0 when the store fails.
    async fn runs_of(&self, name: &str) -> u32 {
        self.store.count_since_end(name).await.unwrap_or_else(|e| {
            tracing::error!(%name, "run count unreadable: {e:#}");
            0
        })
    }

    /// 34 §4 steps 1–5 with 33 §3.1–3.2's stop before them. `forced`
    /// (`/schedules run`) skips the stop: *run* means run. The caller has
    /// checked the entry exists and parsed. With `agent:` (67 §7.4) the
    /// wrapped prompt becomes a task through the registered [`TaskCreator`]
    /// instead of a session input. `when` is a watcher's payload (40 §3.3):
    /// it joins the wrapper, its first line is the row's `envelope`, and the
    /// run commits it when it ends successfully (§1.7, [`run_succeeded`]).
    async fn fire(
        &mut self,
        name: &str,
        now: DateTime<Local>,
        forced: bool,
        when: Option<WhenFire>,
    ) -> Result<Fired, String> {
        let entry = self.entries.get(name).ok_or("no such schedule")?;
        let spec = entry.spec.as_ref().map_err(Clone::clone)?;
        let session = entry.session;
        let (quiet, model, prompt) = (spec.quiet, spec.model.clone(), spec.prompt.clone());
        let (until, max_runs, cwd, approvals) = (
            spec.until.clone(),
            spec.max_runs,
            spec.cwd.clone(),
            spec.approvals,
        );
        let (agent, project) = (spec.agent.clone(), spec.project.clone());
        let (memory, sensor_file) = (spec.memory, spec.sensor.as_ref().map(|s| s.file.clone()));
        let reset_runtime = resets_runtime(spec);
        let run_timeout = spec.run_timeout;
        let one_shot = matches!(spec.when, file::When::At(_));
        let is_loop = until.is_some() || max_runs.is_some();

        if self.is_running(entry) {
            if let Err(e) = self
                .store
                .insert_run(name, session, now, Some("skipped: busy"))
                .await
            {
                tracing::error!(%name, "skip not recorded: {e:#}");
            }
            if let Some(e) = self.entries.get_mut(name) {
                e.next = next_of(&e.spec, now, Some(now));
            }
            self.rt
                .publish(ScheduleEvent::Skipped {
                    name: name.to_owned(),
                    at: now,
                    reason: "busy".to_owned(),
                })
                .await;
            return Err("busy".to_owned());
        }

        // 33 §3.1–3.2: `until` first (a passing check is `done`, not capped),
        // then the cap. `Err(why)` ends the loop.
        let runs = if is_loop { self.runs_of(name).await } else { 0 };
        let mut until_tail = None;
        if !forced {
            let stop = match &until {
                None => Ok(()),
                Some(cmd) => match until_check(cmd, cwd.as_deref()).await {
                    Ok((Some(0), _)) => Err(format!("done: until passed after {runs} runs")),
                    Ok((Some(1), tail)) => {
                        until_tail = Some(tail);
                        Ok(())
                    }
                    Ok((Some(code), tail)) => Err(format!("until broken: exit {code}: {tail}")),
                    Ok((None, tail)) => Err(format!("until broken: killed: {tail}")),
                    Err(e) => Err(format!("until broken: {e}")),
                },
            };
            let stop = stop.and_then(|()| match max_runs {
                Some(max) if runs >= max => Err(format!("max_runs {max} reached")),
                _ => Ok(()),
            });
            if let Err(why) = stop {
                self.end(name, why.clone()).await;
                return Err(why);
            }
        }

        // The wrapper (34 §1.5, 33 §4).
        let mut text = format!(
            "<schedule name=\"{name}\" at=\"{}\" quiet=\"{quiet}\"",
            now.to_rfc3339()
        );
        if is_loop {
            let _ = write!(text, " run=\"{}\"", runs + 1);
        }
        if let Some(max) = max_runs {
            let _ = write!(text, " of=\"{max}\"");
        }
        text.push_str(">\n");
        if let Some(tail) = &until_tail {
            let _ = write!(text, "<until exit=\"1\">\n{}\n</until>\n", tail.trim_end());
        }
        text.push_str(&prompt);
        // 40 §3.3: body, blank line, `<when>` block.
        if let (Some(w), Some(sensor)) = (&when, &sensor_file) {
            text.push_str("\n\n");
            text.push_str(&w.block(sensor));
        }
        text.push_str("\n</schedule>");
        let envelope = when.as_ref().map(|w| w.envelope().to_owned());
        let subject = when
            .as_ref()
            .and_then(|w| agent::sensor::parse_envelope(&w.payload).remove("subject"));
        if let Some(w) = &when {
            tracing::info!(
                %name,
                "fired by: when ({})",
                agent::sensor::parse_envelope(&w.payload)
                    .get("events")
                    .map_or("", String::as_str)
            );
        }

        if let Some(agent) = agent {
            let creator = self.task_creator.clone().ok_or("tasks is not running")?;
            let fire_ms = now.timestamp_millis().max(0) as u64;
            let task = creator
                .create_for_schedule(
                    name,
                    fire_ms,
                    &agent,
                    &text,
                    project.as_deref(),
                    cwd.as_deref().and_then(Path::to_str),
                )
                .await
                .map_err(|e| format!("tasks: {e:#}"))?;
            if one_shot {
                let path = self.dir.join(format!("{name}.md"));
                if let Err(e) = std::fs::remove_file(&path) {
                    tracing::warn!("{}: not removed after firing: {e}", path.display());
                }
            }
            // A finished row: the task, not a session, carries the run (counts toward
            // `max_runs`); a `when:` fire commits with it (40 §1.7: nothing to wait for).
            if let Err(e) = self
                .store
                .insert_run_with(
                    name,
                    None,
                    now,
                    Some(&format!("task {task}")),
                    envelope.as_deref(),
                )
                .await
            {
                tracing::error!(%name, "run not recorded: {e:#}");
            }
            if let Some(w) = &when
                && let Err(e) = self.store.watch_commit(name, &w.state, &w.hash, now).await
            {
                tracing::error!(%name, "watch not committed: {e:#}");
            }
            if let Some(e) = self.entries.get_mut(name) {
                e.last = Some(now);
                e.last_result = Some(format!("task {task}"));
                e.next = if one_shot {
                    None
                } else {
                    next_of(&e.spec, now, Some(now))
                };
            }
            self.rt.publish(ScheduleEvent::Changed).await;
            return Ok(Fired::Task(task));
        }

        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or("agent is not running")?;

        let session = match session {
            Some(s) => {
                // 40 §1.11: a watcher's `memory: continue` keeps the frame; `clear`
                // (and every clock-only file) is 34 §4 step 3.
                if sensor_file.is_none() || memory == file::Memory::Clear {
                    // `Err("nothing to clear")` on an empty session is fine (34 §4.3).
                    if let Err(e) = agents
                        .ask(agent::Compact {
                            session: s,
                            style: Some(agent::CompactStyle::Clear),
                            head: None,
                        })
                        .await
                    {
                        tracing::debug!(%name, "clear before fire: {e}");
                    }
                    // 131 D1: the python runtime goes with the frame unless
                    // `runtime: keep`; a failure never blocks the fire.
                    if reset_runtime
                        && let Err(e) = agents.ask(agent::ResetRuntime { session: s }).await
                    {
                        tracing::debug!(%name, "runtime reset before fire: {e}");
                    }
                }
                s
            }
            None => new_session(&agents, name, cwd.clone(), model, subject.as_deref()).await?,
        };
        // 33 §3.5: in-memory in the registry, so every fire re-sends it.
        if !approvals {
            match ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR) {
                Ok(Some(reg)) => {
                    if let Err(e) = reg
                        .ask(::tools::SetApprovals {
                            session: Some(session),
                            enabled: false,
                        })
                        .await
                    {
                        tracing::warn!(%name, "approvals not switched off: {e}");
                    }
                }
                _ => {
                    tracing::warn!(%name, "approvals not switched off: tool registry is not running")
                }
            }
        }

        agents
            .ask(agent::SendInput {
                session: Some(session),
                text,
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
            .map_err(|e| format!("agent: {e}"))?;
        if one_shot {
            // The fire is the file's purpose; the reconcile that follows drops the entry.
            let path = self.dir.join(format!("{name}.md"));
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!("{}: not removed after firing: {e}", path.display());
            }
        }

        let id = match self
            .store
            .insert_run_with(name, Some(session), now, None, envelope.as_deref())
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(%name, "run not recorded: {e:#}");
                0
            }
        };
        self.running.insert(
            session,
            Run {
                id,
                name: name.to_owned(),
                quiet,
                text: String::new(),
                messaged: false,
                last_error: None,
                deadline: self.deadline_for(run_timeout),
                timed_out: false,
                children: HashSet::new(),
                pending: HashSet::new(),
                turn_open: true,
                last_stop: String::new(),
                watch: when,
                fired_at: now,
            },
        );
        if let Some(e) = self.entries.get_mut(name) {
            e.session = Some(session);
            e.last = Some(now);
            e.next = if one_shot {
                None
            } else {
                next_of(&e.spec, now, Some(now))
            };
        }
        self.rt
            .publish(ScheduleEvent::Fired {
                name: name.to_owned(),
                session,
                at: now,
            })
            .await;
        Ok(Fired::Session(session))
    }

    /// End a loop (33 §3.3): an end row, the file removed (remembered in
    /// `consumed` so the reconcile writes no `removed` row), the user told
    /// regardless of `quiet`. A run in flight finishes normally afterwards.
    async fn end(&mut self, name: &str, why: String) {
        let session = self.entries.get(name).and_then(|e| e.session);
        if let Err(e) = self
            .store
            .insert_run(name, session, Local::now(), Some(&why))
            .await
        {
            tracing::error!(%name, "end not recorded: {e:#}");
        }
        let path = self.dir.join(format!("{name}.md"));
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.consumed.insert(name.to_owned());
            }
            Err(e) => tracing::warn!("{}: not removed after ending: {e}", path.display()),
        }
        if let Some(e) = self.entries.get_mut(name) {
            e.next = None;
            e.last_result = Some(why.clone());
        }
        tracing::info!(%name, "ended: {why}");
        // 67 §7.4: `until` passing closes the agent's open tasks for this schedule.
        if why.starts_with("done")
            && let Some(e) = self.entries.get(name)
            && e.spec.as_ref().is_ok_and(|s| s.agent.is_some())
            && let Some(creator) = self.task_creator.clone()
            && let Err(err) = creator.close_for_schedule(name, &why).await
        {
            tracing::warn!(%name, "schedule tasks not closed: {err:#}");
        }
        deliver(session, name.to_owned(), format!("schedule {name} {why}"));
    }
}

impl Message<Reconcile> for Schedule {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, ctx: &mut Context<Self, Self::Reply>) {
        self.reconcile(ctx.actor_ref()).await;
    }
}

impl Schedule {
    /// Re-read the directory (34 §2.3): new/changed files re-parse, gone files
    /// get a `removed` row; publishes `Changed`.
    async fn rescan(&mut self, now: DateTime<Local>) {
        let mut seen = HashSet::new();
        for (name, raw, spec) in file::load_dir(&self.dir) {
            seen.insert(name.clone());
            let (raw, spec) = stamp_created(&self.dir, &name, raw, spec, now);
            match self.entries.get_mut(&name) {
                Some(e) if e.raw == raw => continue,
                Some(e) => {
                    e.next = next_of(&spec, now, e.last);
                    e.spec = spec;
                    e.raw = raw;
                }
                None => {
                    self.entries
                        .insert(name.clone(), Entry::new(raw, spec, now));
                }
            }
            // A new or changed watcher is dry-run by its first check (40 §1.12).
            if self.entries[&name]
                .spec
                .as_ref()
                .is_ok_and(|s| s.sensor.is_some())
            {
                self.unvalidated.insert(name);
            }
        }
        // 33 §3.3: a file gone other than by the actor's own hand (an `at:`
        // fire, an `end`) gets a `removed` row; the count restarts for the name.
        let gone: Vec<String> = self
            .entries
            .keys()
            .filter(|n| !seen.contains(*n))
            .cloned()
            .collect();
        for name in gone {
            let Some(e) = self.entries.remove(&name) else {
                continue;
            };
            // 40 §3.4: the sensor's state goes with the file; a re-created watcher
            // starts fresh. Unconditional: a watcher broken on load parses as `Err`.
            self.pending.remove(&name);
            self.unvalidated.remove(&name);
            if let Err(err) = self.store.watch_delete(&name).await {
                tracing::error!(%name, "watch state not forgotten: {err:#}");
            }
            if self.consumed.remove(&name)
                || e.spec
                    .as_ref()
                    .is_ok_and(|s| matches!(s.when, file::When::At(_)))
            {
                continue;
            }
            if let Err(err) = self
                .store
                .insert_run(&name, e.session, now, Some("removed"))
                .await
            {
                tracing::error!(%name, "removal not recorded: {err:#}");
            }
        }
        self.rt.publish(ScheduleEvent::Changed).await;
    }

    /// [`Self::rescan`], then wake the tick loop: a new file may already be
    /// due and the loop is asleep for up to TICK.
    async fn reconcile(&mut self, me: &ActorRef<Self>) {
        self.rescan(Local::now()).await;
        let _ = me.tell(Tick).await;
    }
}

impl Message<Tick> for Schedule {
    type Reply = ();

    async fn handle(&mut self, _: Tick, ctx: &mut Context<Self, Self::Reply>) {
        for check in self.tick(Local::now()).await {
            let me = ctx.actor_ref().clone();
            tokio::spawn(async move {
                let _ = me.tell(check.await).await;
            });
        }

        let inst = Instant::now();
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten();
        for (session, run) in self
            .running
            .iter_mut()
            .filter(|(_, r)| !r.timed_out && r.deadline <= inst)
        {
            let Some(agents) = &agents else {
                tracing::warn!(name = %run.name, "timed out but agent is not running");
                break;
            };
            match cancel_tree(agents, *session, &run.name).await {
                Ok(()) => run.timed_out = true,
                Err(e) => tracing::warn!(name = %run.name, "timeout cancel failed: {e}"),
            }
        }
    }
}

impl Schedule {
    /// Fire what is due (34 §3) — on the hub only (67 §4.4). A non-hub
    /// neither fires nor advances `last`; regaining the hub re-arms every
    /// entry as at boot, so `missed:` decides what fires for the pause.
    /// A due `when:` file starts its sensor check instead (40 §3.2); the
    /// checks are returned for the caller to spawn, their results come back
    /// as [`CheckDone`].
    async fn tick(&mut self, now: DateTime<Local>) -> Vec<Check> {
        let hub = mesh::is_hub();
        if hub != self.hub {
            self.hub = hub;
            if hub {
                tracing::info!("schedules: hub regained, ticks resumed");
                for e in self.entries.values_mut() {
                    e.arm(now);
                }
            } else {
                tracing::info!("schedules: not the hub, ticks paused");
            }
        }
        let mut checks = Vec::new();
        if !hub {
            return checks;
        }
        // A watcher ticks while its run is in flight: `mode` decides (40 §1.9).
        let due: Vec<(String, bool)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.next.is_some_and(|t| t <= now))
            .filter_map(|(n, e)| {
                let watcher = e.spec.as_ref().is_ok_and(|s| s.sensor.is_some());
                (watcher || !self.is_running(e)).then(|| (n.clone(), watcher))
            })
            .collect();
        for (name, watcher) in due {
            if watcher {
                checks.extend(self.tick_when(&name, now).await);
            } else if let Err(e) = self.fire(&name, now, false, None).await {
                tracing::warn!(%name, "not fired: {e}");
            }
        }
        checks
    }
}

impl Message<NextWake> for Schedule {
    type Reply = Option<Duration>;

    async fn handle(&mut self, _: NextWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = Local::now();
        let inst = Instant::now();
        // Running entries count: a fire due while its run is still going becomes
        // a `skipped: busy` row at that tick, and a run that finished early must
        // not leave the loop asleep until TICK.
        // A non-hub does not fire: only run deadlines wake it, else TICK.
        let next = self
            .entries
            .values()
            .filter_map(|e| e.next)
            .min()
            .filter(|_| mesh::is_hub())
            .map(|t| (t - now).to_std().unwrap_or_default());
        let deadline = self
            .running
            .values()
            .filter(|r| !r.timed_out)
            .map(|r| r.deadline.saturating_duration_since(inst))
            .min();
        match (next, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Sessions under `session` whose turn is in flight or awaiting a peer
/// (33 §3.4); empty when the agent cannot be asked.
async fn live_children(agents: &ActorRef<agent::Agents>, session: SessionId) -> Vec<SessionId> {
    match agents.ask(agent::ListAgents { session }).await {
        Ok(list) => list
            .into_iter()
            .filter(|a| {
                matches!(
                    a.state,
                    agent::AgentState::Running | agent::AgentState::Awaiting { .. }
                )
            })
            .map(|a| a.id)
            .collect(),
        Err(e) => {
            tracing::warn!(%session, "subagents not listed: {e}");
            Vec::new()
        }
    }
}

/// Hand `text` to the user as `schedule <name>` (34 §4.1, 33 §3.3). Off the
/// mailbox: every channel's owner may be slow (plugins.md §3.2).
fn deliver(session: Option<SessionId>, name: String, text: String) {
    tokio::spawn(async move {
        let Ok(Some(user)) = ActorRef::<agent::UserChannels>::lookup(agent::USER_ACTOR) else {
            tracing::info!(%name, "not delivered: user channels are not running");
            return;
        };
        let m = agent::Outbound {
            sessions: session.into_iter().collect(),
            origin: format!("schedule {name}"),
            text,
            files: vec![],
        };
        match user.ask(agent::Deliver { m, via: None }).await {
            Ok(where_) => tracing::info!(%name, ?where_, "delivered"),
            // No channel: the transcript is the record.
            Err(e) => tracing::info!(%name, "not delivered: {e}"),
        }
    });
}

/// 131 D1: a fire that clears the frame (every clock-only file; a watcher
/// with `memory: clear`) also resets the python runtime, unless the file says
/// `runtime: keep`. A watcher with `memory: continue` clears neither.
fn resets_runtime(spec: &file::Spec) -> bool {
    (spec.sensor.is_none() || spec.memory == file::Memory::Clear)
        && spec.runtime == file::Runtime::Clear
}

/// 34 §4 step 2, shared by [`Schedule::fire`] and `BindSession` (40 §1.12):
/// the schedule's session, titled `<name>` or `<name> · <subject>`, with
/// `model:` applied.
async fn new_session(
    agents: &ActorRef<agent::Agents>,
    name: &str,
    cwd: Option<PathBuf>,
    model: Option<String>,
    subject: Option<&str>,
) -> Result<SessionId, String> {
    // Unattended automation roots are `isolated` (50 §2.3): `NewSession` is
    // the isolated path; the owner promotes one with `SetPeerScope` if a
    // schedule should reach owner roots. `unattended` drops the visualization
    // blocks from its prompt.
    let s = agents
        .ask(agent::NewSession {
            cwd,
            unattended: true,
        })
        .await
        .map_err(|e| format!("agent: {e}"))?;
    let title = match subject {
        Some(subject) => format!("{name} · {subject}"),
        None => name.to_owned(),
    };
    if let Err(e) = agents.ask(agent::RenameSession { session: s, title }).await {
        tracing::warn!(%name, "session title not set: {e}");
    }
    // 82 §2: the schedule's `model:` is for its own session; it never becomes the default.
    if let Some(model) = model
        && let Err(e) = agents
            .ask(agent::SetModel {
                session: Some(s),
                model,
                manual: false,
            })
            .await
    {
        tracing::warn!(%name, "model not set: {e}");
    }
    Ok(s)
}

/// Cancel `session` after its live subtree (33 §3.4: the subtree goes with the run).
async fn cancel_tree(
    agents: &ActorRef<agent::Agents>,
    session: SessionId,
    name: &str,
) -> Result<(), String> {
    for child in live_children(agents, session).await {
        if let Err(e) = agents
            .ask(agent::Cancel {
                session: child,
                keep: None,
                scope: agent::CancelScope::Root,
            })
            .await
        {
            tracing::warn!(%name, %child, "cancel of subagent failed: {e}");
        }
    }
    // 133 §3: the root cancel reaches every running descendant, any depth.
    agents
        .ask(agent::Cancel {
            session,
            keep: None,
            scope: agent::CancelScope::Tree,
        })
        .await
        .map_err(|e| e.to_string())
}

impl Message<CheckDone> for Schedule {
    type Reply = ();

    async fn handle(&mut self, m: CheckDone, _: &mut Context<Self, Self::Reply>) {
        self.check_done(&m.name, m.result, Local::now()).await;
    }
}

impl Schedule {
    /// The sensor state the `watch` row holds for `name`; `{}` before the first commit.
    async fn state_of(&self, name: &str) -> serde_json::Value {
        match self.store.watch_get(name).await {
            Ok(Some(w)) => serde_json::from_str(&w.state).unwrap_or_else(|e| {
                tracing::error!(%name, "watch state unreadable, starting fresh: {e}");
                serde_json::json!({})
            }),
            Ok(None) => serde_json::json!({}),
            Err(e) => {
                tracing::error!(%name, "watch row unreadable: {e:#}");
                serde_json::json!({})
            }
        }
    }

    /// The check of `sensor` against `state` as a future the caller spawns
    /// (`Tick`) or awaits (`Fire`). `Err` when the sensor does not resolve.
    fn check_future(
        &self,
        spec: &file::Spec,
        sensor: &file::Sensor,
        state: serde_json::Value,
    ) -> Result<Check, SensorError> {
        let meta = self.sensors.resolve(&sensor.file)?;
        let name = spec.name.clone();
        #[cfg(test)]
        if let Some(fake) = &self.fake_sensor {
            let result = fake(&sensor.args, &state);
            return Ok(Box::pin(async move { CheckDone { name, result } }));
        }
        let (sensors, args, cwd, timeout) = (
            self.sensors.clone(),
            sensor.args.clone(),
            spec.cwd.clone(),
            spec.timeout,
        );
        Ok(Box::pin(async move {
            let result = sensors
                .check(&meta, &args, &state, cwd.as_deref(), timeout)
                .await;
            CheckDone { name, result }
        }))
    }

    /// `created:` of a watcher (written on first load by [`stamp_created`]),
    /// else the file's mtime when that write failed (40 §1.10).
    fn created_of(&self, spec: &file::Spec) -> Option<DateTime<Local>> {
        spec.created.or_else(|| {
            std::fs::metadata(self.dir.join(format!("{}.md", spec.name)))
                .and_then(|m| m.modified())
                .ok()
                .map(DateTime::<Local>::from)
        })
    }

    /// A gate row (40 §3.1) — `session: None`, so it counts toward nothing
    /// (33 §3.2) — remembered as the entry's `last_result`.
    async fn row(&mut self, name: &str, now: DateTime<Local>, why: &str) {
        if let Err(e) = self.store.insert_run(name, None, now, Some(why)).await {
            tracing::error!(%name, "check not recorded: {e:#}");
        }
        if let Some(e) = self.entries.get_mut(name) {
            e.last_result = Some(why.to_owned());
        }
    }

    /// The watcher's next tick, unless it was disabled meanwhile.
    fn set_next(&mut self, name: &str, next: DateTime<Local>) {
        if let Some(e) = self.entries.get_mut(name)
            && e.spec.as_ref().is_ok_and(|s| s.enabled)
        {
            e.next = Some(next);
        }
    }

    /// 40 §3.2, first half: one tick of a `when:` file up to the sensor call.
    /// `expires` ends the file (§1.10); a check still in flight is a row
    /// (§1.9); else the check starts and is returned for the caller to await.
    /// `None` when nothing is left to await.
    async fn tick_when(&mut self, name: &str, now: DateTime<Local>) -> Option<Check> {
        let spec = self.entries.get(name)?.spec.as_ref().ok()?.clone();
        let sensor = spec.sensor.as_ref()?;
        let every = every_of(&spec);
        if let (Some(ttl), Some(created)) = (spec.expires, self.created_of(&spec))
            && plus(created, ttl) <= now
        {
            self.end(name, "expired".to_owned()).await;
            return None;
        }
        if self.checking.contains(name) {
            self.row(name, now, "skipped: check still running").await;
            self.set_next(name, plus(now, every));
            return None;
        }
        let state = self.state_of(name).await;
        match self.check_future(&spec, sensor, state) {
            Ok(check) => {
                self.checking.insert(name.to_owned());
                // Re-armed by `check_done`; a check outlasting `every` is the row above.
                self.set_next(name, plus(now, every));
                Some(check)
            }
            Err(e) => {
                self.check_done(name, Err(e), now).await;
                None
            }
        }
    }

    /// 40 §3.2, second half: the gate over the sensor's answer, then rows,
    /// state, strikes and the fire. `None` when the file is gone or no longer
    /// a watcher.
    async fn check_done(
        &mut self,
        name: &str,
        result: Result<SensorRun, SensorError>,
        now: DateTime<Local>,
    ) -> Option<CheckOutcome> {
        self.checking.remove(name);
        // Its dry run (40 §1.12); a deferred check leaves it for the next tick.
        let dry_run = !deferred(&result) && self.unvalidated.remove(name);
        let e = self.entries.get(name)?;
        let spec = e.spec.as_ref().ok()?.clone();
        spec.sensor.as_ref()?;
        let busy = self.is_running(e);
        let mut w = self
            .store
            .watch_get(name)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(%name, "watch row unreadable: {e:#}");
                None
            })
            .unwrap_or_default();
        let queued = self.pending.get(name).map(|p| p.coalesced);
        let (outcome, next) = gate(&spec, &mut w, &result, now, busy, queued);
        let reason = outcome.to_string();
        match (&outcome, result) {
            // §5: deferred, not dropped — a row and `last_check`, no strike, no state.
            (CheckOutcome::Deferred, _) => {
                self.row(name, now, &reason).await;
                if let Err(e) = self.store.watch_last_check(name, now, &reason, 0).await {
                    tracing::error!(%name, "check not recorded: {e:#}");
                }
            }
            (CheckOutcome::Broken(why), _) => {
                self.row(name, now, &reason).await;
                if let Err(e) = self.store.watch_last_check(name, now, &reason, 0).await {
                    tracing::error!(%name, "check not recorded: {e:#}");
                }
                if dry_run {
                    // A hand-edited file failing its dry run is listed `check broken`
                    // and never ticks until the file changes (40 §1.12).
                    if let Some(e) = self.entries.get_mut(name) {
                        e.spec = Err(reason.clone());
                        e.next = None;
                    }
                    self.rt.publish(ScheduleEvent::Changed).await;
                } else {
                    self.strike(name, now, why).await;
                }
            }
            (_, Ok(r)) => {
                // A check that fires nothing is a successful check: state commits (§3.2).
                let commit = matches!(outcome, CheckOutcome::Quiet | CheckOutcome::Unchanged)
                    .then(|| r.state.to_string());
                if let Err(e) = self
                    .store
                    .watch_check_ok(
                        name,
                        now,
                        &reason,
                        r.elapsed.as_millis() as u64,
                        w.holding_since,
                        commit.as_deref(),
                    )
                    .await
                {
                    tracing::error!(%name, "check not recorded: {e:#}");
                }
                match &outcome {
                    CheckOutcome::Fired => self.fire_when(name, now, WhenFire::new(&r, 0)).await,
                    CheckOutcome::Restarted => {
                        self.row(name, now, &reason).await;
                        self.restart(name).await;
                        self.fire_when(name, now, WhenFire::new(&r, 0)).await;
                    }
                    CheckOutcome::Queued { coalesced } => {
                        // Newest wins (§1.9); delivered when the run closes.
                        self.pending
                            .insert(name.to_owned(), WhenFire::new(&r, *coalesced));
                        self.row(name, now, &reason).await;
                    }
                    _ => self.row(name, now, &reason).await,
                }
            }
            // `gate` answers `Broken` or `Deferred` for every `Err`.
            (_, Err(_)) => {}
        }
        if !matches!(outcome, CheckOutcome::Fired | CheckOutcome::Restarted) {
            self.set_next(name, next);
        }
        Some(outcome)
    }

    /// 40 §3.3: the fire with the payload; `last_fired_at` is the fire, not
    /// the commit (§1.8: `cooldown` spaces fires whose runs fail too);
    /// `fire: once` disables afterwards (§1.6).
    async fn fire_when(&mut self, name: &str, now: DateTime<Local>, w: WhenFire) {
        let once = self
            .entries
            .get(name)
            .is_some_and(|e| e.spec.as_ref().is_ok_and(|s| s.fire == file::Fire::Once));
        match self.fire(name, now, false, Some(w)).await {
            Ok(_) => {
                if let Err(e) = self.store.watch_fired_at(name, now).await {
                    tracing::error!(%name, "fire not recorded: {e:#}");
                }
                if once {
                    if let Err(e) = self.set_enabled(name, false).await {
                        tracing::warn!(%name, "not disabled after firing once: {e}");
                    }
                    self.rt.publish(ScheduleEvent::Changed).await;
                }
            }
            Err(e) => tracing::warn!(%name, "not fired: {e}"),
        }
    }

    /// §3.3: the `mode: queue` payload meets the gate again when the run
    /// closes — its hash against the just-committed one, its fire against
    /// `cooldown` — so a tick that landed mid-run does not fire on stale news.
    async fn deliver_pending(&mut self, name: &str, p: WhenFire) {
        let now = Local::now();
        let Some(spec) = self.entries.get(name).and_then(|e| e.spec.as_ref().ok()) else {
            return;
        };
        let w = self
            .store
            .watch_get(name)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(%name, "watch row unreadable: {e:#}");
                None
            })
            .unwrap_or_default();
        let dropped =
            if spec.fire == file::Fire::OnChange && w.last_hash.as_deref() == Some(&p.hash) {
                Some(CheckOutcome::Unchanged)
            } else {
                w.last_fired_at
                    .filter(|t| since(now, *t) < spec.cooldown)
                    .map(|t| CheckOutcome::Cooldown {
                        left: since(plus(t, spec.cooldown), now),
                    })
            };
        match dropped {
            Some(o) => {
                tracing::info!(%name, "queued payload dropped: {o}");
                self.row(name, now, &o.to_string()).await;
            }
            None => self.fire_when(name, now, p).await,
        }
    }

    /// `mode: restart` (40 §1.9): the in-flight run closes `cancelled` now and
    /// its session is cancelled; the fire follows on the same session.
    async fn restart(&mut self, name: &str) {
        let Some(session) = self.entries.get(name).and_then(|e| e.session) else {
            return;
        };
        let Some(run) = self.running.remove(&session) else {
            return;
        };
        self.finished(session, run, "cancelled").await;
        match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            Ok(Some(agents)) => {
                if let Err(e) = cancel_tree(&agents, session, name).await {
                    tracing::warn!(%name, "restart cancel failed: {e}");
                }
            }
            _ => tracing::warn!(%name, "restart: agent is not running"),
        }
    }

    /// A strike (40 §5): a broken check or a failed run. The third in a row
    /// disables the watcher, writes `check broken ×3, disabled`, and tells the
    /// user regardless of `quiet`; `resume` re-arms at 0.
    async fn strike(&mut self, name: &str, now: DateTime<Local>, why: &str) {
        let n = match self.store.watch_strike(name).await {
            Ok(n) => n,
            Err(e) => {
                tracing::error!(%name, "strike not recorded: {e:#}");
                return;
            }
        };
        if n < MAX_STRIKES {
            return;
        }
        if let Err(e) = self.set_enabled(name, false).await {
            tracing::warn!(%name, "not disabled after {n} strikes: {e}");
        }
        let row = format!("check broken ×{MAX_STRIKES}, disabled");
        self.row(name, now, &row).await;
        self.rt.publish(ScheduleEvent::Changed).await;
        let session = self.entries.get(name).and_then(|e| e.session);
        deliver(
            session,
            name.to_owned(),
            format!("schedule {name} {row}: {why}"),
        );
    }

    /// Rewrite the file's `enabled:` line (33 §3.5) in place so the reply is
    /// current — the watcher's reconcile then finds `raw` unchanged — and
    /// re-arm. Enabling a watcher resets its strikes (40 §5).
    async fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        let e = self.entries.get_mut(name).ok_or("no such schedule")?;
        let raw = file::set_enabled(&e.raw, enabled);
        file::write_atomic(&self.dir, name, &raw).map_err(|err| err.to_string())?;
        e.spec = file::parse(name, &raw);
        e.raw = raw;
        e.next = next_of(&e.spec, Local::now(), e.last);
        if enabled
            && e.spec.as_ref().is_ok_and(|s| s.sensor.is_some())
            && let Err(err) = self.store.watch_reset_strikes(name).await
        {
            tracing::error!(%name, "strikes not reset: {err:#}");
        }
        Ok(())
    }

    /// `BindSession` (40 §1.12): the watcher's session, created now if it has
    /// none, titled `<name> · <subject>`; idempotent.
    async fn bind_session(
        &mut self,
        name: &str,
        subject: Option<&str>,
    ) -> Result<SessionId, String> {
        let e = self.entries.get(name).ok_or("no such schedule")?;
        let spec = e.spec.as_ref().map_err(Clone::clone)?;
        if spec.sensor.is_none() {
            return Err("not a watcher".into());
        }
        if let Some(s) = e.session {
            return Ok(s);
        }
        let (cwd, model) = (spec.cwd.clone(), spec.model.clone());
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or("agent is not running")?;
        let s = new_session(&agents, name, cwd, model, subject).await?;
        if let Err(err) = self.store.watch_bind(name, s).await {
            tracing::error!(%name, "bound session not recorded: {err:#}");
        }
        if let Some(e) = self.entries.get_mut(name) {
            e.session = Some(s);
        }
        Ok(s)
    }
}

impl Message<agent::AgentEvent> for Schedule {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, ctx: &mut Context<Self, Self::Reply>) {
        use agent::AgentEvent as E;
        match ev {
            E::TextDelta { session, text, .. } => {
                if let Some(r) = self.running.get_mut(&session) {
                    r.text.push_str(&text);
                }
            }
            E::ToolCallStarted { session, name, .. } if name == "message_user" => {
                if let Some(r) = self.running.get_mut(&session) {
                    r.messaged = true;
                }
            }
            E::Error {
                session, message, ..
            } => {
                if let Some(r) = self.running.get_mut(&session) {
                    r.last_error = Some(message);
                }
            }
            // 33 §3.4: grandchildren count.
            E::SubagentSpawned { parent, child, .. } => {
                if let Some(r) = self.running.get_mut(&parent) {
                    r.children.insert(child);
                    r.pending.insert(child);
                } else if let Some(r) = self
                    .running
                    .values_mut()
                    .find(|r| r.children.contains(&parent))
                {
                    r.children.insert(child);
                }
            }
            E::TurnStarted { session, .. } => {
                if let Some(r) = self.running.get_mut(&session) {
                    r.turn_open = true;
                }
            }
            E::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                // A child's turn: the parent's wake follows (33 §3.4).
                let Some(run) = self.running.get_mut(&session) else {
                    return;
                };
                run.turn_open = false;
                run.last_stop = stop_reason;
                // A plain run closes now. A timed-out one too: its subtree was
                // cancelled with it. One that fanned out settles after a grace
                // window, because a child's result queued during this turn
                // starts the next turn inside the agent's `TurnDone` - the
                // `TurnStarted` lands here milliseconds after this event.
                if run.timed_out || run.children.is_empty() {
                    self.close(session, ctx).await;
                    return;
                }
                let me = ctx.actor_ref().clone();
                tokio::spawn(async move {
                    tokio::time::sleep(SETTLE).await;
                    let _ = me.tell(Settle { session }).await;
                });
            }
            _ => {}
        }
    }
}

/// A peer body landed in the run's session (50 §10): a direct child's result
/// clears it from `pending`; the turn it starts (or extends) is the run's.
/// `MessageDelivered` no longer fires for peer bodies, only for jobs and
/// reminders, which never hold a run open.
impl Message<agent::PeerDelivered> for Schedule {
    type Reply = ();

    async fn handle(&mut self, ev: agent::PeerDelivered, _: &mut Context<Self, Self::Reply>) {
        if let Some(r) = self.running.get_mut(&ev.session)
            && let Some(child) = ev
                .origin
                .from
                .strip_prefix("session:")
                .and_then(|id| id.parse::<SessionId>().ok())
        {
            r.pending.remove(&child);
        }
    }
}

/// Internal `tell`, [`SETTLE`] after a `TurnFinished` of a run that fanned out:
/// close it unless a turn started meanwhile, a child's result is still
/// outstanding, or a descendant is live (33 §3.4).
#[derive(Clone, Copy, Debug)]
struct Settle {
    session: SessionId,
}

impl Message<Settle> for Schedule {
    type Reply = ();

    async fn handle(&mut self, Settle { session }: Settle, ctx: &mut Context<Self, Self::Reply>) {
        let Some(run) = self.running.get(&session) else {
            return;
        };
        if run.turn_open || !run.pending.is_empty() {
            tracing::debug!(name = %run.name, turn_open = run.turn_open, pending = ?run.pending, "run stays open");
            return;
        }
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && !live_children(&agents, session).await.is_empty()
        {
            tracing::debug!(name = %run.name, "live subagents; run stays open");
            return;
        }
        self.close(session, ctx).await;
    }
}

impl Schedule {
    /// Close a run with its last `stop_reason`, then tick: an `every: 0s`
    /// loop is due again now and the tick loop may be asleep for up to TICK.
    async fn close(&mut self, session: SessionId, ctx: &mut Context<Self, ()>) {
        let Some(run) = self.running.remove(&session) else {
            return;
        };
        let stop = run.last_stop.clone();
        self.finished(session, run, &stop).await;
        let _ = ctx.actor_ref().tell(Tick).await;
    }
}

impl Schedule {
    /// Close the run row with its report, remember the result, deliver the
    /// reply (34 §4.1, 33 §3.5).
    async fn finished(&mut self, session: SessionId, run: Run, stop_reason: &str) {
        let kind = if run.timed_out {
            "timeout"
        } else {
            stop_reason
        };
        let result = match kind {
            "error" => format!("error: {}", run.last_error.as_deref().unwrap_or_default()),
            k => k.to_owned(),
        };
        let report = (!run.text.trim().is_empty()).then(|| tail_2k(&run.text));
        if let Err(e) = self
            .store
            .finish_run(run.id, Local::now(), &result, report)
            .await
        {
            tracing::error!(name = %run.name, "run not closed: {e:#}");
        }
        if let Some(e) = self.entries.get_mut(&run.name) {
            e.last_result = Some(result.clone());
        }

        let name = run.name;
        let report = match kind {
            "error" | "timeout" => {
                let detail = match (run.last_error, run.text.trim()) {
                    (Some(e), _) => e,
                    (None, "") => "no output".to_owned(),
                    (None, t) => t.to_owned(),
                };
                Some(format!("schedule {name} {kind}: {detail}"))
            }
            // A human did that.
            "cancelled" => None,
            _ if !run.quiet && !run.messaged && !run.text.trim().is_empty() => Some(run.text),
            // Quiet, or `message_user` already spoke.
            _ => None,
        };
        if let Some(text) = report {
            deliver(Some(session), name.clone(), text);
        }

        // 40 §1.7, §5: a watcher's run commits its check when it succeeds; `error` /
        // `timeout` keep the old state and strike; `cancelled` is a human's (or a restart).
        let Some(w) = run.watch else { return };
        if run_succeeded(kind) {
            match self
                .store
                .watch_commit(&name, &w.state, &w.hash, run.fired_at)
                .await
            {
                Ok(()) => tracing::info!(%name, hash = %w.hash, "watch committed"),
                Err(e) => tracing::error!(%name, "watch not committed: {e:#}"),
            }
            if let Some(p) = self.pending.remove(&name) {
                self.deliver_pending(&name, p).await;
            }
        } else if matches!(kind, "error" | "timeout") {
            self.strike(&name, Local::now(), &result).await;
        } else {
            tracing::info!(%name, "watch not committed: run {kind}");
        }
    }

    /// [`ScheduleInfo`] for one entry; `runs` asks the store when the file
    /// has a stop (33 §3.2). A `next` in the past is stale — a non-hub never
    /// fires, so its entries keep the `next` they were armed with — and is
    /// recomputed from the clause as at now (`None` stays: ended, disabled,
    /// one-shot done).
    async fn info(&self, name: &str, e: &Entry) -> ScheduleInfo {
        let now = Local::now();
        let next = match e.next {
            Some(t) if t < now => next_of(&e.spec, now, None),
            other => other,
        };
        let spec = e.spec.as_ref().ok();
        let is_loop = spec.is_some_and(|s| s.until.is_some() || s.max_runs.is_some());
        let sensor = spec.and_then(|s| s.sensor.as_ref());
        let (spec_fire, spec_mode, spec_memory) =
            spec.map_or_else(Default::default, |s| (s.fire, s.mode, s.memory));
        let last_check = match sensor {
            None => None,
            Some(_) => self
                .store
                .watch_get(name)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(%name, "watch row unreadable: {e:#}");
                    None
                })
                .and_then(|w| {
                    Some(LastCheck {
                        at: w.last_check_at?,
                        outcome: w.last_outcome.unwrap_or_default(),
                        elapsed_ms: w.last_elapsed_ms.unwrap_or_default(),
                        hash: w.last_hash,
                    })
                }),
        };
        ScheduleInfo {
            name: name.to_owned(),
            when: spec.map_or_else(|| "—".to_owned(), |s| s.when_text.clone()),
            describe: spec.map_or_else(|| "—".to_owned(), file::Spec::describe),
            enabled: spec.is_some_and(|s| s.enabled),
            quiet: spec.is_some_and(|s| s.quiet),
            next,
            last: e.last,
            last_result: e.last_result.clone(),
            session: e.session,
            running: self.is_running(e),
            error: e.spec.as_ref().err().cloned(),
            until: spec.and_then(|s| s.until.clone()),
            max_runs: spec.and_then(|s| s.max_runs),
            runs: if is_loop { self.runs_of(name).await } else { 0 },
            approvals: spec.is_none_or(|s| s.approvals),
            cwd: spec.and_then(|s| s.cwd.clone()),
            agent: spec.and_then(|s| s.agent.clone()),
            project: spec.and_then(|s| s.project.clone()),
            when_sensor: sensor.map(file::Sensor::render),
            sensor: sensor.map(|s| s.file.clone()),
            fire: sensor.map(|_| spec_fire.to_string()),
            for_: sensor.and(spec).map(|s| dur(s.for_)),
            cooldown: sensor.and(spec).map(|s| dur(s.cooldown)),
            mode: sensor.map(|_| spec_mode.to_string()),
            memory: sensor.map(|_| spec_memory.to_string()),
            expires: spec.and_then(|s| s.expires).map(dur),
            timeout: sensor.and(spec).map(|s| dur(s.timeout)),
            model: spec.and_then(|s| s.model.clone()),
            run_timeout: spec.and_then(|s| s.run_timeout).map(dur),
            created: spec.and_then(|s| s.created),
            last_check,
        }
    }
}

/// A duration as the file writes it (`5m`, `14d`, `0s`).
fn dur(d: Duration) -> String {
    humantime::format_duration(d).to_string()
}

impl Message<ListSensors> for Schedule {
    type Reply = Vec<SensorInfo>;

    async fn handle(&mut self, _: ListSensors, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        list_sensors(&self.sensors)
    }
}

/// 108 §9.2: every `*.py` in the sensors directory (non-recursive, dotfiles
/// ignored), sorted by name, resolved; a header error fills `error`.
pub fn list_sensors(sensors: &agent::sensor::SensorRunner) -> Vec<SensorInfo> {
    let cpython_available = agent::sensor::find_cpython().is_some();
    let mut names: Vec<String> = std::fs::read_dir(sensors.sensors_dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.ends_with(".py") && !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let shipped_version = agent::sensor::SensorRunner::shipped_version(&name);
            let mut info = SensorInfo {
                description: None,
                usage: None,
                args: Vec::new(),
                version: None,
                builtin: shipped_version.is_some(),
                edited: sensors.edited(&name),
                shipped_version: shipped_version.map(str::to_owned),
                needs_cpython: false,
                cpython_available,
                hosts: Vec::new(),
                has_probe: false,
                error: None,
                name,
            };
            match sensors.resolve(&info.name) {
                Ok(m) => {
                    info.description = m.description;
                    info.usage = m.usage;
                    info.args = m.args;
                    info.version = m.version;
                    info.needs_cpython = m.needs_cpython;
                    info.hosts = m.hosts;
                    info.has_probe = m.has_probe;
                }
                Err(agent::sensor::SensorError::NeedsCpython) => info.needs_cpython = true,
                Err(e) => info.error = Some(e.to_string()),
            }
            info
        })
        .collect()
}

impl Message<TestFile> for Schedule {
    type Reply = Result<Verdict, String>;

    async fn handle(&mut self, m: TestFile, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        validate::test_file(&m.0, &self.sensors).await
    }
}

impl Message<ListSchedules> for Schedule {
    type Reply = Vec<ScheduleInfo>;

    async fn handle(
        &mut self,
        _: ListSchedules,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut out = Vec::with_capacity(self.entries.len());
        for (name, e) in &self.entries {
            out.push(self.info(name, e).await);
        }
        out
    }
}

impl Message<Fire> for Schedule {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: Fire, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let e = self.entries.get(&m.name).ok_or("no such schedule")?;
        let spec = e.spec.as_ref().map_err(Clone::clone)?;
        if !spec.enabled {
            return Err("disabled".to_owned());
        }
        // 40 §3.2: `/schedules run` on a watcher means *run* — the sensor supplies
        // the payload, its verdict is ignored, state commits as usual.
        let when = match spec.sensor.clone() {
            None => None,
            Some(sensor) => {
                let spec = spec.clone();
                let state = self.state_of(&m.name).await;
                let done = self
                    .check_future(&spec, &sensor, state)
                    .map_err(|e| format!("check broken: {e}"))?
                    .await;
                let r = done.result.map_err(|e| format!("check broken: {e}"))?;
                Some(WhenFire::new(&r, 0))
            }
        };
        // Not gated on the hub (67 §4.4): *run now* is the user's ask on this engine.
        match self.fire(&m.name, Local::now(), true, when).await? {
            Fired::Session(s) => Ok(s),
            // No session to open: the reply names the task instead (67 §7.4).
            Fired::Task(id) => Err(format!("created task {id}")),
        }
    }
}

impl Message<GetRunTimeout> for Schedule {
    type Reply = u64;

    async fn handle(&mut self, _: GetRunTimeout, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.timeout.as_secs()
    }
}

impl Message<SetRunTimeout> for Schedule {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetRunTimeout,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if m.seconds < 30 {
            return Err("run_timeout: below 30s".into());
        }
        self.timeout = Duration::from_secs(m.seconds);
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(
            &path,
            "schedule",
            "run_timeout_secs",
            &m.seconds.to_string(),
        ) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<History> for Schedule {
    type Reply = Vec<RunInfo>;

    async fn handle(&mut self, m: History, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .history(&m.name, m.limit)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(name = %m.name, "history unreadable: {e:#}");
                Vec::new()
            })
    }
}

impl Message<SetEnabled> for Schedule {
    type Reply = Result<ScheduleInfo, String>;

    async fn handle(&mut self, m: SetEnabled, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.set_enabled(&m.name, m.enabled).await?;
        let e = self.entries.get(&m.name).ok_or("no such schedule")?;
        let info = self.info(&m.name, e).await;
        self.rt.publish(ScheduleEvent::Changed).await;
        Ok(info)
    }
}

impl Message<Get> for Schedule {
    type Reply = Result<file::ScheduleFile, String>;

    async fn handle(&mut self, m: Get, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let e = self.entries.get(&m.name).ok_or("no such schedule")?;
        file::ScheduleFile::read(&m.name, &e.raw)
    }
}

impl Message<Put> for Schedule {
    type Reply = Result<ScheduleInfo, String>;

    async fn handle(&mut self, m: Put, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let name = m.0.name.clone();
        if !file::valid_name(&name) {
            return Err("name: [a-z0-9][a-z0-9-]*, at most 40 chars".into());
        }
        let text = file::render(&m.0.args());
        file::parse(&name, &text)?;
        file::write_atomic(&self.dir, &name, &text).map_err(|e| e.to_string())?;
        self.reconcile(ctx.actor_ref()).await;
        let e = self.entries.get(&name).ok_or("no such schedule")?;
        Ok(self.info(&name, e).await)
    }
}

impl Message<Delete> for Schedule {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Delete, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.entries.contains_key(&m.name) {
            return Err("no such schedule".into());
        }
        std::fs::remove_file(self.dir.join(format!("{}.md", m.name))).map_err(|e| e.to_string())?;
        self.reconcile(ctx.actor_ref()).await;
        Ok(())
    }
}

impl Message<Done> for Schedule {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Done, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let name = self
            .running
            .iter()
            .find(|(s, r)| **s == m.session || r.children.contains(&m.session))
            .map(|(_, r)| r.name.clone())
            .ok_or("not a schedule run")?;
        self.end(&name, format!("done (model): {}", m.why)).await;
        Ok(format!("ended {name}"))
    }
}

impl Message<TestSchedule> for Schedule {
    type Reply = Result<Verdict, String>;

    async fn handle(&mut self, m: TestSchedule, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let e = self.entries.get(&m.name).ok_or("no such schedule")?;
        let spec = e.spec.as_ref().map_err(Clone::clone)?;
        if spec.sensor.is_none() {
            return Err("not a watcher".into());
        }
        // 40 §1.12, §6: the verdict on demand. `would_fire` compares against the
        // last fired hash, as the gate would.
        let last_hash = match self.store.watch_get(&m.name).await {
            Ok(w) => w.and_then(|w| w.last_hash),
            Err(e) => {
                tracing::error!(name = %m.name, "watch row unreadable: {e:#}");
                None
            }
        };
        let verdict = validate::dry_run(spec, &self.sensors, last_hash.as_deref()).await?;
        let outcome = format!(
            "test: {}",
            if verdict.would_fire {
                "would fire"
            } else {
                "quiet"
            }
        );
        if let Err(e) = self
            .store
            .watch_last_check(&m.name, Local::now(), &outcome, verdict.elapsed_ms)
            .await
        {
            tracing::error!(name = %m.name, "test not recorded: {e:#}");
        }
        Ok(verdict)
    }
}

impl Message<BindSession> for Schedule {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: BindSession, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.bind_session(&m.name, m.subject.as_deref()).await
    }
}

/// The plugin (34 §5): reads `schedule`, opens the store, spawns [`Schedule`]
/// as [`SCHEDULE_ACTOR`], subscribes it to [`agent::AgentEvent`], starts the
/// watcher and the tick loop, and registers the `schedule` tool once the
/// registry is up.
pub struct SchedulePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SchedulePlugin {
    fn name(&self) -> &'static str {
        "schedule"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: ScheduleConfig = rt.section("schedule").unwrap_or_else(|e| {
            tracing::debug!("no [schedule] config section, using defaults: {e}");
            ScheduleConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("schedule disabled by config");
            return Ok(());
        }
        let dir = rt.data_dir.join(DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("schedule unavailable: {}: {e}", dir.display());
            return Ok(());
        }
        // 67 §5.1: the directory is a replicated table; a hand edit anywhere lands everywhere.
        tokio::spawn(replica::files::mirror_at(
            rt.data_dir.join(DB),
            dir.clone(),
            DIR,
            std::time::Duration::from_secs(120),
        ));
        // 109 §4.6 step 12: a run whose session has an open turn stays open
        // and re-attaches to the resumed turn's `TurnFinished`; `agent`
        // started before us, so its answer is final. 139 §5.1 item 5: only
        // sessions this engine will resume — one owned elsewhere is not
        // this engine's run to keep open.
        let open_sessions: HashSet<SessionId> =
            match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
                Ok(Some(agents)) => agents
                    .ask(agent::supervisor::OpenTurns(agent::store::Scope::Here))
                    .await
                    .map(|open| open.into_iter().map(|(s, _, _)| s).collect())
                    .unwrap_or_default(),
                _ => HashSet::new(),
            };
        let (store, spared) =
            match store::Store::open_sparing(&rt.data_dir.join(DB), &open_sessions).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("schedule unavailable: {e:#}");
                    return Ok(());
                }
            };
        let timeout = Duration::from_secs(cfg.run_timeout_secs);
        // 40 §4.2: the sensor-profile pool; the worker is this binary (`t subprocess`,
        // 08 §9) as for the main runtime. Without it Monty sensors are `Unavailable`.
        let profile = agent::sensor::SensorProfile::default();
        let pool = match std::env::current_exe() {
            Ok(exe) => agent::repl::open_pool(&exe, profile.max_concurrent).await,
            Err(e) => {
                tracing::warn!("cannot locate own executable ({e}); sensor runtime disabled");
                None
            }
        };
        let sensors = Arc::new(agent::sensor::SensorRunner::new(
            rt.data_dir.clone(),
            pool,
            None,
            profile,
        ));
        if let Err(e) = sensors.install_builtins() {
            tracing::warn!("sensors directory not created: {e}");
        }
        let mut schedule =
            Schedule::load(rt.clone(), dir.clone(), store, timeout, sensors.clone()).await;
        schedule.reattach(spared);
        let actor = Schedule::spawn_with_mailbox(schedule, runtime::subscriber_mailbox());
        actor.register(SCHEDULE_ACTOR)?;
        rt.subscribe::<Schedule, agent::AgentEvent>(&actor).await;
        rt.subscribe::<Schedule, agent::PeerDelivered>(&actor).await;

        // Watcher (34 §2.3): the `tools::mcp` idiom.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })
        .and_then(|mut w| {
            w.watch(&dir, notify::RecursiveMode::NonRecursive)
                .map(|()| w)
        });
        match watcher {
            Ok(watcher) => {
                let me = actor.clone();
                tokio::spawn(async move {
                    let _watcher = watcher;
                    while rx.recv().await.is_some() {
                        // ponytail: fixed 300ms debounce; editors save in bursts.
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        while rx.try_recv().is_ok() {}
                        if me.tell(Reconcile).await.is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => tracing::warn!("schedule directory not watched: {e}; edits need a restart"),
        }

        // Tick loop (34 §3): sleep until the earliest `next` or deadline, at
        // most TICK, at least 1 s so a fire that keeps failing does not spin.
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            loop {
                let Ok(wake) = me.ask(NextWake).await else {
                    break;
                };
                let wake = wake.unwrap_or(TICK).clamp(Duration::from_secs(1), TICK);
                tokio::select! {
                    () = rt2.shutdown.cancelled() => break,
                    () = tokio::time::sleep(wake) => {}
                }
                if me.tell(Tick).await.is_err() {
                    break;
                }
            }
        });

        let rt2 = rt.clone();
        let sensors2 = sensors.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg, dir, sensors2).await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod fire_tests {
    use std::sync::Mutex;

    use super::*;

    /// One `create_for_schedule` call: `(schedule, fire_ms, agent, prompt, project, cwd)`.
    pub(super) type Call = (String, u64, String, String, Option<String>, Option<String>);

    /// Records every call; answers a fixed task id.
    #[derive(Default)]
    pub(super) struct Recorder {
        pub(super) calls: Mutex<Vec<Call>>,
        closed: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl TaskCreator for Recorder {
        async fn create_for_schedule(
            &self,
            schedule: &str,
            fire_ms: u64,
            agent: &str,
            prompt: &str,
            project: Option<&str>,
            cwd: Option<&str>,
        ) -> anyhow::Result<String> {
            self.calls.lock().unwrap().push((
                schedule.to_owned(),
                fire_ms,
                agent.to_owned(),
                prompt.to_owned(),
                project.map(str::to_owned),
                cwd.map(str::to_owned),
            ));
            Ok("task-1".to_owned())
        }
        async fn close_for_schedule(&self, schedule: &str, _: &str) -> anyhow::Result<()> {
            self.closed.lock().unwrap().push(schedule.to_owned());
            Ok(())
        }
    }

    pub(super) async fn actor(dir: &Path, files: &[(&str, &str)]) -> (Schedule, Arc<Recorder>) {
        let sched = dir.join(DIR);
        std::fs::create_dir_all(&sched).unwrap();
        for (name, text) in files {
            std::fs::write(sched.join(format!("{name}.md")), text).unwrap();
        }
        let store = store::Store::open(&dir.join(DB)).await.unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let sensors = Arc::new(agent::sensor::SensorRunner::new(
            dir.to_path_buf(),
            None,
            None,
            agent::sensor::SensorProfile::default(),
        ));
        let mut s = Schedule::load(rt, sched, store, Duration::from_secs(60), sensors).await;
        let rec = Arc::new(Recorder::default());
        s.task_creator = Some(rec.clone());
        (s, rec)
    }

    #[tokio::test]
    async fn agent_fire_creates_a_task_not_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[(
                "nightly",
                "---\nevery: 1h\nagent: ops\nproject: argo\n---\ncheck the deploy\n",
            )],
        )
        .await;
        let now = Local::now();
        let Fired::Task(id) = s.fire("nightly", now, true, None).await.unwrap() else {
            panic!("expected a task")
        };
        assert_eq!(id, "task-1");
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (name, fire_ms, agent, prompt, project, cwd) = &calls[0];
        assert_eq!((name.as_str(), agent.as_str()), ("nightly", "ops"));
        assert_eq!(*fire_ms, now.timestamp_millis() as u64);
        assert!(
            prompt.starts_with("<schedule name=\"nightly\"") && prompt.contains("check the deploy")
        );
        assert_eq!((project.as_deref(), cwd.as_deref()), (Some("argo"), None));
        // the run row carries the task, and counts toward the loop cap
        let h = s.store.history("nightly", 5).await.unwrap();
        assert_eq!(h[0].result.as_deref(), Some("task task-1"));
        assert!(h[0].session.is_none());
        assert_eq!(s.runs_of("nightly").await, 1);
        let e = &s.entries["nightly"];
        assert!(e.session.is_none() && e.last.is_some() && e.next.is_some());
        // the same instant fires again with the same key (tasks coalesces on it)
        let Fired::Task(_) = s.fire("nightly", now, true, None).await.unwrap() else {
            panic!()
        };
        assert_eq!(
            rec.calls.lock().unwrap()[1].1,
            now.timestamp_millis() as u64
        );
    }

    #[tokio::test]
    async fn max_runs_stops_new_tasks_and_until_closes_them() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[("once", "---\nevery: 1h\nagent: ops\nmax_runs: 1\n---\ngo\n")],
        )
        .await;
        s.fire("once", Local::now(), false, None).await.unwrap();
        let e = s.fire("once", Local::now(), false, None).await.unwrap_err();
        assert!(e.starts_with("max_runs 1 reached"), "{e}");
        assert_eq!(rec.calls.lock().unwrap().len(), 1, "no task past the cap");
        assert!(
            rec.closed.lock().unwrap().is_empty(),
            "a cap is not `until` passing"
        );
        // `until` passing ends the loop and closes the agent's tasks
        let (mut s, rec) = actor(
            dir.path(),
            &[(
                "loop",
                "---\nevery: 1h\nagent: ops\nuntil: \"true\"\n---\ngo\n",
            )],
        )
        .await;
        let e = s.fire("loop", Local::now(), false, None).await.unwrap_err();
        assert!(e.starts_with("done: until passed"), "{e}");
        assert_eq!(rec.closed.lock().unwrap().as_slice(), ["loop"]);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plain_schedule_path_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(dir.path(), &[("plain", "---\nevery: 1h\n---\ngo\n")]).await;
        // No agent actor in a unit test: the session path is the one taken, and it says so.
        assert_eq!(
            s.fire("plain", Local::now(), true, None).await.unwrap_err(),
            "agent is not running"
        );
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    /// 88: `run_timeout:` caps one run instead of the schedule's default (60s here).
    #[tokio::test]
    async fn run_timeout_key_sets_the_run_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = actor(
            dir.path(),
            &[
                ("short", "---\nevery: 1h\nrun_timeout: 30s\n---\ngo\n"),
                ("plain", "---\nevery: 1h\n---\ngo\n"),
            ],
        )
        .await;
        let override_of = |name: &str| s.entries[name].spec.as_ref().unwrap().run_timeout;
        // Two `Instant::now()` reads may land in the same clock tick: allow the exact value.
        let secs_left = |d: Instant| d.saturating_duration_since(Instant::now()).as_secs();
        assert_eq!(override_of("short"), Some(Duration::from_secs(30)));
        assert!((29..=30).contains(&secs_left(s.deadline_for(override_of("short")))));
        assert_eq!(override_of("plain"), None);
        assert!((59..=60).contains(&secs_left(s.deadline_for(override_of("plain")))));
    }

    /// 88: the global default is live for new runs and persisted to config.yaml.
    #[tokio::test]
    async fn set_run_timeout_is_live_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = actor(dir.path(), &[]).await;
        let actor = Schedule::spawn(s);
        assert_eq!(actor.ask(GetRunTimeout).await.unwrap(), 60);
        // A handler `Err` is lifted into `SendError::HandlerError`.
        use kameo::error::SendError;
        assert!(matches!(
            actor.ask(SetRunTimeout { seconds: 29 }).await,
            Err(SendError::HandlerError(e)) if e == "run_timeout: below 30s"
        ));
        assert_eq!(actor.ask(GetRunTimeout).await.unwrap(), 60);
        actor.ask(SetRunTimeout { seconds: 90 }).await.unwrap();
        assert_eq!(actor.ask(GetRunTimeout).await.unwrap(), 90);
        let cfg = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert_eq!(cfg, "schedule:\n  run_timeout_secs: 90\n");
    }

    /// 131 D1: the runtime is reset exactly when the fire clears the frame
    /// and the file does not say `runtime: keep`.
    #[tokio::test]
    async fn resets_runtime_follows_the_clear_and_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = actor(
            dir.path(),
            &[
                ("plain", "---\nevery: 1h\n---\ngo\n"),
                ("keep", "---\nevery: 1h\nruntime: keep\n---\ngo\n"),
                (
                    "watch",
                    "---\nevery: 60s\nwhen: a.py\nmax_runs: 1\n---\ngo\n",
                ),
                (
                    "watch-clear",
                    "---\nevery: 60s\nwhen: a.py\nmax_runs: 1\nmemory: clear\n---\ngo\n",
                ),
                (
                    "watch-clear-keep",
                    "---\nevery: 60s\nwhen: a.py\nmax_runs: 1\nmemory: clear\nruntime: keep\n---\ngo\n",
                ),
            ],
        )
        .await;
        let resets = |name: &str| resets_runtime(s.entries[name].spec.as_ref().unwrap());
        assert!(resets("plain"));
        assert!(!resets("keep"));
        assert!(!resets("watch"), "memory: continue keeps frame and runtime");
        assert!(resets("watch-clear"));
        assert!(!resets("watch-clear-keep"));
    }
}

#[cfg(test)]
mod hub_tests {
    use tokio::sync::Mutex;

    use super::fire_tests::actor;
    use super::*;

    /// The hub flag is process-global: these run one at a time and leave it true.
    static HUB: Mutex<()> = Mutex::const_new(());

    async fn hub_lock() -> tokio::sync::MutexGuard<'static, ()> {
        HUB.lock().await
    }

    const DUE: &str = "---\nevery: 1h\nagent: ops\n---\ngo\n";

    #[tokio::test]
    async fn single_engine_default_is_hub() {
        let _g = hub_lock().await;
        assert!(mesh::is_hub());
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(dir.path(), &[("n", DUE)]).await;
        assert!(s.hub);
        s.tick(Local::now()).await;
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn non_hub_ticker_skips_and_does_not_advance_last() {
        let _g = hub_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(dir.path(), &[("n", DUE)]).await;
        mesh::set_hub(false);
        let now = Local::now();
        s.tick(now).await;
        s.tick(now + chrono::Duration::hours(3)).await;
        mesh::set_hub(true);
        assert!(rec.calls.lock().unwrap().is_empty(), "non-hub fired");
        let e = &s.entries["n"];
        assert!(e.last.is_none() && e.next.is_some_and(|t| t <= now));
        assert!(!s.hub);
        assert!(s.store.history("n", 5).await.unwrap().is_empty());
    }

    /// FIXBATCH E: a non-hub's entries keep the `next` they were armed with (it never fires);
    /// `ListSchedules` recomputes a past `next` from the clause, and leaves `None` alone.
    #[tokio::test]
    async fn list_recomputes_a_stale_next_on_a_non_hub() {
        let _g = hub_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let cron = "---\ncron: 0 9 * * *\nagent: ops\n---\ngo\n";
        let (mut s, _rec) = actor(dir.path(), &[("c", cron), ("e", DUE), ("ended", cron)]).await;
        mesh::set_hub(false);
        let now = Local::now();
        let yesterday = now - chrono::Duration::days(1);
        s.entries.get_mut("c").unwrap().next = Some(yesterday);
        s.entries.get_mut("e").unwrap().next = Some(yesterday);
        s.entries.get_mut("ended").unwrap().next = None;
        s.tick(now).await;
        assert_eq!(
            s.entries["c"].next,
            Some(yesterday),
            "the non-hub does not re-arm"
        );
        let mut list = Vec::new();
        for (name, e) in &s.entries {
            list.push(s.info(name, e).await);
        }
        mesh::set_hub(true);
        let by = |n: &str| list.iter().find(|i| i.name == n).unwrap().next;
        assert!(
            by("c").is_some_and(|t| t > now),
            "cron: next occurrence after now, {:?}",
            by("c")
        );
        assert!(
            by("e").is_some_and(|t| t >= now),
            "every: now when the hub's last fire is unknown"
        );
        assert_eq!(by("ended"), None, "an ended entry stays ended");
    }

    #[tokio::test]
    async fn regaining_hub_applies_missed_policy() {
        let _g = hub_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[
                ("skip", "---\ncron: \"*/5 * * * *\"\nagent: ops\n---\ngo\n"),
                (
                    "run",
                    "---\ncron: \"*/5 * * * *\"\nagent: ops\nmissed: run\n---\ngo\n",
                ),
            ],
        )
        .await;
        // Both fired once as the hub, so `last` is set and the next occurrence is armed.
        let t0 = Local::now();
        s.fire("skip", t0, false, None).await.unwrap();
        s.fire("run", t0, false, None).await.unwrap();
        assert_eq!(rec.calls.lock().unwrap().len(), 2);
        // Lose the hub; a day passes with occurrences due; nothing fires.
        mesh::set_hub(false);
        let paused = t0 + chrono::Duration::hours(24);
        s.tick(paused).await;
        assert_eq!(rec.calls.lock().unwrap().len(), 2);
        // Regain it: `missed: run` fires for the pause, `missed: skip` waits for its next occurrence.
        mesh::set_hub(true);
        let back = paused + chrono::Duration::seconds(1);
        s.tick(back).await;
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(
            (calls[2].0.as_str(), calls[2].1),
            ("run", back.timestamp_millis() as u64)
        );
        assert!(s.entries["skip"].next.is_some_and(|t| t > back));
        assert_eq!(s.entries["skip"].last, Some(t0));
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use store::WatchRow;

    fn spec(extra: &str) -> file::Spec {
        file::parse(
            "w",
            &format!(
                "---\nevery: 30s\nwhen: fs_exists.py path=/tmp/x\nmax_runs: 3\n{extra}---\nbody\n"
            ),
        )
        .unwrap()
    }

    fn ok(fire: bool, payload: &str, next: Option<u64>) -> Result<SensorRun, SensorError> {
        Ok(SensorRun {
            fire,
            payload: payload.to_owned(),
            state: serde_json::json!({}),
            next,
            elapsed: Duration::from_millis(5),
            truncated: false,
        })
    }

    fn s(n: i64) -> chrono::TimeDelta {
        chrono::TimeDelta::seconds(n)
    }

    #[test]
    fn broken_quiet_and_next_clamp() {
        let spec = spec("");
        let now = Local::now();
        let mut w = WatchRow::default();
        let (o, next) = gate(
            &spec,
            &mut w,
            &Err(SensorError::Failed("boom".into())),
            now,
            false,
            None,
        );
        assert_eq!(o, CheckOutcome::Broken("boom".into()));
        assert_eq!(o.to_string(), "check broken: boom");
        assert_eq!(next, now + s(30), "broken: last + every");

        w.holding_since = Some(now - s(10));
        let (o, next) = gate(&spec, &mut w, &ok(false, "", Some(5)), now, false, None);
        assert_eq!(
            (o, next),
            (CheckOutcome::Quiet, now + s(30)),
            "next floors at every"
        );
        assert_eq!(w.holding_since, None, "a false tick clears first_true_at");
        let (_, next) = gate(&spec, &mut w, &ok(false, "", Some(3600)), now, false, None);
        assert_eq!(next, now + s(300), "next caps at 10 × every");
        let (_, next) = gate(&spec, &mut w, &ok(false, "", None), now, false, None);
        assert_eq!(next, now + s(30), "no next: every");
    }

    #[test]
    fn deferred_is_not_broken() {
        let spec = spec("for: 1m\n");
        let now = Local::now();
        let mut w = WatchRow {
            holding_since: Some(now - s(10)),
            strikes: 2,
            ..WatchRow::default()
        };
        let r = Err(SensorError::Budget("deferred".into()));
        let (o, next) = gate(&spec, &mut w, &r, now, false, None);
        assert_eq!(o, CheckOutcome::Deferred);
        assert_eq!(o.to_string(), "deferred");
        assert_eq!(next, now + s(30), "deferred: now + every");
        assert_eq!(
            (w.holding_since, w.strikes),
            (Some(now - s(10)), 2),
            "deferred touches neither first_true_at nor strikes"
        );
        let r = Err(SensorError::Budget("heap: boom".into()));
        let (o, _) = gate(&spec, &mut w, &r, now, false, None);
        assert_eq!(o, CheckOutcome::Broken("over budget: heap: boom".into()));
    }

    #[test]
    fn on_change_vs_while_true_vs_once() {
        let now = Local::now();
        let mut w = WatchRow {
            last_hash: Some(agent::sensor::payload_hash("events=x\nsame")),
            ..WatchRow::default()
        };
        let r = ok(true, "events=x\nsame", Some(60));
        let (o, next) = gate(&spec(""), &mut w, &r, now, false, None);
        assert_eq!((o, next), (CheckOutcome::Unchanged, now + s(60)));
        let (o, next) = gate(&spec("fire: while_true\n"), &mut w, &r, now, false, None);
        assert_eq!(
            (o, next),
            (CheckOutcome::Fired, now + s(30)),
            "fired: last + every"
        );
        let (o, _) = gate(&spec("fire: once\n"), &mut w, &r, now, false, None);
        assert_eq!(o, CheckOutcome::Fired);
        let (o, _) = gate(
            &spec(""),
            &mut w,
            &ok(true, "events=x\nother", None),
            now,
            false,
            None,
        );
        assert_eq!(o, CheckOutcome::Fired, "a new hash fires on_change");
    }

    #[test]
    fn holding_until_for_is_met() {
        let spec = spec("for: 1m\n");
        let t0 = Local::now();
        let mut w = WatchRow::default();
        let r = ok(true, "events=x\np", None);
        let (o, _) = gate(&spec, &mut w, &r, t0, false, None);
        assert_eq!(o.to_string(), "holding: 0s of 1m");
        assert_eq!(w.holding_since, Some(t0));
        let (o, _) = gate(&spec, &mut w, &r, t0 + s(30), false, None);
        assert_eq!(o.to_string(), "holding: 30s of 1m");
        let (o, _) = gate(&spec, &mut w, &r, t0 + s(60), false, None);
        assert_eq!(o, CheckOutcome::Fired);
        assert_eq!(
            w.holding_since,
            Some(t0),
            "the gate leaves clearing to the commit"
        );
    }

    #[test]
    fn cooldown_spaces_fires() {
        let spec = spec("fire: while_true\ncooldown: 2m\n");
        let now = Local::now();
        let mut w = WatchRow {
            last_fired_at: Some(now - s(30)),
            ..WatchRow::default()
        };
        let r = ok(true, "events=x\np", None);
        let (o, next) = gate(&spec, &mut w, &r, now, false, None);
        assert_eq!(o.to_string(), "cooldown: 1m 30s left");
        assert_eq!(next, now + s(90), "next: max(clamped, cooldown end)");
        let (o, _) = gate(&spec, &mut w, &r, now + s(90), false, None);
        assert_eq!(o, CheckOutcome::Fired);
    }

    #[test]
    fn busy_follows_mode() {
        let now = Local::now();
        let r = ok(true, "events=x\np", None);
        let mut w = WatchRow::default();
        let (o, _) = gate(&spec(""), &mut w, &r, now, true, None);
        assert_eq!(o.to_string(), "skipped: busy");
        let (o, _) = gate(&spec("mode: queue\n"), &mut w, &r, now, true, None);
        assert_eq!(
            (o.to_string().as_str(), o),
            ("queued", CheckOutcome::Queued { coalesced: 0 })
        );
        let (o, _) = gate(&spec("mode: queue\n"), &mut w, &r, now, true, Some(2));
        assert_eq!(o.to_string(), "queued (3 coalesced)");
        let (o, _) = gate(&spec("mode: restart\n"), &mut w, &r, now, true, None);
        assert_eq!(o, CheckOutcome::Restarted);
        let (o, _) = gate(&spec("mode: restart\n"), &mut w, &r, now, false, None);
        assert_eq!(o, CheckOutcome::Fired, "not busy: mode is moot");
    }

    #[test]
    fn when_block_copies_envelope_and_escapes() {
        let w = WhenFire {
            payload:
                "subject=pr/482 stage=waiting_ci events=ci_failed,new_commits head=9f3c\nline \"2\""
                    .into(),
            hash: String::new(),
            state: "{}".into(),
            coalesced: 2,
        };
        assert_eq!(
            w.block("pr_watch.py"),
            "<when name=\"pr_watch.py\" subject=\"pr/482\" stage=\"waiting_ci\" events=\"ci_failed,new_commits\" coalesced=\"2\">\n\
             subject=pr/482 stage=waiting_ci events=ci_failed,new_commits head=9f3c\nline \"2\"\n</when>"
        );
        assert_eq!(
            w.envelope(),
            "subject=pr/482 stage=waiting_ci events=ci_failed,new_commits head=9f3c"
        );
        assert_eq!(attr("a\"<b>&c"), "a&quot;&lt;b>&amp;c");
    }
}

#[cfg(test)]
mod watch_tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::fire_tests::{Recorder, actor};
    use super::*;

    fn s(n: i64) -> chrono::TimeDelta {
        chrono::TimeDelta::seconds(n)
    }

    /// 108 §9.2: every `*.py` in the sensors dir is a row, sorted; built-ins
    /// carry their header and shipped version; a bad header is `error`; a
    /// dotfile and a non-`.py` file are not listed.
    #[test]
    fn list_sensors_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let sensors = agent::sensor::SensorRunner::new(
            tmp.path().to_path_buf(),
            None,
            None,
            Default::default(),
        );
        sensors.install_builtins().unwrap();
        let dir = sensors.sensors_dir();
        std::fs::write(dir.join("bad.py"), "# arg: x\nasync def check(s, a): ...\n").unwrap();
        std::fs::write(
            dir.join("mine.py"),
            "\"\"\"Mine.\n\nwhen: mine.py k=<v>\n\"\"\"\nasync def check(s, a): ...\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let rows = list_sensors(&sensors);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["bad.py", "fs_exists.py", "mine.py", "pr_watch.py", "rss.py"]
        );
        let bad = &rows[0];
        assert!(
            bad.error.as_deref().is_some_and(|e| e.contains("`# arg:`")),
            "{bad:?}"
        );
        assert!(!bad.builtin && bad.shipped_version.is_none());
        let pr = &rows[3];
        assert!(pr.builtin && !pr.edited && pr.error.is_none());
        assert_eq!(
            (pr.version.as_deref(), pr.shipped_version.as_deref()),
            (Some("2"), Some("2"))
        );
        assert_eq!(pr.args.len(), 4);
        assert_eq!(pr.hosts, ["api.github.com"]);
        assert!(pr.has_probe);
        let mine = &rows[2];
        assert_eq!(mine.description.as_deref(), Some("Mine."));
        assert_eq!(mine.usage.as_deref(), Some("when: mine.py k=<v>"));
        assert!(mine.args.is_empty() && mine.version.is_none() && !mine.builtin);
        assert!(
            rows.iter()
                .all(|r| r.cpython_available == agent::sensor::find_cpython().is_some())
        );
    }

    fn run(
        fire: bool,
        payload: String,
        state: serde_json::Value,
    ) -> Result<SensorRun, SensorError> {
        Ok(SensorRun {
            fire,
            payload,
            state,
            next: None,
            elapsed: Duration::from_millis(3),
            truncated: false,
        })
    }

    /// The fake `fs_exists.py` (40 §9 step 2): fires iff `args["path"]` exists;
    /// payload `subject=<path> events=exists size=<bytes>` then the path — so a
    /// `touch` with the same content is `unchanged` and an append fires.
    fn fs_exists() -> FakeSensor {
        Arc::new(|args, _| {
            let p = &args["path"];
            match std::fs::metadata(p) {
                Ok(m) => run(
                    true,
                    format!("subject={p} events=exists size={}\n{p}", m.len()),
                    serde_json::json!({"seen": p}),
                ),
                Err(_) => run(false, String::new(), serde_json::json!({})),
            }
        })
    }

    /// A watcher file `w` with `agent: ops`, so a fire lands in the [`Recorder`]
    /// (no agent actor in a unit test) and a stub `sensors/fs_exists.py` so
    /// `resolve` is real; only `check` is faked.
    async fn watcher(dir: &Path, front: &str, fake: FakeSensor) -> (Schedule, Arc<Recorder>) {
        std::fs::create_dir_all(dir.join("sensors")).unwrap();
        std::fs::write(
            dir.join("sensors/fs_exists.py"),
            "# theuth-sensor: fs_exists 1\nasync def check(state, args):\n    ...\n",
        )
        .unwrap();
        let text = format!(
            "---\nevery: 30s\n{front}agent: ops\nmax_runs: 9\n---\ntell me when it appears\n"
        );
        let (mut sched, rec) = actor(dir, &[("w", &text)]).await;
        sched.fake_sensor = Some(fake);
        (sched, rec)
    }

    /// One tick as the actor runs it: start the check, await it, run the gate.
    async fn step(sched: &mut Schedule, now: DateTime<Local>) -> Option<CheckOutcome> {
        let mut checks = sched.tick(now).await;
        let done = checks.pop()?.await;
        sched.check_done(&done.name, done.result, now).await
    }

    fn in_flight(name: &str, at: DateTime<Local>, watch: Option<WhenFire>) -> Run {
        Run {
            id: 0,
            name: name.to_owned(),
            quiet: false,
            text: String::new(),
            messaged: false,
            last_error: None,
            deadline: Instant::now() + Duration::from_secs(60),
            timed_out: false,
            children: HashSet::new(),
            pending: HashSet::new(),
            turn_open: true,
            last_stop: String::new(),
            watch,
            fired_at: at,
        }
    }

    fn when(payload: &str) -> WhenFire {
        WhenFire {
            payload: payload.to_owned(),
            hash: agent::sensor::payload_hash(payload),
            state: "{\"n\":1}".to_owned(),
            coalesced: 0,
        }
    }

    async fn results(sched: &Schedule, n: u32) -> Vec<String> {
        sched
            .store
            .history("w", n)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.result.unwrap_or_default())
            .collect()
    }

    /// 40 §9 step 3, headless: `every: 30s` + `fs_exists.py` on a scratch data dir.
    #[tokio::test]
    async fn fs_exists_quiet_fire_unchanged_fire_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("theuth-x");
        let front = format!("when: fs_exists.py path={}\n", target.display());
        let (mut sched, rec) = watcher(dir.path(), &front, fs_exists()).await;
        let t0 = Local::now();

        // absent → quiet, a row with no session, the tick re-armed at every
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Quiet));
        let h = sched.store.history("w", 1).await.unwrap();
        assert_eq!(
            (h[0].result.as_deref(), h[0].session),
            (Some("quiet"), None)
        );
        assert_eq!(sched.entries["w"].next, Some(t0 + s(30)));
        assert_eq!(sched.entries["w"].last_result.as_deref(), Some("quiet"));
        assert_eq!(step(&mut sched, t0 + s(10)).await, None, "not due yet");
        assert_eq!(
            sched.runs_of("w").await,
            0,
            "quiet rows do not count toward max_runs"
        );

        // created → one run whose input has the <when> block
        std::fs::write(&target, "").unwrap();
        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Fired)
        );
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let prompt = &calls[0].3;
        let path = target.display().to_string();
        let block = format!(
            "tell me when it appears\n\n<when name=\"fs_exists.py\" subject=\"{path}\" events=\"exists\" coalesced=\"0\">\nsubject={path} events=exists size=0\n{path}\n</when>\n</schedule>"
        );
        assert!(prompt.ends_with(&block), "{prompt}");
        let h = sched.store.history("w", 1).await.unwrap();
        assert_eq!(h[0].result.as_deref(), Some("task task-1"));
        assert_eq!(
            h[0].envelope.as_deref(),
            Some(format!("subject={path} events=exists size=0").as_str())
        );
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.fired_count, w.strikes, w.last_outcome.as_deref()),
            (1, 0, Some("fired"))
        );
        assert_eq!(
            w.state,
            serde_json::json!({"seen": path}).to_string(),
            "committed with the task"
        );
        assert!(w.last_hash.is_some());
        assert_eq!(sched.entries["w"].next, Some(t0 + s(60)));

        // touched again, same content → unchanged
        std::fs::write(&target, "").unwrap();
        assert_eq!(
            step(&mut sched, t0 + s(60)).await,
            Some(CheckOutcome::Unchanged)
        );
        assert_eq!(rec.calls.lock().unwrap().len(), 1);

        // appended → fired; removed → quiet
        std::fs::write(&target, "x\n").unwrap();
        assert_eq!(
            step(&mut sched, t0 + s(90)).await,
            Some(CheckOutcome::Fired)
        );
        assert_eq!(rec.calls.lock().unwrap().len(), 2);
        std::fs::remove_file(&target).unwrap();
        assert_eq!(
            step(&mut sched, t0 + s(120)).await,
            Some(CheckOutcome::Quiet)
        );

        let rows = results(&sched, 10).await;
        assert_eq!(
            rows,
            ["quiet", "task task-1", "unchanged", "task task-1", "quiet"]
        );
        for r in sched.store.history("w", 10).await.unwrap().iter().rev() {
            println!(
                "{} · {} · {}",
                r.fired_at.format("%H:%M:%S"),
                r.result.as_deref().unwrap_or("running"),
                r.envelope.as_deref().unwrap_or("-")
            );
        }
        let info = sched.info("w", &sched.entries["w"]).await;
        let lc = info.last_check.unwrap();
        assert_eq!((lc.outcome.as_str(), lc.elapsed_ms), ("quiet", 3));
    }

    #[tokio::test]
    async fn queue_keeps_one_pending_and_fires_it_when_the_run_closes() {
        let dir = tempfile::tempdir().unwrap();
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        let fake: FakeSensor = Arc::new(move |_, _| {
            let i = n2.fetch_add(1, Ordering::SeqCst) + 1;
            run(
                true,
                format!("events=change n={i}\nbody {i}"),
                serde_json::json!({"n": i}),
            )
        });
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: while_true\nmode: queue\n",
            fake,
        )
        .await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        sched
            .running
            .insert(sid, in_flight("w", t0, Some(when("events=first\nfirst"))));

        assert_eq!(
            step(&mut sched, t0).await,
            Some(CheckOutcome::Queued { coalesced: 0 })
        );
        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Queued { coalesced: 1 })
        );
        assert_eq!(
            sched.pending["w"].payload, "events=change n=2\nbody 2",
            "newest wins"
        );
        assert_eq!(results(&sched, 2).await, ["queued (1 coalesced)", "queued"]);
        assert!(rec.calls.lock().unwrap().is_empty());

        // the run ends ok: its check commits, then the pending payload fires
        let run = sched.running.remove(&sid).unwrap();
        sched.finished(sid, run, "end_turn").await;
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].3.contains(
                "<when name=\"fs_exists.py\" events=\"change\" coalesced=\"1\">\nevents=change n=2"
            ),
            "{}",
            calls[0].3
        );
        assert!(sched.pending.is_empty());
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.fired_count, 2, "the closed run and the queued fire");
        assert_eq!(
            w.last_hash.as_deref(),
            Some(agent::sensor::payload_hash("events=change n=2\nbody 2").as_str())
        );
        assert_eq!(w.state, "{\"n\":2}");

        // skip mode while busy is a row and nothing else
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: while_true\n",
            fs_exists_true(),
        )
        .await;
        let t1 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        sched.running.insert(sid, in_flight("w", t1, None));
        assert_eq!(step(&mut sched, t1).await, Some(CheckOutcome::Busy));
        assert_eq!(results(&sched, 1).await, ["skipped: busy"]);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    fn fs_exists_true() -> FakeSensor {
        Arc::new(|_, _| run(true, "events=x\np".into(), serde_json::json!({})))
    }

    /// The live `pr-smoke-1` loop: an OpenAI Responses run ends `completed`,
    /// not `end_turn`; the commit must still happen so the next identical
    /// payload is `unchanged` (default `fire: on_change`).
    #[tokio::test]
    async fn completed_run_commits_and_the_same_payload_is_then_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) =
            watcher(dir.path(), "when: fs_exists.py path=/x\n", fs_exists_true()).await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        let run = in_flight("w", t0, Some(when("events=x\np")));
        sched.finished(sid, run, "completed").await;

        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.fired_count, w.strikes, w.state.as_str()),
            (1, 0, "{\"n\":1}")
        );
        assert_eq!(
            w.last_hash.as_deref(),
            Some(agent::sensor::payload_hash("events=x\np").as_str())
        );
        assert_eq!(w.last_fired_at, Some(t0));
        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Unchanged)
        );
        assert!(rec.calls.lock().unwrap().is_empty());
        assert!(run_succeeded("end_turn") && run_succeeded("stop"));
        assert!(!run_succeeded("error") && !run_succeeded("cancelled") && !run_succeeded("lost"));
    }

    /// `cooldown` counts from the fire (`watch_fired_at` in `fire_when`), so a
    /// failed run — no commit — still spaces the next fire; and a payload
    /// queued before the fire was recorded is dropped `cooldown` on close.
    #[tokio::test]
    async fn cooldown_runs_from_the_fire_even_when_the_run_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: while_true\ncooldown: 3m\nmode: queue\n",
            fs_exists_true(),
        )
        .await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);

        // a pre-fix row (no `last_fired_at`): the mid-run tick queues, the close
        // commits `last_fired_at = t0` and the queued payload is dropped `cooldown`
        sched
            .running
            .insert(sid, in_flight("w", t0, Some(when("events=first\nfirst"))));
        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Queued { coalesced: 0 })
        );
        let run = sched.running.remove(&sid).unwrap();
        sched.finished(sid, run, "completed").await;
        assert!(sched.pending.is_empty());
        assert!(rec.calls.lock().unwrap().is_empty(), "stale payload fired");
        // the drop row is stamped `now`, the tick rows are in the future
        let rows = results(&sched, 2).await;
        assert!(rows.iter().any(|r| r.starts_with("cooldown: ")), "{rows:?}");

        // a fire at t0 + 3m whose run fails: `last_fired_at` moved anyway
        let t1 = t0 + s(180);
        sched.store.watch_fired_at("w", t1).await.unwrap();
        let run = in_flight("w", t1, Some(when("events=second\nsecond")));
        sched.finished(sid, run, "error").await;
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.strikes, w.fired_count, w.last_fired_at),
            (1, 1, Some(t1))
        );
        assert_eq!(
            step(&mut sched, t1 + s(60)).await.unwrap().to_string(),
            "cooldown: 2m left"
        );
        assert_eq!(
            step(&mut sched, t1 + s(180)).await,
            Some(CheckOutcome::Fired)
        );
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.last_fired_at, Some(t1 + s(180)), "recorded by fire_when");
    }

    /// A queued payload is re-gated on close: equal to the committed hash →
    /// dropped `unchanged`; different → fires.
    #[tokio::test]
    async fn queued_payload_is_regated_when_the_run_closes() {
        let dir = tempfile::tempdir().unwrap();
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        let fake: FakeSensor = Arc::new(move |_, _| {
            let i = n2.fetch_add(1, Ordering::SeqCst) + 1;
            run(
                true,
                format!("events=change n={i}\nbody {i}"),
                serde_json::json!({"n": i}),
            )
        });
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nmode: queue\n",
            fake,
        )
        .await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);

        // the run's own payload is what the mid-run tick sees again (the live loop)
        sched.running.insert(
            sid,
            in_flight("w", t0, Some(when("events=change n=1\nbody 1"))),
        );
        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Queued { coalesced: 0 })
        );
        let run = sched.running.remove(&sid).unwrap();
        sched.finished(sid, run, "completed").await;
        assert!(sched.pending.is_empty());
        assert!(rec.calls.lock().unwrap().is_empty(), "stale payload fired");
        assert!(results(&sched, 2).await.contains(&"unchanged".to_owned()));
        assert_eq!(
            sched
                .store
                .watch_get("w")
                .await
                .unwrap()
                .unwrap()
                .fired_count,
            1
        );

        // a different one fires
        sched.running.insert(
            sid,
            in_flight("w", t0 + s(60), Some(when("events=first\nfirst"))),
        );
        assert_eq!(
            step(&mut sched, t0 + s(60)).await,
            Some(CheckOutcome::Queued { coalesced: 0 })
        );
        let run = sched.running.remove(&sid).unwrap();
        sched.finished(sid, run, "end_turn").await;
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].3.contains("events=change n=2"), "{}", calls[0].3);
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.fired_count, 3, "two closed runs and the queued fire");
        assert_eq!(
            w.last_hash.as_deref(),
            Some(agent::sensor::payload_hash("events=change n=2\nbody 2").as_str())
        );
    }

    /// The live `pr-smoke-1` case: a fired run spawns a child, its first turn
    /// ends while the child's result is outstanding, the result lands and a
    /// continuation turn runs. The run stays open through the first turn-end
    /// and closes on the last one: `watch_commit`, then the queued payload
    /// fires. Driven through the actor: `Settle` is a self-tell after [`SETTLE`].
    #[tokio::test]
    async fn fanned_out_run_closes_after_its_continuation_turn_and_fires_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: while_true\nmode: queue\n",
            fs_exists_true(),
        )
        .await;
        let t0 = Local::now();
        let sid = SessionId::new();
        let child = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        sched
            .running
            .insert(sid, in_flight("w", t0, Some(when("events=first\nfirst"))));
        assert_eq!(
            step(&mut sched, t0).await,
            Some(CheckOutcome::Queued { coalesced: 0 })
        );
        let store = store::Store::open(&dir.path().join(DB)).await.unwrap();
        let actor = Schedule::spawn(sched);
        let turn = |stop: &str| agent::AgentEvent::TurnFinished {
            session: sid,
            turn: runtime::TurnId::new(),
            stop_reason: stop.to_owned(),
            usage: agent::Usage::default(),
            context: 0,
        };
        let grace = SETTLE + Duration::from_millis(300);

        // turn 1: spawns the child, ends waiting for its result
        actor
            .tell(agent::AgentEvent::SubagentSpawned {
                parent: sid,
                child,
                name: "pr-review".into(),
                cap: None,
            })
            .await
            .unwrap();
        actor.tell(turn("end_turn")).await.unwrap();
        tokio::time::sleep(grace).await;
        assert!(rec.calls.lock().unwrap().is_empty(), "closed too early");
        assert!(
            store
                .watch_get("w")
                .await
                .unwrap()
                .unwrap()
                .last_hash
                .is_none()
        );

        // the child's result lands (50 §10: `PeerDelivered`, not `MessageDelivered`)
        // and starts the continuation turn
        actor
            .tell(agent::PeerDelivered {
                session: sid,
                origin: agent::PeerOrigin {
                    id: SessionId::new().0,
                    from: format!("session:{child}"),
                    reply_to: None,
                    scope: None,
                },
                text: "P1 finding".into(),
                seq: 33,
                turn: runtime::TurnId::new(),
            })
            .await
            .unwrap();
        actor
            .tell(agent::AgentEvent::TurnStarted {
                session: sid,
                turn: runtime::TurnId::new(),
                text: String::new(),
                model: String::new(),
                turn_seq: 34,
            })
            .await
            .unwrap();
        tokio::time::sleep(grace).await;
        assert!(rec.calls.lock().unwrap().is_empty(), "turn open");

        // turn 2 ends: no live child, no turn → close, commit, fire the queue
        actor.tell(turn("end_turn")).await.unwrap();
        tokio::time::sleep(grace).await;
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "queued payload fired once");
        assert!(
            calls[0].3.contains("coalesced=\"0\">\nevents=x"),
            "{}",
            calls[0].3
        );
        let w = store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.fired_count, 2, "the closed run and the queued fire");
    }

    #[tokio::test]
    async fn restart_closes_the_run_cancelled_and_fires() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: while_true\nmode: restart\n",
            fs_exists_true(),
        )
        .await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        sched
            .running
            .insert(sid, in_flight("w", t0, Some(when("events=first\nfirst"))));
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Restarted));
        assert!(sched.running.is_empty());
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
        assert_eq!(results(&sched, 2).await, ["task task-1", "restarted"]);
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.fired_count, 1, "the cancelled run did not commit");
    }

    #[tokio::test]
    async fn three_strikes_disable_and_resume_rearms() {
        let dir = tempfile::tempdir().unwrap();
        // Passes its load dry run (40 §1.12), then times out every tick.
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let fake: FakeSensor = Arc::new(move |_, _| {
            if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                run(false, String::new(), serde_json::json!({}))
            } else {
                Err(SensorError::Timeout(Duration::from_secs(20)))
            }
        });
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nquiet: true\n",
            fake,
        )
        .await;
        let t0 = Local::now();
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Quiet));
        for i in 1..3 {
            assert_eq!(
                step(&mut sched, t0 + s(30 * i)).await,
                Some(CheckOutcome::Broken("timeout after 20s".into()))
            );
            assert!(sched.entries["w"].spec.as_ref().unwrap().enabled);
        }
        assert_eq!(
            sched.store.watch_get("w").await.unwrap().unwrap().strikes,
            2
        );
        step(&mut sched, t0 + s(90)).await;
        let e = &sched.entries["w"];
        assert!(!e.spec.as_ref().unwrap().enabled && e.next.is_none());
        assert!(
            std::fs::read_to_string(dir.path().join("schedules/w.md"))
                .unwrap()
                .contains("enabled: false\n")
        );
        assert_eq!(
            results(&sched, 6).await,
            [
                "check broken ×3, disabled",
                "check broken: timeout after 20s",
                "check broken: timeout after 20s",
                "check broken: timeout after 20s",
                "quiet"
            ]
        );
        assert_eq!(
            sched.entries["w"].last_result.as_deref(),
            Some("check broken ×3, disabled")
        );
        assert!(rec.calls.lock().unwrap().is_empty());
        assert_eq!(
            step(&mut sched, t0 + s(120)).await,
            None,
            "disabled: not due"
        );

        sched.set_enabled("w", true).await.unwrap();
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.strikes, w.last_outcome.as_deref()),
            (0, Some("check broken: timeout after 20s"))
        );
        assert!(sched.entries["w"].next.is_some());
    }

    /// 40 §5: a deferred check (no sensor slot for 1 s) is a row, not a strike;
    /// it keeps `first_true_at`, the dry run and the strike count as they were.
    #[tokio::test]
    async fn deferred_is_a_row_not_a_strike() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let fake: FakeSensor =
            Arc::new(
                move |_, _| match c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                    0 => Err(SensorError::Budget("deferred".into())),
                    1 => run(false, String::new(), serde_json::json!({})),
                    2 => run(true, "here".into(), serde_json::json!({})),
                    3 => Err(SensorError::Budget("deferred".into())),
                    _ => Err(SensorError::Timeout(Duration::from_secs(20))),
                },
            );
        let (mut sched, rec) =
            watcher(dir.path(), "when: fs_exists.py path=/x\nfor: 5m\n", fake).await;
        let t0 = Local::now();

        // deferred before its dry run: the dry run waits for the next tick
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Deferred));
        assert!(sched.unvalidated.contains("w"));
        assert_eq!(sched.entries["w"].next, Some(t0 + s(30)));
        let h = sched.store.history("w", 1).await.unwrap();
        assert_eq!(
            (h[0].result.as_deref(), h[0].session),
            (Some("deferred"), None)
        );
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.last_outcome.as_deref(), w.strikes),
            (Some("deferred"), 0)
        );

        assert_eq!(
            step(&mut sched, t0 + s(30)).await,
            Some(CheckOutcome::Quiet)
        );
        assert!(!sched.unvalidated.contains("w"), "the dry run ran");
        assert_eq!(
            step(&mut sched, t0 + s(60)).await.map(|o| o.to_string()),
            Some("holding: 0s of 5m".into())
        );
        // deferred mid-hold: first_true_at survives
        assert_eq!(
            step(&mut sched, t0 + s(90)).await,
            Some(CheckOutcome::Deferred)
        );
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.holding_since, Some(t0 + s(60)));
        assert_eq!(w.strikes, 0);
        // the first real failure is strike one, not two
        assert_eq!(
            step(&mut sched, t0 + s(120)).await,
            Some(CheckOutcome::Broken("timeout after 20s".into()))
        );
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(w.strikes, 1);
        assert!(sched.entries["w"].spec.as_ref().unwrap().enabled);
        assert_eq!(
            results(&sched, 5).await,
            [
                "check broken: timeout after 20s",
                "deferred",
                "holding: 0s of 5m",
                "quiet",
                "deferred"
            ]
        );
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    /// 40 §1.12 last sentence: a hand-edited file's first check is its dry run;
    /// a failing one is listed `check broken`, never strikes, and never ticks
    /// until the file changes.
    #[tokio::test]
    async fn failed_load_dry_run_lists_check_broken_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(dir.path(), "when: nope.py path=/x\n", fs_exists()).await;
        let t0 = Local::now();
        // `resolve` fails inside `tick_when`: the gate runs inline, no check is returned.
        assert_eq!(step(&mut sched, t0).await, None);
        assert_eq!(
            results(&sched, 1).await,
            ["check broken: when: no such sensor: nope.py"]
        );
        let e = &sched.entries["w"];
        assert_eq!(
            (e.spec.as_ref().err().map(String::as_str), e.next),
            (Some("check broken: when: no such sensor: nope.py"), None)
        );
        assert_eq!(
            sched.store.watch_get("w").await.unwrap().unwrap().strikes,
            0,
            "a dry run is not a strike"
        );
        assert!(sched.tick(t0 + s(3600)).await.is_empty(), "never ticks");
        assert!(rec.calls.lock().unwrap().is_empty());
        let info = sched.info("w", &sched.entries["w"]).await;
        assert_eq!(
            (info.error.as_deref(), info.next),
            (Some("check broken: when: no such sensor: nope.py"), None)
        );

        // The file changes → re-parsed, dry-run again, this time it holds.
        let text = std::fs::read_to_string(dir.path().join("schedules/w.md"))
            .unwrap()
            .replace("nope.py", "fs_exists.py");
        std::fs::write(dir.path().join("schedules/w.md"), text).unwrap();
        sched.rescan(t0 + s(3600)).await;
        assert!(
            sched.unvalidated.contains("w"),
            "a changed file is dry-run again"
        );
        assert_eq!(
            step(&mut sched, t0 + s(3600)).await,
            Some(CheckOutcome::Quiet)
        );
        assert!(sched.entries["w"].spec.is_ok());
    }

    /// 40 §2: a hand-edited `when:` file gains `created:` on first load and
    /// `expires` counts from it, not from the mtime.
    #[tokio::test]
    async fn first_load_writes_created_and_expires_counts_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let (sched, _) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nexpires: 1h\n",
            fs_exists(),
        )
        .await;
        let path = dir.path().join("schedules/w.md");
        let text = std::fs::read_to_string(&path).unwrap();
        let spec = sched.entries["w"].spec.as_ref().unwrap();
        let created = spec.created.expect("created: parsed back");
        assert_eq!(
            text,
            file::set_created(&text, created),
            "the line on disk is the one set_created writes"
        );
        assert!(text.contains("\ncreated: "), "{text}");
        assert!((Local::now() - created).num_seconds().abs() < 5);
        assert_eq!(sched.entries["w"].raw, text, "entry holds what was written");
        assert_eq!(sched.created_of(spec), Some(created));
        // Unchanged on a re-read: no second stamp.
        let (name, raw, s2) = file::load_dir(&sched.dir).pop().unwrap();
        let (raw2, _) = stamp_created(&sched.dir, &name, raw.clone(), s2, Local::now());
        assert_eq!(raw2, raw);
    }

    #[tokio::test]
    async fn a_failed_run_is_a_strike_and_a_good_one_commits() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, _) =
            watcher(dir.path(), "when: fs_exists.py path=/x\n", fs_exists_true()).await;
        let t0 = Local::now();
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        sched
            .finished(
                sid,
                in_flight("w", t0, Some(when("events=a\nfirst"))),
                "error",
            )
            .await;
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (
                w.strikes,
                w.fired_count,
                w.last_hash.as_deref(),
                w.state.as_str()
            ),
            (1, 0, None, "{}")
        );
        let mut r = in_flight("w", t0, Some(when("events=a\nfirst")));
        r.timed_out = true;
        sched.finished(sid, r, "cancelled").await;
        assert_eq!(
            sched.store.watch_get("w").await.unwrap().unwrap().strikes,
            2,
            "timeout strikes"
        );
        sched
            .finished(
                sid,
                in_flight("w", t0, Some(when("events=a\nfirst"))),
                "cancelled",
            )
            .await;
        assert_eq!(
            sched.store.watch_get("w").await.unwrap().unwrap().strikes,
            2,
            "cancelled is not a strike"
        );
        sched
            .finished(
                sid,
                in_flight("w", t0 + s(5), Some(when("events=a\nfirst"))),
                "end_turn",
            )
            .await;
        let w = sched.store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.strikes, w.fired_count, w.state.as_str()),
            (0, 1, "{\"n\":1}")
        );
        assert_eq!(
            w.last_hash.as_deref(),
            Some(agent::sensor::payload_hash("events=a\nfirst").as_str())
        );
        assert_eq!(w.last_fired_at, Some(t0 + s(5)));
        // a clock-only run carries nothing to commit
        sched
            .finished(sid, in_flight("w", t0, None), "end_turn")
            .await;
        assert_eq!(
            sched
                .store
                .watch_get("w")
                .await
                .unwrap()
                .unwrap()
                .fired_count,
            1
        );
    }

    #[tokio::test]
    async fn once_disables_after_the_first_fire() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, rec) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nfire: once\n",
            fs_exists_true(),
        )
        .await;
        let t0 = Local::now();
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Fired));
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
        let e = &sched.entries["w"];
        assert!(!e.spec.as_ref().unwrap().enabled && e.next.is_none());
        assert!(
            e.raw.contains("enabled: false\n"),
            "the file is the record: {}",
            e.raw
        );
        assert_eq!(step(&mut sched, t0 + s(30)).await, None);
    }

    #[tokio::test]
    async fn expires_ends_the_file_and_still_running_is_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let created = (Local::now() - s(120)).to_rfc3339();
        let front = format!("when: fs_exists.py path=/x\nexpires: 1m\ncreated: \"{created}\"\n");
        let (mut sched, rec) = watcher(dir.path(), &front, fs_exists_true()).await;
        // `next` is armed at load: the clock the test ticks with starts after it.
        let t0 = Local::now();
        assert!(sched.tick(t0).await.is_empty(), "expired: no check");
        assert_eq!(results(&sched, 1).await, ["expired"]);
        assert!(!dir.path().join("schedules/w.md").exists());
        assert!(sched.consumed.contains("w") && sched.entries["w"].next.is_none());
        assert!(rec.calls.lock().unwrap().is_empty());

        // no `created:` → the file's mtime stands in: not expired now, expired 2m on
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, _) = watcher(
            dir.path(),
            "when: fs_exists.py path=/x\nexpires: 1m\n",
            fs_exists_true(),
        )
        .await;
        let t0 = Local::now();
        assert_eq!(step(&mut sched, t0).await, Some(CheckOutcome::Fired));
        assert!(sched.tick(t0 + s(120)).await.is_empty(), "past mtime + 1m");
        assert_eq!(results(&sched, 1).await, ["expired"]);

        // a check still in flight is a row, not a second check
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, _) =
            watcher(dir.path(), "when: fs_exists.py path=/x\n", fs_exists_true()).await;
        let t0 = Local::now();
        sched.checking.insert("w".to_owned());
        assert!(sched.tick(t0).await.is_empty());
        assert_eq!(results(&sched, 1).await, ["skipped: check still running"]);
        assert_eq!(sched.entries["w"].next, Some(t0 + s(30)));
    }

    #[tokio::test]
    async fn bind_session_is_idempotent_and_needs_a_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let (mut sched, _) =
            watcher(dir.path(), "when: fs_exists.py path=/x\n", fs_exists_true()).await;
        assert_eq!(
            sched.bind_session("w", Some("x")).await.unwrap_err(),
            "agent is not running"
        );
        let sid = SessionId::new();
        sched.entries.get_mut("w").unwrap().session = Some(sid);
        assert_eq!(sched.bind_session("w", None).await, Ok(sid));
        assert_eq!(
            sched.bind_session("nope", None).await.unwrap_err(),
            "no such schedule"
        );
        // a bound session survives a reload through the watch row
        sched.store.watch_bind("w", sid).await.unwrap();
        let (sched2, _) = actor(dir.path(), &[]).await;
        assert_eq!(sched2.entries["w"].session, Some(sid));
        let (mut plain, _) = actor(dir.path(), &[("p", "---\nevery: 1h\n---\ngo\n")]).await;
        assert_eq!(
            plain.bind_session("p", None).await.unwrap_err(),
            "not a watcher"
        );
    }
}
