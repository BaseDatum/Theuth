//! SQLite persistence for Deck metadata, review-gated script revisions, durable snapshots,
//! cooperative state checkpoints, run status/staleness, and action idempotency.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use runtime::SessionId;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::model::{
    DeckSnapshot, DeclaredAction, InteractErrorCode, InteractResponse, ScriptMode, ScriptRevision,
    ScriptStatus,
};

/// Durable Deck record tying a `DocKind::Deck` doc ID to its dedicated persisted `SessionId`.
#[derive(Clone, Debug)]
pub struct DeckRecord {
    pub deck_id: String,
    pub session_id: SessionId,
    pub active_rev: Option<u64>,
    pub pending_rev: Option<u64>,
    pub schedule_cron: Option<String>,
    pub schedule_enabled: bool,
    pub stream_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Durable cooperative state + `.monty` checkpoint metadata for warm restore vs cold fallback.
#[derive(Clone, Debug)]
pub struct StateCheckpoint {
    pub deck_id: String,
    pub script_rev: u64,
    pub code_sha256: String,
    pub state: Value,
    pub monty_dump_rev: Option<u64>,
    pub monty_dump_sha256: Option<String>,
    pub updated_at: String,
}

/// Durable run health / staleness record.
#[derive(Clone, Debug, Default)]
pub struct RunLogRecord {
    pub last_ok_snapshot_id: Option<String>,
    pub last_ok_at: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub stale: bool,
    pub consecutive_failures: u32,
}

/// SQLite-backed store under `<data_dir>/decks/decks.sqlite`.
#[derive(Clone)]
pub struct DeckStore {
    pool: SqlitePool,
    root: PathBuf,
}

pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

impl DeckStore {
    pub async fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("decks");
        std::fs::create_dir_all(root.join("runtime"))
            .with_context(|| format!("create {}", root.display()))?;
        std::fs::create_dir_all(root.join("spill"))
            .with_context(|| format!("create {}", root.join("spill").display()))?;
        let db_path = root.join("decks.sqlite");
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", db_path.display()))?;

        let store = Self { pool, root };
        store.migrate().await?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn spill_dir(&self) -> PathBuf {
        self.root.join("spill")
    }

    pub fn monty_dump_path(&self, session: SessionId) -> PathBuf {
        self.runtime_dir().join(format!("{session}.monty"))
    }

    async fn migrate(&self) -> Result<()> {
        let ddl = [
            "CREATE TABLE IF NOT EXISTS deck_meta (
                deck_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL UNIQUE,
                active_rev INTEGER,
                pending_rev INTEGER,
                schedule_cron TEXT,
                schedule_enabled INTEGER NOT NULL DEFAULT 0,
                stream_enabled INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS deck_script_revisions (
                deck_id TEXT NOT NULL,
                rev INTEGER NOT NULL,
                code TEXT NOT NULL,
                code_sha256 TEXT NOT NULL,
                declared_actions_json TEXT NOT NULL,
                mode TEXT NOT NULL,
                interval_secs INTEGER,
                schedule_cron TEXT,
                status TEXT NOT NULL,
                proposed_by TEXT NOT NULL,
                approved_by TEXT,
                approved_at TEXT,
                rejected_reason TEXT,
                change_summary TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, rev)
            )",
            "CREATE TABLE IF NOT EXISTS deck_snapshots (
                snapshot_id TEXT PRIMARY KEY,
                deck_id TEXT NOT NULL,
                script_rev INTEGER NOT NULL,
                run_epoch INTEGER NOT NULL,
                seq INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                section TEXT,
                created_at TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_deck_snapshots_deck_created
                ON deck_snapshots (deck_id, created_at DESC)",
            "CREATE TABLE IF NOT EXISTS deck_state_checkpoints (
                deck_id TEXT PRIMARY KEY,
                script_rev INTEGER NOT NULL,
                code_sha256 TEXT NOT NULL,
                state_json TEXT NOT NULL,
                monty_dump_rev INTEGER,
                monty_dump_sha256 TEXT,
                updated_at TEXT NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS deck_run_log (
                deck_id TEXT PRIMARY KEY,
                last_ok_snapshot_id TEXT,
                last_ok_at TEXT,
                last_error TEXT,
                last_error_at TEXT,
                stale INTEGER NOT NULL DEFAULT 0,
                consecutive_failures INTEGER NOT NULL DEFAULT 0
            )",
            "CREATE TABLE IF NOT EXISTS deck_action_idempotency (
                deck_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                caller_id TEXT NOT NULL,
                action_name TEXT NOT NULL,
                response_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, request_id)
            )",
        ];
        for sql in ddl {
            sqlx::query(sql).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Ensure a `deck_meta` row exists with a dedicated persisted `SessionId`.
    pub async fn ensure_deck(&self, deck_id: &str) -> Result<DeckRecord> {
        if let Some(existing) = self.get_deck(deck_id).await? {
            return Ok(existing);
        }
        let session_id = SessionId::new();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO deck_meta
             (deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, NULL, 0, 0, ?3, ?3)",
        )
        .bind(deck_id)
        .bind(session_id.0.to_string())
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_deck(deck_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("failed to ensure deck {deck_id}"))
    }

    pub async fn get_deck(&self, deck_id: &str) -> Result<Option<DeckRecord>> {
        let row = sqlx::query(
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at
             FROM deck_meta WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_deck_row).transpose()
    }

    pub async fn find_deck_by_session(&self, session: SessionId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT deck_id FROM deck_meta WHERE session_id = ?1")
            .bind(session.0.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>(0)))
    }

    pub async fn list_decks(&self) -> Result<Vec<DeckRecord>> {
        let rows = sqlx::query(
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at
             FROM deck_meta ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_deck_row).collect()
    }

    pub async fn set_stream_enabled(&self, deck_id: &str, enabled: bool) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE deck_meta SET stream_enabled = ?2, updated_at = ?3 WHERE deck_id = ?1")
            .bind(deck_id)
            .bind(i64::from(enabled))
            .bind(&now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Propose a new script revision. ALWAYS enters as `pending_review`!
    #[allow(clippy::too_many_arguments)]
    pub async fn propose_script(
        &self,
        deck_id: &str,
        code: String,
        declared_actions: Vec<DeclaredAction>,
        mode: ScriptMode,
        interval_secs: Option<u64>,
        schedule_cron: Option<String>,
        proposed_by: String,
        change_summary: String,
    ) -> Result<ScriptRevision> {
        self.ensure_deck(deck_id).await?;
        let next_rev: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(rev), 0) + 1 FROM deck_script_revisions WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .fetch_one(&self.pool)
        .await?;
        let rev = next_rev as u64;
        let code_sha256 = sha256_hex(&code);
        let actions_json = serde_json::to_string(&declared_actions)?;
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO deck_script_revisions
             (deck_id, rev, code, code_sha256, declared_actions_json, mode, interval_secs, schedule_cron,
              status, proposed_by, approved_by, approved_at, rejected_reason, change_summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_review', ?9, NULL, NULL, NULL, ?10, ?11)",
        )
        .bind(deck_id)
        .bind(next_rev)
        .bind(&code)
        .bind(&code_sha256)
        .bind(&actions_json)
        .bind(mode.as_str())
        .bind(interval_secs.map(|v| v as i64))
        .bind(&schedule_cron)
        .bind(&proposed_by)
        .bind(&change_summary)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        sqlx::query("UPDATE deck_meta SET pending_rev = ?2, updated_at = ?3 WHERE deck_id = ?1")
            .bind(deck_id)
            .bind(next_rev)
            .bind(&now)
            .execute(&self.pool)
            .await?;

        Ok(ScriptRevision {
            deck_id: deck_id.to_owned(),
            rev,
            code,
            code_sha256,
            declared_actions,
            mode,
            interval_secs,
            schedule_cron,
            status: ScriptStatus::PendingReview,
            proposed_by,
            approved_by: None,
            approved_at: None,
            rejected_reason: None,
            change_summary,
            created_at: now,
        })
    }

    /// Owner approval gate: transitions a `pending_review` revision to `approved` and supersedes any prior approved revision.
    pub async fn approve_script(
        &self,
        deck_id: &str,
        rev: u64,
        approved_by: &str,
    ) -> Result<ScriptRevision> {
        let Some(target) = self.get_script_revision(deck_id, rev).await? else {
            anyhow::bail!("script revision {rev} not found for deck {deck_id}");
        };
        if target.status == ScriptStatus::Rejected {
            anyhow::bail!("cannot approve rejected script revision {rev}");
        }
        let now = Utc::now().to_rfc3339();
        // Supersede previous approved revisions.
        sqlx::query(
            "UPDATE deck_script_revisions SET status = 'superseded'
             WHERE deck_id = ?1 AND status = 'approved' AND rev != ?2",
        )
        .bind(deck_id)
        .bind(rev as i64)
        .execute(&self.pool)
        .await?;
        // Approve target.
        sqlx::query(
            "UPDATE deck_script_revisions
             SET status = 'approved', approved_by = ?3, approved_at = ?4
             WHERE deck_id = ?1 AND rev = ?2",
        )
        .bind(deck_id)
        .bind(rev as i64)
        .bind(approved_by)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        let meta = self.ensure_deck(deck_id).await?;
        let new_pending = if meta.pending_rev == Some(rev) {
            None
        } else {
            meta.pending_rev
        };
        let schedule_enabled = target.interval_secs.is_some() || target.schedule_cron.is_some();
        sqlx::query(
            "UPDATE deck_meta
             SET active_rev = ?2, pending_rev = ?3, schedule_cron = ?4, schedule_enabled = ?5, updated_at = ?6
             WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .bind(rev as i64)
        .bind(new_pending.map(|r| r as i64))
        .bind(&target.schedule_cron)
        .bind(i64::from(schedule_enabled))
        .bind(&now)
        .execute(&self.pool)
        .await?;

        self.get_script_revision(deck_id, rev)
            .await?
            .ok_or_else(|| anyhow::anyhow!("approved revision vanished"))
    }

    pub async fn reject_script(
        &self,
        deck_id: &str,
        rev: u64,
        reason: Option<String>,
    ) -> Result<ScriptRevision> {
        let Some(target) = self.get_script_revision(deck_id, rev).await? else {
            anyhow::bail!("script revision {rev} not found for deck {deck_id}");
        };
        if target.status == ScriptStatus::Approved {
            anyhow::bail!("cannot reject active approved revision {rev}");
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE deck_script_revisions SET status = 'rejected', rejected_reason = ?3
             WHERE deck_id = ?1 AND rev = ?2",
        )
        .bind(deck_id)
        .bind(rev as i64)
        .bind(reason.as_deref().unwrap_or("rejected by owner"))
        .execute(&self.pool)
        .await?;

        let meta = self.ensure_deck(deck_id).await?;
        if meta.pending_rev == Some(rev) {
            sqlx::query(
                "UPDATE deck_meta SET pending_rev = NULL, updated_at = ?2 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        self.get_script_revision(deck_id, rev)
            .await?
            .ok_or_else(|| anyhow::anyhow!("rejected revision vanished"))
    }

    pub async fn get_script_revision(
        &self,
        deck_id: &str,
        rev: u64,
    ) -> Result<Option<ScriptRevision>> {
        let row = sqlx::query(
            "SELECT deck_id, rev, code, code_sha256, declared_actions_json, mode, interval_secs,
                    schedule_cron, status, proposed_by, approved_by, approved_at, rejected_reason,
                    change_summary, created_at
             FROM deck_script_revisions WHERE deck_id = ?1 AND rev = ?2",
        )
        .bind(deck_id)
        .bind(rev as i64)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_script_row).transpose()
    }

    pub async fn get_active_script(&self, deck_id: &str) -> Result<Option<ScriptRevision>> {
        let Some(meta) = self.get_deck(deck_id).await? else {
            return Ok(None);
        };
        let Some(rev) = meta.active_rev else {
            return Ok(None);
        };
        let script = self.get_script_revision(deck_id, rev).await?;
        Ok(script.filter(|s| s.status == ScriptStatus::Approved))
    }

    pub async fn get_pending_script(&self, deck_id: &str) -> Result<Option<ScriptRevision>> {
        let Some(meta) = self.get_deck(deck_id).await? else {
            return Ok(None);
        };
        let Some(rev) = meta.pending_rev else {
            return Ok(None);
        };
        let script = self.get_script_revision(deck_id, rev).await?;
        Ok(script.filter(|s| s.status == ScriptStatus::PendingReview))
    }

    pub async fn get_latest_script(&self, deck_id: &str) -> Result<Option<ScriptRevision>> {
        let row = sqlx::query(
            "SELECT deck_id, rev, code, code_sha256, declared_actions_json, mode, interval_secs,
                    schedule_cron, status, proposed_by, approved_by, approved_at, rejected_reason,
                    change_summary, created_at
             FROM deck_script_revisions WHERE deck_id = ?1 ORDER BY rev DESC LIMIT 1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_script_row).transpose()
    }

    /// Persist a durable last-good snapshot and clear the stale flag.
    pub async fn save_snapshot(&self, snap: &DeckSnapshot) -> Result<()> {
        let data_json = serde_json::to_string(&snap.data)?;
        sqlx::query(
            "INSERT INTO deck_snapshots
             (snapshot_id, deck_id, script_rev, run_epoch, seq, data_json, section, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&snap.snapshot_id)
        .bind(&snap.deck_id)
        .bind(snap.script_rev as i64)
        .bind(snap.run_epoch as i64)
        .bind(snap.seq as i64)
        .bind(&data_json)
        .bind(&snap.section)
        .bind(&snap.created_at)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT INTO deck_run_log
             (deck_id, last_ok_snapshot_id, last_ok_at, last_error, last_error_at, stale, consecutive_failures)
             VALUES (?1, ?2, ?3, NULL, NULL, 0, 0)
             ON CONFLICT(deck_id) DO UPDATE SET
               last_ok_snapshot_id = excluded.last_ok_snapshot_id,
               last_ok_at = excluded.last_ok_at,
               last_error = NULL,
               stale = 0,
               consecutive_failures = 0",
        )
        .bind(&snap.deck_id)
        .bind(&snap.snapshot_id)
        .bind(&snap.created_at)
        .execute(&self.pool)
        .await?;

        // Prune old snapshots beyond 100 per Deck to keep SQLite bounded.
        sqlx::query(
            "DELETE FROM deck_snapshots WHERE deck_id = ?1 AND snapshot_id NOT IN (
                SELECT snapshot_id FROM deck_snapshots WHERE deck_id = ?1 ORDER BY created_at DESC LIMIT 100
             )",
        )
        .bind(&snap.deck_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a run/snapshot failure: preserves `last_ok_snapshot_id` intact and marks `stale = true`.
    pub async fn record_failure(&self, deck_id: &str, error: &str) -> Result<RunLogRecord> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO deck_run_log
             (deck_id, last_ok_snapshot_id, last_ok_at, last_error, last_error_at, stale, consecutive_failures)
             VALUES (?1, NULL, NULL, ?2, ?3, 1, 1)
             ON CONFLICT(deck_id) DO UPDATE SET
               last_error = excluded.last_error,
               last_error_at = excluded.last_error_at,
               stale = 1,
               consecutive_failures = deck_run_log.consecutive_failures + 1",
        )
        .bind(deck_id)
        .bind(error)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_run_log(deck_id).await
    }

    pub async fn get_run_log(&self, deck_id: &str) -> Result<RunLogRecord> {
        let row = sqlx::query(
            "SELECT last_ok_snapshot_id, last_ok_at, last_error, last_error_at, stale, consecutive_failures
             FROM deck_run_log WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some(r) => RunLogRecord {
                last_ok_snapshot_id: r.get(0),
                last_ok_at: r.get(1),
                last_error: r.get(2),
                last_error_at: r.get(3),
                stale: r.get::<i64, _>(4) != 0,
                consecutive_failures: r.get::<i64, _>(5).max(0) as u32,
            },
            None => RunLogRecord::default(),
        })
    }

    pub async fn get_last_snapshot(&self, deck_id: &str) -> Result<Option<DeckSnapshot>> {
        let row = sqlx::query(
            "SELECT snapshot_id, deck_id, script_rev, run_epoch, seq, data_json, section, created_at
             FROM deck_snapshots WHERE deck_id = ?1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_snapshot_row).transpose()
    }

    pub async fn get_snapshot(
        &self,
        deck_id: &str,
        snapshot_id: &str,
    ) -> Result<Option<DeckSnapshot>> {
        let row = sqlx::query(
            "SELECT snapshot_id, deck_id, script_rev, run_epoch, seq, data_json, section, created_at
             FROM deck_snapshots WHERE deck_id = ?1 AND snapshot_id = ?2",
        )
        .bind(deck_id)
        .bind(snapshot_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_snapshot_row).transpose()
    }

    pub async fn list_snapshots(&self, deck_id: &str, limit: u32) -> Result<Vec<DeckSnapshot>> {
        let rows = sqlx::query(
            "SELECT snapshot_id, deck_id, script_rev, run_epoch, seq, data_json, section, created_at
             FROM deck_snapshots WHERE deck_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .bind(deck_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_snapshot_row).collect()
    }

    /// Persist cooperative JSON state and optional `.monty` dump compatibility stamp.
    pub async fn save_checkpoint(
        &self,
        deck_id: &str,
        script_rev: u64,
        code_sha256: &str,
        state: Option<&Value>,
        monty_dump: Option<(u64, &str)>,
    ) -> Result<()> {
        let existing = self.get_checkpoint(deck_id).await?;
        let state_json = match state {
            Some(v) => serde_json::to_string(v)?,
            None => existing
                .as_ref()
                .map(|c| serde_json::to_string(&c.state))
                .transpose()?
                .unwrap_or_else(|| "null".to_owned()),
        };
        let (dump_rev, dump_sha) = match monty_dump {
            Some((r, sha)) => (Some(r as i64), Some(sha.to_owned())),
            None => (
                existing
                    .as_ref()
                    .and_then(|c| c.monty_dump_rev.map(|r| r as i64)),
                existing.as_ref().and_then(|c| c.monty_dump_sha256.clone()),
            ),
        };
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO deck_state_checkpoints
             (deck_id, script_rev, code_sha256, state_json, monty_dump_rev, monty_dump_sha256, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(deck_id) DO UPDATE SET
               script_rev = excluded.script_rev,
               code_sha256 = excluded.code_sha256,
               state_json = excluded.state_json,
               monty_dump_rev = excluded.monty_dump_rev,
               monty_dump_sha256 = excluded.monty_dump_sha256,
               updated_at = excluded.updated_at",
        )
        .bind(deck_id)
        .bind(script_rev as i64)
        .bind(code_sha256)
        .bind(&state_json)
        .bind(dump_rev)
        .bind(&dump_sha)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_checkpoint(&self, deck_id: &str) -> Result<Option<StateCheckpoint>> {
        let row = sqlx::query(
            "SELECT deck_id, script_rev, code_sha256, state_json, monty_dump_rev, monty_dump_sha256, updated_at
             FROM deck_state_checkpoints WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let state_raw: String = r.get(3);
            let state = serde_json::from_str(&state_raw).unwrap_or(Value::Null);
            Ok(StateCheckpoint {
                deck_id: r.get(0),
                script_rev: r.get::<i64, _>(1).max(0) as u64,
                code_sha256: r.get(2),
                state,
                monty_dump_rev: r.get::<Option<i64>, _>(4).map(|v| v.max(0) as u64),
                monty_dump_sha256: r.get(5),
                updated_at: r.get(6),
            })
        })
        .transpose()
    }

    pub async fn get_idempotent_action(
        &self,
        deck_id: &str,
        request_id: &str,
    ) -> Result<Option<InteractResponse>> {
        let row = sqlx::query(
            "SELECT response_json FROM deck_action_idempotency WHERE deck_id = ?1 AND request_id = ?2",
        )
        .bind(deck_id)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let raw: String = r.get(0);
                let mut resp: InteractResponse = serde_json::from_str(&raw)?;
                resp.cached = true;
                Ok(Some(resp))
            }
            None => Ok(None),
        }
    }

    pub async fn save_idempotent_action(
        &self,
        deck_id: &str,
        caller_id: &str,
        resp: &InteractResponse,
    ) -> Result<()> {
        // Don't cache transient Busy/TimedOut errors so callers can retry safely.
        if matches!(
            resp.error_code,
            Some(
                InteractErrorCode::Busy
                    | InteractErrorCode::TimedOut
                    | InteractErrorCode::WaitingApproval
            )
        ) {
            return Ok(());
        }
        let now = Utc::now().to_rfc3339();
        let json = serde_json::to_string(resp)?;
        sqlx::query(
            "INSERT OR REPLACE INTO deck_action_idempotency
             (deck_id, request_id, caller_id, action_name, response_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(deck_id)
        .bind(&resp.request_id)
        .bind(caller_id)
        .bind(&resp.action)
        .bind(&json)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        // Keep at most 200 recent request_ids per Deck.
        sqlx::query(
            "DELETE FROM deck_action_idempotency WHERE deck_id = ?1 AND request_id NOT IN (
                SELECT request_id FROM deck_action_idempotency WHERE deck_id = ?1 ORDER BY created_at DESC LIMIT 200
             )",
        )
        .bind(deck_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn parse_deck_row(r: sqlx::sqlite::SqliteRow) -> Result<DeckRecord> {
    let sess_str: String = r.get(1);
    let session_id = SessionId(uuid::Uuid::parse_str(&sess_str)?);
    Ok(DeckRecord {
        deck_id: r.get(0),
        session_id,
        active_rev: r.get::<Option<i64>, _>(2).map(|v| v.max(0) as u64),
        pending_rev: r.get::<Option<i64>, _>(3).map(|v| v.max(0) as u64),
        schedule_cron: r.get(4),
        schedule_enabled: r.get::<i64, _>(5) != 0,
        stream_enabled: r.get::<i64, _>(6) != 0,
        created_at: r.get(7),
        updated_at: r.get(8),
    })
}

fn parse_script_row(r: sqlx::sqlite::SqliteRow) -> Result<ScriptRevision> {
    let actions_raw: String = r.get(4);
    let declared_actions: Vec<DeclaredAction> = serde_json::from_str(&actions_raw)?;
    Ok(ScriptRevision {
        deck_id: r.get(0),
        rev: r.get::<i64, _>(1).max(0) as u64,
        code: r.get(2),
        code_sha256: r.get(3),
        declared_actions,
        mode: ScriptMode::parse(&r.get::<String, _>(5)),
        interval_secs: r.get::<Option<i64>, _>(6).map(|v| v.max(0) as u64),
        schedule_cron: r.get(7),
        status: ScriptStatus::parse(&r.get::<String, _>(8)),
        proposed_by: r.get(9),
        approved_by: r.get(10),
        approved_at: r.get(11),
        rejected_reason: r.get(12),
        change_summary: r.get(13),
        created_at: r.get(14),
    })
}

fn parse_snapshot_row(r: sqlx::sqlite::SqliteRow) -> Result<DeckSnapshot> {
    let data_raw: String = r.get(5);
    let data: Value = serde_json::from_str(&data_raw)?;
    Ok(DeckSnapshot {
        snapshot_id: r.get(0),
        deck_id: r.get(1),
        script_rev: r.get::<i64, _>(2).max(0) as u64,
        run_epoch: r.get::<i64, _>(3).max(0) as u64,
        seq: r.get::<i64, _>(4).max(0) as u64,
        data,
        section: r.get(6),
        created_at: r.get(7),
    })
}
