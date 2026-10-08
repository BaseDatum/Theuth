//! 137 §5.2: the per-session pump — derives units from committed rows,
//! builds nodes in rule-3 order with ≤ `jobs` compactor calls in flight,
//! retries failures, keeps the fold fitted and reports `EndlessProgress`.
//! Also the home of the §9 tools and 8.6 `set_mode`, which read the same
//! live state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use runtime::SessionId;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use super::compactor::{self, Compactor, ModelCompactor, Step};
use super::fold::{self, Fold, FoldCfg, Part, ViewRender};
use super::tree::{self, Node, Tree, Unit, exists, id_n};
use super::{
    Attached, Backfill, EndlessProgress, Inner, MemoryMode, SessionMemory, SetMode, ViewLine,
    units,
};
use crate::backend::Backend;
use crate::cost::{CallKind, CallStatus};
use crate::store::Store;
use crate::store::endless::UnitRow;

/// Frozen `<chat>` contexts kept per session (D6; older ones fall back to the live view).
const FROZEN_KEEP: usize = 8;
/// Progress events: at most one per this interval per session.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// Seconds per level-0 unit before any measurement (ETA, D14).
const DEFAULT_UNIT_SECS: f64 = 4.0;

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
        live.mode == MemoryMode::Endless && live.backfill().is_none()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Shared between the handle, the pump task and its jobs.
pub(crate) struct State {
    session: SessionId,
    store: Store,
    rt: runtime::Runtime,
    cfg: super::EndlessConfig,
    compactor: Arc<dyn Compactor>,
    live: Mutex<Live>,
    /// Pump loop wake-up (new rows, a finished job, a mode switch).
    wake: Notify,
    /// Bumped whenever a node lands; `settle` waits on it.
    built_tx: watch::Sender<u64>,
    /// Serialises row ingestion between the pump and the turn (`settle`, `render_for_turn`).
    ingest: tokio::sync::Mutex<()>,
}

impl State {
    fn new(
        session: SessionId,
        store: Store,
        inner: &Inner,
        compactor: Arc<dyn Compactor>,
        mode: MemoryMode,
    ) -> Self {
        Self {
            session,
            store,
            rt: inner.rt.clone(),
            cfg: inner.cfg.clone(),
            compactor,
            live: Mutex::new(Live::new(mode)),
            wake: Notify::new(),
            built_tx: watch::channel(0).0,
            ingest: tokio::sync::Mutex::new(()),
        }
    }
}

struct Live {
    mode: MemoryMode,
    /// Every unit, dense by `i`.
    // ponytail: whole-session unit text in memory; page from the store if a session outgrows RAM.
    units: Vec<Unit>,
    tree: Tree,
    /// `Fold::rebuild(tree, T)` cache; `None` after the tree or `T` changed.
    fold: Option<Fold>,
    running: HashSet<(u8, u64)>,
    /// Failed nodes and when they may retry.
    failed: HashMap<(u8, u64), Instant>,
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
        if self.fold.is_none() {
            self.fold = Some(Fold::rebuild(&self.tree, self.t(), cfg, &self.units));
        }
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
        if total == 0 {
            return None;
        }
        let done = (0..total).filter(|i| self.tree.built(0, *i)).count() as u64;
        if done >= total {
            return None;
        }
        let per = if self.build_secs.1 > 0 {
            self.build_secs.0 / f64::from(self.build_secs.1)
        } else {
            DEFAULT_UNIT_SECS
        };
        Some(Backfill {
            done,
            total,
            eta_secs: ((total - done) as f64 * per) as u64,
        })
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
}

impl Live {
    fn new(mode: MemoryMode) -> Self {
        Self {
            mode,
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
            last_progress: None,
            dirty: false,
            pending_error: None,
            turns: HashMap::new(),
        }
    }
}

/// The compactor for a session (D8): `agent.endless.model` when set, else the
/// session's own model with the session's effort.
fn compactor_for(inner: &Inner, session: SessionId, attached: &Attached) -> ModelCompactor {
    let (model, effort) = match &inner.cfg.model {
        Some(id) => match inner
            .catalog
            .resolve(id, &*crate::supervisor::configured(inner.auth.clone()))
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
    ModelCompactor {
        session,
        backend: Backend::for_model(&model, &inner.client, &inner.auth),
        model,
        system: compactor::prompt(&attached.agent),
        effort,
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
        let model = match inner
            .catalog
            .resolve(&row.model, &*crate::supervisor::configured(inner.auth.clone()))
        {
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

pub(crate) async fn attach(inner: &Inner, session: SessionId, attached: Attached) -> anyhow::Result<()> {
    let store = attached.store.clone().unwrap_or_else(|| inner.store.clone());
    let mode = store.session_memory(session).await?;
    let existing = inner.sessions.lock().get(&session).map(|h| h.state.clone());
    let state = match existing {
        Some(state) => {
            state.live.lock().mode = mode;
            state
        }
        None => {
            let compactor: Arc<dyn Compactor> = match inner.compactor_for_tests.clone() {
                Some(c) => c,
                None => Arc::new(compactor_for(inner, session, &attached)),
            };
            let state = Arc::new(State::new(session, store, inner, compactor, mode));
            load(&state).await?;
            state
        }
    };
    let cancel = CancellationToken::new();
    let handle = Handle {
        attached,
        state: state.clone(),
        cancel: cancel.clone(),
    };
    inner.sessions.lock().insert(session, handle);
    if mode == MemoryMode::Endless {
        start_pump(state, cancel);
    }
    Ok(())
}

pub(crate) fn detach(inner: &Inner, session: SessionId) {
    inner.sessions.lock().remove(&session);
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
            units::derive(m.seq, m.at, &m.message, state.cfg.cap_chars, 0)
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
    let mut live = state.live.lock();
    live.units = units;
    live.turns = turns;
    live.last_seq = last_seq;
    live.turn = max_turn;
    live.tree = Tree::from_nodes(nodes);
    live.backfill_total = live
        .units
        .iter()
        .filter(|u| live.unit_turn(u.i) == 0 && !live.tree.built(0, u.i))
        .map(|u| u.i + 1)
        .max()
        .unwrap_or(0);
    live.invalidate();
    drop(live);
    // Rows that landed after the cache (or a cache that never existed).
    ingest(state).await
}

/// Derive units from `messages` rows beyond `last_seq` and cache them.
async fn ingest(state: &State) -> anyhow::Result<()> {
    let _guard = state.ingest.lock().await;
    let from = state.live.lock().last_seq + 1;
    let rows = state.store.endless_messages_from(state.session, from).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut new_units = Vec::new();
    let (mut next_i, turn, mut last_seq) = {
        let live = state.live.lock();
        (live.t(), live.turn, live.last_seq)
    };
    for m in &rows {
        last_seq = last_seq.max(m.seq);
        if m.excluded {
            continue;
        }
        let units = units::derive(m.seq, m.at, &m.message, state.cfg.cap_chars, next_i);
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
    state.store.endless_units_put(state.session, &unit_rows).await?;
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
    let cfg = state.cfg.fold_cfg();
    let mut live = state.live.lock();
    if live.mode != MemoryMode::Endless {
        return;
    }
    let t = live.t();
    let now = Instant::now();
    // Rule 3 frontier: the start of the first unbuilt view part (T when all built).
    live.ensure_fold(cfg);
    let frontier = live
        .fold
        .as_ref()
        .expect("ensured")
        .parts
        .iter()
        .find(|p| !live.tree.built(p.l, p.i))
        .map_or(t, |p| p.start());
    let free = |live: &Live, key: &(u8, u64)| {
        !live.running.contains(key) && live.failed.get(key).is_none_or(|at| *at <= now)
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
            if live.tree.built(l, i) || !exists(l, i, t) || (i + 1) * n > frontier {
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
    let slots = usize::from(state.cfg.jobs.max(1)).saturating_sub(live.running.len());
    for key in candidates.into_iter().take(slots) {
        live.running.insert(key);
        spawn_build(state.clone(), key, &mut live);
    }
}

/// Assemble the step for `(l, i)` under the lock and run it on its own task.
fn spawn_build(state: Arc<State>, key: (u8, u64), live: &mut Live) {
    let (l, i) = key;
    let node_bytes = state.cfg.node_bytes;
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
        let a = live.tree.get(l - 1, 2 * i).map(|n| n.text.clone()).unwrap_or_default();
        let b = live.tree.get(l - 1, 2 * i + 1).map(|n| n.text.clone()).unwrap_or_default();
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
            let f = live.fold(state.cfg.fold_cfg());
            fold::render(&live.tree, &f, &live.units).text()
        }
    };
    tokio::spawn(async move {
        let started = Instant::now();
        let outcome = match (free_text, step) {
            (Some(text), _) => Ok(Node {
                l,
                i,
                size: text.len() as u32,
                text,
                free: true,
                model: None,
                tries: 0,
            }),
            (None, Some(step)) => call(&state, l, i, &ctx, &step).await,
            (None, None) => Err("no step".to_owned()),
        };
        finish(&state, key, outcome, started.elapsed()).await;
    });
}

/// One node's compactor build, with its `model_calls` row.
async fn call(state: &State, l: u8, i: u64, ctx: &str, step: &Step) -> Result<Node, String> {
    let c = state.compactor.as_ref();
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
    let built = compactor::build(c, ctx, step, state.cfg.node_bytes, state.cfg.tries).await;
    let (status, usage) = match &built {
        Ok(b) => (CallStatus::Settled, b.usage()),
        Err(_) => (CallStatus::Failed, None),
    };
    let obs = compactor::observation(usage.as_ref(), model);
    if let Err(e) = state.store.settle_call(&reserved, status, obs).await {
        tracing::warn!(error = %e, "endless: settle_call");
    }
    let built = built?;
    if let Err(e) = state
        .store
        .endless_calls_put(state.session, l, i, &built.calls)
        .await
    {
        tracing::warn!(error = %e, "endless: endless_calls");
    }
    Ok(Node {
        l,
        i,
        size: built.text.len() as u32,
        text: built.text,
        free: false,
        model: Some(c.model_id().to_owned()),
        tries: built.tries,
    })
}

/// Persist and publish one job's outcome.
async fn finish(state: &State, key: (u8, u64), outcome: Result<Node, String>, took: Duration) {
    let outcome = match outcome {
        Ok(node) => match state.store.endless_node_put(state.session, &node).await {
            Ok(()) => Ok(node),
            Err(e) => Err(format!("store: {e}")),
        },
        Err(e) => Err(e),
    };
    let mut live = state.live.lock();
    live.running.remove(&key);
    match outcome {
        Ok(node) => {
            if node.l == 0 && !node.free {
                live.build_secs.0 += took.as_secs_f64();
                live.build_secs.1 += 1;
            }
            live.failed.remove(&key);
            live.tree.insert(node);
            live.invalidate();
            state.built_tx.send_modify(|v| *v += 1);
        }
        Err(e) => {
            live.failed
                .insert(key, Instant::now() + Duration::from_secs(u64::from(state.cfg.retry_secs)));
            if live.reported.insert(key) {
                tracing::warn!(session = %state.session, l = key.0, i = key.1, error = %e, "endless: node failed (will retry)");
                live.pending_error = Some(format!("{}+{}: {e}", id_n(key.0, key.1).0, id_n(key.0, key.1).1));
            }
        }
    }
    live.dirty = true;
    drop(live);
    state.wake.notify_one();
}

/// Publish `EndlessProgress` if due; returns how long the loop may sleep.
fn emit_progress(state: &Arc<State>) -> Duration {
    let mut live = state.live.lock();
    let next_retry = live
        .failed
        .values()
        .map(|at| at.saturating_duration_since(Instant::now()))
        .min()
        .unwrap_or(Duration::from_secs(60));
    if !live.dirty {
        return next_retry.max(Duration::from_millis(50));
    }
    if let Some(last) = live.last_progress
        && last.elapsed() < PROGRESS_EVERY
    {
        return (PROGRESS_EVERY - last.elapsed()).min(next_retry).max(Duration::from_millis(20));
    }
    live.dirty = false;
    live.last_progress = Some(Instant::now());
    let cfg = state.cfg.fold_cfg();
    let (built, total) = live.totals();
    let unbuilt = live.fold(cfg).unbuilt(&live.tree);
    let ev = EndlessProgress {
        session: state.session,
        built,
        total,
        settling: unbuilt > 0,
        error: live.pending_error.take(),
        backfill: live.backfill(),
    };
    drop(live);
    let rt = state.rt.clone();
    tokio::spawn(async move { rt.publish(ev).await });
    next_retry.max(Duration::from_millis(50))
}

fn state_of(inner: &Inner, session: SessionId) -> Option<Arc<State>> {
    inner.sessions.lock().get(&session).map(|h| h.state.clone())
}

pub(crate) async fn settle(inner: &Inner, session: SessionId, max: Duration) -> bool {
    let Some(state) = state_of(inner, session) else {
        return true;
    };
    if let Err(e) = ingest(&state).await {
        tracing::warn!(%session, error = %e, "endless: ingest before settle");
    }
    state.wake.notify_one();
    let cfg = state.cfg.fold_cfg();
    let mut rx = state.built_tx.subscribe();
    let deadline = Instant::now() + max;
    loop {
        let unbuilt = {
            let mut live = state.live.lock();
            live.fold(cfg).unbuilt(&live.tree)
        };
        if unbuilt == 0 {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || tokio::time::timeout(left, rx.changed()).await.is_err() {
            return false;
        }
    }
}

pub(crate) async fn render_for_turn(inner: &Inner, session: SessionId) -> anyhow::Result<ViewRender> {
    let Some(state) = state_of(inner, session) else {
        anyhow::bail!("session {session} is not attached to endless");
    };
    ingest(&state).await?;
    let cfg = state.cfg.fold_cfg();
    let mut live = state.live.lock();
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

fn view_line(part: Part, text: String, built: bool) -> ViewLine {
    let (id, n) = part.id_n();
    ViewLine {
        id,
        n,
        level: part.l,
        text: text.replace('\n', " "),
        built,
    }
}

pub(crate) async fn zoom(inner: &Inner, session: SessionId, id: u64, n: u64) -> Result<Vec<ViewLine>, String> {
    let Some(state) = state_of(inner, session) else {
        return Err("this session has no endless memory".into());
    };
    let live = state.live.lock();
    let Some((l, i)) = tree::addr(id, n).filter(|(l, i)| exists(*l, *i, live.t())) else {
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
        }]);
    }
    Ok([2 * i, 2 * i + 1]
        .into_iter()
        .map(|ci| {
            let part = Part { l: l - 1, i: ci };
            let (text, built) = fold::part_text(&live.tree, part, &live.units);
            view_line(part, text, built)
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
    let mut out: Vec<ViewLine> = hits
        .into_iter()
        .map(|(i, text)| view_line(Part { l: 0, i }, text, true))
        .collect();
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
                    super::guards::cut_utf8(&u.tagged(), state.cfg.node_bytes as usize)
                );
                out.push(view_line(Part { l: 0, i }, text, false));
            }
        }
    }
    Ok(out)
}

pub(crate) async fn date(inner: &Inner, session: SessionId, id: u64) -> Result<Option<DateTime<Utc>>, String> {
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
    epoch: u32,
) -> anyhow::Result<SetMode> {
    let store = inner.store_for(session);
    match mode {
        MemoryMode::Endless => {
            store.set_session_memory(session, mode).await?;
            let attached = inner.sessions.lock().get(&session).map(|h| h.attached.clone());
            let attached = match attached {
                Some(a) => a,
                None => {
                    let row = store
                        .session(session)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no session {session}"))?;
                    Attached {
                        model: inner
                            .catalog
                            .resolve(&row.model, &*crate::supervisor::configured(inner.auth.clone()))
                            .map_err(anyhow::Error::msg)?,
                        effort: row.effort.as_deref().and_then(crate::effort_of),
                        agent: row.desk.unwrap_or_else(|| "theuth".to_owned()),
                        store: None,
                    }
                }
            };
            // A fresh pump: `attach` reloads nothing for a known state but restarts the task.
            attach(inner, session, attached).await?;
            if let Some(state) = state_of(inner, session) {
                let mut live = state.live.lock();
                live.backfill_total = live
                    .units
                    .iter()
                    .filter(|u| live.unit_turn(u.i) == 0 && !live.tree.built(0, u.i))
                    .map(|u| u.i + 1)
                    .max()
                    .unwrap_or(0);
                live.dirty = true;
                drop(live);
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
                    let cfg = state.cfg.fold_cfg();
                    let mut live = state.live.lock();
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
            ..Default::default()
        });
    };
    let stats = store.endless_stats(session).await?;
    let cfg = state.cfg.fold_cfg();
    let mut live = state.live.lock();
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
        model: state.compactor.model_id().to_owned(),
        backfill: live.backfill(),
    })
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
