//! The bundle on disk + the FTS5 index (design doc 10 §2–3).
//!
//! Files are the truth; `knowledge.sqlite` is derived from them (`sync`) and
//! can be deleted at any time. Every write goes through [`Knowledge::write_concept`]
//! under one mutex: file, rows, `index.md`, `log.md`, commit, event.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail, ensure};
use okf::{ConceptId, Document, RESERVED_FILENAMES, Value};
use runtime::{Runtime, SessionId};
use sqlx::FromRow;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::harvest;
use crate::repo::{DREAMING_SIGNATURE, Repo, SIGNATURE};
use crate::types::{
    AgentScope, Change, Concept, History, Hit, KnowledgeOp, Meta, Put, Receipt, Revision, Scoper,
    Source, Status, Sync, USER_OWNER, Uses, VerifyOutcome, Visibility,
};
use crate::{
    BODY_MAX, Consolidator, DIR, INDEX_FILE, KnowledgeChanged, KnowledgeConfig, Lifetimes,
};

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
    valid_until UNINDEXED, superseded_by UNINDEXED, merged_into UNINDEXED, consolidated_at UNINDEXED, \
    path UNINDEXED, mtime UNINDEXED, size UNINDEXED, \
    tokenize = 'porter unicode61 remove_diacritics 2')";
/// Columns of [`MetaRow`], in `SELECT` order.
pub(crate) const META_COLS: &str = "id, type, title, description, tags, scope, source, timestamp, owner, visibility, \
    status, hint, evidence, verified, valid_until, superseded_by, merged_into, consolidated_at";
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
    /// Who may run dreaming's consolidation ops as an attended owner root
    /// (136 D6), from [`Knowledge::set_consolidator`]; `None` = nobody yet.
    consolidator: std::sync::RwLock<Option<Arc<dyn Consolidator>>>,
    /// 136 D6: the dreaming session of the open night, from
    /// [`Knowledge::register_consolidating_session`]; the only unattended
    /// session that may consolidate. `None` = no open night.
    consolidating: std::sync::RwLock<Option<SessionId>>,
    /// `knowledge.lifetimes` (136 §5.2): the rank's record cut-off.
    lifetimes: Lifetimes,
    /// Fixed identity of an isolated store; never replicated or published on main's bus.
    isolated: Option<String>,
    /// Lazily opened, ordinary stores. Only the main handle selects bundles.
    bundles: tokio::sync::Mutex<HashMap<String, Arc<Knowledge>>>,
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
    consolidated_at: Option<String>,
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
            consolidated_at: r.consolidated_at,
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
            type_: fm.type_().unwrap_or_default().into_owned(),
            title: fm
                .title()
                .filter(|t| !t.is_empty())
                .map_or_else(|| id.clone(), Cow::into_owned),
            description: fm.description().unwrap_or_default().into_owned(),
            tags: fm.tags(),
            scope: fm
                .get("scope")
                .and_then(Value::as_display_string)
                .map(PathBuf::from),
            source: fm
                .get("source")
                .and_then(Value::as_display_string)
                .and_then(|s| Source::parse(&s)),
            timestamp: fm.timestamp().map(Cow::into_owned),
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
            consolidated_at: fm_string(fm.get("consolidated_at")),
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
    v.as_bool().or_else(|| match v.as_str().map(str::trim) {
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
    // OKF 0.2 accepts any separator-free segment; ids stay `[a-z0-9_][a-z0-9_.-]*`.
    if let Some(seg) = cid
        .segments()
        .iter()
        .find(|s| !okf::concept_id::is_portable_segment(s))
    {
        bail!("invalid id {raw:?}: {seg:?} is not [a-z0-9_][a-z0-9_.-]*");
    }
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

/// Set the writer and the write time (10 §2.3) — the *creation* stamp. A
/// dreaming write to an existing file uses [`touch`] instead (136 D5).
fn stamp(fm: &mut okf::Frontmatter, source: Source) {
    fm.set("source", Value::String(source.as_str().to_owned()));
    fm.set("timestamp", Value::String(now_rfc3339()));
}

fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// 136 D5: dreaming's mark on an existing file. Identity — `owner`,
/// `visibility`, `scope`, `source` and `timestamp` (the event date the rank
/// and the archive rules read) — is left alone; `consolidated_at` records
/// the write (git has the rest).
fn touch(fm: &mut okf::Frontmatter) {
    fm.set("consolidated_at", Value::String(now_rfc3339()));
}

/// 136 D5: a `source: user` concept is human-authored; dreaming may retitle
/// or link it, never merge it away, supersede, archive or rewrite it.
fn ensure_not_user(fm: &okf::Frontmatter, id: &str) -> anyhow::Result<()> {
    let user = fm
        .get("source")
        .and_then(Value::as_display_string)
        .is_some_and(|s| s.trim() == Source::User.as_str());
    if user {
        bail!("{id} is human-authored (source: user); dreaming never rewrites it");
    }
    Ok(())
}

/// Temp + rename; parents created.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    platform::files::write_atomic(path, bytes)
}

/// Design 59 §7.1: a concept that already has a document gets the new file
/// text imported as one block-diff transaction, so an edit that arrived
/// through the file (`knowledge_put`, `write`, `restore`, `sync`) lands in
/// the open editor. Migration is on demand — no document is created here —
/// and the document's own export is recognised by hash and skipped. Fire and
/// forget: the write path never waits on the document actor or its helper.
fn notify_document(id: String, path: PathBuf, isolated_root: Option<PathBuf>) {
    tokio::spawn(async move {
        let docs = match isolated_root {
            Some(root) => document::memory::lookup(&root)
                .await
                .map(document::Handle::from_actor),
            None => document::Handle::lookup(),
        };
        let Some(docs) = docs else {
            return;
        };
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
                    type_: doc.frontmatter.type_().unwrap_or_default().into_owned(),
                    title: doc.frontmatter.title().map_or(stem, Cow::into_owned),
                    link: name,
                    description: doc
                        .frontmatter
                        .description()
                        .unwrap_or_default()
                        .into_owned(),
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
            ..Default::default()
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

/// Preserve a removed agent's files, including when the knowledge plugin is disabled.
pub fn move_removed_bundle(data_dir: &Path, name: &str) -> anyhow::Result<()> {
    ensure!(valid_agent_name(name), "invalid agent name");
    let root = data_dir.join("agent-memory");
    let from = root.join(name);
    if from.exists() {
        let removed = root.join(".removed");
        std::fs::create_dir_all(&removed)?;
        let secs = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();
        std::fs::rename(from, removed.join(format!("{name}-{secs}")))?;
    }
    Ok(())
}

fn valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
    if sqlx::query("SELECT owner, visibility, status, hint, evidence, verified, valid_until, superseded_by, merged_into, consolidated_at FROM concepts LIMIT 0")
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
        Self::open_bundle(data_dir, rt, None).await
    }

    async fn open_bundle(
        data_dir: &Path,
        rt: Runtime,
        isolated: Option<String>,
    ) -> anyhow::Result<Arc<Self>> {
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
        let lifetimes = rt
            .section::<KnowledgeConfig>("knowledge")
            .map(|c| c.lifetimes)
            .unwrap_or_default();
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
            isolated,
            bundles: tokio::sync::Mutex::new(HashMap::new()),
            consolidator: std::sync::RwLock::new(None),
            consolidating: std::sync::RwLock::new(None),
            lifetimes,
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
    /// Select before model-facing work, allowing up to five seconds for boot.
    /// Missing identity never grants main. Harvest can capture a selected handle.
    pub async fn for_session(self: &Arc<Self>, session: SessionId) -> anyhow::Result<Arc<Self>> {
        if self.isolated.is_some() {
            return Ok(self.clone());
        }
        let name = tokio::time::timeout(Duration::from_secs(5), async {
            let scoper = loop {
                let scoper = self
                    .scoper
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(scoper) = scoper {
                    break scoper;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            scoper.bundle_for(session).await
        })
        .await
        .map_err(|_| anyhow!("isolated memory unavailable here: identity not ready"))?
        .map_err(|e| anyhow!("isolated memory unavailable here: {e}"))?;
        match name {
            None => Ok(self.clone()),
            Some(name) => self
                .agent_bundle(&name)
                .await
                .map_err(|e| anyhow!("isolated memory unavailable here: {e:#}")),
        }
    }

    /// Owner RPC inspection, including a parked bundle; never called with model arguments.
    pub async fn inspect(self: &Arc<Self>, bundle: Option<&str>) -> anyhow::Result<Arc<Self>> {
        let Some(bundle) = bundle else {
            return Ok(self.clone());
        };
        let name = bundle
            .strip_prefix("agent:")
            .filter(|n| valid_agent_name(n))
            .ok_or_else(|| anyhow!("unknown bundle: {bundle}"))?;
        let root = self.rt.data_dir.join("agent-memory").join(name);
        // A current isolated file permits lazy creation; a parked directory permits reading.
        let enabled =
            std::fs::read_to_string(self.rt.data_dir.join("agents").join(format!("{name}.md")))
                .ok()
                .and_then(|text| {
                    text.strip_prefix("---\n")
                        .and_then(|s| s.split_once("\n---"))
                        .map(|(fm, _)| fm.to_owned())
                })
                .and_then(|fm| serde_yaml::from_str::<serde_json::Value>(&fm).ok())
                .is_some_and(|fm| fm["isolated_memory"] == true);
        ensure!(
            self.rt
                .data_dir
                .join("agents")
                .join(format!("{name}.md"))
                .is_file()
                && (enabled || root.is_dir()),
            "unknown bundle: {bundle}"
        );
        self.agent_bundle(name).await
    }

    async fn agent_bundle(&self, name: &str) -> anyhow::Result<Arc<Self>> {
        ensure!(valid_agent_name(name), "invalid agent name");
        let mut bundles = self.bundles.lock().await;
        if let Some(k) = bundles.get(name) {
            return Ok(k.clone());
        }
        let root = self.rt.data_dir.join("agent-memory").join(name);
        let k = Self::open_bundle(&root, self.rt.clone(), Some(name.to_owned())).await?;
        if let Some(s) = self
            .scoper
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            k.set_scoper(s);
        }
        if let Some(c) = self
            .consolidator
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            k.set_consolidator(c);
        }
        bundles.insert(name.to_owned(), k.clone());
        Ok(k)
    }

    /// Retire the cached handle before moving a removed agent's files.
    pub async fn remove_bundle(&self, name: &str) -> anyhow::Result<()> {
        ensure!(valid_agent_name(name), "invalid agent name");
        let root = self.rt.data_dir.join("agent-memory");
        let from = root.join(name);
        document::memory::remove(&from).await;
        let mut bundles = self.bundles.lock().await;
        if let Some(k) = bundles.remove(name) {
            k.db.close().await;
        }
        move_removed_bundle(&self.rt.data_dir, name)
    }

    /// Isolated document exports reconcile only their owning physical bundle.
    pub async fn sync_document_root(&self, root: &Path) -> anyhow::Result<()> {
        let k = self
            .bundles
            .lock()
            .await
            .values()
            .find(|k| k.root().parent() == Some(root))
            .cloned()
            .context("isolated memory unavailable here: document bundle was removed")?;
        k.sync().await?;
        Ok(())
    }

    /// Is this an isolated physical store?
    pub fn is_isolated(&self) -> bool {
        self.isolated.is_some()
    }

    async fn publish(&self, event: KnowledgeChanged) {
        if self.isolated.is_none() {
            self.rt.publish(event).await;
        }
    }

    fn enforce_identity(&self, fm: &mut okf::Frontmatter) {
        if let Some(name) = &self.isolated {
            fm.set("owner", Value::String(format!("agent:{name}")));
            fm.set("visibility", Value::String("private".to_owned()));
        }
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
        for mut ix in changed {
            if self.isolated.is_some() {
                let path = self.root.join(&ix.path);
                let text = std::fs::read_to_string(&path)?;
                let mut doc = Document::parse(&text)?;
                self.enforce_identity(&mut doc.frontmatter);
                let normalized = doc.serialize();
                if normalized != text {
                    write_atomic(&path, normalized.as_bytes())?;
                }
                ix = Indexed::load(&self.root, &path)?;
            }
            if known.contains_key(&ix.path) {
                out.updated += 1;
            } else {
                out.added += 1;
            }
            self.upsert(&ix).await?;
            notify_document(
                ix.meta.id.clone(),
                self.root.join(&ix.path),
                self.isolated
                    .as_ref()
                    .map(|_| self.root.parent().unwrap().to_path_buf()),
            );
            written.push(PathBuf::from(&ix.path));
            names.push(format!("edit {}", ix.meta.id));
            events.push((ix.meta.id, KnowledgeOp::Put));
        }
        let commit = if names.is_empty() {
            None
        } else {
            self.commit(written, removed, format!("sync: {}", names.join(", ")))
                .await
        };
        for (id, op) in events {
            self.publish(KnowledgeChanged {
                id,
                op,
                source: Source::Sync,
                commit: commit.clone(),
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

    /// 136 D6: the open night's dreaming session; `BeginNight` registers it,
    /// the night's close clears it. Replaces any earlier registration.
    pub fn register_consolidating_session(&self, session: SessionId) {
        *self
            .consolidating
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(session);
    }

    /// 136 D6: no open night — unattended sessions may not consolidate.
    pub fn clear_consolidating_session(&self) {
        *self
            .consolidating
            .write()
            .unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// 136 D6 / R-28: is `session` the registered dreaming session? Its own
    /// reads are not opens (the reader confirming itself).
    pub fn is_consolidating_session(&self, session: SessionId) -> bool {
        *self.consolidating.read().unwrap_or_else(|p| p.into_inner()) == Some(session)
    }

    /// 136 D6: may `session` run dreaming's consolidation ops? Unattended →
    /// only the session registered by the open night; attended → what the
    /// [`Consolidator`] says (the owner's own root). `false` until one is
    /// installed.
    pub async fn may_consolidate(&self, session: SessionId) -> bool {
        if self.is_consolidating_session(session) {
            return true;
        }
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
    pub(crate) fn scope_for(&self, session: Option<SessionId>) -> Option<AgentScope> {
        if let Some(name) = &self.isolated {
            return Some(AgentScope {
                owner: format!("agent:{name}"),
                line: Default::default(),
            });
        }
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
        let existing = std::fs::read_to_string(cid.to_path(&self.root))
            .ok()
            .map(|text| {
                Document::parse(&text)
                    .map(|d| d.frontmatter)
                    .unwrap_or_default()
            });
        let exists = existing.is_some();
        let mut fm = existing.unwrap_or_default();
        // 136 D5: a dreaming put never rewrites a human-authored entry.
        if exists && source == Source::Dreaming {
            ensure_not_user(&fm, &id)?;
        }
        fm.set("type", Value::String(type_.to_owned()));
        fm.set("title", Value::String(title.to_owned()));
        fm.set("description", Value::String(description.to_owned()));
        if tags.is_empty() {
            fm.as_mapping_mut().remove("tags");
        } else {
            fm.set("tags", Value::Sequence(tags));
        }
        // 136 D5: identity — `owner`, `visibility`, `scope`, `source`,
        // `timestamp` — is set on create; an existing file keeps its own
        // (R-5: the dreaming session's scope must not flip a desk's private
        // entry to `owner: user, visibility: shared`). A put that names
        // `visibility` still sets it; `scope` follows the put unless the
        // writer is dreaming.
        if !(exists && source == Source::Dreaming) {
            match &p.scope {
                Some(scope) => fm.set("scope", Value::String(scope.to_string_lossy().into_owned())),
                None => {
                    fm.as_mapping_mut().remove("scope");
                }
            }
        }
        let scope = self.scope_for(session);
        if !exists {
            let owner = scope.as_ref().map_or(USER_OWNER, |s| s.owner.as_str());
            fm.set("owner", Value::String(owner.to_owned()));
        }
        if let Some(v) = p.visibility {
            fm.set("visibility", Value::String(v.as_str().to_owned()));
        } else if !exists {
            let v = if scope.is_some() {
                Visibility::Private
            } else {
                Visibility::Shared
            };
            fm.set("visibility", Value::String(v.as_str().to_owned()));
        }
        // 136 §5.1: only what the put sets; `None`/empty keeps the file's value.
        // R-23: `fresh` never demotes a consolidated entry (a harvest that
        // re-emits a merged/tidied id), and evidence is a union.
        if let Some(s) = p.status {
            let current = fm
                .get("status")
                .and_then(Value::as_display_string)
                .and_then(|s| Status::parse(&s));
            if s != Status::Fresh || matches!(current, None | Some(Status::Fresh)) {
                fm.set("status", Value::String(s.as_str().to_owned()));
            }
        }
        if let Some(h) = p.hint {
            fm.set("hint", Value::Bool(h));
        }
        if !p.evidence.is_empty() {
            let mut ev = fm_strings(fm.get("evidence"));
            union(&mut ev, p.evidence.clone());
            set_list(&mut fm, "evidence", ev);
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
        if exists && source == Source::Dreaming {
            touch(&mut fm);
        } else {
            stamp(&mut fm, source);
        }
        let text = Document::new(fm, p.body).serialize();

        let entry = format!("put {id}");
        let w = self.write_concept(&cid, text, &entry, &entry).await?;
        self.publish(KnowledgeChanged {
            id,
            op: KnowledgeOp::Put,
            source,
            commit: w.commit.clone(),
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
        ensure!(self.isolated.is_none(), "isolated memory cannot be shared");
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
        self.publish(KnowledgeChanged {
            id,
            op: KnowledgeOp::Put,
            source: Source::Model,
            commit: w.commit.clone(),
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
        self.publish(KnowledgeChanged {
            id,
            op: KnowledgeOp::Put,
            source: Source::User,
            commit: w.commit.clone(),
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
        let commit = self.commit(written, removed, entry).await;
        let docs = if self.isolated.is_some() {
            document::memory::lookup(self.root.parent().unwrap())
                .await
                .map(document::Handle::from_actor)
        } else {
            document::Handle::lookup()
        };
        if let Some(docs) = docs {
            let id = id.clone();
            tokio::spawn(async move {
                if let Some(doc) = docs.resolve_knowledge(&id).await
                    && let Err(e) = docs.delete(doc).await
                {
                    tracing::warn!(id, error = %e, "knowledge → document delete failed");
                }
            });
        }
        self.publish(KnowledgeChanged {
            id,
            op: KnowledgeOp::Delete,
            source: Source::User,
            commit,
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
            .ranked(&terms(query), cwd, limit, scope.as_ref(), true, false)
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
                true,
            )
            .await?;
        let mut out = Vec::new();
        for (hit, body) in hits {
            if out.len() >= limit as usize {
                break;
            }
            if exclude.contains(&hit.meta.id) {
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
        self.publish(KnowledgeChanged {
            id,
            op: KnowledgeOp::Put,
            source: Source::User,
            commit: Some(sha.clone()),
        })
        .await;
        Ok(Change {
            commit: sha,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            message,
        })
    }

    /// [`Self::restore`] for several `(id, commit)` pairs in ONE commit
    /// (`restore <n> concepts`): every file rewritten from its named commit,
    /// indexes regenerated once, one [`KnowledgeChanged`] per id. Any pair
    /// that cannot be read refuses the whole batch before a write (136 P gap
    /// 6: the night's undo at ≈ 1 s per single restore).
    pub async fn restore_many(&self, pairs: &[(String, String)]) -> anyhow::Result<Change> {
        let Some(repo) = self.repo.clone() else {
            bail!("restore unavailable: no repository")
        };
        if pairs.is_empty() {
            bail!("restore_many: nothing to restore");
        }
        let mut wanted = Vec::with_capacity(pairs.len());
        for (id, commit) in pairs {
            wanted.push((concept_id(id)?, commit.clone()));
        }
        let files =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(ConceptId, String)>> {
                let mut files = Vec::with_capacity(wanted.len());
                for (cid, commit) in wanted {
                    let rel = cid.to_path(Path::new(""));
                    let bytes = repo
                        .show(&rel, &commit)?
                        .with_context(|| format!("{cid} is not in commit {commit}"))?;
                    let text = String::from_utf8(bytes).context("concept is not UTF-8")?;
                    files.push((cid, text));
                }
                Ok(files)
            })
            .await??;
        let _g = self.write.lock().await;
        let message = format!("restore {} concepts", files.len());
        let (metas, commit) = self
            .write_concepts(files, &message, &message, SIGNATURE)
            .await?;
        let sha = commit.context("restore: commit failed")?;
        for m in metas {
            self.publish(KnowledgeChanged {
                id: m.id,
                op: KnowledgeOp::Put,
                source: Source::User,
                commit: Some(sha.clone()),
            })
            .await;
        }
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
        let created = !cid.to_path(&self.root).exists();
        let (mut metas, commit) = self
            .write_concepts(vec![(cid.clone(), text)], log_entry, message, SIGNATURE)
            .await?;
        Ok(Written {
            meta: metas.pop().context("write_concepts wrote nothing")?,
            created,
            commit,
        })
    }

    /// [`Self::write_concept`] for several files at once: every file written
    /// and indexed, `index.md`/`log.md` regenerated once, ONE commit as
    /// `author`. Metas in the order given.
    async fn write_concepts(
        &self,
        files: Vec<(ConceptId, String)>,
        log_entry: &str,
        message: &str,
        author: (&'static str, &'static str),
    ) -> anyhow::Result<(Vec<Meta>, Option<String>)> {
        let mut docs = Vec::with_capacity(files.len());
        for (cid, text) in files {
            let doc = Document::parse(&text)?;
            let mut doc = doc;
            self.enforce_identity(&mut doc.frontmatter);
            let text = if self.isolated.is_some() {
                doc.serialize()
            } else {
                text
            };
            doc.validate().map_err(|e| anyhow!("{}: {e}", cid))?;
            docs.push((cid, text, doc));
        }
        let root = self.root.clone();
        let entry = log_entry.to_owned();
        let (ixs, mut touched) =
            tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<Indexed>, Vec<PathBuf>)> {
                let mut ixs = Vec::with_capacity(docs.len());
                for (cid, text, doc) in &docs {
                    let path = cid.to_path(&root);
                    write_atomic(&path, text.as_bytes())
                        .with_context(|| format!("write {}", path.display()))?;
                    let (mtime, size) = stat(&path)?;
                    ixs.push(Indexed::new(&root, &path, doc, mtime, size)?);
                }
                Ok((ixs, finish_files(&root, &entry)?))
            })
            .await??;
        let mut metas = Vec::with_capacity(ixs.len());
        for ix in ixs {
            self.upsert(&ix).await?;
            notify_document(
                ix.meta.id.clone(),
                self.root.join(&ix.path),
                self.isolated
                    .as_ref()
                    .map(|_| self.root.parent().unwrap().to_path_buf()),
            );
            touched.push(PathBuf::from(&ix.path));
            metas.push(ix.meta);
        }
        let commit = self
            .commit_as(touched, Vec::new(), message.to_owned(), author)
            .await;
        Ok((metas, commit))
    }

    /// Commit through the repo when present. A failure is logged, never
    /// fatal: the files stand and the next `open` reconciles (10 §11).
    async fn commit(
        &self,
        written: Vec<PathBuf>,
        removed: Vec<PathBuf>,
        message: String,
    ) -> Option<String> {
        self.commit_as(written, removed, message, SIGNATURE).await
    }

    /// [`Self::commit`] as `author` (136: `dreaming <dreaming@localhost>` for the five ops).
    async fn commit_as(
        &self,
        written: Vec<PathBuf>,
        removed: Vec<PathBuf>,
        message: String,
        author: (&'static str, &'static str),
    ) -> Option<String> {
        let repo = self.repo.clone()?;
        match tokio::task::spawn_blocking(move || {
            repo.commit_as(&written, &removed, &message, author)
        })
        .await
        {
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
             status, hint, evidence, verified, valid_until, superseded_by, merged_into, consolidated_at, path, mtime, size) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&m.consolidated_at)
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

    /// FTS query for `terms`, ranked by 136 §5.3 (`bm25 × scope × type_weight ×
    /// freshness × use`), with each hit's body (the ≥2-term rule needs it).
    /// Empty terms → nothing. With an agent `scope` (67 §9): its own items
    /// rank first; items it may not read are replaced by a stub (`stubs`) or
    /// dropped. `live_only` (the hint path, 136 §5.3): `hint: false`, a
    /// non-live status and a record past its lifetime (rank 0) are filtered
    /// BEFORE the bm25 cut, so stubs and archived rows never spend the window
    /// (R-26).
    async fn ranked(
        &self,
        terms: &[String],
        cwd: &Path,
        limit: u32,
        scope: Option<&AgentScope>,
        stubs: bool,
        live_only: bool,
    ) -> anyhow::Result<Vec<(Hit, String)>> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        // UNINDEXED columns are plain filters on an FTS5 table.
        let live = if live_only {
            " AND hint = 1 AND status IN ('fresh', 'tidy')"
        } else {
            ""
        };
        // Only `META_COLS` and the fixed `live` clause (consts) are
        // interpolated; the MATCH query and limit are bound.
        let rows: Vec<HitRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {META_COLS}, body, \
             bm25(concepts, 0, 3.0, 10.0, 5.0, 4.0, 1.0) AS s, snippet(concepts, 5, '', '', '…', 12) AS snip \
             FROM concepts WHERE concepts MATCH ?{live} ORDER BY s LIMIT ?"
        )))
        .bind(query)
        .bind(i64::from(limit) * 4)
        .fetch_all(&self.db)
        .await?;
        let uses = self
            .uses_of(rows.iter().map(|r| r.meta.id.as_str()))
            .await?;
        let now = chrono::Utc::now();
        let mut hits: Vec<(Hit, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let meta: Meta = r.meta.into();
                let boost = match &meta.scope {
                    Some(scope) if cwd.starts_with(scope) => 1.5,
                    Some(_) => 0.5,
                    None => 1.0,
                };
                let (shown, opened) = uses.get(&meta.id).copied().unwrap_or_default();
                let weight = rank_weight(
                    &meta.type_,
                    age_days(meta.timestamp.as_deref(), now),
                    shown,
                    opened,
                    &self.lifetimes,
                );
                let score = (-r.s * boost * weight) as f32;
                // A record past its lifetime ranks 0: not for a note.
                if live_only && score <= 0.0 {
                    return None;
                }
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

    /// `(shown, opened)` per id from `uses`; ids without a row are absent.
    async fn uses_of<'a>(
        &self,
        ids: impl Iterator<Item = &'a str>,
    ) -> anyhow::Result<HashMap<String, (u64, u64)>> {
        let ids: Vec<&str> = ids.collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let marks = vec!["?"; ids.len()].join(", ");
        // Only the placeholder list is interpolated; every id is bound.
        let mut q = sqlx::query_as::<_, (String, i64, i64)>(sqlx::AssertSqlSafe(format!(
            "SELECT id, shown, opened FROM uses WHERE id IN ({marks})"
        )));
        for id in ids {
            q = q.bind(id);
        }
        Ok(q.fetch_all(&self.db)
            .await?
            .into_iter()
            .map(|(id, s, o)| (id, (s.max(0) as u64, o.max(0) as u64)))
            .collect())
    }
}

/// Whole days between `timestamp` (RFC 3339) and `now`; 0 when absent or unparsable.
fn age_days(timestamp: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> f64 {
    timestamp
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0) as f64 / 86_400.0)
        .unwrap_or(0.0)
}

/// 136 §5.3: the rank's multipliers beyond bm25 and the scope boost —
/// `type_weight × freshness × use`. `type_weight`: decision 1.3 · warning 1.3
/// · procedure 1.1 · fact 1.0 · entity/hosts/project 0.9 · record 0.6 (0 past
/// `lifetimes.record` days) · anything else 1.0. `freshness = 0.5 +
/// 0.5·exp(−age_days/45)` for record and fact, 1.0 otherwise. `use = 1 +
/// ln(1 + opened) − 0.4·[shown ≥ 50 ∧ opened = 0 ∧ type ≠ warning]`.
pub fn rank_weight(
    type_: &str,
    age_days: f64,
    shown: u64,
    opened: u64,
    lifetimes: &Lifetimes,
) -> f64 {
    let type_weight = match type_ {
        "decision" | "warning" => 1.3,
        "procedure" => 1.1,
        "entity" | "hosts" | "project" => 0.9,
        "record" if age_days > f64::from(lifetimes.record) => return 0.0,
        "record" => 0.6,
        _ => 1.0,
    };
    let freshness = match type_ {
        "record" | "fact" => 0.5 + 0.5 * (-age_days / 45.0).exp(),
        _ => 1.0,
    };
    let ignored = shown >= 50 && opened == 0 && type_ != "warning";
    let use_ = 1.0 + (1.0 + opened as f64).ln() - if ignored { 0.4 } else { 0.0 };
    type_weight * freshness * use_
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
    /// `links` rows, one commit `dreaming merge <into>`. A new canonical takes
    /// its type, title and description from the first folded file and its
    /// owner from `scope`.
    pub async fn merge(
        &self,
        ids: &[String],
        into: &str,
        body: &str,
        evidence: &[String],
        scope: &AgentScope,
    ) -> anyhow::Result<Receipt> {
        let into_cid = concept_id(into)?;
        let into = into_cid.to_string();
        body_within_limit(body.len())?;
        let mut folded = Vec::new();
        for id in ids {
            let cid = concept_id(id)?;
            if cid != into_cid && !folded.contains(&cid) {
                folded.push(cid);
            }
        }
        if folded.is_empty() {
            bail!("merge: nothing to fold into {into}");
        }
        let _g = self.write.lock().await;
        let docs = folded
            .iter()
            .map(|cid| {
                let doc = self.load(cid)?;
                // 136 D5: a human-authored entry is never merged away.
                ensure_not_user(&doc.frontmatter, &cid.to_string())?;
                Ok(doc)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut canonical = match self.load(&into_cid) {
            Ok(doc) => {
                // … nor has its body replaced by a merge.
                ensure_not_user(&doc.frontmatter, &into)?;
                let mut doc = doc;
                touch(&mut doc.frontmatter);
                doc
            }
            Err(_) => {
                let first = &docs[0].frontmatter;
                let mut fm = okf::Frontmatter::new();
                for key in ["type", "title", "description"] {
                    if let Some(v) = first.get(key) {
                        fm.set(key, v.clone());
                    }
                }
                fm.set("owner", Value::String(scope.owner.clone()));
                let vis = if scope.owner == USER_OWNER {
                    Visibility::Shared
                } else {
                    Visibility::Private
                };
                fm.set("visibility", Value::String(vis.as_str().to_owned()));
                // A new file: dreaming is its writer and now its event date.
                stamp(&mut fm, Source::Dreaming);
                touch(&mut fm);
                Document::new(fm, String::new())
            }
        };
        // Unions keep the canonical's order first, then each folded file's, then the given.
        let mut tags = canonical.frontmatter.tags();
        let mut ev = fm_strings(canonical.frontmatter.get("evidence"));
        for d in &docs {
            union(&mut tags, d.frontmatter.tags());
            union(&mut ev, fm_strings(d.frontmatter.get("evidence")));
        }
        union(&mut ev, evidence.to_vec());
        set_list(&mut canonical.frontmatter, "tags", tags);
        set_list(&mut canonical.frontmatter, "evidence", ev);
        // The canonical is live again whatever it was before (R-44: a
        // previously archived/superseded canonical would otherwise keep
        // `hint: false` and its `superseded_by`).
        canonical
            .frontmatter
            .set("status", Value::String(Status::Tidy.as_str().to_owned()));
        canonical.frontmatter.set("hint", Value::Bool(true));
        for key in ["merged_into", "superseded_by", "valid_until"] {
            canonical.frontmatter.as_mapping_mut().remove(key);
        }
        canonical.body = body.to_owned();

        let mut files = vec![(into_cid.clone(), canonical.serialize())];
        for (cid, mut doc) in folded.iter().cloned().zip(docs) {
            doc.frontmatter
                .set("status", Value::String(Status::Merged.as_str().to_owned()));
            doc.frontmatter
                .set("merged_into", Value::String(into.clone()));
            doc.frontmatter.set("hint", Value::Bool(false));
            touch(&mut doc.frontmatter);
            doc.body = format!("Merged into [{into}](/{into}.md)\n");
            files.push((cid, doc.serialize()));
        }
        // Old citations are rewritten in the citing files themselves (not only
        // in the `links` index), so each citer is committed and published with
        // the merge and `restore` undoes the retarget (136 P gap 4).
        let mut citers: Vec<ConceptId> = Vec::new();
        for cid in &folded {
            let rows: Vec<(String,)> = sqlx::query_as("SELECT src FROM links WHERE dst = ?")
                .bind(cid.to_string())
                .fetch_all(&self.db)
                .await?;
            for (src,) in rows {
                let Ok(scid) = concept_id(&src) else { continue };
                if scid != into_cid && !folded.contains(&scid) && !citers.contains(&scid) {
                    citers.push(scid);
                }
            }
        }
        for scid in citers {
            let Ok(mut doc) = self.load(&scid) else {
                continue;
            };
            let mut body = doc.body.clone();
            for link in doc.links() {
                if link.resolve(&scid).is_some_and(|t| folded.contains(&t)) {
                    // ponytail: plain text replace; a code span quoting the same
                    // `](target)` is rewritten too. Offsets if okf ever exposes them.
                    body = body.replace(&format!("]({})", link.target), &format!("](/{into}.md)"));
                }
            }
            if body != doc.body {
                doc.body = body;
                touch(&mut doc.frontmatter);
                files.push((scid, doc.serialize()));
            }
        }
        self.consolidate(KnowledgeOp::Merge, &into, files).await
    }

    /// 136 §3.1 item 4: `superseded_by`, `valid_until`, `hint: false` on the
    /// loser; a "Superseded by" paragraph (with `note`) under the loser's body.
    pub async fn supersede(
        &self,
        loser: &str,
        winner: &str,
        valid_until: Option<&str>,
        note: Option<&str>,
    ) -> anyhow::Result<Receipt> {
        let cid = concept_id(loser)?;
        let winner_cid = concept_id(winner)?;
        let winner = winner_cid.to_string();
        if cid == winner_cid {
            bail!("supersede: {winner} cannot supersede itself");
        }
        let _g = self.write.lock().await;
        if !winner_cid.to_path(&self.root).is_file() {
            bail!("no such concept: {winner}");
        }
        let mut doc = self.load(&cid)?;
        // 136 D5: a human-authored entry is never superseded.
        ensure_not_user(&doc.frontmatter, &cid.to_string())?;
        doc.frontmatter
            .set("superseded_by", Value::String(winner.clone()));
        if let Some(v) = valid_until.map(str::trim).filter(|v| !v.is_empty()) {
            doc.frontmatter
                .set("valid_until", Value::String(v.to_owned()));
        }
        doc.frontmatter.set("hint", Value::Bool(false));
        touch(&mut doc.frontmatter);
        let mut line = format!("Superseded by [{winner}](/{winner}.md)");
        if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
            line.push_str(": ");
            line.push_str(n);
        }
        append_paragraph(&mut doc.body, &line);
        let id = cid.to_string();
        self.consolidate(KnowledgeOp::Supersede, &id, vec![(cid, doc.serialize())])
            .await
    }

    /// 136 §3.1 item 4: `status: archived, hint: false`; `reason` appended
    /// under `## Archived`.
    pub async fn archive(&self, id: &str, reason: &str) -> anyhow::Result<Receipt> {
        let cid = concept_id(id)?;
        let _g = self.write.lock().await;
        let mut doc = self.load(&cid)?;
        // 136 D5: a human-authored entry is never archived.
        ensure_not_user(&doc.frontmatter, &cid.to_string())?;
        doc.frontmatter.set(
            "status",
            Value::String(Status::Archived.as_str().to_owned()),
        );
        doc.frontmatter.set("hint", Value::Bool(false));
        touch(&mut doc.frontmatter);
        append_section(&mut doc.body, "## Archived", reason.trim());
        let id = cid.to_string();
        self.consolidate(KnowledgeOp::Archive, &id, vec![(cid, doc.serialize())])
            .await
    }

    /// 136 §3.1 item 4: `verified: <today>` (ok) or `status: stale` + the note
    /// under `## Stale` (stale); `evidence` (the sources the check read) is
    /// unioned into the file's `evidence`.
    pub async fn verify(
        &self,
        id: &str,
        outcome: VerifyOutcome,
        note: Option<&str>,
        evidence: &[String],
    ) -> anyhow::Result<Receipt> {
        let cid = concept_id(id)?;
        let _g = self.write.lock().await;
        let mut doc = self.load(&cid)?;
        match outcome {
            VerifyOutcome::Ok => {
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                doc.frontmatter.set("verified", Value::String(today));
            }
            VerifyOutcome::Stale => {
                doc.frontmatter
                    .set("status", Value::String(Status::Stale.as_str().to_owned()));
                let note = note
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("a read-tool check no longer confirms this");
                append_section(&mut doc.body, "## Stale", note);
            }
        }
        let mut ev = fm_strings(doc.frontmatter.get("evidence"));
        union(&mut ev, evidence.to_vec());
        set_list(&mut doc.frontmatter, "evidence", ev);
        touch(&mut doc.frontmatter);
        let id = cid.to_string();
        self.consolidate(KnowledgeOp::Verify, &id, vec![(cid, doc.serialize())])
            .await
    }

    /// 136 §3.1 item 4: set `title`; `summary` (the old title's content, or
    /// the old title itself when empty) becomes a leading `## Summary`
    /// paragraph of the body — inserted under an existing `## Summary`
    /// heading rather than stacking a second one (R-44). Allowed on a
    /// `source: user` entry (D5: retitle is not a rewrite); `timestamp`
    /// keeps the event date (R-7).
    pub async fn retitle(&self, id: &str, title: &str, summary: &str) -> anyhow::Result<Receipt> {
        let cid = concept_id(id)?;
        let title = title.trim();
        if title.is_empty() {
            bail!("retitle: title must not be empty");
        }
        let n = title.chars().count();
        ensure!(
            n <= harvest::TITLE_MAX,
            "retitle: title is {n} chars, the cap is {}: choose a shorter title (the rest belongs in summary)",
            harvest::TITLE_MAX
        );
        let _g = self.write.lock().await;
        let mut doc = self.load(&cid)?;
        let old = doc.frontmatter.title().unwrap_or_default().into_owned();
        let summary = match summary.trim() {
            "" => old.as_str(),
            s => s,
        };
        doc.frontmatter
            .set("title", Value::String(title.to_owned()));
        touch(&mut doc.frontmatter);
        let rest = std::mem::take(&mut doc.body);
        let rest = rest.trim_start();
        doc.body = match rest.strip_prefix("## Summary\n") {
            Some(under) => format!("## Summary\n\n{summary}\n\n{}", under.trim_start()),
            None => format!("## Summary\n\n{summary}\n\n{rest}"),
        };
        body_within_limit(doc.body.len())?;
        let id = cid.to_string();
        self.consolidate(KnowledgeOp::Retitle, &id, vec![(cid, doc.serialize())])
            .await
    }

    /// Read and parse `<id>.md`; "no such concept" when absent.
    fn load(&self, cid: &ConceptId) -> anyhow::Result<Document> {
        let text = std::fs::read_to_string(cid.to_path(&self.root))
            .with_context(|| format!("no such concept: {cid}"))?;
        Ok(Document::parse(&text)?)
    }

    /// Write `files` as one commit `dreaming <op> <subject>` authored
    /// `dreaming <dreaming@localhost>`, publish [`KnowledgeChanged`] per id,
    /// and report. The caller holds `write`.
    async fn consolidate(
        &self,
        op: KnowledgeOp,
        subject: &str,
        files: Vec<(ConceptId, String)>,
    ) -> anyhow::Result<Receipt> {
        let name = serde_json::to_value(op)?
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let message = format!("dreaming {name} {subject}");
        let (metas, commit) = self
            .write_concepts(files, &message, &message, DREAMING_SIGNATURE)
            .await?;
        let ids: Vec<String> = metas.into_iter().map(|m| m.id).collect();
        for id in &ids {
            self.publish(KnowledgeChanged {
                id: id.clone(),
                op,
                source: Source::Dreaming,
                commit: commit.clone(),
            })
            .await;
        }
        Ok(Receipt { op, ids, commit })
    }
}

/// Append `items` to `acc`, skipping what it already has.
fn union(acc: &mut Vec<String>, items: Vec<String>) {
    for i in items {
        if !acc.contains(&i) {
            acc.push(i);
        }
    }
}

/// Set `key` to a YAML list, or remove it when empty.
fn set_list(fm: &mut okf::Frontmatter, key: &str, items: Vec<String>) {
    if items.is_empty() {
        fm.as_mapping_mut().remove(key);
    } else {
        fm.set(
            key,
            Value::Sequence(items.into_iter().map(Value::String).collect()),
        );
    }
}

/// `body` + blank line + `text`.
fn append_paragraph(body: &mut String, text: &str) {
    let trimmed = body.trim_end();
    let mut out = String::with_capacity(trimmed.len() + text.len() + 3);
    out.push_str(trimmed);
    if !trimmed.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(text);
    out.push('\n');
    *body = out;
}

/// Append `text` under `heading`; the heading is added once.
fn append_section(body: &mut String, heading: &str, text: &str) {
    let has = body.lines().any(|l| l.trim_end() == heading);
    if has {
        append_paragraph(body, text);
    } else {
        append_paragraph(body, &format!("{heading}\n\n{text}"));
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

    /// Guard first in the tuple: it is declared first, so it drops last.
    async fn open() -> (tempfile::TempDir, Arc<Knowledge>) {
        let dir = crate::test_dir("knowledge");
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let k = Knowledge::open(dir.path(), rt).await.unwrap();
        (dir, k)
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
        let (dir, k) = open().await;
        let path = dir.path().join(DIR).join("facts").join("rt.md");
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

        // A put that sets a key overwrites that key only; evidence is a union
        // (R-23); a dreaming put on an existing file keeps `source` and marks
        // `consolidated_at` (D5).
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
        assert_eq!(m.evidence, ["01abc", "user", "01def"]);
        assert_eq!(m.verified.as_deref(), Some("2026-10-01"));
        assert_eq!(m.source, Some(Source::Model));
        assert!(m.consolidated_at.is_some());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("source: model"), "{text}");
        assert!(text.contains("consolidated_at: "), "{text}");
        assert!(text.contains("check: ls /"), "{text}");
        // The row carries the columns too (list reads rows, not files).
        let listed = k.list(Some("facts/rt"), None, None, 10).await.unwrap();
        assert_eq!(listed[0].status, Status::Tidy);
        assert_eq!(listed[0].evidence, ["01abc", "user", "01def"]);
        assert_eq!(listed[0].consolidated_at, m.consolidated_at);

        // A file without any 136 key reads as the defaults.
        k.put(put("facts/plain", "Plain", "b"), Source::Model)
            .await
            .unwrap();
        let m = k.get("facts/plain").await.unwrap().meta;
        assert_eq!(m.status, Status::Tidy);
        assert!(m.hint);
        assert!(m.evidence.is_empty());
        assert!(
            !std::fs::read_to_string(dir.path().join(DIR).join("facts").join("plain.md"))
                .unwrap()
                .contains("status:")
        );
    }

    /// 136 §2.1: `uses` counts impressions and opens, answers zeros for
    /// unknown ids, and survives `sync`.
    #[tokio::test]
    async fn uses_table_records_shown_and_opened_and_survives_sync() {
        let (_dir, k) = open().await;
        k.put(put("facts/a", "A", "a"), Source::Model)
            .await
            .unwrap();
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
        assert_eq!(
            s[2],
            Uses {
                id: "facts/none".into(),
                ..Uses::default()
            }
        );
        k.record_shown(&[], t1).await.unwrap();
    }

    /// 136 §5.3: `hint: false` and a non-live status are filtered from hints
    /// but stay searchable.
    #[tokio::test]
    async fn hint_for_skips_unhinted_and_non_live_but_search_keeps_them() {
        let (_dir, k) = open().await;
        k.put(
            Put {
                hint: Some(false),
                ..put(
                    "facts/quiet",
                    "Quiet rust toolchain",
                    "rust toolchain notes",
                )
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

    /// 136 §3.1 item 1: every shape the live bundle has parses and reads as
    /// the defaults: no `status`, pasted `Cited by:`/`Links:` lines in the
    /// body, `architecture`/`tool` types, no `visibility`, a 1,500-char title.
    #[tokio::test]
    async fn live_shapes_parse_unchanged() {
        let (_dir, k) = open().await;
        let long_title = "x".repeat(1_500);
        let files = [
            (
                "architecture/engine.md",
                "---\ntype: architecture\ntitle: Engine layout\ntags: [theuth]\n---\nActors, see [the tool](/tools/t.md).\n\nLinks: /tools/t.md\nCited by: facts/a, facts/b\n".to_owned(),
            ),
            (
                "tools/t.md",
                "---\ntype: tool\ntitle: A tool\nsource: harvest\ntimestamp: 2026-10-01T00:00:00Z\n---\nbody\n".to_owned(),
            ),
            (
                "facts/long.md",
                format!("---\ntype: fact\ntitle: {long_title}\ndescription: d\n---\nbody\n"),
            ),
        ];
        for (rel, text) in &files {
            let path = k.root().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        let s = k.sync().await.unwrap();
        assert_eq!(s.added, 3);
        let a = k.get("architecture/engine").await.unwrap();
        assert_eq!(a.meta.type_, "architecture");
        assert_eq!(a.meta.status, Status::Tidy);
        assert!(a.meta.hint);
        assert_eq!(a.meta.visibility, Visibility::Shared);
        assert_eq!(a.meta.owner, USER_OWNER);
        assert!(a.text.contains("Cited by: facts/a, facts/b"));
        assert_eq!(a.links, [("tools/t".to_owned(), true)]);
        let t = k.get("tools/t").await.unwrap().meta;
        assert_eq!(t.type_, "tool");
        assert_eq!(t.source, Some(Source::Harvest));
        let l = k.get("facts/long").await.unwrap().meta;
        assert_eq!(l.title.len(), 1_500);
        assert!(l.evidence.is_empty() && l.verified.is_none() && l.merged_into.is_none());
        // A plain put over the long-title file keeps nothing it does not set and adds no 136 key.
        k.put(put("facts/long", "short", "b2"), Source::Model)
            .await
            .unwrap();
        let text = std::fs::read_to_string(k.root().join("facts/long.md")).unwrap();
        assert!(
            !text.contains("status:") && !text.contains("hint:"),
            "{text}"
        );
    }

    /// 136 §5.3 multipliers, pure.
    #[test]
    fn rank_weight_monotonicity() {
        let lt = Lifetimes::default();
        let w =
            |ty: &str, age: f64, shown: u64, opened: u64| rank_weight(ty, age, shown, opened, &lt);
        // More opens → higher.
        assert!(w("fact", 0.0, 10, 3) > w("fact", 0.0, 10, 1));
        assert!(w("fact", 0.0, 10, 1) > w("fact", 0.0, 10, 0));
        // Shown ≥ 50 with 0 opens → lower; one open lifts the penalty.
        assert!(w("fact", 0.0, 50, 0) < w("fact", 0.0, 49, 0));
        assert!(w("fact", 0.0, 50, 1) > w("fact", 0.0, 50, 0));
        // Warnings are exempt from the penalty.
        assert_eq!(w("warning", 0.0, 500, 0), w("warning", 0.0, 0, 0));
        // Type weights.
        assert_eq!(w("decision", 0.0, 0, 0), 1.3);
        assert_eq!(w("warning", 0.0, 0, 0), 1.3);
        assert_eq!(w("procedure", 0.0, 0, 0), 1.1);
        assert_eq!(w("fact", 0.0, 0, 0), 1.0);
        assert_eq!(w("entity", 0.0, 0, 0), 0.9);
        assert_eq!(w("hosts", 0.0, 0, 0), 0.9);
        assert_eq!(w("project", 0.0, 0, 0), 0.9);
        assert_eq!(w("record", 0.0, 0, 0), 0.6);
        assert_eq!(w("architecture", 0.0, 0, 0), 1.0);
        // Freshness decays for records and facts only; a record past its lifetime is 0.
        assert!(w("fact", 45.0, 0, 0) < w("fact", 0.0, 0, 0));
        assert!((w("fact", 45.0, 0, 0) - (0.5 + 0.5 * (-1.0f64).exp())).abs() < 1e-12);
        assert_eq!(w("decision", 400.0, 0, 0), 1.3);
        assert!(w("record", 14.0, 0, 0) > 0.0);
        assert_eq!(w("record", 14.1, 0, 0), 0.0);
        assert_eq!(
            rank_weight("record", 20.0, 0, 0, &Lifetimes { record: 30, ..lt }),
            0.6 * (0.5 + 0.5 * (-20.0f64 / 45.0).exp())
        );
        // Ages.
        let now = chrono::Utc.with_ymd_and_hms(2026, 10, 8, 0, 0, 0).unwrap();
        assert_eq!(age_days(Some("2026-10-06T00:00:00Z"), now), 2.0);
        assert_eq!(age_days(Some("2026-10-09T00:00:00Z"), now), 0.0);
        assert_eq!(age_days(Some("garbage"), now), 0.0);
        assert_eq!(age_days(None, now), 0.0);
    }

    /// 136 §5.3 through `hint`: opens lift a twin above its sibling, 50
    /// impressions without an open sink it, a record past `lifetimes.record`
    /// leaves the shelf but stays searchable.
    #[tokio::test]
    async fn hint_rank_uses_opens_impressions_and_record_lifetime() {
        let (_dir, k) = open().await;
        for id in ["facts/one", "facts/two", "facts/three"] {
            k.put(
                put(id, "rustup toolchain pins", "the toolchain is pinned"),
                Source::Model,
            )
            .await
            .unwrap();
        }
        let cwd = std::env::temp_dir();
        let ids = |ms: &[Meta]| ms.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let now = chrono::Utc::now();
        k.record_opened("facts/two", now).await.unwrap();
        k.record_opened("facts/two", now).await.unwrap();
        let fifty = vec!["facts/three".to_owned()];
        for _ in 0..50 {
            k.record_shown(&fifty, now).await.unwrap();
        }
        let hints = k
            .hint("rustup toolchain", &cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert_eq!(ids(&hints), ["facts/two", "facts/one", "facts/three"]);
        // One open on the ignored twin lifts the penalty back above the untouched one.
        k.record_opened("facts/three", now).await.unwrap();
        let hints = k
            .hint("rustup toolchain", &cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert_eq!(ids(&hints), ["facts/two", "facts/three", "facts/one"]);

        // A record written 20 days ago: rank 0 → not hinted; still found by search.
        let old = (now - chrono::Duration::days(20)).format("%Y-%m-%dT%H:%M:%SZ");
        let path = k.root().join("records/old.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "---\ntype: record\ntitle: rustup toolchain pins\ntimestamp: {old}\n---\nold news\n"
            ),
        )
        .unwrap();
        k.sync().await.unwrap();
        let hints = k
            .hint("rustup toolchain", &cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert!(!ids(&hints).contains(&"records/old".to_owned()));
        let found = k.search("rustup toolchain", &cwd, 10).await.unwrap();
        let old = found.iter().find(|h| h.meta.id == "records/old").unwrap();
        assert_eq!(old.score, 0.0);
        assert_eq!(found.len(), 4);
    }

    /// The commit `Receipt.commit` names: `(author name, email, subject)`.
    async fn commit_of(k: &Knowledge, r: &Receipt) -> (String, String, String) {
        let repo = k.repo.clone().unwrap();
        let sha = r.commit.clone().unwrap();
        let (name, email) = repo.author(&sha).unwrap();
        let subject = k.history(None, 1).await.unwrap().changes[0].message.clone();
        (name, email, subject)
    }

    /// 136 §3.1 item 4: `merge` writes the canonical with the unions, turns
    /// the folded ids into redirect stubs, retargets `links`, commits once as
    /// `dreaming`; `restore` brings a stub back.
    #[tokio::test]
    async fn merge_folds_into_stubs_and_is_restorable() {
        let (_dir, k) = open().await;
        k.put(
            Put {
                evidence: vec!["s1".into()],
                tags: vec!["dev".into(), "mac".into()],
                ..put("records/state-2026-10-01", "State 1 Oct", "day one")
            },
            Source::Harvest,
        )
        .await
        .unwrap();
        k.put(
            Put {
                evidence: vec!["s2".into()],
                tags: vec!["mac".into(), "arm".into()],
                status: Some(Status::Fresh),
                ..put("records/state-2026-10-02", "State 2 Oct", "day two")
            },
            Source::Harvest,
        )
        .await
        .unwrap();
        k.put(
            put(
                "facts/citer",
                "Cites day one",
                "see [day one](/records/state-2026-10-01.md)",
            ),
            Source::Model,
        )
        .await
        .unwrap();
        let scope = AgentScope {
            owner: USER_OWNER.into(),
            line: BTreeSet::new(),
        };
        let before = k.history(None, 10).await.unwrap().changes.len();
        let r = k
            .merge(
                &[
                    "records/state-2026-10-01".into(),
                    "records/state-2026-10-02".into(),
                ],
                "entities/dev-machine",
                "The living entry.",
                &["s3".into()],
                &scope,
            )
            .await
            .unwrap();
        assert_eq!(r.op, KnowledgeOp::Merge);
        assert_eq!(
            r.ids,
            [
                "entities/dev-machine",
                "records/state-2026-10-01",
                "records/state-2026-10-02",
                "facts/citer"
            ],
            "the citer whose link was retargeted is in the receipt"
        );
        let (name, email, subject) = commit_of(&k, &r).await;
        assert_eq!((name.as_str(), email.as_str()), DREAMING_SIGNATURE);
        assert_eq!(subject, "dreaming merge entities/dev-machine");
        assert_eq!(
            k.history(None, 10).await.unwrap().changes.len(),
            before + 1,
            "one commit per op"
        );

        let c = k.get("entities/dev-machine").await.unwrap();
        assert_eq!(c.meta.type_, "fact", "type from the first folded file");
        assert_eq!(c.meta.title, "State 1 Oct");
        assert_eq!(c.meta.tags, ["dev", "mac", "arm"]);
        assert_eq!(c.meta.evidence, ["s1", "s2", "s3"]);
        assert_eq!(c.meta.status, Status::Tidy);
        assert_eq!(c.meta.source, Some(Source::Dreaming));
        assert!(c.text.ends_with("The living entry.\n"), "{}", c.text);
        assert_eq!(
            c.cited_by,
            [
                "facts/citer",
                "records/state-2026-10-01",
                "records/state-2026-10-02"
            ]
        );

        let s = k.get("records/state-2026-10-02").await.unwrap();
        assert_eq!(s.meta.status, Status::Merged);
        assert_eq!(s.meta.merged_into.as_deref(), Some("entities/dev-machine"));
        assert!(!s.meta.hint);
        assert_eq!(
            s.meta.evidence,
            ["s2"],
            "the stub keeps its own frontmatter"
        );
        assert!(
            s.text
                .ends_with("Merged into [entities/dev-machine](/entities/dev-machine.md)\n"),
            "{}",
            s.text
        );
        assert!(
            s.cited_by.is_empty(),
            "the citer now points at the canonical"
        );

        // Stubs never reach a note; the canonical does.
        let cwd = std::env::temp_dir();
        let hinted = k
            .hint("state oct", &cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        assert_eq!(
            hinted.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["entities/dev-machine"]
        );

        // Undo one stub through the ordinary restore.
        k.restore("records/state-2026-10-02", None).await.unwrap();
        let s = k.get("records/state-2026-10-02").await.unwrap();
        assert_eq!(s.meta.status, Status::Fresh);
        assert!(s.meta.merged_into.is_none() && s.meta.hint);
        assert!(s.text.ends_with("day two\n"));

        // Nothing to fold → refused; a missing id → refused before any write.
        assert!(
            k.merge(
                &["entities/dev-machine".into()],
                "entities/dev-machine",
                "b",
                &[],
                &scope
            )
            .await
            .is_err()
        );
        let e = k
            .merge(
                &["records/ghost".into()],
                "entities/dev-machine",
                "b",
                &[],
                &scope,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("no such concept: records/ghost"), "{e}");
    }

    /// Records every [`KnowledgeChanged`] it hears.
    #[derive(kameo::Actor)]
    struct Recorder(Arc<parking_lot::Mutex<Vec<KnowledgeChanged>>>);

    impl kameo::message::Message<KnowledgeChanged> for Recorder {
        type Reply = ();

        async fn handle(
            &mut self,
            e: KnowledgeChanged,
            _: &mut kameo::message::Context<Self, Self::Reply>,
        ) {
            self.0.lock().push(e);
        }
    }

    /// Subscribe a recorder to `k`'s events; `heard(n)` waits for `n` of them.
    async fn record(k: &Knowledge) -> Arc<parking_lot::Mutex<Vec<KnowledgeChanged>>> {
        let heard = Arc::new(parking_lot::Mutex::new(Vec::new()));
        use kameo::actor::Spawn;
        let actor = Recorder::spawn(Recorder(heard.clone()));
        k.rt.subscribe::<Recorder, KnowledgeChanged>(&actor).await;
        heard
    }

    async fn heard(
        rec: &Arc<parking_lot::Mutex<Vec<KnowledgeChanged>>>,
        n: usize,
    ) -> Vec<KnowledgeChanged> {
        for _ in 0..200 {
            if rec.lock().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        rec.lock().clone()
    }

    /// 136 P gap 4: merging A into B where C links to A rewrites C's file,
    /// commits it with the merge, publishes `KnowledgeChanged{C, Merge, dreaming}`,
    /// and `restore(C, before)` brings the old link back in the index too.
    #[tokio::test]
    async fn merge_rewrites_citers_and_publishes_them() {
        let (_dir, k) = open().await;
        k.put(put("facts/a", "A", "a body"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/b", "B", "b body"), Source::Model)
            .await
            .unwrap();
        k.put(
            put(
                "facts/c",
                "C",
                "see [a](/facts/a.md) and [again](../facts/a.md#top), not `[x](/facts/d.md)`",
            ),
            Source::Model,
        )
        .await
        .unwrap();
        let c_before = k.history(Some("facts/c"), 1).await.unwrap().changes[0]
            .commit
            .clone();
        let c_text = k.get("facts/c").await.unwrap().text;
        let rec = record(&k).await;
        let scope = AgentScope {
            owner: USER_OWNER.into(),
            line: BTreeSet::new(),
        };
        let r = k
            .merge(&["facts/a".into()], "facts/b", "ab", &[], &scope)
            .await
            .unwrap();
        assert_eq!(r.ids, ["facts/b", "facts/a", "facts/c"]);
        let events = heard(&rec, 3).await;
        let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["facts/a", "facts/b", "facts/c"]);
        assert!(
            events
                .iter()
                .all(|e| e.op == KnowledgeOp::Merge && e.source == Source::Dreaming),
            "{events:?}"
        );
        let c = k.get("facts/c").await.unwrap();
        assert!(
            c.text
                .contains("see [a](/facts/b.md) and [again](/facts/b.md), not `[x](/facts/d.md)`"),
            "{}",
            c.text
        );
        assert!(c.meta.consolidated_at.is_some());
        assert_eq!(
            c.links.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["facts/b"],
            "the code-span link is text, not a link"
        );
        // The file carries the retarget, so the citer's `restore` undoes it.
        k.restore("facts/c", Some(&c_before)).await.unwrap();
        let c = k.get("facts/c").await.unwrap();
        assert_eq!(c.text, c_text);
        assert_eq!(
            c.links.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["facts/a"]
        );
        assert_eq!(k.get("facts/a").await.unwrap().cited_by, ["facts/c"]);
    }

    /// `restore_many`: three ids from three different commits in ONE commit,
    /// bodies equal to the named revisions, one `KnowledgeChanged` per id.
    #[tokio::test]
    async fn restore_many_restores_several_ids_in_one_commit() {
        let (_dir, k) = open().await;
        let ids = ["facts/x", "facts/y", "facts/z"];
        let mut want = Vec::new();
        for id in ids {
            k.put(put(id, id, &format!("{id} v1")), Source::Model)
                .await
                .unwrap();
            let commit = k.history(Some(id), 1).await.unwrap().changes[0]
                .commit
                .clone();
            want.push((id.to_owned(), commit, k.get(id).await.unwrap().text));
        }
        for id in ids {
            k.put(put(id, id, &format!("{id} v2")), Source::Model)
                .await
                .unwrap();
        }
        let before = k.history(None, 20).await.unwrap().changes.len();
        let rec = record(&k).await;
        let pairs: Vec<(String, String)> = want
            .iter()
            .map(|(id, c, _)| (id.clone(), c.clone()))
            .collect();
        let change = k.restore_many(&pairs).await.unwrap();
        assert_eq!(change.message, "restore 3 concepts");
        let history = k.history(None, 20).await.unwrap().changes;
        assert_eq!(history.len(), before + 1, "one commit");
        assert_eq!(history[0].commit, change.commit);
        for (id, _, text) in &want {
            assert_eq!(&k.get(id).await.unwrap().text, text, "{id}");
        }
        let events = heard(&rec, 3).await;
        let mut heard_ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        heard_ids.sort_unstable();
        assert_eq!(heard_ids, ids);
        assert!(
            events
                .iter()
                .all(|e| e.op == KnowledgeOp::Put && e.source == Source::User)
        );
        // A pair that cannot be read refuses the batch before any write.
        let e = k
            .restore_many(&[
                ("facts/x".into(), want[0].1.clone()),
                ("facts/ghost".into(), want[0].1.clone()),
            ])
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("facts/ghost is not in commit"), "{e}");
        assert_eq!(k.history(None, 20).await.unwrap().changes.len(), before + 1);
    }

    /// 136 §3.1 item 4: `supersede`, `archive`, `verify` (both outcomes) and
    /// `retitle` — file effects, `dreaming` author, `dreaming <op> <id>`
    /// subject, and `restore` undoing each.
    #[tokio::test]
    async fn supersede_archive_verify_retitle_effects_author_and_restore() {
        let (_dir, k) = open().await;
        k.put(put("facts/old", "Old claim", "was true"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/new", "New claim", "is true"), Source::Model)
            .await
            .unwrap();
        let original = k.get("facts/old").await.unwrap().text;
        // The commit before each op — what dreaming's undo restores from.
        async fn base(k: &Knowledge) -> String {
            k.history(Some("facts/old"), 1).await.unwrap().changes[0]
                .commit
                .clone()
        }

        // supersede
        let b = base(&k).await;
        let r = k
            .supersede(
                "facts/old",
                "facts/new",
                Some("2026-10-07T21:05:00Z"),
                Some("the port moved"),
            )
            .await
            .unwrap();
        assert_eq!(
            (r.op, r.ids.as_slice()),
            (KnowledgeOp::Supersede, &["facts/old".to_owned()][..])
        );
        let (name, email, subject) = commit_of(&k, &r).await;
        assert_eq!((name.as_str(), email.as_str()), DREAMING_SIGNATURE);
        assert_eq!(subject, "dreaming supersede facts/old");
        let c = k.get("facts/old").await.unwrap();
        assert_eq!(c.meta.superseded_by.as_deref(), Some("facts/new"));
        assert_eq!(c.meta.valid_until.as_deref(), Some("2026-10-07T21:05:00Z"));
        assert!(!c.meta.hint);
        assert_eq!(c.meta.status, Status::Tidy, "supersede leaves status alone");
        assert!(
            c.text.ends_with(
                "was true\n\nSuperseded by [facts/new](/facts/new.md): the port moved\n"
            ),
            "{}",
            c.text
        );
        assert_eq!(k.get("facts/new").await.unwrap().cited_by, ["facts/old"]);
        assert!(
            k.supersede("facts/old", "facts/none", None, None)
                .await
                .is_err()
        );
        assert!(
            k.supersede("facts/old", "facts/old", None, None)
                .await
                .is_err()
        );
        k.restore("facts/old", Some(&b)).await.unwrap();
        assert_eq!(k.get("facts/old").await.unwrap().text, original);

        // archive
        let b = base(&k).await;
        let r = k.archive("facts/old", "superseded twice").await.unwrap();
        assert_eq!(r.op, KnowledgeOp::Archive);
        let (name, _, subject) = commit_of(&k, &r).await;
        assert_eq!(name, "dreaming");
        assert_eq!(subject, "dreaming archive facts/old");
        let c = k.get("facts/old").await.unwrap();
        assert_eq!(c.meta.status, Status::Archived);
        assert!(!c.meta.hint);
        assert!(
            c.text
                .ends_with("was true\n\n## Archived\n\nsuperseded twice\n"),
            "{}",
            c.text
        );
        // A second reason joins the same section.
        k.archive("facts/old", "and again").await.unwrap();
        let text = k.get("facts/old").await.unwrap().text;
        assert_eq!(text.matches("## Archived").count(), 1);
        assert!(text.ends_with("superseded twice\n\nand again\n"), "{text}");
        k.restore("facts/old", Some(&b)).await.unwrap();
        assert_eq!(k.get("facts/old").await.unwrap().text, original);

        // verify ok
        let b = base(&k).await;
        let r = k
            .verify(
                "facts/old",
                VerifyOutcome::Ok,
                None,
                &["conf/pg.toml".into(), "session:s1".into()],
            )
            .await
            .unwrap();
        assert_eq!(r.op, KnowledgeOp::Verify);
        let (name, _, subject) = commit_of(&k, &r).await;
        assert_eq!(name, "dreaming");
        assert_eq!(subject, "dreaming verify facts/old");
        let c = k.get("facts/old").await.unwrap();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(c.meta.verified.as_deref(), Some(today.as_str()));
        assert_eq!(c.meta.status, Status::Tidy);
        assert_eq!(
            c.meta.evidence,
            ["conf/pg.toml", "session:s1"],
            "the read sources are recorded"
        );
        assert!(
            c.text.ends_with("was true\n"),
            "ok writes no note: {}",
            c.text
        );
        // verify stale
        k.verify(
            "facts/old",
            VerifyOutcome::Stale,
            Some("port is 5433 now"),
            &["session:s1".into(), "conf/pg.d/port".into()],
        )
        .await
        .unwrap();
        let c = k.get("facts/old").await.unwrap();
        assert_eq!(
            c.meta.evidence,
            ["conf/pg.toml", "session:s1", "conf/pg.d/port"],
            "union, in order"
        );
        assert_eq!(c.meta.status, Status::Stale);
        assert_eq!(
            c.meta.verified.as_deref(),
            Some(today.as_str()),
            "verified kept"
        );
        assert!(
            c.text
                .ends_with("was true\n\n## Stale\n\nport is 5433 now\n"),
            "{}",
            c.text
        );
        k.restore("facts/old", Some(&b)).await.unwrap();
        assert_eq!(k.get("facts/old").await.unwrap().text, original);

        // retitle
        let b = base(&k).await;
        let r = k
            .retitle(
                "facts/old",
                "Old claim, short",
                "Old claim — the long first version of the title.",
            )
            .await
            .unwrap();
        assert_eq!(r.op, KnowledgeOp::Retitle);
        let (name, _, subject) = commit_of(&k, &r).await;
        assert_eq!(name, "dreaming");
        assert_eq!(subject, "dreaming retitle facts/old");
        let c = k.get("facts/old").await.unwrap();
        assert_eq!(c.meta.title, "Old claim, short");
        // D5: identity is not restamped; the dreaming touch is `consolidated_at`.
        assert_eq!(c.meta.source, Some(Source::Model));
        assert!(c.meta.consolidated_at.is_some());
        assert!(
            c.text.ends_with(
                "## Summary\n\nOld claim — the long first version of the title.\n\nwas true\n"
            ),
            "{}",
            c.text
        );
        // An empty summary falls back to the old title, inserted under the
        // existing `## Summary` (R-44: never a second heading).
        k.retitle("facts/old", "Shorter", "").await.unwrap();
        let text = k.get("facts/old").await.unwrap().text;
        assert!(
            text.ends_with(
                "## Summary\n\nOld claim, short\n\nOld claim — the long first version of the title.\n\nwas true\n"
            ),
            "{text}"
        );
        assert_eq!(text.matches("## Summary").count(), 1, "{text}");
        assert!(k.retitle("facts/old", "  ", "x").await.is_err());
        let long = "x".repeat(harvest::TITLE_MAX + 1);
        let e = k.retitle("facts/old", &long, "x").await.unwrap_err();
        assert!(e.to_string().contains("121 chars, the cap is 120"), "{e}");
        assert_eq!(k.get("facts/old").await.unwrap().text, text);
        k.retitle("facts/old", &"y".repeat(harvest::TITLE_MAX), "")
            .await
            .unwrap();
        k.restore("facts/old", Some(&b)).await.unwrap();
        assert_eq!(k.get("facts/old").await.unwrap().text, original);

        // Every op wants an existing file.
        for e in [
            k.archive("facts/none", "r").await.err(),
            k.verify("facts/none", VerifyOutcome::Ok, None, &[])
                .await
                .err(),
            k.retitle("facts/none", "t", "s").await.err(),
            k.supersede("facts/none", "facts/new", None, None)
                .await
                .err(),
        ] {
            assert!(
                e.unwrap()
                    .to_string()
                    .contains("no such concept: facts/none")
            );
        }
    }

    /// 136 R-5 / D5: the tidy-mark put from the dreaming session (no scope)
    /// keeps a desk's `owner: agent:ops, visibility: private`, its `scope`,
    /// `source` and `timestamp`; only `consolidated_at` moves.
    #[tokio::test]
    async fn dreaming_put_preserves_identity() {
        let (_dir, k, ops, _cto, _mkt) = scoped().await;
        k.put_for(
            Put {
                scope: Some(PathBuf::from("/work/relay")),
                status: Some(Status::Fresh),
                ..put("ops/p", "Private note", "body")
            },
            Source::Model,
            Some(ops),
        )
        .await
        .unwrap();
        let before = k.get("ops/p").await.unwrap().meta;
        assert_eq!(before.owner, "agent:ops");
        let (m, created) = k
            .put_for(
                Put {
                    status: Some(Status::Tidy),
                    ..put("ops/p", "Private note, tidied", "tidy body")
                },
                Source::Dreaming,
                None,
            )
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(m.owner, "agent:ops");
        assert_eq!(m.visibility, Visibility::Private);
        assert_eq!(m.scope, before.scope, "scope kept");
        assert_eq!(m.source, Some(Source::Model), "source kept");
        assert_eq!(m.timestamp, before.timestamp, "event date kept");
        assert!(m.consolidated_at.is_some());
        assert_eq!(m.status, Status::Tidy);
        // A put that names `visibility` still sets it.
        let (m, _) = k
            .put_for(
                Put {
                    visibility: Some(Visibility::Team),
                    ..put("ops/p", "Private note, tidied", "tidy body")
                },
                Source::Model,
                Some(ops),
            )
            .await
            .unwrap();
        assert_eq!(m.visibility, Visibility::Team);
        assert_eq!(m.owner, "agent:ops");
    }

    /// 136 R-7: `retitle` (and every op) keeps `timestamp`, so a 20-day-old
    /// record is still past the hint shelf's lifetime after a retitle.
    #[tokio::test]
    async fn ops_do_not_restamp_timestamp() {
        let (dir, k) = open().await;
        let path = dir.path().join(DIR).join("records").join("old.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(20))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        std::fs::write(
            &path,
            format!(
                "---\ntype: record\ntitle: Toolchain pinned rustup\nsource: harvest\ntimestamp: {old}\n---\nrustup toolchain pinned\n"
            ),
        )
        .unwrap();
        k.sync().await.unwrap();
        let cwd = Path::new("/tmp");
        assert!(
            k.hint("rustup toolchain", cwd, &BTreeSet::new(), 5)
                .await
                .unwrap()
                .is_empty(),
            "a record past its lifetime is not hinted"
        );
        k.retitle("records/old", "Toolchain pinned", "rustup")
            .await
            .unwrap();
        let m = k.get("records/old").await.unwrap().meta;
        assert_eq!(m.timestamp.as_deref(), Some(old.as_str()));
        assert_eq!(m.source, Some(Source::Harvest));
        assert!(m.consolidated_at.is_some());
        assert!(
            k.hint("rustup toolchain", cwd, &BTreeSet::new(), 5)
                .await
                .unwrap()
                .is_empty(),
            "still past its lifetime after the retitle"
        );
    }

    /// 136 R-23: a harvest that re-emits an id dreaming has merged keeps the
    /// evidence union and the `merged` status.
    #[tokio::test]
    async fn harvest_reput_unions_evidence_and_never_demotes_status() {
        let (dir, k) = open().await;
        let path = dir.path().join(DIR).join("facts").join("m.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\ntype: fact\ntitle: M\nstatus: merged\nmerged_into: facts/canon\nhint: false\nevidence: [a, b]\n---\nMerged into [facts/canon](/facts/canon.md)\n",
        )
        .unwrap();
        k.sync().await.unwrap();
        let (m, _) = k
            .put(
                Put {
                    status: Some(Status::Fresh),
                    evidence: vec!["c".into(), "a".into()],
                    ..put("facts/m", "M", "seen again")
                },
                Source::Harvest,
            )
            .await
            .unwrap();
        assert_eq!(m.evidence, ["a", "b", "c"]);
        assert_eq!(m.status, Status::Merged);
        assert_eq!(m.merged_into.as_deref(), Some("facts/canon"));
        // A file with no status (or `fresh`) still takes `fresh`.
        let (m, _) = k
            .put(
                Put {
                    status: Some(Status::Fresh),
                    ..put("facts/n", "N", "new")
                },
                Source::Harvest,
            )
            .await
            .unwrap();
        assert_eq!(m.status, Status::Fresh);
    }

    /// 136 R-25 / D5: a `source: user` entry is never merged away, superseded,
    /// archived or rewritten by a dreaming put — the file stays byte-identical
    /// — while a retitle is allowed.
    #[tokio::test]
    async fn user_authored_entries_are_refused_by_the_destructive_ops() {
        let (dir, k) = open().await;
        k.put(put("facts/h", "Human note", "typed by hand"), Source::User)
            .await
            .unwrap();
        k.put(put("facts/other", "Other", "model"), Source::Model)
            .await
            .unwrap();
        let path = dir.path().join(DIR).join("facts").join("h.md");
        let before = std::fs::read(&path).unwrap();
        let scope = AgentScope {
            owner: USER_OWNER.into(),
            line: BTreeSet::new(),
        };
        let errs = [
            k.archive("facts/h", "noise").await.err(),
            k.supersede("facts/h", "facts/other", None, None)
                .await
                .err(),
            k.merge(&["facts/h".into()], "facts/other", "b", &[], &scope)
                .await
                .err(),
            k.merge(&["facts/other".into()], "facts/h", "b", &[], &scope)
                .await
                .err(),
            k.put_for(
                Put {
                    status: Some(Status::Archived),
                    ..put("facts/h", "Human note", "rewritten")
                },
                Source::Dreaming,
                None,
            )
            .await
            .err(),
        ];
        for e in errs {
            let e = e.expect("refused").to_string();
            assert!(e.contains("human-authored (source: user)"), "{e}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before, "byte-identical");
        assert_eq!(
            k.get("facts/other").await.unwrap().meta.status,
            Status::Tidy,
            "the model entry was not folded either"
        );
        k.retitle("facts/h", "Human note, short", "Human note")
            .await
            .unwrap();
        assert_eq!(
            k.get("facts/h").await.unwrap().meta.source,
            Some(Source::User)
        );
    }

    /// 136 R-44: merging onto a previously archived canonical brings it back
    /// live — `hint: true`, no `superseded_by`/`valid_until`.
    #[tokio::test]
    async fn merge_revives_an_archived_canonical() {
        let (_dir, k) = open().await;
        k.put(put("facts/canon", "Canon", "old"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/dup", "Dup", "dup"), Source::Model)
            .await
            .unwrap();
        k.put(put("facts/newer", "Newer", "n"), Source::Model)
            .await
            .unwrap();
        k.supersede(
            "facts/canon",
            "facts/newer",
            Some("2026-01-01T00:00:00Z"),
            None,
        )
        .await
        .unwrap();
        k.archive("facts/canon", "superseded").await.unwrap();
        let m = k.get("facts/canon").await.unwrap().meta;
        assert!(!m.hint && m.status == Status::Archived && m.superseded_by.is_some());
        let scope = AgentScope {
            owner: USER_OWNER.into(),
            line: BTreeSet::new(),
        };
        k.merge(
            &["facts/dup".into()],
            "facts/canon",
            "merged body",
            &[],
            &scope,
        )
        .await
        .unwrap();
        let m = k.get("facts/canon").await.unwrap().meta;
        assert!(m.hint);
        assert_eq!(m.status, Status::Tidy);
        assert!(m.superseded_by.is_none() && m.valid_until.is_none());
        assert_eq!(
            m.source,
            Some(Source::Model),
            "existing canonical keeps its source"
        );
    }

    /// 136 R-26: dead rows are filtered before the bm25 cut, so a note with
    /// room for five lines shows both live matches even when four archived
    /// twins and a merge stub outrank them.
    #[tokio::test]
    async fn hint_window_is_not_spent_on_dead_rows() {
        let (_dir, k) = open().await;
        let cwd = Path::new("/tmp");
        // Dead rows repeat the terms (better bm25) …
        for i in 0..4 {
            k.put(
                Put {
                    status: Some(Status::Archived),
                    hint: Some(false),
                    ..put(
                        &format!("facts/dead{i}"),
                        "pelican relay pelican relay",
                        "pelican relay pelican relay pelican relay",
                    )
                },
                Source::Dreaming,
            )
            .await
            .unwrap();
        }
        k.put(
            Put {
                status: Some(Status::Merged),
                merged_into: Some("facts/live0".into()),
                ..put(
                    "facts/stub",
                    "pelican relay pelican",
                    "pelican relay pelican relay",
                )
            },
            Source::Dreaming,
        )
        .await
        .unwrap();
        // … the live ones mention them once.
        for i in 0..2 {
            k.put(
                put(
                    &format!("facts/live{i}"),
                    "the pelican relay",
                    "restart it from the relay host",
                ),
                Source::Model,
            )
            .await
            .unwrap();
        }
        let hints = k
            .hint("pelican relay", cwd, &BTreeSet::new(), 5)
            .await
            .unwrap();
        let mut ids: Vec<&str> = hints.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["facts/live0", "facts/live1"]);
        // `search` still sees everything.
        assert_eq!(k.search("pelican relay", cwd, 10).await.unwrap().len(), 7);
    }

    /// 136 D6: an unattended session consolidates only while registered by
    /// the open night; the attended answer comes from the `Consolidator`.
    #[tokio::test]
    async fn consolidating_session_is_the_registered_night_or_the_owner_root() {
        struct OwnerRoot(SessionId);
        #[async_trait::async_trait]
        impl Consolidator for OwnerRoot {
            async fn may_consolidate(&self, s: SessionId) -> bool {
                s == self.0
            }
        }
        let (_dir, k) = open().await;
        let (owner, dreaming, routine) = (SessionId::new(), SessionId::new(), SessionId::new());
        assert!(!k.may_consolidate(owner).await, "nobody until installed");
        k.set_consolidator(Arc::new(OwnerRoot(owner)));
        assert!(k.may_consolidate(owner).await);
        assert!(!k.may_consolidate(dreaming).await, "no open night");
        k.register_consolidating_session(dreaming);
        assert!(k.may_consolidate(dreaming).await);
        assert!(k.is_consolidating_session(dreaming));
        assert!(!k.may_consolidate(routine).await, "another routine's root");
        assert!(!k.is_consolidating_session(owner));
        k.clear_consolidating_session();
        assert!(!k.may_consolidate(dreaming).await, "night closed");
        assert!(k.may_consolidate(owner).await);
    }

    /// A fixed session → scope table standing in for the roster (67 §9).
    struct Table(HashMap<SessionId, AgentScope>);

    #[async_trait::async_trait]
    impl Scoper for Table {
        fn scope_for(&self, session: SessionId) -> Option<AgentScope> {
            self.0.get(&session).cloned()
        }
        async fn bundle_for(&self, _: SessionId) -> std::result::Result<Option<String>, String> {
            Ok(None)
        }
    }

    fn scope(owner: &str, line: &[&str]) -> AgentScope {
        AgentScope {
            owner: owner.into(),
            line: line.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// `ops` reports to `cto`; `mkt` is another line. Sessions: ops, cto, mkt.
    async fn scoped() -> (
        tempfile::TempDir,
        Arc<Knowledge>,
        SessionId,
        SessionId,
        SessionId,
    ) {
        let (dir, k) = open().await;
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
        (dir, k, ops, cto, mkt)
    }

    #[tokio::test]
    async fn existing_concepts_default_to_user_and_shared() {
        let (dir, k) = open().await;
        // A hand-written file without the 67 §9 keys.
        let path = dir.path().join(DIR).join("facts").join("old.md");
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
        let (_dir, k, ops, cto, mkt) = scoped().await;
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
        let (_dir, k, ops, cto, mkt) = scoped().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
        let (dir, k) = open().await;
        k.put(put("facts/a", "a", "x"), Source::Model)
            .await
            .unwrap();
        drop(k);
        std::fs::remove_file(dir.path().join(INDEX_FILE)).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let k = Knowledge::open(dir.path(), rt).await.unwrap();
        assert_eq!(k.list(None, None, None, 10).await.unwrap().len(), 1);
    }

    #[test]
    fn regenerate_indexes_skips_dot_dirs_and_unchanged_files() {
        let scratch = crate::test_dir("knowledge");
        let dir = scratch.path().to_path_buf();
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
        let (_dir, k) = open().await;
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
        let (_dir, k) = open().await;
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
