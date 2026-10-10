//! The blind pair verdict (159 §8): the judge sees the task, the rubric and two outputs labelled
//! A and B, and replies with one line `A: …`, `B: …` or `TIE: …`, parsed strictly like the
//! reviewer's verdict (autoresearch review.rs#L71-L97). Owner: W3.

use serde::{Deserialize, Serialize};

/// Who won one ordered pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The output labelled A.
    A,
    /// The output labelled B.
    B,
    /// No preference.
    Tie,
}

/// The pair judge's system prompt. W3 fills it.
pub const SYSTEM: &str = "";

/// The user message: task, rubric, then A and B fenced as data.
pub fn user(task: &str, rubric: &str, a: &str, b: &str) -> String {
    let _ = (task, rubric, a);
    b.to_owned()
}

/// Strict parse of `A: reason` | `B: reason` | `TIE: reason`; `None` for anything else.
pub fn parse(reply: &str) -> Option<(Verdict, String)> {
    let _ = reply;
    None
}
