//! The bundle on disk + the FTS5 index (design doc 10 §2–3).
//!
//! Files are the truth; `knowledge.sqlite` is derived from them (`sync`) and
//! can be deleted at any time. Every write goes through [`Knowledge::write_concept`]
//! under one mutex: file, rows, `index.md`, `log.md`, commit, event.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use okf::{ConceptId, Document, RESERVED_FILENAMES, Value};
use runtime::{Runtime, SessionId};
use sqlx::FromRow;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::repo::Repo;
use crate::types::{
    AgentScope, Change, Concept, History, Hit, KnowledgeOp, Meta, Put, Receipt, Revision, Scoper,
    Source, Status, Sync, USER_OWNER, Uses, VerifyOutcome, Visibility,
};
use crate::{BODY_MAX, Consolidator, DIR, INDEX_FILE, KnowledgeChanged};

const LOG_FILE: &str = "log.md";
const INDEX_MD: &str = "index.md";

/// Query stopwords (10 §3.3): English function words plus chat/code verbs
/// that appear in nearly every turn and would match the whole catalog.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "file", "files", "use", "run", "please",
    "let", "can", "into", "your", "you", "are", "was", "were", "have", "has", "had", "not", "but",
    "all", "any", "one", "two", "new", "old", "set", "get", "put", "see", "now", "here", "there",
    "what", "when", "which", "who", "how", "why", "also", "just", "like", "make", "made", "need",
    "needs", "want", "wants", "would", "could", "should", "will", "shall", "may", "might", "must",
    "than", "then", "them", "they", "their", "about", "after", "before", "over", "under",
    "between", "each", "other", "some", "such", "very", "more", "most", "less", "least", "only",
    "own", "same", "too", "read",
];

const CREATE_CONCEPTS: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS concepts USING fts5(\
    id UNINDEXED, type, title, description, tags, body, \
    scope UNINDEXED, source UNINDEXED, timestamp UNINDEXED, \
    owner UNINDEXED, visibility UNINDEXED, \
    status UNINDEXED, hint UNINDEXED, evidence UNINDEXED, verified UNINDEXED, \
    valid_until UNINDEXED, superseded_by UNINDEXED, merged_into UNINDEXED, \
    path UNINDEXED, mtime UNINDEXED, size UNINDEXED, \
    tokenize = 'porter unicode61 remove_diacritics 2')";
/// Columns of [`MetaRow`], in `SELECT` order.
pub(crate) const META_COLS: &str = "id, type, title, description, tags, scope, source, timestamp, owner, visibility, \
    status, hint, evidence, verified, valid_until, superseded_by, merged_into";
const CREATE_LINKS: &str = "CREATE TABLE IF NOT EXISTS links (src TEXT NOT NULL, dst TEXT NOT NULL, PRIMARY KEY (src, dst))";
const CREATE_LINKS_DST: &str = "CREATE INDEX IF NOT EXISTS links_dst ON links(dst)";
/// 136 §2.1: usage counters. DURABLE — `sync` never touches it; deleting the
/// index file loses the counts (design 10's "deletable" rule is amended).
const CREATE_USES: &str = "CREATE TABLE IF NOT EXISTS uses (id TEXT PRIMARY KEY, shown INTEGER NOT NULL DEFAULT 0, \
    opened INTEGER NOT NULL DEFAULT 0, last_shown TEXT, last_opened TEXT)";

/// Handle to `<data_dir>/knowledge/` and `<data_dir>/knowledge.sqlite`.
/// Every write path runs under `write` (10 §2.3).
pub struct Knowledge {
    root: PathBuf,
    pub(crate) db: SqlitePool,
    pub(crate) repo: Option<Arc<Repo>>,
    /// Publishes [`KnowledgeChanged`] after every write (10 §8, 12 §9).
    rt: Runtime,
    write: tokio::sync::Mutex<()>,
    /// Paths `sync` has already warned about; a broken file is reported once (10 §11).
    warned: Mutex<HashSet<PathBuf>>,
    /// Root-relative paths the last `sync` could not read or parse, with the
    /// error: their rows (if any) are stale and left out of human search (49 §3.3).
    pub(crate) stale: Mutex<HashMap<String, String>>,
    /// Human search snapshots (49 §4.3); invalidated at every index mutation.
    pub(crate) search: crate::search::Caches,
    /// Which agent a session is (67 §9), from [`Knowledge::set_scoper`]; `None`
    /// = every session is the user's.
    scoper: std::sync::RwLock<Option<Arc<dyn Scoper>>>,
    /// Who may run dreaming's consolidation ops (136 §2.1), from
    /// [`Knowledge::set_consolidator`]; `None` = nobody yet.
    consolidator: std::sync::RwLock<Option<Arc<dyn Consolidator>>>,
}

/// Frontmatter columns of a row, as `list`/`search`/`get` read them ([`META_COLS`]).
#[derive(FromRow)]
pub(crate) struct MetaRow {
    id: String,
    #[sqlx(rename = "type")]
    type_: String,
    title: String,
    description: String,
    tags: String,
    scope: Option<String>,
    source: Option<String>,
    timestamp: Option<String>,
    owner: String,
    visibility: String,
    status: String,
    hint: i64,
    evidence: String,
    verified: Option<String>,
    valid_until: Option<String>,
    superseded_by: Option<String>,
    merged_into: Option<String>,
}

impl From<MetaRow> for Meta {
    fn from(r: MetaRow) -> Self {
        Self {
            id: r.id,
            type_: r.type_,
            title: r.title,
            description: r.description,
            tags: r.tags.split_whitespace().map(str::to_owned).collect(),
            scope: r.scope.map(PathBuf::from),
            source: r.source.as_deref().and_then(Source::parse),
            timestamp: r.timestamp,
            owner: r.owner,
            visibility: Visibility::parse(&r.visibility).unwrap_or_default(),
            status: Status::parse(&r.status).unwrap_or_default(),
            hint: r.hint != 0,
            evidence: r.evidence.split_whitespace().map(str::to_owned).collect(),
            verified: r.verified,
            valid_until: r.valid_until,
            superseded_by: r.superseded_by,
            merged_into: r.merged_into,
        }
    }
}

#[derive(FromRow)]
struct HitRow {
    #[sqlx(flatten)]
    meta: MetaRow,
    body: String,
    s: f64,
    snip: String,
}

/// One concept as it goes into the index.
struct Indexed {
    meta: Meta,
    body: String,
    /// Root-relative path.
    path: String,
    mtime: i64,
    size: i64,
    /// Outbound link targets (ids), deduplicated.
    links: Vec<String>,
}

impl Indexed {
    fn new(root: &Path, abs: &Path, doc: &Document, mtime: i64, size: i64) -> anyhow::Result<Self> {
        let cid = ConceptId::from_path(root, abs)?;
        let id = cid.to_string();
        let fm = &doc.frontmatter;
        let meta = Meta {
            type_: fm.type_().unwrap_or_default(),
            title: fm
                .title()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| id.clone()),
            description: fm.description().unwrap_or_default(),
            tags: fm.tags(),
            scope: fm
                .get("scope")
                .and_then(Value::as_display_string)
                .map(PathBuf::from),
            source: fm
                .get("source")
                .and_then(Value::as_display_string)
                .and_then(|s| Source::parse(&s)),
            timestamp: fm.timestamp(),
            // 67 §9 defaults for files nobody stamped: the user's, shared.
            owner: fm
                .get("owner")
                .and_then(Value::as_display_string)
                .map(|o| o.trim().to_owned())
                .filter(|o| !o.is_empty())
                .unwrap_or_else(|| USER_OWNER.to_owned()),
            visibility: fm
                .get("visibility")
                .and_then(Value::as_display_string)
                .and_then(|v| Visibility::parse(&v))
                .unwrap_or_default(),
            // 136 §5.1: absent = tidy, hinted, no evidence, never verified.
            status: fm
                .get("status")
                .and_then(Value::as_display_string)
                .and_then(|s| Status::parse(&s))
                .unwrap_or_default(),
            hint: fm.get("hint").and_then(fm_bool).unwrap_or(true),
            evidence: fm_strings(fm.get("evidence")),
            verified: fm_string(fm.get("verified")),
            valid_until: fm_string(fm.get("valid_until")),
            superseded_by: fm_string(fm.get("superseded_by")),
            merged_into: fm_string(fm.get("merged_into")),
            id,
        };
        let links: BTreeSet<String> = doc
            .links()
            .iter()
            .filter_map(|l| l.resolve(&cid))
            .map(|c| c.to_string())
            .collect();
        Ok(Self {
            meta,
            body: doc.body.clone(),
            path: abs.strip_prefix(root)?.to_string_lossy().into_owned(),
            mtime,
            size,
            links: links.into_iter().collect(),
        })
    }

    /// Read, stat and parse a bundle file.
    fn load(root: &Path, abs: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(abs)?;
        let (mtime, size) = stat(abs)?;
        Self::new(root, abs, &Document::parse(&text)?, mtime, size)
    }
}

/// A scalar frontmatter value as trimmed text; `None` when absent, empty or not a scalar.
fn fm_string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_display_string)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `hint: true|false` as written, or `"true"`/`"false"` text.
fn fm_bool(v: &Value) -> Option<bool> {
    v.as_bool()
        .or_else(|| match v.as_str().map(str::trim) {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        })
}

/// A list value (`evidence: [a, b]`), or one scalar as a one-item list.
fn fm_strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(Value::as_display_string)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        other => fm_string(other).into_iter().collect(),
    }
}

/// `(mtime as unix nanos, size)` — the sync key (10 §3.2).
fn stat(path: &Path) -> std::io::Result<(i64, i64)> {
    let m = std::fs::metadata(path)?;
    let mtime = m
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok((mtime, m.len() as i64))
}

/// Normalise and validate an id (10 §2.1): trimmed, lowercased, no leading
/// `/` or trailing `.md`, a valid `ConceptId`, ≤ 3 segments, no reserved name.
pub(crate) fn concept_id(raw: &str) -> anyhow::Result<ConceptId> {
    let id = raw.trim().to_lowercase();
    let id = id.trim_start_matches('/');
    let id = id.strip_suffix(".md").unwrap_or(id);
    let cid = ConceptId::parse(id).map_err(|e| anyhow!("invalid id {raw:?}: {e}"))?;
    if cid.segments().len() > 3 {
        bail!("invalid id {raw:?}: more than 3 segments");
    }
    let reserved = RESERVED_FILENAMES
        .iter()
        .filter_map(|f| f.strip_suffix(".md"));
    if let Some(seg) = cid
        .segments()
        .iter()
        .find(|s| reserved.clone().any(|r| r == s.as_str()))
    {
        bail!("invalid id {raw:?}: {seg:?} is a reserved name");
    }
    Ok(cid)
}

/// `put` body cap (10 §2.2).
fn body_within_limit(len: usize) -> anyhow::Result<()> {
    if len > BODY_MAX {
        bail!(
            "body is {len} bytes; the limit is {BODY_MAX} — link to other concepts instead of pasting"
        );
    }
    Ok(())
}

/// Set the writer and the write time (10 §2.3).
fn stamp(fm: &mut okf::Frontmatter, source: Source) {
    fm.set("source", Value::String(source.as_str().to_owned()));
    fm.set(
        "timestamp",
        Value::String(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    );
}

/// `tmp` + rename; parents created.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Design 59 §7.1: a concept that already has a document gets the new file
/// text imported as one block-diff transaction, so an edit that arrived
/// through the file (`knowledge_put`, `write`, `restore`, `sync`) lands in
/// the open editor. Migration is on demand — no document is created here —
/// and the document's own export is recognised by hash and skipped. Fire and
/// forget: the write path never waits on the document actor or its helper.
fn notify_document(id: String, path: PathBuf) {
    let Some(docs) = document::Handle::lookup() else {
        return;
    };
    tokio::spawn(async move {
        let Some(doc) = docs.resolve_knowledge(&id).await else {
            return;
        };
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(id, error = %e, "knowledge → document import skipped: unreadable");
                return;
            }
        };
        if let Err(e) = docs
            .import_markdown(doc, text, document::Principal::File)
            .await
        {
            tracing::warn!(id, error = %e, "knowledge → document import failed");
        }
    });
}

/// `okf::index::regenerate_indexes` minus dot-directories: the crate's walk
/// lists `.git` under `# Subdirectories`. Same output otherwise (deepest-first
/// so a parent reuses its children's descriptions); files whose text is
/// unchanged are not rewritten. Returns the absolute paths written.
fn regenerate_indexes(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    use okf::index::{IndexEntry, build_index_text, default_synthesize};
    use std::fs;

    fn hidden(p: &Path) -> bool {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    }
    fn is_md(p: &Path) -> bool {
        p.extension().is_some_and(|e| e == "md")
    }
    /// Pushes `dir` and reports `true` when it holds a `.md` at any depth.
    fn collect(dir: &Path, dirs: &mut Vec<PathBuf>) -> std::io::Result<bool> {
        let mut has_md = false;
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            has_md |= if path.is_dir() {
                !hidden(&path) && collect(&path, dirs)?
            } else {
                is_md(&path)
            };
        }
        if has_md {
            dirs.push(dir.to_path_buf());
        }
        Ok(has_md)
    }

    let mut dirs = Vec::new();
    if root.exists() {
        collect(root, &mut dirs)?;
    }
    dirs.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
    let mut descriptions: HashMap<PathBuf, String> = HashMap::new();
    let mut written = Vec::new();
    for dir in &dirs {
        let mut children: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        children.sort();
        let mut entries = Vec::new();
        for child in children {
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name == INDEX_MD {
                continue;
            }
            if child.is_dir() {
                if hidden(&child) {
                    continue;
                }
                entries.push(IndexEntry {
                    type_: "Subdirectories".into(),
                    title: name.clone(),
                    link: format!("{name}/{INDEX_MD}"),
                    description: descriptions.get(&child).cloned().unwrap_or_default(),
                });
            } else if is_md(&child) {
                let Some(doc) = fs::read_to_string(&child)
                    .ok()
                    .and_then(|t| Document::parse(&t).ok())
                else {
                    continue;
                };
                let stem = child
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                entries.push(IndexEntry {
                    type_: doc.frontmatter.type_().unwrap_or_default(),
                    title: doc.frontmatter.title().unwrap_or(stem),
                    link: name,
                    description: doc.frontmatter.description().unwrap_or_default(),
                });
            }
        }
        if entries.is_empty() {
            continue;
        }
        let index_path = dir.join(INDEX_MD);
        let text = build_index_text(&entries);
        if fs::read_to_string(&index_path).ok().as_deref() != Some(text.as_str()) {
            fs::write(&index_path, &text)?;
            written.push(index_path);
        }
        if dir == root {
            continue;
        }
        let desc = match entries.as_slice() {
            [only] if !only.description.is_empty() => only.description.clone(),
            _ => {
                let pairs: Vec<(String, String)> = entries
                    .iter()
                    .map(|e| (e.title.clone(), e.description.clone()))
                    .collect();
                let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy();
                default_synthesize(&rel, &pairs)
            }
        };
        descriptions.insert(dir.clone(), desc);
    }
    Ok(written)
}

/// Steps 4–5 of a write (10 §2.3): regenerate every `index.md`, append `entry`
/// under today's date in `log.md`. Returns the root-relative paths touched.
fn finish_files(root: &Path, entry: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut touched: Vec<PathBuf> = regenerate_indexes(root)
        .context("regenerate index.md")?
        .into_iter()
        .filter_map(|p| p.strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    let log_path = root.join(LOG_FILE);
    let mut log = match std::fs::read_to_string(&log_path) {
        Ok(text) => okf::Log::parse(&text),
        Err(_) => okf::Log {
            title: Some("Directory Update Log".into()),
            days: Vec::new(),
        },
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let e = okf::log::LogEntry {
        kind: None,
        text: entry.to_owned(),
    };
    match log.days.first_mut() {
        Some(day) if day.date == today => day.entries.push(e),
        _ => log.days.insert(
            0,
            okf::log::LogDay {
                date: today,
                entries: vec![e],
            },
        ),
    }
    std::fs::write(&log_path, log.to_markdown()).context("write log.md")?;
    touched.push(PathBuf::from(LOG_FILE));
    Ok(touched)
}

async fn open_db(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await?;
    // 67 §9 added `owner`/`visibility`, 136 §2.1 the status columns: an index
    // from before lacks them. `concepts` is derived, so drop it and let `sync`
    // rebuild; `uses` is durable and is never dropped.
    if sqlx::query("SELECT owner, visibility, status, hint, evidence, verified, valid_until, superseded_by, merged_into FROM concepts LIMIT 0")
        .execute(&pool)
        .await
        .is_err()
    {
        sqlx::query("DROP TABLE IF EXISTS concepts")
            .execute(&pool)
            .await?;
    }
    for sql in [CREATE_CONCEPTS, CREATE_LINKS, CREATE_LINKS_DST, CREATE_USES] {
        sqlx::query(sql).execute(&pool).await?;
    }
    Ok(pool)
}

/// Outcome of [`Knowledge::write_concept`].
struct Written {
    meta: Meta,
    created: bool,
    /// Abbreviated commit id; `None` without a repo or when the commit failed.
    commit: Option<String>,
}

impl Knowledge {
    /// Create/open the bundle dir, open the repo (`gix::open | init`, then
    /// reconcile), open the index (`CREATE VIRTUAL TABLE IF NOT EXISTS`, no
    /// migrations: it is derived), run `sync` (10 §2.1, §2.4, §3.2).
    pub async fn open(data_dir: &Path, rt: Runtime) -> anyhow::Result<Arc<Self>> {
        let root = data_dir.join(DIR);
        std::fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
        let repo = {
            let root = root.clone();
            match tokio::task::spawn_blocking(move || Repo::open(&root)).await {
                Ok(Ok(repo)) => Some(Arc::new(repo)),
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = format!("{e:#}"),
                        "knowledge repo unavailable; history is off"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(error = %e, "knowledge repo unavailable; history is off");
                    None
                }
            }
        };
        let db_path = data_dir.join(INDEX_FILE);
        let db = match open_db(&db_path).await {
            Ok(db) => db,
            Err(e) => {
                // The index is derived: a corrupt file is thrown away and rebuilt by `sync` (10 §11).
                tracing::warn!(error = format!("{e:#}"), path = %db_path.display(), "knowledge index unusable; rebuilding");
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
                }
                open_db(&db_path)
                    .await
                    .with_context(|| format!("open {}", db_path.display()))?
            }
        };
        let k = Arc::new(Self {
            root,
            db,
            repo,
            rt,
            write: tokio::sync::Mutex::new(()),
            warned: Mutex::new(HashSet::new()),
            stale: Mutex::new(HashMap::new()),
            search: crate::search::Caches::default(),
            scoper: std::sync::RwLock::new(None),
            consolidator: std::sync::RwLock::new(None),
        });
        let s = k.sync().await?;
        if !s.is_empty() {
            tracing::info!(
                added = s.added,
                updated = s.updated,
                removed = s.removed,
                "knowledge sync"
            );
        }
        Ok(k)
    }

    /// Bundle root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reconcile rows with the files by (mtime, size); commit external edits (10 §3.2).
    pub async fn sync(&self) -> anyhow::Result<Sync> {
        let _g = self.write.lock().await;
        let rows: Vec<(String, String, i64, i64)> =
            sqlx::query_as("SELECT id, path, mtime, size FROM concepts")
                .fetch_all(&self.db)
                .await?;
        let known: HashMap<String, (String, i64, i64)> = rows
            .into_iter()
            .map(|(id, path, mtime, size)| (path, (id, mtime, size)))
            .collect();
        let keys: HashMap<String, (i64, i64)> = known
            .iter()
            .map(|(p, (_, m, s))| (p.clone(), (*m, *s)))
            .collect();
        let root = self.root.clone();
        let (changed, seen, failed) =
            tokio::task::spawn_blocking(move || walk(&root, &keys)).await?;
        {
            let mut warned = self.warned.lock().unwrap_or_else(|p| p.into_inner());
            let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
            stale.clear();
            for (path, error) in failed {
                if let Ok(rel) = path.strip_prefix(&self.root) {
                    stale.insert(rel.to_string_lossy().into_owned(), error.clone());
                }
                if warned.insert(path.clone()) {
                    tracing::warn!(path = %path.display(), error, "knowledge: unparsable concept skipped");
                }
            }
        }
        // The stale set may have changed even when no row did; a failed human
        // build is retried on the next explicit sync (49 §3.3).
        self.search.invalidate();

        let mut out = Sync::default();
        let mut written = Vec::new();
        let mut removed = Vec::new();
        let mut names = Vec::new();
        let mut events = Vec::new();
        for (path, (id, ..)) in &known {
            if seen.contains(path) {
                continue;
            }
            self.remove_rows(id).await?;
            out.removed += 1;
            removed.push(PathBuf::from(path));
            names.push(format!("remove {id}"));
            events.push((id.clone(), KnowledgeOp::Delete));
        }
        for ix in changed {
            if known.contains_key(&ix.path) {
                out.updated += 1;
            } else {
                out.added += 1;
            }
            self.upsert(&ix).await?;
            notify_document(ix.meta.id.clone(), self.root.join(&ix.path));
            written.push(PathBuf::from(&ix.path));
            names.push(format!("edit {}", ix.meta.id));
            events.push((ix.meta.id, KnowledgeOp::Put));
        }
        if !names.is_empty() {
            self.commit(written, removed, format!("sync: {}", names.join(", ")))
                .await;
        }
        for (id, op) in events {
            self.rt
                .publish(KnowledgeChanged {
                    id,
                    op,
                    source: Source::Sync,
                })
                .await;
        }
        Ok(out)
    }

    /// The file verbatim plus links and backlinks.
    pub async fn get(&self, id: &str) -> anyhow::Result<Concept> {
        let cid = concept_id(id)?;
        let id = cid.to_string();
        let path = cid.to_path(&self.root);
        let text = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("no such concept: {id}"))?;
        // Only `META_COLS` (a const) is interpolated; the id is bound.
        let row: Option<MetaRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {META_COLS} FROM concepts WHERE id = ?"
        )))
        .bind(&id)
        .fetch_optional(&self.db)
        .await?;
        let meta = match row {
            Some(r) => r.into(),
            None => {
                // A file `sync` has not seen yet (or one it could not parse before): index it now.
                let (mtime, size) = stat(&path)?;
                let ix = Indexed::new(&self.root, &path, &Document::parse(&text)?, mtime, size)?;
                self.upsert(&ix).await?;
                ix.meta
            }
        };
        let links: Vec<(String, bool)> = sqlx::query_as(
            "SELECT l.dst, EXISTS(SELECT 1 FROM concepts c WHERE c.id = l.dst) FROM links l WHERE l.src = ? ORDER BY l.dst",
        )
        .bind(&id)
        .fetch_all(&self.db)
        .await?;
        let cited_by: Vec<(String,)> =
            sqlx::query_as("SELECT src FROM links WHERE dst = ? ORDER BY src")
                .bind(&id)
                .fetch_all(&self.db)
                .await?;
        Ok(Concept {
            meta,
            text,
            links,
            cited_by: cited_by.into_iter().map(|(s,)| s).collect(),
        })
    }

    /// Install the session → agent source (67 §9). Called once by the roster plugin.
    pub fn set_scoper(&self, s: Arc<dyn Scoper>) {
        *self.scoper.write().unwrap_or_else(|p| p.into_inner()) = Some(s);
    }

    /// Install the "may this session consolidate" answerer (136 §2.1). Called
    /// once by the agent plugin.
    pub fn set_consolidator(&self, c: Arc<dyn Consolidator>) {
        *self.consolidator.write().unwrap_or_else(|p| p.into_inner()) = Some(c);
    }

    /// 136 §2.1: is `session` the dreaming routine's (unattended) or the
    /// owner's own root session? `false` until a [`Consolidator`] is installed.
    pub async fn may_consolidate(&self, session: SessionId) -> bool {
        let c = self
            .consolidator
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        match c {
            Some(c) => c.may_consolidate(session).await,
            None => false,
        }
    }

    /// The scope `session` reads and writes as; `None` = the user's (sees everything).
    fn scope_for(&self, session: Option<SessionId>) -> Option<AgentScope> {
        let s = session?;
        self.scoper
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(|sc| sc.scope_for(s))
    }

    /// `put_for` from the user's side: `owner: user`, `visibility` as given or `shared`.
    pub async fn put(&self, p: Put, source: Source) -> anyhow::Result<(Meta, bool)> {
        self.put_for(p, source, None).await
    }

    /// Validate, write atomically, index, `index.md`, `log.md`, commit, and
    /// report whether the concept was created (10 §2.3). From an agent's
    /// session the file is stamped `owner: agent:<name>` and, unless the put
    /// says otherwise, `visibility: private` (67 §9). 136 §2.1: an existing
    /// file's frontmatter is the starting point — every key the put does not
    /// set (the seven 136 keys when `None`/empty, and any key the harness
    /// does not know) round-trips verbatim.
    pub async fn put_for(
        &self,
        p: Put,
        source: Source,
        session: Option<SessionId>,
    ) -> anyhow::Result<(Meta, bool)> {
        let cid = concept_id(&p.id)?;
        let id = cid.to_string();
        let type_ = p.type_.trim();
        if type_.is_empty() {
            bail!("type must not be empty");
        }
        body_within_limit(p.body.len())?;
        let title = p.title.trim();
        let title = if title.is_empty() { id.as_str() } else { title };
        let description = p
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(title);
        let tags: Vec<Value> = p
            .tags
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| Value::String(t.to_owned()))
            .collect();

        let _g = self.write.lock().await;
        // The existing frontmatter, so unknown keys survive (136 §15 amends 10 §3).
        let mut fm = match std::fs::read_to_string(cid.to_path(&self.root)) {
            Ok(text) => Document::parse(&text)
                .map(|d| d.frontmatter)
                .unwrap_or_default(),
            Err(_) => okf::Frontmatter::new(),
        };
        fm.set("type", Value::String(type_.to_owned()));
        fm.set("title", Value::String(title.to_owned()));
        fm.set("description", Value::String(description.to_owned()));
        if tags.is_empty() {
            fm.as_mapping_mut().remove("tags");
        } else {
            fm.set("tags", Value::Sequence(tags));
        }
        match &p.scope {
            Some(scope) => fm.set("scope", Value::String(scope.to_string_lossy().into_owned())),
            None => {
                fm.as_mapping_mut().remove("scope");
            }
        }
        let scope = self.scope_for(session);
        let owner = scope.as_ref().map_or(USER_OWNER, |s| s.owner.as_str());
        let visibility = p.visibility.unwrap_or(if scope.is_some() {
            Visibility::Private
        } else {
            Visibility::Shared
        });
        fm.set("owner", Value::String(owner.to_owned()));
        fm.set("visibility", Value::String(visibility.as_str().to_owned()));
        // 136 §5.1: only what the put sets; `None`/empty keeps the file's value.
        if let Some(s) = p.status {
            fm.set("status", Value::String(s.as_str().to_owned()));
        }
        if let Some(h) = p.hint {
            fm.set("hint", Value::Bool(h));
        }
        if !p.evidence.is_empty() {
            fm.set(
                "evidence",
                Value::Sequence(p.evidence.iter().cloned().map(Value::String).collect()),
            );
        }
        for (key, value) in [
            ("verified", &p.verified),
            ("valid_until", &p.valid_until),
            ("superseded_by", &p.superseded_by),
            ("merged_into", &p.merged_into),
        ] {
            if let Some(v) = value {
                fm.set(key, Value::String(v.clone()));
            }
        }
        stamp(&mut fm, source);
        let text = Document::new(fm, p.body).serialize();

        let entry = format!("put {id}");
        let w = self.write_concept(&cid, text, &entry, &entry).await?;
        self.rt
            .publish(KnowledgeChanged {
                id,
                op: KnowledgeOp::Put,
                source,
            })
            .await;
        Ok((w.meta, w.created))
    }

    /// `knowledge_share` (67 §9): flip a concept's `visibility`. Allowed for
    /// the owner and its reporting line; the user's sessions always may.
    pub async fn share(
        &self,
        id: &str,
        visibility: Visibility,
        session: Option<SessionId>,
    ) -> anyhow::Result<Meta> {
        let cid = concept_id(id)?;
        let id = cid.to_string();
        let text = tokio::fs::read_to_string(cid.to_path(&self.root))
            .await
            .with_context(|| format!("no such concept: {id}"))?;
        let mut doc = Document::parse(&text)?;
        let owner = doc
            .frontmatter
            .get("owner")
            .and_then(Value::as_display_string)
            .map(|o| o.trim().to_owned())
            .filter(|o| !o.is_empty())
            .unwrap_or_else(|| USER_OWNER.to_owned());
        let scope = self.scope_for(session);
        if !AgentScope::may_share(scope.as_ref(), &owner) {
            bail!("{id} belongs to {owner}, outside your reporting line; ask them to share it");
        }
        doc.frontmatter
            .set("visibility", Value::String(visibility.as_str().to_owned()));
        let text = doc.serialize();

        let _g = self.write.lock().await;
        let entry = format!("share {id} {}", visibility.as_str());
        let w = self.write_concept(&cid, text, &entry, &entry).await?;
        self.rt
            .publish(KnowledgeChanged {
                id,
                op: KnowledgeOp::Put,
                source: Source::Model,
            })
            .await;
        Ok(w.meta)
    }

    /// Write a concept from raw OKF text (the editor path, 10 §2.3): the
    /// frontmatter is kept as given — unknown keys included — plus `source:
    /// user` and a fresh `timestamp`; the body is capped like `put`.
    pub async fn write(&self, id: &str, text: &str) -> anyhow::Result<Meta> {
        let cid = concept_id(id)?;
        let id = cid.to_string();
        let mut doc = Document::parse(text)?;
        body_within_limit(doc.body.len())?;
        stamp(&mut doc.frontmatter, Source::User);
        let text = doc.serialize();

        let _g = self.write.lock().await;
        let w = self
            .write_concept(&cid, text, &format!("put {id}"), &format!("write {id}"))
            .await?;
        self.rt
            .publish(KnowledgeChanged {
                id,
                op: KnowledgeOp::Put,
                source: Source::User,
            })
            .await;
        Ok(w.meta)
    }

    /// Remove the file, its rows, `index.md`, `log.md`, commit.
    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let cid = concept_id(id)?;
        let id = cid.to_string();
        let _g = self.write.lock().await;
        let root = self.root.clone();
        let path = cid.to_path(&root);
        let rel = path.strip_prefix(&root)?.to_path_buf();
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM concepts WHERE id = ?")
            .bind(&id)
            .fetch_optional(&self.db)
            .await?;
        if row.is_none() && !path.exists() {
            bail!("no such concept: {id}");
        }
        let entry = format!("delete {id}");
        let log_entry = entry.clone();
        let (written, mut removed) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            let mut removed = vec![rel];
            // A directory left with only its index.md would keep listing the deleted concept.
            if let Some(dir) = path.parent()
                && dir != root
                && let Ok(rd) = std::fs::read_dir(dir)
                && rd.flatten().all(|e| e.file_name() == INDEX_MD)
            {
                let index = dir.join(INDEX_MD);
                if std::fs::remove_file(&index).is_ok()
                    && let Ok(r) = index.strip_prefix(&root)
                {
                    removed.push(r.to_path_buf());
                }
                let _ = std::fs::remove_dir(dir);
            }
            Ok((finish_files(&root, &log_entry)?, removed))
        })
        .await??;
        self.remove_rows(&id).await?;
        removed.sort();
        self.commit(written, removed, entry).await;
        if let Some(docs) = document::Handle::lookup() {
            let id = id.clone();
            tokio::spawn(async move {
                if let Some(doc) = docs.resolve_knowledge(&id).await
                    && let Err(e) = docs.delete(doc).await
                {
                    tracing::warn!(id, error = %e, "knowledge → document delete failed");
                }
            });
        }
        self.rt
            .publish(KnowledgeChanged {
                id,
                op: KnowledgeOp::Delete,
                source: Source::User,
            })
            .await;
        Ok(())
    }

    /// `list_for` from the user's side: every concept.
    pub async fn list(
        &self,
        prefix: Option<&str>,
        type_: Option<&str>,
        tag: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<Meta>> {
        self.list_for(prefix, type_, tag, limit, None).await
    }

    /// Metas sorted by id, filtered by id prefix, type and tag; from an
    /// agent's session only what it may read (67 §9).
    pub async fn list_for(
        &self,
        prefix: Option<&str>,
        type_: Option<&str>,
        tag: Option<&str>,
        limit: u32,
        session: Option<SessionId>,
    ) -> anyhow::Result<Vec<Meta>> {
        let scope = self.scope_for(session);
        // ponytail: filters run in Rust over every row; the catalog is hundreds of concepts.
        // Only `META_COLS` (a const) is interpolated.
        let rows: Vec<MetaRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {META_COLS} FROM concepts ORDER BY id"
        )))
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(Meta::from)
            .filter(|m| AgentScope::sees(scope.as_ref(), &m.owner, m.visibility))
            .filter(|m| prefix.is_none_or(|p| m.id.starts_with(p)))
            .filter(|m| type_.is_none_or(|t| m.type_ == t))
            .filter(|m| tag.is_none_or(|t| m.tags.iter().any(|x| x == t)))
            .take(limit as usize)
            .collect())
    }

    /// `search_for` from the user's side: everything, no stubs.
    pub async fn search(&self, query: &str, cwd: &Path, limit: u32) -> anyhow::Result<Vec<Hit>> {
        self.search_for(query, cwd, limit, None).await
    }

    /// bm25 + scope boost, best first (10 §3.3). From an agent's session
    /// (67 §9): its own items first, then what it may read; a match on
    /// another agent's private item is a stub — id, owner, a title saying whom
    /// to ask — with no body or snippet.
    pub async fn search_for(
        &self,
        query: &str,
        cwd: &Path,
        limit: u32,
        session: Option<SessionId>,
    ) -> anyhow::Result<Vec<Hit>> {
        let scope = self.scope_for(session);
        Ok(self
            .ranked(&terms(query), cwd, limit, scope.as_ref(), true)
            .await?
            .into_iter()
            .map(|(hit, _)| hit)
            .collect())
    }

    /// `hint_for` from the user's side.
    pub async fn hint(
        &self,
        text: &str,
        cwd: &Path,
        exclude: &BTreeSet<String>,
        limit: u32,
    ) -> anyhow::Result<Vec<Meta>> {
        self.hint_for(text, cwd, exclude, limit, None).await
    }

    /// `search` with the ≥2-term rule, minus `exclude`, metas only (10 §3.3);
    /// from an agent's session only what it may read, own items first (67 §9).
    pub async fn hint_for(
        &self,
        text: &str,
        cwd: &Path,
        exclude: &BTreeSet<String>,
        limit: u32,
        session: Option<SessionId>,
    ) -> anyhow::Result<Vec<Meta>> {
        let scope = self.scope_for(session);
        let terms = terms(text);
        let need = terms.len().min(2);
        let hits = self
            .ranked(
                &terms,
                cwd,
                limit + exclude.len() as u32,
                scope.as_ref(),
                false,
            )
            .await?;
        let mut out = Vec::new();
        for (hit, body) in hits {
            if out.len() >= limit as usize {
                break;
            }
            // 136 §5.3: `hint: false` and a non-live status never reach a note (rank is W1's).
            if exclude.contains(&hit.meta.id) || !hit.meta.hint || !hit.meta.status.is_live() {
                continue;
            }
            let m = &hit.meta;
            let hay = format!(
                "{} {} {} {}",
                m.title,
                m.description,
                m.tags.join(" "),
                body
            )
            .to_lowercase();
            let tokens: HashSet<&str> = hay
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .collect();
            let matched = terms
                .iter()
                .filter(|t| tokens.iter().any(|tok| tok.starts_with(t.as_str())))
                .count();
            if matched >= need {
                out.push(hit.meta);
            }
        }
        Ok(out)
    }

    /// Commits touching `id` (or the whole bundle), newest first, plus the
    /// unified diff of the latest change to `id` (10 §2.4).
    pub async fn history(&self, id: Option<&str>, limit: u32) -> anyhow::Result<History> {
        let Some(repo) = self.repo.clone() else {
            bail!("history unavailable: no repository")
        };
        let path = id
            .map(concept_id)
            .transpose()?
            .map(|c| c.to_path(Path::new("")));
        tokio::task::spawn_blocking(move || {
            let changes = repo.history(path.as_deref(), limit as usize)?;
            let diff = match &path {
                Some(p) => repo.last_diff(p)?,
                None => None,
            };
            Ok(History { changes, diff })
        })
        .await?
    }

    /// `commit` against its first parent as concept-file diffs, restricted to
    /// `id` when given (10 §2.4). Read-only: nothing is written or committed.
    pub async fn revision(&self, commit: &str, id: Option<&str>) -> anyhow::Result<Revision> {
        let Some(repo) = self.repo.clone() else {
            bail!("history unavailable: no repository")
        };
        let path = id
            .map(concept_id)
            .transpose()?
            .map(|c| c.to_path(Path::new("")));
        let commit = commit.to_owned();
        tokio::task::spawn_blocking(move || repo.revision(&commit, path.as_deref())).await?
    }

    /// Rewrite `<id>.md` from `commit` (default: the commit before the latest
    /// change to `id`) through the ordinary write path (10 §2.4).
    pub async fn restore(&self, id: &str, commit: Option<&str>) -> anyhow::Result<Change> {
        let Some(repo) = self.repo.clone() else {
            bail!("restore unavailable: no repository")
        };
        let cid = concept_id(id)?;
        let id = cid.to_string();
        let rel = cid.to_path(Path::new(""));
        let commit = commit.map(str::to_owned);
        let (commit, text) = {
            let id = id.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String)> {
                let commit = match commit {
                    Some(c) => c,
                    None => repo.previous(&rel)?.context("nothing to restore")?,
                };
                let bytes = repo
                    .show(&rel, &commit)?
                    .with_context(|| format!("{id} is not in commit {commit}"))?;
                Ok((
                    commit,
                    String::from_utf8(bytes).context("concept is not UTF-8")?,
                ))
            })
            .await??
        };
        let _g = self.write.lock().await;
        let message = format!("restore {id} from {commit}");
        let w = self
            .write_concept(&cid, text, &format!("restore {id}"), &message)
            .await?;
        let sha = w.commit.context("restore: commit failed")?;
        self.rt
            .publish(KnowledgeChanged {
                id,
                op: KnowledgeOp::Put,
                source: Source::User,
            })
            .await;
        Ok(Change {
            commit: sha,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            message,
        })
    }

    /// Steps 2–6 of a write (10 §2.3); the caller holds `write` and publishes.
    async fn write_concept(
        &self,
        cid: &ConceptId,
        text: String,
        log_entry: &str,
        message: &str,
    ) -> anyhow::Result<Written> {
        let doc = Document::parse(&text)?;
        doc.validate_conformance().map_err(|e| anyhow!("{e}"))?;
        let root = self.root.clone();
        let path = cid.to_path(&root);
        let rel = path.strip_prefix(&root)?.to_path_buf();
        let created = !path.exists();
        let entry = log_entry.to_owned();
        let (ix, mut touched) =
            tokio::task::spawn_blocking(move || -> anyhow::Result<(Indexed, Vec<PathBuf>)> {
                write_atomic(&path, text.as_bytes())
                    .with_context(|| format!("write {}", path.display()))?;
                let (mtime, size) = stat(&path)?;
                let ix = Indexed::new(&root, &path, &doc, mtime, size)?;
                Ok((ix, finish_files(&root, &entry)?))
            })
            .await??;
        self.upsert(&ix).await?;
        notify_document(cid.to_string(), cid.to_path(&self.root));
        touched.push(rel);
        let commit = self.commit(touched, Vec::new(), message.to_owned()).await;
        Ok(Written {
            meta: ix.meta,
            created,
            commit,
        })
    }

    /// Commit through the repo when present. A failure is logged, never
    /// fatal: the files stand and the next `open` reconciles (10 §11).
    async fn commit(
        &self,
        written: Vec<PathBuf>,
        removed: Vec<PathBuf>,
        message: String,
    ) -> Option<String> {
        let repo = self.repo.clone()?;
        match tokio::task::spawn_blocking(move || repo.commit(&written, &removed, &message)).await {
            Ok(Ok(id)) => Some(id),
            Ok(Err(e)) => {
                tracing::warn!(error = format!("{e:#}"), "knowledge commit failed");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "knowledge commit failed");
                None
            }
        }
    }

    /// Replace the concept's row and links. FTS5 has no UPSERT: delete, then insert.
    async fn upsert(&self, ix: &Indexed) -> anyhow::Result<()> {
        let m = &ix.meta;
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM concepts WHERE id = ?")
            .bind(&m.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO concepts (id, type, title, description, tags, body, scope, source, timestamp, owner, visibility, \
             status, hint, evidence, verified, valid_until, superseded_by, merged_into, path, mtime, size) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&m.id)
        .bind(&m.type_)
        .bind(&m.title)
        .bind(&m.description)
        .bind(m.tags.join(" "))
        .bind(&ix.body)
        .bind(m.scope.as_ref().map(|p| p.to_string_lossy().into_owned()))
        .bind(m.source.map(Source::as_str))
        .bind(&m.timestamp)
        .bind(&m.owner)
        .bind(m.visibility.as_str())
        .bind(m.status.as_str())
        .bind(i64::from(m.hint))
        .bind(m.evidence.join(" "))
        .bind(&m.verified)
        .bind(&m.valid_until)
        .bind(&m.superseded_by)
        .bind(&m.merged_into)
        .bind(&ix.path)
        .bind(ix.mtime)
        .bind(ix.size)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM links WHERE src = ?")
            .bind(&m.id)
            .execute(&mut *tx)
            .await?;
        for dst in &ix.links {
            sqlx::query("INSERT OR IGNORE INTO links (src, dst) VALUES (?, ?)")
                .bind(&m.id)
                .bind(dst)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.stale
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&ix.path);
        self.search.invalidate();
        Ok(())
    }

    async fn remove_rows(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM concepts WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM links WHERE src = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.search.invalidate();
        Ok(())
    }

    /// FTS query for `terms`, scope-adjusted and sorted, with each hit's body
    /// (the ≥2-term rule needs it). Empty terms → nothing. With an agent
    /// `scope` (67 §9): its own items rank first; items it may not read are
    /// replaced by a stub (`stubs`) or dropped.
    async fn ranked(
        &self,
        terms: &[String],
        cwd: &Path,
        limit: u32,
        scope: Option<&AgentScope>,
        stubs: bool,
    ) -> anyhow::Result<Vec<(Hit, String)>> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        // Only `META_COLS` (a const) is interpolated; the MATCH query and limit are bound.
        let rows: Vec<HitRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {META_COLS}, body, \
             bm25(concepts, 0, 3.0, 10.0, 5.0, 4.0, 1.0) AS s, snippet(concepts, 5, '', '', '…', 12) AS snip \
             FROM concepts WHERE concepts MATCH ? ORDER BY s LIMIT ?"
        )))
        .bind(query)
        .bind(i64::from(limit) * 4)
        .fetch_all(&self.db)
        .await?;
        let mut hits: Vec<(Hit, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let meta: Meta = r.meta.into();
                let boost = match &meta.scope {
                    Some(scope) if cwd.starts_with(scope) => 1.5,
                    Some(_) => 0.5,
                    None => 1.0,
                };
                let score = (-r.s * boost) as f32;
                if AgentScope::sees(scope, &meta.owner, meta.visibility) {
                    Some((
                        Hit {
                            meta,
                            score,
                            snippet: r.snip,
                        },
                        r.body,
                    ))
                } else if stubs {
                    Some((
                        Hit {
                            meta: stub(meta),
                            score,
                            snippet: String::new(),
                        },
                        String::new(),
                    ))
                } else {
                    None
                }
            })
            .collect();
        let own = |h: &Hit| scope.is_some_and(|s| s.owner == h.meta.owner);
        hits.sort_by(|a, b| {
            own(&b.0)
                .cmp(&own(&a.0))
                .then_with(|| b.0.score.total_cmp(&a.0.score))
        });
        hits.truncate(limit as usize);
        Ok(hits)
    }
}

/// What a search reports about an item the caller may not read (67 §9):
/// that it exists and whom to ask, nothing of its content.
fn stub(m: Meta) -> Meta {
    Meta {
        title: format!("private — ask {}", m.owner),
        description: String::new(),
        tags: Vec::new(),
        scope: None,
        source: None,
        timestamp: None,
        evidence: Vec::new(),
        ..m
    }
}

/// RFC 3339 seconds, UTC — the `uses` timestamp format.
fn rfc3339(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

impl Knowledge {
    /// 136 §2.1: one impression per id — `uses.shown += 1`, `last_shown = at`.
    /// Unknown ids get a row; nothing else is touched.
    pub async fn record_shown(
        &self,
        ids: &[String],
        at: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let at = rfc3339(at);
        let mut tx = self.db.begin().await?;
        for id in ids {
            sqlx::query(
                "INSERT INTO uses (id, shown, last_shown) VALUES (?, 1, ?) \
                 ON CONFLICT(id) DO UPDATE SET shown = shown + 1, last_shown = excluded.last_shown",
            )
            .bind(id)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 136 §2.1: one open — `uses.opened += 1`, `last_opened = at`.
    pub async fn record_opened(
        &self,
        id: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO uses (id, opened, last_opened) VALUES (?, 1, ?) \
             ON CONFLICT(id) DO UPDATE SET opened = opened + 1, last_opened = excluded.last_opened",
        )
        .bind(id)
        .bind(rfc3339(at))
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// 136 §2.1: the counters of `ids`, in the order given; an id never shown
    /// or opened reports zeros.
    pub async fn stats(&self, ids: &[String]) -> anyhow::Result<Vec<Uses>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let row: Option<(i64, i64, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT shown, opened, last_shown, last_opened FROM uses WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&self.db)
            .await?;
            let (shown, opened, last_shown, last_opened) = row.unwrap_or_default();
            out.push(Uses {
                id: id.clone(),
                shown: shown.max(0) as u64,
                opened: opened.max(0) as u64,
                last_shown,
                last_opened,
            });
        }
        Ok(out)
    }

    /// 136 §3.1 item 4: fold `ids` into `into` (body given; `evidence` and
    /// tags = union; scope = the canonical's), rewrite each folded id as a
    /// `status: merged, merged_into: <into>, hint: false` stub, retarget
    /// `links` rows, one commit `dreaming merge <into>`.
    pub async fn merge(
        &self,
        ids: &[String],
        into: &str,
        body: &str,
        evidence: &[String],
        scope: &AgentScope,
    ) -> anyhow::Result<Receipt> {
        // 136 W1
        let _ = (ids, into, body, evidence, scope);
        bail!("not built: W1 (knowledge merge)")
    }

    /// 136 §3.1 item 4: `superseded_by`, `valid_until`, `hint: false` on the
    /// loser; `note` goes under the loser's body.
    pub async fn supersede(
        &self,
        loser: &str,
        winner: &str,
        valid_until: Option<&str>,
        note: Option<&str>,
    ) -> anyhow::Result<Receipt> {
        // 136 W1
        let _ = (loser, winner, valid_until, note);
        bail!("not built: W1 (knowledge supersede)")
    }

    /// 136 §3.1 item 4: `status: archived, hint: false`; `reason` appended
    /// under `## Archived`.
    pub async fn archive(&self, id: &str, reason: &str) -> anyhow::Result<Receipt> {
        // 136 W1
        let _ = (id, reason);
        bail!("not built: W1 (knowledge archive)")
    }

    /// 136 §3.1 item 4: `verified: <today>` (ok) or `status: stale` + note (stale).
    pub async fn verify(
        &self,
        id: &str,
        outcome: VerifyOutcome,
        note: Option<&str>,
    ) -> anyhow::Result<Receipt> {
        // 136 W1
        let _ = (id, outcome, note);
        bail!("not built: W1 (knowledge verify)")
    }

    /// 136 §3.1 item 4: set `title`; the old title becomes a leading
    /// "Summary" paragraph (`summary` when given, else the old title text).
    pub async fn retitle(&self, id: &str, title: &str, summary: &str) -> anyhow::Result<Receipt> {
        // 136 W1
        let _ = (id, title, summary);
        bail!("not built: W1 (knowledge retitle)")
    }
}

type Walked = (Vec<Indexed>, HashSet<String>, Vec<(PathBuf, String)>);

/// Walk the bundle (skipping `.git/`, `index.md`, `log.md`, non-`.md`):
/// files whose (mtime, size) differ from `known`, parsed; every path seen;
/// paths that failed to parse with the error.
fn walk(root: &Path, known: &HashMap<String, (i64, i64)>) -> Walked {
    let mut changed = Vec::new();
    let mut seen = HashSet::new();
    let mut failed = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name != ".git" {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".md") || RESERVED_FILENAMES.contains(&name.as_ref()) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().into_owned();
            let Ok(key) = stat(&path) else { continue };
            seen.insert(rel.clone());
            if known.get(&rel) == Some(&key) {
                continue;
            }
            match Indexed::load(root, &path) {
                Ok(ix) => changed.push(ix),
                Err(e) => failed.push((path, format!("{e:#}"))),
            }
        }
    }
    (changed, seen, failed)
}

/// Query terms (10 §3.3): split on non-alphanumeric, lowercase, drop `len < 3`
/// and stopwords, dedupe preserving first occurrence, cap 64.
pub fn terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| !c.is_alphanumeric()) {
        if tok.chars().count() < 3 {
            continue;
        }
        let t = tok.to_lowercase();
        if STOPWORDS.contains(&t.as_str()) || out.contains(&t) {
            continue;
        }
        out.push(t);
        if out.len() == 64 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("theuth-knowledge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    async fn open() -> (Arc<Knowledge>, PathBuf) {
        let dir = tempdir();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        (Knowledge::open(&dir, rt).await.unwrap(), dir)
    }

    fn put(id: &str, title: &str, body: &str) -> Put {
        Put {
            id: id.into(),
            type_: "fact".into(),
            title: title.into(),
            description: None,
            tags: vec!["rust".into()],
            scope: None,
            body: body.into(),
            visibility: None,
            ..Put::default()
        }
    }

    /// 136 §2.1: `put_for` keeps the seven keys and any unknown key of an
    /// existing file; a put that sets one overwrites only that one.
    #[tokio::test]
    async fn put_for_round_trips_the_136_keys_and_unknown_keys() {
        let (k, dir) = open().await;
        let path = dir.join(DIR).join("facts").join("rt.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\ntype: fact\ntitle: Old title\nstatus: fresh\nhint: false\nevidence: [01abc, user]\n\
             verified: 2026-10-01\nvalid_until: 2026-10-07T21:05:00Z\nsuperseded_by: facts/new\n\
             merged_into: facts/canon\ncheck: ls /\nresource: https://x.example\n---\nold\n",
        )
        .unwrap();
        k.sync().await.unwrap();
        let m = k.get("facts/rt").await.unwrap().meta;
        assert_eq!(m.status, Status::Fresh);
        assert!(!m.hint);
        assert_eq!(m.evidence, ["01abc", "user"]);
        assert_eq!(m.verified.as_deref(), Some("2026-10-01"));
        assert_eq!(m.valid_until.as_deref(), Some("2026-10-07T21:05:00Z"));
        assert_eq!(m.superseded_by.as_deref(), Some("facts/new"));
        assert_eq!(m.merged_into.as_deref(), Some("facts/canon"));

        // A plain put (nothing of 136 set) keeps every key and the unknown ones verbatim.
        let (m, created) = k
            .put(put("facts/rt", "New title", "new body"), Source::Model)
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(m.title, "New title");
        assert_eq!(m.status, Status::Fresh);
        assert!(!m.hint);
        assert_eq!(m.evidence, ["01abc", "user"]);
        assert_eq!(m.superseded_by.as_deref(), Some("facts/new"));
        let text = std::fs::read_to_string(&path).unwrap();
        for line in [
            "check: ls /",
            "resource: https://x.example",
            "verified: 2026-10-01",
            "merged_into: facts/canon",
            "hint: false",
        ] {
            assert!(text.contains(line), "{line:?} missing from\n{text}");
        }

        // A put that sets a key overwrites that key only.
        let (m, _) = k
            .put(
                Put {
                    status: Some(Status::Tidy),
                    hint: Some(true),
                    evidence: vec!["01def".into()],
                    ..put("facts/rt", "New title", "new body")
                },
                Source::Dreaming,
            )
            .await
            .unwrap();
        assert_eq!(m.status, Status::Tidy);
        assert!(m.hint);
        assert_eq!(m.evidence, ["01def"]);
        assert_eq!(m.verified.as_deref(), Some("2026-10-01"));
        assert_eq!(m.source, Some(Source::Dreaming));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("source: dreaming"), "{text}");
        assert!(text.contains("check: ls /"), "{text}");
        // The row carries the columns too (list reads rows, not files).
        let listed = k.list(Some("facts/rt"), None, None, 10).await.unwrap();
        assert_eq!(listed[0].status, Status::Tidy);
        assert_eq!(listed[0].evidence, ["01def"]);

        // A file without any 136 key reads as the defaults.
        k.put(put("facts/plain", "Plain", "b"), Source::Model)
            .await
            .unwrap();
        let m = k.get("facts/plain").await.unwrap().meta;
        assert_eq!(m.status, Status::Tidy);
        assert!(m.hint);
        assert!(m.evidence.is_empty());
        assert!(!std::fs::read_to_string(dir.join(DIR).join("facts").join("plain.md"))
            .unwrap()
            .contains("status:"));
    }

    /// 136 §2.1: `uses` counts impressions and opens, answers zeros for
    /// unknown ids, and survives `sync`.
    #[tokio::test]
    async fn uses_table_records_shown_and_opened_and_survives_sync() {
        let (k, _dir) = open().await;
        k.put(put("facts/a", "A", "a"), Source::Model).await.unwrap();
        let t0 = chrono::Utc.with_ymd_and_hms(2026, 10, 8, 1, 0, 0).unwrap();
        let t1 = t0 + chrono::Duration::minutes(5);
        let ids = vec!["facts/a".to_owned(), "facts/ghost".to_owned()];
        k.record_shown(&ids, t0).await.unwrap();
        k.record_shown(&ids[..1], t1).await.unwrap();
        k.record_opened("facts/a", t1).await.unwrap();
        k.sync().await.unwrap();
        let s = k
            .stats(&["facts/a".into(), "facts/ghost".into(), "facts/none".into()])
            .await
            .unwrap();
        assert_eq!((s[0].shown, s[0].opened), (2, 1));
        assert_eq!(s[0].last_shown.as_deref(), Some("2026-10-08T01:05:00Z"));
        assert_eq!(s[0].last_opened.as_deref(), Some("2026-10-08T01:05:00Z"));
        assert_eq!((s[1].shown, s[1].opened), (1, 0));
        assert_eq!(s[1].last_opened, None);
        assert_eq!(s[2], Uses { id: "facts/none".into(), ..Uses::default() });
        k.record_shown(&[], t1).await.unwrap();
    }

    /// 136 §5.3: `hint: false` and a non-live status are filtered from hints
    /// but stay searchable.
    #[tokio::test]
    async fn hint_for_skips_unhinted_and_non_live_but_search_keeps_them() {
        let (k, _dir) = open().await;
        k.put(
            Put {
                hint: Some(false),
                ..put("facts/quiet", "Quiet rust toolchain", "rust toolchain notes")
            },
            Source::Model,
        )
        .await
        .unwrap();
        k.put(
            Put {
                status: Some(Status::Archived),
                ..put("facts/old", "Old rust toolchain", "rust toolchain notes")
            },
            Source::Model,
        )
        .await
        .unwrap();
        k.put(
            put("facts/live", "Live rust toolchain", "rust toolchain notes"),
            Source::Model,
        )
        .await
        .unwrap();
        let cwd = std::env::temp_dir();
        let hints = k
            .hint("rust toolchain", &cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert_eq!(
            hints.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["facts/live"]
        );
        let found = k.search("rust toolchain", &cwd, 10).await.unwrap();
        assert_eq!(found.len(), 3);
    }

    /// A fixed session → scope table standing in for the roster (67 §9).
    struct Table(HashMap<SessionId, AgentScope>);

    impl Scoper for Table {
        fn scope_for(&self, session: SessionId) -> Option<AgentScope> {
            self.0.get(&session).cloned()
        }
    }

    fn scope(owner: &str, line: &[&str]) -> AgentScope {
        AgentScope {
            owner: owner.into(),
            line: line.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// `ops` reports to `cto`; `mkt` is another line. Sessions: ops, cto, mkt.
    async fn scoped() -> (Arc<Knowledge>, PathBuf, SessionId, SessionId, SessionId) {
        let (k, dir) = open().await;
        let (ops, cto, mkt) = (SessionId::new(), SessionId::new(), SessionId::new());
        let mut t = HashMap::new();
        t.insert(
            ops,
            scope(
                "agent:ops",
                &["agent:ops", "agent:cto", "agent:theuth", "user"],
            ),
        );
        t.insert(
            cto,
            scope(
                "agent:cto",
                &["agent:cto", "agent:ops", "agent:theuth", "user"],
            ),
        );
        t.insert(
            mkt,
            scope("agent:mkt", &["agent:mkt", "agent:theuth", "user"]),
        );
        k.set_scoper(Arc::new(Table(t)));
        (k, dir, ops, cto, mkt)
    }

    #[tokio::test]
    async fn existing_concepts_default_to_user_and_shared() {
        let (k, dir) = open().await;
        // A hand-written file without the 67 §9 keys.
        let path = dir.join(DIR).join("facts").join("old.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "---\ntype: fact\ntitle: Old\n---\nold body\n").unwrap();
        k.sync().await.unwrap();
        let m = k.get("facts/old").await.unwrap().meta;
        assert_eq!(m.owner, "user");
        assert_eq!(m.visibility, Visibility::Shared);
        // A user-side put is the user's and shared; the file carries both keys.
        let (m, _) = k
            .put(put("facts/new", "New", "body"), Source::User)
            .await
            .unwrap();
        assert_eq!(
            (m.owner.as_str(), m.visibility),
            ("user", Visibility::Shared)
        );
        let text = k.get("facts/new").await.unwrap().text;
        assert!(
            text.contains("owner: user") && text.contains("visibility: shared"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn agent_put_is_private_to_it_and_search_is_scoped() {
        let (k, _dir, ops, cto, mkt) = scoped().await;
        let (m, _) = k
            .put_for(
                put("ops/runbook", "Relay runbook", "pelican restart steps"),
                Source::Model,
                Some(ops),
            )
            .await
            .unwrap();
        assert_eq!(
            (m.owner.as_str(), m.visibility),
            ("agent:ops", Visibility::Private)
        );
        let mut team = put("ops/oncall", "Oncall pelican", "who is on call");
        team.visibility = Some(Visibility::Team);
        k.put_for(team, Source::Model, Some(ops)).await.unwrap();
        k.put(
            put("user/pelican", "Pelican shared", "everyone knows"),
            Source::User,
        )
        .await
        .unwrap();

        let cwd = Path::new("/tmp");
        let ids = |hits: &[Hit]| hits.iter().map(|h| h.meta.id.clone()).collect::<Vec<_>>();
        // Own items first, then team, then shared.
        let own = k.search_for("pelican", cwd, 10, Some(ops)).await.unwrap();
        assert_eq!(ids(&own), ["ops/oncall", "ops/runbook", "user/pelican"]);
        // The manager sees the team item in full, the private one as a stub.
        let boss = k.search_for("pelican", cwd, 10, Some(cto)).await.unwrap();
        let stub = boss.iter().find(|h| h.meta.id == "ops/runbook").unwrap();
        assert_eq!(stub.meta.title, "private — ask agent:ops");
        assert_eq!(
            (
                stub.meta.owner.as_str(),
                stub.snippet.as_str(),
                stub.meta.description.as_str()
            ),
            ("agent:ops", "", "")
        );
        assert!(
            boss.iter()
                .any(|h| h.meta.id == "ops/oncall" && h.meta.title == "Oncall pelican")
        );
        // Another line: team and private items are both stubs; shared is full.
        let other = k.search_for("pelican", cwd, 10, Some(mkt)).await.unwrap();
        assert!(
            other
                .iter()
                .filter(|h| h.meta.title.starts_with("private — ask"))
                .count()
                == 2
        );
        assert!(
            other
                .iter()
                .any(|h| h.meta.id == "user/pelican" && h.meta.title == "Pelican shared")
        );
        // Hints never carry stubs; the user's side sees everything, no stubs.
        let hints = k
            .hint_for("pelican shared", cwd, &BTreeSet::new(), 10, Some(mkt))
            .await
            .unwrap();
        assert_eq!(
            hints.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["user/pelican"]
        );
        let all = k.search("pelican", cwd, 10).await.unwrap();
        assert!(
            all.iter()
                .all(|h| !h.meta.title.starts_with("private — ask"))
        );
        assert_eq!(all.len(), 3);
        assert_eq!(
            k.list_for(None, None, None, 10, Some(mkt))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn share_is_for_the_owner_and_its_line() {
        let (k, _dir, ops, cto, mkt) = scoped().await;
        k.put_for(
            put("ops/secret", "Secret", "body"),
            Source::Model,
            Some(ops),
        )
        .await
        .unwrap();
        assert!(
            k.share("ops/secret", Visibility::Team, Some(mkt))
                .await
                .is_err(),
            "another line may not"
        );
        let m = k
            .share("ops/secret", Visibility::Team, Some(cto))
            .await
            .unwrap();
        assert_eq!(m.visibility, Visibility::Team);
        let m = k
            .share("ops/secret", Visibility::Shared, Some(ops))
            .await
            .unwrap();
        assert_eq!(m.visibility, Visibility::Shared);
        assert!(
            k.search_for("secret", Path::new("/tmp"), 10, Some(mkt))
                .await
                .unwrap()[0]
                .meta
                .title
                == "Secret"
        );
        let m = k
            .share("ops/secret", Visibility::Private, None)
            .await
            .unwrap();
        assert_eq!(
            (m.owner.as_str(), m.visibility),
            ("agent:ops", Visibility::Private),
            "the user may; owner is kept"
        );
    }

    #[tokio::test]
    async fn put_get_round_trip_with_links() {
        let (k, _dir) = open().await;
        let (meta, created) = k
            .put(
                put(
                    "facts/toolchain",
                    "Toolchain",
                    "See [theuth](/projects/theuth.md) and [gone](./missing.md).",
                ),
                Source::Model,
            )
            .await
            .unwrap();
        assert!(created);
        assert_eq!(meta.id, "facts/toolchain");
        assert_eq!(meta.description, "Toolchain");
        assert_eq!(meta.source, Some(Source::Model));
        assert!(meta.timestamp.unwrap().ends_with('Z'));

        let (_, created) = k
            .put(put("projects/theuth", "theuth", "hub"), Source::User)
            .await
            .unwrap();
        assert!(created);
        let (_, created) = k
            .put(put("projects/theuth", "wth2", "hub"), Source::User)
            .await
            .unwrap();
        assert!(!created, "second put on the same id is an update");

        let c = k.get("facts/toolchain").await.unwrap();
        assert!(c.text.starts_with("---\ntype: fact\ntitle: Toolchain\n"));
        assert_eq!(
            c.links,
            vec![
                ("facts/missing".to_string(), false),
                ("projects/theuth".to_string(), true)
            ]
        );
        let w = k.get("/projects/theuth.md").await.unwrap();
        assert_eq!(w.meta.title, "wth2");
        assert_eq!(w.cited_by, vec!["facts/toolchain".to_string()]);
        assert!(k.root().join("index.md").exists());
        assert!(
            std::fs::read_to_string(k.root().join("log.md"))
                .unwrap()
                .contains("put facts/toolchain")
        );

        k.delete("facts/toolchain").await.unwrap();
        assert!(k.get("facts/toolchain").await.is_err());
        assert!(k.get("projects/theuth").await.unwrap().cited_by.is_empty());
        assert!(
            !k.root().join("facts").exists(),
            "emptied directory and its index.md go"
        );
    }

    #[tokio::test]
    async fn invalid_ids_and_documents_rejected() {
        let (k, _dir) = open().await;
        for id in ["../x", "index", "facts/log", "a/b/c/d", ""] {
            assert!(
                k.put(put(id, "t", "b"), Source::Model).await.is_err(),
                "{id:?} accepted"
            );
        }
        let mut p = put("facts/x", "t", "b");
        p.type_ = "  ".into();
        assert!(k.put(p, Source::Model).await.is_err());
        let mut p = put("facts/x", "t", "b");
        p.body = "x".repeat(BODY_MAX + 1);
        assert!(k.put(p, Source::Model).await.is_err());
        assert!(
            k.put(put("Facts/Upper", "t", "b"), Source::Model)
                .await
                .unwrap()
                .0
                .id
                == "facts/upper"
        );
    }

    #[tokio::test]
    async fn write_keeps_unknown_frontmatter_and_stamps_user() {
        let (k, _dir) = open().await;
        let meta = k
            .write(
                "Facts/Raw.md",
                "---\ntype: fact\ntitle: t\nextra: keep\n---\nbody\n",
            )
            .await
            .unwrap();
        assert_eq!(meta.id, "facts/raw");
        assert_eq!(meta.source, Some(Source::User));
        let c = k.get("facts/raw").await.unwrap();
        assert!(c.text.contains("extra: keep"), "{}", c.text);
        assert!(c.text.contains("source: user"), "{}", c.text);
        assert!(
            k.write("facts/raw", "---\ntitle: no type\n---\n")
                .await
                .is_err()
        );
        assert!(
            k.write(
                "facts/raw",
                &format!("---\ntype: fact\n---\n{}", "x".repeat(BODY_MAX + 1))
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn terms_example() {
        assert_eq!(
            terms("read /Users/x/theuth/crates/agent/src/loop_.rs"),
            ["users", "theuth", "crates", "agent", "src", "loop"]
        );
        assert_eq!(terms("The file, the FILE!"), Vec::<String>::new());
        assert_eq!(terms("Toolchain toolchain rustup").len(), 2);
    }

    #[tokio::test]
    async fn search_ranks_title_over_body_and_boosts_scope() {
        let (k, _dir) = open().await;
        k.put(
            put("a/title-hit", "rustup toolchain", "nothing here"),
            Source::Model,
        )
        .await
        .unwrap();
        k.put(
            put(
                "a/body-hit",
                "unrelated",
                "the rustup toolchains live under rustup",
            ),
            Source::Model,
        )
        .await
        .unwrap();
        let hits = k.search("toolchain", Path::new("/tmp"), 10).await.unwrap();
        assert_eq!(
            hits.iter().map(|h| h.meta.id.as_str()).collect::<Vec<_>>(),
            ["a/title-hit", "a/body-hit"]
        );
        assert!(hits[1].snippet.contains("toolchains"));
        assert!(
            k.search("", Path::new("/tmp"), 10)
                .await
                .unwrap()
                .is_empty()
        );

        let mut scoped = put("b/scoped", "cargo path", "cargo lives here");
        scoped.scope = Some(PathBuf::from("/work/theuth"));
        k.put(scoped, Source::Model).await.unwrap();
        k.put(
            put("b/global", "cargo path", "cargo lives here"),
            Source::Model,
        )
        .await
        .unwrap();
        let ids = |hits: Vec<Hit>| hits.into_iter().map(|h| h.meta.id).collect::<Vec<_>>();
        let inside = ids(k
            .search("cargo path", Path::new("/work/theuth/crates"), 10)
            .await
            .unwrap());
        assert_eq!(inside, ["b/scoped", "b/global"]);
        let outside = ids(k
            .search("cargo path", Path::new("/elsewhere"), 10)
            .await
            .unwrap());
        assert_eq!(outside, ["b/global", "b/scoped"]);
    }

    #[tokio::test]
    async fn hint_needs_two_terms_and_honours_exclude() {
        let (k, _dir) = open().await;
        k.put(
            put("a/src-only", "the src directory", "src src src"),
            Source::Model,
        )
        .await
        .unwrap();
        k.put(
            put(
                "a/both",
                "agent loop",
                "the agent loop lives in src/loop_.rs",
            ),
            Source::Model,
        )
        .await
        .unwrap();
        let text = "read /Users/x/theuth/crates/agent/src/loop_.rs";
        let hinted = k
            .hint(text, Path::new("/tmp"), &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert_eq!(
            hinted.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a/both"]
        );
        let excluded = BTreeSet::from(["a/both".to_string()]);
        assert!(
            k.hint(text, Path::new("/tmp"), &excluded, 5)
                .await
                .unwrap()
                .is_empty()
        );
        // One-term queries need one match.
        assert_eq!(
            k.hint("src", Path::new("/tmp"), &BTreeSet::new(), 5)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn sync_sees_external_add_edit_and_remove() {
        let (k, _dir) = open().await;
        let file = k.root().join("facts/external.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "---\ntype: fact\ntitle: External\n---\nbody\n").unwrap();
        std::fs::write(k.root().join("broken.md"), "---\ntype: fact\n").unwrap();
        let s = k.sync().await.unwrap();
        assert_eq!((s.added, s.updated, s.removed), (1, 0, 0));
        assert_eq!(
            k.list(None, None, None, 10).await.unwrap()[0].title,
            "External"
        );

        std::fs::write(
            &file,
            "---\ntype: fact\ntitle: External edited\n---\nlonger body\n",
        )
        .unwrap();
        let s = k.sync().await.unwrap();
        assert_eq!((s.added, s.updated, s.removed), (0, 1, 0));
        assert_eq!(
            k.get("facts/external").await.unwrap().meta.title,
            "External edited"
        );

        std::fs::remove_file(&file).unwrap();
        let s = k.sync().await.unwrap();
        assert_eq!((s.added, s.updated, s.removed), (0, 0, 1));
        assert!(k.list(None, None, None, 10).await.unwrap().is_empty());
        assert!(k.sync().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_filters() {
        let (k, _dir) = open().await;
        k.put(put("facts/a", "a", "x"), Source::Model)
            .await
            .unwrap();
        let mut d = put("decisions/b", "b", "x");
        d.type_ = "decision".into();
        d.tags = vec!["mutex".into(), "rust".into()];
        k.put(d, Source::Model).await.unwrap();
        let ids = |v: Vec<Meta>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            ids(k.list(None, None, None, 10).await.unwrap()),
            ["decisions/b", "facts/a"]
        );
        assert_eq!(
            ids(k.list(Some("facts/"), None, None, 10).await.unwrap()),
            ["facts/a"]
        );
        assert_eq!(
            ids(k.list(None, Some("decision"), None, 10).await.unwrap()),
            ["decisions/b"]
        );
        assert_eq!(
            ids(k.list(None, None, Some("mutex"), 10).await.unwrap()),
            ["decisions/b"]
        );
        assert_eq!(
            ids(k.list(None, None, Some("rust"), 1).await.unwrap()),
            ["decisions/b"]
        );
        let m = &k.list(None, None, Some("mutex"), 10).await.unwrap()[0];
        assert_eq!(m.tags, ["mutex", "rust"]);
    }

    #[tokio::test]
    async fn index_rebuilt_when_deleted() {
        let (k, dir) = open().await;
        k.put(put("facts/a", "a", "x"), Source::Model)
            .await
            .unwrap();
        drop(k);
        std::fs::remove_file(dir.join(INDEX_FILE)).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        assert_eq!(k.list(None, None, None, 10).await.unwrap().len(), 1);
    }

    #[test]
    fn regenerate_indexes_skips_dot_dirs_and_unchanged_files() {
        let dir = tempdir();
        let md = "---\ntype: fact\ntitle: A\n---\nbody\n";
        std::fs::create_dir_all(dir.join("facts")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("facts/a.md"), md).unwrap();
        std::fs::write(dir.join(".git/x.md"), md).unwrap();

        let written = regenerate_indexes(&dir).unwrap();
        assert_eq!(
            written,
            vec![dir.join("facts/index.md"), dir.join(INDEX_MD)]
        );
        let root = std::fs::read_to_string(dir.join(INDEX_MD)).unwrap();
        assert!(root.contains("[facts](facts/index.md)"), "{root}");
        assert!(!root.contains(".git"), "{root}");
        assert!(!dir.join(".git/index.md").exists());
        assert!(regenerate_indexes(&dir).unwrap().is_empty());
    }

    #[tokio::test]
    async fn history_and_restore_through_repo() {
        let (k, _dir) = open().await;
        k.put(put("facts/a", "first", "one"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/a", "second", "two"), Source::Model)
            .await
            .unwrap();
        let h = k.history(Some("facts/a"), 10).await.unwrap();
        assert_eq!(
            h.changes
                .iter()
                .map(|c| c.message.as_str())
                .collect::<Vec<_>>(),
            ["put facts/a", "put facts/a"]
        );
        assert!(
            h.diff
                .as_deref()
                .is_some_and(|d| d.contains("-title: first") && d.contains("+title: second"))
        );

        let change = k.restore("facts/a", None).await.unwrap();
        assert!(change.message.starts_with("restore facts/a from "));
        assert_eq!(change.commit.len(), 7);
        let c = k.get("facts/a").await.unwrap();
        assert_eq!(c.meta.title, "first");
        assert!(
            c.text.ends_with("one\n"),
            "file restored verbatim: {}",
            c.text
        );
        assert_eq!(
            k.history(None, 10).await.unwrap().changes[0].message,
            change.message
        );

        k.delete("facts/a").await.unwrap();
        assert!(k.get("facts/a").await.is_err());
        k.restore("facts/a", None).await.unwrap();
        assert_eq!(
            k.get("facts/a").await.unwrap().meta.title,
            "first",
            "restore brings a deleted concept back"
        );
    }

    #[tokio::test]
    async fn revision_details_root_edit_delete_and_sync_without_writing() {
        use crate::types::ChangeStatus;
        let (k, _dir) = open().await;
        k.put(put("facts/a", "first", "one"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/a", "second", "two"), Source::Model)
            .await
            .unwrap();
        k.delete("facts/a").await.unwrap();
        // One external edit of two concepts lands as a single sync commit.
        std::fs::create_dir_all(k.root().join("facts")).unwrap();
        for (n, body) in [("b", "bee"), ("c", "sea")] {
            std::fs::write(
                k.root().join(format!("facts/{n}.md")),
                format!("---\ntype: fact\ntitle: {n}\n---\n{body}\n"),
            )
            .unwrap();
        }
        k.sync().await.unwrap();
        let h = k.history(None, 10).await.unwrap().changes;
        assert_eq!(h.len(), 4, "{h:?}");

        // Root commit: no parent, the concept only — index.md/log.md are not files here.
        let root = k.revision(&h[3].commit, None).await.unwrap();
        assert_eq!(root.parent, None);
        assert_eq!(root.change, h[3]);
        let ids: Vec<_> = root.files.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["facts/a"]);
        assert_eq!(root.files[0].status, ChangeStatus::Added);
        assert!(
            root.files[0]
                .diff
                .starts_with("--- /dev/null\n+++ b/facts/a.md\n@@"),
            "{}",
            root.files[0].diff
        );

        let edit = k.revision(&h[2].commit, Some("facts/a")).await.unwrap();
        let parent = edit.parent.as_deref().unwrap();
        assert!(
            parent.len() == 40 && parent.starts_with(&h[3].commit),
            "{parent}"
        );
        assert_eq!(edit.files[0].status, ChangeStatus::Modified);
        assert!(
            edit.files[0]
                .diff
                .starts_with("--- a/facts/a.md\n+++ b/facts/a.md\n")
                && edit.files[0].diff.contains("-title: first")
                && edit.files[0].diff.contains("+title: second"),
            "{}",
            edit.files[0].diff
        );

        // The deleted concept is inspectable although `get` no longer finds it.
        let del = k.revision(&h[1].commit, None).await.unwrap();
        assert_eq!(del.files.len(), 1, "{:?}", del.files);
        assert_eq!(del.files[0].status, ChangeStatus::Deleted);
        assert!(
            del.files[0]
                .diff
                .starts_with("--- a/facts/a.md\n+++ /dev/null\n")
                && del.files[0].diff.contains("-title: second"),
            "{}",
            del.files[0].diff
        );

        let sync = k.revision(&h[0].commit, None).await.unwrap();
        let ids: Vec<_> = sync.files.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["facts/b", "facts/c"]);
        let only_c = k.revision(&h[0].commit, Some("Facts/C.md")).await.unwrap();
        assert_eq!(only_c.files.len(), 1);
        assert_eq!(only_c.files[0].id, "facts/c");
        assert!(k.revision(&h[0].commit, Some("index")).await.is_err());
        assert!(k.revision("0000000", None).await.is_err());

        // Inspection wrote and committed nothing.
        assert_eq!(k.history(None, 10).await.unwrap().changes, h);
        assert!(k.get("facts/a").await.is_err());
    }
}
