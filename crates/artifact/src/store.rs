//! The store: `<data_dir>/artifacts/` blobs by content hash plus the
//! `<data_dir>/artifacts.sqlite` index beside it (35 §3.3–§3.4; the
//! `knowledge/` + `knowledge.sqlite` layout) and `<data_dir>/artifacts.owners/`,
//! one OS-locked file per live opener ([`OWNERS`]). Truth is the index; blobs
//! without a row are swept at start.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use tools::SessionResources;

use crate::archive;
use crate::kind::{self, Receipt};
use crate::pdf::{self, Source};
use crate::search;
use crate::tools::ShowArgs;
use crate::{
    Artifact, ArtifactShown, ClaimInvocation, Commit, CompleteInvocation, Kind, MAX_BYTES,
    MarkInvocationUnknown, svg,
};

/// Directory under `data_dir`.
pub const DIR: &str = "artifacts";
/// Index file name, beside [`DIR`] in `data_dir`.
pub const DB: &str = "artifacts.sqlite";
/// Largest serialized [`Invocation::result`] kept; larger results are
/// replaced by an `{"omitted": …}` marker (46 §6.4).
pub const MAX_INVOCATION_RESULT: usize = 64 * 1024;
/// Most rows one [`crate::ListInvocations`] answers.
pub const MAX_LIST_INVOCATIONS: usize = 64;

/// Why a [`Commit`] did not insert a row. Nothing is published on `Err`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitError {
    /// `expected_base` is no longer the latest version of `(session, name)`;
    /// `latest` is the record that is (`None` when the name has no rows).
    Conflict {
        /// `call_id` of the current latest version; `None` when the name has no rows.
        latest: Option<String>,
    },
    /// Invalid input or a store failure; what the model may read.
    Other(String),
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { latest: Some(l) } => {
                write!(f, "conflict: base is not latest ({l} is)")
            }
            Self::Conflict { latest: None } => f.write_str("conflict: base no longer exists"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

impl From<CommitError> for String {
    fn from(e: CommitError) -> Self {
        e.to_string()
    }
}

/// Lifecycle of one app invocation row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum InvocationStatus {
    /// Claimed, dispatch in progress.
    Running,
    /// Completed with `ok: true`.
    Succeeded,
    /// Completed with `ok: false`.
    Failed,
    /// Was `running` when its owning harness stopped: outcome unknowable.
    Unknown,
}

impl InvocationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Running, Self::Succeeded, Self::Failed, Self::Unknown]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// One durable pressed invocation (46 §6.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Invocation {
    /// Row key.
    pub request_id: String,
    /// Owning session.
    pub session: SessionId,
    /// The app record.
    pub record_call_id: String,
    /// The bound statement.
    pub statement_id: String,
    /// The press, if any.
    pub action_id: Option<String>,
    /// Resolved tool name.
    pub tool: String,
    /// [`args_hash`] of the arguments.
    pub args_hash: String,
    /// Current status.
    pub status: InvocationStatus,
    /// Stored (redacted, bounded) result once completed.
    pub result: Option<serde_json::Value>,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds, once completed.
    pub finished_at: Option<i64>,
}

/// Outcome of [`ClaimInvocation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Claim {
    /// Inserted as `running`; the caller dispatches.
    New,
    /// Same request id, same identity and arguments: do not dispatch again.
    Existing(Box<Invocation>),
    /// Same request id, different identity or arguments.
    Conflict,
}

/// `sha256[..16]` of the canonical (sorted-key) JSON of `args`.
pub fn args_hash(args: &serde_json::Value) -> String {
    let mut s = String::new();
    canonical(args, &mut s);
    sha16(s.as_bytes())
}

fn canonical(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        _ => out.push_str(&v.to_string()),
    }
}

/// What one `show` produced: the record plus the numbers its receipt needs.
#[derive(Clone, Debug)]
pub struct Shown {
    /// The stored record.
    pub artifact: Artifact,
    /// Receipt extras (lines, table dims, picture dims, warnings).
    pub receipt: Receipt,
}

/// Blob directory + index + the runtime that publishes [`ArtifactShown`].
pub struct State {
    /// `<data_dir>/artifacts/`, or `<scratch>/artifacts/` for a private tree.
    pub dir: PathBuf,
    pool: SqlitePool,
    rt: Runtime,
    /// Extra font directories for PDF compiles (`artifact.fonts`).
    pub fonts: Vec<PathBuf>,
    /// Writers (`show`, `commit`) hold it shared from first blob write to row
    /// insert; [`State::sweep`] and [`State::close`] hold it exclusively, so
    /// neither collects a blob whose row is about to exist.
    gate: tokio::sync::RwLock<()>,
    /// Set by [`State::close`] before it waits on the gate: a writer that
    /// gets the gate afterwards writes nothing (51 §4.4).
    closing: AtomicBool,
    /// Name of this opener's file in [`OWNERS`]; stamped on the invocations
    /// it claims.
    owner: String,
    /// The OS lock on that file, held for the process lifetime: released by
    /// the kernel when the process dies, so liveness is never guessed from a
    /// pid. `None` for a private tree, whose rows die with the process anyway —
    /// which is also why only a store holding one binds documents (59 §7.2).
    owner_lock: Option<std::fs::File>,
    /// Derived search index (design doc 49 §3.2); invalidated by
    /// [`State::show`], [`State::commit`], [`State::edit_version`] and
    /// [`State::delete_session`], the only writers.
    search: Arc<search::Cache>,
}

/// Directory beside [`DB`]: one empty file per live opener of the index,
/// held under an exclusive OS lock ([`std::fs::File::try_lock`]).
/// `app_invocations.owner` names it; [`State::open`] recovers a `running`
/// row as `unknown` only when its owner's lock is free (46 §6.4), so a second
/// harness on one data dir never voids the first one's in-flight calls.
const OWNERS: &str = "artifacts.owners";

/// Create and lock this opener's owner file. Locked under a dot-name first
/// and renamed in, so no scan ever sees it unlocked under its real name.
fn lock_owner(owners: &Path) -> anyhow::Result<(String, std::fs::File)> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let owner = format!(
        "{}-{nanos:x}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let tmp = owners.join(format!(".{owner}"));
    let file =
        std::fs::File::create_new(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.try_lock()
        .with_context(|| format!("lock {}", tmp.display()))?;
    let path = owners.join(&owner);
    std::fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok((owner, file))
}

/// Take a dead owner's lock: `Some(file)` holding it, `None` when the file is
/// gone (created before its owner's first claim and removed only once its
/// lock was taken, so missing is dead too), `Err(None)` when its owner is alive.
fn lock_dead(path: &Path) -> anyhow::Result<Result<Option<std::fs::File>, ()>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Ok(None)),
        Err(e) => return Err(e).with_context(|| format!("open {}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => Ok(Ok(Some(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Err(())),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("probe {}", path.display()))
        }
    }
}

/// A row still `running` whose owner no longer holds its lock never
/// completed: that harness died mid-dispatch. Its outcome is unknowable,
/// never retried (46 §6.4). Only owners proven dead are recovered, one at a
/// time with their lock held through the update, so an opener that claims
/// between this scan and the update is never mistaken for one; rows of live
/// owners (another harness on this data dir) are theirs to finish. Legacy
/// rows without an owner are recovered. Files of dead openers are removed.
async fn recover_abandoned(pool: &SqlitePool, owners: &Path) -> anyhow::Result<()> {
    let names: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT DISTINCT owner FROM app_invocations WHERE status = 'running'")
            .fetch_all(pool)
            .await
            .context("list running app invocations")?;
    for (name,) in names {
        let _held = match &name {
            None => None,
            Some(name) => match lock_dead(&owners.join(name))? {
                Ok(held) => held,
                Err(()) => continue,
            },
        };
        sqlx::query(
            "UPDATE app_invocations SET status = 'unknown' \
             WHERE status = 'running' AND owner IS ?",
        )
        .bind(&name)
        .execute(pool)
        .await
        .context("recover abandoned app invocations")?;
        if let Some(name) = &name {
            let _ = std::fs::remove_file(owners.join(name));
        }
    }
    // Dead openers that left nothing running. Dot-names are being created:
    // locked, not yet renamed in.
    for entry in std::fs::read_dir(owners)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Ok(Some(_dead)) = lock_dead(&entry.path())? {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

const COLS: &str = "call_id, id, session, name, title, kind, mime, bytes, version, \
                    source, preview, pages, origin, created_at";

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
);

const INV_COLS: &str = "request_id, session, record_call_id, statement_id, action_id, tool, \
                        args_hash, status, result, started_at, finished_at";

type InvRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
);

/// `Err` when the row is corrupt, so a status lookup never mistakes an
/// unreadable outcome for a missing one.
fn invocation_from_row(r: InvRow) -> Result<Invocation, String> {
    let (
        request_id,
        session,
        record_call_id,
        statement_id,
        action_id,
        tool,
        args_hash,
        status,
        result,
        started_at,
        finished_at,
    ) = r;
    Ok(Invocation {
        session: session
            .parse()
            .map_err(|e| format!("{request_id}: app_invocations.session {session:?}: {e}"))?,
        status: InvocationStatus::parse(&status)
            .ok_or_else(|| format!("{request_id}: app_invocations.status {status:?}"))?,
        result: result
            .map(|r| serde_json::from_str(&r))
            .transpose()
            .map_err(|e| format!("{request_id}: app_invocations.result: {e}"))?,
        request_id,
        record_call_id,
        statement_id,
        action_id,
        tool,
        args_hash,
        started_at,
        finished_at,
    })
}

fn sha16(bytes: &[u8]) -> String {
    let mut hex = format!("{:x}", Sha256::digest(bytes));
    hex.truncate(16);
    hex
}

fn blob_name(id: &str, name: &str) -> String {
    match kind::ext(name) {
        Some(e) => format!("{id}.{e}"),
        None => id.to_owned(),
    }
}

/// Document title for a markdown body: the front matter `title:` when there is
/// one (the same key the PDF template prints), else the first `#` heading.
fn doc_title(text: &str) -> Option<String> {
    front_matter_title(text).or_else(|| first_heading(text))
}

fn front_matter_title(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    lines
        .take_while(|l| l.trim_end() != "---")
        .find_map(|l| l.strip_prefix("title:"))
        .map(|t| t.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|t| !t.is_empty())
}

fn first_heading(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.trim_start_matches('#');
        let hashes = l.len() - rest.len();
        ((1..=6).contains(&hashes) && rest.starts_with(' '))
            .then(|| rest.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    })
}

fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

fn row_to_artifact(dir: &Path, r: Row) -> anyhow::Result<Artifact> {
    let (
        call_id,
        id,
        session,
        name,
        title,
        kind,
        mime,
        bytes,
        version,
        source,
        preview,
        pages,
        origin,
        created_at,
    ) = r;
    Ok(Artifact {
        path: dir.join(blob_name(&id, &name)),
        id,
        session: session
            .parse()
            .with_context(|| format!("artifacts.session {session:?}"))?,
        call_id,
        kind: Kind::parse(&kind).with_context(|| format!("artifacts.kind {kind:?}"))?,
        name,
        title,
        mime,
        bytes: u64::try_from(bytes).unwrap_or_default(),
        version: u32::try_from(version).unwrap_or_default(),
        source: source.map(|s| dir.join(s)),
        preview: preview.map(|p| dir.join(p)),
        pages: pages.and_then(|p| u32::try_from(p).ok()),
        origin: origin.map(PathBuf::from),
        created_at,
    })
}

/// `None` → every row; `Some(&[])` → none. Newest first.
async fn select(
    pool: &SqlitePool,
    dir: &Path,
    sessions: Option<&[SessionId]>,
) -> anyhow::Result<Vec<Artifact>> {
    if sessions.is_some_and(<[_]>::is_empty) {
        return Ok(Vec::new());
    }
    // Only `COLS` (a const) and a `?` list sized to `sessions` are interpolated; ids are bound.
    let where_ = sessions.map_or(String::new(), |s| {
        format!("WHERE session IN ({})", vec!["?"; s.len()].join(","))
    });
    let sql = format!("SELECT {COLS} FROM artifacts {where_} ORDER BY created_at DESC, rowid DESC");
    let mut q = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(sql));
    for s in sessions.unwrap_or_default() {
        q = q.bind(s.to_string());
    }
    q.fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| row_to_artifact(dir, r))
        .collect()
}

impl State {
    /// Create `<data_dir>/artifacts/`, take this opener's [`OWNERS`] lock,
    /// open `<data_dir>/artifacts.sqlite`, run migrations, recover the
    /// invocations of dead openers.
    pub async fn open(data_dir: &Path, rt: Runtime, fonts: Vec<PathBuf>) -> anyhow::Result<Self> {
        let dir = data_dir.join(DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let owners = data_dir.join(OWNERS);
        std::fs::create_dir_all(&owners).with_context(|| format!("create {}", owners.display()))?;
        let (owner, owner_lock) = lock_owner(&owners)?;
        let db = data_dir.join(DB);
        let opts = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", db.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        recover_abandoned(&pool, &owners).await?;
        Ok(Self {
            dir,
            pool,
            rt,
            fonts,
            gate: tokio::sync::RwLock::new(()),
            closing: AtomicBool::new(false),
            owner,
            owner_lock: Some(owner_lock),
            search: Arc::default(),
        })
    }

    /// A private tree's store (51 §4.5): the same schema in a single-connection
    /// `:memory:` database (every SQLite memory connection is its own database),
    /// no journal, `temp_store=MEMORY`, blobs under `<root>/artifacts/` created
    /// mode 0700. No owner lock and no recovery: nothing outlives the process.
    pub async fn open_private(root: &Path, rt: Runtime, fonts: Vec<PathBuf>) -> anyhow::Result<Self> {
        let dir = root.join(DIR);
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory)
            .pragma("temp_store", "MEMORY");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .connect_with(opts)
            .await
            .context("open in-memory artifact index")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self {
            dir,
            pool,
            rt,
            fonts,
            gate: tokio::sync::RwLock::new(()),
            closing: AtomicBool::new(false),
            owner: "private".to_owned(),
            owner_lock: None,
            search: Arc::default(),
        })
    }

    /// Close barrier of a private tree (51 §4.4): refuse every writer from
    /// now on, wait for the admitted ones, drop the database, remove the
    /// blobs. Idempotent; a missing blob directory is already clean.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.closing.store(true, Ordering::Release);
        let _exclusive = self.gate.write().await;
        self.pool.close().await;
        match tokio::fs::remove_dir_all(&self.dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove {}", self.dir.display())),
        }
    }

    /// `Err` once [`State::close`] began: checked under the shared gate, so a
    /// writer either finishes before the close or writes nothing.
    fn admit(&self) -> Result<(), String> {
        if self.closing.load(Ordering::Acquire) {
            return Err("session is closing".into());
        }
        Ok(())
    }

    /// Write `<dir>/<file>` unless it already exists (same hash = same bytes).
    /// The temp name is unique per call: two writers of one blob (same bytes
    /// in two sessions) each rename their own file; the second rename is an
    /// atomic replace with identical content.
    async fn put_blob(&self, file: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = self.dir.join(file);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(path);
        }
        let part = self.dir.join(format!(
            "{file}.{}.{}.part",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        tokio::fs::write(&part, bytes)
            .await
            .with_context(|| format!("write {}", part.display()))?;
        if let Err(e) = tokio::fs::rename(&part, &path).await {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e).with_context(|| format!("rename {}", path.display()));
        }
        Ok(path)
    }

    /// 35 §3.3 `Show`: validate → kind → compile/rasterise → blob → version →
    /// insert → publish. `Err` is what the model reads.
    pub async fn show(
        &self,
        session: SessionId,
        call_id: String,
        args: ShowArgs,
    ) -> Result<Shown, String> {
        let t0 = Instant::now();
        let (name, bytes, origin, content) = match (args.path, args.content) {
            (Some(_), Some(_)) => return Err("pass either path or content, not both".into()),
            (None, None) => return Err("pass path (an existing file) or content + name".into()),
            (None, Some(content)) => {
                let name = args.name.ok_or("content requires name")?;
                if kind::ext(&name).is_none() {
                    return Err(format!("{name}: name needs an extension"));
                }
                (name, content.clone().into_bytes(), None, Some(content))
            }
            (Some(path), None) => {
                let meta = tokio::fs::metadata(&path)
                    .await
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if !meta.is_file() {
                    return Err(format!("{}: not a regular file", path.display()));
                }
                if meta.len() > MAX_BYTES {
                    return Err(format!(
                        "{}: {} exceeds {}",
                        path.display(),
                        kind::human(meta.len()),
                        kind::human(MAX_BYTES)
                    ));
                }
                let name = args
                    .name
                    .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .ok_or_else(|| format!("{}: no file name", path.display()))?;
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                (name, bytes, Some(path), None)
            }
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_BYTES {
            return Err(format!(
                "{name}: {} exceeds {}",
                kind::human(bytes.len() as u64),
                kind::human(MAX_BYTES)
            ));
        }

        let kind = kind::of(&name);
        let mut receipt = Receipt::default();
        let mut source_file = None;
        let mut pages = None;
        let mut preview_png = None;
        let mut heading_src: Option<&str> = None;
        let mut compiled = false;
        let mut listing = None;
        let _writing = self.gate.read().await;
        self.admit().map_err(|e| format!("{name}: {e}"))?;

        let bytes = match (kind, &content) {
            (Kind::Pdf, Some(content)) => {
                let source = args.source.unwrap_or(Source::Markdown);
                let src_ext = if matches!(source, Source::Typst) {
                    "typ"
                } else {
                    "md"
                };
                if src_ext == "md" {
                    heading_src = Some(content.as_str());
                }
                let src_name = format!("{}.{src_ext}", sha16(content.as_bytes()));
                self.put_blob(&src_name, content.as_bytes())
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?;
                source_file = Some(src_name);
                let out = pdf::compile(content, source, &self.fonts)
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?
                    .map_err(|d| format!("{name}: compile failed\n{}", pdf::format_diags(&d)))?;
                compiled = true;
                pages = Some(out.pages);
                receipt.warnings = out.warnings;
                preview_png = Some(out.preview_png);
                out.pdf
            }
            (Kind::Svg, _) => {
                let r = svg::rasterize(&bytes).map_err(|e| format!("{name}: {e:#}"))?;
                receipt.dims = Some((r.width, r.height));
                preview_png = Some(r.png);
                bytes
            }
            (Kind::Markdown | Kind::Text | Kind::Html | Kind::Mermaid, _) => {
                receipt.lines = Some(count_lines(&bytes));
                bytes
            }
            // Headers only, never extracted (35 §3.2); a corrupt archive is
            // still shown as an Archive, just without a listing.
            (Kind::Archive, _) => {
                match archive::list(&bytes, &name) {
                    Ok(l) => {
                        receipt.files = Some(l.total_entries);
                        listing = Some(l);
                    }
                    Err(e) => tracing::warn!(target: "artifact", name, "archive listing: {e:#}"),
                }
                bytes
            }
            (Kind::Csv, _) => {
                let tsv = kind::ext(&name).as_deref() == Some("tsv");
                if let Ok((h, _, rows)) = kind::csv_table(&bytes, tsv, 0) {
                    receipt.table = Some((rows, h.len()));
                }
                bytes
            }
            _ => bytes,
        };
        let text_for_title = match kind {
            Kind::Markdown => std::str::from_utf8(&bytes).ok(),
            _ => heading_src,
        };
        let title = args
            .title
            .or_else(|| text_for_title.and_then(doc_title))
            .unwrap_or_else(|| name.clone());

        let id = sha16(&bytes);
        let blob = blob_name(&id, &name);
        let path = self
            .put_blob(&blob, &bytes)
            .await
            .map_err(|e| format!("{name}: {e:#}"))?;
        if let Some(l) = &listing {
            let json = serde_json::to_vec(l).map_err(|e| format!("{name}: {e}"))?;
            self.put_blob(&format!("{blob}.listing.json"), &json)
                .await
                .map_err(|e| format!("{name}: {e:#}"))?;
        }
        let preview_file = match preview_png {
            Some(png) => {
                let f = format!("{id}.preview.png");
                self.put_blob(&f, &png)
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?;
                Some(f)
            }
            None => None,
        };

        let mut artifact = Artifact {
            mime: kind::mime(kind, &name),
            id,
            session,
            call_id,
            name,
            title,
            kind,
            bytes: bytes.len() as u64,
            version: 0,
            path,
            source: source_file.as_deref().map(|s| self.dir.join(s)),
            preview: preview_file.as_deref().map(|p| self.dir.join(p)),
            pages,
            origin,
            created_at: chrono::Utc::now().timestamp(),
        };
        // The version is assigned inside the INSERT so two parallel `show`s of one
        // name (one assistant message, `Show` runs off the mailbox) cannot both read
        // the same count. Plain INSERT: a record is immutable, so a colliding
        // `call_id` (e.g. an app committed under it) is refused, never replaced.
        // Only `COLS` (a const) is interpolated; every value is bound.
        let (version,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO artifacts ({COLS}) VALUES (?,?,?,?,?,?,?,?,\
             (SELECT COUNT(*) + 1 FROM artifacts WHERE session = ? AND name = ?),\
             ?,?,?,?,?) RETURNING version"
        )))
        .bind(&artifact.call_id)
        .bind(&artifact.id)
        .bind(artifact.session.to_string())
        .bind(&artifact.name)
        .bind(&artifact.title)
        .bind(artifact.kind.as_str())
        .bind(&artifact.mime)
        .bind(i64::try_from(artifact.bytes).unwrap_or(i64::MAX))
        .bind(artifact.session.to_string())
        .bind(&artifact.name)
        .bind(source_file)
        .bind(preview_file)
        .bind(artifact.pages.map(i64::from))
        .bind(
            artifact
                .origin
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        )
        .bind(artifact.created_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(|d| d.is_unique_violation())
            {
                format!(
                    "{}: call {} already recorded",
                    artifact.name, artifact.call_id
                )
            } else {
                format!("{}: index: {e}", artifact.name)
            }
        })?;
        artifact.version = u32::try_from(version).unwrap_or_default();
        self.search.invalidate();
        if artifact.kind == Kind::Markdown {
            self.bind_document(&artifact, &bytes).await;
        }

        tracing::info!(
            target: "artifact",
            kind = artifact.kind.as_str(),
            bytes = artifact.bytes,
            ms = t0.elapsed().as_millis() as u64,
            compiled,
            pages = artifact.pages,
            ok = true,
            "show"
        );
        self.rt
            .publish(ArtifactShown {
                session,
                call_id: artifact.call_id.clone(),
                artifact: artifact.clone(),
            })
            .await;
        Ok(Shown { artifact, receipt })
    }

    /// 46 §3.2–§3.3 [`Commit`]: validate → blob + source blob → one
    /// `INSERT … WHERE base is latest RETURNING version` → publish. The
    /// compare and the version insert are one statement, so two patches
    /// against one base cannot both land; the loser gets
    /// [`CommitError::Conflict`] with the current latest record. A record is
    /// immutable: recommitting a `call_id` returns the existing record only
    /// when it is exactly this one ([`same_record`]) and is refused otherwise.
    pub async fn commit(&self, m: Commit) -> Result<Artifact, CommitError> {
        let name = m.name;
        let invalid = |e: String| CommitError::Other(format!("{name}: {e}"));
        if kind::ext(&name).is_none() {
            return Err(invalid("name needs an extension".into()));
        }
        if m.bytes.len() as u64 > MAX_BYTES {
            return Err(invalid(format!(
                "{} exceeds {}",
                kind::human(m.bytes.len() as u64),
                kind::human(MAX_BYTES)
            )));
        }
        if let Some((suffix, src)) = &m.source {
            if suffix.is_empty() || suffix.contains(['/', '\\']) || suffix.contains("..") {
                return Err(invalid(format!(
                    "source suffix {suffix:?} is not a file suffix"
                )));
            }
            if src.len() as u64 > MAX_BYTES {
                return Err(invalid(format!(
                    "source {} exceeds {}",
                    kind::human(src.len() as u64),
                    kind::human(MAX_BYTES)
                )));
            }
        }
        let id = sha16(&m.bytes);
        // The sidecar's own hash: metadata for one UI hash may differ between
        // commits (tool identities), and must never overwrite.
        let source_file = m
            .source
            .as_ref()
            .map(|(suffix, src)| format!("{}.{suffix}", sha16(src)));
        let mut artifact = Artifact {
            mime: kind::mime(m.kind, &name),
            path: self.dir.join(blob_name(&id, &name)),
            source: source_file.as_deref().map(|s| self.dir.join(s)),
            id,
            session: m.session,
            call_id: m.call_id,
            name,
            title: m.title,
            kind: m.kind,
            bytes: m.bytes.len() as u64,
            version: 0,
            preview: None,
            pages: None,
            origin: None,
            created_at: chrono::Utc::now().timestamp(),
        };
        if let Some(recorded) = self.recorded(&artifact).await {
            return recorded;
        }

        let _writing = self.gate.read().await;
        self.admit()
            .map_err(|e| CommitError::Other(format!("{}: {e}", artifact.name)))?;
        if let (Some(f), Some((_, src))) = (&source_file, &m.source) {
            self.put_blob(f, src)
                .await
                .map_err(|e| CommitError::Other(format!("{}: {e:#}", artifact.name)))?;
        }
        self.put_blob(&blob_name(&artifact.id, &artifact.name), &m.bytes)
            .await
            .map_err(|e| CommitError::Other(format!("{}: {e:#}", artifact.name)))?;
        let session = artifact.session.to_string();
        // One statement: SQLite holds the write lock across the version
        // subselect and the latest-row check, so the compare-and-swap cannot
        // interleave with another insert. Only `COLS` (a const) is interpolated.
        let inserted = sqlx::query_as::<_, (i64,)>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO artifacts ({COLS}) SELECT ?,?,?,?,?,?,?,?,\
             (SELECT COUNT(*) + 1 FROM artifacts WHERE session = ? AND name = ?),\
             ?,NULL,NULL,NULL,? \
             WHERE ? IS NULL OR (SELECT call_id FROM artifacts WHERE session = ? AND name = ? \
             ORDER BY version DESC LIMIT 1) = ? \
             RETURNING version"
        )))
        .bind(&artifact.call_id)
        .bind(&artifact.id)
        .bind(&session)
        .bind(&artifact.name)
        .bind(&artifact.title)
        .bind(artifact.kind.as_str())
        .bind(&artifact.mime)
        .bind(i64::try_from(artifact.bytes).unwrap_or(i64::MAX))
        .bind(&session)
        .bind(&artifact.name)
        .bind(source_file)
        .bind(artifact.created_at)
        .bind(&m.expected_base)
        .bind(&session)
        .bind(&artifact.name)
        .bind(&m.expected_base)
        .fetch_optional(&self.pool)
        .await;
        let inserted = match inserted {
            Ok(v) => v,
            // Two retries of one call raced past the pre-check: the row that
            // won is this record if it matches, otherwise a different one.
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation()) =>
            {
                return self.recorded(&artifact).await.unwrap_or_else(|| {
                    Err(CommitError::Other(format!(
                        "{}: {} already records a different artifact",
                        artifact.name, artifact.call_id
                    )))
                });
            }
            Err(e) => {
                return Err(CommitError::Other(format!("{}: index: {e}", artifact.name)));
            }
        };
        let Some((version,)) = inserted else {
            // Zero rows also when an identical retry raced past the pre-check
            // and won: its insert moved `latest` past `expected_base`, so this
            // statement's predicate is false without a unique violation. The
            // record exists exactly as asked; only a different one is a conflict.
            if let Some(recorded) = self.recorded(&artifact).await {
                return recorded;
            }
            let latest: Option<(String,)> = sqlx::query_as(
                "SELECT call_id FROM artifacts WHERE session = ? AND name = ? \
                 ORDER BY version DESC LIMIT 1",
            )
            .bind(&session)
            .bind(&artifact.name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CommitError::Other(format!("{}: index: {e}", artifact.name)))?;
            return Err(CommitError::Conflict {
                latest: latest.map(|(c,)| c),
            });
        };
        artifact.version = u32::try_from(version).unwrap_or_default();
        self.search.invalidate();
        if artifact.kind == Kind::Markdown {
            self.bind_document(&artifact, &m.bytes).await;
        }
        drop(_writing);

        tracing::info!(
            target: "artifact",
            kind = artifact.kind.as_str(),
            bytes = artifact.bytes,
            version = artifact.version,
            ok = true,
            "commit"
        );
        self.rt
            .publish(ArtifactShown {
                session: artifact.session,
                call_id: artifact.call_id.clone(),
                artifact: artifact.clone(),
            })
            .await;
        Ok(artifact)
    }

    /// The row of tool call `call_id`, live session or not: what [`Commit`]
    /// and the app plugin ([`crate::GetRecord`]) need; [`State::get`] adds
    /// the liveness check.
    pub async fn record(&self, call_id: &str) -> Result<Artifact, String> {
        // Only `COLS` (a const) is interpolated; the id is bound.
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM artifacts WHERE call_id = ?"
        )))
        .bind(call_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("index: {e}"))?;
        let row = row.ok_or_else(|| format!("{call_id}: no such artifact"))?;
        row_to_artifact(&self.dir, row).map_err(|e| format!("{e:#}"))
    }

    /// Whether this store's markdown records are documents (59 §7.2): the
    /// durable one only — a private tree's records die with the process while
    /// documents persist.
    fn binds_documents(&self) -> bool {
        self.owner_lock.is_some()
    }

    /// 59 §7.2: the shown markdown becomes the bound document's text — created
    /// on the first show (`Ensure`, mode `replace`), a block diff after — then a
    /// `artifact-v<N>` snapshot. Stamped `Principal::File`, so the import never
    /// exports (the import *is* this version; an export would loop back as an
    /// edit version). Failures are logged: the record stands without a document.
    async fn bind_document(&self, a: &Artifact, bytes: &[u8]) {
        if !self.binds_documents() {
            return;
        }
        let Some(docs) = document::Handle::lookup() else {
            return;
        };
        let Ok(markdown) = std::str::from_utf8(bytes) else {
            return;
        };
        let binding = document::Binding::Artifact {
            session: a.session.to_string(),
            name: a.name.clone(),
        };
        let doc = match docs.actor().ask(document::Lookup { binding: binding.clone() }).await {
            Ok(Some(doc)) => docs
                .import_markdown(doc.clone(), markdown.to_owned(), document::Principal::File)
                .await
                .map(|_| doc),
            Ok(None) => docs
                .actor()
                .ask(document::Ensure {
                    binding,
                    markdown: markdown.to_owned(),
                })
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match doc {
            Ok(doc) => {
                if let Err(e) = docs
                    .actor()
                    .ask(document::Snapshot {
                        doc,
                        label: format!("artifact-v{}", a.version),
                    })
                    .await
                {
                    tracing::warn!(name = %a.name, "artifact document snapshot: {e}");
                }
            }
            Err(e) => tracing::warn!(name = %a.name, "artifact document: {e}"),
        }
    }

    /// The newest version of `(session, name)`, if any.
    async fn latest(&self, session: SessionId, name: &str) -> Result<Option<Artifact>, String> {
        // Only `COLS` (a const) is interpolated; the keys are bound.
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM artifacts WHERE session = ? AND name = ? ORDER BY version DESC LIMIT 1"
        )))
        .bind(session.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("index: {e}"))?;
        row.map(|r| row_to_artifact(&self.dir, r).map_err(|e| format!("{e:#}")))
            .transpose()
    }

    /// 59 §7.2: a settled user edit of a markdown artifact's document — `bytes`
    /// is its export — becomes the next version: a new content-addressed blob
    /// and a row with `call_id = "edit:<ms>-<sha16>"` (the column is the primary
    /// key, so a bare `"edit"` cannot repeat), `origin = None`, the latest
    /// version's title unless the text names its own. `Ok(None)` when the text
    /// is byte-identical to the latest version (nothing to record) or the
    /// lineage is not markdown. Publishes [`ArtifactShown`] like a show.
    pub async fn edit_version(
        &self,
        session: SessionId,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<Option<Artifact>, String> {
        let Some(latest) = self.latest(session, name).await? else {
            return Err(format!("{name}: no version to edit"));
        };
        if latest.kind != Kind::Markdown {
            return Ok(None);
        }
        let id = sha16(&bytes);
        if id == latest.id {
            return Ok(None);
        }
        let _writing = self.gate.read().await;
        self.admit().map_err(|e| format!("{name}: {e}"))?;
        let path = self
            .put_blob(&blob_name(&id, name), &bytes)
            .await
            .map_err(|e| format!("{name}: {e:#}"))?;
        let title = std::str::from_utf8(&bytes)
            .ok()
            .and_then(doc_title)
            .unwrap_or_else(|| latest.title.clone());
        let created_at = chrono::Utc::now().timestamp();
        // ponytail: ms + content hash; two artifacts exporting identical text in one ms collide → logged, no row.
        let mut artifact = Artifact {
            call_id: format!("edit:{}-{id}", chrono::Utc::now().timestamp_millis()),
            mime: kind::mime(Kind::Markdown, name),
            id,
            session,
            name: name.to_owned(),
            title,
            kind: Kind::Markdown,
            bytes: bytes.len() as u64,
            version: 0,
            path,
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at,
        };
        let (version,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO artifacts ({COLS}) VALUES (?,?,?,?,?,?,?,?,\
             (SELECT COUNT(*) + 1 FROM artifacts WHERE session = ? AND name = ?),\
             NULL,NULL,NULL,NULL,?) RETURNING version"
        )))
        .bind(&artifact.call_id)
        .bind(&artifact.id)
        .bind(session.to_string())
        .bind(name)
        .bind(&artifact.title)
        .bind(artifact.kind.as_str())
        .bind(&artifact.mime)
        .bind(i64::try_from(artifact.bytes).unwrap_or(i64::MAX))
        .bind(session.to_string())
        .bind(name)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("{name}: index: {e}"))?;
        artifact.version = u32::try_from(version).unwrap_or_default();
        self.search.invalidate();
        drop(_writing);
        tracing::info!(target: "artifact", kind = "markdown", bytes = artifact.bytes, version = artifact.version, ok = true, "edit");
        self.rt
            .publish(ArtifactShown {
                session,
                call_id: artifact.call_id.clone(),
                artifact: artifact.clone(),
            })
            .await;
        Ok(Some(artifact))
    }

    /// What a [`Commit`] of `wanted.call_id` must answer when that call is
    /// already recorded: the existing record when it is exactly this one
    /// ([`same_record`]), a refusal otherwise; `None` when it is not recorded.
    async fn recorded(&self, wanted: &Artifact) -> Option<Result<Artifact, CommitError>> {
        let existing = self.record(&wanted.call_id).await.ok()?;
        Some(if same_record(&existing, wanted) {
            Ok(existing)
        } else {
            Err(CommitError::Other(format!(
                "{}: {} already records a different artifact",
                wanted.name, wanted.call_id
            )))
        })
    }

    /// 46 §6.4 [`ClaimInvocation`]: one `INSERT … SELECT … WHERE EXISTS
    /// (app record of this session) ON CONFLICT DO NOTHING`, so a claim
    /// cannot outlive its record's deletion or target a non-app; when the id
    /// exists, the stored identity decides `Existing` vs `Conflict`.
    pub async fn claim(&self, m: ClaimInvocation) -> Result<Claim, String> {
        let hash = args_hash(&m.args);
        let session = m.session.to_string();
        let inserted: Option<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO app_invocations ({INV_COLS}, owner) \
             SELECT ?,?,?,?,?,?,?,'running',NULL,?,NULL,? \
             WHERE EXISTS (SELECT 1 FROM artifacts WHERE call_id = ? AND session = ? AND kind = 'app') \
             ON CONFLICT(request_id) DO NOTHING RETURNING request_id"
        )))
        .bind(&m.request_id)
        .bind(&session)
        .bind(&m.record_call_id)
        .bind(&m.statement_id)
        .bind(&m.action_id)
        .bind(&m.tool)
        .bind(&hash)
        .bind(chrono::Utc::now().timestamp())
        .bind(&self.owner)
        .bind(&m.record_call_id)
        .bind(&session)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("{}: index: {e}", m.request_id))?;
        if inserted.is_some() {
            return Ok(Claim::New);
        }
        let Some(existing) = self.invocation(&m.request_id).await? else {
            return Err(format!(
                "{}: {} is not an app record of this session",
                m.request_id, m.record_call_id
            ));
        };
        let same = existing.session == m.session
            && existing.record_call_id == m.record_call_id
            && existing.statement_id == m.statement_id
            && existing.tool == m.tool
            && existing.args_hash == hash;
        Ok(if same {
            Claim::Existing(Box::new(existing))
        } else {
            Claim::Conflict
        })
    }

    /// 46 §6.4 [`CompleteInvocation`]: `running` → `succeeded` / `failed`
    /// with a bounded stored result.
    pub async fn complete(&self, m: CompleteInvocation) -> Result<(), String> {
        let status = if m.ok {
            InvocationStatus::Succeeded
        } else {
            InvocationStatus::Failed
        };
        self.finish(&m.request_id, status, m.result).await
    }

    /// [`MarkInvocationUnknown`]: `running` → `unknown`, the reason kept as
    /// `{"unknown": reason}`. Never `failed`: the tool may have run.
    pub async fn mark_unknown(&self, m: MarkInvocationUnknown) -> Result<(), String> {
        let result = m.reason.map(|r| serde_json::json!({"unknown": r}));
        self.finish(&m.request_id, InvocationStatus::Unknown, result)
            .await
    }

    /// One terminal transition out of `running`; the result is serialized and
    /// bounded to [`MAX_INVOCATION_RESULT`].
    async fn finish(
        &self,
        request_id: &str,
        status: InvocationStatus,
        result: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let result = result.map(|v| {
            let s = v.to_string();
            if s.len() > MAX_INVOCATION_RESULT {
                serde_json::json!({
                    "omitted": format!("result of {} exceeds {}", kind::human(s.len() as u64), kind::human(MAX_INVOCATION_RESULT as u64)),
                })
                .to_string()
            } else {
                s
            }
        });
        let done = sqlx::query(
            "UPDATE app_invocations SET status = ?, result = ?, finished_at = ? \
             WHERE request_id = ? AND status = 'running'",
        )
        .bind(status.as_str())
        .bind(result)
        .bind(chrono::Utc::now().timestamp())
        .bind(request_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("{request_id}: index: {e}"))?;
        if done.rows_affected() == 0 {
            return Err(format!("{request_id}: not a running invocation"));
        }
        Ok(())
    }

    /// Current row of `request_id`: `Ok(None)` when there is none, `Err` when
    /// the index is unreadable or the row is corrupt, so a status lookup never
    /// mistakes an unreadable outcome for a missing one.
    pub async fn invocation(&self, request_id: &str) -> Result<Option<Invocation>, String> {
        let row: Option<InvRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {INV_COLS} FROM app_invocations WHERE request_id = ?"
        )))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("{request_id}: index: {e}"))?;
        row.map(invocation_from_row).transpose()
    }

    /// [`crate::ListInvocations`]: the rows of one app record, newest first,
    /// at most `min(limit, MAX_LIST_INVOCATIONS)`.
    // ponytail: filtered by a table scan; add an index on (record_call_id,
    // started_at) if one data dir ever holds many thousands of presses.
    pub async fn list_invocations(
        &self,
        record_call_id: &str,
        limit: usize,
    ) -> Result<Vec<Invocation>, String> {
        let rows: Vec<InvRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {INV_COLS} FROM app_invocations WHERE record_call_id = ? \
             ORDER BY started_at DESC, rowid DESC LIMIT ?"
        )))
        .bind(record_call_id)
        .bind(limit.min(MAX_LIST_INVOCATIONS) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("{record_call_id}: index: {e}"))?;
        rows.into_iter().map(invocation_from_row).collect()
    }

    /// Artifacts newest first. `Some(session)` → that session's rows;
    /// `descendants` adds every session under it, asked from the agent
    /// supervisor ([`agent::ListAgents`]); when the agent is absent only the
    /// session's own rows are returned. `None` → every session (the
    /// explorer); `descendants` is moot.
    pub async fn list(
        &self,
        session: Option<SessionId>,
        descendants: bool,
    ) -> anyhow::Result<Vec<Artifact>> {
        let Some(session) = session else {
            return select(&self.pool, &self.dir, None).await;
        };
        let mut sessions = vec![session];
        if descendants
            && let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(tree) = agents.ask(agent::ListAgents { session }).await
        {
            sessions.extend(tree.into_iter().map(|a| a.id));
        }
        select(&self.pool, &self.dir, Some(&sessions)).await
    }

    /// The record `call_id` names, or `Err` when there is none or its owning
    /// session is gone (asked of the agent supervisor, never `sessions.sqlite`;
    /// design doc 49 §4.3). Orphan rows left by a deletion while this plugin
    /// was disabled are therefore invisible.
    pub async fn get(&self, call_id: &str) -> Result<Artifact, String> {
        let artifact = self.record(call_id).await?;
        if !search::alive_sessions().await?.contains(&artifact.session) {
            return Err(format!("{call_id}: owning session no longer exists"));
        }
        Ok(artifact)
    }

    /// Every retained occurrence whose owning session still exists, plus
    /// nothing else: the search index's source (design doc 49 §3.2).
    fn fetch(&self) -> impl Fn() -> search::Fetch + Send + 'static {
        let pool = self.pool.clone();
        let dir = self.dir.clone();
        move || -> search::Fetch {
            let pool = pool.clone();
            let dir = dir.clone();
            Box::pin(async move {
                let rows = select(&pool, &dir, None)
                    .await
                    .map_err(|e| format!("artifacts unreadable: {e:#}"))?;
                let alive = search::alive_sessions().await?;
                let rows: Vec<Artifact> = rows
                    .into_iter()
                    .filter(|a| alive.contains(&a.session))
                    .collect();
                Ok(rows)
            })
        }
    }

    /// [`crate::search::SearchArtifacts`]: every occurrence, best field per
    /// record, `indexing` while the first index builds.
    pub async fn search_artifacts(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> Result<::search::SearchPage<search::ArtifactSearchHit>, String> {
        ::search::validate_query(&query)?;
        let limit = ::search::limit(limit);
        if query.trim().is_empty() {
            return Ok(::search::SearchPage::ready(Vec::new(), limit));
        }
        let index = match self.search.index(self.fetch()) {
            None => return Ok(::search::SearchPage::indexing()),
            Some(Err(e)) => return Ok(::search::SearchPage::error(e)),
            Some(Ok(index)) => index,
        };
        Ok(self
            .search
            .matched(move || index.search(&query, limit))
            .await
            .unwrap_or_else(::search::SearchPage::error))
    }

    /// [`crate::search::ReadSearchMatch`]: the winning field of `call_id`
    /// for `query` with up to [`search::CONTEXT_CHARS`] scalars of context,
    /// from the same index a search used. Re-resolves the record first, so a
    /// vanished artifact or dead session is an error, never a sibling version.
    pub async fn read_search_match(
        &self,
        call_id: String,
        query: String,
    ) -> Result<search::ArtifactMatchContext, String> {
        ::search::validate_query(&query)?;
        if query.trim().is_empty() {
            return Err("empty query".into());
        }
        self.get(&call_id).await?;
        let index = self
            .search
            .index(self.fetch())
            .ok_or("artifacts are still indexing; retry shortly")??;
        self.search
            .matched(move || index.read_match(&call_id, &query))
            .await?
    }

    /// [`crate::search::RefreshSearch`]: rebuild on the next query and retry
    /// every failed extraction; successful text stays cached.
    pub fn refresh_search(&self) {
        self.search.refresh();
    }

    /// Copy the blob of `id` to `dest` (parent created). `Err` when `dest`
    /// exists or `id` is unknown.
    pub async fn export(&self, id: &str, dest: &Path) -> Result<PathBuf, String> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT name FROM artifacts WHERE id = ? LIMIT 1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| format!("index: {e}"))?;
        let (name,) = row.ok_or_else(|| format!("{id}: no such artifact"))?;
        if tokio::fs::try_exists(dest).await.unwrap_or(false) {
            return Err(format!("{}: exists", dest.display()));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        tokio::fs::copy(self.dir.join(blob_name(id, &name)), dest)
            .await
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(dest.to_path_buf())
    }

    /// Drop every row of `session`, artifacts and app invocations alike, in
    /// one transaction; the next sweep collects the blobs. The index is
    /// invalidated even when the delete fails: its builds filter by live
    /// sessions, so the dead session's rows vanish from search anyway.
    pub async fn delete_session(&self, session: SessionId) -> anyhow::Result<()> {
        let session = session.to_string();
        let deleted = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("DELETE FROM artifacts WHERE session = ?")
                .bind(&session)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM app_invocations WHERE session = ?")
                .bind(&session)
                .execute(&mut *tx)
                .await?;
            tx.commit().await
        }
        .await;
        self.search.invalidate();
        deleted?;
        Ok(())
    }

    /// Delete blobs (and previews, sources) no row references. Returns how
    /// many. Exclusive against `show`/`commit`, whose blobs precede their rows.
    pub async fn sweep(&self) -> anyhow::Result<usize> {
        let _exclusive = self.gate.write().await;
        let ids: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT id FROM artifacts")
            .fetch_all(&self.pool)
            .await?;
        let sources: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT source FROM artifacts WHERE source IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        let live: HashSet<String> = ids
            .into_iter()
            .map(|(id,)| id)
            .chain(sources.into_iter().map(|(s,)| stem(&s).to_owned()))
            .collect();
        let mut removed = 0;
        let mut rd = tokio::fs::read_dir(&self.dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if file.starts_with(DB) || !entry.file_type().await?.is_file() {
                continue;
            }
            // No writer runs under the exclusive gate: any `.part` is a crash leftover.
            if file.ends_with(".part") || !live.contains(stem(&file)) {
                tokio::fs::remove_file(entry.path()).await?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Which [`State`] a session's rows and blobs live in (51 §4.5): the durable
/// one for every session except the members of an open private tree, which
/// share their root's in-memory state under its scratch dir. Populated by
/// `TreeOpened`, emptied by `CloseTree`; the actor and the `show` tool hold
/// it together.
pub struct Stores {
    durable: Arc<State>,
    trees: parking_lot::RwLock<Trees>,
}

#[derive(Default)]
struct Trees {
    /// Member → its root's state.
    members: HashMap<SessionId, Arc<State>>,
    /// Root → its state and members, for the close.
    roots: HashMap<SessionId, (Arc<State>, Vec<SessionId>)>,
    /// Members of closed trees: a straggler is refused, never routed to the
    /// durable store.
    closed: HashSet<SessionId>,
}

impl Stores {
    /// Wrap the durable store; private trees register through `TreeOpened`.
    pub fn new(durable: Arc<State>) -> Self {
        Self {
            durable,
            trees: parking_lot::RwLock::default(),
        }
    }

    /// The durable store: the explorer's `List`, search, sweep.
    pub fn durable(&self) -> &Arc<State> {
        &self.durable
    }

    /// The store `session` reads and writes. `Err` for a member of a closed
    /// private tree.
    pub fn state_for(&self, session: SessionId) -> Result<Arc<State>, String> {
        let t = self.trees.read();
        if let Some(s) = t.members.get(&session) {
            return Ok(s.clone());
        }
        if t.closed.contains(&session) {
            return Err(format!("{session}: private session is closed"));
        }
        Ok(self.durable.clone())
    }

    /// The private store of `session`, or `None` when it is not a member of
    /// an open private tree: what a caller whose resources say private must
    /// use, so a tree that has not been announced yet never falls through to
    /// the durable store.
    pub fn private_for(&self, session: SessionId) -> Option<Arc<State>> {
        self.trees.read().members.get(&session).cloned()
    }

    /// Every store, durable first: id-keyed lookups (a record by call id, an
    /// invocation by request id) carry no session and try each.
    pub fn all(&self) -> Vec<Arc<State>> {
        let t = self.trees.read();
        std::iter::once(self.durable.clone())
            .chain(t.roots.values().map(|(s, _)| s.clone()))
            .collect()
    }

    /// `TreeOpened`: open the root's private store on first sight and map
    /// every listed session to it. Durable resources are a no-op. Idempotent:
    /// a repeat adds the new members.
    pub async fn open_tree(
        &self,
        root: SessionId,
        sessions: &[SessionId],
        resources: &SessionResources,
    ) -> anyhow::Result<()> {
        if !resources.is_private() {
            return Ok(());
        }
        let existing = self.trees.read().roots.get(&root).map(|(s, _)| s.clone());
        let state = match existing {
            Some(s) => s,
            None => Arc::new(
                State::open_private(
                    &resources.root,
                    self.durable.rt.clone(),
                    self.durable.fonts.clone(),
                )
                .await?,
            ),
        };
        let mut t = self.trees.write();
        let Trees { members, roots, .. } = &mut *t;
        let (state, known) = roots.entry(root).or_insert((state, Vec::new()));
        for &s in std::iter::once(&root).chain(sessions) {
            if !known.contains(&s) {
                known.push(s);
            }
            members.insert(s, state.clone());
        }
        Ok(())
    }

    /// `CloseTree`: for every root in `subtree`, unmap its members (they are
    /// refused from now on), then [`State::close`] its store. Ids that are
    /// not private roots are left alone: a deleted child's rows go with
    /// `SessionDeleted`.
    pub async fn close_tree(&self, subtree: &[SessionId]) -> Result<(), String> {
        let states: Vec<Arc<State>> = {
            let mut t = self.trees.write();
            let mut states = Vec::new();
            for root in subtree {
                let Some((state, known)) = t.roots.remove(root) else {
                    continue;
                };
                state.closing.store(true, Ordering::Release);
                for m in known {
                    t.members.remove(&m);
                    t.closed.insert(m);
                }
                states.push(state);
            }
            states
        };
        let mut errors = Vec::new();
        for state in states {
            if let Err(e) = state.close().await {
                errors.push(format!("{e:#}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn stem(file: &str) -> &str {
    file.split('.').next().unwrap_or(file)
}

/// Whether the existing record of a `call_id` is exactly the one being
/// recommitted: same bytes, session, name, kind, title and source sidecar
/// (its own hash, so different binding metadata is a different record).
fn same_record(existing: &Artifact, wanted: &Artifact) -> bool {
    existing.id == wanted.id
        && existing.session == wanted.session
        && existing.name == wanted.name
        && existing.kind == wanted.kind
        && existing.title == wanted.title
        && existing.source == wanted.source
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (State, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let state = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        (state, dir)
    }

    fn content(name: &str, text: &str) -> ShowArgs {
        ShowArgs {
            path: None,
            content: Some(text.to_owned()),
            name: Some(name.to_owned()),
            title: None,
            source: None,
        }
    }

    fn by_path(path: PathBuf) -> ShowArgs {
        ShowArgs {
            path: Some(path),
            content: None,
            name: None,
            title: None,
            source: None,
        }
    }

    fn blobs(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with(DB))
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn same_bytes_twice_one_blob_two_versions() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let a = s
            .show(sess, "c1".into(), content("notes.md", "# Hello\n\nbody\n"))
            .await
            .unwrap();
        let b = s
            .show(sess, "c2".into(), content("notes.md", "# Hello\n\nbody\n"))
            .await
            .unwrap();
        assert_eq!(a.artifact.id, b.artifact.id);
        assert_eq!((a.artifact.version, b.artifact.version), (1, 2));
        assert_eq!(a.artifact.title, "Hello");
        assert_eq!(a.artifact.kind, Kind::Markdown);
        assert_eq!(a.receipt.lines, Some(3));
        assert_eq!(blobs(&s.dir), [format!("{}.md", a.artifact.id)]);
        let list = s.list(Some(sess), false).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].call_id, "c2");
        assert_eq!(list[0].path, a.artifact.path);
    }

    #[test]
    fn title_prefers_front_matter_then_heading() {
        assert_eq!(
            doc_title("---\ntitle: \"Quarterly summary\"\ntoc: true\n---\n# Executive Overview\n")
                .as_deref(),
            Some("Quarterly summary")
        );
        assert_eq!(
            doc_title("---\ntoc: true\n---\n# Overview\n").as_deref(),
            Some("Overview")
        );
        assert_eq!(doc_title("plain text\n"), None);
    }

    /// Two `show`s of one name in one assistant message run concurrently (the
    /// actor answers `Show` off its mailbox); the version must still be 1 then 2.
    #[tokio::test]
    async fn parallel_shows_of_one_name_get_distinct_versions() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let (a, b) = tokio::join!(
            s.show(sess, "c1".into(), content("notes.md", "one\n")),
            s.show(sess, "c2".into(), content("notes.md", "two\n")),
        );
        let mut versions = [a.unwrap().artifact.version, b.unwrap().artifact.version];
        versions.sort_unstable();
        assert_eq!(versions, [1, 2]);
    }

    #[tokio::test]
    async fn validation_errors() {
        let (s, d) = open().await;
        let sess = SessionId::new();
        let err = s
            .show(sess, "c".into(), by_path(d.path().to_path_buf()))
            .await
            .unwrap_err();
        assert!(err.ends_with("not a regular file"), "{err}");
        let err = s
            .show(sess, "c".into(), by_path(d.path().join("nope.txt")))
            .await
            .unwrap_err();
        assert!(
            err.starts_with(&format!("{}: ", d.path().join("nope.txt").display())),
            "{err}"
        );

        let big = d.path().join("big.mov");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(33 * 1024 * 1024)
            .unwrap();
        let err = s
            .show(sess, "c".into(), by_path(big.clone()))
            .await
            .unwrap_err();
        assert_eq!(err, format!("{}: 33 MB exceeds 32 MB", big.display()));

        let mut args = content("x.txt", "hi");
        args.name = None;
        assert_eq!(
            s.show(sess, "c".into(), args).await.unwrap_err(),
            "content requires name"
        );
        assert!(s.list(Some(sess), false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sweep_and_delete_session() {
        let (s, d) = open().await;
        let sess = SessionId::new();
        let kept = s
            .show(sess, "c1".into(), content("data.csv", "a,b\n1,2\n3,4\n"))
            .await
            .unwrap();
        assert_eq!(kept.receipt.table, Some((2, 2)));
        std::fs::write(s.dir.join("deadbeefdeadbeef.txt"), b"orphan").unwrap();
        assert_eq!(s.sweep().await.unwrap(), 1);
        assert_eq!(blobs(&s.dir), [format!("{}.csv", kept.artifact.id)]);

        let file = d.path().join("shot.png");
        std::fs::write(&file, b"not really png").unwrap();
        let shot = s
            .show(sess, "c2".into(), by_path(file.clone()))
            .await
            .unwrap();
        assert_eq!(shot.artifact.origin.as_deref(), Some(file.as_path()));
        assert_eq!(shot.artifact.kind, Kind::Image);

        let out = d.path().join("out").join("data.csv");
        assert_eq!(s.export(&kept.artifact.id, &out).await.unwrap(), out);
        assert_eq!(std::fs::read(&out).unwrap(), b"a,b\n1,2\n3,4\n");
        assert_eq!(
            s.export(&kept.artifact.id, &out).await.unwrap_err(),
            format!("{}: exists", out.display())
        );

        s.delete_session(sess).await.unwrap();
        assert!(s.list(Some(sess), false).await.unwrap().is_empty());
        assert_eq!(s.sweep().await.unwrap(), 2);
        assert!(blobs(&s.dir).is_empty());
    }

    fn app(sess: SessionId, call_id: &str, src: &str, base: Option<&str>) -> Commit {
        Commit {
            session: sess,
            call_id: call_id.into(),
            name: "panel.openui".into(),
            title: "Panel".into(),
            kind: Kind::App,
            bytes: src.as_bytes().to_vec(),
            source: Some(("app.json".into(), br#"{"library_hash":"h1"}"#.to_vec())),
            expected_base: base.map(str::to_owned),
        }
    }

    /// Ordinary `show` of a `.openui` file is inert text: only `Commit` makes
    /// an active app.
    #[tokio::test]
    async fn show_openui_is_inert_text() {
        let (s, _d) = open().await;
        let shown = s
            .show(
                SessionId::new(),
                "c1".into(),
                content("panel.openui", "root = TextContent(\"hi\")\n"),
            )
            .await
            .unwrap();
        assert_eq!(shown.artifact.kind, Kind::Text);
        assert_eq!(shown.artifact.mime, "text/plain");
        assert_eq!(shown.receipt.lines, Some(1));
    }

    /// A shown archive gets a `<blob>.listing.json` sidecar the sweep keeps;
    /// a corrupt one is still an `Archive` record, just without a listing.
    #[tokio::test]
    async fn show_archive_writes_listing_sidecar() {
        let (s, d) = open().await;
        let sess = SessionId::new();
        let file = d.path().join("src.tar.gz");
        std::fs::write(
            &file,
            crate::archive::tests::tar_gz_fixture(&[("src/main.rs", b"fn main() {}")]),
        )
        .unwrap();
        let shown = s.show(sess, "c1".into(), by_path(file)).await.unwrap();
        assert_eq!(shown.artifact.kind, Kind::Archive);
        assert_eq!(shown.artifact.mime, "application/gzip");
        assert_eq!(shown.receipt.files, Some(1));
        assert_eq!(
            kind::receipt(&shown.artifact, &shown.receipt),
            format!("shown src.tar.gz (v1 · 1 files · {})", kind::human(shown.artifact.bytes))
        );
        let listing = archive::read_listing(&shown.artifact.path).await.unwrap();
        assert_eq!(listing.entries[0].path, "src/main.rs");
        assert_eq!(s.sweep().await.unwrap(), 0);
        assert!(archive::sidecar(&shown.artifact.path).exists());

        let bad = s
            .show(sess, "c2".into(), content("bad.zip", "not a zip"))
            .await
            .unwrap();
        assert_eq!(bad.artifact.kind, Kind::Archive);
        assert_eq!(bad.receipt.files, None);
        assert!(archive::read_listing(&bad.artifact.path).await.is_err());
    }

    /// One blob, one metadata sidecar, two records with their own sessions.
    #[tokio::test]
    async fn commit_same_bytes_two_sessions_one_blob_two_records() {
        let (s, _d) = open().await;
        let (s1, s2) = (SessionId::new(), SessionId::new());
        // Concurrent: both write the same blob and sidecar; each renames its
        // own temp file, neither clobbers the other.
        let (a, b) = tokio::join!(
            s.commit(app(s1, "c1", "root = X\n", None)),
            s.commit(app(s2, "c2", "root = X\n", None)),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.id, b.id);
        assert_eq!((a.version, b.version), (1, 1));
        assert_eq!((a.session, b.session), (s1, s2));
        assert_eq!(a.kind, Kind::App);
        assert_eq!(a.mime, "text/x-openui");
        assert_eq!(a.path.extension().unwrap(), "openui");
        let src = a.source.clone().unwrap();
        assert!(
            src.to_string_lossy().ends_with(".app.json"),
            "{}",
            src.display()
        );
        assert_ne!(
            src.file_name().unwrap().to_string_lossy(),
            format!("{}.app.json", a.id),
            "sidecar is keyed by its own hash, not the UI hash"
        );
        assert_eq!(std::fs::read(&src).unwrap(), br#"{"library_hash":"h1"}"#);
        assert_eq!(blobs(&s.dir).len(), 2);
        assert_eq!(s.record("c2").await.unwrap().session, s2);
        assert_eq!(s.record("nope").await.unwrap_err(), "nope: no such artifact");

        // A record is immutable: the exact same commit is idempotent, any
        // other content/session under a recorded call_id is refused.
        let again = s.commit(app(s1, "c1", "root = X\n", None)).await.unwrap();
        assert_eq!((again.version, again.created_at), (a.version, a.created_at));
        assert_eq!(s.list(Some(s1), false).await.unwrap().len(), 1);
        assert_eq!(
            s.commit(app(s1, "c1", "root = Y\n", None))
                .await
                .unwrap_err(),
            CommitError::Other("panel.openui: c1 already records a different artifact".into())
        );
        assert!(s.commit(app(s2, "c1", "root = X\n", None)).await.is_err());
        // Same UI bytes, different binding metadata: a different record.
        let mut rebound = app(s1, "c1", "root = X\n", None);
        rebound.source = Some(("app.json".into(), br#"{"library_hash":"h9"}"#.to_vec()));
        assert!(s.commit(rebound).await.is_err());
        let mut retitled = app(s1, "c1", "root = X\n", None);
        retitled.title = "Other".into();
        assert!(s.commit(retitled).await.is_err());
        // Ordinary `show` cannot overwrite a committed app under its call id.
        assert_eq!(
            s.show(s1, "c1".into(), content("panel.openui", "root = Z\n"))
                .await
                .unwrap_err(),
            "panel.openui: call c1 already recorded"
        );
        let kept = s.record("c1").await.unwrap();
        assert_eq!((kept.id.as_str(), kept.kind), (a.id.as_str(), Kind::App));
        assert_eq!(s.list(Some(s1), false).await.unwrap().len(), 1);
    }

    /// Two patches against one base race: exactly one lands as v2, the other
    /// gets a conflict naming the winner; a stale base afterwards also conflicts;
    /// unconditional replacement still appends.
    #[tokio::test]
    async fn commit_cas_race_loses_no_patch() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        s.commit(app(sess, "c1", "root = A\n", None)).await.unwrap();
        let (x, y) = tokio::join!(
            s.commit(app(sess, "c2", "root = B\n", Some("c1"))),
            s.commit(app(sess, "c3", "root = C\n", Some("c1"))),
        );
        let (won, lost) = match (x, y) {
            (Ok(w), Err(l)) | (Err(l), Ok(w)) => (w, l),
            other => panic!("expected one winner: {other:?}"),
        };
        assert_eq!(won.version, 2);
        assert_eq!(
            lost,
            CommitError::Conflict {
                latest: Some(won.call_id.clone())
            }
        );
        assert_eq!(
            s.commit(app(sess, "c4", "root = D\n", Some("c1")))
                .await
                .unwrap_err(),
            CommitError::Conflict {
                latest: Some(won.call_id.clone())
            }
        );
        assert_eq!(
            s.commit(app(sess, "c5", "root = E\n", Some("zzz")))
                .await
                .unwrap_err(),
            CommitError::Conflict {
                latest: Some(won.call_id.clone())
            }
        );
        assert_eq!(
            s.commit(app(SessionId::new(), "c6", "root = F\n", Some("c1")))
                .await
                .unwrap_err(),
            CommitError::Conflict { latest: None }
        );
        let full = s.commit(app(sess, "c7", "root = G\n", None)).await.unwrap();
        assert_eq!(full.version, 3);
        let list = s.list(Some(sess), false).await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].call_id, "c7");
    }

    /// Two identical retries of one patch race past the pre-check: the loser
    /// inserts zero rows (the winner moved `latest` past the base) yet the
    /// record it asked for exists exactly, so both answer that one record;
    /// a genuinely different patch against the same base still conflicts.
    #[tokio::test]
    async fn commit_identical_retries_share_one_record() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        s.commit(app(sess, "c1", "root = A\n", None)).await.unwrap();
        let (x, y) = tokio::join!(
            s.commit(app(sess, "c2", "root = B\n", Some("c1"))),
            s.commit(app(sess, "c2", "root = B\n", Some("c1"))),
        );
        let (x, y) = (x.unwrap(), y.unwrap());
        assert_eq!((x.call_id.as_str(), x.version), ("c2", 2));
        assert_eq!((x.id, x.created_at), (y.id, y.created_at));
        assert_eq!(s.list(Some(sess), false).await.unwrap().len(), 2);
        assert_eq!(
            s.commit(app(sess, "c3", "root = C\n", Some("c1")))
                .await
                .unwrap_err(),
            CommitError::Conflict {
                latest: Some("c2".into())
            }
        );
        // Same call id, other content, stale base: refused as a record clash,
        // never reported as a version conflict.
        assert_eq!(
            s.commit(app(sess, "c2", "root = Z\n", Some("c1")))
                .await
                .unwrap_err(),
            CommitError::Other("panel.openui: c2 already records a different artifact".into())
        );
    }

    /// A second opener of one data dir (another harness, a `t --acp`) must
    /// not void the first one's in-flight calls: only rows whose owner's lock
    /// is free become `unknown`; live owners still complete theirs.
    #[tokio::test]
    async fn second_open_keeps_live_owner_invocations_running() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let a = State::open(dir.path(), rt.clone(), Vec::new())
            .await
            .unwrap();
        let sess = SessionId::new();
        let args = serde_json::json!({"name": "portal-web"});
        a.commit(app(sess, "rec", "root = X\n", None))
            .await
            .unwrap();
        assert_eq!(
            a.claim(press(sess, "r1", args.clone())).await.unwrap(),
            Claim::New
        );

        let b = State::open(dir.path(), rt.clone(), Vec::new())
            .await
            .unwrap();
        assert_eq!(
            b.invocation("r1").await.unwrap().unwrap().status,
            InvocationStatus::Running
        );
        // The other opener sees the same row, does not dispatch again.
        assert!(matches!(
            b.claim(press(sess, "r1", args.clone())).await.unwrap(),
            Claim::Existing(_)
        ));
        a.complete(CompleteInvocation {
            request_id: "r1".into(),
            ok: true,
            result: Some(serde_json::json!({"message": "synced"})),
        })
        .await
        .unwrap();
        assert_eq!(
            b.invocation("r1").await.unwrap().unwrap().status,
            InvocationStatus::Succeeded
        );

        // `a` dies with r2 running, `b` lives with r3 running: a third opener
        // recovers exactly r2.
        assert_eq!(
            a.claim(press(sess, "r2", args.clone())).await.unwrap(),
            Claim::New
        );
        assert_eq!(
            b.claim(press(sess, "r3", args.clone())).await.unwrap(),
            Claim::New
        );
        drop(a);
        let c = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        assert_eq!(
            c.invocation("r2").await.unwrap().unwrap().status,
            InvocationStatus::Unknown
        );
        assert_eq!(
            c.invocation("r3").await.unwrap().unwrap().status,
            InvocationStatus::Running
        );
        b.complete(CompleteInvocation {
            request_id: "r3".into(),
            ok: false,
            result: None,
        })
        .await
        .unwrap();
        assert_eq!(
            c.invocation("r3").await.unwrap().unwrap().status,
            InvocationStatus::Failed
        );
        // The dead owner's lock file is gone; the two live ones remain.
        let owners: Vec<_> = std::fs::read_dir(dir.path().join(OWNERS))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(owners.len(), 2, "{owners:?}");
    }

    #[tokio::test]
    async fn commit_validation() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let mut m = app(sess, "c1", "x", None);
        m.name = "noext".into();
        assert_eq!(
            s.commit(m).await.unwrap_err(),
            CommitError::Other("noext: name needs an extension".into())
        );
        let mut m = app(sess, "c1", "x", None);
        m.source = Some(("../evil".into(), Vec::new()));
        assert!(
            matches!(s.commit(m).await.unwrap_err(), CommitError::Other(e) if e.contains("suffix"))
        );
        let mut m = app(sess, "c1", "x", None);
        m.bytes = vec![0; 33 * 1024 * 1024];
        assert_eq!(
            s.commit(m).await.unwrap_err(),
            CommitError::Other("panel.openui: 33 MB exceeds 32 MB".into())
        );
        assert!(s.list(Some(sess), false).await.unwrap().is_empty());
        assert!(blobs(&s.dir).is_empty());
    }

    fn press(sess: SessionId, request_id: &str, args: serde_json::Value) -> ClaimInvocation {
        ClaimInvocation {
            request_id: request_id.into(),
            session: sess,
            record_call_id: "rec".into(),
            statement_id: "syncResult".into(),
            action_id: Some("act1".into()),
            tool: "argo_sync".into(),
            args,
        }
    }

    #[test]
    fn args_hash_is_canonical() {
        let a = args_hash(&serde_json::json!({"a": 1, "b": [1, {"y": 2, "x": 1}]}));
        let b = args_hash(&serde_json::json!({"b": [1, {"x": 1, "y": 2}], "a": 1}));
        assert_eq!(a, b);
        assert_ne!(
            a,
            args_hash(&serde_json::json!({"a": 2, "b": [1, {"x": 1, "y": 2}]}))
        );
    }

    /// Same id + same input → the stored row; same id + other input → conflict;
    /// completion is once; a running row survives a restart as `unknown`;
    /// session deletion purges.
    #[tokio::test]
    async fn invocation_claim_idempotent_conflict_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let s = State::open(dir.path(), rt.clone(), Vec::new())
            .await
            .unwrap();
        let sess = SessionId::new();
        let args = serde_json::json!({"name": "portal-web"});

        // No app record yet: nothing to bind to.
        assert!(
            s.claim(press(sess, "r0", args.clone()))
                .await
                .unwrap_err()
                .ends_with("rec is not an app record of this session")
        );
        s.show(sess, "rec".into(), content("panel.openui", "root = X\n"))
            .await
            .unwrap();
        // An inert shown `.openui` is not an app record either.
        assert!(s.claim(press(sess, "r0", args.clone())).await.is_err());
        s.delete_session(sess).await.unwrap();
        s.commit(app(sess, "rec", "root = X\n", None))
            .await
            .unwrap();
        // Another session cannot bind to this session's record.
        assert!(
            s.claim(press(SessionId::new(), "r0", args.clone()))
                .await
                .is_err()
        );

        assert_eq!(
            s.claim(press(sess, "r1", args.clone())).await.unwrap(),
            Claim::New
        );
        let Claim::Existing(row) = s.claim(press(sess, "r1", args.clone())).await.unwrap() else {
            panic!("repeat must return the stored row");
        };
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.args_hash, args_hash(&args));
        assert_eq!(row.tool, "argo_sync");
        assert_eq!(
            s.claim(press(sess, "r1", serde_json::json!({"name": "portal-api"})))
                .await
                .unwrap(),
            Claim::Conflict
        );
        assert_eq!(
            s.claim(press(SessionId::new(), "r1", args.clone()))
                .await
                .unwrap(),
            Claim::Conflict
        );

        s.complete(CompleteInvocation {
            request_id: "r1".into(),
            ok: true,
            result: Some(serde_json::json!({"message": "synced"})),
        })
        .await
        .unwrap();
        let done = s.invocation("r1").await.unwrap().unwrap();
        assert_eq!(done.status, InvocationStatus::Succeeded);
        assert_eq!(done.result, Some(serde_json::json!({"message": "synced"})));
        assert!(done.finished_at.is_some());
        assert!(
            s.complete(CompleteInvocation {
                request_id: "r1".into(),
                ok: false,
                result: None,
            })
            .await
            .unwrap_err()
            .ends_with("not a running invocation")
        );
        let Claim::Existing(again) = s.claim(press(sess, "r1", args.clone())).await.unwrap() else {
            panic!("completed row must be returned, not re-run");
        };
        assert_eq!(again.status, InvocationStatus::Succeeded);

        assert_eq!(
            s.claim(press(sess, "r2", args.clone())).await.unwrap(),
            Claim::New
        );
        s.complete(CompleteInvocation {
            request_id: "r2".into(),
            ok: false,
            result: Some(serde_json::Value::String("x".repeat(MAX_INVOCATION_RESULT))),
        })
        .await
        .unwrap();
        let big = s.invocation("r2").await.unwrap().unwrap();
        assert_eq!(big.status, InvocationStatus::Failed);
        assert!(big.result.unwrap().get("omitted").is_some());

        assert_eq!(
            s.claim(press(sess, "r3", args.clone())).await.unwrap(),
            Claim::New
        );
        drop(s);
        let s = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        assert_eq!(
            s.invocation("r3").await.unwrap().unwrap().status,
            InvocationStatus::Unknown
        );
        assert!(
            s.complete(CompleteInvocation {
                request_id: "r3".into(),
                ok: true,
                result: None,
            })
            .await
            .is_err()
        );
        assert_eq!(
            s.invocation("r1").await.unwrap().unwrap().status,
            InvocationStatus::Succeeded
        );

        s.delete_session(sess).await.unwrap();
        assert_eq!(s.invocation("r1").await.unwrap(), None);
        assert_eq!(s.invocation("r3").await.unwrap(), None);
        assert_eq!(s.invocation("nope").await.unwrap(), None);
        // The record is gone with its session: no reinsertion under its id.
        assert!(s.claim(press(sess, "r9", args.clone())).await.is_err());
    }

    /// An ambiguous dispatch is `unknown`, once, with its redacted reason;
    /// it is neither completable afterwards nor re-run on a repeat claim.
    #[tokio::test]
    async fn mark_unknown_is_terminal_and_never_failed() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let args = serde_json::json!({"name": "portal-web"});
        s.commit(app(sess, "rec", "root = X\n", None))
            .await
            .unwrap();
        assert!(
            s.mark_unknown(MarkInvocationUnknown {
                request_id: "r1".into(),
                reason: None,
            })
            .await
            .unwrap_err()
            .ends_with("not a running invocation")
        );
        s.claim(press(sess, "r1", args.clone())).await.unwrap();
        s.mark_unknown(MarkInvocationUnknown {
            request_id: "r1".into(),
            reason: Some("registry ask timed out".into()),
        })
        .await
        .unwrap();
        let row = s.invocation("r1").await.unwrap().unwrap();
        assert_eq!(row.status, InvocationStatus::Unknown);
        assert_eq!(
            row.result,
            Some(serde_json::json!({"unknown": "registry ask timed out"}))
        );
        assert!(row.finished_at.is_some());
        assert!(
            s.complete(CompleteInvocation {
                request_id: "r1".into(),
                ok: true,
                result: None,
            })
            .await
            .is_err()
        );
        assert!(
            s.mark_unknown(MarkInvocationUnknown {
                request_id: "r1".into(),
                reason: None,
            })
            .await
            .is_err()
        );
        let Claim::Existing(again) = s.claim(press(sess, "r1", args)).await.unwrap() else {
            panic!("unknown row must be returned, not re-run");
        };
        assert_eq!(again.status, InvocationStatus::Unknown);

        // An oversized reason is bounded like any result.
        s.claim(press(sess, "r2", serde_json::json!({})))
            .await
            .unwrap();
        s.mark_unknown(MarkInvocationUnknown {
            request_id: "r2".into(),
            reason: Some("x".repeat(MAX_INVOCATION_RESULT)),
        })
        .await
        .unwrap();
        let big = s.invocation("r2").await.unwrap().unwrap();
        assert_eq!(big.status, InvocationStatus::Unknown);
        assert!(big.result.unwrap().get("omitted").is_some());
    }

    /// Listing is per record, newest first, capped, and survives a restart
    /// (with the abandoned row recovered as `unknown`); it ends with the
    /// record's session.
    #[tokio::test]
    async fn list_invocations_per_record_newest_first_capped_durable() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let s = State::open(dir.path(), rt.clone(), Vec::new())
            .await
            .unwrap();
        let sess = SessionId::new();
        s.commit(app(sess, "rec", "root = X\n", None))
            .await
            .unwrap();
        s.commit(app(sess, "other", "root = Y\n", None))
            .await
            .unwrap();
        assert_eq!(s.list_invocations("rec", 10).await.unwrap(), vec![]);

        let n = MAX_LIST_INVOCATIONS + 3;
        for i in 0..n {
            let mut m = press(sess, &format!("r{i}"), serde_json::json!({"i": i}));
            if i % 2 == 1 {
                m.record_call_id = "other".into();
            }
            assert_eq!(s.claim(m).await.unwrap(), Claim::New);
        }
        // Even ids on `rec`, odd on `other`; finish the newest `rec` one.
        let newest = format!("r{}", n - 1);
        s.complete(CompleteInvocation {
            request_id: newest.clone(),
            ok: false,
            result: Some(serde_json::json!({"error": "denied"})),
        })
        .await
        .unwrap();

        let rec = s.list_invocations("rec", 3).await.unwrap();
        assert_eq!(rec.len(), 3);
        assert_eq!(rec[0].request_id, newest);
        assert_eq!(rec[0].status, InvocationStatus::Failed);
        assert_eq!(rec[1].request_id, format!("r{}", n - 3));
        assert_eq!(rec[2].request_id, format!("r{}", n - 5));
        assert!(rec.iter().all(|r| r.record_call_id == "rec"));

        // Cap: more rows than the cap exist per record only across both
        // records; ask for everything and get at most the cap, all `rec`.
        for i in n..n + MAX_LIST_INVOCATIONS {
            s.claim(press(sess, &format!("r{i}"), serde_json::json!({"i": i})))
                .await
                .unwrap();
        }
        let all = s.list_invocations("rec", usize::MAX).await.unwrap();
        assert_eq!(all.len(), MAX_LIST_INVOCATIONS);
        assert!(all.iter().all(|r| r.record_call_id == "rec"));
        assert_eq!(
            all[0].request_id,
            format!("r{}", n + MAX_LIST_INVOCATIONS - 1)
        );
        assert_eq!(s.list_invocations("rec", 0).await.unwrap(), vec![]);
        assert_eq!(s.list_invocations("nope", 5).await.unwrap(), vec![]);

        // Restart: the list is still there; still-running rows are `unknown`.
        drop(s);
        let s = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        let after = s.list_invocations("rec", 3).await.unwrap();
        assert_eq!(after.len(), 3);
        assert_eq!(after[0].status, InvocationStatus::Unknown);
        let other = s.list_invocations("other", 2).await.unwrap();
        assert_eq!(other.len(), 2);
        assert!(other.iter().all(|r| r.record_call_id == "other"));
        // `newest` is now beyond the cap (64 later `rec` rows exist): a
        // capped list must not carry it, and its `failed` status must have
        // survived the restart untouched by recovery.
        assert!(
            s.list_invocations("rec", MAX_LIST_INVOCATIONS)
                .await
                .unwrap()
                .iter()
                .all(|r| r.request_id != newest)
        );
        assert_eq!(
            s.invocation(&newest).await.unwrap().unwrap().status,
            InvocationStatus::Failed
        );

        s.delete_session(sess).await.unwrap();
        assert_eq!(s.list_invocations("rec", 5).await.unwrap(), vec![]);
    }

    /// The metadata sidecar is live while its row exists and swept with it.
    #[tokio::test]
    async fn source_metadata_sweep() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let a = s.commit(app(sess, "c1", "root = A\n", None)).await.unwrap();
        let mut m = app(sess, "c2", "root = A\n", Some("c1"));
        m.source = Some(("app.json".into(), br#"{"library_hash":"h2"}"#.to_vec()));
        let b = s.commit(m).await.unwrap();
        assert_eq!(a.id, b.id);
        assert_ne!(a.source, b.source, "metadata is keyed by its own hash");
        assert_eq!(blobs(&s.dir).len(), 3);
        assert_eq!(s.sweep().await.unwrap(), 0);
        s.delete_session(sess).await.unwrap();
        assert_eq!(s.sweep().await.unwrap(), 3);
        assert!(blobs(&s.dir).is_empty());
    }

    fn private(root: &Path) -> SessionResources {
        SessionResources {
            root: root.to_path_buf(),
            retention: tools::Retention::Private,
            generation: 1,
        }
    }

    /// A private member's records and blobs live under its root's scratch dir
    /// and in memory, never in the durable index or blob dir; the close
    /// removes them and refuses the members afterwards.
    #[tokio::test]
    async fn private_tree_routes_to_scratch_and_dies_with_close() {
        let (durable, _d) = open().await;
        let durable = Arc::new(durable);
        let stores = Stores::new(durable.clone());
        let scratch = tempfile::tempdir().unwrap();
        let (root, child) = (SessionId::new(), SessionId::new());
        stores
            .open_tree(root, &[root, child], &private(scratch.path()))
            .await
            .unwrap();
        let artifacts = scratch.path().join(DIR);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&artifacts).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let durable_blobs = blobs(&durable.dir);

        let state = stores.state_for(child).unwrap();
        assert!(Arc::ptr_eq(&state, &stores.state_for(root).unwrap()));
        let a = state
            .commit(app(child, "c1", "root = A\n", None))
            .await
            .unwrap();
        assert!(a.path.starts_with(&artifacts), "{}", a.path.display());
        assert!(a.path.is_file());
        assert_eq!(blobs(&durable.dir), durable_blobs);
        assert!(durable.record("c1").await.is_err());
        assert!(durable.list(Some(child), false).await.unwrap().is_empty());
        assert_eq!(state.list(Some(child), false).await.unwrap().len(), 1);
        assert_eq!(stores.all().len(), 2);

        stores.close_tree(&[root, child]).await.unwrap();
        assert!(!artifacts.exists());
        assert!(stores.state_for(child).is_err());
        assert!(stores.state_for(root).is_err());
        assert_eq!(stores.all().len(), 1);
        // A straggler holding the old handle writes nothing.
        assert!(state.commit(app(child, "c2", "root = B\n", None)).await.is_err());
        assert!(!artifacts.exists());
        // Closing again is clean; an unrelated id routes durable.
        stores.close_tree(&[root]).await.unwrap();
        assert!(Arc::ptr_eq(&stores.state_for(SessionId::new()).unwrap(), &durable));
    }

    /// A writer admitted before the close finishes first (the close waits on
    /// the gate); one that reaches the gate after the close began is refused.
    /// Either way no blob exists once the close returns.
    #[tokio::test]
    async fn close_waits_for_admitted_writers_and_refuses_late_ones() {
        let (durable, _d) = open().await;
        let stores = Arc::new(Stores::new(Arc::new(durable)));
        let scratch = tempfile::tempdir().unwrap();
        let root = SessionId::new();
        stores
            .open_tree(root, &[root], &private(scratch.path()))
            .await
            .unwrap();
        let state = stores.state_for(root).unwrap();
        let artifacts = scratch.path().join(DIR);

        // Stand in for a writer between its first blob write and its row.
        let admitted = state.gate.read().await;
        let closing = tokio::spawn({
            let stores = stores.clone();
            async move { stores.close_tree(&[root]).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!closing.is_finished(), "close must wait for the admitted writer");
        assert!(artifacts.exists());
        // Reaches the gate after the close began: refused, nothing written.
        let late = tokio::spawn({
            let state = state.clone();
            async move { state.commit(app(root, "late", "root = L\n", None)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!late.is_finished());
        drop(admitted);
        closing.await.unwrap().unwrap();
        assert!(late.await.unwrap().is_err());
        assert!(!artifacts.exists());
    }
}
