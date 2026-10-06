//! Run history: the `runs` table in `<data_dir>/schedule.sqlite` (34 §7).
//! One row per fire and per skip; everything the actor needs at boot is a
//! query over it.

use std::collections::{HashMap, HashSet};
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
/// `(session, fired_at, finished_at, result, report, envelope)` as `history()` selects it.
type RunRow = (
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A fire still open at boot whose session is being resumed (109 §4.6
/// step 12): the actor re-attaches a run to it instead of writing `lost`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRun {
    /// Row id.
    pub id: i64,
    /// Schedule name.
    pub name: String,
    /// The run's session.
    pub session: SessionId,
    /// When it fired.
    pub fired_at: DateTime<Local>,
}

impl Store {
    /// Open or create the file, run the migration, and close every row still
    /// `NULL` in `finished_at` as `lost` (the harness died mid-run).
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Self::open_sparing(path, &HashSet::new()).await?.0)
    }

    /// [`Store::open`] that keeps the orphan rows of `spare` sessions open
    /// (their turn is being resumed, 109 §4.6 step 12) and returns them.
    pub async fn open_sparing(
        path: &Path,
        spare: &HashSet<SessionId>,
    ) -> anyhow::Result<(Self, Vec<OpenRun>)> {
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
        let orphans: Vec<(i64, String, Option<String>, String)> = sqlx::query_as(
            "SELECT id, name, session, fired_at FROM runs WHERE finished_at IS NULL",
        )
        .fetch_all(&pool)
        .await?;
        let mut kept = Vec::new();
        let now = Local::now().to_rfc3339();
        for (id, name, session, fired_at) in orphans {
            let session = session.and_then(|s| s.parse::<SessionId>().ok());
            match session.filter(|s| spare.contains(s)) {
                Some(session) => kept.push(OpenRun {
                    id,
                    name,
                    session,
                    fired_at: fired_at.parse().unwrap_or_else(|_| Local::now()),
                }),
                None => {
                    sqlx::query("UPDATE runs SET finished_at = ?, result = 'lost' WHERE id = ?")
                        .bind(&now)
                        .bind(id)
                        .execute(&pool)
                        .await?;
                }
            }
        }
        Ok((Self { pool }, kept))
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
        self.insert_run_with(name, session, fired_at, result, None)
            .await
    }

    /// [`Store::insert_run`] with the payload's first line as `envelope`
    /// (40 §1.5, §3.4): what a `when:` fire records for history grouping.
    pub async fn insert_run_with(
        &self,
        name: &str,
        session: Option<SessionId>,
        fired_at: DateTime<Local>,
        result: Option<&str>,
        envelope: Option<&str>,
    ) -> anyhow::Result<i64> {
        // A skip is finished the moment it is recorded; a fire is closed by `finish_run`.
        let done = sqlx::query(
            "INSERT INTO runs (name, session, fired_at, finished_at, result, envelope) \
             VALUES (?1, ?2, ?3, CASE WHEN ?4 IS NULL THEN NULL ELSE ?3 END, ?4, ?5)",
        )
        .bind(name)
        .bind(session.map(|s| s.to_string()))
        .bind(fired_at.to_rfc3339())
        .bind(result)
        .bind(envelope)
        .execute(&self.pool)
        .await?;
        Ok(done.last_insert_rowid())
    }

    /// Close a fire: `end_turn` / `error: …` / `timeout` / `cancelled`, with the
    /// run's final text as `report` (33 §0.7).
    pub async fn finish_run(
        &self,
        id: i64,
        finished_at: DateTime<Local>,
        result: &str,
        report: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE runs SET finished_at = ?, result = ?, report = ? WHERE id = ?")
            .bind(finished_at.to_rfc3339())
            .bind(result)
            .bind(report)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fires of `name` since its last loop end row (33 §3.2): rows with a
    /// session (or a `task <id>` result, 67 §7.4) that are not skips, newer
    /// than the newest `done…` / `max_runs…` / `until broken…` / `removed`
    /// row. No end row = all its fires.
    pub async fn count_since_end(&self, name: &str) -> anyhow::Result<u32> {
        let (n,): (u32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM runs WHERE name = ?1 AND (session IS NOT NULL OR result LIKE 'task %') \
             AND (result IS NULL OR result NOT LIKE 'skipped:%') \
             AND id > COALESCE((SELECT MAX(id) FROM runs WHERE name = ?1 \
                 AND (result LIKE 'done%' OR result LIKE 'max_runs%' \
                      OR result LIKE 'until broken%' OR result = 'removed')), 0)",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
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
            "SELECT session, fired_at, finished_at, result, report, envelope FROM runs \
             WHERE name = ? ORDER BY fired_at DESC, id DESC LIMIT ?",
        )
        .bind(name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(session, fired_at, finished_at, result, report, envelope)| {
                    Ok(RunInfo {
                        kind: crate::RunKind::of(session.is_some(), result.as_deref()),
                        session: session.as_deref().map(session_of).transpose()?,
                        fired_at: time_of(&fired_at)?,
                        finished_at: finished_at.as_deref().map(time_of).transpose()?,
                        result,
                        report,
                        envelope,
                    })
                },
            )
            .collect()
    }

    // --- watch (40 §3.4) ----------------------------------------------------

    /// The `watch` row of `name`; `None` before its first committed check.
    pub async fn watch_get(&self, name: &str) -> anyhow::Result<Option<WatchRow>> {
        let row: Option<WatchRaw> = sqlx::query_as(
            "SELECT name, state, last_hash, last_fired_at, last_check_at, last_outcome, \
             last_elapsed_ms, holding_since, strikes, fired_count, session_id \
             FROM watch WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        row.map(WatchRow::try_from).transpose()
    }

    /// Insert or replace the whole row.
    pub async fn watch_put(&self, w: &WatchRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO watch (name, state, last_hash, last_fired_at, last_check_at, \
             last_outcome, last_elapsed_ms, holding_since, strikes, fired_count, session_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&w.name)
        .bind(&w.state)
        .bind(&w.last_hash)
        .bind(w.last_fired_at.map(|t| t.to_rfc3339()))
        .bind(w.last_check_at.map(|t| t.to_rfc3339()))
        .bind(&w.last_outcome)
        .bind(w.last_elapsed_ms.map(|m| m as i64))
        .bind(w.holding_since.map(|t| t.to_rfc3339()))
        .bind(w.strikes as i64)
        .bind(w.fired_count as i64)
        .bind(w.session_id.map(|s| s.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Commit a successful fire (40 §1.7, §3.3): `state`, `last_hash`,
    /// `last_fired_at`; `fired_count` +1; `holding_since` and `strikes` cleared.
    /// Creates the row when missing.
    pub async fn watch_commit(
        &self,
        name: &str,
        state: &str,
        hash: &str,
        fired_at: DateTime<Local>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watch (name, state, last_hash, last_fired_at, fired_count) \
             VALUES (?1, ?2, ?3, ?4, 1) \
             ON CONFLICT(name) DO UPDATE SET state = ?2, last_hash = ?3, last_fired_at = ?4, \
             fired_count = fired_count + 1, holding_since = NULL, strikes = 0",
        )
        .bind(name)
        .bind(state)
        .bind(hash)
        .bind(fired_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `last_fired_at` at fire time (40 §1.8: `cooldown` counts from the fire,
    /// whatever its run ends as). Creates the row when missing.
    pub async fn watch_fired_at(&self, name: &str, at: DateTime<Local>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watch (name, last_fired_at) VALUES (?1, ?2) \
             ON CONFLICT(name) DO UPDATE SET last_fired_at = ?2",
        )
        .bind(name)
        .bind(at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `strikes` +1 (40 §5); returns the new count. Creates the row when missing.
    pub async fn watch_strike(&self, name: &str) -> anyhow::Result<u32> {
        let (n,): (i64,) = sqlx::query_as(
            "INSERT INTO watch (name, strikes) VALUES (?1, 1) \
             ON CONFLICT(name) DO UPDATE SET strikes = strikes + 1 RETURNING strikes",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(n.max(0) as u32)
    }

    /// `strikes` = 0 (a successful check, or `resume`). No row → no-op.
    pub async fn watch_reset_strikes(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE watch SET strikes = 0 WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record the last check (`last_check` in `ScheduleInfo`, 40 §3.1). Creates
    /// the row when missing.
    pub async fn watch_last_check(
        &self,
        name: &str,
        at: DateTime<Local>,
        outcome: &str,
        elapsed_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watch (name, last_check_at, last_outcome, last_elapsed_ms) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(name) DO UPDATE SET last_check_at = ?2, last_outcome = ?3, \
             last_elapsed_ms = ?4",
        )
        .bind(name)
        .bind(at.to_rfc3339())
        .bind(outcome)
        .bind(elapsed_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a successful check (40 §3.2): `last_check_*`, `holding_since`
    /// (`first_true_at`), `strikes` = 0, and `state` when the gate commits it
    /// (quiet / unchanged — a true check that did not fire keeps the old state
    /// until its run succeeds, §1.7). Creates the row when missing.
    pub async fn watch_check_ok(
        &self,
        name: &str,
        at: DateTime<Local>,
        outcome: &str,
        elapsed_ms: u64,
        holding_since: Option<DateTime<Local>>,
        state: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watch (name, state, last_check_at, last_outcome, last_elapsed_ms, holding_since) \
             VALUES (?1, COALESCE(?6, '{}'), ?2, ?3, ?4, ?5) \
             ON CONFLICT(name) DO UPDATE SET last_check_at = ?2, last_outcome = ?3, \
             last_elapsed_ms = ?4, holding_since = ?5, strikes = 0, state = COALESCE(?6, state)",
        )
        .bind(name)
        .bind(at.to_rfc3339())
        .bind(outcome)
        .bind(elapsed_ms as i64)
        .bind(holding_since.map(|t| t.to_rfc3339()))
        .bind(state)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Bind the watcher's session before its first fire (40 §1.12,
    /// `BindSession`). Creates the row when missing.
    pub async fn watch_bind(&self, name: &str, session: SessionId) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watch (name, session_id) VALUES (?1, ?2) \
             ON CONFLICT(name) DO UPDATE SET session_id = ?2",
        )
        .bind(name)
        .bind(session.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forget `name`'s sensor state (the file was removed or re-created).
    pub async fn watch_delete(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM watch WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// One `watch` row (40 §3.4): a watcher's sensor state and check bookkeeping.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchRow {
    /// Schedule name (PK).
    pub name: String,
    /// Sensor state JSON (≤ 16 KiB), committed after a successful run; `{}` at first.
    pub state: String,
    /// `sha256(payload)` of the last fired payload.
    pub last_hash: Option<String>,
    /// When the last committed fire happened.
    pub last_fired_at: Option<DateTime<Local>>,
    /// When the sensor last ran.
    pub last_check_at: Option<DateTime<Local>>,
    /// The last check's `CheckOutcome` string.
    pub last_outcome: Option<String>,
    /// The last check's sensor wall clock.
    pub last_elapsed_ms: Option<u64>,
    /// First true tick of the current `for:` window (`first_true_at` in 40 §3.2).
    pub holding_since: Option<DateTime<Local>>,
    /// Consecutive broken checks / failed runs (40 §5).
    pub strikes: u32,
    /// Committed fires, all time.
    pub fired_count: u32,
    /// Session bound at creation (`BindSession`, 40 §1.12).
    pub session_id: Option<SessionId>,
}

/// `watch` columns as sqlite hands them back.
type WatchRaw = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
    i64,
    Option<String>,
);

impl TryFrom<WatchRaw> for WatchRow {
    type Error = anyhow::Error;

    fn try_from(r: WatchRaw) -> anyhow::Result<Self> {
        let (
            name,
            state,
            last_hash,
            last_fired_at,
            last_check_at,
            last_outcome,
            last_elapsed_ms,
            holding_since,
            strikes,
            fired_count,
            session_id,
        ) = r;
        Ok(Self {
            name,
            state,
            last_hash,
            last_fired_at: last_fired_at.as_deref().map(time_of).transpose()?,
            last_check_at: last_check_at.as_deref().map(time_of).transpose()?,
            last_outcome,
            last_elapsed_ms: last_elapsed_ms.map(|m| m.max(0) as u64),
            holding_since: holding_since.as_deref().map(time_of).transpose()?,
            strikes: strikes.max(0) as u32,
            fired_count: fired_count.max(0) as u32,
            session_id: session_id.as_deref().map(session_of).transpose()?,
        })
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
        store.finish_run(first, t1, "end_turn", None).await.unwrap();
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
    async fn count_since_end_and_report() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("schedule.sqlite"))
            .await
            .unwrap();
        let s = SessionId::new();
        let t0 = Local::now() - TimeDelta::minutes(30);
        let at = |m: i64| t0 + TimeDelta::minutes(m);

        // loop: fire, fire, skip, end, fire (finished with report), fire (running)
        let r = store
            .insert_run("loop", Some(s), at(0), None)
            .await
            .unwrap();
        store.finish_run(r, at(1), "end_turn", None).await.unwrap();
        let r = store
            .insert_run("loop", Some(s), at(2), None)
            .await
            .unwrap();
        store.finish_run(r, at(3), "end_turn", None).await.unwrap();
        store
            .insert_run("loop", Some(s), at(4), Some("skipped: busy"))
            .await
            .unwrap();
        store
            .insert_run("loop", Some(s), at(5), Some("max_runs 2 reached"))
            .await
            .unwrap();
        let r = store
            .insert_run("loop", Some(s), at(6), None)
            .await
            .unwrap();
        store
            .finish_run(r, at(7), "end_turn", Some("all green"))
            .await
            .unwrap();
        store
            .insert_run("loop", Some(s), at(8), None)
            .await
            .unwrap();
        assert_eq!(store.count_since_end("loop").await.unwrap(), 2);

        let h = store.history("loop", 10).await.unwrap();
        assert_eq!(h.len(), 6);
        assert_eq!(h[0].report, None);
        assert_eq!(h[1].report.as_deref(), Some("all green"));
        assert!(h[2..].iter().all(|r| r.report.is_none()));

        // plain: no end row = every fire, no skips
        store
            .insert_run("plain", Some(s), at(0), None)
            .await
            .unwrap();
        store
            .insert_run("plain", Some(s), at(1), None)
            .await
            .unwrap();
        store
            .insert_run("plain", Some(s), at(2), Some("skipped: busy"))
            .await
            .unwrap();
        store
            .insert_run("plain", None, at(3), Some("skipped: busy"))
            .await
            .unwrap();
        assert_eq!(store.count_since_end("plain").await.unwrap(), 2);
        assert_eq!(store.count_since_end("nothing").await.unwrap(), 0);
    }

    /// 108 §9.3: `kind` is derived on read — a fire is `run`, a gate row is
    /// `check`, an end string is `end` whether or not a session is attached,
    /// and `skipped: …` is `skip` even with a session.
    #[tokio::test]
    async fn history_rows_carry_their_kind() {
        use crate::RunKind;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("schedule.sqlite"))
            .await
            .unwrap();
        let s = SessionId::new();
        let at = |i: i64| Local::now() + chrono::Duration::seconds(i);
        let rows: [(Option<SessionId>, Option<&str>, RunKind); 9] = [
            (Some(s), None, RunKind::Run),
            (None, Some("quiet"), RunKind::Check),
            (None, Some("holding: 2m of 10m"), RunKind::Check),
            (
                None,
                Some("check broken: timeout after 20s"),
                RunKind::Check,
            ),
            (Some(s), Some("skipped: busy"), RunKind::Skip),
            (None, Some("skipped: check still running"), RunKind::Skip),
            (
                Some(s),
                Some("done: until passed after 4 runs"),
                RunKind::End,
            ),
            (None, Some("check broken ×3, disabled"), RunKind::End),
            (Some(s), Some("max_runs 20 reached"), RunKind::End),
        ];
        for (i, (session, result, _)) in rows.iter().enumerate() {
            let id = store
                .insert_run("w", *session, at(i as i64), *result)
                .await
                .unwrap();
            if result.is_none() {
                store
                    .finish_run(id, at(i as i64), "end_turn", None)
                    .await
                    .unwrap();
            }
        }
        let h = store.history("w", 20).await.unwrap();
        let mut kinds: Vec<RunKind> = h.iter().map(|r| r.kind).collect();
        kinds.reverse();
        assert_eq!(kinds, rows.map(|(_, _, k)| k));
        assert_eq!(RunKind::of(true, Some("expired")), RunKind::End);
        assert_eq!(RunKind::of(true, Some("removed")), RunKind::End);
        assert_eq!(
            RunKind::of(true, Some("until broken: exit 2: x")),
            RunKind::End
        );
        assert_eq!(RunKind::of(true, Some("lost")), RunKind::Run);
        assert_eq!(RunKind::of(true, Some("error: boom")), RunKind::Run);
        assert_eq!(
            RunKind::of(false, Some("queued (2 coalesced)")),
            RunKind::Check
        );
        assert_eq!(RunKind::of(false, Some("task 7f3a")), RunKind::Run);
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

    /// 109 §4.6 step 12: a run whose session is being resumed stays open
    /// and comes back as an `OpenRun`; the other orphan is `lost`.
    #[tokio::test]
    async fn reopen_spares_runs_of_resuming_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedule.sqlite");
        let store = Store::open(&path).await.unwrap();
        let (resuming, dead) = (SessionId::new(), SessionId::new());
        let fired = Local::now();
        let id = store
            .insert_run("a", Some(resuming), fired, None)
            .await
            .unwrap();
        store
            .insert_run("b", Some(dead), fired, None)
            .await
            .unwrap();
        drop(store);

        let (store, kept) = Store::open_sparing(&path, &HashSet::from([resuming]))
            .await
            .unwrap();
        assert_eq!(
            kept,
            [OpenRun {
                id,
                name: "a".into(),
                session: resuming,
                fired_at: fired,
            }]
        );
        let a = store.history("a", 10).await.unwrap();
        assert!(
            a[0].result.is_none() && a[0].finished_at.is_none(),
            "still open"
        );
        assert_eq!(
            store.history("b", 10).await.unwrap()[0].result.as_deref(),
            Some("lost")
        );
        // Closed normally later, with the resumed turn's stop reason.
        store
            .finish_run(id, Local::now(), "end_turn", None)
            .await
            .unwrap();
        assert_eq!(
            store.latest().await.unwrap()["a"].last_result.as_deref(),
            Some("end_turn")
        );
    }

    #[tokio::test]
    async fn watch_rows_and_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("schedule.sqlite"))
            .await
            .unwrap();
        assert!(store.watch_get("w").await.unwrap().is_none());
        assert_eq!(store.watch_strike("w").await.unwrap(), 1);
        assert_eq!(store.watch_strike("w").await.unwrap(), 2);
        let t = Local::now();
        store.watch_last_check("w", t, "quiet", 1200).await.unwrap();
        let w = store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.strikes, w.state.as_str(), w.last_outcome.as_deref()),
            (2, "{}", Some("quiet"))
        );
        assert_eq!((w.last_elapsed_ms, w.last_check_at), (Some(1200), Some(t)));
        store
            .watch_commit("w", r#"{"k":1}"#, "abc", t)
            .await
            .unwrap();
        store
            .watch_commit("w", r#"{"k":2}"#, "def", t)
            .await
            .unwrap();
        let w = store.watch_get("w").await.unwrap().unwrap();
        assert_eq!(
            (w.strikes, w.fired_count, w.last_hash.as_deref()),
            (0, 2, Some("def"))
        );
        assert_eq!((w.state.as_str(), w.last_fired_at), (r#"{"k":2}"#, Some(t)));
        let put = WatchRow {
            holding_since: Some(t),
            session_id: Some(SessionId::new()),
            ..w.clone()
        };
        store.watch_put(&put).await.unwrap();
        assert_eq!(store.watch_get("w").await.unwrap().unwrap(), put);
        store.watch_delete("w").await.unwrap();
        assert!(store.watch_get("w").await.unwrap().is_none());

        store
            .insert_run_with("w", None, t, Some("fired"), Some("subject=pr/1 stage=x"))
            .await
            .unwrap();
        let h = store.history("w", 1).await.unwrap();
        assert_eq!(h[0].envelope.as_deref(), Some("subject=pr/1 stage=x"));
    }
}
