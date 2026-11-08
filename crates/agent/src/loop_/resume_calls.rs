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

use crate::durability::jobs::{hhmmss_ms, job_of_call, log_tail};
use crate::durability::{JournaledCall, ResumeCause, ResumeKind, ResumePlan};
use crate::store::Store;
use crate::supervisor::{DropWait, ResumeQuestion, ResumeWait, remaining};

/// The §4.3 interrupted note: a `started` mutating call the restart must not
/// repeat. `ERROR:`-prefixed so the model's tool-error handling applies.
/// After a transfer / takeover (139 §6.3) the cause is the move, not a
/// restart.
pub(crate) fn interrupted_note(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    let started = call
        .started_at
        .map(|t| format!(" (started {})", hhmmss(t)))
        .unwrap_or_default();
    ToolOutput::err(format!(
        "ERROR: interrupted: {} while this call was running{started}; \
         its effect is unknown — check before repeating it.",
        how(plan)
    ))
}

/// Why the call was cut short, by cause: a restart, or the session moving
/// to this engine (139 §6.3) — spelled for the `interrupted:` notes.
fn how(plan: &ResumePlan) -> String {
    let at = hhmmss(plan.interrupted_at);
    match plan.cause {
        ResumeCause::Transfer => format!("the session moved to this engine at {at}"),
        ResumeCause::Takeover => format!("this engine took the session over at {at}"),
        _ => format!("the engine restarted at {at}"),
    }
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
    // `job_of_call`: a bridged `shell` / `start` spawns under `<call_id>.<n>`.
    let by_call = || {
        rows.iter()
            .find(|j| !j.is_cell_kernel() && job_of_call(&j.call_id, &call.call_id))
    };
    let by_arg = || {
        let id = call.args.get("id").and_then(|v| v.as_str())?;
        rows.iter()
            .rev()
            .find(|j| j.id.0 == id || j.name.as_deref() == Some(id))
    };
    // 171F3 R1 (171 §9.7, 171F B23): a user-cell kernel is neither typed into
    // nor read by the model, lost or live — no last screen through the resume.
    let refused = || {
        let id = call.args.get("id").and_then(|v| v.as_str())?;
        (id.starts_with(tools::jobs::CELL_PREFIX) || by_arg().is_some_and(JobRow::is_cell_kernel))
            .then(|| ToolOutput::err(tools::jobs::CELL_REFUSED))
    };
    if matches!(call.name.as_str(), "send" | "wait")
        && let Some(out) = refused()
    {
        return Some(out);
    }
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
    // 139 §7.1: a job another engine spawned did not die with a restart —
    // the session moved here and the process stayed behind. 183 §11: a
    // transfer or takeover says so even when the job row names no other
    // engine (pre-139 rows, or a row the move outran).
    let me = store.engine();
    let elsewhere = !job.engine.is_empty() && job.engine != me;
    let moved = elsewhere || matches!(plan.cause, ResumeCause::Transfer | ResumeCause::Takeover);
    // 183 R1 (H2): the origin by its mesh name, never a hex id.
    let origin = if elsewhere {
        crate::communication::mesh::machine_name_now(&job.engine).await
    } else {
        "another machine".to_owned()
    };
    let how = match plan.kind {
        _ if moved => format!(
            "the session moved here from {origin} at {}",
            hhmmss(plan.interrupted_at)
        ),
        ResumeKind::Interrupted => {
            format!("the engine restarted at {}", hhmmss(plan.interrupted_at))
        }
        ResumeKind::Suspended => format!(
            "the engine was stopped at {} and restarted",
            hhmmss(plan.interrupted_at)
        ),
    };
    let fate = if moved {
        format!("the job stayed on {origin} and its exit code is unknown here")
    } else {
        "the job died with the engine and its exit code is unknown".to_owned()
    };
    let mut text = format!(
        "ERROR: interrupted: {how} while this {what} ran (job {}, started {}); {fate} — check \
         before repeating it.",
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
    // 139 §7.2: the page is the previous owner's, replicated.
    let (gone, on) = if !job.engine.is_empty() && job.engine != store.engine() {
        let from = crate::communication::mesh::machine_name_now(&job.engine).await;
        (
            format!(
                "the session moved from {from} at {}",
                hhmmss(plan.interrupted_at)
            ),
            format!(" on {from}"),
        )
    } else if matches!(plan.cause, ResumeCause::Transfer | ResumeCause::Takeover) {
        // 183 §11: a move, even when the row names no other engine.
        (how(plan), String::new())
    } else {
        (
            format!("the engine restarted at {}", hhmmss(plan.interrupted_at)),
            String::new(),
        )
    };
    match &screen {
        Some(s) => {
            content.push_str(&format!(
                "[process gone — {gone}; this is the last screen captured{on} at {} (revision {})]\nscreen:\n{}",
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
        None => content.push_str(&format!("[process gone — {gone}; no screen was captured]")),
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
        JobKind::Shell | JobKind::Promoted => {
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
pub(crate) async fn resume_question_call(
    session: SessionId,
    call: &JournaledCall,
    plan: &ResumePlan,
) -> ToolOutput {
    let Ok(agents) = crate::peers::supervisor() else {
        return interrupted_note(call, plan);
    };
    let row = match agents
        .ask(ResumeQuestion {
            session,
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
///
/// 139 §7.3 / Revision 1 Q7: after a transfer the exchange receipts live in
/// the previous owner's engine-local `peer_exchanges`, so nothing re-arms
/// here; the note says the reply, if any, arrives as a `<message>` (69 P
/// forwards it to the owner).
pub(crate) async fn resume_wait_call(
    session: SessionId,
    call: &JournaledCall,
    plan: &ResumePlan,
) -> ToolOutput {
    let Ok(agents) = crate::peers::supervisor() else {
        return interrupted_note(call, plan);
    };
    let mut w = match agents
        .ask(ResumeWait {
            session,
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
        return transfer_wait_note(call, plan).unwrap_or_else(|| interrupted_note(call, plan));
    };
    if !w.child_results.is_empty() {
        return ToolOutput::ok(w.child_results.join("\n\n"));
    }
    if w.receipts.is_empty() {
        return transfer_wait_note(call, plan).unwrap_or_else(|| interrupted_note(call, plan));
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

/// 139 §7.3's wording for a peer wait cut by a transfer / takeover;
/// `None` for every other cause (the §4.3 interrupted note applies).
fn transfer_wait_note(call: &JournaledCall, plan: &ResumePlan) -> Option<ToolOutput> {
    if !matches!(plan.cause, ResumeCause::Transfer | ResumeCause::Takeover) {
        return None;
    }
    let started = call
        .started_at
        .map(|t| format!(" (started {})", hhmmss(t)))
        .unwrap_or_default();
    Some(ToolOutput::err(format!(
        "ERROR: interrupted: waiting for a reply was interrupted{started} — {}; the reply, if any, \
         arrives as a <message> and is not lost. Do not resend the message.",
        how(plan)
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tools::{JobExit, JobKind, JobRow, JobState, Risk};

    use super::*;
    use crate::durability::{CallState, LastStep};
    use runtime::TurnId;

    fn plan(cause: ResumeCause) -> ResumePlan {
        ResumePlan {
            turn: TurnId::new(),
            kind: ResumeKind::Suspended,
            cause,
            interrupted_at: Utc::now(),
            ran_ms: 0,
            bounded: false,
            last: LastStep::ToolBatch {
                all_finished: false,
            },
            calls: Vec::new(),
            partial: None,
        }
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> JournaledCall {
        JournaledCall {
            call_id: id.into(),
            name: name.into(),
            args,
            risk: Risk::Destructive,
            state: CallState::Started,
            attempt: 1,
            started_at: None,
            output: None,
            is_error: false,
            files: Vec::new(),
        }
    }

    /// 139 §6.3 / §7.3 (Q7): the generic note and the peer-wait note name
    /// the move for `transfer` / `takeover` and keep 109's restart wording
    /// otherwise; the peer-wait note exists only for the move causes.
    #[test]
    fn notes_spell_the_cause() {
        let c = call("c1", "send_message", json!({}));
        let crash = interrupted_note(&c, &plan(ResumeCause::Crash));
        assert!(
            crash
                .content
                .starts_with("ERROR: interrupted: the engine restarted at "),
            "{}",
            crash.content
        );
        let moved = interrupted_note(&c, &plan(ResumeCause::Transfer));
        assert!(
            moved
                .content
                .starts_with("ERROR: interrupted: the session moved to this engine at "),
            "{}",
            moved.content
        );
        assert!(transfer_wait_note(&c, &plan(ResumeCause::Shutdown)).is_none());
        let w = transfer_wait_note(&c, &plan(ResumeCause::Takeover)).unwrap();
        assert!(w.is_error);
        assert!(
            w.content.starts_with("ERROR: interrupted: waiting for a reply was interrupted — this engine took the session over at "),
            "{}",
            w.content
        );
        assert!(
            w.content.contains("arrives as a <message>"),
            "{}",
            w.content
        );
    }

    /// 139 §7.1 / §7.2: on engine `b`, the job note and the lost `wait`
    /// reply for a row with `engine = "a"` say the session moved from that
    /// machine and that the page was captured there. 183 R1 (H2): by its mesh
    /// name; `a` is unknown to this test's (absent) mesh, so "another
    /// machine", never the id.
    #[tokio::test]
    async fn job_note_names_the_engine_that_ran_it() {
        let scratch = crate::TestDir::new("139-w3");
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("b".into())),
        )
        .await
        .unwrap();
        let s = SessionId::new();
        store
            .create_session(s, "m", None, None, None)
            .await
            .unwrap();
        let row = |id: &str, call_id: &str, kind: JobKind| JobRow {
            session: s,
            id: tools::JobId(id.into()),
            kind,
            call_id: call_id.into(),
            command: "x".into(),
            name: None,
            pgid: None,
            log: Some(std::path::PathBuf::from("/nonexistent/j.log")),
            started_ms: 1_000,
            state: JobState::Lost,
            exit: Some(JobExit::Lost),
            settled_ms: Some(9_000),
            engine: "a".into(),
        };
        store
            .job_put(&row("j1", "c1", JobKind::Shell))
            .await
            .unwrap();
        store.job_put(&row("j2", "c2", JobKind::Pty)).await.unwrap();
        let p = plan(ResumeCause::Transfer);
        let note = resume_job_call(&store, s, &call("c1", "shell", json!({})), &p)
            .await
            .unwrap();
        assert!(
            note.content.starts_with(&format!(
                "ERROR: interrupted: the session moved here from another machine at {} while this command ran (job j1, started {}); the job stayed on another machine and its exit code is unknown here",
                hhmmss(p.interrupted_at),
                hhmmss_ms(1_000)
            )),
            "{}",
            note.content
        );
        let wait = resume_job_call(&store, s, &call("c5", "wait", json!({"id": "j2"})), &p)
            .await
            .unwrap();
        assert!(
            wait.content.starts_with(&format!(
                "job j2 · lost · 8s\n[process gone — the session moved from another machine at {}; no screen was captured]",
                hhmmss(p.interrupted_at)
            )),
            "{}",
            wait.content
        );
    }

    /// 183 §11 (closer item c): a transfer whose job row names no other
    /// engine (a pre-139 row) still says the session moved, never "the
    /// engine restarted"; a crash keeps 109's wording.
    #[tokio::test]
    async fn job_note_after_a_move_says_moved_without_an_engine_on_the_row() {
        let scratch = crate::TestDir::new("183-z-moved");
        let store = Store::open(
            &scratch.0.join("sessions.sqlite"),
            Some(mesh::EngineId("b".into())),
        )
        .await
        .unwrap();
        let s = SessionId::new();
        store
            .create_session(s, "m", None, None, None)
            .await
            .unwrap();
        for (id, call_id, kind) in [("j1", "c1", JobKind::Shell), ("j2", "c2", JobKind::Pty)] {
            store
                .job_put(&JobRow {
                    session: s,
                    id: tools::JobId(id.into()),
                    kind,
                    call_id: call_id.into(),
                    command: "x".into(),
                    name: None,
                    pgid: None,
                    log: Some(std::path::PathBuf::from("/nonexistent/j.log")),
                    started_ms: 1_000,
                    state: JobState::Lost,
                    exit: Some(JobExit::Lost),
                    settled_ms: Some(9_000),
                    engine: String::new(),
                })
                .await
                .unwrap();
        }
        let p = plan(ResumeCause::Transfer);
        let note = resume_job_call(&store, s, &call("c1", "shell", json!({})), &p)
            .await
            .unwrap();
        assert!(
            note.content.starts_with(&format!(
                "ERROR: interrupted: the session moved here from another machine at {} while this command ran",
                hhmmss(p.interrupted_at)
            )),
            "{}",
            note.content
        );
        let wait = resume_job_call(&store, s, &call("c5", "wait", json!({"id": "j2"})), &p)
            .await
            .unwrap();
        assert!(
            wait.content
                .contains("[process gone — the session moved to this engine at "),
            "{}",
            wait.content
        );
        let crash = resume_job_call(
            &store,
            s,
            &call("c1", "shell", json!({})),
            &plan(ResumeCause::Crash),
        )
        .await
        .unwrap();
        // The fixture plan is `suspended`: 109's stop-and-restart wording, no move.
        assert!(
            crash
                .content
                .starts_with("ERROR: interrupted: the engine was stopped at ")
                && !crash.content.contains("moved"),
            "{}",
            crash.content
        );
    }
}
