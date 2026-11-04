//! Owner: E1 — the run store (design 171 §8.4), `<data_dir>/cells/cells.sqlite`. The migration
//! list is `./migrations` (sqlx): 0001 (C, frozen: every table and column), 0002 (E1), 0003
//! (E2) — reserved files so a worker's extra statements never need a new registration.
//!
//! One row per run, every D17 audit field a column; the run's files live beside it under
//! `runs/<run>/` ([`crate::kernel::CellsPaths::run_dir`]). The record is written before
//! anything is announced (§8.7): callers commit [`Store::finish`] before publishing.

use std::path::Path;

use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};

use crate::error::{CellsError, Result};
use crate::types::{CellRun, ExitStatus, RunId, RunState};

/// Every migration of the cells store, embedded.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The run store.
#[derive(Clone, Debug)]
pub struct Store {
    /// The pool.
    pub pool: SqlitePool,
}

/// One stored run: the client view plus the columns only the engine reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub run: CellRun,
    /// `session:<uuid>` | `doc:<id>`.
    pub scope: String,
    /// `(scope, name)` key when the cell is named.
    pub lineage: Option<String>,
    /// joined | parked | woke | none.
    pub delivery: Option<String>,
    pub consumed_ms: Option<i64>,
    /// Job log path (shells).
    pub log: Option<String>,
    pub log_from: Option<i64>,
    pub log_to: Option<i64>,
    /// Digest of the cleaned output.
    pub output_sha: Option<String>,
}

fn db(e: sqlx::Error) -> CellsError {
    CellsError::Failed(format!("cells.sqlite: {e}"))
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn state_word(s: RunState) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `SELECT <every column> FROM cell_runs <rest>` as one literal (sqlx refuses dynamic SQL).
macro_rules! select {
    ($rest:literal) => {
        concat!(
            "SELECT id, scope, cell, lineage, mode, pass, kernel, interpreter, cwd, env_names, \
             presented_sha, code_sha, edited, pressed_by, state, exit, signal, secs, expect, \
             summary, output_sha, output_bytes, log, log_from, log_to, queued_ms, started_ms, \
             ended_ms, delivery, consumed_ms, typed, masked, private, name FROM cell_runs ",
            $rest
        )
    };
}

fn decode(r: &SqliteRow) -> std::result::Result<Row, sqlx::Error> {
    let parse = |col: &str| -> std::result::Result<serde_json::Value, sqlx::Error> {
        let s: String = r.try_get(col)?;
        serde_json::from_str(&s).map_err(|e| sqlx::Error::Decode(Box::new(e)))
    };
    let state: String = r.try_get("state")?;
    let exit: Option<i64> = r.try_get("exit")?;
    let signal: Option<String> = r.try_get("signal")?;
    let run = CellRun {
        id: RunId(r.try_get("id")?),
        cell: serde_json::from_value(parse("cell")?)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        mode: serde_json::from_str(&r.try_get::<String, _>("mode")?)
            .unwrap_or(crate::types::RunMode::Cell),
        pass: r.try_get("pass")?,
        kernel: serde_json::from_value(parse("kernel")?)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        interpreter: r.try_get("interpreter")?,
        cwd: r.try_get("cwd")?,
        env_names: serde_json::from_value(parse("env_names")?).unwrap_or_default(),
        presented_sha: r.try_get("presented_sha")?,
        code_sha: r.try_get("code_sha")?,
        edited: r.try_get::<i64, _>("edited")? != 0,
        pressed_by: r.try_get("pressed_by")?,
        state: serde_json::from_value(serde_json::Value::String(state)).unwrap_or(RunState::Lost),
        exit: match (exit, signal) {
            (_, Some(s)) => Some(ExitStatus::Signal(s)),
            (Some(c), None) => Some(ExitStatus::Code(c as i32)),
            (None, None) => None,
        },
        secs: r.try_get("secs")?,
        expect: r
            .try_get::<Option<String>, _>("expect")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        summary: r.try_get("summary")?,
        output_bytes: r
            .try_get::<Option<i64>, _>("output_bytes")?
            .map(|n| n.max(0) as u64),
        queued_ms: r.try_get("queued_ms")?,
        started_ms: r.try_get("started_ms")?,
        ended_ms: r.try_get("ended_ms")?,
        masked: u32::try_from(r.try_get::<i64, _>("masked")?).unwrap_or(0),
        private: r.try_get::<i64, _>("private")? != 0,
        name: r.try_get("name")?,
        input: None,
    };
    Ok(Row {
        run,
        scope: r.try_get("scope")?,
        lineage: r.try_get("lineage")?,
        delivery: r.try_get("delivery")?,
        consumed_ms: r.try_get("consumed_ms")?,
        log: r.try_get("log")?,
        log_from: r.try_get("log_from")?,
        log_to: r.try_get("log_to")?,
        output_sha: r.try_get("output_sha")?,
    })
}

impl Store {
    /// Open (create) `path` and migrate.
    pub async fn open(path: &Path) -> Result<Self, CellsError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
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

    /// Insert a new (queued) run.
    pub(crate) async fn insert(
        &self,
        run: &CellRun,
        scope: &str,
        lineage: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO cell_runs (id, scope, cell, lineage, mode, pass, kernel, interpreter, cwd, \
             env_names, presented_sha, code_sha, edited, pressed_by, state, queued_ms, private, \
             name) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run.id.as_str())
        .bind(scope)
        .bind(json(&run.cell))
        .bind(lineage)
        .bind(json(&run.mode))
        .bind(run.pass.as_deref())
        .bind(json(&run.kernel))
        .bind(&run.interpreter)
        .bind(&run.cwd)
        .bind(json(&run.env_names))
        .bind(&run.presented_sha)
        .bind(&run.code_sha)
        .bind(i64::from(run.edited))
        .bind(&run.pressed_by)
        .bind(state_word(run.state))
        .bind(run.queued_ms)
        .bind(i64::from(run.private))
        .bind(run.name.as_deref())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// One run.
    pub(crate) async fn get(&self, id: &RunId) -> Result<Option<Row>> {
        let r = sqlx::query(select!("WHERE id = ?"))
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.map(|r| decode(&r)).transpose().map_err(db)
    }

    /// A scope's runs, newest first.
    pub(crate) async fn runs(&self, scope: &str, limit: u32) -> Result<Vec<Row>> {
        self.rows(
            sqlx::query(select!(
                "WHERE scope = ? ORDER BY queued_ms DESC, id DESC LIMIT ?"
            ))
            .bind(scope)
            .bind(i64::from(limit.clamp(1, 1000))),
        )
        .await
    }

    /// The latest run of one cell (by its `CellRef` JSON).
    pub(crate) async fn latest_of_cell(&self, cell_json: &str) -> Result<Option<Row>> {
        let r = sqlx::query(select!(
            "WHERE cell = ? ORDER BY queued_ms DESC, id DESC LIMIT 1"
        ))
        .bind(cell_json)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        r.map(|r| decode(&r)).transpose().map_err(db)
    }

    /// The latest FINISHED run of a lineage (an `after` gate's candidate, §7.6).
    pub(crate) async fn latest_finished_of_lineage(&self, lineage: &str) -> Result<Option<Row>> {
        let r = sqlx::query(select!(
            "WHERE lineage = ? AND ended_ms IS NOT NULL ORDER BY ended_ms DESC LIMIT 1"
        ))
        .bind(lineage)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        r.map(|r| decode(&r)).transpose().map_err(db)
    }

    /// Rows a previous engine left `queued` or `running` (the restart sweep, §5.3).
    pub(crate) async fn unsettled(&self) -> Result<Vec<Row>> {
        self.rows(sqlx::query(select!(
            "WHERE state IN ('queued', 'running') ORDER BY queued_ms"
        )))
        .await
    }

    /// Parked results not yet consumed by a turn (§9.3), oldest first.
    pub(crate) async fn parked(&self, scope: Option<&str>) -> Result<Vec<Row>> {
        self.rows(
            sqlx::query(select!(
                "WHERE delivery = 'parked' AND consumed_ms IS NULL AND (? IS NULL OR scope = ?) \
                 ORDER BY ended_ms"
            ))
            .bind(scope)
            .bind(scope),
        )
        .await
    }

    async fn rows<'q>(
        &self,
        q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
    ) -> Result<Vec<Row>> {
        q.fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(decode)
            .collect::<std::result::Result<_, _>>()
            .map_err(db)
    }

    /// Mark C arrived: `running`.
    pub(crate) async fn started(
        &self,
        id: &RunId,
        at_ms: i64,
        log: Option<&str>,
        log_from: Option<u64>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE cell_runs SET state = 'running', started_ms = ?, log = ?, log_from = ? WHERE id = ?",
        )
        .bind(at_ms)
        .bind(log)
        .bind(log_from.map(|n| n as i64))
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// The run's end: every result column at once (§8.7 step 1).
    pub(crate) async fn finish(
        &self,
        run: &CellRun,
        output_sha: Option<&str>,
        log_to: Option<u64>,
    ) -> Result<()> {
        let (exit, signal) = match &run.exit {
            Some(ExitStatus::Code(c)) => (Some(i64::from(*c)), None),
            Some(ExitStatus::Signal(s)) => (None, Some(s.clone())),
            None => (None, None),
        };
        sqlx::query(
            "UPDATE cell_runs SET state = ?, exit = ?, signal = ?, secs = ?, expect = ?, summary = ?, \
             output_sha = ?, output_bytes = ?, log_to = ?, started_ms = COALESCE(started_ms, ?), \
             ended_ms = ?, masked = ? WHERE id = ?",
        )
        .bind(state_word(run.state))
        .bind(exit)
        .bind(signal)
        .bind(run.secs)
        .bind(json(&run.expect))
        .bind(run.summary.as_deref())
        .bind(output_sha)
        .bind(run.output_bytes.map(|n| n as i64))
        .bind(log_to.map(|n| n as i64))
        .bind(run.started_ms)
        .bind(run.ended_ms)
        .bind(i64::from(run.masked))
        .bind(run.id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// `delivery` (joined | parked | woke | none).
    pub(crate) async fn delivery(&self, id: &RunId, how: &str) -> Result<()> {
        sqlx::query("UPDATE cell_runs SET delivery = ? WHERE id = ?")
            .bind(how)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// A turn opened in `scope` whose text carries `run="<id>"` for these ids: those parked
    /// results joined it (§9.3). Scoped / peer-opened turns carry none, so nothing is
    /// consumed by them.
    pub(crate) async fn consumed(&self, scope: &str, ids: &[RunId], at_ms: i64) -> Result<u64> {
        let mut n = 0;
        for id in ids {
            n += sqlx::query(
                "UPDATE cell_runs SET consumed_ms = ? WHERE id = ? AND scope = ? \
                 AND delivery = 'parked' AND consumed_ms IS NULL",
            )
            .bind(at_ms)
            .bind(id.as_str())
            .bind(scope)
            .execute(&self.pool)
            .await
            .map_err(db)?
            .rows_affected();
        }
        Ok(n)
    }

    /// One parked run consumed (Ask theuth woke it).
    pub(crate) async fn consume_one(&self, id: &RunId, at_ms: i64) -> Result<()> {
        sqlx::query(
            "UPDATE cell_runs SET consumed_ms = ?, delivery = 'woke' WHERE id = ? AND consumed_ms IS NULL",
        )
        .bind(at_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Keystroke counts per device (§5.10, inv. 13: counts, never contents).
    pub(crate) async fn typed(&self, id: &RunId, device: &str, keys: u64) -> Result<()> {
        let cur: Option<String> = sqlx::query("SELECT typed FROM cell_runs WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .and_then(|r| r.try_get::<Option<String>, _>("typed").ok().flatten());
        let mut map: std::collections::BTreeMap<String, u64> = cur
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        *map.entry(device.to_owned()).or_default() += keys;
        sqlx::query("UPDATE cell_runs SET typed = ? WHERE id = ?")
            .bind(json(&map))
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// 171F B9: the note `entry` of `session` was opened as.
    pub(crate) async fn reply_note(&self, session: &str, entry: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT doc FROM reply_notes WHERE session = ? AND entry = ?")
            .bind(session)
            .bind(entry)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }

    /// 171F B9: remember (or replace) the note `entry` of `session` was opened as.
    pub(crate) async fn set_reply_note(
        &self,
        session: &str,
        entry: &str,
        doc: &str,
        at_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO reply_notes(session, entry, doc, created_ms) VALUES (?, ?, ?, ?)",
        )
        .bind(session)
        .bind(entry)
        .bind(doc)
        .bind(at_ms)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// §8.4 retention: runs older than `retention_days` go, except the latest run of every
    /// cell (by lineage when named, else by `CellRef`); then, over `max_bytes` of run files,
    /// the oldest non-latest runs go first. Returns the deleted ids (the caller removes
    /// their directories).
    pub(crate) async fn expired(
        &self,
        now_ms: i64,
        retention_days: u64,
        sizes: &(dyn Fn(&RunId) -> u64 + Sync),
        max_bytes: u64,
    ) -> Result<Vec<RunId>> {
        // Non-latest finished runs, oldest first.
        let rows: Vec<(String, i64)> = sqlx::query(
            "SELECT id, COALESCE(ended_ms, queued_ms) AS at FROM cell_runs r \
             WHERE ended_ms IS NOT NULL AND id NOT IN ( \
               SELECT (SELECT id FROM cell_runs x WHERE COALESCE(x.lineage, x.cell) = k.key \
                       ORDER BY x.queued_ms DESC, x.id DESC LIMIT 1) \
               FROM (SELECT DISTINCT COALESCE(lineage, cell) AS key FROM cell_runs) k) \
             ORDER BY at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?
        .iter()
        .map(|r| Ok((r.try_get("id")?, r.try_get("at")?)))
        .collect::<std::result::Result<_, sqlx::Error>>()
        .map_err(db)?;
        let cutoff = now_ms - (retention_days as i64) * 86_400_000;
        let mut gone: Vec<RunId> = Vec::new();
        let mut keep: Vec<RunId> = Vec::new();
        for (id, at) in rows {
            if at < cutoff {
                gone.push(RunId(id));
            } else {
                keep.push(RunId(id));
            }
        }
        let all: Vec<String> = sqlx::query("SELECT id FROM cell_runs")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .filter_map(|r| r.try_get("id").ok())
            .collect();
        let mut total: u64 = all
            .iter()
            .map(|id| RunId(id.clone()))
            .filter(|id| !gone.contains(id))
            .map(|id| sizes(&id))
            .sum();
        for id in keep {
            if total <= max_bytes {
                break;
            }
            total = total.saturating_sub(sizes(&id));
            gone.push(id);
        }
        for id in &gone {
            sqlx::query("DELETE FROM cell_runs WHERE id = ?")
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db)?;
        }
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellRef, KernelId, KernelSpec, RunMode, Scope, SessionId};

    pub(crate) fn run(id: &str, ordinal: u32, queued_ms: i64) -> CellRun {
        let session = SessionId(uuid::Uuid::nil());
        CellRun {
            id: RunId(id.into()),
            cell: CellRef::Chat {
                session,
                entry: "7".into(),
                ordinal,
            },
            mode: RunMode::Cell,
            pass: None,
            kernel: KernelId {
                scope: Scope::Session(session),
                spec: KernelSpec::Monty,
            },
            interpreter: "bash 5.2 (Homebrew) /opt/homebrew/bin/bash".into(),
            cwd: "/tmp".into(),
            env_names: vec!["TOKEN".into()],
            presented_sha: "a".into(),
            code_sha: "b".into(),
            edited: false,
            pressed_by: "desktop · mac".into(),
            state: RunState::Queued,
            exit: None,
            secs: None,
            expect: Vec::new(),
            summary: None,
            output_bytes: None,
            queued_ms,
            started_ms: None,
            ended_ms: None,
            masked: 0,
            private: false,
            name: None,
            input: None,
        }
    }

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
        assert_eq!(MIGRATOR.iter().count(), 4);
    }

    /// Every D17 audit field round-trips; env stores names only.
    #[tokio::test]
    async fn audit_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.sqlite")).await.unwrap();
        let mut r = run("r_1", 1, 10);
        r.private = true;
        r.name = Some("preview".into());
        s.insert(&r, "session:x", Some("session:x/preview"))
            .await
            .unwrap();
        s.started(&r.id, 11, Some("/log"), Some(5)).await.unwrap();
        r.state = RunState::Done;
        r.exit = Some(ExitStatus::Code(0));
        r.secs = Some(1.5);
        r.summary = Some("ok".into());
        r.output_bytes = Some(42);
        r.started_ms = Some(11);
        r.ended_ms = Some(12);
        r.masked = 2;
        s.finish(&r, Some("sha"), Some(99)).await.unwrap();
        s.typed(&r.id, "desktop", 3).await.unwrap();
        s.typed(&r.id, "desktop", 2).await.unwrap();
        let got = s.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.run, r);
        assert_eq!(got.lineage.as_deref(), Some("session:x/preview"));
        assert_eq!((got.log_from, got.log_to), (Some(5), Some(99)));
        assert_eq!(got.output_sha.as_deref(), Some("sha"));
        let typed: (Option<String>,) = sqlx::query_as("SELECT typed FROM cell_runs")
            .fetch_one(&s.pool)
            .await
            .unwrap();
        assert_eq!(typed.0.as_deref(), Some(r#"{"desktop":5}"#));
    }

    /// §9.3 (A1): a turn consumes exactly the parked runs its text carries, in its own scope.
    #[tokio::test]
    async fn parked_runs_are_consumed_only_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.sqlite")).await.unwrap();
        for id in ["r_a", "r_b"] {
            let mut r = run(id, 1, 1);
            s.insert(&r, "session:x", None).await.unwrap();
            r.state = RunState::Done;
            r.ended_ms = Some(2);
            s.finish(&r, None, None).await.unwrap();
            s.delivery(&r.id, "parked").await.unwrap();
        }
        let a = [RunId("r_a".into())];
        assert_eq!(s.consumed("session:other", &a, 5).await.unwrap(), 0);
        assert_eq!(s.consumed("session:x", &a, 5).await.unwrap(), 1);
        assert_eq!(s.consumed("session:x", &a, 6).await.unwrap(), 0);
        let left: Vec<RunId> = s
            .parked(None)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.run.id)
            .collect();
        assert_eq!(left, [RunId("r_b".into())]);
    }

    #[tokio::test]
    async fn retention_keeps_the_latest_run_of_every_cell() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.sqlite")).await.unwrap();
        let day = 86_400_000;
        for (id, ord, at) in [("r_a1", 1, 0), ("r_a2", 1, day), ("r_b1", 2, 0)] {
            let mut r = run(id, ord, at);
            s.insert(&r, "session:x", None).await.unwrap();
            r.state = RunState::Done;
            r.ended_ms = Some(at);
            s.finish(&r, None, None).await.unwrap();
        }
        // 40 days later: r_a1 is old and not latest; r_a2 / r_b1 are latest of their cells.
        let gone = s.expired(40 * day, 30, &|_| 1, u64::MAX).await.unwrap();
        assert_eq!(gone, vec![RunId("r_a1".into())]);
        // Size cap evicts only non-latest runs.
        let mut r = run("r_a3", 1, 41 * day);
        s.insert(&r, "session:x", None).await.unwrap();
        r.state = RunState::Done;
        r.ended_ms = Some(41 * day);
        s.finish(&r, None, None).await.unwrap();
        let gone = s.expired(41 * day, 30, &|_| 10, 0).await.unwrap();
        assert_eq!(gone, vec![RunId("r_a2".into())]);
    }

    /// 171F B9: one note per reply; a second set replaces it; other replies are apart.
    #[tokio::test]
    async fn reply_note_round_trips_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.sqlite")).await.unwrap();
        assert_eq!(s.reply_note("s1", "e1").await.unwrap(), None);
        s.set_reply_note("s1", "e1", "d1", 1).await.unwrap();
        assert_eq!(
            s.reply_note("s1", "e1").await.unwrap().as_deref(),
            Some("d1")
        );
        s.set_reply_note("s1", "e1", "d2", 2).await.unwrap();
        assert_eq!(
            s.reply_note("s1", "e1").await.unwrap().as_deref(),
            Some("d2")
        );
        assert_eq!(s.reply_note("s1", "e2").await.unwrap(), None);
        assert_eq!(s.reply_note("s2", "e1").await.unwrap(), None);
    }
}
