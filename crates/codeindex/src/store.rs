//! One derived SQLite per root (60 §4). `CREATE … IF NOT EXISTS`, no
//! migrations: a schema change bumps [`SCHEMA_VERSION`] and the file is
//! removed and rebuilt (10 §3, §11). Nothing here is truth; `rm` at will.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteQueryResult,
};
use sqlx::{FromRow, QueryBuilder};

use crate::parse::ParsedFile;

/// Bump on any DDL change; a mismatch deletes the file (60 §4).
/// 2: `imports.id`, `refs.qualifier` (step 2). 3: receiver-call confidence
/// and stoplist change stored `refs` rows (step 3). 4: `symbols.moniker`
/// and the `symbols_tri` trigram table (CI-SEARCH).
pub const SCHEMA_VERSION: &str = "4";

/// Rows per multi-row `INSERT` (≤ 12 binds each; SQLite allows 32 766).
const ROWS_PER_INSERT: usize = 500;
/// 60 §4, plus `imports.id` (the resolver updates rows by key) and
/// `refs.qualifier` (60 §20).
const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS files (
  id        INTEGER PRIMARY KEY,
  path      TEXT NOT NULL UNIQUE,
  mtime_ns  INTEGER NOT NULL,
  size      INTEGER NOT NULL,
  language  TEXT NOT NULL,
  package   TEXT,
  lines     INTEGER NOT NULL,
  parse_errors INTEGER NOT NULL DEFAULT 0,
  is_test   INTEGER NOT NULL DEFAULT 0,
  rank      REAL NOT NULL DEFAULT 0.0
)",
    "CREATE TABLE IF NOT EXISTS symbols (
  id        INTEGER PRIMARY KEY,
  file_id   INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name      TEXT NOT NULL,
  kind      TEXT NOT NULL,
  parent_id INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  line_start INTEGER NOT NULL, line_end INTEGER NOT NULL,
  byte_start INTEGER NOT NULL, byte_end   INTEGER NOT NULL,
  signature TEXT NOT NULL,
  docs      TEXT,
  exported  INTEGER NOT NULL DEFAULT 0,
  rank      REAL NOT NULL DEFAULT 0.0,
  moniker   TEXT NOT NULL
)",
    "CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id)",
    "CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name)",
    "CREATE INDEX IF NOT EXISTS symbols_moniker ON symbols(moniker)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
  name, signature, docs, path UNINDEXED, symbol_id UNINDEXED,
  tokenize = 'porter unicode61 remove_diacritics 2'
)",
    // Names only, for infix `code_find` (60 §8): FTS5's trigram tokenizer
    // answers `LIKE '%reg%istry%'` from the index (0.1 ms warm here vs 0.3 ms
    // for a hand-rolled postings table and 1 ms for a `symbols.name LIKE` scan).
    "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_tri USING fts5(name, tokenize = 'trigram')",
    "CREATE TABLE IF NOT EXISTS imports (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  specifier  TEXT NOT NULL,
  target_file INTEGER REFERENCES files(id) ON DELETE SET NULL,
  names      TEXT,
  line       INTEGER NOT NULL,
  provenance TEXT NOT NULL
)",
    "CREATE INDEX IF NOT EXISTS imports_file ON imports(file_id)",
    "CREATE INDEX IF NOT EXISTS imports_target ON imports(target_file)",
    "CREATE TABLE IF NOT EXISTS refs (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  from_symbol INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  line       INTEGER NOT NULL,
  qualifier  TEXT,
  to_symbol  INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  candidates INTEGER NOT NULL DEFAULT 0,
  provenance TEXT NOT NULL,
  confidence REAL NOT NULL
)",
    "CREATE INDEX IF NOT EXISTS refs_file ON refs(file_id)",
    "CREATE INDEX IF NOT EXISTS refs_to ON refs(to_symbol)",
    "CREATE INDEX IF NOT EXISTS refs_name ON refs(name)",
    "CREATE TABLE IF NOT EXISTS cochanges (
  file_a INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  file_b INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  count  INTEGER NOT NULL,
  PRIMARY KEY (file_a, file_b)
)",
];

/// Open (creating) the index for `root` at `path`; a schema mismatch or an
/// unusable file removes `path`, `-wal`, `-shm` and starts over.
pub async fn open(path: &Path, root: &Path) -> anyhow::Result<SqlitePool> {
    match open_db(path).await {
        Ok(pool) if schema_version(&pool).await.as_deref() == Some(SCHEMA_VERSION) => {
            return Ok(pool);
        }
        Ok(pool) => {
            if !schema_version(&pool).await.is_none_or(|v| v.is_empty()) {
                tracing::info!(path = %path.display(), "codeindex schema changed; rebuilding");
            }
            pool.close().await;
        }
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), path = %path.display(), "codeindex unusable; rebuilding");
        }
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let pool = open_db(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for (k, v) in [
        ("schema_version", SCHEMA_VERSION.to_owned()),
        ("root", root.to_string_lossy().into_owned()),
        ("created_at", now.to_string()),
    ] {
        set_meta(&pool, k, &v).await?;
    }
    Ok(pool)
}

async fn open_db(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await?;
    for sql in DDL {
        sqlx::query(*sql).execute(&pool).await?;
    }
    Ok(pool)
}

async fn schema_version(pool: &SqlitePool) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM meta WHERE key = 'schema_version'")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Upsert one `meta` row.
pub async fn set_meta(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// One `meta` value.
pub async fn meta(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

/// `files` as the reconcile diff reads it: path → (id, mtime_ns, size).
pub async fn known_files(pool: &SqlitePool) -> anyhow::Result<HashMap<String, (i64, i64, i64)>> {
    let rows: Vec<(i64, String, i64, i64)> =
        sqlx::query_as("SELECT id, path, mtime_ns, size FROM files")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, path, mtime, size)| (path, (id, mtime, size)))
        .collect())
}

/// Row counts for `Stats` and the envelope.
#[derive(Clone, Copy, Debug, Default, FromRow)]
pub struct Counts {
    /// `files` rows.
    pub files: i64,
    /// `files` with `language <> 'other'`.
    pub parsed_files: i64,
    /// `symbols` rows.
    pub symbols: i64,
    /// `refs` rows.
    pub refs: i64,
    /// `refs` with a `to_symbol`.
    pub resolved: i64,
    /// `refs` with `provenance = 'ambiguous'`.
    pub ambiguous: i64,
    /// `imports` rows.
    pub imports: i64,
    /// `imports` with `provenance = 'unresolved'`.
    pub unresolved_imports: i64,
    /// `cochanges` rows.
    pub cochange_pairs: i64,
}

/// Count files, symbols, refs, imports and co-change pairs.
pub async fn counts(pool: &SqlitePool) -> anyhow::Result<Counts> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM files) AS files, (SELECT count(*) FROM files WHERE language <> 'other') AS parsed_files, \
         (SELECT count(*) FROM symbols) AS symbols, (SELECT count(*) FROM refs) AS refs, \
         (SELECT count(*) FROM refs WHERE to_symbol IS NOT NULL) AS resolved, \
         (SELECT count(*) FROM refs WHERE provenance = 'ambiguous') AS ambiguous, \
         (SELECT count(*) FROM imports) AS imports, \
         (SELECT count(*) FROM imports WHERE provenance = 'unresolved') AS unresolved_imports, \
         (SELECT count(*) FROM cochanges) AS cochange_pairs",
    )
    .fetch_one(pool)
    .await?)
}

/// A file to (re)write: its walk facts plus the parse result (`None` for `other`).
pub struct FileWrite<'a> {
    /// Root-relative, forward slashes.
    pub path: &'a str,
    /// Sync key (60 §5.4).
    pub mtime_ns: i64,
    /// Sync key.
    pub size: i64,
    /// `files.language`.
    pub language: &'a str,
    /// `files.is_test` path heuristic.
    pub is_test: bool,
    /// `files.package` (60 §6.1), from the workspace model.
    pub package: Option<String>,
    /// Parse output; `None` writes the `files` row only.
    pub parsed: Option<&'a ParsedFile<'a>>,
}

/// One reconcile write (60 §5.4): delete the rows of removed and changed
/// files (cascades; FTS by rowid), insert the new ones. One transaction.
/// Returns the affected set: paths of the files whose `imports.target_file`
/// or `refs.to_symbol` pointed into a removed file (read before the cascade
/// nulls them), for the resolver to redo.
pub async fn apply(
    pool: &SqlitePool,
    removed: &[i64],
    writes: &[FileWrite<'_>],
) -> anyhow::Result<Vec<String>> {
    let mut tx = pool.begin().await?;
    let mut affected: Vec<String> = Vec::new();
    for batch in removed.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT DISTINCT f.path FROM files f WHERE f.id IN (SELECT file_id FROM imports WHERE target_file IN (",
        );
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push(") UNION SELECT r.file_id FROM refs r JOIN symbols s ON s.id = r.to_symbol WHERE s.file_id IN (");
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push("))");
        let rows: Vec<(String,)> = q.build_query_as().fetch_all(&mut *tx).await?;
        affected.extend(rows.into_iter().map(|(p,)| p));
    }
    for id in removed {
        for sql in [
            "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?)",
            "DELETE FROM symbols_tri WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?)",
        ] {
            sqlx::query(sql).bind(id).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM files WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    for w in writes {
        let (lines, parse_errors) = w.parsed.map_or((0, false), |p| (p.lines, p.has_errors));
        let file_id = sqlx::query(
            "INSERT INTO files (path, mtime_ns, size, language, package, lines, parse_errors, is_test) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(w.path)
        .bind(w.mtime_ns)
        .bind(w.size)
        .bind(w.language)
        .bind(&w.package)
        .bind(lines)
        .bind(parse_errors)
        .bind(w.is_test)
        .execute(&mut *tx)
        .await
        .map(|r: SqliteQueryResult| r.last_insert_rowid())?;
        let Some(p) = w.parsed else { continue };
        for batch in p.imports.chunks(ROWS_PER_INSERT) {
            let mut q = QueryBuilder::new(
                "INSERT INTO imports (file_id, specifier, names, line, provenance) ",
            );
            q.push_values(batch, |mut b, i| {
                b.push_bind(file_id)
                    .push_bind(&i.specifier)
                    .push_bind(i.names.as_ref().and_then(|n| serde_json::to_string(n).ok()))
                    .push_bind(i.line)
                    .push_bind("unresolved");
            });
            q.build().execute(&mut *tx).await?;
        }
        if p.symbols.is_empty() && p.refs.is_empty() {
            continue;
        }
        // Symbol ids are assigned explicitly as `max(id) + 1 ..`: this
        // transaction is the only writer, and parents precede children
        // (parse sorts by range), so `parent_id` and `from_symbol` are known
        // before the insert and rows go in batches instead of one by one.
        let first: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM symbols")
            .fetch_one(&mut *tx)
            .await?;
        let monikers = crate::parse::monikers(w.language, w.path, &p.symbols);
        let mut inserted = 0usize;
        for (bi, batch) in p.symbols.chunks(ROWS_PER_INSERT).enumerate() {
            let base = bi * ROWS_PER_INSERT;
            let mut q = QueryBuilder::new(
                "INSERT INTO symbols (id, file_id, name, kind, parent_id, line_start, line_end, byte_start, byte_end, signature, docs, exported, moniker) ",
            );
            q.push_values(batch.iter().enumerate(), |mut b, (i, s)| {
                b.push_bind(first + (base + i) as i64)
                    .push_bind(file_id)
                    .push_bind(&s.name)
                    .push_bind(s.kind)
                    .push_bind(s.parent.map(|pi| first + pi as i64))
                    .push_bind(s.line_start)
                    .push_bind(s.line_end)
                    .push_bind(s.byte_start)
                    .push_bind(s.byte_end)
                    .push_bind(&s.signature)
                    .push_bind(&s.docs)
                    .push_bind(s.exported)
                    .push_bind(&monikers[base + i]);
            });
            inserted += q.build().execute(&mut *tx).await?.rows_affected() as usize;
            let mut q = QueryBuilder::new(
                "INSERT INTO symbols_fts (rowid, name, signature, docs, path, symbol_id) ",
            );
            q.push_values(batch.iter().enumerate(), |mut b, (i, s)| {
                let id = first + (base + i) as i64;
                b.push_bind(id)
                    .push_bind(&s.name)
                    .push_bind(&s.signature)
                    .push_bind(&s.docs)
                    .push_bind(w.path)
                    .push_bind(id);
            });
            q.build().execute(&mut *tx).await?;
            let mut q = QueryBuilder::new("INSERT INTO symbols_tri (rowid, name) ");
            q.push_values(batch.iter().enumerate(), |mut b, (i, s)| {
                b.push_bind(first + (base + i) as i64).push_bind(&s.name);
            });
            q.build().execute(&mut *tx).await?;
        }
        if inserted != p.symbols.len() {
            anyhow::bail!("symbol id assignment drifted for {}", w.path);
        }
        for batch in p.refs.chunks(ROWS_PER_INSERT) {
            let mut q = QueryBuilder::new(
                "INSERT INTO refs (file_id, from_symbol, name, kind, line, qualifier, provenance, confidence) ",
            );
            q.push_values(batch, |mut b, r| {
                b.push_bind(file_id)
                    .push_bind(r.from.map(|i| first + i as i64))
                    .push_bind(&r.name)
                    .push_bind(r.kind)
                    .push_bind(r.line)
                    .push_bind(&r.qualifier)
                    .push_bind("none")
                    .push_bind(0.0f64);
            });
            q.build().execute(&mut *tx).await?;
        }
    }
    sqlx::query(
        "INSERT INTO meta (key, value) VALUES ('last_reconcile_at', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
            .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    affected.sort_unstable();
    affected.dedup();
    Ok(affected)
}

/// A `code_find` hit (60 §8).
#[derive(Clone, Debug, FromRow, serde::Serialize)]
pub struct FindRow {
    /// `symbols.id`.
    pub symbol_id: i64,
    /// Symbol name.
    pub name: String,
    /// Tags syntax type.
    pub kind: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// The definition line, trimmed.
    pub signature: String,
    /// `pub` / `export` / Uppercase / not `_` (60 §4).
    pub exported: bool,
    /// PageRank (0.0 until step 3).
    pub rank: f64,
    /// Stable identity (60 §8): `<language>:<path>#<Qualified.Name>(<kind>)`.
    pub moniker: String,
    /// How the query hit: `exact` | `prefix` | `infix` (name, case-insensitive) | `text` (signature/docs words).
    #[sqlx(rename = "match")]
    #[serde(rename = "match")]
    pub matched: String,
}

/// `code_find` (60 §8). `exact`: `symbols.name = name` only. Otherwise the
/// union of exact name, case-insensitive infix over names through the
/// trigram table (`tool registry` → `%tool%registry%` → `ToolRegistry`) and
/// FTS5 word-prefix over name/signature/docs; each row says which tier hit,
/// and rows come tier first, then rank, then path and line.
pub async fn find(
    pool: &SqlitePool,
    name: &str,
    kind: Option<&str>,
    exact: bool,
    limit: i64,
) -> anyhow::Result<Vec<FindRow>> {
    const EXACT: &str = "SELECT s.id AS symbol_id, s.name, s.kind, f.path, s.line_start, s.line_end, s.signature, s.exported, s.rank, s.moniker, 'exact' AS match \
         FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE s.name = ?1 AND (?5 IS NULL OR s.kind = ?5) \
         ORDER BY s.rank DESC, f.path, s.line_start LIMIT ?6";
    // The trigram hits are used twice (tier and filter): a materialized CTE
    // computes them once (0.26 ms vs 0.47 ms warm on 20k symbols).
    const TIERED: &str = "WITH tri AS MATERIALIZED (SELECT rowid AS id FROM symbols_tri WHERE name LIKE ?3) \
         SELECT s.id AS symbol_id, s.name, s.kind, f.path, s.line_start, s.line_end, s.signature, s.exported, s.rank, s.moniker, \
         CASE WHEN s.name = ?1 THEN 'exact' WHEN s.name LIKE ?2 THEN 'prefix' WHEN s.id IN tri THEN 'infix' ELSE 'text' END AS match \
         FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE (s.name = ?1 OR s.id IN tri OR s.id IN (SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH ?4)) \
           AND (?5 IS NULL OR s.kind = ?5) \
         ORDER BY CASE match WHEN 'exact' THEN 0 WHEN 'prefix' THEN 1 WHEN 'infix' THEN 2 ELSE 3 END, \
                  s.rank DESC, f.path, s.line_start LIMIT ?6";
    let infix = like_pattern(name);
    let rows = sqlx::query_as::<_, FindRow>(if exact { EXACT } else { TIERED })
        .bind(name)
        .bind(&infix[1..])
        .bind(&infix)
        .bind(fts_prefix(name))
        .bind(kind)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// `%tok%tok2%`: every whitespace-separated token as an infix, `%` stripped
/// from user text (`_` stays: a one-char wildcard over-matches `open_db` by
/// `openXdb` at worst). `[1..]` is the prefix form `tok%tok2%`.
fn like_pattern(name: &str) -> String {
    let mut q = String::with_capacity(name.len() + 4);
    q.push('%');
    for tok in name.split_whitespace() {
        let tok = tok.replace('%', "");
        if tok.is_empty() {
            continue;
        }
        q.push_str(&tok);
        q.push('%');
    }
    q
}

/// `"tok"* "tok2"*`: every whitespace-separated token as a quoted prefix,
/// so user text can never be FTS syntax.
fn fts_prefix(name: &str) -> String {
    let mut q = String::with_capacity(name.len() + 8);
    for tok in name.split_whitespace() {
        let tok = tok.replace('"', "");
        if tok.is_empty() {
            continue;
        }
        if !q.is_empty() {
            q.push(' ');
        }
        q.push('"');
        q.push_str(&tok);
        q.push_str("\"*");
    }
    if q.is_empty() {
        q.push_str("\"\"");
    }
    q
}

/// `files` row as `code_outline` shows it.
#[derive(Clone, Debug, FromRow)]
pub struct FileInfo {
    /// `files.id`.
    pub id: i64,
    /// Root-relative path.
    pub path: String,
    /// `files.language`.
    pub language: String,
    /// Line count.
    pub lines: i64,
    /// tree-sitter `has_errors`.
    pub parse_errors: bool,
}

/// The `files` row for `path`, if indexed.
pub async fn file(pool: &SqlitePool, path: &str) -> anyhow::Result<Option<FileInfo>> {
    Ok(
        sqlx::query_as("SELECT id, path, language, lines, parse_errors FROM files WHERE path = ?")
            .bind(path)
            .fetch_optional(pool)
            .await?,
    )
}

/// One symbol of a file, for the outline tree.
#[derive(Clone, Debug, FromRow)]
pub struct OutlineRow {
    /// `symbols.id`.
    pub id: i64,
    /// `symbols.parent_id`.
    pub parent_id: Option<i64>,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// Definition line.
    pub signature: String,
    /// Exported heuristic.
    pub exported: bool,
    /// Stable identity (60 §8).
    pub moniker: String,
}

/// A file's symbols in source order.
pub async fn outline(pool: &SqlitePool, file_id: i64) -> anyhow::Result<Vec<OutlineRow>> {
    Ok(sqlx::query_as(
        "SELECT id, parent_id, name, kind, line_start, line_end, signature, exported, moniker FROM symbols WHERE file_id = ? ORDER BY byte_start, byte_end DESC",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?)
}

/// One `imports` row as `code_outline` shows it (60 §8).
#[derive(Clone, Debug, FromRow, serde::Serialize)]
pub struct OutlineImport {
    /// As written.
    pub specifier: String,
    /// Target path, `None` when external/unresolved.
    pub target: Option<String>,
    /// JSON array of names, or `None` for a glob.
    pub names: Option<String>,
    /// 1-based.
    pub line: i64,
    /// 60 §4.
    pub provenance: String,
}

/// A file's imports in source order.
pub async fn file_imports(pool: &SqlitePool, file_id: i64) -> anyhow::Result<Vec<OutlineImport>> {
    Ok(sqlx::query_as(
        "SELECT i.specifier, t.path AS target, i.names, i.line, i.provenance FROM imports i \
         LEFT JOIN files t ON t.id = i.target_file WHERE i.file_id = ? ORDER BY i.line, i.id",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?)
}

// --- resolution (60 §6) ---------------------------------------------------

/// `files` as the resolver reads it.
#[derive(Clone, Debug, FromRow)]
pub struct FileRef {
    /// `files.id`.
    pub id: i64,
    /// Root-relative.
    pub path: String,
    /// `files.language`.
    pub language: String,
    /// `files.package`.
    pub package: Option<String>,
    /// `files.is_test`.
    pub is_test: bool,
}

/// Every `files` row.
pub async fn all_files(pool: &SqlitePool) -> anyhow::Result<Vec<FileRef>> {
    Ok(
        sqlx::query_as("SELECT id, path, language, package, is_test FROM files")
            .fetch_all(pool)
            .await?,
    )
}

/// An `imports` row as the resolver reads and rewrites it.
#[derive(Clone, Debug, FromRow)]
pub struct ImportRow {
    /// `imports.id`.
    pub id: i64,
    /// Importing file.
    pub file_id: i64,
    /// As written.
    pub specifier: String,
    /// JSON array or `None` (glob).
    pub names: Option<String>,
    /// Resolved target.
    pub target_file: Option<i64>,
    /// 60 §4.
    pub provenance: String,
}

/// Every `imports` row (the module tree and re-exports need all of them).
pub async fn all_imports(pool: &SqlitePool) -> anyhow::Result<Vec<ImportRow>> {
    Ok(
        sqlx::query_as(
            "SELECT id, file_id, specifier, names, target_file, provenance FROM imports",
        )
        .fetch_all(pool)
        .await?,
    )
}

/// A `symbols` row as a resolution candidate.
#[derive(Clone, Debug, FromRow)]
pub struct SymRow {
    /// `symbols.id`.
    pub id: i64,
    /// Defining file.
    pub file_id: i64,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// Enclosing definition (for Rust methods: the `impl`'s type, 60 §20).
    pub parent_id: Option<i64>,
}

/// Every `symbols` row.
// ponytail: loads the whole table each reconcile (13k rows here, ~10 ms);
// narrow to the names the changed refs use if a root has millions.
pub async fn all_symbols(pool: &SqlitePool) -> anyhow::Result<Vec<SymRow>> {
    Ok(sqlx::query_as(
        "SELECT id, file_id, name, kind, line_start, line_end, parent_id FROM symbols",
    )
    .fetch_all(pool)
    .await?)
}

/// A `refs` row to resolve.
#[derive(Clone, Debug, FromRow)]
pub struct RefRow {
    /// `refs.id`.
    pub id: i64,
    /// Referencing file.
    pub file_id: i64,
    /// Referenced name.
    pub name: String,
    /// `call` | `type` | `class` | `implementation`.
    pub kind: String,
    /// 1-based.
    pub line: i64,
    /// `a::b` / `obj` before the name, when plain.
    pub qualifier: Option<String>,
}

/// The refs of `file_ids`.
pub async fn refs_of(pool: &SqlitePool, file_ids: &[i64]) -> anyhow::Result<Vec<RefRow>> {
    let mut out = Vec::new();
    for batch in file_ids.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT id, file_id, name, kind, line, qualifier FROM refs WHERE file_id IN (",
        );
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push(")");
        out.extend(q.build_query_as::<RefRow>().fetch_all(pool).await?);
    }
    Ok(out)
}

/// Resolution result for one `imports` row.
pub struct ImportUpdate {
    /// `imports.id`.
    pub id: i64,
    /// Target, `None` for external/unresolved.
    pub target_file: Option<i64>,
    /// 60 §4.
    pub provenance: String,
}

/// Write import targets: rows into a temp table in batches, one `UPDATE …
/// FROM` (one statement per row cost seconds in step 1).
pub async fn update_imports(pool: &SqlitePool, updates: &[ImportUpdate]) -> anyhow::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS imp_res (id INTEGER PRIMARY KEY, target_file INTEGER, provenance TEXT NOT NULL)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM imp_res").execute(&mut *tx).await?;
    for batch in updates.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new("INSERT INTO imp_res (id, target_file, provenance) ");
        q.push_values(batch, |mut b, u| {
            b.push_bind(u.id)
                .push_bind(u.target_file)
                .push_bind(&u.provenance);
        });
        q.build().execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE imports SET target_file = r.target_file, provenance = r.provenance FROM imp_res r WHERE imports.id = r.id")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM imp_res").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Resolution result for one `refs` row (60 §6.2).
pub struct RefUpdate {
    /// `refs.id`.
    pub id: i64,
    /// Picked definition.
    pub to_symbol: Option<i64>,
    /// Definitions with that name and a compatible kind, root-wide.
    pub candidates: i64,
    /// `same_file|import|package|unique|ambiguous|none`.
    pub provenance: &'static str,
    /// 1.0 / 0.8 / 0.6 / 0.0; receiver calls 0.4 / 0.3 (60 §21).
    pub confidence: f64,
}

/// Write ref resolutions, the same way as [`update_imports`].
pub async fn update_refs(pool: &SqlitePool, updates: &[RefUpdate]) -> anyhow::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "CREATE TEMP TABLE IF NOT EXISTS ref_res (id INTEGER PRIMARY KEY, to_symbol INTEGER, candidates INTEGER NOT NULL, provenance TEXT NOT NULL, confidence REAL NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM ref_res").execute(&mut *tx).await?;
    for batch in updates.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "INSERT INTO ref_res (id, to_symbol, candidates, provenance, confidence) ",
        );
        q.push_values(batch, |mut b, u| {
            b.push_bind(u.id)
                .push_bind(u.to_symbol)
                .push_bind(u.candidates)
                .push_bind(u.provenance)
                .push_bind(u.confidence);
        });
        q.build().execute(&mut *tx).await?;
    }
    sqlx::query(
        "UPDATE refs SET to_symbol = r.to_symbol, candidates = r.candidates, provenance = r.provenance, confidence = r.confidence \
         FROM ref_res r WHERE refs.id = r.id",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM ref_res").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// --- code_refs (60 §8) ----------------------------------------------------

/// A definition as `code_refs` names it.
#[derive(Clone, Debug, FromRow, serde::Serialize)]
pub struct Definition {
    /// `symbols.id`.
    pub symbol_id: i64,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// Definition line.
    pub signature: String,
    /// Stable identity (60 §8).
    pub moniker: String,
}

/// Shared `SELECT` tail for [`Definition`] (a macro so `concat!` keeps the
/// SQL a literal for sqlx's injection check).
macro_rules! def_cols {
    () => {
        "s.id AS symbol_id, s.name, s.kind, f.path, s.line_start, s.line_end, s.signature, s.moniker \
         FROM symbols s JOIN files f ON f.id = s.file_id"
    };
}

/// Definitions by exact name, by path then line.
pub async fn definitions_named(pool: &SqlitePool, name: &str) -> anyhow::Result<Vec<Definition>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        def_cols!(),
        " WHERE s.name = ? ORDER BY f.path, s.line_start"
    ))
    .bind(name)
    .fetch_all(pool)
    .await?)
}

/// The definition with `symbols.id = id`.
pub async fn definition_by_id(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Definition>> {
    Ok(
        sqlx::query_as(concat!("SELECT ", def_cols!(), " WHERE s.id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// The definition with `symbols.moniker = moniker`.
pub async fn definition_by_moniker(
    pool: &SqlitePool,
    moniker: &str,
) -> anyhow::Result<Option<Definition>> {
    Ok(
        sqlx::query_as(concat!("SELECT ", def_cols!(), " WHERE s.moniker = ?"))
            .bind(moniker)
            .fetch_optional(pool)
            .await?,
    )
}

/// The innermost definition of `path` whose lines contain `line`.
pub async fn definition_at(
    pool: &SqlitePool,
    path: &str,
    line: i64,
) -> anyhow::Result<Option<Definition>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        def_cols!(),
        " WHERE f.path = ?1 AND s.line_start <= ?2 AND ?2 <= s.line_end ORDER BY (s.line_end - s.line_start) LIMIT 1"
    ))
    .bind(path)
    .bind(line)
    .fetch_optional(pool)
    .await?)
}

/// One reference as `code_refs` lists it.
#[derive(Clone, Debug, FromRow, serde::Serialize)]
pub struct RefHit {
    /// File of the reference.
    pub path: String,
    /// 1-based.
    pub line: i64,
    /// Referenced name.
    pub name: String,
    /// Enclosing definition of the reference, `None` at module scope.
    pub from_symbol: Option<String>,
    /// `call` | `type` | `class` | `implementation`.
    pub kind: String,
    /// 60 §6.2.
    pub provenance: String,
    /// 60 §6.2.
    pub confidence: f64,
    /// `direction=out`: `path:line` of the definition this reference resolved to.
    pub target: Option<String>,
}

/// Shared `SELECT` tail for [`RefHit`].
macro_rules! ref_cols {
    () => {
        "f.path, r.line, r.name, fs.name AS from_symbol, r.kind, r.provenance, r.confidence, \
         CASE WHEN t.id IS NULL THEN NULL ELSE tf.path || ':' || t.line_start END AS target \
         FROM refs r JOIN files f ON f.id = r.file_id LEFT JOIN symbols fs ON fs.id = r.from_symbol \
         LEFT JOIN symbols t ON t.id = r.to_symbol LEFT JOIN files tf ON tf.id = t.file_id"
    };
}

/// References resolved to `symbol_id` (`direction=in`), by path and line.
pub async fn refs_to(
    pool: &SqlitePool,
    symbol_id: i64,
    kind: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<RefHit>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        ref_cols!(),
        " WHERE r.to_symbol = ?1 AND (?2 IS NULL OR r.kind = ?2) ORDER BY f.path, r.line LIMIT ?3"
    ))
    .bind(symbol_id)
    .bind(kind)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// References made from inside `symbol_id` (`direction=out`), in source order.
pub async fn refs_from(
    pool: &SqlitePool,
    symbol_id: i64,
    kind: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<RefHit>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        ref_cols!(),
        " WHERE r.from_symbol = ?1 AND (?2 IS NULL OR r.kind = ?2) ORDER BY r.line LIMIT ?3"
    ))
    .bind(symbol_id)
    .bind(kind)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Refs named `name` that stayed ambiguous (60 §6.2 rule 5): the honest
/// residue `code_refs` reports beside the resolved list.
pub async fn ambiguous_named(
    pool: &SqlitePool,
    name: &str,
    kind: Option<&str>,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM refs WHERE name = ?1 AND provenance = 'ambiguous' AND (?2 IS NULL OR kind = ?2)",
    )
    .bind(name)
    .bind(kind)
    .fetch_one(pool)
    .await?)
}

// --- rank (60 §7) ---------------------------------------------------------

/// Store the unpersonalised ranks: `files.rank` for every graph node,
/// `symbols.rank` reset to 0 then set for the ranked definitions. Two
/// temp-table `UPDATE … FROM`s in one transaction.
pub async fn update_ranks(
    pool: &SqlitePool,
    files: &[(i64, f64)],
    symbols: &[(i64, f64)],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "CREATE TEMP TABLE IF NOT EXISTS rank_res (id INTEGER PRIMARY KEY, rank REAL NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    for (table, rows) in [("files", files), ("symbols", symbols)] {
        sqlx::query("DELETE FROM rank_res")
            .execute(&mut *tx)
            .await?;
        for batch in rows.chunks(ROWS_PER_INSERT) {
            let mut q = QueryBuilder::new("INSERT INTO rank_res (id, rank) ");
            q.push_values(batch, |mut b, (id, rank)| {
                b.push_bind(id).push_bind(rank);
            });
            q.build().execute(&mut *tx).await?;
        }
        if table == "symbols" {
            sqlx::query("UPDATE symbols SET rank = 0.0 WHERE rank <> 0.0")
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE symbols SET rank = r.rank FROM rank_res r WHERE symbols.id = r.id")
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE files SET rank = r.rank FROM rank_res r WHERE files.id = r.id")
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query("DELETE FROM rank_res")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The stored map: every ranked definition, best first (`code_map` without
/// `focus` is a lookup, 60 §7).
pub async fn ranked_tags(pool: &SqlitePool) -> anyhow::Result<Vec<crate::rank::Tag>> {
    #[derive(FromRow)]
    struct Row {
        path: String,
        line: i64,
        kind: String,
        name: String,
        signature: String,
        moniker: String,
        rank: f64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT f.path, s.line_start AS line, s.kind, s.name, s.signature, s.moniker, s.rank FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE s.rank > 0 ORDER BY s.rank DESC, f.path, s.line_start",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| crate::rank::Tag {
            path: r.path,
            line: r.line,
            kind: r.kind,
            name: r.name,
            signature: r.signature,
            moniker: r.moniker,
            rank: r.rank,
        })
        .collect())
}

/// Refs inside a line span of a file that resolved to something
/// (`code_symbol` callees), in source order.
pub async fn refs_in_span(
    pool: &SqlitePool,
    path: &str,
    line_start: i64,
    line_end: i64,
) -> anyhow::Result<Vec<RefHit>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        ref_cols!(),
        " WHERE f.path = ?1 AND r.line BETWEEN ?2 AND ?3 AND r.to_symbol IS NOT NULL ORDER BY r.line LIMIT ?4"
    ))
    .bind(path)
    .bind(line_start)
    .bind(line_end)
    .bind(ROWS_PER_INSERT as i64 * 2)
    .fetch_all(pool)
    .await?)
}

// --- git: co-change and code_impact (60 §8, §0.11) ------------------------

/// Replace `cochanges` wholesale (`file_a < file_b`) and record the walked
/// HEAD in `meta.cochange_head` and `meta.git_head`, one transaction.
pub async fn replace_cochanges(
    pool: &SqlitePool,
    rows: &[(i64, i64, i64)],
    head: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM cochanges")
        .execute(&mut *tx)
        .await?;
    for batch in rows.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new("INSERT INTO cochanges (file_a, file_b, count) ");
        q.push_values(batch, |mut b, (a, c, n)| {
            b.push_bind(a).push_bind(c).push_bind(n);
        });
        q.build().execute(&mut *tx).await?;
    }
    for key in ["cochange_head", "git_head"] {
        sqlx::query("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(head)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Files that changed together with `file_id` most often: `(path, count)`.
pub async fn cochange_partners(
    pool: &SqlitePool,
    file_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT f.path, c.count FROM cochanges c JOIN files f ON f.id = CASE WHEN c.file_a = ?1 THEN c.file_b ELSE c.file_a END \
         WHERE c.file_a = ?1 OR c.file_b = ?1 ORDER BY c.count DESC, f.path LIMIT ?2",
    )
    .bind(file_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Files referencing any of `symbol_ids` with `confidence ≥ 0.5` (the rank
/// graph's cut): distinct `(file_id, name, target file_id)`.
pub async fn referencers(
    pool: &SqlitePool,
    symbol_ids: &[i64],
) -> anyhow::Result<Vec<(i64, String, i64)>> {
    let mut out = Vec::new();
    for batch in symbol_ids.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT DISTINCT r.file_id, r.name, s.file_id FROM refs r JOIN symbols s ON s.id = r.to_symbol WHERE r.confidence >= 0.5 AND r.to_symbol IN (",
        );
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push(")");
        out.extend(
            q.build_query_as::<(i64, String, i64)>()
                .fetch_all(pool)
                .await?,
        );
    }
    Ok(out)
}

/// References into the changed symbols that `code_impact` does not count:
/// resolved below the 0.5 cut, or same-name and `ambiguous`.
pub async fn impact_skipped(
    pool: &SqlitePool,
    symbol_ids: &[i64],
    names: &[&str],
) -> anyhow::Result<i64> {
    let mut n = 0i64;
    for batch in symbol_ids.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT count(*) FROM refs WHERE confidence < 0.5 AND to_symbol IN (",
        );
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push(")");
        n += q.build_query_scalar::<i64>().fetch_one(pool).await?;
    }
    for batch in names.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT count(*) FROM refs WHERE provenance = 'ambiguous' AND name IN (",
        );
        let mut sep = q.separated(", ");
        for name in batch {
            sep.push_bind(*name);
        }
        q.push(")");
        n += q.build_query_scalar::<i64>().fetch_one(pool).await?;
    }
    Ok(n)
}

/// Every resolved `imports` edge `(file_id, target_file)` that is a use, not
/// a `mod x;` declaration (60 §21).
pub async fn import_edges(pool: &SqlitePool) -> anyhow::Result<Vec<(i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT file_id, target_file FROM imports WHERE target_file IS NOT NULL AND target_file <> file_id \
         AND specifier NOT LIKE 'mod:%'",
    )
    .fetch_all(pool)
    .await?)
}

/// `imports` rows of `file_ids` that stayed unresolved.
pub async fn unresolved_imports_in(pool: &SqlitePool, file_ids: &[i64]) -> anyhow::Result<i64> {
    let mut n = 0i64;
    for batch in file_ids.chunks(ROWS_PER_INSERT) {
        let mut q = QueryBuilder::new(
            "SELECT count(*) FROM imports WHERE provenance = 'unresolved' AND file_id IN (",
        );
        let mut sep = q.separated(", ");
        for id in batch {
            sep.push_bind(id);
        }
        q.push(")");
        n += q.build_query_scalar::<i64>().fetch_one(pool).await?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_prefix_quotes_every_token() {
        assert_eq!(fts_prefix("ToolRegistry"), "\"ToolRegistry\"*");
        assert_eq!(
            fts_prefix("open db\" OR x"),
            "\"open\"* \"db\"* \"OR\"* \"x\"*"
        );
        assert_eq!(fts_prefix("  "), "\"\"");
    }

    #[test]
    fn like_pattern_joins_tokens_and_strips_wildcards() {
        assert_eq!(like_pattern("Registry"), "%Registry%");
        assert_eq!(like_pattern("tool  registry"), "%tool%registry%");
        assert_eq!(like_pattern("a%b %"), "%ab%");
    }
}
