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
pub const SCHEMA_VERSION: &str = "1";

/// Rows per multi-row `INSERT` (≤ 12 binds each; SQLite allows 32 766).
const ROWS_PER_INSERT: usize = 500;
/// 60 §4, verbatim.
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
  rank      REAL NOT NULL DEFAULT 0.0
)",
    "CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id)",
    "CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
  name, signature, docs, path UNINDEXED, symbol_id UNINDEXED,
  tokenize = 'porter unicode61 remove_diacritics 2'
)",
    "CREATE TABLE IF NOT EXISTS imports (
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  specifier  TEXT NOT NULL,
  target_file INTEGER REFERENCES files(id) ON DELETE SET NULL,
  names      TEXT,
  line       INTEGER NOT NULL,
  provenance TEXT NOT NULL
)",
    "CREATE INDEX IF NOT EXISTS imports_target ON imports(target_file)",
    "CREATE TABLE IF NOT EXISTS refs (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  from_symbol INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  line       INTEGER NOT NULL,
  to_symbol  INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  candidates INTEGER NOT NULL DEFAULT 0,
  provenance TEXT NOT NULL,
  confidence REAL NOT NULL
)",
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
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// `files` rows.
    pub files: i64,
    /// `symbols` rows.
    pub symbols: i64,
    /// `refs` rows.
    pub refs: i64,
}

/// Count files, symbols and refs.
pub async fn counts(pool: &SqlitePool) -> anyhow::Result<Counts> {
    let (files, symbols, refs): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM files), (SELECT count(*) FROM symbols), (SELECT count(*) FROM refs)",
    )
    .fetch_one(pool)
    .await?;
    Ok(Counts {
        files,
        symbols,
        refs,
    })
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
    /// Parse output; `None` writes the `files` row only.
    pub parsed: Option<&'a ParsedFile<'a>>,
}

/// One reconcile write (60 §5.4): delete the rows of removed and changed
/// files (cascades; FTS by rowid), insert the new ones. One transaction.
pub async fn apply(
    pool: &SqlitePool,
    removed: &[i64],
    writes: &[FileWrite<'_>],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for id in removed {
        sqlx::query("DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?)")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM files WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    for w in writes {
        let (lines, parse_errors) = w
            .parsed
            .map_or((0, false), |p| (p.lines, p.has_errors));
        let file_id = sqlx::query(
            "INSERT INTO files (path, mtime_ns, size, language, lines, parse_errors, is_test) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(w.path)
        .bind(w.mtime_ns)
        .bind(w.size)
        .bind(w.language)
        .bind(lines)
        .bind(parse_errors)
        .bind(w.is_test)
        .execute(&mut *tx)
        .await
        .map(|r: SqliteQueryResult| r.last_insert_rowid())?;
        let Some(p) = w.parsed else { continue };
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
        let mut inserted = 0usize;
        for (bi, batch) in p.symbols.chunks(ROWS_PER_INSERT).enumerate() {
            let base = bi * ROWS_PER_INSERT;
            let mut q = QueryBuilder::new(
                "INSERT INTO symbols (id, file_id, name, kind, parent_id, line_start, line_end, byte_start, byte_end, signature, docs, exported) ",
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
                    .push_bind(s.exported);
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
        }
        if inserted != p.symbols.len() {
            anyhow::bail!("symbol id assignment drifted for {}", w.path);
        }
        for batch in p.refs.chunks(ROWS_PER_INSERT) {
            let mut q = QueryBuilder::new(
                "INSERT INTO refs (file_id, from_symbol, name, kind, line, provenance, confidence) ",
            );
            q.push_values(batch, |mut b, r| {
                b.push_bind(file_id)
                    .push_bind(r.from.map(|i| first + i as i64))
                    .push_bind(&r.name)
                    .push_bind(r.kind)
                    .push_bind(r.line)
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
    Ok(())
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
}

/// `code_find`: exact `symbols.name`, or FTS5 prefix over name/signature/docs
/// unioned with the exact name so stemming can never hide an exact hit.
/// Exact-name hits first, then rank, then path and line.
pub async fn find(
    pool: &SqlitePool,
    name: &str,
    kind: Option<&str>,
    exact: bool,
    limit: i64,
) -> anyhow::Result<Vec<FindRow>> {
    const EXACT: &str = "SELECT s.id AS symbol_id, s.name, s.kind, f.path, s.line_start, s.line_end, s.signature, s.exported, s.rank \
         FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE s.name = ?1 AND (?3 IS NULL OR s.kind = ?3) \
         ORDER BY s.rank DESC, f.path, s.line_start LIMIT ?4";
    const PREFIX: &str = "SELECT s.id AS symbol_id, s.name, s.kind, f.path, s.line_start, s.line_end, s.signature, s.exported, s.rank \
         FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE (s.name = ?1 OR s.id IN (SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH ?2)) AND (?3 IS NULL OR s.kind = ?3) \
         ORDER BY (s.name = ?1) DESC, s.rank DESC, f.path, s.line_start LIMIT ?4";
    let rows = sqlx::query_as::<_, FindRow>(if exact { EXACT } else { PREFIX })
        .bind(name)
        .bind(fts_prefix(name))
        .bind(kind)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows)
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
    Ok(sqlx::query_as(
        "SELECT id, path, language, lines, parse_errors FROM files WHERE path = ?",
    )
    .bind(path)
    .fetch_optional(pool)
    .await?)
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
}

/// A file's symbols in source order.
pub async fn outline(pool: &SqlitePool, file_id: i64) -> anyhow::Result<Vec<OutlineRow>> {
    Ok(sqlx::query_as(
        "SELECT id, parent_id, name, kind, line_start, line_end, signature, exported FROM symbols WHERE file_id = ? ORDER BY byte_start, byte_end DESC",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_prefix_quotes_every_token() {
        assert_eq!(fts_prefix("ToolRegistry"), "\"ToolRegistry\"*");
        assert_eq!(fts_prefix("open db\" OR x"), "\"open\"* \"db\"* \"OR\"* \"x\"*");
        assert_eq!(fts_prefix("  "), "\"\"");
    }
}
