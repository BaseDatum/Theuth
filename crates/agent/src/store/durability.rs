//! Design doc 109 §3: the durability journal on the session store —
//! `turn_steps`, `tool_calls`, `questions`, `waits`, `jobs`, `pty_screens`
//! (migration `0033_durability.sql`) and `resume_reports` (0035, §8.2).
//! Every write is one [`WriteTx`] or joins the caller's (`*_in`), so
//! `session_log` gets its row (§3.5). `resume_plan` turns the newest step
//! of an open turn into the [`ResumePlan`] §4.2 acts on. 139 §5.1: every
//! reader of open turns / jobs / child waits takes an explicit [`Scope`].

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use genai::chat::{ChatMessage, MessageContent, ToolCall};
use runtime::{SessionId, TurnId};
use serde_json::Value;
use sqlx::SqliteConnection;
use tools::{
    Answerer, JobExit, JobId, JobKind, JobRow, JobState, PtyScreenRow, QuestionKind, QuestionRow,
    QuestionState, Risk, ToolOutput, WaitKind, WaitRow,
};

use super::{InnerCall, Store, channel_in, insert_in, put_todos};
use crate::cost::{CallId, CallKind};
use crate::durability::{
    CallState, JournaledCall, LastStep, PartialContent, ResumeCause, ResumeKind, ResumePlan,
    ResumeReportRow, StepKind,
};
use crate::todo::{Todo, TodoSnapshot, TodoSource};

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn parse_dt(s: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .map_err(|e| anyhow::anyhow!("bad timestamp {s:?}: {e}"))?
        .with_timezone(&Utc))
}

fn parse_session(s: &str) -> anyhow::Result<SessionId> {
    s.parse()
        .map_err(|e| anyhow::anyhow!("bad session id {s:?}: {e}"))
}

fn parse_turn(s: &str) -> anyhow::Result<TurnId> {
    s.parse::<uuid::Uuid>()
        .map(TurnId)
        .map_err(|e| anyhow::anyhow!("bad turn id {s:?}: {e}"))
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// Append one `turn_steps` row inside the caller's transaction (§3.2).
/// `at_seq` is the session's last message seq at this moment (`-1` before
/// the first message); `n` follows the turn's previous row.
pub(crate) async fn step_in(
    conn: &mut SqliteConnection,
    session: SessionId,
    turn: TurnId,
    kind: StepKind,
    call_id: Option<&str>,
    payload: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO turn_steps (session_id, turn_id, n, kind, at_seq, call_id, payload, at) \
         SELECT ?1, ?2, COALESCE((SELECT MAX(n) FROM turn_steps WHERE session_id = ?1 AND turn_id = ?2), 0) + 1, \
                ?3, COALESCE((SELECT MAX(seq) FROM messages WHERE session_id = ?1), -1), ?4, ?5, ?6",
    )
    .bind(session.to_string())
    .bind(turn.to_string())
    .bind(kind.as_str())
    .bind(call_id)
    .bind(payload.to_string())
    .bind(now())
    .execute(conn)
    .await?;
    Ok(())
}

/// Delete the turn's `assistant_partial` rows inside the caller's
/// transaction: folded into the write that persists the final assistant row
/// (§8.1), so a finished model call leaves no partial behind.
pub(crate) async fn partial_delete_in(
    conn: &mut SqliteConnection,
    session: SessionId,
    turn: TurnId,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM turn_steps WHERE session_id = ? AND turn_id = ? AND kind = 'assistant_partial'",
    )
    .bind(session.to_string())
    .bind(turn.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// One `tool_calls` row per call in `planned`, inside the transaction that
/// persists the assistant `tool_use` row at `at_seq` (§3.3 step 5).
pub(crate) async fn calls_planned_in(
    conn: &mut SqliteConnection,
    session: SessionId,
    turn: TurnId,
    at_seq: i64,
    calls: &[(ToolCall, Risk)],
) -> anyhow::Result<()> {
    let sid = session.to_string();
    let tid = turn.to_string();
    for (call, risk) in calls {
        sqlx::query(
            "INSERT INTO tool_calls (session_id, call_id, turn_id, at_seq, name, args, risk, state) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'planned')",
        )
        .bind(&sid)
        .bind(&call.call_id)
        .bind(&tid)
        .bind(at_seq)
        .bind(&call.fn_name)
        .bind(call.fn_arguments.to_string())
        .bind(risk.as_str())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Every journal row of `sid`, for `delete_session` (§5.1: a deleted session
/// takes its rows; `pty_screens` cascade from `jobs`).
pub(crate) async fn delete_journal_in(
    conn: &mut SqliteConnection,
    sid: &str,
) -> anyhow::Result<()> {
    for table in [
        "waits",
        "questions",
        "tool_calls",
        "turn_steps",
        "jobs",
        "resume_reports",
    ] {
        // Names are this list's, never input.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE session_id = ?"
        )))
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// §3.2 `turn_finished` + §4.5 bookkeeping inside the caller's transaction:
/// the step (closing the turn), the turn's partials gone, and
/// `sessions.last_stop` / `finished_at` set.
pub(crate) async fn turn_finished_in(
    conn: &mut SqliteConnection,
    session: SessionId,
    turn: TurnId,
    stop_reason: &str,
) -> anyhow::Result<()> {
    partial_delete_in(conn, session, turn).await?;
    step_in(
        conn,
        session,
        turn,
        StepKind::TurnFinished,
        None,
        &serde_json::json!({ "stop_reason": stop_reason }),
    )
    .await?;
    sqlx::query("UPDATE sessions SET last_stop = ?, finished_at = ? WHERE id = ?")
        .bind(stop_reason)
        .bind(now())
        .bind(session.to_string())
        .execute(conn)
        .await?;
    Ok(())
}

/// §3.2: journal rows anchored at or above the session's `cut_at` go with
/// the messages a `commit_cut` removes (as `errors` / `inner_calls` do);
/// `waits` by the removed calls' ids. The caller's transaction.
pub(crate) async fn cut_journal_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<()> {
    // 139 Q8: a `child_result` wait outlives its turn (the spawn returned
    // long before the child finishes), so a compaction cut leaves it; 155
    // §5.4: a `peer_state` wait likewise.
    sqlx::query(
        "DELETE FROM waits WHERE session_id = ?1 AND kind NOT IN ('child_result', 'peer_state') AND tool_call_id IN \
         (SELECT call_id FROM tool_calls WHERE session_id = ?1 \
          AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1))",
    )
    .bind(sid)
    .execute(&mut *conn)
    .await?;
    for table in ["tool_calls", "turn_steps"] {
        // Names are this list's, never input.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE session_id = ?1 \
             AND at_seq >= (SELECT cut_at FROM sessions WHERE id = ?1)"
        )))
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// One call's row update at batch end ([`Store::append_batch`]): the
/// spill-capped text the model sees, or a `skipped` settlement.
pub struct BatchRow<'a> {
    /// Provider call id.
    pub call_id: &'a str,
    /// What the row's `output` becomes (the capped content; `ERROR:`-less).
    pub output: &'a str,
    /// Move a non-terminal row to `skipped` (cancel / landing, §3.3 step 9).
    pub skipped: bool,
    /// With `skipped`: the error flag.
    pub is_error: bool,
}

/// One `turn_steps` row as read.
struct StepRow {
    kind: String,
    call_id: Option<String>,
    payload: String,
    at: String,
}

/// 178 F1: a turn's run time from its journal rows (`n` order). The newest
/// `suspended` row carrying `ran_ms` is the drain's cumulative run clock
/// (`durability::run_clock`); the rows after it add their span — a run since
/// that resume which no drain measured (a crash). Rows with no `ran_ms`
/// anywhere (written before 178) fall back to the whole span, first to newest.
/// Downtime between a `suspended` row and the next run is never counted.
/// A `ran_ms` well above that whole span cannot have run (only a damaged
/// row carries one): past `max(5 s, 1 %)` of slack — the run clock is not
/// NTP-slewed and drifts from the wall clock, which may also step — it is
/// ignored for the span, so every caller's arithmetic gets a plausible value.
fn run_ms(session: SessionId, turn: TurnId, steps: &[StepRow]) -> anyhow::Result<u64> {
    let span_ms = |(first, last): (DateTime<Utc>, DateTime<Utc>)| {
        u64::try_from((last - first).num_milliseconds()).unwrap_or(0)
    };
    let mut ran = None;
    let mut whole: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    let mut span: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for s in steps {
        let at = parse_dt(&s.at)?;
        whole = Some(whole.map_or((at, at), |(first, _)| (first, at)));
        if s.kind == StepKind::Suspended.as_str()
            && let Some(ms) = json(&s.payload)["ran_ms"].as_u64()
        {
            ran = Some(ms);
            span = None;
            continue;
        }
        span = Some(span.map_or((at, at), |(first, _)| (first, at)));
    }
    let whole = whole.map_or(0, span_ms);
    match ran {
        None => Ok(whole),
        Some(ran) if ran > whole.saturating_add((whole / 100).max(5_000)) => {
            tracing::warn!(%session, %turn, ran_ms = ran, span_ms = whole, "178: impossible ran_ms ignored; run time is the journal span");
            Ok(whole)
        }
        Some(ran) => Ok(ran.saturating_add(span.map_or(0, span_ms))),
    }
}

type CallTuple = (
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    i64,
    String,
);

fn call_of(r: CallTuple) -> anyhow::Result<JournaledCall> {
    let (call_id, name, args, risk, state, attempt, started_at, output, is_error, files) = r;
    Ok(JournaledCall {
        call_id,
        name,
        args: json(&args),
        risk: risk
            .parse()
            .map_err(|e: String| anyhow::anyhow!("tool_calls.risk: {e}"))?,
        state: CallState::parse(&state)
            .ok_or_else(|| anyhow::anyhow!("tool_calls.state {state:?}"))?,
        attempt: u32::try_from(attempt).unwrap_or(1),
        started_at: started_at.as_deref().map(parse_dt).transpose()?,
        output,
        is_error: is_error != 0,
        files: serde_json::from_str::<Vec<PathBuf>>(&files).unwrap_or_default(),
    })
}

const CALL_COLS: &str =
    "call_id, name, args, risk, state, attempt, started_at, output, is_error, files";

type QuestionTuple = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn question_of(r: QuestionTuple) -> anyhow::Result<QuestionRow> {
    let (
        id,
        session,
        tool_call_id,
        kind,
        payload,
        state,
        asked_at,
        deadline,
        answered_at,
        answer,
        answered_by,
    ) = r;
    Ok(QuestionRow {
        id,
        session: parse_session(&session)?,
        tool_call_id,
        kind: QuestionKind::parse(&kind)
            .ok_or_else(|| anyhow::anyhow!("questions.kind {kind:?}"))?,
        payload: json(&payload),
        state: QuestionState::parse(&state)
            .ok_or_else(|| anyhow::anyhow!("questions.state {state:?}"))?,
        asked_at: parse_dt(&asked_at)?,
        deadline: parse_dt(&deadline)?,
        answered_at: answered_at.as_deref().map(parse_dt).transpose()?,
        answer: answer.as_deref().map(json),
        answered_by: answered_by.as_deref().and_then(Answerer::parse),
    })
}

const QUESTION_COLS: &str = "id, session_id, tool_call_id, kind, payload, state, asked_at, deadline, answered_at, answer, answered_by";

type WaitTuple = (String, String, String, String, Option<String>, String);

fn wait_of(r: WaitTuple) -> anyhow::Result<WaitRow> {
    let (session, tool_call_id, kind, target, deadline, payload) = r;
    Ok(WaitRow {
        session: parse_session(&session)?,
        tool_call_id,
        kind: WaitKind::parse(&kind).ok_or_else(|| anyhow::anyhow!("waits.kind {kind:?}"))?,
        target,
        deadline: deadline.as_deref().map(parse_dt).transpose()?,
        payload: json(&payload),
    })
}

type JobTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<i64>,
    String,
);

fn job_of(r: JobTuple) -> anyhow::Result<JobRow> {
    let (
        session,
        id,
        kind,
        call_id,
        command,
        name,
        pgid,
        log,
        started_ms,
        state,
        exit,
        settled_ms,
        engine,
    ) = r;
    Ok(JobRow {
        session: parse_session(&session)?,
        id: JobId(id),
        kind: JobKind::parse(&kind).ok_or_else(|| anyhow::anyhow!("jobs.kind {kind:?}"))?,
        call_id,
        command,
        name,
        pgid,
        log: log.map(PathBuf::from),
        started_ms,
        state: JobState::parse(&state).ok_or_else(|| anyhow::anyhow!("jobs.state {state:?}"))?,
        exit: exit
            .as_deref()
            .map(|e| {
                e.parse::<JobExit>()
                    .map_err(|e| anyhow::anyhow!("jobs.exit: {e}"))
            })
            .transpose()?,
        settled_ms,
        engine,
    })
}

const JOB_COLS: &str = "session_id, id, kind, call_id, command, name, pgid, log, started_ms, \
     state, exit, settled_ms, engine";

type ReportTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
);

fn report_of(session: SessionId, r: ReportTuple) -> anyhow::Result<ResumeReportRow> {
    let (turn, kind, interrupted_at, at, replayed, interrupted, lost_jobs, restored, handed, cause) =
        r;
    let kind =
        ResumeKind::parse(&kind).ok_or_else(|| anyhow::anyhow!("resume_reports.kind {kind:?}"))?;
    Ok(ResumeReportRow {
        session,
        turn: parse_turn(&turn)?,
        kind,
        // 139 §10: `''` is a pre-139 row; the boot rule without a reason.
        cause: ResumeCause::parse(&cause).unwrap_or(match kind {
            ResumeKind::Interrupted => ResumeCause::Crash,
            ResumeKind::Suspended => ResumeCause::Shutdown,
        }),
        interrupted_at: parse_dt(&interrupted_at)?,
        at: parse_dt(&at)?,
        report: crate::ResumeReport {
            replayed: serde_json::from_str(&replayed)?,
            interrupted: serde_json::from_str(&interrupted)?,
            lost_jobs: serde_json::from_str(&lost_jobs)?,
            children_restored: u32::try_from(restored).unwrap_or(u32::MAX),
            children_handed_off: u32::try_from(handed).unwrap_or(u32::MAX),
        },
    })
}

const REPORT_COLS: &str = "turn_id, kind, interrupted_at, at, replayed, interrupted, lost_jobs, \
     children_restored, children_handed_off, cause";

/// Which sessions a journal reader is asked about (139 §3, D1). Never
/// implicit: there is no `Default`, so a new global consumer fails to
/// compile rather than slipping past review (I9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Sessions `here`: `owner = '' OR owner = me` (Revision 1 Q1).
    Here,
    /// Every session in the file, whoever owns it.
    All,
}

impl Scope {
    /// The predicate over `s`, an alias of `sessions` joined on the row's
    /// session. `?1` is `Store::engine()`, bound by the caller for both
    /// scopes (`All` only references it so the bind count matches).
    const fn sql(self) -> &'static str {
        match self {
            Self::Here => "(s.owner = '' OR s.owner = ?1)",
            Self::All => "(?1 IS NOT NULL)",
        }
    }
}

/// One open turn as `open_turns` lists it: `(session, turn, last activity)`.
pub type OpenTurnRow = (SessionId, TurnId, DateTime<Utc>);

/// The window that picks each turn's newest non-partial step and its last
/// activity time (`last_at` includes partial rows). 146 D5: one window pass
/// over the table — partial rows sort last inside their partition, so
/// `rn = 1` is the newest non-partial step and `MAX(at)` sees every row —
/// instead of a correlated `MAX(at)` subquery per row (1.1 s → 0.08 s over
/// 25k rows; migration 0042 indexes the per-turn `at` lookups).
const NEWEST: &str = "SELECT session_id, turn_id, kind, \
     ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY (kind = 'assistant_partial'), n DESC) AS rn, \
     MAX(at) OVER (PARTITION BY session_id, turn_id) AS last_at \
     FROM turn_steps";

/// 146 D1/D8: the mid-prompt predicate over `m`, a `messages` row (the
/// session's last): a prompt the harness never answered (`User`), a
/// `ToolCall` without its `Tool` response, or a `Tool` response the model
/// never answered. An idle child ends on `Assistant` text; a lost one on a
/// `System` note.
/// 146 (141 coordination): the `sessions.kind` values of ACP children this
/// engine drives — the respawn trigger set and the ACP journal apply to these
/// only. 141's watch-only harness-internal children (`SessionKind::Observed`,
/// never prompted by us) and 125's advisors are outside it by construction.
pub const DRIVEN_KINDS: &str = "('chat')";

const MIDPROMPT: &str = "(json_extract(m.body, '$.role') IN ('User', 'Tool') \
     OR (json_extract(m.body, '$.role') = 'Assistant' AND EXISTS \
         (SELECT 1 FROM json_each(m.body, '$.content') WHERE json_type(value, '$.ToolCall') IS NOT NULL)))";

/// 146 D10/D11: `text` without its leading `<time …/>` line, whitespace
/// collapsed, cut to `max` chars with an ellipsis.
pub fn excerpt(text: &str, max: usize) -> String {
    let body = text
        .trim_start()
        .strip_prefix("<time")
        .and_then(|rest| rest.split_once("/>"))
        .map_or(text, |(_, after)| after);
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

impl Store {
    /// [`step_in`] in its own transaction: the `suspended` row (§9), tests,
    /// and any step not piggy-backed on an existing write.
    pub async fn step(
        &self,
        session: SessionId,
        turn: TurnId,
        kind: StepKind,
        call_id: Option<&str>,
        payload: &Value,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        step_in(&mut tx, session, turn, kind, call_id, payload).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert the one `assistant_partial` row of model call `call_id` (§8.1):
    /// updated in place, never appended, so a streaming session costs one
    /// short changeset per checkpoint.
    pub async fn partial_upsert(
        &self,
        session: SessionId,
        turn: TurnId,
        call_id: &str,
        content: &MessageContent,
        thinking_ms: Option<u64>,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({ "content": content, "thinking_ms": thinking_ms });
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE turn_steps SET payload = ?, at = ?, \
                 at_seq = COALESCE((SELECT MAX(seq) FROM messages WHERE session_id = ?3), -1) \
             WHERE session_id = ?3 AND turn_id = ?4 AND kind = 'assistant_partial' AND call_id = ?5",
        )
        .bind(payload.to_string())
        .bind(now())
        .bind(session.to_string())
        .bind(turn.to_string())
        .bind(call_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n == 0 {
            step_in(
                &mut tx,
                session,
                turn,
                StepKind::AssistantPartial,
                Some(call_id),
                &payload,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// `tool_calls → started` (§3.3 step 6). `false` when no `planned` /
    /// `started` row exists: calls the loop never planned (app invocations,
    /// RPC-started jobs, inner calls) leave no journal.
    pub async fn call_started(&self, session: SessionId, call_id: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE tool_calls SET state = 'started', started_at = ? \
             WHERE session_id = ? AND call_id = ? AND state IN ('planned', 'started')",
        )
        .bind(now())
        .bind(session.to_string())
        .bind(call_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// `tool_calls → finished` with the output as returned (§3.3 step 7).
    /// `false` when the row is missing or already terminal.
    pub async fn call_finished(
        &self,
        session: SessionId,
        call_id: &str,
        out: &ToolOutput,
    ) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE tool_calls SET state = 'finished', finished_at = ?, output = ?, is_error = ?, files = ? \
             WHERE session_id = ? AND call_id = ? AND state IN ('planned', 'started')",
        )
        .bind(now())
        .bind(&out.content)
        .bind(i64::from(out.is_error))
        .bind(serde_json::to_string(&out.files)?)
        .bind(session.to_string())
        .bind(call_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// `tool_calls → interrupted` with the §4.3 note as its error output: a
    /// `started` mutating call a restart must not repeat.
    pub async fn call_interrupted(
        &self,
        session: SessionId,
        call_id: &str,
        note: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE tool_calls SET state = 'interrupted', finished_at = ?, output = ?, is_error = 1 \
             WHERE session_id = ? AND call_id = ? AND state IN ('planned', 'started')",
        )
        .bind(now())
        .bind(note)
        .bind(session.to_string())
        .bind(call_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Every `tool_calls` row of `turn`, in `at_seq`, rowid order: what the
    /// batch message is assembled from (§3.3 step 8).
    pub async fn calls_of(
        &self,
        session: SessionId,
        turn: TurnId,
    ) -> anyhow::Result<Vec<JournaledCall>> {
        let rows: Vec<CallTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {CALL_COLS} FROM tool_calls WHERE session_id = ? AND turn_id = ? ORDER BY at_seq, rowid"
        )))
        .bind(session.to_string())
        .bind(turn.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(call_of).collect()
    }

    /// Every turn whose newest step is not `turn_finished`, most recently
    /// active first (§4.6 step 10's `resume_scan`). `suspended` is open.
    /// `Scope::Here` keeps the turns of sessions this engine owns (139 D1):
    /// a replica never lists the owner's live turn as its own to act on.
    pub async fn open_turns(&self, scope: Scope) -> anyhow::Result<Vec<OpenTurnRow>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT o.session_id, o.turn_id, o.last_at FROM ({NEWEST}) o \
             JOIN sessions s ON s.id = o.session_id \
             WHERE o.rn = 1 AND o.kind != 'turn_finished' AND {} ORDER BY o.last_at DESC",
            scope.sql()
        )))
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(s, t, at)| Ok((parse_session(&s)?, parse_turn(&t)?, parse_dt(&at)?)))
            .collect()
    }

    /// The one session's open turn, whoever owns it (139 §5.1 items 4, 12,
    /// 14: a display or per-session read; the caller decides routing).
    pub async fn open_turn(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Option<(TurnId, DateTime<Utc>)>> {
        let row: Option<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT turn_id, last_at FROM ({NEWEST}) \
             WHERE rn = 1 AND kind != 'turn_finished' AND session_id = ? \
             ORDER BY last_at DESC LIMIT 1"
        )))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(|(t, at)| Ok((parse_turn(&t)?, parse_dt(&at)?)))
            .transpose()
    }

    /// 178 W1/F1: `turn`'s run time before an interruption ([`run_ms`] over
    /// its rows, `n` order); only rows at or before `until` when given (a
    /// resumed turn's own new rows never count). 0 without rows.
    pub async fn turn_run_ms(
        &self,
        session: SessionId,
        turn: TurnId,
        until: Option<DateTime<Utc>>,
    ) -> anyhow::Result<u64> {
        let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT kind, call_id, payload, at FROM turn_steps \
             WHERE session_id = ? AND turn_id = ? ORDER BY n",
        )
        .bind(session.to_string())
        .bind(turn.to_string())
        .fetch_all(&self.read)
        .await?;
        let mut steps = Vec::with_capacity(rows.len());
        for (kind, call_id, payload, at) in rows {
            if until.is_some_and(|u| parse_dt(&at).is_ok_and(|a| a > u)) {
                continue;
            }
            steps.push(StepRow {
                kind,
                call_id,
                payload,
                at,
            });
        }
        run_ms(session, turn, &steps)
    }

    /// Test seam (178): set the `at` of step `n` of `turn`, to journal a
    /// turn that ran before a restart.
    #[cfg(test)]
    pub async fn set_step_at(
        &self,
        session: SessionId,
        turn: TurnId,
        n: i64,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE turn_steps SET at = ? WHERE session_id = ? AND turn_id = ? AND n = ?")
            .bind(at.to_rfc3339())
            .bind(session.to_string())
            .bind(turn.to_string())
            .bind(n)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 146 D9: the ACP child's journal. An ACP child owes work from the
    /// moment the supervisor hands it a prompt (or its harness opens a turn)
    /// until that turn ends; one `input_accepted` row marks it, so
    /// `open_turns` lists a mid-prompt child like a native open turn. At
    /// most one open turn per child: a second prompt while one is open
    /// writes nothing. `Ok(Some(turn))` when a row was written.
    pub async fn acp_turn_open(
        &self,
        session: SessionId,
        payload: &Value,
    ) -> anyhow::Result<Option<TurnId>> {
        if self.open_turn(session).await?.is_some() {
            return Ok(None);
        }
        let turn = TurnId::new();
        self.step(session, turn, StepKind::InputAccepted, None, payload)
            .await?;
        Ok(Some(turn))
    }

    /// 146 D9: the ACP child's turn ended (`stop_reason`): its open journal
    /// turn gets `turn_finished` (and `sessions.last_stop`). `Ok(false)` when
    /// none was open.
    pub async fn acp_turn_close(
        &self,
        session: SessionId,
        stop_reason: &str,
    ) -> anyhow::Result<bool> {
        let Some((turn, _)) = self.open_turn(session).await? else {
            return Ok(false);
        };
        let mut tx = self.begin_write().await?;
        turn_finished_in(&mut tx, session, turn, stop_reason).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// 146 D10/D11: the first and the last `User` row's text of `session`
    /// — the child's original brief and the prompt it was interrupted in —
    /// each capped to `max` chars (the `<time …/>` line dropped). Empty
    /// strings when there is none.
    pub async fn prompt_excerpts(&self, session: SessionId, max: usize) -> (String, String) {
        let one = |order: &'static str| async move {
            let row: Option<(Option<String>,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT json_extract(body, '$.content[0].Text') FROM messages \
                 WHERE session_id = ? AND json_extract(body, '$.role') = 'User' \
                 ORDER BY seq {order} LIMIT 1"
            )))
            .bind(session.to_string())
            .fetch_optional(&self.read)
            .await
            .ok()
            .flatten();
            row.and_then(|(t,)| t).unwrap_or_default()
        };
        let (first, last) = (one("ASC").await, one("DESC").await);
        (excerpt(&first, max), excerpt(&last, max))
    }

    /// What `ResumeTurn` acts on (§4.1, §4.2): the newest step that is not
    /// an `assistant_partial`; a `suspended` row only sets `kind` and defers
    /// to the row before it. An error when the turn has no journal.
    pub async fn resume_plan(
        &self,
        session: SessionId,
        turn: TurnId,
    ) -> anyhow::Result<ResumePlan> {
        let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT kind, call_id, payload, at FROM turn_steps \
             WHERE session_id = ? AND turn_id = ? ORDER BY n",
        )
        .bind(session.to_string())
        .bind(turn.to_string())
        .fetch_all(&self.read)
        .await?;
        let steps: Vec<StepRow> = rows
            .into_iter()
            .map(|(kind, call_id, payload, at)| StepRow {
                kind,
                call_id,
                payload,
                at,
            })
            .collect();
        anyhow::ensure!(!steps.is_empty(), "turn {turn} of {session} has no journal");
        let ats = steps
            .iter()
            .map(|s| parse_dt(&s.at))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let interrupted_at = *ats.iter().max().expect("non-empty");
        let ran_ms = run_ms(session, turn, &steps)?;
        // 178 W2: the opener's source says whether the turn ran bounded.
        let bounded = steps
            .iter()
            .find(|s| s.kind == StepKind::InputAccepted.as_str())
            .is_some_and(|s| {
                matches!(json(&s.payload)["source"].as_str(), Some("peer" | "scoped"))
            });
        let mut kind = ResumeKind::Interrupted;
        let mut suspended_reason = None;
        let mut newest = None;
        for s in steps.iter().rev() {
            match StepKind::parse(&s.kind) {
                Some(StepKind::AssistantPartial) => continue,
                Some(StepKind::Suspended) => {
                    kind = ResumeKind::Suspended;
                    // The newest `suspended` row's reason names the cause (139 §6.3).
                    if suspended_reason.is_none() {
                        suspended_reason = json(&s.payload)["reason"].as_str().map(str::to_owned);
                    }
                }
                Some(k) => {
                    newest = Some((k, s));
                    break;
                }
                None => anyhow::bail!("turn_steps.kind {:?}", s.kind),
            }
        }
        let Some((step, row)) = newest else {
            anyhow::bail!("turn {turn} of {session} has only partial/suspended rows");
        };
        let payload = json(&row.payload);
        let mut calls = Vec::new();
        let mut partial = None;
        let last = match step {
            StepKind::InputAccepted => LastStep::InputAccepted,
            StepKind::ModelCall => {
                partial = steps
                    .iter()
                    .rev()
                    .filter(|s| s.kind == StepKind::AssistantPartial.as_str())
                    .find(|s| row.call_id.is_none() || s.call_id == row.call_id)
                    .map(|s| -> anyhow::Result<PartialContent> {
                        let p = json(&s.payload);
                        Ok(PartialContent {
                            call_id: s.call_id.clone().unwrap_or_default(),
                            content: serde_json::from_value(p["content"].clone())
                                .unwrap_or_else(|_| MessageContent::from_text("")),
                            thinking_ms: p["thinking_ms"].as_u64(),
                            at: parse_dt(&s.at)?,
                        })
                    })
                    .transpose()?;
                LastStep::ModelCall {
                    request_id: payload["request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    attempt: u32::try_from(payload["attempt"].as_u64().unwrap_or(1)).unwrap_or(1),
                }
            }
            StepKind::ToolBatch => {
                let ids: Vec<&str> = payload["call_ids"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut all = self.calls_of(session, turn).await?;
                // The batch's rows in `call_ids` order (ids the journal lacks are skipped).
                calls = ids
                    .iter()
                    .filter_map(|id| {
                        all.iter()
                            .position(|c| c.call_id == *id)
                            .map(|i| all.swap_remove(i))
                    })
                    .collect();
                LastStep::ToolBatch {
                    all_finished: calls.iter().all(|c| c.state.is_terminal()),
                }
            }
            StepKind::TurnFinished => LastStep::Finished,
            StepKind::AssistantPartial | StepKind::Suspended => unreachable!("skipped above"),
        };
        Ok(ResumePlan {
            turn,
            kind,
            cause: ResumeCause::from_journal(kind, suspended_reason.as_deref()),
            interrupted_at,
            ran_ms,
            bounded,
            last,
            calls,
            partial,
        })
    }

    // ---- W1: the loop's piggy-backed writes (§3.3) ----------------------------

    /// §3.3 step 1: the user row (with its 172 `channel` origin, if any) plus
    /// `turn_steps(input_accepted)` in one transaction; returns the row's seq.
    pub async fn append_input(
        &self,
        session: SessionId,
        turn: TurnId,
        message: &ChatMessage,
        input_id: Option<&str>,
        source: &str,
        channel: Option<&crate::ChannelOrigin>,
    ) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let sid = session.to_string();
        let seq = insert_in(&mut tx, &sid, message, None, None).await?;
        channel_in(&mut tx, &sid, seq, channel).await?;
        step_in(
            &mut tx,
            session,
            turn,
            StepKind::InputAccepted,
            None,
            &serde_json::json!({ "input_id": input_id, "source": source }),
        )
        .await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// §3.3 step 1 for a peer input already consumed by `consume_exchange`
    /// (its own transaction, 50 §7): the step alone.
    pub async fn input_accepted(
        &self,
        session: SessionId,
        turn: TurnId,
        input_id: Option<&str>,
        source: &str,
    ) -> anyhow::Result<()> {
        self.step(
            session,
            turn,
            StepKind::InputAccepted,
            None,
            &serde_json::json!({ "input_id": input_id, "source": source }),
        )
        .await
    }

    /// §3.3 step 2: `reserve_call` plus `turn_steps(model_call)` with the
    /// attempt's `call_id`, one transaction.
    #[expect(clippy::too_many_arguments, reason = "one row, one call")]
    pub async fn reserve_journaled_call(
        &self,
        owner: SessionId,
        turn: TurnId,
        request_id: &str,
        attempt: u32,
        kind: CallKind,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> anyhow::Result<CallId> {
        let mut tx = self.begin_write().await?;
        let call = super::accounting::reserve_call_in(
            &mut tx,
            owner,
            request_id,
            attempt,
            Some(turn),
            kind,
            provider,
            model,
        )
        .await?;
        step_in(
            &mut tx,
            owner,
            turn,
            StepKind::ModelCall,
            Some(&call.0),
            &serde_json::json!({ "request_id": request_id, "attempt": attempt, "kind": kind.as_str() }),
        )
        .await?;
        tx.commit().await?;
        Ok(call)
    }

    /// §3.3 step 5: the assistant `tool_use` row, the turn's partials gone
    /// (§8.1), `turn_steps(tool_batch)` and one `planned` row per call, all
    /// in one transaction; returns the row's seq.
    pub async fn append_tool_use(
        &self,
        session: SessionId,
        turn: TurnId,
        message: &ChatMessage,
        thinking_ms: Option<u64>,
        round: u32,
        calls: &[(ToolCall, Risk)],
    ) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let ms = thinking_ms.map(|ms| i64::try_from(ms).unwrap_or(i64::MAX));
        let seq = insert_in(&mut tx, &session.to_string(), message, None, ms).await?;
        partial_delete_in(&mut tx, session, turn).await?;
        let ids: Vec<&str> = calls.iter().map(|(c, _)| c.call_id.as_str()).collect();
        step_in(
            &mut tx,
            session,
            turn,
            StepKind::ToolBatch,
            None,
            &serde_json::json!({ "call_ids": ids, "round": round }),
        )
        .await?;
        calls_planned_in(&mut tx, session, turn, seq, calls).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// §3.3 step 4 + §4.5: the final assistant row, the partials gone,
    /// `turn_steps(turn_finished)` and `sessions.last_stop/finished_at`, one
    /// transaction; returns the row's seq.
    pub async fn append_final(
        &self,
        session: SessionId,
        turn: TurnId,
        message: &ChatMessage,
        thinking_ms: Option<u64>,
        stop_reason: &str,
    ) -> anyhow::Result<i64> {
        let mut tx = self.begin_write().await?;
        let ms = thinking_ms.map(|ms| i64::try_from(ms).unwrap_or(i64::MAX));
        let seq = insert_in(&mut tx, &session.to_string(), message, None, ms).await?;
        turn_finished_in(&mut tx, session, turn, stop_reason).await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// [`turn_finished_in`] alone: the cancel / error / landing paths whose
    /// last row is not an assistant reply (§3.2), and a resumed turn closed
    /// without a model call.
    pub async fn finish_turn(
        &self,
        session: SessionId,
        turn: TurnId,
        stop_reason: &str,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        turn_finished_in(&mut tx, session, turn, stop_reason).await?;
        tx.commit().await?;
        Ok(())
    }

    /// §3.3 step 8: the batch's one tool message, the plan it changed (47
    /// §5.1, when `todos` is set), the rows' spill-capped `output` (step 7)
    /// and the `skipped` rows of a cancel / landing (step 9), one transaction.
    pub async fn append_batch(
        &self,
        session: SessionId,
        message: &ChatMessage,
        todos: Option<(TodoSource, &[Todo])>,
        rows: &[BatchRow<'_>],
    ) -> anyhow::Result<(i64, Option<TodoSnapshot>)> {
        let sid = session.to_string();
        let mut tx = self.begin_write().await?;
        let seq = insert_in(&mut tx, &sid, message, None, None).await?;
        let snapshot = match todos {
            Some((source, items)) => Some(put_todos(&mut tx, &sid, seq, source, items).await?),
            None => None,
        };
        let ts = now();
        for r in rows {
            match r.skipped {
                true => {
                    sqlx::query(
                        "UPDATE tool_calls SET state = 'skipped', finished_at = ?, output = ?, is_error = ? \
                         WHERE session_id = ? AND call_id = ? AND state IN ('planned', 'started')",
                    )
                    .bind(&ts)
                    .bind(r.output)
                    .bind(i64::from(r.is_error))
                    .bind(&sid)
                    .bind(r.call_id)
                    .execute(&mut *tx)
                    .await?;
                }
                false => {
                    sqlx::query(
                        "UPDATE tool_calls SET output = ? WHERE session_id = ? AND call_id = ? AND output IS NOT ?",
                    )
                    .bind(r.output)
                    .bind(&sid)
                    .bind(r.call_id)
                    .bind(r.output)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok((seq, snapshot))
    }

    /// Drop the turn's `assistant_partial` rows on their own (§4.2 row 2: the
    /// partial of a lost model call is published as dropped, never replayed).
    pub async fn partial_delete(&self, session: SessionId, turn: TurnId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        partial_delete_in(&mut tx, session, turn).await?;
        tx.commit().await?;
        Ok(())
    }

    /// §4.2: a `started` read-only call (or a `python` cell with only
    /// read-only inner calls) runs again as `attempt + 1`: back to `planned`
    /// so the registry hook moves it through `started` → `finished` once more.
    pub async fn call_replan(&self, session: SessionId, call_id: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE tool_calls SET state = 'planned', attempt = attempt + 1, started_at = NULL \
             WHERE session_id = ? AND call_id = ? AND state IN ('planned', 'started')",
        )
        .bind(session.to_string())
        .bind(call_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// `sessions.last_stop` / `finished_at` (§4.5), as `turn_finished` wrote
    /// them; `None` for a session that never finished a turn since 0033.
    pub async fn last_stop(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Option<(String, DateTime<Utc>)>> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT last_stop, finished_at FROM sessions WHERE id = ?")
                .bind(session.to_string())
                .fetch_optional(&self.read)
                .await?;
        match row {
            Some((Some(stop), Some(at))) => Ok(Some((stop, parse_dt(&at)?))),
            _ => Ok(None),
        }
    }

    /// The inner calls of one `python` cell (08 as-built), in `n` order: the
    /// python rule's F (finished) and U (`result: None`) sets (§4.2).
    pub async fn inner_calls_of(
        &self,
        session: SessionId,
        parent: &str,
    ) -> anyhow::Result<Vec<InnerCall>> {
        Ok(self
            .inner_calls(session)
            .await?
            .remove(parent)
            .unwrap_or_default())
    }

    /// §4.2 python rule: the unfinished inner calls of `parent` are closed
    /// with `note` as an error so the reloaded transcript shows them settled.
    pub async fn inner_calls_interrupt(
        &self,
        session: SessionId,
        parent: &str,
        note: &str,
    ) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE inner_calls SET output = ?, is_error = 1 \
             WHERE session_id = ? AND parent_id = ? AND output IS NULL",
        )
        .bind(note)
        .bind(session.to_string())
        .bind(parent)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    // ---- questions (§6) -----------------------------------------------------

    /// Upsert a `questions` row by `q.id` (raise, re-publish, resolve).
    pub async fn question_put(&self, q: &QuestionRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO questions (id, session_id, tool_call_id, kind, payload, state, asked_at, deadline, answered_at, answer, answered_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET tool_call_id = excluded.tool_call_id, kind = excluded.kind, \
                 payload = excluded.payload, state = excluded.state, asked_at = excluded.asked_at, \
                 deadline = excluded.deadline, answered_at = excluded.answered_at, answer = excluded.answer, \
                 answered_by = excluded.answered_by",
        )
        .bind(&q.id)
        .bind(q.session.to_string())
        .bind(&q.tool_call_id)
        .bind(q.kind.as_str())
        .bind(q.payload.to_string())
        .bind(q.state.as_str())
        .bind(q.asked_at.to_rfc3339())
        .bind(q.deadline.to_rfc3339())
        .bind(q.answered_at.map(|t| t.to_rfc3339()))
        .bind(q.answer.as_ref().map(Value::to_string))
        .bind(q.answered_by.map(Answerer::as_str))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Move a `pending` question to `answered` / `superseded` / `timed_out`
    /// with `answer`; `false` when it was not pending.
    pub async fn question_resolve(
        &self,
        id: &str,
        state: QuestionState,
        answer: Option<&Value>,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            state != QuestionState::Pending,
            "resolve to a terminal state"
        );
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE questions SET state = ?, answered_at = ?, answer = ? WHERE id = ? AND state = 'pending'",
        )
        .bind(state.as_str())
        .bind(now())
        .bind(answer.map(Value::to_string))
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// The session's `pending` questions, oldest first.
    pub async fn questions_pending(&self, session: SessionId) -> anyhow::Result<Vec<QuestionRow>> {
        let rows: Vec<QuestionTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {QUESTION_COLS} FROM questions WHERE session_id = ? AND state = 'pending' ORDER BY asked_at, id"
        )))
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(question_of).collect()
    }

    /// One `questions` row by id, any state (132 D6: a late answer finds its question).
    pub async fn question_get(&self, id: &str) -> anyhow::Result<Option<QuestionRow>> {
        let row: Option<QuestionTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {QUESTION_COLS} FROM questions WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.read)
        .await?;
        row.map(question_of).transpose()
    }

    /// 109 W3 (§6.2): every `questions` row of one tool call, newest first —
    /// the resume path reads the pending one to re-publish it and the
    /// `answered` one to honour it.
    pub async fn questions_of_call(
        &self,
        session: SessionId,
        call_id: &str,
    ) -> anyhow::Result<Vec<QuestionRow>> {
        let rows: Vec<QuestionTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {QUESTION_COLS} FROM questions WHERE session_id = ? AND tool_call_id = ? ORDER BY asked_at DESC, id DESC"
        )))
        .bind(session.to_string())
        .bind(call_id)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(question_of).collect()
    }

    // ---- waits (§4.4) ---------------------------------------------------------

    /// Upsert the `waits` row of `w.tool_call_id`: the call began blocking.
    pub async fn wait_put(&self, w: &WaitRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO waits (session_id, tool_call_id, kind, target, deadline, payload) VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, tool_call_id) DO UPDATE SET kind = excluded.kind, target = excluded.target, \
                 deadline = excluded.deadline, payload = excluded.payload",
        )
        .bind(w.session.to_string())
        .bind(&w.tool_call_id)
        .bind(w.kind.as_str())
        .bind(&w.target)
        .bind(w.deadline.map(|t| t.to_rfc3339()))
        .bind(w.payload.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Delete the call's `waits` row: it unblocked. `false` when none existed.
    pub async fn wait_clear(&self, session: SessionId, call_id: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("DELETE FROM waits WHERE session_id = ? AND tool_call_id = ?")
            .bind(session.to_string())
            .bind(call_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// The waits of `turn`'s tool calls.
    pub async fn waits_of(&self, session: SessionId, turn: TurnId) -> anyhow::Result<Vec<WaitRow>> {
        let rows: Vec<WaitTuple> = sqlx::query_as(
            "SELECT w.session_id, w.tool_call_id, w.kind, w.target, w.deadline, w.payload \
             FROM waits w JOIN tool_calls c ON c.session_id = w.session_id AND c.call_id = w.tool_call_id \
             WHERE w.session_id = ? AND c.turn_id = ? ORDER BY c.at_seq, c.rowid",
        )
        .bind(session.to_string())
        .bind(turn.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(wait_of).collect()
    }

    // ---- jobs and PTY screens (§5) -------------------------------------------

    /// Upsert a `jobs` row by `(j.session, j.id)`. `j.engine` is written as
    /// given: the `tools::Journal` seam stamps it (`StoreJournal::job`),
    /// 139 §10 — the tools crate never learns the engine id.
    pub async fn job_put(&self, j: &JobRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO jobs (session_id, id, kind, call_id, command, name, pgid, log, started_ms, state, exit, settled_ms, engine) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, id) DO UPDATE SET kind = excluded.kind, call_id = excluded.call_id, \
                 command = excluded.command, name = excluded.name, pgid = excluded.pgid, log = excluded.log, \
                 started_ms = excluded.started_ms, state = excluded.state, exit = excluded.exit, \
                 settled_ms = excluded.settled_ms, engine = excluded.engine",
        )
        .bind(j.session.to_string())
        .bind(&j.id.0)
        .bind(j.kind.as_str())
        .bind(&j.call_id)
        .bind(&j.command)
        .bind(&j.name)
        .bind(j.pgid)
        .bind(j.log.as_ref().map(|p| p.to_string_lossy().into_owned()))
        .bind(j.started_ms)
        .bind(j.state.as_str())
        .bind(j.exit.as_ref().map(ToString::to_string))
        .bind(j.settled_ms)
        .bind(&j.engine)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Settle a `running` job: `settled` with its exit, or `lost` (§5.1).
    /// `false` when the row was not running.
    pub async fn job_settle(
        &self,
        session: SessionId,
        id: &JobId,
        state: JobState,
        exit: &JobExit,
        settled_ms: i64,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(state != JobState::Running, "settle to a terminal state");
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE jobs SET state = ?, exit = ?, settled_ms = ? \
             WHERE session_id = ? AND id = ? AND state = 'running'",
        )
        .bind(state.as_str())
        .bind(exit.to_string())
        .bind(settled_ms)
        .bind(session.to_string())
        .bind(&id.0)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Every `running` job row (the reaper's input, §5.4). `Scope::Here`
    /// keeps the rows of sessions owned here **and** spawned by this engine
    /// (`engine = '' OR engine = me`, 139 D14): a row this engine did not
    /// spawn is never signalled, whatever its session's owner is now.
    pub async fn jobs_open(&self, scope: Scope) -> anyhow::Result<Vec<JobRow>> {
        let engine = match scope {
            Scope::Here => "AND (engine = '' OR engine = ?1)",
            Scope::All => "",
        };
        let rows: Vec<JobTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {JOB_COLS} FROM jobs WHERE state = 'running' \
             AND session_id IN (SELECT s.id FROM sessions s WHERE {}) {engine} \
             ORDER BY started_ms, rowid",
            scope.sql()
        )))
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(job_of).collect()
    }

    /// 139 §7.1 (takeover): settle every `running` job of `session` that
    /// another engine spawned (`engine NOT IN ('', me)` — a `''` row is
    /// this engine's, Q14, as `jobs_open(Here)` reads it) as `lost`, and
    /// return the rows as settled. No `adopt`, no `killpg` — the process is
    /// not ours (I6); the caller (W3's `settle_transferred`) builds the
    /// `Reaped`.
    pub async fn jobs_settle_transferred(
        &self,
        session: SessionId,
        me: &str,
        now_ms: i64,
    ) -> anyhow::Result<Vec<JobRow>> {
        let mut tx = self.begin_write().await?;
        let rows: Vec<JobTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE jobs SET state = 'lost', exit = 'lost', settled_ms = ? \
             WHERE session_id = ? AND state = 'running' AND engine != '' AND engine != ? \
             RETURNING {JOB_COLS}"
        )))
        .bind(now_ms)
        .bind(session.to_string())
        .bind(me)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter().map(job_of).collect()
    }

    /// Every `child_result` wait, scoped by the **parent's** owner (139
    /// §7.4): the trigger for `respawn_acp_children`, which must not go
    /// through `open_turns` — the parent's turn is usually finished by the
    /// time the child is.
    pub async fn child_waits(&self, scope: Scope) -> anyhow::Result<Vec<WaitRow>> {
        let rows: Vec<WaitTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT w.session_id, w.tool_call_id, w.kind, w.target, w.deadline, w.payload \
             FROM waits w JOIN sessions s ON s.id = w.session_id \
             WHERE w.kind = 'child_result' AND {} ORDER BY w.rowid",
            scope.sql()
        )))
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(wait_of).collect()
    }

    /// 155 §5.4: every `peer_state` wait (`wait_peer`), scoped by the
    /// **caller's** owner: the boot re-arm reads them outside `open_turns`,
    /// the way `child_waits` does.
    pub async fn peer_state_waits(&self, scope: Scope) -> anyhow::Result<Vec<WaitRow>> {
        let rows: Vec<WaitTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT w.session_id, w.tool_call_id, w.kind, w.target, w.deadline, w.payload \
             FROM waits w JOIN sessions s ON s.id = w.session_id \
             WHERE w.kind = 'peer_state' AND {} ORDER BY w.rowid",
            scope.sql()
        )))
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(wait_of).collect()
    }

    /// 146 D1: ACP children here that were mid-prompt when the engine died
    /// ([`MIDPROMPT`] over their last `messages` row) with their parent,
    /// newest first; bounded to rows updated after `since`. A harness-made
    /// `System` note (the lost note, `append_local_note`) ends the match, so
    /// a child marked lost is not scanned again. D9: the fallback for
    /// children from before the ACP journal — a child with any `turn_steps`
    /// row is the journal's (`open_turns`), never this heuristic's.
    /// Only `kind = 'chat'` rows: advisors and 141's watch-only `observed`
    /// children (never driven, no `agent`) are not respawned.
    pub async fn midprompt_acp_children(
        &self,
        since: DateTime<Utc>,
    ) -> anyhow::Result<Vec<(SessionId, SessionId)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT c.id, c.parent FROM sessions c \
             JOIN messages m ON m.session_id = c.id \
               AND m.seq = (SELECT MAX(seq) FROM messages WHERE session_id = c.id) \
             WHERE c.agent IS NOT NULL AND c.parent IS NOT NULL AND c.kind IN {DRIVEN_KINDS} \
               AND (c.owner = '' OR c.owner = ?1) AND c.updated_at > ?2 AND {MIDPROMPT} \
               AND NOT EXISTS (SELECT 1 FROM turn_steps t WHERE t.session_id = c.id) \
             ORDER BY c.updated_at DESC"
        )))
        .bind(&*self.me)
        .bind(since.to_rfc3339())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(c, p)| Ok((parse_session(&c)?, parse_session(&p)?)))
            .collect()
    }

    /// 146 D8: whether one session's last `messages` row is mid-prompt
    /// ([`MIDPROMPT`]) — the Resume button's "is there work to continue".
    /// A session with no rows is not.
    pub async fn midprompt(&self, session: SessionId) -> anyhow::Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM messages m WHERE m.session_id = ?1 \
             AND m.seq = (SELECT MAX(seq) FROM messages WHERE session_id = ?1) AND {MIDPROMPT}"
        )))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row.is_some())
    }

    /// The session's job rows, oldest first (`jobs()` after a restart, §5.1).
    pub async fn jobs_of(&self, session: SessionId) -> anyhow::Result<Vec<JobRow>> {
        let rows: Vec<JobTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {JOB_COLS} FROM jobs WHERE session_id = ? ORDER BY started_ms, rowid"
        )))
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(job_of).collect()
    }

    /// Upsert the last screen of a PTY job (§5.3). Attached since 139 §5.3:
    /// the row replicates and is fenced like every journal table, so only
    /// the owner writes screens. The job row must exist.
    pub async fn pty_screen_put(&self, s: &PtyScreenRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO pty_screens (session_id, job_id, revision, rows, cols, cursor_row, cursor_col, text, at_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, job_id) DO UPDATE SET revision = excluded.revision, rows = excluded.rows, \
                 cols = excluded.cols, cursor_row = excluded.cursor_row, cursor_col = excluded.cursor_col, \
                 text = excluded.text, at_ms = excluded.at_ms",
        )
        .bind(s.session.to_string())
        .bind(&s.job_id.0)
        .bind(i64::try_from(s.revision).unwrap_or(i64::MAX))
        .bind(i64::from(s.rows))
        .bind(i64::from(s.cols))
        .bind(i64::from(s.cursor_row))
        .bind(i64::from(s.cursor_col))
        .bind(&s.text)
        .bind(s.at_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The stored screen of `job_id`, if any.
    pub async fn pty_screen(
        &self,
        session: SessionId,
        job_id: &JobId,
    ) -> anyhow::Result<Option<PtyScreenRow>> {
        let row: Option<(i64, i64, i64, i64, i64, String, i64)> = sqlx::query_as(
            "SELECT revision, rows, cols, cursor_row, cursor_col, text, at_ms FROM pty_screens \
             WHERE session_id = ? AND job_id = ?",
        )
        .bind(session.to_string())
        .bind(&job_id.0)
        .fetch_optional(&self.read)
        .await?;
        Ok(row.map(
            |(revision, rows, cols, cursor_row, cursor_col, text, at_ms)| PtyScreenRow {
                session,
                job_id: job_id.clone(),
                revision: u64::try_from(revision).unwrap_or(0),
                rows: u16::try_from(rows).unwrap_or(u16::MAX),
                cols: u16::try_from(cols).unwrap_or(u16::MAX),
                cursor_row: u16::try_from(cursor_row).unwrap_or(0),
                cursor_col: u16::try_from(cursor_col).unwrap_or(0),
                text,
                at_ms,
            },
        ))
    }

    // ---- resume reports (§8.2) ---------------------------------------------

    /// Upsert the `resume_reports` row of `(r.session, r.turn)`: the counts
    /// the resumed step settled with. Written right before the live
    /// `ResumeReport` event so a late desktop reads the same numbers.
    pub async fn resume_report_put(&self, r: &ResumeReportRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO resume_reports (session_id, turn_id, kind, interrupted_at, at, replayed, interrupted, \
                 lost_jobs, children_restored, children_handed_off, cause) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, turn_id) DO UPDATE SET kind = excluded.kind, \
                 interrupted_at = excluded.interrupted_at, at = excluded.at, replayed = excluded.replayed, \
                 interrupted = excluded.interrupted, lost_jobs = excluded.lost_jobs, \
                 children_restored = excluded.children_restored, children_handed_off = excluded.children_handed_off, \
                 cause = excluded.cause",
        )
        .bind(r.session.to_string())
        .bind(r.turn.to_string())
        .bind(r.kind.as_str())
        .bind(r.interrupted_at.to_rfc3339())
        .bind(r.at.to_rfc3339())
        .bind(serde_json::to_string(&r.report.replayed)?)
        .bind(serde_json::to_string(&r.report.interrupted)?)
        .bind(serde_json::to_string(&r.report.lost_jobs)?)
        .bind(i64::from(r.report.children_restored))
        .bind(i64::from(r.report.children_handed_off))
        .bind(r.cause.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The report of `turn`, if it was resumed.
    pub async fn resume_report(
        &self,
        session: SessionId,
        turn: TurnId,
    ) -> anyhow::Result<Option<ResumeReportRow>> {
        let row: Option<ReportTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {REPORT_COLS} FROM resume_reports WHERE session_id = ? AND turn_id = ?"
        )))
        .bind(session.to_string())
        .bind(turn.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(|r| report_of(session, r)).transpose()
    }

    /// The report of the session's newest turn (by its last journal
    /// activity), or `None` when that turn was never resumed: the banner
    /// a reload shows until the next turn starts.
    pub async fn latest_resume_report(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Option<ResumeReportRow>> {
        let row: Option<ReportTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {REPORT_COLS} FROM resume_reports WHERE session_id = ?1 AND turn_id = \
             (SELECT turn_id FROM ({NEWEST}) WHERE rn = 1 AND session_id = ?1 ORDER BY last_at DESC LIMIT 1)"
        )))
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(|r| report_of(session, r)).transpose()
    }

    // ---- retention (§3.6) ----------------------------------------------------

    /// Delete the journal of every **finished** turn whose last activity is
    /// older than `keep_days`, and every terminal question older than that.
    /// Open turns and pending questions are never pruned. Returns the number
    /// of `turn_steps` rows removed. 139 §5.3 (D15): `Scope::Here` on the
    /// owner's hourly tick — the `DELETE`s travel to every replica as log
    /// rows; a foreign session's journal is never pruned here (the fence
    /// would refuse the write anyway, so the scope is what keeps the tick
    /// from failing as a whole).
    pub async fn journal_prune(&self, scope: Scope, keep_days: u32) -> anyhow::Result<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(i64::from(keep_days))).to_rfc3339();
        let mine = format!("SELECT s.id FROM sessions s WHERE {}", scope.sql());
        let fin = format!(
            "WITH fin AS (SELECT session_id, turn_id FROM ({NEWEST}) \
             WHERE rn = 1 AND kind = 'turn_finished' AND last_at < ?2 AND session_id IN ({mine}))"
        );
        let mut tx = self.begin_write().await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{fin} DELETE FROM waits WHERE (session_id, tool_call_id) IN \
             (SELECT c.session_id, c.call_id FROM tool_calls c JOIN fin USING (session_id, turn_id))"
        )))
        .bind(&*self.me)
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?;
        for table in ["tool_calls", "resume_reports"] {
            // Names are this list's, never input.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "{fin} DELETE FROM {table} WHERE (session_id, turn_id) IN (SELECT session_id, turn_id FROM fin)"
            )))
            .bind(&*self.me)
            .bind(&cutoff)
            .execute(&mut *tx)
            .await?;
        }
        let steps = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{fin} DELETE FROM turn_steps WHERE (session_id, turn_id) IN (SELECT session_id, turn_id FROM fin)"
        )))
        .bind(&*self.me)
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM questions WHERE state != 'pending' AND COALESCE(answered_at, deadline) < ?2 \
             AND session_id IN ({mine})"
        )))
        .bind(&*self.me)
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(steps)
    }
}

#[cfg(test)]
mod tests {
    use genai::chat::ChatMessage;
    use serde_json::json;

    use super::*;

    async fn fresh() -> (Store, SessionId, crate::TestDir) {
        let scratch = crate::TestDir::new("dur");
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

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            fn_name: name.into(),
            fn_arguments: json!({"x": 1}),
            thought_signatures: None,
        }
    }

    /// `input_accepted`, `model_call`, then a `tool_batch` of three planned calls.
    async fn batch_turn(store: &Store, s: SessionId) -> TurnId {
        let t = TurnId::new();
        store.append(s, &ChatMessage::user("hi")).await.unwrap();
        store
            .step(
                s,
                t,
                StepKind::InputAccepted,
                None,
                &json!({"source": "user"}),
            )
            .await
            .unwrap();
        store
            .step(
                s,
                t,
                StepKind::ModelCall,
                Some("mc1"),
                &json!({"request_id": "r1", "attempt": 1}),
            )
            .await
            .unwrap();
        let seq = store
            .append(s, &ChatMessage::assistant("tools"))
            .await
            .unwrap();
        let mut tx = store.begin_write().await.unwrap();
        step_in(
            &mut tx,
            s,
            t,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ["c1", "c2", "c3"], "round": 1}),
        )
        .await
        .unwrap();
        calls_planned_in(
            &mut tx,
            s,
            t,
            seq,
            &[
                (call("c1", "read_file"), Risk::ReadOnly),
                (call("c2", "write_file"), Risk::Mutating),
                (call("c3", "shell"), Risk::Destructive),
            ],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        t
    }

    #[tokio::test]
    async fn plan_follows_the_newest_step() {
        let (store, s, _dir) = fresh().await;
        let t = TurnId::new();
        assert!(store.resume_plan(s, t).await.is_err());

        store
            .step(s, t, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(p.last, LastStep::InputAccepted);
        assert_eq!(p.kind, ResumeKind::Interrupted);
        assert!(p.calls.is_empty() && p.partial.is_none());

        store
            .step(
                s,
                t,
                StepKind::ModelCall,
                Some("mc1"),
                &json!({"request_id": "r1", "attempt": 2}),
            )
            .await
            .unwrap();
        // A partial rides with the model call and is upserted, never appended.
        store
            .partial_upsert(s, t, "mc1", &MessageContent::from_text("hel"), None)
            .await
            .unwrap();
        store
            .partial_upsert(s, t, "mc1", &MessageContent::from_text("hello"), Some(12))
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM turn_steps WHERE kind = 'assistant_partial' AND session_id = ?",
        )
        .bind(s.to_string())
        .fetch_one(&store.read)
        .await
        .unwrap();
        assert_eq!(n, 1);
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(
            p.last,
            LastStep::ModelCall {
                request_id: "r1".into(),
                attempt: 2
            }
        );
        let partial = p.partial.unwrap();
        assert_eq!(partial.call_id, "mc1");
        assert_eq!(
            serde_json::to_value(&partial.content).unwrap(),
            serde_json::to_value(MessageContent::from_text("hello")).unwrap()
        );
        assert_eq!(partial.thinking_ms, Some(12));
        assert!(p.interrupted_at >= partial.at);

        // `suspended` keeps the step before it and changes only the kind.
        store
            .step(
                s,
                t,
                StepKind::Suspended,
                None,
                &json!({"reason": "shutdown"}),
            )
            .await
            .unwrap();
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(p.kind, ResumeKind::Suspended);
        assert!(matches!(p.last, LastStep::ModelCall { .. }));

        // The final persist deletes the partial; `turn_finished` closes the turn.
        let mut tx = store.begin_write().await.unwrap();
        partial_delete_in(&mut tx, s, t).await.unwrap();
        step_in(
            &mut tx,
            s,
            t,
            StepKind::TurnFinished,
            None,
            &json!({"stop_reason": "end_turn"}),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(p.last, LastStep::Finished);
        assert!(p.partial.is_none());
        assert!(store.open_turns(Scope::Here).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn batch_plan_tracks_call_states() {
        let (store, s, _dir) = fresh().await;
        let t = batch_turn(&store, s).await;
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(
            p.last,
            LastStep::ToolBatch {
                all_finished: false
            }
        );
        assert_eq!(
            p.calls
                .iter()
                .map(|c| (c.call_id.as_str(), c.state, c.risk))
                .collect::<Vec<_>>(),
            vec![
                ("c1", CallState::Planned, Risk::ReadOnly),
                ("c2", CallState::Planned, Risk::Mutating),
                ("c3", CallState::Planned, Risk::Destructive),
            ]
        );
        assert_eq!(p.calls[0].args, json!({"x": 1}));

        // Unknown call ids are no-ops; planned → started → finished.
        assert!(!store.call_started(s, "nope").await.unwrap());
        assert!(store.call_started(s, "c1").await.unwrap());
        assert!(store.call_started(s, "c2").await.unwrap());
        let out = ToolOutput {
            content: "ok".into(),
            is_error: false,
            structured: None,
            files: vec![PathBuf::from("/tmp/a.png")],
        };
        assert!(store.call_finished(s, "c1", &out).await.unwrap());
        assert!(!store.call_finished(s, "c1", &out).await.unwrap());
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(
            p.last,
            LastStep::ToolBatch {
                all_finished: false
            }
        );
        let c1 = &p.calls[0];
        assert_eq!(
            (c1.state, c1.output.as_deref(), c1.is_error),
            (CallState::Finished, Some("ok"), false)
        );
        assert_eq!(c1.files, vec![PathBuf::from("/tmp/a.png")]);
        assert!(p.calls[1].started_at.is_some());
        assert!(p.calls[2].started_at.is_none());

        // The open turn is listed until every row is terminal and the turn finishes.
        let open = store.open_turns(Scope::Here).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!((open[0].0, open[0].1), (s, t));

        assert!(
            store
                .call_interrupted(s, "c2", "ERROR: interrupted")
                .await
                .unwrap()
        );
        assert!(
            store
                .call_finished(s, "c3", &ToolOutput::err("x"))
                .await
                .unwrap()
        );
        let p = store.resume_plan(s, t).await.unwrap();
        assert_eq!(p.last, LastStep::ToolBatch { all_finished: true });
        assert_eq!(p.calls[1].state, CallState::Interrupted);
        assert!(p.calls[1].is_error);
        assert_eq!(p.calls[2].state, CallState::Finished);
        assert!(p.calls[2].is_error);
        assert_eq!(store.calls_of(s, t).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn open_turns_order_and_prune() {
        let (store, s, _dir) = fresh().await;
        let other = SessionId::new();
        store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();
        let old = batch_turn(&store, s).await;
        let newer = batch_turn(&store, other).await;
        let open = store.open_turns(Scope::All).await.unwrap();
        assert_eq!(
            open.iter().map(|(s, t, _)| (*s, *t)).collect::<Vec<_>>(),
            vec![(other, newer), (s, old)]
        );
        // An open turn is never pruned, however old.
        assert_eq!(store.journal_prune(Scope::Here, 0).await.unwrap(), 0);
        // Finish `old`, give one call a wait, backdate it, and prune.
        store
            .wait_put(&WaitRow {
                session: s,
                tool_call_id: "c3".into(),
                kind: WaitKind::Job,
                target: "j1".into(),
                deadline: None,
                payload: json!({}),
            })
            .await
            .unwrap();
        store
            .step(s, old, StepKind::TurnFinished, None, &json!({}))
            .await
            .unwrap();
        assert_eq!(store.open_turns(Scope::Here).await.unwrap().len(), 1);
        // Fresh rows survive a 7-day keep; a 0-day keep (cutoff = now) takes them.
        assert_eq!(store.journal_prune(Scope::Here, 7).await.unwrap(), 0);
        assert_eq!(store.waits_of(s, old).await.unwrap().len(), 1);
        let n = store.journal_prune(Scope::Here, 0).await.unwrap();
        assert_eq!(n, 4);
        assert!(store.calls_of(s, old).await.unwrap().is_empty());
        assert!(store.waits_of(s, old).await.unwrap().is_empty());
        assert!(store.resume_plan(s, old).await.is_err());
        // The other session's open turn is untouched.
        assert_eq!(store.calls_of(other, newer).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn questions_waits_jobs_screens_round_trip() {
        let (store, s, _dir) = fresh().await;
        let t = batch_turn(&store, s).await;
        let asked = Utc::now();
        let q = QuestionRow {
            id: "q1".into(),
            session: s,
            tool_call_id: Some("c2".into()),
            kind: QuestionKind::Approval,
            payload: json!({"tool": "write_file"}),
            state: QuestionState::Pending,
            asked_at: asked,
            deadline: asked + chrono::Duration::seconds(300),
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        store.question_put(&q).await.unwrap();
        store.question_put(&q).await.unwrap(); // re-publish: same row
        let pending = store.questions_pending(s).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "q1");
        assert_eq!(pending[0].kind, QuestionKind::Approval);
        assert_eq!(pending[0].payload, json!({"tool": "write_file"}));
        assert_eq!(pending[0].deadline, q.deadline);
        assert!(
            store
                .question_resolve("q1", QuestionState::Pending, None)
                .await
                .is_err()
        );
        assert!(
            store
                .question_resolve("q1", QuestionState::Answered, Some(&json!("allow")))
                .await
                .unwrap()
        );
        assert!(
            !store
                .question_resolve("q1", QuestionState::TimedOut, None)
                .await
                .unwrap()
        );
        assert!(store.questions_pending(s).await.unwrap().is_empty());
        let (state, answer): (String, Option<String>) =
            sqlx::query_as("SELECT state, answer FROM questions WHERE id = 'q1'")
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(
            (state.as_str(), answer.as_deref()),
            ("answered", Some("\"allow\""))
        );

        // 126 §4.3: `answered_by` round-trips through the upsert and the mapper;
        // `question_resolve` left it NULL.
        let settled = QuestionRow {
            state: QuestionState::Answered,
            answered_at: Some(asked),
            answer: Some(json!("allow")),
            answered_by: Some(Answerer::Double),
            ..q.clone()
        };
        store.question_put(&settled).await.unwrap();
        let rows = store.questions_of_call(s, "c2").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].answered_by, Some(Answerer::Double));
        let by: Option<String> =
            sqlx::query_scalar("SELECT answered_by FROM questions WHERE id = 'q1'")
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(by.as_deref(), Some("double"));

        // Waits: put / list by turn / clear.
        let w = WaitRow {
            session: s,
            tool_call_id: "c1".into(),
            kind: WaitKind::PeerReply,
            target: "ex1".into(),
            deadline: Some(asked),
            payload: json!({"request_id": "r"}),
        };
        store.wait_put(&w).await.unwrap();
        assert_eq!(store.waits_of(s, t).await.unwrap(), vec![w.clone()]);
        assert!(store.wait_clear(s, "c1").await.unwrap());
        assert!(!store.wait_clear(s, "c1").await.unwrap());
        assert!(store.waits_of(s, t).await.unwrap().is_empty());

        // Jobs: running → open; settle once; screens cascade with the job.
        let j = JobRow {
            session: s,
            id: JobId("j1".into()),
            kind: JobKind::Pty,
            call_id: "c3".into(),
            command: "bash -l".into(),
            name: Some("shell".into()),
            pgid: Some(4242),
            log: Some(PathBuf::from("/tmp/j1.log")),
            started_ms: 1000,
            state: JobState::Running,
            exit: None,
            settled_ms: None,
            engine: String::new(),
        };
        store.job_put(&j).await.unwrap();
        assert_eq!(store.jobs_open(Scope::Here).await.unwrap(), vec![j.clone()]);
        let screen = PtyScreenRow {
            session: s,
            job_id: JobId("j1".into()),
            revision: 418,
            rows: 40,
            cols: 120,
            cursor_row: 3,
            cursor_col: 7,
            text: "$ \n".into(),
            at_ms: 2000,
        };
        store.pty_screen_put(&screen).await.unwrap();
        store.pty_screen_put(&screen).await.unwrap();
        assert_eq!(
            store.pty_screen(s, &JobId("j1".into())).await.unwrap(),
            Some(screen)
        );
        assert!(
            store
                .job_settle(s, &JobId("j1".into()), JobState::Lost, &JobExit::Lost, 3000)
                .await
                .unwrap()
        );
        assert!(
            !store
                .job_settle(
                    s,
                    &JobId("j1".into()),
                    JobState::Settled,
                    &JobExit::Killed,
                    3001
                )
                .await
                .unwrap()
        );
        assert!(store.jobs_open(Scope::Here).await.unwrap().is_empty());
        let rows = store.jobs_of(s).await.unwrap();
        assert_eq!(
            (rows[0].state, rows[0].exit.clone(), rows[0].settled_ms),
            (JobState::Lost, Some(JobExit::Lost), Some(3000))
        );
        // Deleting the session takes every journal row (pty_screens cascade).
        store.delete_session(s).await.unwrap();
        assert!(store.jobs_of(s).await.unwrap().is_empty());
        assert!(
            store
                .pty_screen(s, &JobId("j1".into()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.resume_plan(s, t).await.is_err());
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM questions")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// 139 §5.3 / §7.2: `pty_screens` is attached — a screen row logs like
    /// every other journal row (inverts 109's `…_but_screens_do_not`).
    #[tokio::test]
    async fn pty_screen_rows_replicate() {
        let (store, s, _dir) = fresh().await;
        let before = store.log_after(0).await.unwrap().len();
        let t = TurnId::new();
        store
            .step(s, t, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        let after_step = store.log_after(0).await.unwrap().len();
        assert_eq!(after_step, before + 1);
        assert_eq!(
            store.log_after(0).await.unwrap().last().unwrap().session_id,
            s.to_string()
        );
        store
            .job_put(&JobRow {
                session: s,
                id: JobId("j1".into()),
                kind: JobKind::Shell,
                call_id: "c".into(),
                command: "ls".into(),
                name: None,
                pgid: None,
                log: None,
                started_ms: 1,
                state: JobState::Running,
                exit: None,
                settled_ms: None,
                engine: String::new(),
            })
            .await
            .unwrap();
        let after_job = store.log_after(0).await.unwrap().len();
        assert_eq!(after_job, after_step + 1);
        store
            .pty_screen_put(&PtyScreenRow {
                session: s,
                job_id: JobId("j1".into()),
                revision: 1,
                rows: 1,
                cols: 1,
                cursor_row: 0,
                cursor_col: 0,
                text: String::new(),
                at_ms: 1,
            })
            .await
            .unwrap();
        assert_eq!(store.log_after(0).await.unwrap().len(), after_job + 1);
    }

    /// A file written by engine `a` — one session with an open batch turn,
    /// a finished turn, a running job spawned on `a` and a `child_result`
    /// wait — reopened by engine `b` (the shape of
    /// `store::tests::writes_on_a_foreign_session_are_refused_until_taken`).
    async fn reopened_by_b() -> (Store, SessionId, TurnId, crate::TestDir) {
        let scratch = crate::TestDir::new("dur-ab");
        let dir = scratch.0.clone();
        let path = dir.join("sessions.sqlite");
        let a = Store::open(&path, Some(mesh::EngineId("a".into())))
            .await
            .unwrap();
        let foreign = SessionId::new();
        a.create_session(foreign, "m", None, None, None)
            .await
            .unwrap();
        let done = TurnId::new();
        a.step(foreign, done, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        a.finish_turn(foreign, done, "end_turn").await.unwrap();
        let open = batch_turn(&a, foreign).await;
        a.job_put(&JobRow {
            engine: "a".into(),
            ..job(foreign, "j1")
        })
        .await
        .unwrap();
        a.wait_put(&child_wait(foreign, "c1", "child-of-a"))
            .await
            .unwrap();
        a.close().await;
        let b = Store::open(&path, Some(mesh::EngineId("b".into())))
            .await
            .unwrap();
        (b, foreign, open, scratch)
    }

    fn job(session: SessionId, id: &str) -> JobRow {
        JobRow {
            session,
            id: JobId(id.into()),
            kind: JobKind::Shell,
            call_id: format!("call-{id}"),
            command: "sleep 600".into(),
            name: None,
            pgid: Some(4242),
            log: None,
            started_ms: 1,
            state: JobState::Running,
            exit: None,
            settled_ms: None,
            engine: String::new(),
        }
    }

    fn child_wait(session: SessionId, call_id: &str, child: &str) -> WaitRow {
        WaitRow {
            session,
            tool_call_id: call_id.into(),
            kind: WaitKind::ChildResult,
            target: child.into(),
            deadline: None,
            payload: json!({"agent": null}),
        }
    }

    /// 139 I1 / I9 (Revision 1 Q1): `Scope::Here` is the store's `here`
    /// form — `a`'s open turn and child wait are not `b`'s to act on,
    /// `Scope::All` lists them, `open_turn` (a display read) answers for
    /// any owner, and `b`'s prune tick leaves `a`'s finished journal alone
    /// (an `All` prune would hit the fence and fail as a whole).
    #[tokio::test]
    async fn open_turns_here_excludes_foreign_owners() {
        let (b, foreign, foreign_turn, _dir) = reopened_by_b().await;
        let mine = SessionId::new();
        b.create_session(mine, "m", None, None, None).await.unwrap();
        let my_turn = batch_turn(&b, mine).await;
        b.wait_put(&child_wait(mine, "c1", "child-of-b"))
            .await
            .unwrap();

        let ids = |v: Vec<(SessionId, TurnId, DateTime<Utc>)>| {
            v.into_iter().map(|(s, t, _)| (s, t)).collect::<Vec<_>>()
        };
        assert_eq!(
            ids(b.open_turns(Scope::Here).await.unwrap()),
            vec![(mine, my_turn)]
        );
        // Most recently active first: `mine` was written after `foreign`.
        assert_eq!(
            ids(b.open_turns(Scope::All).await.unwrap()),
            vec![(mine, my_turn), (foreign, foreign_turn)]
        );
        assert_eq!(
            b.open_turn(foreign).await.unwrap().map(|(t, _)| t),
            Some(foreign_turn)
        );
        assert_eq!(
            b.child_waits(Scope::Here)
                .await
                .unwrap()
                .iter()
                .map(|w| w.target.as_str())
                .collect::<Vec<_>>(),
            ["child-of-b"]
        );
        assert_eq!(b.child_waits(Scope::All).await.unwrap().len(), 2);

        // `a`'s finished turn is older than a 0-day keep, yet `b` never prunes it.
        assert_eq!(b.journal_prune(Scope::Here, 0).await.unwrap(), 0);
        let kept: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM turn_steps WHERE session_id = ? AND kind = 'turn_finished'",
        )
        .bind(foreign.to_string())
        .fetch_one(&b.read)
        .await
        .unwrap();
        assert_eq!(kept, 1);
        assert!(b.journal_prune(Scope::All, 0).await.is_err(), "fenced");
    }

    /// 139 D14 / I6: `jobs_open(Here)` keeps the rows of sessions owned
    /// here that this engine spawned (`engine IN ('', me)`); a foreign
    /// session's row and a foreign engine's row are left to
    /// `jobs_settle_transferred`, which settles the latter `lost` without
    /// a signal and returns it settled.
    #[tokio::test]
    async fn jobs_open_here_filters_by_engine() {
        let (b, _foreign, _, _dir) = reopened_by_b().await;
        let mine = SessionId::new();
        b.create_session(mine, "m", None, None, None).await.unwrap();
        for (id, engine) in [("j1", ""), ("j2", "b"), ("j3", "a")] {
            b.job_put(&JobRow {
                engine: engine.into(),
                ..job(mine, id)
            })
            .await
            .unwrap();
        }
        let ids = |rows: Vec<JobRow>| rows.into_iter().map(|j| j.id.0).collect::<Vec<_>>();
        assert_eq!(ids(b.jobs_open(Scope::Here).await.unwrap()), ["j1", "j2"]);
        assert_eq!(b.jobs_open(Scope::All).await.unwrap().len(), 4);

        let settled = b.jobs_settle_transferred(mine, "b", 9_000).await.unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(
            (
                settled[0].id.0.as_str(),
                settled[0].state,
                settled[0].exit.clone(),
                settled[0].settled_ms,
                settled[0].engine.as_str()
            ),
            ("j3", JobState::Lost, Some(JobExit::Lost), Some(9_000), "a")
        );
        assert_eq!(b.jobs_open(Scope::All).await.unwrap().len(), 3);
        assert!(
            b.jobs_settle_transferred(mine, "b", 9_001)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// `resume_reports`: round trip, upsert, `latest` follows the newest
    /// turn, the row replicates, and prune / delete take it.
    #[tokio::test]
    async fn resume_reports_round_trip_latest_and_prune() {
        let (store, s, _dir) = fresh().await;
        let t = batch_turn(&store, s).await;
        assert!(store.resume_report(s, t).await.unwrap().is_none());
        assert!(store.latest_resume_report(s).await.unwrap().is_none());
        let now = Utc::now();
        let row = ResumeReportRow {
            session: s,
            turn: t,
            kind: ResumeKind::Suspended,
            cause: ResumeCause::Transfer,
            interrupted_at: now - chrono::Duration::seconds(5),
            at: now,
            report: crate::ResumeReport {
                replayed: vec![("c1".into(), "read_file".into())],
                interrupted: vec![("c2".into(), "write_file".into())],
                lost_jobs: vec![JobId("j1".into())],
                children_restored: 1,
                children_handed_off: 2,
            },
        };
        let before = store.log_after(0).await.unwrap().len();
        store.resume_report_put(&row).await.unwrap();
        assert_eq!(store.log_after(0).await.unwrap().len(), before + 1);
        assert_eq!(store.resume_report(s, t).await.unwrap(), Some(row.clone()));
        assert_eq!(
            store.latest_resume_report(s).await.unwrap(),
            Some(row.clone())
        );
        // A second resume of the same turn overwrites.
        let again = ResumeReportRow {
            kind: ResumeKind::Interrupted,
            report: crate::ResumeReport::default(),
            ..row
        };
        store.resume_report_put(&again).await.unwrap();
        assert_eq!(
            store.latest_resume_report(s).await.unwrap(),
            Some(again.clone())
        );
        // A newer turn without a report hides it from `latest`, not from `resume_report`.
        store.finish_turn(s, t, "end_turn").await.unwrap();
        let t2 = TurnId::new();
        store
            .step(s, t2, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        assert!(store.latest_resume_report(s).await.unwrap().is_none());
        assert_eq!(
            store.resume_report(s, t).await.unwrap(),
            Some(again.clone())
        );
        // Pruning the finished turn takes its report; the open turn's journal stays.
        assert!(store.journal_prune(Scope::Here, 0).await.unwrap() > 0);
        assert!(store.resume_report(s, t).await.unwrap().is_none());
        assert!(store.resume_plan(s, t2).await.is_ok());
        // Deleting the session takes the open turn's report with the journal.
        store
            .resume_report_put(&ResumeReportRow { turn: t2, ..again })
            .await
            .unwrap();
        assert!(store.latest_resume_report(s).await.unwrap().is_some());
        store.delete_session(s).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM resume_reports")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// 178: a turn journaled over `end` s — opened, a model call, drained
    /// with `ran_ms` = `ran` at 20 s, then run from 25 s to `end` after the
    /// resume.
    fn drained_turn(ran: Value, end: i64) -> Vec<StepRow> {
        let t0 = Utc::now() - chrono::Duration::hours(2);
        let row = |kind: StepKind, payload: Value, secs| StepRow {
            kind: kind.as_str().into(),
            call_id: None,
            payload: payload.to_string(),
            at: (t0 + chrono::Duration::seconds(secs)).to_rfc3339(),
        };
        vec![
            row(StepKind::InputAccepted, json!({"source": "user"}), 0),
            row(StepKind::ModelCall, json!({}), 10),
            row(
                StepKind::Suspended,
                json!({"reason": "shutdown", "ran_ms": ran}),
                20,
            ),
            row(StepKind::ModelCall, json!({}), 25),
            row(StepKind::ModelCall, json!({}), end),
        ]
    }

    /// 178: a `ran_ms` beyond the turn's whole journal span (27 s) plus the
    /// 5 s slack cannot have run — `u64::MAX`, past `i64::MAX`, or just over
    /// — so the span is the run time; a sane one, or one inside the slack
    /// (clock drift), still counts with the run after it. On a 1 h span the
    /// slack is 1 % (36 s): 2 s over is kept, 1 h over is not.
    #[test]
    fn run_ms_ignores_an_impossible_ran_ms() {
        let (s, t) = (SessionId::new(), TurnId::new());
        let run = |ran: u64, end| run_ms(s, t, &drained_turn(json!(ran), end)).unwrap();
        for bad in [u64::MAX, i64::MAX as u64 + 1, 32_001] {
            assert_eq!(run(bad, 27), 27_000, "{bad}");
        }
        assert_eq!(run(15_000, 27), 17_000);
        assert_eq!(run(32_000, 27), 34_000, "inside the slack");
        assert_eq!(run(3_602_000, 3600), 3_602_000 + 3_575_000, "drift kept");
        assert_eq!(run(7_200_000, 3600), 3_600_000, "1 h over");
    }

    /// 178: a `ran_ms` that is no `u64` is no run clock: the span counts.
    #[test]
    fn run_ms_ignores_a_non_numeric_ran_ms() {
        let (s, t) = (SessionId::new(), TurnId::new());
        for bad in [json!("15000"), json!(-5), json!(1.5)] {
            assert_eq!(
                run_ms(s, t, &drained_turn(bad.clone(), 27)).unwrap(),
                27_000,
                "{bad}"
            );
        }
    }
}
