//! Design doc 109 §4.2: the tool-specific resume rules a replayed batch
//! applies to a `started` call. The contract wave leaves one stub per owning
//! wave, each answering the generic interrupted note (§4.3); the per-call
//! rule in `loop_.rs` (W1) routes `shell` / `start` / `send` / `wait` to
//! [`resume_job_call`], the human-wait tools to [`resume_question_call`] and
//! `send_message{await_reply}` / `spawn_subagent` to [`resume_wait_call`].

use chrono::{DateTime, Local, Utc};
use kameo::actor::ActorRef;
use kameo::error::SendError;
use tools::ToolOutput;

use crate::durability::{JournaledCall, ResumePlan};
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

/// **W2 fills this.** The job rule (§5.4): a `shell` / `start` / `send`
/// call that was `started` when the engine died — the job is gone (R1); the
/// note carries the job id, log path and tail (or the last screen for a
/// PTY); `wait` gets the tool's own exited reply with `exit="lost"`.
pub(crate) async fn resume_job_call(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput {
    interrupted_note(call, plan)
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
    let Ok(Some(registry)) =
        ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
    else {
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
        let Some(rx) = w.pending.remove(&r.id) else { continue };
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
