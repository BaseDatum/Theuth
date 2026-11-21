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
use super::tree::{self, Node, Quality, Tree, Unit, exists, id_n};
use super::{
    Attached, Backfill, Backlog, EndlessConfig, EndlessProgress, Inner, MemoryMode, Profile,
    SessionMemory, SetMode, Stalled, TailMode, TailOverride, UnitFilter, ViewLine, units,
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
    /// R1-2 / R2-2: consecutive failures that were not transient
    /// (`compact::classify`), plus empty replies, timeouts and panics.
    hard: u32,
    /// R1-2: when this run of failures began.
    since: Instant,
    /// R1-3: the unit's own call is over `max_usd_per_call`.
    cost: bool,
    /// Last error, one line, ≤ 300 chars.
    error: String,
    /// The compactor model id that failed.
    model: String,
}

impl Retry {
    /// R1-2 / G7: the unit folds mechanically on its next run — over cost,
    /// or at least [`MECH_AFTER`] hard failures, the first ≥ [`MECH_AGE`]
    /// ago, and (R2-2) another node built by the model since then: an
    /// environmental error (401, no credit, unknown model) fails every unit
    /// and must stall visibly, not fold the whole backlog.
    fn mechanical(&self, last_ok: Option<Instant>) -> bool {
        self.cost
            || (self.hard >= MECH_AFTER
                && self.since.elapsed() >= MECH_AGE
                && last_ok.is_some_and(|t| t > self.since))
    }
}

/// R2-2: a failure that is no provider's transient refusal but the call
/// itself not yielding a line — counted hard although `classify` may call it
/// transient, so a unit that always comes back empty or always times out folds.
fn hard_failure(e: &str) -> bool {
    e.contains("compactor returned nothing")
        // 177 E3: a reply spent on thinking is the empty case with a cause.
        || e.contains("reply on thinking")
        || e.contains("timed out")
        || e.contains("stream idle")
        || e.contains("panicked")
        || !matches!(
            crate::compact::classify(e),
            crate::compact::Failure::Transient
        )
}

/// R2-1: a failed call the provider may still bill — dropped by theuth after
/// dispatch (an attempt or job timeout, an idle stream). Refusals (overflow,
/// 4xx, 429/5xx, connect errors, the local preflight's "not sent") are not.
fn billed_drop(e: &str) -> bool {
    e.contains("timed out") || e.contains("stream idle")
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

    /// 201 §4.2: the session's tail override (persisted on the row).
    pub(crate) fn tail(&self) -> Option<TailOverride> {
        self.state.live.lock().tail
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
    /// 201 §4.2: per-session tail override (persisted); read by phase P2.
    tail: Option<TailOverride>,
    /// 137G `endless_pause`: `user` | `spend`; no compactor calls while set (persisted).
    paused_by: Option<String>,
    /// 137G: the compactor chain's first two links — this chat's pick, the desk agent's default.
    picks: Picks,
    /// Every unit, dense by `i`.
    // ponytail: whole-session unit text in memory; page from the store if a session outgrows RAM.
    units: Vec<Unit>,
    tree: Tree,
    /// `Fold::rebuild(tree, T)` cache; `None` after the tree or `T` changed.
    fold: Option<Fold>,
    running: HashSet<(u8, u64)>,
    /// Model-call jobs in flight (a batch is one; free and mechanical nodes none).
    jobs: usize,
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
    /// Measured level-0 build time: (sum of call secs, units they built) — a
    /// batch adds its secs once for all its units (G6 ETA).
    build_secs: (f64, u32),
    /// Calls the pump runs at once (cloud `jobs`, local 1): the ETA divisor.
    parallel: u32,
    /// 137G G6: units whose batch line failed its guard: next time alone.
    solo: HashSet<u64>,
    /// R1-4 / R2-3: the day's spend is within `jobs` calls of the cap — one
    /// call at a time. Starts set (unknown until the first call checks the spend).
    near_cap: bool,
    /// R2-2: when the compactor last built a node (no mechanical fold
    /// without another success since a unit's first failure).
    last_ok: Option<Instant>,
    /// R1-3: the over-cost notice went out (once per session state).
    cost_noticed: bool,
    /// Measured USD per unit from the ledger (R1-D4), refreshed by `backfill_estimate`.
    usd_per_unit: Option<f64>,
    /// Progress event throttle.
    last_progress: Option<Instant>,
    dirty: bool,
    pending_error: Option<String>,
    /// `unit.i → turn` (a unit is pure §4 data; the turn is pump bookkeeping).
    turns: HashMap<u64, u32>,
    /// 201 P1: level-0 units whose node is a draft — the refinement queue,
    /// oldest first.
    drafts: BTreeSet<u64>,
    /// 201 P1: draft merges to rebuild (a child changed, or the model merge
    /// failed and is retried after its backoff).
    stale: HashSet<(u8, u64)>,
    /// 201 P1: merges whose child changed while they were being built; the
    /// node that lands from the old inputs stays stale.
    redo: HashSet<(u8, u64)>,
    /// 201 §3 (P2): the generation in force — its frozen view (cache of the
    /// newest `endless_generations` row; `None` until loaded or started).
    current_gen: Option<Generation>,
    /// 201 §4 (P2): the tail bytes the last turn opened with (status).
    tail_bytes: u32,
    /// 201 §4.2 (P2): the session model's turn allowance in bytes, as the
    /// last turn measured it (`scaled` marks in the status).
    allowance: Option<u64>,
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

    /// D14 progress; `None` once every history unit has its line. 201 P1
    /// (D5): a draft is a line, so a drafting session's backfill ends at
    /// ingest and the endless frame never waits on refinement (the backlog
    /// still counts drafts); a meeting tree counts built units as before.
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

    /// Wall seconds per level-0 unit: measured call secs per unit (a batch
    /// amortised over its units) over the calls run at once, else the default.
    fn secs_per_unit(&self) -> f64 {
        let per = if self.build_secs.1 > 0 {
            self.build_secs.0 / f64::from(self.build_secs.1)
        } else {
            DEFAULT_UNIT_SECS
        };
        per / f64::from(self.parallel.max(1))
    }

    /// Level-0 units in `[base, T)` without a line (201 P1: or with a draft).
    fn unbuilt_level0(&self) -> u64 {
        (self.base.min(self.t())..self.t())
            .filter(|i| !self.lined(*i))
            .count() as u64
    }

    /// 201 P1: unit `i` has its final line — built, and not a draft that
    /// this session refines.
    fn lined(&self, i: u64) -> bool {
        self.tree
            .get(0, i)
            .is_some_and(|n| !(self.drafting() && n.quality() == Quality::Draft))
    }

    /// 201 P1: drafts are made at ingest and refined, merges are built from
    /// drafts, and no frontier gates anything — every session but a
    /// meeting's, whose tree enriches its head exactly as before.
    fn drafting(&self) -> bool {
        self.filter != UnitFilter::Meeting
    }

    /// Insert `node`, keeping the draft queue and the stale marks (201 P1):
    /// a node that replaces another marks its draft ancestors for rebuilding.
    fn put(&mut self, node: Node) {
        let (l, i) = (node.l, node.i);
        let replaced = self.tree.built(l, i);
        if l == 0 {
            if node.quality() == Quality::Draft {
                self.drafts.insert(i);
            } else {
                self.drafts.remove(&i);
            }
        } else {
            self.stale.remove(&(l, i));
            if self.redo.remove(&(l, i)) {
                self.stale.insert((l, i));
            }
        }
        self.tree.insert(node);
        if replaced && self.drafting() {
            // A draft ancestor is rebuilt; one being built right now was
            // built from the old child, so it is rebuilt again when it lands.
            let (mut pl, mut pi) = (l + 1, i / 2);
            loop {
                let key = (pl, pi);
                let running = self.running.contains(&key);
                let draft = self
                    .tree
                    .get(pl, pi)
                    .is_some_and(|p| p.quality() == Quality::Draft);
                if running {
                    self.redo.insert(key);
                }
                if draft {
                    self.stale.insert(key);
                }
                if !(running || draft) {
                    break;
                }
                pl += 1;
                pi /= 2;
            }
        }
        self.invalidate();
    }

    /// 201 P1: rebuild the draft queue and the stale marks from the tree (load).
    fn index_drafts(&mut self) {
        self.drafts = self
            .tree
            .iter()
            .filter(|n| n.l == 0 && n.quality() == Quality::Draft)
            .map(|n| n.i)
            .collect();
        let q = |l: u8, i: u64| self.tree.get(l, i).map(Node::quality);
        self.stale = self
            .tree
            .iter()
            .filter(|n| n.l > 0 && n.quality() == Quality::Draft)
            .filter(|n| {
                let (a, b) = (q(n.l - 1, 2 * n.i), q(n.l - 1, 2 * n.i + 1));
                a != Some(Quality::Draft) && b != Some(Quality::Draft)
            })
            .map(|n| (n.l, n.i))
            .collect();
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

    /// Level-0 lines (text only) of units in `turn` with `i < before`, oldest
    /// first; the newest [`EARLIER_MAX`] only (137G G3).
    fn earlier(&self, turn: u32, before: u64) -> Vec<String> {
        let mut out: Vec<String> = self.units[..(before as usize).min(self.units.len())]
            .iter()
            .rev()
            .filter(|u| self.unit_turn(u.i) == turn)
            .filter_map(|u| self.tree.get(0, u.i).map(|n| n.text.clone()))
            .take(EARLIER_MAX)
            .collect();
        out.reverse();
        out
    }

    fn unit_turn(&self, i: u64) -> u32 {
        self.turns.get(&i).copied().unwrap_or(0)
    }

    /// D14: one past the last history unit (`turn == 0`) without a line; 0 = none.
    fn history_unbuilt(&self) -> u64 {
        self.units
            .iter()
            .filter(|u| u.i >= self.base && self.unit_turn(u.i) == 0 && !self.lined(u.i))
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
    /// `cfg` is the fold whose G2 runs say how many lines show folded.
    fn backlog(&mut self, cfg: FoldCfg) -> Option<Backlog> {
        let lines = u32::try_from(self.unbuilt_level0()).unwrap_or(u32::MAX);
        let paused = self.paused_by.is_some();
        if lines == 0 && !paused {
            return None;
        }
        self.ensure_fold(cfg);
        let folded = self.fold.as_ref().map_or(0, Fold::folded);
        Some(Backlog {
            lines,
            folded,
            eta_secs: if paused {
                0
            } else {
                (f64::from(lines) * self.secs_per_unit()) as u64
            },
            paused,
            paused_by: self.paused_by.clone(),
        })
    }
}

impl Live {
    fn new(meta: crate::store::endless::EndlessMeta) -> Self {
        let picks = Picks::of(&meta);
        Self {
            mode: meta.mode,
            filter: meta.filter,
            base: meta.base,
            view_bytes: meta.view_bytes,
            tail: meta.tail,
            paused_by: meta.paused_by,
            picks,
            units: Vec::new(),
            tree: Tree::default(),
            fold: None,
            running: HashSet::new(),
            jobs: 0,
            failed: HashMap::new(),
            reported: HashSet::new(),
            frozen: VecDeque::new(),
            turn: 0,
            last_seq: -1,
            backfill_total: 0,
            build_secs: (0.0, 0),
            parallel: 1,
            solo: HashSet::new(),
            near_cap: true,
            last_ok: None,
            cost_noticed: false,
            usd_per_unit: None,
            last_progress: None,
            dirty: false,
            pending_error: None,
            turns: HashMap::new(),
            drafts: BTreeSet::new(),
            stale: HashSet::new(),
            redo: HashSet::new(),
            current_gen: None,
            tail_bytes: 0,
            allowance: None,
        }
    }
}

/// 137G: the compactor chain's per-session links.
#[derive(Clone, Debug, Default)]
struct Picks {
    /// This chat's own pick (`sessions.endless_model`).
    chat: Option<String>,
    /// The desk agent's `memory_model` (`sessions.endless_agent_model`).
    agent: Option<String>,
    /// 194: this chat's own effort (`sessions.endless_effort`), over any link's.
    chat_effort: Option<String>,
    /// 194: the desk agent's `memory_effort` (`sessions.endless_agent_effort`).
    agent_effort: Option<String>,
}

impl Picks {
    fn of(meta: &crate::store::endless::EndlessMeta) -> Self {
        Self {
            chat: meta.model_override.clone(),
            agent: meta.agent_model.clone(),
            chat_effort: meta.effort_override.clone(),
            agent_effort: meta.agent_effort.clone(),
        }
    }
}

/// 137G chain: chat override → agent default → `agent.endless.model` →
/// `agent.title_model` → the session's model (`None`), with the link's name
/// and (194) the effort: this chat's own, else the choosing link's own —
/// `memory_effort`, `agent.endless.effort`; none for the title and session
/// links (thinking off, R1 Q1).
fn chosen(inner: &Inner, picks: &Picks) -> (Option<String>, &'static str, Option<String>) {
    let settings = {
        let cfg = inner.cfg.read();
        cfg.model.clone().map(|m| (m, cfg.effort.clone()))
    };
    let (model, source, effort) = if let Some(m) = &picks.chat {
        (Some(m.clone()), "chat", None)
    } else if let Some(m) = &picks.agent {
        (Some(m.clone()), "agent", picks.agent_effort.clone())
    } else if let Some((m, e)) = settings {
        (Some(m), "settings", e)
    } else {
        match inner.title_model.read().clone() {
            Some(m) => (Some(m), "title", None),
            None => (None, "session", None),
        }
    };
    (model, source, picks.chat_effort.clone().or(effort))
}

/// 194 D4b: `effort` against what `model` resolves to
/// (`models::Model::validate_effort`'s words); an id the catalog does not
/// know checks only that the word is an effort ([`crate::effort_of`]).
pub(crate) fn check_effort(model: Option<&models::Model>, effort: &str) -> Result<(), String> {
    match model {
        Some(m) => m.validate_effort(effort),
        None if crate::effort_of(effort).is_none() => Err(format!("unknown effort {effort}")),
        None => Ok(()),
    }
}

/// 194: the chain's effort as `model` can run it — one it does not take
/// (its link's model changed under a chat's own effort) is dropped, warned.
fn usable_effort(model: &models::Resolved, effort: Option<String>) -> Option<String> {
    let e = effort?;
    match check_effort(model.model.as_ref(), &e) {
        Ok(()) => Some(e),
        Err(why) => {
            tracing::warn!(model = %model.id, effort = %e, "endless compactor: effort dropped: {why}");
            None
        }
    }
}

/// The compactor for a session ([`chosen`]; R1-D3 + 137G) with the chain's
/// effort (194) — every link read live (137F E1). Keyed by model id, effort
/// and whether it is the user's pick (two requests differ by it alone: a
/// listed `low` is also the thinking-off default).
fn compactor_for(
    inner: &Inner,
    session: SessionId,
    attached: &Attached,
    picks: &Picks,
) -> (String, Arc<dyn Compactor>) {
    let (chosen, source, effort) = chosen(inner, picks);
    if let Some(f) = &inner.compactor_for_tests {
        let id = chosen.unwrap_or_else(|| attached.model.id.clone());
        let c = f(&id);
        return (id, c);
    }
    let model = match chosen {
        Some(id) => match inner
            .catalog
            .read()
            .resolve(&id, &*crate::supervisor::configured(inner.auth.clone()))
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(model = %id, source, error = %e, "endless compactor: falling back to the session model");
                attached.model.clone()
            }
        },
        None => attached.model.clone(),
    };
    let effort = usable_effort(&model, effort)
        .as_deref()
        .and_then(crate::effort_of);
    let c = model_compactor(inner, session, model, &attached.agent, effort);
    // 177 E6: new limits (a refresh, `set_model_limits`) make a new compactor.
    let limits = c
        .model
        .model
        .as_ref()
        .map(|m| (m.limit.context, m.limit.output));
    (
        format!("{}|{:?}|{}|{limits:?}", c.model.id, c.effort, c.chosen),
        Arc::new(c),
    )
}

/// The real compactor for `model` — the request options of every summary
/// call: locality (137F E6), thinking off (R1 Q1) unless the user `chosen`
/// an effort for it (194).
pub(crate) fn model_compactor(
    inner: &Inner,
    session: SessionId,
    model: models::Resolved,
    agent: &str,
    chosen: Option<genai::chat::ReasoningEffort>,
) -> ModelCompactor {
    let base =
        crate::backend::target_for(&model, &inner.auth).map(|t| t.endpoint.base_url().to_owned());
    let local = compactor::local_endpoint(&model, base.as_deref());
    let backend = Backend::for_model(&model, &inner.client, &inner.auth);
    // 177 E2: a local OpenAI-protocol server is told by `chat_template_kwargs`.
    let quiet = backend.local_openai(&model);
    // R1 Q1: the compactor never thinks, whatever the session's effort (E8:
    // `think: false` only through genai's own Ollama adapter, not a gateway,
    // R1-14; 177 E2: never an effort keyword next to the kwargs) — unless
    // the user picked a level ([`ModelCompactor::with_pick`]).
    let effort = if quiet || (base.is_some() && crate::loop_::ns(&model.genai) == "ollama") {
        None
    } else {
        compactor::thinking_off(&model)
    };
    ModelCompactor {
        session,
        backend,
        model,
        system: compactor::prompt(agent),
        effort,
        chosen: false,
        local,
        quiet,
        cfg: inner.cfg.clone(),
    }
    .with_pick(chosen)
}

/// 177 E9: the fixed sample [`test_summary`] compresses.
const SUMMARY_SAMPLE_CTX: &str = "<chat>\nuser: Can you move the weekly sync to Thursday at 3pm and tell Dana?\ntalk: Done: the weekly sync is now Thursday 15:00 and Dana has the new invite.\n</chat>";
const SUMMARY_SAMPLE_UNIT: &str =
    "Done: the weekly sync is now Thursday 15:00 and Dana has the new invite.";

/// 177 E9: the cost owner the summary tests are charged to (no session owns them).
const SUMMARY_TEST_OWNER: SessionId = SessionId(uuid::Uuid::from_u128(
    0x0177_e9e9_0000_7000_8000_0000_0000_0177,
));

/// 177 E9: what to do about a failed summary test.
fn summary_hint(e: &str) -> Option<&'static str> {
    let l = e.to_ascii_lowercase();
    Some(if l.contains("reply on thinking") {
        compactor::THINKING_HINT
    } else if l.contains("connection refused")
        || l.contains("error sending request")
        || l.contains("connect")
    {
        "The server did not answer: is it running at the provider's URL?"
    } else if l.contains("401") || l.contains("403") || l.contains("unauthorized") {
        "The server wants an API key: add the provider again with its key."
    } else if l.contains("404") || l.contains("not found") {
        "The server does not serve this model id: refresh the provider's models."
    } else if l.contains("timed out") || l.contains("stream idle") {
        "Too slow for the attempt bound: pick a smaller model or raise Memory's local attempt seconds."
    } else if crate::compact::is_overflow(e) {
        "The prompt is over the model's window: set its context under Local servers."
    } else if l.contains("returned nothing") {
        "The model wrote nothing: pick a model that follows short instructions."
    } else {
        return None;
    })
}

/// 177 E9: one real single-unit compactor call for `model` on a fixed
/// two-message sample, with the request options (E2/E3) and guards of a
/// summary, charged like a side call (`model_calls`, kind `endless`, to
/// [`SUMMARY_TEST_OWNER`]). Never writes memory.
pub(crate) async fn test_summary(inner: &Inner, model: &str) -> crate::SummaryTest {
    let mut out = crate::SummaryTest {
        model: model.to_owned(),
        ..Default::default()
    };
    let resolved = match inner
        .catalog
        .read()
        .resolve(model, &*crate::supervisor::configured(inner.auth.clone()))
    {
        Ok(m) => m,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    out.model = resolved.id.clone();
    let c = model_compactor(inner, SessionId::new(), resolved, "theuth", None);
    let (node_bytes, attempt) = {
        let cfg = inner.cfg.read();
        let secs = if c.local.is_some() {
            cfg.local_attempt_secs.clamp(30, 7_200)
        } else {
            cfg.attempt_secs.clamp(10, 3_600)
        };
        (cfg.node_bytes, Duration::from_secs(u64::from(secs)))
    };
    let step = compactor::Step::Unit {
        kind: tree::Kind::Talk,
        text: SUMMARY_SAMPLE_UNIT.to_owned(),
        earlier: Vec::new(),
        aim: None,
    };
    let request_id = format!("endless:test:{}", uuid::Uuid::now_v7());
    let reserved = match inner.store.ensure_owner(SUMMARY_TEST_OWNER, None).await {
        Ok(()) => inner
            .store
            .reserve_call(
                SUMMARY_TEST_OWNER,
                &request_id,
                0,
                None,
                CallKind::Endless,
                c.model.provider.as_deref(),
                Some(&c.model.id),
            )
            .await
            .map_err(|e| tracing::warn!(error = %e, "summary test: reserve"))
            .ok(),
        Err(e) => {
            tracing::warn!(error = %e, "summary test: cost owner");
            None
        }
    };
    let tally = compactor::Tally::new(&c);
    let started = Instant::now();
    let built = compactor::build(&tally, SUMMARY_SAMPLE_CTX, &step, node_bytes, 1, attempt).await;
    out.latency_ms = started.elapsed().as_millis() as u64;
    let calls = match &built {
        Ok(b) => &b.calls,
        Err(f) => &f.calls,
    };
    let usage = compactor::sum_usage(calls);
    if let Some(r) = &reserved {
        let status = if built.is_ok() {
            CallStatus::Settled
        } else {
            CallStatus::Failed
        };
        let obs = compactor::observation(usage.as_ref(), &c.model);
        if let Err(e) = inner.store.settle_call(r, status, obs).await {
            tracing::warn!(error = %e, "summary test: settle");
        }
    }
    if let Some(u) = &usage {
        let n = |v: Option<i32>| v.and_then(|n| u32::try_from(n).ok());
        out.tokens_in = n(u.prompt_tokens);
        out.tokens_out = n(u.completion_tokens);
    }
    match tally.last() {
        Some(r) => {
            out.reasoning_tokens = r.reasoning_tokens();
            out.finish_reason = r.stop.clone();
            out.reasoning_seen = r.reasoning;
            out.thinking_off_sent = r.thinking_off;
        }
        None => out.thinking_off_sent = c.quiet || c.effort.is_some(),
    }
    match built {
        Ok(b) if b.tries > 0 => {
            out.ok = true;
            out.line = Some(b.text);
        }
        Ok(b) => {
            out.error = Some(format!("the reply failed the summary guards: {}", b.text));
            out.hint = Some("Pick a model that follows short instructions.".into());
        }
        Err(f) => {
            out.hint = summary_hint(&f.error).map(str::to_owned);
            out.error = Some(f.error);
        }
    }
    out
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
        let picks = state.live.lock().picks.clone();
        if !state.switch(compactor_for(inner, id, &attached, &picks)) {
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
            live.tail = meta.tail;
            live.paused_by = meta.paused_by.clone();
            live.picks = Picks::of(&meta);
            live.invalidate();
            drop(live);
            state
        }
        None => {
            let picks = Picks::of(&meta);
            let current = compactor_for(inner, session, &attached, &picks);
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
    let picks = state.live.lock().picks.clone();
    state.switch(compactor_for(inner, session, &attached, &picks));
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
        live.index_drafts();
        live.backfill_total = live.history_unbuilt();
        live.invalidate();
    }
    // Rows that landed after the cache (or a cache that never existed); then
    // 201 P1: every unit without a node (an older session) gets its draft.
    ingest(state).await?;
    fill_drafts(state, 0).await?;
    let mut live = state.live.lock();
    live.backfill_total = live.history_unbuilt();
    Ok(())
}

/// 201 P1: the stored node of a new level-0 unit, no model call: its own
/// tagged text when it fits (free), else its [`draft_line`].
fn draft_node(u: &Unit, node_bytes: u32) -> Node {
    let (text, quality) = match tree::free_unit(u, node_bytes) {
        Some(text) => (text, Quality::Free),
        None => (draft_line(u, node_bytes), Quality::Draft),
    };
    let mut node = Node {
        l: 0,
        i: u.i,
        size: text.len() as u32,
        text,
        free: false,
        model: None,
        tries: 0,
        mechanical: false,
    };
    node.set_quality(quality);
    node
}

/// 201 P1: give every level-0 unit in `[max(base, from), T)` without a node
/// its [`draft_node`] (stored only where none exists yet). An epochs session
/// and a meeting tree get none.
async fn fill_drafts(state: &State, from: u64) -> anyhow::Result<()> {
    let nodes: Vec<Node> = {
        let live = state.live.lock();
        if live.mode != MemoryMode::Endless || !live.drafting() {
            return Ok(());
        }
        let node_bytes = state.cfg.read().node_bytes;
        (live.base.max(from).min(live.t())..live.t())
            .filter(|i| !live.tree.built(0, *i))
            .map(|i| draft_node(&live.units[i as usize], node_bytes))
            .collect()
    };
    if nodes.is_empty() {
        return Ok(());
    }
    state
        .store
        .endless_nodes_put_new(state.session, &nodes)
        .await?;
    let mut live = state.live.lock();
    for node in nodes {
        if !live.tree.built(0, node.i) {
            live.put(node);
        }
    }
    live.dirty = true;
    Ok(())
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
    let first = next_i;
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
    {
        let mut live = state.live.lock();
        for u in new_units {
            live.turns.insert(u.i, turn);
            live.units.push(u);
        }
        live.last_seq = last_seq;
        live.invalidate();
        live.dirty = true;
    }
    fill_drafts(state, first).await
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

/// 137G G7 / R1-2: a level-0 unit folds mechanically after this many
/// non-transient failures in a row …
const MECH_AFTER: u32 = 3;
/// … the first at least this long ago (a short outage never folds).
const MECH_AGE: Duration = Duration::from_secs(600);
/// 137G G6: a batch's unit text in bytes at most.
const BATCH_BYTES: usize = 24_000;
/// 137G G3: lines `earlier` carries at most.
const EARLIER_MAX: usize = 24;
/// 137G G11: the error of a call whose prompt alone would cost over
/// `max_usd_per_call` — never sent.
const OVER_COST: &str = "over max_usd_per_call";
/// 201 P1 (F9): the error of a build whose every candidate failed a guard.
const NO_PASSING_LINE: &str = "no line passed the guards";
/// R1-3: the notice when a unit's own call is over `max_usd_per_call`.
const COST_NOTICE: &str =
    "summary model too expensive for max_usd_per_call; lines folded as excerpts";

/// One model-call job the scheduler starts.
enum Work {
    /// One node: a merge or a single unit.
    Node((u8, u64)),
    /// G6: several level-0 units in one call.
    Batch(Vec<u64>),
}

/// A closed batch: one unit is a single job.
fn close(batch: &mut Vec<u64>) -> Option<Work> {
    match std::mem::take(batch).as_slice() {
        [] => None,
        [i] => Some(Work::Node((0, *i))),
        b => Some(Work::Batch(b.to_vec())),
    }
}

/// Start every eligible node. 201 P1: every level-0 unit already has a
/// line, so level 0 is "the next draft to refine", oldest first, and merges
/// are "the next merge whose inputs changed" (missing, or a stale draft);
/// nothing waits behind a failing unit (137 rule 3 is gone). 137G G6:
/// level 0 runs in batches of up to `batch_units` consecutive drafts
/// (≤ 24 KB of text); a unit whose batch line failed its guard runs alone.
/// A merge with a draft child, or whose children fit, is built with no call.
/// A cloud compactor runs up to `jobs` calls at once; a local one one
/// level-0 job and one merge (E9). A meeting tree (no drafts) keeps the 137
/// rules: unbuilt units from the frontier on, mechanical folds, merges only
/// behind the frontier.
fn schedule(state: &Arc<State>) {
    let mut live = state.live.lock();
    if live.mode != MemoryMode::Endless || live.paused_by.is_some() {
        return;
    }
    let (jobs, batch_units, node_bytes) = {
        let c = state.cfg.read();
        (
            usize::from(c.jobs.max(1)),
            usize::from(c.batch_units.clamp(1, 64)),
            c.node_bytes,
        )
    };
    let local = state.compactor().local().is_some();
    live.parallel = if local { 1 } else { jobs as u32 };
    let cfg = live.fold_cfg(&state.cfg.read());
    let t = live.t();
    let base = live.base;
    let now = Instant::now();
    let drafting = live.drafting();
    let frontier = if drafting { t } else { live.frontier(cfg) };
    let due = |live: &Live, key: &(u8, u64)| {
        !live.running.contains(key) && live.failed.get(key).is_none_or(|r| r.at <= now)
    };
    let level0_busy = live.running.iter().any(|k| k.0 == 0);
    let merging = live.running.iter().any(|k| k.0 > 0);
    let mut slots = if local {
        usize::from(!level0_busy)
    } else {
        jobs.saturating_sub(live.jobs)
    };
    // R1-4: within one call of the day cap, one call at a time (the
    // overshoot is at most that call). A local compactor already runs at
    // most one level-0 call and one merge, and waits at its endpoint's
    // queue before the spend check.
    if live.near_cap && !local {
        slots = slots.min(usize::from(live.jobs == 0));
    }
    let mut work: Vec<Work> = Vec::new();
    let mut instant: Vec<(u8, u64)> = Vec::new();
    let mut batch: Vec<u64> = Vec::new();
    let mut bytes = 0usize;
    {
        let live: &Live = &live;
        let todo: Box<dyn Iterator<Item = u64> + '_> = if drafting {
            Box::new(live.drafts.range(base..t).copied())
        } else {
            Box::new(frontier..t)
        };
        for i in todo {
            if slots == 0 && batch.is_empty() {
                break;
            }
            let key = (0u8, i);
            if (!drafting && live.tree.built(0, i)) || !due(live, &key) {
                continue;
            }
            let u = &live.units[i as usize];
            let mech = live
                .failed
                .get(&key)
                .is_some_and(|r| r.mechanical(live.last_ok));
            if drafting && mech {
                // 201 P1: the unit's own call is over cost, or it keeps
                // failing hard: its draft stays until `endless_retry`.
                continue;
            }
            let fails = live.failed.get(&key).map_or(0, |r| r.failures);
            if !drafting && (mech || tree::free_unit(u, node_bytes).is_some()) {
                instant.push(key);
                continue;
            }
            // 201 P1: a draft that failed in a batch rejoins one (F8); only
            // a guard trip of its own batch line sends it alone.
            if (fails > 0 && !drafting)
                || live.solo.contains(&i)
                || batch_units == 1
                || u.size as usize > BATCH_BYTES
            {
                if slots > 0 {
                    slots -= 1;
                    work.push(Work::Node(key));
                }
                continue;
            }
            if batch.is_empty() {
                if slots == 0 {
                    continue;
                }
                slots -= 1;
            }
            batch.push(i);
            bytes += u.size as usize;
            if batch.len() >= batch_units || bytes >= BATCH_BYTES {
                work.extend(close(&mut batch));
                bytes = 0;
            }
        }
    }
    work.extend(close(&mut batch));
    // Merges: missing ones, and stale drafts (201 P1); a meeting tree's only
    // behind the frontier (137 rule 3).
    let mut merges = if local { usize::from(!merging) } else { slots };
    let mut l = 1u8;
    while (1u64 << l) <= t && (drafting || merges > 0) {
        let n = 1u64 << l;
        for i in 0..(t / n) {
            if merges == 0 && !drafting {
                break;
            }
            let key = (l, i);
            if i * n < base || !exists(l, i, t) || (!drafting && (i + 1) * n > frontier) {
                continue;
            }
            if live.tree.built(l, i) && !live.stale.contains(&key) {
                continue;
            }
            let (Some(a), Some(b)) = (live.tree.get(l - 1, 2 * i), live.tree.get(l - 1, 2 * i + 1))
            else {
                continue;
            };
            if !due(&live, &key) {
                continue;
            }
            let cheap = drafting
                && (a.quality() == Quality::Draft
                    || b.quality() == Quality::Draft
                    || tree::free_merge(&a.text, &b.text, node_bytes).is_some());
            if cheap {
                instant.push(key);
            } else if merges > 0 {
                merges -= 1;
                work.push(Work::Node(key));
            }
        }
        l += 1;
    }
    for key in instant {
        live.running.insert(key);
        spawn_build(state.clone(), key, &mut live, false);
    }
    for w in work {
        live.jobs += 1;
        match w {
            Work::Node(key) => {
                live.running.insert(key);
                spawn_build(state.clone(), key, &mut live, true);
            }
            Work::Batch(units) => {
                for i in &units {
                    live.running.insert((0, *i));
                }
                spawn_batch(state.clone(), units, &mut live);
            }
        }
    }
}

/// Release a job's keys (and its call slot when it held one).
fn release(live: &mut Live, keys: &[(u8, u64)], counted: bool) {
    for k in keys {
        live.running.remove(k);
    }
    if counted {
        live.jobs = live.jobs.saturating_sub(1);
    }
}

/// E10: a job task that ends without reaching [`finish`] releases its
/// `running` keys and counts as a failure of each, so they retry — unless
/// its calls were cancelled (a switch, a retry, a detach: R1-3).
struct JobGuard {
    state: Arc<State>,
    keys: Vec<(u8, u64)>,
    counted: bool,
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
            release(&mut live, &self.keys, self.counted);
            if !self.cancel.is_cancelled() {
                for key in &self.keys {
                    record_failure(
                        &self.state,
                        &mut live,
                        *key,
                        "compactor task ended unexpectedly",
                        &self.model,
                    );
                }
            }
            live.dirty = true;
        }
        self.state.built_tx.send_modify(|v| *v += 1);
        self.state.wake.notify_one();
    }
}

/// The `<chat>` context of a job whose last unit committed in `turn`: that
/// turn's frozen view (D6), else the live one — both under the G2 raw budget.
fn ctx_for(state: &State, live: &mut Live, turn: u32) -> String {
    match live.frozen.iter().find(|(t, _)| *t == turn) {
        Some((_, ctx)) => ctx.clone(),
        None => {
            let f = live.fold(live.fold_cfg(&state.cfg.read()));
            fold::render(&live.tree, &f, &live.units).text()
        }
    }
}

/// Assemble the step for `(l, i)` under the lock and run it on its own task.
/// `counted`: the job holds a call slot. A meeting tree's level-0 unit that
/// failed [`MECH_AFTER`] times folds mechanically (G7) instead.
fn spawn_build(state: Arc<State>, key: (u8, u64), live: &mut Live, counted: bool) {
    let (l, i) = key;
    let node_bytes = state.cfg.read().node_bytes;
    let (id, n) = id_n(l, i);
    let last_turn = live.unit_turn(id + n - 1);
    let free = |text: String| Node {
        l,
        i,
        size: text.len() as u32,
        text,
        free: true,
        model: None,
        tries: 0,
        mechanical: false,
    };
    let draft = |text: String| Node {
        l,
        i,
        size: text.len() as u32,
        text,
        free: false,
        model: None,
        tries: 0,
        mechanical: true,
    };
    // 201 P1: the deterministic merge stored when the model merge fails.
    let mut fallback: Option<Node> = None;
    let (ready, step) = if l == 0 {
        let u = &live.units[i as usize];
        if live
            .failed
            .get(&key)
            .is_some_and(|r| r.mechanical(live.last_ok))
        {
            let r = &live.failed[&key];
            tracing::info!(session = %state.session, i, failures = r.failures, hard = r.hard, error = %r.error, "endless: unit folded mechanically");
            (Some(mechanical_node(u, node_bytes)), None)
        } else {
            match tree::free_unit(u, node_bytes) {
                Some(text) => (Some(free(text)), None),
                None => (
                    None,
                    Some(Step::Unit {
                        kind: u.kind,
                        text: u.text.clone(),
                        earlier: live.earlier(last_turn, i),
                        // R1 Q3: a unit back from a batch trip aims at 0.8 × node_bytes.
                        aim: live.solo.contains(&i).then_some(node_bytes * 4 / 5),
                    }),
                ),
            }
        }
    } else {
        let child = |i: u64| live.tree.get(l - 1, i);
        let draft_child = live.drafting()
            && [child(2 * i), child(2 * i + 1)]
                .iter()
                .any(|c| c.is_some_and(|c| c.quality() == Quality::Draft));
        let a = child(2 * i).map(|n| n.text.clone()).unwrap_or_default();
        let b = child(2 * i + 1).map(|n| n.text.clone()).unwrap_or_default();
        if draft_child {
            // 201 P1: a merge over a draft is itself a deterministic draft.
            (Some(draft(tree::draft_merge(&a, &b, node_bytes))), None)
        } else {
            match tree::free_merge(&a, &b, node_bytes) {
                Some(text) => (Some(free(text)), None),
                None => {
                    if live.drafting() {
                        fallback = Some(draft(tree::draft_merge(&a, &b, node_bytes)));
                    }
                    (
                        None,
                        Some(Step::Merge {
                            a,
                            b,
                            earlier: live.earlier(last_turn, id),
                        }),
                    )
                }
            }
        }
    };
    let drafting = live.drafting();
    let ctx = if step.is_some() {
        ctx_for(&state, live, last_turn)
    } else {
        String::new()
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
            keys: vec![key],
            counted,
            model: compactor.model_id().to_owned(),
            cancel: cancel.clone(),
            armed: true,
        };
        let started = Instant::now();
        let outcome = match (ready, step) {
            (Some(node), _) => Ok(Some(node)),
            (None, Some(step)) => {
                let job = Job {
                    l,
                    i,
                    priority,
                    cancel: &cancel,
                    lines: 1,
                };
                call(&state, &compactor, job, &ctx, &step)
                    .await
                    .and_then(|b| match b {
                        // 201 P1 (F9): a line that failed its guards is never
                        // stored; the draft stays (a meeting keeps 137's
                        // shortest failing candidate).
                        Some(b) if b.tries < 0 && drafting => {
                            Err(format!("{NO_PASSING_LINE} after {} attempts", -b.tries))
                        }
                        b => Ok(b.map(|b| Node {
                            l,
                            i,
                            size: b.text.len() as u32,
                            text: b.text,
                            free: false,
                            model: Some(compactor.model_id().to_owned()),
                            tries: b.tries,
                            mechanical: false,
                        })),
                    })
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
            counted,
            fallback,
        )
        .await;
    });
}

/// G6: one call for the level-0 `units` (consecutive, unbuilt); each line
/// lands as its unit's node, a line that fails its guard sends its unit
/// back alone ([`Live::solo`]).
fn spawn_batch(state: Arc<State>, units: Vec<u64>, live: &mut Live) {
    let first = units[0];
    let last_turn = live.unit_turn(*units.last().expect("a batch has units"));
    let step = Step::Batch {
        units: units
            .iter()
            .map(|i| {
                let u = &live.units[*i as usize];
                (u.kind, u.text.clone())
            })
            .collect(),
        earlier: live.earlier(live.unit_turn(first), first),
    };
    let ctx = ctx_for(&state, live, last_turn);
    let (compactor, cancel) = {
        let cur = state.current.read();
        (cur.compactor.clone(), cur.cancel.clone())
    };
    let priority = u8::from(live.backfill().is_none());
    let keys: Vec<(u8, u64)> = units.iter().map(|i| (0, *i)).collect();
    tokio::spawn(async move {
        let mut guard = JobGuard {
            state: state.clone(),
            keys: keys.clone(),
            counted: true,
            model: compactor.model_id().to_owned(),
            cancel: cancel.clone(),
            armed: true,
        };
        let started = Instant::now();
        let job = Job {
            l: 0,
            i: first,
            priority,
            cancel: &cancel,
            lines: units.len() as u32,
        };
        let outcome = call(&state, &compactor, job, &ctx, &step).await;
        guard.armed = false;
        finish_batch(
            &state,
            &units,
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
    /// Cancelled by a switch, a retry, a detach or a pause.
    cancel: &'a CancellationToken,
    /// Reply lines (G6: a batch's unit count, else 1).
    lines: u32,
}

/// Prompt bytes one more try adds (the refused reply + the correction).
const RETRY_BYTES: usize = 3_072;

/// G3: how many bytes of the `<chat>` view `c` may see — at most
/// `compactor_ctx_bytes` (a local compactor: also `local_ctx_bytes`), and at
/// most what the G1 compactor target leaves beside the system prompt, the
/// step and the retries (quality: a quarter of the window; cost:
/// `max_usd_per_call`; latency: the measured local eval rate). The step
/// (the unit itself) is never trimmed.
fn ctx_budget(
    c: &dyn Compactor,
    step: &Step,
    cfg: &super::EndlessConfig,
    local: bool,
    reply_tokens: u32,
) -> usize {
    let mut max = cfg.compactor_ctx_bytes.clamp(8_000, 256_000) as usize;
    if local {
        max = max.min(cfg.local_ctx_bytes.clamp(4_000, 256_000) as usize);
    }
    let model = c.model();
    let fixed = compactor::COMPACT.len()
        + step.render(cfg.node_bytes).len()
        + usize::from(cfg.tries.max(1) - 1) * RETRY_BYTES;
    let b = crate::tokens::budget(
        crate::tokens::Kind::Compactor,
        model,
        crate::tokens::Limits {
            reply_tokens: u64::from(reply_tokens),
            max_usd_per_call: Some(cfg.max_usd_per_call),
            local_attempt_secs: local.then_some(u64::from(cfg.local_attempt_secs)),
            cap_bytes: None,
        },
    );
    max.min(crate::tokens::bytes_of(&model.id, b.target_tokens).saturating_sub(fixed))
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

/// G11: the session reached `max_usd_per_session_day` without a user
/// override for today — settling pauses (`paused_by: "spend"`).
async fn spend_capped(state: &State, cfg: &super::EndlessConfig) -> bool {
    let cap = cfg.max_usd_per_session_day;
    if cap <= 0.0 {
        state.live.lock().near_cap = false;
        return false;
    }
    let spent = match state
        .store
        .endless_spend_since(state.session, &utc_day_start())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(session = %state.session, error = %e, "endless: spend query");
            return false;
        }
    };
    // R1-4 / R2-3 / R2-9: once every slot's call could cross the cap, one call
    // at a time. The flag is set at call start from the settled spend, so one
    // tick may still start `jobs` calls: the overshoot is at most jobs × call.
    state.live.lock().near_cap = spent + f64::from(cfg.jobs.max(1)) * cfg.max_usd_per_call >= cap;
    if spent < cap {
        return false;
    }
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let overridden = state
        .store
        .endless_meta(state.session)
        .await
        .ok()
        .and_then(|m| m.spend_override)
        .is_some_and(|d| d == today);
    if overridden {
        return false;
    }
    if let Err(e) = state
        .store
        .set_endless_paused(state.session, Some("spend"), None)
        .await
    {
        tracing::warn!(session = %state.session, error = %e, "endless: persist the spend pause");
    }
    {
        let mut live = state.live.lock();
        live.paused_by = Some("spend".to_owned());
        live.dirty = true;
    }
    tracing::warn!(session = %state.session, spent, cap, "endless: day spend cap reached; settling paused (resume overrides it for today)");
    state.built_tx.send_modify(|v| *v += 1);
    true
}

/// One job's compactor build, with its `model_calls` row. `Ok(None)`: the
/// job's calls were cancelled (E3 switch, R1-9 detach, R1-10 retry, G8
/// pause) or the G11 day cap paused settling — not a failure, the node
/// reschedules. G11 / R1-3: a job whose largest single call alone costs
/// over `max_usd_per_call` is never sent ([`OVER_COST`]). G5: a
/// context-length refusal ("N tokens > M maximum", the provider's or the
/// G1 preflight's) retries once with the context cut to M/N × 0.85 of its
/// size — never at the same size (R1-6). R1-4: a failed call settles with
/// the usage of its answered attempts plus an estimate of the failed one.
async fn call(
    state: &State,
    c: &Arc<dyn Compactor>,
    job: Job<'_>,
    ctx: &str,
    step: &Step,
) -> Result<Option<compactor::Built>, String> {
    let Job {
        l,
        i,
        priority,
        cancel,
        lines,
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
    if spend_capped(state, &cfg).await {
        return Ok(None);
    }
    let model = c.model();
    let price = c.rates().map(|r| r.input);
    let largest = compactor::largest_call_bytes(step, cfg.node_bytes);
    if let Some(price) = price
        && crate::tokens::tokens_of(&model.id, largest) as f64 * price / 1e6 > cfg.max_usd_per_call
    {
        return Err(format!(
            "{OVER_COST}: one call's prompt alone is ≈ {} tokens at ${price}/M (max ${})",
            crate::tokens::tokens_of(&model.id, largest),
            cfg.max_usd_per_call
        ));
    }
    // E6: the attempt bound starts after the permit.
    let attempt = Duration::from_secs(u64::from(if local.is_some() {
        cfg.local_attempt_secs.clamp(30, 7_200)
    } else {
        cfg.attempt_secs.clamp(10, 3_600)
    }));
    let reply = c.out_tokens(lines);
    let ctx = fit_ctx(
        ctx,
        ctx_budget(c.as_ref(), step, &cfg, local.is_some(), reply),
    );
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
    // E10: the last-resort bound over the whole job (G4: every map chunk
    // and the reduce; G5: the one retry); a panic is a failure whose ledger
    // row still settles (R1-4).
    let parts = match step {
        Step::Unit { text, .. } if text.len() > compactor::MAP_REDUCE_OVER => {
            compactor::chunks(text, compactor::CHUNK_BYTES).len() as u32 + 1
        }
        _ => 1,
    };
    let outer = attempt * u32::from(cfg.tries.max(1)) * parts * 2 + Duration::from_secs(60);
    // R2-11: the answered attempts, kept even if the outer bound drops the build.
    let tally = compactor::Tally::new(c.as_ref());
    let run = async {
        let first = compactor::build(&tally, &ctx, step, cfg.node_bytes, cfg.tries, attempt).await;
        let Err(f) = &first else { return first };
        // G5: the provider's (or preflight's) counts; a shape without numbers
        // (Bedrock's "input is too long") halves the context.
        let ratio = match crate::compact::overflow_numbers(&f.error) {
            Some(n) if n.prompt > n.window => f64::from(n.window) / f64::from(n.prompt) * 0.85,
            Some(_) => return first,
            None if crate::compact::is_overflow(&f.error) => 0.5,
            None => return first,
        };
        let smaller = ((ctx.len() as f64) * ratio) as usize;
        let retry_ctx = fit_ctx(&ctx, smaller);
        // R1-6: never resend the same request.
        if retry_ctx.len() >= ctx.len() {
            return first;
        }
        tracing::warn!(session = %state.session, l, i, ctx = ctx.len(), retry_ctx = retry_ctx.len(), error = %f.error, "endless: context-length refusal; retrying once with a smaller context");
        let earlier = first.err().map(|f| f.calls).unwrap_or_default();
        match compactor::build(&tally, &retry_ctx, step, cfg.node_bytes, cfg.tries, attempt).await {
            Ok(mut b) => {
                b.calls.splice(0..0, earlier);
                Ok(b)
            }
            Err(mut f) => {
                f.calls.splice(0..0, earlier);
                Err(f)
            }
        }
    };
    let build = tokio::time::timeout(outer, std::panic::AssertUnwindSafe(run).catch_unwind());
    let built: Option<Result<compactor::Built, compactor::Failed>> = tokio::select! {
        b = build => Some(match b {
            Ok(Ok(built)) => built,
            Ok(Err(_panic)) => Err(compactor::Failed {
                error: "compactor panicked".to_owned(),
                calls: tally.rows(),
            }),
            Err(_) => Err(compactor::Failed {
                error: format!("compactor job timed out after {}s", outer.as_secs()),
                calls: tally.rows(),
            }),
        }),
        _ = cancel.cancelled() => None,
    };
    // A failure that raced the switch is the old model's: not counted either.
    let built = built.filter(|b| b.is_ok() || !cancel.is_cancelled());
    let (status, usage) = match &built {
        Some(Ok(b)) => (CallStatus::Settled, b.usage()),
        // R1-4 / R2-1: a failed call is charged its answered attempts' usage,
        // plus the failing attempt's prompt at the estimate only when theuth
        // dropped it after dispatch (the provider may bill it).
        Some(Err(f)) => {
            let mut u = compactor::sum_usage(&f.calls);
            if billed_drop(&f.error) {
                let prompt =
                    compactor::COMPACT.len() + ctx.len() + step.render(cfg.node_bytes).len();
                let est = crate::tokens::tokens_of(&model.id, prompt);
                let u = u.get_or_insert_default();
                u.prompt_tokens = Some(
                    u.prompt_tokens
                        .unwrap_or(0)
                        .saturating_add(i32::try_from(est).unwrap_or(i32::MAX)),
                );
            }
            (CallStatus::Failed, u)
        }
        None => (CallStatus::Cancelled, None),
    };
    let obs = compactor::observation(usage.as_ref(), model);
    if let Err(e) = state.store.settle_call(&reserved, status, obs).await {
        tracing::warn!(error = %e, "endless: settle_call");
    }
    let Some(built) = built else {
        return Ok(None);
    };
    let built = match built {
        Ok(b) => b,
        Err(f) => {
            if !f.calls.is_empty()
                && let Err(e) = state
                    .store
                    .endless_calls_put(state.session, l, i, &f.calls)
                    .await
            {
                tracing::warn!(error = %e, "endless: endless_calls");
            }
            return Err(f.error);
        }
    };
    if let Err(e) = state
        .store
        .endless_calls_put(state.session, l, i, &built.calls)
        .await
    {
        tracing::warn!(error = %e, "endless: endless_calls");
    }
    Ok(Some(built))
}

/// One more consecutive failure of `key`, with its backoff (caller holds
/// `live`). R1-2 / R2-2: only a [`hard_failure`] counts toward the
/// mechanical fold. R1-3 / G11: a level-0 unit whose own call is over
/// `max_usd_per_call` folds mechanically now, with one notice.
fn record_failure(state: &State, live: &mut Live, key: (u8, u64), e: &str, model: &str) {
    let base = state.cfg.read().retry_secs;
    let now = Instant::now();
    let prev = live.failed.get(&key);
    let failures = prev.map_or(0, |r| r.failures).saturating_add(1);
    let hard = prev.map_or(0, |r| r.hard) + u32::from(hard_failure(e));
    let since = prev.map_or(now, |r| r.since);
    let mut delay = retry_delay(base, failures);
    let prev_failures = failures - 1;
    let cost = key.0 == 0 && e.starts_with(OVER_COST);
    if cost {
        delay = Duration::ZERO;
        if !live.cost_noticed {
            live.cost_noticed = true;
            tracing::warn!(session = %state.session, i = key.1, error = %e, "endless: {COST_NOTICE}");
            live.pending_error = Some(COST_NOTICE.to_owned());
        }
    }
    live.failed.insert(
        key,
        Retry {
            at: now + delay,
            failures,
            hard,
            since,
            cost,
            error: one_line(e, 300),
            model: model.to_owned(),
        },
    );
    if live.reported.insert(key) && !e.starts_with(OVER_COST) {
        tracing::warn!(session = %state.session, l = key.0, i = key.1, error = %e, "endless: node failed (will retry)");
        live.pending_error = Some(format!(
            "{}+{}: {e}",
            id_n(key.0, key.1).0,
            id_n(key.0, key.1).1
        ));
    }
    if delay >= RETRY_CAP && (prev_failures == 0 || retry_delay(base, prev_failures) < RETRY_CAP) {
        tracing::warn!(session = %state.session, l = key.0, i = key.1, model, failures, error = %e, "endless: node keeps failing (retrying every 30 min)");
    }
}

/// Persist and publish one job's outcome (`Ok(None)`: its calls were
/// cancelled). A failure of a job whose `cancel` fired is the old
/// compactor's (or a retried / detached call's) and is dropped under the
/// `live` lock, so no stale backoff outlives a switch (R1-3). 201 P1: a
/// failed model merge stores its deterministic `fallback` as a draft and
/// stays marked for rebuilding after its backoff.
#[allow(clippy::too_many_arguments)]
async fn finish(
    state: &State,
    key: (u8, u64),
    outcome: Result<Option<Node>, String>,
    took: Duration,
    model: &str,
    cancel: &CancellationToken,
    counted: bool,
    fallback: Option<Node>,
) {
    let outcome = match outcome {
        Ok(Some(node)) => match state.store.endless_node_put(state.session, &node).await {
            Ok(()) => Ok(Some(node)),
            Err(e) => Err(format!("store: {e}")),
        },
        other => other,
    };
    let fallback = match (&outcome, fallback) {
        (Err(_), Some(node)) if !cancel.is_cancelled() => {
            match state.store.endless_node_put(state.session, &node).await {
                Ok(()) => Some(node),
                Err(e) => {
                    tracing::warn!(session = %state.session, error = %e, "endless: draft merge");
                    None
                }
            }
        }
        _ => None,
    };
    let mut live = state.live.lock();
    release(&mut live, &[key], counted);
    match outcome {
        Ok(Some(node)) => {
            if !node.free && !node.mechanical {
                live.last_ok = Some(Instant::now());
            }
            if node.l == 0 && !node.free && !node.mechanical {
                live.build_secs.0 += took.as_secs_f64();
                live.build_secs.1 += 1;
            }
            live.failed.remove(&key);
            live.solo.remove(&key.1);
            live.put(node);
        }
        Ok(None) => {}
        Err(_) if cancel.is_cancelled() => {}
        Err(e) => {
            record_failure(state, &mut live, key, &e, model);
            if let Some(node) = fallback {
                live.put(node);
                live.stale.insert(key);
            }
        }
    }
    live.dirty = true;
    drop(live);
    state.built_tx.send_modify(|v| *v += 1);
    state.wake.notify_one();
}

/// G6: a batch's outcome — every passing line lands as its unit's node (one
/// store transaction), a unit whose line tripped a guard goes solo, a failed
/// call is a failure of every unit (G7 counts it).
async fn finish_batch(
    state: &State,
    units: &[u64],
    outcome: Result<Option<compactor::Built>, String>,
    took: Duration,
    model: &str,
    cancel: &CancellationToken,
) {
    let keys: Vec<(u8, u64)> = units.iter().map(|i| (0, *i)).collect();
    let (nodes, solo, outcome) = match outcome {
        Ok(Some(b)) => {
            let mut nodes = Vec::new();
            let mut solo = Vec::new();
            for (i, line) in units.iter().zip(b.lines) {
                match line {
                    Ok(text) => nodes.push(Node {
                        l: 0,
                        i: *i,
                        size: text.len() as u32,
                        text,
                        free: false,
                        model: Some(model.to_owned()),
                        tries: 1,
                        mechanical: false,
                    }),
                    Err(trip) => solo.push((*i, trip)),
                }
            }
            match state.store.endless_nodes_put(state.session, &nodes).await {
                Ok(()) => (nodes, solo, Ok(true)),
                Err(e) => (Vec::new(), Vec::new(), Err(format!("store: {e}"))),
            }
        }
        Ok(None) => (Vec::new(), Vec::new(), Ok(false)),
        Err(e) => (Vec::new(), Vec::new(), Err(e)),
    };
    let drafts = state.live.lock().drafts.len();
    tracing::info!(session = %state.session, from = units[0], units = units.len(), built = nodes.len(), solo = solo.len(), drafts, secs = took.as_secs_f64(), "endless: batch");
    let mut live = state.live.lock();
    release(&mut live, &keys, true);
    match outcome {
        Ok(true) => {
            if !nodes.is_empty() {
                live.last_ok = Some(Instant::now());
                live.build_secs.0 += took.as_secs_f64();
                live.build_secs.1 += nodes.len() as u32;
            }
            for node in nodes {
                live.failed.remove(&(0, node.i));
                live.put(node);
            }
            for (i, trip) in solo {
                tracing::debug!(session = %state.session, i, guard = trip, "endless: batch line refused; the unit runs alone");
                live.solo.insert(i);
            }
            live.invalidate();
        }
        Ok(false) => {}
        Err(_) if cancel.is_cancelled() => {}
        // R1-3: a batch over `max_usd_per_call` is no failure of its units —
        // each runs alone (and folds only if its own call is over).
        Err(e) if e.starts_with(OVER_COST) => {
            for i in units {
                live.solo.insert(*i);
            }
        }
        Err(e) => {
            for key in &keys {
                record_failure(state, &mut live, *key, &e, model);
            }
        }
    }
    live.dirty = true;
    drop(live);
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
        backlog: live.backlog(cfg),
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

/// 201 P1 (D5): no turn waits on the summary model — every unit has a line
/// from its ingest on. Ingests (no model call: new units get their drafts),
/// wakes the pump and returns `true`; `_max` (the old `settle_secs`) is
/// accepted and ignored.
pub(crate) async fn settle(inner: &Inner, session: SessionId, _max: Duration) -> bool {
    if let Some(state) = state_of(inner, session) {
        if let Err(e) = ingest(&state).await {
            tracing::warn!(%session, error = %e, "endless: ingest before a turn");
        }
        state.wake.notify_one();
    }
    true
}

/// Test seam (the 137 `settle` wait, for tests that need model lines):
/// wait, at most `max`, until every view part is built and, in a drafting
/// session, every draft refined (or given up) and no draft merge stale.
/// `false` when a node keeps failing (≥ 2 in a row), when paused, or at `max`.
#[cfg(test)]
pub(crate) async fn quiet_for_tests(inner: &Inner, session: SessionId, max: Duration) -> bool {
    let Some(state) = state_of(inner, session) else {
        return true;
    };
    if let Err(e) = ingest(&state).await {
        tracing::warn!(%session, error = %e, "endless: ingest before quiet");
    }
    state.wake.notify_one();
    let mut rx = state.built_tx.subscribe();
    let deadline = Instant::now() + max;
    loop {
        let (done, stalled) = {
            let mut live = state.live.lock();
            let cfg = live.fold_cfg(&state.cfg.read());
            let unbuilt = live.fold(cfg).unbuilt(&live.tree);
            let last_ok = live.last_ok;
            let given_up = |i: &u64| {
                live.failed
                    .get(&(0, *i))
                    .is_some_and(|r| r.mechanical(last_ok))
            };
            let drafts = live.drafts.iter().filter(|i| !given_up(i)).count();
            let failing = live.failed.iter().any(|(k, r)| {
                r.failures >= 2 && (k.0 > 0 || (live.drafts.contains(&k.1) && !given_up(&k.1)))
            });
            (
                unbuilt == 0 && drafts == 0 && live.stale.is_empty(),
                live.frontier_stalled(cfg) || (live.drafting() && failing),
            )
        };
        if done {
            return true;
        }
        if stalled || state.live.lock().paused_by.is_some() {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || tokio::time::timeout(left, rx.changed()).await.is_err() {
            return false;
        }
    }
}

/// The turn's view (8.1), frozen as this turn's `<chat>` context (D6).
/// 137G G1: `max_bytes` is what the turn's budget leaves for the view; when
/// `view_bytes + raw_bytes` exceed it, both scale down by the same factor
/// (the live fold is untouched). `again` (G5: the turn's request was
/// refused as too long) re-renders this turn's view instead of starting a
/// new turn.
pub(crate) async fn render_for_turn(
    inner: &Inner,
    session: SessionId,
    max_bytes: Option<usize>,
    again: bool,
) -> anyhow::Result<ViewRender> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} does not use fractal context");
    };
    ingest(&state).await?;
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    let render = render_within(&mut live, cfg, max_bytes, session);
    if again && let Some(last) = live.frozen.back_mut() {
        last.1 = render.text();
    } else {
        live.turn += 1;
        let turn = live.turn;
        live.frozen.push_back((turn, render.text()));
        while live.frozen.len() > FROZEN_KEEP {
            live.frozen.pop_front();
        }
    }
    Ok(render)
}

/// The view under `cfg`, scaled down (view and raw budgets by one factor)
/// when its hard cap `high() + raw_bytes` exceeds `max_bytes` — a scaled
/// fold is rebuilt for that render, the live fold is untouched (G1, R1-7).
/// 201 P1: drafts are built lines, so a view whose merges are not built
/// yet folds to that cap (not to `raw_bytes`); the slack is budgeted too.
fn render_within(
    live: &mut Live,
    cfg: FoldCfg,
    max_bytes: Option<usize>,
    session: SessionId,
) -> ViewRender {
    let want = cfg.high() + cfg.raw_bytes as usize;
    let Some(m) = max_bytes.filter(|m| *m < want) else {
        let f = live.fold(cfg);
        return fold::render(&live.tree, &f, &live.units);
    };
    // The fold sizes lines without their `id+n|` prefix and newline, so a
    // render can overshoot `m` by that overhead: shrink by the overshoot and
    // render again (at most twice more).
    let mut scale = m as f64 / want as f64;
    let mut attempt = 0;
    loop {
        let small = FoldCfg {
            view_bytes: ((f64::from(cfg.view_bytes) * scale) as u32).max(2_000),
            raw_bytes: ((f64::from(cfg.raw_bytes) * scale) as u32).max(1_000),
            slack: cfg.slack,
        };
        let f = Fold::rebuild(
            &live.tree,
            live.base.min(live.t()),
            live.t(),
            small,
            &live.units,
        );
        let r = fold::render(&live.tree, &f, &live.units);
        let bytes = r.text().len();
        attempt += 1;
        if bytes <= m || attempt == 3 {
            tracing::info!(%session, max_bytes = m, view_bytes = small.view_bytes, raw_bytes = small.raw_bytes, bytes, folded = r.folded, "endless: view scaled down to the budget");
            return r;
        }
        scale *= m as f64 / bytes as f64 * 0.98;
    }
}

/// 201 §3 (P2): one generation — the view rendered once at a trim over the
/// units before the tail (`[base, t_cut)`) and reused byte for byte by every
/// turn until the next trim, and after a restart (persisted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    /// 1 = the session's first.
    pub n: u32,
    /// First unit the view does not cover.
    pub t_cut: u64,
    /// First `messages.seq` still in the tail.
    pub first_seq: i64,
    /// The frozen view.
    pub render: ViewRender,
}

impl Generation {
    fn of_row(r: crate::store::endless::GenerationRow) -> Self {
        Self {
            n: r.n,
            t_cut: r.t_cut,
            first_seq: r.first_seq,
            render: ViewRender {
                blocks: r
                    .blocks
                    .into_iter()
                    .map(|(text, mark)| ViewBlock { text, mark })
                    .collect(),
                settled: true,
                unbuilt: 0,
                folded: 0,
            },
        }
    }

    fn row(&self) -> crate::store::endless::GenerationRow {
        crate::store::endless::GenerationRow {
            n: self.n,
            t_cut: self.t_cut,
            first_seq: self.first_seq,
            blocks: self
                .render
                .blocks
                .iter()
                .map(|b| (b.text.clone(), b.mark))
                .collect(),
        }
    }
}

/// 201 §3 (P2): the generation in force (the cached one, else the stored
/// row); `None` for a session that never started one.
pub(crate) async fn generation(
    inner: &Inner,
    session: SessionId,
) -> anyhow::Result<Option<Generation>> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} does not use fractal context");
    };
    if let Some(g) = state.live.lock().current_gen.clone() {
        return Ok(Some(g));
    }
    let Some(row) = state.store.endless_generation(session).await? else {
        return Ok(None);
    };
    let g = Generation::of_row(row);
    state.live.lock().current_gen = Some(g.clone());
    Ok(Some(g))
}

/// 201 §4.3 (P2): start a generation whose tail begins at message
/// `first_seq` (`None`: after every row ingested so far — an empty tail).
/// The fold runs here and only here: the view is rendered over the units
/// before the tail, fitted to `max_bytes`, persisted and frozen. `again`
/// (137G G5, a refused request) re-renders the current generation smaller
/// under the same number instead of opening the next one.
pub(crate) async fn start_generation(
    inner: &Inner,
    session: SessionId,
    first_seq: Option<i64>,
    max_bytes: usize,
    again: bool,
) -> anyhow::Result<Generation> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} does not use fractal context");
    };
    ingest(&state).await?;
    let prev = generation(inner, session).await?.map_or(0, |g| g.n);
    let g = {
        let mut live = state.live.lock();
        let first_seq = first_seq.unwrap_or(live.last_seq + 1);
        let t_cut = live.units.partition_point(|u| u.seq < first_seq) as u64;
        let cfg = live.fold_cfg(&state.cfg.read());
        let render = render_upto(&mut live, cfg, t_cut, Some(max_bytes), session);
        Generation {
            n: if again { prev.max(1) } else { prev + 1 },
            t_cut,
            first_seq,
            render,
        }
    };
    state
        .store
        .endless_generation_put(session, &g.row())
        .await?;
    state.live.lock().current_gen = Some(g.clone());
    Ok(g)
}

/// 201 §6 (P2): a turn of the current generation opens — the compactor
/// jobs of its units get the generation's frozen view as `<chat>` context
/// (the `frozen` cache stays keyed by turn; every turn of a generation
/// carries the same bytes). Records the status numbers.
pub(crate) fn turn_opened(inner: &Inner, session: SessionId, tail_bytes: u32, allowance: u64) {
    let Some(state) = state_of(inner, session) else {
        return;
    };
    let mut live = state.live.lock();
    let text = live
        .current_gen
        .as_ref()
        .map(|g| g.render.text())
        .unwrap_or_default();
    live.turn += 1;
    let turn = live.turn;
    live.frozen.push_back((turn, text));
    while live.frozen.len() > FROZEN_KEEP {
        live.frozen.pop_front();
    }
    live.tail_bytes = tail_bytes;
    live.allowance = Some(allowance);
}

/// [`render_within`] over `[base, t_cut)` only (201 §4.3: the view stops
/// where the tail starts). `t_cut ≥ T` is the live view.
fn render_upto(
    live: &mut Live,
    cfg: FoldCfg,
    t_cut: u64,
    max_bytes: Option<usize>,
    session: SessionId,
) -> ViewRender {
    if t_cut >= live.t() {
        return render_within(live, cfg, max_bytes, session);
    }
    let want = cfg.high() + cfg.raw_bytes as usize;
    let m = max_bytes.unwrap_or(usize::MAX);
    let mut scale = (m as f64 / want as f64).min(1.0);
    let mut attempt = 0;
    loop {
        let c = FoldCfg {
            view_bytes: ((f64::from(cfg.view_bytes) * scale) as u32).max(2_000),
            raw_bytes: ((f64::from(cfg.raw_bytes) * scale) as u32).max(1_000),
            slack: cfg.slack,
        };
        let f = Fold::rebuild(&live.tree, live.base.min(t_cut), t_cut, c, &live.units);
        let r = fold::render(&live.tree, &f, &live.units);
        let bytes = r.text().len();
        attempt += 1;
        if bytes <= m || attempt == 3 {
            return r;
        }
        scale *= m as f64 / bytes as f64 * 0.98;
    }
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

/// 201 §4.2: persist the session's tail override (`None` clears it) and
/// carry it on the live state. Takes effect when phase P2 reads it.
pub(crate) async fn set_tail(
    inner: &Inner,
    session: SessionId,
    tail: Option<TailOverride>,
) -> anyhow::Result<()> {
    inner
        .store_for(session)
        .set_endless_tail(session, tail)
        .await?;
    if let Some(state) = state_of(inner, session) {
        state.live.lock().tail = tail;
    }
    Ok(())
}

/// 201 §4.2: the session model's turn allowance in bytes, which sizes a
/// `scaled` tail — as the session's last turn measured it (`None` before one).
fn turn_allowance_bytes(inner: &Inner, session: SessionId) -> Option<u64> {
    state_of(inner, session).and_then(|s| s.live.lock().allowance)
}

/// 201 §12.2: the tail and profile fields of [`SessionMemory`] — the mode
/// and marks in force after the session's override. `tail_bytes`,
/// `generation` (P2), `drafts` (P1) stay 0 and `profile` `standard` (P3)
/// until those phases fill them.
fn tail_status(
    m: SessionMemory,
    cfg: &EndlessConfig,
    tail: Option<TailOverride>,
    allowance: Option<u64>,
) -> SessionMemory {
    let c = cfg.with_tail(tail.as_ref());
    let (tail_low, tail_high) = match (c.tail_mode, allowance) {
        (TailMode::Fixed, a) => c.tail_marks(a.unwrap_or(u64::MAX)),
        (TailMode::Scaled, Some(a)) => c.tail_marks(a),
        (TailMode::Scaled, None) => (0, 0),
    };
    SessionMemory {
        tail_mode: c.tail_mode,
        tail_low,
        tail_high,
        tail_override: tail,
        profile: Profile::Standard,
        ..m
    }
}

/// R1-D1: the view rendered now under the session's budget, without
/// freezing a context or counting a turn (a native child's first message).
pub(crate) async fn render_snapshot(
    inner: &Inner,
    session: SessionId,
) -> anyhow::Result<Vec<ViewBlock>> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} does not use fractal context");
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
        anyhow::bail!("session {session} does not use fractal context");
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
        return Err("this session does not use fractal context".into());
    };
    let mut live = state.live.lock();
    // R2-6 / R2-10: a folded run of several units is no node even when its
    // range is aligned; a lone unit (n = 1) still zooms to its raw text.
    let cfg = live.fold_cfg(&state.cfg.read());
    if n > 1 && live.fold(cfg).is_folded_line(id, n) {
        return Err(format!(
            "{id}+{n} is a folded run, not a summary line; recall searches it."
        ));
    }
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
        return Err("this session does not use fractal context".into());
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
        return Err("this session does not use fractal context".into());
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
    max_bytes: Option<usize>,
) -> anyhow::Result<SetMode> {
    let store = inner.store_for(session);
    match mode {
        MemoryMode::Endless => {
            // R1-D4 / R1-D6: filter and base are fixed when the tree is first started.
            let meta = store.endless_meta(session).await?;
            // 201 §12: switching on starts generation 1 again (empty tail).
            if meta.mode != MemoryMode::Endless {
                store.endless_generations_clear(session).await?;
                if let Some(state) = state_of(inner, session) {
                    state.live.lock().current_gen = None;
                }
            }
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
                // 201 P1: a session switched (back) on gets its drafts now.
                fill_drafts(&state, 0).await?;
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
                    // G9 / R1-7: the head fits the session model's turn budget too.
                    let view = render_within(&mut live, cfg, max_bytes, session).text();
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
        let tail = store.endless_meta(session).await?.tail;
        let cfg = inner.cfg.read().clone();
        return Ok(tail_status(
            SessionMemory {
                mode,
                view_bytes: cfg.view_bytes,
                ..Default::default()
            },
            &cfg,
            tail,
            turn_allowance_bytes(inner, session),
        ));
    };
    let stats = store.endless_stats(session).await?;
    let spend_today_usd = store.endless_spend_since(session, &utc_day_start()).await?;
    let mut live = state.live.lock();
    let cfg = live.fold_cfg(&state.cfg.read());
    let f = live.fold(cfg);
    let view = if mode == MemoryMode::Endless {
        fold::render_lines(&live.tree, &f, &live.units)
    } else {
        Vec::new()
    };
    let unbuilt = f.unbuilt(&live.tree);
    let full_cfg = state.cfg.read().clone();
    let m = SessionMemory {
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
            live.backlog(cfg)
        } else {
            None
        },
        model_override: live.picks.chat.clone(),
        model_source: if mode == MemoryMode::Endless {
            chosen(inner, &live.picks).1.to_owned()
        } else {
            String::new()
        },
        effort: usable_effort(state.compactor().model(), chosen(inner, &live.picks).2),
        effort_override: live.picks.chat_effort.clone(),
        spend_today_usd,
        drafts: u32::try_from(live.drafts.len()).unwrap_or(u32::MAX),
        tail_bytes: live.tail_bytes,
        generation: live.current_gen.as_ref().map_or(0, |g| g.n),
        ..Default::default()
    };
    let tail = live.tail;
    drop(live);
    Ok(tail_status(
        m,
        &full_cfg,
        tail,
        turn_allowance_bytes(inner, session),
    ))
}

/// RFC 3339 start of the current UTC day (`model_calls.created_at` is UTC RFC 3339).
fn utc_day_start() -> String {
    format!("{}T00:00:00+00:00", Utc::now().format("%Y-%m-%d"))
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
        let nodes: Vec<Node> = (live.base.min(live.t())..live.t())
            .filter(|i| !live.tree.built(0, *i))
            .map(|i| mechanical_node(&live.units[i as usize], node_bytes))
            .collect();
        // 201 P1: a drafting session has nothing to fold (every unit has
        // its draft); its refinement calls go on.
        if !nodes.is_empty() && live.running.iter().any(|k| k.0 == 0) {
            state.cancel_calls();
        }
        nodes
    };
    // R1-8: a model line that landed since the snapshot is kept, never overwritten.
    state.store.endless_nodes_put_new(session, &nodes).await?;
    let n = nodes.len();
    {
        let mut live = state.live.lock();
        for node in nodes {
            if !live.tree.built(0, node.i) {
                live.failed.remove(&(0, node.i));
                live.reported.remove(&(0, node.i));
                live.put(node);
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

/// 201 P1: the deterministic line a level-0 unit that does not fit
/// `node_bytes` gets at ingest (stored as a `draft`, refined later by the
/// model). For now the 137G mechanical cut; phase P3 replaces this body
/// with the per-kind lines of 201 §7.3.
fn draft_line(u: &Unit, node_bytes: u32) -> String {
    tree::mechanical_unit(u, node_bytes).unwrap_or_else(|| u.tagged())
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
    let meta = store.endless_meta(session).await?;
    if meta.mode != MemoryMode::Endless {
        return memory(inner, session).await;
    }
    let by = paused.then_some("user");
    // G11: resuming a spend pause is the user's override for the rest of the UTC day.
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let override_day = (!paused && meta.paused_by.as_deref() == Some("spend")).then_some(today);
    store
        .set_endless_paused(session, by, override_day.as_deref())
        .await?;
    if let Some(state) = state_of(inner, session) {
        {
            let mut live = state.live.lock();
            live.paused_by = by.map(str::to_owned);
            live.dirty = true;
        }
        if paused {
            state.cancel_calls();
        }
        tracing::info!(%session, paused, spend_override = ?override_day, "endless: settling paused/resumed");
        state.built_tx.send_modify(|v| *v += 1);
        state.wake.notify_one();
    }
    memory(inner, session).await
}

/// RPC `set_session_compactor_model` (`agent = false`) and the roster's
/// desk default (`agent = true`), 137G: an id must resolve (`None` clears);
/// persisted, then live — re-resolve, cancel the old model's calls, reset
/// backoff, kick the pump (137F E2). 194: the agent link carries
/// `agent_effort` (its `memory_effort`; cleared with the model); a chat's
/// own effort is cleared with its model (D4d) and dropped when the new model
/// does not take it (D4c).
pub(crate) async fn set_model(
    inner: &Inner,
    session: SessionId,
    model: Option<String>,
    agent: bool,
    agent_effort: Option<String>,
) -> anyhow::Result<SessionMemory> {
    let model = model.map(|m| m.trim().to_owned()).filter(|m| !m.is_empty());
    // The chat's pick must resolve to a model this engine can reach: a
    // configured gateway, or a catalog model of a configured provider (the
    // catalog itself accepts any `provider/model` id). The agent default
    // comes from its file and falls back at run time like `agent.endless.model`.
    if let Some(id) = &model
        && !agent
        && inner.compactor_for_tests.is_none()
    {
        let configured = crate::supervisor::configured(inner.auth.clone());
        let m = inner
            .catalog
            .read()
            .resolve(id, &*configured)
            .map_err(|e| anyhow::anyhow!("compactor model {id:?}: {e}"))?;
        let reachable = crate::backend::target_for(&m, &inner.auth).is_some()
            || (m.model.is_some()
                && m.provider
                    .as_deref()
                    .is_some_and(|p| configured(models::Catalog::adapter(p))));
        anyhow::ensure!(
            reachable,
            "compactor model {id:?}: no configured provider serves it"
        );
    }
    let store = inner.store_for(session);
    let effort = if agent {
        model.as_ref().and(
            agent_effort
                .map(|e| e.trim().to_owned())
                .filter(|e| !e.is_empty()),
        )
    } else {
        let own = store.endless_meta(session).await?.effort_override;
        own.filter(|e| {
            model
                .as_deref()
                .is_some_and(|id| check_effort_for(inner, id, e).is_ok())
        })
    };
    store
        .set_endless_model(session, model.as_deref(), effort.as_deref(), agent)
        .await?;
    if let Some(state) = state_of(inner, session) {
        {
            let mut live = state.live.lock();
            let picks = &mut live.picks;
            if agent {
                picks.agent = model.clone();
                picks.agent_effort = effort.clone();
            } else {
                picks.chat = model.clone();
                picks.chat_effort = effort.clone();
            }
            live.dirty = true;
        }
        reresolve(inner, Some(session));
    }
    tracing::info!(%session, model = ?model, effort = ?effort, agent, "endless: compactor pick set");
    memory(inner, session).await
}

/// RPC `set_session_compactor_effort` (194): this chat's own compactor
/// effort (`None` clears = the choosing link's), checked against the model
/// in force (D4b); persisted in `sessions.endless_effort`; live.
pub(crate) async fn set_effort(
    inner: &Inner,
    session: SessionId,
    effort: Option<String>,
) -> anyhow::Result<SessionMemory> {
    let effort = effort
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty());
    let store = inner.store_for(session);
    if let Some(e) = &effort {
        let checked = match state_of(inner, session) {
            Some(state) => check_effort(state.compactor().model().model.as_ref(), e),
            None => {
                let meta = store.endless_meta(session).await?;
                let id = match chosen(inner, &Picks::of(&meta)).0 {
                    Some(id) => id,
                    None => store
                        .session(session)
                        .await?
                        .map(|row| row.model)
                        .unwrap_or_default(),
                };
                check_effort_for(inner, &id, e)
            }
        };
        checked.map_err(|why| anyhow::anyhow!(why))?;
    }
    store
        .set_endless_effort(session, effort.as_deref(), false)
        .await?;
    if let Some(state) = state_of(inner, session) {
        {
            let mut live = state.live.lock();
            live.picks.chat_effort = effort.clone();
            live.dirty = true;
        }
        reresolve(inner, Some(session));
    }
    tracing::info!(%session, effort = ?effort, "endless: compactor effort set");
    memory(inner, session).await
}

/// 194 D4b: [`check_effort`] against the catalog entry `id` resolves to.
fn check_effort_for(inner: &Inner, id: &str, effort: &str) -> Result<(), String> {
    let m = inner
        .catalog
        .read()
        .resolve(id, &*crate::supervisor::configured(inner.auth.clone()))
        .ok();
    check_effort(m.as_ref().and_then(|m| m.model.as_ref()), effort)
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

/// Test seam (194): the key of the session's compactor in force — its model
/// id, the effort its requests carry and whether that is the user's pick
/// (`{id}|{effort:?}|{chosen}|{limits:?}`).
#[cfg(test)]
pub(crate) fn compactor_key_for_tests(inner: &Inner, session: SessionId) -> String {
    let state = state_of(inner, session).expect("attached");
    state.current.read().key.clone()
}

/// Test seam (194 R1): the session's compactor in force.
#[cfg(test)]
pub(crate) fn compactor_in_force_for_tests(
    inner: &Inner,
    session: SessionId,
) -> Arc<dyn Compactor> {
    state_of(inner, session).expect("attached").compactor()
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

/// Test seam (R1-2): pretend every failure run of the session began `by` earlier.
#[cfg(test)]
pub(crate) fn age_failures_for_tests(inner: &Inner, session: SessionId, by: Duration) {
    let state = state_of(inner, session).expect("attached");
    let mut live = state.live.lock();
    for r in live.failed.values_mut() {
        r.since = r.since.checked_sub(by).unwrap_or(r.since);
    }
    drop(live);
    state.wake.notify_one();
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
        false,
        None,
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

    /// R2-1 / R2-2: which failures count toward the fold and which are charged.
    #[test]
    fn failure_kinds_for_the_fold_and_the_ledger() {
        for hard in [
            "compactor returned nothing",
            "timed out after 120s",
            "compactor job timed out after 900s",
            "stream idle for 120s",
            "compactor panicked",
            "400: model not loaded",
            "401 Unauthorized: invalid x-api-key",
        ] {
            assert!(hard_failure(hard), "{hard}");
        }
        for soft in [
            "529 Overloaded: overloaded_error, try again later",
            "429 Too Many Requests: rate limited",
        ] {
            assert!(!hard_failure(soft), "{soft}");
        }
        assert!(billed_drop("timed out after 120s") && billed_drop("stream idle for 120s"));
        for unbilled in [
            "529 Overloaded: overloaded_error, try again later",
            "401 Unauthorized: invalid x-api-key",
            "prompt is too long: 91610 tokens > 87308 maximum (theuth preflight, not sent: …)",
            "error sending request: connection refused",
        ] {
            assert!(!billed_drop(unbilled), "{unbilled}");
        }
    }

    /// 137G G3 (and 137F E8): the compactor's `<chat>` is capped at
    /// `compactor_ctx_bytes` for every model (an unknown window included), a
    /// local one also at `local_ctx_bytes`; a known window and the per-call
    /// cost shrink it further through the G1 target; the newest lines are kept whole.
    #[test]
    fn prompt_view_fits_the_compactor() {
        let cfg = super::super::EndlessConfig {
            local_ctx_bytes: 4_000,
            tries: 1,
            ..Default::default()
        };
        let step = Step::Unit {
            kind: tree::Kind::User,
            text: "x".repeat(2_000),
            earlier: vec![],
            aim: None,
        };
        let fixed = compactor::COMPACT.len() + step.render(cfg.node_bytes).len();
        let ctx = view(2_000);
        let local = Sized(resolved_for_tests("ollama/q"));
        let max = ctx_budget(&local, &step, &cfg, true, 600);
        assert_eq!(max, 4_000);
        let fit = fit_ctx(&ctx, max);
        assert!(fit.len() <= 4_000, "{}", fit.len());
        assert!(
            fit.starts_with("<chat>\nline ") && fit.ends_with("line 01999 of the view ✓\n</chat>")
        );
        assert!(fit.len() > 3_900, "only whole lines dropped: {}", fit.len());
        assert_eq!(fit_ctx(&ctx, usize::MAX), ctx, "under budget: untouched");

        let unknown = Sized(resolved_for_tests("p/unknown-window"));
        assert_eq!(
            ctx_budget(&unknown, &step, &cfg, false, 600),
            48_000,
            "no window known: compactor_ctx_bytes"
        );

        let model = |id: &str, context: u32, input: f64| {
            let m: models::Model = serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "limit": {"context": context, "output": 4096},
                "cost": {"input": input, "output": input * 5.0}
            }))
            .unwrap();
            Sized(models::Resolved {
                model: Some(m),
                ..resolved_for_tests(id)
            })
        };
        // 32k window: a quarter of it for the whole prompt.
        let small = model("p/32k", 32_000, 1.0);
        let max = ctx_budget(&small, &step, &cfg, false, 600);
        let tokens = crate::tokens::tokens_of("p/32k", fixed + max);
        assert!(tokens <= 8_000 && tokens > 7_900, "{tokens}");
        // An expensive model: $0.10 a call at $15/M input = 6,666 tokens of prompt.
        let pricey = model("p/pricey", 1_000_000, 15.0);
        let max = ctx_budget(&pricey, &step, &cfg, false, 600);
        let tokens = crate::tokens::tokens_of("p/pricey", fixed + max);
        assert!(tokens <= 6_667 && tokens > 6_500, "{tokens}");
    }

    /// 137G G3: `earlier` carries the newest 24 lines of the turn, oldest first.
    #[test]
    fn earlier_keeps_the_newest_24_lines() {
        let mut live = Live::new(Default::default());
        for i in 0..100u64 {
            live.units.push(Unit {
                i,
                seq: i as i64,
                part: 0,
                kind: tree::Kind::User,
                text: format!("u{i}"),
                size: 5,
                at: None,
            });
            live.tree.insert(Node {
                l: 0,
                i,
                text: format!("line {i}"),
                size: 6,
                free: true,
                model: None,
                tries: 0,
                mechanical: false,
            });
        }
        let e = live.earlier(0, 100);
        assert_eq!(e.len(), EARLIER_MAX);
        assert_eq!((e[0].as_str(), e[23].as_str()), ("line 76", "line 99"));
        assert_eq!(live.earlier(0, 5).len(), 5);
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
