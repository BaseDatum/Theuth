//! `replay_case` (design doc 136 §7.2 step 3): render the case's context
//! with the candidate learned text as the system frame, answer it with the
//! target model in a fresh context (`runtime::SideCall`, `CallKind::Dream`,
//! the dreaming session as the cost owner so the budget guard counts it),
//! then have the judge model (136 D12: `dreaming.models.judge`, else
//! `agent.escalate_model`, else the proposer's model) score the answer 0–1
//! against `ideal` + `user_words` with a one-line reason. The judge never
//! sees the proposer's rationale. Every replay is persisted as one row of
//! `dreaming/replays.jsonl` (136 D2) and the gate of §7.2 step 4 — [`gate`]
//! — is recomputed from those rows by `skill_learned_edit`'s record pass
//! ([`gate_from_rows`]); the model's own arithmetic is never trusted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::SessionId;
use sha2::Digest as _;

use crate::cases;
use crate::config::{AgentDefaults, DreamConfig};
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

/// One persisted replay (`dreaming/replays.jsonl`, 136 D2).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReplayRow {
    /// The night.
    pub night: String,
    /// The case replayed.
    pub case_id: String,
    /// `sha256(candidate_text.trim())`; `None` = the baseline run.
    pub candidate_sha256: Option<String>,
    /// The judge's score.
    pub score: f64,
    /// `provider/model` that judged.
    pub judge_model: String,
    /// RFC 3339.
    pub at: String,
}

/// `<data_dir>/dreaming/replays.jsonl`.
pub fn replays_path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("replays.jsonl")
}

/// The candidate's identity: `sha256` of the trimmed text, hex.
pub fn candidate_sha256(text: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(text.trim().as_bytes()))
}

/// Append one row.
pub fn log_row(data_dir: &Path, row: &ReplayRow) -> anyhow::Result<()> {
    let p = replays_path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?
        .write_all(line.as_bytes())?;
    Ok(())
}

/// Every row of `night`, in file order.
pub fn rows(data_dir: &Path, night: &str) -> Vec<ReplayRow> {
    std::fs::read_to_string(replays_path(data_dir))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str::<ReplayRow>(l).ok())
                .filter(|r| r.night == night)
                .collect()
        })
        .unwrap_or_default()
}

/// The gate for `candidate_text` from tonight's persisted replays (136 D2):
/// pair the latest baseline row and the latest candidate row per case,
/// then [`gate`]. `None` when no replay of that candidate was logged.
pub fn gate_from_rows(
    data_dir: &Path,
    night: &str,
    candidate_text: &str,
    cfg: &DreamConfig,
) -> Option<GateResult> {
    let sha = candidate_sha256(candidate_text);
    let rows = rows(data_dir, night);
    let mut baseline: std::collections::BTreeMap<&str, f64> = Default::default();
    let mut candidate: std::collections::BTreeMap<&str, f64> = Default::default();
    for r in &rows {
        match r.candidate_sha256.as_deref() {
            None => {
                baseline.insert(&r.case_id, r.score);
            }
            Some(s) if s == sha => {
                candidate.insert(&r.case_id, r.score);
            }
            Some(_) => {}
        }
    }
    if candidate.is_empty() {
        return None;
    }
    let (mut b, mut c) = (Vec::new(), Vec::new());
    for (case, score) in &candidate {
        if let Some(base) = baseline.get(case) {
            b.push(*base);
            c.push(*score);
        }
    }
    Some(gate(&b, &c, cfg))
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
            .map(|b| b.lines().skip(1).collect::<Vec<_>>().join("\n"))
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
/// baseline run); `target_model` None = the resolved `target` tier
/// (`agent.model`). The judge is the resolved `judge` tier (136 D12). The
/// score is persisted as a [`ReplayRow`] of tonight (136 D2).
pub async fn replay(
    data_dir: &Path,
    cfg: &DreamConfig,
    agent_defaults: &AgentDefaults,
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
    let (models, _) = cfg.resolve_models(agent_defaults);
    let learned = match candidate_text {
        Some(t) => t.to_owned(),
        None => current_learned(data_dir, &case.skill_hint),
    };
    let target = target_model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&models.target);
    let target_role = if target.is_empty() {
        runtime::ModelRole::Session
    } else {
        runtime::ModelRole::Named(target.to_owned())
    };
    let answer = call(
        &agents,
        session,
        target_role,
        target_system(&case.skill_hint, &learned),
        format!(
            "{}\n\n[The transcript stops here. Your next turn:]",
            case.context
        ),
    )
    .await?;
    let judge_role = if models.judge.is_empty() {
        runtime::ModelRole::Session
    } else {
        runtime::ModelRole::Named(models.judge.clone())
    };
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
    log_row(
        data_dir,
        &ReplayRow {
            night: crate::ledger::tonight(),
            case_id: case.id.clone(),
            candidate_sha256: candidate_text.map(candidate_sha256),
            score,
            judge_model: models.judge,
            at: cases::now_rfc3339(),
        },
    )?;
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
    let min_delta = if min_delta.is_finite() {
        min_delta
    } else {
        0.0
    };
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
        let u = judge_user(
            "a numbered list",
            "never use ask_user",
            "Here are the options: 1. …",
        );
        assert!(!u.contains("rationale") && u.starts_with("IDEAL:\n"));
        assert!(
            target_system("harness", "- rule").contains("## Learned (dreaming) — harness\n- rule")
        );
        assert!(
            target_system("prompt:standing-preferences", "- rule")
                .contains("## Standing preferences (learned)\n- rule")
        );
        assert!(!target_system("knowledge:facts/x", "").contains("Learned"));
    }

    /// R-2 (D2): the gate comes from `replays.jsonl`, paired by case; no row
    /// for the candidate → `None` (the record pass refuses); another night's
    /// rows and another candidate's rows do not count; a case replayed twice
    /// keeps its latest score.
    #[test]
    fn gate_is_recomputed_from_tonights_replay_rows() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let text = "- Prefer numbered lists\n";
        assert_eq!(gate_from_rows(dir.path(), "2026-10-08", text, &cfg), None);
        let row = |night: &str, case: &str, sha: Option<&str>, score: f64| ReplayRow {
            night: night.into(),
            case_id: case.into(),
            candidate_sha256: sha.map(str::to_owned),
            score,
            judge_model: "openai/gpt-5.6-sol".into(),
            at: String::new(),
        };
        let sha = candidate_sha256(text);
        for (c, b, k) in [("c1", 0.4, 0.7), ("c2", 0.5, 0.6), ("c3", 0.3, 0.5)] {
            log_row(dir.path(), &row("2026-10-08", c, None, b)).unwrap();
            log_row(dir.path(), &row("2026-10-08", c, Some(&sha), k)).unwrap();
        }
        // Noise: another candidate, another night, a re-run of c3 that lands lower.
        log_row(dir.path(), &row("2026-10-08", "c1", Some("other"), 0.0)).unwrap();
        log_row(dir.path(), &row("2026-10-07", "c4", Some(&sha), 1.0)).unwrap();
        log_row(dir.path(), &row("2026-10-08", "c3", Some(&sha), 0.4)).unwrap();
        let g = gate_from_rows(dir.path(), "2026-10-08", text, &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Accepted, 3));
        assert!((g.baseline - 0.4).abs() < 1e-9, "{g:?}");
        assert!((g.candidate - (0.7 + 0.6 + 0.4) / 3.0).abs() < 1e-9, "{g:?}");
        // Two pairs only → unverified; whitespace around the candidate is not an identity change.
        log_row(dir.path(), &row("2026-10-09", "c1", None, 0.1)).unwrap();
        log_row(dir.path(), &row("2026-10-09", "c1", Some(&sha), 0.9)).unwrap();
        log_row(dir.path(), &row("2026-10-09", "c2", None, 0.1)).unwrap();
        log_row(dir.path(), &row("2026-10-09", "c2", Some(&sha), 0.9)).unwrap();
        let g = gate_from_rows(dir.path(), "2026-10-09", &format!("  {text}\n"), &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Unverified, 2));
        // A candidate replayed without any baseline: rows exist, nothing pairs.
        log_row(dir.path(), &row("2026-10-10", "c9", Some(&sha), 0.9)).unwrap();
        let g = gate_from_rows(dir.path(), "2026-10-10", text, &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Unverified, 0));
    }
}
