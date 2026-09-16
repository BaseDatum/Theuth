//! Schedule plugin (design doc 34): a markdown file in `<data_dir>/schedules/`
//! whose front matter says *when* and whose body is the prompt. One actor
//! ticks, fires each due schedule as a wrapped input into that schedule's own
//! session, and delivers the run's reply to the user through
//! [`agent::Deliver`]. History lives in `<data_dir>/schedule.sqlite`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SCHEDULE_ACTOR`] actor ([`Schedule`]): [`ListSchedules`],
//!   [`Fire`], [`History`]
//! - events: [`ScheduleEvent`]

pub mod file;
pub mod store;
pub mod tools;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
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
    /// `end_turn` / `error: …` / `timeout` / `cancelled` / `skipped: busy` / `lost`; `None` while running.
    pub result: Option<String>,
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
}

/// Internal `tell`: the directory changed (watcher) — re-read it (34 §2.3).
#[derive(Clone, Copy, Debug)]
struct Reconcile;

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
        }
    }

    fn is_running(&self, e: &Entry) -> bool {
        e.session.is_some_and(|s| self.running.contains_key(&s))
    }

    /// 34 §4 steps 1–5. The caller has checked the entry exists and parsed.
    async fn fire(&mut self, name: &str, now: DateTime<Local>) -> Result<SessionId, String> {
        let entry = self.entries.get(name).ok_or("no such schedule")?;
        let spec = entry.spec.as_ref().map_err(Clone::clone)?;
        let session = entry.session;
        let (quiet, model, prompt) = (spec.quiet, spec.model.clone(), spec.prompt.clone());
        let one_shot = matches!(spec.when, file::When::At(_));

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
                let s = agents
                    .ask(agent::NewSession { cwd: None })
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

        let text = format!(
            "<schedule name=\"{name}\" at=\"{}\" quiet=\"{quiet}\">\n{prompt}\n</schedule>",
            now.to_rfc3339()
        );
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
        self.entries.retain(|n, _| seen.contains(n));
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
            if let Err(e) = self.fire(&name, now).await {
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

impl Message<agent::AgentEvent> for Schedule {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
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
            E::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                let Some(run) = self.running.remove(&session) else {
                    return;
                };
                self.finished(session, run, &stop_reason).await;
            }
            _ => {}
        }
    }
}

impl Schedule {
    /// Close the run row, remember the result, deliver the reply (34 §4.1).
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
        if let Err(e) = self.store.finish_run(run.id, Local::now(), &result).await {
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
        // Off the mailbox: every channel's owner may be slow (plugins.md §3.2).
        tokio::spawn(async move {
            let Ok(Some(user)) = ActorRef::<agent::UserChannels>::lookup(agent::USER_ACTOR) else {
                tracing::info!(%name, "not delivered: user channels are not running");
                return;
            };
            let m = agent::Outbound {
                sessions: vec![session],
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
}

impl Message<ListSchedules> for Schedule {
    type Reply = Vec<ScheduleInfo>;

    async fn handle(
        &mut self,
        _: ListSchedules,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.entries
            .iter()
            .map(|(name, e)| {
                let (when, describe, enabled, quiet, error) = match &e.spec {
                    Ok(s) => (
                        s.when_text.clone(),
                        s.when.describe(),
                        s.enabled,
                        s.quiet,
                        None,
                    ),
                    Err(err) => (
                        "—".to_owned(),
                        "—".to_owned(),
                        false,
                        false,
                        Some(err.clone()),
                    ),
                };
                ScheduleInfo {
                    name: name.clone(),
                    when,
                    describe,
                    enabled,
                    quiet,
                    next: e.next,
                    last: e.last,
                    last_result: e.last_result.clone(),
                    session: e.session,
                    running: self.is_running(e),
                    error,
                }
            })
            .collect()
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
        self.fire(&m.name, Local::now()).await
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
