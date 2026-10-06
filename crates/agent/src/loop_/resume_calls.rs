//! Design doc 109 §4.2: the tool-specific resume rules a replayed batch
//! applies to a `started` call. The contract wave leaves one stub per owning
//! wave, each answering the generic interrupted note (§4.3); the per-call
//! rule in `loop_.rs` (W1) routes `shell` / `start` / `send` / `wait` to
//! [`resume_job_call`], the human-wait tools to [`resume_question_call`] and
//! `send_message{await_reply}` / `spawn_subagent` to [`resume_wait_call`].

use chrono::{DateTime, Local, Utc};
use tools::ToolOutput;

use crate::durability::{JournaledCall, ResumePlan};

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
