//! SQLite persistence for Deck metadata, review-gated script revisions, durable snapshots,
//! cooperative state checkpoints, run status/staleness, and action idempotency.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use runtime::SessionId;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::model::{
    DeckSnapshot, DeclaredAction, InteractErrorCode, InteractResponse, MAX_ACTION_NAME_BYTES,
    MAX_CHANGE_SUMMARY_BYTES, MAX_DECLARED_ACTIONS, MAX_INTERVAL_SECS, MAX_SCRIPT_CODE_BYTES,
    MIN_INTERVAL_SECS, ScriptMode, ScriptRevision, ScriptStatus, canonical_json_string,
    clamp_interval_secs,
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
pub fn canonical_revision_sha256(
    code: &str,
    declared_actions: &[DeclaredAction],
    mode: ScriptMode,
    interval_secs: Option<u64>,
    schedule_cron: Option<&str>,
) -> String {
    let payload = serde_json::json!({
        "v": 1,
        "code": code,
        "declared_actions": declared_actions,
        "mode": mode.as_str(),
        "interval_secs": interval_secs,
        "schedule_cron": schedule_cron,
    });
    sha256_hex(&canonical_json_string(&payload))
}

/// Validate safety caps on a proposed script revision (229 H4).
pub fn validate_script_proposal(
    code: &str,
    declared_actions: &[DeclaredAction],
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

    /// Isolated per-Deck working directory under `<data_dir>/deck-work/<deck_id>`,
    /// strictly outside `<data_dir>/decks` (which holds protected engine state) and
    /// never `<data_dir>` or an ancestor of `<data_dir>` (229 H1).
    pub fn deck_work_dir(&self, deck_id: &str) -> Result<PathBuf> {
        let safe_id = sanitize_deck_dir_name(deck_id)?;
        let dir = self.work_root.join(safe_id);
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
        ];
        for sql in ddl {
            sqlx::query(sql).execute(&self.pool).await?;
        }

        // Additive migrations for pre-hardening SQLite databases.
        let _ = sqlx::query(
            "ALTER TABLE deck_script_revisions ADD COLUMN revision_sha256 TEXT NOT NULL DEFAULT ''",
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
            "SELECT deck_id, rev, code, declared_actions_json, mode, interval_secs, schedule_cron
             FROM deck_script_revisions WHERE revision_sha256 = ''",
        )
        .fetch_all(&self.pool)
        .await?;
        for r in legacy_rows {
            let deck_id: String = r.get(0);
            let rev: i64 = r.get(1);
            let code: String = r.get(2);
            let actions_raw: String = r.get(3);
            let mode = ScriptMode::parse(&r.get::<String, _>(4));
            let interval_secs = r
                .get::<Option<i64>, _>(5)
                .map(|v| clamp_interval_secs(v.max(0) as u64));
            let schedule_cron: Option<String> = r.get(6);
            if let Ok(actions) = serde_json::from_str::<Vec<DeclaredAction>>(&actions_raw) {
                let digest = canonical_revision_sha256(
                    &code,
                    &actions,
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

    /// List Decks that should auto-resume on engine startup (229 H3).
    pub async fn list_auto_resume_decks(&self) -> Result<Vec<DeckRecord>> {
        let rows = sqlx::query(
            "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at
             FROM deck_meta
             WHERE (stream_enabled = 1 OR schedule_enabled = 1) AND active_rev IS NOT NULL
             ORDER BY updated_at DESC",
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
        mode: ScriptMode,
        interval_secs: Option<u64>,
        schedule_cron: Option<String>,
        proposed_by: String,
        change_summary: String,
    ) -> Result<ScriptRevision> {
        validate_script_proposal(&code, &declared_actions, &change_summary)?;
        let interval_secs = interval_secs.map(clamp_interval_secs);
        let code_sha256 = sha256_hex(&code);
        let revision_sha256 = canonical_revision_sha256(
            &code,
            &declared_actions,
            mode,
            interval_secs,
            schedule_cron.as_deref(),
        );
        let actions_json = serde_json::to_string(&declared_actions)?;
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
                 (deck_id, rev, code, code_sha256, revision_sha256, declared_actions_json, mode,
                  interval_secs, schedule_cron, status, proposed_by, approved_by, approved_at,
                  rejected_reason, change_summary, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending_review', ?10, NULL, NULL, NULL, ?11, ?12)",
            )
            .bind(deck_id)
            .bind(next_rev)
            .bind(&code)
            .bind(&code_sha256)
            .bind(&revision_sha256)
            .bind(&actions_json)
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
                    mode, interval_secs, schedule_cron, status, proposed_by, approved_by,
                    approved_at, rejected_reason, change_summary, created_at
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

        // Prune old snapshots beyond 100 per Deck (ordered by epoch/seq/rowid, 229 L1).
        sqlx::query(
            "DELETE FROM deck_snapshots WHERE deck_id = ?1 AND snapshot_id NOT IN (
                SELECT snapshot_id FROM deck_snapshots WHERE deck_id = ?1
                ORDER BY run_epoch DESC, seq DESC, rowid DESC LIMIT 100
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
        // Don't cache transient/conflict errors so callers can retry safely.
        if matches!(
            resp.error_code,
            Some(
                InteractErrorCode::Busy
                    | InteractErrorCode::TimedOut
                    | InteractErrorCode::WaitingApproval
                    | InteractErrorCode::Conflict
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
         (deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, 0, 0, ?3, ?3)",
    )
    .bind(deck_id)
    .bind(session_id.0.to_string())
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query(
        "SELECT deck_id, session_id, active_rev, pending_rev, schedule_cron, schedule_enabled, stream_enabled, created_at, updated_at
         FROM deck_meta WHERE deck_id = ?1",
    )
    .bind(deck_id)
    .fetch_one(&mut *conn)
    .await?;
    parse_deck_row(row)
}

async fn get_script_revision_tx(
    conn: &mut SqliteConnection,
    deck_id: &str,
    rev: u64,
) -> Result<Option<ScriptRevision>> {
    let row = sqlx::query(
        "SELECT deck_id, rev, code, code_sha256, revision_sha256, declared_actions_json,
                mode, interval_secs, schedule_cron, status, proposed_by, approved_by,
                approved_at, rejected_reason, change_summary, created_at
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
        created_at: r.get(7),
        updated_at: r.get(8),
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
    let mode = ScriptMode::parse(&r.get::<String, _>(6));
    let interval_secs = r
        .get::<Option<i64>, _>(7)
        .map(|v| v.clamp(MIN_INTERVAL_SECS as i64, MAX_INTERVAL_SECS as i64) as u64);
    let schedule_cron: Option<String> = r.get(8);

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
        mode,
        interval_secs,
        schedule_cron,
        status: ScriptStatus::parse(&r.get::<String, _>(9)),
        proposed_by: r.get(10),
        approved_by: r.get(11),
        approved_at: r.get(12),
        rejected_reason: r.get(13),
        change_summary: r.get(14),
        created_at: r.get(15),
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
