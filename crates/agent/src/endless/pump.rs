//! 137 §5.2: the per-session pump — derives units from committed rows,
//! builds nodes in rule-3 order with ≤ `jobs` compactor calls in flight,
//! retries failures, keeps the fold fitted and reports `EndlessProgress`.
//! Also the home of the §9 tools and 8.6 `set_mode`, which read the same
//! live state.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::FutureExt as _;
use parking_lot::{Mutex, RwLock};
use runtime::SessionId;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use super::compactor::{self, Compactor, ModelCompactor, Step};
use super::fold::ViewBlock;
use super::fold::{self, Fold, FoldCfg, Part, ViewRender};
use super::tree::{self, Node, Tree, Unit, exists, id_n};
use super::{
    Attached, Backfill, Backlog, EndlessProgress, Inner, MemoryMode, SessionMemory, SetMode,
    Stalled, UnitFilter, ViewLine, units,
};
use crate::backend::Backend;
use crate::cost::{CallKind, CallStatus};
use crate::store::Store;
use crate::store::endless::UnitRow;

/// Frozen `<chat>` contexts kept per session (D6; older ones fall back to the live view).
const FROZEN_KEEP: usize = 8;
/// Progress events: at most one per this interval per session.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// Seconds per level-0 unit before any measurement (ETA, D14 / R1-D4).
const DEFAULT_UNIT_SECS: f64 = 5.0;
/// USD per unit before any measurement (R1-D4).
const DEFAULT_UNIT_USD: f64 = 0.02;
/// Nodes that calibrate the estimate (R1-D4).
const CALIBRATE_NODES: u32 = 20;
/// Longest wait between retries of a failing node.
const RETRY_CAP: Duration = Duration::from_secs(1800);

/// Retry delay after `failures` consecutive failures (`failures ≥ 1`):
/// `base_secs × 2^(failures−1)`, capped at [`RETRY_CAP`].
fn retry_delay(base_secs: u32, failures: u32) -> Duration {
    let factor = 2u64
        .checked_pow(failures.saturating_sub(1))
        .unwrap_or(u64::MAX);
    let secs = u64::from(base_secs).saturating_mul(factor);
    Duration::from_secs(secs).min(RETRY_CAP)
}

/// A failed node: when it may retry, how many times in a row it failed, and why.
struct Retry {
    at: Instant,
    failures: u32,
    /// Last error, one line, ≤ 300 chars.
    error: String,
    /// The compactor model id that failed.
    model: String,
}

/// One attached session's live state.
pub struct Handle {
    pub(crate) attached: Attached,
    pub(crate) state: Arc<State>,
    /// Cancels the pump task; replaced when the pump restarts.
    cancel: CancellationToken,
}

impl Handle {
    /// D14: the loop runs the endless frame only once the tree covers the history.
    pub(crate) fn active(&self) -> bool {
        let live = self.state.live.lock();
        live.mode == MemoryMode::Endless
            && live.filter != UnitFilter::Meeting
            && live.backfill().is_none()
    }

    /// §9: the tree answers `zoom` / `recall` / `date` — attached ∧ `endless`,
    /// whatever the filter or the backfill state (a meeting session and a
    /// backfilling chat have a tree to search even though `active()` is false).
    pub(crate) fn served(&self) -> bool {
        self.state.live.lock().mode == MemoryMode::Endless
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 137F E9: one [`Gate`] per local endpoint, engine-wide, sized by
/// `local_jobs` (a changed size gets a fresh gate; calls on the old one finish there).
#[derive(Clone, Default)]
pub(crate) struct Queues(Arc<Mutex<HashMap<String, Queue>>>);

/// `(permits, gate)` of one endpoint.
type Queue = (u8, Arc<Gate>);

impl Queues {
    fn get(&self, endpoint: &str, permits: u8) -> Arc<Gate> {
        let permits = permits.clamp(1, 16);
        let mut map = self.0.lock();
        let slot = map
            .entry(endpoint.to_owned())
            .or_insert_with(|| (permits, Gate::new(usize::from(permits))));
        if slot.0 != permits {
            *slot = (permits, Gate::new(usize::from(permits)));
        }
        slot.1.clone()
    }
}

/// 137F R1-8: a local endpoint's permits, handed out by priority (a live
/// session's frontier before merges and backfill), then by arrival. R2-2:
/// every [`AGE_EVERY`]th grant goes to the oldest waiter whatever its
/// priority, so merges and backfill cannot starve under steady frontier load.
pub(crate) struct Gate {
    state: Mutex<GateState>,
    freed: Notify,
}

/// One grant in this many serves the oldest waiter (R2-2).
const AGE_EVERY: u64 = 4;

struct GateState {
    free: usize,
    next: u64,
    /// Permits handed out so far (the aging turn).
    grants: u64,
    /// `(priority, arrival)`; ordered highest priority first.
    waiting: BTreeSet<(Reverse<u8>, u64)>,
}

impl GateState {
    /// The waiter the next permit goes to.
    fn head(&self) -> Option<(Reverse<u8>, u64)> {
        if self.grants % AGE_EVERY == AGE_EVERY - 1 {
            self.waiting.iter().min_by_key(|k| k.1).copied()
        } else {
            self.waiting.first().copied()
        }
    }
}

/// One held permit; dropping it frees the slot.
pub(crate) struct Permit(Arc<Gate>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.state.lock().free += 1;
        self.0.freed.notify_waiters();
    }
}

/// A waiter's place in line; a dropped wait (cancel) gives it up.
struct Ticket<'a> {
    gate: &'a Gate,
    key: (Reverse<u8>, u64),
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.gate.state.lock().waiting.remove(&self.key) {
            self.gate.freed.notify_waiters();
        }
    }
}

impl Gate {
    fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState {
                free: permits,
                next: 0,
                grants: 0,
                waiting: BTreeSet::new(),
            }),
            freed: Notify::new(),
        })
    }

    /// Wait for a permit; higher `priority` is served first, the oldest
    /// waiter every [`AGE_EVERY`]th grant. Cancel-safe.
    pub(crate) async fn acquire(self: &Arc<Self>, priority: u8) -> Permit {
        let key = {
            let mut s = self.state.lock();
            let key = (Reverse(priority), s.next);
            s.next += 1;
            s.waiting.insert(key);
            key
        };
        let _ticket = Ticket { gate: self, key };
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut s = self.state.lock();
                if s.free > 0 && s.head() == Some(key) {
                    s.waiting.remove(&key);
                    s.free -= 1;
                    s.grants += 1;
                    if s.free > 0 && !s.waiting.is_empty() {
                        self.freed.notify_waiters();
                    }
                    return Permit(self.clone());
                }
            }
            freed.await;
        }
    }
}

/// Test seam: the scripted compactor for a resolved model id.
pub(crate) type CompactorFactory = Arc<dyn Fn(&str) -> Arc<dyn Compactor> + Send + Sync>;

/// The compactor in force and the token that cancels the calls made with it.
struct Current {
    /// Resolved model id + effort: equal keys mean the same compactor.
    key: String,
    compactor: Arc<dyn Compactor>,
    cancel: CancellationToken,
}

/// Shared between the handle, the pump task and its jobs.
pub(crate) struct State {
    session: SessionId,
    store: Store,
    rt: runtime::Runtime,
    /// Shared with [`Inner`]: Settings › Memory replaces it live.
    cfg: Arc<RwLock<super::EndlessConfig>>,
    /// 137F E1: re-resolved whenever a link of the R1-D3 chain changes.
    current: RwLock<Current>,
    /// Shared with [`Inner`] (E9).
    queues: Queues,
    live: Mutex<Live>,
    /// Pump loop wake-up (new rows, a finished job, a mode switch).
    wake: Notify,
    /// Bumped whenever a job finishes (a node lands or fails); `settle` waits on it.
    built_tx: watch::Sender<u64>,
    /// Serialises row ingestion between the pump and the turn (`settle`, `render_for_turn`).
    ingest: tokio::sync::Mutex<()>,
}

impl State {
    fn new(
        session: SessionId,
        store: Store,
        inner: &Inner,
        (key, compactor): (String, Arc<dyn Compactor>),
        meta: crate::store::endless::EndlessMeta,
    ) -> Self {
        Self {
            session,
            store,
            rt: inner.rt.clone(),
            cfg: inner.cfg.clone(),
            current: RwLock::new(Current {
                key,
                compactor,
                cancel: CancellationToken::new(),
            }),
            queues: inner.queues.clone(),
            live: Mutex::new(Live::new(meta)),
            wake: Notify::new(),
            built_tx: watch::channel(0).0,
            ingest: tokio::sync::Mutex::new(()),
        }
    }

    fn compactor(&self) -> Arc<dyn Compactor> {
        self.current.read().compactor.clone()
    }

    /// E2/E3: install `compactor` when its key differs — cancel the calls
    /// made with the old one, reset every backoff, wake the pump. `true` = changed.
    fn switch(&self, (key, compactor): (String, Arc<dyn Compactor>)) -> bool {
        {
            let mut cur = self.current.write();
            if cur.key == key {
                return false;
            }
            tracing::info!(session = %self.session, from = %cur.key, to = %key, "endless: compactor switched");
            cur.cancel.cancel();
            *cur = Current {
                key,
                compactor,
                cancel: CancellationToken::new(),
            };
        }
        self.reset_backoff();
        true
    }

    /// Every failed node may run now (E2, E5); running jobs keep their keys.
    fn reset_backoff(&self) {
        {
            let mut live = self.live.lock();
            live.failed.clear();
            live.reported.clear();
            live.dirty = true;
        }
        self.wake.notify_one();
    }

    /// R1-9 / R1-10: cancel the calls in flight (not failures; their nodes
    /// reschedule) and keep the compactor.
    fn cancel_calls(&self) {
        let mut cur = self.current.write();
        cur.cancel.cancel();
        cur.cancel = CancellationToken::new();
    }
}

struct Live {
    mode: MemoryMode,
    /// R1-D6: which parts become tree units (fixed at switch-on).
    filter: UnitFilter,
    /// R1-D4: first unit the tree covers; units below are raw (recall only).
    base: u64,
    /// R1-D2: per-session view budget override.
    view_bytes: Option<u32>,
    /// 137G `endless_pause`: no compactor calls while set (persisted).
    paused: bool,
    /// Every unit, dense by `i`.
    // ponytail: whole-session unit text in memory; page from the store if a session outgrows RAM.
    units: Vec<Unit>,
    tree: Tree,
    /// `Fold::rebuild(tree, T)` cache; `None` after the tree or `T` changed.
    fold: Option<Fold>,
    running: HashSet<(u8, u64)>,
    /// Failed nodes and when they may retry.
    failed: HashMap<(u8, u64), Retry>,
    /// Nodes whose first failure was reported.
    reported: HashSet<(u8, u64)>,
    /// `(turn, <chat>…</chat>)`, newest last.
    frozen: VecDeque<(u32, String)>,
    /// Rendered turns so far (units tag the turn they committed in).
    turn: u32,
    /// Last `messages.seq` ingested; `-1` = none.
    last_seq: i64,
    /// Units that were history when the mode was switched on (`turn == 0`): the backfill.
    backfill_total: u64,
    /// Measured level-0 build time (sum secs, count) for the ETA.
    build_secs: (f64, u32),
    /// Measured USD per unit from the ledger (R1-D4), refreshed by `backfill_estimate`.
    usd_per_unit: Option<f64>,
    /// Progress event throttle.
    last_progress: Option<Instant>,
    dirty: bool,
    pending_error: Option<String>,
    /// `unit.i → turn` (a unit is pure §4 data; the turn is pump bookkeeping).
    turns: HashMap<u64, u32>,
}

impl Live {
    fn t(&self) -> u64 {
        self.units.len() as u64
    }

    /// Fill the fold cache; read it as `self.fold` (a field borrow, so
    /// `self.tree` / `self.units` stay usable beside it).
    fn ensure_fold(&mut self, cfg: FoldCfg) {
        if self.fold.as_ref().is_none_or(|f| f.cfg != cfg) {
            self.fold = Some(Fold::rebuild(
                &self.tree,
                self.base.min(self.t()),
                self.t(),
                cfg,
                &self.units,
            ));
        }
    }

    /// The session's fold parameters (R1-D2 override, else the config).
    fn fold_cfg(&self, cfg: &super::EndlessConfig) -> FoldCfg {
        cfg.fold_cfg_for(self.view_bytes.unwrap_or(cfg.view_bytes))
    }

    fn fold(&mut self, cfg: FoldCfg) -> Fold {
        self.ensure_fold(cfg);
        self.fold.clone().expect("just built")
    }

    fn invalidate(&mut self) {
        self.fold = None;
    }

    /// D14 progress; `None` once every history unit has its line.
    fn backfill(&self) -> Option<Backfill> {
        let total = self.backfill_total;
        if total <= self.base {
            return None;
        }
        let done = (self.base..total)
            .filter(|i| self.tree.built(0, *i))
            .count() as u64;
        let span = total - self.base;
        if done >= span {
            return None;
        }
        Some(Backfill {
            done,
            total: span,
            eta_secs: ((span - done) as f64 * self.secs_per_unit()) as u64,
            usd_estimate: (span - done) as f64 * self.usd_per_unit.unwrap_or(DEFAULT_UNIT_USD),
        })
    }

    /// Measured seconds per model-built level-0 node, else the default.
    fn secs_per_unit(&self) -> f64 {
        if self.build_secs.1 > 0 {
            self.build_secs.0 / f64::from(self.build_secs.1)
        } else {
            DEFAULT_UNIT_SECS
        }
    }

    /// Level-0 units in `[base, T)` without a line.
    fn unbuilt_level0(&self) -> u64 {
        (self.base.min(self.t())..self.t())
            .filter(|i| !self.tree.built(0, *i))
            .count() as u64
    }

    /// Nodes that exist over `[0, T)` (every level), and how many are built.
    fn totals(&self) -> (u64, u64) {
        let t = self.t();
        let mut total = 0;
        let mut l = 0u8;
        while (1u64 << l) <= t.max(1) {
            total += t >> l;
            l += 1;
        }
        (self.tree.len() as u64, total)
    }

    /// Level-0 lines (text only) of units in `turn` with `i < before`, oldest first.
    fn earlier(&self, turn: u32, before: u64) -> Vec<String> {
        self.units
            .iter()
            .filter(|u| u.i < before && self.unit_turn(u.i) == turn)
            .filter_map(|u| self.tree.get(0, u.i).map(|n| n.text.clone()))
            .collect()
    }

    fn unit_turn(&self, i: u64) -> u32 {
        self.turns.get(&i).copied().unwrap_or(0)
    }

    /// D14: one past the last history unit (`turn == 0`) without a line; 0 = none.
    fn history_unbuilt(&self) -> u64 {
        self.units
            .iter()
            .filter(|u| u.i >= self.base && self.unit_turn(u.i) == 0 && !self.tree.built(0, u.i))
            .map(|u| u.i + 1)
            .max()
            .unwrap_or(0)
    }

    /// Rule 3 frontier: the start of the first unbuilt view part (T when all built).
    fn frontier(&mut self, cfg: FoldCfg) -> u64 {
        self.ensure_fold(cfg);
        let t = self.t();
        self.fold
            .as_ref()
            .expect("ensured")
            .parts
            .iter()
            .find(|p| !self.tree.built(p.l, p.i))
            .map_or(t, |p| p.start())
    }

    /// 137F: the failing node that blocks most — the level-0 frontier, else
    /// the one with the most consecutive failures.
    fn stalled(&mut self, cfg: FoldCfg) -> Option<Stalled> {
        if self.failed.is_empty() {
            return None;
        }
        let front = (0u8, self.frontier(cfg));
        let (key, r) = self.failed.get_key_value(&front).or_else(|| {
            self.failed
                .iter()
                .max_by(|a, b| a.1.failures.cmp(&b.1.failures).then(b.0.cmp(a.0)))
        })?;
        let next_retry_at = (!self.running.contains(key)).then(|| {
            let left = r.at.saturating_duration_since(Instant::now());
            (Utc::now() + chrono::Duration::from_std(left).unwrap_or_default())
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        Some(Stalled {
            failures: r.failures,
            error: r.error.clone(),
            next_retry_at,
            model: r.model.clone(),
            blocking: *key == front,
        })
    }

    /// E4: the frontier failed at least twice in a row and is in backoff —
    /// waiting out its delay or running its retry (a turn does not wait for
    /// a retry of a node that keeps failing; a success clears the state).
    fn frontier_stalled(&mut self, cfg: FoldCfg) -> bool {
        if self.failed.is_empty() {
            return false;
        }
        let front = (0u8, self.frontier(cfg));
        self.failed.get(&front).is_some_and(|r| r.failures >= 2)
    }

    /// 137G: the unsettled level-0 backlog; `None` when empty and not paused.
    fn backlog(&self) -> Option<Backlog> {
        let lines = u32::try_from(self.unbuilt_level0()).unwrap_or(u32::MAX);
        if lines == 0 && !self.paused {
            return None;
        }
        Some(Backlog {
            lines,
            folded: 0,
            eta_secs: if self.paused {
                0
            } else {
                (f64::from(lines) * self.secs_per_unit()) as u64
            },
            paused: self.paused,
        })
    }
}

impl Live {
    fn new(meta: crate::store::endless::EndlessMeta) -> Self {
        Self {
            mode: meta.mode,
            filter: meta.filter,
            base: meta.base,
            view_bytes: meta.view_bytes,
            paused: meta.paused,
            units: Vec::new(),
            tree: Tree::default(),
            fold: None,
            running: HashSet::new(),
            failed: HashMap::new(),
            reported: HashSet::new(),
            frozen: VecDeque::new(),
            turn: 0,
            last_seq: -1,
            backfill_total: 0,
            build_secs: (0.0, 0),
            usd_per_unit: None,
            last_progress: None,
            dirty: false,
            pending_error: None,
            turns: HashMap::new(),
        }
    }
}

/// The compactor for a session (R1-D3): `agent.endless.model` → else
/// `agent.title_model` → else the session's own model with its effort —
/// every link read live (137F E1). Keyed by model id + effort.
fn compactor_for(
    inner: &Inner,
    session: SessionId,
    attached: &Attached,
) -> (String, Arc<dyn Compactor>) {
    let chosen = inner
        .cfg
        .read()
        .model
        .clone()
        .or_else(|| inner.title_model.read().clone());
    if let Some(f) = &inner.compactor_for_tests {
        let id = chosen.unwrap_or_else(|| attached.model.id.clone());
        let c = f(&id);
        return (id, c);
    }
    let (model, effort) = match chosen {
        Some(id) => match inner
            .catalog
            .read()
            .resolve(&id, &*crate::supervisor::configured(inner.auth.clone()))
        {
            Ok(m) => {
                let effort = crate::frame::default_effort(&m);
                (m, effort)
            }
            Err(e) => {
                tracing::warn!(model = %id, error = %e, "agent.endless.model: falling back to the session model");
                (attached.model.clone(), attached.effort.clone())
            }
        },
        None => (attached.model.clone(), attached.effort.clone()),
    };
    let base =
        crate::backend::target_for(&model, &inner.auth).map(|t| t.endpoint.base_url().to_owned());
    let local = compactor::local_endpoint(&model, base.as_deref());
    // E8: a local Ollama model must not spend the reply cap on thinking
    // (`think: false`) — genai's own Ollama adapter only, not a gateway (R1-14).
    let effort = if local.is_some() && base.is_none() && crate::loop_::ns(&model.genai) == "ollama"
    {
        Some(genai::chat::ReasoningEffort::Zero)
    } else {
        effort
    };
    let key = format!("{}|{effort:?}", model.id);
    let c = ModelCompactor {
        session,
        backend: Backend::for_model(&model, &inner.client, &inner.auth),
        model,
        system: compactor::prompt(&attached.agent),
        effort,
        local,
        cfg: inner.cfg.clone(),
    };
    (key, Arc::new(c))
}

/// E1/E2: re-resolve the compactor of `session` (`None` = every attached
/// session); a changed one cancels its old calls and resets backoff. Every
/// target's pump wakes either way (a changed `jobs` applies now, R1-7).
pub(crate) fn reresolve(inner: &Inner, session: Option<SessionId>) {
    let targets: Vec<(SessionId, Arc<State>, Attached)> = inner
        .sessions
        .lock()
        .iter()
        .filter(|(id, _)| session.is_none_or(|s| s == **id))
        .map(|(id, h)| (*id, h.state.clone(), h.attached.clone()))
        .collect();
    for (id, state, attached) in targets {
        if !state.switch(compactor_for(inner, id, &attached)) {
            state.wake.notify_one();
        }
    }
}

pub(crate) async fn resume_all(inner: &Inner) -> anyhow::Result<usize> {
    if let Err(e) = inner.store.endless_fts_sync().await {
        tracing::warn!(error = %e, "endless: fts sync");
    }
    let mut n = 0;
    for row in inner.store.endless_sessions().await? {
        let Ok(session) = row.id.parse::<SessionId>() else {
            continue;
        };
        let model = match inner.catalog.read().resolve(
            &row.model,
            &*crate::supervisor::configured(inner.auth.clone()),
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(%session, error = %e, "endless: cannot resume (model)");
                continue;
            }
        };
        let attached = Attached {
            model,
            effort: row.effort.as_deref().and_then(crate::effort_of),
            agent: row.desk.unwrap_or_else(|| "theuth".to_owned()),
            store: None,
        };
        match attach(inner, session, attached).await {
            Ok(()) => n += 1,
            Err(e) => tracing::warn!(%session, error = %e, "endless: cannot resume"),
        }
    }
    Ok(n)
}

pub(crate) async fn attach(
    inner: &Inner,
    session: SessionId,
    attached: Attached,
) -> anyhow::Result<()> {
    let store = attached
        .store
        .clone()
        .unwrap_or_else(|| inner.store.clone());
    let meta = store.endless_meta(session).await?;
    let mode = meta.mode;
    let existing = inner.sessions.lock().get(&session).map(|h| h.state.clone());
    let state = match existing {
        Some(state) => {
            let mut live = state.live.lock();
            live.mode = meta.mode;
            live.filter = meta.filter;
            live.base = meta.base;
            live.view_bytes = meta.view_bytes;
            live.paused = meta.paused;
            live.invalidate();
            drop(live);
            state
        }
        None => {
            let current = compactor_for(inner, session, &attached);
            let state = Arc::new(State::new(session, store, inner, current, meta));
            load(&state).await?;
            state
        }
    };
    let cancel = CancellationToken::new();
    let handle = Handle {
        attached: attached.clone(),
        state: state.clone(),
        cancel: cancel.clone(),
    };
    inner.sessions.lock().insert(session, handle);
    // R1-2: a link of the chain that changed while this attach ran (or the
    // session's model since the last one) — idempotent when nothing moved.
    state.switch(compactor_for(inner, session, &attached));
    if mode == MemoryMode::Endless {
        start_pump(state, cancel);
    }
    Ok(())
}

/// The actor stopped: stop the pump and cancel its calls in flight, so a
/// local permit is not held for a whole attempt (R1-9).
pub(crate) fn detach(inner: &Inner, session: SessionId) {
    if let Some(h) = inner.sessions.lock().remove(&session) {
        h.state.cancel_calls();
    }
}

/// Load units (cache + rows) and nodes from the store into a fresh state.
async fn load(state: &State) -> anyhow::Result<()> {
    let rows = state.store.endless_units(state.session).await?;
    let messages = state.store.endless_messages_from(state.session, 0).await?;
    let by_seq: HashMap<i64, &crate::store::endless::EndlessMessage> =
        messages.iter().map(|m| (m.seq, m)).collect();
    let mut units = Vec::with_capacity(rows.len());
    let mut turns = HashMap::new();
    let mut last_seq = -1;
    let mut max_turn = 0;
    for r in &rows {
        let text = by_seq.get(&r.seq).and_then(|m| {
            units::derive(m.seq, m.at, &m.message, state.cfg.read().cap_chars, 0)
                .into_iter()
                .find(|u| u.part == r.part)
        });
        let at = by_seq.get(&r.seq).and_then(|m| m.at);
        units.push(Unit {
            i: r.i,
            seq: r.seq,
            part: r.part,
            kind: r.kind,
            text: text.map(|u| u.text).unwrap_or_default(),
            size: r.size,
            at,
        });
        turns.insert(r.i, r.turn);
        last_seq = last_seq.max(r.seq);
        max_turn = max_turn.max(r.turn);
    }
    let nodes = state.store.endless_nodes(state.session).await?;
    {
        let mut live = state.live.lock();
        live.units = units;
        live.turns = turns;
        live.last_seq = last_seq;
        live.turn = max_turn;
        live.tree = Tree::from_nodes(nodes);
        live.backfill_total = live.history_unbuilt();
        live.invalidate();
    }
    // Rows that landed after the cache (or a cache that never existed).
    ingest(state).await
}

/// Derive units from `messages` rows beyond `last_seq` and cache them.
async fn ingest(state: &State) -> anyhow::Result<()> {
    let _guard = state.ingest.lock().await;
    let from = state.live.lock().last_seq + 1;
    let rows = state
        .store
        .endless_messages_from(state.session, from)
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut new_units = Vec::new();
    let (mut next_i, turn, mut last_seq, filter) = {
        let live = state.live.lock();
        (live.t(), live.turn, live.last_seq, live.filter)
    };
    for m in &rows {
        last_seq = last_seq.max(m.seq);
        if m.excluded {
            continue;
        }
        let cap_chars = state.cfg.read().cap_chars;
        let mut units = units::derive(m.seq, m.at, &m.message, cap_chars, next_i);
        units.retain(|u| filter.keeps(u));
        for (k, u) in units.iter_mut().enumerate() {
            u.i = next_i + k as u64;
        }
        next_i += units.len() as u64;
        new_units.extend(units);
    }
    let unit_rows: Vec<UnitRow> = new_units
        .iter()
        .map(|u| UnitRow {
            i: u.i,
            seq: u.seq,
            part: u.part,
            kind: u.kind,
            size: u.size,
            turn,
        })
        .collect();
    state
        .store
        .endless_units_put(state.session, &unit_rows)
        .await?;
    let mut live = state.live.lock();
    for u in new_units {
        live.turns.insert(u.i, turn);
        live.units.push(u);
    }
    live.last_seq = last_seq;
    live.invalidate();
    live.dirty = true;
    Ok(())
}

fn start_pump(state: Arc<State>, cancel: CancellationToken) {
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            if let Err(e) = ingest(&state).await {
                tracing::warn!(session = %state.session, error = %e, "endless: ingest");
            }
            schedule(&state);
            let sleep = emit_progress(&state);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = state.wake.notified() => {}
                _ = tokio::time::sleep(sleep) => {}
            }
        }
    });
}

/// Start every eligible node up to `jobs` (§5.2 rules 1–3).
fn schedule(state: &Arc<State>) {
    let mut live = state.live.lock();
    if live.mode != MemoryMode::Endless || live.paused {
        return;
    }
    let cfg = live.fold_cfg(&state.cfg.read());
    let t = live.t();
    let base = live.base;
    let now = Instant::now();
    let frontier = live.frontier(cfg);
    let free = |live: &Live, key: &(u8, u64)| {
        !live.running.contains(key) && live.failed.get(key).is_none_or(|r| r.at <= now)
    };
    let mut candidates: Vec<(u8, u64)> = Vec::new();
    if frontier < t && !live.tree.built(0, frontier) && free(&live, &(0, frontier)) {
        candidates.push((0, frontier));
    }
    let mut l = 1u8;
    while (1u64 << l) <= t {
        let n = 1u64 << l;
        for i in 0..(t / n) {
            let key = (l, i);
            if i * n < base || live.tree.built(l, i) || !exists(l, i, t) || (i + 1) * n > frontier {
                continue;
            }
            if !live.tree.built(l - 1, 2 * i) || !live.tree.built(l - 1, 2 * i + 1) {
                continue;
            }
            if free(&live, &key) {
                candidates.push(key);
            }
        }
        l += 1;
    }
    let picked: Vec<(u8, u64)> = if state.compactor().local().is_some() {
        // E9 / R2-1: a local compactor is one serial server, so the gate
        // serialises the calls and orders them by priority. Per session, one
        // level-0 job and one merge may wait there at once, so a queued merge
        // never keeps the next frontier job from being spawned.
        let level0 = live.running.iter().any(|k| k.0 == 0);
        let merging = live.running.iter().any(|k| k.0 > 0);
        let frontier = candidates.iter().find(|k| k.0 == 0 && !level0);
        let merge = candidates.iter().find(|k| k.0 > 0 && !merging);
        frontier.into_iter().chain(merge).copied().collect()
    } else {
        let slots = usize::from(state.cfg.read().jobs.max(1)).saturating_sub(live.running.len());
        candidates.into_iter().take(slots).collect()
    };
    for key in picked {
        live.running.insert(key);
        spawn_build(state.clone(), key, &mut live);
    }
}

/// E10: a job task that ends without reaching [`finish`] releases its
/// `running` key and counts as a failure, so the node retries — unless its
/// calls were cancelled (a switch, a retry, a detach: R1-3).
struct JobGuard {
    state: Arc<State>,
    key: (u8, u64),
    model: String,
    cancel: CancellationToken,
    armed: bool,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        {
            let mut live = self.state.live.lock();
            live.running.remove(&self.key);
            if !self.cancel.is_cancelled() {
                record_failure(
                    &self.state,
                    &mut live,
                    self.key,
                    "compactor task ended unexpectedly",
                    &self.model,
                );
            }
            live.dirty = true;
        }
        self.state.built_tx.send_modify(|v| *v += 1);
        self.state.wake.notify_one();
    }
}

/// Assemble the step for `(l, i)` under the lock and run it on its own task.
fn spawn_build(state: Arc<State>, key: (u8, u64), live: &mut Live) {
    let (l, i) = key;
    let node_bytes = state.cfg.read().node_bytes;
    let (id, n) = id_n(l, i);
    let last_turn = live.unit_turn(id + n - 1);
    let (free_text, step) = if l == 0 {
        let u = &live.units[i as usize];
        match tree::free_unit(u, node_bytes) {
            Some(text) => (Some(text), None),
            None => (
                None,
                Some(Step::Unit {
                    kind: u.kind,
                    text: u.text.clone(),
                    earlier: live.earlier(last_turn, i),
                }),
            ),
        }
    } else {
        let a = live
            .tree
            .get(l - 1, 2 * i)
            .map(|n| n.text.clone())
            .unwrap_or_default();
        let b = live
            .tree
            .get(l - 1, 2 * i + 1)
            .map(|n| n.text.clone())
            .unwrap_or_default();
        match tree::free_merge(&a, &b, node_bytes) {
            Some(text) => (Some(text), None),
            None => (
                None,
                Some(Step::Merge {
                    a,
                    b,
                    earlier: live.earlier(last_turn, id),
                }),
            ),
        }
    };
    let ctx = match live.frozen.iter().find(|(t, _)| *t == last_turn) {
        Some((_, ctx)) => ctx.clone(),
        None => {
            let f = live.fold(live.fold_cfg(&state.cfg.read()));
            fold::render(&live.tree, &f, &live.units).text()
        }
    };
    let (compactor, cancel) = {
        let cur = state.current.read();
        (cur.compactor.clone(), cur.cancel.clone())
    };
    // R1-8: a live session's next line goes before merges and backfill.
    let priority = u8::from(l == 0 && live.backfill().is_none());
    tokio::spawn(async move {
        let mut guard = JobGuard {
            state: state.clone(),
            key,
            model: compactor.model_id().to_owned(),
            cancel: cancel.clone(),
            armed: true,
        };
        let started = Instant::now();
        let outcome = match (free_text, step) {
            (Some(text), _) => Ok(Some(Node {
                l,
                i,
                size: text.len() as u32,
                text,
                free: true,
                model: None,
                tries: 0,
                mechanical: false,
            })),
            (None, Some(step)) => {
                let job = Job {
                    l,
                    i,
                    priority,
                    cancel: &cancel,
                };
                call(&state, &compactor, job, &ctx, &step).await
            }
            (None, None) => Err("no step".to_owned()),
        };
        guard.armed = false;
        finish(
            &state,
            key,
            outcome,
            started.elapsed(),
            compactor.model_id(),
            &cancel,
        )
        .await;
    });
}

/// One model-built node's job parameters.
struct Job<'a> {
    l: u8,
    i: u64,
    /// E9 queue priority (R1-8).
    priority: u8,
    /// Cancelled by a switch, a retry or a detach.
    cancel: &'a CancellationToken,
}

/// Bytes per token for prompt sizing (the 128 KB view ≈ 43k tokens).
const BYTES_PER_TOKEN: usize = 3;
/// Prompt bytes one more try adds (the refused reply + the correction).
const RETRY_BYTES: usize = 3_072;

/// E8: how many bytes of the `<chat>` view `c` may see — `local_ctx_bytes`
/// for a local compactor; for a known window, what fits beside the system
/// prompt, the step, the retries and the reply cap with a 10 % margin. The
/// step (the unit itself) is never trimmed.
fn ctx_budget(c: &dyn Compactor, step: &Step, cfg: &super::EndlessConfig, local: bool) -> usize {
    let mut max = if local {
        cfg.local_ctx_bytes.clamp(4_000, 256_000) as usize
    } else {
        usize::MAX
    };
    if let Some(window) = c.model().model.as_ref().map(|m| m.limit.context as usize)
        && window > 0
    {
        let fixed = compactor::COMPACT.len()
            + step.render(cfg.node_bytes).len()
            + usize::from(cfg.tries.max(1) - 1) * RETRY_BYTES;
        let room = (window * 9 / 10).saturating_sub(c.out_tokens() as usize) * BYTES_PER_TOKEN;
        max = max.min(room.saturating_sub(fixed));
    }
    max
}

/// `ctx` cut to at most `max` bytes, keeping its newest lines under the `<chat>` opener.
fn fit_ctx(ctx: &str, max: usize) -> std::borrow::Cow<'_, str> {
    const OPEN: &str = "<chat>\n";
    if ctx.len() <= max {
        return ctx.into();
    }
    let body = ctx.strip_prefix(OPEN).unwrap_or(ctx);
    let from = body.len().saturating_sub(max.saturating_sub(OPEN.len()));
    // Start at a line boundary (a byte after `\n` is a char boundary).
    let from = body.as_bytes()[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(body.len(), |p| from + p + 1);
    format!("{OPEN}{}", &body[from..]).into()
}

/// One node's compactor build, with its `model_calls` row. `Ok(None)`: the
/// job's calls were cancelled (E3 switch, R1-9 detach, R1-10 retry) — not a
/// failure, the node reschedules.
async fn call(
    state: &State,
    c: &Arc<dyn Compactor>,
    job: Job<'_>,
    ctx: &str,
    step: &Step,
) -> Result<Option<Node>, String> {
    let Job {
        l,
        i,
        priority,
        cancel,
    } = job;
    let local = c.local();
    let cfg = state.cfg.read().clone();
    // E9: a local endpoint's engine-wide queue; the wait is never timed.
    let _permit = match &local {
        Some(endpoint) => {
            let gate = state.queues.get(endpoint, cfg.local_jobs);
            tokio::select! {
                p = gate.acquire(priority) => Some(p),
                _ = cancel.cancelled() => return Ok(None),
            }
        }
        None => None,
    };
    // E6: the attempt bound starts after the permit.
    let attempt = Duration::from_secs(u64::from(if local.is_some() {
        cfg.local_attempt_secs.clamp(30, 7_200)
    } else {
        cfg.attempt_secs.clamp(10, 3_600)
    }));
    let ctx = fit_ctx(ctx, ctx_budget(c.as_ref(), step, &cfg, local.is_some()));
    let model = c.model();
    let request_id = format!("endless:{l}:{i}:{}", uuid::Uuid::now_v7());
    let reserved = state
        .store
        .reserve_call(
            state.session,
            &request_id,
            0,
            None,
            CallKind::Endless,
            model.provider.as_deref(),
            Some(&model.id),
        )
        .await
        .map_err(|e| format!("reserve: {e}"))?;
    if let Err(e) = state.store.mark_dispatched(&reserved).await {
        tracing::warn!(error = %e, "endless: mark_dispatched");
    }
    // E10: the last-resort bound over the whole job; a panic is a failure
    // whose ledger row still settles (R1-4).
    let outer = attempt * u32::from(cfg.tries.max(1)) + Duration::from_secs(60);
    let build = tokio::time::timeout(
        outer,
        std::panic::AssertUnwindSafe(compactor::build(
            c.as_ref(),
            &ctx,
            step,
            cfg.node_bytes,
            cfg.tries,
            attempt,
        ))
        .catch_unwind(),
    );
    let built = tokio::select! {
        b = build => Some(match b {
            Ok(Ok(built)) => built,
            Ok(Err(_panic)) => Err("compactor panicked".to_owned()),
            Err(_) => Err(format!("compactor job timed out after {}s", outer.as_secs())),
        }),
        _ = cancel.cancelled() => None,
    };
    // A failure that raced the switch is the old model's: not counted either.
    let built = built.filter(|b| b.is_ok() || !cancel.is_cancelled());
    let (status, usage) = match &built {
        Some(Ok(b)) => (CallStatus::Settled, b.usage()),
        Some(Err(_)) => (CallStatus::Failed, None),
        None => (CallStatus::Cancelled, None),
    };
    let obs = compactor::observation(usage.as_ref(), model);
    if let Err(e) = state.store.settle_call(&reserved, status, obs).await {
        tracing::warn!(error = %e, "endless: settle_call");
    }
    let Some(built) = built else {
        return Ok(None);
    };
    let built = built?;
    if let Err(e) = state
        .store
        .endless_calls_put(state.session, l, i, &built.calls)
        .await
    {
        tracing::warn!(error = %e, "endless: endless_calls");
    }
    Ok(Some(Node {
        l,
        i,
        size: built.text.len() as u32,
        text: built.text,
        free: false,
        model: Some(c.model_id().to_owned()),
        tries: built.tries,
        mechanical: false,
    }))
}

/// One more consecutive failure of `key`, with its backoff (caller holds `live`).
fn record_failure(state: &State, live: &mut Live, key: (u8, u64), e: &str, model: &str) {
    let base = state.cfg.read().retry_secs;
    let prev = live.failed.get(&key).map_or(0, |r| r.failures);
    let failures = prev.saturating_add(1);
    let delay = retry_delay(base, failures);
    live.failed.insert(
        key,
        Retry {
            at: Instant::now() + delay,
            failures,
            error: one_line(e, 300),
            model: model.to_owned(),
        },
    );
    if live.reported.insert(key) {
        tracing::warn!(session = %state.session, l = key.0, i = key.1, error = %e, "endless: node failed (will retry)");
        live.pending_error = Some(format!(
            "{}+{}: {e}",
            id_n(key.0, key.1).0,
            id_n(key.0, key.1).1
        ));
    }
    if delay >= RETRY_CAP && (prev == 0 || retry_delay(base, prev) < RETRY_CAP) {
        tracing::warn!(session = %state.session, l = key.0, i = key.1, model, failures, error = %e, "endless: node keeps failing (retrying every 30 min)");
    }
}

/// Persist and publish one job's outcome (`Ok(None)`: its calls were
/// cancelled). A failure of a job whose `cancel` fired is the old
/// compactor's (or a retried / detached call's) and is dropped under the
/// `live` lock, so no stale backoff outlives a switch (R1-3).
async fn finish(
    state: &State,
    key: (u8, u64),
    outcome: Result<Option<Node>, String>,
    took: Duration,
    model: &str,
    cancel: &CancellationToken,
) {
    let outcome = match outcome {
        Ok(Some(node)) => match state.store.endless_node_put(state.session, &node).await {
            Ok(()) => Ok(Some(node)),
            Err(e) => Err(format!("store: {e}")),
        },
        other => other,
    };
    let mut live = state.live.lock();
    live.running.remove(&key);
    match outcome {
        Ok(Some(node)) => {
            if node.l == 0 && !node.free {
                live.build_secs.0 += took.as_secs_f64();
                live.build_secs.1 += 1;
            }
            live.failed.remove(&key);
            live.tree.insert(node);
            live.invalidate();
        }
        Ok(None) => {}
        Err(_) if cancel.is_cancelled() => {}
        Err(e) => record_failure(state, &mut live, key, &e, model),
    }
    live.dirty = true;
    drop(live);
    // `settle` re-checks on every outcome (a failure may stall the frontier, E4).
    state.built_tx.send_modify(|v| *v += 1);
    state.wake.notify_one();
}

/// Publish `EndlessProgress` if due; returns how long the loop may sleep.
fn emit_progress(state: &Arc<State>) -> Duration {
    let mut live = state.live.lock();
    // R1-11: a failed node that is running or already due wakes the loop
    // through `finish` / `schedule`; only future retries set the timer.
    let now = Instant::now();
    let next_retry = live
        .failed
        .iter()
        .filter(|(k, r)| r.at > now && !live.running.contains(*k))
        .map(|(_, r)| r.at - now)
        .min()
        .unwrap_or(Duration::from_secs(60));
    if !live.dirty {
        return next_retry.max(Duration::from_millis(50));
    }
    if let Some(last) = live.last_progress
        && last.elapsed() < PROGRESS_EVERY
    {
        return (PROGRESS_EVERY - last.elapsed())
            .min(next_retry)
            .max(Duration::from_millis(20));
    }
    live.dirty = false;
    live.last_progress = Some(Instant::now());
    let cfg = live.fold_cfg(&state.cfg.read());
    let (built, total) = live.totals();
    let unbuilt = live.fold(cfg).unbuilt(&live.tree);
    let ev = EndlessProgress {
        session: state.session,
        built,
        total,
        settling: unbuilt > 0,
        error: live.pending_error.take(),
        backfill: live.backfill(),
        stalled: live.stalled(cfg),
        backlog: live.backlog(),
    };
    drop(live);
    let rt = state.rt.clone();
    tokio::spawn(async move { rt.publish(ev).await });
    next_retry.max(Duration::from_millis(50))
}

fn state_of(inner: &Inner, session: SessionId) -> Option<Arc<State>> {
    inner.sessions.lock().get(&session).map(|h| h.state.clone())
}

/// `s` on one line (whitespace runs → one space), at most `max` chars.
fn one_line(s: &str, max: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

pub(crate) async fn settle(inner: &Inner, session: SessionId, max: Duration) -> bool {
    let Some(state) = state_of(inner, session) else {
        return true;
    };
    if let Err(e) = ingest(&state).await {
        tracing::warn!(%session, error = %e, "endless: ingest before settle");
    }
    state.wake.notify_one();
    let mut rx = state.built_tx.subscribe();
    let deadline = Instant::now() + max;
    loop {
        let (unbuilt, stalled) = {
            let mut live = state.live.lock();
            let cfg = live.fold_cfg(&state.cfg.read());
            (
                live.fold(cfg).unbuilt(&live.tree),
                live.frontier_stalled(cfg),
            )
        };
        if unbuilt == 0 {
            return true;
        }
        // E4: a frontier that keeps failing will not land within this turn's
        // wait; 137G: nor will anything while settling is paused.
        if stalled || state.live.lock().paused {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || tokio::time::timeout(left, rx.changed()).await.is_err() {
            return false;
        }
    }
}

pub(crate) async fn render_for_turn(
    inner: &Inner,
    session: SessionId,
) -> anyhow::Result<ViewRender> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} is not attached to endless");
    };
    ingest(&state).await?;
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    let f = live.fold(cfg);
    let render = fold::render(&live.tree, &f, &live.units);
    live.turn += 1;
    let turn = live.turn;
    live.frozen.push_back((turn, render.text()));
    while live.frozen.len() > FROZEN_KEEP {
        live.frozen.pop_front();
    }
    Ok(render)
}

pub(crate) fn nudge(inner: &Inner, session: SessionId) {
    if let Some(state) = state_of(inner, session) {
        state.wake.notify_one();
    }
}

pub(crate) async fn set_view_bytes(
    inner: &Inner,
    session: SessionId,
    bytes: Option<u32>,
) -> anyhow::Result<()> {
    inner
        .store_for(session)
        .set_endless_view_bytes(session, bytes)
        .await?;
    if let Some(state) = state_of(inner, session) {
        let mut live = state.live.lock();
        live.view_bytes = bytes;
        live.invalidate();
    }
    Ok(())
}

/// R1-D1: the view rendered now under the session's budget, without
/// freezing a context or counting a turn (a native child's first message).
pub(crate) async fn render_snapshot(
    inner: &Inner,
    session: SessionId,
) -> anyhow::Result<Vec<ViewBlock>> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} is not attached to endless");
    };
    ingest(&state).await?;
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    let f = live.fold(cfg);
    Ok(fold::render(&live.tree, &f, &live.units).blocks)
}

/// R1-D6: the view re-folded to `bytes` as lines (a meeting head's
/// `## Earlier in this meeting`); the live fold is untouched. Serves a
/// `UnitFilter::Meeting` session, for which `active()` is false.
pub(crate) async fn render_with_budget(
    inner: &Inner,
    session: SessionId,
    bytes: u32,
) -> anyhow::Result<Vec<ViewLine>> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} is not attached to endless");
    };
    ingest(&state).await?;
    let live = state.live.lock();
    let cfg = state.cfg.read().fold_cfg_for(bytes);
    let f = Fold::rebuild(
        &live.tree,
        live.base.min(live.t()),
        live.t(),
        cfg,
        &live.units,
    );
    Ok(fold::render_lines(&live.tree, &f, &live.units))
}

/// Tree units of the rows (the filter applied), for sizing a backfill before any state exists.
async fn count_units(
    store: &Store,
    session: SessionId,
    cap_chars: u32,
    filter: UnitFilter,
) -> anyhow::Result<u64> {
    let rows = store.endless_messages_from(session, 0).await?;
    Ok(rows
        .iter()
        .filter(|m| !m.excluded)
        .map(|m| {
            units::derive(m.seq, m.at, &m.message, cap_chars, 0)
                .iter()
                .filter(|u| filter.keeps(u))
                .count() as u64
        })
        .sum())
}

/// R1-D4: the backfill under way, or the one a switch-on would start, with
/// its cost: the first `CALIBRATE_NODES` built nodes calibrate seconds and
/// USD per unit, else 5 s / $0.02.
pub(crate) async fn backfill_estimate(
    inner: &Inner,
    session: SessionId,
) -> anyhow::Result<Backfill> {
    let store = inner.store_for(session);
    let Some(state) = state_of(inner, session) else {
        let meta = store.endless_meta(session).await?;
        let (cap_chars, max_units) = {
            let cfg = inner.cfg.read();
            (cfg.cap_chars, cfg.backfill_max_units)
        };
        let t = count_units(&store, session, cap_chars, meta.filter).await?;
        let total = t.min(u64::from(max_units));
        return Ok(Backfill {
            done: 0,
            total,
            eta_secs: (total as f64 * DEFAULT_UNIT_SECS) as u64,
            usd_estimate: total as f64 * DEFAULT_UNIT_USD,
        });
    };
    ingest(&state).await?;
    let stats = store.endless_stats(session).await?;
    let mut live = state.live.lock();
    let built = live.tree.built_at(0).min(u64::from(CALIBRATE_NODES));
    if built > 0 && stats.calls > 0 {
        live.usd_per_unit = Some(stats.usd_estimate / built as f64);
    }
    let (done, total) = if live.mode == MemoryMode::Endless {
        let span = live.t().saturating_sub(live.base);
        (span - live.unbuilt_level0(), span)
    } else {
        (
            0,
            live.t().min(u64::from(state.cfg.read().backfill_max_units)),
        )
    };
    let left = total - done;
    Ok(Backfill {
        done,
        total,
        eta_secs: (left as f64 * live.secs_per_unit()) as u64,
        usd_estimate: left as f64 * live.usd_per_unit.unwrap_or(DEFAULT_UNIT_USD),
    })
}

fn view_line(part: Part, text: String, built: bool, live: &Live) -> ViewLine {
    let (id, n) = part.id_n();
    ViewLine {
        id,
        n,
        level: part.l,
        text: text.replace('\n', " "),
        built,
        at: (n == 1)
            .then(|| live.units.get(id as usize).and_then(|u| u.at))
            .flatten(),
        mechanical: live.tree.get(part.l, part.i).is_some_and(|n| n.mechanical),
    }
}

pub(crate) async fn zoom(
    inner: &Inner,
    session: SessionId,
    id: u64,
    n: u64,
) -> Result<Vec<ViewLine>, String> {
    let Some(state) = state_of(inner, session) else {
        return Err("this session has no endless memory".into());
    };
    let live = state.live.lock();
    let Some((l, i)) =
        tree::addr(id, n).filter(|(l, i)| exists(*l, *i, live.t()) && (*l == 0 || id >= live.base))
    else {
        return Err(format!("No line {id}+{n}."));
    };
    if l == 0 {
        let u = &live.units[i as usize];
        return Ok(vec![ViewLine {
            id,
            n: 1,
            level: 0,
            text: u.tagged(),
            built: live.tree.built(0, i),
            at: u.at,
            mechanical: false,
        }]);
    }
    Ok([2 * i, 2 * i + 1]
        .into_iter()
        .map(|ci| {
            let part = Part { l: l - 1, i: ci };
            let (text, built) = fold::part_text(&live.tree, part, &live.units);
            view_line(part, text, built, &live)
        })
        .collect())
}

pub(crate) async fn recall(
    inner: &Inner,
    session: SessionId,
    query: &str,
    limit: u8,
) -> Result<Vec<ViewLine>, String> {
    let Some(state) = state_of(inner, session) else {
        return Err("this session has no endless memory".into());
    };
    let limit = u32::from(limit.max(1));
    let hits = state
        .store
        .endless_recall(session, query, limit)
        .await
        .map_err(|e| e.to_string())?;
    let mut out: Vec<ViewLine> = {
        let live = state.live.lock();
        hits.into_iter()
            .map(|(i, text)| view_line(Part { l: 0, i }, text, true, &live))
            .collect()
    };
    if out.len() < 2 {
        let needle = query.trim();
        if !needle.is_empty() {
            let raw = state
                .store
                .endless_units_like(session, needle, limit)
                .await
                .map_err(|e| e.to_string())?;
            let live = state.live.lock();
            for i in raw {
                if out.iter().any(|v| v.id == i) || out.len() >= limit as usize {
                    continue;
                }
                let Some(u) = live.units.get(i as usize) else {
                    continue;
                };
                let text = format!(
                    "{} (raw)",
                    super::guards::cut_utf8(&u.tagged(), state.cfg.read().node_bytes as usize)
                );
                out.push(view_line(Part { l: 0, i }, text, false, &live));
            }
        }
    }
    Ok(out)
}

pub(crate) async fn date(
    inner: &Inner,
    session: SessionId,
    id: u64,
) -> Result<Option<DateTime<Utc>>, String> {
    let Some(state) = state_of(inner, session) else {
        return Err("this session has no endless memory".into());
    };
    let live = state.live.lock();
    live.units
        .get(id as usize)
        .map(|u| u.at)
        .ok_or_else(|| format!("No message {id}."))
}

/// 8.6 `/endless off` head text.
pub fn off_head(epoch: u32, view: &str) -> String {
    format!("<context_summary epoch=\"{epoch}\" style=\"endless\">\n{view}\n</context_summary>")
}

pub(crate) async fn set_mode(
    inner: &Inner,
    session: SessionId,
    mode: MemoryMode,
    filter: UnitFilter,
    epoch: u32,
) -> anyhow::Result<SetMode> {
    let store = inner.store_for(session);
    match mode {
        MemoryMode::Endless => {
            // R1-D4 / R1-D6: filter and base are fixed when the tree is first started.
            let meta = store.endless_meta(session).await?;
            let has_tree = !store.endless_nodes(session).await?.is_empty();
            if !has_tree {
                let (cap_chars, max_units) = {
                    let cfg = inner.cfg.read();
                    (cfg.cap_chars, cfg.backfill_max_units)
                };
                let t = count_units(&store, session, cap_chars, filter).await?;
                let base = t.saturating_sub(u64::from(max_units));
                store.set_endless_meta(session, filter, base).await?;
                if filter != meta.filter {
                    // The cache was derived under the old filter: re-derive from the rows.
                    store.endless_units_clear(session).await?;
                    if let Some(state) = state_of(inner, session) {
                        let mut live = state.live.lock();
                        live.units.clear();
                        live.turns.clear();
                        live.last_seq = -1;
                        live.invalidate();
                    }
                }
            } else {
                store
                    .set_endless_meta(session, meta.filter, meta.base)
                    .await?;
            }
            store.set_session_memory(session, mode).await?;
            let attached = inner
                .sessions
                .lock()
                .get(&session)
                .map(|h| h.attached.clone());
            let attached = match attached {
                Some(a) => a,
                None => {
                    let row = store
                        .session(session)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no session {session}"))?;
                    let model = inner
                        .catalog
                        .read()
                        .resolve(
                            &row.model,
                            &*crate::supervisor::configured(inner.auth.clone()),
                        )
                        .map_err(anyhow::Error::msg)?;
                    Attached {
                        model,
                        effort: row.effort.as_deref().and_then(crate::effort_of),
                        agent: row.desk.unwrap_or_else(|| "theuth".to_owned()),
                        store: None,
                    }
                }
            };
            // A fresh pump: `attach` reloads nothing for a known state but restarts the task.
            attach(inner, session, attached).await?;
            if let Some(state) = state_of(inner, session) {
                ingest(&state).await?;
                {
                    let mut live = state.live.lock();
                    live.backfill_total = live.history_unbuilt();
                    live.dirty = true;
                }
                state.wake.notify_one();
            }
            Ok(SetMode {
                memory: memory(inner, session).await?,
                head: None,
            })
        }
        MemoryMode::Epochs => {
            let head = match state_of(inner, session) {
                Some(state) => {
                    ingest(&state).await?;
                    let mut live = state.live.lock();
                    let cfg = live.fold_cfg(&state.cfg.read());
                    let f = live.fold(cfg);
                    let view = fold::render(&live.tree, &f, &live.units).text();
                    live.mode = MemoryMode::Epochs;
                    drop(live);
                    if let Some(h) = inner.sessions.lock().get_mut(&session) {
                        h.cancel.cancel();
                        h.cancel = CancellationToken::new();
                    }
                    Some(off_head(epoch, &view))
                }
                None => None,
            };
            store.set_session_memory(session, mode).await?;
            Ok(SetMode {
                memory: memory(inner, session).await?,
                head,
            })
        }
    }
}

pub(crate) async fn memory(inner: &Inner, session: SessionId) -> anyhow::Result<SessionMemory> {
    let store = inner.store_for(session);
    let mode = store.session_memory(session).await?;
    let Some(state) = state_of(inner, session) else {
        return Ok(SessionMemory {
            mode,
            view_bytes: inner.cfg.read().view_bytes,
            ..Default::default()
        });
    };
    let stats = store.endless_stats(session).await?;
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    let f = live.fold(cfg);
    let view = if mode == MemoryMode::Endless {
        fold::render_lines(&live.tree, &f, &live.units)
    } else {
        Vec::new()
    };
    let unbuilt = f.unbuilt(&live.tree);
    Ok(SessionMemory {
        mode,
        view,
        settling: unbuilt > 0,
        unbuilt,
        units: live.t(),
        stats,
        model: state.compactor().model_id().to_owned(),
        view_bytes: cfg.view_bytes,
        backfill: live.backfill(),
        stalled: if mode == MemoryMode::Endless {
            live.stalled(cfg)
        } else {
            None
        },
        backlog: if mode == MemoryMode::Endless {
            live.backlog()
        } else {
            None
        },
    })
}

/// RPC `endless_retry` (137F E5, R1-10): cancel the calls in flight (a hung
/// call restarts; a cancel is no failure), re-resolve the compactor, let
/// every failed node run now, wake the pump. A cancelled job keeps its
/// `running` key until it settles, so `schedule` never starts a duplicate.
pub(crate) async fn retry(inner: &Inner, session: SessionId) -> anyhow::Result<SessionMemory> {
    if let Some(state) = state_of(inner, session) {
        let endless = state.live.lock().mode == MemoryMode::Endless;
        if endless {
            state.cancel_calls();
            reresolve(inner, Some(session));
            state.reset_backoff();
        }
    }
    memory(inner, session).await
}

/// RPC `endless_fold` (137G G7): every unbuilt level-0 unit of `[base, T)`
/// becomes a built node now, no model call — a unit that fits is its free
/// node, a longer one its [`tree::mechanical_unit`] excerpt (flagged, tries
/// 0). Level-0 calls in flight are cancelled (not failures); merges follow
/// through the pump. The rows stay searchable (`recall`'s raw fallback and
/// the FTS mirror of the excerpts). An epochs session is untouched.
pub(crate) async fn fold_all(inner: &Inner, session: SessionId) -> anyhow::Result<SessionMemory> {
    let Some(state) = state_of(inner, session) else {
        return memory(inner, session).await;
    };
    if state.live.lock().mode != MemoryMode::Endless {
        return memory(inner, session).await;
    }
    ingest(&state).await?;
    let node_bytes = state.cfg.read().node_bytes;
    let nodes: Vec<Node> = {
        let live = state.live.lock();
        if live.running.iter().any(|k| k.0 == 0) {
            state.cancel_calls();
        }
        (live.base.min(live.t())..live.t())
            .filter(|i| !live.tree.built(0, *i))
            .map(|i| mechanical_node(&live.units[i as usize], node_bytes))
            .collect()
    };
    state.store.endless_nodes_put(session, &nodes).await?;
    let n = nodes.len();
    {
        let mut live = state.live.lock();
        for node in nodes {
            if !live.tree.built(0, node.i) {
                live.failed.remove(&(0, node.i));
                live.reported.remove(&(0, node.i));
                live.tree.insert(node);
            }
        }
        live.invalidate();
        live.dirty = true;
    }
    tracing::info!(%session, units = n, "endless: folded the backlog mechanically");
    state.built_tx.send_modify(|v| *v += 1);
    state.wake.notify_one();
    memory(inner, session).await
}

/// The node `endless_fold` writes for unit `u`: free when it fits, else the
/// mechanical excerpt.
fn mechanical_node(u: &Unit, node_bytes: u32) -> Node {
    let (text, mechanical) = match tree::mechanical_unit(u, node_bytes) {
        Some(text) => (text, true),
        None => (u.tagged(), false),
    };
    Node {
        l: 0,
        i: u.i,
        size: text.len() as u32,
        text,
        free: !mechanical,
        model: None,
        tries: 0,
        mechanical,
    }
}

/// RPC `endless_pause` (137G G8): persist the flag; pausing cancels the
/// calls in flight (not failures, the nodes reschedule on resume) and
/// `schedule` starts nothing while set; resuming wakes the pump. An epochs
/// session is untouched.
pub(crate) async fn pause(
    inner: &Inner,
    session: SessionId,
    paused: bool,
) -> anyhow::Result<SessionMemory> {
    let store = inner.store_for(session);
    if store.session_memory(session).await? != MemoryMode::Endless {
        return memory(inner, session).await;
    }
    store.set_endless_paused(session, paused).await?;
    if let Some(state) = state_of(inner, session) {
        {
            let mut live = state.live.lock();
            live.paused = paused;
            live.dirty = true;
        }
        if paused {
            state.cancel_calls();
        }
        tracing::info!(%session, paused, "endless: settling paused/resumed");
        state.built_tx.send_modify(|v| *v += 1);
        state.wake.notify_one();
    }
    memory(inner, session).await
}

/// R1-15: the frontier keeps failing — why `settle` returned early.
pub(crate) fn frontier_failing(inner: &Inner, session: SessionId) -> bool {
    let Some(state) = state_of(inner, session) else {
        return false;
    };
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    live.frontier_stalled(cfg)
}

/// Test seam: a state over an in-memory store with a scripted compactor is
/// exercised in `endless::tests`; the real pump only differs in `compactor`.
#[cfg(test)]
pub(crate) fn resolved_for_tests(id: &str) -> models::Resolved {
    models::Resolved {
        id: id.to_owned(),
        genai: id.to_owned(),
        provider: None,
        model: None,
    }
}

/// Test seam: `(key, retry at, consecutive failures)` of every failed node.
#[cfg(test)]
pub(crate) fn failed_for_tests(
    inner: &Inner,
    session: SessionId,
) -> Vec<((u8, u64), Instant, u32)> {
    let state = state_of(inner, session).expect("attached");
    let live = state.live.lock();
    let mut v: Vec<_> = live
        .failed
        .iter()
        .map(|(k, r)| (*k, r.at, r.failures))
        .collect();
    v.sort();
    v
}

/// Test seam: drive one job outcome through [`finish`] without the pump.
#[cfg(test)]
pub(crate) async fn finish_for_tests(
    inner: &Inner,
    session: SessionId,
    key: (u8, u64),
    outcome: Result<Node, String>,
) {
    let state = state_of(inner, session).expect("attached");
    let model = state.compactor().model_id().to_owned();
    let cancel = CancellationToken::new();
    finish(
        &state,
        key,
        outcome.map(Some),
        Duration::ZERO,
        &model,
        &cancel,
    )
    .await;
}

/// Test seam: the key of the compactor in force (the resolved id in tests).
#[cfg(test)]
pub(crate) fn key_for_tests(state: &State) -> String {
    state.current.read().key.clone()
}

/// Test seam: the engine-wide gate of a local endpoint.
#[cfg(test)]
pub(crate) fn gate_for_tests(inner: &Inner, endpoint: &str) -> Arc<Gate> {
    inner.queues.get(endpoint, inner.cfg.read().local_jobs)
}

/// Test seam: the session's running job keys, sorted.
#[cfg(test)]
pub(crate) fn running_for_tests(inner: &Inner, session: SessionId) -> Vec<(u8, u64)> {
    let state = state_of(inner, session).expect("attached");
    let mut v: Vec<_> = state.live.lock().running.iter().copied().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_doubles_to_the_cap() {
        let want = [10, 20, 40, 80, 160, 320, 640, 1280, 1800, 1800];
        for (i, secs) in want.into_iter().enumerate() {
            assert_eq!(
                retry_delay(10, i as u32 + 1),
                Duration::from_secs(secs),
                "n={}",
                i + 1
            );
        }
        assert_eq!(retry_delay(0, 1), Duration::ZERO);
        assert_eq!(retry_delay(0, 40), Duration::ZERO);
        assert_eq!(retry_delay(10, 63), RETRY_CAP);
        assert_eq!(retry_delay(10, 64), RETRY_CAP);
        assert_eq!(retry_delay(10, u32::MAX), RETRY_CAP);
        assert_eq!(retry_delay(u32::MAX, u32::MAX), RETRY_CAP);
    }

    /// A compactor with a catalog entry (window, price) or none (local).
    struct Sized(models::Resolved);

    impl Compactor for Sized {
        fn compress<'a>(
            &'a self,
            _ctx: &'a str,
            _convo: &'a [genai::chat::ChatMessage],
        ) -> futures::future::BoxFuture<'a, Result<compactor::Reply, String>> {
            Box::pin(async { Err("unused".to_owned()) })
        }
        fn model(&self) -> &models::Resolved {
            &self.0
        }
    }

    fn view(lines: usize) -> String {
        let body: String = (0..lines)
            .map(|k| format!("line {k:05} of the view ✓\n"))
            .collect();
        format!("<chat>\n{body}</chat>")
    }

    /// 137F E8: a local compactor sees ≤ `local_ctx_bytes` of the view, the
    /// newest lines kept whole; a known window fits prompt + reply with 10 % margin.
    #[test]
    fn prompt_view_fits_the_compactor() {
        let cfg = super::super::EndlessConfig {
            local_ctx_bytes: 4_000,
            ..Default::default()
        };
        let step = Step::Unit {
            kind: tree::Kind::User,
            text: "x".repeat(cfg.cap_chars as usize),
            earlier: vec![],
        };
        let ctx = view(2_000);
        let local = Sized(resolved_for_tests("ollama/q"));
        let max = ctx_budget(&local, &step, &cfg, true);
        assert_eq!(max, 4_000);
        let fit = fit_ctx(&ctx, max);
        assert!(fit.len() <= 4_000, "{}", fit.len());
        assert!(
            fit.starts_with("<chat>\nline ") && fit.ends_with("line 01999 of the view ✓\n</chat>")
        );
        assert!(fit.len() > 3_900, "only whole lines dropped: {}", fit.len());
        assert_eq!(fit_ctx(&ctx, usize::MAX), ctx, "under budget: untouched");

        let window: models::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "limit": {"context": 32_000, "output": 4096},
            "cost": {"input": 1.0, "output": 2.0}
        }))
        .unwrap();
        let cloud = Sized(models::Resolved {
            model: Some(window),
            ..resolved_for_tests("p/m")
        });
        let max = ctx_budget(&cloud, &step, &cfg, false);
        let prompt = compactor::COMPACT.len()
            + step.render(cfg.node_bytes).len()
            + 4 * RETRY_BYTES
            + fit_ctx(&view(20_000), max).len();
        let tokens = prompt / BYTES_PER_TOKEN + compactor::MAX_TOKENS as usize;
        assert!(tokens <= 32_000 * 9 / 10, "{tokens}");
        assert!(max > 30_000, "the window is used: {max}");
        let big = Sized(models::Resolved {
            model: None,
            ..resolved_for_tests("p/m")
        });
        assert_eq!(
            ctx_budget(&big, &step, &cfg, false),
            usize::MAX,
            "no window known"
        );
    }

    /// R1-8: with the one permit held, a frontier waiter queued after a
    /// merge waiter gets the permit first; a dropped wait gives up its place.
    #[tokio::test]
    async fn local_gate_serves_the_frontier_first() {
        let gate = Gate::new(1);
        let held = gate.acquire(0).await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let waiter = |name: &'static str, priority: u8| {
            let (gate, order) = (gate.clone(), order.clone());
            tokio::spawn(async move {
                let _p = gate.acquire(priority).await;
                order.lock().push(name);
                tokio::task::yield_now().await;
            })
        };
        let gone = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let _p = gate.acquire(1).await;
                unreachable!("aborted before the permit");
            })
        };
        let merge = waiter("merge", 0);
        tokio::task::yield_now().await;
        let frontier = waiter("frontier", 1);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        gone.abort();
        let _ = gone.await;
        assert_eq!(
            gate.state.lock().waiting.len(),
            2,
            "the cancelled wait left the line"
        );
        drop(held);
        frontier.await.unwrap();
        merge.await.unwrap();
        assert_eq!(*order.lock(), ["frontier", "merge"]);
        assert_eq!(gate.state.lock().free, 1, "every permit came back");
    }

    /// R2-2: under steady frontier load every 4th grant goes to the oldest
    /// waiter, so a merge queued first is not starved.
    #[tokio::test]
    async fn local_gate_ages_the_oldest_waiter_in() {
        let gate = Gate::new(1);
        let held = gate.acquire(1).await; // grant 1
        let order = Arc::new(Mutex::new(Vec::new()));
        let waiter = |name: &'static str, priority: u8| {
            let (gate, order) = (gate.clone(), order.clone());
            tokio::spawn(async move {
                let _p = gate.acquire(priority).await;
                order.lock().push(name);
                tokio::task::yield_now().await;
            })
        };
        let mut tasks = vec![waiter("merge", 0)];
        tokio::task::yield_now().await;
        for name in ["f1", "f2", "f3", "f4"] {
            tasks.push(waiter(name, 1));
            tokio::task::yield_now().await;
        }
        drop(held);
        for t in tasks {
            t.await.unwrap();
        }
        // Grants 2 and 3 by priority, grant 4 to the oldest, then priority again.
        assert_eq!(*order.lock(), ["f1", "f2", "merge", "f3", "f4"]);
    }
}
