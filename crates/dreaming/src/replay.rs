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
//!
//! 159 §13 (W2): the judge prompt, the parse, the margin gate and the replay
//! itself live in `crates/evalcore`; this module keeps thin re-exports so the
//! skill test and the night stay as they were. Two things changed on purpose:
//! [`gate_from_rows`] drops pairs whose case is not `val` before the gate
//! (§13.2, once skill text only), and [`gate`] is `evalcore::gate::decide` —
//! the margin rule plus a paired t from 10 pairs up (W-r; a cluster of 3–6
//! cases never reaches it).

use std::path::{Path, PathBuf};
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::cases;
use crate::config::{AgentDefaults, DreamConfig};
use crate::types::{CaseOrigin, GateResult, Split, Verdict};

/// The judge prompt, verbatim in `skill.md` (a test keeps them identical); the
/// shared core's [`evalcore::judge::SYSTEM`].
pub const JUDGE_PROMPT: &str = evalcore::judge::SYSTEM;

/// Model call time budget.
pub const CALL_BUDGET: Duration = Duration::from_secs(180);

/// `owner` of a row the night wrote (159 §13.5).
pub const OWNER_DREAM: &str = "dream";

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

/// One persisted replay (`dreaming/replays.jsonl`, 136 D2). The shape of every row ever
/// written; 159 §13.5 adds [`ReplayMeta`] beside it on the same line.
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

/// 159 §13.5: the three optional fields new rows carry. Old rows lack them: the reader
/// derives `split` from the case store (the split is a stable hash of the id) and `owner`
/// defaults to [`OWNER_DREAM`]. Nothing is rewritten.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReplayMeta {
    /// Written at log time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
    /// `dream` or `study:<slug>`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// The judge's reason.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// One line of the file: the row and its meta, flattened.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Line {
    #[serde(flatten)]
    row: ReplayRow,
    #[serde(flatten)]
    meta: ReplayMeta,
}

/// `<data_dir>/dreaming/replays.jsonl`.
pub fn replays_path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("replays.jsonl")
}

/// The candidate's identity: `sha256` of the trimmed text, hex.
pub fn candidate_sha256(text: &str) -> String {
    evalcore::candidate_hash(text.trim())
}

/// Append one row without meta (the pre-159 shape; tests and old callers).
pub fn log_row(data_dir: &Path, row: &ReplayRow) -> anyhow::Result<()> {
    log_row_meta(data_dir, row, &ReplayMeta::default())
}

/// Append one row with its 159 §13.5 meta.
pub fn log_row_meta(data_dir: &Path, row: &ReplayRow, meta: &ReplayMeta) -> anyhow::Result<()> {
    let p = replays_path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_string(&Line {
        row: row.clone(),
        meta: meta.clone(),
    })?;
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
    rows_with_meta(data_dir, night)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// Every row of `night` with its meta (defaults for old rows), in file order.
pub fn rows_with_meta(data_dir: &Path, night: &str) -> Vec<(ReplayRow, ReplayMeta)> {
    std::fs::read_to_string(replays_path(data_dir))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str::<Line>(l).ok())
                .filter(|l| l.row.night == night)
                .map(|l| (l.row, l.meta))
                .collect()
        })
        .unwrap_or_default()
}

/// The split a row counts under (159 §13.2): the meta's when written, else the case store's
/// (the stable hash of the id, so an old row is placed exactly as the night that wrote it
/// would place it). A row whose case is not in the store at all cannot be placed and counts,
/// as every row did before 159 — only pre-159 rows can be in that position, since
/// [`replay`] reads the case first.
fn row_split(data_dir: &Path, row: &ReplayRow, meta: &ReplayMeta) -> Option<Split> {
    meta.split
        .or_else(|| cases::get(data_dir, &row.case_id).ok().map(|c| c.split))
}

/// The gate for `candidate_text` from tonight's persisted replays (136 D2):
/// pair the latest baseline row and the latest candidate row per **val** case
/// (159 §13.2: a train replay still logs, its row never counts), then [`gate`].
/// `None` when no replay of that candidate was logged.
pub fn gate_from_rows(
    data_dir: &Path,
    night: &str,
    candidate_text: &str,
    cfg: &DreamConfig,
) -> Option<GateResult> {
    let sha = candidate_sha256(candidate_text);
    let rows = rows_with_meta(data_dir, night);
    let mut baseline: std::collections::BTreeMap<&str, f64> = Default::default();
    let mut candidate: std::collections::BTreeMap<&str, f64> = Default::default();
    let mut any_candidate = false;
    for (r, meta) in &rows {
        let counts = match r.candidate_sha256.as_deref() {
            None => true,
            Some(s) if s == sha => {
                any_candidate = true;
                true
            }
            Some(_) => false,
        };
        if !counts || row_split(data_dir, r, meta) == Some(Split::Train) {
            continue;
        }
        match r.candidate_sha256 {
            None => baseline.insert(&r.case_id, r.score),
            Some(_) => candidate.insert(&r.case_id, r.score),
        };
    }
    if !any_candidate {
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

/// The judge's user message: ideal, the user's words, the answer (fenced as data, 159 §7) —
/// nothing else.
pub fn judge_user(ideal: &str, user_words: &str, answer: &str) -> String {
    evalcore::judge::user(Some(ideal), &[user_words.to_owned()], answer)
}

/// `{"score": …, "reason": …}` out of the judge's text, tolerating prose
/// around it; a bare number is a score with no reason.
pub fn parse_judge(text: &str) -> Option<(f64, String)> {
    evalcore::judge::parse_with_reason(text)
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

/// A model behind `runtime::SideCall` with `CallKind::Dream`, charged to the dreaming session.
struct DreamModel {
    agents: ActorRef<agent::Agents>,
    session: SessionId,
    role: runtime::ModelRole,
}

#[async_trait::async_trait]
impl evalcore::run::Model for DreamModel {
    async fn call(&self, system: &str, user: String) -> Result<String, String> {
        self.agents
            .ask(runtime::SideCall {
                session: self.session,
                role: self.role.clone(),
                system: system.to_owned(),
                user,
                budget: CALL_BUDGET,
                kind: runtime::CallKind::Dream,
            })
            .await
            .map(|r| r.text)
            .map_err(|e| e.to_string())
    }
}

fn role(model: &str) -> runtime::ModelRole {
    if model.is_empty() {
        runtime::ModelRole::Session
    } else {
        runtime::ModelRole::Named(model.to_owned())
    }
}

/// A dreaming case as the shared core's: `context` is the input, `ideal` and `user_words`
/// the judge's references; synthetic is never val.
pub fn eval_case(case: &crate::types::Case) -> evalcore::Case {
    evalcore::Case {
        id: case.id.clone(),
        input: format!(
            "{}\n\n[The transcript stops here. Your next turn:]",
            case.context
        ),
        ideal: Some(case.ideal.clone()),
        references: vec![case.user_words.clone()],
        check: None,
        tags: case.tags.clone(),
        synthetic: case.origin == CaseOrigin::Synthetic,
    }
}

/// Replay one case: `candidate_text` None = the current learned text (the
/// baseline run); `target_model` None = the resolved `target` tier
/// (`agent.model`). The judge is the resolved `judge` tier (136 D12). The
/// score is persisted as a [`ReplayRow`] of tonight (136 D2) with its split
/// and owner (159 §13.5).
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
    let target = DreamModel {
        agents: agents.clone(),
        session,
        role: role(target),
    };
    let judge = DreamModel {
        agents,
        session,
        role: role(&models.judge),
    };
    let scored = evalcore::run::run_case(
        &target,
        Some(&judge),
        &target_system(&case.skill_hint, &learned),
        None,
        &eval_case(&case),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    log_row_meta(
        data_dir,
        &ReplayRow {
            night: crate::ledger::tonight(),
            case_id: case.id.clone(),
            candidate_sha256: candidate_text.map(candidate_sha256),
            score: scored.score,
            judge_model: models.judge,
            at: cases::now_rfc3339(),
        },
        &ReplayMeta {
            split: Some(case.split),
            owner: OWNER_DREAM.to_owned(),
            reason: scored.reason.clone(),
        },
    )?;
    Ok(Replay {
        answer: scored.answer,
        score: scored.score,
        reason: scored.reason,
    })
}

/// Dreaming's gate parameters as the shared core's.
pub fn gate_cfg(cfg: &DreamConfig) -> evalcore::gate::Cfg {
    evalcore::gate::Cfg {
        min_mean_delta: cfg.gate.margin,
        max_case_regression: cfg.gate.max_regression,
        min_pairs: cfg.gate.min_val_cases as usize,
    }
}

/// The gate (136 §7.2 step 4) over paired per-case scores — the shared
/// `evalcore::gate::decide` (159 W-r): accepted iff `mean(candidate) −
/// mean(baseline) ≥ margin` and `min(candidate_i − baseline_i) ≥
/// −max_regression`, plus a paired t from 10 pairs up; fewer than
/// `min_val_cases` pairs → `unverified`.
pub fn gate(baseline: &[f64], candidate: &[f64], cfg: &DreamConfig) -> GateResult {
    let pairs: Vec<evalcore::gate::Pair> = baseline
        .iter()
        .zip(candidate)
        .enumerate()
        .map(|(i, (b, c))| evalcore::gate::Pair {
            id: i.to_string(),
            baseline: *b,
            candidate: *c,
        })
        .collect();
    let n = pairs.len();
    let mean = |f: fn(&evalcore::gate::Pair) -> f64| {
        if n == 0 {
            0.0
        } else {
            pairs.iter().map(f).sum::<f64>() / n as f64
        }
    };
    let (b, c) = (mean(|p| p.baseline), mean(|p| p.candidate));
    let min_delta = pairs
        .iter()
        .map(evalcore::gate::Pair::delta)
        .fold(f64::INFINITY, f64::min);
    let min_delta = if min_delta.is_finite() {
        min_delta
    } else {
        0.0
    };
    let verdict = match evalcore::gate::decide(&pairs, &gate_cfg(cfg)) {
        evalcore::gate::Decision::Accept => Verdict::Accepted,
        evalcore::gate::Decision::Reject(_) => Verdict::Rejected,
        evalcore::gate::Decision::Unverified(_) => Verdict::Unverified,
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
        // 159 §7: the answer is fenced as data.
        assert!(u.contains("<<<ANSWER\nHere are the options: 1. …\nANSWER>>>"));
        assert!(
            target_system("harness", "- rule").contains("## Learned (dreaming) — harness\n- rule")
        );
        assert!(
            target_system("prompt:standing-preferences", "- rule")
                .contains("## Standing preferences (learned)\n- rule")
        );
        assert!(!target_system("knowledge:facts/x", "").contains("Learned"));
        // The dreaming case maps onto the shared core's with synthetic never val.
        let mut c = cases::sample("c1", "harness");
        c.origin = CaseOrigin::Synthetic;
        let e = eval_case(&c);
        assert!(e.synthetic && e.ideal.as_deref() == Some(c.ideal.as_str()));
        assert_eq!(e.references, vec![c.user_words.clone()]);
        assert!(e.input.starts_with(&c.context));
    }

    /// R-2 (D2): the gate comes from `replays.jsonl`, paired by case; no row
    /// for the candidate → `None` (the record pass refuses); another night's
    /// rows and another candidate's rows do not count; a case replayed twice
    /// keeps its latest score. (These ids are not in the case store: they
    /// count as before 159; the split test below covers placed rows.)
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
        assert!(
            (g.candidate - (0.7 + 0.6 + 0.4) / 3.0).abs() < 1e-9,
            "{g:?}"
        );
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
        // Old lines (no meta) read back with default meta; a line with meta keeps it.
        let all = rows_with_meta(dir.path(), "2026-10-10");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1, ReplayMeta::default());
        log_row_meta(
            dir.path(),
            &row("2026-10-10", "c9", None, 0.2),
            &ReplayMeta {
                split: Some(Split::Val),
                owner: "study:x".into(),
                reason: "why".into(),
            },
        )
        .unwrap();
        let all = rows_with_meta(dir.path(), "2026-10-10");
        assert_eq!(all[1].1.owner, "study:x");
        assert_eq!(all[1].1.split, Some(Split::Val));
        assert_eq!(rows(dir.path(), "2026-10-10").len(), 2);
    }

    /// 159 §13.2: only val pairs gate. A record pass whose rows are all train is
    /// unverified (refused by the record pass); a mixed pass gates on val only; the
    /// split comes from the meta when written, else from the case store.
    #[test]
    fn gate_from_rows_counts_val_pairs_only() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cfg = DreamConfig::default();
        let night = "2026-10-10";
        // Ids whose stable split is known: c1, c2, c3 → val; c4, c9 → train (seed "136").
        for id in ["c1", "c2", "c3", "c4", "c9"] {
            cases::append(d, &cases::sample(id, "harness"), night).unwrap();
        }
        assert_eq!(cases::get(d, "c1").unwrap().split, Split::Val);
        assert_eq!(cases::get(d, "c4").unwrap().split, Split::Train);
        let text = "- rule\n";
        let sha = candidate_sha256(text);
        let row = |case: &str, sha: Option<&str>, score: f64| ReplayRow {
            night: night.into(),
            case_id: case.into(),
            candidate_sha256: sha.map(str::to_owned),
            score,
            judge_model: "j".into(),
            at: String::new(),
        };
        // All-train pass: three train pairs would have passed the old gate.
        for c in ["c4", "c9"] {
            log_row(d, &row(c, None, 0.2)).unwrap();
            log_row(d, &row(c, Some(&sha), 0.9)).unwrap();
        }
        log_row_meta(
            d,
            &row("c1", None, 0.2),
            &ReplayMeta {
                split: Some(Split::Train), // the meta wins over the store
                ..Default::default()
            },
        )
        .unwrap();
        log_row_meta(
            d,
            &row("c1", Some(&sha), 0.9),
            &ReplayMeta {
                split: Some(Split::Train),
                ..Default::default()
            },
        )
        .unwrap();
        let g = gate_from_rows(d, night, text, &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Unverified, 0), "{g:?}");
        // Mixed pass: the val pairs decide; one val case regressing badly rejects even
        // though the train pairs would have carried the mean.
        for (c, b, k) in [("c2", 0.5, 0.6), ("c3", 0.5, 0.65)] {
            log_row(d, &row(c, None, b)).unwrap();
            log_row(d, &row(c, Some(&sha), k)).unwrap();
        }
        let g = gate_from_rows(d, night, text, &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Unverified, 2), "{g:?}");
        // The meta on c1 corrected to val: three val pairs, accepted on val alone.
        log_row_meta(
            d,
            &row("c1", None, 0.3),
            &ReplayMeta {
                split: Some(Split::Val),
                ..Default::default()
            },
        )
        .unwrap();
        log_row_meta(
            d,
            &row("c1", Some(&sha), 0.5),
            &ReplayMeta {
                split: Some(Split::Val),
                ..Default::default()
            },
        )
        .unwrap();
        let g = gate_from_rows(d, night, text, &cfg).unwrap();
        assert_eq!((g.verdict, g.val_cases), (Verdict::Accepted, 3), "{g:?}");
        assert!((g.baseline - (0.3 + 0.5 + 0.5) / 3.0).abs() < 1e-9);
        // Train rows never entered: the train-only candidate mean (0.9) is nowhere in it.
        assert!(g.candidate < 0.7);
    }
}
