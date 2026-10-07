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

pub mod attention;
pub mod budget;
pub mod config;
pub mod desk;
pub mod file;
pub mod handoff;
pub mod lease;
pub mod pin;
pub mod proposals;
pub mod routing;
pub mod store;
pub mod tools;
pub mod types;

use std::collections::{BTreeMap, HashMap, HashSet};
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
pub use types::{AgentFile, AgentName, AgentStatus, AgentSummary, Budget, Episode, RunsIn};

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
pub struct ApproveAgent {
    /// The proposal.
    pub name: String,
    /// 130 E4: who approved — `user` | `user@<client>`; the activity row's actor.
    pub actor: String,
}

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

/// `ask` → `Result<runtime::SessionId, String>`: the agent's desk session,
/// opened now when it is not (the Desks row's click, UI-FIXES-3). `Err` when
/// the agent is unknown, paused, a proposal, or this engine does not hold the
/// hub lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenDesk(pub String);

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
    /// 70 DESK-PROTOCOL (f): wake keys whose hold was already written to the thread.
    diagnosed: HashSet<String>,
}

/// ms since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Roster {
    async fn load(
        rt: Runtime,
        cfg: RosterConfig,
        dir: PathBuf,
        store: store::Store,
        shared: Arc<Shared>,
    ) -> Self {
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
            diagnosed: HashSet::new(),
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
                tracing::warn!(
                    "{}/{stem}.md ignored: theuth is reserved",
                    self.dir.display()
                );
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
                        push_charter(
                            before
                                .get(&stem)
                                .and_then(|p| p.as_ref().ok())
                                .map(|f| f.body.as_str()),
                            f,
                        );
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
        self.lease.holding
            && (name == AgentName::DEFAULT || self.wanted().iter().any(|f| f.name.as_str() == name))
    }

    /// Open the desks that are missing, close the ones no longer wanted.
    async fn sync_desks(&mut self) {
        let wanted = self.wanted();
        for f in &wanted {
            if !self.desks.contains_key(f.name.as_str()) && !self.is_closing(f.name.as_str()) {
                self.open_desk(f).await;
            }
        }
        let stale: Vec<String> = self
            .desks
            .keys()
            .filter(|n| !self.allowed(n))
            .cloned()
            .collect();
        for name in stale {
            self.close_desk(&name, "paused").await;
        }
    }

    /// One pinned session titled `<name> · desk`, marked as the agent's desk on
    /// its row, with the file's model and tools ceiling; the episode row; the
    /// read model. One desk per agent (STALE-WAKE-DESKS): the session already
    /// marked `desk = name` here is reopened — a new one is created only when
    /// there is none (or the last was retired for context, [`Self::put_away`]).
    /// Silently skipped when the agent actor is not up yet (`Tick` retries).
    async fn open_desk(&mut self, f: &AgentFile) {
        let name = f.name.as_str();
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let existing = agents
            .ask(agent::DeskSession(name.to_owned()))
            .await
            .ok()
            .flatten();
        let session = match existing {
            Some(s) => s,
            None => {
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
                        title: format!("{name} · desk"),
                    })
                    .await
                {
                    tracing::warn!(agent = %name, "desk not named: {e}");
                }
                if let Err(e) = agents
                    .ask(agent::MarkDesk {
                        session,
                        agent: Some(name.to_owned()),
                    })
                    .await
                {
                    tracing::warn!(agent = %name, "desk not marked: {e}");
                }
                session
            }
        };
        if existing.is_some() {
            // A desk put away by an earlier build was archived; it is the desk again.
            let _ = agents
                .ask(agent::ArchiveSession {
                    session,
                    archived: false,
                })
                .await;
            // Whatever turn claimed wakes on it died with the last process.
            if let Some(t) = tasks::actor() {
                let _ = t.tell(tasks::ReleaseClaims(session.to_string())).await;
            }
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
        if let Some(cap) = f
            .tools
            .as_deref()
            .and_then(|t| t.parse::<::tools::Risk>().ok())
            && let Ok(Some(reg)) =
                ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
        {
            let _ = reg
                .tell(::tools::SetCap {
                    session,
                    cap: Some(cap),
                })
                .await;
        }
        let now = now_ms();
        let episode = match self
            .store
            .open_episode(name, &session.to_string(), now)
            .await
        {
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
                serving: None,
            },
        );
        tracing::info!(agent = %name, %session, "desk opened");
    }

    /// [`OpenDesk`]: the open desk's session, else open one now (theuth's
    /// lazily like [`DeliverWake`] does). The refusals name their gate.
    async fn desk_or_open(&mut self, name: &str) -> Result<runtime::SessionId, String> {
        if let Some(d) = self.desks.get(name) {
            return Ok(d.session);
        }
        let file = if name == AgentName::DEFAULT {
            self.shared.theuth()
        } else {
            self.shared.file(name)
        }
        .ok_or_else(|| "no such agent".to_owned())?;
        if !self.lease.holding {
            return Err("another engine holds the hub lease; desks open there".into());
        }
        if file.paused {
            return Err("agent is paused".into());
        }
        if file.proposed_by.is_some() {
            return Err("agent is a proposal awaiting approval".into());
        }
        if self.is_closing(name) {
            return Err("desk is closing; try again in a moment".into());
        }
        self.open_desk(&file).await;
        self.desks
            .get(name)
            .map(|d| d.session)
            .ok_or_else(|| "desk not opened (agent actor not running)".to_owned())
    }

    /// Close the agent's episode (67 §6.2): the desk stops taking wakes now;
    /// a primed desk gets the journal turn and finishes in `finish_closing`,
    /// an empty one is put away at once. The agent → session mapping is kept
    /// (STALE-WAKE-DESKS): the desk is closed in memory, not severed.
    async fn close_desk(&mut self, name: &str, reason: &str) {
        let Some(d) = self.desks.remove(name) else {
            return;
        };
        // 67 §7.3 last rule (70 DESK-PROTOCOL 3): a task the episode leaves in
        // `doing` with no continuation is blocked and the manager woken.
        if d.primed
            && let Some(t) = tasks::actor()
        {
            match t
                .ask(tasks::EpisodeClosing {
                    agent: name.to_owned(),
                    session: d.session.to_string(),
                })
                .await
            {
                Ok(blocked) if !blocked.is_empty() => {
                    tracing::warn!(agent = %name, ?blocked, "episode closed with tasks left in doing: blocked, manager woken")
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(agent = %name, "episode closing not applied to tasks: {e}")
                }
            }
        }
        if !d.primed {
            self.put_away(name, d.session, &d.episode, reason, None)
                .await;
            return;
        }
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            self.put_away(name, d.session, &d.episode, reason, None)
                .await;
            return;
        };
        if let Err(e) = agents
            .ask(agent::SendInput {
                session: Some(d.session),
                text: desk::notice("journal", desk::CLOSING),
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
        {
            tracing::warn!(agent = %name, "closing turn not started: {e}");
            self.put_away(name, d.session, &d.episode, reason, None)
                .await;
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
        self.put_away(&c.agent, session, &c.episode, &c.reason, Some(&c.text))
            .await;
    }

    /// The episode is over. The session stays the agent's desk — pinned,
    /// marked, mapped — for the next episode (STALE-WAKE-DESKS: one desk per
    /// agent, across idle closes, lease moves and restarts). Only a `context`
    /// close retires it: the mark moves off the row, which is archived under
    /// `<name> · desk (superseded <date>)` so its transcript stays readable,
    /// and the next episode opens a fresh session.
    async fn put_away(
        &mut self,
        name: &str,
        session: runtime::SessionId,
        episode: &str,
        reason: &str,
        journal: Option<&str>,
    ) {
        if let Some(j) = journal.map(str::trim).filter(|j| !j.is_empty()) {
            match desk::write_journal(
                &self.rt.data_dir,
                name,
                j,
                self.cfg.journal_max_bytes as usize,
            ) {
                Ok(p) => tracing::info!(agent = %name, "journal written to {}", p.display()),
                Err(e) => tracing::warn!(agent = %name, "journal not written: {e}"),
            }
        }
        if !episode.is_empty()
            && let Err(e) = self.store.close_episode(episode, now_ms(), reason).await
        {
            tracing::warn!(agent = %name, "episode not closed: {e:#}");
        }
        if reason == "context" {
            if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
                let _ = agents
                    .ask(agent::MarkDesk {
                        session,
                        agent: None,
                    })
                    .await;
                let date = chrono::Local::now().format("%Y-%m-%d");
                let _ = agents
                    .ask(agent::RenameSession {
                        session,
                        title: format!("{name} · desk (superseded {date})"),
                    })
                    .await;
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
            self.shared.clear_desk(name);
            self.shared.forget(session);
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

    /// The wake the desk served is handled (or the task was blocked): the
    /// row is deleted durably (STALE-WAKE-DESKS), so it can neither hold the
    /// agent busy nor be re-armed by a later restart.
    async fn clear_serving(&mut self, name: &str) {
        let Some(s) = self.desks.get_mut(name).and_then(|d| d.serving.take()) else {
            return;
        };
        if let Some(t) = tasks::actor() {
            let _ = t.tell(tasks::FinishWake(s.wake)).await;
        }
    }

    /// A system line in the task's thread that wakes nobody (a progress note).
    async fn thread_note(&self, t: &tasks::Task, text: &str) {
        if let Some(tasks) = tasks::actor() {
            let _ = tasks
                .ask(tasks::CommentTask {
                    id: t.id.clone(),
                    text: text.to_owned(),
                    progress: true,
                    actor: "system".into(),
                    session: None,
                })
                .await;
        }
    }

    /// The desk session's descendants as the agent actor sees them; empty
    /// without the actor.
    async fn children_of(session: runtime::SessionId) -> Vec<agent::AgentInfo> {
        match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
        {
            Some(a) => a
                .ask(agent::ListAgents { session })
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 70 DESK-PROTOCOL 3: a desk turn ended. If it was woken for a task and
    /// the row in [`desk::nudges::TABLE`] for its wake reason says the task is
    /// still unhandled: the first time, log `nudged: …` in the thread and re-run
    /// the turn with the nudge as input; the second time, apply the 67 §7.3
    /// rule (`blocked: no continuation`, manager woken). A handled wake clears
    /// `serving`; a turn that leaves a live child keeps it for the child's
    /// return, which lands in this same session (70 DESK-PROTOCOL 4). A live
    /// child is one running now or spawned for this wake and not finished
    /// ([`desk::nudges::live_child`], WAKE-LIVE-CHILD D1) — a cold descendant
    /// of an earlier turn or process is not one.
    ///
    /// Only a turn the model ended on its own is judged ([`desk::nudges::yielded`]):
    /// a user cancel, the engine shutting down, a provider error or a timeout
    /// leaves the task as it is and hands the wake back to the ledger
    /// ([`tasks::ReleaseWake`], WAKE-LIVE-CHILD D2) — 5 min after a cancel,
    /// the delivery back-off otherwise — so it is delivered again instead of
    /// holding the agent busy for ever.
    async fn judge_turn(&mut self, name: &str, stop_reason: &str) {
        if !desk::nudges::yielded(stop_reason) {
            let Some(s) = self.desks.get_mut(name).and_then(|d| d.serving.take()) else {
                return;
            };
            tracing::info!(
                agent = %name,
                task = ?s.wake.task,
                wake = %s.wake.id,
                stop_reason,
                "wake released: turn did not yield"
            );
            if let Some(t) = tasks::actor() {
                let _ = t
                    .tell(tasks::ReleaseWake {
                        wake: s.wake,
                        min_backoff_ms: if stop_reason == "cancelled" {
                            300_000
                        } else {
                            0
                        },
                    })
                    .await;
            }
            return;
        }
        let Some(d) = self.desks.get(name) else {
            return;
        };
        let Some(s) = d.serving.clone() else { return };
        let session = d.session;
        let Some(id) = s.wake.task.clone() else {
            self.clear_serving(name).await;
            return;
        };
        let Some(tasks) = tasks::actor() else { return };
        let Some(row) = desk::nudges::for_reason(s.wake.reason) else {
            self.clear_serving(name).await;
            return;
        };
        let desk::nudges::Nudge::TurnEnd {
            name: rule,
            unhandled,
            text,
            ..
        } = row
        else {
            return;
        };
        let (Ok(task), Ok(events)) = (
            tasks.ask(tasks::GetTask(id.clone())).await,
            tasks.ask(tasks::TaskEvents(id.clone())).await,
        ) else {
            return;
        };
        let me = format!("agent:{name}");
        let mine: Vec<tasks::TaskEvent> = events
            .into_iter()
            .skip(s.events)
            .filter(|e| e.actor == me)
            .collect();
        let kids = Self::children_of(session).await;
        let live_child = desk::nudges::live_child(&s, &kids);
        if !unhandled(&s, &task, &mine, live_child) {
            self.clear_serving(name).await;
            return;
        }
        if live_child {
            // The child's result continues this episode; judged again then.
            if !s.logged_live_child {
                tracing::info!(
                    agent = %name,
                    task = %id,
                    wake = s.wake.reason.as_str(),
                    children = kids.len(),
                    "turn not judged: a child of the desk is still running"
                );
                if let Some(s) = self.desks.get_mut(name).and_then(|d| d.serving.as_mut()) {
                    s.logged_live_child = true;
                }
            }
            return;
        }
        if !s.nudged {
            let mut nudge = text(&task);
            if s.rounds >= self.cfg.max_rounds {
                nudge.push_str(&format!(
                    " You have used {} of {} follow-up rounds on this task this episode: no more children — write the deliverable and dispose.",
                    s.rounds, self.cfg.max_rounds
                ));
            }
            self.thread_note(
                &task,
                &format!(
                    "nudged: {rule} — task {} still {} after the turn woken `{}`",
                    task.id,
                    task.state.as_str(),
                    s.wake.reason.as_str()
                ),
            )
            .await;
            if let Some(a) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten()
                && a.ask(agent::SendInput {
                    session: Some(session),
                    text: desk::notice("nudge", &nudge),
                    files: vec![],
                    delivery: agent::Delivery::default(),
                    id: None,
                })
                .await
                .is_ok()
                && let Some(d) = self.desks.get_mut(name)
                && let Some(s) = d.serving.as_mut()
            {
                s.nudged = true;
            }
            return;
        }
        let why = format!(
            "nudged once and still {} after the turn woken `{}`: blocked, manager woken",
            task.state.as_str(),
            s.wake.reason.as_str()
        );
        if let Err(e) = tasks
            .ask(tasks::NoContinuation {
                id: id.clone(),
                agent: name.to_owned(),
                why,
            })
            .await
        {
            tracing::warn!(agent = %name, task = %id, "no-continuation not applied: {e}");
        }
        self.clear_serving(name).await;
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
        std::fs::read_to_string(self.dir.join(format!("{name}.md")))
            .map_err(|_| "no such agent".to_owned())
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

    /// A lease event is acted on unless this engine is shutting down: the
    /// desks then simply stop with the process and reopen on the same
    /// sessions at the next start; their tasks are not blocked and their
    /// wakes not released as if the hub had moved.
    fn lease_event_applies(&self) -> bool {
        if self.rt.shutdown.is_cancelled() {
            tracing::info!(
                "lease event during shutdown ignored: desks stay mapped, wakes stay claimed"
            );
            return false;
        }
        true
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
        self.write_text(
            name,
            &file::set_key(&text, "paused", paused.then_some("true")),
        )
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
        let total = (money(&cost.direct.returned) + money(&cost.direct.estimated_for_unavailable))
            as f64
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
        if let Err(e) = self
            .store
            .add_spent(&name, &month, &engine, delta, &currency, now)
            .await
        {
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
            done_when: None,
            attachments: vec![],
            priority: None,
        };
        if let Some(t) = budget::crossed(&f.budget, before, after) {
            let pct = if t == budget::Threshold::Warn {
                "80 %"
            } else {
                "100 %, paused"
            };
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::BudgetThreshold,
                "system",
                &name,
                format!("{name} at {pct} of budget ({after:.2} of {amount:.2} {currency})"),
            )
            .await;
        }
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
                if self
                    .store
                    .has_override(&name, &month)
                    .await
                    .unwrap_or(false)
                {
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
        // WAKE-LIVE-CHILD D5: a desk the clock alone would close stays open
        // while a child, job or PTY of its session works (same probe as the
        // tasks sweep). Probed only for the clock's candidates.
        let mut idle = Vec::new();
        for (name, d) in &self.desks {
            if !d.idle_due(now, self.cfg.episode_idle_secs, false) {
                continue;
            }
            let live_work =
                tasks::ledger::LivenessProbe::liveness(&tasks::AgentProbe, &d.session.to_string())
                    .await
                    .is_some_and(|l| l.active_now);
            if d.idle_due(now, self.cfg.episode_idle_secs, live_work) {
                idle.push(name.clone());
            }
        }
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
        // 70 DESK-PROTOCOL (f): gates re-checked on every wake; a held wake
        // records one diagnostic instead of starting a run.
        let file = self
            .shared
            .file(&m.agent)
            .ok_or_else(|| "no such agent".to_owned())?;
        let gate = if file.paused {
            Some("agent is paused".to_owned())
        } else if let Some(t) = &m.task
            && t.state == tasks::TaskState::Blocked
            && !t.blockers.is_empty()
            && !matches!(
                m.wake.reason,
                tasks::WakeReason::Comment
                    | tasks::WakeReason::Mention
                    | tasks::WakeReason::Answer
                    | tasks::WakeReason::Manual
            )
        {
            Some(format!("task is blocked on {}", t.blockers.join(", ")))
        } else {
            None
        };
        if let Some(why) = gate {
            if self.diagnosed.insert(m.wake.key.clone())
                && let Some(t) = &m.task
            {
                self.thread_note(
                    t,
                    &format!(
                        "wake `{}` for {} held: {why}",
                        m.wake.reason.as_str(),
                        m.agent
                    ),
                )
                .await;
            }
            // `Err` leaves the wake unclaimed and retried by the ledger (5 s → 5 min):
            // it is delivered once the gate opens; the diagnostic is written once.
            return Err(format!("held: {why}"));
        }
        self.diagnosed.remove(&m.wake.key);
        let Some(d) = self.desks.get_mut(&m.agent) else {
            return Err(format!("no desk for {}", m.agent));
        };
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "agent actor is not running".to_owned())?;
        // The charter, journal and manual lead the desk's system prompt
        // (`Identities`, 70 DESK-PROTOCOL); the input is the wake, plus the
        // fit-check roster on an assignment (70 (e)).
        let mut text = desk::wake_text(&m.wake, m.task.as_ref());
        if matches!(
            m.wake.reason,
            tasks::WakeReason::Assigned | tasks::WakeReason::Reassigned
        ) {
            let files: Vec<AgentFile> = self
                .shared
                .files()
                .into_values()
                .filter_map(Result::ok)
                .collect();
            text.push_str("\n\n");
            text.push_str(&desk::roster_block(&m.agent, files.iter()));
        }
        // 68 §4.4: a `Resume` whose checkout session was taken here (the
        // sweep re-pointed its engine at us) continues that session.
        let resumed = m
            .task
            .as_ref()
            .and_then(|t| t.checkout.as_ref())
            .filter(|c| m.wake.reason == tasks::WakeReason::Resume && c.taken)
            .and_then(|c| c.session.parse().ok());
        // 70 DESK-PROTOCOL 3: what this turn is for, judged at its end.
        let events = match (&m.task, tasks::actor()) {
            (Some(t), Some(tasks)) => tasks
                .ask(tasks::TaskEvents(t.id.clone()))
                .await
                .map(|e| e.len())
                .unwrap_or(0),
            _ => 0,
        };
        agents
            .ask(agent::SendInput {
                session: Some(resumed.unwrap_or(d.session)),
                text,
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        d.primed = true;
        d.last_active_ms = now_ms();
        // Every wake is tracked, task or not, so its row is finished when the turn ends.
        d.serving = Some(desk::Serving {
            wake: m.wake.clone(),
            state: m.task.as_ref().map(|t| t.state),
            owner: m.task.as_ref().map(|t| t.owner.clone()),
            events,
            nudged: false,
            rounds: 0,
            children: HashSet::new(),
            logged_live_child: false,
        });
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
                session,
                context,
                stop_reason,
                ..
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
                // 70 DESK-PROTOCOL 3: the nudge point.
                self.judge_turn(&name, &stop_reason).await;
                if let Some(model) = model
                    && self.desks.contains_key(&name)
                {
                    let window = self.window(&model).await;
                    if desk::context_due(context, window, self.cfg.episode_context_pct) {
                        self.close_desk(&name, "context").await;
                    }
                }
            }
            // 67 §6.3: a child works for its parent's agent; 70 DESK-PROTOCOL 2:
            // a desk's child is one follow-up round on the task it serves.
            E::SubagentSpawned { parent, child, .. } => {
                self.shared.inherit(parent, child);
                if let Some(name) = self.desk_name(parent)
                    && let Some(d) = self.desks.get_mut(&name)
                    && let Some(s) = d.serving.as_mut()
                {
                    s.rounds += 1;
                    s.children.insert(child);
                }
            }
            // 67 §6.4: the session's spend moved.
            E::ModelUsageChanged { session, .. } => self.charge(session).await,
            // The thread ended under the desk: the episode is over, no journal possible.
            E::SessionDeleted { session } => {
                self.shared.forget(session);
                self.costs.forget(session);
                if let Some(c) = self.closing.remove(&session) {
                    self.put_away(&c.agent, session, &c.episode, &c.reason, Some(&c.text))
                        .await;
                } else if let Some(name) = self.desk_name(session)
                    && let Some(d) = self.desks.remove(&name)
                {
                    self.shared.clear_desk(&name);
                    if !d.episode.is_empty()
                        && let Err(e) = self
                            .store
                            .close_episode(&d.episode, now_ms(), "thread_end")
                            .await
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
/// Our own shutdown is not a lease loss (STALE-WAKE-DESKS): the mesh emits
/// `Lease { holder: None, reason: "release" }` while tearing itself down,
/// after `rt.shutdown` fired; acting on it closed every desk and blocked
/// their tasks on the way out. See [`Roster::lease_event_applies`].
impl Message<mesh::MeshEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: mesh::MeshEvent, _: &mut Context<Self, Self::Reply>) {
        if let mesh::MeshEvent::Lease(l) = ev
            && self.lease_event_applies()
        {
            let change = self.lease.observe(&l);
            self.lease_changed(change).await;
        }
    }
}

/// 79 §4.9: a pinned task waits `blocked` on `sandbox:<name>`; when that VM
/// takes work again the blocker is cleared, the ledger's unblock wakes the
/// owner, and its desk runs `task_place` again — now with the engine present.
impl Message<sandbox::SandboxEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: sandbox::SandboxEvent, _: &mut Context<Self, Self::Reply>) {
        let sandbox::SandboxEvent::Changed { sandbox: info } = ev else {
            return;
        };
        if !self.lease.holding || !pin::takes_work(&info.state) {
            return;
        }
        let Some(tasks) = tasks::actor() else {
            return;
        };
        let blocked = tasks
            .ask(tasks::ListTasks(tasks::TaskFilter {
                state: Some(vec![tasks::TaskState::Blocked]),
                ..Default::default()
            }))
            .await
            .ok()
            .unwrap_or_default();
        for t in blocked {
            if !t
                .blockers
                .iter()
                .any(|b| pin::blocked_on(b) == Some(info.name.as_str()))
            {
                continue;
            }
            let rest: Vec<String> = t
                .blockers
                .iter()
                .filter(|b| pin::blocked_on(b) != Some(info.name.as_str()))
                .cloned()
                .collect();
            let r = tasks
                .ask(tasks::UpdateTask {
                    id: t.id.clone(),
                    patch: tasks::TaskPatch {
                        blockers: Some(rest),
                        ..Default::default()
                    },
                    actor: "system".into(),
                    session: None,
                })
                .await;
            if let Err(e) = r {
                tracing::warn!(task = %t.id, sandbox = %info.name, error = %e, "sandbox ready: could not unblock");
            }
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

    async fn handle(
        &mut self,
        _: RoutableAgents,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
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
                            runs_in: Default::default(),
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

impl Message<OpenDesk> for Roster {
    type Reply = Result<runtime::SessionId, String>;

    async fn handle(&mut self, m: OpenDesk, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.desk_or_open(&m.0).await
    }
}

impl Message<PutAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PutAgent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.0.name.is_default() {
            return Err(
                "theuth is the built-in default agent: edit prompt.md, not the roster".into(),
            );
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
            && let Some(Budget::Monthly { amount, .. }) =
                self.shared.file(&m.name).map(|f| f.budget)
        {
            let month = budget::month(now_ms());
            if self.store.spent_month(&m.name, &month).await.unwrap_or(0.0) >= amount
                && let Err(e) = self.store.set_override(&m.name, &month, now_ms()).await
            {
                tracing::warn!(agent = %m.name, "budget override not recorded: {e:#}");
            }
        }
        self.set_paused(&m.name, m.paused).await?;
        tasks::activity::record(
            &self.rt,
            tasks::activity::ActivityKind::AgentPaused,
            "user",
            &m.name,
            format!(
                "{} {}",
                if m.paused { "paused" } else { "un-paused" },
                m.name
            ),
        )
        .await;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        Ok(())
    }
}

impl Message<ApproveAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ApproveAgent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if m.name == AgentName::DEFAULT {
            return Err("not a proposal".into());
        }
        let text = self.read_text(&m.name)?;
        let f = self.shared.file(&m.name);
        let approval = self
            .store
            .delete_proposal(&m.name)
            .await
            .map_err(|e| e.to_string())?;
        if approval.is_none() && f.is_none_or(|f| f.proposed_by.is_none()) {
            return Err("not a proposal".into());
        }
        let text = file::set_key(&text, "paused", None);
        let text = file::set_key(&text, "proposed_by", None);
        self.write_text(&m.name, &text).await?;
        let _ = ctx.actor_ref().tell(Reconcile).await;
        tasks::activity::record(
            &self.rt,
            tasks::activity::ActivityKind::AgentHired,
            &m.actor,
            &m.name,
            format!("hired {}", m.name),
        )
        .await;
        // 67 §6.5: the hire task was the approval; close it.
        if let Some(id) = approval {
            close_hire_task(&m.name, id, "approved").await;
        }
        Ok(())
    }
}

impl Message<RemoveAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveAgent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
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
        tasks::activity::record(
            &self.rt,
            tasks::activity::ActivityKind::AgentRemoved,
            "user",
            &m.name,
            format!("removed {}", m.name),
        )
        .await;
        if let Some(id) = approval {
            let note = m.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
            close_hire_task(
                &m.name,
                id,
                &note.map_or_else(|| "rejected".to_owned(), |n| format!("rejected: {n}")),
            )
            .await;
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
    let Some(docs) = document::Handle::lookup() else {
        return;
    };
    let (name, body) = (f.name.as_str().to_owned(), f.body.clone());
    tokio::spawn(async move {
        let binding = document::Binding::AgentCharter(name.clone());
        let Ok(Some(doc)) = docs.actor().ask(document::Lookup { binding }).await else {
            return;
        };
        if let Err(e) = docs
            .import_markdown(doc, body, document::Principal::File)
            .await
        {
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
        let Some(mut f) = self.shared.file(name).filter(|f| !f.name.is_default()) else {
            return false;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
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
        std::fs::remove_file(self.dir.join(format!("{name}.md")))
            .map_err(|_| "no such agent".to_owned())?;
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

    async fn handle(
        &mut self,
        m: ProposeAgent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut f = m.file;
        if f.name.is_default() {
            return Err("theuth is reserved".into());
        }
        if self.shared.files().contains_key(f.name.as_str()) {
            return Err(format!(
                "agent {} exists; pick another name or create a task for it",
                f.name
            ));
        }
        f.paused = true;
        f.proposed_by = Some(m.proposed_by.clone());
        file::parse(f.name.as_str(), &file::render(&f))?;
        self.write_file(&f).await?;
        // 67 §6.5: the hire task, owned by the proposer's manager, is the
        // approval; without the tasks plugin a bare uuid stands in.
        let proposer = m
            .proposed_by
            .strip_prefix("agent:")
            .unwrap_or(&m.proposed_by);
        let manager = if proposer == "user" {
            tasks::Owner::User
        } else {
            self.manager_owner(proposer)
        };
        let approval_id = match self
            .file_task(
                proposals::hire_task(&f, &m.proposed_by, manager),
                &m.proposed_by,
            )
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
        tasks::activity::record(
            &self.rt,
            tasks::activity::ActivityKind::AgentProposed,
            &m.proposed_by,
            f.name.as_str(),
            format!("proposed hiring {} ({})", f.name, f.title),
        )
        .await;
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

    async fn pin_of(&self, agent: &str) -> Result<Option<tasks::place::Pin>, String> {
        let runs_in = self.0.file(agent).map(|f| f.runs_in).unwrap_or_default();
        pin::resolve(agent, &runs_in).await
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
        self.0
            .file(agent)
            .map(|f| f.reports_to)
            .filter(|m| m != "user")
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

/// 70 DESK-PROTOCOL: who a session speaks as. A desk session leads its system
/// prompt with the agent's identity ([`desk::identity`]: charter, capabilities,
/// journal, manual) in place of theuth's persona and `prompt.md`; a session a
/// desk spawned works *for* the agent ([`desk::child_identity`], naming the
/// task the desk has checked out). The user's own sessions get `None`. The
/// desk is found through the lineage (a child's root), so no spawn event has
/// to arrive first. `theuth`'s charter is `prompt.md` with its root-only spans cut.
pub struct Identities {
    shared: Arc<Shared>,
    data_dir: PathBuf,
    max_rounds: u32,
}

/// How long [`Identities::identity`] waits on the tasks actor. The agent
/// actor asks this from its `Identity` handler; the tasks actor's own asks
/// into the agent actor run off its mailbox, so an answer is normally
/// immediate — the bound keeps a regression from freezing every turn start.
const IDENTITY_TASKS_TIMEOUT: Duration = Duration::from_secs(2);

#[async_trait::async_trait]
impl agent::IdentitySource for Identities {
    async fn identity(&self, lineage: &[runtime::SessionId]) -> Option<String> {
        let agent = lineage.iter().find_map(|s| self.shared.agent_of(*s))?;
        let mut f = self.shared.file(&agent)?;
        if f.name.is_default() {
            f.body = agent::frame::root_only(&f.body, false);
        }
        if lineage.len() == 1 {
            let journal = desk::read_journal(&self.data_dir, &agent);
            return Some(desk::identity(&f, journal.as_deref(), self.max_rounds));
        }
        let desk = lineage.last().map(|s| s.to_string());
        let task = match tasks::actor() {
            Some(t) => {
                let asked = t.ask(tasks::ListTasks(tasks::TaskFilter {
                    owner: Some(f.name.peer()),
                    state: Some(vec![tasks::TaskState::Doing]),
                    ..Default::default()
                }));
                match tokio::time::timeout(IDENTITY_TASKS_TIMEOUT, asked).await {
                    Ok(r) => r.ok(),
                    Err(_) => {
                        static WARNED: std::sync::Once = std::sync::Once::new();
                        WARNED.call_once(|| {
                            tracing::warn!(
                                agent = %agent,
                                "tasks did not answer ListTasks in {IDENTITY_TASKS_TIMEOUT:?}; \
                                 child identity without its task"
                            )
                        });
                        None
                    }
                }
                .and_then(|v| {
                    v.into_iter().find(|t| {
                        t.checkout
                            .as_ref()
                            .is_some_and(|c| Some(&c.session) == desk.as_ref())
                    })
                })
            }
            None => None,
        };
        Some(desk::child_identity(&f, task.as_ref()))
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
        // 67 §5.1: `agents` and `budget` replicate, 79 §6.6: `handoffs` too; local only without the plugin.
        match replica::attach(store.pool(), &["agents", "budget", "handoffs"]).await {
            Ok(_) => tracing::info!("roster: agents, budget and handoffs replicate"),
            Err(e) => tracing::debug!("roster: local only ({e})"),
        }
        let attention_store = store.clone();
        let shared = Arc::new(Shared::default());
        let max_rounds = cfg.max_rounds;
        let actor = Roster::spawn_with_mailbox(
            Roster::load(rt.clone(), cfg, dir.clone(), store, shared.clone()).await,
            runtime::subscriber_mailbox(),
        );
        actor.register(ROSTER_ACTOR)?;
        if let Err(e) = attention::Attention::start(rt, &attention_store).await {
            tracing::warn!("attention inbox not started: {e:#}");
        }
        rt.subscribe::<Roster, agent::AgentEvent>(&actor).await;
        // The charter document's exports become the file's body.
        rt.subscribe::<Roster, document::Exported>(&actor).await;
        // 67 §4.4: desks and dispatch follow the lease; self id once the mesh is up.
        rt.subscribe::<Roster, mesh::MeshEvent>(&actor).await;
        // 79 §4.9: a VM that takes work again unblocks the tasks pinned to it.
        rt.subscribe::<Roster, sandbox::SandboxEvent>(&actor).await;
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
            if let Some(t) = rt2.lookup_when::<tasks::Tasks>(tasks::TASKS_ACTOR).await {
                let _ = t
                    .ask(tasks::RegisterDeskRouter(Arc::new(Desks(desks))))
                    .await;
                let _ = me.tell(TasksUp).await;
            }
        });
        let rt2 = rt.clone();
        let identities = Identities {
            shared: shared.clone(),
            data_dir: rt.data_dir.clone(),
            max_rounds,
        };
        tokio::spawn(async move {
            if let Some(a) = rt2.lookup_when::<agent::Agents>(agent::AGENT_ACTOR).await {
                let _ = a
                    .ask(agent::InstallIdentitySource(Arc::new(identities)))
                    .await;
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
        let handoff_tool = handoff::DesktopHandoffTool {
            pool: attention_store.pool().clone(),
            shared: shared.clone(),
            timeout: handoff::timeout(rt),
        };
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
                // 79 §6.6: on every engine, VM or not.
                let _ = reg.ask(::tools::RegisterTool(Arc::new(handoff_tool))).await;
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
        let dir = std::env::temp_dir().join(format!(
            "roster-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
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
        std::fs::write(
            dir.join("ops.md"),
            "---\ntitle: Ops\ntools: mutating\n---\nYou are Ops.\n",
        )
        .unwrap();
        std::fs::write(dir.join("bad.md"), "---\ncomputer: build-A\n---\nNope.\n").unwrap();
        std::fs::write(dir.join("Bad Name.md"), "charter\n").unwrap();
        std::fs::write(data.join("prompt.md"), "Standing instructions.\n").unwrap();
        let r = roster(&data).await;

        let files = r.shared.files();
        assert_eq!(files.len(), 2);
        assert_eq!(files["ops"].as_ref().unwrap().title, "Ops");
        assert!(
            files["bad"]
                .as_ref()
                .unwrap_err()
                .starts_with("front matter")
        );
        assert_eq!(
            r.store.agent_names().await.unwrap(),
            ["ops"],
            "only valid files are mirrored"
        );

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
        assert!(
            r.lease.holding,
            "a lone engine holds the lease from the start"
        );
        assert!(r.wanted().is_empty());
        assert!(r.allowed("theuth"));
        assert!(!r.allowed("ops"));

        std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
        let mut r = r;
        r.reconcile().await;
        assert_eq!(
            r.wanted()
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["ops"]
        );
        assert!(r.allowed("ops") && r.allowed("theuth"));
        r.lease.holding = false;
        assert!(
            !r.allowed("theuth") && r.wanted().is_empty(),
            "duties follow the lease"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn desk_or_open_returns_the_open_desk_and_names_its_refusals() {
        let data = tmp("open-desk");
        std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
        std::fs::write(
            data.join(file::DIR).join("nap.md"),
            "---\npaused: true\n---\nNap.\n",
        )
        .unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;
        assert_eq!(r.desk_or_open("nobody").await, Err("no such agent".into()));
        assert_eq!(r.desk_or_open("nap").await, Err("agent is paused".into()));
        // no agent actor in this test: the open is attempted and reported, not silently swallowed
        assert!(
            r.desk_or_open("theuth")
                .await
                .unwrap_err()
                .contains("not opened")
        );
        let session = runtime::SessionId::new();
        r.desks.insert(
            "ops".into(),
            desk::Desk {
                session,
                episode: String::new(),
                last_active_ms: 0,
                working: false,
                primed: false,
                model: None,
                serving: None,
            },
        );
        assert_eq!(r.desk_or_open("ops").await, Ok(session));
        r.lease.holding = false;
        assert_eq!(
            r.desk_or_open("ops").await,
            Ok(session),
            "an open desk is returned regardless"
        );
        assert!(
            r.desk_or_open("theuth")
                .await
                .unwrap_err()
                .contains("lease")
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// STALE-WAKE-DESKS 3: the mesh's own teardown emits `Lease { holder: None }`
    /// after `rt.shutdown` fired; the roster must not read it as "hub lease
    /// elsewhere". Before shutdown the same event is a real loss.
    #[tokio::test]
    async fn lease_events_are_ignored_during_shutdown() {
        let data = tmp("shutdown-lease");
        let mut r = roster(&data).await;
        r.lease.set_self("me".into());
        assert!(r.lease.holding);
        let release = mesh::LeaseState {
            holder: None,
            since_ms: 0,
            reason: "release".into(),
        };
        // Not shutting down: the loss is applied.
        let actor = Roster::spawn(r);
        actor
            .tell(mesh::MeshEvent::Lease(release.clone()))
            .await
            .unwrap();
        let err = actor
            .ask(OpenDesk("theuth".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("hub lease"), "{err}");
        actor.kill();
        // Shutting down: the same event changes nothing — the desks stay mapped for the next start.
        let mut r = roster(&data).await;
        r.lease.set_self("me".into());
        r.rt.shutdown.cancel();
        assert!(!r.lease_event_applies());
        let actor = Roster::spawn(r);
        actor.tell(mesh::MeshEvent::Lease(release)).await.unwrap();
        let err = actor
            .ask(OpenDesk("theuth".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not opened"), "still holding: {err}");
        actor.kill();
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn knowledge_scope_is_the_agent_and_its_line() {
        use knowledge::Scoper as _;
        let s = Arc::new(Shared::default());
        let mut files = BTreeMap::new();
        files.insert("cto".into(), file::parse("cto", "Boss."));
        files.insert(
            "ops".into(),
            file::parse("ops", "---\nreports_to: cto\n---\nOps."),
        );
        files.insert("mkt".into(), file::parse("mkt", "Marketing."));
        s.set_files(
            files,
            file::default_agent(std::path::Path::new("/nonexistent")),
        );
        let (ops, theuth, user) = (
            runtime::SessionId::new(),
            runtime::SessionId::new(),
            runtime::SessionId::new(),
        );
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
        let f = file::parse(
            "qa",
            "---\ntitle: QA\nmcp: [grafana]\nbudget: 10 EUR/month\n---\nTest everything.\n",
        )
        .unwrap();
        r.write_file(&f).await.unwrap();
        r.reconcile().await;
        assert_eq!(r.shared.file("qa"), Some(f.clone()));
        assert_eq!(
            file::parse(
                "qa",
                &std::fs::read_to_string(data.join("agents/qa.md")).unwrap()
            )
            .unwrap(),
            f
        );
        assert_eq!(r.store.agent_names().await.unwrap(), ["qa"]);

        // Pause rewrites one line; the rest of the file is byte-for-byte.
        let text = r.read_text("qa").unwrap();
        r.write_text("qa", &file::set_key(&text, "paused", Some("true")))
            .await
            .unwrap();
        assert!(r.shared.file("qa").unwrap().paused);
        assert!(
            std::fs::read_to_string(data.join("agents/qa.md"))
                .unwrap()
                .contains("budget: 10 EUR/month\n")
        );

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
        let f = file::parse(
            "qa",
            "---\ntitle: QA\nmcp: [grafana]\nbudget: 10 EUR/month\n---\nTest everything.\n",
        )
        .unwrap();
        r.write_file(&f).await.unwrap();
        let export = data.join("charter.md");

        std::fs::write(&export, "# QA\n\nTest everything, twice.\n").unwrap();
        assert!(r.charter_exported("qa", &export).await);
        let on_disk = std::fs::read_to_string(data.join("agents/qa.md")).unwrap();
        assert!(on_disk.contains("budget: 10 EUR/month\n"), "{on_disk}");
        assert!(on_disk.contains("grafana"), "{on_disk}");
        assert_eq!(
            r.shared.file("qa").unwrap().body,
            "# QA\n\nTest everything, twice."
        );
        assert_eq!(
            file::parse("qa", &on_disk).unwrap(),
            r.shared.file("qa").unwrap()
        );

        // The same text again, an empty export, an unknown name and theuth change nothing.
        assert!(!r.charter_exported("qa", &export).await);
        std::fs::write(&export, "\n\n").unwrap();
        assert!(!r.charter_exported("qa", &export).await);
        std::fs::write(&export, "# Draft\n").unwrap();
        assert!(!r.charter_exported("not-hired-yet", &export).await);
        assert!(!r.charter_exported("theuth", &export).await);
        assert_eq!(
            r.shared.file("qa").unwrap().body,
            "# QA\n\nTest everything, twice."
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn remove_deletes_file_and_proposal_and_refuses_theuth() {
        let data = tmp("remove");
        let mut r = roster(&data).await;
        let f = file::parse(
            "qa",
            "---\ntitle: QA\nproposed_by: agent:ops\npaused: true\n---\nTest.\n",
        )
        .unwrap();
        r.write_file(&f).await.unwrap();
        r.store
            .insert_proposal("qa", "agent:ops", "approval-1", 0)
            .await
            .unwrap();

        assert_eq!(
            r.remove_agent("qa").await,
            Ok(Some("approval-1".to_owned()))
        );
        assert!(!data.join("agents/qa.md").exists());
        r.reconcile().await;
        assert!(r.shared.file("qa").is_none());
        assert_eq!(
            r.store.delete_proposal("qa").await.unwrap(),
            None,
            "proposal row is gone"
        );

        assert_eq!(r.remove_agent("qa").await, Err("no such agent".to_owned()));
        assert!(
            r.remove_agent("theuth")
                .await
                .unwrap_err()
                .contains("built-in")
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn desk_router_reads_the_line() {
        use tasks::DeskRouter as _;
        let s = Arc::new(Shared::default());
        let mut files = BTreeMap::new();
        files.insert("cto".into(), file::parse("cto", "Boss."));
        files.insert(
            "ops".into(),
            file::parse("ops", "---\nreports_to: cto\n---\nOps."),
        );
        files.insert(
            "bad".into(),
            file::parse("bad", "---\ncomputer: x\n---\nNope."),
        );
        s.set_files(
            files,
            file::default_agent(std::path::Path::new("/nonexistent")),
        );
        let ops = runtime::SessionId::new();
        s.set_desk("ops", ops);
        let desks = Desks(s);
        assert_eq!(desks.manager_of("ops").as_deref(), Some("cto"));
        assert_eq!(desks.manager_of("cto").as_deref(), Some("theuth"));
        assert_eq!(
            desks.manager_of("theuth"),
            None,
            "theuth reports to the user"
        );
        assert_eq!(desks.manager_of("nobody"), None);
        assert_eq!(desks.agent_for(&ops.to_string()).as_deref(), Some("ops"));
        assert_eq!(desks.agent_for("not-a-session"), None);
        assert_eq!(
            desks.agents(),
            ["cto", "ops", "theuth"],
            "valid files, then theuth"
        );
    }

    #[tokio::test]
    async fn lease_gates_desks_and_budget_rows_sum() {
        let data = tmp("lease");
        std::fs::write(
            data.join("agents/ops.md"),
            "---\nbudget: 10 USD/month\n---\nOps.\n",
        )
        .unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;
        assert_eq!(
            r.wanted().len(),
            1,
            "ops while we hold by default; theuth's desk is lazy (opens on its first wake)"
        );
        assert!(
            r.allowed("theuth"),
            "theuth's desk is allowed while we hold"
        );

        let lease = |h: &str| mesh::LeaseState {
            holder: Some(mesh::EngineId(h.into())),
            since_ms: 0,
            reason: "test".into(),
        };
        assert_eq!(
            r.lease.observe(&lease("other")),
            None,
            "self unknown: nothing moves"
        );
        assert_eq!(r.lease.set_self("me".into()), Some(false));
        assert!(r.wanted().is_empty(), "no desks wanted on a non-hub");
        assert_eq!(r.lease.observe(&lease("me")), Some(true));
        assert_eq!(
            r.wanted().len(),
            1,
            "ops again once we hold; theuth stays lazy"
        );

        // Budget rows: one per engine, the month sums them; the override is per month.
        let month = budget::month(now_ms());
        r.store
            .add_spent("ops", &month, "a", 3.0, "USD", 1)
            .await
            .unwrap();
        r.store
            .add_spent("ops", &month, "a", 2.0, "USD", 2)
            .await
            .unwrap();
        r.store
            .add_spent("ops", &month, "b", 4.0, "USD", 3)
            .await
            .unwrap();
        r.store
            .add_spent("ops", "1999-01", "a", 40.0, "USD", 4)
            .await
            .unwrap();
        assert_eq!(r.store.spent_month("ops", &month).await.unwrap(), 9.0);
        assert_eq!(
            r.summary(r.shared.file("ops").unwrap()).await.spent_month,
            9.0
        );
        assert!(!r.store.has_override("ops", &month).await.unwrap());
        r.store.set_override("ops", &month, 5).await.unwrap();
        assert!(r.store.has_override("ops", &month).await.unwrap());
        assert!(!r.store.has_override("ops", "1999-01").await.unwrap());

        // The proposal row answers its approval id once.
        r.store
            .insert_proposal("qa", "agent:ops", "task-1", 6)
            .await
            .unwrap();
        assert_eq!(
            r.store.delete_proposal("qa").await.unwrap().as_deref(),
            Some("task-1")
        );
        assert_eq!(r.store.delete_proposal("qa").await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&data);
    }
}
