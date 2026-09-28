//! The shared engine behind the actor and the tools: open roots (LRU of 8
//! pools), per-root writer lock, reconcile-on-call (60 §5.4, §0.8).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Context;
use sqlx::SqlitePool;

use crate::git::Git;
use crate::lang::Language;
use crate::parse::{ParsedFile, Parsers, parse};
use crate::rank;
use crate::resolve::{self, Workspace};
use crate::store::{self, FileWrite};
use crate::walk::{self, Entry};
use crate::{CodeindexConfig, IndexStats};

/// Open pools kept at once (60 §3).
const POOL_LRU: usize = 8;
/// Reconcile guard: a diff that would delete more than this share of the
/// indexed files keeps the old rows instead (moved root, unmounted volume,
/// half-checked-out worktree).
const SUSPECT_GONE_PERCENT: usize = 80;
/// Envelope `hint` when the guard fires.
pub const SUSPECT_ROOT_HINT: &str = "most indexed files are missing from the walk; kept the old index — check the root is fully checked out and mounted, or force a reconcile";

/// Everything the plugin shares: config, the index directory, open roots.
pub struct Codeindex {
    /// `codeindex:` section.
    pub cfg: CodeindexConfig,
    dir: PathBuf,
    roots: tokio::sync::Mutex<Lru>,
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
        })
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
        });
        lru.map.insert(root.to_path_buf(), ri.clone());
        lru.order.push_back(root.to_path_buf());
        while lru.order.len() > POOL_LRU {
            if let Some(old) = lru.order.pop_front()
                && let Some(evicted) = lru.map.remove(&old)
            {
                tokio::spawn(async move { evicted.pool.close().await });
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
        if d.changed.is_empty() && d.delete.is_empty() {
            out.stale = ri.background.load(Ordering::Acquire);
            out.pending = ri.pending.load(Ordering::Acquire);
        } else if !force && d.suspect() {
            out.stale = true;
            out.suspect_root = true;
            out.hint = Some(SUSPECT_ROOT_HINT);
            out.indexed_files = store::counts(&ri.pool).await?.files;
            if !ri.suspect_logged.swap(true, Ordering::AcqRel) {
                tracing::warn!(root = %ri.root.display(), known = d.known, gone = d.gone, "codeindex reconcile refused: most indexed files missing from the walk");
            }
            return Ok(out);
        } else if force || d.known == 0 || d.changed.len() + d.gone <= self.cfg.sync_limit {
            let t = Instant::now();
            out.removed = d.gone;
            out.reparsed = self.write(ri, d.changed, d.delete).await?;
            tracing::info!(root = %ri.root.display(), reparsed = out.reparsed, removed = out.removed, ms = t.elapsed().as_millis(), "codeindex reconciled");
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
                                tracing::warn!(root = %ri2.root.display(), known = d.known, gone = d.gone, "codeindex reconcile refused: most indexed files missing from the walk");
                            }
                            Ok((0, 0))
                        }
                        Ok(d) => me
                            .write(&ri2, d.changed, d.delete)
                            .await
                            .map(|n| (n, d.gone)),
                        Err(e) => Err(e),
                    };
                    match r {
                        Ok((n, gone)) => {
                            tracing::info!(root = %ri2.root.display(), reparsed = n, removed = gone, "codeindex background reconcile done")
                        }
                        Err(e) => {
                            tracing::warn!(root = %ri2.root.display(), error = format!("{e:#}"), "codeindex background reconcile failed")
                        }
                    }
                    if let Err(e) = me.cochange(&ri2).await {
                        tracing::warn!(root = %ri2.root.display(), error = format!("{e:#}"), "codeindex co-change walk failed");
                    }
                    ri2.pending.store(0, Ordering::Release);
                    ri2.background.store(false, Ordering::Release);
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
        for (_, &(id, _, _)) in known.iter().filter(|(p, _)| !seen.contains(p.as_str())) {
            d.delete.push(id);
            d.gone += 1;
        }
        Ok(d)
    }

    /// Parse `changed` on `available_parallelism()` blocking workers, each
    /// owning its parsers, then one transaction, then 60 §6 resolution for
    /// the changed files plus the affected set (§5.4). Returns the number of
    /// files written.
    async fn write(
        &self,
        ri: &RootIndex,
        changed: Vec<Entry>,
        delete: Vec<i64>,
    ) -> anyhow::Result<usize> {
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
        let (nodes, iterations) = rank::persist(&ri.pool).await?;
        let rank_ms = t.elapsed().as_millis();
        store::set_meta(&ri.pool, "rank_iterations", &iterations.to_string()).await?;
        store::set_meta(&ri.pool, "rank_ms", &rank_ms.to_string()).await?;
        tracing::debug!(
            parse_ms,
            apply_ms,
            resolve_ms,
            subjects_ms,
            test_subjects = subjects,
            rank_ms,
            rank_nodes = nodes,
            rank_iterations = iterations,
            resolved_files = targets.len(),
            "codeindex write phases"
        );
        Ok(writes.len())
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

/// Walk vs `files`.
#[derive(Default)]
struct Diff {
    /// New or changed files to parse and write.
    changed: Vec<Entry>,
    /// `files.id`s to delete first: changed files' old rows and gone files.
    delete: Vec<i64>,
    /// Files no longer present.
    gone: usize,
    /// `files` rows before this diff (0 = first index).
    known: usize,
    /// The walk hit [`walk::MAX_FILES`].
    truncated: bool,
}

impl Diff {
    /// More than [`SUSPECT_GONE_PERCENT`] of the indexed files vanished.
    fn suspect(&self) -> bool {
        self.gone * 100 > self.known * SUSPECT_GONE_PERCENT
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
