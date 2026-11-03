//! Owner: E1 — the run store (design 171 §8.4), `~/.theuth/cells/cells.sqlite`. The migration
//! list is `./migrations` (sqlx): 0001 (C, frozen: every table and column), 0002 (E1), 0003
//! (E2) — reserved files so a worker's extra statements never need a new registration.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use crate::error::{CellsError, Result};

/// Every migration of the cells store, embedded.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The run store.
#[derive(Clone, Debug)]
pub struct Store {
    /// The pool.
    pub pool: SqlitePool,
}

impl Store {
    /// Open (create) `path` and migrate.
    pub async fn open(path: &Path) -> Result<Self, CellsError> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .connect_with(opts)
            .await
            .map_err(|e| CellsError::Failed(format!("cells.sqlite: {e}")))?;
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| CellsError::Failed(format!("cells.sqlite migrate: {e}")))?;
        Ok(Self { pool })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_create_every_section_8_4_column() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("cells.sqlite")).await.unwrap();
        let cols: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM pragma_table_info('cell_runs')")
                .fetch_all(&s.pool)
                .await
                .unwrap();
        let cols: Vec<&str> = cols.iter().map(|c| c.0.as_str()).collect();
        for c in [
            "id",
            "scope",
            "cell",
            "lineage",
            "mode",
            "pass",
            "kernel",
            "interpreter",
            "cwd",
            "env_names",
            "presented_sha",
            "code_sha",
            "edited",
            "pressed_by",
            "typed",
            "state",
            "exit",
            "signal",
            "secs",
            "expect",
            "summary",
            "output_sha",
            "output_bytes",
            "log",
            "log_from",
            "log_to",
            "queued_ms",
            "started_ms",
            "ended_ms",
            "delivery",
            "consumed_ms",
        ] {
            assert!(cols.contains(&c), "{c} missing from {cols:?}");
        }
        assert_eq!(MIGRATOR.iter().count(), 3);
    }
}
