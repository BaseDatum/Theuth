//! `replay_case` (design doc 136 §7.2 step 3): render the case's context
//! with the candidate learned text as the system frame, answer it with the
//! target model in a fresh context (`runtime::SideCall`, `CallKind::Dream`,
//! the dreaming session as the cost owner so the budget guard counts it),
//! then have the judge model — `dreaming.models.judge`, else the session's —
//! score the answer 0–1 against `ideal` + `user_words` with a one-line
//! reason. The judge never sees the proposer's rationale; the gate math of
//! §7.2 step 4 is [`gate`], the pure function the skill's recipe mirrors.

use std::path::Path;
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::cases;
use crate::config::DreamConfig;
use crate::types::{GateResult, Verdict};

/// The judge prompt, verbatim in `skill.md` (a test keeps them identical).
pub const JUDGE_PROMPT: &str = "You are the judge of one replayed case. You see the IDEAL (what right looked like), the USER_WORDS (what the user said when it went wrong) and the ANSWER an assistant gave to the same situation. Score how close the ANSWER comes to the IDEAL in the spirit of the USER_WORDS: 1.0 = the user would have had nothing to correct; 0.5 = half right or right for the wrong reason; 0.0 = the same mistake again or worse. Judge substance, not wording; an answer that merely promises to behave is not a behaving answer. Reply with exactly one JSON object and nothing else: {\"score\": <number 0-1>, \"reason\": \"<one line, under 140 chars>\"}";

/// Model call time budget.
pub const CALL_BUDGET: Duration = Duration::from_secs(180);

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

/// The system frame for the target model: the learned text as the live
/// frame would append it.
pub fn target_system(skill_hint: &str, learned: &str) -> String {
    let mut s = String::from(
        "You are the assistant in a theuth session, answering from the point where the transcript below stops. \
         Reply as you would have replied there — the next assistant turn only, no commentary about this exercise.",
    );
    let learned = learned.trim();
    if !learned.is_empty() {
        if skill_hint == "prompt:standing-preferences" {
            s.push_str("\n\n## Standing preferences (learned)\n");
        } else {
            s.push_str(&format!("\n\n## Learned (dreaming) — {skill_hint}\n"));
        }
        s.push_str(learned);
    }
    s
}

/// The judge's user message: ideal, the user's words, the answer — nothing else.
pub fn judge_user(ideal: &str, user_words: &str, answer: &str) -> String {
    format!("IDEAL:\n{ideal}\n\nUSER_WORDS:\n{user_words}\n\nANSWER:\n{answer}")
}

/// `{"score": …, "reason": …}` out of the judge's text, tolerating prose
/// around it; a bare number is a score with no reason.
pub fn parse_judge(text: &str) -> Option<(f64, String)> {
    let t = text.trim();
    let (start, end) = (t.find('{')?, t.rfind('}')?);
    if start < end
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&t[start..=end])
    {
        let score = match &v["score"] {
            serde_json::Value::Number(n) => n.as_f64()?,
            serde_json::Value::String(s) => s.trim().parse().ok()?,
            _ => return None,
        };
        let reason = v["reason"].as_str().unwrap_or("").trim().to_owned();
        return Some((score.clamp(0.0, 1.0), reason));
    }
    None
}

/// The learned text the baseline run sees for a `skill_hint`.
pub fn current_learned(data_dir: &Path, skill_hint: &str) -> String {
    if skill_hint == "prompt:standing-preferences" {
        crate::preferences::block(data_dir)
            .map(|b| {
                b.lines()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    } else if skill_hint.starts_with("knowledge:") {
        String::new()
    } else {
        crate::edits::current(data_dir, skill_hint)
    }
}

async fn call(
    agents: &ActorRef<agent::Agents>,
    session: SessionId,
    role: runtime::ModelRole,
    system: String,
    user: String,
) -> anyhow::Result<String> {
    let reply = agents
        .ask(runtime::SideCall {
            session,
            role,
            system,
            user,
            budget: CALL_BUDGET,
            kind: runtime::CallKind::Dream,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(reply.text)
}

/// Replay one case: `candidate_text` None = the current learned text (the
/// baseline run); `target_model` None = the session's model.
pub async fn replay(
    data_dir: &Path,
    cfg: &DreamConfig,
    session: SessionId,
    case_id: &str,
    candidate_text: Option<&str>,
    target_model: Option<&str>,
) -> anyhow::Result<Replay> {
    let case = cases::get(data_dir, case_id)?;
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("agent actor is not running"))?;
    let learned = match candidate_text {
        Some(t) => t.to_owned(),
        None => current_learned(data_dir, &case.skill_hint),
    };
    let target_role = target_model
        .map(|m| runtime::ModelRole::Named(m.to_owned()))
        .unwrap_or(runtime::ModelRole::Session);
    let answer = call(
        &agents,
        session,
        target_role,
        target_system(&case.skill_hint, &learned),
        format!("{}\n\n[The transcript stops here. Your next turn:]", case.context),
    )
    .await?;
    let judge_role = cfg
        .models
        .judge
        .clone()
        .map(runtime::ModelRole::Named)
        .unwrap_or(runtime::ModelRole::Session);
    let verdict = call(
        &agents,
        session,
        judge_role,
        JUDGE_PROMPT.to_owned(),
        judge_user(&case.ideal, &case.user_words, &answer),
    )
    .await?;
    let (score, reason) = parse_judge(&verdict)
        .ok_or_else(|| anyhow::anyhow!("judge answered without a score: {}", verdict.trim()))?;
    Ok(Replay {
        answer,
        score,
        reason,
    })
}

/// The gate (136 §7.2 step 4) over paired per-case scores: accepted iff
/// `mean(candidate) − mean(baseline) ≥ margin` and
/// `min(candidate_i − baseline_i) ≥ −max_regression`; fewer than
/// `min_val_cases` pairs → `unverified`.
pub fn gate(baseline: &[f64], candidate: &[f64], cfg: &DreamConfig) -> GateResult {
    let n = baseline.len().min(candidate.len());
    let mean = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().take(n).sum::<f64>() / n as f64
        }
    };
    let (b, c) = (mean(baseline), mean(candidate));
    let min_delta = baseline
        .iter()
        .zip(candidate)
        .map(|(x, y)| y - x)
        .fold(f64::INFINITY, f64::min);
    let min_delta = if min_delta.is_finite() { min_delta } else { 0.0 };
    let verdict = if n < cfg.gate.min_val_cases as usize {
        Verdict::Unverified
    } else if c - b >= cfg.gate.margin && min_delta >= -cfg.gate.max_regression {
        Verdict::Accepted
    } else {
        Verdict::Rejected
    };
    GateResult {
        baseline: b,
        candidate: c,
        min_delta,
        val_cases: n as u32,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_parsing_tolerates_prose_and_clamps() {
        assert_eq!(
            parse_judge("Sure. {\"score\": 0.75, \"reason\": \"lists the forks\"} done"),
            Some((0.75, "lists the forks".into()))
        );
        assert_eq!(
            parse_judge("{\"score\": \"1.4\", \"reason\": \"\"}"),
            Some((1.0, String::new()))
        );
        assert_eq!(parse_judge("no json"), None);
        assert_eq!(parse_judge("{\"reason\": \"x\"}"), None);
    }

    #[test]
    fn gate_accepts_rejects_and_marks_small_sets_unverified() {
        let cfg = DreamConfig::default();
        let g = gate(&[0.4, 0.5, 0.3], &[0.7, 0.6, 0.5], &cfg);
        assert_eq!(g.verdict, Verdict::Accepted);
        assert!((g.baseline - 0.4).abs() < 1e-9 && (g.candidate - 0.6).abs() < 1e-9);
        assert!((g.min_delta - 0.1).abs() < 1e-9);
        // Mean improves but one case regresses by 0.3.
        assert_eq!(
            gate(&[0.4, 0.5, 0.9], &[0.9, 0.9, 0.6], &cfg).verdict,
            Verdict::Rejected
        );
        // Mean improves by less than the margin.
        assert_eq!(
            gate(&[0.4, 0.5, 0.3], &[0.45, 0.55, 0.35], &cfg).verdict,
            Verdict::Rejected
        );
        let g = gate(&[0.1, 0.1], &[0.9, 0.9], &cfg);
        assert_eq!((g.verdict, g.val_cases), (Verdict::Unverified, 2));
        assert_eq!(gate(&[], &[], &cfg).verdict, Verdict::Unverified);
    }

    #[test]
    fn the_judge_never_sees_a_rationale_and_the_frame_names_the_rung() {
        let u = judge_user("a numbered list", "never use ask_user", "Here are the options: 1. …");
        assert!(!u.contains("rationale") && u.starts_with("IDEAL:\n"));
        assert!(target_system("harness", "- rule").contains("## Learned (dreaming) — harness\n- rule"));
        assert!(target_system("prompt:standing-preferences", "- rule")
            .contains("## Standing preferences (learned)\n- rule"));
        assert!(!target_system("knowledge:facts/x", "").contains("Learned"));
    }
}
