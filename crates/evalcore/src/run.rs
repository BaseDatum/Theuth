//! One replay (159 §13.1): render, call the target, score by checker or judge. The model call is
//! behind [`Model`] so the caller owns cost and `CallKind` (autoresearch charges
//! `CallKind::Autoresearch` to the study session, dreaming `CallKind::Dream` to its own).
//! Owner: W2.

use crate::case::Case;

/// A model the runner may call: one system prompt, one user message, one reply.
#[async_trait::async_trait]
pub trait Model: Send + Sync {
    /// Ask; `Err` is the provider's or the budget's message.
    async fn call(&self, system: &str, user: String) -> Result<String, String>;
}

/// What one case cost and scored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaseScore {
    /// 0–1.
    pub score: f64,
    /// The target's answer.
    pub answer: String,
    /// The checker's diff or the judge's reason.
    pub reason: String,
    /// Tokens the target read.
    pub tokens_in: u64,
    /// Tokens the target wrote.
    pub tokens_out: u64,
    /// USD, both calls.
    pub cost_usd: f64,
}

/// Render (system = the artefact, or `template` with `{{artefact}}`; user = the case input),
/// call `target`, then score with the case's check or with `judge` and the 0–1 prompt.
pub async fn run_case(
    target: &dyn Model,
    judge: Option<&dyn Model>,
    artefact: &str,
    template: Option<&str>,
    case: &Case,
) -> Result<CaseScore, String> {
    let _ = (target, judge, artefact, template, case);
    Err("not built (W2)".into())
}
