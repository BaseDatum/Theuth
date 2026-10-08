//! The map's own SQLite beside the index (147 §9, O7):
//! `<data_dir>/codeindex/<slug>-<hash8>-map.sqlite`, own schema version, so
//! an index rebuild never deletes paid prose. The DDL and open/create live
//! here for every worker; the row queries are W1's.
//!
//! owner: W1 — design §9, §5 (reads), §8 (stale marks)

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::root;

/// Own schema version (`meta.schema_version`); mismatch = remove and rebuild.
pub const MAP_SCHEMA_VERSION: u32 = 1;

/// §9 verbatim. `CREATE … IF NOT EXISTS` so a reopen is a no-op.
pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT
)",
    // schema_version, root, index_state, git_head, skeleton_ms
    "CREATE TABLE IF NOT EXISTS perspectives (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  title      TEXT NOT NULL,
  origin     TEXT NOT NULL,      -- structure | generated | imported
  why        TEXT,
  order_no   INTEGER NOT NULL,
  eligible   TEXT,               -- JSON detector summary
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
)",
    "CREATE TABLE IF NOT EXISTS elements (
  id            TEXT PRIMARY KEY, -- <perspective>/<node>
  perspective   TEXT NOT NULL,
  node          TEXT NOT NULL,
  parent        TEXT,
  depth         INTEGER NOT NULL,
  order_no      INTEGER NOT NULL,
  name          TEXT NOT NULL,
  path          TEXT,
  kind          TEXT NOT NULL,    -- unit | file | symbol | cluster | external | bundle | perspective
  members       TEXT NOT NULL,    -- JSON node ids
  leaf          INTEGER NOT NULL,
  origin        TEXT NOT NULL,    -- structure | generated | imported
  stale         INTEGER NOT NULL DEFAULT 0,
  facts_hash    TEXT,
  diagram       TEXT,             -- GraphProps JSON
  prev_diagram  TEXT,
  description   TEXT,
  context       TEXT,
  constraint_   TEXT,
  concern       TEXT,
  todo          TEXT,
  note          TEXT,
  model         TEXT,
  git_head      TEXT,
  index_state   TEXT,
  generated_ms  INTEGER,
  call_ids      TEXT,             -- JSON
  cost_nanos    INTEGER,
  currency      TEXT,
  input_tokens  INTEGER,
  output_tokens INTEGER,
  repaired      INTEGER,
  error         TEXT
)",
    "CREATE TABLE IF NOT EXISTS edges (
  element   TEXT NOT NULL,
  from_node TEXT NOT NULL,
  to_node   TEXT NOT NULL,
  label     TEXT NOT NULL,
  backed    INTEGER NOT NULL,
  evidence  TEXT NOT NULL,        -- JSON MapEvidence[]
  PRIMARY KEY (element, from_node, to_node)
)",
    "CREATE TABLE IF NOT EXISTS anchors (
  element    TEXT NOT NULL,
  path       TEXT NOT NULL,
  line_start INTEGER,
  line_end   INTEGER,
  PRIMARY KEY (element, path, line_start)
)",
    "CREATE TABLE IF NOT EXISTS jobs (
  id          TEXT PRIMARY KEY,
  session     TEXT NOT NULL,
  started_ms  INTEGER NOT NULL,
  finished_ms INTEGER,
  stage       TEXT NOT NULL,
  done        INTEGER NOT NULL DEFAULT 0,
  total       INTEGER NOT NULL DEFAULT 0,
  calls       INTEGER NOT NULL DEFAULT 0,
  cost_nanos  INTEGER NOT NULL DEFAULT 0,
  currency    TEXT NOT NULL DEFAULT 'USD',
  error       TEXT,
  error_log   TEXT
)",
    "CREATE INDEX IF NOT EXISTS elements_parent ON elements(perspective, parent, order_no)",
    "CREATE INDEX IF NOT EXISTS elements_node ON elements(node)",
    "CREATE INDEX IF NOT EXISTS elements_stale ON elements(stale)",
    "CREATE INDEX IF NOT EXISTS anchors_path ON anchors(path)",
];

/// `<dir>/<slug>-<hash8>-map.sqlite`: the index's file name (`root::db_file_name`)
/// with `-map` before the extension, so `ls` pairs each index with its map.
pub fn path(dir: &Path, root: &Path) -> PathBuf {
    let index = root::db_file_name(root);
    let stem = index.strip_suffix(".sqlite").unwrap_or(&index);
    dir.join(format!("{stem}-map.sqlite"))
}

/// Open (creating) the map for `root` at `path`; a schema mismatch or an
/// unusable file removes `path`, `-wal`, `-shm` and starts over (60 §0.7's
/// rule — a map schema bump costs one regeneration).
pub async fn open(path: &Path, root: &Path) -> anyhow::Result<SqlitePool> {
    match open_db(path).await {
        Ok(pool) if schema_version(&pool).await == Some(MAP_SCHEMA_VERSION) => return Ok(pool),
        Ok(pool) => {
            if schema_version(&pool).await.is_some() {
                tracing::info!(path = %path.display(), "codemap schema changed; rebuilding");
            }
            pool.close().await;
        }
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), path = %path.display(), "codemap unusable; rebuilding");
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
        .map_or(0, |d| d.as_millis());
    for (k, v) in [
        ("schema_version", MAP_SCHEMA_VERSION.to_string()),
        ("root", root.to_string_lossy().into_owned()),
        ("created_ms", now.to_string()),
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

async fn schema_version(pool: &SqlitePool) -> Option<u32> {
    meta(pool, "schema_version")
        .await
        .ok()
        .flatten()?
        .parse()
        .ok()
}

/// One `meta` value.
pub async fn meta(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

/// Upsert one `meta` row.
pub async fn set_meta(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_path_pairs_with_the_index_file() {
        let dir = Path::new("/data/codeindex");
        let root = Path::new("/Users/sky/Develop/theuth");
        let index = root::db_file_name(root);
        let map = path(dir, root);
        let name = map.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with("-map.sqlite"), "{name}");
        assert_eq!(
            name.strip_suffix("-map.sqlite").unwrap(),
            index.strip_suffix(".sqlite").unwrap()
        );
    }

    #[tokio::test]
    async fn open_creates_every_table_and_rebuilds_on_version_mismatch() {
        let dir = std::env::temp_dir().join(format!("codemap-store-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = Path::new("/r");
        let p = path(&dir, root);
        let pool = open(&p, root).await.unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            tables,
            [
                "anchors",
                "edges",
                "elements",
                "jobs",
                "meta",
                "perspectives"
            ]
        );
        set_meta(&pool, "schema_version", "0").await.unwrap();
        sqlx::query("INSERT INTO perspectives (id, kind, title, origin, order_no, created_ms, updated_ms) VALUES ('p', 'k', 't', 'structure', 0, 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let pool = open(&p, root).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM perspectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "a version mismatch starts over");
        assert_eq!(schema_version(&pool).await, Some(MAP_SCHEMA_VERSION));
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
