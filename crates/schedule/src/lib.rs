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
use std::process::Stdio;
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

/// Run `until` through `$SHELL -lc` (else `/bin/sh`) in `cwd`, no stdin
/// (33 §3.1). `Ok((exit code, output tail))`; `Err(why)` when it did not exit
/// in [`UNTIL_TIMEOUT`] or could not start.
async fn until_check(cmd: &str, cwd: Option<&Path>) -> Result<(Option<i32>, String), String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut c = tokio::process::Command::new(shell);
    c.arg("-lc")
        .arg(cmd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
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
        let next = next_of(&spec, now, None);
        Self {
            spec,
            raw,
            next,
            session: None,
            last: None,
            last_result: None,
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
                e.next = next_of(&e.spec, now, e.last);
                if let (Ok(s), Some(last)) = (&e.spec, e.last)
                    && s.enabled
                    && s.missed_run
                    && s.when.missed(last, now)
                {
                    e.next = Some(now);
                }
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
    /// checked the entry exists and parsed.
    async fn fire(
        &mut self,
        name: &str,
        now: DateTime<Local>,
        forced: bool,
    ) -> Result<SessionId, String> {
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
                let s = agents
                    .ask(agent::NewSession { cwd: cwd.clone() })
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
        agents
            .ask(agent::SendInput {
                session: Some(session),
                text,
                files: vec![],
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
        Ok(session)
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
        deliver(session, name.to_owned(), format!("schedule {name} {why}"));
    }
}

impl Message<Reconcile> for Schedule {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, ctx: &mut Context<Self, Self::Reply>) {
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
        let _ = ctx.actor_ref().tell(Tick).await;
    }
}

impl Message<Tick> for Schedule {
    type Reply = ();

    async fn handle(&mut self, _: Tick, _: &mut Context<Self, Self::Reply>) {
        let now = Local::now();
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

impl Message<NextWake> for Schedule {
    type Reply = Option<Duration>;

    async fn handle(&mut self, _: NextWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = Local::now();
        let inst = Instant::now();
        // Running entries count: a fire due while its run is still going becomes
        // a `skipped: busy` row at that tick, and a run that finished early must
        // not leave the loop asleep until TICK.
        let next = self
            .entries
            .values()
            .filter_map(|e| e.next)
            .min()
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
    /// has a stop (33 §3.2).
    async fn info(&self, name: &str, e: &Entry) -> ScheduleInfo {
        let spec = e.spec.as_ref().ok();
        let is_loop = spec.is_some_and(|s| s.until.is_some() || s.max_runs.is_some());
        ScheduleInfo {
            name: name.to_owned(),
            when: spec.map_or_else(|| "—".to_owned(), |s| s.when_text.clone()),
            describe: spec.map_or_else(|| "—".to_owned(), |s| s.when.describe()),
            enabled: spec.is_some_and(|s| s.enabled),
            quiet: spec.is_some_and(|s| s.quiet),
            next: e.next,
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
        self.fire(&m.name, Local::now(), true).await
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
