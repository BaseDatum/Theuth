//! SQLite persistence for Deck metadata, review-gated script revisions, durable snapshots,
//! cooperative state checkpoints, run status/staleness, and action idempotency.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use runtime::SessionId;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::mesh::{
    DeckSourceEvent, DeckSourceRegistration, DeckSourceStatus, IngestSourceAck,
    MAX_DEDUP_ENTRIES_PER_SOURCE, SourceHealthState, SourceSpoolLimits, SpoolError, SpoolStats,
    TriggerDispatchRecord, TriggerFireState, normalize_source_timeouts, validate_engine_id,
    validate_source_id,
};
use crate::model::{
    DeckSnapshot, DeclaredAction, InteractErrorCode, InteractResponse, MAX_ACTION_NAME_BYTES,
    MAX_CHANGE_SUMMARY_BYTES, MAX_DECLARED_ACTIONS, MAX_INTERVAL_SECS, MAX_SCRIPT_CODE_BYTES,
    MIN_INTERVAL_SECS, ScriptMode, ScriptRevision, ScriptStatus, TriggerSpec,
    canonical_json_string, clamp_interval_secs,
};
use crate::triggers::{SourceTriggerState, TriggerState, normalize_triggers, validate_triggers};

/// Durable Deck record tying a `DocKind::Deck` doc ID to its dedicated persisted `SessionId`
/// and authoritative home engine lease.
#[derive(Clone, Debug)]
pub struct DeckRecord {
    pub deck_id: String,
    pub session_id: SessionId,
    pub active_rev: Option<u64>,
    pub pending_rev: Option<u64>,
    pub schedule_cron: Option<String>,
    pub schedule_enabled: bool,
    pub stream_enabled: bool,
    pub run_epoch: u64,
    pub home_engine_id: Option<String>,
    pub home_epoch: u64,
    pub home_moved_at: Option<String>,
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
    /// Canonical `revision_sha256` of the script revision that produced the `.monty` dump.
    pub monty_dump_revision_sha256: Option<String>,
    /// Actual SHA-256 hex digest of the `.monty` file bytes on disk (verified before `restore`).
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

/// Result of looking up an interactive action request in the per-caller idempotency cache (229 B7).
#[derive(Clone, Debug)]
pub enum IdempotencyLookup {
    Miss,
    Hit(InteractResponse),
    Conflict(String),
}

/// SQLite-backed store under `<data_dir>/decks/decks.sqlite`.
#[derive(Clone)]
pub struct DeckStore {
    pool: SqlitePool,
    root: PathBuf,
    work_root: PathBuf,
}

pub fn sha256_bytes(input: &[u8]) -> String {
    hex::encode(Sha256::digest(input))
}

pub fn sha256_hex(input: &str) -> String {
    sha256_bytes(input.as_bytes())
}

/// Canonical SHA-256 digest over every executable/review-gated field of a script revision (229 B2/B3).
///
/// Backward-compatible: preserves exact `v: 1` digest when `triggers` is empty, and uses
/// `v: 2` including normalized `triggers` when non-empty.
pub fn canonical_revision_sha256(
    code: &str,
    declared_actions: &[DeclaredAction],
    triggers: &[TriggerSpec],
    mode: ScriptMode,
    interval_secs: Option<u64>,
    schedule_cron: Option<&str>,
) -> String {
    let payload = if triggers.is_empty() {
        serde_json::json!({
            "v": 1,
            "code": code,
            "declared_actions": declared_actions,
            "mode": mode.as_str(),
            "interval_secs": interval_secs,
            "schedule_cron": schedule_cron,
        })
    } else {
        serde_json::json!({
            "v": 2,
            "code": code,
            "declared_actions": declared_actions,
            "triggers": normalize_triggers(triggers),
            "mode": mode.as_str(),
            "interval_secs": interval_secs,
            "schedule_cron": schedule_cron,
        })
    };
    sha256_hex(&canonical_json_string(&payload))
}

/// Validate safety caps on a proposed script revision (229 H4).
pub fn validate_script_proposal(
    code: &str,
    declared_actions: &[DeclaredAction],
    triggers: &[TriggerSpec],
    change_summary: &str,
) -> Result<()> {
    if code.len() > MAX_SCRIPT_CODE_BYTES {
        anyhow::bail!(
            "script code ({} bytes) exceeds cap ({MAX_SCRIPT_CODE_BYTES} bytes)",
            code.len()
        );
    }
    if declared_actions.len() > MAX_DECLARED_ACTIONS {
        anyhow::bail!(
            "declared_actions count ({}) exceeds cap ({MAX_DECLARED_ACTIONS})",
            declared_actions.len()
        );
    }
    if change_summary.len() > MAX_CHANGE_SUMMARY_BYTES {
        anyhow::bail!(
            "change_summary ({} bytes) exceeds cap ({MAX_CHANGE_SUMMARY_BYTES} bytes)",
            change_summary.len()
        );
    }
    let mut seen = HashSet::with_capacity(declared_actions.len());
    for action in declared_actions {
        let name = action.name.trim();
        if name.is_empty() || name != action.name {
            anyhow::bail!("declared action name must be non-empty without surrounding whitespace");
        }
        if name.len() > MAX_ACTION_NAME_BYTES {
            anyhow::bail!(
                "declared action name `{name}` ({} bytes) exceeds cap ({MAX_ACTION_NAME_BYTES} bytes)",
                name.len()
            );
        }
        if !seen.insert(name) {
            anyhow::bail!("duplicate declared action name `{name}`");
        }
    }
    validate_triggers(triggers)?;
    Ok(())
}

impl DeckStore {
    pub async fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("decks");
        let work_root = data_dir.join("deck-work");
        std::fs::create_dir_all(root.join("runtime"))
            .with_context(|| format!("create {}", root.display()))?;
        std::fs::create_dir_all(root.join("spill"))
            .with_context(|| format!("create {}", root.join("spill").display()))?;
        std::fs::create_dir_all(&work_root)
            .with_context(|| format!("create {}", work_root.display()))?;
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

        let store = Self {
            pool,
            root,
            work_root,
        };
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

    /// Read-only path to a Deck's isolated working directory without creating it (used on teardown).
    pub fn deck_work_dir_path(&self, deck_id: &str) -> Result<PathBuf> {
        let safe_id = sanitize_deck_dir_name(deck_id)?;
        Ok(self.work_root.join(safe_id))
    }

    /// Isolated per-Deck working directory under `<data_dir>/deck-work/<deck_id>`,
    /// strictly outside `<data_dir>/decks` (which holds protected engine state) and
    /// never `<data_dir>` or an ancestor of `<data_dir>` (229 H1).
    pub fn deck_work_dir(&self, deck_id: &str) -> Result<PathBuf> {
        let dir = self.deck_work_dir_path(deck_id)?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create deck work dir {}", dir.display()))?;
        let canon = std::fs::canonicalize(&dir).unwrap_or(dir);
        Ok(canon)
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
                run_epoch INTEGER NOT NULL DEFAULT 0,
                home_engine_id TEXT,
                home_epoch INTEGER NOT NULL DEFAULT 1,
                home_moved_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS deck_script_revisions (
                deck_id TEXT NOT NULL,
                rev INTEGER NOT NULL,
                code TEXT NOT NULL,
                code_sha256 TEXT NOT NULL,
                revision_sha256 TEXT NOT NULL DEFAULT '',
                declared_actions_json TEXT NOT NULL,
                triggers_json TEXT NOT NULL DEFAULT '[]',
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
            "CREATE INDEX IF NOT EXISTS idx_deck_snapshots_deck_epoch_seq
                ON deck_snapshots (deck_id, run_epoch DESC, seq DESC)",
            "CREATE TABLE IF NOT EXISTS deck_state_checkpoints (
                deck_id TEXT PRIMARY KEY,
                script_rev INTEGER NOT NULL,
                code_sha256 TEXT NOT NULL,
                state_json TEXT NOT NULL,
                monty_dump_rev INTEGER,
                monty_dump_revision_sha256 TEXT,
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
                caller_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                action_name TEXT NOT NULL,
                args_sha256 TEXT NOT NULL,
                script_rev INTEGER NOT NULL,
                response_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, caller_id, request_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_trigger_state (
                deck_id TEXT NOT NULL,
                trigger_id TEXT NOT NULL,
                script_rev INTEGER NOT NULL,
                spec_sha256 TEXT NOT NULL,
                first_match_at_ms INTEGER,
                consecutive_matches INTEGER NOT NULL DEFAULT 0,
                latched INTEGER NOT NULL DEFAULT 0,
                last_fired_at_ms INTEGER,
                episode_seq INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, trigger_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_source_trigger_state (
                deck_id TEXT NOT NULL,
                trigger_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                script_rev INTEGER NOT NULL,
                spec_sha256 TEXT NOT NULL,
                source_epoch INTEGER NOT NULL DEFAULT 0,
                last_seq INTEGER NOT NULL DEFAULT 0,
                first_match_at_ms INTEGER,
                consecutive_matches INTEGER NOT NULL DEFAULT 0,
                latched INTEGER NOT NULL DEFAULT 0,
                last_fired_at_ms INTEGER,
                episode_seq INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, trigger_id, source_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_trigger_fires (
                dedup_key TEXT PRIMARY KEY,
                deck_id TEXT NOT NULL,
                trigger_id TEXT NOT NULL,
                script_rev INTEGER NOT NULL,
                routine TEXT NOT NULL,
                fired_at_ms INTEGER NOT NULL,
                target TEXT,
                created_at TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_deck_trigger_fires_deck_fired
                ON deck_trigger_fires (deck_id, fired_at_ms DESC, dedup_key DESC)",
            "CREATE TABLE IF NOT EXISTS deck_sources (
                deck_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                engine_id TEXT NOT NULL,
                label TEXT NOT NULL,
                heartbeat_secs INTEGER NOT NULL DEFAULT 30,
                stale_after_secs INTEGER NOT NULL DEFAULT 90,
                offline_after_secs INTEGER NOT NULL DEFAULT 300,
                enabled INTEGER NOT NULL DEFAULT 1,
                registered_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, source_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_source_state (
                deck_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                last_epoch INTEGER NOT NULL DEFAULT 0,
                last_seq INTEGER NOT NULL DEFAULT 0,
                last_live_seq INTEGER NOT NULL DEFAULT 0,
                last_observed_at TEXT,
                last_received_at TEXT,
                last_live_received_at TEXT,
                last_replay_at TEXT,
                replaying INTEGER NOT NULL DEFAULT 0,
                health TEXT NOT NULL DEFAULT 'offline',
                lag_ms INTEGER,
                dropped_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_payload_json TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, source_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_source_dedup (
                deck_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                source_epoch INTEGER NOT NULL,
                seq INTEGER NOT NULL,
                result_json TEXT NOT NULL,
                received_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, source_id, source_epoch, seq)
            )",
            "CREATE TABLE IF NOT EXISTS deck_trigger_fire_state (
                deck_id TEXT NOT NULL,
                trigger_key TEXT NOT NULL,
                source_id TEXT NOT NULL,
                source_epoch INTEGER NOT NULL DEFAULT 0,
                last_seq INTEGER NOT NULL DEFAULT 0,
                sustained_since_ms INTEGER,
                fired_count INTEGER NOT NULL DEFAULT 0,
                last_fired_at TEXT,
                last_dispatch_dedup_key TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, trigger_key)
            )",
            "CREATE TABLE IF NOT EXISTS deck_source_local_meta (
                deck_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                source_epoch INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, source_id)
            )",
            "CREATE TABLE IF NOT EXISTS deck_source_spool (
                deck_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                source_epoch INTEGER NOT NULL,
                seq INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                payload_bytes INTEGER NOT NULL,
                enqueued_at TEXT NOT NULL,
                PRIMARY KEY (deck_id, source_id, source_epoch, seq)
            )",
            "CREATE INDEX IF NOT EXISTS idx_deck_source_spool_order
                ON deck_source_spool (deck_id, source_id, source_epoch ASC, seq ASC)",
        ];
        for sql in ddl {
            sqlx::query(sql).execute(&self.pool).await?;
        }

        // Additive migrations for pre-hardening / pre-mesh SQLite databases.
        let _ =
            sqlx::query("ALTER TABLE deck_meta ADD COLUMN run_epoch INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await;
        let _ = sqlx::query("ALTER TABLE deck_meta ADD COLUMN home_engine_id TEXT")
            .execute(&self.pool)
            .await;
        let _ =
            sqlx::query("ALTER TABLE deck_meta ADD COLUMN home_epoch INTEGER NOT NULL DEFAULT 1")
                .execute(&self.pool)
                .await;
        let _ = sqlx::query("ALTER TABLE deck_meta ADD COLUMN home_moved_at TEXT")
            .execute(&self.pool)
            .await;
        // Backfill run_epoch floor from existing snapshots so post-restart epochs stay monotonic.
        let _ = sqlx::query(
            "UPDATE deck_meta
             SET run_epoch = COALESCE(
                 (SELECT MAX(run_epoch) FROM deck_snapshots WHERE deck_snapshots.deck_id = deck_meta.deck_id),
                 run_epoch
             )
             WHERE run_epoch < COALESCE(
                 (SELECT MAX(run_epoch) FROM deck_snapshots WHERE deck_snapshots.deck_id = deck_meta.deck_id),
                 0
             )",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query(
            "ALTER TABLE deck_script_revisions ADD COLUMN revision_sha256 TEXT NOT NULL DEFAULT ''",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query(
            "ALTER TABLE deck_script_revisions ADD COLUMN triggers_json TEXT NOT NULL DEFAULT '[]'",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query(
            "ALTER TABLE deck_state_checkpoints ADD COLUMN monty_dump_revision_sha256 TEXT",
        )
        .execute(&self.pool)
        .await;

        // Upgrade legacy deck_action_idempotency table (keyed only on deck_id + request_id)
        // to the caller-scoped schema (229 B7). It is an ephemeral response cache.
        let cols: Vec<String> = sqlx::query("PRAGMA table_info(deck_action_idempotency)")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| r.get::<String, _>(1))
            .collect();
        if !cols.iter().any(|c| c == "args_sha256") {
            sqlx::query("DROP TABLE IF EXISTS deck_action_idempotency")
                .execute(&self.pool)
                .await?;
            sqlx::query(
                "CREATE TABLE deck_action_idempotency (
                    deck_id TEXT NOT NULL,
                    caller_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    action_name TEXT NOT NULL,
                    args_sha256 TEXT NOT NULL,
                    script_rev INTEGER NOT NULL,
                    response_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (deck_id, caller_id, request_id)
                )",
            )
            .execute(&self.pool)
            .await?;
        }

        // Backfill empty revision_sha256 on legacy rows (never mutates already populated digests).
        let legacy_rows = sqlx::query(
            "SELECT deck_id, rev, code, declared_actions_json, triggers_json, mode, interval_secs, schedule_cron
             FROM deck_script_revisions WHERE revision_sha256 = ''",
        )
        .fetch_all(&self.pool)
        .await?;
        for r in legacy_rows {
            let deck_id: String = r.get(0);
            let rev: i64 = r.get(1);
            let code: String = r.get(2);
            let actions_raw: String = r.get(3);
            let triggers_raw: String = r.get(4);
            let mode = ScriptMode::parse(&r.get::<String, _>(5));
            let interval_secs = r
                .get::<Option<i64>, _>(6)
                .map(|v| clamp_interval_secs(v.max(0) as u64));
            let schedule_cron: Option<String> = r.get(7);
            if let (Ok(actions), Ok(triggers)) = (
                serde_json::from_str::<Vec<DeclaredAction>>(&actions_raw),
                serde_json::from_str::<Vec<TriggerSpec>>(&triggers_raw),
            ) {
                let digest = canonical_revision_sha256(
                    &code,
                    &actions,
                    &triggers,
                    mode,
                    interval_secs,
                    schedule_cron.as_deref(),
                );
                let _ = sqlx::query(
                    "UPDATE deck_script_revisions SET revision_sha256 = ?3
                     WHERE deck_id = ?1 AND rev = ?2 AND revision_sha256 = ''",
                )
                .bind(&deck_id)
                .bind(rev)
                .bind(&digest)
                .execute(&self.pool)
                .await;
            }
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
             (deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, NULL, 0, 0, 0, NULL, 1, NULL, ?3, ?3)",
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
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
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
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
             FROM deck_meta ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_deck_row).collect()
    }

    /// List Decks that should auto-resume on engine startup (229 H3).
    pub async fn list_auto_resume_decks(&self) -> Result<Vec<DeckRecord>> {
        let rows = sqlx::query(
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
             FROM deck_meta
             WHERE (stream_enabled = 1 OR schedule_enabled = 1) AND active_rev IS NOT NULL
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_deck_row).collect()
    }

    /// Monotonically persist `run_epoch` so snapshot ordering/pruning stays monotonic across restarts.
    pub async fn set_run_epoch(&self, deck_id: &str, run_epoch: u64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let epoch_i64 = i64::try_from(run_epoch).unwrap_or(i64::MAX);
        sqlx::query(
            "UPDATE deck_meta
             SET run_epoch = MAX(run_epoch, ?2), updated_at = ?3
             WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .bind(epoch_i64)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
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

    pub async fn set_runtime_enabled(
        &self,
        deck_id: &str,
        stream_enabled: bool,
        schedule_enabled: bool,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE deck_meta
             SET stream_enabled = ?2, schedule_enabled = ?3, updated_at = ?4
             WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .bind(i64::from(stream_enabled))
        .bind(i64::from(schedule_enabled))
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Transactionally delete all SQLite rows for a deleted Deck (229 H2), returning its `SessionId`.
    pub async fn delete_deck_state(&self, deck_id: &str) -> Result<Option<SessionId>> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<Option<SessionId>> = async {
            let session_id = sqlx::query("SELECT session_id FROM deck_meta WHERE deck_id = ?1")
                .bind(deck_id)
                .fetch_optional(&mut *conn)
                .await?
                .and_then(|r| uuid::Uuid::parse_str(&r.get::<String, _>(0)).ok())
                .map(SessionId);

            for sql in [
                "DELETE FROM deck_trigger_fires WHERE deck_id = ?1",
                "DELETE FROM deck_source_trigger_state WHERE deck_id = ?1",
                "DELETE FROM deck_trigger_state WHERE deck_id = ?1",
                "DELETE FROM deck_source_spool WHERE deck_id = ?1",
                "DELETE FROM deck_source_local_meta WHERE deck_id = ?1",
                "DELETE FROM deck_trigger_fire_state WHERE deck_id = ?1",
                "DELETE FROM deck_source_dedup WHERE deck_id = ?1",
                "DELETE FROM deck_source_state WHERE deck_id = ?1",
                "DELETE FROM deck_sources WHERE deck_id = ?1",
                "DELETE FROM deck_action_idempotency WHERE deck_id = ?1",
                "DELETE FROM deck_run_log WHERE deck_id = ?1",
                "DELETE FROM deck_state_checkpoints WHERE deck_id = ?1",
                "DELETE FROM deck_snapshots WHERE deck_id = ?1",
                "DELETE FROM deck_script_revisions WHERE deck_id = ?1",
                "DELETE FROM deck_meta WHERE deck_id = ?1",
            ] {
                sqlx::query(sql).bind(deck_id).execute(&mut *conn).await?;
            }
            Ok(session_id)
        }
        .await;
        match res {
            Ok(sid) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(sid)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Propose a new script revision inside a `BEGIN IMMEDIATE` transaction.
    /// ALWAYS enters as `pending_review`!
    #[allow(clippy::too_many_arguments)]
    pub async fn propose_script(
        &self,
        deck_id: &str,
        code: String,
        declared_actions: Vec<DeclaredAction>,
        triggers: Vec<TriggerSpec>,
        mode: ScriptMode,
        interval_secs: Option<u64>,
        schedule_cron: Option<String>,
        proposed_by: String,
        change_summary: String,
    ) -> Result<ScriptRevision> {
        validate_script_proposal(&code, &declared_actions, &triggers, &change_summary)?;
        let triggers = normalize_triggers(&triggers);
        let interval_secs = interval_secs.map(clamp_interval_secs);
        let code_sha256 = sha256_hex(&code);
        let revision_sha256 = canonical_revision_sha256(
            &code,
            &declared_actions,
            &triggers,
            mode,
            interval_secs,
            schedule_cron.as_deref(),
        );
        let actions_json = serde_json::to_string(&declared_actions)?;
        let triggers_json = serde_json::to_string(&triggers)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<ScriptRevision> = async {
            ensure_deck_tx(&mut conn, deck_id, &now).await?;
            let next_rev: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(rev), 0) + 1 FROM deck_script_revisions WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .fetch_one(&mut *conn)
            .await?;
            let rev = next_rev as u64;

            // 229 B3: supersede any prior unreviewed pending_review revision atomically.
            sqlx::query(
                "UPDATE deck_script_revisions SET status = 'superseded'
                 WHERE deck_id = ?1 AND status = 'pending_review'",
            )
            .bind(deck_id)
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "INSERT INTO deck_script_revisions
                 (deck_id, rev, code, code_sha256, revision_sha256, declared_actions_json, triggers_json, mode,
                  interval_secs, schedule_cron, status, proposed_by, approved_by, approved_at,
                  rejected_reason, change_summary, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending_review', ?11, NULL, NULL, NULL, ?12, ?13)",
            )
            .bind(deck_id)
            .bind(next_rev)
            .bind(&code)
            .bind(&code_sha256)
            .bind(&revision_sha256)
            .bind(&actions_json)
            .bind(&triggers_json)
            .bind(mode.as_str())
            .bind(interval_secs.map(|v| v as i64))
            .bind(&schedule_cron)
            .bind(&proposed_by)
            .bind(&change_summary)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            sqlx::query("UPDATE deck_meta SET pending_rev = ?2, updated_at = ?3 WHERE deck_id = ?1")
                .bind(deck_id)
                .bind(next_rev)
                .bind(&now)
                .execute(&mut *conn)
                .await?;

            Ok(ScriptRevision {
                deck_id: deck_id.to_owned(),
                rev,
                code,
                code_sha256,
                revision_sha256,
                declared_actions,
                triggers,
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
        .await;
        match res {
            Ok(rev) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(rev)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Owner approval gate (229 B2/B3): runs inside a `BEGIN IMMEDIATE` transaction,
    /// requires `status = 'pending_review'`, verifies recomputed canonical `revision_sha256`
    /// matches both the row and `expected_digest`, and NEVER enables `schedule_enabled` or
    /// `stream_enabled` ("Approve only" stays manual until explicit owner `Start`).
    pub async fn approve_script(
        &self,
        deck_id: &str,
        rev: u64,
        expected_digest: &str,
        approved_by: &str,
    ) -> Result<ScriptRevision> {
        let expected_digest = expected_digest.trim();
        if expected_digest.is_empty() {
            anyhow::bail!("expected_digest is required to approve script revision {rev}");
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<ScriptRevision> = async {
            let Some(target) = get_script_revision_tx(&mut conn, deck_id, rev).await? else {
                anyhow::bail!("script revision {rev} not found for deck {deck_id}");
            };
            if target.status != ScriptStatus::PendingReview {
                anyhow::bail!(
                    "cannot approve script revision {rev}: status is `{}` (must be `pending_review`)",
                    target.status.as_str()
                );
            }
            if target.revision_sha256 != expected_digest {
                anyhow::bail!(
                    "script revision {rev} digest mismatch: expected {expected_digest}, found {}",
                    target.revision_sha256
                );
            }

            // Supersede any previously approved revision.
            sqlx::query(
                "UPDATE deck_script_revisions SET status = 'superseded'
                 WHERE deck_id = ?1 AND status = 'approved' AND rev != ?2",
            )
            .bind(deck_id)
            .bind(rev as i64)
            .execute(&mut *conn)
            .await?;

            // Approve target (strictly guarded on status = 'pending_review').
            let updated = sqlx::query(
                "UPDATE deck_script_revisions
                 SET status = 'approved', approved_by = ?3, approved_at = ?4
                 WHERE deck_id = ?1 AND rev = ?2 AND status = 'pending_review'",
            )
            .bind(deck_id)
            .bind(rev as i64)
            .bind(approved_by)
            .bind(&now)
            .execute(&mut *conn)
            .await?;
            if updated.rows_affected() != 1 {
                anyhow::bail!("script revision {rev} was modified concurrently before approval");
            }

            let meta = ensure_deck_tx(&mut conn, deck_id, &now).await?;
            let new_pending = if meta.pending_rev == Some(rev) {
                None
            } else {
                meta.pending_rev
            };
            // Approve-only NEVER enables schedule_enabled or stream_enabled; only explicit owner Start does.
            sqlx::query(
                "UPDATE deck_meta
                 SET active_rev = ?2, pending_rev = ?3, schedule_cron = ?4,
                     schedule_enabled = 0, stream_enabled = 0, updated_at = ?5
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(rev as i64)
            .bind(new_pending.map(|r| r as i64))
            .bind(&target.schedule_cron)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            get_script_revision_tx(&mut conn, deck_id, rev)
                .await?
                .ok_or_else(|| anyhow::anyhow!("approved revision vanished"))
        }
        .await;
        match res {
            Ok(approved) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(approved)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Reject a `pending_review` script revision inside a `BEGIN IMMEDIATE` transaction.
    pub async fn reject_script(
        &self,
        deck_id: &str,
        rev: u64,
        reason: Option<String>,
    ) -> Result<ScriptRevision> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<ScriptRevision> = async {
            let Some(target) = get_script_revision_tx(&mut conn, deck_id, rev).await? else {
                anyhow::bail!("script revision {rev} not found for deck {deck_id}");
            };
            if target.status != ScriptStatus::PendingReview {
                anyhow::bail!(
                    "cannot reject script revision {rev}: status is `{}` (must be `pending_review`)",
                    target.status.as_str()
                );
            }
            let updated = sqlx::query(
                "UPDATE deck_script_revisions SET status = 'rejected', rejected_reason = ?3
                 WHERE deck_id = ?1 AND rev = ?2 AND status = 'pending_review'",
            )
            .bind(deck_id)
            .bind(rev as i64)
            .bind(reason.as_deref().unwrap_or("rejected by owner"))
            .execute(&mut *conn)
            .await?;
            if updated.rows_affected() != 1 {
                anyhow::bail!("script revision {rev} was modified concurrently before rejection");
            }

            let meta = ensure_deck_tx(&mut conn, deck_id, &now).await?;
            if meta.pending_rev == Some(rev) {
                sqlx::query(
                    "UPDATE deck_meta SET pending_rev = NULL, updated_at = ?2 WHERE deck_id = ?1",
                )
                .bind(deck_id)
                .bind(&now)
                .execute(&mut *conn)
                .await?;
            }
            get_script_revision_tx(&mut conn, deck_id, rev)
                .await?
                .ok_or_else(|| anyhow::anyhow!("rejected revision vanished"))
        }
        .await;
        match res {
            Ok(rejected) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(rejected)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    pub async fn get_script_revision(
        &self,
        deck_id: &str,
        rev: u64,
    ) -> Result<Option<ScriptRevision>> {
        let mut conn = self.pool.acquire().await?;
        get_script_revision_tx(&mut conn, deck_id, rev).await
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
            "SELECT deck_id, rev, code, code_sha256, revision_sha256, declared_actions_json,
                    triggers_json, mode, interval_secs, schedule_cron, status, proposed_by,
                    approved_by, approved_at, rejected_reason, change_summary, created_at
             FROM deck_script_revisions WHERE deck_id = ?1 ORDER BY rev DESC LIMIT 1",
        )
        .bind(deck_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(parse_script_row).transpose()
    }

    /// Verify that `deck_id` still exists in `deck_meta` and its `active_rev` equals `expected_rev` (229 H2).
    pub async fn is_active_rev(&self, deck_id: &str, expected_rev: u64) -> Result<bool> {
        let row = sqlx::query("SELECT active_rev FROM deck_meta WHERE deck_id = ?1")
            .bind(deck_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .and_then(|r| r.get::<Option<i64>, _>(0))
            .is_some_and(|r| r >= 0 && r as u64 == expected_rev))
    }

    /// Persist a durable last-good snapshot and clear the stale flag atomically.
    pub async fn save_snapshot(&self, snap: &DeckSnapshot) -> Result<()> {
        let data_json = serde_json::to_string(&snap.data)?;
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<()> = async {
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
            .execute(&mut *conn)
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
            .execute(&mut *conn)
            .await?;

            // Prune old snapshots beyond 100 per Deck (ordered by epoch/seq/rowid, 229 L1).
            sqlx::query(
                "DELETE FROM deck_snapshots WHERE deck_id = ?1 AND snapshot_id NOT IN (
                    SELECT snapshot_id FROM deck_snapshots WHERE deck_id = ?1
                    ORDER BY run_epoch DESC, seq DESC, rowid DESC LIMIT 100
                 )",
            )
            .bind(&snap.deck_id)
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
        .await;
        match res {
            Ok(()) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(())
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
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
             FROM deck_snapshots WHERE deck_id = ?1
             ORDER BY run_epoch DESC, seq DESC, rowid DESC LIMIT 1",
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
             FROM deck_snapshots WHERE deck_id = ?1
             ORDER BY run_epoch DESC, seq DESC, rowid DESC LIMIT ?2",
        )
        .bind(deck_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(parse_snapshot_row).collect()
    }

    /// Persist cooperative JSON state and optional `.monty` dump compatibility stamp.
    /// `monty_dump` is `(script_rev, revision_sha256, dump_file_bytes_sha256)`.
    pub async fn save_checkpoint(
        &self,
        deck_id: &str,
        script_rev: u64,
        code_sha256: &str,
        state: Option<&Value>,
        monty_dump: Option<(u64, &str, &str)>,
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
        let (dump_rev, dump_rev_sha, dump_bytes_sha) = match monty_dump {
            Some((r, rev_sha, bytes_sha)) => (
                Some(r as i64),
                Some(rev_sha.to_owned()),
                Some(bytes_sha.to_owned()),
            ),
            None => (
                existing
                    .as_ref()
                    .and_then(|c| c.monty_dump_rev.map(|r| r as i64)),
                existing
                    .as_ref()
                    .and_then(|c| c.monty_dump_revision_sha256.clone()),
                existing.as_ref().and_then(|c| c.monty_dump_sha256.clone()),
            ),
        };
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO deck_state_checkpoints
             (deck_id, script_rev, code_sha256, state_json, monty_dump_rev,
              monty_dump_revision_sha256, monty_dump_sha256, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(deck_id) DO UPDATE SET
               script_rev = excluded.script_rev,
               code_sha256 = excluded.code_sha256,
               state_json = excluded.state_json,
               monty_dump_rev = excluded.monty_dump_rev,
               monty_dump_revision_sha256 = excluded.monty_dump_revision_sha256,
               monty_dump_sha256 = excluded.monty_dump_sha256,
               updated_at = excluded.updated_at",
        )
        .bind(deck_id)
        .bind(script_rev as i64)
        .bind(code_sha256)
        .bind(&state_json)
        .bind(dump_rev)
        .bind(&dump_rev_sha)
        .bind(&dump_bytes_sha)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear recorded `.monty` dump metadata (preserving JSON cooperative state) after rollover/mismatch.
    pub async fn clear_monty_dump_meta(&self, deck_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE deck_state_checkpoints
             SET monty_dump_rev = NULL, monty_dump_revision_sha256 = NULL,
                 monty_dump_sha256 = NULL, updated_at = ?2
             WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_checkpoint(&self, deck_id: &str) -> Result<Option<StateCheckpoint>> {
        let row = sqlx::query(
            "SELECT deck_id, script_rev, code_sha256, state_json, monty_dump_rev,
                    monty_dump_revision_sha256, monty_dump_sha256, updated_at
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
                monty_dump_revision_sha256: r.get(5),
                monty_dump_sha256: r.get(6),
                updated_at: r.get(7),
            })
        })
        .transpose()
    }

    /// Look up a cached action response scoped strictly to `(deck_id, caller_id, request_id)`
    /// and validated against `(action_name, args_sha256, script_rev)` (229 B7).
    pub async fn get_idempotent_action(
        &self,
        deck_id: &str,
        caller_id: &str,
        request_id: &str,
        action_name: &str,
        args_sha256: &str,
        script_rev: u64,
    ) -> Result<IdempotencyLookup> {
        let row = sqlx::query(
            "SELECT action_name, args_sha256, script_rev, response_json
             FROM deck_action_idempotency
             WHERE deck_id = ?1 AND caller_id = ?2 AND request_id = ?3",
        )
        .bind(deck_id)
        .bind(caller_id)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else {
            return Ok(IdempotencyLookup::Miss);
        };
        let stored_action: String = r.get(0);
        let stored_args_sha: String = r.get(1);
        let stored_rev = r.get::<i64, _>(2).max(0) as u64;
        if stored_action != action_name
            || stored_args_sha != args_sha256
            || stored_rev != script_rev
        {
            return Ok(IdempotencyLookup::Conflict(format!(
                "request_id `{request_id}` was already used for a different action/arguments/revision"
            )));
        }
        let raw: String = r.get(3);
        let mut resp: InteractResponse = serde_json::from_str(&raw)?;
        resp.cached = true;
        Ok(IdempotencyLookup::Hit(resp))
    }

    pub async fn save_idempotent_action(
        &self,
        deck_id: &str,
        caller_id: &str,
        args_sha256: &str,
        resp: &InteractResponse,
    ) -> Result<()> {
        // Don't cache transient/conflict/not-home errors so callers can retry safely.
        if matches!(
            resp.error_code,
            Some(
                InteractErrorCode::Busy
                    | InteractErrorCode::TimedOut
                    | InteractErrorCode::WaitingApproval
                    | InteractErrorCode::Conflict
                    | InteractErrorCode::NotHome
            )
        ) {
            return Ok(());
        }
        let now = Utc::now().to_rfc3339();
        let json = serde_json::to_string(resp)?;
        sqlx::query(
            "INSERT OR REPLACE INTO deck_action_idempotency
             (deck_id, caller_id, request_id, action_name, args_sha256, script_rev, response_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(deck_id)
        .bind(caller_id)
        .bind(&resp.request_id)
        .bind(&resp.action)
        .bind(args_sha256)
        .bind(resp.script_rev as i64)
        .bind(&json)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        // Keep at most 200 recent request_ids per (deck_id, caller_id).
        sqlx::query(
            "DELETE FROM deck_action_idempotency
             WHERE deck_id = ?1 AND caller_id = ?2 AND rowid NOT IN (
                SELECT rowid FROM deck_action_idempotency
                WHERE deck_id = ?1 AND caller_id = ?2
                ORDER BY rowid DESC LIMIT 200
             )",
        )
        .bind(deck_id)
        .bind(caller_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load persisted trigger evaluation state for one `(deck_id, trigger_id)`.
    pub async fn load_trigger_state(
        &self,
        deck_id: &str,
        trigger_id: &str,
    ) -> Result<Option<TriggerState>> {
        let row = sqlx::query(
            "SELECT deck_id, trigger_id, script_rev, spec_sha256, first_match_at_ms,
                    consecutive_matches, latched, last_fired_at_ms, episode_seq
             FROM deck_trigger_state WHERE deck_id = ?1 AND trigger_id = ?2",
        )
        .bind(deck_id)
        .bind(trigger_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| TriggerState {
            deck_id: r.get(0),
            trigger_id: r.get(1),
            script_rev: r.get::<i64, _>(2).max(0) as u64,
            spec_sha256: r.get(3),
            first_match_at_ms: r.get(4),
            consecutive_matches: r.get::<i64, _>(5).max(0) as u32,
            latched: r.get::<i64, _>(6) != 0,
            last_fired_at_ms: r.get(7),
            episode_seq: r.get::<i64, _>(8).max(0) as u64,
        }))
    }

    /// Upsert persisted trigger evaluation state.
    pub async fn save_trigger_state(&self, state: &TriggerState) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO deck_trigger_state
             (deck_id, trigger_id, script_rev, spec_sha256, first_match_at_ms,
              consecutive_matches, latched, last_fired_at_ms, episode_seq, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(deck_id, trigger_id) DO UPDATE SET
               script_rev = excluded.script_rev,
               spec_sha256 = excluded.spec_sha256,
               first_match_at_ms = excluded.first_match_at_ms,
               consecutive_matches = excluded.consecutive_matches,
               latched = excluded.latched,
               last_fired_at_ms = excluded.last_fired_at_ms,
               episode_seq = excluded.episode_seq,
               updated_at = excluded.updated_at",
        )
        .bind(&state.deck_id)
        .bind(&state.trigger_id)
        .bind(state.script_rev as i64)
        .bind(&state.spec_sha256)
        .bind(state.first_match_at_ms)
        .bind(i64::from(state.consecutive_matches))
        .bind(i64::from(state.latched))
        .bind(state.last_fired_at_ms)
        .bind(state.episode_seq as i64)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete all persisted trigger states for a Deck (e.g. when a new script revision is approved).
    pub async fn reset_trigger_states_for_deck(&self, deck_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM deck_trigger_state WHERE deck_id = ?1")
            .bind(deck_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM deck_source_trigger_state WHERE deck_id = ?1")
            .bind(deck_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove trigger states whose `trigger_id` is no longer present in the active revision.
    pub async fn prune_obsolete_trigger_states(
        &self,
        deck_id: &str,
        active_trigger_ids: &[String],
    ) -> Result<()> {
        if active_trigger_ids.is_empty() {
            return self.reset_trigger_states_for_deck(deck_id).await;
        }
        let existing: Vec<String> =
            sqlx::query_scalar("SELECT trigger_id FROM deck_trigger_state WHERE deck_id = ?1")
                .bind(deck_id)
                .fetch_all(&self.pool)
                .await?;
        for tid in existing {
            if !active_trigger_ids.iter().any(|a| a == &tid) {
                sqlx::query(
                    "DELETE FROM deck_trigger_state WHERE deck_id = ?1 AND trigger_id = ?2",
                )
                .bind(deck_id)
                .bind(&tid)
                .execute(&self.pool)
                .await?;
            }
        }
        let existing_src: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT trigger_id FROM deck_source_trigger_state WHERE deck_id = ?1",
        )
        .bind(deck_id)
        .fetch_all(&self.pool)
        .await?;
        for tid in existing_src {
            if !active_trigger_ids.iter().any(|a| a == &tid) {
                sqlx::query(
                    "DELETE FROM deck_source_trigger_state WHERE deck_id = ?1 AND trigger_id = ?2",
                )
                .bind(deck_id)
                .bind(&tid)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Load persisted per-source trigger evaluation state for one `(deck_id, trigger_id, source_id)`.
    pub async fn load_source_trigger_state(
        &self,
        deck_id: &str,
        trigger_id: &str,
        source_id: &str,
    ) -> Result<Option<SourceTriggerState>> {
        let row = sqlx::query(
            "SELECT deck_id, trigger_id, source_id, script_rev, spec_sha256, source_epoch,
                    last_seq, first_match_at_ms, consecutive_matches, latched, last_fired_at_ms,
                    episode_seq
             FROM deck_source_trigger_state
             WHERE deck_id = ?1 AND trigger_id = ?2 AND source_id = ?3",
        )
        .bind(deck_id)
        .bind(trigger_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| SourceTriggerState {
            state: TriggerState {
                deck_id: r.get(0),
                trigger_id: r.get(1),
                script_rev: r.get::<i64, _>(3).max(0) as u64,
                spec_sha256: r.get(4),
                first_match_at_ms: r.get(7),
                consecutive_matches: r.get::<i64, _>(8).max(0) as u32,
                latched: r.get::<i64, _>(9) != 0,
                last_fired_at_ms: r.get(10),
                episode_seq: r.get::<i64, _>(11).max(0) as u64,
            },
            source_id: r.get(2),
            source_epoch: r.get::<i64, _>(5).max(0) as u64,
            last_seq: r.get::<i64, _>(6).max(0) as u64,
        }))
    }

    /// Upsert persisted per-source trigger evaluation state.
    pub async fn save_source_trigger_state(&self, src: &SourceTriggerState) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO deck_source_trigger_state
             (deck_id, trigger_id, source_id, script_rev, spec_sha256, source_epoch, last_seq,
              first_match_at_ms, consecutive_matches, latched, last_fired_at_ms, episode_seq,
              updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(deck_id, trigger_id, source_id) DO UPDATE SET
               script_rev = excluded.script_rev,
               spec_sha256 = excluded.spec_sha256,
               source_epoch = excluded.source_epoch,
               last_seq = excluded.last_seq,
               first_match_at_ms = excluded.first_match_at_ms,
               consecutive_matches = excluded.consecutive_matches,
               latched = excluded.latched,
               last_fired_at_ms = excluded.last_fired_at_ms,
               episode_seq = excluded.episode_seq,
               updated_at = excluded.updated_at",
        )
        .bind(&src.state.deck_id)
        .bind(&src.state.trigger_id)
        .bind(&src.source_id)
        .bind(src.state.script_rev as i64)
        .bind(&src.state.spec_sha256)
        .bind(src.source_epoch as i64)
        .bind(src.last_seq as i64)
        .bind(src.state.first_match_at_ms)
        .bind(src.state.consecutive_matches as i64)
        .bind(i64::from(src.state.latched))
        .bind(src.state.last_fired_at_ms)
        .bind(src.state.episode_seq as i64)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear in-progress sustained windows for a source (e.g. after a stale/offline/replay gap),
    /// preserving latch, cooldown, and deduplicated fire history.
    pub async fn clear_source_trigger_streaks(
        &self,
        deck_id: &str,
        source_id: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE deck_source_trigger_state
             SET first_match_at_ms = NULL, consecutive_matches = 0, updated_at = ?3
             WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE deck_trigger_fire_state
             SET sustained_since_ms = NULL, updated_at = ?3
             WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically reserve a trigger fire dedup key before dispatching to `schedule::FireTrigger`.
    /// Returns `Ok(true)` if reserved (first fire), or `Ok(false)` if already recorded (deduped).
    pub async fn reserve_trigger_fire(
        &self,
        dedup_key: &str,
        deck_id: &str,
        trigger_id: &str,
        script_rev: u64,
        routine: &str,
        fired_at_ms: i64,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "INSERT OR IGNORE INTO deck_trigger_fires
             (dedup_key, deck_id, trigger_id, script_rev, routine, fired_at_ms, target, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        )
        .bind(dedup_key)
        .bind(deck_id)
        .bind(trigger_id)
        .bind(script_rev as i64)
        .bind(routine)
        .bind(fired_at_ms)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Mark a reserved trigger fire with its dispatched target and prune history to 500 rows/Deck.
    pub async fn finalize_trigger_fire(
        &self,
        dedup_key: &str,
        deck_id: &str,
        target: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE deck_trigger_fires SET target = ?2 WHERE dedup_key = ?1")
            .bind(dedup_key)
            .bind(target)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "DELETE FROM deck_trigger_fires
             WHERE deck_id = ?1 AND rowid NOT IN (
                SELECT rowid FROM deck_trigger_fires
                WHERE deck_id = ?1
                ORDER BY fired_at_ms DESC, rowid DESC LIMIT 500
             )",
        )
        .bind(deck_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Native Mesh Decks: home ownership, sources, dedup/fencing, liveness,
    // sustained trigger state, and bounded offline spool
    // -----------------------------------------------------------------------

    /// Initialize `home_engine_id` if it has not been persisted yet.
    pub async fn set_home_if_unset(&self, deck_id: &str, engine_id: &str) -> Result<DeckRecord> {
        let engine_id = validate_engine_id(engine_id).map_err(|e| anyhow::anyhow!(e))?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE deck_meta
             SET home_engine_id = ?2, updated_at = ?3
             WHERE deck_id = ?1 AND home_engine_id IS NULL",
        )
        .bind(deck_id)
        .bind(&engine_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_deck(deck_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("deck `{deck_id}` not found"))
    }

    /// Explicitly move a Deck's authoritative home lease to `target_engine_id`.
    ///
    /// Atomically bumps `home_epoch` + `run_epoch`, disables auto-resume (`stream_enabled = 0`,
    /// `schedule_enabled = 0`), clears `.monty` warm-restore metadata for mandatory cold start,
    /// and clears in-flight sustained trigger windows while preserving dispatch dedup keys.
    pub async fn move_deck_home(
        &self,
        deck_id: &str,
        target_engine_id: &str,
    ) -> Result<(Option<String>, DeckRecord)> {
        let target = validate_engine_id(target_engine_id).map_err(|e| anyhow::anyhow!(e))?;
        let now = Utc::now().to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<(Option<String>, DeckRecord)> = async {
            let row = sqlx::query(
                "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
                 FROM deck_meta WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .fetch_optional(&mut *conn)
            .await?;
            let existing = row
                .map(parse_deck_row)
                .transpose()?
                .ok_or_else(|| anyhow::anyhow!("deck `{deck_id}` not found"))?;
            let previous_home = existing.home_engine_id.clone();

            sqlx::query(
                "UPDATE deck_meta
                 SET home_engine_id = ?2,
                     home_epoch = home_epoch + 1,
                     run_epoch = run_epoch + 1,
                     home_moved_at = ?3,
                     stream_enabled = 0,
                     schedule_enabled = 0,
                     updated_at = ?3
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&target)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            // Force cold start on next run by clearing .monty dump digests.
            sqlx::query(
                "UPDATE deck_state_checkpoints
                 SET monty_dump_rev = NULL,
                     monty_dump_revision_sha256 = NULL,
                     monty_dump_sha256 = NULL,
                     updated_at = ?2
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            // Clear active sustained-time windows (new home requires fresh live evidence),
            // preserving latch/cooldown/dedup history.
            sqlx::query(
                "UPDATE deck_trigger_fire_state
                 SET sustained_since_ms = NULL, updated_at = ?2
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE deck_trigger_state
                 SET first_match_at_ms = NULL, consecutive_matches = 0, updated_at = ?2
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE deck_source_trigger_state
                 SET first_match_at_ms = NULL, consecutive_matches = 0, updated_at = ?2
                 WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            let updated_row = sqlx::query(
                "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
                 FROM deck_meta WHERE deck_id = ?1",
            )
            .bind(deck_id)
            .fetch_one(&mut *conn)
            .await?;
            let updated = parse_deck_row(updated_row)?;
            Ok((previous_home, updated))
        }
        .await;
        match res {
            Ok(out) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(out)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Register or update a Deck source binding `(deck_id, source_id)` to an authenticated `engine_id`.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_source(
        &self,
        deck_id: &str,
        source_id: &str,
        engine_id: &str,
        label: Option<String>,
        heartbeat_secs: Option<u64>,
        stale_after_secs: Option<u64>,
        offline_after_secs: Option<u64>,
    ) -> Result<DeckSourceRegistration> {
        let source_id = validate_source_id(source_id).map_err(|e| anyhow::anyhow!(e))?;
        let engine_id = validate_engine_id(engine_id).map_err(|e| anyhow::anyhow!(e))?;
        let (hb, stale, offline) =
            normalize_source_timeouts(heartbeat_secs, stale_after_secs, offline_after_secs);
        let raw_label = label
            .map(|s| tools::secrets::redact(s.trim()).into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| source_id.clone());
        let label = if raw_label.len() > crate::mesh::MAX_SOURCE_LABEL_BYTES {
            let mut end = crate::mesh::MAX_SOURCE_LABEL_BYTES;
            while end > 0 && !raw_label.is_char_boundary(end) {
                end -= 1;
            }
            raw_label[..end].to_owned()
        } else {
            raw_label
        };

        let now = Utc::now().to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<DeckSourceRegistration> = async {
            let exists: Option<String> =
                sqlx::query_scalar("SELECT deck_id FROM deck_meta WHERE deck_id = ?1")
                    .bind(deck_id)
                    .fetch_optional(&mut *conn)
                    .await?;
            if exists.is_none() {
                anyhow::bail!("deck `{deck_id}` not found");
            }

            sqlx::query(
                "INSERT INTO deck_sources
                 (deck_id, source_id, engine_id, label, heartbeat_secs, stale_after_secs, offline_after_secs, enabled, registered_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)
                 ON CONFLICT(deck_id, source_id) DO UPDATE SET
                    engine_id = excluded.engine_id,
                    label = excluded.label,
                    heartbeat_secs = excluded.heartbeat_secs,
                    stale_after_secs = excluded.stale_after_secs,
                    offline_after_secs = excluded.offline_after_secs,
                    enabled = 1,
                    updated_at = excluded.updated_at",
            )
            .bind(deck_id)
            .bind(&source_id)
            .bind(&engine_id)
            .bind(&label)
            .bind(hb as i64)
            .bind(stale as i64)
            .bind(offline as i64)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "INSERT OR IGNORE INTO deck_source_state
                 (deck_id, source_id, last_epoch, last_seq, last_live_seq, replaying, health, dropped_count, updated_at)
                 VALUES (?1, ?2, 0, 0, 0, 0, 'offline', 0, ?3)",
            )
            .bind(deck_id)
            .bind(&source_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;

            let row = sqlx::query(
                "SELECT deck_id, source_id, engine_id, label, heartbeat_secs, stale_after_secs, offline_after_secs, enabled, registered_at, updated_at
                 FROM deck_sources WHERE deck_id = ?1 AND source_id = ?2",
            )
            .bind(deck_id)
            .bind(&source_id)
            .fetch_one(&mut *conn)
            .await?;
            Ok(parse_source_registration_row(row))
        }
        .await;
        match res {
            Ok(reg) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(reg)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Unregister a source and clean up its persisted runtime/dedup/trigger state on home.
    pub async fn unregister_source(&self, deck_id: &str, source_id: &str) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<bool> = async {
            let deleted =
                sqlx::query("DELETE FROM deck_sources WHERE deck_id = ?1 AND source_id = ?2")
                    .bind(deck_id)
                    .bind(source_id)
                    .execute(&mut *conn)
                    .await?
                    .rows_affected()
                    > 0;
            for sql in [
                "DELETE FROM deck_source_state WHERE deck_id = ?1 AND source_id = ?2",
                "DELETE FROM deck_source_dedup WHERE deck_id = ?1 AND source_id = ?2",
                "DELETE FROM deck_trigger_fire_state WHERE deck_id = ?1 AND source_id = ?2",
                "DELETE FROM deck_source_trigger_state WHERE deck_id = ?1 AND source_id = ?2",
            ] {
                sqlx::query(sql)
                    .bind(deck_id)
                    .bind(source_id)
                    .execute(&mut *conn)
                    .await?;
            }
            Ok(deleted)
        }
        .await;
        match res {
            Ok(deleted) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(deleted)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    pub async fn get_source(
        &self,
        deck_id: &str,
        source_id: &str,
    ) -> Result<Option<DeckSourceRegistration>> {
        let row = sqlx::query(
            "SELECT deck_id, source_id, engine_id, label, heartbeat_secs, stale_after_secs, offline_after_secs, enabled, registered_at, updated_at
             FROM deck_sources WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(parse_source_registration_row))
    }

    pub async fn list_sources(&self, deck_id: &str) -> Result<Vec<DeckSourceRegistration>> {
        let rows = sqlx::query(
            "SELECT deck_id, source_id, engine_id, label, heartbeat_secs, stale_after_secs, offline_after_secs, enabled, registered_at, updated_at
             FROM deck_sources WHERE deck_id = ?1 ORDER BY source_id ASC",
        )
        .bind(deck_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(parse_source_registration_row).collect())
    }

    pub async fn get_source_status(
        &self,
        deck_id: &str,
        source_id: &str,
    ) -> Result<Option<DeckSourceStatus>> {
        let row = sqlx::query(
            "SELECT s.deck_id, s.source_id, s.engine_id, s.label, s.enabled,
                    COALESCE(st.health, 'offline'), COALESCE(st.replaying, 0),
                    COALESCE(st.last_epoch, 0), COALESCE(st.last_seq, 0), COALESCE(st.last_live_seq, 0),
                    st.last_observed_at, st.last_received_at, st.last_live_received_at,
                    st.lag_ms, COALESCE(st.dropped_count, 0), st.last_error,
                    (SELECT COUNT(*) FROM deck_source_spool sp WHERE sp.deck_id = s.deck_id AND sp.source_id = s.source_id)
             FROM deck_sources s
             LEFT JOIN deck_source_state st ON st.deck_id = s.deck_id AND st.source_id = s.source_id
             WHERE s.deck_id = ?1 AND s.source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(parse_source_status_row))
    }

    pub async fn list_source_statuses(&self, deck_id: &str) -> Result<Vec<DeckSourceStatus>> {
        let rows = sqlx::query(
            "SELECT s.deck_id, s.source_id, s.engine_id, s.label, s.enabled,
                    COALESCE(st.health, 'offline'), COALESCE(st.replaying, 0),
                    COALESCE(st.last_epoch, 0), COALESCE(st.last_seq, 0), COALESCE(st.last_live_seq, 0),
                    st.last_observed_at, st.last_received_at, st.last_live_received_at,
                    st.lag_ms, COALESCE(st.dropped_count, 0), st.last_error,
                    (SELECT COUNT(*) FROM deck_source_spool sp WHERE sp.deck_id = s.deck_id AND sp.source_id = s.source_id)
             FROM deck_sources s
             LEFT JOIN deck_source_state st ON st.deck_id = s.deck_id AND st.source_id = s.source_id
             WHERE s.deck_id = ?1
             ORDER BY s.source_id ASC",
        )
        .bind(deck_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(parse_source_status_row).collect())
    }

    pub async fn lookup_source_dedup(
        &self,
        deck_id: &str,
        source_id: &str,
        source_epoch: u64,
        seq: u64,
    ) -> Result<Option<IngestSourceAck>> {
        let row = sqlx::query(
            "SELECT result_json FROM deck_source_dedup
             WHERE deck_id = ?1 AND source_id = ?2 AND source_epoch = ?3 AND seq = ?4",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(source_epoch as i64)
        .bind(seq as i64)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let mut ack: IngestSourceAck = serde_json::from_str(&r.get::<String, _>(0))?;
            ack.duplicate = true;
            ack.status = crate::mesh::IngestStatus::Duplicate;
            Ok(ack)
        })
        .transpose()
    }

    /// Return the current persisted `(last_epoch, last_seq, health)` cursor for a source.
    pub async fn get_source_cursor(
        &self,
        deck_id: &str,
        source_id: &str,
    ) -> Result<(u64, u64, SourceHealthState)> {
        let (epoch, seq, health, _) = self.get_source_cursor_and_live_ts(deck_id, source_id).await?;
        Ok((epoch, seq, health))
    }

    /// Return the current persisted `(last_epoch, last_seq, health, last_live_received_at)` cursor for a source.
    pub async fn get_source_cursor_and_live_ts(
        &self,
        deck_id: &str,
        source_id: &str,
    ) -> Result<(u64, u64, SourceHealthState, Option<DateTime<Utc>>)> {
        let row = sqlx::query(
            "SELECT last_epoch, last_seq, health, last_live_received_at
             FROM deck_source_state WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some(r) => {
                let last_live = r
                    .get::<Option<String>, _>(3)
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                (
                    r.get::<i64, _>(0).max(0) as u64,
                    r.get::<i64, _>(1).max(0) as u64,
                    SourceHealthState::parse(&r.get::<String, _>(2)),
                    last_live,
                )
            }
            None => (0, 0, SourceHealthState::Offline, None),
        })
    }

    /// Atomically validate dedup + epoch fencing, advance source state, and persist the dedup receipt.
    ///
    /// Enforces:
    /// - Replayed events (`event.replay == true`) NEVER transition health to `Live` and NEVER touch
    ///   `last_live_received_at` or `last_live_seq`.
    /// - Older epochs (`event.source_epoch < last_epoch`) are rejected with `RejectedStaleEpoch`.
    /// - Duplicate `(deck_id, source_id, source_epoch, seq)` returns the persisted receipt with
    ///   `duplicate = true`.
    pub async fn commit_source_ingest(
        &self,
        event: &DeckSourceEvent,
        observed_at: DateTime<Utc>,
        received_at: DateTime<Utc>,
        mut ack: IngestSourceAck,
    ) -> Result<(IngestSourceAck, DeckSourceStatus)> {
        let received_str = received_at.to_rfc3339();
        let observed_str = observed_at.to_rfc3339();
        let lag_ms = received_at
            .signed_duration_since(observed_at)
            .num_milliseconds()
            .max(0);
        let payload_json = if event.kind == crate::mesh::SourceEventKind::Heartbeat {
            None
        } else {
            Some(serde_json::to_string(&event.payload)?)
        };

        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<(IngestSourceAck, DeckSourceStatus)> = async {
            if let Some(dup_row) = sqlx::query(
                "SELECT result_json FROM deck_source_dedup
                 WHERE deck_id = ?1 AND source_id = ?2 AND source_epoch = ?3 AND seq = ?4",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .bind(event.source_epoch as i64)
            .bind(event.seq as i64)
            .fetch_optional(&mut *conn)
            .await?
            {
                let mut prev: IngestSourceAck = serde_json::from_str(&dup_row.get::<String, _>(0))?;
                prev.duplicate = true;
                prev.status = crate::mesh::IngestStatus::Duplicate;
                let status = load_source_status_tx(&mut conn, &event.deck_id, &event.source_id).await?;
                return Ok((prev, status));
            }

            let cur_row = sqlx::query(
                "SELECT last_epoch, last_seq, last_live_seq, last_live_received_at, health
                 FROM deck_source_state WHERE deck_id = ?1 AND source_id = ?2",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .fetch_optional(&mut *conn)
            .await?;
            let (last_epoch, last_seq, prev_live_seq, prev_live_at, cur_health) = match cur_row {
                Some(r) => (
                    r.get::<i64, _>(0).max(0) as u64,
                    r.get::<i64, _>(1).max(0) as u64,
                    r.get::<i64, _>(2).max(0) as u64,
                    r.get::<Option<String>, _>(3),
                    SourceHealthState::parse(&r.get::<String, _>(4)),
                ),
                None => (0, 0, 0, None, SourceHealthState::Offline),
            };

            if event.source_epoch < last_epoch {
                ack.accepted = false;
                ack.duplicate = false;
                ack.status = crate::mesh::IngestStatus::RejectedStaleEpoch;
                ack.source_health = cur_health;
                ack.error = Some(format!(
                    "stale source_epoch {} < active epoch {}",
                    event.source_epoch, last_epoch
                ));
                let status = load_source_status_tx(&mut conn, &event.deck_id, &event.source_id).await?;
                return Ok((ack, status));
            }

            if event.source_epoch == last_epoch && event.seq <= last_seq {
                // Sequence was already processed (older than dedup retention window).
                ack.accepted = true;
                ack.duplicate = true;
                ack.status = crate::mesh::IngestStatus::Duplicate;
                ack.source_health = cur_health;
                let status = load_source_status_tx(&mut conn, &event.deck_id, &event.source_id).await?;
                return Ok((ack, status));
            }

            // Replayed history MUST NOT mark a source healthy (`Live`) or update `last_live_received_at`.
            let (new_health, replaying, new_live_seq, new_live_at, new_replay_at) = if event.replay {
                (
                    SourceHealthState::Replaying,
                    1_i64,
                    prev_live_seq,
                    prev_live_at,
                    Some(received_str.clone()),
                )
            } else {
                (
                    SourceHealthState::Live,
                    0_i64,
                    event.seq,
                    Some(received_str.clone()),
                    None,
                )
            };

            ack.accepted = true;
            ack.duplicate = false;
            ack.status = crate::mesh::IngestStatus::Accepted;
            ack.source_health = new_health;
            ack.error = None;

            sqlx::query(
                "INSERT INTO deck_source_state
                 (deck_id, source_id, last_epoch, last_seq, last_live_seq, last_observed_at, last_received_at,
                  last_live_received_at, last_replay_at, replaying, health, lag_ms, dropped_count, last_error, last_payload_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, NULL, ?13, ?7)
                 ON CONFLICT(deck_id, source_id) DO UPDATE SET
                    last_epoch = excluded.last_epoch,
                    last_seq = excluded.last_seq,
                    last_live_seq = excluded.last_live_seq,
                    last_observed_at = excluded.last_observed_at,
                    last_received_at = excluded.last_received_at,
                    last_live_received_at = excluded.last_live_received_at,
                    last_replay_at = COALESCE(excluded.last_replay_at, deck_source_state.last_replay_at),
                    replaying = excluded.replaying,
                    health = excluded.health,
                    lag_ms = excluded.lag_ms,
                    last_error = NULL,
                    last_payload_json = COALESCE(excluded.last_payload_json, deck_source_state.last_payload_json),
                    updated_at = excluded.updated_at",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .bind(event.source_epoch as i64)
            .bind(event.seq as i64)
            .bind(new_live_seq as i64)
            .bind(&observed_str)
            .bind(&received_str)
            .bind(&new_live_at)
            .bind(&new_replay_at)
            .bind(replaying)
            .bind(new_health.as_str())
            .bind(lag_ms)
            .bind(&payload_json)
            .execute(&mut *conn)
            .await?;

            let ack_json = serde_json::to_string(&ack)?;
            sqlx::query(
                "INSERT OR REPLACE INTO deck_source_dedup
                 (deck_id, source_id, source_epoch, seq, result_json, received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .bind(event.source_epoch as i64)
            .bind(event.seq as i64)
            .bind(&ack_json)
            .bind(&received_str)
            .execute(&mut *conn)
            .await?;

            // Prune dedup table to bounded MAX_DEDUP_ENTRIES_PER_SOURCE per source.
            sqlx::query(
                "DELETE FROM deck_source_dedup
                 WHERE deck_id = ?1 AND source_id = ?2 AND rowid NOT IN (
                    SELECT rowid FROM deck_source_dedup
                    WHERE deck_id = ?1 AND source_id = ?2
                    ORDER BY source_epoch DESC, seq DESC
                    LIMIT ?3
                 )",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .bind(MAX_DEDUP_ENTRIES_PER_SOURCE as i64)
            .execute(&mut *conn)
            .await?;

            let status = load_source_status_tx(&mut conn, &event.deck_id, &event.source_id).await?;
            Ok((ack, status))
        }
        .await;
        match res {
            Ok(out) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(out)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Persist a redacted source error on `deck_source_state` without altering epoch/seq cursors.
    pub async fn record_source_error(
        &self,
        deck_id: &str,
        source_id: &str,
        error: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let redacted = tools::secrets::redact(error).into_owned();
        sqlx::query(
            "UPDATE deck_source_state SET last_error = ?3, updated_at = ?4
             WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(&redacted)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Evaluate heartbeat/freshness timeouts (`Live -> Stale -> Offline`) and return every
    /// source whose health state changed. Clears active sustained-time trigger windows when
    /// a source becomes `Stale` or `Offline`.
    pub async fn sweep_source_liveness(
        &self,
        deck_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<DeckSourceStatus>> {
        let now_str = now.to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<Vec<DeckSourceStatus>> = async {
            let rows = if let Some(did) = deck_id {
                sqlx::query(
                    "SELECT s.deck_id, s.source_id, s.stale_after_secs, s.offline_after_secs,
                            COALESCE(st.health, 'offline'), COALESCE(st.replaying, 0),
                            st.last_live_received_at, st.last_replay_at
                     FROM deck_sources s
                     LEFT JOIN deck_source_state st ON st.deck_id = s.deck_id AND st.source_id = s.source_id
                     WHERE s.deck_id = ?1",
                )
                .bind(did)
                .fetch_all(&mut *conn)
                .await?
            } else {
                sqlx::query(
                    "SELECT s.deck_id, s.source_id, s.stale_after_secs, s.offline_after_secs,
                            COALESCE(st.health, 'offline'), COALESCE(st.replaying, 0),
                            st.last_live_received_at, st.last_replay_at
                     FROM deck_sources s
                     LEFT JOIN deck_source_state st ON st.deck_id = s.deck_id AND st.source_id = s.source_id",
                )
                .fetch_all(&mut *conn)
                .await?
            };

            let mut changed = Vec::new();
            for r in rows {
                let did: String = r.get(0);
                let sid: String = r.get(1);
                let stale_after = r.get::<i64, _>(2).max(1) as u64;
                let offline_after = r.get::<i64, _>(3).max(stale_after as i64) as u64;
                let cur_health = SourceHealthState::parse(&r.get::<String, _>(4));
                let cur_replaying = r.get::<i64, _>(5) != 0;
                let last_live_at: Option<String> = r.get(6);
                let last_replay_at: Option<String> = r.get(7);

                let live_age_secs = last_live_at
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| {
                        now.signed_duration_since(dt.with_timezone(&Utc))
                            .num_seconds()
                            .max(0) as u64
                    });
                let replay_age_secs = last_replay_at
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| {
                        now.signed_duration_since(dt.with_timezone(&Utc))
                            .num_seconds()
                            .max(0) as u64
                    });

                let (next_health, next_replaying) = match live_age_secs {
                    None => {
                        if cur_health == SourceHealthState::Replaying
                            && replay_age_secs.is_some_and(|a| a < stale_after)
                        {
                            (SourceHealthState::Replaying, true)
                        } else {
                            (SourceHealthState::Offline, false)
                        }
                    }
                    Some(age) if age >= offline_after => (SourceHealthState::Offline, false),
                    Some(age) if age >= stale_after => (SourceHealthState::Stale, false),
                    Some(_) if cur_health == SourceHealthState::Replaying => {
                        if replay_age_secs.is_some_and(|a| a < stale_after) {
                            (SourceHealthState::Replaying, true)
                        } else {
                            // Do not promote replay directly to Live; require fresh live heartbeat/event.
                            (SourceHealthState::Stale, false)
                        }
                    }
                    Some(_) => (SourceHealthState::Live, false),
                };

                if next_health != cur_health || next_replaying != cur_replaying {
                    sqlx::query(
                        "UPDATE deck_source_state
                         SET health = ?3, replaying = ?4, updated_at = ?5
                         WHERE deck_id = ?1 AND source_id = ?2",
                    )
                    .bind(&did)
                    .bind(&sid)
                    .bind(next_health.as_str())
                    .bind(i64::from(next_replaying))
                    .bind(&now_str)
                    .execute(&mut *conn)
                    .await?;

                    if !next_health.is_healthy() {
                        sqlx::query(
                            "UPDATE deck_trigger_fire_state
                             SET sustained_since_ms = NULL, updated_at = ?3
                             WHERE deck_id = ?1 AND source_id = ?2",
                        )
                        .bind(&did)
                        .bind(&sid)
                        .bind(&now_str)
                        .execute(&mut *conn)
                        .await?;
                        sqlx::query(
                            "UPDATE deck_source_trigger_state
                             SET first_match_at_ms = NULL, consecutive_matches = 0, updated_at = ?3
                             WHERE deck_id = ?1 AND source_id = ?2",
                        )
                        .bind(&did)
                        .bind(&sid)
                        .bind(&now_str)
                        .execute(&mut *conn)
                        .await?;
                    }

                    let status = load_source_status_tx(&mut conn, &did, &sid).await?;
                    changed.push(status);
                }
            }
            Ok(changed)
        }
        .await;
        match res {
            Ok(out) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(out)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Evaluate and persist a sustained-threshold trigger state transition on the Deck's home engine.
    ///
    /// Persistent deduplicated dispatch keyed by `(deck_id, trigger_key, source_id, source_epoch, seq)`.
    #[allow(clippy::too_many_arguments)]
    pub async fn evaluate_sustained_trigger(
        &self,
        deck_id: &str,
        trigger_key: &str,
        source_id: &str,
        source_epoch: u64,
        seq: u64,
        received_at: DateTime<Utc>,
        breached: bool,
        sustained_secs: u64,
    ) -> Result<Option<TriggerDispatchRecord>> {
        let now_str = received_at.to_rfc3339();
        let now_ms = received_at.timestamp_millis();
        let dedup_key = format!("{deck_id}:{trigger_key}:{source_id}:{source_epoch}:{seq}");

        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<Option<TriggerDispatchRecord>> = async {
            let existing = sqlx::query(
                "SELECT source_id, source_epoch, last_seq, sustained_since_ms, fired_count, last_fired_at, last_dispatch_dedup_key
                 FROM deck_trigger_fire_state WHERE deck_id = ?1 AND trigger_key = ?2",
            )
            .bind(deck_id)
            .bind(trigger_key)
            .fetch_optional(&mut *conn)
            .await?;

            let (prev_source, prev_epoch, prev_sustained, fired_count, last_fired_at, last_dedup) =
                match existing {
                    Some(r) => (
                        r.get::<String, _>(0),
                        r.get::<i64, _>(1).max(0) as u64,
                        r.get::<Option<i64>, _>(3),
                        r.get::<i64, _>(4).max(0) as u64,
                        r.get::<Option<String>, _>(5),
                        r.get::<Option<String>, _>(6),
                    ),
                    None => (String::new(), 0, None, 0, None, None),
                };

            if last_dedup.as_deref() == Some(dedup_key.as_str()) {
                return Ok(None);
            }

            if !breached {
                sqlx::query(
                    "INSERT INTO deck_trigger_fire_state
                     (deck_id, trigger_key, source_id, source_epoch, last_seq, sustained_since_ms, fired_count, last_fired_at, last_dispatch_dedup_key, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9)
                     ON CONFLICT(deck_id, trigger_key) DO UPDATE SET
                        source_id = excluded.source_id,
                        source_epoch = excluded.source_epoch,
                        last_seq = excluded.last_seq,
                        sustained_since_ms = NULL,
                        updated_at = excluded.updated_at",
                )
                .bind(deck_id)
                .bind(trigger_key)
                .bind(source_id)
                .bind(source_epoch as i64)
                .bind(seq as i64)
                .bind(fired_count as i64)
                .bind(&last_fired_at)
                .bind(&last_dedup)
                .bind(&now_str)
                .execute(&mut *conn)
                .await?;
                return Ok(None);
            }

            let start_ms = if prev_source == source_id
                && prev_epoch == source_epoch
                && prev_sustained.is_some_and(|s| now_ms >= s)
            {
                prev_sustained.unwrap()
            } else {
                now_ms
            };
            let elapsed_ms = now_ms.saturating_sub(start_ms);
            let required_ms = i64::try_from(sustained_secs.saturating_mul(1000)).unwrap_or(i64::MAX);

            if elapsed_ms >= required_ms {
                let new_fired = fired_count.saturating_add(1);
                sqlx::query(
                    "INSERT INTO deck_trigger_fire_state
                     (deck_id, trigger_key, source_id, source_epoch, last_seq, sustained_since_ms, fired_count, last_fired_at, last_dispatch_dedup_key, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?7)
                     ON CONFLICT(deck_id, trigger_key) DO UPDATE SET
                        source_id = excluded.source_id,
                        source_epoch = excluded.source_epoch,
                        last_seq = excluded.last_seq,
                        sustained_since_ms = NULL,
                        fired_count = excluded.fired_count,
                        last_fired_at = excluded.last_fired_at,
                        last_dispatch_dedup_key = excluded.last_dispatch_dedup_key,
                        updated_at = excluded.updated_at",
                )
                .bind(deck_id)
                .bind(trigger_key)
                .bind(source_id)
                .bind(source_epoch as i64)
                .bind(seq as i64)
                .bind(new_fired as i64)
                .bind(&now_str)
                .bind(&dedup_key)
                .execute(&mut *conn)
                .await?;

                Ok(Some(TriggerDispatchRecord {
                    deck_id: deck_id.to_owned(),
                    trigger_key: trigger_key.to_owned(),
                    source_id: source_id.to_owned(),
                    source_epoch,
                    seq,
                    dedup_key,
                    fired_at: now_str,
                }))
            } else {
                sqlx::query(
                    "INSERT INTO deck_trigger_fire_state
                     (deck_id, trigger_key, source_id, source_epoch, last_seq, sustained_since_ms, fired_count, last_fired_at, last_dispatch_dedup_key, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(deck_id, trigger_key) DO UPDATE SET
                        source_id = excluded.source_id,
                        source_epoch = excluded.source_epoch,
                        last_seq = excluded.last_seq,
                        sustained_since_ms = excluded.sustained_since_ms,
                        updated_at = excluded.updated_at",
                )
                .bind(deck_id)
                .bind(trigger_key)
                .bind(source_id)
                .bind(source_epoch as i64)
                .bind(seq as i64)
                .bind(start_ms)
                .bind(fired_count as i64)
                .bind(&last_fired_at)
                .bind(&last_dedup)
                .bind(&now_str)
                .execute(&mut *conn)
                .await?;
                Ok(None)
            }
        }
        .await;
        match res {
            Ok(out) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(out)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    pub async fn list_trigger_fire_states(&self, deck_id: &str) -> Result<Vec<TriggerFireState>> {
        let rows = sqlx::query(
            "SELECT deck_id, trigger_key, source_id, source_epoch, last_seq, sustained_since_ms, fired_count, last_fired_at, last_dispatch_dedup_key
             FROM deck_trigger_fire_state WHERE deck_id = ?1 ORDER BY trigger_key ASC",
        )
        .bind(deck_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TriggerFireState {
                deck_id: r.get(0),
                trigger_key: r.get(1),
                source_id: r.get(2),
                source_epoch: r.get::<i64, _>(3).max(0) as u64,
                last_seq: r.get::<i64, _>(4).max(0) as u64,
                sustained_since_ms: r.get(5),
                fired_count: r.get::<i64, _>(6).max(0) as u64,
                last_fired_at: r.get(7),
                last_dispatch_dedup_key: r.get(8),
            })
            .collect())
    }

    /// Monotonically allocate the next persisted `source_epoch` on a local source node.
    pub async fn next_local_source_epoch(&self, deck_id: &str, source_id: &str) -> Result<u64> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let res: Result<u64> = async {
            sqlx::query(
                "INSERT INTO deck_source_local_meta (deck_id, source_id, source_epoch, updated_at)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(deck_id, source_id) DO UPDATE SET
                    source_epoch = deck_source_local_meta.source_epoch + 1,
                    updated_at = excluded.updated_at",
            )
            .bind(deck_id)
            .bind(source_id)
            .bind(&now)
            .execute(&mut *conn)
            .await?;
            let epoch: i64 = sqlx::query_scalar(
                "SELECT source_epoch FROM deck_source_local_meta WHERE deck_id = ?1 AND source_id = ?2",
            )
            .bind(deck_id)
            .bind(source_id)
            .fetch_one(&mut *conn)
            .await?;
            Ok(epoch.max(1) as u64)
        }
        .await;
        match res {
            Ok(epoch) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(epoch)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Durably enqueue an offline source event into the bounded per-source spool.
    /// Rejects with [`SpoolError::Backpressure`] when `max_events` or `max_bytes` would be exceeded.
    pub async fn spool_enqueue(
        &self,
        event: &DeckSourceEvent,
        limits: SourceSpoolLimits,
    ) -> std::result::Result<SpoolStats, SpoolError> {
        let event_json = serde_json::to_string(event)
            .map_err(|e| SpoolError::Storage(format!("serialize spool event: {e}")))?;
        let payload_bytes = event_json.len();
        let now = Utc::now().to_rfc3339();

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| SpoolError::Storage(e.to_string()))?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| SpoolError::Storage(e.to_string()))?;

        let res: std::result::Result<SpoolStats, SpoolError> = async {
            let row = sqlx::query(
                "SELECT COUNT(*), COALESCE(SUM(payload_bytes), 0)
                 FROM deck_source_spool WHERE deck_id = ?1 AND source_id = ?2",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| SpoolError::Storage(e.to_string()))?;

            let depth = row.get::<i64, _>(0).max(0) as usize;
            let bytes = row.get::<i64, _>(1).max(0) as usize;

            if depth >= limits.max_events || bytes.saturating_add(payload_bytes) > limits.max_bytes {
                return Err(SpoolError::Backpressure {
                    depth,
                    max_events: limits.max_events,
                    bytes,
                    max_bytes: limits.max_bytes,
                });
            }

            sqlx::query(
                "INSERT OR IGNORE INTO deck_source_spool
                 (deck_id, source_id, source_epoch, seq, event_json, payload_bytes, enqueued_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .bind(event.source_epoch as i64)
            .bind(event.seq as i64)
            .bind(&event_json)
            .bind(payload_bytes as i64)
            .bind(&now)
            .execute(&mut *conn)
            .await
            .map_err(|e| SpoolError::Storage(e.to_string()))?;

            let updated = sqlx::query(
                "SELECT COUNT(*), COALESCE(SUM(payload_bytes), 0)
                 FROM deck_source_spool WHERE deck_id = ?1 AND source_id = ?2",
            )
            .bind(&event.deck_id)
            .bind(&event.source_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| SpoolError::Storage(e.to_string()))?;

            Ok(SpoolStats {
                depth: updated.get::<i64, _>(0).max(0) as usize,
                bytes: updated.get::<i64, _>(1).max(0) as usize,
            })
        }
        .await;

        match res {
            Ok(stats) => {
                sqlx::query("COMMIT")
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| SpoolError::Storage(e.to_string()))?;
                Ok(stats)
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Peek up to `limit` spooled events in strict `(source_epoch ASC, seq ASC)` replay order.
    pub async fn spool_peek_batch(
        &self,
        deck_id: &str,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<DeckSourceEvent>> {
        let rows = sqlx::query(
            "SELECT event_json FROM deck_source_spool
             WHERE deck_id = ?1 AND source_id = ?2
             ORDER BY source_epoch ASC, seq ASC
             LIMIT ?3",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| Ok(serde_json::from_str(&r.get::<String, _>(0))?))
            .collect()
    }

    pub async fn spool_remove(
        &self,
        deck_id: &str,
        source_id: &str,
        source_epoch: u64,
        seq: u64,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM deck_source_spool
             WHERE deck_id = ?1 AND source_id = ?2 AND source_epoch = ?3 AND seq = ?4",
        )
        .bind(deck_id)
        .bind(source_id)
        .bind(source_epoch as i64)
        .bind(seq as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Release a reserved trigger fire row when `schedule::FireTrigger` rejects/fails dispatch.
    pub async fn release_trigger_fire(&self, dedup_key: &str) -> Result<()> {
        sqlx::query("DELETE FROM deck_trigger_fires WHERE dedup_key = ?1")
            .bind(dedup_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn spool_stats(&self, deck_id: &str, source_id: &str) -> Result<SpoolStats> {
        let row = sqlx::query(
            "SELECT COUNT(*), COALESCE(SUM(payload_bytes), 0)
             FROM deck_source_spool WHERE deck_id = ?1 AND source_id = ?2",
        )
        .bind(deck_id)
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(SpoolStats {
            depth: row.get::<i64, _>(0).max(0) as usize,
            bytes: row.get::<i64, _>(1).max(0) as usize,
        })
    }

    /// Expose raw pool in unit tests for direct SQLite forgery regression tests.
    #[cfg(test)]
    pub(crate) fn pool_for_test(&self) -> &SqlitePool {
        &self.pool
    }
}

fn sanitize_deck_dir_name(deck_id: &str) -> Result<String> {
    let trimmed = deck_id.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        anyhow::bail!("invalid deck_id `{deck_id}` for isolated work directory");
    }
    Ok(trimmed.to_owned())
}

async fn ensure_deck_tx(
    conn: &mut SqliteConnection,
    deck_id: &str,
    now: &str,
) -> Result<DeckRecord> {
    let session_id = SessionId::new();
    sqlx::query(
        "INSERT OR IGNORE INTO deck_meta
         (deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, 0, 0, 0, NULL, 1, NULL, ?3, ?3)",
    )
    .bind(deck_id)
    .bind(session_id.0.to_string())
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query(
        "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, run_epoch, home_engine_id, home_epoch, home_moved_at, created_at, updated_at
         FROM deck_meta WHERE deck_id = ?1",
    )
    .bind(deck_id)
    .fetch_one(&mut *conn)
    .await?;
    parse_deck_row(row)
}

async fn load_source_status_tx(
    conn: &mut SqliteConnection,
    deck_id: &str,
    source_id: &str,
) -> Result<DeckSourceStatus> {
    let row = sqlx::query(
        "SELECT s.deck_id, s.source_id, s.engine_id, s.label, s.enabled,
                COALESCE(st.health, 'offline'), COALESCE(st.replaying, 0),
                COALESCE(st.last_epoch, 0), COALESCE(st.last_seq, 0), COALESCE(st.last_live_seq, 0),
                st.last_observed_at, st.last_received_at, st.last_live_received_at,
                st.lag_ms, COALESCE(st.dropped_count, 0), st.last_error,
                (SELECT COUNT(*) FROM deck_source_spool sp WHERE sp.deck_id = s.deck_id AND sp.source_id = s.source_id)
         FROM deck_sources s
         LEFT JOIN deck_source_state st ON st.deck_id = s.deck_id AND st.source_id = s.source_id
         WHERE s.deck_id = ?1 AND s.source_id = ?2",
    )
    .bind(deck_id)
    .bind(source_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(parse_source_status_row(row))
}

fn parse_source_registration_row(r: sqlx::sqlite::SqliteRow) -> DeckSourceRegistration {
    DeckSourceRegistration {
        deck_id: r.get(0),
        source_id: r.get(1),
        engine_id: r.get(2),
        label: r.get(3),
        heartbeat_secs: r.get::<i64, _>(4).max(1) as u64,
        stale_after_secs: r.get::<i64, _>(5).max(1) as u64,
        offline_after_secs: r.get::<i64, _>(6).max(1) as u64,
        enabled: r.get::<i64, _>(7) != 0,
        registered_at: r.get(8),
        updated_at: r.get(9),
    }
}

fn parse_source_status_row(r: sqlx::sqlite::SqliteRow) -> DeckSourceStatus {
    DeckSourceStatus {
        deck_id: r.get(0),
        source_id: r.get(1),
        engine_id: r.get(2),
        label: r.get(3),
        enabled: r.get::<i64, _>(4) != 0,
        health: SourceHealthState::parse(&r.get::<String, _>(5)),
        replaying: r.get::<i64, _>(6) != 0,
        last_epoch: r.get::<i64, _>(7).max(0) as u64,
        last_seq: r.get::<i64, _>(8).max(0) as u64,
        last_live_seq: r.get::<i64, _>(9).max(0) as u64,
        last_observed_at: r.get(10),
        last_received_at: r.get(11),
        last_live_received_at: r.get(12),
        lag_ms: r.get::<Option<i64>, _>(13).map(|v| v.max(0) as u64),
        dropped_count: r.get::<i64, _>(14).max(0) as u64,
        last_error: r.get(15),
        spool_depth: r.get::<i64, _>(16).max(0) as u64,
    }
}

async fn get_script_revision_tx(
    conn: &mut SqliteConnection,
    deck_id: &str,
    rev: u64,
) -> Result<Option<ScriptRevision>> {
    let row = sqlx::query(
        "SELECT deck_id, rev, code, code_sha256, revision_sha256, declared_actions_json,
                triggers_json, mode, interval_secs, schedule_cron, status, proposed_by,
                approved_by, approved_at, rejected_reason, change_summary, created_at
         FROM deck_script_revisions WHERE deck_id = ?1 AND rev = ?2",
    )
    .bind(deck_id)
    .bind(rev as i64)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(parse_script_row).transpose()
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
        run_epoch: r.get::<i64, _>(7).max(0) as u64,
        home_engine_id: r.get(8),
        home_epoch: r.get::<i64, _>(9).max(1) as u64,
        home_moved_at: r.get(10),
        created_at: r.get(11),
        updated_at: r.get(12),
    })
}

fn parse_script_row(r: sqlx::sqlite::SqliteRow) -> Result<ScriptRevision> {
    let deck_id: String = r.get(0);
    let rev = r.get::<i64, _>(1).max(0) as u64;
    let code: String = r.get(2);
    let stored_code_sha: String = r.get(3);
    let stored_rev_sha: String = r.get(4);
    let actions_raw: String = r.get(5);
    let declared_actions: Vec<DeclaredAction> = serde_json::from_str(&actions_raw)?;
    let triggers_raw: String = r.get(6);
    let triggers: Vec<TriggerSpec> = serde_json::from_str(&triggers_raw)?;
    let mode = ScriptMode::parse(&r.get::<String, _>(7));
    let interval_secs = r
        .get::<Option<i64>, _>(8)
        .map(|v| v.clamp(MIN_INTERVAL_SECS as i64, MAX_INTERVAL_SECS as i64) as u64);
    let schedule_cron: Option<String> = r.get(9);

    // 229 B2: recompute both source and canonical revision digests on every load.
    let actual_code_sha = sha256_hex(&code);
    if stored_code_sha != actual_code_sha {
        anyhow::bail!(
            "deck `{deck_id}` rev {rev} failed code_sha256 integrity verification (stored {stored_code_sha}, actual {actual_code_sha})"
        );
    }
    let actual_rev_sha = canonical_revision_sha256(
        &code,
        &declared_actions,
        &triggers,
        mode,
        interval_secs,
        schedule_cron.as_deref(),
    );
    if stored_rev_sha != actual_rev_sha {
        anyhow::bail!(
            "deck `{deck_id}` rev {rev} failed revision_sha256 integrity verification (stored {stored_rev_sha}, actual {actual_rev_sha})"
        );
    }

    Ok(ScriptRevision {
        deck_id,
        rev,
        code,
        code_sha256: stored_code_sha,
        revision_sha256: stored_rev_sha,
        declared_actions,
        triggers,
        mode,
        interval_secs,
        schedule_cron,
        status: ScriptStatus::parse(&r.get::<String, _>(10)),
        proposed_by: r.get(11),
        approved_by: r.get(12),
        approved_at: r.get(13),
        rejected_reason: r.get(14),
        change_summary: r.get(15),
        created_at: r.get(16),
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
