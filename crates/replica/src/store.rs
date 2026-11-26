//! `<data_dir>/replica.sqlite`: sync bookkeeping — the outbox of every batch
//! held (own and relayed), per-column shadow clocks, tombstones, this
//! replica's identity and HLC. Replicated rows never live here; they stay in
//! the owning plugin's file.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Row as _, SqliteConnection, SqliteExecutor};

use crate::types::{Hlc, PeerId};

/// Handle to `<data_dir>/replica.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open or create the file and run the migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        platform::startup::migrate::run("replica", &sqlx::migrate!("./migrations"), &pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool })
    }

    /// The underlying pool, for the sync engine.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// This replica's origin id, generated (uuid v7) on first call.
    pub async fn identity(&self) -> anyhow::Result<PeerId> {
        if let Some(row) = sqlx::query("SELECT peer FROM identity WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?
        {
            return Ok(PeerId(row.get(0)));
        }
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query("INSERT OR IGNORE INTO identity (id, peer) VALUES (1, ?)")
            .bind(&id)
            .execute(&self.pool)
            .await?;
        // Another connection may have won the race: read back.
        let row = sqlx::query("SELECT peer FROM identity WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(PeerId(row.get(0)))
    }

    /// `(wall_ms, counter)` persisted by [`save_clock`]; zero if never.
    pub async fn load_clock(&self) -> anyhow::Result<(u64, u32)> {
        let row = sqlx::query("SELECT wall_ms, counter FROM clock WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .map(|r| (r.get::<i64, _>(0) as u64, r.get::<i64, _>(1) as u32))
            .unwrap_or_default())
    }

    /// Highest `seq` held per origin: what this engine has, for `Hello`.
    pub async fn version_vector(&self) -> anyhow::Result<HashMap<PeerId, u64>> {
        let rows = sqlx::query("SELECT origin, MAX(seq) FROM outbox GROUP BY origin")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (PeerId(r.get(0)), r.get::<i64, _>(1) as u64))
            .collect())
    }

    /// Lowest `seq` still held of `origin`, `None` when nothing is; anything below was pruned.
    pub async fn oldest_seq(&self, origin: &PeerId) -> anyhow::Result<Option<u64>> {
        let row = sqlx::query("SELECT MIN(seq) FROM outbox WHERE origin = ?")
            .bind(&origin.0)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<Option<i64>, _>(0).map(|s| s as u64))
    }

    /// Encoded batches of `origin` with `seq > after`, in order.
    pub async fn batches_after(&self, origin: &PeerId, after: u64) -> anyhow::Result<Vec<Vec<u8>>> {
        let rows =
            sqlx::query("SELECT batch FROM outbox WHERE origin = ? AND seq > ? ORDER BY seq")
                .bind(&origin.0)
                .bind(after as i64)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Whether `(origin, seq)` was already applied (or produced) here.
    pub async fn has_batch(&self, origin: &PeerId, seq: u64) -> anyhow::Result<bool> {
        Ok(
            sqlx::query("SELECT 1 FROM outbox WHERE origin = ? AND seq = ?")
                .bind(&origin.0)
                .bind(seq as i64)
                .fetch_optional(&self.pool)
                .await?
                .is_some(),
        )
    }
}

/// Remember the clock so a restart never stamps below an earlier write.
pub(crate) async fn save_clock(
    exec: impl SqliteExecutor<'_>,
    wall_ms: u64,
    counter: u32,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO clock (id, wall_ms, counter) VALUES (1, ?, ?)
         ON CONFLICT (id) DO UPDATE SET wall_ms = excluded.wall_ms, counter = excluded.counter",
    )
    .bind(wall_ms as i64)
    .bind(i64::from(counter))
    .execute(exec)
    .await?;
    Ok(())
}

/// Insert one encoded batch; a duplicate `(origin, seq)` is ignored.
pub(crate) async fn insert_batch(
    exec: impl SqliteExecutor<'_>,
    origin: &PeerId,
    seq: u64,
    bytes: &[u8],
) -> anyhow::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO outbox (origin, seq, batch) VALUES (?, ?, ?)")
        .bind(&origin.0)
        .bind(seq as i64)
        .bind(bytes)
        .execute(exec)
        .await?;
    Ok(())
}

/// Shadow clocks of every column of one row.
pub(crate) async fn col_clocks(
    conn: &mut SqliteConnection,
    table: &str,
    pk: &str,
) -> anyhow::Result<HashMap<String, Hlc>> {
    let rows =
        sqlx::query("SELECT column_name, hlc FROM col_clock WHERE table_name = ? AND pk = ?")
            .bind(table)
            .bind(pk)
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r.get(0), Hlc::parse(r.get::<&str, _>(1))?)))
        .collect())
}

/// Record the clock of a column write (newer replaces older; the caller
/// already compared).
pub(crate) async fn set_col_clock(
    conn: &mut SqliteConnection,
    table: &str,
    pk: &str,
    column: &str,
    hlc: &Hlc,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO col_clock (table_name, pk, column_name, hlc) VALUES (?, ?, ?, ?)
         ON CONFLICT (table_name, pk, column_name) DO UPDATE SET hlc = excluded.hlc",
    )
    .bind(table)
    .bind(pk)
    .bind(column)
    .bind(hlc.to_text())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The row's tombstone clock, if it was deleted.
pub(crate) async fn tombstone(
    conn: &mut SqliteConnection,
    table: &str,
    pk: &str,
) -> anyhow::Result<Option<Hlc>> {
    let row = sqlx::query("SELECT hlc FROM tombstone WHERE table_name = ? AND pk = ?")
        .bind(table)
        .bind(pk)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.and_then(|r| Hlc::parse(r.get::<&str, _>(0))))
}

/// The row died at `hlc`: drop its column clocks, remember the tombstone.
pub(crate) async fn bury(
    conn: &mut SqliteConnection,
    table: &str,
    pk: &str,
    hlc: &Hlc,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM col_clock WHERE table_name = ? AND pk = ?")
        .bind(table)
        .bind(pk)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO tombstone (table_name, pk, hlc) VALUES (?, ?, ?)
         ON CONFLICT (table_name, pk) DO UPDATE SET hlc = excluded.hlc",
    )
    .bind(table)
    .bind(pk)
    .bind(hlc.to_text())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The row came back: forget the tombstone.
pub(crate) async fn unbury(
    conn: &mut SqliteConnection,
    table: &str,
    pk: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM tombstone WHERE table_name = ? AND pk = ?")
        .bind(table)
        .bind(pk)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
