//! `replay_case` (design doc 136 §7.2 step 3): render the case's context and
//! the candidate text, call the target model under `CallKind::Dream` and the
//! routine's cost owner, then the judge model from config (never the
//! proposer's rationale), return `{answer, score, reason}`. 136 W3.

use std::path::Path;

/// The judge's answer.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Replay {
    /// The target model's answer.
    pub answer: String,
    /// 0–1.
    pub score: f64,
    /// One line.
    pub reason: String,
}

/// Replay one case (136 W3).
pub async fn replay(
    data_dir: &Path,
    case_id: &str,
    candidate_text: Option<&str>,
    target_model: Option<&str>,
) -> anyhow::Result<Replay> {
    // 136 W3
    let _ = (data_dir, case_id, candidate_text, target_model);
    anyhow::bail!("not built: W3 (replay_case)")
}
