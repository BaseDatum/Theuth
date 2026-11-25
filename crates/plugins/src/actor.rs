//! Owner: B1 — `PluginsActor` and the `Message` impls of every §5.2 message, plus the host's
//! internal messages (boot, folder changes, supervisor reports, plugin → host traffic). The
//! actor never awaits a child on its mailbox: pipe requests run in spawned tasks.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::attention::SharedInbox;
use crate::config::PluginsConfig;
use crate::error::PluginsError;
use crate::events::{PluginEvent, PluginUi};
use crate::grants::{self, GrantEntry, GrantsFile};
use crate::host::{self, Conn, Launch, Paths, PipeError};
use crate::logs::PluginLog;
use crate::manifest::{self, Manifest, Parsed};
use crate::messages::*;
use crate::proc::{Backoff, ExitKind, Next, Outbox};
use crate::route::{Target, Targets};
use crate::types::*;
use crate::ui::{self, Board, RateLimit, Status, UiParams};
use crate::{PLUGINS_ACTOR, broker, catalog, ptools};

/// Requests a plugin may have in flight to the host (design §6.6).
const MAX_INFLIGHT: usize = 16;
/// `plugin_logs` cap.
const MAX_TAIL: u32 = 2000;
/// `plugin_approve_request` re-files per plugin per minute (an agent tool that never prompts).
const APPROVE_REQUESTS_PER_MIN: u32 = 3;
/// Refusal `log` lines sent back to one plugin per second; the rest are counted and reported
/// with the next one that goes out.
const REFUSALS_PER_S: u32 = 5;

/// ms since the epoch.
pub(crate) fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// Why the host stopped a process (what happens when it is gone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterStop {
    /// `SetEnabled { on: false }` → `disabled`.
    Disable,
    /// Restart, a narrower manifest, changed code, an approval: start again.
    Again,
    /// Uninstall or engine shutdown: nothing.
    Gone,
}

/// The process generation currently up.
struct Running {
    conn: Arc<Conn>,
    granted: Grants,
    manifest: Manifest,
    manifest_sha256: String,
    code_sha256: String,
    stopping: Option<AfterStop>,
}

/// Coalescing of `status` / `ui` per placement (design §6.6: last write wins).
struct Pace {
    status_rl: RateLimit,
    ui_rl: RateLimit,
    status: Option<Status>,
    ui: Option<UiParams>,
    scheduled: bool,
}

/// One plugin folder.
struct Entry {
    name: String,
    dir: PathBuf,
    parsed: Option<Parsed>,
    issues: Vec<CheckIssue>,
    requested: Option<Grants>,
    code_files: BTreeMap<String, String>,
    code_sha256: String,
    approved_files: Option<BTreeMap<String, String>>,
    state: PluginRunState,
    error: Option<String>,
    card: Option<InstallCard>,
    card_seq: u32,
    run: Option<Running>,
    epoch: u64,
    backoff: Backoff,
    log: Arc<PluginLog>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    line: ptools::Line,
    tools: Vec<String>,
    session: broker::SessionSlot,
    emit_rl: RateLimit,
    query_rl: RateLimit,
    input_rl: RateLimit,
    pace: HashMap<String, Pace>,
    inflight: Arc<AtomicUsize>,
    /// `ping` / `catalog`: answered inline, so paced like OpenUI queries.
    ctl_rl: RateLimit,
    approve_rl: RateLimit,
    refuse_rl: RateLimit,
    /// Refusals not sent since the last one that was.
    refused_quiet: u64,
}

impl Entry {
    fn manifest(&self) -> Option<&Manifest> {
        self.parsed.as_ref().map(|p| &p.manifest)
    }
}

/// The plugins host actor, registered as [`PLUGINS_ACTOR`].
#[derive(kameo::Actor)]
pub struct PluginsActor {
    rt: runtime::Runtime,
    cfg: PluginsConfig,
    paths: Paths,
    key: Option<Zeroizing<[u8; 32]>>,
    base_env: Arc<BTreeMap<String, String>>,
    grants: GrantsFile,
    plugins: BTreeMap<String, Entry>,
    board: Board,
    inbox: SharedInbox,
    targets: Targets,
    wire_catalog: Option<Vec<CatalogEntry>>,
    helper: Option<Arc<app::helper::Helper>>,
}

/// Spawn the actor on its own mailbox and register it as PLUGINS_ACTOR.
pub(crate) fn spawn(
    rt: &runtime::Runtime,
    cfg: PluginsConfig,
) -> anyhow::Result<ActorRef<PluginsActor>> {
    let actor = PluginsActor::spawn(PluginsActor::new(rt.clone(), cfg));
    actor.register(PLUGINS_ACTOR)?;
    Ok(actor)
}

// ---------------------------------------------------------------------------------------------
// Internal messages (host, watcher, supervisors).

/// `ask` → [`Booted`]: load grants, scan the folder, start what is covered, file cards.
pub(crate) struct Boot {
    /// The vault's `plugins/grants-key`; `None` = nothing verifies and nothing can be approved.
    pub key: Option<Zeroizing<[u8; 32]>>,
    /// The vault holding the grant generations (review R3); `None` = in memory only.
    pub vault: Option<Arc<secrets::Vault>>,
    /// [`host::base_env`].
    pub base_env: Arc<BTreeMap<String, String>>,
}

/// What [`Boot`] hands the background tasks.
#[derive(kameo::Reply)]
pub(crate) struct Booted {
    /// For the attention Source.
    pub inbox: SharedInbox,
    /// For the router.
    pub targets: Targets,
}

/// `tell`: plugin folders changed on disk (watcher, install, scaffold).
pub(crate) struct FolderChanged(pub BTreeSet<String>);
/// `tell`: a supervisor spawned process epoch `epoch`.
pub(crate) struct ProcessUp {
    pub name: String,
    pub epoch: u64,
}
/// `tell`: process epoch `epoch` is gone.
pub(crate) struct ProcessEnded {
    pub name: String,
    pub epoch: u64,
    pub kind: ExitKind,
    pub error: Option<String>,
    pub ran_for: Duration,
}
/// `tell`: one plugin → host message (not a response).
pub(crate) struct Incoming {
    pub name: String,
    pub epoch: u64,
    pub msg: Value,
}
/// `tell`: a backoff delay elapsed.
struct Retry {
    name: String,
    epoch: u64,
}
/// `tell`: a `ui` finished validation.
struct UiChecked {
    name: String,
    epoch: u64,
    ui: UiParams,
    result: Result<Vec<SurfaceBinding>, CheckIssue>,
}
/// `tell`: flush coalesced `status`/`ui` of one placement.
struct FlushPace {
    name: String,
    placement: String,
}
/// `ask` → `()` once every process is gone (engine shutdown).
pub(crate) struct ShutdownAll;

// ---------------------------------------------------------------------------------------------

fn short(h: &str) -> &str {
    h.get(..8).unwrap_or(h)
}

fn risk_rank(r: tools::Risk) -> u8 {
    match r {
        tools::Risk::ReadOnly => 0,
        tools::Risk::Mutating => 1,
        tools::Risk::Destructive => 2,
    }
}

/// Run blocking file work (reads, hashing, copies, `sync_all`) off the runtime's worker threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, PluginsError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| PluginsError::Io {
            message: format!("file task: {e}"),
        })
}

fn not_found(name: &str) -> PluginsError {
    PluginsError::NotFound {
        name: name.to_owned(),
    }
}

impl PluginsActor {
    fn new(rt: runtime::Runtime, cfg: PluginsConfig) -> Self {
        let paths = Paths::new(&rt.data_dir, &cfg);
        Self {
            rt,
            paths,
            cfg,
            key: None,
            base_env: Arc::new(BTreeMap::new()),
            grants: GrantsFile::default(),
            plugins: BTreeMap::new(),
            board: Board::default(),
            inbox: SharedInbox::default(),
            targets: Targets::default(),
            wire_catalog: None,
            helper: None,
        }
    }

    fn new_entry(&self, name: &str) -> Entry {
        let cfg = &self.cfg;
        Entry {
            name: name.to_owned(),
            dir: self.paths.plugins.join(name),
            parsed: None,
            issues: Vec::new(),
            requested: None,
            code_files: BTreeMap::new(),
            code_sha256: String::new(),
            approved_files: None,
            state: PluginRunState::Invalid,
            error: None,
            card: None,
            card_seq: 0,
            run: None,
            epoch: 0,
            backoff: Backoff::new(cfg),
            log: Arc::new(PluginLog::new(&self.paths.logs, name, cfg.log_max_bytes)),
            stderr_tail: Arc::default(),
            line: Arc::new(RwLock::new(None)),
            tools: Vec::new(),
            session: Arc::default(),
            emit_rl: RateLimit::new(
                cfg.emit_per_s,
                Duration::from_secs(1),
                cfg.emit_per_s.saturating_mul(4),
            ),
            query_rl: RateLimit::new(cfg.query_per_s, Duration::from_secs(1), cfg.query_per_s),
            input_rl: RateLimit::new(
                cfg.input_per_min,
                Duration::from_secs(60),
                cfg.input_per_min,
            ),
            pace: HashMap::new(),
            inflight: Arc::default(),
            ctl_rl: RateLimit::new(cfg.query_per_s, Duration::from_secs(1), cfg.query_per_s),
            approve_rl: RateLimit::new(
                APPROVE_REQUESTS_PER_MIN,
                Duration::from_secs(60),
                APPROVE_REQUESTS_PER_MIN,
            ),
            refuse_rl: RateLimit::new(REFUSALS_PER_S, Duration::from_secs(1), REFUSALS_PER_S),
            refused_quiet: 0,
        }
    }

    fn wire_catalog(&mut self) -> &[CatalogEntry] {
        if self.wire_catalog.is_none() {
            self.wire_catalog = Some(catalog::wire_entries(&self.rt));
        }
        self.wire_catalog.as_deref().unwrap_or_default()
    }

    /// Every catalog entry: public wire topics + every parsed plugin's events.
    fn catalog(&mut self, topic: Option<&str>) -> Vec<CatalogEntry> {
        let mut out: Vec<CatalogEntry> = self.wire_catalog().to_vec();
        for e in self.plugins.values() {
            if let Some(m) = e.manifest() {
                out.extend(catalog::plugin_entries(&e.name, m));
            }
        }
        if let Some(t) = topic {
            out.retain(|c| c.topic == t || c.name == t);
        }
        out
    }

    fn describe(&mut self) -> HashMap<String, String> {
        self.catalog(None)
            .into_iter()
            .map(|c| (c.name, c.describe))
            .collect()
    }

    /// Topic checks that need the catalog (design §4.3): `subscribe.topic` must be public.
    fn catalog_issues(&mut self, m: &Manifest) -> Vec<CheckIssue> {
        let known: BTreeSet<String> = self
            .wire_catalog()
            .iter()
            .map(|c| c.topic.clone())
            .collect();
        let names: BTreeSet<String> = self.wire_catalog().iter().map(|c| c.name.clone()).collect();
        let mut out = Vec::new();
        for (i, s) in m.subscribe.iter().enumerate() {
            let Some(topic) = &s.topic else { continue };
            if !known.contains(topic) {
                out.push(CheckIssue {
                    path: format!("/subscribe/{i}/topic"),
                    message: format!("`{topic}` is not a public topic of the event catalog"),
                });
                continue;
            }
            for (j, t) in s.types.iter().enumerate() {
                if !names.contains(&format!("{topic}#{t}")) {
                    out.push(CheckIssue {
                        path: format!("/subscribe/{i}/types/{j}"),
                        message: format!("`{topic}` has no public type `{t}`"),
                    });
                }
            }
        }
        out
    }

    /// Secrets named in `env` must exist in the vault (design §4.3).
    async fn secret_issues(&self, req: &Grants) -> Vec<CheckIssue> {
        if req.secrets.is_empty() {
            return Vec::new();
        }
        let Some(vault) = secrets::handle(&self.rt).await else {
            return vec![CheckIssue {
                path: "/env".into(),
                message: "the vault is unavailable: secrets not checked".into(),
            }];
        };
        req.secrets
            .iter()
            .filter(|s| vault.get(s, 0).is_none())
            .map(|s| CheckIssue {
                path: "/env".into(),
                message: format!("no secret named `{s}` in the vault"),
            })
            .collect()
    }

    fn tool_risk(&self, plugin: &str) -> impl Fn(&manifest::ToolDecl) -> tools::Risk + '_ {
        let mcp = self.paths.mcp.clone();
        let plugin = plugin.to_owned();
        move |t| ptools::risk_of(&mcp, &plugin, t).unwrap_or_else(|| manifest::hint_risk(t))
    }

    // ----- grants ------------------------------------------------------------------------------

    fn verified(&self, name: &str) -> Option<&GrantEntry> {
        self.grants.verified(self.key.as_ref()?, name)
    }

    async fn save_grants(&self) -> Result<(), PluginsError> {
        let (file, path) = (self.grants.clone(), self.paths.grants());
        blocking(move || file.save(&path))
            .await?
            .map_err(|e| PluginsError::Io {
                message: format!("grants.yaml: {e}"),
            })
    }

    fn order_index(&self, name: &str) -> (usize, String) {
        let i = self
            .grants
            .order
            .iter()
            .position(|n| n == name)
            .unwrap_or(usize::MAX);
        (i, name.to_owned())
    }

    fn ordered_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.plugins.keys().cloned().collect();
        names.sort_by_key(|n| self.order_index(n));
        names
    }

    // ----- views ------------------------------------------------------------------------------

    fn info(&self, e: &Entry) -> PluginInfo {
        let g = self.verified(&e.name);
        let order = self
            .ordered_names()
            .iter()
            .position(|n| n == &e.name)
            .unwrap_or(0);
        let m = e.manifest();
        PluginInfo {
            name: e.name.clone(),
            version: m.map(|m| m.version.clone()).unwrap_or_default(),
            description: m.map(|m| m.description.clone()).unwrap_or_default(),
            state: e.state,
            error: e.error.clone(),
            enabled: g.is_some_and(|g| g.enabled),
            needs_approval: e.card.is_some(),
            card: e.card.as_ref().map(|c| c.id.clone()),
            placements: m
                .map(|m| {
                    m.placements
                        .iter()
                        .map(|p| {
                            manifest::placement_info(
                                p,
                                &e.name,
                                self.grants.is_hidden(&e.name, &p.id),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            tools: m
                .map(|m| m.tools.iter().map(|t| t.name.clone()).collect())
                .unwrap_or_default(),
            dir: e.dir.display().to_string(),
            log: e.log.path_today().display().to_string(),
            approved_at: g.map(|g| g.approved_at.clone()),
            order: u32::try_from(order).unwrap_or(u32::MAX),
        }
    }

    fn info_of(&self, name: &str) -> Result<PluginInfo, PluginsError> {
        self.plugins
            .get(name)
            .map(|e| self.info(e))
            .ok_or_else(|| not_found(name))
    }

    fn list(&self) -> Vec<PluginInfo> {
        self.ordered_names()
            .iter()
            .filter_map(|n| self.plugins.get(n))
            .map(|e| self.info(e))
            .collect()
    }

    fn snapshots(&self) -> Vec<SurfaceSnapshot> {
        let mut out = Vec::new();
        for n in self.ordered_names() {
            let Some(e) = self.plugins.get(&n) else {
                continue;
            };
            let Some(m) = e.manifest() else { continue };
            for p in &m.placements {
                let rev = self.board.content(&n, &p.id).map_or(0, |c| c.revision);
                out.push(ui::snapshot(
                    &n,
                    p,
                    e.state,
                    self.board.status(&n, &p.id),
                    rev,
                ));
            }
        }
        out
    }

    async fn publish_state(&self, name: &str) {
        let Some(e) = self.plugins.get(name) else {
            return;
        };
        self.rt
            .publish(PluginUi::PluginState {
                plugin: name.to_owned(),
                state: e.state,
                error: e.error.clone(),
            })
            .await;
        // Dots follow the state (design §9.3).
        if let Some(m) = e.manifest() {
            for p in m.placements.iter().filter(|p| p.status) {
                let s = ui::snapshot(name, p, e.state, self.board.status(name, &p.id), 0);
                self.rt
                    .publish(PluginUi::PluginStatus {
                        plugin: name.to_owned(),
                        placement: p.id.clone(),
                        dot: s.dot,
                        badge: s.badge,
                        label: s.label,
                        tooltip: s.tooltip,
                    })
                    .await;
            }
        }
    }

    async fn changed(&self) {
        self.rt.publish(PluginUi::PluginsChanged).await;
    }

    async fn set_state(&mut self, name: &str, state: PluginRunState, error: Option<String>) {
        if let Some(e) = self.plugins.get_mut(name) {
            if e.state == state && e.error == error {
                return;
            }
            e.state = state;
            e.error = error;
            e.log.line(
                "host",
                &format!(
                    "state: {}",
                    serde_json::to_value(state)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                ),
            );
        }
        self.publish_state(name).await;
    }

    // ----- loading and the gate ---------------------------------------------------------------

    /// Re-read one folder: parse, hashes, requested grants (file IO off the runtime threads).
    async fn reload(&mut self, name: &str) {
        let mut e = self
            .plugins
            .remove(name)
            .unwrap_or_else(|| self.new_entry(name));
        let (dir, mcp, plugin) = (e.dir.clone(), self.paths.mcp.clone(), name.to_owned());
        let read = blocking(move || {
            manifest::load(&dir).map(|parsed| {
                let risk = |t: &manifest::ToolDecl| {
                    ptools::risk_of(&mcp, &plugin, t).unwrap_or_else(|| manifest::hint_risk(t))
                };
                let requested = manifest::requested(&parsed.manifest, &risk);
                let files = manifest::code_files(&dir);
                let sha = manifest::code_hash(&files);
                (parsed, requested, files, sha)
            })
        })
        .await;
        match read {
            Ok(Ok((parsed, requested, files, sha))) => {
                let mut issues = self.catalog_issues(&parsed.manifest);
                issues.extend(ptools::clashes(&parsed.manifest).await);
                if let Err(err) = manifest::check_name(name) {
                    issues.push(CheckIssue {
                        path: "/name".into(),
                        message: err.to_string(),
                    });
                }
                e.requested = Some(requested);
                e.code_files = files;
                e.code_sha256 = sha;
                if issues.is_empty() {
                    e.issues = parsed.warnings.clone();
                    e.parsed = Some(parsed);
                } else {
                    e.issues = issues;
                    e.parsed = None;
                }
            }
            Ok(Err((errors, _))) => {
                e.parsed = None;
                e.requested = None;
                e.issues = errors;
            }
            Err(err) => {
                e.parsed = None;
                e.requested = None;
                e.issues = vec![CheckIssue {
                    path: String::new(),
                    message: err.to_string(),
                }];
            }
        }
        self.plugins.insert(name.to_owned(), e);
    }

    /// The card this plugin needs now, if any (design §5, §10.2).
    fn needed_card(&mut self, name: &str) -> Option<InstallCard> {
        let dev = self.cfg.dev;
        let describe = self.describe();
        let g = self.verified(name).cloned();
        let unverified = g.is_none() && self.grants.entries.contains_key(name);
        let e = self.plugins.get(name)?;
        let parsed = e.parsed.as_ref()?;
        let req = e.requested.as_ref()?;
        let code = (!dev && manifest::needs_code_hash(req)).then(|| e.code_sha256.clone());
        let (kind, prior, changed) = match &g {
            None => (CardKind::Install, None, Vec::new()),
            Some(g) if !grants::covers(&g.grants, req) => {
                (CardKind::Widen, Some(g.grants.clone()), Vec::new())
            }
            // Code changed under a hashed grant, or a grant approved hashless (`plugins.dev`)
            // now needs a hash: either way the code was never approved as it is.
            Some(g)
                if !dev
                    && match (&g.code_sha256, &code) {
                        (Some(old), _) => old != &e.code_sha256,
                        (None, need) => need.is_some(),
                    } =>
            {
                let changed: Vec<String> = match &e.approved_files {
                    Some(old) => e
                        .code_files
                        .iter()
                        .filter(|(p, h)| old.get(*p) != Some(*h))
                        .map(|(p, _)| p.clone())
                        .chain(
                            old.keys()
                                .filter(|p| !e.code_files.contains_key(*p))
                                .cloned(),
                        )
                        .collect(),
                    None => e.code_files.keys().cloned().collect(),
                };
                (CardKind::CodeChanged, None, changed)
            }
            Some(_) => return None,
        };
        let lines = if kind == CardKind::CodeChanged {
            Vec::new()
        } else {
            manifest::card_lines(
                &parsed.manifest,
                req,
                prior.as_ref(),
                &e.dir,
                self.cfg.input_per_min,
                &|n| describe.get(n).cloned(),
            )
        };
        let risk = lines
            .iter()
            .map(|l| l.risk)
            .max_by_key(|r| risk_rank(*r))
            .unwrap_or(tools::Risk::Destructive);
        let risk =
            if risk_rank(risk) < risk_rank(tools::Risk::Destructive) && kind == CardKind::Install {
                tools::Risk::Destructive
            } else {
                risk
            };
        let c8 = code.as_deref().map_or("00000000", short).to_owned();
        Some(InstallCard {
            id: format!(
                "plugin:{name}:{}:{c8}:{}",
                short(&parsed.manifest_sha256),
                e.card_seq
            ),
            plugin: name.to_owned(),
            version: parsed.manifest.version.clone(),
            kind,
            manifest_sha256: parsed.manifest_sha256.clone(),
            code_sha256: code,
            risk,
            lines,
            changed_paths: changed,
            unverified,
            dir: e.dir.display().to_string(),
            at_ms: now_ms(),
        })
    }

    /// File or withdraw the card; keep an identical pending card (same id) as it is.
    fn sync_card(&mut self, name: &str) -> Option<InstallCard> {
        let card = self.needed_card(name);
        let e = self.plugins.get_mut(name)?;
        match card {
            Some(mut c) => {
                if let Some(old) = &e.card
                    && old.id == c.id
                {
                    c.at_ms = old.at_ms;
                }
                e.card = Some(c.clone());
                if let Ok(mut i) = self.inbox.lock() {
                    i.file(c.clone());
                }
                Some(c)
            }
            None => {
                e.card = None;
                if let Ok(mut i) = self.inbox.lock() {
                    i.withdraw(name);
                }
                None
            }
        }
    }

    /// Bring one plugin to where its folder, grant and state say it should be.
    async fn evaluate(&mut self, name: &str, ctx_ref: &ActorRef<Self>) {
        self.reload(name).await;
        let card = self.sync_card(name);
        let Some(e) = self.plugins.get(name) else {
            return;
        };
        if e.parsed.is_none() {
            let msg = e
                .issues
                .iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; ");
            if e.run.is_some() {
                // The running process keeps its approved manifest; the edit is reported.
                self.set_state(name, PluginRunState::Running, Some(msg))
                    .await;
            } else {
                self.set_state(name, PluginRunState::Invalid, Some(msg))
                    .await;
            }
            return;
        }
        let enabled = self.verified(name).is_some_and(|g| g.enabled);
        let (msha, csha) = {
            let p = e
                .parsed
                .as_ref()
                .map(|p| p.manifest_sha256.clone())
                .unwrap_or_default();
            (p, e.code_sha256.clone())
        };
        match (&e.run, card) {
            (Some(_), Some(_)) => {
                // Widened or changed code: the old process keeps its old grant (design §5).
            }
            (Some(run), None) => {
                let differs =
                    run.manifest_sha256 != msha || run.code_sha256 != csha || self.cfg.dev;
                if run.stopping.is_none() && differs {
                    if !enabled {
                        self.stop(name, AfterStop::Disable);
                    } else if !self.apply_live(name) {
                        self.stop(name, AfterStop::Again);
                    }
                }
            }
            (None, Some(_)) => {
                if e.state != PluginRunState::AwaitingApproval {
                    self.set_state(name, PluginRunState::AwaitingApproval, None)
                        .await;
                }
            }
            (None, None) => {
                if !enabled {
                    self.set_state(name, PluginRunState::Disabled, None).await;
                } else if matches!(
                    e.state,
                    PluginRunState::Invalid
                        | PluginRunState::AwaitingApproval
                        | PluginRunState::Exited
                        | PluginRunState::Failed
                        | PluginRunState::Disabled
                ) {
                    if let Some(e) = self.plugins.get_mut(name) {
                        e.backoff.reset();
                    }
                    self.start(name, ctx_ref).await;
                }
            }
        }
    }

    // ----- processes --------------------------------------------------------------------------

    fn hello(&self, m: &Manifest, granted: &Grants, data_dir: &Path, engine: &str) -> Value {
        json!({
            "protocol": crate::PROTOCOL,
            "plugin": m.name,
            "version": m.version,
            "theuth": platform::version::get(),
            "engine": engine,
            "data_dir": data_dir.display().to_string(),
            "grants": grants_view(granted),
            "clients": {},
        })
    }

    /// Start one covered plugin (a new process generation).
    async fn start(&mut self, name: &str, me: &ActorRef<Self>) {
        let Some(g) = self.verified(name).cloned() else {
            return;
        };
        let Some((parsed, req)) = self
            .plugins
            .get(name)
            .and_then(|e| Some((e.parsed.clone()?, e.requested.clone()?)))
        else {
            return;
        };
        if !grants::covers(&g.grants, &req) {
            return;
        }
        let data_dir = self.paths.data.join(name);
        let timeout = Duration::from_millis(self.cfg.request_timeout_ms);
        let engine = agent::communication::mesh::engine_names().await.0;
        let outbox = Outbox::new(self.cfg.queue_frames);
        // `hello` is queued before the outbox is shared with the router, tools or actions, so it
        // is the first line whatever reaches the outbox next (design §6.2).
        outbox.first(json!({
            "jsonrpc": "2.0", "method": "hello",
            "params": self.hello(&parsed.manifest, &req, &data_dir, &engine),
        }));
        let conn = Conn::new(name, outbox.clone(), timeout);
        let Some(e) = self.plugins.get_mut(name) else {
            return;
        };
        e.epoch += 1;
        if let Ok(mut l) = e.line.write() {
            *l = Some(conn.clone());
        }
        e.approved_files = Some(e.code_files.clone());
        e.run = Some(Running {
            conn: conn.clone(),
            granted: req.clone(),
            manifest: parsed.manifest.clone(),
            manifest_sha256: parsed.manifest_sha256.clone(),
            code_sha256: e.code_sha256.clone(),
            stopping: None,
        });
        if let Ok(mut t) = e.stderr_tail.lock() {
            t.clear();
        }
        let launch = Launch {
            name: name.to_owned(),
            epoch: e.epoch,
            dir: e.dir.clone(),
            manifest: parsed.manifest.clone(),
            data_dir,
            base_env: self.base_env.clone(),
            conn,
            log: e.log.clone(),
            stderr_tail: e.stderr_tail.clone(),
            cfg: self.cfg.clone(),
            pgids: self.paths.pgids(),
            actor: me.clone(),
        };
        if let Ok(mut t) = self.targets.write() {
            t.retain(|t| t.name != name);
            t.push(Target {
                name: name.to_owned(),
                subs: parsed.manifest.subscribe.clone(),
                granted: req.subscribe.clone(),
                outbox,
            });
        }
        self.register_tools(name, &parsed);
        tokio::spawn(host::supervise(launch));
        self.set_state(name, PluginRunState::Starting, None).await;
    }

    /// Register the manifest's tools (they survive restarts) and drop the ones it no longer has.
    fn register_tools(&mut self, name: &str, parsed: &Parsed) {
        let Some(e) = self.plugins.get_mut(name) else {
            return;
        };
        let tools = ptools::build(
            &self.paths.mcp,
            &parsed.manifest,
            &parsed.manifest_sha256,
            &e.line,
        );
        let names: Vec<String> = tools.iter().map(|t| t.name().to_owned()).collect();
        let gone: Vec<String> = e
            .tools
            .iter()
            .filter(|t| !names.contains(t))
            .cloned()
            .collect();
        e.tools = names;
        tokio::spawn(ptools::sync(self.rt.clone(), name.to_owned(), gone, tools));
    }

    /// A covered manifest change that leaves the spawn recipe (`run`, `cwd`, `env`) and the code
    /// as they are: the running process takes the new grants and is told with `granted` (design
    /// §6.2) instead of being restarted. False = the caller restarts it. `plugins.dev` always
    /// restarts.
    fn apply_live(&mut self, name: &str) -> bool {
        if self.cfg.dev {
            return false;
        }
        let Some(e) = self.plugins.get(name) else {
            return false;
        };
        let (Some(parsed), Some(req), Some(run)) =
            (e.parsed.clone(), e.requested.clone(), e.run.as_ref())
        else {
            return false;
        };
        let (old, new) = (&run.manifest, &parsed.manifest);
        if run.stopping.is_some()
            || run.code_sha256 != e.code_sha256
            || old.run != new.run
            || old.cwd != new.cwd
            || old.env != new.env
        {
            return false;
        }
        let (conn, changed) = (run.conn.clone(), run.granted != req);
        if let Some(run) = self.plugins.get_mut(name).and_then(|e| e.run.as_mut()) {
            run.granted = req.clone();
            run.manifest = parsed.manifest.clone();
            run.manifest_sha256 = parsed.manifest_sha256.clone();
        }
        if let Ok(mut t) = self.targets.write()
            && let Some(t) = t.iter_mut().find(|t| t.name == name)
        {
            t.subs = parsed.manifest.subscribe.clone();
            t.granted = req.subscribe.clone();
        }
        self.register_tools(name, &parsed);
        if changed {
            self.log_to(name, "host", "grants changed while running: `granted` sent");
            conn.notify("granted", json!({ "grants": grants_view(&req) }));
        }
        true
    }

    /// Ask the running process to stop (`shutdown` ahead of both lanes, then grace, then kill).
    fn stop(&mut self, name: &str, after: AfterStop) {
        let grace = self.cfg.shutdown_grace_ms;
        let Some(run) = self.plugins.get_mut(name).and_then(|e| e.run.as_mut()) else {
            return;
        };
        if run.stopping.is_some() {
            run.stopping = Some(after);
            return;
        }
        run.stopping = Some(after);
        let reason = match after {
            AfterStop::Disable => "disable",
            AfterStop::Again => "restart",
            AfterStop::Gone => "shutdown",
        };
        run.conn.outbox.shutdown(json!({
            "jsonrpc": "2.0", "method": "shutdown", "params": { "reason": reason, "grace_ms": grace }
        }));
        run.conn.stop.cancel();
    }

    async fn unregister_tools(&mut self, name: &str) {
        let Some(e) = self.plugins.get_mut(name) else {
            return;
        };
        let gone = std::mem::take(&mut e.tools);
        if !gone.is_empty() {
            tokio::spawn(ptools::sync(
                self.rt.clone(),
                name.to_owned(),
                gone,
                Vec::new(),
            ));
        }
    }

    /// Sign and save `entry`. Replacing an earlier entry (re-approve, enable, disable) bumps the
    /// plugin's generation first, so the earlier entry cannot be written back (review R3).
    async fn sign_and_store(&mut self, name: &str, entry: GrantEntry) -> Result<(), PluginsError> {
        let Some(key) = self.key.clone() else {
            return Err(PluginsError::Io {
                message: "the vault's grants key is unavailable".into(),
            });
        };
        let io = |e: anyhow::Error| PluginsError::Io {
            message: format!("grants: {e:#}"),
        };
        if self.grants.entries.contains_key(name) {
            self.grants.bump(name).await.map_err(io)?;
        }
        self.grants.insert_signed(&key, name, entry).map_err(io)?;
        self.save_grants().await
    }

    // ----- plugin → host ----------------------------------------------------------------------

    fn log_to(&self, name: &str, source: &str, text: &str) {
        if let Some(e) = self.plugins.get(name) {
            e.log.line(source, &tools::secrets::redact(text));
        }
    }

    /// The validator's verdict goes back to the plugin as a `log` error (design §9.4) and into its
    /// log, at most [`REFUSALS_PER_S`] a second; the ones over are counted into the next.
    fn refuse(&mut self, name: &str, conn: &Conn, path: &str, message: &str) {
        let Some(e) = self.plugins.get_mut(name) else {
            return;
        };
        if e.refuse_rl.check(Instant::now()).is_err() {
            e.refused_quiet += 1;
            return;
        }
        let message = match std::mem::take(&mut e.refused_quiet) {
            0 => message.to_owned(),
            n => format!("{message} (and {n} earlier refusals not reported)"),
        };
        self.log_to(name, "host", &format!("refused {path}: {message}"));
        conn.notify(
            "log",
            json!({ "level": "error", "message": message, "path": path }),
        );
    }

    async fn on_request(
        &mut self,
        name: &str,
        conn: Arc<Conn>,
        id: Value,
        method: &str,
        params: Value,
    ) {
        let Some(e) = self.plugins.get_mut(name) else {
            return;
        };
        let Some(run) = e.run.as_ref() else { return };
        let granted = run.granted.clone();
        if e.inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            conn.reply(id, Err(PipeError::rate_limited(100)));
            return;
        }
        if matches!(method, "ping" | "catalog")
            && let Err(ms) = e.ctl_rl.check(Instant::now())
        {
            conn.reply(id, Err(PipeError::rate_limited(ms)));
            return;
        }
        match method {
            "ping" => conn.reply(id, Ok(broker::ping())),
            "catalog" => {
                let topic = params
                    .get("topic")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let entries = self.catalog(topic.as_deref());
                conn.reply(id, Ok(serde_json::to_value(entries).unwrap_or_default()));
            }
            "call" => match broker::check_call(&params, granted.capabilities.call.as_ref()) {
                Err(err) => {
                    self.log_to(name, "host", &err.message);
                    conn.reply(id, Err(err));
                }
                Ok((tool, args)) => {
                    let Some(grant) = granted.capabilities.call.clone() else {
                        conn.reply(id, Err(PipeError::not_granted("capability", "call")));
                        return;
                    };
                    let inflight = e.inflight.clone();
                    inflight.fetch_add(1, Ordering::Relaxed);
                    let (slot, plugin) = (e.session.clone(), name.to_owned());
                    tokio::spawn(async move {
                        let r = broker::call(slot, plugin, grant, tool, args).await;
                        inflight.fetch_sub(1, Ordering::Relaxed);
                        conn.reply(id, r);
                    });
                }
            },
            "input" => {
                if !granted.capabilities.input {
                    self.log_to(name, "host", "not granted: capability input");
                    conn.reply(id, Err(PipeError::not_granted("capability", "input")));
                    return;
                }
                if let Err(ms) = e.input_rl.check(Instant::now()) {
                    conn.reply(id, Err(PipeError::rate_limited(ms)));
                    return;
                }
                let inflight = e.inflight.clone();
                inflight.fetch_add(1, Ordering::Relaxed);
                let plugin = name.to_owned();
                tokio::spawn(async move {
                    let r = broker::input(plugin, params).await;
                    inflight.fetch_sub(1, Ordering::Relaxed);
                    conn.reply(id, r);
                });
            }
            other => conn.reply(id, Err(PipeError::not_found(other))),
        }
    }

    async fn on_notification(
        &mut self,
        name: &str,
        conn: Arc<Conn>,
        epoch: u64,
        method: &str,
        params: Value,
        me: &ActorRef<Self>,
    ) {
        let Some(e) = self.plugins.get(name) else {
            return;
        };
        let Some(run) = e.run.as_ref() else { return };
        let granted = run.granted.clone();
        let manifest = run.manifest.clone();
        let log = e.log.clone();
        match method {
            "log" => {
                let level = params
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("info");
                let mut text = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(f) = params.get("fields").filter(|f| !f.is_null()) {
                    text.push(' ');
                    text.push_str(&f.to_string());
                }
                log.line(&format!("log:{level}"), &tools::secrets::redact(&text));
            }
            "emit" => {
                let Some(event) = params.get("event").and_then(Value::as_str) else {
                    return;
                };
                let Some(decl) = manifest
                    .emits
                    .get(event)
                    .filter(|_| granted.emits.iter().any(|x| x == event))
                else {
                    self.refuse(name, &conn, "/event", &format!("not granted: emit {event}"));
                    return;
                };
                let Some(mut data) = params.get("data").filter(|d| d.is_object()).cloned() else {
                    self.refuse(name, &conn, "/data", "emit data must be a JSON object");
                    return;
                };
                let over = self
                    .plugins
                    .get_mut(name)
                    .is_none_or(|e| e.emit_rl.check(Instant::now()).is_err());
                if over {
                    log.line("host", &format!("emit {event} dropped: over emit_per_s"));
                    return;
                }
                let session = match params.get("session") {
                    Some(s) if !s.is_null() => {
                        serde_json::from_value::<runtime::SessionId>(s.clone()).ok()
                    }
                    _ => None,
                };
                if decl.scope == Scoped::Session && session.is_none() {
                    self.refuse(
                        name,
                        &conn,
                        "/session",
                        &format!("{event} is session-scoped: `session` is required"),
                    );
                    return;
                }
                tools::secrets::redact_value(&mut data);
                let ev = PluginEvent {
                    plugin: name.to_owned(),
                    event: event.to_owned(),
                    version: decl.version,
                    session,
                    data,
                };
                self.rt.publish(ev).await;
            }
            "status" => {
                let (placement, mut status) = match ui::parse_status(&params) {
                    Ok(s) => s,
                    Err(err) => {
                        self.refuse(name, &conn, "/", &err.to_string());
                        return;
                    }
                };
                let declared = manifest
                    .placements
                    .iter()
                    .any(|p| p.id == placement && p.status);
                if !declared || !granted.placements.contains(&placement) {
                    self.refuse(
                        name,
                        &conn,
                        "/placement",
                        &format!("not granted: placement {placement}"),
                    );
                    return;
                }
                status.label = status
                    .label
                    .map(|l| tools::secrets::redact(&l).into_owned());
                status.tooltip = status
                    .tooltip
                    .map(|t| tools::secrets::redact(&t).into_owned());
                let admitted = {
                    let Some(e) = self.plugins.get_mut(name) else {
                        return;
                    };
                    let pace = pace_of(e, &placement, &self.cfg);
                    match pace.status_rl.check(Instant::now()) {
                        Ok(()) => Ok(status),
                        Err(ms) => {
                            pace.status = Some(status);
                            Err(ms)
                        }
                    }
                };
                match admitted {
                    Ok(status) => self.apply_status(name, &placement, status).await,
                    Err(ms) => self.schedule_flush(name, &placement, ms, me),
                }
            }
            "ui" => {
                let ui = match ui::parse_ui(&params) {
                    Ok(u) => u,
                    Err(err) => {
                        self.refuse(name, &conn, "/", &err.to_string());
                        return;
                    }
                };
                let decl = manifest.placements.iter().find(|p| p.id == ui.placement);
                if decl.is_none() || !granted.placements.contains(&ui.placement) {
                    self.refuse(
                        name,
                        &conn,
                        "/placement",
                        &format!("not granted: placement {}", ui.placement),
                    );
                    return;
                }
                if decl.and_then(|p| p.format) != Some(ui.format) {
                    self.refuse(
                        name,
                        &conn,
                        "/format",
                        "format differs from the placement's declared format",
                    );
                    return;
                }
                let placement = ui.placement.clone();
                let admitted = {
                    let Some(e) = self.plugins.get_mut(name) else {
                        return;
                    };
                    let pace = pace_of(e, &placement, &self.cfg);
                    match pace.ui_rl.check(Instant::now()) {
                        Ok(()) => Ok(ui),
                        Err(ms) => {
                            pace.ui = Some(ui);
                            Err(ms)
                        }
                    }
                };
                match admitted {
                    Ok(ui) => self.check_ui(name, epoch, ui, me),
                    Err(ms) => self.schedule_flush(name, &placement, ms, me),
                }
            }
            "invalidate" => {
                let Some(placement) = params.get("placement").and_then(Value::as_str) else {
                    return;
                };
                if granted.placements.iter().any(|p| p == placement) {
                    self.rt
                        .publish(PluginUi::PluginInvalidate {
                            plugin: name.to_owned(),
                            placement: placement.to_owned(),
                        })
                        .await;
                }
            }
            "notify" => {
                if !granted.capabilities.notify {
                    self.refuse(name, &conn, "/", "not granted: capability notify");
                    return;
                }
                let title = params.get("title").and_then(Value::as_str).unwrap_or(name);
                let body = params
                    .get("body")
                    .and_then(Value::as_str)
                    .map(|b| tools::secrets::redact(b).into_owned());
                let title = tools::secrets::redact(title).into_owned();
                // `open` names one of its granted placements; anything else is dropped.
                let open = params
                    .pointer("/open/placement")
                    .and_then(Value::as_str)
                    .filter(|p| granted.placements.iter().any(|g| g == p))
                    .map(|p| crate::attention::placement_link(name, p));
                // OS notifications at warn/error are deferred (as-built); the level is recorded.
                let note = crate::attention::Note {
                    id: String::new(),
                    title,
                    body,
                    level: crate::attention::NoteLevel::parse(
                        params.get("level").and_then(Value::as_str),
                    ),
                    open,
                    at_ms: now_ms(),
                };
                if let Ok(mut i) = self.inbox.lock() {
                    i.note(name, note);
                }
            }
            _ => {} // unknown notifications are ignored (design §7.5)
        }
    }

    fn schedule_flush(&mut self, name: &str, placement: &str, after_ms: u64, me: &ActorRef<Self>) {
        let Some(pace) = self
            .plugins
            .get_mut(name)
            .and_then(|e| e.pace.get_mut(placement))
        else {
            return;
        };
        if pace.scheduled {
            return;
        }
        pace.scheduled = true;
        let (me, name, placement) = (me.clone(), name.to_owned(), placement.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(after_ms.max(1))).await;
            let _ = me.tell(FlushPace { name, placement }).await;
        });
    }

    async fn apply_status(&mut self, name: &str, placement: &str, status: Status) {
        if !self.board.set_status(name, placement, status) {
            return;
        }
        let Some(e) = self.plugins.get(name) else {
            return;
        };
        let Some(p) = e
            .manifest()
            .and_then(|m| m.placements.iter().find(|p| p.id == placement))
        else {
            return;
        };
        let s = ui::snapshot(name, p, e.state, self.board.status(name, placement), 0);
        self.rt
            .publish(PluginUi::PluginStatus {
                plugin: name.to_owned(),
                placement: placement.to_owned(),
                dot: s.dot,
                badge: s.badge,
                label: s.label,
                tooltip: s.tooltip,
            })
            .await;
    }

    fn check_ui(&mut self, name: &str, epoch: u64, ui: UiParams, me: &ActorRef<Self>) {
        let helper = if ui.format == SurfaceFormat::Openui {
            Some(
                self.helper
                    .get_or_insert_with(|| {
                        Arc::new(app::helper::Helper::new(self.rt.shutdown.clone()))
                    })
                    .clone(),
            )
        } else {
            None
        };
        let (me, name) = (me.clone(), name.to_owned());
        tokio::spawn(async move {
            let result = ui::validate(helper.as_deref(), &name, ui.format, &ui.source).await;
            let _ = me
                .tell(UiChecked {
                    name,
                    epoch,
                    ui,
                    result,
                })
                .await;
        });
    }

    fn conn_of(&self, name: &str) -> Result<Arc<Conn>, PluginsError> {
        let e = self.plugins.get(name).ok_or_else(|| not_found(name))?;
        if e.card.is_some() && e.run.is_none() {
            return Err(PluginsError::AwaitingApproval {
                name: name.to_owned(),
            });
        }
        match &e.run {
            Some(r) if r.stopping.is_none() && e.state == PluginRunState::Running => {
                Ok(r.conn.clone())
            }
            _ => Err(PluginsError::NotRunning {
                name: name.to_owned(),
            }),
        }
    }

    fn placement(
        &self,
        name: &str,
        placement: &str,
    ) -> Result<manifest::PlacementDecl, PluginsError> {
        let e = self.plugins.get(name).ok_or_else(|| not_found(name))?;
        let run = e.run.as_ref().ok_or_else(|| PluginsError::NotRunning {
            name: name.to_owned(),
        })?;
        let p = run
            .manifest
            .placements
            .iter()
            .find(|p| p.id == placement)
            .ok_or_else(|| PluginsError::Invalid {
                message: format!("{name} has no placement {placement}"),
            })?;
        if !run.granted.placements.iter().any(|x| x == placement) {
            return Err(PluginsError::NotGranted {
                what: format!("placement {placement}"),
            });
        }
        Ok(p.clone())
    }
}

fn pace_of<'a>(e: &'a mut Entry, placement: &str, cfg: &PluginsConfig) -> &'a mut Pace {
    e.pace.entry(placement.to_owned()).or_insert_with(|| Pace {
        status_rl: RateLimit::new(cfg.ui_per_s, Duration::from_secs(1), cfg.ui_per_s),
        ui_rl: RateLimit::new(cfg.ui_per_s, Duration::from_secs(1), cfg.ui_per_s),
        status: None,
        ui: None,
        scheduled: false,
    })
}

/// `hello.params.grants` / `granted.params.grants` (design §6.2).
pub(crate) fn grants_view(g: &Grants) -> Value {
    json!({
        "subscribe": g.subscribe,
        "emits": g.emits,
        "placements": g.placements,
        "tools": g.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        "capabilities": {
            "notify": g.capabilities.notify,
            "input": g.capabilities.input,
            "call": g.capabilities.call,
        },
    })
}

/// A `command` reply (design §6.2): `{text}`, `{ui: {format, source, text}}` or `{}`. OpenUI is
/// refused in replies (contract §5.1 CommandUi).
fn command_reply(v: &Value) -> Result<CommandReply, PluginsError> {
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .map(|t| tools::secrets::redact(t).into_owned());
    let ui = match v.get("ui").filter(|u| !u.is_null()) {
        None => None,
        Some(u) => {
            let format: SurfaceFormat = serde_json::from_value(
                u.get("format").cloned().unwrap_or_default(),
            )
            .map_err(|_| PluginsError::Invalid {
                message: "command reply: ui.format".into(),
            })?;
            if format == SurfaceFormat::Openui {
                return Err(PluginsError::Invalid {
                    message: "command replies take json-render or markdown, not openui".into(),
                });
            }
            let source = u.get("source").and_then(Value::as_str).unwrap_or_default();
            let fallback = u.get("text").and_then(Value::as_str).unwrap_or_default();
            if format == SurfaceFormat::JsonRender && surface::parse(source).is_err() {
                return Err(PluginsError::Invalid {
                    message: "command reply: invalid json-render".into(),
                });
            }
            Some(CommandUi {
                format,
                source: tools::secrets::redact(source).into_owned(),
                text: tools::secrets::redact(fallback).into_owned(),
            })
        }
    };
    Ok(CommandReply { text, ui })
}

/// Files one install copies at most (review S2).
const INSTALL_MAX_FILES: usize = 10_000;
/// Bytes one install copies at most.
const INSTALL_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Copy the plugin folder `src` to `tmp` under `plugins`, then rename it to `dest`. Refuses a
/// source that is, holds or sits inside the plugins folder, any symlink, and more than
/// [`INSTALL_MAX_FILES`] files or [`INSTALL_MAX_BYTES`] bytes; `tmp` is removed on failure.
fn install_copy(src: &Path, plugins: &Path, tmp: &Path, dest: &Path) -> Result<(), PluginsError> {
    let io = |e: std::io::Error| PluginsError::Io {
        message: format!("install: {e}"),
    };
    let src = std::fs::canonicalize(src).map_err(|e| PluginsError::Invalid {
        message: format!("{}: {e}", src.display()),
    })?;
    std::fs::create_dir_all(plugins).map_err(io)?;
    let plugins = std::fs::canonicalize(plugins).map_err(io)?;
    if src.starts_with(&plugins) || plugins.starts_with(&src) {
        return Err(PluginsError::Invalid {
            message: format!(
                "{} is, holds or sits inside the plugins folder {}",
                src.display(),
                plugins.display()
            ),
        });
    }
    let mut left = (INSTALL_MAX_FILES, INSTALL_MAX_BYTES);
    let res = copy_dir(&src, tmp, &mut left).and_then(|()| std::fs::rename(tmp, dest).map_err(io));
    if res.is_err() {
        let _ = std::fs::remove_dir_all(tmp);
    }
    res
}

/// Recursive copy without `.git`, `node_modules`, `target`; `left` = (files, bytes) still allowed.
fn copy_dir(from: &Path, to: &Path, left: &mut (usize, u64)) -> Result<(), PluginsError> {
    let io = |e: std::io::Error| PluginsError::Io {
        message: format!("install: {e}"),
    };
    std::fs::create_dir_all(to).map_err(io)?;
    for entry in std::fs::read_dir(from).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|n| n == ".git" || n == "node_modules" || n == "target")
        {
            continue;
        }
        let ty = entry.file_type().map_err(io)?;
        let dest = to.join(&name);
        if ty.is_symlink() {
            return Err(PluginsError::Invalid {
                message: format!(
                    "{} is a symlink: a plugin folder is installed without links",
                    entry.path().display()
                ),
            });
        } else if ty.is_dir() {
            copy_dir(&entry.path(), &dest, left)?;
        } else if ty.is_file() {
            let len = entry.metadata().map_err(io)?.len();
            if left.0 == 0 || len > left.1 {
                return Err(PluginsError::Invalid {
                    message: format!(
                        "the plugin folder is over {INSTALL_MAX_FILES} files or {} MiB",
                        INSTALL_MAX_BYTES / (1024 * 1024)
                    ),
                });
            }
            left.0 -= 1;
            left.1 -= len;
            std::fs::copy(entry.path(), &dest).map_err(io)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Internal message handlers.

impl Message<Boot> for PluginsActor {
    type Reply = Booted;
    async fn handle(&mut self, m: Boot, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.key = m.key;
        self.base_env = m.base_env;
        let (grants_path, dir) = (self.paths.grants(), self.paths.plugins.clone());
        let (file, names) = blocking(move || {
            let names: BTreeSet<String> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                        .filter(|n| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
            (GrantsFile::load(&grants_path), names)
        })
        .await
        .unwrap_or_default();
        self.grants = file;
        if let Some(vault) = m.vault
            && let Err(e) = self.grants.attach(vault).await
        {
            // Fail closed: nothing verifies, nothing can be approved.
            tracing::warn!(error = %e, "plugins: grant generations unreadable");
        }
        let me = ctx.actor_ref().clone();
        for n in &names {
            self.evaluate(n, &me).await;
        }
        self.changed().await;
        Booted {
            inbox: self.inbox.clone(),
            targets: self.targets.clone(),
        }
    }
}

impl Message<FolderChanged> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: FolderChanged, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        for name in m.0 {
            let dir = self.paths.plugins.join(&name);
            if !dir.join(manifest::FILE).is_file() {
                if self.plugins.contains_key(&name) && !dir.exists() {
                    // Folder removed by hand: stop and forget it (the grant stays until uninstall).
                    self.stop(&name, AfterStop::Gone);
                    self.unregister_tools(&name).await;
                    if let Ok(mut i) = self.inbox.lock() {
                        i.forget(&name);
                    }
                    self.board.drop_plugin(&name);
                    if let Some(e) = self.plugins.get(&name)
                        && e.run.is_none()
                    {
                        self.plugins.remove(&name);
                    }
                    self.changed().await;
                }
                continue;
            }
            self.evaluate(&name, &me).await;
        }
        self.changed().await;
    }
}

impl Message<ProcessUp> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: ProcessUp, _: &mut Context<Self, Self::Reply>) {
        let Some(e) = self.plugins.get(&m.name) else {
            return;
        };
        if e.epoch != m.epoch || e.run.as_ref().is_none_or(|r| r.stopping.is_some()) {
            return;
        }
        self.set_state(&m.name, PluginRunState::Running, None).await;
    }
}

impl Message<ProcessEnded> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: ProcessEnded, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        let Some(e) = self.plugins.get_mut(&m.name) else {
            return;
        };
        if e.epoch != m.epoch {
            return;
        }
        let stopping = e.run.take().and_then(|r| r.stopping);
        if let Ok(mut l) = e.line.write() {
            *l = None;
        }
        e.pace.clear();
        if let Ok(mut t) = self.targets.write() {
            t.retain(|t| t.name != m.name);
        }
        self.board.drop_plugin(&m.name);
        let name = m.name.clone();
        match stopping {
            Some(AfterStop::Disable) => self.set_state(&name, PluginRunState::Disabled, None).await,
            Some(AfterStop::Gone) => {
                self.set_state(&name, PluginRunState::Stopping, None).await;
                if !self.paths.plugins.join(&name).exists() {
                    self.plugins.remove(&name);
                    self.changed().await;
                }
            }
            Some(AfterStop::Again) => {
                if let Some(e) = self.plugins.get_mut(&name) {
                    e.backoff.reset();
                    e.state = PluginRunState::Exited; // evaluate starts from a stopped state
                }
                self.evaluate(&name, &me).await;
            }
            None => {
                let Some(e) = self.plugins.get_mut(&name) else {
                    return;
                };
                if e.card.is_some() {
                    // A widen/code-changed card is pending: wait for the press (design §5).
                    self.set_state(&name, PluginRunState::AwaitingApproval, m.error)
                        .await;
                    return;
                }
                let policy = e
                    .manifest()
                    .map_or(manifest::RestartPolicy::OnFailure, |m| m.restart);
                match e
                    .backoff
                    .after_exit(policy, m.kind, m.ran_for, Instant::now())
                {
                    Next::Exited => self.set_state(&name, PluginRunState::Exited, m.error).await,
                    Next::Retry(delay) => {
                        let epoch = e.epoch;
                        let me = me.clone();
                        let n = name.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            let _ = me.tell(Retry { name: n, epoch }).await;
                        });
                        self.set_state(&name, PluginRunState::Backoff, m.error)
                            .await;
                    }
                    Next::Failed => {
                        let tail: Vec<String> = e
                            .stderr_tail
                            .lock()
                            .map(|t| t.iter().rev().take(5).rev().cloned().collect())
                            .unwrap_or_default();
                        if let Ok(mut i) = self.inbox.lock() {
                            i.note(
                                &name,
                                crate::attention::Note {
                                    id: String::new(),
                                    title: format!("{name} keeps crashing and was stopped"),
                                    body: Some(tail.join("\n")).filter(|t| !t.is_empty()),
                                    level: crate::attention::NoteLevel::Error,
                                    open: None,
                                    at_ms: now_ms(),
                                },
                            );
                        }
                        self.set_state(&name, PluginRunState::Failed, m.error).await;
                    }
                }
            }
        }
    }
}

impl Message<Retry> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: Retry, ctx: &mut Context<Self, Self::Reply>) {
        let Some(e) = self.plugins.get(&m.name) else {
            return;
        };
        if e.epoch != m.epoch || e.state != PluginRunState::Backoff || e.run.is_some() {
            return;
        }
        let me = ctx.actor_ref().clone();
        self.reload(&m.name).await;
        if self.sync_card(&m.name).is_some() {
            self.set_state(&m.name, PluginRunState::AwaitingApproval, None)
                .await;
            return;
        }
        self.start(&m.name, &me).await;
    }
}

impl Message<Incoming> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: Incoming, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        let Some(e) = self.plugins.get(&m.name) else {
            return;
        };
        let Some(run) = e.run.as_ref().filter(|_| e.epoch == m.epoch) else {
            return;
        };
        let conn = run.conn.clone();
        let method = m
            .msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = m.msg.get("params").cloned().unwrap_or_else(|| json!({}));
        match m.msg.get("id").cloned() {
            Some(id) => self.on_request(&m.name, conn, id, &method, params).await,
            None => {
                self.on_notification(&m.name, conn, m.epoch, &method, params, &me)
                    .await
            }
        }
    }
}

impl Message<UiChecked> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: UiChecked, _: &mut Context<Self, Self::Reply>) {
        let Some(e) = self.plugins.get(&m.name) else {
            return;
        };
        let Some(conn) = e
            .run
            .as_ref()
            .filter(|_| e.epoch == m.epoch)
            .map(|r| r.conn.clone())
        else {
            return;
        };
        let bindings = match m.result {
            Ok(b) => b,
            Err(issue) => {
                self.refuse(&m.name, &conn, &issue.path, &issue.message);
                return;
            }
        };
        let (placement, format) = (m.ui.placement.clone(), m.ui.format);
        let mut ui = m.ui;
        ui.text = tools::secrets::redact(&ui.text).into_owned();
        match self.board.set_content(&m.name, &placement, ui, bindings) {
            Ok(Some(revision)) => {
                self.rt
                    .publish(PluginUi::PluginSurface {
                        plugin: m.name.clone(),
                        placement,
                        format,
                        revision,
                    })
                    .await;
            }
            Ok(None) => {} // byte-identical source: no new revision (design §6.3)
            Err(why) => self.refuse(&m.name, &conn, "/source", &why),
        }
    }
}

impl Message<FlushPace> for PluginsActor {
    type Reply = ();
    async fn handle(&mut self, m: FlushPace, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        let Some(e) = self.plugins.get_mut(&m.name) else {
            return;
        };
        let epoch = e.epoch;
        let Some(pace) = e.pace.get_mut(&m.placement) else {
            return;
        };
        pace.scheduled = false;
        let (status, ui) = (pace.status.take(), pace.ui.take());
        if let Some(s) = status {
            self.apply_status(&m.name, &m.placement, s).await;
        }
        if let Some(u) = ui {
            self.check_ui(&m.name, epoch, u, &me);
        }
    }
}

impl Message<ShutdownAll> for PluginsActor {
    type Reply = DelegatedReply<()>;
    async fn handle(
        &mut self,
        _: ShutdownAll,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|(_, e)| e.run.is_some())
            .map(|(n, _)| n.clone())
            .collect();
        let gone: Vec<_> = names
            .iter()
            .filter_map(|n| {
                self.plugins
                    .get(n)
                    .and_then(|e| e.run.as_ref())
                    .map(|r| r.conn.gone.clone())
            })
            .collect();
        for n in &names {
            self.stop(n, AfterStop::Gone);
        }
        let (reply, tx) = ctx.reply_sender();
        tokio::spawn(async move {
            for g in gone {
                g.cancelled().await;
            }
            if let Some(tx) = tx {
                tx.send(());
            }
        });
        reply
    }
}

// ---------------------------------------------------------------------------------------------
// §5.2 messages.

impl Message<ListPlugins> for PluginsActor {
    type Reply = Vec<PluginInfo>;
    async fn handle(&mut self, _: ListPlugins, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.list()
    }
}

impl Message<GetPlugin> for PluginsActor {
    type Reply = Result<PluginDetail, PluginsError>;
    async fn handle(&mut self, m: GetPlugin, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let info = self.info_of(&m.name)?;
        let catalog = self.catalog(None);
        let e = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        let granted = self.verified(&m.name).map(|g| g.grants.clone());
        let plugin_events: Vec<CatalogEntry> = catalog
            .into_iter()
            .filter(|c| c.owner == format!("plugin:{}", m.name))
            .collect();
        Ok(PluginDetail {
            info,
            manifest: e
                .parsed
                .as_ref()
                .map(|p| p.manifest.json.clone())
                .unwrap_or(Value::Null),
            requested: e.requested.clone().unwrap_or_default(),
            granted,
            card: e.card.clone(),
            catalog: plugin_events,
            stderr_tail: e
                .stderr_tail
                .lock()
                .map(|t| t.iter().cloned().collect())
                .unwrap_or_default(),
            warnings: e
                .issues
                .iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect(),
        })
    }
}

impl Message<CheckPath> for PluginsActor {
    type Reply = Result<CheckReport, PluginsError>;
    async fn handle(&mut self, m: CheckPath, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !m.path.join(manifest::FILE).is_file() {
            return Err(PluginsError::Invalid {
                message: format!("{} has no plugin.yaml", m.path.display()),
            });
        }
        let mut report = manifest::check_offline(&m.path);
        report
            .warnings
            .retain(|w| !w.message.starts_with("engine not running"));
        if let Ok(parsed) = manifest::load(&m.path) {
            let name = parsed.manifest.name.clone();
            let req = manifest::requested(&parsed.manifest, &self.tool_risk(&name));
            let mut errors = self.catalog_issues(&parsed.manifest);
            errors.extend(ptools::clashes(&parsed.manifest).await);
            errors.extend(self.secret_issues(&req).await);
            let describe = self.describe();
            report.lines = manifest::card_lines(
                &parsed.manifest,
                &req,
                None,
                &m.path,
                self.cfg.input_per_min,
                &|n| describe.get(n).cloned(),
            );
            report.errors.extend(errors);
            report.ok = report.errors.is_empty();
        }
        Ok(report)
    }
}

impl Message<Install> for PluginsActor {
    type Reply = Result<InstallReport, PluginsError>;
    async fn handle(&mut self, m: Install, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // The source folder may have any name (`~/Downloads/turns-main`): only the manifest's
        // `name` must be valid; the folder rules run on the installed copy.
        let src = m.path.clone();
        let text = blocking(move || std::fs::read_to_string(src.join(manifest::FILE)))
            .await?
            .map_err(|e| PluginsError::Invalid {
                message: format!("{}: {e}", m.path.join(manifest::FILE).display()),
            })?;
        let parsed = manifest::parse(&text).map_err(|(errors, _)| PluginsError::Invalid {
            message: errors
                .iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; "),
        })?;
        let name = parsed.manifest.name.clone();
        manifest::check_name(&name)?;
        let dest = self.paths.plugins.join(&name);
        if dest.exists() {
            return Err(PluginsError::Exists { name });
        }
        let tmp = self
            .paths
            .plugins
            .join(format!(".install-{name}-{}", now_ms()));
        let (src, plugins, to) = (m.path.clone(), self.paths.plugins.clone(), dest.clone());
        blocking(move || install_copy(&src, &plugins, &tmp, &to)).await??;
        let me = ctx.actor_ref().clone();
        self.evaluate(&name, &me).await;
        self.changed().await;
        Ok(InstallReport {
            name: name.clone(),
            dir: dest.display().to_string(),
            next: format!(
                "awaiting approval: press Approve in theuth or run `t plugin approve {name}`"
            ),
        })
    }
}

impl Message<New> for PluginsActor {
    type Reply = Result<ScaffoldReport, PluginsError>;
    async fn handle(&mut self, m: New, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let report = crate::scaffold::new_plugin(&self.paths.plugins, &m.name, m.lang, m.raw)?;
        let me = ctx.actor_ref().clone();
        self.evaluate(&m.name, &me).await;
        self.changed().await;
        Ok(report)
    }
}

impl Message<Approve> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, m: Approve, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        if !self.plugins.contains_key(&m.name) {
            return Err(not_found(&m.name));
        }
        // The card is recomputed from disk: a press on a card that changed meanwhile is stale.
        self.reload(&m.name).await;
        let card = self.sync_card(&m.name);
        let e = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        let Some(card) = card else {
            return Err(PluginsError::Invalid {
                message: format!("nothing to approve for {}", m.name),
            });
        };
        if card.manifest_sha256 != m.manifest_sha256 || card.code_sha256 != m.code_sha256 {
            return Err(PluginsError::Stale { name: m.name });
        }
        let req = e.requested.clone().unwrap_or_default();
        let entry = GrantEntry {
            manifest_sha256: card.manifest_sha256.clone(),
            code_sha256: card.code_sha256.clone(),
            approved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            approved_via: m.press.client().to_owned(),
            enabled: true,
            grants: req,
            mac: String::new(),
        };
        self.sign_and_store(&m.name, entry).await?;
        self.sync_card(&m.name);
        let e = self
            .plugins
            .get_mut(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        e.log
            .line("host", &format!("approved via {}", m.press.client()));
        e.backoff.reset();
        if e.run.is_some() {
            // A widening with the same run/env/code applies to the running process (`granted`);
            // anything else restarts it.
            if !self.apply_live(&m.name) {
                self.stop(&m.name, AfterStop::Again);
            }
        } else {
            e.state = PluginRunState::AwaitingApproval;
            self.start(&m.name, &me).await;
        }
        self.changed().await;
        self.info_of(&m.name)
    }
}

impl Message<RequestApproval> for PluginsActor {
    type Reply = Result<InstallCard, PluginsError>;
    async fn handle(
        &mut self,
        m: RequestApproval,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !self.plugins.contains_key(&m.name) {
            return Err(not_found(&m.name));
        }
        self.reload(&m.name).await;
        let e = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        if e.parsed.is_none() {
            let msg = e
                .issues
                .iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(PluginsError::Invalid { message: msg });
        }
        if self.needed_card(&m.name).is_none() {
            return Err(PluginsError::Invalid {
                message: format!("nothing to approve for {}", m.name),
            });
        }
        if let Some(e) = self.plugins.get_mut(&m.name) {
            if let Err(ms) = e.approve_rl.check(Instant::now()) {
                return Err(PluginsError::RateLimited { retry_after_ms: ms });
            }
            e.card_seq += 1;
        }
        let card = self
            .sync_card(&m.name)
            .ok_or_else(|| PluginsError::Invalid {
                message: "nothing to approve".into(),
            })?;
        self.changed().await;
        Ok(card)
    }
}

impl Message<SetEnabled> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, m: SetEnabled, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        if !self.plugins.contains_key(&m.name) {
            return Err(not_found(&m.name));
        }
        let Some(mut g) = self.verified(&m.name).cloned() else {
            return self.info_of(&m.name);
        };
        if g.enabled != m.on {
            g.enabled = m.on;
            self.sign_and_store(&m.name, g).await?;
        }
        if m.on {
            self.evaluate(&m.name, &me).await;
        } else if self.plugins.get(&m.name).is_some_and(|e| e.run.is_some()) {
            self.stop(&m.name, AfterStop::Disable);
            self.unregister_tools(&m.name).await;
            self.set_state(&m.name, PluginRunState::Stopping, None)
                .await;
        } else {
            self.set_state(&m.name, PluginRunState::Disabled, None)
                .await;
        }
        self.changed().await;
        self.info_of(&m.name)
    }
}

impl Message<SetHidden> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, m: SetHidden, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let e = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        if !e
            .manifest()
            .is_some_and(|mf| mf.placements.iter().any(|p| p.id == m.placement))
        {
            return Err(PluginsError::Invalid {
                message: format!("{} has no placement {}", m.name, m.placement),
            });
        }
        self.grants.set_hidden(&m.name, &m.placement, m.hidden);
        self.save_grants().await?;
        self.changed().await;
        self.info_of(&m.name)
    }
}

impl Message<SetOrder> for PluginsActor {
    type Reply = Result<Vec<PluginInfo>, PluginsError>;
    async fn handle(&mut self, m: SetOrder, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let current = self.ordered_names();
        let mut order: Vec<String> = m
            .order
            .into_iter()
            .filter(|n| self.plugins.contains_key(n))
            .collect();
        let listed: BTreeSet<String> = order.iter().cloned().collect();
        order.extend(current.into_iter().filter(|n| !listed.contains(n)));
        self.grants.order = order;
        self.save_grants().await?;
        self.changed().await;
        Ok(self.list())
    }
}

impl Message<Restart> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, m: Restart, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        let e = self
            .plugins
            .get_mut(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        e.backoff.reset();
        if e.run.is_some() {
            self.stop(&m.name, AfterStop::Again);
        } else {
            if e.state == PluginRunState::Backoff {
                e.state = PluginRunState::Exited;
            }
            if !matches!(e.state, PluginRunState::Disabled) {
                e.state = PluginRunState::Exited;
            }
            self.evaluate(&m.name, &me).await;
        }
        self.info_of(&m.name)
    }
}

impl Message<Uninstall> for PluginsActor {
    type Reply = Result<(), PluginsError>;
    async fn handle(&mut self, m: Uninstall, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.plugins.contains_key(&m.name) {
            return Err(not_found(&m.name));
        }
        self.stop(&m.name, AfterStop::Gone);
        self.unregister_tools(&m.name).await;
        let trash = self.paths.plugins.join(".trash").join(format!(
            "{}-{}",
            m.name,
            chrono::Utc::now().format("%Y%m%dT%H%M%S")
        ));
        let dir = self.paths.plugins.join(&m.name);
        let data = self.paths.data.join(&m.name);
        let keep_data = m.keep_data;
        blocking(move || -> Result<(), String> {
            std::fs::create_dir_all(&trash).map_err(|e| format!("trash: {e}"))?;
            std::fs::rename(&dir, trash.join("plugin"))
                .map_err(|e| format!("move {}: {e}", dir.display()))?;
            if !keep_data && data.exists() {
                std::fs::rename(&data, trash.join("data"))
                    .map_err(|e| format!("move {}: {e}", data.display()))?;
            }
            Ok(())
        })
        .await?
        .map_err(|message| PluginsError::Io { message })?;
        if let Err(e) = self.grants.revoke(&m.name).await {
            tracing::warn!(plugin = %m.name, error = %e, "plugins: grant not revoked in the vault");
            self.grants.entries.remove(&m.name);
        }
        self.grants.order.retain(|n| n != &m.name);
        let prefix = format!("{}/", m.name);
        self.grants.hidden.retain(|h| !h.starts_with(&prefix));
        self.save_grants().await?;
        if let Ok(mut i) = self.inbox.lock() {
            i.forget(&m.name);
        }
        self.board.drop_plugin(&m.name);
        if self.plugins.get(&m.name).is_some_and(|e| e.run.is_none()) {
            self.plugins.remove(&m.name);
        }
        self.changed().await;
        Ok(())
    }
}

impl Message<Action> for PluginsActor {
    type Reply = DelegatedReply<Result<Value, PluginsError>>;
    async fn handle(&mut self, m: Action, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let checked = self.placement(&m.name, &m.placement).and_then(|p| {
            if p.format == Some(SurfaceFormat::Openui) && m.activation.is_none() {
                return Err(PluginsError::NotAPress);
            }
            self.conn_of(&m.name)
        });
        let (reply, tx) = ctx.reply_sender();
        let params = json!({
            "placement": m.placement, "action": m.action, "args": m.args, "session": m.session,
            "client": m.client, "activation": m.activation, "form": m.form,
        });
        tokio::spawn(async move {
            let r = match checked {
                Ok(conn) => conn.request("action", params).await,
                Err(e) => Err(e),
            };
            if let Some(tx) = tx {
                tx.send(r);
            }
        });
        reply
    }
}

impl Message<Query> for PluginsActor {
    type Reply = DelegatedReply<Result<Value, PluginsError>>;
    async fn handle(&mut self, m: Query, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut checked = self
            .placement(&m.name, &m.placement)
            .and_then(|_| self.conn_of(&m.name));
        if checked.is_ok()
            && let Some(e) = self.plugins.get_mut(&m.name)
            && let Err(ms) = e.query_rl.check(Instant::now())
        {
            checked = Err(PluginsError::RateLimited { retry_after_ms: ms });
        }
        let (reply, tx) = ctx.reply_sender();
        let params = json!({ "placement": m.placement, "query": m.query, "args": m.args, "session": m.session });
        tokio::spawn(async move {
            let r = match checked {
                Ok(conn) => conn.request("query", params).await,
                Err(e) => Err(e),
            };
            if let Some(tx) = tx {
                tx.send(r);
            }
        });
        reply
    }
}

impl Message<Command> for PluginsActor {
    type Reply = DelegatedReply<Result<CommandReply, PluginsError>>;
    async fn handle(&mut self, m: Command, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let found = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))
            .and_then(|e| {
                e.manifest()
                    .and_then(|mf| {
                        mf.placements.iter().find(|p| {
                            p.kind == PlacementKind::Command
                                && p.command.as_ref().is_some_and(|c| c.name == m.command)
                        })
                    })
                    .map(|p| p.id.clone())
                    .ok_or_else(|| PluginsError::Invalid {
                        message: format!("{} has no command {}", m.name, m.command),
                    })
            });
        let checked = found.and_then(|placement| {
            self.placement(&m.name, &placement)?;
            Ok((placement, self.conn_of(&m.name)?))
        });
        let (reply, tx) = ctx.reply_sender();
        tokio::spawn(async move {
            let r = match checked {
                Ok((placement, conn)) => {
                    let params = json!({ "placement": placement, "name": m.command, "args": m.args, "session": m.session, "client": m.client });
                    conn.request("command", params)
                        .await
                        .and_then(|v| command_reply(&v))
                }
                Err(e) => Err(e),
            };
            if let Some(tx) = tx {
                tx.send(r);
            }
        });
        reply
    }
}

impl Message<Catalog> for PluginsActor {
    type Reply = Vec<CatalogEntry>;
    async fn handle(&mut self, m: Catalog, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.catalog(m.topic.as_deref())
    }
}

impl Message<Surfaces> for PluginsActor {
    type Reply = Vec<SurfaceSnapshot>;
    async fn handle(&mut self, _: Surfaces, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.snapshots()
    }
}

impl Message<Surface> for PluginsActor {
    type Reply = Result<SurfaceContent, PluginsError>;
    async fn handle(&mut self, m: Surface, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let e = self
            .plugins
            .get(&m.name)
            .ok_or_else(|| not_found(&m.name))?;
        let p = e
            .manifest()
            .and_then(|mf| mf.placements.iter().find(|p| p.id == m.placement))
            .ok_or_else(|| PluginsError::Invalid {
                message: format!("{} has no placement {}", m.name, m.placement),
            })?;
        if let Some(c) = self.board.content(&m.name, &m.placement) {
            return Ok(c);
        }
        let mut out = SurfaceContent {
            plugin: m.name.clone(),
            placement: m.placement.clone(),
            revision: 0,
            state: SurfaceLoad::Loading,
            format: p.format,
            source: None,
            text: None,
            bindings: Vec::new(),
            error: None,
        };
        match self.conn_of(&m.name) {
            Ok(conn) => {
                let params =
                    json!({ "placement": m.placement, "session": m.session, "client": m.client });
                let log = e.log.clone();
                tokio::spawn(async move {
                    match conn.request("render", params).await {
                        Ok(_) | Err(PluginsError::Plugin { code: -32601, .. }) => {}
                        Err(err) => log.line("host", &format!("plugin did not render: {err}")),
                    }
                });
            }
            Err(err) => {
                out.state = SurfaceLoad::Error;
                out.error = Some(err.to_string());
            }
        }
        Ok(out)
    }
}

impl Message<Logs> for PluginsActor {
    type Reply = Result<LogTail, PluginsError>;
    async fn handle(&mut self, m: Logs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let path = match self.plugins.get(&m.name) {
            Some(e) => e.log.path_today(),
            None => {
                // Not a loaded plugin: the name is joined into paths, so it must be one.
                manifest::check_name(&m.name)?;
                if !self.paths.plugins.join(&m.name).is_dir() {
                    return Err(not_found(&m.name));
                }
                crate::logs::path_today(&self.paths.logs, &m.name)
            }
        };
        let n = usize::try_from(m.tail.min(MAX_TAIL)).unwrap_or(200);
        let file = path.clone();
        let lines = blocking(move || crate::logs::tail(&file, n)).await?;
        Ok(LogTail {
            name: m.name,
            path: path.display().to_string(),
            lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_SH: &str = "#!/bin/sh\nwhile read -r line; do :; done\n";

    fn write_plugin(plugins: &Path, name: &str, extra: &str) -> PathBuf {
        let dir = plugins.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let yaml = format!(
            "name: {name}\nversion: 0.1.0\ndescription: test plugin\nprotocol: 1\nrun: [sh, run.sh]\nwatch: [run.sh]\n{extra}"
        );
        std::fs::write(dir.join("plugin.yaml"), yaml).unwrap();
        std::fs::write(dir.join("run.sh"), RUN_SH).unwrap();
        dir
    }

    async fn booted(data: &Path) -> ActorRef<PluginsActor> {
        booted_with(data, false).await
    }

    async fn booted_with(data: &Path, dev: bool) -> ActorRef<PluginsActor> {
        let rt = runtime::Runtime::new(Default::default(), data.to_path_buf());
        let cfg = PluginsConfig {
            watch: false,
            dev,
            ..PluginsConfig::default()
        };
        let actor = PluginsActor::spawn(PluginsActor::new(rt, cfg));
        let env: BTreeMap<String, String> = std::env::vars()
            .filter(|(k, _)| !k.starts_with("THEUTH_"))
            .collect();
        actor
            .ask(Boot {
                key: Some(Zeroizing::new([7u8; 32])),
                vault: None,
                base_env: Arc::new(env),
            })
            .await
            .unwrap();
        actor
    }

    async fn wait_state(
        actor: &ActorRef<PluginsActor>,
        name: &str,
        want: PluginRunState,
    ) -> PluginInfo {
        for _ in 0..100 {
            let info = actor
                .ask(GetPlugin { name: name.into() })
                .await
                .unwrap()
                .info;
            if info.state == want {
                return info;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{name} never reached {want:?}");
    }

    async fn card_of(actor: &ActorRef<PluginsActor>, name: &str) -> InstallCard {
        actor
            .ask(GetPlugin { name: name.into() })
            .await
            .unwrap()
            .card
            .expect("a pending card")
    }

    async fn approve(
        actor: &ActorRef<PluginsActor>,
        card: &InstallCard,
    ) -> Result<PluginInfo, PluginsError> {
        let m = Approve {
            name: card.plugin.clone(),
            manifest_sha256: card.manifest_sha256.clone(),
            code_sha256: card.code_sha256.clone(),
            press: crate::press::HumanPress::for_test(),
        };
        actor.ask(m).await.map_err(|e| match e {
            kameo::error::SendError::HandlerError(e) => e,
            other => PluginsError::Io {
                message: other.to_string(),
            },
        })
    }

    #[tokio::test]
    async fn approve_stale_hashes_refused() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(&tmp.path().join("plugins"), "alpha", "");
        let actor = booted(tmp.path()).await;
        let card = card_of(&actor, "alpha").await;
        assert_eq!(card.kind, CardKind::Install);
        let mut stale = card.clone();
        stale.manifest_sha256 = "0".repeat(64);
        assert_eq!(
            approve(&actor, &stale).await.unwrap_err(),
            PluginsError::Stale {
                name: "alpha".into()
            }
        );
        // The folder changed after the card was read: the old card is stale too.
        write_plugin(&tmp.path().join("plugins"), "alpha", "description2: x\n");
        assert!(matches!(
            approve(&actor, &card).await.unwrap_err(),
            PluginsError::Stale { .. }
        ));
        assert!(!tmp.path().join("plugins/grants.yaml").exists());
    }

    #[tokio::test]
    async fn approve_writes_grant_and_starts() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(&tmp.path().join("plugins"), "alpha", "");
        let actor = booted(tmp.path()).await;
        assert!(
            !wait_state(&actor, "alpha", PluginRunState::AwaitingApproval)
                .await
                .enabled
        );
        let card = card_of(&actor, "alpha").await;
        let info = approve(&actor, &card).await.unwrap();
        assert!(info.enabled && !info.needs_approval && info.approved_at.is_some());
        let file = GrantsFile::load(&tmp.path().join("plugins/grants.yaml"));
        let entry = file.entries.get("alpha").expect("grant written");
        assert!(file.verified(&[7u8; 32], "alpha").is_some());
        assert_eq!(entry.manifest_sha256, card.manifest_sha256);
        assert_eq!(entry.approved_via, "desktop");
        assert_eq!(
            entry.code_sha256, None,
            "read-only power carries no code hash"
        );
        wait_state(&actor, "alpha", PluginRunState::Running).await;
        // A second press has nothing to approve.
        assert!(matches!(
            approve(&actor, &card).await.unwrap_err(),
            PluginsError::Invalid { .. }
        ));
        let off = actor
            .ask(SetEnabled {
                name: "alpha".into(),
                on: false,
            })
            .await
            .unwrap();
        assert!(!off.enabled);
        wait_state(&actor, "alpha", PluginRunState::Disabled).await;
    }

    #[tokio::test(start_paused = true)]
    async fn card_survives_approval_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(&tmp.path().join("plugins"), "alpha", "");
        let actor = booted(tmp.path()).await;
        let card = card_of(&actor, "alpha").await;
        let timeout = ::tools::ToolsConfig::default().approval_timeout_secs;
        tokio::time::advance(Duration::from_secs(timeout + 1)).await;
        let detail = actor
            .ask(GetPlugin {
                name: "alpha".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            detail.card.as_ref().map(|c| c.id.clone()),
            Some(card.id.clone())
        );
        assert_eq!(detail.info.state, PluginRunState::AwaitingApproval);
        let item = crate::attention::card_item(&card);
        assert_eq!(
            (item.expires_ms, item.deadline_ms, item.waiting_since_ms),
            (None, None, None)
        );
    }

    #[tokio::test]
    async fn code_change_with_secret_files_card_keeps_old_process() {
        let tmp = tempfile::tempdir().unwrap();
        let open = "{";
        let env = format!("env:\n  TOKEN: \"{open}{open}secret:github.pat}}}}\"\n");
        let dir = write_plugin(&tmp.path().join("plugins"), "alpha", &env);
        let actor = booted(tmp.path()).await;
        let card = card_of(&actor, "alpha").await;
        assert!(
            card.code_sha256.is_some(),
            "a secret holder's grant carries the code hash"
        );
        approve(&actor, &card).await.unwrap();
        wait_state(&actor, "alpha", PluginRunState::Running).await;
        let pgids = std::fs::read_to_string(tmp.path().join("plugins/.pgids")).unwrap();
        let pgid: u32 = pgids.split_whitespace().next().unwrap().parse().unwrap();

        std::fs::write(dir.join("run.sh"), format!("{RUN_SH}# edited\n")).unwrap();
        actor
            .tell(FolderChanged(BTreeSet::from(["alpha".to_owned()])))
            .await
            .unwrap();
        let detail = actor
            .ask(GetPlugin {
                name: "alpha".into(),
            })
            .await
            .unwrap();
        let changed = detail.card.expect("a code-changed card");
        assert_eq!(changed.kind, CardKind::CodeChanged);
        assert_eq!(changed.changed_paths, vec!["run.sh".to_owned()]);
        assert_eq!(
            detail.info.state,
            PluginRunState::Running,
            "the old process keeps running"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            platform::process::ProcessTree::adopt(pgid).unwrap().alive(),
            "same process still up"
        );
        // Only one card: a second scan of the same change keeps the same id.
        actor
            .tell(FolderChanged(BTreeSet::from(["alpha".to_owned()])))
            .await
            .unwrap();
        let again = actor
            .ask(GetPlugin {
                name: "alpha".into(),
            })
            .await
            .unwrap()
            .card
            .unwrap();
        assert_eq!(again.id, changed.id);
        actor.ask(ShutdownAll).await.unwrap();
    }

    #[tokio::test]
    async fn hashless_dev_grant_needs_a_card_once_dev_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        let open = "{";
        let env = format!("env:\n  TOKEN: \"{open}{open}secret:github.pat}}}}\"\n");
        write_plugin(&tmp.path().join("plugins"), "alpha", &env);
        let dev = booted_with(tmp.path(), true).await;
        let card = card_of(&dev, "alpha").await;
        assert_eq!(
            card.code_sha256, None,
            "dev mode approves without a code hash"
        );
        approve(&dev, &card).await.unwrap();
        wait_state(&dev, "alpha", PluginRunState::Running).await;
        dev.ask(ShutdownAll).await.unwrap();

        let actor = booted(tmp.path()).await;
        let card = card_of(&actor, "alpha").await;
        assert_eq!(card.kind, CardKind::CodeChanged);
        assert!(card.code_sha256.is_some());
        assert_eq!(
            wait_state(&actor, "alpha", PluginRunState::AwaitingApproval)
                .await
                .state,
            PluginRunState::AwaitingApproval
        );
    }

    #[tokio::test]
    async fn logs_refuses_names_that_are_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("plugins")).unwrap();
        std::fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
        let actor = booted(tmp.path()).await;
        for name in ["../elsewhere", "/etc", "a/../../elsewhere"] {
            let err = actor
                .ask(Logs {
                    name: name.into(),
                    tail: 10,
                })
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    kameo::error::SendError::HandlerError(PluginsError::Invalid { .. })
                ),
                "{name}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn approve_request_is_rate_limited_per_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(&tmp.path().join("plugins"), "alpha", "");
        write_plugin(&tmp.path().join("plugins"), "beta", "");
        let actor = booted(tmp.path()).await;
        let ask = |name: &str| {
            actor.ask(RequestApproval {
                name: name.to_owned(),
            })
        };
        for _ in 0..APPROVE_REQUESTS_PER_MIN {
            ask("alpha").await.unwrap();
        }
        assert!(matches!(
            ask("alpha").await.unwrap_err(),
            kameo::error::SendError::HandlerError(PluginsError::RateLimited { .. })
        ));
        ask("beta").await.unwrap();
    }

    #[test]
    fn install_copy_refuses_plugins_dir_links_and_floods() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let inside = write_plugin(&plugins, "alpha", "");
        let copy = |src: &Path, dest: &str| {
            let tmp_dir = plugins.join(format!(".install-{dest}"));
            install_copy(src, &plugins, &tmp_dir, &plugins.join(dest))
        };
        let invalid = |r: Result<(), PluginsError>| matches!(r, Err(PluginsError::Invalid { .. }));
        // The plugins folder itself, a folder inside it, and a folder holding it.
        assert!(invalid(copy(&plugins, "x1")));
        assert!(invalid(copy(&inside, "x2")));
        assert!(invalid(copy(tmp.path(), "x3")));
        assert!(!plugins.join("x1").exists() && !plugins.join(".install-x1").exists());

        let src = write_plugin(&tmp.path().join("src"), "beta", "");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", src.join("hosts")).unwrap();
            assert!(invalid(copy(&src, "beta")));
            assert!(!plugins.join(".install-beta").exists(), "temp copy removed");
            std::fs::remove_file(src.join("hosts")).unwrap();
        }
        let many = src.join("many");
        std::fs::create_dir_all(&many).unwrap();
        for i in 0..INSTALL_MAX_FILES {
            std::fs::write(many.join(i.to_string()), "").unwrap();
        }
        assert!(invalid(copy(&src, "beta")));
        std::fs::remove_dir_all(&many).unwrap();
        copy(&src, "beta").unwrap();
        assert!(plugins.join("beta/run.sh").is_file());
    }

    #[tokio::test]
    async fn install_from_a_folder_named_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let src = write_plugin(&tmp.path().join("Downloads"), "alpha-main", "");
        let yaml = std::fs::read_to_string(src.join("plugin.yaml"))
            .unwrap()
            .replace("name: alpha-main", "name: alpha");
        std::fs::write(src.join("plugin.yaml"), yaml).unwrap();
        std::fs::create_dir_all(src.join("target/debug")).unwrap();
        std::fs::write(src.join("target/debug/big"), "x").unwrap();
        let actor = booted(tmp.path()).await;
        let report = actor.ask(Install { path: src }).await.unwrap();
        assert_eq!(report.name, "alpha");
        let dest = tmp.path().join("plugins/alpha");
        assert!(dest.join("run.sh").is_file());
        assert!(!dest.join("target").exists(), "build output is not copied");
        wait_state(&actor, "alpha", PluginRunState::AwaitingApproval).await;
    }

    #[tokio::test]
    async fn hello_first_and_granted_on_a_live_narrowing() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let dir = write_plugin(&plugins, "alpha", "capabilities: [notify]\n");
        std::fs::write(
            dir.join("run.sh"),
            "#!/bin/sh\nwhile read -r line; do printf '%s\\n' \"$line\" >> \"$THEUTH_PLUGIN_DATA/stdin.log\"; done\n",
        )
        .unwrap();
        let actor = booted(tmp.path()).await;
        approve(&actor, &card_of(&actor, "alpha").await)
            .await
            .unwrap();
        wait_state(&actor, "alpha", PluginRunState::Running).await;
        let log = tmp.path().join("plugin-data/alpha/stdin.log");
        let read = || std::fs::read_to_string(&log).unwrap_or_default();
        for _ in 0..100 {
            if !read().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let first: Value = serde_json::from_str(read().lines().next().unwrap()).unwrap();
        assert_eq!(first["method"], "hello");
        assert!(first["params"]["engine"].is_string());
        assert_eq!(first["params"]["grants"]["capabilities"]["notify"], true);
        let pgids = || std::fs::read_to_string(plugins.join(".pgids")).unwrap();
        let before = pgids();

        // Narrow: drop `notify`; run, env and code stay → applied live, no restart.
        let yaml = std::fs::read_to_string(dir.join("plugin.yaml"))
            .unwrap()
            .replace("capabilities: [notify]\n", "");
        std::fs::write(dir.join("plugin.yaml"), yaml).unwrap();
        actor
            .tell(FolderChanged(BTreeSet::from(["alpha".to_owned()])))
            .await
            .unwrap();
        let mut granted = None;
        for _ in 0..100 {
            granted = read()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find(|v| v["method"] == "granted");
            if granted.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let granted = granted.expect("a granted notification");
        assert_eq!(granted["params"]["grants"]["capabilities"]["notify"], false);
        assert_eq!(pgids(), before, "same process");
        assert_eq!(
            wait_state(&actor, "alpha", PluginRunState::Running)
                .await
                .state,
            PluginRunState::Running
        );
        actor.ask(ShutdownAll).await.unwrap();
    }
}
