//! sqlite session store (sqlx): `sessions` + `messages` + `epochs` +
//! `todo_snapshots` + `errors`; resume by replay. Only the agent writes it;
//! UIs read through the supervisor's commands.
//! `sessions.cut_at` is the undo marker: rows with `seq >= cut_at` (epochs,
//! todo snapshots and errors `at_seq >= cut_at`) are hidden from every reader
//! until `commit_cut` deletes them. The plan is the latest visible snapshot
//! ordered by `(at_seq, k)`; `sessions.todo_version` only ever grows (47 §5.2).
//! `errors` are UI-only rows anchored at the last visible message seq when
//! they happened; they never enter the model frame.
//!
//! Context removal (48 §4): `messages.excluded` / `epochs.excluded` hold
//! [`Mask`]s beside untouched rows. *Raw* readers (`messages`, undo/redo,
//! `epochs`, search) see the rows; *effective* readers (`frame`,
//! `context_items`, `effective_message`) apply the masks.
//! `sessions.context_revision` is the mutation handle: bumped in the same
//! transaction as every append, epoch, exclusion edit, undo/redo and
//! committed cut; never reset when seq/epoch numbers are reused.
//!
//! Communication accounting (50 §7: root scope, peer exchanges, A2A
//! contexts/tasks, peer bindings) lives in [`communication`] on the same pool.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};
use runtime::SessionId;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{FromRow, SqliteConnection};

use crate::SessionSummary;
use crate::context::{
    Active, ContextHistory, ContextOwner, ContextRemoval, ContextSnapshot, ContextTarget, Mask,
    entry, project,
};
use crate::loop_::text_of;
use crate::todo::{Todo, TodoSnapshot, TodoSource};

pub mod communication;
pub mod accounting;

/// Handle to `<data_dir>/sessions.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// One compaction (07 §8.4); epoch 0 has no row.
#[derive(Clone, Debug)]
pub struct Epoch {
    /// Epoch number, 1-based.
    pub n: u32,
    /// Last message seq when compaction ran (UI divider position).
    pub at_seq: i64,
    /// First message seq replayed after the head.
    pub tail_from: i64,
    /// The `<context_summary>` user message.
    pub head: ChatMessage,
}

/// Raw `sessions` row; ids and timestamps are TEXT and parsed on the way out.
#[derive(FromRow)]
struct SessionRow {
    id: String,
    created_at: String,
    updated_at: String,
    title: String,
    model: String,
    parent: Option<String>,
    name: Option<String>,
    effort: Option<String>,
    agent: Option<String>,
    cwd: Option<String>,
    pinned: bool,
    project: Option<String>,
}

/// Scalar subquery: the `project` of the root above the outer `sessions` row
/// (51 §3: children carry none of their own). Correlated on the outer table
/// name, so the enclosing query must select `FROM sessions` unaliased.
macro_rules! effective_project {
    () => {
        "(WITH RECURSIVE up(id, parent) AS ( \
              SELECT sessions.id, sessions.parent \
              UNION ALL \
              SELECT p.id, p.parent FROM sessions p JOIN up ON p.id = up.parent) \
          SELECT r.project FROM up JOIN sessions r ON r.id = up.id WHERE up.parent IS NULL)"
    };
}

/// Column list every `sessions` reader selects, qualified with the table name
/// so joins against `tree` stay unambiguous. A macro (not a const) so it
/// splices into `concat!` and the query stays a `'static` literal.
macro_rules! session_cols {
    () => {
        concat!(
            "sessions.id, sessions.created_at, sessions.updated_at, sessions.title, sessions.model, \
             sessions.parent, sessions.name, sessions.effort, sessions.agent, sessions.cwd, sessions.pinned, ",
            effective_project!(),
            " AS project"
        )
    };
}

impl TryFrom<SessionRow> for SessionSummary {
    type Error = anyhow::Error;

    fn try_from(r: SessionRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: r.id.parse().context("bad session id")?,
            created_at: r.created_at.parse().context("bad created_at")?,
            updated_at: r.updated_at.parse().context("bad updated_at")?,
            title: r.title,
            model: r.model,
            parent: r
                .parent
                .as_deref()
                .map(str::parse)
                .transpose()
                .context("bad parent id")?,
            name: r.name,
            effort: r.effort,
            agent: r.agent,
            cwd: r.cwd,
            pinned: r.pinned,
            project: r.project,
            retention: tools::Retention::Durable,
        })
    }
}

impl Store {
    /// Open (creating) the database in WAL mode and run migrations.
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
        Ok(Self { pool })
    }

    /// An in-memory database with the same schema, for a private root tree
    /// (51 §4.5): no file, no journal, `temp_store=MEMORY` so sorts and
    /// temp tables never spill to disk. Exactly one pooled connection:
    /// every SQLite `:memory:` connection is its own database, so a second
    /// one would see an empty, unmigrated store.
    pub async fn open_memory() -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory)
            .pragma("temp_store", "MEMORY");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .connect_with(opts)
            .await
            .context("open in-memory store")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool })
    }

    /// The pool, for crate-internal probes (tests of pragmas and the like).
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert a session row and its accounting owner in one transaction.
    /// `title` starts empty; `name` is the peer name of a subagent; `project`
    /// is stored on roots only (a child's is its root's, 51 §3).
    pub async fn create_session(
        &self,
        id: SessionId,
        model: &str,
        parent: Option<SessionId>,
        name: Option<&str>,
        project: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO sessions (id, created_at, updated_at, title, model, parent, name, project) VALUES (?, ?, ?, '', ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&now)
        .bind(&now)
        .bind(model)
        .bind(parent.map(|p| p.to_string()))
        .bind(name)
        .bind(parent.is_none().then_some(project).flatten())
        .execute(&mut *tx)
        .await?;
        accounting::ensure_owner_in(&mut tx, id, parent).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Set the display title (first user message, truncated by the caller).
    pub async fn set_title(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Pin (or unpin) a session: pinned rows sort first in `list_sessions`.
    pub async fn set_pinned(&self, id: SessionId, pinned: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET pinned = ? WHERE id = ?")
            .bind(pinned)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record a model switch (canonical `provider/model` id).
    pub async fn set_model(&self, id: SessionId, model: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET model = ? WHERE id = ?")
            .bind(model)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record the reasoning effort; `None` clears it (provider default).
    pub async fn set_effort(&self, id: SessionId, effort: Option<&str>) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET effort = ? WHERE id = ?")
            .bind(effort)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark `id` as an external harness session: `agent` is the `agents.yaml`
    /// key, `cwd` the harness's working directory (design doc 06 §3.6).
    pub async fn set_acp(
        &self,
        id: SessionId,
        agent: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET agent = ?, cwd = COALESCE(?, cwd) WHERE id = ?")
            .bind(agent)
            .bind(cwd.map(|p| p.to_string_lossy().into_owned()))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record the harness's own session id, for `session/load` on resume.
    pub async fn set_acp_session(&self, id: SessionId, sid: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET acp_session = ? WHERE id = ?")
            .bind(sid)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The harness session id recorded by [`Store::set_acp_session`], if any.
    pub async fn acp_session(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT acp_session FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(s,)| s))
    }

    /// Record the working directory an ACP client gave `session/new` (design doc 06 §4.1).
    pub async fn set_cwd(&self, id: SessionId, cwd: &Path) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET cwd = ? WHERE id = ?")
            .bind(cwd.to_string_lossy().into_owned())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Append one message; returns its `seq` (max+1 within the session).
    /// Bumps `sessions.updated_at` (the list order).
    pub async fn append(&self, id: SessionId, message: &ChatMessage) -> anyhow::Result<i64> {
        let mut tx = self.pool.begin().await?;
        let seq = append_in(&mut tx, &id.to_string(), message).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// [`Store::append`] plus the plan it produced, in one transaction: the
    /// snapshot is anchored at the new row's seq so it shares its visibility
    /// (47 §5.2). An unchanged plan writes no snapshot and keeps its version.
    pub async fn append_with_todos(
        &self,
        id: SessionId,
        message: &ChatMessage,
        source: TodoSource,
        items: &[Todo],
    ) -> anyhow::Result<(i64, TodoSnapshot)> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        let seq = append_in(&mut tx, &sid, message).await?;
        let snapshot = put_todos(&mut tx, &sid, seq, source, items).await?;
        tx.commit().await?;
        Ok((seq, snapshot))
    }

    /// Record an externally owned plan (ACP) at the latest visible message.
    /// Same change/version rule as [`Store::append_with_todos`].
    pub async fn save_todos(
        &self,
        id: SessionId,
        source: TodoSource,
        items: &[Todo],
    ) -> anyhow::Result<TodoSnapshot> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        let (at_seq,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE session_id = ?1 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807)",
        )
        .bind(&sid)
        .fetch_one(&mut *tx)
        .await?;
        let snapshot = put_todos(&mut tx, &sid, at_seq, source, items).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// The visible plan and its projection version, read together so they
    /// describe one committed state. Empty (never backfilled) when no snapshot
    /// is visible; `Err` for an unknown session or a failed read.
    pub async fn todos(&self, id: SessionId) -> anyhow::Result<TodoSnapshot> {
        let mut conn = self.pool.acquire().await?;
        todos_in(&mut conn, &id.to_string()).await
    }

    /// Record a UI-only failure behind the last visible message (`-1` in an
    /// empty session). Not a context change: no revision or `updated_at` bump.
    pub async fn record_error(&self, id: SessionId, text: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO errors (session_id, at_seq, text) \
             SELECT ?1, COALESCE(MAX(seq), -1), ?2 FROM messages WHERE session_id = ?1 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807)",
        )
        .bind(id.to_string())
        .bind(text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Visible errors as `(at_seq, text)`, in the order they happened.
    pub async fn errors(&self, id: SessionId) -> anyhow::Result<Vec<(i64, String)>> {
        Ok(sqlx::query_as(
            "SELECT at_seq, text FROM errors WHERE session_id = ?1 \
             AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
             ORDER BY at_seq, id",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?)
    }

    /// All visible messages with their seq, in `seq` order. Raw: exclusion
    /// masks are not applied (turn identity, undo/redo, archive).
    pub async fn messages(&self, id: SessionId) -> anyhow::Result<Vec<(i64, ChatMessage)>> {
        let mut conn = self.pool.acquire().await?;
        Ok(rows_in(&mut conn, id, 0)
            .await?
            .into_iter()
            .map(|(seq, m, _)| (seq, m))
            .collect())
    }

    /// Rows hidden by undo (`seq >= cut_at`); empty when `cut_at` is NULL.
    async fn messages_hidden(&self, id: SessionId) -> anyhow::Result<Vec<(i64, ChatMessage)>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seq, body FROM messages WHERE session_id = ?1 \
             AND seq >= (SELECT cut_at FROM sessions WHERE id = ?1) ORDER BY seq",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(decode_rows(id, rows))
    }

    /// `(id, title, updated_at)` of every session, for the search index (26 §2).
    pub async fn search_titles(&self) -> anyhow::Result<Vec<(SessionId, String, String)>> {
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT id, title, updated_at FROM sessions")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(id, title, updated_at)| Ok((id.parse()?, title, updated_at)))
            .collect()
    }

    /// Every visible (`seq < cut_at`) user and assistant message of every
    /// session, for the search index (26 §2), with whether the row is removed
    /// from context (48 §2.2: hits identify removed material). Tool rows are
    /// skipped in SQL so their bodies - the bulk of the table - are never decoded.
    pub async fn search_bodies(&self) -> anyhow::Result<Vec<(SessionId, i64, ChatMessage, bool)>> {
        let rows: Vec<(String, i64, String, bool)> = sqlx::query_as(
            "SELECT m.session_id, m.seq, m.body, \
             COALESCE(json_extract(m.excluded, '$.whole'), 0) \
             FROM messages m JOIN sessions s ON s.id = m.session_id \
             WHERE m.seq < COALESCE(s.cut_at, 9223372036854775807) \
             AND json_extract(m.body, '$.role') IN ('User', 'Assistant') \
             ORDER BY m.session_id, m.seq",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(sid, seq, body, removed)| {
                let id: SessionId = sid.parse().ok()?;
                let m = decode_rows(id, vec![(seq, body)]).pop()?;
                Some((id, m.0, m.1, removed))
            })
            .collect())
    }

    /// Move the undo marker. Snapshot visibility follows it, so when the
    /// selected plan changes the projection version bumps even though history
    /// moved backward (47 §5.2: never resurrect an old version). Every move
    /// changes the visible context, so `context_revision` bumps (48 §3.1).
    async fn set_cut(&self, id: SessionId, cut: Option<i64>) -> anyhow::Result<()> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        let before = todos_in(&mut tx, &sid).await?;
        sqlx::query(
            "UPDATE sessions SET cut_at = ?, context_revision = context_revision + 1 WHERE id = ?",
        )
        .bind(cut)
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        let after = todos_in(&mut tx, &sid).await?;
        if before.source != after.source || before.items != after.items {
            sqlx::query("UPDATE sessions SET todo_version = todo_version + 1 WHERE id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Hide the last visible turn (its user message and everything after).
    /// `None` when nothing is left to hide; `Some` is the hidden turn's user text.
    pub async fn undo(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let visible = self.messages(id).await?;
        let Some(i) = turn_starts(&visible).last() else {
            return Ok(None);
        };
        let (seq, m) = &visible[i];
        self.set_cut(id, Some(*seq)).await?;
        Ok(Some(text_of(&m.content)))
    }

    /// Restore the oldest hidden turn. `None` when nothing is hidden; `Some`
    /// is the restored turn's user text.
    pub async fn redo(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let hidden = self.messages_hidden(id).await?;
        let mut starts = turn_starts(&hidden);
        let Some(first) = starts.next() else {
            return Ok(None);
        };
        let next = starts.next().map(|i| hidden[i].0);
        self.set_cut(id, next).await?;
        Ok(Some(text_of(&hidden[first].1.content)))
    }

    /// Hide `seq >= turn` (a visible turn start) and return the opener row
    /// (design doc 29 §4.1). `Err("not a turn start")` when `turn` is not a
    /// visible turn start; the marker is untouched then.
    pub async fn cut_to(&self, id: SessionId, turn: i64) -> anyhow::Result<ChatMessage> {
        let visible = self.messages(id).await?;
        let i = visible
            .iter()
            .position(|(s, _)| *s == turn)
            .filter(|&i| is_turn_start(i.checked_sub(1).map(|p| &visible[p].1), &visible[i].1));
        let Some(i) = i else {
            anyhow::bail!("not a turn start");
        };
        self.set_cut(id, Some(turn)).await?;
        Ok(visible[i].1.clone())
    }

    /// Delete the hidden tail for real and clear `cut_at`; no-op when nothing
    /// is hidden. Masks go with their rows; the revision bumps because the
    /// deleted seqs can now be reused (48 §3.1).
    pub async fn commit_cut(&self, id: SessionId) -> anyhow::Result<()> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM messages WHERE session_id = ?1 \
             AND seq >= (SELECT cut_at FROM sessions WHERE id = ?1)",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM epochs WHERE session_id = ?1 \
             AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1)",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM todo_snapshots WHERE session_id = ?1 \
             AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1)",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM errors WHERE session_id = ?1 \
             AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1)",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sessions SET cut_at = NULL, context_revision = context_revision + 1 \
             WHERE id = ? AND cut_at IS NOT NULL",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// New session with the source's title/model/effort/cwd and copies of the
    /// visible rows and epochs before the turn after `through` (all visible
    /// rows when `None`). With `reply`, the copy ends at the `reply`-th
    /// (0-based) assistant text of that turn instead, its tool calls dropped so
    /// the fork never ends on a dangling `tool_use`. Returns the new id; the
    /// source is only read (design doc 29 §3, §4.1). `parent`/`name`/`agent`/
    /// `acp_session` stay NULL. Exclusion masks travel with their rows and
    /// heads (48 §4.4); the stripped reply is materialized in its effective
    /// form so old binary positions never land on a shortened part vector.
    pub async fn fork(
        &self,
        src: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> anyhow::Result<SessionId> {
        let mut tx = self.pool.begin().await?;
        let rows = rows_in(&mut tx, src, 0).await?;
        let mut starts = (0..rows.len())
            .filter(|&i| is_turn_start(i.checked_sub(1).map(|p| &rows[p].1), &rows[i].1))
            .map(|i| rows[i].0)
            .peekable();
        if starts.peek().is_none() {
            anyhow::bail!("nothing to fork");
        }
        let mut end = match through {
            None => i64::MAX,
            Some(t) => {
                if !starts.any(|s| s == t) {
                    anyhow::bail!("not a turn start");
                }
                starts.next().unwrap_or(i64::MAX)
            }
        };
        // Mid-turn: stop after the chosen reply; a message that also carried
        // tool calls is copied without them.
        let mut stripped: Option<(i64, String, Option<String>)> = None;
        if let (Some(t), Some(n)) = (through, reply) {
            let (seq, m, mask) = rows
                .iter()
                .filter(|(s, m, _)| {
                    (t..end).contains(s)
                        && m.role == ChatRole::Assistant
                        && !text_of(&m.content).is_empty()
                })
                .nth(n as usize)
                .ok_or_else(|| anyhow::anyhow!("no such reply"))?;
            end = seq + 1;
            if !m.content.tool_calls().is_empty() {
                // A whole-excluded reply keeps its mask over the stripped raw
                // row; a partially masked one is copied effective, mask cleared.
                let (source, excluded) = if mask.whole || mask.binaries.is_empty() {
                    (m.clone(), mask.encode()?)
                } else {
                    (project(m, mask).unwrap_or_else(|| m.clone()), None)
                };
                let parts: Vec<ContentPart> = source
                    .content
                    .iter()
                    .filter(|p| !matches!(p, ContentPart::ToolCall(_)))
                    .cloned()
                    .collect();
                let m = ChatMessage {
                    content: MessageContent::from_parts(parts),
                    ..source
                };
                stripped = Some((*seq, serde_json::to_string(&m)?, excluded));
            }
        }

        let id = SessionId::new();
        let (new, old) = (id.to_string(), src.to_string());
        let now = chrono::Utc::now().to_rfc3339();
        let inserted = sqlx::query(concat!(
            "INSERT INTO sessions (id, created_at, updated_at, title, model, parent, name, effort, cwd, context_revision, project) \
             SELECT ?, ?, ?, title, model, NULL, NULL, effort, cwd, context_revision, ",
            effective_project!(),
            " FROM sessions WHERE id = ?"
        ))
        .bind(&new)
        .bind(&now)
        .bind(&now)
        .bind(&old)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            anyhow::bail!("no such session");
        }
        accounting::ensure_owner_in(&mut tx, id, None).await?;
        sqlx::query(
            "INSERT INTO messages (session_id, seq, body, excluded) \
             SELECT ?1, seq, body, excluded FROM messages WHERE session_id = ?2 AND seq < ?3 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?2), 9223372036854775807)",
        )
        .bind(&new)
        .bind(&old)
        .bind(end)
        .execute(&mut *tx)
        .await?;
        if let Some((seq, body, excluded)) = &stripped {
            sqlx::query(
                "UPDATE messages SET body = ?, excluded = ? WHERE session_id = ? AND seq = ?",
            )
            .bind(body)
            .bind(excluded)
            .bind(&new)
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO epochs (session_id, n, at_seq, tail_from, head, created_at, excluded) \
             SELECT ?1, n, at_seq, tail_from, head, created_at, excluded FROM epochs WHERE session_id = ?2 \
             AND at_seq < ?3 \
             AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?2), 9223372036854775807)",
        )
        .bind(&new)
        .bind(&old)
        .bind(end)
        .execute(&mut *tx)
        .await?;
        // Snapshots through the same boundary; a stripped tool call's response
        // lies past `end`, so its plan is not copied. The fork's own version
        // starts at its snapshot count: non-zero iff it carries a plan.
        sqlx::query(
            "INSERT INTO todo_snapshots (session_id, at_seq, k, source, items) \
             SELECT ?1, at_seq, k, source, items FROM todo_snapshots WHERE session_id = ?2 \
             AND at_seq < ?3 \
             AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?2), 9223372036854775807)",
        )
        .bind(&new)
        .bind(&old)
        .bind(end)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO errors (session_id, at_seq, text) \
             SELECT ?1, at_seq, text FROM errors WHERE session_id = ?2 \
             AND at_seq < ?3 \
             AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?2), 9223372036854775807) \
             ORDER BY at_seq, id",
        )
        .bind(&new)
        .bind(&old)
        .bind(end)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sessions SET todo_version = \
             (SELECT COUNT(*) FROM todo_snapshots WHERE session_id = ?1) WHERE id = ?1",
        )
        .bind(&new)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Number of turns hidden by undo (the redo depth).
    pub async fn hidden_turns(&self, id: SessionId) -> anyhow::Result<u32> {
        let hidden = self.messages_hidden(id).await?;
        Ok(u32::try_from(turn_starts(&hidden).count())?)
    }

    /// All visible epochs of a session, by `n` ascending. Raw heads (masks not applied).
    pub async fn epochs(&self, id: SessionId) -> anyhow::Result<Vec<Epoch>> {
        let mut conn = self.pool.acquire().await?;
        Ok(epochs_in(&mut conn, &id.to_string())
            .await?
            .into_iter()
            .map(|(e, _)| e)
            .collect())
    }

    /// Record a compaction: one `epochs` row. `messages` and the plan are untouched.
    pub async fn compact(
        &self,
        id: SessionId,
        n: u32,
        at_seq: i64,
        tail_from: i64,
        head: &ChatMessage,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        compact_in(&mut tx, &id.to_string(), n, at_seq, tail_from, head).await?;
        tx.commit().await?;
        Ok(())
    }

    /// [`Store::compact`] that also sets the plan (`/clear` passes an empty
    /// one) atomically, anchored at the compaction boundary. Ordered after a
    /// todo update at the same seq; unchanged items write nothing (47 §5.2).
    pub async fn compact_with_todos(
        &self,
        id: SessionId,
        n: u32,
        at_seq: i64,
        tail_from: i64,
        head: &ChatMessage,
        (source, items): (TodoSource, &[Todo]),
    ) -> anyhow::Result<TodoSnapshot> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        let snapshot = put_todos(&mut tx, &sid, at_seq, source, items).await?;
        compact_in(&mut tx, &sid, n, at_seq, tail_from, head).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// What the model sees: latest epoch number, its effective head, and the
    /// effective messages from `tail_from` on (`(0, None, all)` before any
    /// compaction). Exclusion masks are applied (48 §4.1); a `None` head with
    /// `n > 0` means the user removed it, and the tail still starts at its
    /// `tail_from`.
    pub async fn frame(
        &self,
        id: SessionId,
    ) -> anyhow::Result<(u32, Option<ChatMessage>, Vec<(i64, ChatMessage)>)> {
        let mut conn = self.pool.acquire().await?;
        let active = active_in(&mut conn, id).await?;
        let (head, tail) = active.effective();
        Ok((active.epoch, head, tail))
    }

    /// Decimal `sessions.context_revision`; `Err` for an unknown session.
    pub async fn context_revision(&self, id: SessionId) -> anyhow::Result<String> {
        let mut conn = self.pool.acquire().await?;
        Ok(revision_in(&mut conn, &id.to_string()).await?.to_string())
    }

    /// Active-context inventory at the current revision (48 §3.2), read in one
    /// snapshot. `editable`/`reason` are left for the session actor.
    pub async fn context_items(&self, id: SessionId) -> anyhow::Result<ContextSnapshot> {
        let mut tx = self.pool.begin().await?;
        let revision = revision_in(&mut tx, &id.to_string()).await?;
        let active = active_in(&mut tx, id).await?;
        let groups = active.groups();
        Ok(active.snapshot(&revision.to_string(), &groups))
    }

    /// The effective form of one visible row (`None` when removed or emptied
    /// by removals), for regenerate to check its opener before cutting (48 §4.4).
    pub async fn effective_message(
        &self,
        id: SessionId,
        seq: i64,
    ) -> anyhow::Result<Option<ChatMessage>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT body, excluded FROM messages WHERE session_id = ?1 AND seq = ?2 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807)",
        )
        .bind(id.to_string())
        .bind(seq)
        .fetch_optional(&self.pool)
        .await?;
        let Some((body, excluded)) = row else {
            return Ok(None);
        };
        let m: ChatMessage = serde_json::from_str(&body).context("bad message row")?;
        Ok(project(&m, &Mask::decode(excluded.as_deref())?))
    }

    /// Set the title only if the session is still at `revision` (48 §4.3:
    /// a title job that captured pre-edit content is discarded). `Ok(false)`
    /// when the revision moved on or the session is unknown.
    pub async fn set_title_at_revision(
        &self,
        id: SessionId,
        title: &str,
        revision: &str,
    ) -> anyhow::Result<bool> {
        let revision: i64 = revision
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("bad revision {revision:?}"))?;
        let n = sqlx::query("UPDATE sessions SET title = ? WHERE id = ? AND context_revision = ?")
            .bind(title)
            .bind(id.to_string())
            .bind(revision)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n == 1)
    }

    /// Apply a context removal atomically (48 §4.3): refuse a stale
    /// `expected_revision`, resolve `targets` against the persisted active
    /// frame, require protocol closure, validate the projected frame, then
    /// write the masks and bump the revision in one transaction. Nothing is
    /// written on any error. Returns the removal summary and the committed
    /// effective `(epoch, head, tail)` for the actor to install. Exact
    /// already-removed targets are a no-op: unchanged revision, nothing written.
    pub async fn remove_context(
        &self,
        id: SessionId,
        expected_revision: &str,
        targets: &[ContextTarget],
    ) -> anyhow::Result<(
        ContextRemoval,
        u32,
        Option<ChatMessage>,
        Vec<(i64, ChatMessage)>,
    )> {
        let expected: i64 = expected_revision
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("bad revision {expected_revision:?}"))?;
        let sid = id.to_string();
        // IMMEDIATE: writers serialize, so the revision check and the write see one state.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = revision_in(&mut tx, &sid).await?;
        if current != expected {
            anyhow::bail!(
                "stale revision: the context changed (now {current}, confirmed at {expected})"
            );
        }
        let active = active_in(&mut tx, id).await?;
        let groups = active.groups();
        let plan = active.plan(targets, &groups).map_err(anyhow::Error::msg)?;
        if plan.is_empty() {
            let (head, tail) = active.effective();
            let removal = ContextRemoval {
                revision: current.to_string(),
                estimated_tokens: active.estimate(),
                ..ContextRemoval::default()
            };
            return Ok((removal, active.epoch, head, tail));
        }
        let after = active.apply(&plan);
        after.validate().map_err(anyhow::Error::msg)?;
        for (seq, mask) in &plan.rows {
            let n =
                sqlx::query("UPDATE messages SET excluded = ? WHERE session_id = ? AND seq = ?")
                    .bind(mask.encode()?)
                    .bind(&sid)
                    .bind(seq)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
            if n != 1 {
                anyhow::bail!("message {seq} disappeared during the edit");
            }
        }
        if let Some(mask) = &plan.head {
            let n = sqlx::query("UPDATE epochs SET excluded = ? WHERE session_id = ? AND n = ?")
                .bind(mask.encode()?)
                .bind(&sid)
                .bind(i64::from(active.epoch))
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if n != 1 {
                anyhow::bail!("epoch {} disappeared during the edit", active.epoch);
            }
        }
        let (revision,): (i64,) = sqlx::query_as(
            "UPDATE sessions SET context_revision = context_revision + 1 WHERE id = ? \
             RETURNING context_revision",
        )
        .bind(&sid)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(
            session = %id,
            revision,
            messages = plan.messages,
            images = plan.images,
            documents = plan.documents,
            heads = plan.heads,
            "context_edit"
        );
        let (head, tail) = after.effective();
        let removal = ContextRemoval {
            revision: revision.to_string(),
            messages: plan.messages,
            images: plan.images,
            documents: plan.documents,
            heads: plan.heads,
            estimated_tokens: after.estimate(),
        };
        Ok((removal, after.epoch, head, tail))
    }

    /// Raw cut-visible messages and epochs with their context metadata and
    /// the active inventory, all read at one revision (48 §5). Bodies are
    /// the stored rows; previews are the caller's to materialize.
    pub async fn context_history(&self, id: SessionId) -> anyhow::Result<ContextHistory> {
        let sid = id.to_string();
        let mut tx = self.pool.begin().await?;
        let revision = revision_in(&mut tx, &sid).await?.to_string();
        let rows = rows_in(&mut tx, id, 0).await?;
        let epochs = epochs_in(&mut tx, &sid).await?;
        drop(tx);
        let latest = epochs.last().map(|(e, _)| (e.n, e.tail_from));
        let (epoch, tail_from) = latest.unwrap_or((0, 0));
        let active = Active {
            epoch,
            tail_from,
            head: epochs
                .last()
                .map(|(e, mask)| (e.head.clone(), mask.clone())),
            rows: rows
                .iter()
                .filter(|(seq, _, _)| *seq >= tail_from)
                .cloned()
                .collect(),
        };
        let groups = active.groups();
        let snapshot = active.snapshot(&revision, &groups);
        let entries = rows
            .iter()
            .map(|(seq, m, mask)| {
                let active_row = *seq >= tail_from;
                let remove_with = if active_row && !mask.whole {
                    groups.remove_with(*seq)
                } else {
                    Vec::new()
                };
                (
                    *seq,
                    entry(
                        ContextOwner::Message { seq: *seq },
                        m,
                        mask,
                        active_row,
                        remove_with,
                    ),
                )
            })
            .collect();
        let heads = epochs
            .iter()
            .map(|(e, mask)| {
                (
                    e.n,
                    entry(
                        ContextOwner::Head { epoch: e.n },
                        &e.head,
                        mask,
                        e.n == epoch,
                        Vec::new(),
                    ),
                )
            })
            .collect();
        Ok(ContextHistory {
            revision,
            messages: rows.into_iter().map(|(seq, m, _)| (seq, m)).collect(),
            epochs: epochs.into_iter().map(|(e, _)| e).collect(),
            entries,
            heads,
            snapshot,
        })
    }

    /// One session, if it exists.
    pub async fn session(&self, id: SessionId) -> anyhow::Result<Option<SessionSummary>> {
        let row: Option<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions WHERE id = ?"
        ))
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(SessionSummary::try_from).transpose()
    }

    /// Every session, pinned first, then most recently active first.
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let rows: Vec<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions ORDER BY pinned DESC, updated_at DESC, id DESC"
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(SessionSummary::try_from).collect()
    }

    /// The subtree under `root` (children, grandchildren, …) with each row's
    /// depth (1 = direct child), in pre-order: a parent right before its
    /// children, siblings oldest first (design doc 19 §2.1).
    pub async fn descendants(&self, root: SessionId) -> anyhow::Result<Vec<(u8, SessionSummary)>> {
        #[derive(FromRow)]
        struct TreeRow {
            #[sqlx(flatten)]
            row: SessionRow,
            depth: i64,
        }
        // `path` = the created_at||id of every ancestor down to the row, so
        // string order is pre-order with siblings by creation time.
        let rows: Vec<TreeRow> = sqlx::query_as(concat!(
            "WITH RECURSIVE tree(id, depth, path) AS ( \
                 SELECT id, 1, created_at || id FROM sessions WHERE parent = ? \
                 UNION ALL \
                 SELECT s.id, t.depth + 1, t.path || '/' || s.created_at || s.id \
                 FROM sessions s JOIN tree t ON s.parent = t.id) \
             SELECT ",
            session_cols!(),
            ", t.depth FROM sessions JOIN tree t ON sessions.id = t.id ORDER BY t.path"
        ))
        .bind(root.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    u8::try_from(r.depth).context("bad depth")?,
                    SessionSummary::try_from(r.row)?,
                ))
            })
            .collect()
    }

    /// Delete `root` and every descendant with their messages, epochs, plans
    /// and communication accounting (exchanges, A2A contexts/tasks). Spend
    /// stays (51 §6.4): a true root takes its whole ledger tree with it, a
    /// subtree only tombstones its owners.
    /// Returns the removed ids, children before parents.
    pub async fn delete_session(&self, root: SessionId) -> anyhow::Result<Vec<SessionId>> {
        let mut ids: Vec<SessionId> = self
            .descendants(root)
            .await?
            .into_iter()
            .rev() // reverse pre-order: every child precedes its parent
            .map(|(_, s)| s.id)
            .collect();
        ids.push(root);
        let mut tx = self.pool.begin().await?;
        let is_root: bool = sqlx::query_scalar("SELECT parent IS NULL FROM sessions WHERE id = ?")
            .bind(root.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(false);
        if is_root {
            accounting::delete_root_ledger_in(&mut tx, root).await?;
        } else {
            for id in &ids {
                accounting::tombstone_owner_in(&mut tx, *id).await?;
            }
        }
        for id in &ids {
            let sid = id.to_string();
            communication::delete_communication_in(&mut tx, &sid).await?;
            sqlx::query("DELETE FROM messages WHERE session_id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM epochs WHERE session_id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM todo_snapshots WHERE session_id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM errors WHERE session_id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// The child of `parent` named `name`.
    pub async fn child_by_name(
        &self,
        parent: SessionId,
        name: &str,
    ) -> anyhow::Result<Option<SessionId>> {
        let id: Option<(String,)> =
            sqlx::query_as("SELECT id FROM sessions WHERE parent = ? AND name = ?")
                .bind(parent.to_string())
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        id.map(|(s,)| s.parse().context("bad session id"))
            .transpose()
    }

    /// Length of the parent chain (0 for a top-level session).
    pub async fn depth(&self, mut id: SessionId) -> anyhow::Result<u8> {
        let mut depth = 0u8;
        while let Some((Some(parent),)) =
            sqlx::query_as::<_, (Option<String>,)>("SELECT parent FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
        {
            id = parent.parse().context("bad parent id")?;
            depth = depth.saturating_add(1);
        }
        Ok(depth)
    }

    /// Assistant text after the last user message, joined by newlines: the
    /// "final answer" of the most recent turn.
    pub async fn last_turn_text(&self, id: SessionId) -> anyhow::Result<String> {
        let messages = self.messages(id).await?;
        let start = messages
            .iter()
            .rposition(|(_, m)| m.role == ChatRole::User)
            .map_or(0, |i| i + 1);
        Ok(messages[start..]
            .iter()
            .filter(|(_, m)| m.role == ChatRole::Assistant)
            .map(|(_, m)| text_of(&m.content))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Set the project label on the root above `session` (51 §3: a child call
    /// mutates its root; `None` clears). The caller has normalized `project`.
    /// Returns the root id; unknown `session` is an error.
    pub async fn set_project(
        &self,
        session: SessionId,
        project: Option<&str>,
    ) -> anyhow::Result<SessionId> {
        let root: Option<(String,)> = sqlx::query_as(
            "UPDATE sessions SET project = ? WHERE id = ( \
                 WITH RECURSIVE up(id, parent) AS ( \
                     SELECT id, parent FROM sessions WHERE id = ? \
                     UNION ALL \
                     SELECT p.id, p.parent FROM sessions p JOIN up ON p.id = up.parent) \
                 SELECT id FROM up WHERE parent IS NULL) \
             RETURNING id",
        )
        .bind(project)
        .bind(session.to_string())
        .fetch_optional(&self.pool)
        .await?;
        root.map(|(id,)| id.parse().context("bad session id"))
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("no such session"))
    }

    /// The project label in force for `session`: its root's value.
    pub async fn effective_project(&self, session: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> = sqlx::query_as(concat!(
            "SELECT ",
            effective_project!(),
            " FROM sessions WHERE id = ?"
        ))
        .bind(session.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(p,)| p)
            .ok_or_else(|| anyhow::anyhow!("no such session"))
    }
}

/// A user message that opens a turn: first in the session or right after an
/// assistant reply. Steers and notes follow Tool/User rows and never count.
pub(crate) fn is_turn_start(prev: Option<&ChatMessage>, m: &ChatMessage) -> bool {
    m.role == ChatRole::User && prev.is_none_or(|p| p.role == ChatRole::Assistant)
}

/// Indices of turn starts within `rows` (in order).
fn turn_starts(rows: &[(i64, ChatMessage)]) -> impl Iterator<Item = usize> + '_ {
    (0..rows.len())
        .filter(move |&i| is_turn_start(i.checked_sub(1).map(|p| &rows[p].1), &rows[i].1))
}

fn decode_rows(id: SessionId, rows: Vec<(i64, String)>) -> Vec<(i64, ChatMessage)> {
    rows.into_iter()
        .filter_map(|(seq, body)| match serde_json::from_str(&body) {
            Ok(m) => Some((seq, m)),
            Err(e) => {
                tracing::warn!(session = %id, seq, error = %e, "skipping undecodable message row");
                None
            }
        })
        .collect()
}

/// Insert one message at max+1, touch `updated_at` and bump the context
/// revision; the caller's transaction, so a failed write mints no seq (48 §3.1).
async fn append_in(
    conn: &mut SqliteConnection,
    sid: &str,
    message: &ChatMessage,
) -> anyhow::Result<i64> {
    let body = serde_json::to_string(message)?;
    let (seq,): (i64,) = sqlx::query_as(
        "INSERT INTO messages (session_id, seq, body) \
         SELECT ?1, COALESCE(MAX(seq), -1) + 1, ?2 FROM messages WHERE session_id = ?1 \
         RETURNING seq",
    )
    .bind(sid)
    .bind(body)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE sessions SET updated_at = ?, context_revision = context_revision + 1 WHERE id = ?",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(sid)
    .execute(conn)
    .await?;
    Ok(seq)
}

/// One `epochs` row plus the revision bump; the caller's transaction.
async fn compact_in(
    conn: &mut SqliteConnection,
    sid: &str,
    n: u32,
    at_seq: i64,
    tail_from: i64,
    head: &ChatMessage,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO epochs (session_id, n, at_seq, tail_from, head, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(sid)
    .bind(i64::from(n))
    .bind(at_seq)
    .bind(tail_from)
    .bind(serde_json::to_string(head)?)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sessions SET context_revision = context_revision + 1 WHERE id = ?")
        .bind(sid)
        .execute(conn)
        .await?;
    Ok(())
}

/// `sessions.context_revision`; `Err("no such session")` when missing.
async fn revision_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<i64> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT context_revision FROM sessions WHERE id = ?")
        .bind(sid)
        .fetch_optional(conn)
        .await?;
    row.map(|(r,)| r)
        .ok_or_else(|| anyhow::anyhow!("no such session"))
}

/// Visible rows from `from` with their masks, seq ascending. Undecodable
/// bodies are skipped (warned); an undecodable mask is an error, because
/// replaying a row the user removed is the one wrong answer.
async fn rows_in(
    conn: &mut SqliteConnection,
    id: SessionId,
    from: i64,
) -> anyhow::Result<Vec<(i64, ChatMessage, Mask)>> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT seq, body, excluded FROM messages WHERE session_id = ?1 AND seq >= ?2 \
         AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
         ORDER BY seq",
    )
    .bind(id.to_string())
    .bind(from)
    .fetch_all(conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (seq, body, excluded) in rows {
        let mask =
            Mask::decode(excluded.as_deref()).with_context(|| format!("message {seq} of {id}"))?;
        if let Some((seq, m)) = decode_rows(id, vec![(seq, body)]).pop() {
            out.push((seq, m, mask));
        }
    }
    Ok(out)
}

/// Visible epochs with their head masks, by `n` ascending.
async fn epochs_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<Vec<(Epoch, Mask)>> {
    let rows: Vec<(i64, i64, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT n, at_seq, tail_from, head, excluded FROM epochs WHERE session_id = ?1 \
         AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
         ORDER BY n",
    )
    .bind(sid)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(n, at_seq, tail_from, head, excluded)| {
            Ok((
                Epoch {
                    n: u32::try_from(n).context("bad epoch n")?,
                    at_seq,
                    tail_from,
                    head: serde_json::from_str(&head).context("bad epoch head")?,
                },
                Mask::decode(excluded.as_deref()).with_context(|| format!("epoch {n}"))?,
            ))
        })
        .collect()
}

/// The persisted active frame: the latest visible epoch (boundary resolved
/// before any projection) and the raw rows from its `tail_from`, with masks.
async fn active_in(conn: &mut SqliteConnection, id: SessionId) -> anyhow::Result<Active> {
    let latest = epochs_in(conn, &id.to_string()).await?.pop();
    let (epoch, tail_from, head) = match latest {
        Some((e, mask)) => (e.n, e.tail_from, Some((e.head, mask))),
        None => (0, 0, None),
    };
    Ok(Active {
        epoch,
        tail_from,
        head,
        rows: rows_in(conn, id, tail_from).await?,
    })
}

/// Version and latest visible snapshot in one statement, so both describe the
/// same committed state. `Err("no such session")` rather than an empty plan.
async fn todos_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<TodoSnapshot> {
    let row: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.todo_version, t.source, t.items FROM sessions s \
         LEFT JOIN todo_snapshots t ON t.session_id = s.id \
         AND t.at_seq < COALESCE(s.cut_at, 9223372036854775807) \
         WHERE s.id = ?1 ORDER BY t.at_seq DESC, t.k DESC LIMIT 1",
    )
    .bind(sid)
    .fetch_optional(conn)
    .await?;
    let Some((version, source, items)) = row else {
        anyhow::bail!("no such session");
    };
    let version = u64::try_from(version).context("bad todo_version")?;
    Ok(match (source, items) {
        (Some(source), Some(items)) => TodoSnapshot {
            version,
            source: source.parse().map_err(anyhow::Error::msg)?,
            items: serde_json::from_str(&items).context("bad todo items")?,
        },
        _ => TodoSnapshot {
            version,
            ..TodoSnapshot::default()
        },
    })
}

/// Make `items` the plan at `at_seq` (next `k` under that anchor) and bump the
/// version, unless it equals the visible plan: then nothing is written and the
/// current snapshot is returned. The caller's transaction.
async fn put_todos(
    conn: &mut SqliteConnection,
    sid: &str,
    at_seq: i64,
    source: TodoSource,
    items: &[Todo],
) -> anyhow::Result<TodoSnapshot> {
    let current = todos_in(conn, sid).await?;
    if current.source == source && current.items == items {
        return Ok(current);
    }
    sqlx::query(
        "INSERT INTO todo_snapshots (session_id, at_seq, k, source, items) \
         SELECT ?1, ?2, COALESCE(MAX(k), -1) + 1, ?3, ?4 FROM todo_snapshots \
         WHERE session_id = ?1 AND at_seq = ?2",
    )
    .bind(sid)
    .bind(at_seq)
    .bind(source.as_str())
    .bind(serde_json::to_string(items)?)
    .execute(&mut *conn)
    .await?;
    let version = current.version + 1;
    sqlx::query("UPDATE sessions SET todo_version = ? WHERE id = ?")
        .bind(i64::try_from(version).context("todo_version overflow")?)
        .bind(sid)
        .execute(conn)
        .await?;
    Ok(TodoSnapshot {
        version,
        source,
        items: items.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use genai::chat::{MessageContent, ToolCall, ToolResponse};

    use super::*;

    /// Fresh store in a temp dir with one session; caller removes `dir` at the end.
    async fn fresh() -> (Store, SessionId, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("wth-store-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite")).await.unwrap();
        let id = SessionId::new();
        store.create_session(id, "m", None, None, None).await.unwrap();
        (store, id, dir)
    }

    async fn append_all(store: &Store, id: SessionId, msgs: impl IntoIterator<Item = ChatMessage>) {
        for m in msgs {
            store.append(id, &m).await.unwrap();
        }
    }

    fn texts(rows: &[(i64, ChatMessage)]) -> Vec<(i64, String)> {
        rows.iter()
            .map(|(s, m)| (*s, text_of(&m.content)))
            .collect()
    }

    fn asst(t: &str) -> ChatMessage {
        ChatMessage::assistant(MessageContent::from_text(t))
    }

    #[tokio::test]
    async fn resume_round_trip() {
        let (store, id, dir) = fresh().await;
        for (i, m) in [
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant(MessageContent::from_text("hello")),
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(store.append(id, m).await.unwrap(), i as i64);
        }

        let got = store.messages(id).await.unwrap();
        let flat: Vec<(ChatRole, String)> = got
            .iter()
            .map(|(_, m)| (m.role.clone(), m.content.joined_texts().unwrap_or_default()))
            .collect();
        assert_eq!(
            flat,
            [
                (ChatRole::System, "sys".to_string()),
                (ChatRole::User, "hi".to_string()),
                (ChatRole::Assistant, "hello".to_string()),
            ]
        );

        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].parent, None);
        assert_eq!(list[0].title, "");

        let child = SessionId::new();
        store.create_session(child, "m", Some(id), Some("kid"), None)
            .await
            .unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), [child, id]);
        assert_eq!(list[0].parent, Some(id));
        assert_eq!(list[0].name.as_deref(), Some("kid"));
        // Activity, not creation, orders the list: a message in the older session moves it to the top.
        tokio::time::sleep(Duration::from_millis(2)).await;
        store.append(id, &ChatMessage::user("again")).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), [id, child]);
        assert!(list[0].updated_at > list[0].created_at);
        // A pin outranks activity; unpinning restores the activity order.
        store.set_pinned(child, true).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(
            list.iter().map(|s| (s.id, s.pinned)).collect::<Vec<_>>(),
            [(child, true), (id, false)]
        );
        store.set_pinned(child, false).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), [id, child]);
        assert_eq!(
            store
                .descendants(id)
                .await
                .unwrap()
                .iter()
                .map(|(d, s)| (*d, s.id))
                .collect::<Vec<_>>(),
            [(1, child)]
        );
        assert_eq!(store.child_by_name(id, "kid").await.unwrap(), Some(child));
        assert_eq!(store.child_by_name(id, "nope").await.unwrap(), None);
        assert_eq!(store.depth(id).await.unwrap(), 0);
        assert_eq!(store.depth(child).await.unwrap(), 1);

        store.set_title(id, "first words").await.unwrap();
        assert_eq!(
            store.session(id).await.unwrap().unwrap().title,
            "first words"
        );
        assert!(store.session(SessionId::new()).await.unwrap().is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn descendants_is_preorder_with_depth() {
        let (store, root, dir) = fresh().await;
        let a = SessionId::new();
        let b = SessionId::new();
        let a1 = SessionId::new();
        let other = SessionId::new();
        store.create_session(a, "m", Some(root), Some("a"), None)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        store.create_session(a1, "m", Some(a), Some("a1"), None)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        store.create_session(b, "m", Some(root), Some("b"), None)
            .await
            .unwrap();
        store.create_session(other, "m", None, None, None).await.unwrap();

        let got: Vec<(u8, SessionId)> = store
            .descendants(root)
            .await
            .unwrap()
            .iter()
            .map(|(d, s)| (*d, s.id))
            .collect();
        // a, then a's child a1 right under it, then b; `other` is not in the tree.
        assert_eq!(got, [(1, a), (2, a1), (1, b)]);
        assert_eq!(
            store
                .descendants(a)
                .await
                .unwrap()
                .iter()
                .map(|(d, s)| (*d, s.id))
                .collect::<Vec<_>>(),
            [(1, a1)]
        );
        assert!(store.descendants(b).await.unwrap().is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn acp_columns_round_trip() {
        let (store, id, dir) = fresh().await;
        let row = store.session(id).await.unwrap().unwrap();
        assert_eq!((row.agent, row.cwd), (None, None));
        assert_eq!(store.acp_session(id).await.unwrap(), None);

        store
            .set_acp(id, "codex", Some(Path::new("/tmp/w")))
            .await
            .unwrap();
        store.set_acp_session(id, "s-1").await.unwrap();
        let row = store.session(id).await.unwrap().unwrap();
        assert_eq!(row.agent.as_deref(), Some("codex"));
        assert_eq!(row.cwd.as_deref(), Some("/tmp/w"));
        assert_eq!(store.acp_session(id).await.unwrap().as_deref(), Some("s-1"));
        assert_eq!(
            store.list_sessions().await.unwrap()[0].agent.as_deref(),
            Some("codex")
        );

        // `set_acp` without a cwd keeps the recorded one; `set_cwd` replaces it.
        store.set_acp(id, "codex", None).await.unwrap();
        assert_eq!(
            store.session(id).await.unwrap().unwrap().cwd.as_deref(),
            Some("/tmp/w")
        );
        store.set_cwd(id, Path::new("/tmp/v")).await.unwrap();
        assert_eq!(
            store.session(id).await.unwrap().unwrap().cwd.as_deref(),
            Some("/tmp/v")
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn last_turn_text_joins_assistant_text_after_last_user() {
        let (store, id, dir) = fresh().await;
        for m in [
            ChatMessage::user("earlier"),
            ChatMessage::assistant(MessageContent::from_text("stale")),
            ChatMessage::user("task"),
            ChatMessage::assistant(MessageContent::from_text("first")),
            ChatMessage::from(vec![ToolCall {
                call_id: "c1".into(),
                fn_name: "shell".into(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]),
            ChatMessage::from(vec![ToolResponse::new("c1", "out")]),
            ChatMessage::assistant(MessageContent::from_text("second")),
        ] {
            store.append(id, &m).await.unwrap();
        }
        assert_eq!(store.last_turn_text(id).await.unwrap(), "first\nsecond");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn compact_frames_from_tail() {
        let (store, id, dir) = fresh().await;
        for i in 0..6 {
            store
                .append(id, &ChatMessage::user(format!("m{i}")))
                .await
                .unwrap();
        }
        let (n, head, tail) = store.frame(id).await.unwrap();
        assert_eq!((n, head.is_none(), tail.len()), (0, true, 6));

        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store.compact(id, 1, 5, 4, &head).await.unwrap();
        let (n, got, tail) = store.frame(id).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(text_of(&got.unwrap().content), text_of(&head.content));
        assert_eq!(
            tail.iter()
                .map(|(s, m)| (*s, text_of(&m.content)))
                .collect::<Vec<_>>(),
            [(4, "m4".to_string()), (5, "m5".to_string())]
        );
        let epochs = store.epochs(id).await.unwrap();
        assert_eq!(epochs.len(), 1);
        assert_eq!(
            (epochs[0].n, epochs[0].at_seq, epochs[0].tail_from),
            (1, 5, 4)
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 13 §6.5: a multi-part snap head (caption, PNG frames, closing tag)
    /// survives the epochs row byte for byte.
    #[tokio::test]
    async fn snap_head_round_trips_parts() {
        use base64::Engine as _;
        use genai::chat::ContentPart;

        use crate::{compact, snap};

        let (store, id, dir) = fresh().await;
        append_all(&store, id, [ChatMessage::user("m0"), asst("a0")]).await;
        let caption = "Archive of 2 folded messages as 2 image(s), most recent last.";
        let shape = snap::shape("anthropic");
        let png = snap::rasterize(&["x".into()], &shape).remove(0);
        let frame = |i: usize| {
            ContentPart::from_binary_base64(
                "image/png",
                base64::engine::general_purpose::STANDARD.encode(&png),
                Some(format!("frame-{i}.png")),
            )
        };
        let head = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(compact::wrap_head_open(
                1,
                compact::CompactStyle::Snap,
                caption,
                [String::from("Roster: a")],
            )),
            frame(0),
            frame(1),
            ContentPart::Text(compact::HEAD_CLOSE.into()),
        ]));
        store.compact(id, 1, 1, 1, &head).await.unwrap();

        let (n, got, tail) = store.frame(id).await.unwrap();
        let got = got.unwrap();
        assert_eq!((n, tail.len()), (1, 1));
        assert_eq!(got.content.parts().len(), 4);
        assert_eq!(
            got.content.parts()[1]
                .as_binary()
                .map(|b| b.content_type.as_str()),
            Some("image/png")
        );
        let text = text_of(&got.content);
        assert_eq!(compact::style_of_head(&text), compact::CompactStyle::Snap);
        assert!(text.contains(caption));
        assert!(text.contains("Roster: a"));
        assert!(text.ends_with(compact::HEAD_CLOSE));
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            serde_json::to_string(&head).unwrap()
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn undo_redo_hides_whole_turns_including_steers() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                asst("a2"),
                ChatMessage::user("u3"),
                ChatMessage::from(vec![ToolCall {
                    call_id: "c1".into(),
                    fn_name: "shell".into(),
                    fn_arguments: serde_json::json!({}),
                    thought_signatures: None,
                }]),
                ChatMessage::from(vec![ToolResponse::new("c1", "out")]),
                ChatMessage::user("steer"),
                asst("a3"),
            ],
        )
        .await;
        assert_eq!(store.hidden_turns(id).await.unwrap(), 0);

        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u3"));
        assert_eq!(
            texts(&store.messages(id).await.unwrap()),
            [
                (0, "u1".into()),
                (1, "a1".into()),
                (2, "u2".into()),
                (3, "a2".into())
            ]
        );
        assert_eq!(store.frame(id).await.unwrap().2.len(), 4);
        assert_eq!(store.last_turn_text(id).await.unwrap(), "a2");

        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 2);
        assert_eq!(store.messages(id).await.unwrap().len(), 2);

        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 1);
        assert_eq!(store.messages(id).await.unwrap().len(), 4);

        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u3"));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 0);
        assert_eq!(store.messages(id).await.unwrap().len(), 9);

        assert_eq!(store.redo(id).await.unwrap(), None);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn undo_hides_epochs_inside_the_turn() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store.compact(id, 1, 3, 2, &head).await.unwrap();
        assert_eq!(store.frame(id).await.unwrap().0, 1);

        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert!(store.epochs(id).await.unwrap().is_empty());
        let (n, head, tail) = store.frame(id).await.unwrap();
        assert_eq!((n, head.is_none(), tail.len()), (0, true, 2));

        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.epochs(id).await.unwrap().len(), 1);
        assert_eq!(store.frame(id).await.unwrap().0, 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn commit_cut_deletes_hidden_tail() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;
        store.commit_cut(id).await.unwrap();
        assert_eq!(store.messages(id).await.unwrap().len(), 4);

        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        store.commit_cut(id).await.unwrap();
        assert_eq!(
            texts(&store.messages(id).await.unwrap()),
            [(0, "u1".into()), (1, "a1".into())]
        );
        assert_eq!(
            store.append(id, &ChatMessage::user("u2b")).await.unwrap(),
            2
        );
        assert_eq!(store.redo(id).await.unwrap(), None);
        assert_eq!(store.hidden_turns(id).await.unwrap(), 0);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn errors_follow_their_turn() {
        let (store, id, dir) = fresh().await;
        // Failure before any message anchors at -1 and is never hidden.
        store.record_error(id, "e0").await.unwrap();
        append_all(&store, id, [ChatMessage::user("u1"), asst("a1")]).await;
        // Two consecutive failed requests behind the same opener keep order.
        store.append(id, &ChatMessage::user("u2")).await.unwrap();
        store.record_error(id, "e1").await.unwrap();
        store.record_error(id, "e2").await.unwrap();
        let all = vec![(-1, "e0".into()), (2, "e1".into()), (2, "e2".into())];
        assert_eq!(store.errors(id).await.unwrap(), all);
        assert_eq!(
            texts(&store.frame(id).await.unwrap().2),
            [(0, "u1".into()), (1, "a1".into()), (2, "u2".into())]
        );
        drop(store);
        let store = Store::open(&dir.join("sessions.sqlite")).await.unwrap();
        assert_eq!(store.errors(id).await.unwrap(), all);

        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.errors(id).await.unwrap(), [(-1, "e0".into())]);
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.errors(id).await.unwrap(), all);
        let whole = store.fork(id, None, None).await.unwrap();
        assert_eq!(store.errors(whole).await.unwrap(), all);

        // A fork through u1 leaves u2's failures behind.
        let forked = store.fork(id, Some(0), None).await.unwrap();
        assert_eq!(store.errors(forked).await.unwrap(), [(-1, "e0".into())]);

        store.undo(id).await.unwrap();
        store.commit_cut(id).await.unwrap();
        assert_eq!(store.errors(id).await.unwrap(), [(-1, "e0".into())]);
        store.delete_session(id).await.unwrap();
        let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM errors WHERE session_id = ?")
            .bind(id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(left.0, 0);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cut_to_accepts_only_turn_starts() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                ChatMessage::from(vec![ToolCall {
                    call_id: "c1".into(),
                    fn_name: "shell".into(),
                    fn_arguments: serde_json::json!({}),
                    thought_signatures: None,
                }]),
                ChatMessage::from(vec![ToolResponse::new("c1", "out")]),
                ChatMessage::user("steer"),
                asst("a2"),
            ],
        )
        .await;

        // A steer (seq 5), an assistant row (seq 1) and an unknown seq are refused and hide nothing.
        for bad in [5, 1, 99] {
            let err = store.cut_to(id, bad).await.unwrap_err();
            assert_eq!(err.to_string(), "not a turn start");
        }
        assert_eq!(store.hidden_turns(id).await.unwrap(), 0);
        assert_eq!(store.messages(id).await.unwrap().len(), 7);

        let opener = store.cut_to(id, 2).await.unwrap();
        assert_eq!(text_of(&opener.content), "u2");
        assert_eq!(
            texts(&store.messages(id).await.unwrap()),
            [(0, "u1".into()), (1, "a1".into())]
        );
        assert_eq!(store.hidden_turns(id).await.unwrap(), 1);
        // The hidden turn is once again not visible, so it cannot be cut at.
        assert_eq!(
            store.cut_to(id, 2).await.unwrap_err().to_string(),
            "not a turn start"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fork_copies_turns_through_the_given_one() {
        let (store, src, dir) = fresh().await;
        store.set_title(src, "topic").await.unwrap();
        append_all(
            &store,
            src,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store.compact(src, 1, 1, 1, &head).await.unwrap();
        store.compact(src, 2, 3, 2, &head).await.unwrap();

        assert_eq!(
            store
                .fork(src, Some(1), None)
                .await
                .unwrap_err()
                .to_string(),
            "not a turn start"
        );

        // Through turn A (seq 0): A's rows and the epoch inside A only.
        let a = store.fork(src, Some(0), None).await.unwrap();
        assert_eq!(
            texts(&store.messages(a).await.unwrap()),
            [(0, "u1".into()), (1, "a1".into())]
        );
        let epochs = store.epochs(a).await.unwrap();
        assert_eq!(epochs.len(), 1);
        assert_eq!((epochs[0].n, epochs[0].at_seq), (1, 1));
        let row = store.session(a).await.unwrap().unwrap();
        assert_eq!((row.title.as_str(), row.model.as_str()), ("topic", "m"));
        assert_eq!((row.parent, row.name, row.agent), (None, None, None));
        // A fork appends where the copy ends.
        assert_eq!(store.append(a, &ChatMessage::user("u2b")).await.unwrap(), 2);

        // Through the last turn / `None`: everything visible.
        for through in [Some(2), None] {
            let all = store.fork(src, through, None).await.unwrap();
            assert_eq!(
                texts(&store.messages(all).await.unwrap()),
                texts(&store.messages(src).await.unwrap())
            );
            assert_eq!(store.epochs(all).await.unwrap().len(), 2);
        }

        // The source is untouched.
        assert_eq!(
            texts(&store.messages(src).await.unwrap()),
            [
                (0, "u1".into()),
                (1, "a1".into()),
                (2, "u2".into()),
                (3, "a2".into())
            ]
        );
        assert_eq!(store.epochs(src).await.unwrap().len(), 2);
        assert_eq!(store.hidden_turns(src).await.unwrap(), 0);

        // Undone rows are not part of a fork; nothing visible → nothing to fork.
        assert_eq!(store.undo(src).await.unwrap().as_deref(), Some("u2"));
        let cut = store.fork(src, None, None).await.unwrap();
        assert_eq!(store.messages(cut).await.unwrap().len(), 2);
        assert_eq!(store.epochs(cut).await.unwrap().len(), 1);
        assert_eq!(store.undo(src).await.unwrap().as_deref(), Some("u1"));
        assert_eq!(
            store.fork(src, None, None).await.unwrap_err().to_string(),
            "nothing to fork"
        );
        assert_eq!(
            store
                .fork(SessionId::new(), None, None)
                .await
                .unwrap_err()
                .to_string(),
            "nothing to fork"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fork at a reply inside a multi-round turn ends on that reply with its
    /// tool calls dropped; later rounds and steers stay behind.
    #[tokio::test]
    async fn fork_at_a_reply_stops_there_without_tool_calls() {
        use genai::chat::{ContentPart, MessageContent, ToolCall, ToolResponse};
        let (store, src, dir) = fresh().await;
        let call = ToolCall {
            thought_signatures: None,
            call_id: "c1".into(),
            fn_name: "shell".into(),
            fn_arguments: serde_json::json!({ "cmd": "ls" }),
        };
        let first = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::Text("looking".into()),
            ContentPart::ToolCall(call),
        ]));
        append_all(
            &store,
            src,
            [
                ChatMessage::user("u1"),
                first,
                ChatMessage::from(vec![ToolResponse::new("c1", "ok")]),
                ChatMessage::user("steer"),
                asst("done"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;

        let f = store.fork(src, Some(0), Some(0)).await.unwrap();
        let rows = store.messages(f).await.unwrap();
        assert_eq!(texts(&rows), [(0, "u1".into()), (1, "looking".into())]);
        assert!(
            rows[1].1.content.tool_calls().is_empty(),
            "tool call dropped"
        );
        // The second reply is the turn's last text: the tool round before it comes along.
        let g = store.fork(src, Some(0), Some(1)).await.unwrap();
        assert_eq!(store.messages(g).await.unwrap().len(), 5);
        assert_eq!(
            store
                .fork(src, Some(0), Some(2))
                .await
                .unwrap_err()
                .to_string(),
            "no such reply"
        );
        // The source still has the call.
        assert!(
            !store.messages(src).await.unwrap()[1]
                .1
                .content
                .tool_calls()
                .is_empty()
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- todo snapshots (47 §5.2) ----

    use crate::todo::Status;

    fn todo(text: &str, status: Status) -> Todo {
        Todo {
            text: text.into(),
            status,
            reason: None,
        }
    }

    fn todo_call(id: &str) -> ChatMessage {
        ChatMessage::from(vec![ToolCall {
            call_id: id.into(),
            fn_name: "todo".into(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }])
    }

    fn resp(id: &str) -> ChatMessage {
        ChatMessage::from(vec![ToolResponse::new(id, "ok")])
    }

    async fn snapshot_rows(store: &Store, id: SessionId) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM todo_snapshots WHERE session_id = ?")
            .bind(id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn todos_version_bumps_only_on_material_change() {
        let (store, id, dir) = fresh().await;
        assert_eq!(store.todos(id).await.unwrap(), TodoSnapshot::default());
        assert!(
            store.todos(SessionId::new()).await.is_err(),
            "unknown session is not an empty plan"
        );

        let plan = [todo("a", Status::InProgress), todo("b", Status::Pending)];
        append_all(&store, id, [ChatMessage::user("u1"), todo_call("c1")]).await;
        let (seq, snap) = store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &plan)
            .await
            .unwrap();
        assert_eq!(seq, 2);
        assert_eq!(
            (snap.version, snap.source, snap.items.as_slice()),
            (1, TodoSource::Native, &plan[..])
        );

        // Same list again: the response lands, the plan does not move.
        let (seq, snap) = store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &plan)
            .await
            .unwrap();
        assert_eq!((seq, snap.version), (3, 1));
        assert_eq!(snapshot_rows(&store, id).await, 1);
        assert_eq!(store.messages(id).await.unwrap().len(), 4);

        let plan2 = [todo("a", Status::Done), todo("b", Status::InProgress)];
        let (_, snap) = store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &plan2)
            .await
            .unwrap();
        assert_eq!(snap.version, 2);
        // A source change alone is material.
        let snap = store.save_todos(id, TodoSource::Acp, &plan2).await.unwrap();
        assert_eq!((snap.version, snap.source), (3, TodoSource::Acp));
        assert_eq!(store.todos(id).await.unwrap(), snap);

        // Failed transactions leave neither a row nor a plan behind.
        let ghost = SessionId::new();
        assert!(
            store
                .append_with_todos(ghost, &resp("c1"), TodoSource::Native, &plan)
                .await
                .is_err()
        );
        assert!(store.messages(ghost).await.unwrap().is_empty());
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store
            .compact_with_todos(id, 1, 4, 4, &head, (TodoSource::Acp, &plan2))
            .await
            .unwrap();
        let before = store.todos(id).await.unwrap();
        // Duplicate epoch number fails after the snapshot insert: both roll back.
        assert!(
            store
                .compact_with_todos(id, 1, 4, 4, &head, (TodoSource::Native, &[]))
                .await
                .is_err()
        );
        assert_eq!(store.todos(id).await.unwrap(), before);
        assert_eq!(snapshot_rows(&store, id).await, 3);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn clear_at_compaction_orders_after_update_at_same_seq() {
        let (store, id, dir) = fresh().await;
        let plan = [todo("a", Status::InProgress)];
        append_all(&store, id, [ChatMessage::user("u1"), todo_call("c1")]).await;
        let (at, _) = store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &plan)
            .await
            .unwrap();
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        // Ordinary compaction at that boundary keeps the plan and writes nothing.
        store.compact(id, 1, at, at, &head).await.unwrap();
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (1, &plan[..]));
        assert_eq!(snapshot_rows(&store, id).await, 1);

        // `/clear` at the very same seq: the ordered key puts it after the update.
        let cleared = store
            .compact_with_todos(id, 2, at, at, &head, (TodoSource::Native, &[]))
            .await
            .unwrap();
        assert_eq!((cleared.version, cleared.items.is_empty()), (2, true));
        assert_eq!(store.todos(id).await.unwrap(), cleared);
        assert_eq!(store.epochs(id).await.unwrap().len(), 2);

        // Hiding the turn hides both snapshots; empty -> empty is no change.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u1"));
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.is_empty()), (2, true));
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u1"));
        assert_eq!(store.todos(id).await.unwrap(), cleared);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn undo_redo_select_plan_under_cut_with_monotonic_version() {
        let (store, id, dir) = fresh().await;
        let p1 = [todo("a", Status::InProgress)];
        let p2 = [todo("a", Status::Done), todo("b", Status::InProgress)];
        append_all(&store, id, [ChatMessage::user("u1"), todo_call("c1")]).await;
        store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &p1)
            .await
            .unwrap();
        append_all(
            &store,
            id,
            [asst("a1"), ChatMessage::user("u2"), todo_call("c2")],
        )
        .await;
        store
            .append_with_todos(id, &resp("c2"), TodoSource::Native, &p2)
            .await
            .unwrap();
        append_all(
            &store,
            id,
            [asst("a2"), ChatMessage::user("u3"), asst("a3")],
        )
        .await;
        assert_eq!(store.todos(id).await.unwrap().version, 2);

        // Hiding a turn without todo activity leaves plan and version alone.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u3"));
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (2, &p2[..]));

        // Older list, newer version.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (3, &p1[..]));
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (4, &p2[..]));

        // Committing the cut deletes the hidden plan with the hidden rows.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        store.commit_cut(id).await.unwrap();
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (5, &p1[..]));
        assert_eq!(snapshot_rows(&store, id).await, 1);
        assert_eq!(store.redo(id).await.unwrap(), None);

        // Rewinding to the first turn leaves no visible plan, still forward.
        store.cut_to(id, 0).await.unwrap();
        let snap = store.todos(id).await.unwrap();
        assert_eq!((snap.version, snap.items.is_empty()), (6, true));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fork_and_delete_carry_plan_through_boundary() {
        let (store, src, dir) = fresh().await;
        let p1 = [todo("a", Status::InProgress)];
        let p2 = [todo("a", Status::Done)];
        append_all(&store, src, [ChatMessage::user("u1"), todo_call("c1")]).await;
        store
            .append_with_todos(src, &resp("c1"), TodoSource::Native, &p1)
            .await
            .unwrap();
        let reply_with_call = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::Text("working".into()),
            ContentPart::ToolCall(ToolCall {
                call_id: "c2".into(),
                fn_name: "todo".into(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }),
        ]));
        append_all(
            &store,
            src,
            [asst("a1"), ChatMessage::user("u2"), reply_with_call],
        )
        .await;
        store
            .append_with_todos(src, &resp("c2"), TodoSource::Native, &p2)
            .await
            .unwrap();
        store.append(src, &asst("a2")).await.unwrap();

        let whole = store.fork(src, None, None).await.unwrap();
        let snap = store.todos(whole).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (2, &p2[..]));

        let first = store.fork(src, Some(0), None).await.unwrap();
        let snap = store.todos(first).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (1, &p1[..]));

        // Mid-turn at the stripped reply: its unexecuted call contributes no plan.
        let mid = store.fork(src, Some(4), Some(0)).await.unwrap();
        let snap = store.todos(mid).await.unwrap();
        assert_eq!((snap.version, snap.items.as_slice()), (1, &p1[..]));
        // Forks have their own version: a change there is version 2, not 3.
        assert_eq!(
            store
                .save_todos(mid, TodoSource::Native, &[])
                .await
                .unwrap()
                .version,
            2
        );
        assert_eq!(store.todos(src).await.unwrap().version, 2);

        store.delete_session(mid).await.unwrap();
        assert!(store.todos(mid).await.is_err());
        assert_eq!(snapshot_rows(&store, mid).await, 0);
        assert_eq!(snapshot_rows(&store, src).await, 2);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // region:    --- Context removal (48)

    use crate::compact;
    use crate::context::{ContextKind, ContextOwner, ContextTarget, IMAGE_REMOVED};
    use genai::chat::Binary;

    fn img(name: &str) -> Binary {
        Binary::from_base64("image/png", "AAAA", Some(name.into()))
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            fn_name: "shell".into(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    fn msg(seq: i64) -> ContextTarget {
        ContextTarget::Message { seq }
    }

    fn bin(seq: i64, part: u32, image: Option<u32>) -> ContextTarget {
        ContextTarget::Binary {
            owner: ContextOwner::Message { seq },
            part,
            image,
        }
    }

    fn names(m: &ChatMessage) -> Vec<String> {
        let mut out: Vec<String> = m
            .content
            .binaries()
            .iter()
            .map(|b| b.name.clone().unwrap_or_default())
            .collect();
        for r in m.content.tool_responses() {
            out.extend(r.images.iter().map(|b| b.name.clone().unwrap_or_default()));
        }
        out
    }

    /// 48 §7.1: user text + A/B (B shares a name with a tool image), tool
    /// text + C/D with a second image-only response, then a later reply.
    async fn seeded() -> (Store, SessionId, std::path::PathBuf) {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user(MessageContent::from_parts(vec![
                    ContentPart::Text("look".into()),
                    ContentPart::Binary(img("a.png")),
                    ContentPart::Binary(img("same.png")),
                ])),
                ChatMessage::assistant(MessageContent::from_parts(vec![
                    ContentPart::ThoughtSignature("sig".into()),
                    ContentPart::Text("checking".into()),
                    ContentPart::ToolCall(call("c1")),
                    ContentPart::ToolCall(call("c2")),
                ])),
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "out").with_images(vec![img("c.png"), img("same.png")]),
                    ToolResponse::new("c2", "").with_images(vec![img("d.png")]),
                ]),
                asst("done"),
            ],
        )
        .await;
        (store, id, dir)
    }

    #[tokio::test]
    async fn remove_context_drops_exact_occurrences_and_guards_by_revision() {
        let (store, id, dir) = seeded().await;
        let rev = store.context_revision(id).await.unwrap();
        assert_eq!(rev, "4", "one bump per append");
        let snap = store.context_items(id).await.unwrap();
        assert_eq!(snap.revision, rev);
        assert_eq!((snap.epoch, snap.tail_from), (0, 0));
        assert_eq!(
            snap.items
                .iter()
                .filter(|i| i.kind == ContextKind::Image)
                .count(),
            5
        );

        // Stale revision refuses first, even with an unknown target.
        let err = store.remove_context(id, "3", &[msg(99)]).await.unwrap_err();
        assert!(err.to_string().contains("stale revision"), "{err}");
        // Unknown / malformed targets write nothing.
        assert!(store.remove_context(id, &rev, &[msg(99)]).await.is_err());
        assert!(
            store
                .remove_context(id, &rev, &[bin(2, 0, None)])
                .await
                .is_err()
        );
        assert!(store.remove_context(id, "x", &[]).await.is_err());
        assert_eq!(store.context_revision(id).await.unwrap(), rev);

        // A and C: exact occurrences only; `same.png` stays twice.
        let (removal, epoch, head, tail) = store
            .remove_context(
                id,
                &rev,
                &[bin(0, 1, None), bin(2, 0, Some(0)), bin(0, 1, None)],
            )
            .await
            .unwrap();
        assert_eq!(
            (
                removal.messages,
                removal.images,
                removal.documents,
                removal.heads
            ),
            (0, 2, 0, 0)
        );
        assert_eq!(removal.revision, "5");
        assert_eq!((epoch, head.is_none(), tail.len()), (0, true, 4));
        assert_eq!(names(&tail[0].1), ["same.png"]);
        assert_eq!(names(&tail[2].1), ["same.png", "d.png"]);
        assert_eq!(tail[0].1.content.texts(), ["look"]);
        let rs = tail[2].1.content.tool_responses();
        assert_eq!(
            (rs[0].call_id.as_str(), rs[0].content.as_str()),
            ("c1", "out")
        );
        assert_eq!(text_of(&tail[3].1.content), "done");
        // `frame` is the same projection; raw rows are untouched.
        let (_, _, frame) = store.frame(id).await.unwrap();
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            serde_json::to_string(&tail).unwrap()
        );
        let raw = store.messages(id).await.unwrap();
        assert_eq!(raw.len(), 4);
        assert_eq!(names(&raw[0].1), ["a.png", "same.png"]);
        assert_eq!(names(&raw[2].1), ["c.png", "same.png", "d.png"]);
        assert_eq!(store.context_revision(id).await.unwrap(), "5");

        // Exact already-removed target: no-op, no bump. A second edit still
        // addresses original positions; D's response keeps its envelope.
        let (noop, ..) = store
            .remove_context(id, "5", &[bin(0, 1, None)])
            .await
            .unwrap();
        assert_eq!((noop.revision.as_str(), noop.images), ("5", 0));
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        let (removal, _, _, tail) = store
            .remove_context(id, "5", &[bin(0, 2, None), bin(2, 1, Some(0))])
            .await
            .unwrap();
        assert_eq!((removal.revision.as_str(), removal.images), ("6", 2));
        assert!(tail[0].1.content.binaries().is_empty());
        let rs = tail[2].1.content.tool_responses();
        assert_eq!(
            (rs[1].call_id.as_str(), rs[1].content.as_str()),
            ("c2", IMAGE_REMOVED)
        );
        assert!(rs[1].images.is_empty());
        assert_eq!(
            store
                .effective_message(id, 0)
                .await
                .unwrap()
                .unwrap()
                .content
                .texts(),
            ["look"]
        );
        // Inventory follows the masks.
        let snap = store.context_items(id).await.unwrap();
        assert_eq!(snap.revision, "6");
        assert_eq!(
            snap.items
                .iter()
                .filter(|i| i.kind == ContextKind::Image)
                .count(),
            1
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_context_whole_group_keeps_later_rows_and_turns() {
        let (store, id, dir) = seeded().await;
        let snap = store.context_items(id).await.unwrap();
        let asst_item = snap.items.iter().find(|i| i.target == msg(1)).unwrap();
        assert_eq!(asst_item.remove_with, [msg(2)]);
        assert_eq!(
            snap.items.iter().find(|i| i.target == msg(2)).unwrap().kind,
            ContextKind::Tool
        );

        // The incomplete set refuses and writes nothing.
        let err = store.remove_context(id, "4", &[msg(2)]).await.unwrap_err();
        assert!(err.to_string().contains("requires message 1"), "{err}");
        assert_eq!(store.context_revision(id).await.unwrap(), "4");

        let (removal, _, _, tail) = store
            .remove_context(id, "4", &[msg(1), msg(2), bin(2, 0, Some(0))])
            .await
            .unwrap();
        assert_eq!((removal.messages, removal.images), (2, 0));
        assert_eq!(
            texts(&tail),
            [
                (0, "look[image: a.png][image: same.png]".into()),
                (3, "done".into())
            ]
        );
        // Raw turn identity is unchanged: undo still hides the whole turn,
        // redo restores it with its exclusions.
        assert_eq!(store.messages(id).await.unwrap().len(), 4);
        assert_eq!(
            store.undo(id).await.unwrap().as_deref(),
            Some("look[image: a.png][image: same.png]")
        );
        assert_eq!(store.frame(id).await.unwrap().2.len(), 0);
        assert!(store.redo(id).await.unwrap().is_some());
        assert_eq!(store.frame(id).await.unwrap().2.len(), 2);
        assert_eq!(store.context_revision(id).await.unwrap(), "7");
        // Removed rows are still addressable no-ops, and gone from the inventory.
        let (noop, ..) = store
            .remove_context(id, "7", &[msg(1), msg(2)])
            .await
            .unwrap();
        assert_eq!(noop.messages, 0);
        assert!(store.effective_message(id, 1).await.unwrap().is_none());
        let snap = store.context_items(id).await.unwrap();
        assert!(
            snap.items
                .iter()
                .all(|i| i.target != msg(1) && i.target != msg(2))
        );
        // Search still sees the rows, labelled removed.
        let bodies = store.search_bodies().await.unwrap();
        assert_eq!(
            bodies
                .iter()
                .map(|(_, seq, _, removed)| (*seq, *removed))
                .collect::<Vec<_>>(),
            [(0, false), (1, true), (3, false)]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_head_keeps_epoch_and_tail_from() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user("u1"),
                asst("a1"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;
        let head = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(compact::wrap_head_open(
                1,
                compact::CompactStyle::Snap,
                "Archive of 2 folded messages as 2 image(s), most recent last.",
                Vec::<String>::new(),
            )),
            ContentPart::Binary(img("frame-0.png")),
            ContentPart::Binary(img("frame-1.png")),
            ContentPart::Text(compact::HEAD_CLOSE.into()),
        ]));
        store.compact(id, 1, 3, 2, &head).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        let snap = store.context_items(id).await.unwrap();
        assert_eq!((snap.epoch, snap.tail_from), (1, 2));
        assert_eq!(snap.items[0].kind, ContextKind::Summary);
        assert!(snap.items[1].label.starts_with("Summary image"));

        // One frame: the head stays, recaptioned.
        let frame0 = ContextTarget::Binary {
            owner: ContextOwner::Head { epoch: 1 },
            part: 1,
            image: None,
        };
        let (removal, epoch, head, tail) = store.remove_context(id, "5", &[frame0]).await.unwrap();
        assert_eq!((removal.images, removal.heads, epoch), (1, 0, 1));
        let head = head.unwrap();
        assert_eq!(names(&head), ["frame-1.png"]);
        assert!(text_of(&head.content).contains("as 1 image(s) (1 of 2 removed"));
        assert_eq!(texts(&tail), [(2, "u2".into()), (3, "a2".into())]);
        // The raw epoch row is untouched.
        assert_eq!(
            names(&store.epochs(id).await.unwrap()[0].head),
            ["frame-0.png", "frame-1.png"]
        );

        // Whole head: no head, same epoch and tail; folded rows never return.
        let (removal, epoch, head, tail) = store
            .remove_context(id, "6", &[ContextTarget::Head { epoch: 1 }])
            .await
            .unwrap();
        assert_eq!((removal.heads, epoch, head.is_none()), (1, 1, true));
        assert_eq!(texts(&tail), [(2, "u2".into()), (3, "a2".into())]);
        let (n, head, tail) = store.frame(id).await.unwrap();
        assert_eq!((n, head.is_none(), tail.len()), (1, true, 2));
        assert!(
            store
                .remove_context(id, "7", &[ContextTarget::Head { epoch: 2 }])
                .await
                .is_err()
        );
        let snap = store.context_items(id).await.unwrap();
        assert_eq!((snap.epoch, snap.tail_from, snap.items.len()), (1, 2, 2));
        // Undo hides the epoch with its turn; redo brings the exclusion back.
        store.undo(id).await.unwrap();
        assert_eq!(store.frame(id).await.unwrap().0, 0);
        store.redo(id).await.unwrap();
        let (n, head, _) = store.frame(id).await.unwrap();
        assert_eq!((n, head.is_none()), (1, true));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revision_bumps_on_every_visibility_change_and_never_resets() {
        let (store, id, dir) = fresh().await;
        assert_eq!(store.context_revision(id).await.unwrap(), "0");
        assert!(store.context_revision(SessionId::new()).await.is_err());
        append_all(
            &store,
            id,
            [ChatMessage::user("u1"), asst("a1"), ChatMessage::user("u2")],
        )
        .await;
        assert_eq!(store.context_revision(id).await.unwrap(), "3");
        store.undo(id).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "4");
        store.redo(id).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        // Nothing hidden: commit is a no-op, no bump.
        store.commit_cut(id).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        store.cut_to(id, 2).await.unwrap();
        store.commit_cut(id).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "7");
        // Seq 2 is reused by the next append; a button confirmed at "6" is stale.
        assert_eq!(
            store.append(id, &ChatMessage::user("u2b")).await.unwrap(),
            2
        );
        assert!(store.remove_context(id, "6", &[msg(2)]).await.is_err());
        let (r, ..) = store.remove_context(id, "8", &[msg(2)]).await.unwrap();
        assert_eq!((r.revision.as_str(), r.messages), ("9", 1));
        // Conditional title: only at the current revision.
        assert!(!store.set_title_at_revision(id, "old", "8").await.unwrap());
        assert!(store.set_title_at_revision(id, "new", "9").await.unwrap());
        assert_eq!(store.session(id).await.unwrap().unwrap().title, "new");
        assert!(store.set_title_at_revision(id, "x", "bad").await.is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 48 §4.4: a fork carries masks with its rows and heads; the stripped
    /// mid-reply copy is materialized effective so no old part index lands on
    /// a shortened vector.
    #[tokio::test]
    async fn fork_copies_masks_and_remaps_the_stripped_reply() {
        let (store, src, dir) = fresh().await;
        let reply = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::Text("looking".into()),
            ContentPart::Binary(img("shot.png")),
            ContentPart::ToolCall(call("c1")),
        ]));
        append_all(
            &store,
            src,
            [
                ChatMessage::user(MessageContent::from_parts(vec![
                    ContentPart::Text("u1".into()),
                    ContentPart::Binary(img("a.png")),
                ])),
                reply,
                ChatMessage::from(vec![ToolResponse::new("c1", "ok")]),
                asst("done"),
                ChatMessage::user("u2"),
                asst("a2"),
            ],
        )
        .await;
        let head = ChatMessage::user(compact::wrap_head(
            1,
            compact::CompactStyle::Summary,
            "s",
            Vec::<String>::new(),
        ));
        store.compact(src, 1, 3, 4, &head).await.unwrap();
        let rev = store.context_revision(src).await.unwrap();
        // Head removed, u2 removed; the mid-reply binary at part 1 removed.
        store
            .remove_context(src, &rev, &[ContextTarget::Head { epoch: 1 }, msg(4)])
            .await
            .unwrap();
        // Turn 0's reply is outside the active frame now, so mask it directly
        // like an earlier edit would have (folded rows keep their masks).
        sqlx::query("UPDATE messages SET excluded = ? WHERE session_id = ? AND seq = 1")
            .bind(r#"{"whole":false,"binaries":[{"part":1}]}"#)
            .bind(src.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        // Everything: masks copied, revision snapshotted, frame identical.
        let all = store.fork(src, None, None).await.unwrap();
        assert_eq!(
            store.context_revision(all).await.unwrap(),
            store.context_revision(src).await.unwrap()
        );
        let (n, head, tail) = store.frame(all).await.unwrap();
        assert_eq!((n, head.is_none()), (1, true));
        assert_eq!(texts(&tail), [(5, "a2".into())]);
        assert_eq!(store.messages(all).await.unwrap().len(), 6);
        let hist = store.context_history(all).await.unwrap();
        assert!(hist.entries[&4].removed);
        assert!(hist.heads[&1].removed && hist.heads[&1].active);
        assert!(hist.entries[&1].attachments[0].removed);

        // Mid-reply: the copied reply has neither the removed binary nor the
        // call, and no mask left to misapply.
        let f = store.fork(src, Some(0), Some(0)).await.unwrap();
        let rows = store.messages(f).await.unwrap();
        assert_eq!(
            texts(&rows),
            [(0, "u1[image: a.png]".into()), (1, "looking".into())]
        );
        assert!(rows[1].1.content.tool_calls().is_empty());
        assert!(rows[1].1.content.binaries().is_empty());
        let hist = store.context_history(f).await.unwrap();
        assert!(hist.entries[&1].attachments.is_empty());
        let (_, _, frame) = store.frame(f).await.unwrap();
        assert_eq!(names(&frame[0].1), ["a.png"]);
        // The source is untouched, including its raw reply.
        assert_eq!(
            names(&store.messages(src).await.unwrap()[1].1),
            ["shot.png"]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn context_history_reads_raw_rows_with_membership() {
        let (store, id, dir) = seeded().await;
        let head = ChatMessage::user(compact::wrap_head(
            1,
            compact::CompactStyle::Summary,
            "s",
            Vec::<String>::new(),
        ));
        // Fold the user row only: the tail keeps the whole call/result group
        // (assistant 1, tool 2) plus the reply, so the frame stays valid.
        store.compact(id, 1, 3, 1, &head).await.unwrap();
        store
            .remove_context(id, "5", &[bin(2, 1, Some(0))])
            .await
            .unwrap();
        let h = store.context_history(id).await.unwrap();
        assert_eq!(h.revision, "6");
        assert_eq!(
            h.messages.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        // Bodies are the raw rows: the folded user keeps its images and the
        // tool row still carries the removed one.
        assert_eq!(names(&h.messages[0].1), ["a.png", "same.png"]);
        assert_eq!(names(&h.messages[2].1), ["c.png", "same.png", "d.png"]);
        assert_eq!(h.epochs.len(), 1);
        assert_eq!((h.epochs[0].at_seq, h.epochs[0].tail_from), (3, 1));
        assert_eq!(
            (
                h.snapshot.epoch,
                h.snapshot.tail_from,
                h.snapshot.revision.as_str()
            ),
            (1, 1, "6")
        );
        // Folded row: inactive, coordinates kept, no companions.
        let e0 = &h.entries[&0];
        assert_eq!((e0.target, e0.active, e0.removed), (msg(0), false, false));
        assert_eq!(e0.attachments.len(), 2);
        assert!(e0.remove_with.is_empty());
        // Active call/result group: each names the other as companion.
        assert_eq!(h.entries[&1].remove_with, [msg(2)]);
        let e2 = &h.entries[&2];
        assert!(e2.active && !e2.removed);
        assert_eq!(e2.remove_with, [msg(1)]);
        assert!(h.entries[&3].remove_with.is_empty());
        // The removed image is flagged in place; the raw row is untouched.
        assert_eq!(
            e2.attachments
                .iter()
                .map(|a| (a.target, a.removed))
                .collect::<Vec<_>>(),
            [
                (bin(2, 0, Some(0)), false),
                (bin(2, 0, Some(1)), false),
                (bin(2, 1, Some(0)), true)
            ]
        );
        assert_eq!(e2.attachments[2].name, "d.png");
        assert!(e2.attachments.iter().all(|a| a.path.is_none()));
        assert_eq!(
            (h.heads[&1].target, h.heads[&1].active),
            (ContextTarget::Head { epoch: 1 }, true)
        );
        assert!(store.context_history(SessionId::new()).await.is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failure anywhere in the edit transaction (here: the revision bump,
    /// after every mask was written) leaves masks and revision untouched.
    #[tokio::test]
    async fn remove_context_failed_write_leaves_masks_and_revision() {
        let (store, id, dir) = seeded().await;
        let before = serde_json::to_string(&store.frame(id).await.unwrap()).unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_rev BEFORE UPDATE OF context_revision ON sessions \
             BEGIN SELECT RAISE(ABORT, 'injected'); END",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        let targets = [msg(1), msg(2), bin(0, 1, None)];
        let err = store.remove_context(id, "4", &targets).await.unwrap_err();
        assert!(err.to_string().contains("injected"), "{err}");
        assert_eq!(store.context_revision(id).await.unwrap(), "4");
        assert_eq!(
            serde_json::to_string(&store.frame(id).await.unwrap()).unwrap(),
            before
        );
        let snap = store.context_items(id).await.unwrap();
        assert_eq!(snap.revision, "4");
        assert_eq!(
            snap.items
                .iter()
                .filter(|i| i.kind == ContextKind::Image)
                .count(),
            5
        );
        assert!(store.effective_message(id, 1).await.unwrap().is_some());
        // Same request without the fault: the edit was valid all along.
        sqlx::query("DROP TRIGGER fail_rev")
            .execute(&store.pool)
            .await
            .unwrap();
        let (removal, _, _, tail) = store.remove_context(id, "4", &targets).await.unwrap();
        assert_eq!(
            (removal.revision.as_str(), removal.messages, removal.images),
            ("5", 2, 1)
        );
        assert_eq!(
            texts(&tail),
            [(0, "look[image: same.png]".into()), (3, "done".into())]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A committed removal survives a restart: the reopened store projects the
    /// same effective frame, revision, and inventory from the raw rows.
    #[tokio::test]
    async fn reopen_after_remove_context_keeps_effective_frame() {
        let (store, id, dir) = seeded().await;
        let (removal, epoch, head, tail) = store
            .remove_context(id, "4", &[msg(1), msg(2), bin(0, 1, None)])
            .await
            .unwrap();
        assert_eq!(removal.revision, "5");
        let committed = serde_json::to_string(&(epoch, head, tail)).unwrap();
        store.pool.close().await;
        drop(store);

        let store = Store::open(&dir.join("sessions.sqlite")).await.unwrap();
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        assert_eq!(
            serde_json::to_string(&store.frame(id).await.unwrap()).unwrap(),
            committed
        );
        assert_eq!(store.messages(id).await.unwrap().len(), 4, "raw rows kept");
        assert!(store.effective_message(id, 1).await.unwrap().is_none());
        let snap = store.context_items(id).await.unwrap();
        assert_eq!(snap.revision, "5");
        assert!(
            snap.items
                .iter()
                .all(|i| i.target != msg(1) && i.target != msg(2))
        );
        assert_eq!(
            snap.items
                .iter()
                .filter(|i| i.kind == ContextKind::Image)
                .count(),
            1
        );
        // The persisted revision still guards edits confirmed before the restart.
        assert!(
            store
                .remove_context(id, "4", &[bin(0, 2, None)])
                .await
                .is_err()
        );
        let (removal, ..) = store
            .remove_context(id, "5", &[bin(0, 2, None)])
            .await
            .unwrap();
        assert_eq!((removal.revision.as_str(), removal.images), ("6", 1));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two edits confirmed at the same revision: exactly one commits, the
    /// other is rejected as stale, and the frame carries only the winner's
    /// masks. The loser succeeds once it reconfirms at the new revision.
    #[tokio::test]
    async fn concurrent_remove_context_from_same_revision_admits_one() {
        let (store, id, dir) = seeded().await;
        let (a, c) = (bin(0, 1, None), bin(2, 0, Some(0)));
        let (ta, tc) = ([a], [c]);
        let (ra, rc) = tokio::join!(
            store.remove_context(id, "4", &ta),
            store.remove_context(id, "4", &tc),
        );
        let (a_won, err) = match (ra, rc) {
            (Ok(_), Err(e)) => (true, e),
            (Err(e), Ok(_)) => (false, e),
            (a, c) => panic!("expected exactly one winner: {} {}", a.is_ok(), c.is_ok()),
        };
        assert!(err.to_string().contains("stale revision"), "{err}");
        assert_eq!(store.context_revision(id).await.unwrap(), "5");
        let (_, _, frame) = store.frame(id).await.unwrap();
        let (user, tool) = (names(&frame[0].1), names(&frame[2].1));
        if a_won {
            assert_eq!(user, ["same.png"]);
            assert_eq!(tool, ["c.png", "same.png", "d.png"]);
        } else {
            assert_eq!(user, ["a.png", "same.png"]);
            assert_eq!(tool, ["same.png", "d.png"]);
        }
        let loser = if a_won { c } else { a };
        let (removal, _, _, tail) = store.remove_context(id, "5", &[loser]).await.unwrap();
        assert_eq!((removal.revision.as_str(), removal.images), ("6", 1));
        assert_eq!(names(&tail[0].1), ["same.png"]);
        assert_eq!(names(&tail[2].1), ["same.png", "d.png"]);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // endregion: --- Context removal (48)

    // region: --- Project metadata (51 §3)

    #[tokio::test]
    async fn project_lives_on_the_root_and_shows_on_every_row() {
        let (store, root, dir) = fresh().await;
        let child = SessionId::new();
        store
            .create_session(child, "m", Some(root), Some("kid"), Some("ignored"))
            .await
            .unwrap();
        let project_of = |list: Vec<SessionSummary>, id| {
            list.into_iter().find(|s| s.id == id).unwrap().project
        };

        // A child call mutates the root; the caller already normalized.
        assert_eq!(store.set_project(child, Some("Alpha")).await.unwrap(), root);
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|s| s.project.as_deref() == Some("Alpha")));
        assert_eq!(
            store.descendants(root).await.unwrap()[0].1.project.as_deref(),
            Some("Alpha")
        );
        assert_eq!(
            store.effective_project(child).await.unwrap().as_deref(),
            Some("Alpha")
        );

        // A fork of the child copies the effective value into its own root.
        store.append(child, &ChatMessage::user("hi")).await.unwrap();
        let fork = store.fork(child, None, None).await.unwrap();
        assert_eq!(
            store.session(fork).await.unwrap().unwrap().project.as_deref(),
            Some("Alpha")
        );

        // Clearing the root clears its tree; the fork has diverged.
        assert_eq!(store.set_project(root, None).await.unwrap(), root);
        let list = store.list_sessions().await.unwrap();
        assert_eq!(project_of(list.clone(), root), None);
        assert_eq!(project_of(list.clone(), child), None);
        assert_eq!(project_of(list, fork).as_deref(), Some("Alpha"));
        assert_eq!(
            store
                .set_project(SessionId::new(), Some("x"))
                .await
                .unwrap_err()
                .to_string(),
            "no such session"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // endregion: --- Project metadata (51 §3)
}
