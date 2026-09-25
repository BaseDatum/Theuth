//! Schedule plugin (design doc 34): a markdown file in `<data_dir>/schedules/`
//! whose front matter says *when* and whose body is the prompt. One actor
//! ticks, fires each due schedule as a wrapped input into that schedule's own
//! session, and delivers the run's reply to the user through
//! [`agent::Deliver`]. History lives in `<data_dir>/schedule.sqlite`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SCHEDULE_ACTOR`] actor ([`Schedule`]): [`ListSchedules`],
//!   [`Fire`], [`History`], [`SetEnabled`], [`Done`]
//! - events: [`ScheduleEvent`]
//!
//! Loops (design doc 33) are schedule files with a stop (`until:` /
//! `max_runs:`); the same actor runs them.

pub mod file;
pub mod store;
pub mod tools;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

/// `ask` → `Result<SessionId, String>`: fire now (`/schedules run`), ignoring `next`.
/// `Err("no such schedule" | "busy" | "disabled" | "<parse error>")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fire {
    /// File stem.
    pub name: String,
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
        return Err("until: no login shell on this platform (brush in-process is 58 step 3)".into());
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
    /// Direct children whose `<subagent_result>` has not reached the run's
    /// session yet: a just-spawned child is `Idle` before its first turn, so
    /// liveness alone would close the run early (33 §3.4).
    pending: HashSet<String>,
    /// A turn is in flight on the run's session (`TurnStarted`/`TurnFinished`).
    turn_open: bool,
    /// `stop_reason` of the last `TurnFinished`; what [`Settle`] closes with.
    last_stop: String,
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
}

impl Message<RegisterTaskCreator> for Schedule {
    type Reply = ();

    async fn handle(&mut self, m: RegisterTaskCreator, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.task_creator = Some(m.0);
    }
}

impl Schedule {
    /// Read the directory, join the store's latest row per name (34 §7) and
    /// arm `missed: run` cron schedules (34 §3).
    async fn load(rt: Runtime, dir: PathBuf, store: store::Store, timeout: Duration) -> Self {
        let now = Local::now();
        let mut latest = match store.latest().await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("schedule history unreadable: {e:#}");
                HashMap::new()
            }
        };
        let mut entries = BTreeMap::new();
        for (name, raw, spec) in file::load_dir(&dir) {
            let mut e = Entry::new(raw, spec, now);
            if let Some(l) = latest.remove(&name) {
                e.session = l.session;
                e.last = l.last;
                e.last_result = l.last_result;
                e.arm(now);
            }
            entries.insert(name, e);
        }
        Self {
            rt,
            dir,
            store,
            timeout,
            entries,
            running: HashMap::new(),
            consumed: HashSet::new(),
            task_creator: None,
            hub: mesh::is_hub(),
        }
    }

    fn is_running(&self, e: &Entry) -> bool {
        e.session.is_some_and(|s| self.running.contains_key(&s))
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
    /// instead of a session input.
    async fn fire(
        &mut self,
        name: &str,
        now: DateTime<Local>,
        forced: bool,
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
        let _ = write!(text, "{prompt}\n</schedule>");

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
            // A finished row: the task, not a session, carries the run (counts toward `max_runs`).
            if let Err(e) = self
                .store
                .insert_run(name, None, now, Some(&format!("task {task}")))
                .await
            {
                tracing::error!(%name, "run not recorded: {e:#}");
            }
            if let Some(e) = self.entries.get_mut(name) {
                e.last = Some(now);
                e.last_result = Some(format!("task {task}"));
                e.next = if one_shot { None } else { next_of(&e.spec, now, Some(now)) };
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
                s
            }
            None => {
                // Unattended automation roots are `isolated` (50 §2.3):
                // `NewSession` is the isolated path; the owner promotes one
                // with `SetPeerScope` if a schedule should reach owner roots.
                // `unattended` drops the visualization blocks from its prompt.
                let s = agents
                    .ask(agent::NewSession {
                        cwd: cwd.clone(),
                        unattended: true,
                    })
                    .await
                    .map_err(|e| format!("agent: {e}"))?;
                if let Err(e) = agents
                    .ask(agent::RenameSession {
                        session: s,
                        title: name.to_owned(),
                    })
                    .await
                {
                    tracing::warn!(%name, "session title not set: {e}");
                }
                if let Some(model) = model
                    && let Err(e) = agents
                        .ask(agent::SetModel {
                            session: Some(s),
                            model,
                        })
                        .await
                {
                    tracing::warn!(%name, "model not set: {e}");
                }
                s
            }
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

        let id = match self.store.insert_run(name, Some(session), now, None).await {
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
                deadline: Instant::now() + self.timeout,
                timed_out: false,
                children: HashSet::new(),
                pending: HashSet::new(),
                turn_open: true,
                last_stop: String::new(),
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
    /// get a `removed` row; publishes `Changed` and wakes the tick loop.
    async fn reconcile(&mut self, me: &ActorRef<Self>) {
        let now = Local::now();
        let mut seen = HashSet::new();
        for (name, raw, spec) in file::load_dir(&self.dir) {
            seen.insert(name.clone());
            match self.entries.get_mut(&name) {
                Some(e) if e.raw == raw => {}
                Some(e) => {
                    e.next = next_of(&spec, now, e.last);
                    e.spec = spec;
                    e.raw = raw;
                }
                None => {
                    self.entries.insert(name, Entry::new(raw, spec, now));
                }
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
        // A new file may already be due; the tick loop is asleep for up to TICK.
        let _ = me.tell(Tick).await;
    }
}

impl Message<Tick> for Schedule {
    type Reply = ();

    async fn handle(&mut self, _: Tick, _: &mut Context<Self, Self::Reply>) {
        self.tick(Local::now()).await;

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
            // 33 §3.4: the subtree goes with the run.
            for child in live_children(agents, *session).await {
                if let Err(e) = agents.ask(agent::Cancel { session: child }).await {
                    tracing::warn!(name = %run.name, %child, "timeout cancel of subagent failed: {e}");
                }
            }
            match agents.ask(agent::Cancel { session: *session }).await {
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
    async fn tick(&mut self, now: DateTime<Local>) {
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
        if !hub {
            return;
        }
        let due: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.next.is_some_and(|t| t <= now) && !self.is_running(e))
            .map(|(n, _)| n.clone())
            .collect();
        for name in due {
            if let Err(e) = self.fire(&name, now, false).await {
                tracing::warn!(%name, "not fired: {e}");
            }
        }
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
            E::SubagentSpawned {
                parent,
                child,
                name,
                ..
            } => {
                if let Some(r) = self.running.get_mut(&parent) {
                    r.children.insert(child);
                    r.pending.insert(name);
                } else if let Some(r) = self
                    .running
                    .values_mut()
                    .find(|r| r.children.contains(&parent))
                {
                    r.children.insert(child);
                }
            }
            // A direct child's result landed in the run's session; the turn it
            // starts (or extends) is the run's.
            E::MessageDelivered { session, from, .. } => {
                if let Some(r) = self.running.get_mut(&session) {
                    r.pending.remove(&from);
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
            e.last_result = Some(result);
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
        let Some(text) = report else { return };
        deliver(Some(session), name, text);
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
        ScheduleInfo {
            name: name.to_owned(),
            when: spec.map_or_else(|| "—".to_owned(), |s| s.when_text.clone()),
            describe: spec.map_or_else(|| "—".to_owned(), |s| s.when.describe()),
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
        }
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
        // Not gated on the hub (67 §4.4): *run now* is the user's ask on this engine.
        match self.fire(&m.name, Local::now(), true).await? {
            Fired::Session(s) => Ok(s),
            // No session to open: the reply names the task instead (67 §7.4).
            Fired::Task(id) => Err(format!("created task {id}")),
        }
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
        let e = self.entries.get_mut(&m.name).ok_or("no such schedule")?;
        let raw = file::set_enabled(&e.raw, m.enabled);
        file::write_atomic(&self.dir, &m.name, &raw).map_err(|err| err.to_string())?;
        // Updated in place so the reply is current; the watcher's reconcile
        // then finds `raw` unchanged (33 §3.5).
        e.spec = file::parse(&m.name, &raw);
        e.raw = raw;
        e.next = next_of(&e.spec, Local::now(), e.last);
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
        tokio::spawn(replica::files::mirror_at(rt.data_dir.join(DB), dir.clone(), DIR, std::time::Duration::from_secs(120)));
        let store = match store::Store::open(&rt.data_dir.join(DB)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("schedule unavailable: {e:#}");
                return Ok(());
            }
        };
        let timeout = Duration::from_secs(cfg.run_timeout_secs);
        let actor = Schedule::spawn_with_mailbox(
            Schedule::load(rt.clone(), dir.clone(), store, timeout).await,
            runtime::subscriber_mailbox(),
        );
        actor.register(SCHEDULE_ACTOR)?;
        rt.subscribe::<Schedule, agent::AgentEvent>(&actor).await;

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
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg, dir).await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod fire_tests {
    use std::sync::Mutex;

    use super::*;

    /// Records every call; answers a fixed task id.
    #[derive(Default)]
    pub(super) struct Recorder {
        pub(super) calls: Mutex<Vec<(String, u64, String, String, Option<String>, Option<String>)>>,
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
        let mut s = Schedule::load(rt, sched, store, Duration::from_secs(60)).await;
        let rec = Arc::new(Recorder::default());
        s.task_creator = Some(rec.clone());
        (s, rec)
    }

    #[tokio::test]
    async fn agent_fire_creates_a_task_not_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[("nightly", "---\nevery: 1h\nagent: ops\nproject: argo\n---\ncheck the deploy\n")],
        )
        .await;
        let now = Local::now();
        let Fired::Task(id) = s.fire("nightly", now, true).await.unwrap() else {
            panic!("expected a task")
        };
        assert_eq!(id, "task-1");
        let calls = rec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (name, fire_ms, agent, prompt, project, cwd) = &calls[0];
        assert_eq!((name.as_str(), agent.as_str()), ("nightly", "ops"));
        assert_eq!(*fire_ms, now.timestamp_millis() as u64);
        assert!(prompt.starts_with("<schedule name=\"nightly\"") && prompt.contains("check the deploy"));
        assert_eq!((project.as_deref(), cwd.as_deref()), (Some("argo"), None));
        // the run row carries the task, and counts toward the loop cap
        let h = s.store.history("nightly", 5).await.unwrap();
        assert_eq!(h[0].result.as_deref(), Some("task task-1"));
        assert!(h[0].session.is_none());
        assert_eq!(s.runs_of("nightly").await, 1);
        let e = &s.entries["nightly"];
        assert!(e.session.is_none() && e.last.is_some() && e.next.is_some());
        // the same instant fires again with the same key (tasks coalesces on it)
        let Fired::Task(_) = s.fire("nightly", now, true).await.unwrap() else { panic!() };
        assert_eq!(rec.calls.lock().unwrap()[1].1, now.timestamp_millis() as u64);
    }

    #[tokio::test]
    async fn max_runs_stops_new_tasks_and_until_closes_them() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[("once", "---\nevery: 1h\nagent: ops\nmax_runs: 1\n---\ngo\n")],
        )
        .await;
        s.fire("once", Local::now(), false).await.unwrap();
        let e = s.fire("once", Local::now(), false).await.unwrap_err();
        assert!(e.starts_with("max_runs 1 reached"), "{e}");
        assert_eq!(rec.calls.lock().unwrap().len(), 1, "no task past the cap");
        assert!(rec.closed.lock().unwrap().is_empty(), "a cap is not `until` passing");
        // `until` passing ends the loop and closes the agent's tasks
        let (mut s, rec) = actor(
            dir.path(),
            &[("loop", "---\nevery: 1h\nagent: ops\nuntil: \"true\"\n---\ngo\n")],
        )
        .await;
        let e = s.fire("loop", Local::now(), false).await.unwrap_err();
        assert!(e.starts_with("done: until passed"), "{e}");
        assert_eq!(rec.closed.lock().unwrap().as_slice(), ["loop"]);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plain_schedule_path_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(dir.path(), &[("plain", "---\nevery: 1h\n---\ngo\n")]).await;
        // No agent actor in a unit test: the session path is the one taken, and it says so.
        assert_eq!(s.fire("plain", Local::now(), true).await.unwrap_err(), "agent is not running");
        assert!(rec.calls.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod hub_tests {
    use parking_lot::Mutex;

    use super::fire_tests::actor;
    use super::*;

    /// The hub flag is process-global: these run one at a time and leave it true.
    static HUB: Mutex<()> = Mutex::new(());

    fn hub_lock() -> parking_lot::MutexGuard<'static, ()> {
        HUB.lock()
    }

    const DUE: &str = "---\nevery: 1h\nagent: ops\n---\ngo\n";

    #[tokio::test]
    async fn single_engine_default_is_hub() {
        let _g = hub_lock();
        assert!(mesh::is_hub());
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(dir.path(), &[("n", DUE)]).await;
        assert!(s.hub);
        s.tick(Local::now()).await;
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn non_hub_ticker_skips_and_does_not_advance_last() {
        let _g = hub_lock();
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
        let _g = hub_lock();
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
        assert_eq!(s.entries["c"].next, Some(yesterday), "the non-hub does not re-arm");
        let mut list = Vec::new();
        for (name, e) in &s.entries {
            list.push(s.info(name, e).await);
        }
        mesh::set_hub(true);
        let by = |n: &str| list.iter().find(|i| i.name == n).unwrap().next;
        assert!(by("c").is_some_and(|t| t > now), "cron: next occurrence after now, {:?}", by("c"));
        assert!(by("e").is_some_and(|t| t >= now), "every: now when the hub's last fire is unknown");
        assert_eq!(by("ended"), None, "an ended entry stays ended");
    }

    #[tokio::test]
    async fn regaining_hub_applies_missed_policy() {
        let _g = hub_lock();
        let dir = tempfile::tempdir().unwrap();
        let (mut s, rec) = actor(
            dir.path(),
            &[
                ("skip", "---\ncron: \"*/5 * * * *\"\nagent: ops\n---\ngo\n"),
                ("run", "---\ncron: \"*/5 * * * *\"\nagent: ops\nmissed: run\n---\ngo\n"),
            ],
        )
        .await;
        // Both fired once as the hub, so `last` is set and the next occurrence is armed.
        let t0 = Local::now();
        s.fire("skip", t0, false).await.unwrap();
        s.fire("run", t0, false).await.unwrap();
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
        assert_eq!((calls[2].0.as_str(), calls[2].1), ("run", back.timestamp_millis() as u64));
        assert!(s.entries["skip"].next.is_some_and(|t| t > back));
        assert_eq!(s.entries["skip"].last, Some(t0));
    }
}
