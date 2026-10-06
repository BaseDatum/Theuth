//! Design doc 109 §4.2: the tool-specific resume rules a replayed batch
//! applies to a `started` call. The contract wave leaves one stub per owning
//! wave, each answering the generic interrupted note (§4.3); the per-call
//! rule in `loop_.rs` (W1) routes `shell` / `start` / `send` / `wait` to
//! [`resume_job_call`], the human-wait tools to [`resume_question_call`] and
//! `send_message{await_reply}` / `spawn_subagent` to [`resume_wait_call`].

use chrono::{DateTime, Local, Utc};
use runtime::SessionId;
use tools::{JobKind, JobRow, ToolOutput};

use crate::durability::jobs::{hhmmss_ms, log_tail};
use crate::durability::{JournaledCall, ResumeKind, ResumePlan};
use crate::store::Store;

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

/// **W3 fills this.** The question rule (§6.2): re-publish the pending
/// `questions` row under the same id with the remaining deadline; an
/// answered row is honoured; a passed deadline is the tool's own timeout.
pub(crate) async fn resume_question_call(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    interrupted_note(call, plan)
}

/// **W3 fills this.** The wait rule (§4.4): a replied exchange or spared
/// `result:` row is delivered as the call's result; otherwise the wait is
/// re-created with `deadline - now`; a passed deadline is the timeout receipt.
pub(crate) async fn resume_wait_call(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    interrupted_note(call, plan)
}
