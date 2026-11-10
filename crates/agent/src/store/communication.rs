//! Durable communication records (design doc 50 §2.3, §7, §8): root scope and
//! alias, peer exchanges (intake rows that double as receipts), remote peer
//! identity bindings and A2A context/task bindings. Only `Agents` and the
//! session loop write these; the A2A plugin reaches them through agent
//! commands. Every entry point fails with a [`CommError`] (downcast it) or a
//! plain SQL error; nothing here fabricates a success.
//!
//! Exchanges: `accepted` is committed intake, `consumed` links exactly one
//! history row (via [`append_in`], so revision/todos stay right), `replied`,
//! `suppressed`, `failed` and `unknown` are outcomes. Dedup is
//! `(sender, request_id)` plus a recipient/body/reply fingerprint: same →
//! existing receipt, different → [`CommError::Conflict`].
//!
//! Tasks: `status`/`error` columns are authoritative for lifecycle checks; the
//! SDK `snapshot` is the adapter's last projection. After
//! [`Store::fail_interrupted`] the snapshot lags until the adapter refreshes it
//! with [`Store::update_a2a_task`] at the unchanged terminal status.

use std::collections::HashSet;
use std::fmt;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use genai::chat::ChatMessage;
use runtime::SessionId;
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, SqliteConnection};
use uuid::Uuid;

use super::{Store, append_in};
use crate::communication::{
    ALIAS_MAX, PEER_TEXT_MAX, PENDING_PER_HARNESS, PENDING_PER_RECIPIENT, PeerMessageState,
    PeerReceipt, PeerRecord, PeerScope,
};

/// Pending intake payload budget per recipient (50 §8).
pub const PENDING_BYTES_PER_RECIPIENT: i64 = 2 << 20;
/// Pending intake payload budget across the harness (50 §8).
pub const PENDING_BYTES_PER_HARNESS: i64 = 32 << 20;
/// Bound on external identifiers (request ids, task/context ids, principal
/// and endpoint keys): opaque strings, never paths or UUID-checked.
pub const EXTERNAL_ID_MAX: usize = 256;
/// `list_a2a_tasks` page bound.
pub const TASK_PAGE_MAX: u16 = 100;
/// Ancestry walks stop here: deeper is a cycle in persisted topology.
const LINEAGE_MAX: i64 = 64;

/// Why a communication store operation refused. Unknown and unauthorized are
/// one answer (`NotFound`) so a principal cannot probe another's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommError {
    /// Unknown row, or one the caller is not a participant of.
    NotFound,
    /// `request_id` reused with a different recipient, body or reply target.
    Conflict,
    /// The row is terminal; the transition is refused.
    Terminal,
    /// Optimistic `revision` mismatch; `current` is the committed value.
    Stale {
        /// The committed revision.
        current: i64,
    },
    /// Pending-intake quota exceeded; the sender should retry later.
    Busy,
    /// Body or identifier over its bound.
    TooLarge,
    /// The admission generation is no longer live.
    Revoked,
    /// Another request scope is active in the exported session.
    ScopeBusy,
    /// Owned native work is still pending; the task cannot complete.
    WorkPending,
    /// Persisted ancestry is inconsistent (cycle or dangling parent).
    Topology(String),
    /// The session is not a root, or the argument is malformed.
    Invalid(String),
}

impl fmt::Display for CommError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Conflict => f.write_str("request id reused with different content or target"),
            Self::Terminal => f.write_str("already in a terminal state"),
            Self::Stale { current } => write!(f, "stale revision (now {current})"),
            Self::Busy => f.write_str("pending intake quota exceeded"),
            Self::TooLarge => f.write_str("body or identifier exceeds its bound"),
            Self::Revoked => f.write_str("communication generation revoked"),
            Self::ScopeBusy => f.write_str("another request scope is active in this session"),
            Self::WorkPending => f.write_str("owned work is still pending"),
            Self::Topology(why) => write!(f, "invalid session topology: {why}"),
            Self::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for CommError {}

/// Persisted ancestry of a session: `chain[0]` is the session, the last entry
/// its root, whose scope and alias follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lineage {
    /// The session first, then each parent up to the root.
    pub chain: Vec<SessionId>,
    /// The root's communication scope.
    pub scope: PeerScope,
    /// The root's alias, when set.
    pub alias: Option<String>,
}

impl Lineage {
    /// The root session, the last entry of `chain`.
    pub fn root(&self) -> SessionId {
        *self
            .chain
            .last()
            .expect("lineage has at least the session itself")
    }
}

/// Immutable remote identity behind an editable alias (50 §5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerBinding {
    /// Stable binding id; the `peer:<id>` endpoint key.
    pub id: Uuid,
    /// User-editable name.
    pub alias: String,
    /// Remote A2A endpoint URL.
    pub endpoint: String,
}

/// Input to [`Store::accept_exchange`]. `sender`/`recipient` are authenticated
/// endpoint keys (`session:<uuid>`, `peer:<binding-uuid>`), never message text.
/// `request_id` defaults to the minted message id. At least one of the
/// session fields must be local: that side owns the row.
#[derive(Clone, Debug)]
pub struct AcceptExchange<'a> {
    /// Authenticated sender endpoint key.
    pub sender: &'a str,
    /// Authenticated recipient endpoint key.
    pub recipient: &'a str,
    /// The sender when it is a local session.
    pub sender_session: Option<SessionId>,
    /// The recipient when it is a local session.
    pub recipient_session: Option<SessionId>,
    /// Sender-supplied idempotency key; the minted message id when absent.
    pub request_id: Option<&'a str>,
    /// The exchange this one answers.
    pub reply_to: Option<Uuid>,
    /// The request scope the exchange is bound to, when already known.
    pub request_scope: Option<Uuid>,
    /// Communication generation the admission was checked against.
    pub generation: i64,
    /// Bounded, already redacted text.
    pub body: &'a str,
}

/// Which A2A task an inbound request addresses (50 §6.3).
#[derive(Clone, Debug)]
pub enum A2aRequest<'a> {
    /// Server-minted `task_id` in an existing authorized `context_id`.
    New {
        /// Server-minted task id, unused so far.
        task_id: &'a str,
        /// An existing context the caller owns.
        context_id: &'a str,
        /// Initial SDK task snapshot.
        snapshot: serde_json::Value,
    },
    /// Existing authorized nonterminal task; `context_id`, when given, must agree.
    Resume {
        /// Existing nonterminal task the caller owns.
        task_id: &'a str,
        /// Must equal the task's context when given.
        context_id: Option<&'a str>,
    },
}

/// Authenticated principal plus the export it addresses; every A2A read and
/// write is scoped by both before lookup.
#[derive(Clone, Copy, Debug)]
pub struct A2aScope<'a> {
    /// Authenticated principal key (`peer:<binding-uuid>`).
    pub principal: &'a str,
    /// Exported session alias the request addresses.
    pub export: &'a str,
}

/// A2A task lifecycle mirrored beside the SDK snapshot for atomic checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A2aTaskStatus {
    /// Accepted, not yet started.
    Submitted,
    /// Running; holds the session's active scope.
    Working,
    /// Waiting on the caller; holds the scope.
    InputRequired,
    /// Waiting on authentication; holds the scope.
    AuthRequired,
    /// Terminal: succeeded.
    Completed,
    /// Terminal: failed.
    Failed,
    /// Terminal: canceled.
    Canceled,
    /// Terminal: refused before work started.
    Rejected,
}

impl A2aTaskStatus {
    /// No further transition is allowed.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Rejected
        )
    }

    /// Holds the session's single active request scope (50 §6.3).
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Working | Self::InputRequired | Self::AuthRequired
        )
    }

    /// The persisted / wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input_required",
            Self::AuthRequired => "auth_required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Rejected => "rejected",
        }
    }
}

impl std::str::FromStr for A2aTaskStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "submitted" => Self::Submitted,
            "working" => Self::Working,
            "input_required" => Self::InputRequired,
            "auth_required" => Self::AuthRequired,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            "rejected" => Self::Rejected,
            other => anyhow::bail!("bad task status {other:?}"),
        })
    }
}

/// An authorized A2A context: the caller's handle on one exported session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aContext {
    /// Caller-visible context id.
    pub id: String,
    /// Owning principal key.
    pub principal: String,
    /// Export the context addresses.
    pub export: String,
    /// The exported session.
    pub session: SessionId,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// One inbound A2A task. `seq` is the insertion order and the list cursor's
/// tie-breaker; `request_scope` is the local scope exchanges and work bind to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aTask {
    /// Insertion order.
    pub seq: i64,
    /// Caller-visible task id.
    pub id: String,
    /// The context the task was created in.
    pub context_id: String,
    /// Owning principal key.
    pub principal: String,
    /// Export the task addresses.
    pub export: String,
    /// The exported session doing the work.
    pub session: SessionId,
    /// Local request scope exchanges and owned work bind to.
    pub request_scope: Uuid,
    /// Lifecycle status mirrored beside `snapshot`.
    pub status: A2aTaskStatus,
    /// Optimistic-concurrency revision, bumped by every update.
    pub revision: i64,
    /// A cancel was requested and not yet settled.
    pub cancel_requested: bool,
    /// The SDK task snapshot as last written.
    pub snapshot: serde_json::Value,
    /// Failure reason once terminal by error.
    pub error: Option<String>,
    /// The exchange whose acceptance created the task.
    pub dedup_exchange: Option<Uuid>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last status/snapshot change; the list order.
    pub updated_at: DateTime<Utc>,
}

/// What [`Store::fail_interrupted`] settled.
#[derive(Clone, Debug, Default)]
pub struct Interrupted {
    /// Accepted intake that was failed.
    pub exchanges: Vec<PeerReceipt>,
    /// Nonterminal tasks that were failed.
    pub tasks: Vec<A2aTask>,
}

/// `ListTasks` filters (50 §6.2): every field narrows the caller's own scope,
/// none widens it. `after` is the previous page's cursor.
#[derive(Clone, Copy, Debug, Default)]
pub struct TaskQuery<'a> {
    /// Only tasks of this context.
    pub context: Option<&'a str>,
    /// Only tasks at this status.
    pub status: Option<A2aTaskStatus>,
    /// Only tasks whose last status change is after this instant.
    pub updated_after: Option<DateTime<Utc>>,
    /// Previous page's cursor.
    pub after: Option<&'a str>,
    /// Page size, clamped to `1..=TASK_PAGE_MAX`.
    pub limit: u16,
}

/// One `ListTasks` page.
#[derive(Clone, Debug, Default)]
pub struct TaskPage {
    /// The rows, most recently updated first.
    pub tasks: Vec<A2aTask>,
    /// Cursor for the next page; `None` at the end.
    pub next: Option<String>,
    /// Rows matching the filters across all pages.
    pub total: i64,
}

/// Projection literal for `ExchangeRow`; a macro so `concat!` can splice it into
/// `&'static str` SQL (sqlx 0.9 `SqlSafeStr`).
macro_rules! exchange_cols {
    () => {
        "id, sender, recipient, sender_session, recipient_session, session, \
         reply_to, request_scope, generation, body, state, reply, task_id, error, history_seq, \
         answered_by, created_at"
    };
}

#[derive(FromRow)]
struct ExchangeRow {
    id: String,
    sender: String,
    recipient: String,
    sender_session: Option<String>,
    recipient_session: Option<String>,
    session: String,
    reply_to: Option<String>,
    request_scope: Option<String>,
    generation: i64,
    body: String,
    state: String,
    reply: Option<String>,
    task_id: Option<String>,
    error: Option<String>,
    history_seq: Option<i64>,
    answered_by: Option<String>,
    created_at: String,
}

impl ExchangeRow {
    fn receipt(&self) -> anyhow::Result<PeerReceipt> {
        Ok(PeerReceipt {
            id: self.id.parse().context("bad exchange id")?,
            to: self.recipient.clone(),
            state: self.state()?,
            reply_to: parse_opt(self.reply_to.as_deref())?,
            reply: self.reply.clone(),
            task_id: self.task_id.clone(),
            error: self.error.clone(),
            answered_by: self.answered_by.clone(),
        })
    }

    fn state(&self) -> anyhow::Result<PeerMessageState> {
        self.state.parse().map_err(anyhow::Error::msg)
    }
}

impl TryFrom<ExchangeRow> for PeerRecord {
    type Error = anyhow::Error;
    fn try_from(r: ExchangeRow) -> anyhow::Result<Self> {
        let receipt = r.receipt()?;
        Ok(PeerRecord {
            receipt,
            from: r.sender,
            session: r.session.parse().context("bad session id")?,
            text: r.body,
            created_at: parse_time(&r.created_at)?,
            request_scope: parse_opt(r.request_scope.as_deref())?,
        })
    }
}

/// Projection literal for `TaskRow`; see `exchange_cols!`.
macro_rules! task_cols {
    () => {
        "seq, id, context_id, principal, export, session, request_scope, status, \
         revision, cancel_requested, snapshot, error, dedup_exchange, created_at, updated_at"
    };
}

#[derive(FromRow)]
struct TaskRow {
    seq: i64,
    id: String,
    context_id: String,
    principal: String,
    export: String,
    session: String,
    request_scope: String,
    status: String,
    revision: i64,
    cancel_requested: bool,
    snapshot: String,
    error: Option<String>,
    dedup_exchange: Option<String>,
    created_at: String,
    updated_at: String,
}

impl TryFrom<TaskRow> for A2aTask {
    type Error = anyhow::Error;
    fn try_from(r: TaskRow) -> anyhow::Result<Self> {
        Ok(A2aTask {
            seq: r.seq,
            id: r.id,
            context_id: r.context_id,
            principal: r.principal,
            export: r.export,
            session: r.session.parse().context("bad session id")?,
            request_scope: r.request_scope.parse().context("bad request scope")?,
            status: r.status.parse()?,
            revision: r.revision,
            cancel_requested: r.cancel_requested,
            snapshot: serde_json::from_str(&r.snapshot).context("bad task snapshot")?,
            error: r.error,
            dedup_exchange: parse_opt(r.dedup_exchange.as_deref())?,
            created_at: parse_time(&r.created_at)?,
            updated_at: parse_time(&r.updated_at)?,
        })
    }
}

#[derive(FromRow)]
struct ContextRow {
    id: String,
    principal: String,
    export: String,
    session: String,
    created_at: String,
}

impl TryFrom<ContextRow> for A2aContext {
    type Error = anyhow::Error;
    fn try_from(r: ContextRow) -> anyhow::Result<Self> {
        Ok(A2aContext {
            id: r.id,
            principal: r.principal,
            export: r.export,
            session: r.session.parse().context("bad session id")?,
            created_at: parse_time(&r.created_at)?,
        })
    }
}

fn parse_opt(s: Option<&str>) -> anyhow::Result<Option<Uuid>> {
    s.map(|s| s.parse().context("bad uuid")).transpose()
}

fn parse_time(s: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .context("bad timestamp")?
        .with_timezone(&Utc))
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// Recipient, body and reply target: what a retried `request_id` must repeat.
fn fingerprint(recipient: &str, body: &str, reply_to: Option<Uuid>) -> String {
    let mut h = Sha256::new();
    h.update(recipient.as_bytes());
    h.update([0]);
    h.update(body.as_bytes());
    h.update([0]);
    if let Some(r) = reply_to {
        h.update(r.as_bytes());
    }
    hex::encode(h.finalize())
}

fn bounded(name: &str, s: &str, max: usize) -> anyhow::Result<()> {
    if s.is_empty() {
        anyhow::bail!(CommError::Invalid(format!("{name} is empty")));
    }
    if s.len() > max {
        anyhow::bail!(CommError::TooLarge);
    }
    Ok(())
}

fn scope_ok(scope: A2aScope<'_>) -> anyhow::Result<()> {
    bounded("principal", scope.principal, EXTERNAL_ID_MAX)?;
    bounded("export", scope.export, EXTERNAL_ID_MAX)
}

async fn session_exists(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM sessions WHERE id = ?")
        .bind(sid)
        .fetch_optional(conn)
        .await?;
    Ok(row.is_some())
}

async fn exchange_in(conn: &mut SqliteConnection, id: &str) -> anyhow::Result<Option<ExchangeRow>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        exchange_cols!(),
        " FROM peer_exchanges WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

async fn task_in(
    conn: &mut SqliteConnection,
    scope: A2aScope<'_>,
    id: &str,
) -> anyhow::Result<Option<A2aTask>> {
    let row: Option<TaskRow> = sqlx::query_as(concat!(
        "SELECT ",
        task_cols!(),
        " FROM a2a_tasks WHERE id = ? AND principal = ? AND export = ?"
    ))
    .bind(id)
    .bind(scope.principal)
    .bind(scope.export)
    .fetch_optional(conn)
    .await?;
    row.map(A2aTask::try_from).transpose()
}

async fn context_in(
    conn: &mut SqliteConnection,
    scope: A2aScope<'_>,
    id: &str,
) -> anyhow::Result<Option<A2aContext>> {
    let row: Option<ContextRow> = sqlx::query_as(
        "SELECT id, principal, export, session, created_at FROM a2a_contexts \
         WHERE id = ? AND principal = ? AND export = ?",
    )
    .bind(id)
    .bind(scope.principal)
    .bind(scope.export)
    .fetch_optional(conn)
    .await?;
    row.map(A2aContext::try_from).transpose()
}

async fn task_unscoped_in(conn: &mut SqliteConnection, id: &str) -> anyhow::Result<A2aTask> {
    let row: Option<TaskRow> = sqlx::query_as(concat!(
        "SELECT ",
        task_cols!(),
        " FROM a2a_tasks WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    row.ok_or_else(|| anyhow::Error::from(CommError::NotFound))
        .and_then(A2aTask::try_from)
}

/// The acceptance transaction body (50 §7). Returns the receipt and whether
/// it is a dedup hit on an existing row. `id`: the message id when the
/// sender's engine already minted it (69 P: a peer message forwarded to the
/// owner keeps one id on both engines so `reply_to` matches); minted here
/// otherwise.
async fn accept_in(
    conn: &mut SqliteConnection,
    a: &AcceptExchange<'_>,
    id: Option<Uuid>,
) -> anyhow::Result<(PeerReceipt, bool)> {
    bounded("sender", a.sender, EXTERNAL_ID_MAX)?;
    bounded("recipient", a.recipient, EXTERNAL_ID_MAX)?;
    if a.body.len() > PEER_TEXT_MAX {
        anyhow::bail!(CommError::TooLarge);
    }
    let owner = a
        .recipient_session
        .or(a.sender_session)
        .ok_or_else(|| CommError::Invalid("no local participant".into()))?;
    let id = id.unwrap_or_else(Uuid::now_v7);
    let id_s = id.to_string();
    let request_id = match a.request_id {
        Some(r) => {
            bounded("request_id", r, EXTERNAL_ID_MAX)?;
            r
        }
        None => id_s.as_str(),
    };
    let fp = fingerprint(a.recipient, a.body, a.reply_to);

    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id, fingerprint FROM peer_exchanges WHERE sender = ? AND request_id = ?",
    )
    .bind(a.sender)
    .bind(request_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((eid, efp)) = existing {
        if efp != fp {
            anyhow::bail!(CommError::Conflict);
        }
        let row = exchange_in(conn, &eid).await?.ok_or(CommError::NotFound)?;
        let receipt = row.receipt()?;
        // 183 §6.4 (W-15): a message whose owner was out of reach is forwarded again on a
        // retry with the same `request_id`: back to `accepted`, not a dedup hit.
        let unreachable = receipt.state == PeerMessageState::Failed
            && receipt
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(crate::communication::mesh::UNREACHABLE_PREFIX));
        if unreachable {
            sqlx::query(
                "UPDATE peer_exchanges SET state = 'accepted', error = NULL, updated_at = ? \
                 WHERE id = ? AND state = 'failed'",
            )
            .bind(now())
            .bind(&eid)
            .execute(&mut *conn)
            .await?;
            let row = exchange_in(conn, &eid).await?.ok_or(CommError::NotFound)?;
            return Ok((row.receipt()?, false));
        }
        return Ok((receipt, true));
    }

    if !session_exists(conn, &owner.to_string()).await? {
        anyhow::bail!(CommError::NotFound);
    }

    // A reply: exact id, reversed authenticated endpoints, live original.
    let mut request_scope = a.request_scope;
    if let Some(reply_to) = a.reply_to {
        let orig = exchange_in(conn, &reply_to.to_string())
            .await?
            .filter(|o| o.recipient == a.sender && o.sender == a.recipient)
            .ok_or(CommError::NotFound)?;
        match orig.state()? {
            PeerMessageState::Accepted | PeerMessageState::Consumed => {}
            _ => anyhow::bail!(CommError::Terminal),
        }
        sqlx::query(
            "UPDATE peer_exchanges SET state = ?, reply = ?, answered_by = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(PeerMessageState::Replied.as_str())
        .bind(a.body)
        .bind(a.sender)
        .bind(now())
        .bind(&orig.id)
        .execute(&mut *conn)
        .await?;
        if request_scope.is_none() {
            request_scope = parse_opt(orig.request_scope.as_deref())?;
        }
    }

    if let Some(recipient) = a.recipient_session {
        let (n, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(body AS BLOB))), 0) FROM peer_exchanges \
             WHERE recipient_session = ? AND state = 'accepted'",
        )
        .bind(recipient.to_string())
        .fetch_one(&mut *conn)
        .await?;
        if n >= PENDING_PER_RECIPIENT as i64
            || bytes + a.body.len() as i64 > PENDING_BYTES_PER_RECIPIENT
        {
            anyhow::bail!(CommError::Busy);
        }
        let (n, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(body AS BLOB))), 0) FROM peer_exchanges \
             WHERE recipient_session IS NOT NULL AND state = 'accepted'",
        )
        .fetch_one(&mut *conn)
        .await?;
        if n >= PENDING_PER_HARNESS as i64
            || bytes + a.body.len() as i64 > PENDING_BYTES_PER_HARNESS
        {
            anyhow::bail!(CommError::Busy);
        }
    }

    let ts = now();
    sqlx::query(
        "INSERT INTO peer_exchanges (id, sender, recipient, sender_session, recipient_session, session, \
         request_id, fingerprint, reply_to, request_scope, generation, body, state, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'accepted', ?, ?)",
    )
    .bind(&id_s)
    .bind(a.sender)
    .bind(a.recipient)
    .bind(a.sender_session.map(|s| s.to_string()))
    .bind(a.recipient_session.map(|s| s.to_string()))
    .bind(owner.to_string())
    .bind(request_id)
    .bind(&fp)
    .bind(a.reply_to.map(|r| r.to_string()))
    .bind(request_scope.map(|r| r.to_string()))
    .bind(a.generation)
    .bind(a.body)
    .bind(&ts)
    .bind(&ts)
    .execute(&mut *conn)
    .await?;
    Ok((
        PeerReceipt {
            id,
            to: a.recipient.to_owned(),
            state: PeerMessageState::Accepted,
            reply_to: a.reply_to,
            reply: None,
            task_id: None,
            error: None,
            answered_by: None,
        },
        false,
    ))
}

/// Move `id` to a terminal-ish outcome from one of `from`; idempotent when
/// already at `to`, [`CommError::Terminal`] otherwise.
async fn finish_in(
    conn: &mut SqliteConnection,
    id: Uuid,
    from: &[PeerMessageState],
    to: PeerMessageState,
    task_id: Option<&str>,
    error: Option<&str>,
) -> anyhow::Result<PeerReceipt> {
    let ids = id.to_string();
    let row = exchange_in(conn, &ids).await?.ok_or(CommError::NotFound)?;
    let state = row.state()?;
    if state == to {
        return row.receipt();
    }
    if !from.contains(&state) {
        anyhow::bail!(CommError::Terminal);
    }
    let n = sqlx::query(
        "UPDATE peer_exchanges SET state = ?, task_id = COALESCE(?, task_id), \
         error = COALESCE(?, error), updated_at = ? WHERE id = ? AND state = ?",
    )
    .bind(to.as_str())
    .bind(task_id)
    .bind(error)
    .bind(now())
    .bind(&ids)
    .bind(state.as_str())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n != 1 {
        anyhow::bail!("exchange {id} changed underneath a settle");
    }
    exchange_in(conn, &ids)
        .await?
        .ok_or_else(|| anyhow::Error::from(CommError::NotFound))
        .and_then(|r| r.receipt())
}

/// Every `accepted` row whose sender or recipient lies in `subtree` (all rows
/// when `None`) becomes `suppressed` with `reason`.
async fn suppress_in(
    conn: &mut SqliteConnection,
    subtree: Option<SessionId>,
    reason: &str,
) -> anyhow::Result<Vec<PeerReceipt>> {
    let rows: Vec<ExchangeRow> = match subtree {
        None => {
            sqlx::query_as(concat!(
                "UPDATE peer_exchanges SET state = 'suppressed', error = ?, updated_at = ? \
                 WHERE state = 'accepted' RETURNING ",
                exchange_cols!()
            ))
            .bind(reason)
            .bind(now())
            .fetch_all(conn)
            .await?
        }
        Some(root) => {
            sqlx::query_as(concat!(
                "WITH RECURSIVE tree(id) AS ( \
                     SELECT id FROM sessions WHERE id = ?1 \
                     UNION ALL SELECT s.id FROM sessions s JOIN tree t ON s.parent = t.id) \
                 UPDATE peer_exchanges SET state = 'suppressed', error = ?2, updated_at = ?3 \
                 WHERE state = 'accepted' AND (sender_session IN tree OR recipient_session IN tree) \
                 RETURNING ",
                exchange_cols!()
            ))
            .bind(root.to_string())
            .bind(reason)
            .bind(now())
            .fetch_all(conn)
            .await?
        }
    };
    rows.iter().map(ExchangeRow::receipt).collect()
}

/// Fail every still-`accepted` intake row of a request scope when its task
/// goes terminal: queued input of a finished task never starts.
async fn fail_scope_in(
    conn: &mut SqliteConnection,
    scope: Uuid,
    reason: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE peer_exchanges SET state = 'failed', error = ?, updated_at = ? \
         WHERE request_scope = ? AND state = 'accepted'",
    )
    .bind(reason)
    .bind(now())
    .bind(scope.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// [`Store::bind_peer`]'s body inside the caller's transaction.
async fn bind_peer_in(
    conn: &mut SqliteConnection,
    alias: &str,
    endpoint: &str,
) -> anyhow::Result<PeerBinding> {
    bounded("alias", alias, ALIAS_MAX)?;
    bounded("endpoint", endpoint, EXTERNAL_ID_MAX)?;
    let live: Option<(String, String)> = sqlx::query_as(
        "SELECT id, endpoint FROM peer_bindings WHERE alias = ? AND revoked_at IS NULL",
    )
    .bind(alias)
    .fetch_optional(&mut *conn)
    .await?;
    let ts = now();
    if let Some((id, ep)) = live {
        if ep == endpoint {
            return Ok(PeerBinding {
                id: id.parse().context("bad binding id")?,
                alias: alias.to_owned(),
                endpoint: ep,
            });
        }
        sqlx::query("UPDATE peer_bindings SET revoked_at = ? WHERE id = ?")
            .bind(&ts)
            .bind(&id)
            .execute(&mut *conn)
            .await?;
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO peer_bindings (id, alias, endpoint, created_at) VALUES (?, ?, ?, ?)")
        .bind(id.to_string())
        .bind(alias)
        .bind(endpoint)
        .bind(&ts)
        .execute(&mut *conn)
        .await?;
    Ok(PeerBinding {
        id,
        alias: alias.to_owned(),
        endpoint: endpoint.to_owned(),
    })
}

impl Store {
    // ---- root scope and topology -------------------------------------------

    /// Persisted ancestry up to the root plus the root's scope and alias.
    /// `NotFound` for an unknown session; `Topology` for a cycle or a parent
    /// that no longer exists, so lazy resume cannot bypass inheritance (50 §2.1).
    pub async fn lineage(&self, id: SessionId) -> anyhow::Result<Lineage> {
        let rows: Vec<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
            "WITH RECURSIVE up(id, parent, scope, alias, depth) AS ( \
                 SELECT id, parent, scope, alias, 0 FROM sessions WHERE id = ?1 \
                 UNION ALL \
                 SELECT s.id, s.parent, s.scope, s.alias, up.depth + 1 \
                 FROM sessions s JOIN up ON s.id = up.parent WHERE up.depth < ?2) \
             SELECT id, parent, scope, alias FROM up ORDER BY depth",
        )
        .bind(id.to_string())
        .bind(LINEAGE_MAX)
        .fetch_all(&self.read)
        .await?;
        let Some((_, last_parent, scope, alias)) = rows.last() else {
            anyhow::bail!(CommError::NotFound);
        };
        let mut seen = HashSet::with_capacity(rows.len());
        let mut chain = Vec::with_capacity(rows.len());
        for (sid, _, _, _) in &rows {
            let sid: SessionId = sid.parse().context("bad session id")?;
            if !seen.insert(sid) {
                anyhow::bail!(CommError::Topology(format!("cycle through {sid}")));
            }
            chain.push(sid);
        }
        if let Some(p) = last_parent {
            anyhow::bail!(CommError::Topology(format!(
                "ancestry of {id} does not reach a root (parent {p} missing or cycle)"
            )));
        }
        Ok(Lineage {
            chain,
            scope: scope
                .parse()
                .map_err(|_| anyhow::anyhow!("bad scope {scope:?}"))?,
            alias: alias.clone(),
        })
    }

    /// Owner-only: set a root's scope and alias (`None` clears the alias).
    /// `Invalid` for a non-root, `NotFound` for an unknown session.
    pub async fn set_root_scope(
        &self,
        root: SessionId,
        scope: PeerScope,
        alias: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(a) = alias {
            bounded("alias", a, ALIAS_MAX)?;
        }
        let sid = root.to_string();
        let mut tx = self.begin_write().await?;
        let n =
            sqlx::query("UPDATE sessions SET scope = ?, alias = ? WHERE id = ? AND parent IS NULL")
                .bind(scope.as_str())
                .bind(alias)
                .bind(&sid)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if n != 1 {
            if session_exists(&mut tx, &sid).await? {
                anyhow::bail!(CommError::Invalid(format!("{root} is not a root session")));
            }
            anyhow::bail!(CommError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    /// 183 §6.6: one root's `(scope, alias)` as its row says now; `None` for no such root.
    pub async fn root_scope(
        &self,
        root: SessionId,
    ) -> anyhow::Result<Option<(PeerScope, Option<String>)>> {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT scope, alias FROM sessions WHERE id = ? AND parent IS NULL")
                .bind(root.to_string())
                .fetch_optional(&self.read)
                .await?;
        row.map(|(scope, alias)| {
            let scope = scope
                .parse()
                .map_err(|_| anyhow::anyhow!("bad scope {scope:?}"))?;
            Ok((scope, alias))
        })
        .transpose()
    }

    /// 183 §6.6: every `owner`-scope root whose alias is `alias` (a desk is
    /// `<agent>@<engine>`) with its owning engine (`""`: this one), newest first. Any mesh
    /// member writes a root's alias, so the caller keeps only the owner the `@engine` names.
    pub async fn root_by_alias(&self, alias: &str) -> anyhow::Result<Vec<(SessionId, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, owner FROM sessions WHERE parent IS NULL AND scope = 'owner' \
             AND alias = ? ORDER BY created_at DESC, id DESC",
        )
        .bind(alias)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(id, owner)| Ok((id.parse().context("bad session id")?, owner)))
            .collect()
    }

    /// Every root with its scope and alias, oldest first: the cross-root
    /// directory source. Callers apply scope/enablement before disclosure.
    pub async fn roots(&self) -> anyhow::Result<Vec<(SessionId, PeerScope, Option<String>)>> {
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, scope, alias FROM sessions WHERE parent IS NULL ORDER BY created_at, id",
        )
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(id, scope, alias)| {
                Ok((
                    id.parse().context("bad session id")?,
                    scope
                        .parse()
                        .map_err(|_| anyhow::anyhow!("bad scope {scope:?}"))?,
                    alias,
                ))
            })
            .collect()
    }

    // ---- remote peer identity bindings -------------------------------------

    /// The live binding id for `alias` at `endpoint`: reused when both match,
    /// minted (revoking the old row) when the endpoint changed or none is live.
    pub async fn bind_peer(&self, alias: &str, endpoint: &str) -> anyhow::Result<PeerBinding> {
        let mut tx = self.begin_write().await?;
        let b = bind_peer_in(&mut tx, alias, endpoint).await?;
        tx.commit().await?;
        Ok(b)
    }

    /// One reload's binding changes in one transaction (50 §6.1: nothing is
    /// partially installed): `revoke` aliases first, then every `(alias,
    /// endpoint)` bound as [`Store::bind_peer`] would, in order. Any failure
    /// leaves every row as it was.
    pub async fn commit_peer_bindings(
        &self,
        bind: &[(&str, &str)],
        revoke: &[&str],
    ) -> anyhow::Result<Vec<PeerBinding>> {
        let mut tx = self.begin_write().await?;
        let ts = now();
        for alias in revoke {
            sqlx::query(
                "UPDATE peer_bindings SET revoked_at = ? WHERE alias = ? AND revoked_at IS NULL",
            )
            .bind(&ts)
            .bind(alias)
            .execute(&mut *tx)
            .await?;
        }
        let mut out = Vec::with_capacity(bind.len());
        for (alias, endpoint) in bind {
            out.push(bind_peer_in(&mut tx, alias, endpoint).await?);
        }
        tx.commit().await?;
        Ok(out)
    }

    /// Revoke the live binding of `alias`; `Ok(false)` when none is live.
    pub async fn revoke_peer_binding(&self, alias: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE peer_bindings SET revoked_at = ? WHERE alias = ? AND revoked_at IS NULL",
        )
        .bind(now())
        .bind(alias)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// The live binding of `alias`, if any.
    pub async fn peer_binding(&self, alias: &str) -> anyhow::Result<Option<PeerBinding>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT id, endpoint FROM peer_bindings WHERE alias = ? AND revoked_at IS NULL",
        )
        .bind(alias)
        .fetch_optional(&self.read)
        .await?;
        row.map(|(id, endpoint)| {
            Ok(PeerBinding {
                id: id.parse().context("bad binding id")?,
                alias: alias.to_owned(),
                endpoint,
            })
        })
        .transpose()
    }

    /// The binding behind an authenticated id, live only (revoked → `None`).
    pub async fn peer_binding_by_id(&self, id: Uuid) -> anyhow::Result<Option<PeerBinding>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT alias, endpoint FROM peer_bindings WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(id.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row.map(|(alias, endpoint)| PeerBinding {
            id,
            alias,
            endpoint,
        }))
    }

    // ---- peer exchanges ----------------------------------------------------

    /// The acceptance transaction (50 §7): dedup by `(sender, request_id)`,
    /// reply correlation, quota, insert, commit. The receipt is returned only
    /// after commit; a duplicate returns the existing outcome, a changed
    /// duplicate is `Conflict`, an unknown/mismatched `reply_to` is `NotFound`.
    pub async fn accept_exchange(&self, a: &AcceptExchange<'_>) -> anyhow::Result<PeerReceipt> {
        let mut tx = self.begin_write().await?;
        let (receipt, _) = accept_in(&mut tx, a, None).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// [`Self::accept_exchange`] under a message id another engine minted
    /// (69 P: the owner's intake of a forwarded peer message). A duplicate
    /// `(sender, request_id)` still returns the existing row.
    pub async fn accept_exchange_as(
        &self,
        id: Uuid,
        a: &AcceptExchange<'_>,
    ) -> anyhow::Result<PeerReceipt> {
        let mut tx = self.begin_write().await?;
        let (receipt, _) = accept_in(&mut tx, a, Some(id)).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Consume `id` into `recipient`'s history: append `message` through the
    /// normal append path and link its seq, atomically. `Ok(None)` when there
    /// is nothing to consume (already linked, suppressed, failed): a repeated
    /// wakeup appends nothing. `NotFound` when `id` is not addressed to
    /// `recipient`; `Revoked` when `generation` is not the admitted one.
    pub async fn consume_exchange(
        &self,
        id: Uuid,
        recipient: SessionId,
        generation: i64,
        message: ChatMessage,
    ) -> anyhow::Result<Option<(i64, ChatMessage)>> {
        let ids = id.to_string();
        let sid = recipient.to_string();
        let mut tx = self.begin_write().await?;
        let row = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?;
        if row.recipient_session.as_deref() != Some(sid.as_str()) {
            anyhow::bail!(CommError::NotFound);
        }
        if row.history_seq.is_some() || row.state()? != PeerMessageState::Accepted {
            return Ok(None);
        }
        if row.generation != generation {
            anyhow::bail!(CommError::Revoked);
        }
        let seq = append_in(&mut tx, &sid, &message).await?;
        let n = sqlx::query(
            "UPDATE peer_exchanges SET state = 'consumed', history_seq = ?, updated_at = ? \
             WHERE id = ? AND state = 'accepted' AND history_seq IS NULL",
        )
        .bind(seq)
        .bind(now())
        .bind(&ids)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            anyhow::bail!("exchange {id} changed underneath consumption");
        }
        tx.commit().await?;
        Ok(Some((seq, message)))
    }

    /// 179: re-admit a still-`accepted` exchange under `generation` — a
    /// paused agent's message kept across a restart is delivered under the
    /// policy as it is now (the caller checked the route); the admission
    /// counter does not survive the restart. `false` when it is no longer
    /// `accepted`.
    pub async fn readmit_exchange(&self, id: Uuid, generation: i64) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE peer_exchanges SET generation = ?, updated_at = ? \
             WHERE id = ? AND state = 'accepted' AND history_seq IS NULL",
        )
        .bind(generation)
        .bind(now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// Suppress one still-`accepted` exchange (admission failed at intake).
    /// Idempotent when already suppressed; `Terminal` for any other state.
    pub async fn suppress_exchange(&self, id: Uuid, reason: &str) -> anyhow::Result<PeerReceipt> {
        let mut tx = self.begin_write().await?;
        let r = finish_in(
            &mut tx,
            id,
            &[PeerMessageState::Accepted],
            PeerMessageState::Suppressed,
            None,
            Some(reason),
        )
        .await?;
        tx.commit().await?;
        Ok(r)
    }

    /// Suppress every `accepted` exchange whose sender or recipient lies in
    /// the subtree rooted at `subtree` (whole harness when `None`), using
    /// persisted topology. Returns the affected receipts for notification.
    pub async fn suppress_exchanges(
        &self,
        subtree: Option<SessionId>,
        reason: &str,
    ) -> anyhow::Result<Vec<PeerReceipt>> {
        let mut tx = self.begin_write().await?;
        let r = suppress_in(&mut tx, subtree, reason).await?;
        tx.commit().await?;
        Ok(r)
    }

    /// The human answered request `original` on behalf of `by` (50 §5.3):
    /// `accepted|consumed` → `replied` with `text`, `answered_by =
    /// owner:<by>`; an already `replied` row whose answer travelled as
    /// `session:<by>` is relabelled the same way (the human typed it);
    /// anything else is `Terminal`. `NotFound` unless `by` is a participant.
    pub async fn owner_answer(
        &self,
        original: Uuid,
        by: SessionId,
        text: &str,
    ) -> anyhow::Result<PeerReceipt> {
        if text.len() > PEER_TEXT_MAX {
            anyhow::bail!(CommError::TooLarge);
        }
        let ids = original.to_string();
        let sid = by.to_string();
        let owner = format!("owner:{by}");
        let mut tx = self.begin_write().await?;
        let row = exchange_in(&mut tx, &ids)
            .await?
            .filter(|r| {
                r.sender_session.as_deref() == Some(sid.as_str())
                    || r.recipient_session.as_deref() == Some(sid.as_str())
            })
            .ok_or(CommError::NotFound)?;
        match row.state()? {
            PeerMessageState::Accepted | PeerMessageState::Consumed => {
                sqlx::query(
                    "UPDATE peer_exchanges SET state = 'replied', reply = ?, answered_by = ?, \
                     updated_at = ? WHERE id = ?",
                )
                .bind(text)
                .bind(&owner)
                .bind(now())
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            }
            PeerMessageState::Replied
                if row.answered_by.as_deref() == Some(format!("session:{by}").as_str()) =>
            {
                sqlx::query(
                    "UPDATE peer_exchanges SET answered_by = ?, updated_at = ? WHERE id = ?",
                )
                .bind(&owner)
                .bind(now())
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            }
            _ => anyhow::bail!(CommError::Terminal),
        }
        let r = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?
            .receipt()?;
        tx.commit().await?;
        Ok(r)
    }

    /// Record a definite failure (`accepted`/`consumed`/`unknown` → `failed`).
    pub async fn fail_exchange(&self, id: Uuid, error: &str) -> anyhow::Result<PeerReceipt> {
        let mut tx = self.begin_write().await?;
        let r = finish_in(
            &mut tx,
            id,
            &[
                PeerMessageState::Accepted,
                PeerMessageState::Consumed,
                PeerMessageState::Unknown,
            ],
            PeerMessageState::Failed,
            None,
            Some(error),
        )
        .await?;
        tx.commit().await?;
        Ok(r)
    }

    /// Remote outcome unresolved after bytes may have left (50 §7): keep any
    /// known task handle, require an explicit decision before resending.
    pub async fn mark_exchange_unknown(
        &self,
        id: Uuid,
        task_id: Option<&str>,
        error: &str,
    ) -> anyhow::Result<PeerReceipt> {
        if let Some(t) = task_id {
            bounded("task_id", t, EXTERNAL_ID_MAX)?;
        }
        let mut tx = self.begin_write().await?;
        let r = finish_in(
            &mut tx,
            id,
            &[PeerMessageState::Accepted, PeerMessageState::Consumed],
            PeerMessageState::Unknown,
            task_id,
            Some(error),
        )
        .await?;
        tx.commit().await?;
        Ok(r)
    }

    /// Attach the remote task handle a peer returned for an outbound send.
    /// `Terminal` once the exchange has settled.
    pub async fn bind_exchange_task(&self, id: Uuid, task_id: &str) -> anyhow::Result<PeerReceipt> {
        bounded("task_id", task_id, EXTERNAL_ID_MAX)?;
        let ids = id.to_string();
        let mut tx = self.begin_write().await?;
        let row = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?;
        match row.state()? {
            PeerMessageState::Accepted | PeerMessageState::Consumed | PeerMessageState::Unknown => {
            }
            _ => anyhow::bail!(CommError::Terminal),
        }
        sqlx::query("UPDATE peer_exchanges SET task_id = ?, updated_at = ? WHERE id = ?")
            .bind(task_id)
            .bind(now())
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        let r = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?
            .receipt()?;
        tx.commit().await?;
        Ok(r)
    }

    /// Current receipt of any exchange (harness-internal; not caller-scoped).
    pub async fn exchange_receipt(&self, id: Uuid) -> anyhow::Result<Option<PeerReceipt>> {
        let mut conn = self.read.acquire().await?;
        exchange_in(&mut conn, &id.to_string())
            .await?
            .map(|r| r.receipt())
            .transpose()
    }

    /// The owner-side record of any exchange (harness-internal; not
    /// caller-scoped): both participants of a receipt for notification.
    pub async fn exchange_record(&self, id: Uuid) -> anyhow::Result<Option<PeerRecord>> {
        let mut conn = self.read.acquire().await?;
        exchange_in(&mut conn, &id.to_string())
            .await?
            .map(PeerRecord::try_from)
            .transpose()
    }

    /// Consume `id` without a history row: the reply was returned to the
    /// recipient's awaiting `send_message` call instead of opening a turn
    /// (50 §5.3). Same admission checks as [`Store::consume_exchange`];
    /// `Ok(None)` when it is no longer `accepted`.
    pub async fn consume_exchange_direct(
        &self,
        id: Uuid,
        recipient: SessionId,
        generation: i64,
    ) -> anyhow::Result<Option<PeerReceipt>> {
        let ids = id.to_string();
        let mut tx = self.begin_write().await?;
        let row = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?;
        if row.recipient_session.as_deref() != Some(recipient.to_string().as_str()) {
            anyhow::bail!(CommError::NotFound);
        }
        if row.state()? != PeerMessageState::Accepted {
            return Ok(None);
        }
        if row.generation != generation {
            anyhow::bail!(CommError::Revoked);
        }
        sqlx::query(
            "UPDATE peer_exchanges SET state = 'consumed', updated_at = ? \
             WHERE id = ? AND state = 'accepted'",
        )
        .bind(now())
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
        let r = exchange_in(&mut tx, &ids)
            .await?
            .ok_or(CommError::NotFound)?
            .receipt()?;
        tx.commit().await?;
        Ok(Some(r))
    }

    /// A participant's own sent or received record (50 §5.2 `peer_message`);
    /// `None` for anything `caller` did not send or receive.
    pub async fn peer_record(
        &self,
        caller: SessionId,
        id: Uuid,
    ) -> anyhow::Result<Option<PeerRecord>> {
        let row: Option<ExchangeRow> = sqlx::query_as(concat!(
            "SELECT ",
            exchange_cols!(),
            " FROM peer_exchanges WHERE id = ?1 \
             AND (sender_session = ?2 OR recipient_session = ?2)"
        ))
        .bind(id.to_string())
        .bind(caller.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(PeerRecord::try_from).transpose()
    }

    /// A participant's own records, newest first, at most `limit`; `after`
    /// is an exclusive cursor (that row's acceptance order), an unknown or
    /// foreign cursor yields nothing rather than the top of the list.
    pub async fn peer_records(
        &self,
        caller: SessionId,
        after: Option<Uuid>,
        limit: u16,
    ) -> anyhow::Result<Vec<PeerRecord>> {
        let rows: Vec<ExchangeRow> = sqlx::query_as(concat!(
            "SELECT ",
            exchange_cols!(),
            " FROM peer_exchanges \
             WHERE (sender_session = ?1 OR recipient_session = ?1) \
             AND (?2 IS NULL OR accepted_seq < (SELECT accepted_seq FROM peer_exchanges \
                 WHERE id = ?2 AND (sender_session = ?1 OR recipient_session = ?1))) \
             ORDER BY accepted_seq DESC LIMIT ?3"
        ))
        .bind(caller.to_string())
        .bind(after.map(|a| a.to_string()))
        .bind(i64::from(limit.clamp(1, TASK_PAGE_MAX)))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(PeerRecord::try_from).collect()
    }

    /// `accepted` intake for `recipient` in acceptance order, at most `limit`.
    pub async fn pending_exchanges(
        &self,
        recipient: SessionId,
        limit: u16,
    ) -> anyhow::Result<Vec<PeerRecord>> {
        let rows: Vec<ExchangeRow> = sqlx::query_as(concat!(
            "SELECT ",
            exchange_cols!(),
            " FROM peer_exchanges \
             WHERE recipient_session = ? AND state = 'accepted' ORDER BY accepted_seq LIMIT ?"
        ))
        .bind(recipient.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(PeerRecord::try_from).collect()
    }

    /// 109 W3 (§7.2): the child results [`Self::fail_interrupted`] spared —
    /// `accepted` rows from a `session:*` sender with a `result:` request id
    /// — in acceptance order, for the boot pass that delivers them.
    pub async fn spared_child_results(&self) -> anyhow::Result<Vec<PeerRecord>> {
        let rows: Vec<ExchangeRow> = sqlx::query_as(concat!(
            "SELECT ",
            exchange_cols!(),
            " FROM peer_exchanges \
             WHERE state = 'accepted' AND sender LIKE 'session:%' AND request_id LIKE 'result:%' \
             ORDER BY accepted_seq"
        ))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(PeerRecord::try_from).collect()
    }

    /// Every exchange linked into `recipient`'s history, as `(history_seq,
    /// record)` in seq order: the provenance of peer-originated user rows
    /// when a transcript is loaded (50 §9).
    pub async fn consumed_exchanges(
        &self,
        recipient: SessionId,
    ) -> anyhow::Result<Vec<(i64, PeerRecord)>> {
        let rows: Vec<ExchangeRow> = sqlx::query_as(concat!(
            "SELECT ",
            exchange_cols!(),
            " FROM peer_exchanges \
             WHERE recipient_session = ? AND history_seq IS NOT NULL ORDER BY history_seq"
        ))
        .bind(recipient.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|r| {
                let seq = r.history_seq.unwrap_or_default();
                Ok((seq, PeerRecord::try_from(r)?))
            })
            .collect()
    }

    /// Restart accounting (50 §7): every `accepted` exchange fails with
    /// `reason`, every nonterminal task becomes `failed` keeping its snapshot
    /// and recording `reason`. Nothing is re-run. Narrowed by 109 §7.2: a
    /// child's finished result (`request_id = result:<turn>` from a
    /// `session:*` sender) stays `accepted` and is delivered on resume.
    pub async fn fail_interrupted(&self, reason: &str) -> anyhow::Result<Interrupted> {
        self.fail_interrupted_except(reason, &[]).await
    }

    /// [`Self::fail_interrupted`] sparing the exchanges addressed to `spare`
    /// (179 D6: a paused agent's sessions — they stay `accepted` and are
    /// delivered when the agent is un-paused).
    pub async fn fail_interrupted_except(
        &self,
        reason: &str,
        spare: &[SessionId],
    ) -> anyhow::Result<Interrupted> {
        let spare =
            serde_json::to_string(&spare.iter().map(|s| s.to_string()).collect::<Vec<_>>())?;
        let mut tx = self.begin_write().await?;
        let ts = now();
        let rows: Vec<ExchangeRow> = sqlx::query_as(concat!(
            "UPDATE peer_exchanges SET state = 'failed', error = ?, updated_at = ? \
             WHERE state = 'accepted' \
               AND NOT (sender LIKE 'session:%' AND request_id LIKE 'result:%') \
               AND (recipient_session IS NULL \
                    OR recipient_session NOT IN (SELECT value FROM json_each(?))) RETURNING ",
            exchange_cols!()
        ))
        .bind(reason)
        .bind(&ts)
        .bind(&spare)
        .fetch_all(&mut *tx)
        .await?;
        let exchanges = rows
            .iter()
            .map(ExchangeRow::receipt)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let tasks: Vec<TaskRow> = sqlx::query_as(concat!(
            "UPDATE a2a_tasks SET status = 'failed', error = ?, revision = revision + 1, updated_at = ? \
             WHERE status NOT IN ('completed', 'failed', 'canceled', 'rejected') RETURNING ",
            task_cols!()
        ))
        .bind(reason)
        .bind(&ts)
        .fetch_all(&mut *tx)
        .await?;
        let tasks = tasks
            .into_iter()
            .map(A2aTask::try_from)
            .collect::<anyhow::Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(Interrupted { exchanges, tasks })
    }

    // ---- A2A contexts and tasks --------------------------------------------

    /// 183 Z3-R4: drop context `id` of `scope` when no task names it — the one a request
    /// minted before its acceptance turned out to be a retry (dedup) of an earlier one.
    pub async fn drop_unused_a2a_context(
        &self,
        scope: A2aScope<'_>,
        id: &str,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "DELETE FROM a2a_contexts WHERE id = ?1 AND principal = ?2 AND export = ?3 \
             AND NOT EXISTS (SELECT 1 FROM a2a_tasks WHERE context_id = ?1)",
        )
        .bind(id)
        .bind(scope.principal)
        .bind(scope.export)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Tests: how many contexts `scope` has.
    #[cfg(test)]
    pub(crate) async fn a2a_context_count_for_test(&self, scope: A2aScope<'_>) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM a2a_contexts WHERE principal = ? AND export = ?")
            .bind(scope.principal)
            .bind(scope.export)
            .fetch_one(&self.read)
            .await
            .unwrap()
    }

    /// Mint a context for `(principal, export)` over `session` (50 §6.3).
    /// The caller passes the id so the SDK snapshot can name it.
    pub async fn create_a2a_context(
        &self,
        scope: A2aScope<'_>,
        id: &str,
        session: SessionId,
    ) -> anyhow::Result<A2aContext> {
        scope_ok(scope)?;
        bounded("context_id", id, EXTERNAL_ID_MAX)?;
        let sid = session.to_string();
        let mut tx = self.begin_write().await?;
        if !session_exists(&mut tx, &sid).await? {
            anyhow::bail!(CommError::NotFound);
        }
        let ts = now();
        let n = sqlx::query(
            "INSERT OR IGNORE INTO a2a_contexts (id, principal, export, session, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(scope.principal)
        .bind(scope.export)
        .bind(&sid)
        .bind(&ts)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            anyhow::bail!(CommError::Conflict);
        }
        tx.commit().await?;
        Ok(A2aContext {
            id: id.to_owned(),
            principal: scope.principal.to_owned(),
            export: scope.export.to_owned(),
            session,
            created_at: parse_time(&ts)?,
        })
    }

    /// A context the caller owns; `None` for unknown or another principal's.
    pub async fn a2a_context(
        &self,
        scope: A2aScope<'_>,
        id: &str,
    ) -> anyhow::Result<Option<A2aContext>> {
        let mut conn = self.read.acquire().await?;
        context_in(&mut conn, scope, id).await
    }

    /// 183 §7.6: the inbox session of `scope` (a contact and an agent export): the session
    /// of its oldest context whose session still exists; `None` before the first.
    pub async fn a2a_inbox(&self, scope: A2aScope<'_>) -> anyhow::Result<Option<SessionId>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT c.session FROM a2a_contexts c JOIN sessions s ON s.id = c.session \
             WHERE c.principal = ? AND c.export = ? ORDER BY c.created_at, c.id LIMIT 1",
        )
        .bind(scope.principal)
        .bind(scope.export)
        .fetch_optional(&self.read)
        .await?;
        row.map(|(s,)| {
            s.parse()
                .map_err(|e| anyhow::anyhow!("a2a_contexts.session {s}: {e}"))
        })
        .transpose()
    }

    /// 183 Z3-R2: mark the contexts of `session` as a contact's inbox (its default-deny
    /// limits come back at every actor start), speaking as `identity` (the exported agent's
    /// persona at this admission, Z3-R3; `None`: the engine's own).
    pub async fn mark_a2a_inbox(
        &self,
        session: SessionId,
        identity: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE a2a_contexts SET inbox = 1, inbox_identity = ? WHERE session = ?")
            .bind(identity)
            .bind(session.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 183 Z3-R2: `session` is a contact's inbox: `Some(its persona)` (`Some(None)`: the
    /// engine's own), `None` when it is not one.
    pub async fn a2a_inbox_identity(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Option<Option<String>>> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT inbox_identity FROM a2a_contexts WHERE session = ? AND inbox = 1 LIMIT 1",
        )
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row.map(|(a,)| a))
    }

    /// Inbound A2A acceptance in one transaction: the exchange is accepted
    /// (dedup as [`Store::accept_exchange`]) and bound to a new or resumed
    /// task in the caller's scope. `New` requires an owned context and an
    /// unused task id; `Resume` an owned nonterminal task whose context agrees
    /// with any supplied one. A dedup hit returns the existing exchange and
    /// its bound task. The exchange's recipient session is the context's.
    pub async fn accept_a2a_request(
        &self,
        scope: A2aScope<'_>,
        a: &AcceptExchange<'_>,
        request: A2aRequest<'_>,
    ) -> anyhow::Result<(PeerReceipt, A2aTask)> {
        scope_ok(scope)?;
        let mut tx = self.begin_write().await?;
        let mut a = a.clone();
        let existing_task: Option<A2aTask>;
        let request_scope = match &request {
            A2aRequest::New {
                task_id,
                context_id,
                ..
            } => {
                bounded("task_id", task_id, EXTERNAL_ID_MAX)?;
                bounded("context_id", context_id, EXTERNAL_ID_MAX)?;
                let ctx = context_in(&mut tx, scope, context_id)
                    .await?
                    .ok_or(CommError::NotFound)?;
                a.recipient_session = Some(ctx.session);
                existing_task = None;
                Uuid::now_v7()
            }
            A2aRequest::Resume {
                task_id,
                context_id,
            } => {
                bounded("task_id", task_id, EXTERNAL_ID_MAX)?;
                let task = task_in(&mut tx, scope, task_id)
                    .await?
                    .filter(|t| context_id.is_none_or(|c| c == t.context_id))
                    .ok_or(CommError::NotFound)?;
                if task.status.is_terminal() {
                    anyhow::bail!(CommError::Terminal);
                }
                a.recipient_session = Some(task.session);
                let scope_id = task.request_scope;
                existing_task = Some(task);
                scope_id
            }
        };
        a.request_scope = Some(request_scope);
        let (receipt, dedup) = accept_in(&mut tx, &a, None).await?;
        if dedup {
            // The existing outcome: the task bound to that acceptance.
            let row = exchange_in(&mut tx, &receipt.id.to_string())
                .await?
                .ok_or(CommError::NotFound)?;
            let bound = parse_opt(row.request_scope.as_deref())?.ok_or(CommError::NotFound)?;
            let task: Option<TaskRow> = sqlx::query_as(concat!(
                "SELECT ",
                task_cols!(),
                " FROM a2a_tasks WHERE request_scope = ? AND principal = ? AND export = ?"
            ))
            .bind(bound.to_string())
            .bind(scope.principal)
            .bind(scope.export)
            .fetch_optional(&mut *tx)
            .await?;
            let task = task
                .map(A2aTask::try_from)
                .transpose()?
                .ok_or(CommError::NotFound)?;
            return Ok((receipt, task));
        }
        let task = match (request, existing_task) {
            (_, Some(task)) => task,
            (
                A2aRequest::New {
                    task_id,
                    context_id,
                    snapshot,
                },
                None,
            ) => {
                let ts = now();
                let n = sqlx::query(
                    "INSERT OR IGNORE INTO a2a_tasks (id, context_id, principal, export, session, \
                     request_scope, status, snapshot, dedup_exchange, created_at, updated_at) \
                     SELECT ?1, ?2, ?3, ?4, session, ?5, 'submitted', ?6, ?7, ?8, ?8 \
                     FROM a2a_contexts WHERE id = ?2",
                )
                .bind(task_id)
                .bind(context_id)
                .bind(scope.principal)
                .bind(scope.export)
                .bind(request_scope.to_string())
                .bind(serde_json::to_string(&snapshot)?)
                .bind(receipt.id.to_string())
                .bind(&ts)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if n != 1 {
                    anyhow::bail!(CommError::Conflict);
                }
                task_in(&mut tx, scope, task_id)
                    .await?
                    .ok_or(CommError::NotFound)?
            }
            (A2aRequest::Resume { .. }, None) => anyhow::bail!("resume resolved no task"),
        };
        tx.commit().await?;
        Ok((receipt, task))
    }

    /// One task the caller owns; unknown and unauthorized are both `None`.
    pub async fn a2a_task(&self, scope: A2aScope<'_>, id: &str) -> anyhow::Result<Option<A2aTask>> {
        let mut conn = self.read.acquire().await?;
        task_in(&mut conn, scope, id).await
    }

    /// Caller-scoped page (`ListTasks`): filtered by principal/export and the
    /// query's context/status/time before pagination, most recently updated
    /// first (ties by `seq` descending). The cursor is the last row's
    /// `(updated_at, seq)`, so a task updated between pages moves to the top
    /// rather than shifting the rest. `total` counts every match.
    pub async fn list_a2a_tasks(
        &self,
        scope: A2aScope<'_>,
        q: TaskQuery<'_>,
    ) -> anyhow::Result<TaskPage> {
        let limit = q.limit.clamp(1, TASK_PAGE_MAX);
        let (after_at, after_seq): (Option<&str>, i64) = match q.after {
            None => (None, 0),
            Some(c) => {
                let bad = || CommError::Invalid(format!("bad cursor {c:?}"));
                let (at, seq) = c.split_once('|').ok_or_else(bad)?;
                parse_time(at).map_err(|_| bad())?;
                (Some(at), seq.parse().map_err(|_| bad())?)
            }
        };
        let status = q.status.map(A2aTaskStatus::as_str);
        // Same RFC 3339 text `now()` writes, so the comparison is what SQLite sorted.
        let updated_after = q.updated_after.map(|t| t.to_rfc3339());
        let (total,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM a2a_tasks WHERE principal = ?1 AND export = ?2 \
             AND (?3 IS NULL OR context_id = ?3) AND (?4 IS NULL OR status = ?4) \
             AND (?5 IS NULL OR updated_at > ?5)",
        )
        .bind(scope.principal)
        .bind(scope.export)
        .bind(q.context)
        .bind(status)
        .bind(updated_after.as_deref())
        .fetch_one(&self.read)
        .await?;
        // All parameters numbered: sqlx binds anonymous `?` by position and
        // `?N` by number, so mixing them misassigns later arguments.
        let mut rows: Vec<TaskRow> = sqlx::query_as(concat!(
            "SELECT ",
            task_cols!(),
            " FROM a2a_tasks WHERE principal = ?1 AND export = ?2 \
             AND (?3 IS NULL OR context_id = ?3) AND (?4 IS NULL OR status = ?4) \
             AND (?5 IS NULL OR updated_at > ?5) \
             AND (?6 IS NULL OR (updated_at, seq) < (?6, ?7)) \
             ORDER BY updated_at DESC, seq DESC LIMIT ?8"
        ))
        .bind(scope.principal)
        .bind(scope.export)
        .bind(q.context)
        .bind(status)
        .bind(updated_after.as_deref())
        .bind(after_at)
        .bind(after_seq)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.read)
        .await?;
        let limit = usize::from(limit);
        // The stored text verbatim, so the keyset compares what SQLite sorted.
        let next = (rows.len() > limit).then(|| {
            let last = &rows[limit - 1];
            format!("{}|{}", last.updated_at, last.seq)
        });
        rows.truncate(limit);
        let tasks = rows
            .into_iter()
            .map(A2aTask::try_from)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(TaskPage { tasks, next, total })
    }

    /// Optimistic lifecycle update: refuse a stale `expected_revision`, a
    /// transition out of a terminal status (a same-status snapshot refresh is
    /// allowed), completion with unsettled owned work, and entering an active
    /// status while another task holds the session's scope. Going terminal
    /// fails the scope's still-queued intake. All in one transaction.
    pub async fn update_a2a_task(
        &self,
        scope: A2aScope<'_>,
        id: &str,
        expected_revision: i64,
        status: A2aTaskStatus,
        snapshot: &serde_json::Value,
        error: Option<&str>,
    ) -> anyhow::Result<A2aTask> {
        let mut tx = self.begin_write().await?;
        let cur = task_in(&mut tx, scope, id)
            .await?
            .ok_or(CommError::NotFound)?;
        if cur.revision != expected_revision {
            anyhow::bail!(CommError::Stale {
                current: cur.revision
            });
        }
        if cur.status.is_terminal() && status != cur.status {
            anyhow::bail!(CommError::Terminal);
        }
        // A claimed completion or cancellation is only honest once every
        // owned work row settled (50 §8); `failed` (deadline, revoke) is not held.
        if matches!(status, A2aTaskStatus::Completed | A2aTaskStatus::Canceled)
            && !cur.status.is_terminal()
        {
            let (pending,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM a2a_task_work WHERE task_id = ? AND settled = 0",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if pending > 0 {
                anyhow::bail!(CommError::WorkPending);
            }
        }
        if status.is_active() && !cur.status.is_active() {
            let (other,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM a2a_tasks WHERE session = ? AND id <> ? \
                 AND status IN ('working', 'input_required', 'auth_required')",
            )
            .bind(cur.session.to_string())
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if other > 0 {
                anyhow::bail!(CommError::ScopeBusy);
            }
        }
        let n = sqlx::query(
            "UPDATE a2a_tasks SET status = ?, snapshot = ?, error = COALESCE(?, error), \
             revision = revision + 1, updated_at = ? WHERE id = ? AND revision = ?",
        )
        .bind(status.as_str())
        .bind(serde_json::to_string(snapshot)?)
        .bind(error)
        .bind(now())
        .bind(id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            anyhow::bail!(CommError::Stale {
                current: cur.revision
            });
        }
        if status.is_terminal() && !cur.status.is_terminal() {
            fail_scope_in(
                &mut tx,
                cur.request_scope,
                &format!("task {}", status.as_str()),
            )
            .await?;
        }
        let task = task_in(&mut tx, scope, id)
            .await?
            .ok_or(CommError::NotFound)?;
        tx.commit().await?;
        Ok(task)
    }

    /// Record a cancel request (idempotent) on a nonterminal owned task and
    /// return it; the caller settles queued input / signals active work and
    /// then publishes `Canceled` through [`Store::update_a2a_task`], which
    /// resolves the race against completion. `Terminal` when already settled.
    pub async fn request_a2a_cancel(
        &self,
        scope: A2aScope<'_>,
        id: &str,
    ) -> anyhow::Result<A2aTask> {
        let mut tx = self.begin_write().await?;
        let cur = task_in(&mut tx, scope, id)
            .await?
            .ok_or(CommError::NotFound)?;
        if cur.status.is_terminal() {
            anyhow::bail!(CommError::Terminal);
        }
        if !cur.cancel_requested {
            sqlx::query(
                "UPDATE a2a_tasks SET cancel_requested = 1, updated_at = ? \
                 WHERE id = ? AND status NOT IN ('completed', 'failed', 'canceled', 'rejected')",
            )
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        let task = task_in(&mut tx, scope, id)
            .await?
            .ok_or(CommError::NotFound)?;
        tx.commit().await?;
        Ok(task)
    }

    /// Bind native work (`turn:<TurnId>`, `child:<SessionId>`, …) to a
    /// nonterminal task scope; idempotent. `Terminal` once the task settled.
    pub async fn bind_a2a_work(&self, task_id: &str, key: &str) -> anyhow::Result<()> {
        bounded("key", key, EXTERNAL_ID_MAX)?;
        let mut tx = self.begin_write().await?;
        let task = task_unscoped_in(&mut tx, task_id).await?;
        if task.status.is_terminal() {
            anyhow::bail!(CommError::Terminal);
        }
        sqlx::query("INSERT OR IGNORE INTO a2a_task_work (task_id, key) VALUES (?, ?)")
            .bind(task_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Mark bound work settled; `Ok(false)` when it was not pending.
    pub async fn settle_a2a_work(&self, task_id: &str, key: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE a2a_task_work SET settled = 1 WHERE task_id = ? AND key = ? AND settled = 0",
        )
        .bind(task_id)
        .bind(key)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// The nonterminal task owning pending work `key`, if any (newest wins).
    pub async fn a2a_task_by_work(&self, key: &str) -> anyhow::Result<Option<A2aTask>> {
        // Work columns (task_id, key, settled) never clash with task_cols!.
        let row: Option<TaskRow> = sqlx::query_as(concat!(
            "SELECT ",
            task_cols!(),
            " FROM a2a_tasks t JOIN a2a_task_work w ON w.task_id = t.id \
             WHERE w.key = ? AND w.settled = 0 \
             AND t.status NOT IN ('completed', 'failed', 'canceled', 'rejected') \
             ORDER BY t.seq DESC LIMIT 1"
        ))
        .bind(key)
        .fetch_optional(&self.read)
        .await?;
        row.map(A2aTask::try_from).transpose()
    }

    /// The task holding `session`'s active request scope, if any.
    pub async fn active_a2a_task(&self, session: SessionId) -> anyhow::Result<Option<A2aTask>> {
        let row: Option<TaskRow> = sqlx::query_as(concat!(
            "SELECT ",
            task_cols!(),
            " FROM a2a_tasks WHERE session = ? \
             AND status IN ('working', 'input_required', 'auth_required') ORDER BY seq LIMIT 1"
        ))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(A2aTask::try_from).transpose()
    }

    /// The oldest `submitted` task of `session`: next in accepted order.
    pub async fn next_a2a_task(&self, session: SessionId) -> anyhow::Result<Option<A2aTask>> {
        let row: Option<TaskRow> = sqlx::query_as(concat!(
            "SELECT ",
            task_cols!(),
            " FROM a2a_tasks WHERE session = ? AND status = 'submitted' \
             ORDER BY seq LIMIT 1"
        ))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(A2aTask::try_from).transpose()
    }

    /// Every nonterminal task, oldest first, optionally only `session`'s:
    /// the set a revocation or shutdown must settle.
    pub async fn open_a2a_tasks(&self, session: Option<SessionId>) -> anyhow::Result<Vec<A2aTask>> {
        let rows: Vec<TaskRow> = sqlx::query_as(concat!(
            "SELECT ",
            task_cols!(),
            " FROM a2a_tasks WHERE (?1 IS NULL OR session = ?1) \
             AND status NOT IN ('completed', 'failed', 'canceled', 'rejected') ORDER BY seq"
        ))
        .bind(session.map(|s| s.to_string()))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(A2aTask::try_from).collect()
    }
}

/// Drop every communication row owned by `sid`; the session-deletion
/// transaction (50 §7: accounting lives and dies with its owning session).
pub(super) async fn delete_communication_in(
    conn: &mut SqliteConnection,
    sid: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM a2a_task_work WHERE task_id IN (SELECT id FROM a2a_tasks WHERE session = ?)",
    )
    .bind(sid)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM a2a_tasks WHERE session = ?")
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM peer_exchanges WHERE session = ?")
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM a2a_contexts WHERE session = ?")
        .bind(sid)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> (Store, SessionId, crate::TestDir) {
        let scratch = crate::TestDir::new("comm");
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

    fn err_is(e: &anyhow::Error, want: &CommError) -> bool {
        e.downcast_ref::<CommError>() == Some(want)
    }

    /// The pre-filter page shape the lifecycle tests were written against.
    async fn list(
        store: &Store,
        scope: A2aScope<'_>,
        context: Option<&str>,
        after: Option<&str>,
        limit: u16,
    ) -> anyhow::Result<(Vec<A2aTask>, Option<String>)> {
        let page = store
            .list_a2a_tasks(
                scope,
                TaskQuery {
                    context,
                    after,
                    limit,
                    ..TaskQuery::default()
                },
            )
            .await?;
        Ok((page.tasks, page.next))
    }

    impl Store {
        async fn pipe_list(
            &self,
            scope: A2aScope<'_>,
            context: Option<&str>,
            after: Option<&str>,
            limit: u16,
        ) -> anyhow::Result<(Vec<A2aTask>, Option<String>)> {
            list(self, scope, context, after, limit).await
        }
    }

    fn accept<'a>(
        from: &'a str,
        to: &'a str,
        from_s: Option<SessionId>,
        to_s: Option<SessionId>,
        body: &'a str,
    ) -> AcceptExchange<'a> {
        AcceptExchange {
            sender: from,
            recipient: to,
            sender_session: from_s,
            recipient_session: to_s,
            request_id: None,
            reply_to: None,
            request_scope: None,
            generation: 1,
            body,
        }
    }

    #[tokio::test]
    async fn dedup_returns_existing_and_conflicts_on_change() {
        let (store, a, _dir) = fresh().await;
        let b = SessionId::new();
        store
            .create_session(b, "m", Some(a), Some("b"), None)
            .await
            .unwrap();
        let (sa, sb) = (format!("session:{a}"), format!("session:{b}"));
        let mut req = accept(&sa, &sb, Some(a), Some(b), "hello");
        req.request_id = Some("r1");
        let first = store.accept_exchange(&req).await.unwrap();
        assert_eq!(first.state, PeerMessageState::Accepted);
        // Same sender/request/content: the same receipt, no second row.
        let again = store.accept_exchange(&req).await.unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(store.pending_exchanges(b, 10).await.unwrap().len(), 1);
        // Same key, changed body: conflict.
        let changed = AcceptExchange {
            body: "bye",
            ..req.clone()
        };
        let e = store.accept_exchange(&changed).await.unwrap_err();
        assert!(err_is(&e, &CommError::Conflict), "{e}");
        // Same key, changed target: conflict too.
        let moved = AcceptExchange {
            recipient: &sa,
            recipient_session: Some(a),
            ..req.clone()
        };
        let e = store.accept_exchange(&moved).await.unwrap_err();
        assert!(err_is(&e, &CommError::Conflict), "{e}");
        // Another sender may reuse the request id.
        let other = AcceptExchange {
            sender: &sb,
            sender_session: Some(b),
            recipient: &sa,
            recipient_session: Some(a),
            ..req.clone()
        };
        assert_ne!(store.accept_exchange(&other).await.unwrap().id, first.id);
    }

    #[tokio::test]
    async fn consume_links_history_once_and_is_idempotent() {
        let (store, a, _dir) = fresh().await;
        let b = SessionId::new();
        store
            .create_session(b, "m", Some(a), Some("b"), None)
            .await
            .unwrap();
        let (sa, sb) = (format!("session:{a}"), format!("session:{b}"));
        let r = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "hi"))
            .await
            .unwrap();
        let before = store.context_revision(b).await.unwrap();
        // Wrong generation: refused, nothing appended.
        let e = store
            .consume_exchange(r.id, b, 2, ChatMessage::user("hi"))
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::Revoked), "{e}");
        assert!(store.messages(b).await.unwrap().is_empty());
        // Wrong recipient: not found.
        let e = store
            .consume_exchange(r.id, a, 1, ChatMessage::user("hi"))
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        // First consumption appends through the normal path (revision bumps).
        let (seq, _) = store
            .consume_exchange(r.id, b, 1, ChatMessage::user("hi"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seq, 0);
        assert_ne!(store.context_revision(b).await.unwrap(), before);
        // Repeated wakeup: nothing to consume, no second row.
        assert!(
            store
                .consume_exchange(r.id, b, 1, ChatMessage::user("hi"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.messages(b).await.unwrap().len(), 1);
        let rec = store.peer_record(b, r.id).await.unwrap().unwrap();
        assert_eq!(rec.receipt.state, PeerMessageState::Consumed);
        assert_eq!(rec.from, sa);
        assert_eq!(rec.session, b);
        // The sender sees its own record; a stranger does not.
        assert!(store.peer_record(a, r.id).await.unwrap().is_some());
        let c = SessionId::new();
        store
            .create_session(c, "m", None, None, None)
            .await
            .unwrap();
        assert!(store.peer_record(c, r.id).await.unwrap().is_none());
        // Consumed cannot be suppressed; it can still fail.
        let e = store.suppress_exchange(r.id, "off").await.unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
    }

    #[tokio::test]
    async fn reply_requires_reversed_endpoints_and_live_original() {
        let (store, a, _dir) = fresh().await;
        let b = SessionId::new();
        let c = SessionId::new();
        store
            .create_session(b, "m", Some(a), Some("b"), None)
            .await
            .unwrap();
        store
            .create_session(c, "m", Some(a), Some("c"), None)
            .await
            .unwrap();
        let (sa, sb, sc) = (
            format!("session:{a}"),
            format!("session:{b}"),
            format!("session:{c}"),
        );
        let q = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "q?"))
            .await
            .unwrap();
        // c cannot answer a question addressed to b.
        let mut bad = accept(&sc, &sa, Some(c), Some(a), "me!");
        bad.reply_to = Some(q.id);
        let e = store.accept_exchange(&bad).await.unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        // b answers: original becomes replied with the reply text.
        let mut ok = accept(&sb, &sa, Some(b), Some(a), "42");
        ok.reply_to = Some(q.id);
        let r = store.accept_exchange(&ok).await.unwrap();
        assert_eq!(r.reply_to, Some(q.id));
        let orig = store.exchange_receipt(q.id).await.unwrap().unwrap();
        assert_eq!(orig.state, PeerMessageState::Replied);
        assert_eq!(orig.reply.as_deref(), Some("42"));
        // A second answer to the same id is refused: delivered once.
        let mut twice = accept(&sb, &sa, Some(b), Some(a), "43");
        twice.reply_to = Some(q.id);
        twice.request_id = Some("second");
        let e = store.accept_exchange(&twice).await.unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
    }

    #[tokio::test]
    async fn suppression_follows_subtree_and_restart_fails_pending() {
        let (store, root, _dir) = fresh().await;
        let kid = SessionId::new();
        let other = SessionId::new();
        store
            .create_session(kid, "m", Some(root), Some("k"), None)
            .await
            .unwrap();
        store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();
        let (sr, sk, so) = (
            format!("session:{root}"),
            format!("session:{kid}"),
            format!("session:{other}"),
        );
        let in_tree = store
            .accept_exchange(&accept(&sr, &sk, Some(root), Some(kid), "1"))
            .await
            .unwrap();
        let outbound = store
            .accept_exchange(&accept(&sk, &so, Some(kid), Some(other), "2"))
            .await
            .unwrap();
        let mut req = accept(&so, &sr, Some(other), Some(root), "4");
        req.request_id = Some("x");
        let inbound = store.accept_exchange(&req).await.unwrap();
        let far = SessionId::new();
        store
            .create_session(far, "m", None, None, None)
            .await
            .unwrap();
        let sf = format!("session:{far}");
        let untouched = store
            .accept_exchange(&accept(&so, &sf, Some(other), Some(far), "5"))
            .await
            .unwrap();
        // 109 §7.2: a child's finished result (`result:<turn>` from a
        // `session:*` sender) survives the restart; a `result:` request from a
        // non-session sender does not.
        let mut child_result = accept(&sf, &so, Some(far), Some(other), "done");
        child_result.request_id = Some("result:11111111-1111-1111-1111-111111111111");
        let spared = store.accept_exchange(&child_result).await.unwrap();
        let mut foreign_result = accept("a2a:peer", &so, None, Some(other), "done");
        foreign_result.request_id = Some("result:x");
        let failed_result = store.accept_exchange(&foreign_result).await.unwrap();

        let hit = store
            .suppress_exchanges(Some(kid), "opt-out")
            .await
            .unwrap();
        let ids: Vec<Uuid> = hit.iter().map(|r| r.id).collect();
        assert!(
            ids.contains(&in_tree.id) && ids.contains(&outbound.id),
            "{ids:?}"
        );
        assert!(!ids.contains(&inbound.id) && !ids.contains(&untouched.id));
        assert!(
            hit.iter().all(|r| r.state == PeerMessageState::Suppressed
                && r.error.as_deref() == Some("opt-out"))
        );
        // Suppressed input cannot be consumed later.
        assert!(
            store
                .consume_exchange(in_tree.id, kid, 1, ChatMessage::user("1"))
                .await
                .unwrap()
                .is_none()
        );
        // Restart: the rest of the accepted rows fail with the reason.
        let done = store
            .fail_interrupted("interrupted_by_restart")
            .await
            .unwrap();
        let ids: Vec<Uuid> = done.exchanges.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert!(ids.contains(&inbound.id) && ids.contains(&untouched.id));
        assert!(ids.contains(&failed_result.id) && !ids.contains(&spared.id));
        assert_eq!(
            store
                .exchange_receipt(spared.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            PeerMessageState::Accepted
        );
        assert_eq!(
            store
                .exchange_receipt(inbound.id)
                .await
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("interrupted_by_restart")
        );
        // Deleting the owner removes its accounting, not a neighbour's.
        store.delete_session(far).await.unwrap();
        assert!(
            store
                .exchange_receipt(untouched.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.exchange_receipt(inbound.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn lineage_and_root_scope() {
        let (store, root, _dir) = fresh().await;
        let kid = SessionId::new();
        let grandkid = SessionId::new();
        store
            .create_session(kid, "m", Some(root), Some("k"), None)
            .await
            .unwrap();
        store
            .create_session(grandkid, "m", Some(kid), Some("g"), None)
            .await
            .unwrap();
        let l = store.lineage(grandkid).await.unwrap();
        assert_eq!(l.chain, [grandkid, kid, root]);
        assert_eq!(l.root(), root);
        assert_eq!(l.scope, PeerScope::Isolated);
        store
            .set_root_scope(root, PeerScope::Owner, Some("main"))
            .await
            .unwrap();
        let l = store.lineage(kid).await.unwrap();
        assert_eq!(
            (l.scope, l.alias.as_deref()),
            (PeerScope::Owner, Some("main"))
        );
        // Not a root / unknown.
        let e = store
            .set_root_scope(kid, PeerScope::Owner, None)
            .await
            .unwrap_err();
        assert!(
            matches!(e.downcast_ref::<CommError>(), Some(CommError::Invalid(_))),
            "{e}"
        );
        let e = store.lineage(SessionId::new()).await.unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        // A fork is a fresh isolated root, whatever the source's scope.
        store.append(root, &ChatMessage::user("hi")).await.unwrap();
        let fork = store.fork(root, None, None).await.unwrap();
        let l = store.lineage(fork).await.unwrap();
        assert_eq!(
            (l.chain.as_slice(), l.scope, l.alias),
            ([fork].as_slice(), PeerScope::Isolated, None)
        );
        // Persisted cycle: detected, not looped.
        sqlx::query("UPDATE sessions SET parent = ? WHERE id = ?")
            .bind(grandkid.to_string())
            .bind(root.to_string())
            .execute(&store.write)
            .await
            .unwrap();
        let e = store.lineage(kid).await.unwrap_err();
        assert!(
            matches!(e.downcast_ref::<CommError>(), Some(CommError::Topology(_))),
            "{e}"
        );
    }

    #[tokio::test]
    async fn peer_bindings_are_stable_per_endpoint() {
        let (store, _, _dir) = fresh().await;
        let b1 = store
            .bind_peer("lab", "https://lab.example/a")
            .await
            .unwrap();
        assert_eq!(
            store
                .bind_peer("lab", "https://lab.example/a")
                .await
                .unwrap()
                .id,
            b1.id
        );
        let b2 = store
            .bind_peer("lab", "https://lab.example/b")
            .await
            .unwrap();
        assert_ne!(b2.id, b1.id);
        assert!(store.peer_binding_by_id(b1.id).await.unwrap().is_none());
        assert_eq!(store.peer_binding("lab").await.unwrap().unwrap().id, b2.id);
        assert!(store.revoke_peer_binding("lab").await.unwrap());
        assert!(!store.revoke_peer_binding("lab").await.unwrap());
        assert_ne!(
            store
                .bind_peer("lab", "https://lab.example/b")
                .await
                .unwrap()
                .id,
            b2.id
        );
    }

    #[tokio::test]
    async fn answers_record_who_answered() {
        let (store, a, _dir) = fresh().await;
        let b = SessionId::new();
        store
            .create_session(b, "m", Some(a), Some("b"), None)
            .await
            .unwrap();
        let (sa, sb) = (format!("session:{a}"), format!("session:{b}"));
        // A peer reply names the replying endpoint.
        let q1 = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "q1"))
            .await
            .unwrap();
        let mut reply = accept(&sb, &sa, Some(b), Some(a), "a1");
        reply.reply_to = Some(q1.id);
        store.accept_exchange(&reply).await.unwrap();
        let r = store.exchange_receipt(q1.id).await.unwrap().unwrap();
        assert_eq!(
            (r.state, r.answered_by.as_deref()),
            (PeerMessageState::Replied, Some(sb.as_str()))
        );
        // The human relabels an answer that travelled as `b`; a stranger cannot.
        let c = SessionId::new();
        store
            .create_session(c, "m", None, None, None)
            .await
            .unwrap();
        let e = store.owner_answer(q1.id, c, "x").await.unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        let r = store.owner_answer(q1.id, b, "ignored").await.unwrap();
        assert_eq!(r.reply.as_deref(), Some("a1"));
        assert_eq!(
            r.answered_by.as_deref(),
            Some(format!("owner:{b}").as_str())
        );
        // Relabelling again for the other side is Terminal: it was not `a`'s answer.
        let e = store.owner_answer(q1.id, a, "x").await.unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
        // A pending request answered by the human directly.
        let q2 = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "q2"))
            .await
            .unwrap();
        let r = store.owner_answer(q2.id, b, "typed").await.unwrap();
        assert_eq!(
            (r.state, r.reply.as_deref(), r.answered_by.as_deref()),
            (
                PeerMessageState::Replied,
                Some("typed"),
                Some(format!("owner:{b}").as_str())
            )
        );
    }

    #[tokio::test]
    async fn batched_binding_commit_is_all_or_nothing() {
        let (store, _, _dir) = fresh().await;
        let lab = store
            .bind_peer("lab", "https://lab.example/a")
            .await
            .unwrap();
        let old = store.bind_peer("old", "https://old.example").await.unwrap();
        // One bad alias (over ALIAS_MAX) fails the whole commit: `old` stays live,
        // nothing about `new` is minted.
        let long = "x".repeat(ALIAS_MAX + 1);
        let e = store
            .commit_peer_bindings(
                &[
                    ("new", "https://new.example"),
                    (&long, "https://bad.example"),
                ],
                &["old"],
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::TooLarge), "{e}");
        assert!(store.peer_binding("new").await.unwrap().is_none());
        assert_eq!(store.peer_binding("old").await.unwrap().unwrap().id, old.id);
        // A good commit: unchanged endpoint keeps its id, rotated endpoint mints,
        // revoked alias is gone, order follows the input.
        let out = store
            .commit_peer_bindings(
                &[
                    ("new", "https://new.example"),
                    ("lab", "https://lab.example/a"),
                ],
                &["old"],
            )
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].alias, "new");
        assert_eq!(out[1].id, lab.id);
        assert!(store.peer_binding("old").await.unwrap().is_none());
        let rotated = store
            .commit_peer_bindings(&[("lab", "https://lab.example/b")], &[])
            .await
            .unwrap();
        assert_ne!(rotated[0].id, lab.id);
        assert!(store.peer_binding_by_id(lab.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn direct_consumption_and_history_provenance() {
        let (store, a, _dir) = fresh().await;
        let b = SessionId::new();
        store
            .create_session(b, "m", Some(a), Some("b"), None)
            .await
            .unwrap();
        let (sa, sb) = (format!("session:{a}"), format!("session:{b}"));
        let first = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "one"))
            .await
            .unwrap();
        let second = store
            .accept_exchange(&accept(&sa, &sb, Some(a), Some(b), "two"))
            .await
            .unwrap();
        // A reply handed to an awaiting tool call: consumed, no history row.
        let e = store
            .consume_exchange_direct(first.id, b, 9)
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::Revoked), "{e}");
        let e = store
            .consume_exchange_direct(first.id, a, 1)
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        let r = store
            .consume_exchange_direct(first.id, b, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.state, PeerMessageState::Consumed);
        assert!(
            store
                .consume_exchange_direct(first.id, b, 1)
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.messages(b).await.unwrap().is_empty());
        // The other one enters history and shows up as provenance at its seq.
        let (seq, _) = store
            .consume_exchange(second.id, b, 1, ChatMessage::user("two"))
            .await
            .unwrap()
            .unwrap();
        let linked = store.consumed_exchanges(b).await.unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].0, seq);
        assert_eq!(linked[0].1.receipt.id, second.id);
        assert!(store.consumed_exchanges(a).await.unwrap().is_empty());
        // The unscoped owner view names the recipient as its session.
        let rec = store.exchange_record(second.id).await.unwrap().unwrap();
        assert_eq!((rec.session, rec.from.as_str()), (b, sa.as_str()));
        // Participant pages: newest first, exclusive cursor, foreign cursor empty.
        let page = store.peer_records(b, None, 10).await.unwrap();
        assert_eq!(
            page.iter().map(|r| r.receipt.id).collect::<Vec<_>>(),
            [second.id, first.id]
        );
        let page = store.peer_records(a, Some(second.id), 10).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].receipt.id, first.id);
        let stranger = SessionId::new();
        store
            .create_session(stranger, "m", None, None, None)
            .await
            .unwrap();
        assert!(
            store
                .peer_records(stranger, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .peer_records(b, Some(Uuid::now_v7()), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    fn snap(state: &str) -> serde_json::Value {
        serde_json::json!({ "status": { "state": state } })
    }

    #[tokio::test]
    async fn a2a_scope_isolation_and_terminal_races() {
        let (store, session, _dir) = fresh().await;
        let lab = A2aScope {
            principal: "peer:lab",
            export: "research",
        };
        let mallory = A2aScope {
            principal: "peer:mallory",
            export: "research",
        };
        let ctx = store
            .create_a2a_context(lab, "ctx-1", session)
            .await
            .unwrap();
        assert!(store.a2a_context(mallory, "ctx-1").await.unwrap().is_none());
        let e = store
            .create_a2a_context(lab, "ctx-1", session)
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::Conflict), "{e}");

        let sender = "peer:lab";
        let to = format!("session:{session}");
        let mut req = accept(sender, &to, None, None, "do research");
        req.request_id = Some("msg-1");
        let (receipt, task) = store
            .accept_a2a_request(
                lab,
                &req,
                A2aRequest::New {
                    task_id: "task-1",
                    context_id: &ctx.id,
                    snapshot: snap("submitted"),
                },
            )
            .await
            .unwrap();
        assert_eq!(task.status, A2aTaskStatus::Submitted);
        assert_eq!(task.dedup_exchange, Some(receipt.id));
        assert_eq!(task.session, session);
        let rec = store
            .peer_record(session, receipt.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.request_scope, Some(task.request_scope));
        // Retry of the same message: same exchange, same task, nothing new.
        let (r2, t2) = store
            .accept_a2a_request(
                lab,
                &req,
                A2aRequest::New {
                    task_id: "task-9",
                    context_id: &ctx.id,
                    snapshot: snap("submitted"),
                },
            )
            .await
            .unwrap();
        assert_eq!((r2.id, t2.id.as_str()), (receipt.id, "task-1"));
        assert!(store.a2a_task(lab, "task-9").await.unwrap().is_none());
        // Another principal: the task does not exist for it, in any operation.
        assert!(store.a2a_task(mallory, "task-1").await.unwrap().is_none());
        let e = store
            .request_a2a_cancel(mallory, "task-1")
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        let e = store
            .accept_a2a_request(
                mallory,
                &accept(sender, &to, None, None, "steal"),
                A2aRequest::Resume {
                    task_id: "task-1",
                    context_id: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        // A guessed unknown task id is not created.
        let e = store
            .accept_a2a_request(
                lab,
                &accept(sender, &to, None, None, "resume?"),
                A2aRequest::Resume {
                    task_id: "task-404",
                    context_id: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");
        // Mismatched context on resume is refused.
        let e = store
            .accept_a2a_request(
                lab,
                &accept(sender, &to, None, None, "resume?"),
                A2aRequest::Resume {
                    task_id: "task-1",
                    context_id: Some("ctx-other"),
                },
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::NotFound), "{e}");

        // Listing is scoped before pagination.
        let (page, next) = list(&store, mallory, None, None, 10).await.unwrap();
        assert!(page.is_empty() && next.is_none());
        let (page, next) = store.pipe_list(lab, Some(&ctx.id), None, 10).await.unwrap();
        assert_eq!(page.len(), 1);
        assert!(next.is_none());

        // Lifecycle: stale revision refused; working; owned work blocks completion.
        let e = store
            .update_a2a_task(
                lab,
                "task-1",
                7,
                A2aTaskStatus::Working,
                &snap("working"),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                e.downcast_ref::<CommError>(),
                Some(CommError::Stale { current: 0 })
            ),
            "{e}"
        );
        let task = store
            .update_a2a_task(
                lab,
                "task-1",
                0,
                A2aTaskStatus::Working,
                &snap("working"),
                None,
            )
            .await
            .unwrap();
        assert_eq!((task.status, task.revision), (A2aTaskStatus::Working, 1));
        assert_eq!(
            store.active_a2a_task(session).await.unwrap().unwrap().id,
            "task-1"
        );
        store.bind_a2a_work("task-1", "turn:t1").await.unwrap();
        assert_eq!(
            store.a2a_task_by_work("turn:t1").await.unwrap().unwrap().id,
            "task-1"
        );
        let e = store
            .update_a2a_task(
                lab,
                "task-1",
                1,
                A2aTaskStatus::Completed,
                &snap("completed"),
                None,
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::WorkPending), "{e}");
        // A second task in the same session cannot go active while the first holds the scope.
        let mut req2 = accept(sender, &to, None, None, "and this");
        req2.request_id = Some("msg-2");
        let (_, task2) = store
            .accept_a2a_request(
                lab,
                &req2,
                A2aRequest::New {
                    task_id: "task-2",
                    context_id: &ctx.id,
                    snapshot: snap("submitted"),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store.next_a2a_task(session).await.unwrap().unwrap().id,
            "task-2"
        );
        let e = store
            .update_a2a_task(
                lab,
                "task-2",
                task2.revision,
                A2aTaskStatus::Working,
                &snap("working"),
                None,
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::ScopeBusy), "{e}");

        // Cancel/complete race: cancel is requested, completion commits first,
        // the cancel publication then loses atomically.
        assert!(store.settle_a2a_work("task-1", "turn:t1").await.unwrap());
        let t = store.request_a2a_cancel(lab, "task-1").await.unwrap();
        assert!(t.cancel_requested);
        let done = store
            .update_a2a_task(
                lab,
                "task-1",
                t.revision,
                A2aTaskStatus::Completed,
                &snap("completed"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(done.status, A2aTaskStatus::Completed);
        let e = store
            .update_a2a_task(
                lab,
                "task-1",
                done.revision,
                A2aTaskStatus::Canceled,
                &snap("canceled"),
                None,
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
        let e = store.request_a2a_cancel(lab, "task-1").await.unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
        // Same-status snapshot refresh on a terminal task is allowed.
        store
            .update_a2a_task(
                lab,
                "task-1",
                done.revision,
                A2aTaskStatus::Completed,
                &snap("completed-final"),
                None,
            )
            .await
            .unwrap();
        // Terminal tasks never reopen through resume.
        let e = store
            .accept_a2a_request(
                lab,
                &accept(sender, &to, None, None, "more"),
                A2aRequest::Resume {
                    task_id: "task-1",
                    context_id: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err_is(&e, &CommError::Terminal), "{e}");
        assert!(store.a2a_task_by_work("turn:t1").await.unwrap().is_none());

        // Canceling task-2 while queued fails its still-accepted intake.
        let t2 = store.request_a2a_cancel(lab, "task-2").await.unwrap();
        let t2 = store
            .update_a2a_task(
                lab,
                "task-2",
                t2.revision,
                A2aTaskStatus::Canceled,
                &snap("canceled"),
                None,
            )
            .await
            .unwrap();
        let intake = store
            .exchange_receipt(t2.dedup_exchange.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(intake.state, PeerMessageState::Failed);
        assert_eq!(intake.error.as_deref(), Some("task canceled"));

        // Restart fails only nonterminal tasks and keeps the snapshot.
        let mut req3 = accept(sender, &to, None, None, "third");
        req3.request_id = Some("msg-3");
        store
            .accept_a2a_request(
                lab,
                &req3,
                A2aRequest::New {
                    task_id: "task-3",
                    context_id: &ctx.id,
                    snapshot: snap("submitted"),
                },
            )
            .await
            .unwrap();
        let done = store
            .fail_interrupted("interrupted_by_restart")
            .await
            .unwrap();
        assert_eq!(
            done.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["task-3"]
        );
        let t3 = store.a2a_task(lab, "task-3").await.unwrap().unwrap();
        assert_eq!(
            (t3.status, t3.error.as_deref()),
            (A2aTaskStatus::Failed, Some("interrupted_by_restart"))
        );
        assert_eq!(t3.snapshot, snap("submitted"));

        // Pagination: three tasks, most recently updated first (task-3 failed
        // last, task-2 canceled before it, task-1 completed first).
        let (p1, next) = list(&store, lab, None, None, 2).await.unwrap();
        assert_eq!(
            p1.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["task-3", "task-2"]
        );
        let (p2, end) = store
            .pipe_list(lab, None, next.as_deref(), 2)
            .await
            .unwrap();
        assert_eq!(
            p2.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["task-1"]
        );
        assert!(end.is_none());

        // Session deletion takes its tasks and contexts; a fork copies none.
        store
            .append(session, &ChatMessage::user("hi"))
            .await
            .unwrap();
        let fork = store.fork(session, None, None).await.unwrap();
        assert!(store.active_a2a_task(fork).await.unwrap().is_none());
        assert!(store.pending_exchanges(fork, 10).await.unwrap().is_empty());
        store.delete_session(session).await.unwrap();
        assert!(store.a2a_task(lab, "task-1").await.unwrap().is_none());
        assert!(store.a2a_context(lab, "ctx-1").await.unwrap().is_none());
        assert!(store.exchange_receipt(receipt.id).await.unwrap().is_none());
    }

    async fn new_task(store: &Store, session: SessionId, scope: A2aScope<'_>, ctx: &str, id: &str) {
        let to = format!("session:{session}");
        let mut req = accept(scope.principal, &to, None, None, id);
        req.request_id = Some(id);
        store
            .accept_a2a_request(
                scope,
                &req,
                A2aRequest::New {
                    task_id: id,
                    context_id: ctx,
                    snapshot: snap("submitted"),
                },
            )
            .await
            .unwrap();
    }

    fn ids(page: &[A2aTask]) -> Vec<&str> {
        page.iter().map(|t| t.id.as_str()).collect()
    }

    #[tokio::test]
    async fn a2a_task_listing_orders_by_activity_and_scopes_cursor() {
        let (store, session, _dir) = fresh().await;
        let lab = A2aScope {
            principal: "peer:lab",
            export: "research",
        };
        let mallory = A2aScope {
            principal: "peer:mallory",
            export: "research",
        };
        for (scope, ctx) in [(lab, "ctx-a"), (lab, "ctx-b"), (mallory, "ctx-m")] {
            store.create_a2a_context(scope, ctx, session).await.unwrap();
        }
        // Insertion order a1, a2, b1, m1; then a1 is touched, so activity
        // order for lab is a1, b1, a2 and seq order would be wrong.
        new_task(&store, session, lab, "ctx-a", "a1").await;
        new_task(&store, session, lab, "ctx-a", "a2").await;
        new_task(&store, session, lab, "ctx-b", "b1").await;
        new_task(&store, session, mallory, "ctx-m", "m1").await;
        store
            .update_a2a_task(lab, "a1", 0, A2aTaskStatus::Working, &snap("working"), None)
            .await
            .unwrap();

        // Another principal sees only its own rows, in any context filter.
        let (page, next) = list(&store, mallory, None, None, 10).await.unwrap();
        assert_eq!((ids(&page), next), (vec!["m1"], None));
        let (page, next) = store
            .pipe_list(mallory, Some("ctx-a"), None, 10)
            .await
            .unwrap();
        assert!(page.is_empty() && next.is_none());

        // No context: newest activity first, the cursor walks every row once.
        let (p1, next) = list(&store, lab, None, None, 2).await.unwrap();
        assert_eq!(ids(&p1), ["a1", "b1"]);
        let (p2, end) = store
            .pipe_list(lab, None, next.as_deref(), 2)
            .await
            .unwrap();
        assert_eq!((ids(&p2), end), (vec!["a2"], None));

        // Context filter applies before pagination and on every page.
        let (p1, next) = store.pipe_list(lab, Some("ctx-a"), None, 1).await.unwrap();
        assert_eq!(ids(&p1), ["a1"]);
        let (p2, end) = store
            .pipe_list(lab, Some("ctx-a"), next.as_deref(), 1)
            .await
            .unwrap();
        assert_eq!((ids(&p2), end), (vec!["a2"], None));
        let (page, next) = store.pipe_list(lab, Some("ctx-b"), None, 10).await.unwrap();
        assert_eq!((ids(&page), next), (vec!["b1"], None));

        // Status filter narrows the scope, `total` counts the whole match, and
        // the page cap applies even when the count is larger.
        let page = store
            .list_a2a_tasks(
                lab,
                TaskQuery {
                    status: Some(A2aTaskStatus::Working),
                    limit: 10,
                    ..TaskQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!((ids(&page.tasks), page.total), (vec!["a1"], 1));
        let page = store
            .list_a2a_tasks(
                lab,
                TaskQuery {
                    limit: 1,
                    ..TaskQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!((page.tasks.len(), page.total), (1, 3));
        assert!(page.next.is_some());
        // `updated_after` in the future matches nothing; far past matches all.
        let future = Utc::now() + chrono::Duration::hours(1);
        let page = store
            .list_a2a_tasks(
                lab,
                TaskQuery {
                    updated_after: Some(future),
                    limit: 10,
                    ..TaskQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!((page.tasks.len(), page.total), (0, 0));
        let page = store
            .list_a2a_tasks(
                lab,
                TaskQuery {
                    updated_after: Some(Utc::now() - chrono::Duration::hours(1)),
                    limit: 10,
                    ..TaskQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 3);

        // Malformed cursors are refused, not interpreted.
        for bad in ["7", "2026-01-01T00:00:00+00:00|x", "yesterday|1"] {
            let e = store.pipe_list(lab, None, Some(bad), 10).await.unwrap_err();
            assert!(
                matches!(e.downcast_ref(), Some(CommError::Invalid(_))),
                "{bad}: {e}"
            );
        }
    }
}
