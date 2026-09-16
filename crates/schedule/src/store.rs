//! Run history: the `runs` table in `<data_dir>/schedule.sqlite` (34 §7).
//! One row per fire and per skip; everything the actor needs at boot is a
//! query over it.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Local};
use runtime::SessionId;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::RunInfo;

/// What the actor needs to know about one schedule at boot (34 §7).
#[derive(Clone, Debug, Default)]
pub struct Latest {
    /// Session of the newest row (fire or skip).
    pub session: Option<SessionId>,
    /// `fired_at` of the newest row that is not a skip; a running row counts.
    pub last: Option<DateTime<Local>>,
    /// `result` of the newest finished row that is not a skip.
    pub last_result: Option<String>,
}

/// Handle to `<data_dir>/schedule.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// `(name, session, fired_at, result)` as `latest()` selects it.
type LatestRow = (String, Option<String>, Option<String>, Option<String>);
/// `(session, fired_at, finished_at, result)` as `history()` selects it.
type RunRow = (Option<String>, String, Option<String>, Option<String>);

impl Store {
    /// Open or create the file, run the migration, and close every row still
    /// `NULL` in `finished_at` as `lost` (the harness died mid-run).
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
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        sqlx::query("UPDATE runs SET finished_at = ?, result = 'lost' WHERE finished_at IS NULL")
            .bind(Local::now().to_rfc3339())
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    /// Insert a fire (`result: None`, finished later by [`Store::finish_run`])
    /// or a skip (`result: Some("skipped: busy")`, finished at `fired_at`).
    /// Returns the row id.
    pub async fn insert_run(
        &self,
        name: &str,
        session: Option<SessionId>,
        fired_at: DateTime<Local>,
        result: Option<&str>,
    ) -> anyhow::Result<i64> {
        // A skip is finished the moment it is recorded; a fire is closed by `finish_run`.
        let done = sqlx::query(
            "INSERT INTO runs (name, session, fired_at, finished_at, result) \
             VALUES (?1, ?2, ?3, CASE WHEN ?4 IS NULL THEN NULL ELSE ?3 END, ?4)",
        )
        .bind(name)
        .bind(session.map(|s| s.to_string()))
        .bind(fired_at.to_rfc3339())
        .bind(result)
        .execute(&self.pool)
        .await?;
        Ok(done.last_insert_rowid())
    }

    /// Close a fire: `end_turn` / `error: …` / `timeout` / `cancelled`.
    pub async fn finish_run(
        &self,
        id: i64,
        finished_at: DateTime<Local>,
        result: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE runs SET finished_at = ?, result = ? WHERE id = ?")
            .bind(finished_at.to_rfc3339())
            .bind(result)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Per name, the boot state derived from its rows (34 §7). Newest = highest
    /// id: rows are only ever inserted at fire time, so id order is fire order.
    pub async fn latest(&self) -> anyhow::Result<HashMap<String, Latest>> {
        // Bare columns beside MAX() come from the max row (sqlite guarantee).
        let rows: Vec<LatestRow> = sqlx::query_as(
            "SELECT n.name, s.session, l.fired_at, r.result \
             FROM (SELECT DISTINCT name FROM runs) n \
             LEFT JOIN (SELECT name, session, MAX(id) FROM runs GROUP BY name) s \
                 ON s.name = n.name \
             LEFT JOIN (SELECT name, fired_at, MAX(id) FROM runs \
                 WHERE result IS NULL OR result NOT LIKE 'skipped%' GROUP BY name) l \
                 ON l.name = n.name \
             LEFT JOIN (SELECT name, result, MAX(id) FROM runs \
                 WHERE finished_at IS NOT NULL AND result NOT LIKE 'skipped%' GROUP BY name) r \
                 ON r.name = n.name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(name, session, last, last_result)| {
                let latest = Latest {
                    session: session.as_deref().map(session_of).transpose()?,
                    last: last.as_deref().map(time_of).transpose()?,
                    last_result,
                };
                Ok((name, latest))
            })
            .collect()
    }

    /// The last `limit` fires and skips of `name`, newest first.
    pub async fn history(&self, name: &str, limit: u32) -> anyhow::Result<Vec<RunInfo>> {
        let rows: Vec<RunRow> = sqlx::query_as(
            "SELECT session, fired_at, finished_at, result FROM runs \
             WHERE name = ? ORDER BY fired_at DESC, id DESC LIMIT ?",
        )
        .bind(name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(session, fired_at, finished_at, result)| {
                Ok(RunInfo {
                    session: session.as_deref().map(session_of).transpose()?,
                    fired_at: time_of(&fired_at)?,
                    finished_at: finished_at.as_deref().map(time_of).transpose()?,
                    result,
                })
            })
            .collect()
    }
}

fn session_of(s: &str) -> anyhow::Result<SessionId> {
    s.parse::<SessionId>()
        .with_context(|| format!("runs.session {s:?}"))
}

fn time_of(s: &str) -> anyhow::Result<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Local))
        .with_context(|| format!("runs timestamp {s:?}"))
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    #[tokio::test]
    async fn latest_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("schedule.sqlite"))
            .await
            .unwrap();
        let (s1, s2) = (SessionId::new(), SessionId::new());
        let t0 = Local::now() - TimeDelta::minutes(30);
        let t1 = t0 + TimeDelta::minutes(10);
        let t2 = t0 + TimeDelta::minutes(20);

        let first = store.insert_run("a", Some(s1), t0, None).await.unwrap();
        store.finish_run(first, t1, "end_turn").await.unwrap();
        store.insert_run("a", Some(s2), t1, None).await.unwrap();
        store
            .insert_run("a", Some(s2), t2, Some("skipped: busy"))
            .await
            .unwrap();
        store
            .insert_run("b", None, t2, Some("skipped: busy"))
            .await
            .unwrap();

        let latest = store.latest().await.unwrap();
        let a = &latest["a"];
        assert_eq!(a.session, Some(s2));
        assert_eq!(a.last, Some(t1));
        assert_eq!(a.last_result.as_deref(), Some("end_turn"));
        let b = &latest["b"];
        assert_eq!(b.session, None);
        assert_eq!(b.last, None);
        assert_eq!(b.last_result, None);

        let h = store.history("a", 2).await.unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].fired_at, t2);
        assert_eq!(h[0].finished_at, Some(t2));
        assert_eq!(h[0].result.as_deref(), Some("skipped: busy"));
        assert_eq!(h[1].fired_at, t1);
        assert_eq!(h[1].session, Some(s2));
        assert_eq!(h[1].finished_at, None);
        assert_eq!(h[1].result, None);
    }

    #[tokio::test]
    async fn reopen_closes_orphans_as_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedule.sqlite");
        let store = Store::open(&path).await.unwrap();
        store
            .insert_run("a", Some(SessionId::new()), Local::now(), None)
            .await
            .unwrap();
        drop(store);

        let store = Store::open(&path).await.unwrap();
        let h = store.history("a", 10).await.unwrap();
        assert_eq!(h[0].result.as_deref(), Some("lost"));
        assert!(h[0].finished_at.is_some());
        assert_eq!(
            store.latest().await.unwrap()["a"].last_result.as_deref(),
            Some("lost")
        );
    }
}
