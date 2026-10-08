//! The shared engine behind the actor and the tools: open roots (LRU of 8
//! pools), per-root writer lock, reconcile-on-call (60 §5.4, §0.8).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use runtime::Runtime;
use sqlx::SqlitePool;

use crate::event::{CodeindexEvent, RECONCILED_PATHS};
use crate::git::Git;
use crate::lang::Language;
use crate::map;
use crate::parse::{ParsedFile, Parsers, parse};
use crate::rank;
use crate::resolve::{self, Workspace};
use crate::store::{self, FileWrite};
use crate::walk::{self, Entry};
use crate::{CodeindexConfig, IndexStats, root};

/// Open pools kept at once (60 §3).
const POOL_LRU: usize = 8;
/// Reconcile guard: a diff that would delete more than this share of the
/// indexed files keeps the old rows instead (moved root, unmounted volume,
/// half-checked-out worktree).
const SUSPECT_GONE_PERCENT: usize = 80;
/// Envelope `hint` when the guard fires.
pub const SUSPECT_ROOT_HINT: &str = "most indexed files are missing from the walk; kept the old index — check the root is fully checked out and mounted, or force a reconcile";
/// Minimum gap between two `CodeReconciled` events of one root (73 §5.1);
/// a write inside the gap defers one trailing event.
pub const RECONCILED_GATE: Duration = Duration::from_millis(250);
/// Touched files inside [`BURST_WINDOW`] above which a touch is a burst
/// (73 §7, 147 §5.8 item 5): the rank persist and the skeleton pass are
/// deferred to one trailing run instead of ~300–700 ms per keystroke.
pub const BURST_FILES: usize = 4;
/// The burst window; the trailing run waits for this much quiet.
pub const BURST_WINDOW: Duration = Duration::from_secs(2);
/// `meta` key: `1` while a burst holds the rank (and skeleton) back.
pub const RANK_DEFERRED: &str = "rank_deferred";

/// Everything the plugin shares: config, the index directory, open roots.
pub struct Codeindex {
    /// `codeindex:` section.
    pub cfg: CodeindexConfig,
    dir: PathBuf,
    roots: tokio::sync::Mutex<Lru>,
    /// Event sink (73 §5.1); `None` in tests and embedders without a bus.
    rt: Option<Runtime>,
}

#[derive(Default)]
struct Lru {
    order: VecDeque<PathBuf>,
    map: HashMap<PathBuf, Arc<RootIndex>>,
}

/// One open root: its pool, writer lock and background state.
pub struct RootIndex {
    /// Canonical root.
    pub root: PathBuf,
    /// The root's `<slug>-<hash8>.sqlite`.
    pub pool: SqlitePool,
    // ponytail: one lock across walk + diff + write serialises readers of the
    // same root on the walk too; split into a read snapshot if calls pile up.
    write: tokio::sync::Mutex<()>,
    background: AtomicBool,
    pending: AtomicUsize,
    suspect_logged: AtomicBool,
    /// The last reconcile's guard verdict, for lens replies that never walk.
    suspect: AtomicBool,
    /// The last walk hit [`walk::MAX_FILES`].
    truncated: AtomicBool,
    /// Stamped on every event and lens reply (73 §4).
    seq: AtomicU64,
    gate: tokio::sync::Mutex<Gate>,
    /// The root's `-map.sqlite` (147 §9), opened on first use.
    map: tokio::sync::Mutex<Option<SqlitePool>>,
    /// Touch-burst state (147 §5.8 item 5).
    burst: tokio::sync::Mutex<Burst>,
}

/// Touches in the last [`BURST_WINDOW`] and whether a trailing rank run is owed.
#[derive(Default)]
struct Burst {
    recent: VecDeque<Instant>,
    /// A write skipped its rank persist; the trailing run owes one.
    deferred: bool,
    /// The trailing task is alive.
    scheduled: bool,
}

/// `CodeReconciled` coalescing state (73 §4).
#[derive(Default)]
struct Gate {
    last: Option<Instant>,
    /// A trailing event waiting for the gap to pass; merged into by later writes.
    deferred: Option<(Vec<String>, Vec<String>)>,
}

impl RootIndex {
    /// Next per-root sequence number.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The current sequence number (a lens reply reflects the index as of it).
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Freshness without a walk: the background flags and the last walk's verdicts.
    pub fn snapshot(&self) -> Reconciled {
        let suspect = self.suspect.load(Ordering::Acquire);
        Reconciled {
            stale: self.background.load(Ordering::Acquire) || suspect,
            pending: self.pending.load(Ordering::Acquire),
            suspect_root: suspect,
            hint: suspect.then_some(SUSPECT_ROOT_HINT),
            truncated_walk: self.truncated.load(Ordering::Acquire),
            ..Default::default()
        }
    }
}

/// What one reconcile did (the envelope's freshness fields, 60 §8).
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Reconciled {
    /// Files parsed and written this call.
    pub reparsed: usize,
    /// Files deleted this call.
    pub removed: usize,
    /// Answered from an index a background task is still updating.
    pub stale: bool,
    /// Files that background task still has to write.
    pub pending: usize,
    /// The walk stopped at [`walk::MAX_FILES`].
    pub truncated_walk: bool,
    /// `files` rows after this call.
    pub indexed_files: i64,
    /// The co-change walk ran this call (HEAD moved): `(commits walked, ms)`.
    pub cochange_walk: Option<(usize, u128)>,
    /// The walk lost more than [`SUSPECT_GONE_PERCENT`] of the indexed files;
    /// nothing was deleted, the answer comes from the old rows.
    pub suspect_root: bool,
    /// Why the answer may be off, when `suspect_root`.
    pub hint: Option<&'static str>,
}

impl Codeindex {
    /// `dir` = `<data_dir>/codeindex/`, created.
    pub fn new(cfg: CodeindexConfig, data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join(crate::DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self {
            cfg,
            dir,
            roots: tokio::sync::Mutex::default(),
            rt: None,
        })
    }

    /// Publish [`CodeindexEvent`]s on `rt`'s bus (73 §5.1).
    pub fn with_runtime(mut self, rt: Runtime) -> Self {
        self.rt = Some(rt);
        self
    }

    /// Whether `root` already has an index file (a `code_*` call opened it once).
    pub fn indexed(&self, root: &Path) -> bool {
        self.dir.join(root::db_file_name(root)).is_file()
    }

    /// `<data_dir>/codeindex/`: where the index and map files live (147 §9).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether `ri` has a map file (147 §9): the CodeMap was opened for it
    /// once, so the skeleton pass after a write has something to keep fresh.
    pub fn has_map(&self, ri: &RootIndex) -> bool {
        map::store::path(&self.dir, &ri.root).is_file()
    }

    /// The root's map pool (147 §9), opened — and created — on first use and
    /// kept with the root. Every map read and write goes through here.
    pub async fn map_pool(&self, ri: &RootIndex) -> anyhow::Result<SqlitePool> {
        let mut slot = ri.map.lock().await;
        if let Some(p) = &*slot {
            return Ok(p.clone());
        }
        let p = map::store::open(&map::store::path(&self.dir, &ri.root), &ri.root).await?;
        *slot = Some(p.clone());
        Ok(p)
    }
    /// Publish on the bus when the plugin owns one.
    pub(crate) async fn publish(&self, ev: CodeindexEvent) {
        if let Some(rt) = &self.rt {
            rt.publish(ev).await;
        }
    }

    /// Open (or reuse) the index for a canonical `root`; evicts the least
    /// recently used pool beyond [`POOL_LRU`].
    pub async fn open_root(&self, root: &Path) -> anyhow::Result<Arc<RootIndex>> {
        let mut lru = self.roots.lock().await;
        if let Some(ri) = lru.map.get(root) {
            let ri = ri.clone();
            lru.order.retain(|p| p != root);
            lru.order.push_back(root.to_path_buf());
            return Ok(ri);
        }
        let pool = store::open(&self.dir.join(crate::root::db_file_name(root)), root).await?;
        let ri = Arc::new(RootIndex {
            root: root.to_path_buf(),
            pool,
            write: tokio::sync::Mutex::new(()),
            background: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            suspect_logged: AtomicBool::new(false),
            suspect: AtomicBool::new(false),
            truncated: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            gate: tokio::sync::Mutex::default(),
            map: tokio::sync::Mutex::new(None),
            burst: tokio::sync::Mutex::default(),
        });
        lru.map.insert(root.to_path_buf(), ri.clone());
        lru.order.push_back(root.to_path_buf());
        while lru.order.len() > POOL_LRU {
            if let Some(old) = lru.order.pop_front()
                && let Some(evicted) = lru.map.remove(&old)
            {
                tokio::spawn(async move {
                    evicted.pool.close().await;
                    if let Some(m) = evicted.map.lock().await.take() {
                        m.close().await;
                    }
                });
            }
        }
        Ok(ri)
    }

    /// Walk, diff `(mtime_ns, size)` against `files`, write the changed set:
    /// synchronously when it is ≤ `sync_limit` or the index is empty (or
    /// `force`), else answer stale and finish in a background task.
    pub async fn reconcile(
        self: &Arc<Self>,
        ri: &Arc<RootIndex>,
        force: bool,
    ) -> anyhow::Result<Reconciled> {
        let _g = ri.write.lock().await;
        let d = self.diff(ri).await?;
        let mut out = Reconciled {
            truncated_walk: d.truncated,
            ..Default::default()
        };
        ri.suspect.store(!force && d.suspect(), Ordering::Release);
        ri.truncated.store(d.truncated, Ordering::Release);
        let mut wrote = None;
        if d.changed.is_empty() && d.delete.is_empty() {
            out.stale = ri.background.load(Ordering::Acquire);
            out.pending = ri.pending.load(Ordering::Acquire);
        } else if !force && d.suspect() {
            out.stale = true;
            out.suspect_root = true;
            out.hint = Some(SUSPECT_ROOT_HINT);
            out.indexed_files = store::counts(&ri.pool).await?.files;
            if !ri.suspect_logged.swap(true, Ordering::AcqRel) {
                tracing::warn!(root = %ri.root.display(), known = d.known, gone = d.gone.len(), "codeindex reconcile refused: most indexed files missing from the walk");
            }
            return Ok(out);
        } else if force || d.known == 0 || d.changed.len() + d.gone.len() <= self.cfg.sync_limit {
            let t = Instant::now();
            out.removed = d.gone.len();
            let written = self.write(ri, d.changed, d.delete, true).await?;
            out.reparsed = written.len();
            tracing::info!(root = %ri.root.display(), reparsed = out.reparsed, removed = out.removed, ms = t.elapsed().as_millis(), "codeindex reconciled");
            wrote = Some((written, d.gone));
        } else {
            out.stale = true;
            out.pending = d.changed.len();
            out.indexed_files = store::counts(&ri.pool).await?.files;
            if ri
                .background
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                ri.pending.store(d.changed.len(), Ordering::Release);
                let (me, ri2) = (Arc::clone(self), Arc::clone(ri));
                tokio::spawn(async move {
                    // Re-diff under the lock: the world may have moved on.
                    let _g = ri2.write.lock().await;
                    let r = match me.diff(&ri2).await {
                        Ok(d) if d.suspect() => {
                            if !ri2.suspect_logged.swap(true, Ordering::AcqRel) {
                                tracing::warn!(root = %ri2.root.display(), known = d.known, gone = d.gone.len(), "codeindex reconcile refused: most indexed files missing from the walk");
                            }
                            Ok((Vec::new(), Vec::new()))
                        }
                        Ok(d) => me
                            .write(&ri2, d.changed, d.delete, true)
                            .await
                            .map(|written| (written, d.gone)),
                        Err(e) => Err(e),
                    };
                    let written = match r {
                        Ok((written, gone)) => {
                            tracing::info!(root = %ri2.root.display(), reparsed = written.len(), removed = gone.len(), "codeindex background reconcile done");
                            Some((written, gone))
                        }
                        Err(e) => {
                            tracing::warn!(root = %ri2.root.display(), error = format!("{e:#}"), "codeindex background reconcile failed");
                            None
                        }
                    };
                    if let Err(e) = me.cochange(&ri2).await {
                        tracing::warn!(root = %ri2.root.display(), error = format!("{e:#}"), "codeindex co-change walk failed");
                    }
                    ri2.pending.store(0, Ordering::Release);
                    ri2.background.store(false, Ordering::Release);
                    if let Some((written, gone)) = written {
                        me.reconciled(&ri2, written, gone).await;
                    }
                });
            }
            return Ok(out);
        }
        if !out.stale {
            match self.cochange(ri).await {
                Ok(w) => out.cochange_walk = w,
                Err(e) => {
                    tracing::warn!(root = %ri.root.display(), error = format!("{e:#}"), "codeindex co-change walk failed")
                }
            }
        }
        out.indexed_files = store::counts(&ri.pool).await?.files;
        // After the co-change walk: the event carries `meta.git_head`.
        if let Some((written, gone)) = wrote {
            self.reconciled(ri, written, gone).await;
        }
        Ok(out)
    }

    /// 60 §0.11: when the root is a git repository, `cochange_commits > 0`
    /// and HEAD moved since `meta.cochange_head`, walk the first-parent
    /// history and rewrite `cochanges`. Not a repo → nothing. Returns
    /// `(commits walked, ms)` when the walk ran.
    async fn cochange(&self, ri: &RootIndex) -> anyhow::Result<Option<(usize, u128)>> {
        let limit = self.cfg.cochange_commits as usize;
        if limit == 0 {
            return Ok(None);
        }
        let root = ri.root.clone();
        let head = tokio::task::spawn_blocking(move || {
            Ok::<_, anyhow::Error>(Git::open(&root)?.and_then(|g| g.head()))
        })
        .await??;
        let Some(head) = head.map(|h| h.to_string()) else {
            return Ok(None);
        };
        if store::meta(&ri.pool, "cochange_head").await?.as_deref() == Some(head.as_str()) {
            return Ok(None);
        }
        let t = Instant::now();
        let indexed: HashMap<String, i64> = store::known_files(&ri.pool)
            .await?
            .into_iter()
            .map(|(p, (id, ..))| (p, id))
            .collect();
        let root = ri.root.clone();
        let (rows, walked) = tokio::task::spawn_blocking(move || {
            let Some(g) = Git::open(&root)? else {
                return Ok::<_, anyhow::Error>((Vec::new(), 0));
            };
            g.cochanges(limit, &indexed)
        })
        .await??;
        store::replace_cochanges(&ri.pool, &rows, &head).await?;
        let ms = t.elapsed().as_millis();
        tracing::info!(root = %ri.root.display(), commits = walked, pairs = rows.len(), ms, "codeindex co-change walk");
        Ok(Some((walked, ms)))
    }

    /// Walk and diff against `files`.
    async fn diff(&self, ri: &RootIndex) -> anyhow::Result<Diff> {
        let (root, max) = (ri.root.clone(), self.cfg.max_file_bytes);
        let w = tokio::task::spawn_blocking(move || walk::walk(&root, max)).await?;
        let known = store::known_files(&ri.pool).await?;
        let mut d = Diff {
            known: known.len(),
            truncated: w.truncated,
            ..Default::default()
        };
        let mut seen = HashSet::with_capacity(w.entries.len());
        for e in w.entries {
            seen.insert(e.path.clone());
            match known.get(&e.path) {
                Some(&(_, m, s)) if m == e.mtime_ns && s == e.size => {}
                Some(&(id, _, _)) => {
                    d.delete.push(id);
                    d.changed.push(e);
                }
                None => d.changed.push(e),
            }
        }
        for (p, &(id, _, _)) in known.iter().filter(|(p, _)| !seen.contains(p.as_str())) {
            d.delete.push(id);
            d.gone.push(p.clone());
        }
        Ok(d)
    }

    /// 73 §5.2 `codemap_touch(root, paths)`: stat and re-parse only `paths`
    /// (root-relative or absolute under the root) — no walk. A path that
    /// escapes the root fails the whole call; a known path that is gone is
    /// removed; an unchanged `(mtime_ns, size)` is a no-op. Publishes
    /// `CodeReconciled` when anything was written. More than [`BURST_FILES`]
    /// files touched inside [`BURST_WINDOW`] is a burst (147 §5.8 item 5):
    /// the rank persist and the skeleton pass wait for one trailing run after
    /// the window closes, so a stale mark never costs the whole rank per file.
    pub async fn touch(
        self: &Arc<Self>,
        ri: &Arc<RootIndex>,
        paths: &[String],
    ) -> anyhow::Result<Reconciled> {
        let _g = ri.write.lock().await;
        let known = store::known_files(&ri.pool).await?;
        let mut d = Diff::default();
        for p in paths {
            let raw = Path::new(p);
            let full = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                ri.root.join(raw)
            };
            let rel = match root::under_root(&ri.root, &full) {
                Ok(rel) => root::rel_str(&rel),
                // Gone from disk: a plain relative path we know is a removal.
                Err(_)
                    if !full.exists()
                        && raw.is_relative()
                        && raw
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_))) =>
                {
                    let rel = root::rel_str(raw);
                    if let Some(&(id, _, _)) = known.get(&rel) {
                        d.delete.push(id);
                        d.gone.push(rel);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            let Some(e) = walk::entry(&ri.root, &rel, self.cfg.max_file_bytes) else {
                continue;
            };
            match known.get(&rel) {
                Some(&(_, m, s)) if m == e.mtime_ns && s == e.size => {}
                Some(&(id, _, _)) => {
                    d.delete.push(id);
                    d.changed.push(e);
                }
                None => d.changed.push(e),
            }
        }
        let mut out = ri.snapshot();
        if !d.changed.is_empty() || !d.delete.is_empty() {
            let t = Instant::now();
            out.removed = d.gone.len();
            let n = d.changed.len() + d.gone.len();
            let burst = {
                let mut b = ri.burst.lock().await;
                let now = Instant::now();
                while b
                    .recent
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > BURST_WINDOW)
                {
                    b.recent.pop_front();
                }
                b.recent.extend(std::iter::repeat_n(now, n));
                b.recent.len() > BURST_FILES
            };
            let written = self.write(ri, d.changed, d.delete, !burst).await?;
            if burst {
                let mut b = ri.burst.lock().await;
                b.deferred = true;
                if !b.scheduled {
                    b.scheduled = true;
                    let (me, ri2) = (Arc::clone(self), Arc::clone(ri));
                    tokio::spawn(async move { me.trailing_rank(ri2).await });
                }
            }
            out.reparsed = written.len();
            tracing::info!(root = %ri.root.display(), reparsed = out.reparsed, removed = out.removed, burst, ms = t.elapsed().as_millis(), "codeindex touched");
            self.reconciled(ri, written, d.gone).await;
        }
        out.indexed_files = store::counts(&ri.pool).await?.files;
        Ok(out)
    }

    /// The trailing run of a touch burst: wait for [`BURST_WINDOW`] of quiet,
    /// then persist the rank and run the skeleton pass once, under the
    /// writer lock. A non-burst write in between already did both.
    async fn trailing_rank(&self, ri: Arc<RootIndex>) {
        loop {
            tokio::time::sleep(BURST_WINDOW).await;
            let mut b = ri.burst.lock().await;
            if b.recent.back().is_some_and(|t| t.elapsed() < BURST_WINDOW) {
                continue;
            }
            b.scheduled = false;
            b.recent.clear();
            let due = std::mem::take(&mut b.deferred);
            drop(b);
            if due {
                let _g = ri.write.lock().await;
                let t = Instant::now();
                if let Err(e) = self.persist_rank(&ri).await {
                    tracing::warn!(root = %ri.root.display(), error = format!("{e:#}"), "codeindex deferred rank failed");
                } else {
                    tracing::info!(root = %ri.root.display(), ms = t.elapsed().as_millis(), "codeindex deferred rank persisted");
                }
            }
            return;
        }
    }

    /// 60 §7: the unpersonalised ranks after a write (`files.rank`,
    /// `symbols.rank`, `meta.entry_points`), then the skeleton pass when the
    /// root has a map (147 §5, §8). Clears [`RANK_DEFERRED`].
    async fn persist_rank(&self, ri: &RootIndex) -> anyhow::Result<()> {
        let t = Instant::now();
        let (nodes, iterations) = rank::persist(&ri.pool).await?;
        let rank_ms = t.elapsed().as_millis();
        store::set_meta(&ri.pool, "rank_iterations", &iterations.to_string()).await?;
        store::set_meta(&ri.pool, "rank_ms", &rank_ms.to_string()).await?;
        store::set_meta(&ri.pool, RANK_DEFERRED, "0").await?;
        ri.burst.lock().await.deferred = false;
        tracing::debug!(
            rank_ms,
            rank_nodes = nodes,
            rank_iterations = iterations,
            "codeindex rank persisted"
        );
        if self.has_map(ri) {
            let t = Instant::now();
            match map::skeleton::recompute(self, ri).await {
                Ok(changed) => {
                    tracing::debug!(
                        changed = changed.len(),
                        ms = t.elapsed().as_millis(),
                        "codemap skeleton pass"
                    )
                }
                Err(e) => {
                    tracing::warn!(root = %ri.root.display(), error = format!("{e:#}"), "codemap skeleton pass failed")
                }
            }
        }
        Ok(())
    }

    /// Publish `CodeReconciled` for a write, ≥ [`RECONCILED_GATE`] apart per
    /// root: inside the gap the paths merge into one trailing event.
    async fn reconciled(&self, ri: &Arc<RootIndex>, reparsed: Vec<String>, removed: Vec<String>) {
        let Some(rt) = &self.rt else { return };
        let mut g = ri.gate.lock().await;
        if let Some((r, d)) = &mut g.deferred {
            r.extend(reparsed);
            d.extend(removed);
            return;
        }
        let wait = g.last.map(|t| RECONCILED_GATE.saturating_sub(t.elapsed()));
        match wait {
            Some(w) if !w.is_zero() => {
                g.deferred = Some((reparsed, removed));
                drop(g);
                let (rt, ri) = (rt.clone(), Arc::clone(ri));
                tokio::spawn(async move {
                    tokio::time::sleep(w).await;
                    let mut g = ri.gate.lock().await;
                    let Some((r, d)) = g.deferred.take() else {
                        return;
                    };
                    g.last = Some(Instant::now());
                    drop(g);
                    rt.publish(reconciled_event(&ri, r, d).await).await;
                });
            }
            _ => {
                g.last = Some(Instant::now());
                drop(g);
                rt.publish(reconciled_event(ri, reparsed, removed).await)
                    .await;
            }
        }
    }

    /// Parse `changed` on `available_parallelism()` blocking workers, each
    /// owning its parsers, then one transaction, then 60 §6 resolution for
    /// the changed files plus the affected set (§5.4), then — unless a touch
    /// burst defers it — the rank persist and the skeleton pass
    /// ([`Self::persist_rank`]). Returns the paths written.
    async fn write(
        &self,
        ri: &RootIndex,
        changed: Vec<Entry>,
        delete: Vec<i64>,
        persist_rank: bool,
    ) -> anyhow::Result<Vec<String>> {
        let enabled: HashSet<&str> = self.cfg.languages.iter().map(String::as_str).collect();
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = changed.len().div_ceil(workers).max(1);
        let mut tasks = Vec::with_capacity(workers + 1);
        for entries in changed.chunks(chunk) {
            let entries: Vec<(Entry, bool)> = entries
                .iter()
                .map(|e| {
                    (
                        e.clone(),
                        e.language != Language::Other && enabled.contains(e.language.config_name()),
                    )
                })
                .collect();
            let root = ri.root.clone();
            tasks.push(tokio::task::spawn_blocking(move || {
                parse_chunk(&root, entries)
            }));
        }
        let root = ri.root.clone();
        let ws = tokio::task::spawn_blocking(move || Workspace::load(&root));
        let t = Instant::now();
        let mut parsed: Vec<Parsed> = Vec::with_capacity(changed.len());
        for t in tasks {
            parsed.extend(t.await??);
        }
        let ws = ws.await?;
        let parse_ms = t.elapsed().as_millis();
        let writes: Vec<FileWrite<'_>> = parsed
            .iter()
            .map(|p| FileWrite {
                path: &p.entry.path,
                mtime_ns: p.entry.mtime_ns,
                size: p.entry.size,
                language: p.entry.language.as_str(),
                is_test: walk::is_test_path(&p.entry.path),
                package: ws.package(&p.entry.path, p.entry.language),
                parsed: p.file.as_ref(),
            })
            .collect();
        let t = Instant::now();
        let affected = store::apply(&ri.pool, &delete, &writes).await?;
        let apply_ms = t.elapsed().as_millis();
        let mut targets: HashSet<String> = affected.into_iter().collect();
        targets.extend(
            writes
                .iter()
                .filter(|w| w.parsed.is_some())
                .map(|w| w.path.to_owned()),
        );
        let t = Instant::now();
        resolve::resolve(&ri.pool, &ws, &targets).await?;
        let resolve_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let subjects = test_subjects(&ri.pool).await?;
        let subjects_ms = t.elapsed().as_millis();
        // 60 §7: unpersonalised ranks stored after every write, so `code_map`
        // without `focus` and `code_find`'s ordering are lookups.
        let t = Instant::now();
        if persist_rank {
            self.persist_rank(ri).await?;
        } else {
            store::set_meta(&ri.pool, RANK_DEFERRED, "1").await?;
        }
        let rank_ms = t.elapsed().as_millis();
        tracing::debug!(
            parse_ms,
            apply_ms,
            resolve_ms,
            subjects_ms,
            test_subjects = subjects,
            rank_ms,
            rank_deferred = !persist_rank,
            resolved_files = targets.len(),
            "codeindex write phases"
        );
        Ok(writes.iter().map(|w| w.path.to_owned()).collect())
    }

    /// Row counts and freshness for `root` (60 §10).
    pub async fn stats(&self, ri: &RootIndex) -> anyhow::Result<IndexStats> {
        let c = store::counts(&ri.pool).await?;
        let num = |v: Option<String>| v.and_then(|s| s.parse().ok());
        Ok(IndexStats {
            root: ri.root.clone(),
            files: c.files,
            parsed_files: c.parsed_files,
            symbols: c.symbols,
            refs: c.refs,
            resolved: c.resolved,
            ambiguous: c.ambiguous,
            imports: c.imports,
            unresolved_imports: c.unresolved_imports,
            cochange_pairs: c.cochange_pairs,
            last_reconcile_at: num(store::meta(&ri.pool, "last_reconcile_at").await?),
            git_head: store::meta(&ri.pool, "git_head").await?,
            rank_iterations: num(store::meta(&ri.pool, "rank_iterations").await?),
            rank_ms: num(store::meta(&ri.pool, "rank_ms").await?),
            stale: ri.background.load(Ordering::Acquire),
            pending: ri.pending.load(Ordering::Acquire),
        })
    }
}

/// 60 §8 `test_subject`: for every test file the file it most plausibly
/// exercises — the first [`walk::subject_candidates`] that is an indexed
/// non-test file, else the non-test file it imports most (ties: first path).
/// Recomputed whole after every write (test files are a small share; a
/// new non-test file can become the subject of an old test) and stored as
/// `meta.test_subjects` JSON. Returns the number of tests with a subject.
async fn test_subjects(pool: &SqlitePool) -> anyhow::Result<usize> {
    let files = store::all_files(pool).await?;
    let by_id: HashMap<i64, &str> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let sources: HashSet<&str> = files
        .iter()
        .filter(|f| !f.is_test)
        .map(|f| f.path.as_str())
        .collect();
    let mut most: HashMap<i64, (i64, &str)> = HashMap::new();
    for (test, target, n) in store::test_import_counts(pool).await? {
        let Some(&path) = by_id.get(&target) else {
            continue;
        };
        let e = most.entry(test).or_insert((n, path));
        if n > e.0 || n == e.0 && path < e.1 {
            *e = (n, path);
        }
    }
    let map: BTreeMap<String, String> = files
        .iter()
        .filter(|f| f.is_test && f.language != "other" && !walk::is_test_data(&f.path))
        .filter_map(|f| {
            walk::subject_candidates(&f.path)
                .into_iter()
                .find(|c| sources.contains(c.as_str()))
                .or_else(|| most.get(&f.id).map(|(_, p)| (*p).to_owned()))
                .map(|s| (f.path.clone(), s))
        })
        .collect();
    store::set_test_subjects(pool, &map).await?;
    Ok(map.len())
}

/// The `CodeReconciled` payload for `ri` after a write: paths capped at
/// [`RECONCILED_PATHS`] each, freshness from the atomics, HEAD from `meta`.
async fn reconciled_event(
    ri: &RootIndex,
    mut reparsed: Vec<String>,
    mut removed: Vec<String>,
) -> CodeindexEvent {
    let more = reparsed.len().saturating_sub(RECONCILED_PATHS)
        + removed.len().saturating_sub(RECONCILED_PATHS);
    reparsed.truncate(RECONCILED_PATHS);
    removed.truncate(RECONCILED_PATHS);
    let r = ri.snapshot();
    CodeindexEvent::CodeReconciled {
        root: ri.root.clone(),
        reparsed,
        removed,
        more,
        stale: r.stale,
        pending: r.pending,
        suspect_root: r.suspect_root,
        hint: r.hint.map(str::to_owned),
        git_head: store::meta(&ri.pool, "git_head").await.ok().flatten(),
        seq: ri.next_seq(),
    }
}

/// Walk vs `files`.
#[derive(Default)]
struct Diff {
    /// New or changed files to parse and write.
    changed: Vec<Entry>,
    /// `files.id`s to delete first: changed files' old rows and gone files.
    delete: Vec<i64>,
    /// Paths no longer present.
    gone: Vec<String>,
    /// `files` rows before this diff (0 = first index).
    known: usize,
    /// The walk hit [`walk::MAX_FILES`].
    truncated: bool,
}

impl Diff {
    /// More than [`SUSPECT_GONE_PERCENT`] of the indexed files vanished.
    fn suspect(&self) -> bool {
        self.gone.len() * 100 > self.known * SUSPECT_GONE_PERCENT
    }
}

struct Parsed {
    entry: Entry,
    file: Option<ParsedFile<'static>>,
}

/// One worker: read and parse its entries. A file that vanished or is
/// binary/non-UTF-8 is dropped from the write (60 §5.1).
fn parse_chunk(root: &Path, entries: Vec<(Entry, bool)>) -> anyhow::Result<Vec<Parsed>> {
    let mut parsers = Parsers::default();
    let mut out = Vec::with_capacity(entries.len());
    for (entry, parse_it) in entries {
        if !parse_it {
            out.push(Parsed { entry, file: None });
            continue;
        }
        let Some(cfg) = entry.language.tags() else {
            out.push(Parsed { entry, file: None });
            continue;
        };
        let Ok(src) = std::fs::read(root.join(&entry.path)) else {
            continue;
        };
        match parse(&mut parsers, cfg, entry.language, &src) {
            Some(file) => out.push(Parsed {
                entry,
                file: Some(file),
            }),
            None => tracing::debug!(path = %entry.path, "skipped: binary or not UTF-8"),
        }
    }
    Ok(out)
}
