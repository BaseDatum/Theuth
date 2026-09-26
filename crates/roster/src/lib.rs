//! Roster plugin (design doc 67 §6): agents as files in `<data_dir>/agents/`,
//! each with one pinned desk session on the hub, episodes with a journal,
//! budgets, proposals. It implements [`tasks::DeskRouter`] (wakes reach a
//! desk through it) and, later, the knowledge owner/visibility and the MCP
//! allowlist consulted by other plugins.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`ROSTER_ACTOR`] actor ([`Roster`]): [`ListAgents`],
//!   [`GetAgent`], [`PutAgent`], [`PauseAgent`], [`ApproveAgent`], [`ProposeAgent`],
//!   [`RoutableAgents`]
//! - [`file::parse`]: `agents/<name>.md` → [`AgentFile`]
//! - [`Desks`]: the shared read model handed to the tasks plugin

pub mod budget;
pub mod config;
pub mod desk;
pub mod file;
pub mod lease;
pub mod proposals;
pub mod routing;
pub mod store;
pub mod tools;
pub mod types;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use notify::Watcher;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub use config::RosterConfig;
pub use desk::Shared;
pub use types::{AgentFile, AgentName, AgentStatus, AgentSummary, Budget, Episode};

/// Registry name of the [`Roster`] actor.
pub const ROSTER_ACTOR: &str = "roster";

/// Sqlite file under `data_dir` (67 §6).
pub const DB: &str = "roster.sqlite";

/// `ask` → `Result<Vec<AgentSummary>, String>`: `theuth` first, then every
/// file in the directory (invalid ones as [`AgentStatus::Invalid`]) with its
/// live status.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAgents;

/// `ask` → `Result<AgentFile, String>`. `theuth` is synthesised. `Err("no such agent")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetAgent(pub String);

/// `ask` → `Result<(), String>`: write `agents/<name>.md` from the file
/// (create or replace); the watcher reconciles and the record replicates.
/// `theuth` is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutAgent(pub AgentFile);

/// `ask` → `Result<(), String>`: rewrite the file's `paused:` line; a paused
/// agent's queued wakes are held. `Err("no such agent")`; `theuth` is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PauseAgent {
    /// Agent name.
    pub name: String,
    /// New value.
    pub paused: bool,
}

/// `ask` → `Result<(), String>`: approve a proposal (67 §6.5): clears
/// `proposed_by`, sets `paused: false`, drops the `proposals` row.
/// `Err("no such agent" | "not a proposal")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApproveAgent(pub String);

/// `ask` → `Result<(), String>`: delete `agents/<name>.md` (67 §6.5 refusal,
/// the Agents view's remove). A pending proposal's row goes too and its hire
/// task is cancelled with `note` (or "rejected"). `Err("no such agent")`;
/// `theuth` is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveAgent {
    /// Agent name.
    pub name: String,
    /// Why, for a proposal's hire task; `None` for a plain removal.
    pub note: Option<String>,
}

/// `ask` → `Result<String, String>`: `agent_propose` (67 §6.5). Writes the
/// file with `paused: true` and `proposed_by`, records the proposal, answers
/// the approval id. `Err` when the name is taken or reserved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposeAgent {
    /// The file to write; `paused`/`proposed_by` are set here.
    pub file: AgentFile,
    /// `agent:<name>` of the proposer.
    pub proposed_by: String,
}

/// `ask` → `Option<String>`: the agent whose desk (or a child of it)
/// `session` is; `None` for the user's own sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AgentOf(pub runtime::SessionId);

/// `ask` → `Result<(), String>`: close the agent's current episode (67 §6.2):
/// one journal turn, the journal file, then a fresh desk. Sent by the mesh
/// lease handler on a hub move (`reason: "lease"`) or by an operator.
/// `Err("no such agent" | "no desk")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpisodeClose {
    /// Agent name.
    pub agent: String,
    /// `idle | context | thread_end | lease | shutdown | …`, recorded on the episode.
    pub reason: String,
}

/// `ask` → `Result<(), String>`: [`tasks::DeskRouter::deliver`] lands here —
/// the scoped wake (67 §7.3), headed by the opening block when the episode
/// has had no input yet. `Err("no desk for <agent>")` while the desk is
/// paused, closing or not yet open; the tasks plugin re-dispatches.
#[derive(Clone, Debug)]
pub struct DeliverWake {
    /// Bare agent name.
    pub agent: String,
    /// The wake.
    pub wake: tasks::Wake,
    /// Its task, when the tasks plugin has it.
    pub task: Option<tasks::Task>,
}

/// `ask` → `Result<Vec<Routable>, String>`: the agents a chat message can
/// name (67 §10): every valid, unpaused, approved file — not `theuth`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RoutableAgents;

/// One row of [`RoutableAgents`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct Routable {
    /// File stem.
    pub name: String,
    /// Display title.
    pub title: String,
    /// The desk session id, when a desk is open.
    pub desk: Option<String>,
}

/// Internal `tell`: the mesh answered `Status` — this engine's id.
#[derive(Clone, Debug)]
struct SetSelf(String);

/// Internal `tell`: the tasks actor is up — tell it whether we dispatch.
#[derive(Clone, Copy, Debug)]
struct TasksUp;

/// Internal `tell`: the directory changed (watcher) — re-read it.
#[derive(Clone, Copy, Debug)]
struct Reconcile;

/// Internal `tell`, every [`TICK`]: idle episodes, closing deadlines, desks
/// that could not open yet (the agent actor was not up).
#[derive(Clone, Copy, Debug)]
struct Tick;

/// Episode housekeeping cadence.
const TICK: Duration = Duration::from_secs(60);

/// A closing turn that never finishes is given up after this.
const CLOSING_GRACE_MS: u64 = 10 * 60 * 1000;

/// A desk whose closing turn is in flight (67 §6.2).
struct Closing {
    agent: String,
    episode: String,
    reason: String,
    /// The reply so far (`TextDelta`s): the journal.
    text: String,
    deadline_ms: u64,
}

/// The [`ROSTER_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Roster {
    rt: Runtime,
    cfg: RosterConfig,
    dir: PathBuf,
    store: store::Store,
    shared: Arc<Shared>,
    /// Agent → its live desk.
    desks: HashMap<String, desk::Desk>,
    /// Desk session → its closing turn.
    closing: HashMap<runtime::SessionId, Closing>,
    /// Model id → context window (from `agent::ListModels`), 0 = unknown.
    windows: HashMap<String, u32>,
    /// 67 §4.4: desks and dispatch follow the hub lease.
    lease: lease::Follower,
    /// 67 §6.4: per-session cost totals last seen.
    costs: budget::Tracker,
}

/// ms since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Roster {
    async fn load(rt: Runtime, cfg: RosterConfig, dir: PathBuf, store: store::Store, shared: Arc<Shared>) -> Self {
        let mut me = Self {
            rt,
            cfg,
            dir,
            store,
            shared,
            desks: HashMap::new(),
            closing: HashMap::new(),
            windows: HashMap::new(),
            lease: lease::Follower::default(),
            costs: budget::Tracker::default(),
        };
        // ponytail: the previous run's open episodes are closed without a
        // journal turn (their sessions may be gone); their journals are the
        // ones written at their last close. Fresh desks open below.
        match me.store.open_episodes().await {
            Ok(eps) => {
                let now = now_ms();
                for ep in eps {
                    if let Err(e) = me.store.close_episode(&ep.id, now, "restart").await {
                        tracing::warn!(agent = %ep.agent, "episode not closed: {e:#}");
                    }
                }
            }
            Err(e) => tracing::warn!("open episodes not read: {e:#}"),
        }
        me.reconcile().await;
        me
    }

    /// Read the directory, update the read model and the `agents` mirror.
    async fn reconcile(&mut self) {
        let before = self.shared.files();
        let mut files = BTreeMap::new();
        for (stem, parsed) in file::load_dir(&self.dir) {
            if let Err(e) = AgentName::new(&stem) {
                tracing::warn!("{}/{stem}.md ignored: {e}", self.dir.display());
                continue;
            }
            if stem == AgentName::DEFAULT {
                tracing::warn!("{}/{stem}.md ignored: theuth is reserved", self.dir.display());
                continue;
            }
            if before.get(&stem) != Some(&parsed) {
                match &parsed {
                    Ok(f) => {
                        let mtime = std::fs::metadata(self.dir.join(format!("{stem}.md")))
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map_or_else(now_ms, |d| d.as_millis() as u64);
                        if let Err(e) = self.store.upsert_agent(f, mtime).await {
                            tracing::warn!(agent = %stem, "agents row not written: {e:#}");
                        }
                        push_charter(before.get(&stem).and_then(|p| p.as_ref().ok()).map(|f| f.body.as_str()), f);
                    }
                    Err(e) => tracing::warn!("{}/{stem}.md: {e}", self.dir.display()),
                }
            }
            files.insert(stem, parsed);
        }
        for gone in before.keys().filter(|n| !files.contains_key(*n)) {
            if let Err(e) = self.store.delete_agent(gone).await {
                tracing::warn!(agent = %gone, "agents row not removed: {e:#}");
            }
        }
        self.shared
            .set_files(files, file::default_agent(&self.rt.data_dir));
        self.sync_desks().await;
    }

    /// Agents whose desk opens eagerly: every valid, unpaused, approved file
    /// (67 §6.2, §6.5) — none while another engine holds the hub lease
    /// (67 §4.4), so nothing here reopens a desk after a lease close.
    /// `theuth`'s desk is not here: it opens on the first wake addressed to
    /// it ([`DeliverWake`]), so an engine with no `agents/*.md` creates no
    /// session at all (67 decision 5: a one-laptop user never sees any of this).
    fn wanted(&self) -> Vec<AgentFile> {
        if !self.lease.holding {
            return Vec::new();
        }
        self.shared
            .files()
            .into_values()
            .filter_map(Result::ok)
            .filter(|f| !f.paused && f.proposed_by.is_none())
            .collect()
    }

    /// Whether a desk for `name` may stay open: wanted, or `theuth`'s while
    /// this engine holds the lease.
    fn allowed(&self, name: &str) -> bool {
        self.lease.holding && (name == AgentName::DEFAULT || self.wanted().iter().any(|f| f.name.as_str() == name))
    }

    /// Open the desks that are missing, close the ones no longer wanted.
    async fn sync_desks(&mut self) {
        let wanted = self.wanted();
        for f in &wanted {
            if !self.desks.contains_key(f.name.as_str()) && !self.is_closing(f.name.as_str()) {
                self.open_desk(f).await;
            }
        }
        let stale: Vec<String> = self.desks.keys().filter(|n| !self.allowed(n)).cloned().collect();
        for name in stale {
            self.close_desk(&name, "paused").await;
        }
    }

    /// One pinned session named `agent:<name>` with the file's model and
    /// tools ceiling; the episode row; the read model. Silently skipped when
    /// the agent actor is not up yet (`Tick` retries).
    async fn open_desk(&mut self, f: &AgentFile) {
        let name = f.name.as_str();
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let open = agents
            .ask(agent::CreateSession {
                options: agent::CreateSessionOptions {
                    model: f.model.clone(),
                    ..Default::default()
                },
                peer: None,
            })
            .await;
        let session = match open {
            Ok(o) => o.session,
            Err(e) => {
                tracing::warn!(agent = %name, "desk not opened: {e}");
                return;
            }
        };
        if let Err(e) = agents
            .ask(agent::RenameSession {
                session,
                title: f.name.peer(),
            })
            .await
        {
            tracing::warn!(agent = %name, "desk not named: {e}");
        }
        if let Err(e) = agents
            .ask(agent::PinSession {
                session,
                pinned: true,
            })
            .await
        {
            tracing::warn!(agent = %name, "desk not pinned: {e}");
        }
        // 67 §6.3: the file's `tools:` is the registry ceiling on the desk root;
        // children inherit it by the min-over-ancestors rule.
        if let Some(cap) = f.tools.as_deref().and_then(|t| t.parse::<::tools::Risk>().ok())
            && let Ok(Some(reg)) = ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
        {
            let _ = reg
                .tell(::tools::SetCap {
                    session,
                    cap: Some(cap),
                })
                .await;
        }
        let now = now_ms();
        let episode = match self.store.open_episode(name, &session.to_string(), now).await {
            Ok(ep) => ep.id,
            Err(e) => {
                tracing::warn!(agent = %name, "episode not recorded: {e:#}");
                String::new()
            }
        };
        self.shared.set_desk(name, session);
        self.desks.insert(
            name.to_owned(),
            desk::Desk {
                session,
                episode,
                last_active_ms: now,
                working: false,
                primed: false,
                model: f.model.clone(),
            },
        );
        tracing::info!(agent = %name, %session, "desk opened");
    }

    /// Close the agent's episode (67 §6.2): the desk stops taking wakes now;
    /// a primed desk gets the journal turn and finishes in `finish_closing`,
    /// an empty one is put away at once.
    async fn close_desk(&mut self, name: &str, reason: &str) {
        let Some(d) = self.desks.remove(name) else {
            return;
        };
        self.shared.clear_desk(name);
        if !d.primed {
            self.put_away(name, d.session, &d.episode, reason, None).await;
            return;
        }
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            self.put_away(name, d.session, &d.episode, reason, None).await;
            return;
        };
        if let Err(e) = agents
            .ask(agent::SendInput {
                session: Some(d.session),
                text: desk::CLOSING.to_owned(),
                files: vec![],
                delivery: agent::Delivery::default(),
            })
            .await
        {
            tracing::warn!(agent = %name, "closing turn not started: {e}");
            self.put_away(name, d.session, &d.episode, reason, None).await;
            return;
        }
        self.closing.insert(
            d.session,
            Closing {
                agent: name.to_owned(),
                episode: d.episode,
                reason: reason.to_owned(),
                text: String::new(),
                deadline_ms: now_ms() + CLOSING_GRACE_MS,
            },
        );
    }

    /// The closing turn ended (or was given up): journal, episode row, archive
    /// the session, and open the next episode if the agent still wants one.
    async fn finish_closing(&mut self, session: runtime::SessionId) {
        let Some(c) = self.closing.remove(&session) else {
            return;
        };
        self.put_away(&c.agent, session, &c.episode, &c.reason, Some(&c.text)).await;
    }

    async fn put_away(
        &mut self,
        name: &str,
        session: runtime::SessionId,
        episode: &str,
        reason: &str,
        journal: Option<&str>,
    ) {
        if let Some(j) = journal.map(str::trim).filter(|j| !j.is_empty()) {
            match desk::write_journal(&self.rt.data_dir, name, j, self.cfg.journal_max_bytes as usize) {
                Ok(p) => tracing::info!(agent = %name, "journal written to {}", p.display()),
                Err(e) => tracing::warn!(agent = %name, "journal not written: {e}"),
            }
        }
        if !episode.is_empty()
            && let Err(e) = self.store.close_episode(episode, now_ms(), reason).await
        {
            tracing::warn!(agent = %name, "episode not closed: {e:#}");
        }
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            let _ = agents
                .ask(agent::PinSession {
                    session,
                    pinned: false,
                })
                .await;
            let _ = agents
                .ask(agent::ArchiveSession {
                    session,
                    archived: true,
                })
                .await;
        }
        tracing::info!(agent = %name, %session, %reason, "episode closed");
        if !self.desks.contains_key(name)
            && let Some(f) = self.wanted().into_iter().find(|f| f.name.as_str() == name)
        {
            self.open_desk(&f).await;
        }
    }

    /// The window of `model`, fetched once from the agent's catalog.
    async fn window(&mut self, model: &str) -> u32 {
        if let Some(w) = self.windows.get(model) {
            return *w;
        }
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(models) = agents.ask(agent::ListModels).await
        {
            for m in models {
                self.windows.insert(m.id, m.context);
            }
        }
        *self.windows.entry(model.to_owned()).or_insert(0)
    }

    fn is_closing(&self, agent: &str) -> bool {
        self.closing.values().any(|c| c.agent == agent)
    }

    fn desk_name(&self, session: runtime::SessionId) -> Option<String> {
        self.desks
            .iter()
            .find(|(_, d)| d.session == session)
            .map(|(n, _)| n.clone())
    }

    /// Write the file, update the read model in place (so a `GetAgent` right
    /// after sees it) and mirror the row; the watcher's reconcile then finds
    /// it unchanged and does nothing. A changed body follows into the charter
    /// document; the document's own export lands through `write_text`.
    async fn write_file(&self, f: &AgentFile) -> Result<(), String> {
        let old = self.shared.file(f.name.as_str());
        self.write_text(f.name.as_str(), &file::render(f)).await?;
        push_charter(old.as_ref().map(|o| o.body.as_str()), f);
        Ok(())
    }

    async fn write_text(&self, name: &str, text: &str) -> Result<(), String> {
        file::write_atomic(&self.dir, name, text).map_err(|e| e.to_string())?;
        let parsed = file::parse(name, text);
        if let Ok(f) = &parsed
            && let Err(e) = self.store.upsert_agent(f, now_ms()).await
        {
            tracing::warn!(agent = %name, "agents row not written: {e:#}");
        }
        self.shared.set_file(name, parsed);
        Ok(())
    }

    fn read_text(&self, name: &str) -> Result<String, String> {
        std::fs::read_to_string(self.dir.join(format!("{name}.md"))).map_err(|_| "no such agent".to_owned())
    }

    /// Tasks the agent owns in `todo | doing | blocked | review`, and whether
    /// every one of them is blocked. `(0, false)` while the tasks plugin is not up.
    async fn open_tasks(&self, name: &str) -> (u32, bool) {
        let Some(t) = tasks::actor() else {
            return (0, false);
        };
        let filter = tasks::TaskFilter {
            owner: Some(format!("agent:{name}")),
            state: Some(vec![
                tasks::TaskState::Todo,
                tasks::TaskState::Doing,
                tasks::TaskState::Blocked,
                tasks::TaskState::Review,
            ]),
            ..Default::default()
        };
        match t.ask(tasks::ListTasks(filter)).await {
            Ok(list) => (
                list.len() as u32,
                !list.is_empty() && list.iter().all(|t| t.state == tasks::TaskState::Blocked),
            ),
            Err(_) => (0, false),
        }
    }

    async fn summary(&self, f: AgentFile) -> AgentSummary {
        let name = f.name.as_str().to_owned();
        let desk = self.shared.desk(&name);
        let (open_tasks, all_blocked) = self.open_tasks(&name).await;
        let status = if f.proposed_by.is_some() {
            AgentStatus::Proposed
        } else if f.paused {
            AgentStatus::Paused
        } else if desk.is_some_and(|d| self.working(d)) {
            AgentStatus::Working
        } else if all_blocked {
            AgentStatus::Blocked
        } else {
            AgentStatus::Idle
        };
        AgentSummary {
            file: f,
            status,
            desk: desk.map(|d| d.to_string()),
            open_tasks,
            spent_month: self
                .store
                .spent_month(&name, &budget::month(now_ms()))
                .await
                .unwrap_or(0.0),
            error: None,
        }
    }

    /// A wake is being handled on the desk.
    fn working(&self, desk: runtime::SessionId) -> bool {
        self.desks.values().any(|d| d.session == desk && d.working)
    }

    /// 67 §4.4: the hub duties move with the lease (mirrors [`lease::apply`]).
    async fn lease_changed(&mut self, change: Option<bool>) {
        match change {
            Some(true) => {
                tracing::info!("hub lease held: desks open, wakes dispatch");
                self.dispatching(true).await;
                self.sync_desks().await;
            }
            Some(false) => {
                tracing::info!("hub lease elsewhere: desks close, wakes held");
                let open: Vec<String> = self.desks.keys().cloned().collect();
                for name in open {
                    self.close_desk(&name, "lease").await;
                }
                self.dispatching(false).await;
            }
            None => {}
        }
    }

    async fn dispatching(&self, on: bool) {
        if let Some(t) = tasks::actor() {
            let _ = t.ask(tasks::SetDispatching(on)).await;
        }
    }

    /// `agent`'s `reports_to` as a task owner; `user` is the user.
    fn manager_owner(&self, agent: &str) -> tasks::Owner {
        match self.shared.file(agent).map(|f| f.reports_to) {
            Some(m) if m != "user" => tasks::Owner::Agent(m),
            _ => tasks::Owner::User,
        }
    }

    /// Rewrite the file's `paused:` line (`PauseAgent`, the budget cap).
    async fn set_paused(&self, name: &str, paused: bool) -> Result<(), String> {
        let text = self.read_text(name)?;
        self.write_text(name, &file::set_key(&text, "paused", paused.then_some("true")))
            .await
    }

    /// File a task; `None` (logged) when the tasks plugin is down or refuses.
    async fn file_task(&self, input: tasks::TaskCreate, created_by: &str) -> Option<tasks::TaskId> {
        let title = input.title.clone();
        let Some(t) = tasks::actor() else {
            tracing::warn!(%title, "task not filed: tasks plugin is not running");
            return None;
        };
        match t
            .ask(tasks::CreateTask {
                input,
                created_by: created_by.to_owned(),
                session: None,
            })
            .await
        {
            Ok(task) => Some(task.id),
            Err(e) => {
                tracing::warn!(%title, "task not filed: {e}");
                None
            }
        }
    }

    /// 67 §6.4: a session's cost moved. Charge what is new to its agent's
    /// month on this engine; at 80 % the manager gets a task, at 100 % the
    /// agent is paused (unless the month is overridden) and the manager gets
    /// the override task.
    async fn charge(&mut self, session: runtime::SessionId) {
        let Some(name) = self.shared.agent_of(session) else {
            return;
        };
        if name == AgentName::DEFAULT {
            return;
        }
        let Some(f) = self.shared.file(&name) else {
            return;
        };
        let Budget::Monthly { amount, currency } = f.budget.clone() else {
            return;
        };
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let Ok(Some(cost)) = agents.ask(agent::SessionCostQuery(session)).await else {
            return;
        };
        // The session's own rows in the budget's currency; children report
        // their own `ModelUsageChanged` and are charged through `agent_of`.
        let money = |v: &[agent::cost::Money]| -> u64 {
            v.iter()
                .filter(|m| m.currency == currency)
                .map(|m| m.nanos)
                .sum()
        };
        let total = (money(&cost.direct.returned) + money(&cost.direct.estimated_for_unavailable)) as f64
            / agent::cost::NANOS_PER_UNIT as f64;
        let delta = self.costs.delta(session, total);
        if delta <= 0.0 {
            return;
        }
        let now = now_ms();
        let month = budget::month(now);
        let engine = self
            .lease
            .self_id
            .clone()
            .unwrap_or_else(|| platform::process::hostname().to_owned());
        if let Err(e) = self.store.add_spent(&name, &month, &engine, delta, &currency, now).await {
            tracing::warn!(agent = %name, "spend not recorded: {e:#}");
            return;
        }
        let after = match self.store.spent_month(&name, &month).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %name, "spend not read: {e:#}");
                return;
            }
        };
        let before = after - delta;
        let owner = self.manager_owner(&name);
        let task = move |title: String, brief: String| tasks::TaskCreate {
            state: tasks::TaskState::Todo,
            title,
            brief,
            owner: owner.clone(),
            reviewer: None,
            parent: None,
            project: None,
            where_: None,
            harness: None,
            external: vec![],
        };
        match budget::crossed(&f.budget, before, after) {
            None => {}
            Some(budget::Threshold::Warn) => {
                self.file_task(
                    task(
                        format!("Budget: {name} at 80 % ({after:.2} of {amount:.2} {currency})"),
                        format!(
                            "{} (`{name}`) has spent {after:.2} of its {amount:.2} {currency} for {month}. \
                             At {amount:.2} it is paused until you un-pause it (`agent_pause`, Agents view / RPC), \
                             which lifts the cap for the month.",
                            f.title
                        ),
                    ),
                    "system",
                )
                .await;
            }
            Some(budget::Threshold::Cap) => {
                if self.store.has_override(&name, &month).await.unwrap_or(false) {
                    return;
                }
                if let Err(e) = self.set_paused(&name, true).await {
                    tracing::warn!(agent = %name, "not paused at the budget cap: {e}");
                }
                self.sync_desks().await;
                self.file_task(
                    task(
                        format!("Budget override: {name} spent {after:.2} of {amount:.2} {currency} in {month}"),
                        format!(
                            "{} (`{name}`) reached its {amount:.2} {currency} budget for {month} and is paused. \
                             Un-pausing it (`agent_pause`, Agents view / RPC) lifts the cap for the rest of the month.",
                            f.title
                        ),
                    ),
                    "system",
                )
                .await;
            }
        }
    }
}

impl Message<Tick> for Roster {
    type Reply = ();

    async fn handle(&mut self, _: Tick, _: &mut Context<Self, Self::Reply>) {
        let now = now_ms();
        let idle: Vec<String> = self
            .desks
            .iter()
            .filter(|(_, d)| d.idle_due(now, self.cfg.episode_idle_secs))
            .map(|(n, _)| n.clone())
            .collect();
        for name in idle {
            self.close_desk(&name, "idle").await;
        }
        let overdue: Vec<runtime::SessionId> = self
            .closing
            .iter()
            .filter(|(_, c)| now > c.deadline_ms)
            .map(|(s, _)| *s)
            .collect();
        for s in overdue {
            tracing::warn!(%s, "closing turn gave no journal in time");
            self.finish_closing(s).await;
        }
        // Desks that could not open earlier (the agent actor was not up).
        self.sync_desks().await;
    }
}

impl Message<EpisodeClose> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: EpisodeClose, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.shared.file(&m.agent).is_none() {
            return Err("no such agent".into());
        }
        if !self.desks.contains_key(&m.agent) {
            return Err("no desk".into());
        }
        self.close_desk(&m.agent, &m.reason).await;
        Ok(())
    }
}

impl Message<DeliverWake> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: DeliverWake, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.agent == AgentName::DEFAULT
            && !self.desks.contains_key(&m.agent)
            && !self.is_closing(&m.agent)
            && self.lease.holding
            && let Some(t) = self.shared.theuth()
        {
            self.open_desk(&t).await;
        }
        let Some(d) = self.desks.get_mut(&m.agent) else {
            return Err(format!("no desk for {}", m.agent));
        };
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "agent actor is not running".to_owned())?;
        let mut text = String::new();
        if !d.primed {
            let f = self
                .shared
                .file(&m.agent)
                .ok_or_else(|| "no such agent".to_owned())?;
            let journal = desk::read_journal(&self.rt.data_dir, &m.agent);
            text.push_str(&desk::opening(&f, journal.as_deref()));
            text.push_str("\n\n");
        }
        text.push_str(&desk::wake_text(&m.wake, m.task.as_ref()));
        // 68 §4.4: a `Resume` whose checkout session was taken here (the
        // sweep re-pointed its engine at us) continues that session.
        let resumed = m
            .task
            .as_ref()
            .and_then(|t| t.checkout.as_ref())
            .filter(|c| m.wake.reason == tasks::WakeReason::Resume && c.taken)
            .and_then(|c| c.session.parse().ok());
        agents
            .ask(agent::SendInput {
                session: Some(resumed.unwrap_or(d.session)),
                text,
                files: vec![],
                delivery: agent::Delivery::default(),
            })
            .await
            .map_err(|e| e.to_string())?;
        d.primed = true;
        d.last_active_ms = now_ms();
        Ok(())
    }
}

impl Message<agent::AgentEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        use agent::AgentEvent as E;
        match ev {
            E::TurnStarted { session, model, .. } => {
                if let Some(name) = self.desk_name(session)
                    && let Some(d) = self.desks.get_mut(&name)
                {
                    d.working = true;
                    d.last_active_ms = now_ms();
                    d.model = Some(model);
                }
            }
            E::TextDelta { session, text, .. } => {
                if let Some(c) = self.closing.get_mut(&session) {
                    c.text.push_str(&text);
                }
            }
            E::TurnFinished {
                session, context, ..
            } => {
                if self.closing.contains_key(&session) {
                    self.finish_closing(session).await;
                    return;
                }
                let Some(name) = self.desk_name(session) else {
                    return;
                };
                let model = {
                    let Some(d) = self.desks.get_mut(&name) else {
                        return;
                    };
                    d.working = false;
                    d.last_active_ms = now_ms();
                    d.model.clone()
                };
                if let Some(model) = model {
                    let window = self.window(&model).await;
                    if desk::context_due(context, window, self.cfg.episode_context_pct) {
                        self.close_desk(&name, "context").await;
                    }
                }
            }
            // 67 §6.3: a child works for its parent's agent.
            E::SubagentSpawned { parent, child, .. } => self.shared.inherit(parent, child),
            // 67 §6.4: the session's spend moved.
            E::ModelUsageChanged { session, .. } => self.charge(session).await,
            // The thread ended under the desk: the episode is over, no journal possible.
            E::SessionDeleted { session } => {
                self.shared.forget(session);
                self.costs.forget(session);
                if let Some(c) = self.closing.remove(&session) {
                    self.put_away(&c.agent, session, &c.episode, &c.reason, Some(&c.text)).await;
                } else if let Some(name) = self.desk_name(session)
                    && let Some(d) = self.desks.remove(&name)
                {
                    self.shared.clear_desk(&name);
                    if !d.episode.is_empty()
                        && let Err(e) = self.store.close_episode(&d.episode, now_ms(), "thread_end").await
                    {
                        tracing::warn!(agent = %name, "episode not closed: {e:#}");
                    }
                    if let Some(f) = self.wanted().into_iter().find(|f| f.name.as_str() == name) {
                        self.open_desk(&f).await;
                    }
                }
            }
            _ => {}
        }
    }
}

/// 67 §4.4: the hub duties follow the lease. `Peer` events are not ours.
impl Message<mesh::MeshEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: mesh::MeshEvent, _: &mut Context<Self, Self::Reply>) {
        if let mesh::MeshEvent::Lease(l) = ev {
            let change = self.lease.observe(&l);
            self.lease_changed(change).await;
        }
    }
}

impl Message<SetSelf> for Roster {
    type Reply = ();

    async fn handle(&mut self, m: SetSelf, _: &mut Context<Self, Self::Reply>) {
        let change = self.lease.set_self(m.0);
        self.lease_changed(change).await;
    }
}

impl Message<TasksUp> for Roster {
    type Reply = ();

    async fn handle(&mut self, _: TasksUp, _: &mut Context<Self, Self::Reply>) {
        self.dispatching(self.lease.holding).await;
    }
}

impl Message<RoutableAgents> for Roster {
    type Reply = Result<Vec<Routable>, String>;

    async fn handle(&mut self, _: RoutableAgents, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self
            .shared
            .files()
            .into_values()
            .filter_map(Result::ok)
            .filter(|f| !f.paused && f.proposed_by.is_none())
            .map(|f| Routable {
                desk: self.shared.desk(f.name.as_str()).map(|s| s.to_string()),
                name: f.name.as_str().to_owned(),
                title: f.title,
            })
            .collect())
    }
}

impl Message<Reconcile> for Roster {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, _: &mut Context<Self, Self::Reply>) {
        self.reconcile().await;
    }
}

impl Message<ListAgents> for Roster {
    type Reply = Result<Vec<AgentSummary>, String>;

    async fn handle(&mut self, _: ListAgents, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut out = Vec::new();
        if let Some(t) = self.shared.theuth() {
            out.push(self.summary(t).await);
        }
        for (name, parsed) in self.shared.files() {
            match parsed {
                Ok(f) => out.push(self.summary(f).await),
                Err(e) => {
                    let Ok(agent) = AgentName::new(&name) else {
                        continue;
                    };
                    out.push(AgentSummary {
                        file: AgentFile {
                            title: name.clone(),
                            name: agent,
                            reports_to: AgentName::DEFAULT.to_owned(),
                            model: None,
                            effort: None,
                            tools: None,
                            mcp: None,
                            budget: Budget::None,
                            capabilities: None,
                            paused: true,
                            proposed_by: None,
                            body: String::new(),
                        },
                        status: AgentStatus::Invalid,
                        desk: None,
                        open_tasks: 0,
                        spent_month: 0.0,
                        error: Some(e),
                    });
                }
            }
        }
        Ok(out)
    }
}

impl Message<GetAgent> for Roster {
    type Reply = Result<AgentFile, String>;

    async fn handle(&mut self, m: GetAgent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.shared
            .file(&m.0)
            .ok_or_else(|| "no such agent".to_owned())
    }
}

impl Message<AgentOf> for Roster {
    type Reply = Option<String>;

    async fn handle(&mut self, m: AgentOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.shared.agent_of(m.0)
    }
}

impl Message<PutAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PutAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.0.name.is_default() {
            return Err("theuth is the built-in default agent: edit prompt.md, not the roster".into());
        }
        // Validate through the parser so the file on disk is always readable.
        file::parse(m.0.name.as_str(), &file::render(&m.0))?;
        self.write_file(&m.0).await?;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        Ok(())
    }
}

impl Message<PauseAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PauseAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.name == AgentName::DEFAULT {
            return Err("theuth cannot be paused: it is the built-in default agent".into());
        }
        // 67 §6.4: un-pausing an agent past its cap is the manager's override
        // for the month; the cap does not pause it again.
        if !m.paused
            && let Some(Budget::Monthly { amount, .. }) = self.shared.file(&m.name).map(|f| f.budget)
        {
            let month = budget::month(now_ms());
            if self.store.spent_month(&m.name, &month).await.unwrap_or(0.0) >= amount
                && let Err(e) = self.store.set_override(&m.name, &month, now_ms()).await
            {
                tracing::warn!(agent = %m.name, "budget override not recorded: {e:#}");
            }
        }
        self.set_paused(&m.name, m.paused).await?;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        Ok(())
    }
}

impl Message<ApproveAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ApproveAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.0 == AgentName::DEFAULT {
            return Err("not a proposal".into());
        }
        let text = self.read_text(&m.0)?;
        let f = self.shared.file(&m.0);
        let approval = self
            .store
            .delete_proposal(&m.0)
            .await
            .map_err(|e| e.to_string())?;
        if approval.is_none() && !f.is_some_and(|f| f.proposed_by.is_some()) {
            return Err("not a proposal".into());
        }
        let text = file::set_key(&text, "paused", None);
        let text = file::set_key(&text, "proposed_by", None);
        self.write_text(&m.0, &text).await?;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        // 67 §6.5: the hire task was the approval; close it.
        if let Some(id) = approval {
            close_hire_task(&m.0, id, "approved").await;
        }
        Ok(())
    }
}

impl Message<RemoveAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RemoveAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let approval = self.remove_agent(&m.name).await?;
        // Reconcile drops the read model, the `agents` row and the desk; the charter document goes to `.trash`.
        let _ = ctx.actor_ref().tell(Reconcile).await;
        if let Some(docs) = document::Handle::lookup() {
            let binding = document::Binding::AgentCharter(m.name.clone());
            tokio::spawn(async move {
                if let Ok(Some(doc)) = docs.actor().ask(document::Lookup { binding }).await {
                    let _ = docs.delete(doc).await;
                }
            });
        }
        if let Some(id) = approval {
            let note = m.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
            close_hire_task(&m.name, id, &note.map_or_else(|| "rejected".to_owned(), |n| format!("rejected: {n}"))).await;
        }
        Ok(())
    }
}

/// 67 §6.1: the file is the charter's source of truth and its document follows it — a new or
/// changed body is imported into `agent-charter:<name>` (59 §4.4, one diff transaction) when
/// that document exists. Fire-and-forget: the roster never waits on the documents actor.
fn push_charter(old_body: Option<&str>, f: &AgentFile) {
    if old_body == Some(f.body.as_str()) {
        return;
    }
    let Some(docs) = document::Handle::lookup() else { return };
    let (name, body) = (f.name.as_str().to_owned(), f.body.clone());
    tokio::spawn(async move {
        let binding = document::Binding::AgentCharter(name.clone());
        let Ok(Some(doc)) = docs.actor().ask(document::Lookup { binding }).await else { return };
        if let Err(e) = docs.import_markdown(doc, body, document::Principal::File).await {
            tracing::warn!(agent = %name, "charter document not updated from the file: {e}");
        }
    });
}

impl Message<document::Exported> for Roster {
    type Reply = ();

    async fn handle(&mut self, m: document::Exported, _: &mut Context<Self, Self::Reply>) {
        if let document::Binding::AgentCharter(name) = m.binding {
            self.charter_exported(&name, &m.path).await;
        }
    }
}

impl Roster {
    /// The other direction: the charter document's export (`documents.export_quiet_secs` after the
    /// last edit) becomes the file's body under its front matter. A name without a file (the Hire
    /// wizard's draft, `theuth`) has nothing to write yet; an empty export would make the file
    /// invalid (`empty charter`) and is ignored. Answers whether the file was rewritten.
    async fn charter_exported(&self, name: &str, path: &std::path::Path) -> bool {
        let Some(mut f) = self.shared.file(name).filter(|f| !f.name.is_default()) else { return false };
        let Ok(text) = std::fs::read_to_string(path) else { return false };
        let body = text.trim();
        if body.is_empty() || body == f.body {
            return false;
        }
        f.body = body.to_owned();
        match self.write_text(f.name.as_str(), &file::render(&f)).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(agent = %name, "charter export not written to the file: {e}");
                false
            }
        }
    }
}

impl Roster {
    /// Delete the file and any proposal row; answers the proposal's approval id.
    async fn remove_agent(&self, name: &str) -> Result<Option<String>, String> {
        if name == AgentName::DEFAULT {
            return Err("theuth is the built-in default agent and cannot be removed".into());
        }
        std::fs::remove_file(self.dir.join(format!("{name}.md"))).map_err(|_| "no such agent".to_owned())?;
        self.store
            .delete_proposal(name)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Comment `text` on a hire task and cancel it, as the user. Best effort — a
/// pre-task uuid or a down tasks plugin just logs.
async fn close_hire_task(agent: &str, id: String, text: &str) {
    let Some(t) = tasks::actor() else { return };
    let id = tasks::TaskId(id);
    let r = match t
        .ask(tasks::CommentTask {
            id: id.clone(),
            text: text.into(),
            progress: false,
            actor: "user".into(),
            session: None,
        })
        .await
    {
        Ok(()) => t
            .ask(tasks::UpdateTask {
                id,
                patch: tasks::TaskPatch {
                    state: Some(tasks::TaskState::Cancelled),
                    ..Default::default()
                },
                actor: "user".into(),
                session: None,
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = r {
        tracing::debug!(agent, "hire task not closed: {e}");
    }
}

impl Message<ProposeAgent> for Roster {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: ProposeAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut f = m.file;
        if f.name.is_default() {
            return Err("theuth is reserved".into());
        }
        if self.shared.files().contains_key(f.name.as_str()) {
            return Err(format!("agent {} exists; pick another name or create a task for it", f.name));
        }
        f.paused = true;
        f.proposed_by = Some(m.proposed_by.clone());
        file::parse(f.name.as_str(), &file::render(&f))?;
        self.write_file(&f).await?;
        // 67 §6.5: the hire task, owned by the proposer's manager, is the
        // approval; without the tasks plugin a bare uuid stands in.
        let proposer = m.proposed_by.strip_prefix("agent:").unwrap_or(&m.proposed_by);
        let manager = if proposer == "user" {
            tasks::Owner::User
        } else {
            self.manager_owner(proposer)
        };
        let approval_id = match self
            .file_task(proposals::hire_task(&f, &m.proposed_by, manager), &m.proposed_by)
            .await
        {
            Some(id) => id.0,
            None => {
                tracing::warn!(agent = %f.name, "hire task not filed; approval id is a bare uuid");
                uuid::Uuid::now_v7().to_string()
            }
        };
        self.store
            .insert_proposal(f.name.as_str(), &m.proposed_by, &approval_id, now_ms())
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        Ok(approval_id)
    }
}

/// The roster's read model as other plugins see it: [`tasks::DeskRouter`]
/// (67 §6.2, §7.3) and [`::tools::McpAccess`] (67 §6.3). Registered with the
/// tasks actor and the tool registry by the plugin.
pub struct Desks(pub Arc<Shared>);

#[async_trait::async_trait]
impl tasks::DeskRouter for Desks {
    fn desk_for(&self, agent: &str) -> Option<String> {
        self.0.desk(agent).map(|s| s.to_string())
    }

    fn tools_of(&self, agent: &str) -> Option<String> {
        self.0.file(agent)?.tools
    }

    async fn deliver(
        &self,
        agent: &str,
        wake: &tasks::Wake,
        task: Option<&tasks::Task>,
    ) -> anyhow::Result<()> {
        let roster = actor().ok_or_else(|| anyhow::anyhow!("roster is not running"))?;
        roster
            .ask(DeliverWake {
                agent: agent.to_owned(),
                wake: wake.clone(),
                task: task.cloned(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The file's `reports_to`; `user` (the top of the line) is `None`.
    fn manager_of(&self, agent: &str) -> Option<String> {
        self.0.file(agent).map(|f| f.reports_to).filter(|m| m != "user")
    }

    fn agent_for(&self, session: &str) -> Option<String> {
        session.parse().ok().and_then(|s| self.0.agent_of(s))
    }

    /// Every valid file's name, then `theuth`.
    fn agents(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .0
            .files()
            .into_iter()
            .filter_map(|(n, p)| p.ok().map(|_| n))
            .collect();
        v.push(AgentName::DEFAULT.to_owned());
        v
    }
}

/// 67 §6.3: an MCP tool is offered to a session only when the agent it works
/// for lists the server (or `server.tool`); `theuth` and sessions of no agent
/// see everything. Descendants are the agent's too ([`Shared::inherit`]).
impl ::tools::McpAccess for Desks {
    fn allows(&self, session: runtime::SessionId, server: &str, tool: &str) -> bool {
        self.0
            .agent_of(session)
            .and_then(|a| self.0.file(&a))
            .is_none_or(|f| desk::mcp_visible(&f, server, tool))
    }
}

/// 67 §9: a session of agent X reads and writes knowledge as `agent:X`, with
/// X's reporting line (up and down) for `team` items. The user's sessions
/// and `theuth`'s carry no scope and see everything.
impl knowledge::Scoper for Desks {
    fn scope_for(&self, session: runtime::SessionId) -> Option<knowledge::AgentScope> {
        let agent = self.0.agent_of(session)?;
        if agent == AgentName::DEFAULT {
            return None;
        }
        Some(knowledge::AgentScope {
            owner: format!("agent:{agent}"),
            line: self
                .0
                .line(&agent)
                .into_iter()
                .map(|n| if n == "user" { n } else { format!("agent:{n}") })
                .collect(),
        })
    }
}

/// The plugin: reads `roster`, creates `agents/`, opens the store, spawns
/// [`Roster`] as [`ROSTER_ACTOR`], watches the directory, registers
/// [`Desks`] with the tasks actor and the two tools with the registry once
/// each is up.
pub struct RosterPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for RosterPlugin {
    fn name(&self) -> &'static str {
        "roster"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: RosterConfig = rt.section("roster").unwrap_or_else(|e| {
            tracing::debug!("no [roster] config section, using defaults: {e}");
            RosterConfig::default()
        });
        let dir = rt.data_dir.join(file::DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("roster unavailable: {}: {e}", dir.display());
            return Ok(());
        }
        let store = match store::Store::open(&rt.data_dir.join(DB)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("roster unavailable: {e:#}");
                return Ok(());
            }
        };
        // 67 §5.1: `agents` and `budget` replicate; local only without the plugin.
        match replica::attach(store.pool(), &["agents", "budget"]).await {
            Ok(_) => tracing::info!("roster: agents and budget replicate"),
            Err(e) => tracing::debug!("roster: local only ({e})"),
        }
        let shared = Arc::new(Shared::default());
        let actor = Roster::spawn_with_mailbox(
            Roster::load(rt.clone(), cfg, dir.clone(), store, shared.clone()).await,
            runtime::subscriber_mailbox(),
        );
        actor.register(ROSTER_ACTOR)?;
        rt.subscribe::<Roster, agent::AgentEvent>(&actor).await;
        // The charter document's exports become the file's body.
        rt.subscribe::<Roster, document::Exported>(&actor).await;
        // 67 §4.4: desks and dispatch follow the lease; self id once the mesh is up.
        rt.subscribe::<Roster, mesh::MeshEvent>(&actor).await;
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(mesh) = rt2.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await
                && let Ok(status) = mesh.ask(mesh::Status).await
            {
                let _ = me.tell(SetSelf(status.self_id.0)).await;
            }
        });
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = rt2.shutdown.cancelled() => break,
                    () = tokio::time::sleep(TICK) => {}
                }
                if me.tell(Tick).await.is_err() {
                    break;
                }
            }
        });

        // Watcher: the schedule crate's idiom (34 §2.3).
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
            Err(e) => tracing::warn!("agents directory not watched: {e}; edits need a restart"),
        }

        let rt2 = rt.clone();
        let desks = shared.clone();
        let me = actor.clone();
        tokio::spawn(async move {
            if let Some(t) = rt2
                .lookup_when::<tasks::Tasks>(tasks::TASKS_ACTOR)
                .await
            {
                let _ = t
                    .ask(tasks::RegisterDeskRouter(Arc::new(Desks(desks))))
                    .await;
                let _ = me.tell(TasksUp).await;
            }
        });
        let rt2 = rt.clone();
        let scoper = shared.clone();
        tokio::spawn(async move {
            if let Some(k) = rt2
                .lookup_when::<knowledge::KnowledgeActor>(knowledge::KNOWLEDGE_ACTOR)
                .await
            {
                let _ = k
                    .ask(knowledge::RegisterScoper(Arc::new(Desks(scoper))))
                    .await;
            }
        });
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                // 67 §6.3: the allowlist consult, then the tools and the hiring prompt block.
                let _ = reg
                    .ask(::tools::InstallMcpAccess(Arc::new(Desks(shared.clone()))))
                    .await;
                tools::register(&reg, shared).await;
                let _ = reg
                    .ask(::tools::RegisterPrompt {
                        plugin: "roster",
                        text: tools::PROMPT.into(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}

/// The roster actor; `None` before the plugin started or when its store failed.
pub fn actor() -> Option<ActorRef<Roster>> {
    ActorRef::<Roster>::lookup(ROSTER_ACTOR).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("roster-{tag}-{}-{}", std::process::id(), uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join(file::DIR)).unwrap();
        dir
    }

    async fn roster(data_dir: &std::path::Path) -> Roster {
        let rt = Runtime::new(figment::Figment::new(), data_dir.to_path_buf());
        let store = store::Store::open(&data_dir.join(DB)).await.unwrap();
        Roster::load(
            rt,
            RosterConfig::default(),
            data_dir.join(file::DIR),
            store,
            Arc::new(Shared::default()),
        )
        .await
    }

    #[tokio::test]
    async fn scan_valid_invalid_and_theuth() {
        let data = tmp("scan");
        let dir = data.join(file::DIR);
        std::fs::write(dir.join("ops.md"), "---\ntitle: Ops\ntools: mutating\n---\nYou are Ops.\n").unwrap();
        std::fs::write(dir.join("bad.md"), "---\ncomputer: build-A\n---\nNope.\n").unwrap();
        std::fs::write(dir.join("Bad Name.md"), "charter\n").unwrap();
        std::fs::write(data.join("prompt.md"), "Standing instructions.\n").unwrap();
        let r = roster(&data).await;

        let files = r.shared.files();
        assert_eq!(files.len(), 2);
        assert_eq!(files["ops"].as_ref().unwrap().title, "Ops");
        assert!(files["bad"].as_ref().unwrap_err().starts_with("front matter"));
        assert_eq!(r.store.agent_names().await.unwrap(), ["ops"], "only valid files are mirrored");

        let t = r.shared.file("theuth").unwrap();
        assert_eq!(t.body, "Standing instructions.");
        assert_eq!(t.reports_to, "user");
        assert_eq!(t.tools.as_deref(), Some("destructive"));
        assert!(r.shared.file("nobody").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 67 decision 5: with no `agents/*.md` nothing wants a desk — theuth's is
    /// allowed to stay open once a wake opened it, never opened eagerly.
    #[tokio::test]
    async fn no_files_means_no_eager_desk_and_theuth_on_demand_only() {
        let data = tmp("lazy");
        let r = roster(&data).await;
        assert!(r.lease.holding, "a lone engine holds the lease from the start");
        assert!(r.wanted().is_empty());
        assert!(r.allowed("theuth"));
        assert!(!r.allowed("ops"));

        std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
        let mut r = r;
        r.reconcile().await;
        assert_eq!(r.wanted().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["ops"]);
        assert!(r.allowed("ops") && r.allowed("theuth"));
        r.lease.holding = false;
        assert!(!r.allowed("theuth") && r.wanted().is_empty(), "duties follow the lease");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn knowledge_scope_is_the_agent_and_its_line() {
        use knowledge::Scoper as _;
        let s = Arc::new(Shared::default());
        let mut files = BTreeMap::new();
        files.insert("cto".into(), file::parse("cto", "Boss."));
        files.insert("ops".into(), file::parse("ops", "---\nreports_to: cto\n---\nOps."));
        files.insert("mkt".into(), file::parse("mkt", "Marketing."));
        s.set_files(files, file::default_agent(std::path::Path::new("/nonexistent")));
        let (ops, theuth, user) = (runtime::SessionId::new(), runtime::SessionId::new(), runtime::SessionId::new());
        s.set_desk("ops", ops);
        s.set_desk("theuth", theuth);
        let desks = Desks(s);
        let scope = desks.scope_for(ops).unwrap();
        assert_eq!(scope.owner, "agent:ops");
        assert_eq!(
            scope.line.iter().map(String::as_str).collect::<Vec<_>>(),
            ["agent:cto", "agent:ops", "agent:theuth", "user"]
        );
        assert_eq!(desks.scope_for(theuth), None, "theuth sees everything");
        assert_eq!(desks.scope_for(user), None);
    }

    #[tokio::test]
    async fn put_round_trip_and_theuth_refused() {
        let data = tmp("put");
        let mut r = roster(&data).await;
        let f = file::parse("qa", "---\ntitle: QA\nmcp: [grafana]\nbudget: 10 EUR/month\n---\nTest everything.\n").unwrap();
        r.write_file(&f).await.unwrap();
        r.reconcile().await;
        assert_eq!(r.shared.file("qa"), Some(f.clone()));
        assert_eq!(file::parse("qa", &std::fs::read_to_string(data.join("agents/qa.md")).unwrap()).unwrap(), f);
        assert_eq!(r.store.agent_names().await.unwrap(), ["qa"]);

        // Pause rewrites one line; the rest of the file is byte-for-byte.
        let text = r.read_text("qa").unwrap();
        r.write_text("qa", &file::set_key(&text, "paused", Some("true"))).await.unwrap();
        assert!(r.shared.file("qa").unwrap().paused);
        assert!(std::fs::read_to_string(data.join("agents/qa.md")).unwrap().contains("budget: 10 EUR/month\n"));

        // Removing the file drops the row.
        std::fs::remove_file(data.join("agents/qa.md")).unwrap();
        r.reconcile().await;
        assert!(r.shared.file("qa").is_none());
        assert!(r.store.agent_names().await.unwrap().is_empty());

        assert!(file::parse("theuth", "x").is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn charter_export_replaces_the_body_and_keeps_the_front_matter() {
        let data = tmp("charter");
        let r = roster(&data).await;
        let f = file::parse("qa", "---\ntitle: QA\nmcp: [grafana]\nbudget: 10 EUR/month\n---\nTest everything.\n").unwrap();
        r.write_file(&f).await.unwrap();
        let export = data.join("charter.md");

        std::fs::write(&export, "# QA\n\nTest everything, twice.\n").unwrap();
        assert!(r.charter_exported("qa", &export).await);
        let on_disk = std::fs::read_to_string(data.join("agents/qa.md")).unwrap();
        assert!(on_disk.contains("budget: 10 EUR/month\n"), "{on_disk}");
        assert!(on_disk.contains("grafana"), "{on_disk}");
        assert_eq!(r.shared.file("qa").unwrap().body, "# QA\n\nTest everything, twice.");
        assert_eq!(file::parse("qa", &on_disk).unwrap(), r.shared.file("qa").unwrap());

        // The same text again, an empty export, an unknown name and theuth change nothing.
        assert!(!r.charter_exported("qa", &export).await);
        std::fs::write(&export, "\n\n").unwrap();
        assert!(!r.charter_exported("qa", &export).await);
        std::fs::write(&export, "# Draft\n").unwrap();
        assert!(!r.charter_exported("not-hired-yet", &export).await);
        assert!(!r.charter_exported("theuth", &export).await);
        assert_eq!(r.shared.file("qa").unwrap().body, "# QA\n\nTest everything, twice.");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn remove_deletes_file_and_proposal_and_refuses_theuth() {
        let data = tmp("remove");
        let mut r = roster(&data).await;
        let f = file::parse("qa", "---\ntitle: QA\nproposed_by: agent:ops\npaused: true\n---\nTest.\n").unwrap();
        r.write_file(&f).await.unwrap();
        r.store.insert_proposal("qa", "agent:ops", "approval-1", 0).await.unwrap();

        assert_eq!(r.remove_agent("qa").await, Ok(Some("approval-1".to_owned())));
        assert!(!data.join("agents/qa.md").exists());
        r.reconcile().await;
        assert!(r.shared.file("qa").is_none());
        assert_eq!(r.store.delete_proposal("qa").await.unwrap(), None, "proposal row is gone");

        assert_eq!(r.remove_agent("qa").await, Err("no such agent".to_owned()));
        assert!(r.remove_agent("theuth").await.unwrap_err().contains("built-in"));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn desk_router_reads_the_line() {
        use tasks::DeskRouter as _;
        let s = Arc::new(Shared::default());
        let mut files = BTreeMap::new();
        files.insert("cto".into(), file::parse("cto", "Boss."));
        files.insert("ops".into(), file::parse("ops", "---\nreports_to: cto\n---\nOps."));
        files.insert("bad".into(), file::parse("bad", "---\ncomputer: x\n---\nNope."));
        s.set_files(files, file::default_agent(std::path::Path::new("/nonexistent")));
        let ops = runtime::SessionId::new();
        s.set_desk("ops", ops);
        let desks = Desks(s);
        assert_eq!(desks.manager_of("ops").as_deref(), Some("cto"));
        assert_eq!(desks.manager_of("cto").as_deref(), Some("theuth"));
        assert_eq!(desks.manager_of("theuth"), None, "theuth reports to the user");
        assert_eq!(desks.manager_of("nobody"), None);
        assert_eq!(desks.agent_for(&ops.to_string()).as_deref(), Some("ops"));
        assert_eq!(desks.agent_for("not-a-session"), None);
        assert_eq!(desks.agents(), ["cto", "ops", "theuth"], "valid files, then theuth");
    }

    #[tokio::test]
    async fn lease_gates_desks_and_budget_rows_sum() {
        let data = tmp("lease");
        std::fs::write(data.join("agents/ops.md"), "---\nbudget: 10 USD/month\n---\nOps.\n").unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;
        assert_eq!(r.wanted().len(), 1, "ops while we hold by default; theuth's desk is lazy (opens on its first wake)");
        assert!(r.allowed("theuth"), "theuth's desk is allowed while we hold");

        let lease = |h: &str| mesh::LeaseState {
            holder: Some(mesh::EngineId(h.into())),
            since_ms: 0,
            reason: "test".into(),
        };
        assert_eq!(r.lease.observe(&lease("other")), None, "self unknown: nothing moves");
        assert_eq!(r.lease.set_self("me".into()), Some(false));
        assert!(r.wanted().is_empty(), "no desks wanted on a non-hub");
        assert_eq!(r.lease.observe(&lease("me")), Some(true));
        assert_eq!(r.wanted().len(), 1, "ops again once we hold; theuth stays lazy");

        // Budget rows: one per engine, the month sums them; the override is per month.
        let month = budget::month(now_ms());
        r.store.add_spent("ops", &month, "a", 3.0, "USD", 1).await.unwrap();
        r.store.add_spent("ops", &month, "a", 2.0, "USD", 2).await.unwrap();
        r.store.add_spent("ops", &month, "b", 4.0, "USD", 3).await.unwrap();
        r.store.add_spent("ops", "1999-01", "a", 40.0, "USD", 4).await.unwrap();
        assert_eq!(r.store.spent_month("ops", &month).await.unwrap(), 9.0);
        assert_eq!(r.summary(r.shared.file("ops").unwrap()).await.spent_month, 9.0);
        assert!(!r.store.has_override("ops", &month).await.unwrap());
        r.store.set_override("ops", &month, 5).await.unwrap();
        assert!(r.store.has_override("ops", &month).await.unwrap());
        assert!(!r.store.has_override("ops", "1999-01").await.unwrap());

        // The proposal row answers its approval id once.
        r.store.insert_proposal("qa", "agent:ops", "task-1", 6).await.unwrap();
        assert_eq!(r.store.delete_proposal("qa").await.unwrap().as_deref(), Some("task-1"));
        assert_eq!(r.store.delete_proposal("qa").await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&data);
    }
}
