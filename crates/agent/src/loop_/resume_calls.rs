//! Design doc 109 §4.2: the tool-specific resume rules a replayed batch
//! applies to a `started` call. The contract wave leaves one stub per owning
//! wave, each answering the generic interrupted note (§4.3); the per-call
//! rule in `loop_.rs` (W1) routes `shell` / `start` / `send` / `wait` to
//! [`resume_job_call`], the human-wait tools to [`resume_question_call`] and
//! `send_message{await_reply}` / `spawn_subagent` to [`resume_wait_call`].

use chrono::{DateTime, Local, Utc};
use kameo::actor::ActorRef;
use kameo::error::SendError;
use runtime::SessionId;
use tools::{JobKind, JobRow, ToolOutput};

use crate::durability::jobs::{hhmmss_ms, log_tail};
use crate::durability::{JournaledCall, ResumeKind, ResumePlan};
use crate::store::Store;
use crate::supervisor::{DropWait, ResumeQuestion, ResumeWait, remaining};

/// The §4.3 interrupted note: a `started` mutating call the restart must not
/// repeat. `ERROR:`-prefixed so the model's tool-error handling applies.
pub(crate) fn interrupted_note(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    let started = call
        .started_at
        .map(|t| format!(" (started {})", hhmmss(t)))
        .unwrap_or_default();
    ToolOutput::err(format!(
        "ERROR: interrupted: the engine restarted at {} while this call was running{started}; \
         its effect is unknown — check before repeating it.",
        hhmmss(plan.interrupted_at)
    ))
}

/// Local `HH:MM:SS`, as §4.3's examples read.
fn hhmmss(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M:%S").to_string()
}

/// The job rule (§5.4, §5.6): the `shell` / `start` / `send` / `wait` call
/// that was `started` when the engine died. The job is gone (R1) — dead
/// with the engine or reaped, its row `lost` by the time any turn resumes.
///
/// - `shell` / `start` with a `jobs` row for this call → `Some(job note)`:
///   the interrupted note extended with the job id, log path and the last
///   40 log lines (the last screen for a PTY). No `<job_result>` follows: the
///   model never had the stub, this note is its one report.
/// - `shell` / `start` with **no** row (the kill fell between approval and
///   spawn) → `None`: nothing ran; the caller treats the row as `planned`.
/// - `send` → the interrupted note plus the last screen (typed bytes are an
///   unknown effect).
/// - `wait` → the tool's own exited reply, state `lost`, with the last
///   screen; not an error.
pub(crate) async fn resume_job_call(
    store: &Store,
    session: SessionId,
    call: &JournaledCall,
    plan: &ResumePlan,
) -> Option<ToolOutput> {
    let rows = match store.jobs_of(session).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("109: jobs_of for the job rule: {e:#}");
            return Some(interrupted_note(call, plan));
        }
    };
    let by_call = || rows.iter().find(|j| j.call_id == call.call_id);
    let by_arg = || {
        let id = call.args.get("id").and_then(|v| v.as_str())?;
        rows.iter()
            .rev()
            .find(|j| j.id.0 == id || j.name.as_deref() == Some(id))
    };
    let (job, what) = match call.name.as_str() {
        "shell" => (by_call()?, "command"),
        "start" => (by_call()?, "terminal"),
        "send" => match by_arg() {
            Some(j) => (j, "terminal"),
            None => return Some(interrupted_note(call, plan)),
        },
        "wait" => {
            let Some(j) = by_arg() else {
                return Some(interrupted_note(call, plan));
            };
            return Some(lost_wait_reply(store, session, j, plan).await);
        }
        _ => return Some(interrupted_note(call, plan)),
    };
    let how = match plan.kind {
        ResumeKind::Interrupted => {
            format!("the engine restarted at {}", hhmmss(plan.interrupted_at))
        }
        ResumeKind::Suspended => format!(
            "the engine was stopped at {} and restarted",
            hhmmss(plan.interrupted_at)
        ),
    };
    let mut text = format!(
        "ERROR: interrupted: {how} while this {what} ran (job {}, started {}); the job died with the \
         engine and its exit code is unknown — check before repeating it.",
        job.id,
        hhmmss_ms(job.started_ms)
    );
    append_last_seen(&mut text, store, session, job).await;
    Some(ToolOutput::err(text))
}

/// `wait`'s exited reply for a lost PTY (§5.6): the `send`/`screen`/`wait`
/// shape with state `lost`, the §5.3 banner, and the stored screen.
async fn lost_wait_reply(
    store: &Store,
    session: SessionId,
    job: &JobRow,
    plan: &ResumePlan,
) -> ToolOutput {
    let secs = job
        .settled_ms
        .map_or(0, |s| (s - job.started_ms).max(0) / 1000);
    let label = match &job.name {
        Some(n) => format!("{} ({n})", job.id),
        None => job.id.0.clone(),
    };
    let mut content = format!("job {label} · lost · {secs}s\n");
    let screen = store.pty_screen(session, &job.id).await.ok().flatten();
    match &screen {
        Some(s) => {
            content.push_str(&format!(
                "[process gone — the engine restarted at {}; this is the last screen captured at {} (revision {})]\nscreen:\n{}",
                hhmmss(plan.interrupted_at),
                hhmmss_ms(s.at_ms),
                s.revision,
                s.text
            ));
            content.push_str(&format!(
                "\ncursor: row {} col {}",
                s.cursor_row + 1,
                s.cursor_col + 1
            ));
        }
        None => content.push_str(&format!(
            "[process gone — the engine restarted at {}; no screen was captured]",
            hhmmss(plan.interrupted_at)
        )),
    }
    let rows: Vec<&str> = screen
        .as_ref()
        .map_or(Vec::new(), |s| s.text.split('\n').collect());
    ToolOutput {
        content,
        is_error: false,
        structured: Some(serde_json::json!({
            "job": job.id, "rows": rows, "matched": null, "exit": "lost",
        })),
        files: Vec::new(),
    }
}

/// `Log: … (last 40 lines follow)` and the lines (§4.3), or the stored screen
/// for a PTY job.
async fn append_last_seen(text: &mut String, store: &Store, session: SessionId, job: &JobRow) {
    let Some(log) = &job.log else { return };
    match job.kind {
        JobKind::Pty => match store.pty_screen(session, &job.id).await.ok().flatten() {
            Some(s) => text.push_str(&format!(
                "\nLog: {} — last screen (captured at {}, revision {}):\n{}",
                log.display(),
                hhmmss_ms(s.at_ms),
                s.revision,
                s.text
            )),
            None => text.push_str(&format!(
                "\nLog: {} (no screen was captured)",
                log.display()
            )),
        },
        JobKind::Shell => {
            let tail = log_tail(log);
            let lines: Vec<&str> = tail.lines().collect();
            let last = &lines[lines.len().saturating_sub(40)..];
            text.push_str(&format!(
                "\nLog: {} (last {} lines follow)\n{}",
                log.display(),
                last.len(),
                last.join("\n")
            ));
        }
    }
}

/// The question rule (§6.2, W3): the call's newest `questions` row is handed
/// to its tool through `tools::registry::republish` and the call is
/// re-invoked — the body re-arms the *same* question (same id, `deadline -
/// now`) instead of raising a new one, renders an `answered` row without
/// asking, and answers a passed deadline with its own timeout text. With no
/// row at all (the kill fell before the question was journaled) the call
/// re-runs and asks afresh.
pub(crate) async fn resume_question_call(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    let Ok(agents) = crate::peers::supervisor() else {
        return interrupted_note(call, plan);
    };
    let (session, row) = match agents
        .ask(ResumeQuestion {
            turn: plan.turn,
            call_id: call.call_id.clone(),
        })
        .await
    {
        Ok(r) => r,
        Err(SendError::HandlerError(e)) => {
            return ToolOutput::err(format!("{}: resume: {e}", call.name));
        }
        Err(e) => return ToolOutput::err(format!("{}: agent supervisor: {e}", call.name)),
    };
    if let Some(row) = row {
        // A passed deadline (§6.3) is the tool's own timeout: each body
        // settles the row `timed_out` when it finds nothing left to wait.
        tools::registry::republish(row);
    }
    let Ok(Some(registry)) = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR) else {
        return ToolOutput::err(format!("{}: tool registry not running", call.name));
    };
    match registry
        .ask(tools::Invoke(tools::ToolCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            args: call.args.clone(),
            session,
            quiet: false,
        }))
        .await
    {
        Ok(out) => out,
        Err(e) => ToolOutput::err(format!("tool registry: {e}")),
    }
}

/// The wait rule (§4.4, W3) for `send_message{await_reply}`: every awaited
/// exchange already `replied` is the result as it stands; the ones still
/// open are awaited again on the waiter `rebuild_waits` re-armed, until the
/// row's remaining deadline — a passed deadline is the tool's own timeout
/// receipt. A `child_result` wait delivers the spared `result:` text. No
/// `waits` row (nothing was accepted before the kill) → the interrupted note.
pub(crate) async fn resume_wait_call(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    let Ok(agents) = crate::peers::supervisor() else {
        return interrupted_note(call, plan);
    };
    let mut w = match agents
        .ask(ResumeWait {
            turn: plan.turn,
            call_id: call.call_id.clone(),
        })
        .await
    {
        Ok(w) => w,
        Err(SendError::HandlerError(e)) => {
            return ToolOutput::err(format!("{}: resume: {e}", call.name));
        }
        Err(e) => return ToolOutput::err(format!("{}: agent supervisor: {e}", call.name)),
    };
    let Some(row) = w.row.take() else {
        return interrupted_note(call, plan);
    };
    if !w.child_results.is_empty() {
        return ToolOutput::ok(w.child_results.join("\n\n"));
    }
    if w.receipts.is_empty() {
        return interrupted_note(call, plan);
    }
    let deadline = row
        .deadline
        .and_then(remaining)
        .map(|d| tokio::time::Instant::now() + d);
    // The tool's own wording: the wait began with the call, so the row's
    // deadline minus `started_at` is the timeout it was given.
    let after = match (row.deadline, call.started_at) {
        (Some(d), Some(s)) => format!(" after {}s", (d - s).num_seconds().max(0)),
        _ => String::new(),
    };
    for r in &mut w.receipts {
        let Some(rx) = w.pending.remove(&r.id) else {
            continue;
        };
        let fin = match deadline {
            Some(at) => tokio::time::timeout_at(at, rx).await.ok(),
            None => None,
        };
        match fin {
            Some(Ok(fin)) => *r = fin,
            Some(Err(_)) => r.error = Some("wait dropped".into()),
            // Deadline passed, now or while down: the timeout receipt.
            None => {
                let _ = agents.tell(DropWait(r.id)).await;
                r.error = Some(format!("timed out{after}; message not recalled"));
            }
        }
    }
    crate::peers::receipts_output(&w.receipts)
}
