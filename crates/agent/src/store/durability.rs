//! Design doc 109 §3: the durability journal on the session store —
//! `turn_steps`, `tool_calls`, `questions`, `waits`, `jobs`, `pty_screens`
//! (migration `0033_durability.sql`). Every write is one [`WriteTx`] or
//! joins the caller's (`*_in`), so `session_log` gets its row (§3.5);
//! `pty_screens` is the one table left out of `capture::ATTACHED`.
//! `resume_plan` turns the newest step of an open turn into the
//! [`ResumePlan`] §4.2 acts on.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use genai::chat::{MessageContent, ToolCall};
use runtime::{SessionId, TurnId};
use serde_json::Value;
use sqlx::SqliteConnection;
use tools::{
    JobExit, JobId, JobKind, JobRow, JobState, PtyScreenRow, QuestionKind, QuestionRow,
    QuestionState, Risk, ToolOutput, WaitKind, WaitRow,
};

use super::Store;
use crate::durability::{
    CallState, JournaledCall, LastStep, PartialContent, ResumeKind, ResumePlan, StepKind,
};

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
#[allow(dead_code)] // 109: W1 calls it from the loop's final persist
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
#[allow(dead_code)] // 109: W1 calls it from the loop's tool_use persist
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
    for table in ["waits", "questions", "tool_calls", "turn_steps", "jobs"] {
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

/// One `turn_steps` row as read.
struct StepRow {
    kind: String,
    call_id: Option<String>,
    payload: String,
    at: String,
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
);

fn question_of(r: QuestionTuple) -> anyhow::Result<QuestionRow> {
    let (id, session, tool_call_id, kind, payload, state, asked_at, deadline, answered_at, answer) =
        r;
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
    })
}

const QUESTION_COLS: &str =
    "id, session_id, tool_call_id, kind, payload, state, asked_at, deadline, answered_at, answer";

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
);

fn job_of(r: JobTuple) -> anyhow::Result<JobRow> {
    let (session, id, kind, call_id, command, name, pgid, log, started_ms, state, exit, settled_ms) =
        r;
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
    })
}

const JOB_COLS: &str =
    "session_id, id, kind, call_id, command, name, pgid, log, started_ms, state, exit, settled_ms";

/// The window that picks each turn's newest non-partial step and its last
/// activity time (`last_at` includes partial rows).
const NEWEST: &str = "SELECT session_id, turn_id, kind, \
     ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY n DESC) AS rn, \
     (SELECT MAX(at) FROM turn_steps t2 WHERE t2.session_id = t1.session_id AND t2.turn_id = t1.turn_id) AS last_at \
     FROM turn_steps t1 WHERE kind != 'assistant_partial'";

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
    pub async fn open_turns(&self) -> anyhow::Result<Vec<(SessionId, TurnId, DateTime<Utc>)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT session_id, turn_id, last_at FROM ({NEWEST}) \
             WHERE rn = 1 AND kind != 'turn_finished' ORDER BY last_at DESC"
        )))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(s, t, at)| Ok((parse_session(&s)?, parse_turn(&t)?, parse_dt(&at)?)))
            .collect()
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
        let interrupted_at = steps
            .iter()
            .map(|s| parse_dt(&s.at))
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .max()
            .expect("non-empty");
        let mut kind = ResumeKind::Interrupted;
        let mut newest = None;
        for s in steps.iter().rev() {
            match StepKind::parse(&s.kind) {
                Some(StepKind::AssistantPartial) => continue,
                Some(StepKind::Suspended) => kind = ResumeKind::Suspended,
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
            interrupted_at,
            last,
            calls,
            partial,
        })
    }

    // ---- questions (§6) -----------------------------------------------------

    /// Upsert a `questions` row by `q.id` (raise, re-publish, resolve).
    pub async fn question_put(&self, q: &QuestionRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO questions (id, session_id, tool_call_id, kind, payload, state, asked_at, deadline, answered_at, answer) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET tool_call_id = excluded.tool_call_id, kind = excluded.kind, \
                 payload = excluded.payload, state = excluded.state, asked_at = excluded.asked_at, \
                 deadline = excluded.deadline, answered_at = excluded.answered_at, answer = excluded.answer",
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

    /// Upsert a `jobs` row by `(j.session, j.id)`.
    pub async fn job_put(&self, j: &JobRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO jobs (session_id, id, kind, call_id, command, name, pgid, log, started_ms, state, exit, settled_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, id) DO UPDATE SET kind = excluded.kind, call_id = excluded.call_id, \
                 command = excluded.command, name = excluded.name, pgid = excluded.pgid, log = excluded.log, \
                 started_ms = excluded.started_ms, state = excluded.state, exit = excluded.exit, \
                 settled_ms = excluded.settled_ms",
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

    /// Every `running` job row, across sessions (the reaper's input, §5.4).
    pub async fn jobs_open(&self) -> anyhow::Result<Vec<JobRow>> {
        let rows: Vec<JobTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {JOB_COLS} FROM jobs WHERE state = 'running' ORDER BY started_ms, rowid"
        )))
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(job_of).collect()
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

    /// Upsert the last screen of a PTY job (§5.3). Engine-local: not in the
    /// capture, so it leaves no `session_log` row. The job row must exist.
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

    // ---- retention (§3.6) ----------------------------------------------------

    /// Delete the journal of every **finished** turn whose last activity is
    /// older than `keep_days`, and every terminal question older than that.
    /// Open turns and pending questions are never pruned. Returns the number
    /// of `turn_steps` rows removed.
    pub async fn journal_prune(&self, keep_days: u32) -> anyhow::Result<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(i64::from(keep_days))).to_rfc3339();
        let fin = format!(
            "WITH fin AS (SELECT session_id, turn_id FROM ({NEWEST}) \
             WHERE rn = 1 AND kind = 'turn_finished' AND last_at < ?1)"
        );
        let mut tx = self.begin_write().await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{fin} DELETE FROM waits WHERE (session_id, tool_call_id) IN \
             (SELECT c.session_id, c.call_id FROM tool_calls c JOIN fin USING (session_id, turn_id))"
        )))
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{fin} DELETE FROM tool_calls WHERE (session_id, turn_id) IN (SELECT session_id, turn_id FROM fin)"
        )))
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?;
        let steps = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{fin} DELETE FROM turn_steps WHERE (session_id, turn_id) IN (SELECT session_id, turn_id FROM fin)"
        )))
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "DELETE FROM questions WHERE state != 'pending' AND COALESCE(answered_at, deadline) < ?",
        )
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

    async fn fresh() -> (Store, SessionId, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("theuth-dur-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        (store, id, dir)
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
        let (store, s, dir) = fresh().await;
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
        assert!(store.open_turns().await.unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn batch_plan_tracks_call_states() {
        let (store, s, dir) = fresh().await;
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
        let open = store.open_turns().await.unwrap();
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
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn open_turns_order_and_prune() {
        let (store, s, dir) = fresh().await;
        let other = SessionId::new();
        store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();
        let old = batch_turn(&store, s).await;
        let newer = batch_turn(&store, other).await;
        let open = store.open_turns().await.unwrap();
        assert_eq!(
            open.iter().map(|(s, t, _)| (*s, *t)).collect::<Vec<_>>(),
            vec![(other, newer), (s, old)]
        );
        // An open turn is never pruned, however old.
        assert_eq!(store.journal_prune(0).await.unwrap(), 0);
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
        assert_eq!(store.open_turns().await.unwrap().len(), 1);
        // Fresh rows survive a 7-day keep; a 0-day keep (cutoff = now) takes them.
        assert_eq!(store.journal_prune(7).await.unwrap(), 0);
        assert_eq!(store.waits_of(s, old).await.unwrap().len(), 1);
        let n = store.journal_prune(0).await.unwrap();
        assert_eq!(n, 4);
        assert!(store.calls_of(s, old).await.unwrap().is_empty());
        assert!(store.waits_of(s, old).await.unwrap().is_empty());
        assert!(store.resume_plan(s, old).await.is_err());
        // The other session's open turn is untouched.
        assert_eq!(store.calls_of(other, newer).await.unwrap().len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn questions_waits_jobs_screens_round_trip() {
        let (store, s, dir) = fresh().await;
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
        };
        store.job_put(&j).await.unwrap();
        assert_eq!(store.jobs_open().await.unwrap(), vec![j.clone()]);
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
        assert!(store.jobs_open().await.unwrap().is_empty());
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
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn journal_rows_replicate_but_screens_do_not() {
        let (store, s, dir) = fresh().await;
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
        assert_eq!(store.log_after(0).await.unwrap().len(), after_job);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
