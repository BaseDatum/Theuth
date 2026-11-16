//! Roster plugin (design doc 67 §6): agents as files in `<data_dir>/agents/`,
//! each with one pinned desk session on the hub, episodes with a journal,
//! budgets, proposals. It implements [`tasks::DeskRouter`] (wakes reach a
//! desk through it) and, later, the knowledge owner/visibility and the MCP
//! allowlist consulted by other plugins.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`ROSTER_ACTOR`] actor ([`Roster`]): [`ListAgents`],
//!   [`GetAgent`], [`PutAgent`], [`PauseAgent`], [`ApproveAgent`], [`ProposeAgent`],
//!   [`RoutableAgents`], [`TheuthModel`], [`SetTheuthModel`], [`TheuthEffort`],
//!   [`SetTheuthEffort`]
//! - [`file::parse`]: `agents/<name>.md` → [`AgentFile`]
//! - [`Desks`]: the shared read model handed to the tasks plugin

/// Documents RPC methods and typed clients.
pub mod rpc_documents;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc::rpc_module(server.clone()),
        rpc_documents::rpc_module(server),
    ])
    .expect("roster RPC composition")
}

/// Boards RPC methods and typed client.
pub mod rpc;
/// Provider-owned helpers used by RPC method owners.
pub mod rpc_support;
pub use rpc::BoardsApiClient;
pub mod alert;
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
pub use types::{
    AgentFile, AgentName, AgentStatus, AgentSummary, Budget, EngineAgent, Episode, RunsIn,
};

/// Registry name of the [`Roster`] actor.
pub const ROSTER_ACTOR: &str = "roster";

/// Sqlite file under `data_dir` (67 §6).
pub const DB: &str = "roster.sqlite";

/// `ask` → `Result<Vec<AgentSummary>, String>`: `theuth` first, then every
/// file in the directory (invalid ones as [`AgentStatus::Invalid`]) with its
/// live status.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAgents;

/// `ask` → `Result<Vec<EngineAgent>, String>`: 182 D8 — every engine's
/// agents from the replicated per-engine list (this engine's rows included),
/// by name then engine.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListEngineAgents;

/// `ask` → `u32`: desks with a turn running now (178 F4: a desk only
/// serving a wake — waiting on a child — does not count; the dreaming gate).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct DesksRunning;

/// `ask` → `Result<AgentFile, String>`. `theuth` is synthesised. `Err("no such agent")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetAgent(pub String);

/// `ask` → `Result<(), String>`: write `agents/<name>.md` from the file
/// (create or replace); the watcher reconciles and the record replicates. A
/// changed `model:` re-points the open desk (and its children) at once.
/// `theuth` is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutAgent(pub AgentFile);

/// An approved server add grants only the caller's agent, through its file.
struct GrantMcp {
    agent: String,
    server: String,
}

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

/// `ask` → `Option<agent::ModelInfo>`: the live `roster.theuth_model` (148
/// E3) resolved through the catalog — what the built-in `theuth` agent's
/// desk and its work run on. `None` when unset (the default for new sessions);
/// an id the catalog no longer resolves answers as a bare `ModelInfo` naming
/// itself (F7), so the row shows the stale value.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct TheuthModel;

/// `ask` → `Result<Option<agent::ModelInfo>, String>`: set or clear (`None`)
/// `roster.theuth_model` (148 E3). Validated through the catalog (an unknown
/// or un-keyed id is refused like `agent::SetSubagentModel`), written to
/// `config.yaml`, kept live, mirrored into the synthesised `theuth` file;
/// an open theuth desk switches now and its native children follow. 194
/// D4c/d: `None` clears `roster.theuth_effort`; a model that does not take
/// it drops it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetTheuthModel {
    /// `provider/model`, bare id, or `None` for the default.
    pub model: Option<String>,
}

/// `ask` → `Option<String>`: the live `roster.theuth_effort` (194 rows 4–5);
/// `None` when unset (the default for new sessions).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct TheuthEffort;

/// `ask` → `Result<Option<String>, String>`: set or clear (`None`)
/// `roster.theuth_effort` (194 rows 4–5). Validated against theuth's
/// effective model (`roster.theuth_model`, else the default for new sessions)
/// with `agent::ValidateEffort`, written to `config.yaml`, kept live, carried
/// by the synthesised `theuth` file; its open desk switches now and the
/// desk's native children start on it. The reply is the stored value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetTheuthEffort {
    /// Effort keyword or token budget, or `None` for the default.
    pub effort: Option<String>,
}

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

/// Internal `tell`: the tasks actor is up — its claims of desks reopened before it.
#[derive(Clone, Copy, Debug)]
struct TasksUp;

/// Internal `tell` (182 M2a): the first sync with the mesh is done — lift the
/// boot holds and open the desks.
#[derive(Clone, Copy, Debug)]
struct Synced;

/// Internal `tell`: the directory changed (watcher) — re-read it.
#[derive(Clone, Copy, Debug)]
struct Reconcile;

/// Internal `tell`, every [`TICK`]: idle episodes, closing deadlines, desks
/// that could not open yet (the agent actor was not up).
#[derive(Clone, Copy, Debug)]
struct Tick;

/// Episode housekeeping cadence.
const TICK: Duration = Duration::from_secs(60);

/// 178 W5: how long a serving desk's session tree must be inactive before
/// the `Tick` backstop judges its wake (2 × [`TICK`]).
const SERVING_GRACE_MS: u64 = 2 * TICK.as_millis() as u64;

/// A closing turn that never finishes is given up after this.
const CLOSING_GRACE_MS: u64 = 10 * 60 * 1000;

/// 179: the episode-close reason of a pause, and the tree suspension's.
const PAUSED: &str = agent::supervisor::pause::PAUSED;

/// 179: a paused agent's desk tree, on hold in the agent actor until the
/// un-pause resumes it.
struct PausedDesk {
    /// The desk session (the tree's root).
    session: runtime::SessionId,
    /// The wake the desk served when the pause closed it (D2: its claim
    /// stands and the resumed work continues it).
    serving: Option<desk::Serving>,
    /// The desk turn's stop reason when it finished inside the pause's
    /// drain window instead of being suspended: judged at the un-pause.
    finished: Option<String>,
}

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
    /// 137 D12 / R1-D7: `agent.endless` as booted — the default memory mode
    /// of desks without a `memory:` field and the default view budget.
    endless: agent::EndlessConfig,
    /// The `Tick`'s liveness probe (the tasks sweep's); a fake in tests.
    probe: Arc<dyn tasks::ledger::LivenessProbe>,
    /// 178: desks reopened before the tasks plugin was up; their dead turns'
    /// claims are released at [`TasksUp`].
    unreleased: Vec<runtime::SessionId>,
    /// 178 F4: `AgentSummary::waiting_on` per desk ([`Self::refresh_waiting`]).
    waiting: HashMap<String, Vec<String>>,
    /// 179: agents whose desk tree this process holds paused.
    held: HashMap<String, PausedDesk>,
    /// 179: paused agents found without a desk session (not asked again).
    deskless: HashSet<String>,
}

/// The `agent` config section's `endless` key alone (the roster reads
/// another crate's section by name, as `handoff.rs` does for `sandbox`).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct AgentSection {
    endless: agent::EndlessConfig,
}

/// ms since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 148 E3/F7: the agent's configured model as the canonical catalog id, or
/// `None` when it has none — or when the id does not resolve or its provider
/// has no credentials (`agent::ResolveModel`, 148 R4): warned, and the desk
/// follows the default instead of opening on a model that cannot answer.
async fn resolve_configured(
    agents: &ActorRef<agent::Agents>,
    name: &str,
    configured: Option<String>,
) -> Option<String> {
    let m = configured?;
    match agents.ask(agent::ResolveModel { model: m.clone() }).await {
        Ok(info) => Some(info.id),
        Err(e) => {
            tracing::warn!(
                agent = %name, model = %m,
                "configured model not usable, the default stands in: {e}"
            );
            None
        }
    }
}

/// 194 D4b–d: `memory_effort` follows `memory_model` — cleared with it,
/// dropped when a changed `memory_model` does not take it, refused with
/// `agent::ValidateEffort`'s words otherwise (a new file, the same model).
/// R1: with the agent actor unreachable the word alone is checked
/// (`agent::effort_of`), never stored unchecked.
async fn settle_memory_effort(f: &mut AgentFile, old: Option<&AgentFile>) -> Result<(), String> {
    f.memory_effort = f
        .memory_effort
        .take()
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty());
    if f.memory_model.is_none() {
        f.memory_effort = None;
    }
    let Some(effort) = f.memory_effort.clone() else {
        return Ok(());
    };
    let word = |e: &str| {
        agent::effort_of(e)
            .map(|_| ())
            .ok_or_else(|| format!("unknown effort {e}"))
    };
    let checked = match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => {
            let check = agent::ValidateEffort {
                model: f.memory_model.clone(),
                effort: effort.clone(),
            };
            match agents.ask(check).await {
                Ok(()) => Ok(()),
                Err(kameo::error::SendError::HandlerError(why)) => Err(why),
                Err(e) => {
                    tracing::warn!("roster: effort check unavailable ({e}): the word alone");
                    word(&effort)
                }
            }
        }
        _ => word(&effort),
    };
    match checked {
        Err(_) if old.is_some_and(|o| o.memory_model != f.memory_model) => {
            f.memory_effort = None;
            Ok(())
        }
        Err(why) => Err(format!("memory_effort: {why}")),
        Ok(()) => Ok(()),
    }
}

/// 148 E3: the model a desk runs on — its (resolved) configured one, else
/// the live default for new sessions; `None` when the agent actor cannot say.
async fn effective_model(
    agents: &ActorRef<agent::Agents>,
    configured: Option<String>,
) -> Option<String> {
    match configured {
        Some(m) => Some(m),
        None => agents.ask(agent::GetDefaultModel).await.ok(),
    }
}

/// 148 F7: a stored model id the catalog no longer resolves, shown as itself
/// (name = id, provider = the part before `/`) so a Settings row names the
/// stale value instead of "Same as new sessions".
fn stale_info(id: String) -> agent::ModelInfo {
    agent::ModelInfo {
        provider: id.split_once('/').map_or("", |(p, _)| p).to_owned(),
        name: id.clone(),
        id,
        context: 0,
        reasoning: false,
        image: false,
        efforts: Vec::new(),
        budget_min: None,
        budget_max: None,
        local: false,
    }
}

/// Put a desk session on `model` — that session only (148 R1: `SetModel`
/// with a session never touches the default). 148 F4: a session whose row
/// already stores `model` is left alone (no session actor spawned, no
/// `ModelChanged`, a stored effort kept). The canonical id, or `None` when
/// the switch failed (logged).
async fn switch_session(
    agents: &ActorRef<agent::Agents>,
    name: &str,
    session: runtime::SessionId,
    model: String,
) -> Option<String> {
    if agents
        .ask(agent::SessionModel(session))
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some(model.as_str())
    {
        return Some(model);
    }
    match agents
        .ask(agent::SetModel {
            session: Some(session),
            model,
            manual: false,
        })
        .await
    {
        Ok(info) => Some(info.id),
        Err(e) => {
            tracing::warn!(agent = %name, "desk model not switched: {e}");
            None
        }
    }
}

/// 148 E3: validate a model id for [`SetTheuthModel`] the way
/// `agent::SetSubagentModel` does — `agent::ResolveModel` resolves it through
/// the catalog and refuses a provider without credentials; no side effects.
async fn resolve_keyed(
    agents: &ActorRef<agent::Agents>,
    id: &str,
) -> Result<agent::ModelInfo, String> {
    agents
        .ask(agent::ResolveModel {
            model: id.to_owned(),
        })
        .await
        .map_err(|e| e.to_string())
}

impl Roster {
    async fn load(
        rt: Runtime,
        cfg: RosterConfig,
        dir: PathBuf,
        store: store::Store,
        shared: Arc<Shared>,
    ) -> Self {
        let endless = rt
            .section::<AgentSection>("agent")
            .map(|s| s.endless)
            .unwrap_or_default();
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
            endless,
            probe: Arc::new(tasks::AgentProbe),
            unreleased: Vec::new(),
            waiting: HashMap::new(),
            held: HashMap::new(),
            deskless: HashSet::new(),
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
        // 148 E2: open desks whose file `model:` changed on disk (an edit
        // the watcher saw). `PutAgent` updates the read model before this
        // runs, so it applies its own change (F1).
        let mut remodelled = Vec::new();
        // 137G: the same for `memory_model:` (the desk's compactor default)
        // and (194) its `memory_effort:`.
        let mut rememory = Vec::new();
        let mut reroute = Vec::new();
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
                        let old = before.get(&stem).and_then(|p| p.as_ref().ok());
                        if old.is_some_and(|o| o.isolated_memory != f.isolated_memory) {
                            reroute.push(stem.clone());
                        }
                        // 194 R1: an `effort:`-only edit applies too (the pair travels together).
                        if old.is_some_and(|o| o.model != f.model || o.effort != f.effort) {
                            remodelled.push((stem.clone(), f.model.clone()));
                        }
                        if old.is_some_and(|o| {
                            (&o.memory_model, &o.memory_effort)
                                != (&f.memory_model, &f.memory_effort)
                        }) {
                            rememory.push((
                                stem.clone(),
                                f.memory_model.clone(),
                                f.memory_effort.clone(),
                            ));
                        }
                        push_charter(old.map(|f| f.body.as_str()), f);
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
        for name in reroute {
            if let Err(e) = self.retire_memory_context(&name).await {
                tracing::warn!(agent = %name, "memory context not retired: {e}");
            }
        }
        self.shared.set_files(files, self.theuth_file());
        self.publish_agents().await;
        // 186 review 2: a contact's inbox shared in full follows its agent's `tools:` now.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            let _ = agents
                .tell(agent::communication::network::InboxPersonasChanged)
                .await;
        }
        self.sync_desks().await;
        for (name, model) in remodelled {
            self.apply_desk_model(&name, model).await;
        }
        for (name, model, effort) in rememory {
            self.apply_desk_memory_model(&name, model, effort).await;
        }
    }

    /// 182 D8: this engine's row set in the replicated per-engine agent list
    /// — every valid file plus theuth — once the mesh named the engine.
    async fn publish_agents(&self) {
        let Some(engine) = self.lease.self_id.clone() else {
            return;
        };
        let mut files: Vec<AgentFile> = self
            .shared
            .files()
            .into_values()
            .filter_map(Result::ok)
            .collect();
        files.extend(self.shared.theuth());
        if let Err(e) = self.store.put_engine_agents(&engine, &files).await {
            tracing::warn!("engine_agents not written: {e:#}");
        }
    }

    /// Agents whose desk opens eagerly: every valid, unpaused, approved file
    /// (67 §6.2, §6.5). 182 D4: every engine runs its own roster's desks —
    /// for the tasks homed on it — whoever holds the lease.
    /// `theuth`'s desk is not here: it opens on the first wake addressed to
    /// it ([`DeliverWake`]), so an engine with no `agents/*.md` creates no
    /// session at all (67 decision 5: a one-laptop user never sees any of this).
    fn wanted(&self) -> Vec<AgentFile> {
        self.shared
            .files()
            .into_values()
            .filter_map(Result::ok)
            .filter(|f| !f.paused && f.proposed_by.is_none())
            .collect()
    }

    /// Whether a desk for `name` may stay open: wanted, or `theuth`'s.
    fn allowed(&self, name: &str) -> bool {
        name == AgentName::DEFAULT || self.wanted().iter().any(|f| f.name.as_str() == name)
    }

    /// Open the desks that are missing, close the ones no longer wanted;
    /// 179: first suspend or resume the trees of agents paused or un-paused.
    /// 182 M2a: no desk opens before the first sync with the mesh (its tasks'
    /// homes may have moved while this engine was away); [`Synced`] runs this again.
    async fn sync_desks(&mut self) {
        self.sync_pauses().await;
        let wanted = if mesh::synced() {
            self.wanted()
        } else {
            Vec::new()
        };
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
        // A paused agent's desk was closed by `sync_pauses`: these are
        // removed files and proposals.
        for name in stale {
            self.close_desk(&name, "closed").await;
        }
    }

    /// 179 D1/D2: an agent whose file says `paused` gets its desk closed and
    /// its session tree suspended and held (once per pause, at boot too); one
    /// whose file no longer says so is resumed, then its desk reopens. Holds
    /// the agent actor has that this roster did not place (the boot hold, an
    /// invalid file) are reconciled too; a removed agent's is released.
    async fn sync_pauses(&mut self) {
        let files = self.shared.files();
        for (name, parsed) in &files {
            let Ok(f) = parsed else { continue };
            match (f.paused, self.held.contains_key(name)) {
                (true, false) if !self.deskless.contains(name) => self.pause_tree(name).await,
                (false, true) => self.resume_tree(f).await,
                (false, false) => {
                    self.deskless.remove(name);
                }
                _ => {}
            }
        }
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        // A removed agent: its hold goes without a resume (no charter would
        // run them); the agent actor closes its open turns.
        let gone: Vec<String> = self
            .held
            .keys()
            .filter(|n| !files.contains_key(*n))
            .cloned()
            .collect();
        for name in gone {
            if let Some(p) = self.held.remove(&name) {
                tracing::info!(agent = %name, desk = %p.session, "179: paused agent removed; its hold released");
                let _ = agents
                    .ask(agent::supervisor::ReleaseTree { desk: p.session })
                    .await;
            }
        }
        // 182 M2a: the desk trees the agent actor held at boot wait for the first
        // sync with the mesh — their turns may serve a task that moved meanwhile.
        if !mesh::synced() {
            return;
        }
        let held = agents
            .ask(agent::supervisor::HeldDesks)
            .await
            .unwrap_or_default();
        for (desk, agent) in held {
            let Some(name) = agent.filter(|n| !self.held.contains_key(n)) else {
                continue;
            };
            let file = if name == AgentName::DEFAULT {
                self.shared.theuth().map(Ok)
            } else {
                files.get(&name).cloned()
            };
            match file {
                None => {
                    tracing::info!(agent = %name, %desk, "179: held desk of a removed agent released");
                    let _ = agents.ask(agent::supervisor::ReleaseTree { desk }).await;
                }
                Some(Ok(f)) => {
                    self.shared.set_desk(&name, desk);
                    if !f.paused && self.serves_moved_task(desk).await {
                        tracing::info!(agent = %name, %desk, "182: the held turn served a task now worked on another engine; closed, not resumed");
                        let _ = agents.ask(agent::supervisor::ReleaseTree { desk }).await;
                        continue;
                    }
                    self.held.insert(
                        name.clone(),
                        PausedDesk {
                            session: desk,
                            serving: None,
                            finished: None,
                        },
                    );
                    if !f.paused {
                        self.resume_tree(&f).await;
                    }
                }
                // An invalid file: its text still decides; a lifted hold
                // resumes the tree, the desk stays shut until the file parses.
                Some(Err(_)) => {
                    let paused = self
                        .read_text(&name)
                        .is_ok_and(|t| agent::supervisor::pause::agent_file_paused(&t));
                    if !paused {
                        tracing::info!(agent = %name, %desk, "179: invalid file no longer paused; hold lifted");
                        let _ = agents.ask(agent::supervisor::ResumeTree { desk }).await;
                    }
                }
            }
        }
    }

    /// 179 D1: close the desk without its journal turn or the episode's
    /// "no continuation" rule (D4: checkouts survive), keep the agent ↔ desk
    /// mapping (D8), and have the agent actor suspend the tree.
    async fn pause_tree(&mut self, name: &str) {
        if self.desks.contains_key(name) {
            self.close_desk(name, PAUSED).await;
        }
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let session = match self.shared.desk(name) {
            Some(s) => Some(s),
            None => agents
                .ask(agent::DeskSession(name.to_owned()))
                .await
                .ok()
                .flatten(),
        };
        // Never had a desk: nothing runs, nothing to hold — and a paused
        // agent gets none, so it is not asked again until un-paused.
        let Some(session) = session else {
            self.deskless.insert(name.to_owned());
            return;
        };
        self.shared.set_desk(name, session);
        self.held.entry(name.to_owned()).or_insert(PausedDesk {
            session,
            serving: None,
            finished: None,
        });
        tracing::info!(agent = %name, desk = %session, "179: paused: desk closed, its session tree suspending");
        if let Err(e) = agents
            .tell(agent::supervisor::PauseTree { desk: session })
            .await
        {
            tracing::warn!(agent = %name, "179: tree not suspended: {e}");
        }
    }

    /// 179 D2: resume the tree (the suspended turns continue with their
    /// remaining caps; an un-pause inside the pause's drain waits for it),
    /// then reopen the desk through the [`Self::wanted`] gates. The served
    /// wake's claim stands when work resumed or the desk's turn ended inside
    /// the drain window (judged here) — so it is not delivered again; a
    /// resumed desk turn holds the agent's other wakes until it ends
    /// ([`desk::Shared::set_resuming`]).
    async fn resume_tree(&mut self, f: &AgentFile) {
        let name = f.name.as_str();
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let Some(p) = self.held.remove(name) else {
            return;
        };
        let report = match agents
            .ask(agent::supervisor::ResumeTree { desk: p.session })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(agent = %name, "179: tree not resumed: {e}");
                Default::default()
            }
        };
        let desk_turn = report.resumed.iter().any(|(s, _)| *s == p.session);
        let keep = !report.resumed.is_empty() || p.finished.is_some();
        tracing::info!(agent = %name, desk = %p.session, turns = report.resumed.len(), replayed = report.replayed, desk_turn, "179: un-paused: tree resumed, desk reopening");
        if self.wanted().iter().any(|w| w.name == f.name) {
            self.open_desk_with(f, !keep).await;
        }
        let serving = match (keep, p.serving) {
            (false, _) => None,
            (true, Some(s)) => Some(s),
            (true, None) => Self::claimed_serving(p.session).await,
        };
        if let Some(d) = self.desks.get_mut(name) {
            d.primed |= serving.is_some() || desk_turn;
            d.serving = serving;
            d.working |= desk_turn;
        }
        if desk_turn {
            self.shared.set_resuming(name, true);
        } else if let Some(t) = tasks::actor() {
            let _ = t.tell(tasks::Unpaused(name.to_owned())).await;
        }
        if let Some(stop) = p.finished {
            self.judge_turn(name, &stop).await;
        }
    }

    /// 182 M2a: the open turn of `desk`'s tree (held since boot) served a task
    /// that another engine works now — the last input to the desk names it
    /// (`Wake: task <id>` / a nudge `Task <id>`).
    async fn serves_moved_task(&self, desk: runtime::SessionId) -> bool {
        let (Some(t), Ok(Some(agents))) = (
            tasks::actor(),
            ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR),
        ) else {
            return false;
        };
        let Ok(store) = agents.ask(agent::DurableStore).await else {
            return false;
        };
        let Ok((Some(request), _)) = store.last_exchange(desk).await else {
            return false;
        };
        let Some(id) = task_id_in(&request) else {
            return false;
        };
        t.ask(tasks::GetTask(tasks::TaskId(id.to_owned())))
            .await
            .is_ok_and(|task| !self.works_here(&task))
    }

    /// 182 D3: this engine works `task` — homed here, or legacy while it holds the lease.
    fn works_here(&self, task: &tasks::Task) -> bool {
        match &task.home {
            // Before the mesh named this engine: nothing is known to have moved.
            Some(h) => self.lease.self_id.as_deref().is_none_or(|me| me == h),
            None => self.lease.holding,
        }
    }

    /// 179 D2 after a restart: the wake a desk's claim still names, as the
    /// `serving` it was (its delivery-time task event count unknown: 0).
    async fn claimed_serving(session: runtime::SessionId) -> Option<desk::Serving> {
        let t = tasks::actor()?;
        let wake = t
            .ask(tasks::ClaimedWake(session.to_string()))
            .await
            .ok()??;
        let task = match &wake.task {
            Some(id) => t.ask(tasks::GetTask(id.clone())).await.ok(),
            None => None,
        };
        Some(desk::Serving {
            state: task.as_ref().map(|t| t.state),
            owner: task.as_ref().map(|t| t.owner.clone()),
            wake,
            events: 0,
            nudged: false,
            rounds: 0,
            children: HashSet::new(),
            logged_live_child: false,
            turn_seen: false,
        })
    }

    /// One pinned session titled `<name> · desk`, marked as the agent's desk on
    /// its row, with the file's model and tools ceiling; the episode row; the
    /// read model. One desk per agent (STALE-WAKE-DESKS): the session already
    /// marked `desk = name` here is reopened — a new one is created only when
    /// there is none (or the last was retired for context, [`Self::put_away`]).
    /// Silently skipped when the agent actor is not up yet (`Tick` retries).
    async fn open_desk(&mut self, f: &AgentFile) {
        self.open_desk_with(f, true).await;
    }

    /// [`Self::open_desk`]; `release: false` (179 D2, an un-pause whose
    /// suspended work resumed) keeps the reopened desk's claims — they are
    /// the resumed turns' wake, not a dead turn's.
    async fn open_desk_with(&mut self, f: &AgentFile, release: bool) {
        let name = f.name.as_str();
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let mut existing = agents
            .ask(agent::DeskSession(name.to_owned()))
            .await
            .ok()
            .flatten();
        if let Some(session) = existing {
            let identity = agents.ask(agent::DurableStore).await.ok();
            let mismatch = match identity {
                Some(store) => match store.memory_identity(session).await {
                    Ok((Some((_, isolated, _)), _)) => isolated != f.isolated_memory,
                    Ok((None, _)) => f.isolated_memory,
                    Err(_) => true,
                },
                None => true,
            };
            if mismatch {
                if let Err(e) = self.retire_memory_context(name).await {
                    tracing::warn!(agent = %name, "memory context not retired: {e}");
                    return;
                }
                existing = None;
            }
        }
        // 137 D12: the file's `memory:`, else `agent.endless.default_desk`.
        let wanted = self.desk_mode(f);
        // 148 F7: a `model:` the catalog no longer resolves does not keep
        // the desk shut — the default stands in (warned).
        let configured = resolve_configured(&agents, name, f.model.clone()).await;
        let session = match existing {
            Some(s) => s,
            None => {
                let open = agents
                    .ask(agent::CreateSession {
                        options: agent::CreateSessionOptions {
                            model: configured.clone(),
                            memory: Some(wanted),
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
        // Backfill legacy desk provenance before any turn, too.
        if let Err(e) = agents
            .ask(agent::MarkDesk {
                session,
                agent: Some(name.to_owned()),
            })
            .await
        {
            tracing::warn!(agent = %name, "desk identity unavailable: {e}");
            return;
        }
        // 183 §6.6: the desk answers to `<agent>@<engine>` from any machine of the mesh. A new
        // desk is an owner-scope root; an older desk keeps its scope and gains the alias.
        let alias = format!("{name}@{}", mesh::self_name());
        let scope = match existing {
            None => Some(agent::PeerScope::Owner),
            Some(_) => agents
                .ask(agent::GetCommunication {
                    session: Some(session),
                })
                .await
                .ok()
                .filter(|s| s.alias.as_deref() != Some(alias.as_str()))
                .map(|s| s.scope),
        };
        if let Some(scope) = scope
            && let Err(e) = agents
                .ask(agent::SetPeerScope {
                    root: session,
                    scope,
                    alias: Some(alias),
                })
                .await
        {
            tracing::warn!(agent = %name, "desk alias not set: {e}");
        }
        // 148 E2: the agent's configured model — a native child of the desk
        // that names none starts on it (a hired agent's `model:`, theuth's
        // live `roster.theuth_model`). Re-sent on every (re)open: Agents
        // drops the entry when a desk is put away.
        if let Err(e) = agents
            .ask(agent::SetDeskModel {
                session,
                model: configured.clone(),
                effort: f.effort.clone(),
            })
            .await
        {
            tracing::warn!(agent = %name, "desk model not recorded: {e}");
        }
        let mut model = configured.clone();
        if existing.is_some() {
            // A desk put away by an earlier build was archived; it is the desk again.
            let _ = agents
                .ask(agent::ArchiveSession {
                    session,
                    archived: false,
                })
                .await;
            // Whatever turn claimed wakes on it died with the last process.
            // 178: at boot the roster opens desks before the tasks plugin
            // starts; the release then waits for `TasksUp` — without it the
            // dead turn's claim held the agent's queue for good.
            match tasks::actor() {
                _ if !release => {}
                Some(t) => {
                    let _ = t.tell(tasks::ReleaseClaims(session.to_string())).await;
                }
                None => self.unreleased.push(session),
            }
            // 148 E3/F4: a reopened desk runs on its effective model — the
            // configured one, else the live default; `switch_session` leaves
            // a row already storing it alone. A fresh session was created
            // with it above.
            if let Some(m) = effective_model(&agents, configured).await {
                model = switch_session(&agents, name, session, m).await.or(model);
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
        // 137 R1-D7 / D14: a reopened desk whose session is in the other
        // mode switches at its next idle moment (backfill, then flip); a
        // per-agent `view_bytes:` is applied the same way.
        let memory = agents
            .ask(agent::SessionMemoryQuery { session })
            .await
            .map(|m| m.mode)
            .unwrap_or_default();
        if let Some(bytes) = f.view_bytes
            && let Err(e) = agents
                .ask(agent::SetSessionViewBytes { session, bytes })
                .await
        {
            tracing::warn!(agent = %name, bytes, "desk view_bytes not applied: {e}");
        }
        // 137G: the agent's `memory_model:` is its desk's compactor default
        // (below a chat's own pick), with its `memory_effort:` (194); written
        // every open, so a cleared key clears.
        if let Err(e) = agents
            .ask(agent::SetAgentCompactorModel {
                session,
                model: f.memory_model.clone(),
                effort: f.memory_effort.clone(),
            })
            .await
        {
            tracing::warn!(agent = %name, model = ?f.memory_model, effort = ?f.memory_effort, "desk memory_model not applied: {e}");
        }
        self.shared.set_desk(name, session);
        self.desks.insert(
            name.to_owned(),
            desk::Desk {
                session,
                episode,
                last_active_ms: now,
                working: false,
                primed: false,
                model,
                serving: None,
                memory,
                pending_memory: (memory != wanted).then_some(wanted),
            },
        );
        tracing::info!(agent = %name, %session, %memory, "desk opened");
        self.apply_pending_memory(name).await;
    }

    /// 137 D12: the mode a desk of `f` runs in.
    fn desk_mode(&self, f: &AgentFile) -> agent::MemoryMode {
        f.memory.unwrap_or(if self.endless.default_desk {
            agent::MemoryMode::Endless
        } else {
            agent::MemoryMode::Epochs
        })
    }

    /// 148 E3: `roster.theuth_model` := `model` (a canonical id, or `None`
    /// for the default): written to `config.yaml` (best-effort, like
    /// `agent::SetSubagentModel`: a failure is logged and the live value
    /// still changes; an empty value reads back as `None`), kept live,
    /// mirrored into the synthesised `theuth` file, applied to its open desk.
    async fn set_theuth_model(&mut self, model: Option<String>) {
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(
            &path,
            "roster",
            "theuth_model",
            model.as_deref().unwrap_or_default(),
        ) {
            tracing::warn!("config.yaml: {e}");
        }
        self.cfg.theuth_model = model;
        self.refresh_theuth().await;
    }

    /// 194 rows 4–5: `roster.theuth_effort` := `effort`, written to
    /// `config.yaml` (quoted so a bare budget stays a string; `~` clears),
    /// then kept live. R1: a failed write is the caller's `Err` and leaves
    /// the live value alone. The caller refreshes theuth.
    fn store_theuth_effort(&mut self, effort: Option<String>) -> Result<(), String> {
        let value = effort
            .as_deref()
            .map_or_else(|| "~".to_owned(), |e| format!("\"{e}\""));
        let path = self.rt.data_dir.join("config.yaml");
        runtime::write_config_value(&path, "roster", "theuth_effort", &value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        self.cfg.theuth_effort = effort;
        Ok(())
    }

    /// 194 [`SetTheuthEffort`]: validate against theuth's effective model,
    /// store, then refresh theuth (its file and desk, open or not).
    async fn put_theuth_effort(
        &mut self,
        effort: Option<String>,
    ) -> Result<Option<String>, String> {
        if let Some(e) = &effort {
            let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten()
                .ok_or_else(|| "agent actor not running".to_owned())?;
            let model = effective_model(&agents, self.cfg.theuth_model.clone()).await;
            agents
                .ask(agent::ValidateEffort {
                    model,
                    effort: e.clone(),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        self.store_theuth_effort(effort.clone())?;
        self.refresh_theuth().await;
        self.put_unopened_desk_effort().await;
        Ok(effort)
    }

    /// 194 R1: theuth's effort setting changed while its (lazily opened) desk
    /// is not open: put the persisted desk row on it, so the next open starts
    /// on the setting, not on the row's stale value. An open desk got it from
    /// [`Self::refresh_theuth`]. A desk-local `/effort` survives every open
    /// that follows no change.
    async fn put_unopened_desk_effort(&self) {
        let name = AgentName::DEFAULT;
        if self.desks.contains_key(name) {
            return;
        }
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let Some(session) = agents
            .ask(agent::DeskSession(name.to_owned()))
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        let model = resolve_configured(&agents, name, self.cfg.theuth_model.clone()).await;
        if let Err(e) = agents
            .ask(agent::SetDeskEffort {
                session,
                model,
                effort: self.cfg.theuth_effort.clone(),
            })
            .await
        {
            tracing::warn!(agent = %name, "desk effort not recorded: {e}");
        }
    }

    /// The synthesised `theuth` file: the live `roster.theuth_model` and
    /// (194) `roster.theuth_effort`.
    fn theuth_file(&self) -> AgentFile {
        AgentFile {
            effort: self.cfg.theuth_effort.clone(),
            ..file::default_agent(&self.rt.data_dir, self.cfg.theuth_model.clone())
        }
    }

    /// theuth's live model or effort changed: mirror both into its file and
    /// apply them to its open desk.
    async fn refresh_theuth(&mut self) {
        self.shared.set_theuth(self.theuth_file());
        self.apply_desk_model(AgentName::DEFAULT, self.cfg.theuth_model.clone())
            .await;
    }

    /// 148 E2/E3: the agent's configured model changed (theuth's live value,
    /// a hired agent's `model:` through [`PutAgent`] or an on-disk edit):
    /// tell Agents (its native children start on it) and put the open desk
    /// on its effective model — the configured one, else the live default
    /// (`switch_session` skips a row already storing it). 194: the file's
    /// `effort:` travels with it. No-op without an open desk.
    async fn apply_desk_model(&mut self, name: &str, configured: Option<String>) {
        let Some(session) = self.desks.get(name).map(|d| d.session) else {
            return;
        };
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let configured = resolve_configured(&agents, name, configured).await;
        if let Err(e) = agents
            .ask(agent::SetDeskModel {
                session,
                model: configured.clone(),
                effort: self.shared.file(name).and_then(|f| f.effort),
            })
            .await
        {
            tracing::warn!(agent = %name, "desk model not recorded: {e}");
        }
        if let Some(m) = effective_model(&agents, configured).await
            && let Some(id) = switch_session(&agents, name, session, m).await
            && let Some(d) = self.desks.get_mut(name)
        {
            d.model = Some(id);
        }
    }

    /// 137G: the agent's `memory_model` (194: or `memory_effort`) changed (a
    /// [`PutAgent`] or an on-disk edit): its open desk's compactor default
    /// follows, live.
    async fn apply_desk_memory_model(
        &self,
        name: &str,
        model: Option<String>,
        effort: Option<String>,
    ) {
        let Some(session) = self.desks.get(name).map(|d| d.session) else {
            return;
        };
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        if let Err(e) = agents
            .ask(agent::SetAgentCompactorModel {
                session,
                model,
                effort,
            })
            .await
        {
            tracing::warn!(agent = %name, "desk memory_model not applied: {e}");
        }
    }

    /// 137 R1-D7 / D14: switch the desk session to its pending mode if it is
    /// idle now; `busy` keeps it pending for the next turn end or tick. One
    /// `info!` with the backfill ETA (and model) when the switch lands.
    /// Idempotent across restarts: the mode is a column, so a switched desk
    /// has nothing pending the next time it opens.
    async fn apply_pending_memory(&mut self, name: &str) {
        let Some((session, mode)) = self
            .desks
            .get(name)
            .filter(|d| !d.working)
            .and_then(|d| d.pending_memory.map(|m| (d.session, m)))
        else {
            return;
        };
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        match agents
            .ask(agent::SetSessionMemory {
                session,
                mode,
                filter: agent::UnitFilter::All,
            })
            .await
        {
            Ok(r) => {
                let m = r.memory;
                if let Some(d) = self.desks.get_mut(name) {
                    d.memory = m.mode;
                    d.pending_memory = None;
                }
                let est = agent::endless_ext::backfill_estimate(&m);
                tracing::info!(agent = %name, %session, %mode, units = est.units, eta_secs = est.eta_secs, model = %est.model, usd = est.usd, "desk memory switched");
            }
            Err(e) if e.to_string().contains("busy") => {
                tracing::debug!(agent = %name, "desk memory switch deferred: busy");
            }
            Err(e) => {
                tracing::warn!(agent = %name, %mode, "desk memory not switched: {e}");
                if let Some(d) = self.desks.get_mut(name) {
                    d.pending_memory = None;
                }
            }
        }
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
    /// 179: a pause (`reason` [`PAUSED`]) is a clean quit for the desk — no
    /// journal turn, no "no continuation" rule; the claim and the served
    /// wake are kept for the un-pause ([`PausedDesk`]).
    async fn close_desk(&mut self, name: &str, reason: &str) {
        let Some(d) = self.desks.remove(name) else {
            return;
        };
        if reason == PAUSED {
            self.held.insert(
                name.to_owned(),
                PausedDesk {
                    session: d.session,
                    serving: d.serving,
                    finished: None,
                },
            );
            self.put_away(name, d.session, &d.episode, reason, None)
                .await;
            return;
        }
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
                text: desk::notice(
                    "journal",
                    if self.shared.file(name).is_some_and(|f| f.isolated_memory) {
                        desk::ISOLATED_CLOSING
                    } else {
                        desk::CLOSING
                    },
                ),
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
                channel: None,
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

    /// A turn started (`model` from `TurnStarted`) or resumed on a session.
    fn desk_turn_began(&mut self, session: runtime::SessionId, model: Option<String>) {
        if let Some(name) = self.desk_name(session)
            && let Some(d) = self.desks.get_mut(&name)
        {
            d.working = true;
            d.last_active_ms = now_ms();
            if model.is_some() {
                d.model = model;
            }
            if let Some(s) = d.serving.as_mut() {
                s.turn_seen = true;
            }
        }
    }

    /// 178 F4: a turn event of `session`; when it is a child spawned for a
    /// desk's current wake, that desk's `waiting_on` is refreshed.
    async fn child_turn_event(&mut self, session: runtime::SessionId) {
        let name = self
            .desks
            .iter()
            .find(|(_, d)| {
                d.serving
                    .as_ref()
                    .is_some_and(|s| s.children.contains(&session))
            })
            .map(|(n, _)| n.clone());
        if let Some(name) = name {
            self.refresh_waiting(&name).await;
        }
    }

    /// 178 F4: the cached `AgentSummary::waiting_on` of a desk that serves a
    /// wake with no turn running — refreshed on Tick, at the desk's turn end
    /// and on its children's turn events, never per `ListAgents` (each
    /// refresh is one `ListAgents` ask to the agent actor).
    async fn refresh_waiting(&mut self, name: &str) {
        let target = self.desks.get(name).and_then(|d| {
            d.serving
                .clone()
                .filter(|_| !d.working)
                .map(|s| (d.session, s))
        });
        match target {
            Some((session, s)) => {
                let kids = Self::children_of(session).await;
                self.waiting
                    .insert(name.to_owned(), desk::nudges::waiting_on(&s, &kids));
            }
            None => {
                self.waiting.remove(name);
            }
        }
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
        use desk::nudges::Verdict;
        let Some(d) = self.desks.get(name) else {
            return;
        };
        let Some(s) = d.serving.clone() else { return };
        let session = d.session;
        // What a yield on a task wake is judged against; a release needs nothing.
        let mut judged = None;
        let mut kids = Vec::new();
        if desk::nudges::yielded(stop_reason)
            && let Some(id) = s.wake.task.clone()
            && desk::nudges::for_reason(s.wake.reason).is_some()
        {
            let Some(tasks) = tasks::actor() else { return };
            let (Ok(task), Ok(events)) = (
                tasks.ask(tasks::GetTask(id.clone())).await,
                tasks.ask(tasks::TaskEvents(id)).await,
            ) else {
                return;
            };
            let me = format!("agent:{name}");
            let mine: Vec<tasks::TaskEvent> = events
                .into_iter()
                .skip(s.events)
                .filter(|e| e.actor == me)
                .collect();
            kids = Self::children_of(session).await;
            judged = Some((task, mine));
        }
        // 182 S5: the task moved to another engine meanwhile — its work and its
        // nudges are that engine's; this desk is done with the wake.
        if let Some((task, _)) = &judged
            && !self.works_here(task)
        {
            tracing::info!(agent = %name, task = %task.id, "182: turn not judged: the task moved to another engine");
            self.clear_serving(name).await;
            return;
        }
        let live_child = desk::nudges::live_child(&s, &kids);
        let verdict = desk::nudges::verdict(
            stop_reason,
            &s,
            judged.as_ref().map(|(t, m)| (t, m.as_slice())),
            live_child,
        );
        match verdict {
            Verdict::Release { min_backoff_ms } => {
                if let Some(d) = self.desks.get_mut(name) {
                    d.serving = None;
                }
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
                            min_backoff_ms,
                        })
                        .await;
                }
            }
            Verdict::Handled => self.clear_serving(name).await,
            // The child's result continues this episode; judged again then.
            Verdict::Defer => {
                if !s.logged_live_child {
                    tracing::info!(
                        agent = %name,
                        task = ?s.wake.task,
                        wake = s.wake.reason.as_str(),
                        children = kids.len(),
                        "turn not judged: a child of the desk is still running"
                    );
                    if let Some(s) = self.desks.get_mut(name).and_then(|d| d.serving.as_mut()) {
                        s.logged_live_child = true;
                    }
                }
            }
            Verdict::Nudge => {
                let (
                    Some((task, _)),
                    Some(desk::nudges::Nudge::TurnEnd {
                        name: rule, text, ..
                    }),
                ) = (&judged, desk::nudges::for_reason(s.wake.reason))
                else {
                    return;
                };
                let mut nudge = text(task);
                if s.rounds >= self.cfg.max_rounds {
                    nudge.push_str(&format!(
                        " You have used {} of {} follow-up rounds on this task this episode: no more children — write the deliverable and dispose.",
                        s.rounds, self.cfg.max_rounds
                    ));
                }
                self.thread_note(
                    task,
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
                        channel: None,
                    })
                    .await
                    .is_ok()
                    && let Some(d) = self.desks.get_mut(name)
                    && let Some(s) = d.serving.as_mut()
                {
                    s.nudged = true;
                }
            }
            Verdict::Block => {
                let (Some((task, _)), Some(tasks)) = (&judged, tasks::actor()) else {
                    return;
                };
                let why = format!(
                    "nudged once and still {} after the turn woken `{}`: blocked, manager woken",
                    task.state.as_str(),
                    s.wake.reason.as_str()
                );
                if let Err(e) = tasks
                    .ask(tasks::NoContinuation {
                        id: task.id.clone(),
                        agent: name.to_owned(),
                        why,
                    })
                    .await
                {
                    tracing::warn!(agent = %name, task = %task.id, "no-continuation not applied: {e}");
                }
                self.clear_serving(name).await;
            }
        }
    }

    /// A memory-mode transition cannot keep a model frame or journal from the other partition.
    async fn retire_memory_context(&mut self, name: &str) -> Result<(), String> {
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten();
        if let Some(agents) = agents
            && let Some(session) = agents
                .ask(agent::DeskSession(name.to_owned()))
                .await
                .map_err(|e| e.to_string())?
        {
            agents
                .ask(agent::supervisor::PauseTree { desk: session })
                .await
                .map_err(|e| e.to_string())?;
            agents
                .ask(agent::supervisor::ReleaseTree { desk: session })
                .await
                .map_err(|e| e.to_string())?;
            agents
                .ask(agent::MarkDesk {
                    session,
                    agent: None,
                })
                .await
                .map_err(|e| e.to_string())?;
            agents
                .ask(agent::ArchiveSession {
                    session,
                    archived: true,
                })
                .await
                .map_err(|e| e.to_string())?;
            self.closing.remove(&session);
        }
        document::memory::remove(&self.rt.data_dir.join("agent-memory").join(name)).await;
        self.desks.remove(name);
        self.held.remove(name);
        self.shared.clear_desk(name);
        self.shared.set_resuming(name, false);
        let journal = self
            .rt
            .data_dir
            .join(desk::JOURNAL_DIR)
            .join(format!("{name}.md"));
        if journal.exists() {
            let parked = journal.parent().unwrap().join(".parked");
            std::fs::create_dir_all(&parked).map_err(|e| e.to_string())?;
            std::fs::rename(&journal, parked.join(format!("{name}-{}.md", now_ms())))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
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
        // 178 W4/F4: what the desk's current wake waits on — the cache, and
        // only while no turn of the desk runs.
        let waiting_on = self
            .desks
            .get(&name)
            .filter(|d| d.serving.is_some() && !d.working)
            .and_then(|_| self.waiting.get(&name).cloned())
            .unwrap_or_default();
        // 179 D9: what a paused agent holds.
        let (held_wakes, suspended) = if status == AgentStatus::Paused {
            self.paused_state(&name, desk).await
        } else {
            (0, false)
        };
        AgentSummary {
            isolated_memory: f.isolated_memory,
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
            waiting_on,
            held_wakes,
            suspended,
        }
    }

    /// 179 D9: a paused agent's unclaimed wakes, and whether a turn of its
    /// desk tree is suspended; `(0, false)` without the actors.
    async fn paused_state(&self, name: &str, desk: Option<runtime::SessionId>) -> (u32, bool) {
        let held = match tasks::actor() {
            Some(t) => t.ask(tasks::HeldWakes(name.to_owned())).await.unwrap_or(0),
            None => 0,
        };
        let suspended = match (
            desk,
            ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten(),
        ) {
            (Some(d), Some(a)) => a
                .ask(agent::supervisor::TreeSuspended(d))
                .await
                .unwrap_or(false),
            _ => false,
        };
        (held, suspended)
    }

    /// A wake is being handled on the desk (178 W4: a turn runs on it, or it
    /// still serves a wake — e.g. one waiting on a child's result).
    fn working(&self, desk: runtime::SessionId) -> bool {
        self.desks
            .values()
            .any(|d| d.session == desk && (d.working || d.serving.is_some()))
    }

    /// 182 D4: the lease no longer moves desks — every engine keeps its own
    /// open for the tasks homed on it. Only legacy (NULL-home) rows follow
    /// the lease, and that gate is the ledger's (`Ledger::works`); a legacy
    /// task this engine checked out is homed here by that checkout, so a
    /// loss leaves nothing of ours to block.
    fn lease_changed(&self, change: Option<bool>) {
        match change {
            Some(true) => tracing::info!("hub lease held: legacy (unhomed) tasks dispatch here"),
            Some(false) => {
                tracing::info!("hub lease elsewhere: desks stay open for the tasks homed here")
            }
            None => {}
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

    /// 67 §6.4 at 100 %: the file says `paused`, and — 179 D1 — the pause is
    /// applied at once, as the switch's is: the desk closes, its tree suspends.
    async fn budget_pause(&mut self, name: &str) {
        if let Err(e) = self.set_paused(name, true).await {
            tracing::warn!(agent = %name, "not paused at the budget cap: {e}");
        }
        self.sync_desks().await;
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
                self.budget_pause(&name).await;
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

impl Message<agent::EndlessConfigChanged> for Roster {
    type Reply = ();

    /// 137 §11.3: Settings › Memory changed `agent.endless`; desks opened
    /// from now on follow the new `default_desk`.
    async fn handle(&mut self, m: agent::EndlessConfigChanged, _: &mut Context<Self, Self::Reply>) {
        self.endless = m.0;
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
            let live_work = self
                .probe
                .liveness(&d.session.to_string())
                .await
                .is_some_and(|l| l.active_now);
            if d.idle_due(now, self.cfg.episode_idle_secs, live_work) {
                idle.push(name.clone());
            }
        }
        for name in idle {
            self.close_desk(&name, "idle").await;
        }
        // 178 W5: the backstop for a served wake no turn end will judge (a
        // lost `TurnFinished`, a refused or lost child result): a desk that
        // serves one while nothing of its session tree has been alive for
        // `SERVING_GRACE_MS` is judged as for a yielded turn end. Liveness,
        // never age: a running child keeps it deferring; unknown → skipped.
        let mut stale = Vec::new();
        for (name, d) in &self.desks {
            // 178 F3: only once the desk was seen to run a turn for the wake —
            // a resumed turn waiting for its resume slot publishes nothing yet.
            if !d.serving.as_ref().is_some_and(|s| s.turn_seen) {
                continue;
            }
            let Some(l) = self.probe.liveness(&d.session.to_string()).await else {
                continue;
            };
            let last = l.last_active_ms.unwrap_or(0).max(d.last_active_ms);
            if !l.active_now && now.saturating_sub(last) > SERVING_GRACE_MS {
                stale.push(name.clone());
            }
        }
        for name in stale {
            if let Some(d) = self.desks.get_mut(&name) {
                d.working = false;
            }
            tracing::info!(agent = %name, "178: desk serves a wake with nothing alive; judged as a turn end");
            self.judge_turn(&name, "end_turn").await;
        }
        // 178 F4: the `waiting_on` cache catches children no event names.
        let serving: Vec<String> = self
            .desks
            .iter()
            .filter(|(_, d)| d.serving.is_some())
            .map(|(n, _)| n.clone())
            .collect();
        for name in serving {
            self.refresh_waiting(&name).await;
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
        // 179 D2 backstop: a resumed desk turn whose end was never seen (the
        // 178 W5 rule above cleared `working`) no longer holds the wakes.
        let idle: Vec<String> = self
            .desks
            .iter()
            .filter(|(_, d)| !d.working)
            .map(|(n, _)| n.clone())
            .collect();
        for name in idle {
            if self.shared.set_resuming(&name, false)
                && let Some(t) = tasks::actor()
            {
                let _ = t.tell(tasks::Unpaused(name)).await;
            }
        }
        // Desks that could not open earlier (the agent actor was not up).
        self.sync_desks().await;
        // 137 R1-D7: idle desks with a memory switch still pending.
        let pending: Vec<String> = self
            .desks
            .iter()
            .filter(|(_, d)| d.pending_memory.is_some() && !d.working)
            .map(|(n, _)| n.clone())
            .collect();
        for name in pending {
            self.apply_pending_memory(&name).await;
        }
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
        // 182 D3 backstop: a task another engine works is never delivered here.
        if let Some(t) = &m.task {
            let here = match &t.home {
                Some(h) => self.lease.self_id.as_deref() == Some(h.as_str()),
                None => self.lease.holding,
            };
            if !here {
                return Err(format!(
                    "task {} lives on {}, not here",
                    t.id,
                    t.home.as_deref().unwrap_or("the hub lease holder")
                ));
            }
        }
        if m.agent == AgentName::DEFAULT
            && !self.desks.contains_key(&m.agent)
            && !self.is_closing(&m.agent)
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
                channel: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        d.primed = true;
        d.last_active_ms = now_ms();
        // 178 W5: a taken session's turn is not the desk's — no desk turn
        // will end to judge it, so serving it here would hold the agent's
        // queue for ever. The wake is done once the session has its input.
        if resumed.is_some() {
            tracing::info!(agent = %m.agent, wake = %m.wake.id, "resume wake handed to the taken session; not served on the desk");
            if let Some(t) = tasks::actor() {
                let _ = t.tell(tasks::FinishWake(m.wake)).await;
            }
            return Ok(());
        }
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
            turn_seen: false,
        });
        // A new wake: the previous one's `waiting_on` is not this one's.
        self.waiting.remove(&m.agent);
        Ok(())
    }
}

impl Message<agent::AgentEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        use agent::AgentEvent as E;
        match ev {
            E::TurnStarted { session, model, .. } => {
                self.desk_turn_began(session, Some(model));
                self.child_turn_event(session).await;
            }
            // 178 W4: a resumed desk turn is a turn all the same (no model field).
            E::TurnResumed { session, .. } => {
                self.desk_turn_began(session, None);
                self.child_turn_event(session).await;
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
                    // 179: a paused desk's turn finished inside the pause's
                    // drain window instead of being suspended — judged at the
                    // un-pause, so its wake is neither lost nor delivered again.
                    if let Some(p) = self.held.values_mut().find(|p| p.session == session) {
                        p.finished = Some(stop_reason);
                        return;
                    }
                    self.child_turn_event(session).await;
                    return;
                };
                let (model, endless) = {
                    let Some(d) = self.desks.get_mut(&name) else {
                        return;
                    };
                    d.working = false;
                    d.last_active_ms = now_ms();
                    if let Some(s) = d.serving.as_mut() {
                        s.turn_seen = true;
                    }
                    (d.model.clone(), d.memory == agent::MemoryMode::Endless)
                };
                // 70 DESK-PROTOCOL 3: the nudge point.
                self.judge_turn(&name, &stop_reason).await;
                // 179 D2: the turn an un-pause resumed is over; the agent's
                // held wakes go now, in rank order.
                if self.shared.set_resuming(&name, false)
                    && let Some(t) = tasks::actor()
                {
                    let _ = t.tell(tasks::Unpaused(name.clone())).await;
                }
                self.refresh_waiting(&name).await;
                // 137 R1-D7: the idle moment a pending memory switch waits for.
                self.apply_pending_memory(&name).await;
                // 137 D12: an endless desk's frame is bounded by its view; the
                // context trigger is off for it (idle/lease/thread-end stay).
                if let Some(model) = model
                    && !endless
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
            // 148 E3: desks without a configured model (theuth with
            // `roster.theuth_model` unset, hired agents without `model:`)
            // follow the default for new sessions — each switched
            // session-only.
            E::DefaultModelChanged { model } => {
                let following: Vec<(String, runtime::SessionId)> = self
                    .desks
                    .iter()
                    .filter(|(n, _)| self.shared.file(n).is_some_and(|f| f.model.is_none()))
                    .map(|(n, d)| (n.clone(), d.session))
                    .collect();
                let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
                    return;
                };
                for (name, session) in following {
                    if let Some(id) = switch_session(&agents, &name, session, model.clone()).await
                        && let Some(d) = self.desks.get_mut(&name)
                    {
                        d.model = Some(id);
                    }
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

async fn main_knowledge() -> Result<Arc<knowledge::Knowledge>, String> {
    let actor = ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
        .ok()
        .flatten()
        .ok_or("knowledge is not running")?;
    actor
        .ask(knowledge::Handle)
        .await
        .map_err(|e| e.to_string())
}

/// 67 §4.4: the lease is followed for the legacy (NULL-home) rows and the
/// DeliverWake backstop only (182 D4); `Peer` events are not ours.
impl Message<mesh::MeshEvent> for Roster {
    type Reply = ();

    async fn handle(&mut self, ev: mesh::MeshEvent, _: &mut Context<Self, Self::Reply>) {
        if let mesh::MeshEvent::Lease(l) = ev {
            let change = self.lease.observe(&l);
            self.lease_changed(change);
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
        // 182 M3: the sandbox event is local to the engine hosting it — not a lease duty.
        if !pin::takes_work(&info.state) {
            return;
        }
        let Some(tasks) = tasks::actor() else {
            return;
        };
        let blocked = tasks
            .ask(tasks::ListTasks(tasks::TaskFilter {
                state: Some(vec![tasks::TaskState::Blocked]),
                // 182: a same-named sandbox elsewhere is that engine's to report.
                here: true,
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
        self.lease_changed(change);
        self.publish_agents().await;
    }
}

impl Message<Synced> for Roster {
    type Reply = ();

    async fn handle(&mut self, _: Synced, _: &mut Context<Self, Self::Reply>) {
        tracing::info!("182: first sync done — boot holds lifted, desks open");
        self.sync_desks().await;
    }
}

/// 182 M2a: the task id a desk input names — `task <uuid>` (a wake, a nudge).
fn task_id_in(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("task ") {
        let start = from + i + 5;
        let id = text.get(start..start + 36);
        if let Some(id) = id
            && id.bytes().enumerate().all(|(k, b)| match k {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_hexdigit(),
            })
        {
            return Some(id);
        }
        from = start;
    }
    None
}

impl Message<TasksUp> for Roster {
    type Reply = ();

    async fn handle(&mut self, _: TasksUp, _: &mut Context<Self, Self::Reply>) {
        if let Some(t) = tasks::actor() {
            for session in std::mem::take(&mut self.unreleased) {
                tracing::info!(%session, "178: claims of a desk reopened before tasks started released");
                let _ = t.tell(tasks::ReleaseClaims(session.to_string())).await;
            }
        }
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

impl Message<DesksRunning> for Roster {
    type Reply = u32;

    async fn handle(&mut self, _: DesksRunning, _: &mut Context<Self, Self::Reply>) -> u32 {
        self.desks.values().filter(|d| d.working).count() as u32
    }
}

impl Message<ListEngineAgents> for Roster {
    type Reply = Result<Vec<EngineAgent>, String>;

    async fn handle(
        &mut self,
        _: ListEngineAgents,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self
            .store
            .engine_agents()
            .await
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .filter(|(engine, _)| known_engine(engine))
            .map(|(engine, file)| EngineAgent { engine, file })
            .collect())
    }
}

/// 182 S3: `engine` is this one or a paired peer — the replicated rosters of
/// engines unpaired since are left in the table (deleting them would replicate
/// to peers still paired with them) but never read. Everything without a mesh
/// or before the mesh bound its endpoint (pairing unknown).
pub(crate) fn known_engine(engine: &str) -> bool {
    mesh::self_id().is_none_or(|me| me == engine) || mesh::is_paired(engine) != Some(false)
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
                        isolated_memory: false,
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
                            memory: None,
                            isolated_memory: false,
                            view_bytes: None,
                            memory_model: None,
                            memory_effort: None,
                            body: String::new(),
                        },
                        status: AgentStatus::Invalid,
                        desk: None,
                        open_tasks: 0,
                        spent_month: 0.0,
                        error: Some(e),
                        waiting_on: Vec::new(),
                        held_wakes: 0,
                        suspended: false,
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

impl Message<TheuthModel> for Roster {
    type Reply = Option<agent::ModelInfo>;

    /// The stored id resolved through the catalog (no session, not manual:
    /// 148 E1). 148 F7: an id that no longer resolves — or no agent actor to
    /// ask — answers as itself ([`stale_info`]) so the row shows the stale
    /// value rather than "Same as new sessions".
    async fn handle(&mut self, _: TheuthModel, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = self.cfg.theuth_model.clone()?;
        let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return Some(stale_info(id));
        };
        let resolved = agents
            .ask(agent::SetModel {
                session: None,
                model: id.clone(),
                manual: false,
            })
            .await;
        Some(resolved.unwrap_or_else(|_| stale_info(id)))
    }
}

impl Message<SetTheuthModel> for Roster {
    type Reply = Result<Option<agent::ModelInfo>, String>;

    /// 148 E3: validate (a set needs the agent actor), then [`Roster::set_theuth_model`].
    async fn handle(
        &mut self,
        m: SetTheuthModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (info, effort) = match m.model {
            Some(id) => {
                let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    .ok()
                    .flatten()
                    .ok_or_else(|| "agent actor not running".to_owned())?;
                let info = resolve_keyed(&agents, &id).await?;
                // 194 D4c: an effort the new model does not take is dropped.
                let effort = match self.cfg.theuth_effort.clone() {
                    Some(e) => agents
                        .ask(agent::ValidateEffort {
                            model: Some(info.id.clone()),
                            effort: e.clone(),
                        })
                        .await
                        .ok()
                        .map(|()| e),
                    None => None,
                };
                (Some(info), effort)
            }
            // 194 D4d: "Same as new sessions" clears the effort too.
            None => (None, None),
        };
        let effort_changed = effort != self.cfg.theuth_effort;
        if effort_changed {
            self.store_theuth_effort(effort)?;
        }
        self.set_theuth_model(info.as_ref().map(|i| i.id.clone()))
            .await;
        if effort_changed {
            self.put_unopened_desk_effort().await;
        }
        Ok(info)
    }
}

impl Message<TheuthEffort> for Roster {
    type Reply = Option<String>;

    async fn handle(&mut self, _: TheuthEffort, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cfg.theuth_effort.clone()
    }
}

impl Message<SetTheuthEffort> for Roster {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: SetTheuthEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.put_theuth_effort(m.effort).await
    }
}

impl Message<PutAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        mut m: PutAgent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if m.0.name.is_default() {
            return Err(
                "theuth is the built-in default agent: edit prompt.md, not the roster; \
                        its model is set in Settings"
                    .into(),
            );
        }
        // 179 D7: `paused` and `proposed_by` of an existing file are the
        // switch's and the approval's — a stale copy (the budget card, a
        // second window) never flips them. A new file ("Hire paused") sets them.
        let old = self.shared.file(m.0.name.as_str());
        if let Some(o) = &old {
            m.0.paused = o.paused;
            m.0.proposed_by.clone_from(&o.proposed_by);
        }
        settle_memory_effort(&mut m.0, old.as_ref()).await?;
        // Validate through the parser so the file on disk is always readable.
        file::parse(m.0.name.as_str(), &file::render(&m.0))?;
        // 148 F1: `write_file` updates the read model, so the reconcile below
        // cannot see the `model:` change — apply it here. 194 R1: an
        // `effort:`-only change too (the pair travels together).
        let old_model = old.as_ref().map(|f| (f.model.clone(), f.effort.clone()));
        let isolation_changed = old
            .as_ref()
            .is_some_and(|f| f.isolated_memory != m.0.isolated_memory);
        if m.0.isolated_memory && old.as_ref().is_none_or(|f| !f.isolated_memory) {
            let k = main_knowledge().await?;
            let owner = format!("agent:{}", m.0.name);
            let count = k
                .list(None, None, None, u32::MAX)
                .await
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .filter(|m| m.owner == owner)
                .count();
            if count > 0 {
                return Err(format!(
                    "isolated_memory: {} already has {count} notes in the main memory; hire a new agent with isolated memory instead (moving existing notes is not supported yet)",
                    m.0.name
                ));
            }
        }
        if isolation_changed {
            self.retire_memory_context(m.0.name.as_str()).await?;
        }
        let old_memory = old.map(|f| (f.memory_model, f.memory_effort));
        self.write_file(&m.0).await?;
        if old_model != Some((m.0.model.clone(), m.0.effort.clone())) {
            self.apply_desk_model(m.0.name.as_str(), m.0.model.clone())
                .await;
        }
        let memory = (m.0.memory_model.clone(), m.0.memory_effort.clone());
        if old_memory.unwrap_or_default() != memory {
            self.apply_desk_memory_model(m.0.name.as_str(), memory.0, memory.1)
                .await;
        }
        let _ = ctx.actor_ref().tell(Reconcile).await;
        Ok(())
    }
}

impl Message<GrantMcp> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: GrantMcp, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // Read disk, not a possibly stale watcher snapshot: preserve an edit
        // that arrived while the server was connecting.
        let path = self.dir.join(format!("{}.md", m.agent));
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut f = file::parse(&m.agent, &text)?;
        if let Some(list) = &mut f.mcp
            && !list.contains(&m.server)
        {
            list.push(m.server);
            self.write_file(&f).await?;
        }
        Ok(())
    }
}

impl Message<PauseAgent> for Roster {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PauseAgent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
        // 179: applied before the reply — a queued `Reconcile` left the desk
        // open (judged, nudged, woken) until it ran.
        self.sync_desks().await;
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
        AgentName::new(&m.name)?;
        if m.name == AgentName::DEFAULT {
            return Err("theuth is the built-in default agent and cannot be removed".into());
        }
        if !self.dir.join(format!("{}.md", m.name)).is_file() {
            return Err("no such agent".into());
        }
        self.retire_memory_context(&m.name).await?;
        if let Ok(k) = main_knowledge().await {
            k.remove_bundle(&m.name)
                .await
                .map_err(|e| format!("{e:#}"))?;
        } else {
            knowledge::store::move_removed_bundle(&self.rt.data_dir, &m.name)
                .map_err(|e| format!("{e:#}"))?;
        }
        let approval = self.remove_agent(&m.name).await?;
        self.shared.remove_file(&m.name);
        self.store
            .delete_agent(&m.name)
            .await
            .map_err(|e| format!("{e:#}"))?;
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
        // 194 R1: the same `memory_effort` rules as `PutAgent` (a new file).
        settle_memory_effort(&mut f, None).await?;
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

    fn has_agent(&self, agent: &str) -> Option<bool> {
        if agent == AgentName::DEFAULT {
            return Some(true);
        }
        match self.0.files().get(agent) {
            Some(Ok(_)) => Some(true),
            Some(Err(_)) => None,
            None => Some(false),
        }
    }

    /// 179 D3: the file's `paused:`, or an un-paused desk still finishing
    /// the turn its pause suspended.
    fn is_paused(&self, agent: &str) -> bool {
        self.0.is_paused(agent)
    }

    /// 182 D8: the replicated per-engine agent list (no actor: tasks asks
    /// this inside its handlers, and the roster asks tasks in its own).
    async fn engines_with(&self, agent: &str) -> Vec<String> {
        let Some(store) = self.0.store.get() else {
            return Vec::new();
        };
        match store.engines_with(agent).await {
            Ok(mut v) => {
                v.retain(|e| known_engine(e));
                v
            }
            Err(e) => {
                tracing::warn!(agent, "engine_agents not read: {e:#}");
                Vec::new()
            }
        }
    }
}

/// 67 §6.3: an MCP tool is offered to a session only when the agent it works
/// for lists the server (or `server.tool`); `theuth` and sessions of no agent
/// see everything. Descendants are the agent's too ([`Shared::inherit`]).
#[async_trait::async_trait]
impl ::tools::McpAccess for Desks {
    fn allows(&self, session: runtime::SessionId, server: &str, tool: &str) -> bool {
        self.0
            .agent_of(session)
            .and_then(|a| self.0.file(&a))
            .is_none_or(|f| desk::mcp_visible(&f, server, tool))
    }

    async fn grant(&self, session: runtime::SessionId, server: &str) -> Result<(), String> {
        let Some(agent) = self.0.agent_of(session).filter(|a| a != AgentName::DEFAULT) else {
            return Ok(());
        };
        if self
            .0
            .file(&agent)
            .is_none_or(|f| f.mcp.is_none_or(|list| list.iter().any(|s| s == server)))
        {
            return Ok(());
        }
        actor()
            .ok_or("roster is not running")?
            .ask(GrantMcp {
                agent,
                server: server.to_owned(),
            })
            .await
            .map_err(|e| e.to_string())
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

    async fn inbox(&self, agent: &str) -> Option<agent::InboxPersona> {
        let f = self.shared.file(agent)?;
        Some(agent::InboxPersona {
            // theuth's inbox keeps theuth's default persona.
            identity: (!f.name.is_default()).then(|| desk::inbox_identity(&f)),
            model: f.model.clone(),
            tools: f
                .tools
                .as_deref()
                .and_then(|t| t.parse::<::tools::Risk>().ok()),
        })
    }
}

/// 67 §9: a session of agent X reads and writes knowledge as `agent:X`, with
/// X's reporting line (up and down) for `team` items. The user's sessions
/// and `theuth`'s carry no scope and see everything.
#[async_trait::async_trait]
impl knowledge::Scoper for Desks {
    async fn bundle_for(&self, session: runtime::SessionId) -> Result<Option<String>, String> {
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or("session identity service unavailable")?;
        let store = agents
            .ask(agent::DurableStore)
            .await
            .map_err(|e| e.to_string())?;
        let (identity, desk) = store
            .memory_identity(session)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some((name, isolated, home)) = identity {
            self.0.associate(session, &name);
            if isolated {
                if self.0.file(&name).is_none_or(|f| !f.isolated_memory) {
                    return Err(format!("bundle for {name} is parked or removed"));
                }
                if !home.is_empty() && mesh::self_id().is_none_or(|me| me != home) {
                    return Err("bundle belongs to another engine".into());
                }
                return Ok(Some(name));
            }
            if self.0.file(&name).is_some_and(|f| f.isolated_memory) {
                return Err(format!(
                    "memory mode changed for {name}; a fresh desk is required"
                ));
            }
            return Ok(None);
        }
        if let Some(name) = desk.or_else(|| self.0.agent_of(session)) {
            self.0.associate(session, &name);
            if self.0.file(&name).is_none_or(|f| f.isolated_memory) {
                return Err(format!("identity not ready for agent {name}"));
            }
        }
        Ok(None)
    }

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

/// Declare wire contracts before starting producers.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<attention::AttentionChanged>()
        .await
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

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
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
        // 67 §5.1: `agents` and `budget` replicate, 79 §6.6: `handoffs` too,
        // 182 D8: `engine_agents` (every engine's roster); local only without the plugin.
        let replica_handle = match replica::attach(
            store.pool(),
            &["agents", "budget", "handoffs", "engine_agents"],
        )
        .await
        {
            Ok(h) => {
                tracing::info!("roster: agents, budget, handoffs and engine_agents replicate");
                Some(h)
            }
            Err(e) => {
                tracing::debug!("roster: local only ({e})");
                None
            }
        };
        let attention_store = store.clone();
        let shared = Arc::new(Shared::default());
        let _ = shared.store.set(store.clone());
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
        // 137 §11.3: `default_desk` follows Settings › Memory live.
        rt.subscribe::<Roster, agent::EndlessConfigChanged>(&actor)
            .await;
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(mesh) = rt2.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await
                && let Ok(status) = mesh.ask(mesh::Status).await
            {
                let _ = me.tell(SetSelf(status.self_id.0)).await;
            }
        });
        // 182 M2a: lift the boot holds and open the desks once the first sync is
        // done and the tasks actor is up (it answers whether a held turn's task
        // moved; up to 10 s — without the tasks plugin nothing moved).
        let me = actor.clone();
        let stop = rt.shutdown.clone();
        tokio::spawn(async move {
            let mut tasks_wait = 10;
            while !mesh::synced() || (tasks::actor().is_none() && tasks_wait > 0) {
                if mesh::synced() {
                    tasks_wait -= 1;
                }
                tokio::select! {
                    () = stop.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
            let _ = me.tell(Synced).await;
        });
        // 182 M1: every engine's roster rows reach a peer that attached
        // `engine_agents` late (a staggered upgrade).
        if let Some(handle) = replica_handle {
            mesh::reannounce_on_connect(rt, handle, "engine_agents", "id <> ?", None);
        }
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

    mod mcp_live;

    #[test]
    fn isolated_memory_refuses_legacy_parks_resumes_and_removes_without_main_fallback() {
        with_engine_lock(async {
            use runtime::Plugin;
            let data = tmp("isolated-lifecycle");
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            Box::new(knowledge::KnowledgePlugin)
                .start(&rt)
                .await
                .unwrap();
            let knowledge = main_knowledge().await.unwrap();
            let r = roster_on(rt.clone(), &data).await;
            knowledge.set_scoper(Arc::new(Desks(r.shared.clone())));
            let actor = Roster::spawn(r);
            let legacy = file::parse("legacy", "Legacy.").unwrap();
            actor.ask(PutAgent(legacy.clone())).await.unwrap();
            let desk = actor.ask(OpenDesk("legacy".into())).await.unwrap();
            knowledge
                .for_session(desk)
                .await
                .unwrap()
                .put_for(
                    knowledge::Put {
                        id: "facts/legacy".into(),
                        type_: "fact".into(),
                        title: "Legacy".into(),
                        body: "legacy body".into(),
                        ..Default::default()
                    },
                    knowledge::Source::Model,
                    Some(desk),
                )
                .await
                .unwrap();
            let mut enabled = legacy;
            enabled.isolated_memory = true;
            let e = actor.ask(PutAgent(enabled)).await.unwrap_err();
            let kameo::error::SendError::HandlerError(e) = e else {
                panic!("unexpected error")
            };
            assert_eq!(
                e,
                "isolated_memory: legacy already has 1 notes in the main memory; hire a new agent with isolated memory instead (moving existing notes is not supported yet)"
            );
            let mut f = file::parse("ops", "---\nisolated_memory: true\n---\nOps.").unwrap();
            actor.ask(PutAgent(f.clone())).await.unwrap();
            let isolated = actor.ask(OpenDesk("ops".into())).await.unwrap();
            let own = knowledge.for_session(isolated).await.unwrap();
            own.put(
                knowledge::Put {
                    id: "facts/secret".into(),
                    type_: "fact".into(),
                    title: "Secret".into(),
                    body: "isolated body".into(),
                    ..Default::default()
                },
                knowledge::Source::Model,
            )
            .await
            .unwrap();
            assert!(
                actor
                    .ask(ListAgents)
                    .await
                    .unwrap()
                    .iter()
                    .find(|a| a.file.name.as_str() == "ops")
                    .unwrap()
                    .isolated_memory
            );
            f.isolated_memory = false;
            actor.ask(PutAgent(f.clone())).await.unwrap();
            assert!(knowledge.for_session(isolated).await.is_err());
            let plain = actor.ask(OpenDesk("ops".into())).await.unwrap();
            assert_ne!(plain, isolated);
            assert!(!knowledge.for_session(plain).await.unwrap().is_isolated());
            assert!(
                data.join("agent-memory/ops/knowledge/facts/secret.md")
                    .is_file()
            );
            f.isolated_memory = true;
            actor.ask(PutAgent(f)).await.unwrap();
            let resumed = actor.ask(OpenDesk("ops".into())).await.unwrap();
            assert_eq!(
                knowledge
                    .for_session(resumed)
                    .await
                    .unwrap()
                    .get("facts/secret")
                    .await
                    .unwrap()
                    .meta
                    .owner,
                "agent:ops"
            );
            actor
                .ask(RemoveAgent {
                    name: "ops".into(),
                    note: None,
                })
                .await
                .unwrap();
            assert!(!data.join("agent-memory/ops").exists());
            assert!(
                std::fs::read_dir(data.join("agent-memory/.removed"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path()
                    .join("knowledge/facts/secret.md")
                    .is_file()
            );
            rt.shutdown.cancel();
            actor.kill();
            agents.kill();
            if let Ok(Some(k)) =
                ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
            {
                k.kill();
            }
            unregister(&[agent::AGENT_ACTOR, knowledge::KNOWLEDGE_ACTOR]);
            let _ = std::fs::remove_dir_all(data);
        });
    }

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
        roster_on(rt, data_dir).await
    }

    /// A `Roster` over `data_dir` on a caller's runtime (shared with a test
    /// engine, so events route between them).
    async fn roster_on(rt: Runtime, data_dir: &std::path::Path) -> Roster {
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

    /// Held by the test that registers a live engine as `AGENT_ACTOR`
    /// (process-global) and by every test whose assertions depend on there
    /// being none (the agent crate's `REGISTRY_TEST_LOCK` pattern).
    static AGENT_RIG: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `body` under [`AGENT_RIG`]: the lock is taken before the runtime
    /// is built and released after it is dropped, so a registered engine is
    /// dead before the next gated test looks the name up.
    fn with_engine_lock<F: Future>(body: F) -> F::Output {
        let _lock = AGENT_RIG.lock().unwrap_or_else(|p| p.into_inner());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(body)
    }

    /// Poll `ok` (every 50 ms, up to 10 s) until it holds; events ride the
    /// runtime bus asynchronously.
    async fn eventually(what: &str, mut ok: impl AsyncFnMut() -> bool) {
        for _ in 0..200 {
            if ok().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{what}: not within 10s");
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

    /// 148 E3: `roster.theuth_model` is set and cleared live, persisted to
    /// config.yaml (an empty value reads back as `None`), and reaches the
    /// synthesised file that `ListAgents` / `GetAgent` answer with. PutAgent
    /// still refuses theuth: the model has one setter.
    #[tokio::test]
    async fn theuth_model_is_live_persisted_and_listed() {
        let data = tmp("theuth-model");
        let read = |data: &std::path::Path| -> RosterConfig {
            use figment::providers::Format;
            figment::Figment::new()
                .merge(figment::providers::Yaml::file(data.join("config.yaml")))
                .extract_inner("roster")
                .unwrap()
        };
        let mut r = roster(&data).await;
        assert!(r.shared.theuth().unwrap().model.is_none());
        r.set_theuth_model(Some("testvis/eyes".into())).await;
        assert_eq!(r.cfg.theuth_model.as_deref(), Some("testvis/eyes"));
        assert_eq!(read(&data).theuth_model.as_deref(), Some("testvis/eyes"));
        let actor = Roster::spawn(r);
        let listed = actor.ask(ListAgents).await.unwrap();
        assert!(listed[0].file.name.is_default());
        assert_eq!(listed[0].file.model.as_deref(), Some("testvis/eyes"));
        assert_eq!(
            actor
                .ask(GetAgent("theuth".into()))
                .await
                .unwrap()
                .model
                .as_deref(),
            Some("testvis/eyes")
        );
        // Clearing needs no catalog; the empty value reads back as None.
        assert!(
            actor
                .ask(SetTheuthModel { model: None })
                .await
                .unwrap()
                .is_none()
        );
        let yaml = std::fs::read_to_string(data.join("config.yaml")).unwrap();
        assert!(yaml.contains("theuth_model:"), "{yaml}");
        assert!(read(&data).theuth_model.is_none(), "{yaml}");
        assert!(actor.ask(TheuthModel).await.unwrap().is_none());
        assert!(actor.ask(ListAgents).await.unwrap()[0].file.model.is_none());
        let err = actor
            .ask(PutAgent(file::default_agent(&data, Some("x/y".into()))))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("built-in default agent"), "{err}");
        actor.kill();
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 148 F7a: a stored `roster.theuth_model` that no longer resolves (here:
    /// no agent actor to resolve it) still answers [`TheuthModel`] as itself,
    /// so the Settings row shows the stale id, not "Same as new sessions".
    #[test]
    fn theuth_model_unresolved_shows_as_itself() {
        with_engine_lock(async {
            let data = tmp("theuth-stale");
            let mut r = roster(&data).await;
            r.set_theuth_model(Some("gone/old-model".into())).await;
            let actor = Roster::spawn(r);
            let info = actor.ask(TheuthModel).await.unwrap().unwrap();
            assert_eq!(info.id, "gone/old-model");
            assert_eq!(info.name, "gone/old-model");
            assert_eq!(info.provider, "gone");
            assert!(info.efforts.is_empty() && info.context == 0);
            assert_eq!(stale_info("bare".into()).provider, "");
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 148 F1: `PutAgent` updates the read model as it writes, so the
    /// reconcile it queues sees no `model:` change — the handler applies the
    /// change itself (`old_model != new`). The premise, provable without an
    /// agent actor: right after the write the read model already holds the
    /// new model and a reconcile finds nothing to diff.
    #[tokio::test]
    async fn put_agent_model_change_is_in_the_read_model_before_reconcile() {
        let data = tmp("put-model");
        let mut r = roster(&data).await;
        let qa = |model: &str| {
            file::parse(
                "qa",
                &format!("---\ntitle: QA\nmodel: {model}\n---\nTest everything.\n"),
            )
            .unwrap()
        };
        r.write_file(&qa("testvis/eyes")).await.unwrap();
        r.reconcile().await;
        let old_model = r.shared.file("qa").and_then(|f| f.model);
        assert_eq!(old_model.as_deref(), Some("testvis/eyes"));
        r.write_file(&qa("testvis/blind")).await.unwrap();
        let before = r.shared.files();
        assert_eq!(
            before["qa"].as_ref().unwrap().model.as_deref(),
            Some("testvis/blind"),
            "the read model moved with the write: reconcile's diff is empty"
        );
        r.reconcile().await;
        assert_eq!(r.shared.files(), before);
        assert_ne!(old_model, r.shared.file("qa").and_then(|f| f.model));
        let _ = std::fs::remove_dir_all(&data);
    }

    /// Collects every `AgentEvent` the bus delivers.
    #[derive(kameo::Actor)]
    struct Sink {
        tx: tokio::sync::mpsc::UnboundedSender<agent::AgentEvent>,
    }

    impl Message<agent::AgentEvent> for Sink {
        type Reply = ();

        async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.tx.send(ev);
        }
    }

    /// `agent.model` := `model` through the Settings path.
    async fn set_default(agents: &ActorRef<agent::Agents>, model: &str) {
        agents
            .ask(agent::SetModel {
                session: None,
                model: model.to_owned(),
                manual: true,
            })
            .await
            .unwrap();
    }

    /// The model id stored on the session's row.
    async fn stored(agents: &ActorRef<agent::Agents>, session: runtime::SessionId) -> String {
        agents
            .ask(agent::SessionModel(session))
            .await
            .unwrap()
            .expect("known session")
    }

    /// The session marked as `name`'s desk.
    async fn desk_of(agents: &ActorRef<agent::Agents>, name: &str) -> Option<runtime::SessionId> {
        agents
            .ask(agent::DeskSession(name.to_owned()))
            .await
            .unwrap()
    }

    /// Publish a marker and count the `ModelChanged` events delivered before
    /// it: the bus is FIFO per subscriber, so everything published earlier
    /// has landed once the marker has. Only while no `Roster` actor is
    /// subscribed (it would follow the marker as a default).
    async fn model_changes_before_marker(
        rt: &Runtime,
        events: &mut tokio::sync::mpsc::UnboundedReceiver<agent::AgentEvent>,
    ) -> usize {
        rt.publish(agent::AgentEvent::DefaultModelChanged {
            model: "marker".into(),
        })
        .await;
        let mut n = 0;
        loop {
            match events.recv().await.unwrap() {
                agent::AgentEvent::DefaultModelChanged { model } if model == "marker" => return n,
                agent::AgentEvent::ModelChanged { .. } => n += 1,
                _ => {}
            }
        }
    }

    /// 148 F8, against a live model-less engine (`agent::spawn_test_agents`):
    /// (iii) a reopened desk is switched only when its row's model differs
    /// from the effective one (no `ModelChanged` otherwise); (i)
    /// `SetTheuthModel` re-points theuth's open desk and leaves the default
    /// alone; (v) an unresolvable configured id shows as itself and the desk
    /// follows the default; (ii) `DefaultModelChanged` follows for desks
    /// without a model and leaves a configured one alone; (iv) a `PutAgent`
    /// model change re-points the open desk at once; (v) a hired agent whose
    /// `model:` does not resolve still gets a desk, on the default.
    #[test]
    fn desks_follow_configured_and_default_models_on_a_live_engine() {
        with_engine_lock(async {
            let data = tmp("rig");
            // `eyes` under two providers: the bare id is ambiguous — the one
            // way a catalog resolve fails (unknown `provider/model` passes through).
            std::fs::write(
                data.join("models.json"),
                r#"{"testvis":{"id":"testvis","name":"Test Vision","models":{
                    "eyes":{"id":"eyes","name":"eyes","limit":{"context":100000,"output":4096}},
                    "blind":{"id":"blind","name":"blind","limit":{"context":100000,"output":4096}}}},
                   "othervis":{"id":"othervis","name":"Other","models":{
                    "eyes":{"id":"eyes","name":"eyes","limit":{"context":100000,"output":4096}}}}}"#,
            )
            .unwrap();
            let dir = data.join(file::DIR);
            std::fs::write(
                dir.join("fixed.md"),
                "---\nmodel: testvis/eyes\n---\nFixed.\n",
            )
            .unwrap();
            std::fs::write(dir.join("free.md"), "Free.\n").unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            // `nope` is not a keyed provider (148 R4 Z2-C); everything else is.
            let agents = agent::spawn_test_agents(rt.clone(), |ns: &str| ns != "nope")
                .await
                .unwrap();
            set_default(&agents, "testvis/blind").await;
            let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
            let sink = Sink::spawn(Sink { tx });
            rt.subscribe::<Sink, agent::AgentEvent>(&sink).await;

            // Boot 1: fresh desks, created on their effective models.
            let r1 = roster_on(rt.clone(), &data).await;
            let fixed = desk_of(&agents, "fixed").await.expect("fixed desk opened");
            let free = desk_of(&agents, "free").await.expect("free desk opened");
            assert_eq!(stored(&agents, fixed).await, "testvis/eyes");
            assert_eq!(stored(&agents, free).await, "testvis/blind");
            drop(r1);
            let _ = model_changes_before_marker(&rt, &mut events).await;

            // (iii) Boot 2: both rows already store their effective model — no switch.
            let r2 = roster_on(rt.clone(), &data).await;
            assert_eq!(
                desk_of(&agents, "fixed").await,
                Some(fixed),
                "the desk is reopened, not recreated"
            );
            assert_eq!(
                model_changes_before_marker(&rt, &mut events).await,
                0,
                "equal stored/effective: no ModelChanged"
            );
            drop(r2);

            // (iii) Boot 3 after the default moved: only the desk without a model is switched.
            set_default(&agents, "othervis/eyes").await;
            let mut r = roster_on(rt.clone(), &data).await;
            assert_eq!(stored(&agents, free).await, "othervis/eyes");
            assert_eq!(stored(&agents, fixed).await, "testvis/eyes");
            assert_eq!(r.desks["free"].model.as_deref(), Some("othervis/eyes"));
            assert_eq!(model_changes_before_marker(&rt, &mut events).await, 1);

            // (i) theuth's desk opens on the default; SetTheuthModel re-points it, default untouched.
            let theuth = r.desk_or_open("theuth").await.unwrap();
            assert_eq!(stored(&agents, theuth).await, "othervis/eyes");
            r.set_theuth_model(Some("testvis/eyes".into())).await;
            assert_eq!(stored(&agents, theuth).await, "testvis/eyes");
            assert_eq!(
                agents.ask(agent::GetDefaultModel).await.unwrap(),
                "othervis/eyes"
            );
            r.set_theuth_model(None).await;
            assert_eq!(
                stored(&agents, theuth).await,
                "othervis/eyes",
                "cleared: back on the default"
            );

            // (v) A stored theuth model that no longer resolves (set directly:
            // SetTheuthModel would refuse it) leaves the desk on the default;
            // so does one whose provider has no credentials (148 R4 Z2-C),
            // and TheuthModel still answers the raw id.
            r.set_theuth_model(Some("eyes".into())).await;
            assert_eq!(stored(&agents, theuth).await, "othervis/eyes");
            r.set_theuth_model(Some("nope/none".into())).await;
            assert_eq!(stored(&agents, theuth).await, "othervis/eyes");
            let actor = Roster::spawn(r);
            let info = actor.ask(TheuthModel).await.unwrap().unwrap();
            assert_eq!(
                (info.id.as_str(), info.provider.as_str()),
                ("nope/none", "nope")
            );
            actor.ask(SetTheuthModel { model: None }).await.unwrap();

            // (ii) DefaultModelChanged: desks without a model follow; `fixed` keeps its own.
            rt.subscribe::<Roster, agent::AgentEvent>(&actor).await;
            set_default(&agents, "testvis/blind").await;
            eventually("free follows the default", async || {
                stored(&agents, free).await == "testvis/blind"
            })
            .await;
            eventually("theuth follows the default", async || {
                stored(&agents, theuth).await == "testvis/blind"
            })
            .await;
            assert_eq!(stored(&agents, fixed).await, "testvis/eyes");

            // (iv) PutAgent with a new `model:` re-points the open desk before replying…
            let free_file = file::parse("free", "---\nmodel: othervis/eyes\n---\nFree.\n").unwrap();
            actor.ask(PutAgent(free_file)).await.unwrap();
            assert_eq!(stored(&agents, free).await, "othervis/eyes");
            // …and the desk no longer follows the default.
            set_default(&agents, "testvis/eyes").await;
            eventually("theuth follows the default", async || {
                stored(&agents, theuth).await == "testvis/eyes"
            })
            .await;
            // A reply from the actor proves the event handler has finished.
            let _ = actor.ask(ListAgents).await.unwrap();
            assert_eq!(
                stored(&agents, free).await,
                "othervis/eyes",
                "configured: left alone"
            );

            // (v) A hired agent whose `model:` does not resolve still gets a desk, on the default.
            let stale = file::parse("stale", "---\nmodel: eyes\n---\nStale.\n").unwrap();
            actor.ask(PutAgent(stale)).await.unwrap();
            eventually("stale desk opens", async || {
                desk_of(&agents, "stale").await.is_some()
            })
            .await;
            let stale = desk_of(&agents, "stale").await.unwrap();
            assert_eq!(stored(&agents, stale).await, "testvis/eyes");

            // (vi) 148 R4 Z2-C: a provider without credentials resolves in the
            // catalog but cannot answer — the desk opens on the default (the
            // warn path), SetTheuthModel refuses the id, and a stored theuth
            // id still reads back as itself.
            let unkeyed = file::parse("unkeyed", "---\nmodel: nope/none\n---\nUnkeyed.\n").unwrap();
            actor.ask(PutAgent(unkeyed)).await.unwrap();
            eventually("unkeyed desk opens", async || {
                desk_of(&agents, "unkeyed").await.is_some()
            })
            .await;
            let unkeyed = desk_of(&agents, "unkeyed").await.unwrap();
            assert_eq!(stored(&agents, unkeyed).await, "testvis/eyes");
            // SetTheuthModel refuses it with the engine's words.
            let refused = actor
                .ask(SetTheuthModel {
                    model: Some("nope/none".into()),
                })
                .await
                .unwrap_err()
                .to_string();
            assert!(
                refused.contains("provider `nope` has no credentials"),
                "{refused}"
            );

            actor.kill();
            sink.kill();
            agents.kill();
            kameo::registry::ACTOR_REGISTRY
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(agent::AGENT_ACTOR);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 194 rows 4–5, on a live engine: `SetTheuthEffort` is checked against theuth's
    /// effective model (D4b), persisted to `roster.theuth_effort` (D4g), carried by
    /// the synthesised file and put on its open desk at once (D4e/f; the desk's
    /// children follow through `SetDeskModel`, agent tests); `SetTheuthModel` drops
    /// an effort the new model does not take (D4c) and `None` clears it (D4d).
    #[test]
    fn theuth_effort_is_validated_persisted_and_reaches_its_desk() {
        with_engine_lock(async {
            let data = tmp("theuth-effort");
            std::fs::write(
                data.join("models.json"),
                r#"{"testvis":{"id":"testvis","name":"Test","models":{
                    "think":{"id":"think","name":"think","limit":{"context":100000,"output":4096},
                             "reasoning_options":[{"type":"effort","values":["low","high"]}]},
                    "blind":{"id":"blind","name":"blind","limit":{"context":100000,"output":4096}}}}}"#,
            )
            .unwrap();
            let read = |data: &std::path::Path| -> RosterConfig {
                use figment::providers::Format;
                figment::Figment::new()
                    .merge(figment::providers::Yaml::file(data.join("config.yaml")))
                    .extract_inner("roster")
                    .unwrap()
            };
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            set_default(&agents, "testvis/think").await;
            let mut r = roster_on(rt.clone(), &data).await;
            let theuth = r.desk_or_open("theuth").await.unwrap();
            let actor = Roster::spawn(r);
            let effort = |e: Option<&str>| SetTheuthEffort {
                effort: e.map(Into::into),
            };
            let model = |m: Option<&str>| SetTheuthModel {
                model: m.map(Into::into),
            };
            let desk_effort = async || {
                agents
                    .ask(agent::ListSessions)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|s| s.id == theuth)
                    .and_then(|s| s.effort)
            };

            // D4b against the default model (theuth has none of its own).
            let err = actor
                .ask(effort(Some("bogus")))
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(err, "think accepts off|low|high");
            let yaml = std::fs::read_to_string(data.join("config.yaml")).unwrap_or_default();
            assert!(
                !yaml.contains("theuth_effort"),
                "a refused effort writes nothing: {yaml}"
            );
            assert_eq!(
                actor.ask(effort(Some("high"))).await.unwrap().as_deref(),
                Some("high")
            );
            assert_eq!(
                actor.ask(TheuthEffort).await.unwrap().as_deref(),
                Some("high")
            );
            assert_eq!(read(&data).theuth_effort.as_deref(), Some("high"));
            let listed = actor.ask(ListAgents).await.unwrap();
            assert!(listed[0].file.name.is_default());
            assert_eq!(listed[0].file.effort.as_deref(), Some("high"));
            assert_eq!(desk_effort().await.as_deref(), Some("high"));

            // D4c: a model without efforts drops it; that model then refuses one.
            actor.ask(model(Some("testvis/blind"))).await.unwrap();
            assert_eq!(actor.ask(TheuthEffort).await.unwrap(), None);
            assert_eq!(read(&data).theuth_effort, None);
            assert_eq!(desk_effort().await, None);
            assert!(actor.ask(effort(Some("low"))).await.is_err());

            // D4d: "Same as new sessions" clears it, and the desk goes back on agent.effort.
            actor.ask(model(Some("testvis/think"))).await.unwrap();
            actor.ask(effort(Some("low"))).await.unwrap();
            assert_eq!(desk_effort().await.as_deref(), Some("low"));
            actor.ask(model(None)).await.unwrap();
            assert_eq!(actor.ask(TheuthEffort).await.unwrap(), None);
            assert_eq!(read(&data).theuth_effort, None);
            assert_eq!(desk_effort().await, None);

            actor.kill();
            agents.kill();
            kameo::registry::ACTOR_REGISTRY
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(agent::AGENT_ACTOR);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 194: a `roster:` section written before `theuth_effort` existed still loads;
    /// the absent key is the default (same as new sessions).
    #[test]
    fn a_roster_section_without_theuth_effort_still_loads() {
        use figment::providers::Format;
        let c: RosterConfig = figment::Figment::new()
            .merge(figment::providers::Yaml::string(
                "roster:\n  theuth_model: testvis/think\n  max_rounds: 2\n",
            ))
            .extract_inner("roster")
            .unwrap();
        assert_eq!(c.theuth_model.as_deref(), Some("testvis/think"));
        assert_eq!((c.max_rounds, c.theuth_effort), (2, None));
    }

    /// `testvis/think` takes `off|low|high`, `testvis/blind` no effort.
    const EFFORT_MODELS: &str = r#"{"testvis":{"id":"testvis","name":"Test","models":{
        "think":{"id":"think","name":"think","limit":{"context":100000,"output":4096},
                 "reasoning_options":[{"type":"effort","values":["low","high"]}]},
        "blind":{"id":"blind","name":"blind","limit":{"context":100000,"output":4096}}}}}"#;

    /// The effort stored on `id`'s row, as the next request of the session sends it.
    async fn session_effort(
        agents: &ActorRef<agent::Agents>,
        id: runtime::SessionId,
    ) -> Option<String> {
        agents
            .ask(agent::ListSessions)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)
            .expect("listed session")
            .effort
    }

    /// 194 R1: `roster.theuth_effort` reaches theuth's desk when it is not open
    /// (it opens lazily). After a restart, a clear made before the desk reopens
    /// puts the persisted row back on the default, so the reopen starts on
    /// Default; a desk-local `/effort` survives a reopen that follows no change.
    /// Persist-first: a failed config write is `Err` and changes nothing.
    #[test]
    fn theuth_effort_reaches_a_desk_that_is_not_open() {
        with_engine_lock(async {
            let data = tmp("theuth-effort-closed");
            std::fs::write(data.join("models.json"), EFFORT_MODELS).unwrap();
            let yaml = data.join("config.yaml");
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            // A fresh engine and roster, `roster.theuth_effort` as the last run left it.
            let boot = async |effort: Option<&str>| {
                let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                    .await
                    .unwrap();
                set_default(&agents, "testvis/think").await;
                let mut r = roster_on(rt.clone(), &data).await;
                r.cfg.theuth_effort = effort.map(Into::into);
                r.shared.set_theuth(r.theuth_file());
                (agents, r)
            };
            let stop = |agents: ActorRef<agent::Agents>| {
                agents.kill();
                unregister(&[agent::AGENT_ACTOR]);
            };

            // Run 1: theuth on `high`, its open desk follows.
            let (agents, mut r) = boot(None).await;
            let desk = r.desk_or_open("theuth").await.unwrap();
            r.put_theuth_effort(Some("high".into())).await.unwrap();
            assert_eq!(session_effort(&agents, desk).await.as_deref(), Some("high"));
            drop(r);
            stop(agents);

            // Run 2: the desk is not open yet. A failed write is refused, nothing moves.
            let (agents, mut r) = boot(Some("high")).await;
            assert!(!r.desks.contains_key("theuth"), "theuth opens lazily");
            let before = std::fs::read_to_string(&yaml).unwrap();
            let mode = |m| {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&yaml, std::fs::Permissions::from_mode(m)).unwrap();
            };
            mode(0o444);
            let err = r.put_theuth_effort(None).await.unwrap_err();
            assert!(err.contains("config.yaml"), "{err}");
            assert!(
                r.put_theuth_effort(Some("low".into())).await.is_err(),
                "a set too"
            );
            mode(0o644);
            assert_eq!(r.cfg.theuth_effort.as_deref(), Some("high"));
            assert_eq!(std::fs::read_to_string(&yaml).unwrap(), before);
            assert_eq!(session_effort(&agents, desk).await.as_deref(), Some("high"));
            // The clear reaches the closed desk's row, and the reopen starts on Default.
            r.put_theuth_effort(None).await.unwrap();
            assert_eq!(session_effort(&agents, desk).await, None);
            assert_eq!(r.desk_or_open("theuth").await.unwrap(), desk);
            assert_eq!(session_effort(&agents, desk).await, None);
            // A desk-local pick, made in the desk chat.
            agents
                .ask(agent::SetEffort {
                    session: Some(desk),
                    effort: Some("low".into()),
                    manual: true,
                })
                .await
                .unwrap();
            drop(r);
            stop(agents);

            // Run 3: the setting is unchanged; the reopen keeps the desk's own pick.
            let (agents, mut r) = boot(None).await;
            assert_eq!(r.desk_or_open("theuth").await.unwrap(), desk);
            assert_eq!(session_effort(&agents, desk).await.as_deref(), Some("low"));
            drop(r);
            stop(agents);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 194 R1 (D4f): an `effort:`-only edit of a hired agent — through `PutAgent`
    /// and through an on-disk edit the reconcile sees — reaches its open desk (its
    /// next request) and the desk's next native child at once, with no reopen.
    #[test]
    fn a_hired_agents_effort_only_edit_is_live() {
        with_engine_lock(async {
            let data = tmp("hired-effort");
            std::fs::write(data.join("models.json"), EFFORT_MODELS).unwrap();
            let dir = data.join(file::DIR);
            let ops = |effort: &str| format!("---\nmodel: testvis/think\n{effort}---\nOps.\n");
            std::fs::write(dir.join("ops.md"), ops("")).unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            set_default(&agents, "testvis/think").await;
            let r = roster_on(rt.clone(), &data).await;
            let desk = desk_of(&agents, "ops").await.expect("ops desk opened");
            let actor = Roster::spawn(r);
            let kid = async || {
                let (_, child, _) = agents
                    .ask(agent::supervisor::SpawnChild {
                        parent: desk,
                        name: None,
                        task: "check".into(),
                        agent: None,
                        cwd: None,
                        model: None,
                        effort: None,
                        cap: None,
                        skills: Vec::new(),
                        call_id: String::new(),
                    })
                    .await
                    .unwrap();
                session_effort(&agents, child).await
            };
            assert_eq!(session_effort(&agents, desk).await, None);

            // PutAgent: only `effort:` changes.
            let put = file::parse("ops", &ops("effort: high\n")).unwrap();
            actor.ask(PutAgent(put)).await.unwrap();
            assert_eq!(session_effort(&agents, desk).await.as_deref(), Some("high"));
            assert_eq!(kid().await.as_deref(), Some("high"));

            // On disk: the reconcile sees an `effort:`-only edit.
            std::fs::write(dir.join("ops.md"), ops("effort: low\n")).unwrap();
            actor.ask(Reconcile).await.unwrap();
            assert_eq!(session_effort(&agents, desk).await.as_deref(), Some("low"));
            assert_eq!(kid().await.as_deref(), Some("low"));

            actor.kill();
            agents.kill();
            unregister(&[agent::AGENT_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
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
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 182 D4: desks do not depend on the lease — an engine that does not
    /// hold it still wants its own agents' desks, keeps theuth's, and opens
    /// one on demand.
    #[test]
    fn desks_open_without_the_lease() {
        with_engine_lock(async {
            let data = tmp("no-lease");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let mut r = roster(&data).await;
            r.lease.set_self("me".into());
            r.lease.observe(&mesh::LeaseState {
                holder: Some(mesh::EngineId("other".into())),
                since_ms: 0,
                reason: "test".into(),
            });
            assert!(!r.lease.holding);
            r.reconcile().await;
            let wanted: Vec<String> = r
                .wanted()
                .iter()
                .map(|f| f.name.as_str().to_owned())
                .collect();
            assert_eq!(wanted, ["ops"]);
            assert!(r.allowed("ops") && r.allowed("theuth"));
            // No agent actor here: the open is attempted, not refused for the lease.
            let e = r.desk_or_open("theuth").await.unwrap_err();
            assert!(e.contains("not opened"), "{e}");
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    #[test]
    fn desk_or_open_returns_the_open_desk_and_names_its_refusals() {
        with_engine_lock(async {
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
                    memory: agent::MemoryMode::Epochs,
                    pending_memory: None,
                },
            );
            assert_eq!(r.desk_or_open("ops").await, Ok(session));
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// An open desk for tests: no turn, no wake.
    fn idle_desk(session: runtime::SessionId) -> desk::Desk {
        desk::Desk {
            session,
            episode: String::new(),
            last_active_ms: 0,
            working: false,
            primed: false,
            model: None,
            serving: None,
            memory: agent::MemoryMode::Epochs,
            pending_memory: None,
        }
    }

    /// 182 D4: losing the lease closes no desk — they serve the tasks homed
    /// here; before 182 every desk closed and its doing tasks were blocked.
    #[test]
    fn losing_the_lease_keeps_homed_desks_open() {
        with_engine_lock(async {
            let data = tmp("lease-loss");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let mut r = roster(&data).await;
            r.reconcile().await;
            r.lease.set_self("me".into());
            assert!(r.lease.holding);
            let session = runtime::SessionId::new();
            r.desks.insert("ops".into(), idle_desk(session));
            let actor = Roster::spawn(r);
            for holder in [Some("other"), None] {
                actor
                    .tell(mesh::MeshEvent::Lease(mesh::LeaseState {
                        holder: holder.map(|h| mesh::EngineId(h.into())),
                        since_ms: 0,
                        reason: "test".into(),
                    }))
                    .await
                    .unwrap();
                assert_eq!(
                    actor.ask(OpenDesk("ops".into())).await.unwrap(),
                    session,
                    "lease → {holder:?}: the desk stays open"
                );
            }
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 182 D3 backstop: the roster refuses a wake whose task another engine
    /// works — homed elsewhere, or a legacy row while the lease is elsewhere.
    #[test]
    fn deliver_wake_refuses_a_task_homed_elsewhere() {
        with_engine_lock(async {
            let data = tmp("backstop");
            let mut r = roster(&data).await;
            r.lease.set_self("me".into());
            let wake = tasks::Wake {
                id: "w1".into(),
                agent: "theuth".into(),
                task: None,
                reason: tasks::WakeReason::Assigned,
                key: "k".into(),
                count: 1,
                claimed_by: None,
                created_ms: 0,
            };
            let actor = Roster::spawn(r);
            let ask = |home: Option<&'static str>| {
                let mut t = desk::tests::task_in(tasks::TaskState::Todo);
                t.home = home.map(str::to_owned);
                actor.ask(DeliverWake {
                    agent: "theuth".into(),
                    wake: wake.clone(),
                    task: Some(t),
                })
            };
            let e = ask(Some("other")).await.unwrap_err().to_string();
            assert!(e.contains("lives on other"), "{e}");
            // Homed here: past the backstop (no agent actor: theuth's desk is not opened).
            let e = ask(Some("me")).await.unwrap_err().to_string();
            assert!(!e.contains("lives on"), "{e}");
            // A legacy row follows the lease.
            let e = ask(None).await.unwrap_err().to_string();
            assert!(!e.contains("lives on"), "lease held here: {e}");
            actor
                .tell(mesh::MeshEvent::Lease(mesh::LeaseState {
                    holder: Some(mesh::EngineId("other".into())),
                    since_ms: 0,
                    reason: "test".into(),
                }))
                .await
                .unwrap();
            let e = ask(None).await.unwrap_err().to_string();
            assert!(e.contains("lives on the hub lease holder"), "{e}");
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 182 D8: each engine publishes its roster (files + theuth) under its id;
    /// a peer's replicated rows list beside them; a removed file's row goes.
    #[test]
    fn agents_are_listed_per_engine() {
        with_engine_lock(async {
            let data = tmp("per-engine");
            std::fs::write(data.join(file::DIR).join("hans.md"), "Notes.\n").unwrap();
            let mut r = roster(&data).await;
            r.lease.set_self("A".into());
            r.reconcile().await;
            // A peer's rows, as the replica lands them.
            let theuth = r.shared.theuth().unwrap();
            r.store.put_engine_agents("B", &[theuth]).await.unwrap();
            let rows: Vec<(String, String)> = r
                .store
                .engine_agents()
                .await
                .unwrap()
                .into_iter()
                .map(|(e, f)| (f.name.as_str().to_owned(), e))
                .collect();
            assert_eq!(
                rows,
                [
                    ("hans".into(), "A".into()),
                    ("theuth".into(), "A".into()),
                    ("theuth".into(), "B".into())
                ]
            );
            let desks = Desks(r.shared.clone());
            let _ = r.shared.store.set(r.store.clone());
            assert_eq!(tasks::DeskRouter::engines_with(&desks, "hans").await, ["A"]);
            assert_eq!(
                tasks::DeskRouter::engines_with(&desks, "theuth").await,
                ["A", "B"]
            );
            std::fs::remove_file(data.join(file::DIR).join("hans.md")).unwrap();
            r.reconcile().await;
            assert!(
                tasks::DeskRouter::engines_with(&desks, "hans")
                    .await
                    .is_empty()
            );
            let _ = std::fs::remove_dir_all(&data);
        });
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
            file::default_agent(std::path::Path::new("/nonexistent"), None),
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
            file::default_agent(std::path::Path::new("/nonexistent"), None),
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
    async fn budget_rows_sum_and_proposals_answer_once() {
        let data = tmp("lease");
        std::fs::write(
            data.join("agents/ops.md"),
            "---\nbudget: 10 USD/month\n---\nOps.\n",
        )
        .unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;

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

    // ----- 178: desk status and the Tick backstop -----

    /// The Tick's probe, answering what the test sets.
    #[derive(Default)]
    struct FakeProbe(std::sync::Mutex<Option<tasks::ledger::Liveness>>);

    #[async_trait::async_trait]
    impl tasks::ledger::LivenessProbe for FakeProbe {
        async fn liveness(&self, _: &str) -> Option<tasks::ledger::Liveness> {
            *self.0.lock().unwrap()
        }
    }

    impl FakeProbe {
        fn set(&self, active_now: bool, last_active_ms: Option<u64>) {
            *self.0.lock().unwrap() = Some(tasks::ledger::Liveness {
                active_now,
                last_active_ms,
            });
        }
    }

    /// A wake without a task (judged handled at a yield) for `ops`.
    fn plain_serving() -> desk::Serving {
        desk::Serving {
            wake: tasks::Wake {
                id: "w1".into(),
                agent: "ops".into(),
                task: None,
                reason: tasks::WakeReason::Manual,
                key: String::new(),
                count: 1,
                claimed_by: Some("d".into()),
                created_ms: 0,
            },
            state: None,
            owner: None,
            events: 0,
            nudged: false,
            rounds: 0,
            children: HashSet::new(),
            logged_live_child: false,
            // A turn of the desk ran for it (the backstop's precondition).
            turn_seen: true,
        }
    }

    /// `ops` hired with its desk at a fresh session last active `idle_ms`
    /// ago, serving `serving`; the Tick probes through the returned fake.
    async fn desk_rig(
        tag: &str,
        serving: Option<desk::Serving>,
        idle_ms: u64,
    ) -> (PathBuf, Roster, runtime::SessionId, Arc<FakeProbe>) {
        let data = tmp(tag);
        std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;
        let session = runtime::SessionId::new();
        r.desks.insert(
            "ops".into(),
            desk::Desk {
                session,
                episode: String::new(),
                last_active_ms: now_ms() - idle_ms,
                working: false,
                primed: false,
                model: None,
                serving,
                memory: agent::MemoryMode::Epochs,
                pending_memory: None,
            },
        );
        r.shared.set_desk("ops", session);
        let probe = Arc::new(FakeProbe::default());
        r.probe = probe.clone();
        (data, r, session, probe)
    }

    async fn status_of(actor: &ActorRef<Roster>) -> AgentStatus {
        let rows = actor.ask(ListAgents).await.unwrap();
        rows.into_iter()
            .find(|a| a.file.name.as_str() == "ops")
            .expect("ops listed")
            .status
    }

    /// 178 W4: a desk turn resumed after a restart (`TurnResumed`, never
    /// `TurnStarted`) shows the desk working until its `TurnFinished`.
    #[test]
    fn turn_resumed_marks_desk_working_until_finished() {
        with_engine_lock(async {
            let (data, r, session, _) = desk_rig("resumed-working", None, 0).await;
            let actor = Roster::spawn(r);
            assert_eq!(status_of(&actor).await, AgentStatus::Idle);
            let turn = runtime::TurnId::new();
            actor
                .tell(agent::AgentEvent::TurnResumed {
                    session,
                    turn,
                    kind: agent::durability::ResumeKind::Interrupted,
                    interrupted_at: chrono::Utc::now(),
                    resumed_at: chrono::Utc::now(),
                    partial_dropped: false,
                    cause: agent::durability::ResumeCause::Crash,
                    from: None,
                })
                .await
                .unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Working);
            actor
                .tell(agent::AgentEvent::TurnFinished {
                    session,
                    turn,
                    stop_reason: "end_turn".into(),
                    usage: agent::Usage::default(),
                    context: 0,
                })
                .await
                .unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Idle);
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 178 W4: "Working = a wake is being handled" — a desk that serves a wake
    /// between turns (waiting on a child's result) is working, not idle.
    #[test]
    fn summary_is_working_while_a_wake_is_served() {
        with_engine_lock(async {
            let (data, mut r, _, _) = desk_rig("served-working", None, 0).await;
            let f = r.shared.file("ops").unwrap();
            assert_eq!(r.summary(f.clone()).await.status, AgentStatus::Idle);
            r.desks.get_mut("ops").unwrap().serving = Some(plain_serving());
            assert_eq!(r.summary(f).await.status, AgentStatus::Working);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 178 W4: the children a served wake waits on, by the judge's rule —
    /// spawned for it and unfinished, or running now — named, and the chip
    /// text the views show for them.
    #[test]
    fn summary_lists_children_the_desk_waits_on() {
        use agent::AgentState as A;
        let kid = |name: &str, state, finished: bool| agent::AgentInfo {
            id: runtime::SessionId::new(),
            name: name.into(),
            parent: runtime::SessionId::new(),
            depth: 1,
            agent: None,
            model: String::new(),
            state,
            started_at: None,
            finished_at: finished.then(chrono::Utc::now),
            last: String::new(),
            project: None,
            retention: Default::default(),
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
            observed: None,
        };
        let (scout, done, cold, runner) = (
            kid("scout", A::Idle, false),
            kid("done", A::Idle, true),
            kid("cold", A::Idle, false),
            kid("", A::Running, false),
        );
        let mut s = plain_serving();
        s.children = HashSet::from([scout.id, done.id]);
        let kids = [scout, done, cold, runner.clone()];
        let names = desk::nudges::waiting_on(&s, &kids);
        assert_eq!(
            names,
            [
                "scout".to_owned(),
                format!("session:{}", &runner.id.to_string()[..8])
            ]
        );
        let row = |waiting_on: Vec<String>| AgentSummary {
            isolated_memory: false,
            file: file::parse("ops", "Ops.").unwrap(),
            status: AgentStatus::Working,
            desk: None,
            open_tasks: 0,
            spent_month: 0.0,
            error: None,
            waiting_on,
            held_wakes: 0,
            suspended: false,
        };
        assert_eq!(
            row(names).waiting_label().as_deref(),
            Some("waiting on scout +1")
        );
        assert_eq!(
            row(vec!["scout".into()]).waiting_label().as_deref(),
            Some("waiting on scout")
        );
        assert_eq!(row(Vec::new()).waiting_label(), None);
    }

    /// 178 W5: a desk serving a wake while nothing of its session tree has
    /// been alive past the grace (a lost `TurnFinished` left it `working`)
    /// is judged as for a yielded end — this task-less wake: handled.
    #[test]
    fn tick_rejudges_a_serving_desk_when_nothing_is_alive() {
        with_engine_lock(async {
            let (data, mut r, _, probe) =
                desk_rig("tick-rejudge", Some(plain_serving()), 3 * 60_000).await;
            r.desks.get_mut("ops").unwrap().working = true;
            probe.set(false, None);
            let actor = Roster::spawn(r);
            assert_eq!(status_of(&actor).await, AgentStatus::Working);
            actor.tell(Tick).await.unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Idle);
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 178 W5 is liveness-based: while the session tree works (a child
    /// runs) the wake keeps waiting however old it is, and an unknown
    /// liveness judges nothing.
    #[test]
    fn tick_keeps_deferring_while_a_child_runs() {
        with_engine_lock(async {
            let (data, r, _, probe) =
                desk_rig("tick-defer", Some(plain_serving()), 24 * 3_600_000).await;
            let actor = Roster::spawn(r);
            probe.set(true, None);
            actor.tell(Tick).await.unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Working);
            *probe.0.lock().unwrap() = None;
            actor.tell(Tick).await.unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Working, "unknown");
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 178 W5: the grace (2 × TICK) runs from the newest activity of the
    /// desk or any descendant — a wake just delivered, or a child that just
    /// finished, is not judged yet.
    #[test]
    fn tick_waits_out_the_delivery_grace() {
        with_engine_lock(async {
            let (data, r, _, probe) = desk_rig("tick-grace", Some(plain_serving()), 0).await;
            let actor = Roster::spawn(r);
            probe.set(false, None);
            actor.tell(Tick).await.unwrap();
            assert_eq!(
                status_of(&actor).await,
                AgentStatus::Working,
                "just delivered"
            );
            let (data2, r, _, probe2) =
                desk_rig("tick-grace2", Some(plain_serving()), 10 * 60_000).await;
            let actor2 = Roster::spawn(r);
            probe2.set(false, Some(now_ms() - 60_000));
            actor2.tell(Tick).await.unwrap();
            assert_eq!(
                status_of(&actor2).await,
                AgentStatus::Working,
                "a child finished a minute ago"
            );
            probe2.set(false, Some(now_ms() - 3 * 60_000));
            actor2.tell(Tick).await.unwrap();
            assert_eq!(status_of(&actor2).await, AgentStatus::Idle);
            actor.kill();
            actor2.kill();
            let _ = std::fs::remove_dir_all(&data);
            let _ = std::fs::remove_dir_all(&data2);
        });
    }

    /// 178 F3: a re-delivered wake whose desk turn is a resumed one still
    /// waiting for its resume slot (no `TurnResumed` yet, nothing active) is
    /// not judged by the backstop however long it waits.
    #[test]
    fn tick_leaves_a_desk_whose_resumed_turn_waits_for_a_slot() {
        with_engine_lock(async {
            let mut waiting = plain_serving();
            waiting.turn_seen = false;
            let (data, r, _, probe) = desk_rig("tick-slot", Some(waiting), 30 * 60_000).await;
            probe.set(false, None);
            let actor = Roster::spawn(r);
            for _ in 0..2 {
                actor.tell(Tick).await.unwrap();
            }
            assert_eq!(
                status_of(&actor).await,
                AgentStatus::Working,
                "no turn seen yet"
            );
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 178 W5: a `Resume` wake whose checkout session was taken here
    /// continues that session; no desk turn will end to judge it, so the
    /// desk is not left serving it (that held the agent's queue for ever).
    #[test]
    fn resume_wake_to_a_taken_session_does_not_set_serving() {
        with_engine_lock(async {
            let data = tmp("resume-taken");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            let taken = agents.ask(agent::NewSession::default()).await.unwrap();
            let r = roster_on(rt.clone(), &data).await;
            assert!(r.desks.contains_key("ops"), "desk opened");
            let actor = Roster::spawn(r);
            let mut task = crate::desk::tests::task_in(tasks::TaskState::Doing);
            task.checkout = Some(tasks::Checkout {
                session: taken.to_string(),
                engine: "here".into(),
                epoch: 2,
                progress_at_ms: 0,
                orphaned: true,
                taken: true,
            });
            let mut wake = plain_serving().wake;
            wake.reason = tasks::WakeReason::Resume;
            wake.task = Some(task.id.clone());
            actor
                .ask(DeliverWake {
                    agent: "ops".into(),
                    wake,
                    task: Some(task),
                })
                .await
                .unwrap();
            assert_eq!(status_of(&actor).await, AgentStatus::Idle);
            actor.kill();
            agents.kill();
            kameo::registry::ACTOR_REGISTRY
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(agent::AGENT_ACTOR);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// Forget actors a test registered under global names (their runtime dies with the test).
    fn unregister(names: &[&str]) {
        let mut reg = kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for n in names {
            reg.remove(*n);
        }
    }

    /// A todo task owned by `agent:ops`, through the tasks actor.
    async fn ops_task(t: &ActorRef<tasks::Tasks>) -> tasks::Task {
        t.ask(tasks::CreateTask {
            input: tasks::TaskCreate {
                state: tasks::TaskState::Todo,
                title: "fix deploy".into(),
                brief: "the flaky one".into(),
                owner: tasks::Owner::Agent("ops".into()),
                reviewer: None,
                parent: None,
                project: None,
                where_: None,
                harness: None,
                external: vec![],
                done_when: None,
                attachments: vec![],
                priority: None,
            },
            created_by: "user".into(),
            session: None,
        })
        .await
        .unwrap()
    }

    /// 179 D3 backstop: a wake reaching a paused agent whose desk is still
    /// open (the dispatch pass raced the pause) is refused `held:` and the
    /// desk does not start serving it.
    #[test]
    fn deliver_wake_to_a_paused_agent_is_held_with_an_open_desk() {
        with_engine_lock(async {
            let (data, r, _, _) = desk_rig("paused-held", None, 0).await;
            r.set_paused("ops", true).await.unwrap();
            assert!(r.desks.contains_key("ops"), "not synced: the desk is open");
            let actor = Roster::spawn(r);
            let refused = actor
                .ask(DeliverWake {
                    agent: "ops".into(),
                    wake: plain_serving().wake,
                    task: None,
                })
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("held: agent is paused"), "{refused}");
            let rows = actor.ask(ListAgents).await.unwrap();
            let ops = rows.iter().find(|a| a.file.name.as_str() == "ops").unwrap();
            assert_eq!(ops.status, AgentStatus::Paused);
            actor.kill();
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 179 D4: a pause closes the desk without the episode's "no
    /// continuation" rule — the checked-out `doing` task keeps its state and
    /// checkout, no manager is woken — and keeps what the desk served.
    #[test]
    fn pause_closes_the_desk_and_keeps_checkouts() {
        with_engine_lock(async {
            use runtime::Plugin as _;
            let (data, mut r, session, _) = desk_rig("pause-keeps", Some(plain_serving()), 0).await;
            Box::new(tasks::TasksPlugin).start(&r.rt).await.unwrap();
            let t = tasks::actor().expect("tasks up");
            let task = ops_task(&t).await;
            t.ask(tasks::CheckoutTask {
                id: task.id.clone(),
                session: session.to_string(),
                actor: "agent:ops".into(),
            })
            .await
            .unwrap();
            r.desks.get_mut("ops").unwrap().primed = true;
            let manager = r.shared.file("ops").unwrap().reports_to;
            let before = t.ask(tasks::HeldWakes(manager.clone())).await.unwrap();

            r.set_paused("ops", true).await.unwrap();
            r.sync_desks().await;
            assert!(!r.desks.contains_key("ops"), "desk closed");
            assert_eq!(r.shared.desk("ops"), Some(session), "mapping kept");
            assert_eq!(
                r.held["ops"].serving.as_ref().map(|s| s.wake.id.as_str()),
                Some("w1"),
                "the served wake is kept for the un-pause"
            );
            let after = t.ask(tasks::GetTask(task.id.clone())).await.unwrap();
            assert_eq!(after.state, tasks::TaskState::Doing);
            assert!(after.blockers.is_empty(), "{:?}", after.blockers);
            assert_eq!(after.checkout.map(|c| c.session), Some(session.to_string()));
            assert_eq!(
                t.ask(tasks::HeldWakes(manager)).await.unwrap(),
                before,
                "no manager wake"
            );
            t.kill();
            unregister(&[tasks::TASKS_ACTOR, tasks::activity::ACTIVITY_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 179 D2: un-pausing an agent whose tree had nothing suspended reopens
    /// the same desk session and releases its claims (here queued for
    /// `TasksUp`: no tasks actor in this rig) — they are a dead turn's.
    #[test]
    fn unpause_reopens_desk_and_releases_claims() {
        with_engine_lock(async {
            let data = tmp("unpause");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            let mut r = roster_on(rt.clone(), &data).await;
            let desk = r.desks["ops"].session;
            r.set_paused("ops", true).await.unwrap();
            r.sync_desks().await;
            assert!(!r.desks.contains_key("ops") && r.held.contains_key("ops"));
            r.unreleased.clear();

            r.set_paused("ops", false).await.unwrap();
            r.sync_desks().await;
            assert_eq!(
                r.desks.get("ops").map(|d| d.session),
                Some(desk),
                "same desk"
            );
            assert!(r.held.is_empty());
            assert_eq!(r.unreleased, vec![desk], "claims released");
            assert!(!r.shared.is_paused("ops"), "wakes flow at once");
            agents.kill();
            unregister(&[agent::AGENT_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 179 D7: a stale copy of an existing file (the budget card's spread)
    /// cannot flip `paused` or `proposed_by`; the rest of it is written.
    #[tokio::test]
    async fn put_agent_keeps_stored_paused() {
        let data = tmp("put-paused");
        std::fs::write(
            data.join(file::DIR).join("ops.md"),
            "---\ntitle: Ops\npaused: true\n---\nOps.\n",
        )
        .unwrap();
        let mut r = roster(&data).await;
        r.reconcile().await;
        let actor = Roster::spawn(r);
        let mut stale = actor.ask(GetAgent("ops".into())).await.unwrap();
        stale.paused = false;
        stale.proposed_by = Some("agent:lead".into());
        stale.title = "Ops renamed".into();
        actor.ask(PutAgent(stale)).await.unwrap();
        let now = actor.ask(GetAgent("ops".into())).await.unwrap();
        assert!(now.paused, "still paused");
        assert_eq!(now.proposed_by, None);
        assert_eq!(now.title, "Ops renamed");
        let text = std::fs::read_to_string(data.join(file::DIR).join("ops.md")).unwrap();
        assert!(text.contains("paused: true"), "{text}");
        actor.kill();
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 179 D7: a new file ("Hire paused") is written with the `paused` it carries.
    #[tokio::test]
    async fn put_agent_new_file_honours_paused() {
        let data = tmp("put-new");
        let actor = Roster::spawn(roster(&data).await);
        let mut f = file::parse("nap", "Nap.\n").unwrap();
        f.paused = true;
        actor.ask(PutAgent(f)).await.unwrap();
        assert!(actor.ask(GetAgent("nap".into())).await.unwrap().paused);
        actor.kill();
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 179 D1: the budget cap's pause is the switch's — the desk closes on
    /// the pause path (its served wake kept) and the agent actor holds the
    /// tree: a person's text to the desk is refused until the un-pause.
    #[test]
    fn budget_cap_pause_suspends() {
        with_engine_lock(async {
            let data = tmp("budget-pause");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            let mut r = roster_on(rt.clone(), &data).await;
            let desk = r.desks["ops"].session;
            r.desks.get_mut("ops").unwrap().serving = Some(plain_serving());
            r.budget_pause("ops").await;
            assert!(r.shared.file("ops").unwrap().paused);
            assert!(!r.desks.contains_key("ops"), "desk closed");
            assert!(r.held["ops"].serving.is_some(), "served wake kept");
            let refused = agents
                .ask(agent::SendInput {
                    session: Some(desk),
                    text: "late".into(),
                    files: vec![],
                    delivery: agent::Delivery::default(),
                    id: None,
                    channel: None,
                })
                .await
                .unwrap_err()
                .to_string();
            assert!(
                refused.contains("ops is paused — un-pause it to send"),
                "{refused}"
            );
            agents
                .ask(agent::supervisor::ResumeTree { desk })
                .await
                .unwrap();
            assert!(
                agents
                    .ask(agent::SendInput {
                        session: Some(desk),
                        text: "now".into(),
                        files: vec![],
                        delivery: agent::Delivery::default(),
                        id: None,
                        channel: None,
                    })
                    .await
                    .is_ok(),
                "accepted after the un-pause"
            );
            agents.kill();
            unregister(&[agent::AGENT_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// A closure over the live roster (assertions on state no message reads).
    struct Peek<F>(F);

    impl<F, R> Message<Peek<F>> for Roster
    where
        F: FnOnce(&mut Roster) -> R + Send + 'static,
        R: kameo::reply::Reply + Send + 'static,
    {
        type Reply = R;

        async fn handle(&mut self, m: Peek<F>, _: &mut Context<Self, Self::Reply>) -> R {
            (m.0)(self)
        }
    }

    /// 179 SHOULD-FIX 5: the agent crate's boot reading of `paused` is the
    /// roster's own parse — `True`, a trailing comment and CRLF included.
    #[test]
    fn pause_parse_matches_the_roster() {
        let cases = [
            ("---\npaused: true\n---\nOps.\n", true),
            ("---\npaused: True\n---\nOps.\n", true),
            ("---\npaused: true # held for the audit\n---\nOps.\n", true),
            ("---\r\ntitle: Ops\r\npaused: true\r\n---\r\nOps.\r\n", true),
            ("---\npaused: false\n---\nOps.\n", false),
            ("---\ntitle: Ops\n---\npaused: true\n", false),
            ("Ops.\n", false),
        ];
        for (text, want) in cases {
            let roster = file::parse("ops", text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(roster.paused, want, "roster: {text:?}");
            assert_eq!(
                agent::supervisor::pause::agent_file_paused(text),
                want,
                "agent: {text:?}"
            );
        }
    }

    /// 179 SHOULD-FIX 6: a desk turn that ended inside the pause's drain
    /// window (nothing left to resume) is judged at the un-pause — its wake
    /// finished, its claim kept — not handed back for a second delivery.
    #[test]
    fn turn_that_ends_inside_the_drain_window_is_judged_not_redelivered() {
        with_engine_lock(async {
            let data = tmp("drain-window");
            std::fs::write(data.join(file::DIR).join("ops.md"), "Ops.\n").unwrap();
            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            let mut r = roster_on(rt.clone(), &data).await;
            let desk = r.desks["ops"].session;
            let d = r.desks.get_mut("ops").unwrap();
            d.serving = Some(plain_serving());
            d.primed = true;
            r.set_paused("ops", true).await.unwrap();
            r.sync_desks().await;
            let actor = Roster::spawn(r);
            actor
                .tell(agent::AgentEvent::TurnFinished {
                    session: desk,
                    turn: runtime::TurnId::new(),
                    stop_reason: "end_turn".into(),
                    usage: agent::Usage::default(),
                    context: 0,
                })
                .await
                .unwrap();
            let recorded = actor
                .ask(Peek(|r: &mut Roster| {
                    r.unreleased.clear();
                    r.held["ops"].finished.as_deref() == Some("end_turn")
                }))
                .await
                .unwrap();
            assert!(recorded, "the turn's end is kept for the un-pause");
            actor
                .ask(PauseAgent {
                    name: "ops".into(),
                    paused: false,
                })
                .await
                .unwrap();
            let (kept, judged) = actor
                .ask(Peek(|r: &mut Roster| {
                    (
                        r.unreleased.is_empty(),
                        r.desks.get("ops").is_some_and(|d| d.serving.is_none()),
                    )
                }))
                .await
                .unwrap();
            assert!(kept, "the claim is not released for a second delivery");
            assert!(judged, "the wake was judged handled");
            actor.kill();
            agents.kill();
            unregister(&[agent::AGENT_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
    }

    /// 194 R1: `ProposeAgent` (the `agent_propose` tool's handler) runs
    /// `PutAgent`'s `memory_effort` rules: a level the memory model does not
    /// take and an unknown word are refused, an effort without a
    /// `memory_model` is cleared; with no agent actor the word alone is checked.
    #[test]
    fn propose_checks_the_memory_effort() {
        with_engine_lock(async {
            let data = tmp("propose-effort");
            std::fs::write(
                data.join("models.json"),
                r#"{"testvis":{"id":"testvis","name":"Test Vision","models":{
                    "eyes":{"id":"eyes","name":"eyes","reasoning":true,
                    "reasoning_options":[{"type":"effort","values":["low","high"]}],
                    "limit":{"context":100000,"output":4096}}}}}"#,
            )
            .unwrap();
            let propose = |name: &str, front: &str| ProposeAgent {
                file: file::parse(name, &format!("---\n{front}---\nBody.\n")).unwrap(),
                proposed_by: "user".into(),
            };
            let refused = |r: Result<String, kameo::error::SendError<ProposeAgent, String>>| match r
            {
                Err(kameo::error::SendError::HandlerError(e)) => e,
                other => panic!("{other:?}"),
            };
            let eyes = "memory_model: testvis/eyes\n";

            let actor = Roster::spawn(roster(&data).await);
            let e = refused(
                actor
                    .ask(propose("alpha", &format!("{eyes}memory_effort: warp\n")))
                    .await,
            );
            assert_eq!(e, "memory_effort: unknown effort warp");
            actor.kill();

            let rt = Runtime::new(figment::Figment::new(), data.clone());
            let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
                .await
                .unwrap();
            let actor = Roster::spawn(roster_on(rt.clone(), &data).await);
            for (name, effort) in [("bravo", "max"), ("charlie", "warp")] {
                let e = refused(
                    actor
                        .ask(propose(name, &format!("{eyes}memory_effort: {effort}\n")))
                        .await,
                );
                assert!(e.starts_with("memory_effort: "), "{e}");
                assert!(!data.join(file::DIR).join(format!("{name}.md")).exists());
            }
            actor
                .ask(propose("delta", "memory_effort: high\n"))
                .await
                .unwrap();
            let delta = actor.ask(GetAgent("delta".into())).await.unwrap();
            assert_eq!(delta.memory_effort, None, "no memory_model, no effort");
            actor
                .ask(propose("echo", &format!("{eyes}memory_effort: low\n")))
                .await
                .unwrap();
            let echo = actor.ask(GetAgent("echo".into())).await.unwrap();
            assert_eq!(echo.memory_effort.as_deref(), Some("low"));
            actor.kill();
            agents.kill();
            unregister(&[agent::AGENT_ACTOR]);
            let _ = std::fs::remove_dir_all(&data);
        });
    }
}
