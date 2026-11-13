//! sqlite session store (sqlx): `sessions` + `messages` + `epochs` +
//! `todo_snapshots` + `errors` + `inner_calls`; resume by replay. Only the
//! agent writes it; UIs read through the supervisor's commands.
//! `sessions.cut_at` is the undo marker: rows with `seq >= cut_at` (epochs,
//! todo snapshots, errors and inner calls `at_seq >= cut_at`) are hidden from
//! every reader until `commit_cut` deletes them. The plan is the latest visible
//! snapshot ordered by `(at_seq, k)`; `sessions.todo_version` only ever grows
//! (47 §5.2). `errors` are UI-only rows anchored at the last visible message
//! seq when they happened; they never enter the model frame. `inner_calls`
//! (08 as-built) are the tool calls a `python` cell made, keyed by the cell's
//! call id, anchored the same way and equally invisible to the model.
//!
//! Context removal (48 §4): `messages.excluded` / `epochs.excluded` hold
//! [`Mask`]s beside untouched rows. *Raw* readers (`messages`, undo/redo,
//! `epochs`, search) see the rows; *effective* readers (`frame`,
//! `context_items`, `effective_message`) apply the masks.
//! `sessions.context_revision` is the mutation handle: bumped in the same
//! transaction as every append, epoch, exclusion edit, undo/redo and
//! committed cut; never reset when seq/epoch numbers are reused.
//!
//! Vision notes (52 §6): `messages.vision` holds the typed [`VisionNote`] of
//! a consultation row, NULL otherwise. Only that column marks a note; such
//! rows never open a turn and are transparent when a turn reader looks for
//! the preceding role. [`Store::vision_frame`] reads the effective frame,
//! the original-coordinate image index and the active notes in one snapshot.
//!
//! Communication accounting (50 §7: root scope, peer exchanges, A2A
//! contexts/tasks, peer bindings) lives in [`communication`] on the same pool.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};
use parking_lot::Mutex;
use runtime::SessionId;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{FromRow, Sqlite, SqliteConnection, Transaction};

use crate::context::{
    Active, ContextHistory, ContextOwner, ContextRemoval, ContextSnapshot, ContextTarget, Mask,
    entry, project,
};
use crate::loop_::text_of;
use crate::todo::{Todo, TodoSnapshot, TodoSource};
use crate::vision::{VisionImage, VisionNote};
use crate::{
    CostRule, ObservedMeta, ObservedSource, ObservedState, ProjectStyle, SessionKind,
    SessionSummary,
};

pub mod accounting;
pub mod advisor;
mod capture;
pub mod communication;
pub mod contacts;
pub mod dreaming;
pub mod durability;
pub mod endless;
pub mod harness;
mod moved;
pub mod turn_usage;
pub use capture::ATTACHED;
pub use durability::Scope;

/// Handle to `<data_dir>/sessions.sqlite`. Cheap to clone.
#[derive(Clone, kameo::Reply)]
pub struct Store {
    /// Capture session on the write connection (68 §3); `None` on a private
    /// store. Declared first so it is deleted before the pools drop.
    session: Arc<Mutex<Option<capture::Session>>>,
    /// A `:memory:` private store: one pool, no capture, no log.
    private: bool,
    /// This engine's mesh id as `sessions.owner` spells it; `""` when opened
    /// without one (a lone engine: every `''` row is ours).
    me: Arc<str>,
    /// Column layout of every attached table, for splitting changesets.
    tables: Arc<Vec<capture::TableMeta>>,
    /// Last `session_log.seq` this engine wrote (`MAX(seq)` at open).
    seq: Arc<AtomicI64>,
    /// A commit future was dropped midway (it may or may not have
    /// committed): the next write transaction re-reads `seq` from the log.
    reseed: Arc<std::sync::atomic::AtomicBool>,
    /// Stamps log rows; origin = `me`.
    clock: Arc<replica::Clock>,
    /// Last logged `seq`, for shippers waiting on new rows (68 §6; `log_watch`).
    log_tx: Arc<tokio::sync::watch::Sender<i64>>,
    /// Bumped by every `harness_enqueue`, for shippers waiting on outbox rows (139 R2).
    harness_tx: Arc<tokio::sync::watch::Sender<u64>>,
    /// Exactly one connection; every mutating statement runs on it.
    write: SqlitePool,
    /// Readers (four connections; the write pool itself on a private store).
    read: SqlitePool,
    /// 176 E5: test-only stall (ms) before `session`, `list_sessions` and
    /// `descendants`, so a test can hold a read in flight.
    #[cfg(test)]
    read_delay: Arc<std::sync::atomic::AtomicU64>,
}

/// Why the store refused a write (68 §3: writes on a non-owner are refused
/// at the store; the only way past is forwarding or a take).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The session is owned by another engine.
    NotOwner {
        /// The session the write touched.
        session: SessionId,
        /// Its owner (a mesh `EngineId` as text).
        owner: String,
        /// Its current epoch.
        epoch: u32,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOwner {
                session,
                owner,
                epoch,
            } => write!(f, "session {session} is owned by {owner} (epoch {epoch})"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One `session_log` row: the changeset of one committed transaction,
/// restricted to one session (68 §2).
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct LogRow {
    /// This engine's log position.
    pub seq: i64,
    /// The session every row of `changeset` belongs to.
    pub session_id: String,
    /// The session's epoch when the row was written.
    pub epoch: i64,
    /// A `sqlite3session` changeset.
    pub changeset: Vec<u8>,
    /// [`replica::Hlc::to_text`] of the writing engine's clock.
    pub hlc: String,
    /// Written by a demoted owner; never shipped (68 §4.3).
    pub orphaned: bool,
}

/// One `session_cursors` row.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Cursor {
    /// Whose log.
    pub origin: String,
    /// Who applied up to `seq` (`""` = this engine's own applied cursor).
    pub peer: String,
    /// Last applied seq.
    pub seq: i64,
}

/// The one write transaction (see [`Store::begin_write`]). Dereferences to
/// the connection so statements and the `*_in` helpers use it unchanged.
pub(crate) struct WriteTx<'a> {
    tx: Transaction<'static, Sqlite>,
    store: &'a Store,
}

impl Deref for WriteTx<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &SqliteConnection {
        &self.tx
    }
}

impl DerefMut for WriteTx<'_> {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        &mut self.tx
    }
}

impl WriteTx<'_> {
    /// Log what this transaction changed, then commit; dropping without
    /// committing rolls back. On a durable store (68 §3): take the capture
    /// session's changeset (uncommitted changes of this connection), split
    /// it by session, refuse the whole transaction with
    /// [`StoreError::NotOwner`] if any touched session is owned elsewhere,
    /// and append one `session_log` row per session inside the same
    /// transaction (`session_log` is not attached, so the row is not
    /// recaptured).
    pub(crate) async fn commit(self) -> anyhow::Result<()> {
        // A failure after seqs were taken (`NotOwner` for a later session, a
        // failed commit) gives them back: a seq never committed is a gap the
        // peers' cursors stop at, and each costs them a full snapshot. The
        // write connection is single: no other transaction took one meanwhile.
        // A future dropped midway cannot tell whether COMMIT ran: the guard
        // has the next `begin_write` re-read the head instead.
        struct Midway<'s>(Option<&'s Store>);
        impl Drop for Midway<'_> {
            fn drop(&mut self) {
                if let Some(store) = self.0 {
                    store.reseed.store(true, Ordering::SeqCst);
                }
            }
        }
        let store = self.store;
        let mut midway = Midway(Some(store));
        let start = store.seq.load(Ordering::SeqCst);
        let done = self.log_and_commit().await;
        midway.0 = None;
        if done.is_err() {
            store.seq.store(start, Ordering::SeqCst);
        }
        done
    }

    async fn log_and_commit(mut self) -> anyhow::Result<()> {
        let store = self.store;
        let changeset = {
            // Lock the handle out from the worker thread first; the session
            // reads current row values through it.
            let _handle = self.tx.lock_handle().await?;
            let mut guard = store.session.lock();
            match guard.as_mut() {
                Some(session) => {
                    let bytes = session.changeset()?;
                    session.reset()?;
                    bytes
                }
                None => Vec::new(),
            }
        };
        if !changeset.is_empty() {
            let mut ops = capture::scan(&changeset, &store.tables)?;
            // An UPDATE on a table whose session column is not in the key
            // (`model_calls`, `remote_meters`) carries no session: ask the
            // row, which still exists with its owner unchanged.
            for op in ops.iter_mut().filter(|o| o.session.is_none()) {
                let t = &store.tables[op.table];
                let sql = format!(
                    "SELECT {} FROM {} WHERE {} = ?",
                    t.cols[t.session_col],
                    t.name,
                    t.cols[t.pk_col()]
                );
                // Names come from `capture::ATTACHED` and `PRAGMA table_info`, never from input.
                let row: Option<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
                    .bind(&op.pk)
                    .fetch_optional(&mut *self.tx)
                    .await?;
                op.session = Some(row.map(|(s,)| s).unwrap_or_default());
            }
            let deleted: HashMap<&str, (String, i64)> = ops
                .iter()
                .filter_map(|o| Some((o.session.as_deref()?, o.deleted_session.clone()?)))
                .collect();
            let sessions: Vec<String> = ops
                .iter()
                .map(|o| o.session.clone().unwrap_or_default())
                .collect();
            let groups = capture::split(&changeset, &sessions)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            for (sid, bytes) in groups {
                // The fence after this transaction: the highest `(epoch,
                // owner)` of `session_epochs` and the post-state session row.
                // A take (`owner = me`) passes; a foreign owner refuses the
                // whole transaction.
                let row: Option<(String, i64)> = sqlx::query_as(
                    "SELECT owner, epoch FROM (SELECT owner, epoch FROM session_epochs WHERE session_id = ?1 \
                     UNION ALL SELECT owner, epoch FROM sessions WHERE id = ?1) ORDER BY epoch DESC LIMIT 1",
                )
                .bind(&sid)
                .fetch_optional(&mut *self.tx)
                .await?;
                let (owner, epoch) = match row.or_else(|| deleted.get(sid.as_str()).cloned()) {
                    Some(v) => v,
                    // Rows of a session that has no row (accounting for an
                    // owner whose session row is gone): ours, epoch 1.
                    None => (String::new(), 1),
                };
                if !(owner.is_empty() || *owner == *store.me) {
                    anyhow::bail!(StoreError::NotOwner {
                        session: sid
                            .parse()
                            .with_context(|| format!("bad session id {sid}"))?,
                        owner,
                        epoch: u32::try_from(epoch).unwrap_or(u32::MAX),
                    });
                }
                let seq = store.seq.fetch_add(1, Ordering::SeqCst) + 1;
                sqlx::query(
                    "INSERT INTO session_log (seq, session_id, epoch, changeset, hlc) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(seq)
                .bind(&sid)
                .bind(epoch)
                .bind(bytes)
                .bind(store.clock.tick(now).to_text())
                .execute(&mut *self.tx)
                .await?;
            }
        }
        self.tx.commit().await?;
        store
            .log_tx
            .send_if_modified(|s| std::mem::replace(s, store.seq.load(Ordering::SeqCst)) != *s);
        Ok(())
    }
}

/// A write transaction with capture **disabled** (68 §6 "apply with the
/// session detached"): what a remote changeset is applied through, so the
/// replica's copy never echoes into its own log. No owner guard: the rows
/// belong to whoever wrote them. Re-enables capture on commit or drop.
pub struct ApplyTx<'a> {
    /// `None` only after `commit` took it (the `Drop` re-enables capture either way).
    tx: Option<Transaction<'static, Sqlite>>,
    store: &'a Store,
}

impl Deref for ApplyTx<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &SqliteConnection {
        self.tx.as_ref().expect("open apply transaction")
    }
}

impl DerefMut for ApplyTx<'_> {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.tx.as_mut().expect("open apply transaction")
    }
}

impl ApplyTx<'_> {
    /// Run `f` with the raw `sqlite3*` of the write connection (for
    /// `sqlite3changeset_apply`), the worker thread locked out meanwhile.
    pub async fn with_raw<R>(
        &mut self,
        f: impl FnOnce(std::ptr::NonNull<libsqlite3_sys::sqlite3>) -> R,
    ) -> anyhow::Result<R> {
        let mut handle = self.deref_mut().lock_handle().await?;
        Ok(f(handle.as_raw_handle()))
    }

    /// Commit without logging; `Drop` turns capture back on.
    pub async fn commit(mut self) -> anyhow::Result<()> {
        let tx = self.tx.take().expect("open apply transaction");
        tx.commit().await?;
        Ok(())
    }
}

impl Drop for ApplyTx<'_> {
    fn drop(&mut self) {
        self.store.set_capture(true);
    }
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

/// One transaction-consistent read for text-only preparation (52 §4.2, §6):
/// the effective frame as [`Store::frame`] returns it, every active image
/// occurrence addressed by its **original** coordinates (raw rows + masks,
/// head first, then rows by seq) and the typed notes whose rows are still
/// effective, all at `revision`.
#[derive(Debug)]
pub struct VisionFrame {
    /// Current epoch; 0 before any compaction.
    pub epoch: u32,
    /// Effective head (`None`: no epoch, or the user removed it).
    pub head: Option<ChatMessage>,
    /// Effective tail rows from `tail_from`.
    pub tail: Vec<(i64, ChatMessage)>,
    /// Active, unmasked image occurrences (never PDFs or removed images).
    pub images: Vec<VisionImage>,
    /// Typed notes of active, unmasked rows by seq.
    pub notes: BTreeMap<i64, VisionNote>,
    /// Decimal `sessions.context_revision` everything above was read at.
    pub revision: String,
}

/// One visible or hidden raw `messages` row.
struct Row {
    seq: i64,
    message: ChatMessage,
    mask: Mask,
    /// Raw `messages.vision`; `Some` marks a typed note (parsed lazily by
    /// the readers that need it, so a corrupt note cannot hide a transcript
    /// from turn readers).
    vision: Option<String>,
    /// Raw `messages.at` (RFC 3339 UTC); `None` for rows older than 0026.
    at: Option<String>,
    /// Raw `messages.thinking_ms` (91); `None` before 91 or when no reasoning streamed.
    thinking_ms: Option<i64>,
    /// Raw `messages.advisory` (125 §5.4); `Some` marks an Advisory row.
    advisory: Option<String>,
    /// Raw `messages.moved` (183 §5.3); `Some` marks a Moved row.
    moved: Option<String>,
}

/// Raw `sessions` row; ids and timestamps are TEXT and parsed on the way out.
#[derive(FromRow)]
struct SessionRow {
    id: String,
    created_at: String,
    updated_at: String,
    title: String,
    tags: String,
    model: String,
    parent: Option<String>,
    name: Option<String>,
    effort: Option<String>,
    agent: Option<String>,
    cwd: Option<String>,
    harness_model: Option<String>,
    context_size: Option<i64>,
    desk: Option<String>,
    pinned: bool,
    archived: bool,
    project: Option<String>,
    notebook: Option<String>,
    owner: String,
    epoch: i64,
    kind: String,
    memory: String,
    acp_session: Option<String>,
    observed_source: Option<String>,
    observed_state: Option<String>,
    observed_stop: Option<String>,
    observed_cancel: bool,
    observed_description: Option<String>,
    observed_tool_call: Option<String>,
    observed_detail: Option<String>,
    observed_cost_rule: Option<String>,
}

/// Scalar subquery: column `$col` of the root above the outer `sessions` row
/// (51 §3: children carry no `project`/`archived` of their own). Correlated
/// on the outer table name, so the enclosing query must select `FROM sessions`
/// unaliased.
macro_rules! root_col {
    ($col:literal) => {
        concat!(
            "(WITH RECURSIVE up(id, parent) AS ( \
              SELECT sessions.id, sessions.parent \
              UNION ALL \
              SELECT p.id, p.parent FROM sessions p JOIN up ON p.id = up.parent) \
          SELECT r.",
            $col,
            " FROM up JOIN sessions r ON r.id = up.id WHERE up.parent IS NULL)"
        )
    };
}

/// Column list every `sessions` reader selects, qualified with the table name
/// so joins against `tree` stay unambiguous. A macro (not a const) so it
/// splices into `concat!` and the query stays a `'static` literal.
macro_rules! session_cols {
    () => {
        concat!(
            "sessions.id, sessions.created_at, sessions.updated_at, sessions.title, sessions.tags, sessions.model, \
             sessions.parent, sessions.name, sessions.effort, sessions.agent, sessions.cwd, sessions.harness_model, \
             sessions.context_size, sessions.desk, sessions.pinned, ",
            root_col!("archived"),
            " AS archived, ",
            root_col!("project"),
            " AS project, ",
            root_col!("notebook"),
            " AS notebook, sessions.owner, sessions.epoch, sessions.kind, sessions.memory, \
             sessions.acp_session, sessions.observed_source, sessions.observed_state, sessions.observed_stop, \
             sessions.observed_cancel, sessions.observed_description, sessions.observed_tool_call, \
             sessions.observed_detail, sessions.observed_cost_rule"
        )
    };
}

impl SessionRow {
    /// The summary as readers return it; `me` is this engine's id (`""` on a
    /// lone engine), which decides `here` (68 §5).
    fn summary(self, me: &str) -> anyhow::Result<SessionSummary> {
        let r = self;
        let observed = r.observed()?;
        Ok(SessionSummary {
            id: r.id.parse().context("bad session id")?,
            created_at: r.created_at.parse().context("bad created_at")?,
            updated_at: r.updated_at.parse().context("bad updated_at")?,
            title: r.title,
            tags: r
                .tags
                .split(',')
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect(),
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
            harness_model: r.harness_model,
            context_size: r.context_size.and_then(|n| u32::try_from(n).ok()),
            desk: r.desk,
            pinned: r.pinned,
            archived: r.archived,
            project: r.project,
            notebook: r.notebook,
            retention: tools::Retention::Durable,
            running: false,
            here: r.owner.is_empty() || r.owner == me,
            owner: r.owner,
            epoch: u32::try_from(r.epoch).context("bad epoch")?,
            kind: advisor::kind_of(&r.kind),
            memory: r.memory.parse().unwrap_or_default(),
            observed,
        })
    }

    /// The watch-only facts (141 §5.1) when `observed_source` is set. Unknown
    /// stored strings (a newer engine's values) fall back to `unknown` /
    /// the source's default cost rule rather than failing the read.
    fn observed(&self) -> anyhow::Result<Option<ObservedMeta>> {
        let Some(source) = self.observed_source.as_deref() else {
            return Ok(None);
        };
        let source: ObservedSource = source
            .parse()
            .map_err(|e: String| anyhow::anyhow!("{e} on session {}", self.id))?;
        let state = self
            .observed_state
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(ObservedState::Unknown);
        let cost_rule = self
            .observed_cost_rule
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| default_cost_rule(source));
        let detail = match self.observed_detail.as_deref() {
            Some(raw) => serde_json::from_str(raw)
                .with_context(|| format!("bad observed_detail on session {}", self.id))?,
            None => serde_json::Value::Null,
        };
        Ok(Some(ObservedMeta {
            source,
            external: self.acp_session.clone().unwrap_or_default(),
            state,
            stop: self.observed_stop.clone(),
            can_cancel: self.observed_cancel,
            description: self.observed_description.clone(),
            tool_call: self.observed_tool_call.clone(),
            cost_rule,
            detail,
        }))
    }
}

/// The cost rule a source implies when none was recorded (141 §7.6): omp
/// background spawns count (sync spawns set `inside_parent` explicitly), RFD
/// usage is display-only, opencode reports nothing.
pub fn default_cost_rule(source: ObservedSource) -> CostRule {
    match source {
        ObservedSource::Rfd => CostRule::DisplayOnly,
        ObservedSource::OpencodeExt | ObservedSource::OpencodeTask => CostRule::Unreported,
        ObservedSource::OmpTask => CostRule::Counted,
    }
}

/// One observation of a watch-only child for [`Store::upsert_observed`]
/// (141 §11.2). Identity is `(parent, source, external)`; every other field
/// is `None` = unchanged (`stop: Some(None)` clears the stop text). On
/// creation the row gets `kind = observed`, no `agent`, `model` = the
/// parent's `harness_model` (else its `model`) as the display fallback, and
/// `harness_model` = `model` when given.
#[derive(Clone, Debug)]
pub struct ObservedUpsert<'a> {
    /// The ACP child, or the observed parent when nested.
    pub parent: SessionId,
    /// Which feed describes the child.
    pub source: ObservedSource,
    /// The harness's own id for the child (`sessions.acp_session`).
    pub external: &'a str,
    /// Display name; also the title.
    pub name: Option<&'a str>,
    /// The child's own model (`harness_model`).
    pub model: Option<&'a str>,
    /// Lifecycle state.
    pub state: Option<ObservedState>,
    /// Stop reason / error text; `Some(None)` clears it.
    pub stop: Option<Option<&'a str>>,
    /// The harness accepts `session/cancel` for this child.
    pub can_cancel: Option<bool>,
    /// Roster card second line.
    pub description: Option<&'a str>,
    /// The spawning tool call id in the parent's transcript.
    pub tool_call: Option<&'a str>,
    /// Cost rule (141 §7.6); unset rows read as the source's default.
    pub cost_rule: Option<CostRule>,
    /// Live fields merged shallowly (top-level keys) into `observed_detail`.
    pub detail: Option<&'a serde_json::Map<String, serde_json::Value>>,
}

impl<'a> ObservedUpsert<'a> {
    /// Identity only; every other field unchanged.
    pub fn new(parent: SessionId, source: ObservedSource, external: &'a str) -> Self {
        Self {
            parent,
            source,
            external,
            name: None,
            model: None,
            state: None,
            stop: None,
            can_cancel: None,
            description: None,
            tool_call: None,
            cost_rule: None,
            detail: None,
        }
    }
}

/// One `reminders` row (14 as-built 2026-09-23); times are unix seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderRow {
    /// `r<n>`, per session.
    pub id: String,
    /// Owning session.
    pub session: SessionId,
    /// What comes back as `<reminder>`.
    pub message: String,
    /// When it was set.
    pub set_at: i64,
    /// When it fires.
    pub fires_at: i64,
    /// Requested delay.
    pub seconds: u64,
}

/// One tool call made from inside a `python` cell (08 as-built): call id
/// `<parent>.<n>`, where the parent is the cell's own call id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerCall {
    /// 1-based position within the cell.
    pub n: u32,
    /// Tool name.
    pub name: String,
    /// Raw arguments.
    pub args: serde_json::Value,
    /// `(output, is_error)`; `None` while the call has not finished.
    pub result: Option<(String, bool)>,
    /// Files the tool produced.
    pub files: Vec<PathBuf>,
}

/// `(parent call id, n)` of an inner call id `<parent>.<n>` (08 §4.1);
/// `None` for a top-level call id.
pub fn inner_call_id(call_id: &str) -> Option<(&str, u32)> {
    let (parent, n) = call_id.rsplit_once('.')?;
    Some((parent, n.parse().ok()?))
}

/// Most parent hops `Store::root_of` follows before it gives up on a
/// corrupt (cyclic) chain; deeper trees than this do not exist.
const ROOT_HOPS: usize = 32;

/// 183 R1/R2 (E-M3): the schema of the replicated session tables, as
/// sessionsync sends it in `Hello` and `Release`: a peer on an older value
/// is not shipped rows whose columns it cannot apply, and its takes are
/// refused. Named after the migration that last changed them (0063:
/// `sessions.endless_effort`, `sessions.endless_agent_effort`).
///
/// **Bump rule:** set this to a new migration's number exactly when that
/// migration adds, drops or renames a column of a table the session log
/// replicates (`sessions`, `messages`, `turn_steps`, … — the tables the
/// `sqlite3session` attaches, see [`Store::open`]). A migration that touches
/// only local tables leaves it alone; bumping it holds session sync between
/// mixed versions for nothing. Never a lower or an unrelated number: a peer
/// whose value reads as newer is shipped rows its schema cannot apply, and
/// `sqlite3changeset_apply` skips the table while the cursor advances.
pub const SESSIONS_WIRE_SCHEMA: u32 = 63;

impl Store {
    /// Open (creating) the database in WAL mode and run migrations. Two
    /// pools on one file (68 §3): `write` holds exactly one connection that
    /// never expires, with a `sqlite3session` attached to the transcript
    /// tables; `read` holds four, opened on demand and never recycled (sqlx's defaults
    /// would reap idle connections every 10 min and churn a worker thread
    /// per reconnect). The writer runs `synchronous=NORMAL`: in WAL mode a
    /// committed transaction is in the WAL file before `COMMIT` returns, so
    /// an engine kill loses nothing; only an OS crash or power loss can
    /// drop the last commits, against ~5 fsyncs saved per model call.
    /// `engine` is this engine's mesh id: rows
    /// still owned by `''` (pre-68, or created while unknown) are backfilled
    /// to it; `None` keeps `''` and treats it as self.
    pub async fn open(path: &Path, engine: Option<mesh::EngineId>) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        // Migrations (and any backfill) run before the capture session exists:
        // DDL on an attached table would leave the session unusable.
        let read = SqlitePoolOptions::new()
            .max_connections(4)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts.clone())
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&read)
            .await
            .context("run migrations")?;
        let me: Arc<str> = engine.map(|e| e.0).unwrap_or_default().into();
        if !me.is_empty() {
            // 68 §8 / 139 D17: a store written before this engine had a mesh
            // identity; local and unreplicated (Revision 1 Q6: log only).
            let n = sqlx::query("UPDATE sessions SET owner = ? WHERE owner = ''")
                .bind(&*me)
                .execute(&read)
                .await
                .context("backfill sessions.owner")?
                .rows_affected();
            if n > 0 {
                tracing::info!("68: claimed {n} unowned session(s) as {me}");
            }
        }
        // 180 D3: owners that replicated `sessions`, `model_calls`,
        // `remote_meters` and `remote_turns` rows reference but no ledger row
        // backs (the record replica delivers `cost_owners` on its own stream,
        // often minutes after the session log). Stand-ins carry the sentinel
        // HLC, so the record replica lets the owner's real row replace them.
        match repair_cost_owners(&read).await {
            Ok(r) if r.derived + r.tombstoned > 0 => tracing::info!(
                "accounting: repaired {} missing cost owner(s): {} derived from sessions, {} tombstoned root(s)",
                r.derived + r.tombstoned,
                r.derived,
                r.tombstoned
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("accounting: cost owner repair: {e:#}"),
        }
        let session: Arc<Mutex<Option<capture::Session>>> = Arc::new(Mutex::new(None));
        let slot = session.clone();
        let write = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .after_connect(move |conn, _| {
                let slot = slot.clone();
                Box::pin(async move {
                    let db = conn.lock_handle().await?.as_raw_handle();
                    let fresh = unsafe { capture::Session::create(db) }
                        .map_err(|e| sqlx::Error::Configuration(e.into()))?;
                    if let Some(stale) = slot.lock().replace(fresh) {
                        // Its connection is gone (sqlx replaced it); deleting a
                        // session on a closed handle is undefined, so leak it.
                        tracing::warn!(
                            "store: write connection replaced; leaking its capture session"
                        );
                        std::mem::forget(stale);
                    }
                    Ok(())
                })
            })
            .connect_with(opts.synchronous(SqliteSynchronous::Normal))
            .await
            .with_context(|| format!("open {} (writer)", path.display()))?;
        let tables = table_meta(&read).await?;
        let (last_seq,): (Option<i64>,) = sqlx::query_as("SELECT MAX(seq) FROM session_log")
            .fetch_one(&read)
            .await?;
        let (last_hlc,): (Option<String>,) = sqlx::query_as("SELECT MAX(hlc) FROM session_log")
            .fetch_one(&read)
            .await?;
        let last = last_hlc
            .as_deref()
            .and_then(replica::Hlc::parse)
            .map_or((0, 0), |h| (h.wall_ms, h.counter));
        Ok(Self {
            session,
            private: false,
            clock: Arc::new(replica::Clock::new(hlc_origin(&me), last)),
            me,
            tables: Arc::new(tables),
            seq: Arc::new(AtomicI64::new(last_seq.unwrap_or(0))),
            reseed: Arc::default(),
            log_tx: Arc::new(tokio::sync::watch::Sender::new(last_seq.unwrap_or(0))),
            harness_tx: Arc::new(tokio::sync::watch::Sender::new(0)),
            write,
            read,
            #[cfg(test)]
            read_delay: Arc::default(),
        })
    }

    /// An in-memory database with the same schema, for a private root tree
    /// (51 §4.5): no file, no journal, `temp_store=MEMORY` so sorts and
    /// temp tables never spill to disk. Exactly one pooled connection:
    /// every SQLite `:memory:` connection is its own database, so a second
    /// one would see an empty, unmigrated store. No capture session and no
    /// log: private trees never replicate (68 decision 6).
    pub async fn open_memory() -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory)
            .pragma("temp_store", "MEMORY");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .context("open in-memory store")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self {
            session: Arc::new(Mutex::new(None)),
            private: true,
            me: Arc::from(""),
            tables: Arc::new(Vec::new()),
            seq: Arc::new(AtomicI64::new(0)),
            reseed: Arc::default(),
            log_tx: Arc::new(tokio::sync::watch::Sender::new(0)),
            harness_tx: Arc::new(tokio::sync::watch::Sender::new(0)),
            clock: Arc::new(replica::Clock::new(hlc_origin(""), (0, 0))),
            write: pool.clone(),
            read: pool,
            #[cfg(test)]
            read_delay: Arc::default(),
        })
    }

    /// Whether this is a private `:memory:` store (one pool, nothing captured).
    pub fn is_private(&self) -> bool {
        self.private
    }

    /// The read pool, for crate-internal probes (tests of pragmas and the like).
    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.read
    }

    /// 176 E5: make `session`, `list_sessions` and `descendants` take `d`
    /// longer on every clone of this store (zero turns it off).
    #[cfg(test)]
    pub(crate) fn set_read_delay(&self, d: Duration) {
        self.read_delay
            .store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// The [`Self::set_read_delay`] stall; nothing outside tests.
    async fn stall(&self) {
        #[cfg(test)]
        {
            let ms = self.read_delay.load(std::sync::atomic::Ordering::Relaxed);
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        }
    }

    /// Close both pools. The capture session goes first: deleting it after
    /// its connection closed is undefined behaviour.
    pub async fn close(&self) {
        drop(self.session.lock().take());
        self.write.close().await;
        self.read.close().await;
    }

    /// Begin the one write transaction (`BEGIN IMMEDIATE` on the single
    /// write connection). Every mutating statement in this module and its
    /// children runs inside one of these; a write anywhere else escapes
    /// capture.
    pub(crate) async fn begin_write(&self) -> anyhow::Result<WriteTx<'_>> {
        let mut tx = self.write.begin_with("BEGIN IMMEDIATE").await?;
        if self.reseed.swap(false, Ordering::SeqCst) {
            let max: anyhow::Result<(Option<i64>,)> =
                sqlx::query_as("SELECT MAX(seq) FROM session_log")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(Into::into);
            match max {
                // The head row always stays (prune), and the watch never steps back.
                Ok((max,)) => self.seq.store(
                    max.unwrap_or(0).max(*self.log_tx.borrow()),
                    Ordering::SeqCst,
                ),
                Err(e) => {
                    self.reseed.store(true, Ordering::SeqCst);
                    return Err(e);
                }
            }
        }
        Ok(WriteTx { tx, store: self })
    }

    /// Begin a transaction with capture disabled, for applying a remote
    /// changeset (68 §6). Serialized with every local write by the single
    /// connection; capture resumes when the returned value commits or drops.
    pub async fn begin_apply(&self) -> anyhow::Result<ApplyTx<'_>> {
        anyhow::ensure!(
            !self.private,
            "a private store never applies remote changes"
        );
        let tx = self.write.begin_with("BEGIN IMMEDIATE").await?;
        self.set_capture(false);
        Ok(ApplyTx {
            tx: Some(tx),
            store: self,
        })
    }

    fn set_capture(&self, on: bool) {
        if let Some(session) = self.session.lock().as_ref() {
            session.enable(on);
        }
    }

    /// This engine's id as `sessions.owner` spells it (`""` on a lone engine).
    pub fn engine(&self) -> &str {
        &self.me
    }

    /// 68 decision 8: `projects` and `cost_owners` are global records, not
    /// session rows — they join the stage-1 record replica (per-column LWW)
    /// on the write connection, so a replica that takes a session already
    /// holds the accounting owner its `model_calls` rows reference. 191 (D2): so
    /// do the mesh-wide contact records ([`contacts::RECORD_TABLES`]). None of
    /// these tables is in the capture session, so replicated rows never echo into
    /// the session log. Rows the replica has not stamped yet (every row of a
    /// pre-68 store; rows written since the last start — the accounting
    /// writers never stamp) are announced and stamped once. Returns how many
    /// were. Errors on a private store or without the replica plugin.
    pub async fn replicate_records(&self) -> anyhow::Result<u64> {
        anyhow::ensure!(!self.private, "a private store never replicates");
        let mut tables = vec!["projects", "cost_owners"];
        tables.extend(contacts::RECORD_TABLES);
        let handle = replica::attach(&self.write, &tables).await?;
        handle.announce_unstamped().await
    }

    /// Log rows above `seq` that are not orphaned, in seq order: what a peer
    /// behind `seq` still needs.
    pub async fn log_after(&self, seq: i64) -> anyhow::Result<Vec<LogRow>> {
        Ok(sqlx::query_as(
            "SELECT seq, session_id, epoch, changeset, hlc, orphaned FROM session_log \
             WHERE seq > ? AND orphaned = 0 ORDER BY seq",
        )
        .bind(seq)
        .fetch_all(&self.read)
        .await?)
    }

    /// [`Store::log_after`], at most `limit` rows: one page of a long catch-up.
    pub async fn log_page(&self, seq: i64, limit: i64) -> anyhow::Result<Vec<LogRow>> {
        Ok(sqlx::query_as(
            "SELECT seq, session_id, epoch, changeset, hlc, orphaned FROM session_log \
             WHERE seq > ? AND orphaned = 0 ORDER BY seq LIMIT ?",
        )
        .bind(seq)
        .bind(limit)
        .fetch_all(&self.read)
        .await?)
    }

    /// Orphaned rows of `session` (68 §4.3), in seq order: the turns a
    /// demoted owner never delivered.
    pub async fn log_orphans(&self, session: SessionId) -> anyhow::Result<Vec<LogRow>> {
        Ok(sqlx::query_as(
            "SELECT seq, session_id, epoch, changeset, hlc, orphaned FROM session_log \
             WHERE session_id = ? AND orphaned = 1 ORDER BY seq",
        )
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?)
    }

    /// Mark every row of `(session, epoch)` orphaned: written by this engine
    /// as an owner that has since been superseded; never shipped. Returns
    /// how many rows were marked.
    pub async fn log_orphan(&self, session: SessionId, epoch: u32) -> anyhow::Result<u64> {
        self.log_orphan_after(session, epoch, 0).await
    }

    /// [`Store::log_orphan`] for the rows above `after` only: what the new
    /// owner had not applied when it took over (68 §4.3).
    pub async fn log_orphan_after(
        &self,
        session: SessionId,
        epoch: u32,
        after: i64,
    ) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE session_log SET orphaned = 1 WHERE session_id = ? AND epoch = ? AND seq > ? AND orphaned = 0",
        )
        .bind(session.to_string())
        .bind(i64::from(epoch))
        .bind(after)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// `MAX(seq)` of the log rows of `session` (orphaned or not); `0` when none.
    pub async fn session_log_seq(&self, session: SessionId) -> anyhow::Result<i64> {
        let (seq,): (Option<i64>,) =
            sqlx::query_as("SELECT MAX(seq) FROM session_log WHERE session_id = ?")
                .bind(session.to_string())
                .fetch_one(&self.read)
                .await?;
        Ok(seq.unwrap_or(0))
    }

    /// Wait for every write ahead of this one: an empty transaction on the
    /// single write connection (68 §4.2 "flush"). Logs nothing.
    pub async fn flush(&self) -> anyhow::Result<()> {
        self.begin_write().await?.commit().await
    }

    /// Append `message` to `id` **without** logging it (capture off): a note
    /// meant for this engine's copy only, such as the demoted owner's
    /// "N turns were not delivered" (68 §4.3). No owner guard.
    pub async fn append_local_note(&self, id: SessionId, text: &str) -> anyhow::Result<i64> {
        let mut tx = self.begin_apply().await?;
        let seq = append_in(&mut tx, &id.to_string(), &ChatMessage::system(text)).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// Delete shipped rows below `seq` (every known peer is past them, or a
    /// snapshot supersedes them). Orphans stay; see [`Store::prune_orphans_before`].
    pub async fn prune_log_below(&self, seq: i64) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("DELETE FROM session_log WHERE seq < ? AND orphaned = 0")
            .bind(seq)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// Delete orphaned rows stamped before `wall_ms` (`sessions.orphan_keep_days`).
    pub async fn prune_orphans_before(&self, wall_ms: u64) -> anyhow::Result<u64> {
        let bound = format!("{wall_ms:016x}");
        let mut tx = self.begin_write().await?;
        let n =
            sqlx::query("DELETE FROM session_log WHERE orphaned = 1 AND substr(hlc, 1, 16) < ?")
                .bind(bound)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// The next `session_log.seq` minus one: may count a transaction still
    /// in flight. What peers can be shipped is [`Store::log_head`].
    pub fn log_seq(&self) -> i64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// The last committed log seq (the [`Store::log_watch`] value).
    pub fn log_head(&self) -> i64 {
        *self.log_tx.borrow()
    }

    /// Wakes with the new [`Store::log_seq`] after every commit that logged a row.
    pub fn log_watch(&self) -> tokio::sync::watch::Receiver<i64> {
        self.log_tx.subscribe()
    }

    /// Does this engine own at least one session? What a peer can
    /// bootstrap from us even when the log is empty (a migrated store).
    pub async fn owns_sessions(&self) -> anyhow::Result<bool> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM sessions WHERE owner = ?)")
                .bind(&*self.me)
                .fetch_one(&self.read)
                .await?,
        )
    }

    /// Every `(origin, peer) → seq` cursor.
    pub async fn cursors(&self) -> anyhow::Result<Vec<Cursor>> {
        Ok(
            sqlx::query_as("SELECT origin, peer, seq FROM session_cursors ORDER BY origin, peer")
                .fetch_all(&self.read)
                .await?,
        )
    }

    /// Record that `peer` has applied `origin`'s log through `seq`
    /// (`peer = ""`: this engine's own applied cursor for a remote origin).
    pub async fn set_cursor(&self, origin: &str, peer: &str, seq: i64) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO session_cursors (origin, peer, seq) VALUES (?, ?, ?) \
             ON CONFLICT(origin, peer) DO UPDATE SET seq = excluded.seq",
        )
        .bind(origin)
        .bind(peer)
        .bind(seq)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The epoch fence of `session`: highest `(epoch, owner)` seen, from
    /// `session_epochs`, falling back to the session row itself.
    pub async fn fence(&self, session: SessionId) -> anyhow::Result<Option<(u32, String)>> {
        let sid = session.to_string();
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT epoch, owner FROM session_epochs WHERE session_id = ?1 \
             UNION ALL SELECT epoch, owner FROM sessions WHERE id = ?1 \
             ORDER BY epoch DESC LIMIT 1",
        )
        .bind(&sid)
        .fetch_optional(&self.read)
        .await?;
        row.map(|(e, o)| Ok((u32::try_from(e).context("bad epoch")?, o)))
            .transpose()
    }

    /// Raise the fence of `session` to `(epoch, owner)`; a lower epoch is
    /// ignored. Returns whether it moved.
    pub async fn set_fence(
        &self,
        session: SessionId,
        epoch: u32,
        owner: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "INSERT INTO session_epochs (session_id, epoch, owner) VALUES (?, ?, ?) \
             ON CONFLICT(session_id) DO UPDATE SET epoch = excluded.epoch, owner = excluded.owner \
             WHERE excluded.epoch > session_epochs.epoch",
        )
        .bind(session.to_string())
        .bind(i64::from(epoch))
        .bind(owner)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// `(owner, epoch)` of the session row; `None` for an unknown id.
    pub async fn owner(&self, session: SessionId) -> anyhow::Result<Option<(String, u32)>> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT owner, epoch FROM sessions WHERE id = ?")
                .bind(session.to_string())
                .fetch_optional(&self.read)
                .await?;
        row.map(|(o, e)| Ok((o, u32::try_from(e).context("bad epoch")?)))
            .transpose()
    }

    /// Write `owner`/`epoch` on the session row (68 §4.2 step 2): the take
    /// itself, logged like any write so it is the first row of the new epoch
    /// on every replica. Passes the owner guard only when `owner` is this
    /// engine (or the session is already ours). `Err("no such session")` when
    /// the row is missing.
    pub async fn set_owner(
        &self,
        session: SessionId,
        owner: &str,
        epoch: u32,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("UPDATE sessions SET owner = ?, epoch = ? WHERE id = ?")
            .bind(owner)
            .bind(i64::from(epoch))
            .bind(session.to_string())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        anyhow::ensure!(n == 1, "no such session");
        tx.commit().await
    }

    /// Test probe: the log rows written since `*since` (which moves to the
    /// newest seq). Panics when there are none — the last mutating call ran
    /// outside the write pool or touched no attached table. Empty, without
    /// panicking, on a private store.
    #[cfg(test)]
    pub(crate) async fn assert_write_pool(&self, what: &str, since: &mut i64) -> Vec<LogRow> {
        if self.private {
            return Vec::new();
        }
        let rows = self.log_after(*since).await.unwrap();
        assert!(
            !rows.is_empty(),
            "{what}: no session_log row from the write connection"
        );
        *since = rows.last().map_or(*since, |r| r.seq);
        rows
    }

    /// Insert a session row and its accounting owner in one transaction.
    /// `title` starts empty; `name` is the peer name of a subagent; `project`
    /// is stored on roots only (a child's is its root's, 51 §3). The row is
    /// a `chat`; [`Store::create_session_kind`] for an advisor.
    pub async fn create_session(
        &self,
        id: SessionId,
        model: &str,
        parent: Option<SessionId>,
        name: Option<&str>,
        project: Option<&str>,
    ) -> anyhow::Result<()> {
        self.create_session_kind(id, model, parent, name, project, SessionKind::Chat)
            .await
    }

    /// [`Store::create_session`] with an explicit `sessions.kind` (125 §6.1).
    pub async fn create_session_kind(
        &self,
        id: SessionId,
        model: &str,
        parent: Option<SessionId>,
        name: Option<&str>,
        project: Option<&str>,
        kind: SessionKind,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO sessions (id, created_at, updated_at, title, model, parent, name, project, owner, epoch, kind) \
             VALUES (?, ?, ?, '', ?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(id.to_string())
        .bind(&now)
        .bind(&now)
        .bind(model)
        .bind(parent.map(|p| p.to_string()))
        .bind(name)
        .bind(parent.is_none().then_some(project).flatten())
        .bind(&*self.me)
        .bind(advisor::kind_str(kind))
        .execute(&mut *tx)
        .await?;
        accounting::ensure_owner_in(&mut tx, id, parent).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Set the display title (first user message, truncated by the caller).
    pub async fn set_title(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Set `sessions.kind` of an existing row (144 §9: a root created as a chat
    /// becomes the `ask` session the moment the ask run claims it).
    pub async fn set_kind(&self, id: SessionId, kind: SessionKind) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET kind = ? WHERE id = ?")
            .bind(advisor::kind_str(kind))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Pin (or unpin) a session: pinned rows sort first in `list_sessions`.
    pub async fn set_pinned(&self, id: SessionId, pinned: bool) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET pinned = ? WHERE id = ?")
            .bind(pinned)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Mark (or unmark) a session as `agent`'s desk (67 §6.2); the roster's write.
    pub async fn set_desk(&self, id: SessionId, agent: Option<&str>) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET desk = ? WHERE id = ?")
            .bind(agent)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The session marked as `agent`'s desk on this engine, if any
    /// (STALE-WAKE-DESKS: at most one per `(desk, owner)`, archived or not —
    /// a desk put away by an earlier build is still the desk).
    pub async fn desk_session(&self, agent: &str) -> anyhow::Result<Option<SessionId>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM sessions WHERE desk = ? AND (owner = '' OR owner = ?) ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .bind(agent)
        .bind(&*self.me)
        .fetch_optional(&self.read)
        .await?;
        Ok(row.and_then(|(id,)| id.parse().ok()))
    }

    /// Archive (or unarchive) a session: archived roots and their trees leave
    /// the sidebar but stay searchable (51 §3). Stored on the row; readers
    /// resolve the root's value, so call this on the root.
    pub async fn set_archived(&self, id: SessionId, archived: bool) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET archived = ? WHERE id = ?")
            .bind(archived)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record a model switch (canonical `provider/model` id).
    pub async fn set_model(&self, id: SessionId, model: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET model = ? WHERE id = ?")
            .bind(model)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record the reasoning effort; `None` clears it (provider default).
    pub async fn set_effort(&self, id: SessionId, effort: Option<&str>) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET effort = ? WHERE id = ?")
            .bind(effort)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 183 §4.5 (W-4): the session's approvals pin: `Some(true)` = `'on'`,
    /// `Some(false)` = `'off'`, `None` = inherit `tools.approvals` (NULL).
    pub async fn set_session_approvals(
        &self,
        id: SessionId,
        pin: Option<bool>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET approvals = ? WHERE id = ?")
            .bind(pin.map(|on| if on { "on" } else { "off" }))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 183 §4.5: the session's agent-messaging setting: `Some(false)` = `0`
    /// (opted out), `Some(true)` = `1`, `None` = inherit (NULL). The policy
    /// only has an opt-out, so callers store opt-in as `None`.
    pub async fn set_session_communication(
        &self,
        id: SessionId,
        enabled: Option<bool>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET communication = ? WHERE id = ?")
            .bind(enabled)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 183 §4.5 (D4): `(approvals pin, communication)` of the row, as the
    /// setters wrote them; `None` when the session does not exist. Read by the
    /// supervisor when it spawns the actor (start, resume, take).
    pub async fn session_settings(
        &self,
        id: SessionId,
    ) -> anyhow::Result<Option<(Option<bool>, Option<bool>)>> {
        self.stall().await;
        let row: Option<(Option<String>, Option<bool>)> =
            sqlx::query_as("SELECT approvals, communication FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row.map(|(a, c)| (a.map(|a| a == "on"), c)))
    }

    /// Per-chat tool hides (`set_mcp_hidden`): the session's own hidden tool
    /// names as `sessions.hidden_tools` (a JSON array; empty stores NULL).
    /// Written by the owner's registry before it replies.
    pub async fn set_session_hidden(&self, id: SessionId, names: &[String]) -> anyhow::Result<()> {
        let json = (!names.is_empty())
            .then(|| serde_json::to_string(names))
            .transpose()?;
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET hidden_tools = ? WHERE id = ?")
            .bind(json)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The names [`Store::set_session_hidden`] wrote; empty when none or the
    /// session does not exist. `Err` on a list that does not parse. Read by
    /// the supervisor when it spawns the actor (start, resume, take).
    pub async fn session_hidden(&self, id: SessionId) -> anyhow::Result<Vec<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT hidden_tools FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        match row.and_then(|(j,)| j) {
            Some(j) => serde_json::from_str(&j).context("sessions.hidden_tools"),
            None => Ok(Vec::new()),
        }
    }

    /// 136 R-22: persist the session's money cap (`None` clears) so a
    /// durability-resumed turn runs under it after a restart.
    pub async fn set_budget(
        &self,
        id: SessionId,
        budget: Option<&crate::Budget>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET budget = ? WHERE id = ?")
            .bind(budget.map(serde_json::to_string).transpose()?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The cap recorded by [`Store::set_budget`]; `None` when unset.
    pub async fn budget(&self, id: SessionId) -> anyhow::Result<Option<crate::Budget>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT budget FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row
            .and_then(|(s,)| s)
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }

    /// Remember that `model`'s provider refused image input in session `id`
    /// (read-file-images notes §2), so a resumed session never sends that
    /// model an image. A local table: the session log does not replicate it.
    pub async fn refuse_images(&self, id: SessionId, model: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("INSERT OR IGNORE INTO image_refusals (session, model) VALUES (?, ?)")
            .bind(id.to_string())
            .bind(model)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Forget [`Store::refuse_images`] for `model` in session `id`: the user
    /// re-picked the model, or the refusal turned out not to be one.
    pub async fn allow_images(&self, id: SessionId, model: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM image_refusals WHERE session = ? AND model = ?")
            .bind(id.to_string())
            .bind(model)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The model ids recorded by [`Store::refuse_images`].
    pub async fn image_refusals(&self, id: SessionId) -> anyhow::Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT model FROM image_refusals WHERE session = ? ORDER BY model")
                .bind(id.to_string())
                .fetch_all(&self.read)
                .await?,
        )
    }

    /// Mark `id` as an external harness session: `agent` is the `agents.yaml`
    /// key, `cwd` the harness's working directory (design doc 06 §3.6),
    /// `skills` the theuth skills named for it (75 §10: replayed by a resume).
    pub async fn set_acp(
        &self,
        id: SessionId,
        agent: &str,
        cwd: Option<&Path>,
        skills: &[String],
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE sessions SET agent = ?, cwd = COALESCE(?, cwd), acp_skills = ? WHERE id = ?",
        )
        .bind(agent)
        .bind(cwd.map(|p| p.to_string_lossy().into_owned()))
        .bind(serde_json::to_string(skills)?)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The skill names recorded by [`Store::set_acp`]; empty when none or unset.
    pub async fn acp_skills(&self, id: SessionId) -> anyhow::Result<Vec<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT acp_skills FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row
            .and_then(|(s,)| s)
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default())
    }

    /// Record the harness's own session id, for `session/load` on resume.
    /// 146 D11: an id that replaces a different one (a fresh session after a
    /// failed load) keeps the replaced id in `acp_previous` for the RESUME brief.
    pub async fn set_acp_session(&self, id: SessionId, sid: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE sessions SET acp_previous = CASE \
                 WHEN acp_session IS NOT NULL AND acp_session != ?1 THEN acp_session \
                 ELSE acp_previous END, \
             acp_session = ?1 WHERE id = ?2",
        )
        .bind(sid)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 146 D11: the harness session id a fresh one replaced, if any.
    pub async fn acp_previous(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT acp_previous FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row.and_then(|(s,)| s))
    }

    /// The harness session id recorded by [`Store::set_acp_session`], if any.
    pub async fn acp_session(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT acp_session FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row.and_then(|(s,)| s))
    }

    // ----- 141: watch-only children (`kind = 'observed'`) -----

    /// Create or update a watch-only child (141 §11.2). Resolves the row by
    /// `(parent, source, external)`; creates it (`kind = observed`, no
    /// `agent`, cost owner under `parent`) when absent, else applies the
    /// `Some` fields. `detail` merges shallowly into the stored JSON.
    /// Returns the row id and whether it was created. One write transaction.
    pub async fn upsert_observed(
        &self,
        u: ObservedUpsert<'_>,
    ) -> anyhow::Result<(SessionId, bool)> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.begin_write().await?;
        let found: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, observed_detail FROM sessions \
             WHERE parent = ? AND observed_source = ? AND acp_session = ?",
        )
        .bind(u.parent.to_string())
        .bind(u.source.as_str())
        .bind(u.external)
        .fetch_optional(&mut *tx)
        .await?;
        let (id, created, stored_detail) = match found {
            Some((id, detail)) => (
                id.parse::<SessionId>().context("bad session id")?,
                false,
                detail,
            ),
            None => {
                let parent: Option<(String, Option<String>)> =
                    sqlx::query_as("SELECT model, harness_model FROM sessions WHERE id = ?")
                        .bind(u.parent.to_string())
                        .fetch_optional(&mut *tx)
                        .await?;
                let Some((parent_model, parent_harness_model)) = parent else {
                    anyhow::bail!("observed child: parent {} has no row", u.parent);
                };
                let id = SessionId::new();
                sqlx::query(
                    "INSERT INTO sessions (id, created_at, updated_at, title, model, parent, name, owner, epoch, kind, \
                     acp_session, observed_source, observed_state) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 'observed', ?, ?, 'pending')",
                )
                .bind(id.to_string())
                .bind(&now)
                .bind(&now)
                .bind(u.name.unwrap_or(""))
                .bind(parent_harness_model.unwrap_or(parent_model))
                .bind(u.parent.to_string())
                .bind(u.name.unwrap_or(u.external))
                .bind(&*self.me)
                .bind(u.external)
                .bind(u.source.as_str())
                .execute(&mut *tx)
                .await?;
                accounting::ensure_owner_in(&mut tx, id, Some(u.parent)).await?;
                (id, true, None)
            }
        };
        let detail = match u.detail {
            Some(patch) => {
                let mut merged: serde_json::Map<String, serde_json::Value> = stored_detail
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .with_context(|| format!("bad observed_detail on session {id}"))?
                    .unwrap_or_default();
                for (k, v) in patch {
                    merged.insert(k.clone(), v.clone());
                }
                Some(serde_json::Value::Object(merged).to_string())
            }
            None => None,
        };
        // COALESCE(?, col) keeps a column when the bind is NULL; `stop` is the
        // one field that can be cleared, so it binds a flag and a value.
        sqlx::query(
            "UPDATE sessions SET updated_at = ?, \
             name = COALESCE(?, name), title = COALESCE(?, title), \
             harness_model = COALESCE(?, harness_model), \
             observed_state = COALESCE(?, observed_state), \
             observed_stop = CASE WHEN ? THEN ? ELSE observed_stop END, \
             observed_cancel = COALESCE(?, observed_cancel), \
             observed_description = COALESCE(?, observed_description), \
             observed_tool_call = COALESCE(?, observed_tool_call), \
             observed_cost_rule = COALESCE(?, observed_cost_rule), \
             observed_detail = COALESCE(?, observed_detail) \
             WHERE id = ?",
        )
        .bind(&now)
        .bind(u.name)
        .bind(u.name)
        .bind(u.model)
        .bind(u.state.map(ObservedState::as_str))
        .bind(u.stop.is_some())
        .bind(u.stop.flatten())
        .bind(u.can_cancel)
        .bind(u.description)
        .bind(u.tool_call)
        .bind(u.cost_rule.map(CostRule::as_str))
        .bind(detail)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((id, created))
    }

    /// The watch-only children directly under `parent` (not nested ones),
    /// oldest first, each with its [`ObservedMeta`] (141 §7.2 seeds the
    /// actor's map from this on `Connected`).
    pub async fn observed_children(
        &self,
        parent: SessionId,
    ) -> anyhow::Result<Vec<(SessionSummary, ObservedMeta)>> {
        let rows: Vec<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions WHERE sessions.parent = ? AND sessions.observed_source IS NOT NULL \
             ORDER BY sessions.created_at, sessions.id"
        ))
        .bind(parent.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|r| {
                let s = r.summary(&self.me)?;
                let m = s
                    .observed
                    .clone()
                    .context("observed row without observed_source")?;
                Ok((s, m))
            })
            .collect()
    }

    /// The row id of the watch-only child `(parent, source, external)` names, if any.
    pub async fn observed_child(
        &self,
        parent: SessionId,
        source: ObservedSource,
        external: &str,
    ) -> anyhow::Result<Option<SessionId>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM sessions WHERE parent = ? AND observed_source = ? AND acp_session = ?",
        )
        .bind(parent.to_string())
        .bind(source.as_str())
        .bind(external)
        .fetch_optional(&self.read)
        .await?;
        row.map(|(id,)| id.parse().context("bad session id"))
            .transpose()
    }

    /// The omp transcript byte offset read so far (`observed_cursor`); `None`
    /// when nothing was read or the row is not observed.
    pub async fn observed_cursor(&self, id: SessionId) -> anyhow::Result<Option<u64>> {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT observed_cursor FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        row.and_then(|(c,)| c)
            .map(|c| u64::try_from(c).context("negative observed_cursor"))
            .transpose()
    }

    /// Persist the omp transcript byte offset after an applied batch (141 §7.3).
    pub async fn set_observed_cursor(&self, id: SessionId, cursor: u64) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET observed_cursor = ? WHERE id = ?")
            .bind(i64::try_from(cursor).context("observed_cursor overflows i64")?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Engine start (141 D11, §7.8): every non-terminal watch-only child whose
    /// parent is not in `live_parents` becomes `unknown` / `"lost contact"`.
    /// Terminal rows (`completed | failed | cancelled`) and already-`unknown`
    /// rows are untouched. Returns the ids changed, for `ObservedChildChanged`.
    pub async fn mark_observed_lost(
        &self,
        live_parents: &[SessionId],
    ) -> anyhow::Result<Vec<SessionId>> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.begin_write().await?;
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, parent FROM sessions WHERE observed_source IS NOT NULL \
             AND observed_state IN ('pending', 'running', 'requires_action', 'idle')",
        )
        .fetch_all(&mut *tx)
        .await?;
        let live: std::collections::HashSet<String> =
            live_parents.iter().map(ToString::to_string).collect();
        let mut lost = Vec::new();
        for (id, parent) in rows {
            if parent.is_some_and(|p| live.contains(&p)) {
                continue;
            }
            sqlx::query(
                "UPDATE sessions SET observed_state = 'unknown', observed_stop = 'lost contact', \
                 updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            lost.push(id.parse().context("bad session id")?);
        }
        tx.commit().await?;
        Ok(lost)
    }

    /// Record the model an external harness reports for this session
    /// (design doc 06 §8.2); published as [`crate::AgentEvent::HarnessModelChanged`].
    pub async fn set_harness_model(&self, id: SessionId, model: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET harness_model = ? WHERE id = ?")
            .bind(model)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record the command manifest an external harness announced (design doc
    /// 75 §3); published as [`crate::AgentEvent::HarnessCommandsChanged`].
    pub async fn set_harness_commands(
        &self,
        id: SessionId,
        commands: &[crate::HarnessCommand],
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET harness_commands = ? WHERE id = ?")
            .bind(serde_json::to_string(commands)?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The manifest [`Self::set_harness_commands`] stored; empty when none.
    pub async fn harness_commands(
        &self,
        id: SessionId,
    ) -> anyhow::Result<Vec<crate::HarnessCommand>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT harness_commands FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.read)
                .await?;
        match row.and_then(|(s,)| s) {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    /// Record the context window size an external harness reported (design doc 75 §5).
    pub async fn set_context_size(&self, id: SessionId, size: u32) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET context_size = ? WHERE id = ?")
            .bind(i64::from(size))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record the working directory an ACP client gave `session/new` (design doc 06 §4.1).
    pub async fn set_cwd(&self, id: SessionId, cwd: &Path) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET cwd = ? WHERE id = ?")
            .bind(cwd.to_string_lossy().into_owned())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Mark a root nobody reads directly (the scheduler's): its prompt gets no
    /// visualization instructions. Set once at creation; never cleared.
    pub async fn set_unattended(&self, id: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET unattended = 1 WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Whether [`Self::set_unattended`] marked this session; `false` for an unknown id.
    pub async fn unattended(&self, id: SessionId) -> anyhow::Result<bool> {
        let row = sqlx::query_as::<_, (bool,)>("SELECT unattended FROM sessions WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.read)
            .await?;
        Ok(row.is_some_and(|(u,)| u))
    }

    /// 185 A4: attended root sessions (no parent, not unattended) with
    /// activity after `since` (RFC 3339; `None` = ever) — every engine's,
    /// since session rows replicate with their `updated_at`.
    pub async fn attended_roots_since(&self, since: Option<&str>) -> anyhow::Result<u32> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sessions WHERE parent IS NULL AND NOT unattended \
             AND (?1 IS NULL OR julianday(updated_at) > julianday(?1))",
        )
        .bind(since)
        .fetch_one(&self.read)
        .await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// 185 R1 A4: the latest activity (RFC 3339) of another engine's session
    /// of the routine `name` — titled `name`, or `name · <subject>` for a
    /// watcher — from the replicated session rows: when it last ran there.
    pub async fn routine_ran_elsewhere(&self, name: &str) -> anyhow::Result<Option<String>> {
        let (at,): (Option<String>,) = sqlx::query_as(
            "SELECT MAX(updated_at) FROM sessions WHERE unattended AND parent IS NULL \
             AND owner != ?1 AND (title = ?2 OR substr(title, 1, length(?2) + 3) = ?2 || ' · ')",
        )
        .bind(&self.me)
        .bind(name)
        .fetch_one(&self.read)
        .await?;
        Ok(at.filter(|a| !a.is_empty()))
    }

    /// 136 D6: may this session run dreaming's consolidation ops as the
    /// owner's own root — `scope = 'owner'`, no parent, no desk, attended?
    /// Unattended (routine) sessions are never admitted here: only the open
    /// night's registered dreaming session consolidates unattended, and
    /// knowledge answers that itself. `false` for an unknown id.
    pub async fn may_consolidate(&self, id: SessionId) -> anyhow::Result<bool> {
        let row = sqlx::query_as::<_, (bool,)>(
            "SELECT scope = 'owner' AND parent IS NULL AND desk IS NULL AND NOT unattended \
             FROM sessions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row.is_some_and(|(ok,)| ok))
    }

    /// Append one message; returns its `seq` (max+1 within the session).
    /// Bumps `sessions.updated_at` (the list order).
    pub async fn append(&self, id: SessionId, message: &ChatMessage) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let seq = append_in(&mut tx, &id.to_string(), message).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// [`Store::append`] for a user row a channel delivered (172): the row
    /// and its `messages.channel` origin in one transaction.
    pub async fn append_user(
        &self,
        id: SessionId,
        message: &ChatMessage,
        channel: Option<&crate::ChannelOrigin>,
    ) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let sid = id.to_string();
        let seq = append_in(&mut tx, &sid, message).await?;
        channel_in(&mut tx, &sid, seq, channel).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// 172: the channel origin of every user row of `id` that has one, by
    /// seq. A row stored before 172 has none; malformed JSON is an error.
    pub async fn channels(
        &self,
        id: SessionId,
    ) -> anyhow::Result<BTreeMap<i64, crate::ChannelOrigin>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seq, channel FROM messages WHERE session_id = ? AND channel IS NOT NULL \
             ORDER BY seq",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(seq, raw)| {
                serde_json::from_str(&raw)
                    .with_context(|| format!("bad channel metadata on message {seq} of {id}"))
                    .map(|c| (seq, c))
            })
            .collect()
    }

    /// 141 R3 T7: a tool result that arrives after its call's turn was
    /// stored — the call's response row still reads `stale` (the turn
    /// closed it unfinished) → that row is rewritten; the call has no
    /// response row yet (an earlier engine life stored the call alone) →
    /// `response` is appended. `Ok(false)` when the transcript has no such
    /// call or already holds its real result (nothing to do).
    pub async fn finish_tool_call(
        &self,
        id: SessionId,
        response: &ChatMessage,
        stale: &str,
    ) -> anyhow::Result<bool> {
        let Some(call_id) = response
            .content
            .tool_responses()
            .first()
            .map(|r| r.call_id.clone())
        else {
            return Ok(false);
        };
        let mut called = false;
        let mut answered: Option<(i64, bool)> = None;
        for (seq, m) in self.messages(id).await? {
            if m.content.tool_calls().iter().any(|c| c.call_id == call_id) {
                called = true;
            }
            if let Some(r) = m
                .content
                .tool_responses()
                .iter()
                .find(|r| r.call_id == call_id)
            {
                answered = Some((seq, r.content == stale));
            }
        }
        match answered {
            Some((seq, true)) => {
                let body = serde_json::to_string(response)?;
                sqlx::query("UPDATE messages SET body = ? WHERE session_id = ? AND seq = ?")
                    .bind(body)
                    .bind(id.to_string())
                    .bind(seq)
                    .execute(&self.write)
                    .await?;
                Ok(true)
            }
            Some((_, false)) => Ok(false),
            None if called => {
                self.append(id, response).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Append a vision consultation note with its typed metadata in one
    /// transaction (52 §6): the row, `updated_at` and the revision commit
    /// together, so a note is either durable and typed or absent.
    pub async fn append_vision(
        &self,
        id: SessionId,
        message: &ChatMessage,
        note: &VisionNote,
    ) -> anyhow::Result<i64> {
        let vision = serde_json::to_string(note)?;
        let mut tx = self.begin_write().await?;
        let seq = insert_in(&mut tx, &id.to_string(), message, Some(&vision), None).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// [`Store::append`] for the model's reply row with the ms it thought
    /// before it (91 THOUGHT-TIME); `None` when no reasoning streamed.
    pub async fn append_thinking(
        &self,
        id: SessionId,
        message: &ChatMessage,
        thinking_ms: Option<u64>,
    ) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let ms = thinking_ms.map(|ms| i64::try_from(ms).unwrap_or(i64::MAX));
        let seq = insert_in(&mut tx, &id.to_string(), message, None, ms).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// Typed notes of every visible raw row, by seq (history rendering).
    /// Masks and the epoch boundary are not applied; malformed metadata is
    /// an error, never a plain message.
    pub async fn vision_notes(&self, id: SessionId) -> anyhow::Result<BTreeMap<i64, VisionNote>> {
        let mut conn = self.read.acquire().await?;
        rows_in(&mut conn, id, 0)
            .await?
            .iter()
            .filter_map(|r| r.vision.as_deref().map(|raw| (r.seq, raw)))
            .map(|(seq, raw)| Ok((seq, note_of(id, seq, raw)?)))
            .collect()
    }

    /// The effective frame plus the image index and active notes at one
    /// revision, for text-only preparation (52 §4.2, §6). Images come from
    /// the raw head/rows and their masks, so each carries its original
    /// coordinates even when the projected part vector is shorter. A note
    /// counts only while its row is visible, in the tail and not removed.
    pub async fn vision_frame(&self, id: SessionId) -> anyhow::Result<VisionFrame> {
        let sid = id.to_string();
        let mut tx = self.read.begin().await?;
        let revision = revision_in(&mut tx, &sid).await?.to_string();
        let latest = epochs_in(&mut tx, &sid).await?.pop();
        let (epoch, tail_from, at_seq, head) = match latest {
            Some((e, mask)) => (e.n, e.tail_from, e.at_seq, Some((e.head, mask))),
            None => (0, 0, -1, None),
        };
        let rows = rows_in(&mut tx, id, tail_from).await?;
        drop(tx);
        let mut images = Vec::new();
        if let Some((h, mask)) = &head {
            images.extend(crate::vision::images(ContextOwner::Head { epoch }, h, mask));
        }
        let mut notes = BTreeMap::new();
        for r in &rows {
            images.extend(crate::vision::images(
                ContextOwner::Message { seq: r.seq },
                &r.message,
                &r.mask,
            ));
            if let (false, Some(raw)) = (r.mask.whole, r.vision.as_deref()) {
                notes.insert(r.seq, note_of(id, r.seq, raw)?);
            }
        }
        let active = Active {
            epoch,
            tail_from,
            at_seq,
            head,
            rows: rows.into_iter().map(Row::into_active).collect(),
        };
        let (head, tail) = active.effective();
        Ok(VisionFrame {
            epoch,
            head,
            tail,
            images,
            notes,
            revision,
        })
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
        let mut tx = self.begin_write().await?;
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
        let mut tx = self.begin_write().await?;
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
        let mut conn = self.read.acquire().await?;
        todos_in(&mut conn, &id.to_string()).await
    }

    /// Record a UI-only failure behind the last visible message (`-1` in an
    /// empty session). Not a context change: no revision or `updated_at` bump.
    pub async fn record_error(&self, id: SessionId, text: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO errors (id, session_id, at_seq, text) \
             SELECT ?3, ?1, COALESCE(MAX(seq), -1), ?2 FROM messages WHERE session_id = ?1 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807)",
        )
        .bind(id.to_string())
        .bind(text)
        .bind(uuid::Uuid::now_v7().to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
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
        .fetch_all(&self.read)
        .await?)
    }

    /// An inner call began (08 as-built): a row with no output yet, anchored
    /// like [`Store::record_error`]. Not a context change.
    pub async fn inner_call_started(
        &self,
        id: SessionId,
        call_id: &str,
        name: &str,
        args: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let Some((parent, n)) = inner_call_id(call_id) else {
            anyhow::bail!("not an inner call id: {call_id:?}");
        };
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO inner_calls (session_id, parent_id, n, at_seq, name, args, at) \
             SELECT ?1, ?2, ?3, COALESCE(MAX(seq), -1), ?4, ?5, ?6 FROM messages WHERE session_id = ?1 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807)",
        )
        .bind(id.to_string())
        .bind(parent)
        .bind(n)
        .bind(name)
        .bind(args.to_string())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// An inner call returned: complete its row (`output` already capped by
    /// the publisher). A missing start row is created with an unknown name.
    pub async fn inner_call_finished(
        &self,
        id: SessionId,
        call_id: &str,
        output: &str,
        is_error: bool,
        files: &[PathBuf],
    ) -> anyhow::Result<()> {
        let Some((parent, n)) = inner_call_id(call_id) else {
            anyhow::bail!("not an inner call id: {call_id:?}");
        };
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO inner_calls (session_id, parent_id, n, at_seq, name, args, output, is_error, files, at) \
             SELECT ?1, ?2, ?3, COALESCE(MAX(seq), -1), '?', 'null', ?4, ?5, ?6, ?7 FROM messages WHERE session_id = ?1 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
             ON CONFLICT(session_id, parent_id, n) DO UPDATE SET \
             output = excluded.output, is_error = excluded.is_error, files = excluded.files, at = excluded.at",
        )
        .bind(id.to_string())
        .bind(parent)
        .bind(n)
        .bind(output)
        .bind(is_error)
        .bind(serde_json::to_string(files)?)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Every inner call of the session by parent call id, each list in `n`
    /// order. Raw: rows of a hidden or removed parent are included and the
    /// reader drops them by never finding their parent card.
    pub async fn inner_calls(
        &self,
        id: SessionId,
    ) -> anyhow::Result<HashMap<String, Vec<InnerCall>>> {
        #[derive(FromRow)]
        struct Row {
            parent_id: String,
            n: u32,
            name: String,
            args: String,
            output: Option<String>,
            is_error: bool,
            files: String,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT parent_id, n, name, args, output, is_error, files FROM inner_calls \
             WHERE session_id = ? ORDER BY parent_id, n",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        let mut out: HashMap<String, Vec<InnerCall>> = HashMap::new();
        for r in rows {
            out.entry(r.parent_id).or_default().push(InnerCall {
                n: r.n,
                name: r.name,
                args: serde_json::from_str(&r.args).unwrap_or(serde_json::Value::Null),
                result: r.output.map(|o| (o, r.is_error)),
                files: serde_json::from_str(&r.files).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// All visible messages with their seq, in `seq` order. Raw: exclusion
    /// masks are not applied (turn identity, undo/redo, archive).
    pub async fn messages(&self, id: SessionId) -> anyhow::Result<Vec<(i64, ChatMessage)>> {
        let mut conn = self.read.acquire().await?;
        Ok(rows_in(&mut conn, id, 0)
            .await?
            .into_iter()
            .map(|r| (r.seq, r.message))
            .collect())
    }

    /// Rows hidden by undo (`seq >= cut_at`); empty when `cut_at` is NULL.
    async fn hidden(&self, id: SessionId) -> anyhow::Result<Vec<Row>> {
        let rows = sqlx::query_as(
            "SELECT seq, body, excluded, vision, at, thinking_ms, advisory, moved FROM messages WHERE session_id = ?1 \
             AND seq >= (SELECT cut_at FROM sessions WHERE id = ?1) ORDER BY seq",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        decode_masked(id, rows)
    }

    /// `(id, title, created_at, updated_at, archived)` of every session, for the search
    /// index (26 §2); `archived` is the root's (49 §7: hits mark archived chats).
    /// Advisor rows are excluded (125 §4.2).
    pub async fn search_titles(
        &self,
    ) -> anyhow::Result<Vec<(SessionId, String, String, String, bool)>> {
        let rows: Vec<(String, String, String, String, bool)> = sqlx::query_as(concat!(
            "SELECT id, title, created_at, updated_at, ",
            root_col!("archived"),
            " FROM sessions WHERE kind != 'advisor'"
        ))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(id, title, created_at, updated_at, archived)| {
                Ok((id.parse()?, title, created_at, updated_at, archived))
            })
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
             AND s.kind != 'advisor' \
             AND json_extract(m.body, '$.role') IN ('User', 'Assistant') \
             ORDER BY m.session_id, m.seq",
        )
        .fetch_all(&self.read)
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
        let mut tx = self.begin_write().await?;
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
        let mut conn = self.read.acquire().await?;
        let visible = rows_in(&mut conn, id, 0).await?;
        drop(conn);
        let Some(&i) = turn_starts(&visible).last() else {
            return Ok(None);
        };
        let r = &visible[i];
        self.set_cut(id, Some(r.seq)).await?;
        Ok(Some(text_of(&r.message.content)))
    }

    /// Restore the oldest hidden turn. `None` when nothing is hidden; `Some`
    /// is the restored turn's user text.
    pub async fn redo(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let hidden = self.hidden(id).await?;
        let starts = turn_starts(&hidden);
        let Some(&first) = starts.first() else {
            return Ok(None);
        };
        let next = starts.get(1).map(|&i| hidden[i].seq);
        self.set_cut(id, next).await?;
        Ok(Some(text_of(&hidden[first].message.content)))
    }

    /// Hide `seq >= turn` (a visible turn start) and return the opener row
    /// (design doc 29 §4.1). `Err("not a turn start")` when `turn` is not a
    /// visible turn start; the marker is untouched then.
    pub async fn cut_to(&self, id: SessionId, turn: i64) -> anyhow::Result<ChatMessage> {
        let mut conn = self.read.acquire().await?;
        let visible = rows_in(&mut conn, id, 0).await?;
        drop(conn);
        let i = turn_starts(&visible)
            .into_iter()
            .find(|&i| visible[i].seq == turn);
        let Some(i) = i else {
            anyhow::bail!("not a turn start");
        };
        self.set_cut(id, Some(turn)).await?;
        Ok(visible[i].message.clone())
    }

    /// Delete the hidden tail for real and clear `cut_at`; no-op when nothing
    /// is hidden. Masks go with their rows; the revision bumps because the
    /// deleted seqs can now be reused (48 §3.1).
    pub async fn commit_cut(&self, id: SessionId) -> anyhow::Result<()> {
        let sid = id.to_string();
        let mut tx = self.begin_write().await?;
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
            "DELETE FROM inner_calls WHERE session_id = ?1 \
             AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1)",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
        // 109 §3.2: the journal rows anchored at the cut go with their messages.
        durability::cut_journal_in(&mut tx, &sid).await?;
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

    /// 76 §7, the post-turn cut: the reply of the visible turn whose first
    /// row is `turn` (`TurnStarted.turn_seq`; a user row — after a cancelled
    /// turn that left no reply it is not a turn *start* in the store's sense,
    /// so any user row counts) had finished streaming when the user cut in:
    /// its last assistant row before the next turn start is replaced by the
    /// heard prefix `keep` (empty: the row is deleted). Tool rows stay; a
    /// removal mask on the rewritten row goes with the body it masked. One
    /// transaction with the revision bump. Returns the new revision, how many
    /// assistant rows the turn had (more than one: tool calls ran after the
    /// spoken narration and only the last row was trimmed) and the effective
    /// frame, as [`Store::frame`] would. `Err("not a user row" | "no assistant row")`.
    pub async fn trim_assistant(
        &self,
        id: SessionId,
        turn: i64,
        keep: &str,
    ) -> anyhow::Result<(
        String,
        usize,
        u32,
        Option<ChatMessage>,
        Vec<(i64, ChatMessage)>,
    )> {
        let sid = id.to_string();
        let mut tx = self.begin_write().await?;
        let visible = rows_in(&mut tx, id, 0).await?;
        let Some(at) = visible
            .iter()
            .position(|r| r.seq == turn && r.message.role == ChatRole::User)
        else {
            anyhow::bail!("not a user row");
        };
        let end = turn_starts(&visible)
            .into_iter()
            .find(|&i| i > at)
            .unwrap_or(visible.len());
        let assistants: Vec<i64> = visible[at..end]
            .iter()
            .filter(|r| r.message.role == ChatRole::Assistant && !r.is_note())
            .map(|r| r.seq)
            .collect();
        let Some(&seq) = assistants.last() else {
            anyhow::bail!("no assistant row");
        };
        if keep.trim().is_empty() {
            sqlx::query("DELETE FROM messages WHERE session_id = ? AND seq = ?")
                .bind(&sid)
                .bind(seq)
                .execute(&mut *tx)
                .await?;
        } else {
            let body = serde_json::to_string(&ChatMessage::assistant(keep))?;
            sqlx::query(
                "UPDATE messages SET body = ?, excluded = NULL WHERE session_id = ? AND seq = ?",
            )
            .bind(body)
            .bind(&sid)
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        }
        let (revision,): (i64,) = sqlx::query_as(
            "UPDATE sessions SET context_revision = context_revision + 1 WHERE id = ? \
             RETURNING context_revision",
        )
        .bind(&sid)
        .fetch_one(&mut *tx)
        .await?;
        let active = active_in(&mut tx, id).await?;
        tx.commit().await?;
        let (head, tail) = active.effective();
        Ok((
            revision.to_string(),
            assistants.len(),
            active.epoch,
            head,
            tail,
        ))
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
        let mut tx = self.begin_write().await?;
        let rows = rows_in(&mut tx, src, 0).await?;
        let mut starts = turn_starts(&rows)
            .into_iter()
            .map(|i| rows[i].seq)
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
            let Row {
                seq,
                message: m,
                mask,
                ..
            } = rows
                .iter()
                .filter(|r| {
                    (t..end).contains(&r.seq)
                        && r.message.role == ChatRole::Assistant
                        && !text_of(&r.message.content).is_empty()
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
            "INSERT INTO sessions (id, created_at, updated_at, title, tags, model, parent, name, effort, cwd, context_revision, project, owner, epoch) \
             SELECT ?, ?, ?, title, tags, model, NULL, NULL, effort, cwd, context_revision, ",
            root_col!("project"),
            ", ?, 1 FROM sessions WHERE id = ?"
        ))
        .bind(&new)
        .bind(&now)
        .bind(&now)
        .bind(&*self.me)
        .bind(&old)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            anyhow::bail!("no such session");
        }
        accounting::ensure_owner_in(&mut tx, id, None).await?;
        sqlx::query(
            "INSERT INTO messages (session_id, seq, body, excluded, vision, at, advisory, moved, channel) \
             SELECT ?1, seq, body, excluded, vision, at, advisory, moved, channel FROM messages WHERE session_id = ?2 AND seq < ?3 \
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
            "INSERT INTO errors (id, session_id, at_seq, text) \
             SELECT id || ':' || ?1, ?1, at_seq, text FROM errors WHERE session_id = ?2 \
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
            "INSERT INTO inner_calls (session_id, parent_id, n, at_seq, name, args, output, is_error, files, at) \
             SELECT ?1, parent_id, n, at_seq, name, args, output, is_error, files, at FROM inner_calls \
             WHERE session_id = ?2 AND at_seq < ?3 \
             AND at_seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?2), 9223372036854775807)",
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
        // read-file-images: a model that refused images still refuses them in the fork.
        sqlx::query(
            "INSERT OR IGNORE INTO image_refusals (session, model) \
             SELECT ?1, model FROM image_refusals WHERE session = ?2",
        )
        .bind(&new)
        .bind(&old)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Number of turns hidden by undo (the redo depth).
    pub async fn hidden_turns(&self, id: SessionId) -> anyhow::Result<u32> {
        let hidden = self.hidden(id).await?;
        Ok(u32::try_from(turn_starts(&hidden).len())?)
    }

    /// All visible epochs of a session, by `n` ascending. Raw heads (masks not applied).
    pub async fn epochs(&self, id: SessionId) -> anyhow::Result<Vec<Epoch>> {
        let mut conn = self.read.acquire().await?;
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
        let mut tx = self.begin_write().await?;
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
        let mut tx = self.begin_write().await?;
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
        let mut conn = self.read.acquire().await?;
        let active = active_in(&mut conn, id).await?;
        let (head, tail) = active.effective();
        Ok((active.epoch, head, tail))
    }

    /// Decimal `sessions.context_revision`; `Err` for an unknown session.
    pub async fn context_revision(&self, id: SessionId) -> anyhow::Result<String> {
        let mut conn = self.read.acquire().await?;
        Ok(revision_in(&mut conn, &id.to_string()).await?.to_string())
    }

    /// Active-context inventory at the current revision (48 §3.2), read in one
    /// snapshot. `editable`/`reason` are left for the session actor.
    pub async fn context_items(&self, id: SessionId) -> anyhow::Result<ContextSnapshot> {
        let mut tx = self.read.begin().await?;
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
        .fetch_optional(&self.read)
        .await?;
        let Some((body, excluded)) = row else {
            return Ok(None);
        };
        let m: ChatMessage = serde_json::from_str(&body).context("bad message row")?;
        Ok(project(&m, &Mask::decode(excluded.as_deref())?))
    }

    /// Set the title and tags only if the session is still at `revision`
    /// (48 §4.3: a title job that captured pre-edit content is discarded).
    /// `Ok(false)` when the revision moved on or the session is unknown.
    /// `tags` are stored comma-joined; the caller normalises them
    /// ([`crate::title::tags`]).
    pub async fn set_title_at_revision(
        &self,
        id: SessionId,
        title: &str,
        tags: &[String],
        revision: &str,
    ) -> anyhow::Result<bool> {
        let revision: i64 = revision
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("bad revision {revision:?}"))?;
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE sessions SET title = ?, tags = ? WHERE id = ? AND context_revision = ?",
        )
        .bind(title)
        .bind(tags.join(","))
        .bind(id.to_string())
        .bind(revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
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
        let mut tx = self.begin_write().await?;
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
    /// the stored rows; previews are the caller's to materialize. A vision
    /// note's entry carries its typed metadata (52 §6); malformed metadata
    /// is an error rather than a plain message.
    pub async fn context_history(&self, id: SessionId) -> anyhow::Result<ContextHistory> {
        let sid = id.to_string();
        let mut tx = self.read.begin().await?;
        let revision = revision_in(&mut tx, &sid).await?.to_string();
        let rows = rows_in(&mut tx, id, 0).await?;
        let epochs = epochs_in(&mut tx, &sid).await?;
        drop(tx);
        let latest = epochs.last().map(|(e, _)| (e.n, e.tail_from, e.at_seq));
        let (epoch, tail_from, at_seq) = latest.unwrap_or((0, 0, -1));
        let active = Active {
            epoch,
            tail_from,
            at_seq,
            head: epochs
                .last()
                .map(|(e, mask)| (e.head.clone(), mask.clone())),
            rows: rows
                .iter()
                .filter(|r| r.seq >= tail_from)
                .map(|r| (r.seq, r.message.clone(), r.mask.clone()))
                .collect(),
        };
        let groups = active.groups();
        let snapshot = active.snapshot(&revision, &groups);
        let thinking = rows
            .iter()
            .filter_map(|r| Some((r.seq, u64::try_from(r.thinking_ms?).ok()?)))
            .collect();
        let mut entries = BTreeMap::new();
        for r in &rows {
            let active_row = r.seq >= tail_from;
            let remove_with = if active_row && !r.mask.whole {
                groups.remove_with(r.seq)
            } else {
                Vec::new()
            };
            let mut e = entry(
                ContextOwner::Message { seq: r.seq },
                &r.message,
                &r.mask,
                active_row,
                remove_with,
            );
            if let Some(raw) = &r.vision {
                e.vision = Some(note_of(id, r.seq, raw)?);
            }
            e.at = r.at.clone();
            entries.insert(r.seq, e);
        }
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
            messages: rows.into_iter().map(|r| (r.seq, r.message)).collect(),
            epochs: epochs.into_iter().map(|(e, _)| e).collect(),
            entries,
            thinking,
            heads,
            snapshot,
        })
    }

    /// One session, if it exists.
    pub async fn session(&self, id: SessionId) -> anyhow::Result<Option<SessionSummary>> {
        self.stall().await;
        let row: Option<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions WHERE id = ?"
        ))
        .bind(id.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(|r| r.summary(&self.me)).transpose()
    }

    /// Tests: `id`'s row no longer reads ([`Self::session`] errs on its timestamp).
    #[cfg(test)]
    pub(crate) async fn break_session_for_test(&self, id: SessionId) {
        sqlx::query("UPDATE sessions SET created_at = 'not a time' WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.write)
            .await
            .unwrap();
    }

    /// Every session, pinned first, then most recently active first; advisor
    /// rows (125 §4.2) hidden.
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        self.list_sessions_incl(false).await
    }

    /// [`Store::list_sessions`] with `kind = advisor` rows included when
    /// `include_advisors` (the Advisors actor and cost readers).
    pub async fn list_sessions_incl(
        &self,
        include_advisors: bool,
    ) -> anyhow::Result<Vec<SessionSummary>> {
        self.stall().await;
        let rows: Vec<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions WHERE (? OR sessions.kind != 'advisor') \
             ORDER BY pinned DESC, updated_at DESC, id DESC"
        ))
        .bind(include_advisors)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(|r| r.summary(&self.me)).collect()
    }

    /// The subtree under `root` (children, grandchildren, …) with each row's
    /// depth (1 = direct child), in pre-order: a parent right before its
    /// children, siblings oldest first (design doc 19 §2.1); advisor rows
    /// (125 §4.2) hidden.
    pub async fn descendants(&self, root: SessionId) -> anyhow::Result<Vec<(u8, SessionSummary)>> {
        self.descendants_incl(root, false).await
    }

    /// [`Store::descendants`] with `kind = advisor` rows included when
    /// `include_advisors` (subtree deletion, the Advisors actor, cost).
    pub async fn descendants_incl(
        &self,
        root: SessionId,
        include_advisors: bool,
    ) -> anyhow::Result<Vec<(u8, SessionSummary)>> {
        #[derive(FromRow)]
        struct TreeRow {
            #[sqlx(flatten)]
            row: SessionRow,
            depth: i64,
        }
        self.stall().await;
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
            ", t.depth FROM sessions JOIN tree t ON sessions.id = t.id \
             WHERE (? OR sessions.kind != 'advisor') ORDER BY t.path"
        ))
        .bind(root.to_string())
        .bind(include_advisors)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    u8::try_from(r.depth).context("bad depth")?,
                    r.row.summary(&self.me)?,
                ))
            })
            .collect()
    }

    /// Delete `root` and every descendant with their messages, epochs, plans
    /// and communication accounting (exchanges, A2A contexts/tasks). Spend
    /// stays (51 §6.4, 160 H1): a true root tombstones its whole ledger tree
    /// and keeps its attribution, a subtree tombstones its owners.
    /// Returns the removed ids, children before parents.
    pub async fn delete_session(&self, root: SessionId) -> anyhow::Result<Vec<SessionId>> {
        let mut ids: Vec<SessionId> = self
            .descendants_incl(root, true)
            .await?
            .into_iter()
            .rev() // reverse pre-order: every child precedes its parent
            .map(|(_, s)| s.id)
            .collect();
        ids.push(root);
        let mut tx = self.begin_write().await?;
        let is_root: bool = sqlx::query_scalar("SELECT parent IS NULL FROM sessions WHERE id = ?")
            .bind(root.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(false);
        if is_root {
            accounting::tombstone_root_in(&mut tx, root).await?;
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
            sqlx::query("DELETE FROM inner_calls WHERE session_id = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            // 144 §5.1 (E1 hand-off): the dial rows of a deleted thread go with it (no FK on the table).
            sqlx::query("DELETE FROM session_context WHERE session = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            // 176 E2: the roster's cached last reply.
            sqlx::query("DELETE FROM last_texts WHERE session = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            // read-file-images: the session's remembered image refusals.
            sqlx::query("DELETE FROM image_refusals WHERE session = ?")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            durability::delete_journal_in(&mut tx, &sid).await?;
            endless::delete_endless_in(&mut tx, &sid).await?;
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
                .fetch_optional(&self.read)
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
                .fetch_optional(&self.read)
                .await?
        {
            id = parent.parse().context("bad parent id")?;
            depth = depth.saturating_add(1);
        }
        Ok(depth)
    }

    /// The parent chain of `id`, nearest first (empty for a top-level or
    /// unknown session), at most [`ROOT_HOPS`] long — the walk `root_of`
    /// runs, keeping every id (125 R1.5: the nearest advised ancestor).
    pub async fn ancestors(&self, mut id: SessionId) -> anyhow::Result<Vec<SessionId>> {
        let mut out = Vec::new();
        while out.len() < ROOT_HOPS {
            let Some((Some(parent),)) =
                sqlx::query_as::<_, (Option<String>,)>("SELECT parent FROM sessions WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(&self.read)
                    .await?
            else {
                break;
            };
            id = parent.parse().context("bad parent id")?;
            out.push(id);
        }
        Ok(out)
    }

    /// The top of `id`'s parent chain (`id` itself for a top-level or
    /// unknown session): the orchestrator tree's key (01 §2.6). The walk
    /// stops after [`ROOT_HOPS`] hops — a corrupt parent cycle warns once
    /// and yields the id reached, never a hang.
    pub async fn root_of(&self, mut id: SessionId) -> anyhow::Result<SessionId> {
        let start = id;
        for _ in 0..ROOT_HOPS {
            let Some((Some(parent),)) =
                sqlx::query_as::<_, (Option<String>,)>("SELECT parent FROM sessions WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(&self.read)
                    .await?
            else {
                return Ok(id);
            };
            id = parent.parse().context("bad parent id")?;
        }
        tracing::warn!(session = %start, "parent chain exceeds {ROOT_HOPS} hops; using {id} as the tree root");
        Ok(id)
    }

    /// Assistant text after the last user message, joined by newlines: the
    /// "final answer" of the most recent turn. Vision notes are not user
    /// messages here: a maintenance note after a reply does not erase it.
    /// 176 E2: cached in `last_texts` while the session's `context_revision`
    /// is unchanged, so an untouched session costs one indexed lookup; a
    /// miss reads only the last exchange and writes the cache back.
    pub async fn last_turn_text(&self, id: SessionId) -> anyhow::Result<String> {
        let sid = id.to_string();
        let row: Option<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT s.context_revision, c.rev, c.text FROM sessions s \
             LEFT JOIN last_texts c ON c.session = s.id WHERE s.id = ?",
        )
        .bind(&sid)
        .fetch_optional(&self.read)
        .await?;
        let Some((rev, cached_rev, cached)) = row else {
            return Ok(String::new());
        };
        if let Some(text) = cached.filter(|_| cached_rev == Some(rev)) {
            return Ok(text);
        }
        let text = self.last_exchange(id).await?.1;
        // Keyed to the revision read first: a write in between bumped it, so
        // the guard drops this (possibly newer) text instead of mislabelling it.
        if let Err(e) = sqlx::query(
            "INSERT INTO last_texts (session, rev, text) \
             SELECT ?1, ?2, ?3 FROM sessions WHERE id = ?1 AND context_revision = ?2 \
             ON CONFLICT(session) DO UPDATE SET rev = excluded.rev, text = excluded.text",
        )
        .bind(&sid)
        .bind(rev)
        .bind(&text)
        .execute(&self.write)
        .await
        {
            tracing::warn!(session = %id, "last_texts: {e:#}");
        }
        Ok(text)
    }

    /// 149 E6: the session's last exchange — what the user last typed (the
    /// clock stamp and attachment markers stripped; a markers-only input
    /// names its first attachment) and the assistant text after it as
    /// [`Self::last_turn_text`]. `(None, "")` for an empty session.
    pub async fn last_exchange(&self, id: SessionId) -> anyhow::Result<(Option<String>, String)> {
        let mut conn = self.read.acquire().await?;
        let (last_user, after) = last_exchange_in(&mut conn, id).await?;
        let request = last_user.and_then(|r| {
            let text = text_of(&r.message.content);
            let p = crate::attach::parse(crate::clock::strip(&text));
            let body = p.body.trim();
            if body.is_empty() {
                p.refs
                    .into_iter()
                    .map(|r| r.title)
                    .find(|t| !t.trim().is_empty())
            } else {
                Some(body.to_owned())
            }
        });
        let reply = after
            .iter()
            .filter(|r| r.message.role == ChatRole::Assistant)
            .map(|r| text_of(&r.message.content))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Ok((request, reply))
    }

    /// Set the project label on the root above `session` (51 §3: a child call
    /// mutates its root; `None` clears). The caller has normalized `project`.
    /// Returns the root id; unknown `session` is an error.
    pub async fn set_project(
        &self,
        session: SessionId,
        project: Option<&str>,
    ) -> anyhow::Result<SessionId> {
        let mut tx = self.begin_write().await?;
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
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        root.map(|(id,)| id.parse().context("bad session id"))
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("no such session"))
    }

    /// The project label in force for `session`: its root's value.
    pub async fn effective_project(&self, session: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> = sqlx::query_as(concat!(
            "SELECT ",
            root_col!("project"),
            " FROM sessions WHERE id = ?"
        ))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(|(p,)| p)
            .ok_or_else(|| anyhow::anyhow!("no such session"))
    }

    /// 144 §8: bind the root above `session` to a notebook (`None` unbinds); the same root rule as
    /// [`Store::set_project`]. Returns the root id; unknown `session` is an error.
    pub async fn set_notebook(
        &self,
        session: SessionId,
        notebook: Option<&str>,
    ) -> anyhow::Result<SessionId> {
        let mut tx = self.begin_write().await?;
        let root: Option<(String,)> = sqlx::query_as(
            "UPDATE sessions SET notebook = ? WHERE id = ( \
                 WITH RECURSIVE up(id, parent) AS ( \
                     SELECT id, parent FROM sessions WHERE id = ? \
                     UNION ALL \
                     SELECT p.id, p.parent FROM sessions p JOIN up ON p.id = up.parent) \
                 SELECT id FROM up WHERE parent IS NULL) \
             RETURNING id",
        )
        .bind(notebook)
        .bind(session.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        root.map(|(id,)| id.parse().context("bad session id"))
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("no such session"))
    }

    /// 144 §8: the notebook `session` is bound to — its root's value; `None` for a plain session.
    pub async fn effective_notebook(&self, session: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> = sqlx::query_as(concat!(
            "SELECT ",
            root_col!("notebook"),
            " FROM sessions WHERE id = ?"
        ))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row.and_then(|(n,)| n))
    }

    /// 144 §8: the root sessions bound to `notebook`, newest first (the "Chats · 3 ▾" menu).
    pub async fn notebook_chats(&self, notebook: &str) -> anyhow::Result<Vec<SessionSummary>> {
        let rows: Vec<SessionRow> = sqlx::query_as(concat!(
            "SELECT ",
            session_cols!(),
            " FROM sessions WHERE sessions.notebook = ? AND sessions.parent IS NULL \
             ORDER BY created_at DESC, id DESC"
        ))
        .bind(notebook)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(|r| r.summary(&self.me)).collect()
    }

    /// 144 §8 (R1): bound roots per notebook — `Notebook.chats`, the same rows [`Store::notebook_chats`] lists.
    pub async fn notebook_chat_counts(&self) -> anyhow::Result<Vec<(String, u32)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT notebook, COUNT(*) FROM sessions \
             WHERE notebook IS NOT NULL AND parent IS NULL GROUP BY notebook",
        )
        .fetch_all(&self.read)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(n, c)| (n, u32::try_from(c).unwrap_or(u32::MAX)))
            .collect())
    }

    /// 144 §5.1 (E1): pin `item`'s dial level for `session` (`session_context`, owner-only, never captured).
    pub async fn context_set(
        &self,
        session: SessionId,
        item: &str,
        kind: &str,
        level: &str,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO session_context (session, item, kind, level, hlc) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(session, item) DO UPDATE SET kind = excluded.kind, level = excluded.level, \
             hlc = excluded.hlc",
        )
        .bind(session.to_string())
        .bind(item)
        .bind(kind)
        .bind(level)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 144 §5.1 (E1): forget the pin — the item goes back to the default rule.
    pub async fn context_clear(&self, session: SessionId, item: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM session_context WHERE session = ? AND item = ?")
            .bind(session.to_string())
            .bind(item)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 144 §5.1 (E1): every pinned `(item, kind, level)` of `session`.
    pub async fn context_get(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        Ok(sqlx::query_as(
            "SELECT item, kind, level FROM session_context WHERE session = ? ORDER BY hlc, item",
        )
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?)
    }

    /// The style row of project `name`, if any (51 §3 as-built).
    pub async fn project_style(&self, name: &str) -> anyhow::Result<Option<ProjectStyle>> {
        Ok(
            sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
                "SELECT name, icon, color FROM projects WHERE name = ?",
            )
            .bind(name)
            .fetch_optional(&self.read)
            .await?
            .map(|(name, icon, color)| ProjectStyle { name, icon, color }),
        )
    }

    /// Every styled project, by name.
    pub async fn list_project_styles(&self) -> anyhow::Result<Vec<ProjectStyle>> {
        Ok(
            sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
                "SELECT name, icon, color FROM projects ORDER BY name",
            )
            .fetch_all(&self.read)
            .await?
            .into_iter()
            .map(|(name, icon, color)| ProjectStyle { name, icon, color })
            .collect(),
        )
    }

    /// Upsert the style of project `name` (the caller normalized the label);
    /// both `None` deletes the row. Unknown icon or colour keys are an error
    /// ([`project::validate_style`]).
    pub async fn set_project_style(
        &self,
        name: &str,
        icon: Option<&str>,
        color: Option<&str>,
    ) -> anyhow::Result<()> {
        crate::project::validate_style(icon, color).map_err(anyhow::Error::msg)?;
        if icon.is_none() && color.is_none() {
            let mut tx = self.begin_write().await?;
            sqlx::query("DELETE FROM projects WHERE name = ?")
                .bind(name)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(());
        }
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO projects (name, icon, color, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET icon = excluded.icon, color = excluded.color, \
             updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(icon)
        .bind(color)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record a pending reminder (14 as-built 2026-09-23).
    pub async fn insert_reminder(&self, r: &ReminderRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO reminders (id, session, message, set_at, fires_at, seconds) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&r.id)
        .bind(r.session.to_string())
        .bind(&r.message)
        .bind(r.set_at)
        .bind(r.fires_at)
        .bind(i64::try_from(r.seconds).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Forget a reminder that fired or was cancelled.
    pub async fn delete_reminder(&self, session: SessionId, id: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM reminders WHERE session = ? AND id = ?")
            .bind(session.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Every stored reminder, for re-arming after a restart.
    pub async fn reminders(&self) -> anyhow::Result<Vec<ReminderRow>> {
        Ok(sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
            "SELECT id, session, message, set_at, fires_at, seconds FROM reminders ORDER BY fires_at",
        )
        .fetch_all(&self.read)
        .await?
        .into_iter()
        .filter_map(|(id, session, message, set_at, fires_at, seconds)| {
            Some(ReminderRow {
                id,
                session: session.parse().ok()?,
                message,
                set_at,
                fires_at,
                seconds: u64::try_from(seconds).unwrap_or(0),
            })
        })
        .collect())
    }
}

/// 180 D3: [`accounting::repair_owners_in`] over the whole store in one
/// transaction, at open.
async fn repair_cost_owners(pool: &SqlitePool) -> anyhow::Result<accounting::OwnersRepaired> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let repaired = accounting::repair_owners_in(&mut tx).await?;
    tx.commit().await?;
    Ok(repaired)
}

/// Column layout of every attached table (`PRAGMA table_info`), in
/// [`capture::ATTACHED`] order; the session column must exist.
async fn table_meta(pool: &SqlitePool) -> anyhow::Result<Vec<capture::TableMeta>> {
    let mut out = Vec::with_capacity(capture::ATTACHED.len());
    for (table, session_col) in capture::ATTACHED {
        let rows: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
                .fetch_all(pool)
                .await?;
        let cols: Vec<String> = rows.iter().map(|r| r.1.clone()).collect();
        let pk: Vec<bool> = rows.iter().map(|r| r.5 > 0).collect();
        let session_col = cols
            .iter()
            .position(|c| c == session_col)
            .ok_or_else(|| anyhow::anyhow!("{table} has no column {session_col}"))?;
        out.push(capture::TableMeta {
            name: table.to_owned(),
            cols,
            pk,
            session_col,
        });
    }
    Ok(out)
}

/// The HLC origin of this engine's log rows: its id, or `local` before it
/// has one (such rows never ship; [`replica::Hlc::parse`] needs a non-empty origin).
fn hlc_origin(me: &str) -> replica::PeerId {
    replica::PeerId(if me.is_empty() {
        "local".to_owned()
    } else {
        me.to_owned()
    })
}

/// A user message that opens a turn: first in the session or right after an
/// assistant reply. Steers and notes follow Tool/User rows and never count.
/// Callers with vision notes in `rows` use [`turn_openers`], which skips them.
pub(crate) fn is_turn_start(prev: Option<&ChatMessage>, m: &ChatMessage) -> bool {
    m.role == ChatRole::User && prev.is_none_or(|p| p.role == ChatRole::Assistant)
}

/// Indices of turn starts among `(message, is_note)` rows, in order.
/// A typed note (vision, 52 §6; advisory, 125 §5.4) never opens a turn and
/// is skipped when locating the preceding role: a note appended after an
/// assistant reply is no phantom turn, and the real user message after it
/// still opens one.
fn openers<'a>(rows: impl Iterator<Item = (&'a ChatMessage, bool)>) -> Vec<usize> {
    let mut prev: Option<&ChatMessage> = None;
    let mut out = Vec::new();
    for (i, (m, note)) in rows.enumerate() {
        if note {
            continue;
        }
        if is_turn_start(prev, m) {
            out.push(i);
        }
        prev = Some(m);
    }
    out
}

/// Indices of turn starts within `rows`.
fn turn_starts(rows: &[Row]) -> Vec<usize> {
    openers(rows.iter().map(|r| (&r.message, r.is_note())))
}

/// Seqs opening a turn in raw `(seq, message)` rows, for readers outside the
/// store (history turn handles). `note(seq)` says whether the row is a typed
/// note: a vision note, an advisory or (183 §5.3) a moved record, as the
/// history renderer's closure checks from [`Store::context_history`] and the
/// advisory and moved maps.
pub(crate) fn turn_openers(rows: &[(i64, ChatMessage)], note: impl Fn(i64) -> bool) -> Vec<i64> {
    openers(rows.iter().map(|(seq, m)| (m, note(*seq))))
        .into_iter()
        .map(|i| rows[i].0)
        .collect()
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

/// One `messages` row as selected: `(seq, body, excluded, vision, at, thinking_ms, advisory, moved)`.
type RawRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// [`RawRow`]s to [`Row`]s: undecodable bodies are skipped (warned); an
/// undecodable mask is an error, because replaying a row the user removed is
/// the one wrong answer.
fn decode_masked(id: SessionId, rows: Vec<RawRow>) -> anyhow::Result<Vec<Row>> {
    let mut out = Vec::with_capacity(rows.len());
    for (seq, body, excluded, vision, at, thinking_ms, advisory, moved) in rows {
        let mask =
            Mask::decode(excluded.as_deref()).with_context(|| format!("message {seq} of {id}"))?;
        if let Some((seq, message)) = decode_rows(id, vec![(seq, body)]).pop() {
            out.push(Row {
                seq,
                message,
                mask,
                vision,
                at,
                thinking_ms,
                advisory,
                moved,
            });
        }
    }
    Ok(out)
}

/// Parse stored vision metadata; malformed JSON, an unknown version or a
/// non-binary source is a visible store error (52 §6: fail closed, never
/// treat the row as an ordinary message or another source as answered).
fn note_of(id: SessionId, seq: i64, raw: &str) -> anyhow::Result<VisionNote> {
    VisionNote::decode(raw).with_context(|| format!("bad vision metadata on message {seq} of {id}"))
}

impl Row {
    fn into_active(self) -> (i64, ChatMessage, Mask) {
        (self.seq, self.message, self.mask)
    }

    /// A typed note (vision consultation or advisory, 52 §6 / 125 §5.4):
    /// never opens a turn, transparent to turn readers.
    fn is_note(&self) -> bool {
        self.vision.is_some() || self.advisory.is_some() || self.moved.is_some()
    }
}

/// Insert one message at max+1, touch `updated_at` and bump the context
/// revision; the caller's transaction, so a failed write mints no seq (48 §3.1).
async fn append_in(
    conn: &mut SqliteConnection,
    sid: &str,
    message: &ChatMessage,
) -> anyhow::Result<i64> {
    insert_in(conn, sid, message, None, None).await
}

/// [`append_in`] with the row's typed vision metadata and thinking time
/// (`None` for every ordinary message).
async fn insert_in(
    conn: &mut SqliteConnection,
    sid: &str,
    message: &ChatMessage,
    vision: Option<&str>,
    thinking_ms: Option<i64>,
) -> anyhow::Result<i64> {
    let body = serde_json::to_string(message)?;
    let now = chrono::Utc::now().to_rfc3339();
    let (seq,): (i64,) = sqlx::query_as(
        "INSERT INTO messages (session_id, seq, body, vision, at, thinking_ms) \
         SELECT ?1, COALESCE(MAX(seq), -1) + 1, ?2, ?3, ?4, ?5 FROM messages WHERE session_id = ?1 \
         RETURNING seq",
    )
    .bind(sid)
    .bind(body)
    .bind(vision)
    .bind(&now)
    .bind(thinking_ms)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE sessions SET updated_at = ?, context_revision = context_revision + 1 WHERE id = ?",
    )
    .bind(&now)
    .bind(sid)
    .execute(conn)
    .await?;
    Ok(seq)
}

/// 172: set row `seq`'s `messages.channel`; nothing for `None`. The caller's
/// transaction, right after the row's insert.
async fn channel_in(
    conn: &mut SqliteConnection,
    sid: &str,
    seq: i64,
    channel: Option<&crate::ChannelOrigin>,
) -> anyhow::Result<()> {
    let Some(channel) = channel else {
        return Ok(());
    };
    sqlx::query("UPDATE messages SET channel = ? WHERE session_id = ? AND seq = ?")
        .bind(serde_json::to_string(channel)?)
        .bind(sid)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
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

/// Visible rows from `from` with their masks and raw vision/advisory
/// metadata, seq ascending (see [`decode_masked`] for the decoding rules).
async fn rows_in(
    conn: &mut SqliteConnection,
    id: SessionId,
    from: i64,
) -> anyhow::Result<Vec<Row>> {
    let rows = sqlx::query_as(
        "SELECT seq, body, excluded, vision, at, thinking_ms, advisory, moved FROM messages \
         WHERE session_id = ?1 AND seq >= ?2 \
         AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
         ORDER BY seq",
    )
    .bind(id.to_string())
    .bind(from)
    .fetch_all(conn)
    .await?;
    decode_masked(id, rows)
}

/// 176 E2: `id`'s last exchange read from the tail — its last user row (a
/// typed note is not one) and the visible rows after it, seq ascending; no
/// user row: `None` and every row. Pages of [`TAIL_PAGE`] rows seq
/// descending, so only the exchange (plus one page's slack) is decoded.
async fn last_exchange_in(
    conn: &mut SqliteConnection,
    id: SessionId,
) -> anyhow::Result<(Option<Row>, Vec<Row>)> {
    let mut after = Vec::new();
    let mut before = i64::MAX;
    loop {
        let raw: Vec<RawRow> = sqlx::query_as(
            "SELECT seq, body, excluded, vision, at, thinking_ms, advisory, moved FROM messages \
             WHERE session_id = ?1 AND seq < ?2 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
             ORDER BY seq DESC LIMIT ?3",
        )
        .bind(id.to_string())
        .bind(before)
        .bind(TAIL_PAGE)
        .fetch_all(&mut *conn)
        .await?;
        let done = raw.len() < TAIL_PAGE as usize;
        before = raw.last().map_or(before, |r| r.0);
        for row in decode_masked(id, raw)? {
            if row.message.role == ChatRole::User && !row.is_note() {
                after.reverse();
                return Ok((Some(row), after));
            }
            after.push(row);
        }
        if done {
            after.reverse();
            return Ok((None, after));
        }
    }
}

/// Rows per [`last_exchange_in`] page.
const TAIL_PAGE: i64 = 64;

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
    let (epoch, tail_from, at_seq, head) = match latest {
        Some((e, mask)) => (e.n, e.tail_from, e.at_seq, Some((e.head, mask))),
        None => (0, 0, -1, None),
    };
    Ok(Active {
        epoch,
        tail_from,
        at_seq,
        head,
        rows: rows_in(conn, id, tail_from)
            .await?
            .into_iter()
            .map(Row::into_active)
            .collect(),
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

    /// Fresh store in a temp dir with one session; the dir goes when the guard drops.
    async fn fresh() -> (Store, SessionId, crate::TestDir) {
        let scratch = crate::TestDir::new("store");
        let dir = scratch.0.clone();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        (store, id, scratch)
    }

    /// 172: a channel input's origin is stored with its row and replayed on
    /// the history entry; a row stored before 172 (no `channel`) reads `None`,
    /// and a fork keeps the origin. The wire shape is the bindings contract:
    /// lowercase enums, `from`/`title`/`channel` absent when unset, and old
    /// JSON without the field still parses.
    #[tokio::test]
    async fn channel_origin_round_trips_and_old_rows_read_none() {
        use crate::{ChannelChat, ChannelOrigin, ChannelVia, HistoryEntry};
        let (store, id, _dir) = fresh().await;
        // A pre-172 row, written the way the old schema did.
        sqlx::query("INSERT INTO messages (session_id, seq, body) VALUES (?, 0, ?)")
            .bind(id.to_string())
            .bind(serde_json::to_string(&ChatMessage::user("old")).unwrap())
            .execute(&store.write)
            .await
            .unwrap();
        let origin = ChannelOrigin {
            via: ChannelVia::Slack,
            chat: ChannelChat::Dm,
            from: None,
            title: None,
        };
        let turn = runtime::TurnId::new();
        let msg = ChatMessage::user("[slack dm]\nhi");
        let seq = store
            .append_input(id, turn, &msg, None, "user", Some(&origin))
            .await
            .unwrap();
        assert_eq!(seq, 1);
        assert_eq!(
            store.channels(id).await.unwrap(),
            BTreeMap::from([(1, origin.clone())])
        );

        let history = |store: Store, id: SessionId| async move {
            let h = store.context_history(id).await.unwrap();
            let mut out = crate::supervisor::render_with(
                &h.messages,
                &h.entries,
                &h.thinking,
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
            crate::supervisor::mark_channels(&mut out, &store.channels(id).await.unwrap());
            out.into_iter()
                .filter_map(|(_, e)| match e {
                    HistoryEntry::User { text, channel, .. } => Some((text, channel)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let want = vec![
            ("old".to_owned(), None),
            ("[slack dm]\nhi".to_owned(), Some(origin.clone())),
        ];
        assert_eq!(history(store.clone(), id).await, want);
        let fork = store.fork(id, None, None).await.unwrap();
        assert_eq!(history(store.clone(), fork).await, want);

        // Wire: what the TS bindings promise the UI.
        let entry = |channel| HistoryEntry::User {
            text: "hi".into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
            channel,
        };
        assert_eq!(
            serde_json::to_value(entry(Some(origin.clone()))).unwrap()["channel"],
            serde_json::json!({ "via": "slack", "chat": "dm" })
        );
        let full = ChannelOrigin {
            via: ChannelVia::Telegram,
            chat: ChannelChat::Group,
            from: Some("@ops".into()),
            title: Some("Ops".into()),
        };
        assert_eq!(
            serde_json::to_value(&full).unwrap(),
            serde_json::json!({ "via": "telegram", "chat": "group", "from": "@ops", "title": "Ops" })
        );
        assert!(
            serde_json::to_value(entry(None))
                .unwrap()
                .get("channel")
                .is_none()
        );
        let old: HistoryEntry =
            serde_json::from_value(serde_json::json!({ "role": "user", "text": "hi" })).unwrap();
        assert!(matches!(old, HistoryEntry::User { channel: None, .. }));
        let old: crate::SendInput =
            serde_json::from_value(serde_json::json!({ "session": null, "text": "hi" })).unwrap();
        assert_eq!(old.channel, None);
    }

    /// The writer commits with `synchronous=NORMAL` (1): WAL mode keeps a
    /// committed transaction durable against an engine kill without an
    /// fsync per commit. The readers never fsync, so only the writer counts.
    #[tokio::test]
    async fn write_connection_is_synchronous_normal() {
        let (store, _, _dir) = fresh().await;
        let (sync,): (i64,) = sqlx::query_as("PRAGMA synchronous")
            .fetch_one(&store.write)
            .await
            .unwrap();
        assert_eq!(sync, 1, "PRAGMA synchronous on the write connection");
        drop(store);
    }

    async fn append_all(store: &Store, id: SessionId, msgs: impl IntoIterator<Item = ChatMessage>) {
        for m in msgs {
            store.append(id, &m).await.unwrap();
        }
    }

    /// `root_of` walks to the top of a chain and, on a corrupt parent cycle,
    /// stops after `ROOT_HOPS` with the id it reached instead of hanging.
    #[tokio::test]
    async fn root_of_tops_the_chain_and_survives_a_cycle() {
        let (store, root, _dir) = fresh().await;
        let kid = SessionId::new();
        let grandkid = SessionId::new();
        store
            .create_session(kid, "m", Some(root), Some("kid"), None)
            .await
            .unwrap();
        store
            .create_session(grandkid, "m", Some(kid), Some("grandkid"), None)
            .await
            .unwrap();
        assert_eq!(store.root_of(grandkid).await.unwrap(), root);
        assert_eq!(store.root_of(root).await.unwrap(), root);
        let stranger = SessionId::new();
        assert_eq!(
            store.root_of(stranger).await.unwrap(),
            stranger,
            "unknown: itself"
        );
        // root → grandkid closes a cycle.
        let mut tx = store.begin_write().await.unwrap();
        sqlx::query("UPDATE sessions SET parent = ? WHERE id = ?")
            .bind(grandkid.to_string())
            .bind(root.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let reached = tokio::time::timeout(std::time::Duration::from_secs(10), store.root_of(kid))
            .await
            .expect("bounded")
            .unwrap();
        assert!([root, kid, grandkid].contains(&reached));
    }

    fn texts(rows: &[(i64, ChatMessage)]) -> Vec<(i64, String)> {
        rows.iter()
            .map(|(s, m)| (*s, text_of(&m.content)))
            .collect()
    }

    fn asst(t: &str) -> ChatMessage {
        ChatMessage::assistant(MessageContent::from_text(t))
    }

    /// 74 §5: every appended row records when, readable as `ContextEntry.at`;
    /// a row from before the column (simulated by clearing it) has none.
    #[tokio::test]
    async fn message_time_is_stored_and_exposed() {
        let (store, id, _dir) = fresh().await;
        let before = chrono::Utc::now();
        append_all(&store, id, [ChatMessage::user("hi"), asst("hello")]).await;
        sqlx::query("UPDATE messages SET at = NULL WHERE session_id = ? AND seq = 0")
            .bind(id.to_string())
            .execute(&store.read)
            .await
            .unwrap();
        let h = store.context_history(id).await.unwrap();
        assert_eq!(h.entries[&0].at, None);
        let at = chrono::DateTime::parse_from_rfc3339(h.entries[&1].at.as_deref().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(at >= before - chrono::Duration::seconds(1) && at <= chrono::Utc::now());
        drop(store);
    }

    /// 91: the migration adds `messages.thinking_ms`; `append_thinking` writes
    /// it on the reply row and `context_history` exposes it by seq, while
    /// plain appends and `None` leave it NULL (absent from the map).
    #[tokio::test]
    async fn thinking_time_round_trips() {
        let (store, id, _dir) = fresh().await;
        let cols: Vec<(i64, String)> =
            sqlx::query_as("SELECT cid, name FROM pragma_table_info('messages')")
                .fetch_all(&store.read)
                .await
                .unwrap();
        assert!(cols.iter().any(|(_, n)| n == "thinking_ms"), "{cols:?}");
        store.append(id, &ChatMessage::user("hi")).await.unwrap();
        store
            .append_thinking(id, &asst("hello"), Some(9_400))
            .await
            .unwrap();
        store
            .append_thinking(id, &asst("again"), None)
            .await
            .unwrap();
        let h = store.context_history(id).await.unwrap();
        assert_eq!(h.thinking, BTreeMap::from([(1, 9_400)]));
        assert_eq!(h.messages.len(), 3);
        drop(store);
    }

    /// 183 R2 (item 3): [`SESSIONS_WIRE_SCHEMA`] names the columns of the tables the session
    /// log replicates (`capture::ATTACHED`). A migration that adds, drops or renames one of
    /// them fails this until the constant is bumped to that migration and the snapshot is
    /// taken again, in the same change (the bump rule on the constant).
    #[tokio::test]
    async fn sessions_wire_schema_tracks_the_replicated_columns() {
        const SNAPSHOT: (u32, &str) = (
            63,
            "sessions: id,created_at,title,model,parent,name,effort,cut_at,agent,acp_session,cwd,updated_at,pinned,todo_version,context_revision,scope,alias,project,archived,tags,harness_model,unattended,owner,epoch,desk,harness_commands,context_size,acp_skills,last_stop,finished_at,kind,memory,endless_base,endless_filter,endless_view_bytes,budget,observed_source,observed_state,observed_stop,observed_cancel,observed_description,observed_tool_call,observed_detail,observed_cursor,observed_cost_rule,notebook,acp_previous,endless_paused,endless_spend_override,endless_model,endless_agent_model,approvals,communication,contact_inbox,hidden_tools,endless_effort,endless_agent_effort
messages: session_id,seq,body,excluded,vision,at,thinking_ms,advisory,channel,moved
epochs: session_id,n,at_seq,tail_from,head,created_at,excluded
todo_snapshots: session_id,at_seq,k,source,items
errors: id,session_id,at_seq,text
inner_calls: session_id,parent_id,n,at_seq,name,args,output,is_error,files,at
reminders: id,session,message,set_at,fires_at,seconds
model_calls: call_id,owner_id,request_id,attempt,turn_id,kind,status,provider,requested_model,served_model,provider_request_id,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,context_tokens,returned_nanos,returned_currency,returned_source,estimated_nanos,estimated_currency,estimate_source,unknown_reason,coverage,source_metadata,created_at,dispatched_at,settled_at
remote_meters: id,owner_id,origin,external_session,epoch,source,currency,baseline_nanos,high_water_nanos,attributed_nanos,baseline_kind,scope,coverage,coverage_boundary,last_report_id,source_metadata,updated_at
turn_steps: session_id,turn_id,n,kind,at_seq,call_id,payload,at
tool_calls: session_id,call_id,turn_id,at_seq,name,args,risk,state,attempt,started_at,finished_at,output,is_error,files
questions: id,session_id,tool_call_id,kind,payload,state,asked_at,deadline,answered_at,answer,answered_by
waits: session_id,tool_call_id,kind,target,deadline,payload
jobs: session_id,id,kind,call_id,command,name,pgid,log,started_ms,state,exit,settled_ms,engine
pty_screens: session_id,job_id,revision,rows,cols,cursor_row,cursor_col,text,at_ms
resume_reports: session_id,turn_id,kind,interrupted_at,at,replayed,interrupted,lost_jobs,children_restored,children_handed_off,cause
session_advisors: session,advisor,model,effort,cursor,state,updated_at
endless_units: session_id,i,seq,part,kind,size,turn
endless_nodes: session_id,l,i,text,size,free,model,tries,created_at,mechanical
endless_calls: call_id,session_id,l,i,try,bytes,guard,usage_json,usd_estimate,latency_ms,created_at",
        );
        let (store, _id, _dir) = fresh().await;
        let mut tables = Vec::new();
        for (table, _) in capture::ATTACHED {
            let cols: Vec<String> =
                sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                    .bind(table)
                    .fetch_all(&store.read)
                    .await
                    .unwrap();
            tables.push(format!("{table}: {}", cols.join(",")));
        }
        let got = tables.join("\n");
        assert_eq!(
            (SESSIONS_WIRE_SCHEMA, got.as_str()),
            SNAPSHOT,
            "a replicated session table changed its columns: bump SESSIONS_WIRE_SCHEMA to the \
             migration that did it and take the snapshot again"
        );
    }

    /// 183 §9.1: 0053's per-session settings start NULL (inherit) and accept
    /// only the tri-state values; `messages.moved` starts NULL.
    #[tokio::test]
    async fn remote_session_columns_default_null_and_check_values() {
        let (store, id, _dir) = fresh().await;
        store.append(id, &ChatMessage::user("hi")).await.unwrap();
        let row: (Option<String>, Option<i64>) =
            sqlx::query_as("SELECT approvals, communication FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(row, (None, None));
        let moved: Option<String> =
            sqlx::query_scalar("SELECT moved FROM messages WHERE session_id = ?")
                .bind(id.to_string())
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(moved, None);
        let set = |sql: &'static str, v: &'static str| {
            sqlx::query(sql)
                .bind(v)
                .bind(id.to_string())
                .execute(&store.read)
        };
        let approvals = "UPDATE sessions SET approvals = ? WHERE id = ?";
        let communication = "UPDATE sessions SET communication = ? WHERE id = ?";
        assert!(set(approvals, "on").await.is_ok());
        assert!(set(approvals, "off").await.is_ok());
        assert!(set(approvals, "maybe").await.is_err());
        assert!(set(communication, "0").await.is_ok());
        assert!(set(communication, "2").await.is_err());
        drop(store);
    }

    // ----- 141 §11.3: watch-only children -----

    #[tokio::test]
    async fn observed_upsert_is_idempotent_per_parent_source_external() {
        let (store, root, _dir) = fresh().await;
        let acp = SessionId::new();
        store
            .create_session(acp, "m", Some(root), Some("omp"), None)
            .await
            .unwrap();
        store.set_harness_model(acp, "anthropic/x").await.unwrap();

        let mut first = ObservedUpsert::new(acp, ObservedSource::OmpTask, "ReadA");
        first.name = Some("ReadA");
        first.state = Some(ObservedState::Running);
        first.tool_call = Some("toolu_1");
        let detail = serde_json::json!({"tokens": 10, "agent": "scout"});
        first.detail = detail.as_object();
        let (child, created) = store.upsert_observed(first).await.unwrap();
        assert!(created);

        let row = store.session(child).await.unwrap().unwrap();
        assert_eq!(row.kind, SessionKind::Observed);
        assert_eq!(row.parent, Some(acp));
        assert_eq!(row.agent, None, "nothing drives an observed row");
        assert_eq!(
            row.model, "anthropic/x",
            "parent's harness model is the fallback"
        );
        assert_eq!(row.name.as_deref(), Some("ReadA"));
        assert_eq!(row.title, "ReadA");
        let m = row.observed.unwrap();
        assert_eq!(m.source, ObservedSource::OmpTask);
        assert_eq!(m.external, "ReadA");
        assert_eq!(m.state, ObservedState::Running);
        assert_eq!(m.cost_rule, CostRule::Counted, "omp default");
        assert_eq!(m.tool_call.as_deref(), Some("toolu_1"));
        assert_eq!(m.detail, detail);
        assert_eq!(
            store.acp_session(child).await.unwrap().as_deref(),
            Some("ReadA")
        );

        // Same triple: the row is updated in place; `None` leaves fields alone,
        // `detail` merges shallowly, `stop: Some(None)` clears.
        let mut again = ObservedUpsert::new(acp, ObservedSource::OmpTask, "ReadA");
        again.state = Some(ObservedState::Failed);
        again.stop = Some(Some("boom"));
        again.model = Some("anthropic/claude-x");
        again.cost_rule = Some(CostRule::InsideParent);
        let patch = serde_json::json!({"tokens": 99, "cost": 0.5});
        again.detail = patch.as_object();
        let (same, created) = store.upsert_observed(again).await.unwrap();
        assert_eq!(same, child);
        assert!(!created);
        let row = store.session(child).await.unwrap().unwrap();
        assert_eq!(row.harness_model.as_deref(), Some("anthropic/claude-x"));
        let m = row.observed.unwrap();
        assert_eq!(m.state, ObservedState::Failed);
        assert_eq!(m.stop.as_deref(), Some("boom"));
        assert_eq!(m.cost_rule, CostRule::InsideParent);
        assert_eq!(m.tool_call.as_deref(), Some("toolu_1"));
        assert_eq!(
            m.detail,
            serde_json::json!({"tokens": 99, "agent": "scout", "cost": 0.5})
        );
        let mut clear = ObservedUpsert::new(acp, ObservedSource::OmpTask, "ReadA");
        clear.stop = Some(None);
        store.upsert_observed(clear).await.unwrap();
        let m = store
            .session(child)
            .await
            .unwrap()
            .unwrap()
            .observed
            .unwrap();
        assert_eq!(m.stop, None);
        assert_eq!(m.state, ObservedState::Failed);
        assert_eq!(
            store
                .observed_child(acp, ObservedSource::OmpTask, "ReadA")
                .await
                .unwrap(),
            Some(child)
        );

        // A different source with the same external id is another row.
        let (other, created) = store
            .upsert_observed(ObservedUpsert::new(acp, ObservedSource::Rfd, "ReadA"))
            .await
            .unwrap();
        assert!(created);
        assert_ne!(other, child);
        let m = store
            .session(other)
            .await
            .unwrap()
            .unwrap()
            .observed
            .unwrap();
        assert_eq!(
            m.state,
            ObservedState::Pending,
            "created rows start pending"
        );
        assert_eq!(m.cost_rule, CostRule::DisplayOnly, "rfd default");
        assert_eq!(m.detail, serde_json::Value::Null);
        assert_eq!(
            store
                .observed_child(acp, ObservedSource::OpencodeExt, "ReadA")
                .await
                .unwrap(),
            None
        );

        // A parent without a row is an error, not a silent orphan.
        assert!(
            store
                .upsert_observed(ObservedUpsert::new(
                    SessionId::new(),
                    ObservedSource::Rfd,
                    "x"
                ))
                .await
                .is_err()
        );
        drop(store);
    }

    #[tokio::test]
    async fn observed_children_lists_under_the_acp_child_in_order() {
        let (store, root, _dir) = fresh().await;
        let acp = SessionId::new();
        store
            .create_session(acp, "m", Some(root), Some("omp"), None)
            .await
            .unwrap();
        let (a, _) = store
            .upsert_observed(ObservedUpsert::new(acp, ObservedSource::OmpTask, "a"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        // Nested: `a.b` hangs under `a`, not under the ACP child.
        let (ab, _) = store
            .upsert_observed(ObservedUpsert::new(a, ObservedSource::OmpTask, "a.b"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        let (b, _) = store
            .upsert_observed(ObservedUpsert::new(acp, ObservedSource::OmpTask, "b"))
            .await
            .unwrap();

        let direct: Vec<(SessionId, String)> = store
            .observed_children(acp)
            .await
            .unwrap()
            .into_iter()
            .map(|(s, m)| (s.id, m.external))
            .collect();
        assert_eq!(direct, [(a, "a".into()), (b, "b".into())]);
        assert_eq!(
            store
                .observed_children(a)
                .await
                .unwrap()
                .into_iter()
                .map(|(s, _)| s.id)
                .collect::<Vec<_>>(),
            [ab]
        );
        assert!(store.observed_children(root).await.unwrap().is_empty());

        // `descendants` is pre-order with the observed grandchild under `a`,
        // and observed rows are visible to the plain readers (not advisors).
        let tree: Vec<(u8, SessionId)> = store
            .descendants(root)
            .await
            .unwrap()
            .iter()
            .map(|(d, s)| (*d, s.id))
            .collect();
        assert_eq!(tree, [(1, acp), (2, a), (3, ab), (2, b)]);
        let listed = store.list_sessions().await.unwrap();
        let obs = listed.iter().find(|s| s.id == ab).unwrap();
        assert_eq!(obs.kind, SessionKind::Observed);
        assert_eq!(obs.parent, Some(a));
        assert!(obs.observed.is_some());
        drop(store);
    }

    #[test]
    fn kind_of_observed_round_trips() {
        assert_eq!(advisor::kind_str(SessionKind::Observed), "observed");
        assert_eq!(advisor::kind_of("observed"), SessionKind::Observed);
        assert_eq!(advisor::kind_of("advisor"), SessionKind::Advisor);
        assert_eq!(advisor::kind_of("chat"), SessionKind::Chat);
        assert_eq!(advisor::kind_of("whatever"), SessionKind::Chat);
        // The stored strings of the observed enums are their serde names.
        assert_eq!("requires_action".parse(), Ok(ObservedState::RequiresAction));
        assert_eq!(ObservedState::RequiresAction.as_str(), "requires_action");
        assert_eq!("opencode_ext".parse(), Ok(ObservedSource::OpencodeExt));
        assert_eq!("inside_parent".parse(), Ok(CostRule::InsideParent));
        assert!(ObservedState::Cancelled.is_terminal());
        assert!(!ObservedState::Unknown.is_terminal());
    }

    /// 144 §9: `ask` is a stored kind; `set_kind` flips a chat root into it and the row reads back as such.
    #[tokio::test]
    async fn ask_kind_round_trips_through_the_row() {
        assert_eq!(advisor::kind_str(SessionKind::Ask), "ask");
        assert_eq!(advisor::kind_of("ask"), SessionKind::Ask);
        assert_eq!(serde_json::to_string(&SessionKind::Ask).unwrap(), "\"ask\"");
        let (store, id, _dir) = fresh().await;
        assert_eq!(
            store.session(id).await.unwrap().unwrap().kind,
            SessionKind::Chat
        );
        store.set_kind(id, SessionKind::Ask).await.unwrap();
        assert_eq!(
            store.session(id).await.unwrap().unwrap().kind,
            SessionKind::Ask
        );
        // Listed like a chat (only advisors are hidden).
        assert!(
            store
                .list_sessions()
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == id)
        );
    }

    #[tokio::test]
    async fn mark_observed_lost_touches_non_terminal_rows_only() {
        let (owned, root, _dir) = fresh().await;
        let store = &owned;
        let live = SessionId::new();
        let gone = SessionId::new();
        for (id, name) in [(live, "live"), (gone, "gone")] {
            store
                .create_session(id, "m", Some(root), Some(name), None)
                .await
                .unwrap();
        }
        let mk = |parent: SessionId, ext: &'static str, state: ObservedState| async move {
            let mut u = ObservedUpsert::new(parent, ObservedSource::Rfd, ext);
            u.state = Some(state);
            u.stop = Some(Some("was"));
            store.upsert_observed(u).await.unwrap().0
        };
        let live_running = mk(live, "r", ObservedState::Running).await;
        let gone_running = mk(gone, "r", ObservedState::Running).await;
        let gone_idle = mk(gone, "i", ObservedState::Idle).await;
        let gone_done = mk(gone, "d", ObservedState::Completed).await;
        let gone_unknown = mk(gone, "u", ObservedState::Unknown).await;

        let lost: std::collections::HashSet<SessionId> = store
            .mark_observed_lost(&[live])
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            lost,
            std::collections::HashSet::from([gone_running, gone_idle])
        );
        let meta = |id| async move { store.session(id).await.unwrap().unwrap().observed.unwrap() };
        assert_eq!(meta(live_running).await.state, ObservedState::Running);
        let m = meta(gone_running).await;
        assert_eq!(
            (m.state, m.stop.as_deref()),
            (ObservedState::Unknown, Some("lost contact"))
        );
        assert_eq!(meta(gone_idle).await.state, ObservedState::Unknown);
        let m = meta(gone_done).await;
        assert_eq!(
            (m.state, m.stop.as_deref()),
            (ObservedState::Completed, Some("was"))
        );
        assert_eq!(meta(gone_unknown).await.stop.as_deref(), Some("was"));
        // A second sweep finds nothing new.
        let again = store.mark_observed_lost(&[live]).await.unwrap();
        assert!(again.is_empty());
        drop(owned);
    }

    #[tokio::test]
    async fn observed_cursor_round_trip() {
        let (store, root, _dir) = fresh().await;
        let acp = SessionId::new();
        store
            .create_session(acp, "m", Some(root), Some("omp"), None)
            .await
            .unwrap();
        let (child, _) = store
            .upsert_observed(ObservedUpsert::new(acp, ObservedSource::OmpTask, "ReadA"))
            .await
            .unwrap();
        assert_eq!(store.observed_cursor(child).await.unwrap(), None);
        store.set_observed_cursor(child, 4_096).await.unwrap();
        assert_eq!(store.observed_cursor(child).await.unwrap(), Some(4_096));
        store.set_observed_cursor(child, 0).await.unwrap();
        assert_eq!(store.observed_cursor(child).await.unwrap(), Some(0));
        assert_eq!(store.observed_cursor(acp).await.unwrap(), None);
        drop(store);
    }

    #[tokio::test]
    async fn migration_0041_adds_the_observed_indexes() {
        let store = Store::open_memory().await.unwrap();
        let names: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'sessions' \
             AND name IN ('sessions_parent', 'sessions_observed_identity') ORDER BY name",
        )
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(
            names,
            [
                ("sessions_observed_identity".to_owned(),),
                ("sessions_parent".to_owned(),)
            ]
        );
    }

    #[tokio::test]
    async fn resume_round_trip() {
        let (store, id, _dir) = fresh().await;
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
        store
            .create_session(child, "m", Some(id), Some("kid"), None)
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
        // Archiving the root marks its whole tree (the child resolves the
        // root's flag); the order and the rows themselves are untouched.
        store.set_archived(id, true).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(
            list.iter().map(|s| (s.id, s.archived)).collect::<Vec<_>>(),
            [(id, true), (child, true)]
        );
        assert_eq!(
            store
                .search_titles()
                .await
                .unwrap()
                .iter()
                .map(|(s, _, _, _, a)| (*s, *a))
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from([(id, true), (child, true)])
        );
        store.set_archived(id, false).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert!(list.iter().all(|s| !s.archived));
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
    }

    #[tokio::test]
    async fn descendants_is_preorder_with_depth() {
        let (store, root, _dir) = fresh().await;
        let a = SessionId::new();
        let b = SessionId::new();
        let a1 = SessionId::new();
        let other = SessionId::new();
        store
            .create_session(a, "m", Some(root), Some("a"), None)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        store
            .create_session(a1, "m", Some(a), Some("a1"), None)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        store
            .create_session(b, "m", Some(root), Some("b"), None)
            .await
            .unwrap();
        store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();

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
    }

    #[tokio::test]
    async fn acp_columns_round_trip() {
        let (store, id, _dir) = fresh().await;
        let row = store.session(id).await.unwrap().unwrap();
        assert_eq!((row.agent, row.cwd, row.harness_model), (None, None, None));
        assert_eq!(store.acp_session(id).await.unwrap(), None);

        store
            .set_acp(id, "codex", Some(Path::new("/tmp/w")), &[])
            .await
            .unwrap();
        store.set_acp_session(id, "s-1").await.unwrap();
        store
            .set_harness_model(id, "anthropic/claude-opus-4-8")
            .await
            .unwrap();
        let row = store.session(id).await.unwrap().unwrap();
        assert_eq!(row.agent.as_deref(), Some("codex"));
        assert_eq!(row.cwd.as_deref(), Some("/tmp/w"));
        assert_eq!(
            row.harness_model.as_deref(),
            Some("anthropic/claude-opus-4-8")
        );
        assert_eq!(store.acp_session(id).await.unwrap().as_deref(), Some("s-1"));
        assert_eq!(
            store.list_sessions().await.unwrap()[0].agent.as_deref(),
            Some("codex")
        );

        // `set_acp` without a cwd keeps the recorded one; `set_cwd` replaces it.
        store.set_acp(id, "codex", None, &[]).await.unwrap();
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
    }

    #[tokio::test]
    async fn last_turn_text_joins_assistant_text_after_last_user() {
        let (store, id, _dir) = fresh().await;
        for m in [
            ChatMessage::user("earlier"),
            ChatMessage::assistant(MessageContent::from_text("stale")),
            ChatMessage::user(
                "<time now=\"2026-01-01 00:00 +00:00\"/>\n[attached note n1 \"Groceries\"]\ntask",
            ),
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
        let (request, reply) = store.last_exchange(id).await.unwrap();
        assert_eq!(
            (request.as_deref(), reply.as_str()),
            (Some("task"), "first\nsecond")
        );
        drop(store);
    }

    /// The pre-176 `last_turn_text`: every visible row replayed. E2's tail
    /// read and cache must answer exactly this.
    async fn replayed_last_text(store: &Store, id: SessionId) -> String {
        let mut conn = store.read.acquire().await.unwrap();
        let rows = rows_in(&mut conn, id, 0).await.unwrap();
        let last_user = rows
            .iter()
            .rposition(|r| r.message.role == ChatRole::User && !r.is_note());
        rows[last_user.map_or(0, |i| i + 1)..]
            .iter()
            .filter(|r| r.message.role == ChatRole::Assistant)
            .map(|r| text_of(&r.message.content))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 176 E2: the tail read and its revision-keyed cache equal the full
    /// replay across page boundaries, tool rounds, an empty final reply, a
    /// user-less transcript, undo/redo and a committed cut that reuses seqs.
    #[tokio::test]
    async fn last_turn_text_tail_and_cache_equal_the_replay() {
        let (store, id, _dir) = fresh().await;
        let call = |c: &str| {
            ChatMessage::from(vec![ToolCall {
                call_id: c.into(),
                fn_name: "shell".into(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }])
        };
        let text = |t: &str| ChatMessage::assistant(MessageContent::from_text(t));
        let check = |what: &'static str| {
            let store = store.clone();
            async move {
                let want = replayed_last_text(&store, id).await;
                assert_eq!(
                    store.last_turn_text(id).await.unwrap(),
                    want,
                    "{what}: miss"
                );
                assert_eq!(store.last_turn_text(id).await.unwrap(), want, "{what}: hit");
                want
            }
        };
        assert_eq!(check("empty").await, "");
        // No user row yet: every assistant text counts.
        store.append(id, &text("hello")).await.unwrap();
        assert_eq!(check("user-less").await, "hello");
        // One exchange of 3 × 40 rows: spans pages of the tail read.
        store.append(id, &ChatMessage::user("q1")).await.unwrap();
        for i in 0..40 {
            let c = format!("c{i}");
            store.append(id, &text(&format!("step {i}"))).await.unwrap();
            store.append(id, &call(&c)).await.unwrap();
            let resp = ChatMessage::from(vec![ToolResponse::new(&c, "out")]);
            store.append(id, &resp).await.unwrap();
        }
        store.append(id, &text("done")).await.unwrap();
        let long = check("tool rounds").await;
        assert!(long.starts_with("step 0\nstep 1\n") && long.ends_with("step 39\ndone"));
        // An exchange whose reply is only a tool round: empty.
        store.append(id, &ChatMessage::user("q2")).await.unwrap();
        store.append(id, &call("z")).await.unwrap();
        let resp = ChatMessage::from(vec![ToolResponse::new("z", "out")]);
        store.append(id, &resp).await.unwrap();
        assert_eq!(check("empty final reply").await, "");
        // Undo hides q2 and redo restores it; each move bumps the revision.
        store.undo(id).await.unwrap().unwrap();
        assert_eq!(check("undo").await, long);
        store.redo(id).await.unwrap().unwrap();
        assert_eq!(check("redo").await, "");
        // Undo + commit, then the same seqs (same MAX(seq) as the cached ""
        // of "redo") carry another reply: only the revision tells them apart.
        store.undo(id).await.unwrap().unwrap();
        store.commit_cut(id).await.unwrap();
        store.append(id, &ChatMessage::user("q3")).await.unwrap();
        store.append(id, &text("other")).await.unwrap();
        store.append(id, &resp).await.unwrap();
        assert_eq!(check("reused seqs").await, "other");
        // An unknown session answers "" and caches nothing.
        assert_eq!(store.last_turn_text(SessionId::new()).await.unwrap(), "");
        drop(store);
    }

    #[tokio::test]
    async fn compact_frames_from_tail() {
        let (store, id, _dir) = fresh().await;
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
    }

    /// 70 COMPACT-SIZE: a tool result that was in the kept tail at the cut
    /// comes back folded from `frame` (as the actor folded it in memory); one
    /// appended after the cut is whole, and so is the raw row.
    #[tokio::test]
    async fn frame_folds_oversize_tool_results_in_the_compaction_tail() {
        let (store, id, _dir) = fresh().await;
        let big = "x".repeat(compact::FOLD_CHARS * 3);
        let tool = |c: &str| ChatMessage::from(vec![ToolResponse::new(c, big.clone())]);
        store.append(id, &ChatMessage::user("u0")).await.unwrap();
        store.append(id, &ChatMessage::user("u1")).await.unwrap(); // seq 1: tail_from
        store.append(id, &tool("c1")).await.unwrap(); // seq 2: in the tail at the cut
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store.compact(id, 1, 2, 1, &head).await.unwrap();
        store.append(id, &tool("c2")).await.unwrap(); // seq 3: after the cut
        let (_, _, tail) = store.frame(id).await.unwrap();
        let texts: Vec<(i64, String)> = tail
            .iter()
            .map(|(s, m)| {
                let body = m
                    .content
                    .tool_responses()
                    .first()
                    .map_or("", |r| r.content.as_str());
                (*s, body.to_owned())
            })
            .collect();
        assert_eq!(texts[0], (1, String::new()));
        assert_eq!(texts[1].0, 2);
        assert!(
            texts[1].1.contains("chars folded — read the archive]") && texts[1].1.len() < big.len(),
            "{}",
            texts[1].1.len()
        );
        assert_eq!(texts[2], (3, big.clone()));
        // The stored row is untouched.
        let raw = store.messages(id).await.unwrap();
        assert_eq!(raw[2].1.content.tool_responses()[0].content, big);
        drop(store);
    }

    /// 13 §6.5: a multi-part snap head (caption, PNG frames, closing tag)
    /// survives the epochs row byte for byte.
    #[tokio::test]
    async fn snap_head_round_trips_parts() {
        use base64::Engine as _;
        use genai::chat::ContentPart;

        use crate::{compact, snap};

        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn undo_redo_hides_whole_turns_including_steers() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn undo_hides_epochs_inside_the_turn() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn commit_cut_deletes_hidden_tail() {
        let (store, id, _dir) = fresh().await;
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
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
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
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(left.0, 0);
        drop(store);
    }

    /// 08 as-built: inner calls of a `python` cell round-trip by parent in `n`
    /// order, a start row without its finish reads back unfinished, and the
    /// rows follow their turn: kept through a whole fork, dropped by a
    /// committed undo and by session deletion.
    #[tokio::test]
    async fn inner_calls_follow_their_turn() {
        let (store, id, _dir) = fresh().await;
        assert_eq!(inner_call_id("toolu_1.3"), Some(("toolu_1", 3)));
        assert_eq!(inner_call_id("toolu_1"), None);
        assert_eq!(inner_call_id("call.x"), None);
        store.append(id, &ChatMessage::user("u1")).await.unwrap();
        let args = serde_json::json!({"cmd": "ls"});
        store
            .inner_call_started(id, "c.1", "shell", &args)
            .await
            .unwrap();
        store
            .inner_call_started(id, "c.2", "read", &serde_json::Value::Null)
            .await
            .unwrap();
        store
            .inner_call_started(id, "c.3", "grep", &serde_json::Value::Null)
            .await
            .unwrap();
        store
            .inner_call_finished(id, "c.1", "ok", false, &[PathBuf::from("/s.png")])
            .await
            .unwrap();
        store
            .inner_call_finished(id, "c.2", "ERROR: boom", true, &[])
            .await
            .unwrap();
        // A finish without its start still lands, under an unknown name.
        store
            .inner_call_finished(id, "c.5", "late", false, &[])
            .await
            .unwrap();
        let want = vec![
            InnerCall {
                n: 1,
                name: "shell".into(),
                args: args.clone(),
                result: Some(("ok".into(), false)),
                files: vec![PathBuf::from("/s.png")],
            },
            InnerCall {
                n: 2,
                name: "read".into(),
                args: serde_json::Value::Null,
                result: Some(("ERROR: boom".into(), true)),
                files: Vec::new(),
            },
            InnerCall {
                n: 3,
                name: "grep".into(),
                args: serde_json::Value::Null,
                result: None,
                files: Vec::new(),
            },
            InnerCall {
                n: 5,
                name: "?".into(),
                args: serde_json::Value::Null,
                result: Some(("late".into(), false)),
                files: Vec::new(),
            },
        ];
        assert_eq!(store.inner_calls(id).await.unwrap()["c"], want);
        let whole = store.fork(id, None, None).await.unwrap();
        assert_eq!(store.inner_calls(whole).await.unwrap()["c"], want);

        store.undo(id).await.unwrap();
        store.commit_cut(id).await.unwrap();
        assert!(store.inner_calls(id).await.unwrap().is_empty());
        store.delete_session(whole).await.unwrap();
        let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM inner_calls WHERE session_id = ?")
            .bind(whole.to_string())
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(left.0, 0);
        drop(store);
    }

    #[tokio::test]
    async fn cut_to_accepts_only_turn_starts() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn fork_copies_turns_through_the_given_one() {
        let (store, src, _dir) = fresh().await;
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
    }

    /// A fork at a reply inside a multi-round turn ends on that reply with its
    /// tool calls dropped; later rounds and steers stay behind.
    #[tokio::test]
    async fn fork_at_a_reply_stops_there_without_tool_calls() {
        use genai::chat::{ContentPart, MessageContent, ToolCall, ToolResponse};
        let (store, src, _dir) = fresh().await;
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
            .fetch_one(&store.read)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn todos_version_bumps_only_on_material_change() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn clear_at_compaction_orders_after_update_at_same_seq() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn undo_redo_select_plan_under_cut_with_monotonic_version() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn fork_and_delete_carry_plan_through_boundary() {
        let (store, src, _dir) = fresh().await;
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
    async fn seeded() -> (Store, SessionId, crate::TestDir) {
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
                    ContentPart::ThoughtSignature(genai::chat::ThoughtSignature::new("sig")),
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
        let (store, id, _dir) = seeded().await;
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
    }

    #[tokio::test]
    async fn remove_context_whole_group_keeps_later_rows_and_turns() {
        let (store, id, _dir) = seeded().await;
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
    }

    #[tokio::test]
    async fn remove_head_keeps_epoch_and_tail_from() {
        let (store, id, _dir) = fresh().await;
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
    }

    #[tokio::test]
    async fn revision_bumps_on_every_visibility_change_and_never_resets() {
        let (store, id, _dir) = fresh().await;
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
        // Conditional title + tags: only at the current revision; tags round-trip as a list.
        let tags = vec!["theuth".to_owned(), "merge".to_owned()];
        assert!(
            !store
                .set_title_at_revision(id, "old", &tags, "8")
                .await
                .unwrap()
        );
        assert!(
            store
                .set_title_at_revision(id, "new", &tags, "9")
                .await
                .unwrap()
        );
        let row = store.session(id).await.unwrap().unwrap();
        assert_eq!((row.title.as_str(), &row.tags), ("new", &tags));
        assert!(
            store
                .set_title_at_revision(id, "new", &[], "9")
                .await
                .unwrap()
        );
        assert!(store.session(id).await.unwrap().unwrap().tags.is_empty());
        assert!(
            store
                .set_title_at_revision(id, "x", &[], "bad")
                .await
                .is_err()
        );
        drop(store);
    }

    /// 48 §4.4: a fork carries masks with its rows and heads; the stripped
    /// mid-reply copy is materialized effective so no old part index lands on
    /// a shortened vector.
    #[tokio::test]
    async fn fork_copies_masks_and_remaps_the_stripped_reply() {
        let (store, src, _dir) = fresh().await;
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
            .execute(&store.write)
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
    }

    #[tokio::test]
    async fn context_history_reads_raw_rows_with_membership() {
        let (store, id, _dir) = seeded().await;
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
    }

    /// A failure anywhere in the edit transaction (here: the revision bump,
    /// after every mask was written) leaves masks and revision untouched.
    #[tokio::test]
    async fn remove_context_failed_write_leaves_masks_and_revision() {
        let (store, id, _dir) = seeded().await;
        let before = serde_json::to_string(&store.frame(id).await.unwrap()).unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_rev BEFORE UPDATE OF context_revision ON sessions \
             BEGIN SELECT RAISE(ABORT, 'injected'); END",
        )
        .execute(&store.write)
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
            .execute(&store.write)
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
    }

    /// 183 §4.5 (D4): the approvals pin and the messaging opt-out live on the
    /// row: set, cleared and still there after a restart.
    #[tokio::test]
    async fn session_settings_round_trip_and_survive_reopen() {
        let (store, id, dir) = seeded().await;
        assert_eq!(
            store.session_settings(id).await.unwrap(),
            Some((None, None))
        );
        store.set_session_approvals(id, Some(true)).await.unwrap();
        store
            .set_session_communication(id, Some(false))
            .await
            .unwrap();
        assert_eq!(
            store.session_settings(id).await.unwrap(),
            Some((Some(true), Some(false)))
        );
        store.set_session_approvals(id, Some(false)).await.unwrap();
        store.close().await;
        drop(store);

        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        assert_eq!(
            store.session_settings(id).await.unwrap(),
            Some((Some(false), Some(false)))
        );
        store.set_session_approvals(id, None).await.unwrap();
        store.set_session_communication(id, None).await.unwrap();
        assert_eq!(
            store.session_settings(id).await.unwrap(),
            Some((None, None))
        );
        assert_eq!(
            store.session_settings(SessionId::new()).await.unwrap(),
            None
        );
    }

    /// Per-chat tool hides live on the row: set, still there after a
    /// restart, cleared by an empty list; a row that does not parse is `Err`.
    #[tokio::test]
    async fn session_hidden_round_trips_and_survives_reopen() {
        let (store, id, dir) = seeded().await;
        assert!(store.session_hidden(id).await.unwrap().is_empty());
        let names = vec!["mcp__a__x".to_string(), "mcp__b__y".to_string()];
        store.set_session_hidden(id, &names).await.unwrap();
        store.close().await;
        drop(store);
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        assert_eq!(store.session_hidden(id).await.unwrap(), names);
        store.set_session_hidden(id, &[]).await.unwrap();
        let raw: Option<String> =
            sqlx::query_scalar("SELECT hidden_tools FROM sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(raw, None);
        assert!(
            store
                .session_hidden(SessionId::new())
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE sessions SET hidden_tools = 'nope' WHERE id = ?")
            .bind(id.to_string())
            .execute(&store.read)
            .await
            .unwrap();
        assert!(store.session_hidden(id).await.is_err());
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
        store.close().await;
        drop(store);

        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
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
    }

    /// Two edits confirmed at the same revision: exactly one commits, the
    /// other is rejected as stale, and the frame carries only the winner's
    /// masks. The loser succeeds once it reconfirms at the new revision.
    #[tokio::test]
    async fn concurrent_remove_context_from_same_revision_admits_one() {
        let (store, id, _dir) = seeded().await;
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
    }

    // endregion: --- Context removal (48)

    // region: --- Project metadata (51 §3)

    #[tokio::test]
    async fn project_lives_on_the_root_and_shows_on_every_row() {
        let (store, root, _dir) = fresh().await;
        let child = SessionId::new();
        store
            .create_session(child, "m", Some(root), Some("kid"), Some("ignored"))
            .await
            .unwrap();
        let project_of =
            |list: Vec<SessionSummary>, id| list.into_iter().find(|s| s.id == id).unwrap().project;

        // A child call mutates the root; the caller already normalized.
        assert_eq!(store.set_project(child, Some("Alpha")).await.unwrap(), root);
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|s| s.project.as_deref() == Some("Alpha")));
        assert_eq!(
            store.descendants(root).await.unwrap()[0]
                .1
                .project
                .as_deref(),
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
            store
                .session(fork)
                .await
                .unwrap()
                .unwrap()
                .project
                .as_deref(),
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
    }

    #[tokio::test]
    async fn project_style_round_trips_and_rejects_unknown_keys() {
        let (store, _, _dir) = fresh().await;
        let style = |icon: &str, color: Option<&str>| ProjectStyle {
            name: "Alpha".into(),
            icon: Some(icon.into()),
            color: color.map(str::to_owned),
        };
        assert_eq!(store.project_style("Alpha").await.unwrap(), None);
        store
            .set_project_style("Alpha", Some("Rocket"), Some("teal"))
            .await
            .unwrap();
        store
            .set_project_style("Beta", None, Some("red"))
            .await
            .unwrap();
        assert_eq!(
            store.project_style("Alpha").await.unwrap(),
            Some(style("Rocket", Some("teal")))
        );
        assert_eq!(store.list_project_styles().await.unwrap().len(), 2);
        // A second set replaces, never merges; both `None` drops the row.
        store
            .set_project_style("Alpha", Some("Bug"), None)
            .await
            .unwrap();
        assert_eq!(
            store.project_style("Alpha").await.unwrap(),
            Some(style("Bug", None))
        );
        store.set_project_style("Alpha", None, None).await.unwrap();
        assert_eq!(
            store
                .list_project_styles()
                .await
                .unwrap()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Beta"]
        );
        // Allow-listed keys only, and the rejected write leaves the row alone.
        assert!(
            store
                .set_project_style("Beta", Some("Skull"), None)
                .await
                .is_err()
        );
        assert!(
            store
                .set_project_style("Beta", Some("Bug"), Some("#ff0000"))
                .await
                .is_err()
        );
        assert_eq!(
            store.project_style("Beta").await.unwrap(),
            Some(ProjectStyle {
                name: "Beta".into(),
                icon: None,
                color: Some("red".into()),
            })
        );
        drop(store);
    }

    /// 14 as-built: rows round-trip, ids are per session (`r1` twice), and
    /// a fired/cancelled row goes by `(session, id)`.
    #[tokio::test]
    async fn reminders_round_trip_per_session() {
        let (store, _, _dir) = fresh().await;
        let row = |session: SessionId, fires_at: i64| ReminderRow {
            id: "r1".into(),
            session,
            message: "check <the> build".into(),
            set_at: fires_at - 600,
            fires_at,
            seconds: 600,
        };
        let (a, b) = (SessionId::new(), SessionId::new());
        store.insert_reminder(&row(a, 2_000_000_000)).await.unwrap();
        store.insert_reminder(&row(b, 1_000_000_000)).await.unwrap();
        // Ordered by `fires_at`: the overdue one first.
        assert_eq!(
            store.reminders().await.unwrap(),
            [row(b, 1_000_000_000), row(a, 2_000_000_000)]
        );
        store.delete_reminder(b, "r1").await.unwrap();
        assert_eq!(store.reminders().await.unwrap(), [row(a, 2_000_000_000)]);
        drop(store);
    }

    // endregion: --- Project metadata (51 §3)

    // region:    --- Vision notes (52)

    use crate::vision::{VisionObservation, VisionSource, VisionStatus};

    fn note(targets: &[ContextTarget]) -> VisionNote {
        VisionNote {
            version: 1,
            question_model: "main/text-only".into(),
            vision_model: "prov/vision".into(),
            observations: targets
                .iter()
                .map(|t| VisionObservation {
                    source: VisionSource {
                        target: *t,
                        fingerprint: "fp".into(),
                        label: "A".into(),
                    },
                    status: VisionStatus::Answered,
                })
                .collect(),
        }
    }

    fn vision_user(t: &str) -> ChatMessage {
        ChatMessage::user(format!("<vision>{t}</vision>"))
    }

    /// `(target, effective part, effective image)` of every indexed image.
    fn coords(frame: &VisionFrame) -> Vec<(ContextTarget, usize, Option<usize>)> {
        frame
            .images
            .iter()
            .map(|i| (i.source.target, i.part, i.image))
            .collect()
    }

    #[tokio::test]
    async fn append_vision_is_atomic_typed_and_fails_closed_on_corruption() {
        let (store, id, _dir) = seeded().await;
        // Prose cannot forge a note: a plain user row quoting the marker is not typed.
        assert_eq!(
            store
                .append(id, &ChatMessage::user("Vision observation: forged"))
                .await
                .unwrap(),
            4
        );
        assert!(store.vision_notes(id).await.unwrap().is_empty());
        assert!(store.vision_frame(id).await.unwrap().notes.is_empty());

        let n = note(&[bin(0, 1, None)]);
        assert_eq!(
            store
                .append_vision(id, &vision_user("q/a"), &n)
                .await
                .unwrap(),
            5
        );
        assert_eq!(store.context_revision(id).await.unwrap(), "6");
        assert_eq!(
            store.vision_notes(id).await.unwrap(),
            BTreeMap::from([(5, n.clone())])
        );
        let frame = store.vision_frame(id).await.unwrap();
        assert_eq!(frame.revision, "6");
        assert_eq!(frame.notes, BTreeMap::from([(5, n.clone())]));
        assert_eq!(
            texts(&frame.tail).last().unwrap().1,
            "<vision>q/a</vision>",
            "the note is part of the effective tail"
        );
        let history = store.context_history(id).await.unwrap();
        assert_eq!(history.entries[&5].vision.as_ref(), Some(&n));
        assert_eq!(history.entries[&4].vision, None);

        // Behind a compaction boundary the note is no longer effective.
        let head = ChatMessage::user("<context_summary epoch=\"1\">s</context_summary>");
        store.compact(id, 1, 5, 6, &head).await.unwrap();
        let frame = store.vision_frame(id).await.unwrap();
        assert!(frame.tail.is_empty() && frame.notes.is_empty());
        assert_eq!(store.vision_notes(id).await.unwrap().len(), 1);

        // Corrupt metadata is a visible store error for every typed reader;
        // turn readers still see the row and never treat it as a plain message.
        sqlx::query("UPDATE messages SET vision = '{nope' WHERE session_id = ? AND seq = 5")
            .bind(id.to_string())
            .execute(&store.write)
            .await
            .unwrap();
        store.compact(id, 2, 5, 5, &head).await.unwrap();
        for err in [
            store.vision_notes(id).await.err(),
            store.context_history(id).await.err(),
            store.vision_frame(id).await.err(),
        ] {
            let err = err.expect("corrupt note must fail");
            assert!(
                err.to_string().contains("bad vision metadata on message 5"),
                "{err}"
            );
        }
        assert_eq!(store.messages(id).await.unwrap().len(), 6);
        assert_eq!(
            store.undo(id).await.unwrap().as_deref(),
            Some("Vision observation: forged"),
            "the corrupt note is still no turn opener"
        );
        drop(store);
    }

    #[tokio::test]
    async fn vision_frame_indexes_original_coordinates_under_masks_and_heads() {
        let (store, id, _dir) = seeded().await;
        let frame = store.vision_frame(id).await.unwrap();
        assert_eq!(
            (frame.epoch, frame.head.is_none(), frame.tail.len()),
            (0, true, 4)
        );
        assert_eq!(
            coords(&frame),
            [
                (bin(0, 1, None), 1, None),
                (bin(0, 2, None), 2, None),
                (bin(2, 0, Some(0)), 0, Some(0)),
                (bin(2, 0, Some(1)), 0, Some(1)),
                (bin(2, 1, Some(0)), 1, Some(0)),
            ]
        );

        // A partial removal shifts projected indexes but never the reference.
        store
            .remove_context(id, "4", &[bin(0, 1, None), bin(2, 0, Some(0))])
            .await
            .unwrap();
        let frame = store.vision_frame(id).await.unwrap();
        assert_eq!(frame.revision, "5");
        assert_eq!(
            coords(&frame),
            [
                (bin(0, 2, None), 1, None),
                (bin(2, 0, Some(1)), 0, Some(0)),
                (bin(2, 1, Some(0)), 1, Some(0)),
            ]
        );
        assert_eq!(names(&frame.tail[0].1), ["same.png"]);
        assert_eq!(
            frame.images[0].binary.name.as_deref(),
            Some("same.png"),
            "the index carries the surviving payload"
        );

        // A whole-removed row contributes nothing; a head's images come first.
        store.remove_context(id, "5", &[msg(0)]).await.unwrap();
        let head = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(
                "<context_summary epoch=\"1\">snap as 1 image</context_summary>".into(),
            ),
            ContentPart::Binary(img("h.png")),
        ]));
        store.compact(id, 1, 3, 2, &head).await.unwrap();
        let frame = store.vision_frame(id).await.unwrap();
        assert_eq!((frame.epoch, frame.tail.len()), (1, 2));
        assert_eq!(
            coords(&frame),
            [
                (
                    ContextTarget::Binary {
                        owner: ContextOwner::Head { epoch: 1 },
                        part: 1,
                        image: None,
                    },
                    1,
                    None
                ),
                (bin(2, 0, Some(1)), 0, Some(0)),
                (bin(2, 1, Some(0)), 1, Some(0)),
            ]
        );
        assert_eq!(frame.revision, store.context_revision(id).await.unwrap());
        drop(store);
    }

    #[tokio::test]
    async fn vision_note_after_reply_opens_no_turn_and_next_user_stays_real() {
        let (store, id, _dir) = fresh().await;
        append_all(&store, id, [ChatMessage::user("u1"), asst("a1")]).await;
        let n = note(&[bin(0, 0, None)]);
        assert_eq!(
            store
                .append_vision(id, &vision_user("h"), &n)
                .await
                .unwrap(),
            2
        );
        // Manual harvest after a reply does not erase the turn's final answer.
        assert_eq!(store.last_turn_text(id).await.unwrap(), "a1");
        append_all(&store, id, [ChatMessage::user("u2"), asst("a2")]).await;
        assert_eq!(store.last_turn_text(id).await.unwrap(), "a2");

        let messages = store.messages(id).await.unwrap();
        assert_eq!(turn_openers(&messages, |s| s == 2), [0, 3]);
        assert_eq!(
            turn_openers(&messages, |_| false),
            [0, 2],
            "without the typed flag the note would forge a turn and hide u2"
        );

        let err = store.cut_to(id, 2).await.unwrap_err();
        assert_eq!(err.to_string(), "not a turn start");
        assert!(store.fork(id, Some(2), None).await.is_err());

        assert_eq!(text_of(&store.cut_to(id, 3).await.unwrap().content), "u2");
        assert_eq!(store.hidden_turns(id).await.unwrap(), 1);
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u1"));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 2);
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u1"));
        assert_eq!(
            texts(&store.messages(id).await.unwrap()),
            [
                (0, "u1".into()),
                (1, "a1".into()),
                (2, "<vision>h</vision>".into())
            ],
            "redo restores the note with its turn"
        );
        assert_eq!(store.redo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.redo(id).await.unwrap(), None);

        // A fork through the first turn copies the note with its metadata.
        let fork = store.fork(id, Some(0), None).await.unwrap();
        assert_eq!(store.messages(fork).await.unwrap().len(), 3);
        assert_eq!(
            store.vision_notes(fork).await.unwrap(),
            BTreeMap::from([(2, n.clone())])
        );
        assert_eq!(store.vision_frame(fork).await.unwrap().notes.len(), 1);
        drop(store);
    }

    #[tokio::test]
    async fn resume_reuses_notes_and_committed_cut_frees_their_seq() {
        let (store, id, dir) = fresh().await;
        append_all(
            &store,
            id,
            [
                ChatMessage::user(MessageContent::from_parts(vec![
                    ContentPart::Text("look".into()),
                    ContentPart::Binary(img("a.png")),
                ])),
                asst("a1"),
                ChatMessage::user("u2"),
            ],
        )
        .await;
        let n = note(&[bin(0, 1, None)]);
        assert_eq!(
            store
                .append_vision(id, &vision_user("q/a"), &n)
                .await
                .unwrap(),
            3
        );
        store.append(id, &asst("a2")).await.unwrap();

        // Resume: a fresh handle reloads the committed note, no inference implied.
        store.close().await;
        drop(store);
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let frame = store.vision_frame(id).await.unwrap();
        assert_eq!(frame.notes, BTreeMap::from([(3, n.clone())]));
        assert_eq!(coords(&frame), [(bin(0, 1, None), 1, None)]);

        // Removing the note row keeps it in raw history but out of the active notes.
        let (removal, _, _, _) = store.remove_context(id, "5", &[msg(3)]).await.unwrap();
        assert_eq!(removal.messages, 1);
        assert!(store.vision_frame(id).await.unwrap().notes.is_empty());
        assert_eq!(store.vision_notes(id).await.unwrap().len(), 1);
        assert!(store.context_history(id).await.unwrap().entries[&3].removed);

        // Undo hides the turn holding the note: nothing typed remains visible.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert!(store.vision_frame(id).await.unwrap().notes.is_empty());
        assert!(store.vision_notes(id).await.unwrap().is_empty());
        assert_eq!(store.hidden_turns(id).await.unwrap(), 1);

        // Committed cut deletes the note with its row; the reused seq is plain.
        store.commit_cut(id).await.unwrap();
        assert_eq!(
            store.append(id, &ChatMessage::user("again")).await.unwrap(),
            2
        );
        assert_eq!(store.append(id, &asst("a3")).await.unwrap(), 3);
        assert!(store.vision_notes(id).await.unwrap().is_empty());
        assert!(
            store.context_history(id).await.unwrap().entries[&3]
                .vision
                .is_none()
        );
        let left: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM messages WHERE session_id = ? AND vision IS NOT NULL",
        )
        .bind(id.to_string())
        .fetch_one(&store.read)
        .await
        .unwrap();
        assert_eq!(left.0, 0);
        drop(store);
    }

    // endregion: --- Vision notes (52)

    // region: --- Capture (68 §3)

    /// Every public mutating method that touches a replicated table leaves at
    /// least one `session_log` row: a statement that slipped onto the read
    /// pool would leave none.
    #[tokio::test]
    async fn every_mutation_is_captured_on_the_write_connection() {
        use crate::communication::PeerScope;
        use crate::cost::{CallKind, CallStatus, CostObservation, MeterUpdate, Money};
        use crate::todo::Status;

        let scratch = crate::TestDir::new("capture");
        let dir = scratch.0.clone();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        assert!(!store.is_private());
        let id = SessionId::new();
        let mut since = store.log_seq();
        // Log rows per mutating call; the contract doc reports the share of
        // calls that produced exactly one (one session touched).
        let mut rows: Vec<usize> = Vec::new();
        let plan = [Todo {
            text: "t".into(),
            status: Status::Pending,
            reason: None,
        }];

        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("create_session", &mut since)
                .await
                .len(),
        );
        store.set_title(id, "t").await.unwrap();
        rows.push(store.assert_write_pool("set_title", &mut since).await.len());
        store.set_pinned(id, true).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_pinned", &mut since)
                .await
                .len(),
        );
        store.set_archived(id, true).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_archived", &mut since)
                .await
                .len(),
        );
        store.set_model(id, "m2").await.unwrap();
        rows.push(store.assert_write_pool("set_model", &mut since).await.len());
        store.set_effort(id, Some("high")).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_effort", &mut since)
                .await
                .len(),
        );
        store.set_session_approvals(id, Some(true)).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_session_approvals", &mut since)
                .await
                .len(),
        );
        store
            .set_session_communication(id, Some(false))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("set_session_communication", &mut since)
                .await
                .len(),
        );
        store
            .set_acp(id, "codex", Some(Path::new("/tmp")), &[])
            .await
            .unwrap();
        rows.push(store.assert_write_pool("set_acp", &mut since).await.len());
        store.set_acp_session(id, "x").await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_acp_session", &mut since)
                .await
                .len(),
        );
        store.set_harness_model(id, "hm").await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_harness_model", &mut since)
                .await
                .len(),
        );
        store
            .set_cwd(id, Path::new("/tmp/elsewhere"))
            .await
            .unwrap();
        rows.push(store.assert_write_pool("set_cwd", &mut since).await.len());
        store.set_unattended(id).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_unattended", &mut since)
                .await
                .len(),
        );
        store.set_project(id, Some("p")).await.unwrap();
        rows.push(
            store
                .assert_write_pool("set_project", &mut since)
                .await
                .len(),
        );
        store
            .set_root_scope(id, PeerScope::Owner, Some("alias"))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("set_root_scope", &mut since)
                .await
                .len(),
        );
        store.append(id, &ChatMessage::user("hi")).await.unwrap();
        rows.push(store.assert_write_pool("append", &mut since).await.len());
        store.append(id, &asst("a")).await.unwrap();
        rows.push(store.assert_write_pool("append", &mut since).await.len());
        store
            .append_with_todos(id, &resp("c1"), TodoSource::Native, &plan)
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("append_with_todos", &mut since)
                .await
                .len(),
        );
        store.save_todos(id, TodoSource::Native, &[]).await.unwrap();
        rows.push(
            store
                .assert_write_pool("save_todos", &mut since)
                .await
                .len(),
        );
        store.record_error(id, "boom").await.unwrap();
        rows.push(
            store
                .assert_write_pool("record_error", &mut since)
                .await
                .len(),
        );
        store
            .inner_call_started(id, "c1.1", "read", &serde_json::json!({}))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("inner_call_started", &mut since)
                .await
                .len(),
        );
        store
            .inner_call_finished(id, "c1.1", "ok", false, &[])
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("inner_call_finished", &mut since)
                .await
                .len(),
        );
        let reminder = ReminderRow {
            id: "r1".into(),
            session: id,
            message: "m".into(),
            set_at: 1,
            fires_at: 2,
            seconds: 1,
        };
        store.insert_reminder(&reminder).await.unwrap();
        rows.push(
            store
                .assert_write_pool("insert_reminder", &mut since)
                .await
                .len(),
        );
        store.delete_reminder(id, "r1").await.unwrap();
        rows.push(
            store
                .assert_write_pool("delete_reminder", &mut since)
                .await
                .len(),
        );
        let revision = store.context_revision(id).await.unwrap();
        store
            .set_title_at_revision(id, "t2", &[], &revision)
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("set_title_at_revision", &mut since)
                .await
                .len(),
        );
        store
            .compact(id, 1, 2, 3, &ChatMessage::system("head"))
            .await
            .unwrap();
        rows.push(store.assert_write_pool("compact", &mut since).await.len());
        store.append(id, &ChatMessage::user("u2")).await.unwrap();
        rows.push(store.assert_write_pool("append", &mut since).await.len());
        store.append(id, &asst("a2")).await.unwrap();
        rows.push(store.assert_write_pool("append", &mut since).await.len());
        store
            .compact_with_todos(
                id,
                2,
                4,
                5,
                &ChatMessage::system("head2"),
                (TodoSource::Native, &plan),
            )
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("compact_with_todos", &mut since)
                .await
                .len(),
        );
        store.append(id, &ChatMessage::user("u3")).await.unwrap();
        rows.push(store.assert_write_pool("append", &mut since).await.len());
        store.undo(id).await.unwrap();
        rows.push(store.assert_write_pool("undo", &mut since).await.len());
        store.redo(id).await.unwrap();
        rows.push(store.assert_write_pool("redo", &mut since).await.len());
        store.cut_to(id, 5).await.unwrap();
        rows.push(store.assert_write_pool("cut_to", &mut since).await.len());
        store.commit_cut(id).await.unwrap();
        rows.push(
            store
                .assert_write_pool("commit_cut", &mut since)
                .await
                .len(),
        );
        let forked = store.fork(id, None, None).await.unwrap();
        rows.push(store.assert_write_pool("fork", &mut since).await.len());

        let call = store
            .reserve_call(id, "req", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("reserve_call", &mut since)
                .await
                .len(),
        );
        store.mark_dispatched(&call).await.unwrap();
        rows.push(
            store
                .assert_write_pool("mark_dispatched", &mut since)
                .await
                .len(),
        );
        let obs = CostObservation {
            returned: Some(Money::from_f64(0.1, "USD").unwrap()),
            ..Default::default()
        };
        store
            .settle_call(&call, CallStatus::Settled, obs)
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("settle_call", &mut since)
                .await
                .len(),
        );
        let call = store
            .reserve_call(id, "req2", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("reserve_call", &mut since)
                .await
                .len(),
        );
        store.mark_not_dispatched(&call, "no").await.unwrap();
        rows.push(
            store
                .assert_write_pool("mark_not_dispatched", &mut since)
                .await
                .len(),
        );
        store
            .reserve_call(id, "req3", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("reserve_call", &mut since)
                .await
                .len(),
        );
        store.unresolve_open_calls().await.unwrap();
        rows.push(
            store
                .assert_write_pool("unresolve_open_calls", &mut since)
                .await
                .len(),
        );
        store
            .apply_meter(
                id,
                MeterUpdate {
                    origin: "h".into(),
                    external_session: "e".into(),
                    cumulative: Money::from_f64(0.2, "USD").unwrap(),
                    confirmed_new_session: true,
                    report_id: None,
                },
            )
            .await
            .unwrap();
        rows.push(
            store
                .assert_write_pool("apply_meter", &mut since)
                .await
                .len(),
        );

        store.delete_session(forked).await.unwrap();
        rows.push(
            store
                .assert_write_pool("delete_session(fork)", &mut since)
                .await
                .len(),
        );
        store.delete_session(id).await.unwrap();
        rows.push(
            store
                .assert_write_pool("delete_session", &mut since)
                .await
                .len(),
        );

        let single = rows.iter().filter(|&&n| n == 1).count();
        eprintln!(
            "single-session changesets: {single}/{} ({:.1} %)",
            rows.len(),
            100.0 * single as f64 / rows.len() as f64
        );
        // Only `fork` (source + new session) and `delete_session` of a fork's
        // parent tree touch two sessions in one transaction.
        assert!(rows.iter().all(|&n| n <= 2), "{rows:?}");
        for row in store.log_after(0).await.unwrap() {
            assert!(replica::Hlc::parse(&row.hlc).is_some(), "{}", row.hlc);
            assert!(!row.changeset.is_empty());
            assert_eq!(row.epoch, 1);
        }

        // A private store captures nothing and never panics here.
        let private = Store::open_memory().await.unwrap();
        assert!(private.is_private());
        private
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        assert!(
            private
                .assert_write_pool("private", &mut 0)
                .await
                .is_empty()
        );

        store.close().await;
    }

    /// 136 D6: only the owner's own attended root consolidates here — an
    /// `isolated` root (ACP/channel/`NewSession`), a routine's unattended
    /// root, a child and a desk are all refused. The dreaming session is
    /// admitted by knowledge's registered night, never by this query.
    #[tokio::test]
    async fn may_consolidate_admits_only_the_attended_owner_root() {
        use crate::communication::PeerScope;
        let store = &Store::open_memory().await.unwrap();
        let mk = |parent: Option<SessionId>| async move {
            let id = SessionId::new();
            store
                .create_session(id, "m", parent, parent.map(|_| "kid"), None)
                .await
                .unwrap();
            id
        };
        let isolated = mk(None).await;
        assert!(
            !store.may_consolidate(isolated).await.unwrap(),
            "isolated root"
        );

        let owner = mk(None).await;
        store
            .set_root_scope(owner, PeerScope::Owner, None)
            .await
            .unwrap();
        assert!(store.may_consolidate(owner).await.unwrap(), "owner root");

        let routine = mk(None).await;
        store
            .set_root_scope(routine, PeerScope::Owner, None)
            .await
            .unwrap();
        store.set_unattended(routine).await.unwrap();
        assert!(
            !store.may_consolidate(routine).await.unwrap(),
            "a routine's unattended root is not the owner at the keyboard"
        );

        let child = mk(Some(owner)).await;
        assert!(!store.may_consolidate(child).await.unwrap(), "child");

        let desk = mk(None).await;
        store
            .set_root_scope(desk, PeerScope::Owner, None)
            .await
            .unwrap();
        store.set_desk(desk, Some("builder")).await.unwrap();
        assert!(!store.may_consolidate(desk).await.unwrap(), "desk");
        assert!(
            !store.may_consolidate(SessionId::new()).await.unwrap(),
            "unknown"
        );
    }

    /// `owner` starts as the opening engine's id (`''` when unknown), is
    /// backfilled on the next open that knows one, and decides `here`.
    #[tokio::test]
    async fn owner_is_set_at_creation_backfilled_at_open_and_decides_here() {
        let scratch = crate::TestDir::new("owner");
        let dir = scratch.0.clone();
        let path = dir.join("sessions.sqlite");
        let store = Store::open(&path, None).await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let s = store.session(id).await.unwrap().unwrap();
        assert_eq!((s.owner.as_str(), s.epoch, s.here), ("", 1, true));
        store.close().await;

        let a = mesh::EngineId("engine-a".into());
        let store = Store::open(&path, Some(a.clone())).await.unwrap();
        let s = store.session(id).await.unwrap().unwrap();
        assert_eq!((s.owner.as_str(), s.here), ("engine-a", true));
        store
            .create_session(SessionId::new(), "m", Some(id), None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("u")).await.unwrap();
        let forked = store.fork(id, None, None).await.unwrap();
        for s in store.list_sessions().await.unwrap() {
            assert_eq!(
                (s.owner.as_str(), s.epoch, s.here),
                ("engine-a", 1, true),
                "{}",
                s.id
            );
        }
        store.close().await;

        let store = Store::open(&path, Some(mesh::EngineId("engine-b".into())))
            .await
            .unwrap();
        let s = store.session(forked).await.unwrap().unwrap();
        assert_eq!((s.owner.as_str(), s.here), ("engine-a", false));
        store.close().await;
    }

    /// STALE-WAKE-DESKS: one desk row per agent per engine. `desk_session`
    /// is what the roster reopens; a second mark for the same agent is refused
    /// by the schema; an archived (put-away) desk is still the desk; another
    /// engine's desk is not ours.
    #[tokio::test]
    async fn one_desk_per_agent_is_found_and_enforced() {
        let scratch = crate::TestDir::new("desk");
        let dir = scratch.0.clone();
        let path = dir.join("sessions.sqlite");
        let theirs = Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        let remote = SessionId::new();
        theirs
            .create_session(remote, "m", None, None, None)
            .await
            .unwrap();
        theirs.set_desk(remote, Some("ops")).await.unwrap();
        theirs.close().await;

        let store = Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        assert_eq!(
            store.desk_session("ops").await.unwrap(),
            None,
            "another engine's desk is not reopened here"
        );
        let (a, b) = (SessionId::new(), SessionId::new());
        store
            .create_session(a, "m", None, None, None)
            .await
            .unwrap();
        store
            .create_session(b, "m", None, None, None)
            .await
            .unwrap();
        store.set_desk(a, Some("ops")).await.unwrap();
        assert_eq!(store.desk_session("ops").await.unwrap(), Some(a));
        assert!(
            store.set_desk(b, Some("ops")).await.is_err(),
            "a second desk for ops is refused"
        );
        store.set_archived(a, true).await.unwrap();
        assert_eq!(
            store.desk_session("ops").await.unwrap(),
            Some(a),
            "a put-away desk is still the desk"
        );
        store.set_desk(a, None).await.unwrap();
        store.set_desk(b, Some("ops")).await.unwrap();
        assert_eq!(
            store.desk_session("ops").await.unwrap(),
            Some(b),
            "retired → the next one is the desk"
        );
        store.close().await;
    }

    /// Migration 0025 on a store with the duplicates a pre-fix engine left:
    /// the most recently updated row per agent keeps the mark; the others are
    /// unmarked, unpinned, archived and titled `<agent> · desk (superseded …)`.
    #[tokio::test]
    async fn migration_dedupes_desks_by_last_update() {
        let scratch = crate::TestDir::new("dedupe");
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("me".into())),
        )
        .await
        .unwrap();
        // The fixture predates the index: drop it, build the duplicates, replay the migration.
        sqlx::query("DROP INDEX sessions_desk_owner")
            .execute(&store.read)
            .await
            .unwrap();
        let rows = [
            ("old-t", "theuth", "2026-09-26T10:06:48+00:00"),
            ("new-t", "theuth", "2026-09-26T11:43:05+00:00"),
            ("old-s", "stas", "2026-09-26T10:27:45+00:00"),
            ("new-s", "stas", "2026-09-26T11:43:00+00:00"),
            ("only", "ops", "2026-09-26T09:00:00+00:00"),
        ];
        for (id, desk, at) in rows {
            sqlx::query("INSERT INTO sessions (id, created_at, updated_at, title, model, desk, pinned, owner) VALUES (?, ?, ?, ?, 'm', ?, 1, 'me')")
                .bind(id).bind(at).bind(at).bind(format!("{desk} · desk")).bind(desk)
                .execute(&store.read).await.unwrap();
        }
        sqlx::raw_sql(include_str!("../migrations/0025_one_desk_per_agent.sql"))
            .execute(&store.read)
            .await
            .unwrap();
        let got: Vec<(String, Option<String>, bool, bool, String)> =
            sqlx::query_as("SELECT id, desk, pinned, archived, title FROM sessions WHERE id IN ('old-t','new-t','old-s','new-s','only') ORDER BY id")
                .fetch_all(&store.read).await.unwrap();
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string(); // sqlite date('now') is UTC
        let sup = |a: &str| format!("{a} · desk (superseded {date})");
        assert_eq!(
            got,
            vec![
                (
                    "new-s".into(),
                    Some("stas".into()),
                    true,
                    false,
                    "stas · desk".into()
                ),
                (
                    "new-t".into(),
                    Some("theuth".into()),
                    true,
                    false,
                    "theuth · desk".into()
                ),
                ("old-s".into(), None, false, true, sup("stas")),
                ("old-t".into(), None, false, true, sup("theuth")),
                (
                    "only".into(),
                    Some("ops".into()),
                    true,
                    false,
                    "ops · desk".into()
                ),
            ]
        );
        assert_eq!(
            store
                .desk_session("theuth")
                .await
                .unwrap()
                .map(|s| s.to_string())
                .as_deref(),
            None,
            "fixture ids are not uuids"
        );
        store.close().await;
    }

    /// `errors.id` is text: fresh rows mint uuid v7, a fork's copies keep the
    /// source order behind one anchor and never collide with the originals.
    #[tokio::test]
    async fn error_ids_are_text_and_unique_across_fork() {
        let (store, id, _dir) = fresh().await;
        store.append(id, &ChatMessage::user("u")).await.unwrap();
        store.record_error(id, "first").await.unwrap();
        store.record_error(id, "second").await.unwrap();
        let forked = store.fork(id, None, None).await.unwrap();
        let ids: Vec<(String, String)> =
            sqlx::query_as("SELECT id, session_id FROM errors ORDER BY id")
                .fetch_all(&store.read)
                .await
                .unwrap();
        assert_eq!(ids.len(), 4);
        assert!(
            ids.iter()
                .all(|(i, _)| uuid::Uuid::parse_str(i.split(':').next().unwrap()).is_ok())
        );
        assert_eq!(
            store
                .errors(forked)
                .await
                .unwrap()
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        drop(store);
    }

    /// A session owned elsewhere refuses every local write with `NotOwner`
    /// and rolls the transaction back; a take (`owner = me`) is the one write
    /// that passes, and it is logged under the new epoch.
    #[tokio::test]
    async fn writes_on_a_foreign_session_are_refused_until_taken() {
        let scratch = crate::TestDir::new("guard");
        let dir = scratch.0.clone();
        let path = dir.join("sessions.sqlite");
        let store = Store::open(&path, Some(mesh::EngineId("a".into())))
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("u")).await.unwrap();
        store.close().await;

        let store = Store::open(&path, Some(mesh::EngineId("b".into())))
            .await
            .unwrap();
        let before = store.log_seq();
        let err = store
            .append(id, &ChatMessage::user("nope"))
            .await
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<StoreError>(),
            Some(&StoreError::NotOwner {
                session: id,
                owner: "a".into(),
                epoch: 1,
            })
        );
        assert_eq!(store.messages(id).await.unwrap().len(), 1, "rolled back");
        assert_eq!(store.log_seq(), before, "nothing logged");
        assert!(
            store.set_owner(id, "c", 2).await.is_err(),
            "only a take by this engine passes"
        );
        store.set_owner(id, "b", 2).await.unwrap();
        let rows = store.log_after(before).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].session_id.as_str(), rows[0].epoch),
            (id.to_string().as_str(), 2)
        );
        store
            .append(id, &ChatMessage::user("mine now"))
            .await
            .unwrap();
        assert_eq!(
            store.log_after(before).await.unwrap().last().unwrap().epoch,
            2
        );
        assert_eq!(store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        assert_eq!(store.fence(id).await.unwrap(), Some((2, "b".into())));
        store.close().await;
    }

    /// The fence (`session_epochs` ∪ the session row) decides the guard:
    /// with it raised to a foreign owner, every public mutating method that
    /// touches the session is refused with `NotOwner` and writes nothing.
    #[tokio::test]
    async fn every_mutator_is_refused_behind_a_foreign_fence() {
        use crate::communication::PeerScope;
        use crate::cost::{CallKind, CallStatus, CostObservation, MeterUpdate, Money};
        use crate::todo::Status;

        let scratch = crate::TestDir::new("fence");
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let id = SessionId::new();
        let plan = [Todo {
            text: "t".into(),
            status: Status::Pending,
            reason: None,
        }];
        // State every conditional write needs: two turns, a reminder, an
        // inner call, two reserved model calls.
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("u1")).await.unwrap();
        store.append(id, &asst("a1")).await.unwrap();
        store.append(id, &ChatMessage::user("u2")).await.unwrap();
        store.append(id, &asst("a2")).await.unwrap();
        let reminder = ReminderRow {
            id: "r1".into(),
            session: id,
            message: "m".into(),
            set_at: 1,
            fires_at: 2,
            seconds: 1,
        };
        store.insert_reminder(&reminder).await.unwrap();
        store
            .inner_call_started(id, "c1.1", "read", &serde_json::json!({}))
            .await
            .unwrap();
        let call = store
            .reserve_call(id, "req", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        let call2 = store
            .reserve_call(id, "req2", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        let revision = store.context_revision(id).await.unwrap();
        let messages = store.messages(id).await.unwrap().len();

        // Another engine took it: the fence moves, the row is still ours until its log row lands.
        assert!(store.set_fence(id, 2, "x").await.unwrap());
        assert_eq!(store.owner(id).await.unwrap(), Some(("a".into(), 1)));
        let before = store.log_seq();

        macro_rules! refused {
            ($what:expr, $call:expr) => {{
                let err = $call
                    .await
                    .err()
                    .unwrap_or_else(|| panic!("{} passed behind a foreign fence", $what));
                match err.downcast_ref::<StoreError>() {
                    Some(StoreError::NotOwner {
                        session,
                        owner,
                        epoch,
                    }) => {
                        assert_eq!(
                            (*session, owner.as_str(), *epoch),
                            (id, "x", 2),
                            "{}",
                            $what
                        );
                    }
                    _ => panic!("{}: {err:#}", $what),
                }
            }};
        }
        refused!("set_title", store.set_title(id, "t"));
        refused!("set_pinned", store.set_pinned(id, true));
        refused!("set_archived", store.set_archived(id, true));
        refused!("set_model", store.set_model(id, "m2"));
        refused!("set_effort", store.set_effort(id, Some("high")));
        refused!(
            "set_session_approvals",
            store.set_session_approvals(id, Some(true))
        );
        refused!(
            "set_session_communication",
            store.set_session_communication(id, Some(false))
        );
        refused!(
            "set_session_hidden",
            store.set_session_hidden(id, &["x".to_string()])
        );
        refused!(
            "set_acp",
            store.set_acp(id, "codex", Some(Path::new("/tmp")), &[])
        );
        refused!("set_acp_session", store.set_acp_session(id, "x"));
        refused!("set_harness_model", store.set_harness_model(id, "hm"));
        refused!("set_cwd", store.set_cwd(id, Path::new("/tmp/elsewhere")));
        refused!("set_unattended", store.set_unattended(id));
        refused!("set_project", store.set_project(id, Some("p")));
        refused!(
            "set_root_scope",
            store.set_root_scope(id, PeerScope::Owner, Some("alias"))
        );
        refused!("append", store.append(id, &ChatMessage::user("nope")));
        refused!(
            "append_with_todos",
            store.append_with_todos(id, &resp("c1"), TodoSource::Native, &plan)
        );
        refused!(
            "save_todos",
            store.save_todos(id, TodoSource::Native, &plan)
        );
        refused!("record_error", store.record_error(id, "boom"));
        refused!(
            "inner_call_started",
            store.inner_call_started(id, "c1.2", "read", &serde_json::json!({}))
        );
        refused!(
            "inner_call_finished",
            store.inner_call_finished(id, "c1.1", "ok", false, &[])
        );
        let r2 = ReminderRow {
            id: "r2".into(),
            ..reminder.clone()
        };
        refused!("insert_reminder", store.insert_reminder(&r2));
        refused!("delete_reminder", store.delete_reminder(id, "r1"));
        refused!(
            "set_title_at_revision",
            store.set_title_at_revision(id, "t2", &[], &revision)
        );
        refused!(
            "compact",
            store.compact(id, 1, 2, 3, &ChatMessage::system("head"))
        );
        refused!(
            "compact_with_todos",
            store.compact_with_todos(
                id,
                2,
                4,
                5,
                &ChatMessage::system("head2"),
                (TodoSource::Native, &plan)
            )
        );
        refused!("undo", store.undo(id));
        refused!("cut_to", store.cut_to(id, 2));
        refused!("mark_dispatched", store.mark_dispatched(&call));
        refused!(
            "mark_not_dispatched",
            store.mark_not_dispatched(&call2, "no")
        );
        refused!(
            "settle_call",
            store.settle_call(
                &call,
                CallStatus::Settled,
                CostObservation {
                    returned: Some(Money::from_f64(0.1, "USD").unwrap()),
                    ..Default::default()
                },
            )
        );
        refused!(
            "apply_meter",
            store.apply_meter(
                id,
                MeterUpdate {
                    origin: "h".into(),
                    external_session: "e".into(),
                    cumulative: Money::from_f64(0.2, "USD").unwrap(),
                    confirmed_new_session: true,
                    report_id: None,
                },
            )
        );
        // A take for someone else is judged on its post-state: refused as `y`'s.
        let err = store.set_owner(id, "y", 3).await.unwrap_err();
        assert!(
            matches!(err.downcast_ref::<StoreError>(), Some(StoreError::NotOwner { owner, epoch: 3, .. }) if owner == "y"),
            "{err:#}"
        );
        refused!("delete_session", store.delete_session(id));

        assert_eq!(store.log_seq(), before, "nothing logged");
        assert_eq!(
            store.messages(id).await.unwrap().len(),
            messages,
            "everything rolled back"
        );
        assert!(store.session(id).await.unwrap().is_some());
        // A fork reads the foreign session and writes only the new one, which is ours.
        let forked = store.fork(id, None, None).await.unwrap();
        assert_eq!(store.owner(forked).await.unwrap(), Some(("a".into(), 1)));
        assert_eq!(
            store.owner(id).await.unwrap(),
            Some(("a".into(), 1)),
            "the source row is untouched"
        );

        // Taking it back here (epoch 3 > fence) is the one write that passes.
        store.set_owner(id, "a", 3).await.unwrap();
        store
            .append(id, &ChatMessage::user("mine again"))
            .await
            .unwrap();
        assert_eq!(
            store.log_after(before).await.unwrap().last().unwrap().epoch,
            3
        );
        store.close().await;
    }

    /// A transaction that took a log seq for one session and is then refused
    /// for another (`NotOwner`) gives the seq back: the next logged row is
    /// `head + 1`, so the log stays contiguous and no peer stalls on a gap.
    /// Both creation orders, so the refused session is captured after ours once.
    #[tokio::test]
    async fn a_refused_commit_burns_no_log_seq() {
        let scratch = crate::TestDir::new("burned-seq");
        let store = Store::open(
            &scratch.0.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        for ours_first in [true, false] {
            let (mine, theirs) = (SessionId::new(), SessionId::new());
            let order = if ours_first {
                [mine, theirs]
            } else {
                [theirs, mine]
            };
            for id in order {
                store
                    .create_session(id, "m", None, None, None)
                    .await
                    .unwrap();
            }
            // `theirs` now belongs to `b` (a replicated take: capture off).
            let mut tx = store.begin_apply().await.unwrap();
            sqlx::query("UPDATE sessions SET owner = 'b', epoch = 2 WHERE id = ?")
                .bind(theirs.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let head = store.log_seq();
            let mut tx = store.begin_write().await.unwrap();
            for id in order {
                sqlx::query("UPDATE sessions SET title = 'both' WHERE id = ?")
                    .bind(id.to_string())
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            let err = tx.commit().await.unwrap_err();
            assert!(
                matches!(
                    err.downcast_ref::<StoreError>(),
                    Some(StoreError::NotOwner { .. })
                ),
                "{err:#}"
            );
            assert_eq!(
                store.log_seq(),
                head,
                "ours_first={ours_first}: seq given back"
            );
            store.set_title(mine, "mine").await.unwrap();
            let seqs: Vec<i64> = store
                .log_after(head)
                .await
                .unwrap()
                .iter()
                .map(|r| r.seq)
                .collect();
            assert_eq!(seqs, vec![head + 1], "ours_first={ours_first}: no gap");
        }
        store.close().await;
    }

    /// Demote bookkeeping (68 §4.3): only rows above what the taker had
    /// applied are orphaned; a local note is not logged and never guarded;
    /// `session_log_seq` follows the session's own rows; `flush` logs nothing.
    #[tokio::test]
    async fn orphans_above_the_shipped_cursor_and_local_notes_are_not_logged() {
        let scratch = crate::TestDir::new("orphan");
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("u1")).await.unwrap();
        let shipped = store.log_seq();
        assert_eq!(store.session_log_seq(id).await.unwrap(), shipped);
        store.append(id, &ChatMessage::user("u2")).await.unwrap();
        store.append(id, &ChatMessage::user("u3")).await.unwrap();
        store.flush().await.unwrap();
        assert_eq!(store.log_seq(), shipped + 2, "flush logs nothing");
        assert_eq!(store.session_log_seq(id).await.unwrap(), shipped + 2);

        assert_eq!(store.log_orphan_after(id, 1, shipped).await.unwrap(), 2);
        let orphans = store.log_orphans(id).await.unwrap();
        assert_eq!(
            orphans.iter().map(|r| r.seq).collect::<Vec<_>>(),
            [shipped + 1, shipped + 2]
        );
        assert_eq!(
            store.log_after(0).await.unwrap().len(),
            shipped as usize,
            "orphans left the shipping view"
        );

        // The taker's row landed: the fence is theirs; the local note still writes and is not logged.
        assert!(store.set_fence(id, 2, "b").await.unwrap());
        let before = store.log_seq();
        let seq = store
            .append_local_note(id, "2 turns were not delivered")
            .await
            .unwrap();
        assert_eq!(store.log_seq(), before);
        let last = store.messages(id).await.unwrap().pop().unwrap();
        assert_eq!((last.0, last.1.role), (seq, ChatRole::System));
        store.close().await;
    }

    /// Log rows split per session, seqs are contiguous per engine, orphans
    /// leave `log_after`, pruning keeps orphans, cursors and fences upsert,
    /// and an apply transaction leaves no echo.
    #[tokio::test]
    async fn log_api_round_trip() {
        let (store, id, _dir) = fresh().await;
        let first = store.log_seq();
        store.append(id, &ChatMessage::user("u")).await.unwrap();
        let other = store.fork(id, None, None).await.unwrap();
        let rows = store.log_after(first).await.unwrap();
        // append → one row; fork → one row per session touched (the source
        // row's revision is untouched, so the new session alone).
        let per_session: HashMap<&str, usize> = rows.iter().fold(HashMap::new(), |mut m, r| {
            *m.entry(r.session_id.as_str()).or_default() += 1;
            m
        });
        assert!(per_session.contains_key(id.to_string().as_str()));
        assert!(per_session.contains_key(other.to_string().as_str()));
        assert_eq!(
            rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
            (first + 1..=store.log_seq()).collect::<Vec<_>>()
        );

        // Orphans: marked rows leave the shipping view and appear in the orphan view.
        let n = store.log_orphan(other, 1).await.unwrap();
        assert!(n >= 1);
        assert!(
            store
                .log_after(first)
                .await
                .unwrap()
                .iter()
                .all(|r| r.session_id != other.to_string())
        );
        assert_eq!(store.log_orphans(other).await.unwrap().len(), n as usize);
        // Prune keeps orphans; pruning orphans by time removes them.
        let top = store.log_seq();
        store.prune_log_below(top + 1).await.unwrap();
        assert!(store.log_after(0).await.unwrap().is_empty());
        assert_eq!(store.log_orphans(other).await.unwrap().len(), n as usize);
        store.prune_orphans_before(u64::MAX).await.unwrap();
        assert!(store.log_orphans(other).await.unwrap().is_empty());

        // Cursors and fences upsert; a lower fence is ignored.
        store.set_cursor("o", "", 5).await.unwrap();
        store.set_cursor("o", "", 7).await.unwrap();
        store.set_cursor("o", "p", 3).await.unwrap();
        assert_eq!(
            store.cursors().await.unwrap(),
            [
                Cursor {
                    origin: "o".into(),
                    peer: "".into(),
                    seq: 7
                },
                Cursor {
                    origin: "o".into(),
                    peer: "p".into(),
                    seq: 3
                },
            ]
        );
        // (On `other`: a fence raised to a foreign owner also guards writes, see
        // `every_mutator_is_refused_behind_a_foreign_fence`.)
        assert!(store.set_fence(other, 3, "x").await.unwrap());
        assert!(!store.set_fence(other, 2, "y").await.unwrap());
        assert_eq!(store.fence(other).await.unwrap(), Some((3, "x".into())));

        // Apply: a write through `begin_apply` is not logged.
        let before = store.log_seq();
        let mut tx = store.begin_apply().await.unwrap();
        sqlx::query("UPDATE sessions SET title = 'applied' WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(store.log_seq(), before);
        assert_eq!(store.session(id).await.unwrap().unwrap().title, "applied");
        // …and capture is back on for the next ordinary write.
        store.set_title(id, "again").await.unwrap();
        assert_eq!(store.log_seq(), before + 1);
        drop(store);
    }

    // endregion: --- Capture (68 §3)
}
