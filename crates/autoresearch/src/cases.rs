//! 159 W2 — tuning prompts: the engine-side case runner, the paired gate on val cases and the
//! case strip (159 §7, §13). Owner: W2. Signatures frozen by C; bodies W2's. `crates/evalcore`
//! holds the shared pieces (split, judge prompt, paired gate, rows) that dreaming shares.
//!
//! How a prompt study flows through the 140 loop:
//! - `Scoring::Cases` routes every sample (baseline, candidate, incumbent re-measure, holdout)
//!   to [`sample`]: the artefact is read from the worktree, the case set from its custody
//!   snapshot, each case is rendered, answered by the target model (`Backend::side_call`,
//!   charged to the study session) and scored by its check or the 0–1 judge. One `Sample`
//!   per tree: `value` = the val mean (holdout mean for the holdout role), `case_values[id]`
//!   = `{score, tokens_in, tokens_out, cost}`, `cases` = the **train** ids only (what the
//!   brief's "cases that moved" may show), `asi["train:<id>"]` = the failing train cases'
//!   input, answer and reason (the GEPA-style feedback of [`brief_section`]).
//! - `GateRule::Paired` routes the decision to [`step`]: per val case, the candidate's mean
//!   score minus the incumbent's; `evalcore::gate::decide` (margin + paired t, W-n/W-r).
//! - Guards in code (§7): the split is a stable hash; val and holdout content reaches only the
//!   judge and the ledger; a candidate whose text shares an 8-word sequence with any val or
//!   holdout input or ideal is refused before any call; the case set is snapshotted into
//!   custody at the first sample and never re-read from the source.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use evalcore::gate::{Decision, Pair};
use evalcore::run::Model;
use evalcore::{Case, Shares, Split};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{Store, StudyRecord};
use crate::types::{
    Call, CaseRow, CaseSetSpec, CaseSource, Effect, GateCfg, GateDecision, GateInput, GateRule,
    GateStep, JudgmentRow, KindView, ObjectiveDelta, Sample, SampleRole, SplitCounts, StudySpec,
    Verdict,
};

/// Model call time budget per case call.
pub const CALL_BUDGET: Duration = Duration::from_secs(180);
/// Target calls in flight per sample (§7 Cost: "8 calls in flight").
pub const IN_FLIGHT: usize = 8;
/// Train cases replayed per sample when `cases.train_shown` is 0.
pub const TRAIN_SHOWN: u32 = 16;
/// Failing train cases the brief shows (§7 Mechanism).
pub const BRIEF_CASES: usize = 8;
/// Val pairs a decision needs (§7 rule 4); Start refuses fewer val cases.
pub const MIN_VAL: usize = 10;
/// The leakage window: a candidate sharing this many consecutive words with a val or holdout
/// input or ideal is refused (§7 Guard).
pub const LEAK_WORDS: usize = 8;
/// `max_regression` when the headline objective has none (§7 rule 3).
pub const DEFAULT_MAX_REGRESSION: f64 = 0.2;
/// `min_effect` when the spec's is empty (§7 rule 2).
pub const DEFAULT_MIN_EFFECT: f64 = 0.05;
/// The custody snapshot's file name under `evaluator/<segment>/`.
pub const SNAPSHOT: &str = "cases.jsonl";
/// What `bench.command` says for a prompt study (W2-e): `create` and the setup panel still
/// require a bench command, and this word names the engine's case runner. `custody::take`
/// treats it as a program name; [`sample`] never runs it.
pub const BENCH_COMMAND: &str = "cases";
/// `asi` key prefix of a failing train case.
const ASI_TRAIN: &str = "train:";
/// Chars of input / answer / reason kept per brief line.
const CLIP: usize = 160;

// ───────────────────────────── the case set ─────────────────────────────

fn spec_cases(spec: &StudySpec) -> Result<&CaseSetSpec> {
    spec.cases
        .as_ref()
        .ok_or_else(|| Error::refused("a prompt study needs a `cases` block (159 §7)"))
}

/// `train/val/holdout` percent shares; empty = 40/40/20 (W-g).
pub fn shares(cs: &CaseSetSpec) -> std::result::Result<Shares, String> {
    let s = cs.split.trim();
    if s.is_empty() {
        return Ok(Shares::default());
    }
    let parts: Vec<u8> = s
        .split('/')
        .map(|p| {
            p.trim()
                .parse::<u8>()
                .map_err(|_| format!("cases.split `{s}`: three percents like 40/40/20"))
        })
        .collect::<std::result::Result<_, _>>()?;
    let [train, val, holdout] = parts[..] else {
        return Err(format!("cases.split `{s}`: three percents like 40/40/20"));
    };
    if u32::from(train) + u32::from(val) + u32::from(holdout) != 100 {
        return Err(format!("cases.split `{s}`: the shares must sum to 100"));
    }
    if val == 0 {
        return Err(format!(
            "cases.split `{s}`: val must be above 0 (it decides)"
        ));
    }
    Ok(Shares {
        train,
        val,
        holdout,
    })
}

/// Parse a JSONL case file (`{id, input, ideal?, check?, tags}`; `synthetic` tolerated).
pub fn parse_cases(text: &str) -> std::result::Result<Vec<Case>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let c: Case =
            serde_json::from_str(line).map_err(|e| format!("cases line {}: {e}", i + 1))?;
        if c.id.trim().is_empty() || c.input.trim().is_empty() {
            return Err(format!(
                "cases line {}: `id` and `input` are required",
                i + 1
            ));
        }
        if !seen.insert(c.id.clone()) {
            return Err(format!("cases line {}: duplicate id `{}`", i + 1, c.id));
        }
        out.push(c);
    }
    Ok(out)
}

/// A dreaming case line, as much of it as a skill study reads (159 §13.4): `context` is the
/// input, `ideal` + `user_words` the judge's references; synthetic is never val.
#[derive(serde::Deserialize)]
struct DreamCase {
    id: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    user_words: String,
    #[serde(default)]
    ideal: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    skill_hint: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    state: String,
}

/// `dreaming/cases.jsonl` folded by id (the last line wins), filtered by `skill_hint` and by
/// state (`open`, `reopened`, `closed`), as the shared core's cases.
pub fn dreaming_cases(path: &Path, skill_hint: &str) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Error::refused(format!(
            "cases.source dreaming: {} is unreadable: {e}",
            path.display()
        ))
    })?;
    let mut folded: BTreeMap<String, DreamCase> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(c) = serde_json::from_str::<DreamCase>(line) {
            folded.insert(c.id.clone(), c);
        }
    }
    Ok(folded
        .into_values()
        .filter(|c| c.skill_hint == skill_hint)
        .filter(|c| matches!(c.state.as_str(), "" | "open" | "reopened" | "closed"))
        .map(|c| Case {
            id: c.id,
            input: format!(
                "{}\n\n[The transcript stops here. Your next turn:]",
                c.context
            ),
            ideal: Some(c.ideal),
            references: vec![c.user_words],
            check: None,
            tags: c.tags,
            synthetic: c.origin == "synthetic",
        })
        .collect())
}

fn snapshot_path(env: &Env, study: &StudyRecord) -> PathBuf {
    env.evaluator_dir(&study.slug, study.segment).join(SNAPSHOT)
}

/// The study's cases with their split. The custody snapshot (`evaluator/<segment>/cases.jsonl`)
/// is read when it exists; otherwise it is written once from the source — the `file` in the
/// worktree at `worktree`, or dreaming's store — and its hash joins the segment's custody map,
/// so a later change to the source never changes the set (§7 Guard).
pub async fn case_set(
    env: &Env,
    study: &StudyRecord,
    worktree: &Path,
) -> Result<Vec<(Case, Split)>> {
    let cs = spec_cases(&study.spec)?;
    let snap = snapshot_path(env, study);
    let text = match std::fs::read_to_string(&snap) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut cases = match cs.source {
                CaseSource::File => {
                    if cs.path.trim().is_empty() {
                        return Err(Error::refused("cases.path is empty"));
                    }
                    let custody_copy = env.evaluator_dir(&study.slug, study.segment).join(&cs.path);
                    let src = if custody_copy.is_file() {
                        custody_copy
                    } else {
                        worktree.join(&cs.path)
                    };
                    let t = std::fs::read_to_string(&src).map_err(|e| {
                        Error::refused(format!("cases.path {}: {e}", src.display()))
                    })?;
                    parse_cases(&t).map_err(Error::refused)?
                }
                CaseSource::Dreaming => {
                    if cs.skill_hint.trim().is_empty() {
                        return Err(Error::refused("cases.skill_hint is empty"));
                    }
                    dreaming_cases(
                        &env.data_dir.join("dreaming").join("cases.jsonl"),
                        cs.skill_hint.trim(),
                    )?
                }
            };
            if cs.max_cases > 0 {
                cases.truncate(cs.max_cases as usize);
            }
            let text: String = cases
                .iter()
                .map(|c| serde_json::to_string(c).expect("case serialises") + "\n")
                .collect();
            if let Some(d) = snap.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&snap, &text)?;
            if let Some(mut seg) = env.store.get_segment(&study.slug, study.segment).await? {
                seg.custody.insert(
                    format!("{}{SNAPSHOT}", crate::custody::CUSTODY_ONLY),
                    crate::custody::sha256_file(&snap)?,
                );
                env.store.put_segment(&seg).await?;
            }
            text
        }
        Err(e) => return Err(e.into()),
    };
    let shares = shares(cs).map_err(Error::refused)?;
    let cases = parse_cases(&text).map_err(Error::refused)?;
    Ok(cases
        .into_iter()
        .map(|c| {
            let s = evalcore::split(&study.slug, &c.id, shares, c.synthetic);
            (c, s)
        })
        .collect())
}

fn counts(set: &[(Case, Split)]) -> SplitCounts {
    let mut c = SplitCounts::default();
    for (_, s) in set {
        match s {
            Split::Train => c.train += 1,
            Split::Val => c.val += 1,
            Split::Holdout => c.holdout += 1,
        }
    }
    c
}

// ───────────────────────────── leakage ─────────────────────────────

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The first [`LEAK_WORDS`]-word sequence `artefact` shares with any `protected` text, as the
/// words joined by spaces; `None` when it shares none.
pub fn leak<'a>(artefact: &str, protected: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut grams: HashSet<Vec<String>> = HashSet::new();
    for p in protected {
        let w = words(p);
        for g in w.windows(LEAK_WORDS) {
            grams.insert(g.to_vec());
        }
    }
    if grams.is_empty() {
        return None;
    }
    words(artefact)
        .windows(LEAK_WORDS)
        .find(|g| grams.contains(*g))
        .map(|g| g.join(" "))
}

// ───────────────────────────── the runner ─────────────────────────────

fn vendor(model: &str) -> &str {
    model.split('/').next().unwrap_or(model)
}

/// The target model: `models.target`, else the executor (`agent.model`).
pub fn target_model(env: &Env, spec: &StudySpec) -> String {
    spec.models
        .target
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| env.cfg.executor(&spec.models, &env.agent))
}

/// The judge model: `models.judge`, else the first `agent:` model of another vendor than the
/// target (§7 Guard: the judge defaults to a vendor other than the target's), else the target.
pub fn judge_model(env: &Env, spec: &StudySpec) -> String {
    let target = target_model(env, spec);
    spec.models
        .judge
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            env.agent
                .others
                .iter()
                .find(|m| vendor(m) != vendor(&target))
                .cloned()
                .unwrap_or(target)
        })
}

/// A model behind `Backend::side_call`, charged to the study session; it sums what the
/// provider reported so the sample carries tokens and cost per case.
struct StudyModel {
    env: Arc<Env>,
    study: Arc<StudyRecord>,
    model: String,
    spent: Mutex<Spent>,
}

#[derive(Clone, Copy, Default)]
struct Spent {
    tokens_in: u64,
    tokens_out: u64,
    cost_nanos: u64,
}

impl StudyModel {
    fn new(env: &Arc<Env>, study: &Arc<StudyRecord>, model: String) -> Arc<Self> {
        Arc::new(Self {
            env: env.clone(),
            study: study.clone(),
            model,
            spent: Mutex::new(Spent::default()),
        })
    }

    fn take(&self) -> Spent {
        std::mem::take(&mut *self.spent.lock())
    }
}

#[async_trait::async_trait]
impl Model for StudyModel {
    async fn call(&self, system: &str, user: String) -> std::result::Result<String, String> {
        let reply = self
            .env
            .backend
            .side_call(
                &self.study,
                runtime::ModelRole::Named(self.model.clone()),
                system,
                user,
                CALL_BUDGET,
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut s = self.spent.lock();
        s.tokens_in += u64::from(reply.input_tokens.unwrap_or(0));
        s.tokens_out += u64::from(reply.output_tokens.unwrap_or(0));
        s.cost_nanos += reply.cost_nanos.unwrap_or(0);
        Ok(reply.text)
    }
}

/// One case's result inside a sample.
struct Scored {
    case: Case,
    split: Split,
    score: f64,
    answer: String,
    reason: String,
    spent: Spent,
}

fn log_name(k: u32, role: SampleRole) -> String {
    match role {
        SampleRole::Candidate => format!("sample-{k}.log"),
        SampleRole::Incumbent => format!("sample-{k}-incumbent.log"),
        SampleRole::Baseline => format!("sample-{k}-baseline.log"),
        SampleRole::Holdout => format!("sample-{k}-holdout.log"),
        SampleRole::Final => format!("sample-{k}-final.log"),
    }
}

/// `<slug>/runs/<n>/…` from the absolute log path (runner.rs's convention).
fn root_relative(log: &Path) -> String {
    let parts: Vec<String> = log
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let start = parts
        .iter()
        .rposition(|c| c == "runs")
        .and_then(|i| i.checked_sub(1))
        .unwrap_or(parts.len().saturating_sub(4));
    parts[start..].join("/")
}

fn clip(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= CLIP {
        one
    } else {
        one.chars().take(CLIP).collect::<String>() + "…"
    }
}

fn crash(
    req: &SampleRequest<'_>,
    log: &Path,
    failure: String,
    started: std::time::Instant,
) -> Sample {
    let _ = std::fs::write(log, format!("{failure}\n"));
    Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        value: None,
        exit: Some(1),
        wall_ms: started.elapsed().as_millis() as u64,
        log_path: Some(root_relative(log)),
        at: crate::store::now(),
        failure: Some(failure),
        ..Sample::default()
    }
}

/// Engine-side scoring for this kind (`Scoring` dispatch in actor.rs): one `Sample` per tree.
/// Val cases (holdout cases for the holdout role) plus up to `train_shown` train cases, each
/// `repeats` times, [`IN_FLIGHT`] calls at a time; one `judgments` row per case.
pub async fn sample(
    env: &Env,
    study: &StudyRecord,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    let started = std::time::Instant::now();
    std::fs::create_dir_all(req.runs_dir)?;
    let log = req.runs_dir.join(log_name(req.k, req.role));
    let cs = spec_cases(&study.spec)?;
    if cs.artefact.trim().is_empty() {
        return Err(Error::refused(
            "cases.artefact is empty (the text file the study edits)",
        ));
    }
    let artefact = match std::fs::read_to_string(req.worktree.join(&cs.artefact)) {
        Ok(t) => t,
        Err(e) => {
            return Ok(crash(
                &req,
                &log,
                format!("artefact `{}` unreadable: {e}", cs.artefact),
                started,
            ));
        }
    };
    let set = case_set(env, study, req.worktree).await?;
    let (val, holdout): (Vec<_>, Vec<_>) = set
        .iter()
        .filter(|(_, s)| *s != Split::Train)
        .partition(|(_, s)| *s == Split::Val);
    if req.role == SampleRole::Candidate
        && let Some(words) = leak(
            &artefact,
            val.iter()
                .chain(holdout.iter())
                .flat_map(|(c, _)| std::iter::once(c.input.as_str()).chain(c.ideal.as_deref())),
        )
    {
        return Ok(crash(
            &req,
            &log,
            format!(
                "refused: the artefact repeats {LEAK_WORDS} words of a val or holdout case (\"{words}\"); \
                 the eval set is not for pasting"
            ),
            started,
        ));
    }
    let train_shown = if cs.train_shown == 0 {
        TRAIN_SHOWN
    } else {
        cs.train_shown
    } as usize;
    let chosen: Vec<(Case, Split)> = if req.role == SampleRole::Holdout {
        holdout.into_iter().cloned().collect()
    } else {
        val.into_iter()
            .cloned()
            .chain(
                set.iter()
                    .filter(|(_, s)| *s == Split::Train)
                    .take(train_shown)
                    .cloned(),
            )
            .collect()
    };
    if chosen.is_empty() {
        return Ok(crash(
            &req,
            &log,
            format!(
                "no {} cases in the set",
                if req.role == SampleRole::Holdout {
                    "holdout"
                } else {
                    "val"
                }
            ),
            started,
        ));
    }
    let env_a = Arc::new(env.clone());
    let study_a = Arc::new(study.clone());
    let target = StudyModel::new(&env_a, &study_a, target_model(env, &study.spec));
    let judge_name = judge_model(env, &study.spec);
    let judge = StudyModel::new(&env_a, &study_a, judge_name.clone());
    let needs_judge = chosen.iter().any(|(c, _)| c.check.is_none());
    let artefact = Arc::new(artefact);
    let template: Option<Arc<str>> = Some(cs.render.trim())
        .filter(|t| !t.is_empty())
        .map(Arc::from);
    let repeats = cs.repeats.max(1);
    let sem = Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT));
    let mut set_js: tokio::task::JoinSet<
        std::result::Result<(usize, Vec<evalcore::run::CaseScore>), String>,
    > = tokio::task::JoinSet::new();
    for (i, (case, _)) in chosen.iter().enumerate() {
        let (sem, target, judge, artefact, template, case) = (
            sem.clone(),
            target.clone(),
            judge.clone(),
            artefact.clone(),
            template.clone(),
            case.clone(),
        );
        set_js.spawn(async move {
            let _permit = sem.acquire().await.map_err(|e| e.to_string())?;
            let mut scores = Vec::with_capacity(usize::from(repeats));
            for _ in 0..repeats {
                scores.push(
                    evalcore::run::run_case(
                        target.as_ref(),
                        needs_judge.then_some(judge.as_ref() as &dyn Model),
                        &artefact,
                        template.as_deref(),
                        &case,
                    )
                    .await?,
                );
            }
            Ok((i, scores))
        });
    }
    let mut results: Vec<Option<Vec<evalcore::run::CaseScore>>> = vec![None; chosen.len()];
    let mut failure: Option<String> = None;
    while let Some(r) = set_js.join_next().await {
        if cancel.is_cancelled() {
            set_js.abort_all();
            return Ok(Sample {
                n: req.n,
                k: req.k,
                tree_sha: req.tree_sha.to_owned(),
                role: Some(req.role),
                lost: true,
                at: crate::store::now(),
                ..Sample::default()
            });
        }
        match r {
            Ok(Ok((i, scores))) => results[i] = Some(scores),
            Ok(Err(e)) => {
                failure.get_or_insert(e);
                set_js.abort_all();
            }
            Err(e) => {
                failure.get_or_insert(format!("case task failed: {e}"));
                set_js.abort_all();
            }
        }
    }
    if let Some(f) = failure {
        return Ok(crash(&req, &log, format!("case call failed: {f}"), started));
    }
    // Per-case spend is not attributable with calls in flight; the sample's totals are, and the
    // per-case share is the mean (ponytail: exact per-case tokens if a report objective needs them).
    let spent_total = {
        let t = target.take();
        let j = judge.take();
        Spent {
            tokens_in: t.tokens_in + j.tokens_in,
            tokens_out: t.tokens_out + j.tokens_out,
            cost_nanos: t.cost_nanos + j.cost_nanos,
        }
    };
    let per_case = chosen.len() as u64;
    let scored: Vec<Scored> = chosen
        .into_iter()
        .zip(results)
        .map(|((case, split), scores)| {
            let scores = scores.expect("every case scored");
            let score = scores.iter().map(|s| s.score).sum::<f64>() / scores.len() as f64;
            let last = scores.last().expect("at least one repeat");
            Scored {
                case,
                split,
                score,
                answer: last.answer.clone(),
                reason: last.reason.clone(),
                spent: Spent {
                    tokens_in: spent_total.tokens_in / per_case,
                    tokens_out: spent_total.tokens_out / per_case,
                    cost_nanos: spent_total.cost_nanos / per_case,
                },
            }
        })
        .collect();

    // The log: one CASE line per case (the live tail), then the means.
    let mut text = String::new();
    let mut case_values: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut cases: BTreeMap<String, f64> = BTreeMap::new();
    let mut asi: BTreeMap<String, String> = BTreeMap::new();
    let mut by_split: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let at = crate::store::now();
    for s in &scored {
        text.push_str(&format!(
            "CASE {} score={:.3} split={}{}\n",
            s.case.id,
            s.score,
            s.split.word(),
            if s.reason.is_empty() {
                String::new()
            } else {
                format!(" — {}", clip(&s.reason))
            }
        ));
        case_values.insert(
            s.case.id.clone(),
            BTreeMap::from([
                ("score".to_owned(), s.score),
                ("tokens_in".to_owned(), s.spent.tokens_in as f64),
                ("tokens_out".to_owned(), s.spent.tokens_out as f64),
                ("cost".to_owned(), s.spent.cost_nanos as f64 / 1e9),
            ]),
        );
        by_split.entry(s.split.word()).or_default().push(s.score);
        if s.split == Split::Train {
            cases.insert(s.case.id.clone(), s.score);
            if s.score < 1.0 {
                asi.insert(
                    format!("{ASI_TRAIN}{}", s.case.id),
                    format!(
                        "score {:.2} · input: {} · answer: {} · why: {}",
                        s.score,
                        clip(&s.case.input),
                        clip(&s.answer),
                        clip(&s.reason)
                    ),
                );
            }
        }
        env.store
            .judgments_insert(&JudgmentRow {
                id: 0,
                slug: study.slug.clone(),
                n: req.n,
                case_id: s.case.id.clone(),
                split: s.split.word().to_owned(),
                a_sha: req.tree_sha.to_owned(),
                b_sha: None,
                a_first: None,
                model: if s.case.check.is_some() {
                    "check".to_owned()
                } else {
                    judge_name.clone()
                },
                score: Some(s.score),
                winner: None,
                reason: s.reason.clone(),
                human: false,
                cost_nanos: s.spent.cost_nanos,
                at: at.clone(),
            })
            .await?;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let deciding = if req.role == SampleRole::Holdout {
        "holdout"
    } else {
        "val"
    };
    let value = by_split.get(deciding).map(|v| mean(v));
    for (split, v) in &by_split {
        text.push_str(&format!(
            "MEAN {split}={:.4} ({} cases)\n",
            mean(v),
            v.len()
        ));
    }
    text.push_str(&format!(
        "METRIC {}={:.4}\nSPEND tokens_in={} tokens_out={} usd={:.4}\n",
        study.spec.metric.name,
        value.unwrap_or(0.0),
        spent_total.tokens_in,
        spent_total.tokens_out,
        spent_total.cost_nanos as f64 / 1e9
    ));
    std::fs::write(&log, text)?;
    let n_cases = scored.len() as f64;
    Ok(Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        value,
        secondaries: BTreeMap::from([
            (
                "tokens_out".to_owned(),
                spent_total.tokens_out as f64 / n_cases,
            ),
            ("cost_usd".to_owned(), spent_total.cost_nanos as f64 / 1e9),
        ]),
        cases,
        asi,
        exit: Some(0),
        wall_ms: started.elapsed().as_millis() as u64,
        log_path: Some(root_relative(&log)),
        at,
        case_values,
        // Engine-side calls are SideCalls on the study session (the Session channel, §5.8):
        // nothing on the Bench channel, or the money would count twice.
        spend_nanos: 0,
        ..Sample::default()
    })
}

// ───────────────────────────── the decision ─────────────────────────────

/// Per-case mean score over the samples that have it (`case_values[id].score`).
fn case_means(samples: &[Sample]) -> BTreeMap<String, f64> {
    let mut acc: BTreeMap<String, (f64, u32)> = BTreeMap::new();
    for s in samples
        .iter()
        .filter(|s| !s.noisy && !s.lost && s.role != Some(SampleRole::Holdout))
    {
        for (id, v) in &s.case_values {
            if let Some(score) = v.get("score") {
                let e = acc.entry(id.clone()).or_insert((0.0, 0));
                e.0 += score;
                e.1 += 1;
            }
        }
    }
    acc.into_iter()
        .map(|(id, (sum, n))| (id, sum / f64::from(n)))
        .collect()
}

/// The val pairs of a candidate against a reference, over the study's case set.
fn pairs_of(val_ids: &BTreeSet<String>, candidate: &[Sample], reference: &[Sample]) -> Vec<Pair> {
    let c = case_means(candidate);
    let r = case_means(reference);
    val_ids
        .iter()
        .filter_map(|id| {
            Some(Pair {
                id: id.clone(),
                baseline: *r.get(id)?,
                candidate: *c.get(id)?,
            })
        })
        .collect()
}

fn pct(delta: f64, reference: f64) -> f64 {
    if reference == 0.0 {
        0.0
    } else {
        delta / reference.abs() * 100.0
    }
}

/// The kind's decision rule (`GateRule` dispatch in actor.rs): §7 "Decision (paired, on val)".
/// Asks for one incumbent re-measure when the incumbent's newest val scores are older than
/// `remeasure_after` experiments (the provider's model drifts); otherwise decides at once.
pub async fn step(
    env: &Env,
    study: &StudyRecord,
    cfg: &GateCfg,
    input: &GateInput,
    candidate: &[Sample],
    reference: &[Sample],
) -> Result<GateStep> {
    if input.candidate.is_empty() {
        return Ok(GateStep::Confirm {
            more: 1,
            incumbent_sample: false,
        });
    }
    if reference
        .iter()
        .all(|s| s.role == Some(SampleRole::Holdout))
        || (cfg.remeasure_after > 0 && input.incumbent_age >= cfg.remeasure_after)
    {
        return Ok(GateStep::Confirm {
            more: 0,
            incumbent_sample: true,
        });
    }
    let worktree = crate::actor::worktree_of(study)?;
    let set = case_set(env, study, &worktree).await?;
    let val_ids: BTreeSet<String> = set
        .iter()
        .filter(|(_, s)| *s == Split::Val)
        .map(|(c, _)| c.id.clone())
        .collect();
    let pairs = pairs_of(&val_ids, candidate, reference);
    let cand_mean = crate::actor::mean(&pairs.iter().map(|p| p.candidate).collect::<Vec<_>>());
    let ref_mean = crate::actor::mean(&pairs.iter().map(|p| p.baseline).collect::<Vec<_>>());
    let direction = cfg.direction;
    let delta = crate::gate::normalise(direction, cand_mean, ref_mean);
    let delta_pct = pct(cand_mean - ref_mean, ref_mean);
    // Direction-normalised pairs: the gate reads "candidate − baseline ≥ margin".
    let pairs: Vec<Pair> = pairs
        .into_iter()
        .map(|p| match direction {
            crate::types::Direction::Higher => p,
            crate::types::Direction::Lower => Pair {
                id: p.id,
                baseline: -p.baseline,
                candidate: -p.candidate,
            },
        })
        .collect();
    let min_effect = match cfg.min_effect {
        Effect::Absolute(v) if v > 0.0 => v,
        Effect::Relative(_) => cfg.min_effect.against(ref_mean).abs(),
        _ => DEFAULT_MIN_EFFECT,
    };
    let max_regression = Effect::parse(&study.spec.headline().max_regression)
        .ok()
        .map(|e| e.against(ref_mean).abs())
        .filter(|v| *v > 0.0)
        .unwrap_or(DEFAULT_MAX_REGRESSION);
    let gcfg = evalcore::gate::Cfg {
        min_mean_delta: min_effect,
        max_case_regression: max_regression,
        min_pairs: MIN_VAL,
    };
    let paired = evalcore::gate::paired(&pairs);
    let (t, critical) = paired.map_or((None, None), |p| {
        (p.t.is_finite().then_some(p.t), Some(p.critical))
    });
    let decision = evalcore::gate::decide(&pairs, &gcfg);
    let (verdict, call) = match &decision {
        Decision::Accept => (Verdict::Improved, Call::Better),
        Decision::Reject(_) => {
            let worse = paired.is_some_and(|p| p.mean < 0.0 && p.t <= -p.critical);
            if worse {
                (Verdict::Worse, Call::Worse)
            } else {
                (Verdict::NoGain, Call::Same)
            }
        }
        Decision::Unverified(_) => (Verdict::NoGain, Call::Same),
    };
    let headline = study.spec.headline();
    let detail = GateDecision {
        v: GateDecision::V,
        rule: GateRule::Paired,
        objectives: vec![ObjectiveDelta {
            name: headline.name.clone(),
            role: headline.role,
            candidate: Some(cand_mean),
            reference: Some(ref_mean),
            delta: Some(delta),
            delta_pct: Some(delta_pct),
            z: t,
            call,
        }],
        stat: t,
        threshold: critical,
        n: pairs.len() as u32,
    };
    let why = match &decision {
        Decision::Accept => String::new(),
        Decision::Reject(r) | Decision::Unverified(r) => r.clone(),
    };
    if !why.is_empty() {
        env.store
            .append_event(
                &study.slug,
                "paired_gate",
                &serde_json::json!({ "n": candidate.first().map_or(0, |s| s.n), "why": why, "pairs": pairs.len() }),
            )
            .await?;
    }
    Ok(GateStep::Decide {
        verdict,
        delta,
        delta_pct,
        z: t,
        detail: Some(detail),
    })
}

// ───────────────────────────── views, brief, headline ─────────────────────────────

/// Whether a judgment row is a point of the strip: the base (n = 0), a candidate tree (`exp:…`),
/// or a holdout case (measured on the incumbent after a keep).
fn strip_row(j: &JudgmentRow) -> bool {
    j.n == 0 || j.split == "holdout" || j.a_sha.starts_with("exp:")
}

/// The case × experiment rows from the `judgments` table, and how many scored cases each split
/// has (train counts the cases replayed for feedback, not the whole train share).
pub fn rows_from(judgments: &[JudgmentRow]) -> (Vec<CaseRow>, SplitCounts) {
    /// Per experiment: the scores' sum and count.
    type Sums = BTreeMap<u32, (f64, u32)>;
    let mut acc: BTreeMap<String, (String, Sums)> = BTreeMap::new();
    for j in judgments {
        let Some(score) = j.score else { continue };
        let e = acc
            .entry(j.case_id.clone())
            .or_insert_with(|| (j.split.clone(), BTreeMap::new()));
        if strip_row(j) {
            let v = e.1.entry(j.n).or_insert((0.0, 0));
            v.0 += score;
            v.1 += 1;
        }
    }
    let mut split = SplitCounts::default();
    let rows = acc
        .into_iter()
        .map(|(id, (s, values))| {
            match s.as_str() {
                "train" => split.train += 1,
                "val" => split.val += 1,
                _ => split.holdout += 1,
            }
            CaseRow {
                id,
                split: s,
                values: values
                    .into_iter()
                    .map(|(n, (sum, c))| (n, sum / f64::from(c)))
                    .collect(),
            }
        })
        .collect();
    (rows, split)
}

/// `KindView::Cases` for the Study view.
pub async fn view(store: &Store, study: &StudyRecord) -> Result<KindView> {
    let (rows, split) = rows_from(&store.judgments_list(&study.slug, None).await?);
    Ok(KindView::Cases { rows, split })
}

/// `autoresearch.cases {slug, n?}` on a study's view: every row, or each row restricted to
/// experiment `n` (rows without a value at `n` dropped). Not a prompt study → an error.
pub fn rows_of(view: KindView, n: Option<u32>) -> std::result::Result<Vec<CaseRow>, String> {
    let KindView::Cases { rows, .. } = view else {
        return Err("not a prompt study".to_owned());
    };
    Ok(match n {
        None => rows,
        Some(n) => rows
            .into_iter()
            .filter_map(|mut r| {
                let v = r.values.remove(&n)?;
                r.values = BTreeMap::from([(n, v)]);
                Some(r)
            })
            .collect(),
    })
}

/// `autoresearch.cases {slug, n?}`: the case × experiment rows the heat strip shows.
pub async fn rows(store: &Store, slug: &str, n: Option<u32>) -> Result<Vec<CaseRow>> {
    let (rows, _) = rows_from(&store.judgments_list(slug, n).await?);
    Ok(rows)
}

/// The brief's per-kind section: the incumbent's failing train cases (input, answer, reason;
/// never an ideal, never a val or holdout case), from the newest sample of the incumbent tree.
pub async fn brief_section(store: &Store, study: &StudyRecord) -> Result<String> {
    let Some(incumbent) = study.incumbent_sha.as_deref() else {
        return Ok(String::new());
    };
    let samples = store.samples_of_tree(&study.slug, incumbent).await?;
    let Some(newest) = samples
        .iter()
        .rev()
        .find(|s| s.role != Some(SampleRole::Holdout) && !s.lost && s.value.is_some())
    else {
        return Ok(String::new());
    };
    let mut failing: Vec<(&str, &str)> = newest
        .asi
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(ASI_TRAIN).map(|id| (id, v.as_str())))
        .collect();
    let val_mean = newest.value.unwrap_or(0.0);
    let mut out = format!(
        "## Prompt study\n\nYou are editing `{}`; the val score of the current text is {val_mean:.3} (0–1, higher is better). \
         A keep needs the paired per-case gain over the val cases to clear the noise and the margin; the val and holdout cases themselves are never shown. \
         Edit the rules and the wording of the artefact — never paste answers.\n",
        study
            .spec
            .cases
            .as_ref()
            .map(|c| c.artefact.as_str())
            .unwrap_or("the artefact")
    );
    if failing.is_empty() {
        out.push_str("\nEvery train case shown passed on the current text.\n");
        return Ok(out);
    }
    // Lowest scores first (`score 0.00 · …` sorts before `score 0.50 · …`), then at most 8.
    failing.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
    failing.truncate(BRIEF_CASES);
    out.push_str("\n### Failing train cases\n\n");
    for (id, line) in failing {
        out.push_str(&format!("- `{id}`: {line}\n"));
    }
    Ok(out)
}

/// Finalize's headline for this kind: `val score 0.52 → 0.81 on 48 cases`, with the holdout
/// line when the study has one. Base = the segment-0 baseline samples; final = the incumbent's
/// newest val samples. `None` only when nothing was measured (the scalar path then runs).
pub async fn headline(store: &Store, study: &StudyRecord) -> Result<Option<Headline>> {
    let (Some(base), Some(incumbent)) = (study.base_sha.as_deref(), study.incumbent_sha.as_deref())
    else {
        return Ok(None);
    };
    let clean = |v: Vec<Sample>, role: SampleRole| -> Vec<f64> {
        v.into_iter()
            .filter(|s| s.role == Some(role) && !s.noisy && !s.lost)
            .filter_map(|s| s.value)
            .collect()
    };
    let base_samples = store.samples_of_tree(&study.slug, base).await?;
    let inc_samples = store.samples_of_tree(&study.slug, incumbent).await?;
    let base_vals = clean(base_samples.clone(), SampleRole::Baseline);
    let final_vals: Vec<f64> = if incumbent == base {
        base_vals.clone()
    } else {
        inc_samples
            .iter()
            .filter(|s| !s.noisy && !s.lost && s.role != Some(SampleRole::Holdout))
            .filter_map(|s| s.value)
            .collect()
    };
    if base_vals.is_empty() || final_vals.is_empty() {
        return Ok(None);
    }
    let (b, f) = (
        crate::actor::mean(&base_vals),
        crate::actor::mean(&final_vals),
    );
    let n_cases = store
        .judgments_list(&study.slug, Some(0))
        .await?
        .into_iter()
        .filter(|j| j.split == "val")
        .map(|j| j.case_id)
        .collect::<BTreeSet<_>>()
        .len();
    let holdout = {
        let hb = clean(base_samples, SampleRole::Holdout);
        let hf = clean(inc_samples, SampleRole::Holdout);
        (!hb.is_empty() && !hf.is_empty()).then(|| {
            format!(
                "holdout {:.3} → {:.3}",
                crate::actor::mean(&hb),
                crate::actor::mean(&hf)
            )
        })
    };
    Ok(Some(Headline {
        text: format!("val score {b:.2} → {f:.2} on {n_cases} cases"),
        base_mean: b,
        final_mean: f,
        delta_pct: pct(f - b, b),
        holdout,
        sigma: 0.0,
        n: (base_vals.len() as u32, final_vals.len() as u32),
    }))
}

// ───────────────────────────── draft card and setup rows ─────────────────────────────

/// Draft card line 1 for a prompt study: "Tune `prompts/x.md` on 120 cases (48 train · 48 val ·
/// 24 holdout) with `<target>`, judged by `<judge>`." The counts come from the case file when
/// it is readable at draft time; a dreaming source names its `skill_hint`.
pub fn card_line(spec: &StudySpec) -> String {
    let Some(cs) = spec.cases.as_ref() else {
        return format!("prompt study `{}`: no `cases` block", spec.slug);
    };
    let artefact = if cs.artefact.is_empty() {
        "the artefact"
    } else {
        cs.artefact.as_str()
    };
    let set = match cs.source {
        CaseSource::Dreaming => format!("dreaming's `{}` cases", cs.skill_hint),
        CaseSource::File => {
            let counted = shares(cs).ok().and_then(|sh| {
                let p = Path::new(&spec.repo).join(&cs.path);
                let text = std::fs::read_to_string(p).ok()?;
                let mut cases = parse_cases(&text).ok()?;
                if cs.max_cases > 0 {
                    cases.truncate(cs.max_cases as usize);
                }
                let set: Vec<(Case, Split)> = cases
                    .into_iter()
                    .map(|c| {
                        let s = evalcore::split(&spec.slug, &c.id, sh, c.synthetic);
                        (c, s)
                    })
                    .collect();
                let c = counts(&set);
                Some(format!(
                    "{} cases ({} train · {} val · {} holdout)",
                    set.len(),
                    c.train,
                    c.val,
                    c.holdout
                ))
            });
            counted.unwrap_or_else(|| format!("the cases in `{}`", cs.path))
        }
    };
    let target = spec
        .models
        .target
        .as_deref()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or("the session model");
    let scoring = match spec
        .models
        .judge
        .as_deref()
        .filter(|m| !m.trim().is_empty())
    {
        Some(j) => format!("judged by `{j}`"),
        None => "scored by each case's check, else a judge of another vendor".to_owned(),
    };
    format!("Tune `{artefact}` on {set} with `{target}`, {scoring}.")
}

/// Setup rows this kind adds to the draft panel (§7 Data and UI): `cases`, `target`, `scoring`.
pub const SETUP_ROWS: &[SetupRow] = &[
    SetupRow {
        row: "cases",
        label: "Cases",
        leaves: &[
            "cases.source",
            "cases.path",
            "cases.skill_hint",
            "cases.artefact",
            "cases.render",
            "cases.split",
            "cases.repeats",
            "cases.max_cases",
            "cases.train_shown",
        ],
    },
    SetupRow {
        row: "target",
        label: "Target model",
        leaves: &["models.target"],
    },
    SetupRow {
        row: "scoring",
        label: "Scoring",
        leaves: &["models.judge"],
    },
];

/// One-line value of a kind row for the draft panel.
pub fn setup_value(spec: &StudySpec, row: &str) -> String {
    match row {
        "cases" => {
            let Some(cs) = spec.cases.as_ref() else {
                return String::new();
            };
            let source = match cs.source {
                CaseSource::File => format!("file {}", cs.path),
                CaseSource::Dreaming => format!("dreaming {}", cs.skill_hint),
            };
            let split = if cs.split.is_empty() {
                "40/40/20"
            } else {
                cs.split.as_str()
            };
            let mut parts = vec![
                source.trim().to_owned(),
                format!("artefact {}", cs.artefact),
                format!("split {split}"),
            ];
            if cs.repeats > 1 {
                parts.push(format!("repeats {}", cs.repeats));
            }
            if cs.max_cases > 0 {
                parts.push(format!("max {}", cs.max_cases));
            }
            parts.join(" · ")
        }
        "target" => spec.models.target.clone().unwrap_or_default(),
        "scoring" => match spec.models.judge.as_deref() {
            Some(j) if !j.trim().is_empty() => format!("judge {j}"),
            _ if spec.cases.is_some() => "checks, else a judge of another vendor".to_owned(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// Set one kind leaf from the panel's text.
pub fn setup_set(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    let v = v.trim();
    let opt = |v: &str| (!v.is_empty()).then(|| v.to_owned());
    match leaf {
        "models.target" => spec.models.target = opt(v),
        "models.judge" => spec.models.judge = opt(v),
        _ => {
            let Some(field) = leaf.strip_prefix("cases.") else {
                return Err(format!("unknown field `{leaf}`"));
            };
            let cs = spec.cases.get_or_insert_with(CaseSetSpec::default);
            match field {
                "source" => {
                    cs.source = match v {
                        "file" => CaseSource::File,
                        "dreaming" => CaseSource::Dreaming,
                        _ => return Err(format!("cases.source: `file` or `dreaming`, got `{v}`")),
                    }
                }
                "path" => cs.path = v.to_owned(),
                "skill_hint" => cs.skill_hint = v.to_owned(),
                "artefact" => cs.artefact = v.to_owned(),
                "render" => cs.render = v.to_owned(),
                "split" => {
                    let probe = CaseSetSpec {
                        split: v.to_owned(),
                        ..CaseSetSpec::default()
                    };
                    shares(&probe)?;
                    cs.split = v.to_owned();
                }
                "repeats" => {
                    cs.repeats = v
                        .parse()
                        .map_err(|_| format!("cases.repeats: a small count, got `{v}`"))?
                }
                "max_cases" => {
                    cs.max_cases = v
                        .parse()
                        .map_err(|_| format!("cases.max_cases: a count, got `{v}`"))?
                }
                "train_shown" => {
                    cs.train_shown = v
                        .parse()
                        .map_err(|_| format!("cases.train_shown: a count, got `{v}`"))?
                }
                _ => return Err(format!("unknown field `{leaf}`")),
            }
            spec.axes.scoring = crate::types::Scoring::Cases;
            spec.axes.artefact = crate::types::Artefact::File;
            if spec.bench.command.trim().is_empty() {
                spec.bench.command = BENCH_COMMAND.to_owned();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;
    use crate::actor::Backend;
    use crate::config::{AgentDefaults, AutoresearchConfig};
    use crate::types::{
        Axes, Begin, BenchSpec, BudgetSpec, Create, Decision, Direction, ExperimentState,
        HoldoutSpec, Log, LogArgs, MetricSpec, ModelsSpec, ReviewVerdict, Run, RunArgs, Scoring,
        StartWhen, StudyState,
    };
    use crate::{Studies, binding};
    use runtime::SessionId;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/toy-prompt");

    fn fixture_cases() -> Vec<Case> {
        parse_cases(&std::fs::read_to_string(Path::new(FIXTURE).join("cases.jsonl")).unwrap())
            .unwrap()
    }

    #[test]
    fn shares_parse_and_refuse_bad_splits() {
        let cs = |s: &str| CaseSetSpec {
            split: s.into(),
            ..CaseSetSpec::default()
        };
        assert_eq!(shares(&cs("")).unwrap(), Shares::default());
        assert_eq!(
            shares(&cs("50/30/20")).unwrap(),
            Shares {
                train: 50,
                val: 30,
                holdout: 20
            }
        );
        assert!(shares(&cs("50/50")).is_err());
        assert!(shares(&cs("50/30/30")).is_err());
        assert!(shares(&cs("100/0/0")).is_err());
    }

    #[test]
    fn the_fixture_parses_and_splits_as_the_readme_says() {
        let cases = fixture_cases();
        assert_eq!(cases.len(), 90);
        assert!(cases.iter().all(|c| c.check.is_some() && c.ideal.is_some()));
        let set: Vec<(Case, Split)> = cases
            .into_iter()
            .map(|c| {
                let s = evalcore::split("toy-prompt", &c.id, Shares::default(), false);
                (c, s)
            })
            .collect();
        let c = counts(&set);
        assert_eq!((c.train, c.val, c.holdout), (35, 38, 17));
        assert!(
            parse_cases("{\"id\":\"a\",\"input\":\"x\"}\n{\"id\":\"a\",\"input\":\"y\"}").is_err()
        );
        assert!(parse_cases("{\"id\":\"\",\"input\":\"x\"}").is_err());
    }

    #[test]
    fn leak_finds_eight_word_runs_only() {
        let protected = [
            "Normalise this date to ISO 8601: 22 March 2012",
            "2012-03-22",
        ];
        // Seven shared words: fine. The whole input: refused. Case and punctuation do not matter.
        assert_eq!(
            leak(
                "normalise this date to iso 8601 22",
                protected.iter().copied()
            ),
            None
        );
        let hit = leak(
            "Rules: NORMALISE this date, to ISO-8601: 22 march 2012 always.",
            protected.iter().copied(),
        );
        assert_eq!(
            hit.as_deref(),
            Some("normalise this date to iso 8601 22 march")
        );
        assert_eq!(leak("anything", std::iter::empty()), None);
    }

    #[test]
    fn strip_rows_group_scores_by_experiment_and_split() {
        let j = |n: u32, id: &str, split: &str, sha: &str, score: f64| JudgmentRow {
            n,
            case_id: id.into(),
            split: split.into(),
            a_sha: sha.into(),
            score: Some(score),
            ..JudgmentRow::default()
        };
        let rows = rows_from(&[
            j(0, "v1", "val", "base", 0.0),
            j(0, "v1", "val", "base", 1.0), // two baseline passes → mean 0.5
            j(1, "v1", "val", "exp:1", 1.0),
            j(1, "v1", "val", "base", 0.0), // an incumbent re-measure at #1: not a strip point
            j(0, "h1", "holdout", "base", 0.0),
            j(2, "h1", "holdout", "keep0002", 1.0), // holdout after a keep: a strip point
            j(1, "t1", "train", "exp:1", 0.0),
        ]);
        let (rows, split) = rows;
        assert_eq!((split.train, split.val, split.holdout), (1, 1, 1));
        let v1 = rows.iter().find(|r| r.id == "v1").unwrap();
        assert_eq!(v1.values, BTreeMap::from([(0, 0.5), (1, 1.0)]));
        let h1 = rows.iter().find(|r| r.id == "h1").unwrap();
        assert_eq!(h1.values, BTreeMap::from([(0, 0.0), (2, 1.0)]));
        let view = KindView::Cases {
            rows: rows.clone(),
            split,
        };
        let at1 = rows_of(view.clone(), Some(1)).unwrap();
        assert_eq!(at1.len(), 2);
        assert!(
            at1.iter()
                .all(|r| r.values.len() == 1 && r.values.contains_key(&1))
        );
        assert_eq!(rows_of(view, None).unwrap().len(), 3);
        assert!(rows_of(KindView::Scalar, None).is_err());
    }

    #[test]
    fn setup_rows_round_trip_and_the_card_line_counts_the_fixture() {
        let mut spec = StudySpec::default();
        assert!(setup_set(&mut spec, "cases.source", "jsonl").is_err());
        setup_set(&mut spec, "cases.path", "cases.jsonl").unwrap();
        setup_set(&mut spec, "cases.artefact", "prompt.md").unwrap();
        setup_set(&mut spec, "cases.split", "50/30/20").unwrap();
        assert!(setup_set(&mut spec, "cases.split", "50/50").is_err());
        setup_set(&mut spec, "cases.repeats", "2").unwrap();
        setup_set(&mut spec, "models.target", "openai/gpt-5-nano").unwrap();
        assert_eq!(spec.axes.scoring, Scoring::Cases);
        assert_eq!(spec.kind(), crate::types::KIND_PROMPT);
        assert_eq!(
            setup_value(&spec, "cases"),
            "file cases.jsonl · artefact prompt.md · split 50/30/20 · repeats 2"
        );
        assert_eq!(setup_value(&spec, "target"), "openai/gpt-5-nano");
        assert_eq!(
            setup_value(&spec, "scoring"),
            "checks, else a judge of another vendor"
        );
        assert!(setup_set(&mut spec, "cases.nope", "x").is_err());
        // The card line reads the file when the repo has it.
        spec.repo = FIXTURE.to_owned();
        spec.slug = "toy-prompt".into();
        setup_set(&mut spec, "cases.split", "").unwrap_or_default();
        spec.cases.as_mut().unwrap().split.clear();
        let line = card_line(&spec);
        assert_eq!(
            line,
            "Tune `prompt.md` on 90 cases (35 train · 38 val · 17 holdout) with `openai/gpt-5-nano`, scored by each case's check, else a judge of another vendor."
        );
        // A spec with every field default serialises without a `cases` key; the block round-trips.
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["cases"]["path"], "cases.jsonl");
        assert!(v["cases"].get("split").is_none());
        let back: StudySpec = serde_json::from_value(v).unwrap();
        assert_eq!(back.cases, spec.cases);
    }

    // ───────────────────────────── the end-to-end fake-target study ─────────────────────────────

    /// The rule words a prompt must contain for the fake target to get a class right; the
    /// other four classes are "easy" (right on any prompt), so the one-line prompt scores 4/9.
    const RULES: [(&str, &str); 5] = [
        ("us_slash", "MM/DD"),
        ("eu_dots", "DD.MM"),
        ("ymd_slash", "YYYY/M/D"),
        ("short_year", "20YY"),
        ("compact", "YYYYMMDD"),
    ];

    /// A target that answers right only when the system prompt states the case class's rule
    /// (§7 Guard P), and a `Backend` that routes `side_call` to it; everything else is production
    /// on a real temp git repo. The reviewer is off; the money ledger is unreachable (= 0).
    struct FakeTarget {
        by_input: HashMap<String, (String, String)>, // input → (ideal, tag)
        calls: std::sync::atomic::AtomicU32,
    }

    #[async_trait::async_trait]
    impl Backend for FakeTarget {
        async fn side_call(
            &self,
            _study: &StudyRecord,
            role: runtime::ModelRole,
            system: &str,
            user: String,
            _budget: Duration,
        ) -> Result<runtime::SideReply> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(matches!(role, runtime::ModelRole::Named(_)));
            let (ideal, tag) = self
                .by_input
                .get(&user)
                .unwrap_or_else(|| panic!("unknown case input {user:?}"));
            let rule = RULES.iter().find(|(t, _)| t == tag).map(|(_, r)| *r);
            let right = rule.is_none_or(|r| system.contains(r));
            let text = if right {
                ideal.clone()
            } else {
                // A plausible wrong answer: day and month swapped, or the 19xx century.
                let (y, m, d) = (&ideal[0..4], &ideal[5..7], &ideal[8..10]);
                if m <= d {
                    format!("{y}-{d}-{m}")
                } else {
                    format!("19{}-{m}-{d}", &y[2..])
                }
            };
            Ok(runtime::SideReply {
                text,
                model: "fake/target".into(),
                input_tokens: Some(40),
                output_tokens: Some(4),
                latency: Duration::from_millis(1),
                cost_nanos: Some(2_000_000),
                request_id: None,
            })
        }
        async fn review(
            &self,
            _: &Store,
            _: &StudyRecord,
            _: u32,
            _: &str,
            _: &runtime::Runtime,
        ) -> Result<ReviewVerdict> {
            Ok(ReviewVerdict::Clean {
                reason: "fake".into(),
            })
        }
        async fn code_map(
            &self,
            _: &runtime::Runtime,
            _: &str,
            _: &[String],
            _: u32,
        ) -> Option<String> {
            None
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn copy_fixture(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        for f in ["prompt.md", "cases.jsonl", "gen_cases.py", "README.md"] {
            std::fs::copy(Path::new(FIXTURE).join(f), repo.join(f)).unwrap();
        }
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["add", "-A"]);
        git(
            repo,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "base"],
        );
    }

    fn prompt_spec(repo: &Path) -> StudySpec {
        StudySpec {
            slug: "toy-prompt".into(),
            goal: "normalise every date form".into(),
            repo: repo.display().to_string(),
            base: "HEAD".into(),
            axes: Axes {
                scoring: Scoring::Cases,
                artefact: crate::types::Artefact::File,
                ..Axes::default()
            },
            metric: MetricSpec {
                name: "score".into(),
                unit: String::new(),
                direction: Direction::Higher,
                min_effect: "0.05".into(),
            },
            cases: Some(CaseSetSpec {
                path: "cases.jsonl".into(),
                artefact: "prompt.md".into(),
                ..CaseSetSpec::default()
            }),
            bench: BenchSpec {
                // W2-e: `create` still requires a bench command; `cases` names the engine runner.
                command: BENCH_COMMAND.into(),
                repeats: 2,
                ..BenchSpec::default()
            },
            holdout: Some(HoldoutSpec {
                command: String::new(),
                every: 1,
                repeats: Some(1),
            }),
            evaluator_paths: vec!["cases.jsonl".into()],
            scope: vec!["prompt.md".into()],
            off_limits: vec!["cases.jsonl".into(), "gen_cases.py".into()],
            models: ModelsSpec {
                review: Some("off".into()),
                target: Some("fake/target".into()),
                ..ModelsSpec::default()
            },
            budget: BudgetSpec {
                experiments: 20,
                ..BudgetSpec::default()
            },
            ..StudySpec::default()
        }
    }

    async fn wait_state(
        store: &Store,
        slug: &str,
        pred: impl Fn(&StudyState) -> bool,
    ) -> StudyRecord {
        for _ in 0..600 {
            let s = store.get_study(slug).await.unwrap().unwrap();
            if pred(&s.state) {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("study `{slug}` never reached the state");
    }

    fn write_prompt(worktree: &Path, text: &str) {
        std::fs::write(worktree.join("prompt.md"), text).unwrap();
    }

    const GOOD_PROMPT: &str = "Convert the date in the message to ISO 8601 and reply with the date only.\n\
        Rules: slashes are MM/DD/YYYY (US); dots are DD.MM.YYYY (European); a leading year is YYYY/M/D;\n\
        a two-digit year is 20YY; eight digits are YYYYMMDD.\n";

    /// 159 §7 P and T on the fixture through the test backend: the baseline scores 4/9 on val,
    /// a prompt stating two rules is kept by the paired gate, a candidate pasting a val case is
    /// refused before any call, a reworded prompt with no new rule is no gain, the full rule
    /// list is kept and the holdout follows val within 0.05; the brief shows failing train
    /// cases and never a val or holdout input or ideal; the view, headline and `judgments`
    /// rows carry every split.
    #[tokio::test]
    async fn prompt_study_end_to_end_on_the_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        copy_fixture(&repo);
        let cases = fixture_cases();
        let fake = Arc::new(FakeTarget {
            by_input: cases
                .iter()
                .map(|c| {
                    (
                        c.input.clone(),
                        (c.ideal.clone().unwrap(), c.tags[0].clone()),
                    )
                })
                .collect(),
            calls: Default::default(),
        });
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let root = dir.path().join("autoresearch");
        let store = Store::open(&root).await.unwrap();
        let mut h = Studies {
            data_dir: dir.path().to_path_buf(),
            root: root.clone(),
            cfg: AutoresearchConfig::default(),
            agent: AgentDefaults {
                model: "fake/exec".into(),
                others: vec!["other/judge".into()],
                communication: false,
            },
            rt,
            store: store.clone(),
            backend: fake.clone(),
        };
        let spec = prompt_spec(&repo);
        crate::tools::validate_spec(&spec).unwrap();
        let session = SessionId::new();
        crate::actor::create(
            &mut h,
            Create {
                spec,
                origin: SessionId::new(),
                start: StartWhen::Now,
            },
        )
        .await
        .unwrap();
        let study = wait_state(&store, "toy-prompt", |s| {
            matches!(s, StudyState::Running | StudyState::Failed { .. })
        })
        .await;
        assert_eq!(study.state, StudyState::Running, "{:?}", study.stop_reason);
        let base_mean = study.incumbent_mean.unwrap();
        // 19 of the 38 val cases are in the four classes the one-line prompt gets right.
        assert!(
            (base_mean - 0.5).abs() < 1e-9,
            "baseline val mean {base_mean}"
        );
        // The case file is in custody (hashed from the worktree as an evaluator path; the
        // snapshot is that copy) and the worktree copy matches it; the base holdout ran.
        let snap = root.join("toy-prompt/evaluator/0").join(SNAPSHOT);
        assert!(snap.is_file());
        let seg = store.get_segment("toy-prompt", 0).await.unwrap().unwrap();
        assert_eq!(
            seg.custody.get("cases.jsonl").map(String::as_str),
            Some(crate::custody::sha256_file(&snap).unwrap().as_str())
        );
        let base_calls = fake.calls.load(std::sync::atomic::Ordering::SeqCst);
        // 2 baseline passes × (38 val + 16 train) + 1 holdout pass × 17 = 125 target calls, no judge calls.
        assert_eq!(base_calls, 2 * 54 + 17);
        let worktree = root.join("toy-prompt/worktree");
        binding::on_fired(&mut h, "autoresearch-toy-prompt", session)
            .await
            .unwrap();

        let val_inputs: Vec<&str> = cases
            .iter()
            .filter(|c| {
                evalcore::split("toy-prompt", &c.id, Shares::default(), false) != Split::Train
            })
            .map(|c| c.input.as_str())
            .collect();
        let val_ideals: Vec<&str> = cases
            .iter()
            .filter(|c| {
                evalcore::split("toy-prompt", &c.id, Shares::default(), false) != Split::Train
            })
            .map(|c| c.ideal.as_deref().unwrap())
            .collect();
        let never_leaks = |text: &str| {
            for i in &val_inputs {
                assert!(
                    !text.contains(i),
                    "a val/holdout input reached the brief: {i}"
                );
            }
            // Ideals are bare ISO dates; a train case's ideal could coincide, so check ids instead.
            for c in cases.iter().filter(|c| {
                evalcore::split("toy-prompt", &c.id, Shares::default(), false) != Split::Train
            }) {
                assert!(
                    !text.contains(&c.id),
                    "a val/holdout case id reached the brief: {}",
                    c.id
                );
            }
        };

        // #1: two rules added → kept.
        let brief = crate::actor::begin(
            &mut h,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        assert!(brief.text.contains("Failing train cases"), "{}", brief.text);
        assert!(brief.text.contains("answer:") && brief.text.contains("why:"));
        never_leaks(&brief.text);
        write_prompt(
            &worktree,
            "Convert the date in the message to ISO 8601. Slashes are MM/DD/YYYY (US); dots are DD.MM.YYYY (European).\n",
        );
        let rep = crate::actor::run(
            &mut h,
            Run {
                session,
                slot: None,
                args: RunArgs {
                    idea: "state the slash and dot rules".into(),
                    category: "rules".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(rep.verdict, Verdict::Improved, "{}", rep.line);
        assert!(rep.line.contains("samples") || !rep.line.is_empty());
        let lr = crate::actor::log(
            &mut h,
            Log {
                session,
                slot: None,
                args: LogArgs {
                    description: "d".into(),
                    learned: "l".into(),
                    decision: Decision::Auto,
                    ..LogArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(lr.applied, ExperimentState::Kept);
        let e1 = store
            .get_experiment("toy-prompt", 1)
            .await
            .unwrap()
            .unwrap();
        let g = e1.gate.expect("a paired decision");
        assert_eq!(g.rule, GateRule::Paired);
        assert!(
            g.n >= 10 && g.stat.is_some() && g.threshold.is_some(),
            "{g:?}"
        );
        assert_eq!(g.objectives[0].call, Call::Better);
        // The holdout after the keep ran on the holdout split and is near val.
        let s1 = store.get_study("toy-prompt").await.unwrap().unwrap();
        let hold: Vec<f64> = store
            .list_samples("toy-prompt", Some(1))
            .await
            .unwrap()
            .into_iter()
            .filter(|s| s.role == Some(SampleRole::Holdout))
            .filter_map(|s| s.value)
            .collect();
        assert_eq!(hold.len(), 1, "one holdout sample after the keep");
        assert!(
            (hold[0] - s1.incumbent_mean.unwrap()).abs() <= 0.12,
            "holdout {} vs val {}",
            hold[0],
            s1.incumbent_mean.unwrap()
        );

        // #2: a val case pasted into the prompt → refused before any call.
        let brief = crate::actor::begin(
            &mut h,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        never_leaks(&brief.text);
        let calls_before = fake.calls.load(std::sync::atomic::Ordering::SeqCst);
        let pasted = format!(
            "{}\nFor example: {} → {}\n",
            GOOD_PROMPT, val_inputs[0], val_ideals[0]
        );
        write_prompt(&worktree, &pasted);
        let rep = crate::actor::run(
            &mut h,
            Run {
                session,
                slot: None,
                args: RunArgs {
                    idea: "add an example".into(),
                    category: "examples".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(rep.verdict, Verdict::Crash, "{}", rep.line);
        let tail = rep.tail.clone().unwrap_or_default();
        assert!(
            tail.contains("refused") && tail.contains("val or holdout"),
            "{tail}"
        );
        assert_eq!(
            fake.calls.load(std::sync::atomic::Ordering::SeqCst),
            calls_before,
            "no call was made"
        );
        let lr = crate::actor::log(
            &mut h,
            Log {
                session,
                slot: None,
                args: LogArgs {
                    description: "d".into(),
                    learned: "l".into(),
                    decision: Decision::Auto,
                    ..LogArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(lr.applied, ExperimentState::Crashed);

        // #3: a rewrite that drops the rule words → worse (t ≤ −c_n), discarded.
        crate::actor::begin(
            &mut h,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        write_prompt(
            &worktree,
            "You normalise dates. Reply with the ISO 8601 form only. Slashes are the US order month, day, year; dots are the European order day, month, year.\n",
        );
        let rep = crate::actor::run(
            &mut h,
            Run {
                session,
                slot: None,
                args: RunArgs {
                    idea: "say the rules in words".into(),
                    category: "wording".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(rep.verdict, Verdict::Worse, "{}", rep.line);
        assert_eq!(
            store
                .get_experiment("toy-prompt", 3)
                .await
                .unwrap()
                .unwrap()
                .gate
                .unwrap()
                .objectives[0]
                .call,
            Call::Worse
        );
        let lr = crate::actor::log(
            &mut h,
            Log {
                session,
                slot: None,
                args: LogArgs {
                    description: "d".into(),
                    learned: "l".into(),
                    decision: Decision::Auto,
                    ..LogArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(lr.applied, ExperimentState::Discarded);

        // #4: every rule → kept; val 1.0, holdout 1.0.
        crate::actor::begin(
            &mut h,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        write_prompt(&worktree, GOOD_PROMPT);
        let rep = crate::actor::run(
            &mut h,
            Run {
                session,
                slot: None,
                args: RunArgs {
                    idea: "state every rule".into(),
                    category: "rules".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(rep.verdict, Verdict::Improved, "{}", rep.line);
        let lr = crate::actor::log(
            &mut h,
            Log {
                session,
                slot: None,
                args: LogArgs {
                    description: "d".into(),
                    learned: "l".into(),
                    decision: Decision::Auto,
                    ..LogArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(lr.applied, ExperimentState::Kept);
        let s4 = store.get_study("toy-prompt").await.unwrap().unwrap();
        assert!(
            (s4.incumbent_mean.unwrap() - 1.0).abs() < 1e-9,
            "{:?}",
            s4.incumbent_mean
        );
        let hold: Vec<f64> = store
            .list_samples("toy-prompt", Some(4))
            .await
            .unwrap()
            .into_iter()
            .filter(|s| s.role == Some(SampleRole::Holdout))
            .filter_map(|s| s.value)
            .collect();
        assert_eq!(hold, vec![1.0]);
        assert!(
            store
                .list_events("toy-prompt", Some("overfit"), 5)
                .await
                .unwrap()
                .is_empty()
        );

        // The brief after the keeps: every shown train case passed; nothing from val/holdout.
        let section = brief_section(&store, &s4).await.unwrap();
        assert!(
            section.contains("Every train case shown passed"),
            "{section}"
        );
        never_leaks(&section);

        // The view: every scored case (38 val, 17 holdout, the 16 train cases replayed per
        // sample), the base column and the kept experiments' columns.
        let KindView::Cases { rows, split } = view(&store, &s4).await.unwrap() else {
            panic!("not a cases view")
        };
        assert_eq!((split.train, split.val, split.holdout), (16, 38, 17));
        assert_eq!(rows.len(), 71);
        let val_rows: Vec<&CaseRow> = rows.iter().filter(|r| r.split == "val").collect();
        assert!(
            val_rows
                .iter()
                .all(|r| r.values.contains_key(&0) && r.values.contains_key(&4))
        );
        assert!(val_rows.iter().all(|r| r.values[&4] == 1.0));
        assert!(
            rows.iter()
                .filter(|r| r.split == "holdout")
                .all(|r| r.values.contains_key(&0) && r.values.contains_key(&4))
        );
        // Only 16 train cases are replayed per sample.
        assert_eq!(
            rows.iter()
                .filter(|r| r.split == "train" && r.values.contains_key(&0))
                .count(),
            16
        );
        assert_eq!(
            rows_of(
                KindView::Cases {
                    rows: rows.clone(),
                    split
                },
                Some(2)
            )
            .unwrap()
            .len(),
            0,
            "the refused candidate scored nothing"
        );

        // The headline.
        let hl = headline(&store, &s4).await.unwrap().unwrap();
        assert_eq!(
            hl.text,
            format!("val score {base_mean:.2} → 1.00 on 38 cases")
        );
        assert!(
            hl.holdout
                .as_deref()
                .is_some_and(|h| h.ends_with("→ 1.000")),
            "{:?}",
            hl.holdout
        );

        // The judgments: one row per case per tree per sample, cost attached, the base's rows at n = 0.
        let j = store.judgments_list("toy-prompt", Some(0)).await.unwrap();
        assert_eq!(j.len(), 2 * 54 + 17);
        assert!(
            j.iter()
                .all(|r| r.model == "check" && r.b_sha.is_none() && r.cost_nanos > 0)
        );
        // The samples carry case values with tokens and cost; no bench spend (the money is on the session channel).
        let s = store.list_samples("toy-prompt", Some(4)).await.unwrap();
        let cand = s
            .iter()
            .find(|s| s.role == Some(SampleRole::Candidate))
            .unwrap();
        assert_eq!(cand.case_values.len(), 54);
        assert!(
            cand.case_values
                .values()
                .all(|v| v["tokens_in"] > 0.0 && v["cost"] > 0.0)
        );
        assert_eq!(cand.spend_nanos, 0);
        assert_eq!(cand.cases.len(), 16, "`cases` holds the train ids only");
    }
}
