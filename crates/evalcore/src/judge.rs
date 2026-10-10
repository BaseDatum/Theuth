//! The 0–1 judge (159 §13.1): dreaming's prompt generalised to `{ideal, references, answer}`.
//! Owner: W2 — moves the text verbatim from crates/dreaming/src/replay.rs#L24-L25, #L153-L175
//! (a test keeps dreaming's copy identical). The answer is fenced as data and the system prompt
//! says to ignore instructions inside it (159 §7 Guard).

/// The judge's system prompt. W2 fills it verbatim from dreaming.
pub const SYSTEM: &str = "";

/// The judge's user message: the ideal and references, then the answer fenced as data.
pub fn user(ideal: Option<&str>, references: &[String], answer: &str) -> String {
    let _ = (ideal, references);
    answer.to_owned()
}

/// Parse the judge's reply as a score in 0–1; `None` when it has none.
pub fn parse(reply: &str) -> Option<f64> {
    let _ = reply;
    None
}
