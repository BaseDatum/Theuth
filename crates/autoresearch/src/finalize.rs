//! W2 RATCHET — finalize (design doc 140 §11): interleaved base/final
//! confirmation (+ holdout), changesets by union-find over touched paths
//! each cherry-picked alone onto the base and measured, the report
//! document's numbers and tables, the knowledge record.
//! Each step is an `events` row (`kind = "finalize"`, `payload.step`) so a
//! restart resumes the step. The Deliver message and the attention row are
//! W3's (actor), as is the finalize turn whose prose wraps these tables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kameo::actor::ActorRef;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::config::AutoresearchConfig;
use crate::error::{Error, Result};
use crate::git;
use crate::runner::{self, SampleRequest};
use crate::store::{ExperimentRecord, Store, StudyRecord};
use crate::types::{
    Changeset, Direction, Effect, ExperimentState, Idea, MetricSpec, ReviewVerdict, SampleRole,
    Scoring, StopReason, StudySpec,
};

/// `events.kind` of every finalize step.
pub const EVENT: &str = "finalize";

/// The headline of a finished study (140 §11.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Headline {
    /// `time_ms 52.1 → 41.2 (−20.9 %, CI ±0.8 %)`.
    pub text: String,
    /// Base mean after confirmation.
    pub base_mean: f64,
    /// Final mean after confirmation.
    pub final_mean: f64,
    /// Δ %.
    pub delta_pct: f64,
    /// Holdout line, when configured.
    pub holdout: Option<String>,
    /// Pooled σ of the confirmation samples (0 when fewer than two per tree).
    #[serde(default)]
    pub sigma: f64,
    /// Valid confirmation samples per tree `(base, final)`.
    #[serde(default)]
    pub n: (u32, u32),
}

/// Everything the report needs that is a number or a table (140 §11.3); the
/// model's prose goes around [`FinalReport::tables`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinalReport {
    /// The study.
    pub slug: String,
    /// The goal, verbatim.
    pub goal: String,
    /// Metric name and unit.
    pub metric: String,
    /// Base sha.
    pub base_sha: String,
    /// Final incumbent sha.
    pub final_sha: String,
    /// Why it stopped.
    pub reason: StopReason,
    /// §11.1.
    pub headline: Headline,
    /// §11.2, measured.
    pub changesets: Vec<Changeset>,
    /// Reverted-by-review, flagged, deviations with justifications, unreviewed keeps, holdout gap.
    pub needs_eyes: Vec<String>,
    /// Discarded / crashed / checks-failed / refused / rejected, grouped by category.
    pub dead_ends: BTreeMap<String, Vec<String>>,
    /// Experiments closed.
    pub experiments: u32,
    /// Keeps that stood.
    pub kept: u32,
    /// Model spend.
    pub cost_usd: f64,
    /// Wall hours since Start, paused time excluded.
    pub hours: f64,
    /// 159 W5: kept experiments that tried a backlog idea, counted by the idea's `source`.
    #[serde(default)]
    pub keeps_by_source: BTreeMap<String, u32>,
}

/// 159 W5 §10 T: keeps by idea source. A kept experiment counts under the source of the idea
/// row whose `tried_in` names it (`mark_tried`'s match at `experiment_run`), the first by
/// priority; keeps that tried no backlog idea are not counted.
fn keeps_by_source(kept: &[ExperimentRecord], ideas: &[Idea]) -> BTreeMap<String, u32> {
    let mut by = BTreeMap::new();
    for e in kept {
        if let Some(i) = ideas.iter().find(|i| i.tried_in.contains(&e.n)) {
            let word = serde_json::to_value(i.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            *by.entry(word).or_insert(0) += 1;
        }
    }
    by
}

impl FinalReport {
    /// The markdown the plugin owns: headline, changesets table, needs-your-eyes,
    /// dead ends, cost, how to land. Inserted after the finalize turn; the model
    /// cannot change a number here.
    pub fn tables(&self) -> String {
        let mut s = format!(
            "# Autoresearch: {}\n\n**Goal.** {}\n\n**Headline.** {}",
            self.slug, self.goal, self.headline.text
        );
        if let Some(h) = &self.headline.holdout {
            s.push_str(&format!(" · holdout {h}"));
        }
        s.push_str(&format!(
            "\n\nBase `{}` → final `{}` · {} experiments, {} kept · ${:.2} · {:.1} h · stopped: {}\n",
            short(&self.base_sha),
            short(&self.final_sha),
            self.experiments,
            self.kept,
            self.cost_usd,
            self.hours,
            reason_text(&self.reason)
        ));
        if !self.keeps_by_source.is_empty() {
            let by = self
                .keeps_by_source
                .iter()
                .map(|(s, n)| format!("{s} {n}"))
                .collect::<Vec<_>>()
                .join(" · ");
            s.push_str(&format!("\nKeeps by idea source: {by}\n"));
        }
        s.push_str("\n## Changesets\n\n");
        if self.changesets.is_empty() {
            s.push_str("No kept changes.\n");
        } else {
            s.push_str("| # | changeset | Δ alone | Δ together | files | experiments | review | branch |\n|---|---|---|---|---|---|---|---|\n");
            for c in &self.changesets {
                let alone = match (c.delta_alone_pct, c.only_with.is_empty()) {
                    (Some(d), true) => pct(d),
                    (Some(d), false) => format!(
                        "{} (only with {})",
                        pct(d),
                        c.only_with
                            .iter()
                            .map(|k| format!("#{k}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    (None, _) => "not measured".into(),
                };
                s.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | `{}` |\n",
                    c.k,
                    c.name,
                    alone,
                    c.delta_together_pct.map(pct).unwrap_or_else(|| "—".into()),
                    c.files.join(", "),
                    c.experiments
                        .iter()
                        .map(|n| format!("#{n}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    c.review
                        .as_ref()
                        .map(review_text)
                        .unwrap_or_else(|| "—".into()),
                    c.branch
                ));
            }
        }
        s.push_str("\n## Needs your eyes\n\n");
        if self.needs_eyes.is_empty() {
            s.push_str("Nothing flagged.\n");
        } else {
            for l in &self.needs_eyes {
                s.push_str(&format!("- {l}\n"));
            }
        }
        s.push_str("\n## Dead ends\n\n");
        if self.dead_ends.is_empty() {
            s.push_str("None.\n");
        }
        for (cat, lines) in &self.dead_ends {
            s.push_str(&format!("### {cat}\n\n"));
            for l in lines {
                s.push_str(&format!("- {l}\n"));
            }
            s.push('\n');
        }
        s.push_str("## How to land\n\n");
        for c in &self.changesets {
            s.push_str(&format!("- `git merge --no-ff {}`\n", c.branch));
        }
        s.push_str("\nOr from the Study view: Open PR / Merge into my branch per changeset; Discard study removes the worktree and the branches. Nothing was pushed.\n");
        s
    }
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

fn pct(d: f64) -> String {
    format!("{d:+.1} %")
}

fn reason_text(r: &StopReason) -> String {
    match r {
        StopReason::User => "stopped by the user".into(),
        StopReason::UserNow => "stopped now by the user".into(),
        StopReason::Experiments => "experiment budget reached".into(),
        StopReason::Budget => "money budget reached".into(),
        StopReason::Hours => "time budget reached".into(),
        StopReason::Deadline => "deadline reached".into(),
        StopReason::Plateau => "plateau".into(),
        StopReason::Overfit => "holdout says overfit".into(),
        StopReason::Failed { reason } => format!("failed: {reason}"),
    }
}

fn review_text(r: &ReviewVerdict) -> String {
    match r {
        ReviewVerdict::Clean { .. } => "clean".into(),
        ReviewVerdict::Reverted { reason } => format!("reverted: {reason}"),
        ReviewVerdict::Flagged { reason } => format!("flagged: {reason}"),
        ReviewVerdict::Error { reason } => format!("unreviewed: {reason}"),
    }
}

fn review_rank(r: &ReviewVerdict) -> u8 {
    match r {
        ReviewVerdict::Reverted { .. } => 3,
        ReviewVerdict::Flagged { .. } => 2,
        ReviewVerdict::Error { .. } => 1,
        ReviewVerdict::Clean { .. } => 0,
    }
}

/// Group kept commits into changesets that share no files (140 §11.2), unmeasured.
/// Input `(n, sha, files)`; output ordered by first experiment, `k` 1-based,
/// `branch` empty (the caller knows the slug).
pub fn group(kept: &[(u32, String, Vec<String>)]) -> Vec<Changeset> {
    // ponytail: O(n·files) scan with a path → group map; a study has tens of keeps.
    let mut parent: Vec<usize> = (0..kept.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut c = i;
        while p[c] != r {
            let next = p[c];
            p[c] = r;
            c = next;
        }
        r
    }
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, (_, _, files)) in kept.iter().enumerate() {
        for f in files {
            match owner.get(f.as_str()) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
                None => {
                    owner.insert(f, i);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Changeset> = BTreeMap::new();
    for (i, (n, _, files)) in kept.iter().enumerate() {
        let root = find(&mut parent, i);
        let c = groups.entry(root).or_default();
        c.experiments.push(*n);
        for f in files {
            if !c.files.contains(f) {
                c.files.push(f.clone());
            }
        }
    }
    let mut out: Vec<Changeset> = groups.into_values().collect();
    for c in &mut out {
        c.experiments.sort_unstable();
        c.files.sort();
    }
    out.sort_by_key(|c| c.experiments[0]);
    renumber(&mut out);
    out
}

/// Re-index `k` and derive each name from its files.
fn renumber(cs: &mut [Changeset]) {
    for (i, c) in cs.iter_mut().enumerate() {
        c.k = i as u32 + 1;
        c.name = name_of(&c.files);
    }
}

/// A short `[a-z0-9-]` name: the common directory's last segment, else the first file's stem.
fn name_of(files: &[String]) -> String {
    fn dir(f: &str) -> Vec<&str> {
        f.rsplit_once('/')
            .map_or("", |(d, _)| d)
            .split('/')
            .filter(|s| !s.is_empty())
            .collect()
    }
    let mut common: Vec<&str> = files.first().map(|f| dir(f)).unwrap_or_default();
    for f in files.iter().skip(1) {
        let d = dir(f);
        let n = common.iter().zip(&d).take_while(|(a, b)| a == b).count();
        common.truncate(n);
    }
    let raw = match common.last() {
        Some(dir) if files.len() > 1 => (*dir).to_owned(),
        _ => files
            .first()
            .map(|f| {
                f.rsplit('/')
                    .next()
                    .unwrap_or(f)
                    .split('.')
                    .next()
                    .unwrap_or("change")
                    .to_owned()
            })
            .unwrap_or_else(|| "change".into()),
    };
    let s: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "change".into()
    } else {
        s.chars().take(24).collect()
    }
}

/// Merge changeset `from` into `into` (a cherry-pick dependency), then renumber.
fn merge(cs: &mut Vec<Changeset>, from: usize, into: usize) {
    let moved = cs.remove(from);
    let into = if from < into { into - 1 } else { into };
    let c = &mut cs[into];
    c.experiments.extend(moved.experiments);
    c.experiments.sort_unstable();
    c.experiments.dedup();
    for f in moved.files {
        if !c.files.contains(&f) {
            c.files.push(f);
        }
    }
    c.files.sort();
    cs.sort_by_key(|c| c.experiments[0]);
    renumber(cs);
}

/// The changeset holding the newest kept commit before `n` (what a conflicting
/// cherry-pick most plausibly needs). A changeset whose file is, or lies
/// under or above, one of `conflicts` wins first; among those (or when none
/// touch the conflict) the newest keep before `n`, else the oldest after it.
fn partner(cs: &[Changeset], k: usize, n: u32, conflicts: &[String]) -> Option<usize> {
    let related = |c: &Changeset| {
        c.files.iter().any(|f| {
            conflicts.iter().filter(|p| !p.is_empty()).any(|p| {
                f == p || f.starts_with(&format!("{p}/")) || p.starts_with(&format!("{f}/"))
            })
        })
    };
    let pick = |pool: &dyn Fn(&Changeset) -> bool| -> Option<usize> {
        let before = cs
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != k && pool(c))
            .filter_map(|(i, c)| {
                c.experiments
                    .iter()
                    .filter(|&&m| m < n)
                    .max()
                    .map(|&m| (m, i))
            })
            .max();
        if let Some((_, i)) = before {
            return Some(i);
        }
        cs.iter()
            .enumerate()
            .filter(|(i, c)| *i != k && pool(c))
            .filter_map(|(i, c)| {
                c.experiments
                    .iter()
                    .filter(|&&m| m > n)
                    .min()
                    .map(|&m| (m, i))
            })
            .min()
            .map(|(_, i)| i)
    };
    pick(&related).or_else(|| pick(&|_| true))
}

/// Direction-normalised Δ: positive is better.
fn better(direction: Direction, base: f64, other: f64) -> f64 {
    match direction {
        Direction::Lower => base - other,
        Direction::Higher => other - base,
    }
}

/// 159 R-W4-1 / R-SEAMS-26: the re-measure decides. One-sided t at the realised dof (3.75 at
/// 4 dof, 2.33 only past ~30), so a population champion — the best of many lucky draws — is
/// claimed in ≤ 1 % of null studies; a deterministic metric (σ = 0) confirms on the sign alone.
pub(crate) fn confirmed(
    direction: Direction,
    base_mean: f64,
    final_mean: f64,
    sigma: f64,
    nb: u32,
    nf: u32,
) -> bool {
    let gain = better(direction, base_mean, final_mean);
    if sigma > 0.0 && nb + nf > 2 {
        let se = sigma * (1.0 / f64::from(nb) + 1.0 / f64::from(nf)).sqrt();
        gain / se >= evalcore::gate::t_quantile(f64::from(nb + nf - 2), 0.99)
    } else {
        sigma == 0.0 && gain > 0.0
    }
}

/// The scalar confirmation's headline line, ending in its word (` · confirmed` / ` · unconfirmed`).
#[allow(clippy::too_many_arguments)]
fn scalar_text(
    metric: &MetricSpec,
    base_mean: f64,
    final_mean: f64,
    delta_pct: f64,
    sigma: f64,
    nb: u32,
    nf: u32,
) -> String {
    let ci = if nb > 0 && nf > 0 && base_mean != 0.0 {
        1.96 * sigma * (1.0 / f64::from(nb) + 1.0 / f64::from(nf)).sqrt() / base_mean.abs() * 100.0
    } else {
        f64::NAN
    };
    let word = if confirmed(metric.direction, base_mean, final_mean, sigma, nb, nf) {
        "confirmed"
    } else {
        "unconfirmed"
    };
    let unit = &metric.unit;
    if ci.is_finite() {
        format!(
            "{} {:.4} → {:.4} {unit} ({:+.1} %, CI ±{:.1} %) · {word}",
            metric.name, base_mean, final_mean, delta_pct, ci
        )
    } else {
        format!(
            "{} {:.4} → {:.4} {unit} ({:+.1} %) · {word}",
            metric.name, base_mean, final_mean, delta_pct
        )
    }
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Pooled within-tree standard deviation over trees with ≥ 2 samples (140 §5.2 step 1).
fn pooled_sigma(trees: &[&[f64]]) -> f64 {
    let (mut ss, mut df) = (0.0, 0usize);
    for t in trees {
        if let Some(m) = mean(t).filter(|_| t.len() > 1) {
            ss += t.iter().map(|x| (x - m).powi(2)).sum::<f64>();
            df += t.len() - 1;
        }
    }
    if df == 0 {
        0.0
    } else {
        (ss / df as f64).sqrt()
    }
}

/// 159 §18.3 `kind_headline`: one arm per kind, the wave's module builds its headline
/// (`front of 3 · time_ms −21 % · …`, `val score 0.52 → 0.81 on 48 cases`, …); `None` for
/// `one metric`, and for a module that defers to the scalar confirmation.
async fn kind_headline(store: &Store, study: &StudyRecord) -> Result<Option<Headline>> {
    use crate::types::{
        KIND_JUDGED, KIND_META, KIND_PARETO, KIND_POPULATION, KIND_PROMPT, KIND_WORLD,
    };
    match study.spec.kind() {
        KIND_PARETO => crate::pareto::headline(store, study).await, // 159:W1
        KIND_PROMPT => crate::cases::headline(store, study).await,  // 159:W2
        KIND_JUDGED => crate::judge::headline(store, study).await,  // 159:W3
        KIND_POPULATION => crate::population::headline(store, study).await, // 159:W4
        KIND_WORLD => crate::world::headline(store, study).await,   // 159:W6
        KIND_META => crate::suite::headline(store, study).await,    // 159:W7
        _ => Ok(None),
    }
}

/// What `run` carries between steps.
struct Ctx<'a> {
    store: &'a Store,
    study: &'a StudyRecord,
    spec: &'a StudySpec,
    cfg: AutoresearchConfig,
    repo: PathBuf,
    worktree: PathBuf,
    study_dir: PathBuf,
    base: String,
    final_sha: String,
    cancel: CancellationToken,
}

impl Ctx<'_> {
    fn finalize_dir(&self) -> PathBuf {
        self.study_dir.join("finalize")
    }

    async fn step_done(&self, step: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .store
            .list_events(&self.study.slug, Some(EVENT), 200)
            .await?
            .into_iter()
            .find(|e| e.payload.get("step").and_then(|s| s.as_str()) == Some(step))
            .map(|e| e.payload))
    }

    async fn record_step(&self, step: &str, mut payload: serde_json::Value) -> Result<()> {
        payload["step"] = serde_json::Value::String(step.to_owned());
        self.store
            .append_event(&self.study.slug, EVENT, &payload)
            .await
            .map(drop)
    }

    /// `repeats` samples of `spec.bench` (or `command` when given) on `tree` in `cwd`; valid values only.
    async fn samples(
        &self,
        spec: &StudySpec,
        cwd: &Path,
        tree: &str,
        role: SampleRole,
        repeats: u8,
    ) -> Result<Vec<f64>> {
        let mut out = Vec::new();
        for k in 0..u32::from(repeats) {
            out.extend(self.one(spec, cwd, tree, role, k).await?);
        }
        Ok(out)
    }

    async fn one(
        &self,
        spec: &StudySpec,
        cwd: &Path,
        tree: &str,
        role: SampleRole,
        k: u32,
    ) -> Result<Option<f64>> {
        let evaluator = self
            .study_dir
            .join("evaluator")
            .join(self.study.segment.to_string());
        let runs = self.study_dir.join("runs").join("final");
        std::fs::create_dir_all(&runs)?;
        let req = SampleRequest {
            slug: &self.study.slug,
            n: 0,
            k,
            tree_sha: tree,
            role,
            worktree: cwd,
            evaluator: &evaluator,
            runs_dir: &runs,
        };
        let s = runner::sample(spec, &self.cfg.bench, req, &self.cancel).await?;
        self.store.insert_sample(&self.study.slug, &s).await?;
        Ok(s.value.filter(|_| !s.noisy && !s.lost))
    }

    /// Step 1 (140 §11.1): A B A B … on base and final, holdout likewise.
    async fn confirm(&self) -> Result<Headline> {
        if let Some(p) = self.step_done("confirm").await? {
            return Ok(serde_json::from_value(p["headline"].clone())?);
        }
        // 159 §18.3 `kind_headline`: a kind with its own headline (front, cases, pairs, cells,
        // world, suite) records it instead of the scalar confirmation; `None` = the 140 path.
        if let Some(headline) = kind_headline(self.store, self.study).await? {
            self.record_step(
                "confirm",
                serde_json::json!({ "headline": headline, "notes": Vec::<String>::new() }),
            )
            .await?;
            return Ok(headline);
        }
        // R-56: the study turn may still be editing after `experiment_log`;
        // the "final" tree is the kept incumbent, nothing newer.
        git::reset_clean(&self.worktree, &self.final_sha)?;
        let base_wt = self.finalize_dir().join("base");
        git::worktree_remove(&self.repo, &base_wt).ok();
        git::worktree_add_detached(&self.repo, &base_wt, &self.base)?;
        let (mut b, mut f) = (Vec::new(), Vec::new());
        for k in 0..u32::from(self.spec.bench.repeats) {
            b.extend(
                self.one(self.spec, &base_wt, &self.base, SampleRole::Final, k)
                    .await?,
            );
            f.extend(
                self.one(
                    self.spec,
                    &self.worktree,
                    &self.final_sha,
                    SampleRole::Final,
                    k,
                )
                .await?,
            );
        }
        let mut notes = Vec::new();
        let base_mean = match mean(&b) {
            Some(m) => m,
            None => {
                notes.push(
                    "confirmation: every base sample crashed; the ledger's baseline is used"
                        .to_owned(),
                );
                self.store
                    .base_mean(&self.study.slug)
                    .await?
                    .unwrap_or(f64::NAN)
            }
        };
        let final_mean = match mean(&f) {
            Some(m) => m,
            None => {
                notes.push(
                    "confirmation: every final sample crashed; the ledger's incumbent mean is used"
                        .to_owned(),
                );
                self.study.incumbent_mean.unwrap_or(f64::NAN)
            }
        };
        let sigma = pooled_sigma(&[&b, &f]);
        let delta_pct = if base_mean != 0.0 {
            (final_mean - base_mean) / base_mean.abs() * 100.0
        } else {
            0.0
        };
        let (nb, nf) = (b.len() as u32, f.len() as u32);
        let text = scalar_text(
            &self.spec.metric,
            base_mean,
            final_mean,
            delta_pct,
            sigma,
            nb,
            nf,
        );
        let mut holdout = None;
        if let Some(h) = &self.spec.holdout {
            let mut hs = self.spec.clone();
            hs.bench.command = h.command.clone();
            let reps = h.repeats.unwrap_or(self.spec.bench.repeats);
            let (mut hb, mut hf) = (Vec::new(), Vec::new());
            for k in 0..u32::from(reps) {
                hb.extend(
                    self.one(&hs, &base_wt, &self.base, SampleRole::Holdout, k)
                        .await?,
                );
                hf.extend(
                    self.one(&hs, &self.worktree, &self.final_sha, SampleRole::Holdout, k)
                        .await?,
                );
            }
            holdout = Some(match (mean(&hb), mean(&hf)) {
                (Some(a), Some(z)) if a != 0.0 => {
                    format!("{a:.4} → {z:.4} ({:+.1} %)", (z - a) / a.abs() * 100.0)
                }
                (Some(a), Some(z)) => format!("{a:.4} → {z:.4}"),
                _ => "holdout crashed".to_owned(),
            });
        }
        git::worktree_remove(&self.repo, &base_wt)?;
        let headline = Headline {
            text,
            base_mean,
            final_mean,
            delta_pct,
            holdout,
            sigma,
            n: (nb, nf),
        };
        self.record_step(
            "confirm",
            serde_json::json!({ "headline": headline, "notes": notes }),
        )
        .await?;
        Ok(headline)
    }

    /// Step 2 (140 §11.2): group, resolve cherry-pick dependencies, measure each alone, branch.
    async fn changesets(
        &self,
        kept: &[ExperimentRecord],
        headline: &Headline,
    ) -> Result<(Vec<Changeset>, Vec<String>)> {
        if let Some(p) = self.step_done("changesets").await? {
            return Ok((
                serde_json::from_value(p["changesets"].clone())?,
                serde_json::from_value(p["notes"].clone()).unwrap_or_default(),
            ));
        }
        let input: Vec<(u32, String, Vec<String>)> = kept
            .iter()
            .filter_map(|e| {
                let sha = e.commit_sha.clone()?;
                let files = git::files_of(&self.worktree, &sha).ok()?;
                Some((e.n, sha, files))
            })
            .collect();
        let dir = self.finalize_dir();
        let (mut cs, mut notes) = resolve(&self.repo, &self.base, &dir, &input)?;
        let sigma = if headline.sigma > 0.0 {
            headline.sigma
        } else {
            self.study.sigma.unwrap_or(0.0)
        };
        let min_effect =
            Effect::parse(&self.spec.metric.min_effect).unwrap_or(Effect::Absolute(0.0));
        let by_n: BTreeMap<u32, &ExperimentRecord> = kept.iter().map(|e| (e.n, e)).collect();
        let mut not_alone = Vec::new();
        // Both changeset columns read like the headline and the ledger's per-experiment
        // `delta_pct`: the raw relative change, negative for a lower-is-better win (Revision 2).
        for c in &mut cs {
            c.delta_together_pct = Some(
                c.experiments
                    .iter()
                    .filter_map(|n| by_n.get(n).and_then(|e| e.delta_pct))
                    .sum::<f64>(),
            );
            c.review = c
                .experiments
                .iter()
                .filter_map(|n| by_n.get(n).and_then(|e| e.review.clone()))
                .max_by_key(review_rank);
            // 159 R-W2-4 / R-SEAMS-3: engine-side scoring (cases, pairwise, world, suite) has no
            // bench command to run alone; the kind's gate already measured each keep against its
            // incumbent.
            if self.spec.axes.scoring != Scoring::Bench {
                notes.push(format!(
                    "changeset {}: measured by the kind's gate only",
                    c.k
                ));
                continue;
            }
            let wt = scratch(&dir, c.k);
            if !wt.exists() {
                continue;
            }
            let tree = git::head(&wt)?;
            let xs = self
                .samples(
                    self.spec,
                    &wt,
                    &tree,
                    SampleRole::Final,
                    self.spec.bench.repeats,
                )
                .await?;
            if let Some(m) = mean(&xs).filter(|_| headline.base_mean.is_finite()) {
                let base = headline.base_mean;
                c.delta_alone_pct = Some(if base != 0.0 {
                    (m - base) / base.abs() * 100.0
                } else {
                    0.0
                });
                let delta = better(self.spec.metric.direction, base, m);
                let nb = f64::from(headline.n.0.max(1));
                let t = (self.cfg.gate.z * sigma * (1.0 / xs.len() as f64 + 1.0 / nb).sqrt())
                    .max(min_effect.against(base));
                if delta <= t {
                    not_alone.push(c.k);
                }
            } else {
                notes.push(format!(
                    "changeset {}: every sample crashed alone on the base",
                    c.k
                ));
            }
        }
        let all: Vec<u32> = cs.iter().map(|c| c.k).collect();
        for c in &mut cs {
            if not_alone.contains(&c.k) {
                // ponytail: "only helps together with" names every other changeset; pairwise measurement would say which.
                c.only_with = all.iter().copied().filter(|k| *k != c.k).collect();
            }
        }
        branch_all(&self.repo, &self.study.slug, &dir, &mut cs)?;
        self.record_step(
            "changesets",
            serde_json::json!({ "changesets": cs, "notes": notes }),
        )
        .await?;
        Ok((cs, notes))
    }
}

/// `<finalize dir>/cs-<k>`.
fn scratch(dir: &Path, k: u32) -> PathBuf {
    dir.join(format!("cs-{k}"))
}

/// Step 2a: [`group`], then make every changeset cherry-pick alone onto
/// `base` in `<dir>/cs-<k>`; a conflict merges the changeset into its
/// [`partner`] and starts over. Scratch worktrees of the resolved changesets
/// are left in place for measurement ([`branch_all`] removes them). Returns
/// the changesets and the notes for the report.
pub fn resolve(
    repo: &Path,
    base: &str,
    dir: &Path,
    kept: &[(u32, String, Vec<String>)],
) -> Result<(Vec<Changeset>, Vec<String>)> {
    let shas: BTreeMap<u32, &str> = kept.iter().map(|(n, sha, _)| (*n, sha.as_str())).collect();
    let mut cs = group(kept);
    let mut notes = Vec::new();
    let mut rounds = 0;
    'resolve: loop {
        rounds += 1;
        for c in cs.clone() {
            let wt = scratch(dir, c.k);
            git::worktree_remove(repo, &wt).ok();
            git::worktree_add_detached(repo, &wt, base)?;
            for &n in &c.experiments {
                let conflicts = git::cherry_pick_conflicts(&wt, shas[&n])?;
                if conflicts.is_empty() {
                    continue;
                }
                git::worktree_remove(repo, &wt)?;
                let k = c.k as usize - 1;
                match partner(&cs, k, n, &conflicts) {
                    Some(into) if rounds <= kept.len() + 1 => {
                        notes.push(format!(
                            "changeset {} (#{}) only applies together with changeset {}; merged",
                            c.k, n, cs[into].k
                        ));
                        merge(&mut cs, k, into);
                        for c in &cs {
                            git::worktree_remove(repo, &scratch(dir, c.k)).ok();
                        }
                        continue 'resolve;
                    }
                    _ => {
                        notes.push(format!("changeset {} (#{}) does not apply onto the base alone and has no partner; not measured", c.k, n));
                        break;
                    }
                }
            }
        }
        return Ok((cs, notes));
    }
}

/// Step 2c: branch `autoresearch/<slug>+<k>-<name>` at each resolved scratch
/// worktree's HEAD, then remove the scratch worktrees. Local branches only.
pub fn branch_all(repo: &Path, slug: &str, dir: &Path, cs: &mut [Changeset]) -> Result<()> {
    for c in cs.iter_mut() {
        c.branch = git::changeset_branch(slug, c.k, &c.name);
        let wt = scratch(dir, c.k);
        if !wt.exists() {
            continue;
        }
        let tree = git::head(&wt)?;
        git::branch_at(repo, &c.branch, &tree)?;
        git::worktree_remove(repo, &wt)?;
    }
    Ok(())
}

/// The finalize changesets recorded for `slug` (empty before step 2 ran).
pub async fn changesets(store: &Store, slug: &str) -> Result<Vec<Changeset>> {
    let evs = store.list_events(slug, Some(EVENT), 200).await?;
    Ok(evs
        .into_iter()
        .find(|e| e.payload.get("step").and_then(|s| s.as_str()) == Some("changesets"))
        .and_then(|e| serde_json::from_value(e.payload["changesets"].clone()).ok())
        .unwrap_or_default())
}

/// The recorded [`FinalReport`] of a finished study, if step 3 ran.
pub async fn report(store: &Store, slug: &str) -> Result<Option<FinalReport>> {
    let evs = store.list_events(slug, Some(EVENT), 200).await?;
    Ok(evs
        .into_iter()
        .find(|e| e.payload.get("step").and_then(|s| s.as_str()) == Some("report"))
        .and_then(|e| serde_json::from_value(e.payload["report"].clone()).ok()))
}

/// Build the report value from the ledger (pure over rows; no git, no bench).
pub fn assemble(
    study: &StudyRecord,
    reason: StopReason,
    headline: Headline,
    changesets: Vec<Changeset>,
    rows: &[ExperimentRecord],
    notes: Vec<String>,
) -> FinalReport {
    let mut needs_eyes = notes;
    let mut dead_ends: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut experiments, mut kept, mut cost) = (0u32, 0u32, 0u64);
    for e in rows {
        if e.state.is_open() {
            continue;
        }
        experiments += 1;
        cost += e.cost_nanos;
        let d = e.delta_pct.map(pct).unwrap_or_default();
        match e.state {
            ExperimentState::Kept => {
                kept += 1;
                match &e.review {
                    Some(ReviewVerdict::Flagged { reason }) => {
                        needs_eyes.push(format!("#{} flagged by review: {reason}", e.n))
                    }
                    Some(ReviewVerdict::Error { reason }) => {
                        needs_eyes.push(format!("#{} kept unreviewed: {reason}", e.n))
                    }
                    _ => {}
                }
                if !e.deviations.is_empty() {
                    needs_eyes.push(format!(
                        "#{} edited outside scope ({}) — {}",
                        e.n,
                        e.deviations.join(", "),
                        e.justification
                            .as_deref()
                            .filter(|j| !j.trim().is_empty())
                            .unwrap_or("no justification given")
                    ));
                }
            }
            ExperimentState::RejectedByReview => {
                let why = match &e.review {
                    Some(ReviewVerdict::Reverted { reason }) => reason.clone(),
                    _ => "suspicious".into(),
                };
                needs_eyes.push(format!(
                    "#{} reverted by review: {why} (Restore re-measures it)",
                    e.n
                ));
                dead_ends.entry(cat(&e.category)).or_default().push(format!(
                    "#{} rejected by review {d} — {}",
                    e.n,
                    idea(e)
                ));
            }
            ExperimentState::Discarded
            | ExperimentState::Crashed
            | ExperimentState::ChecksFailed
            | ExperimentState::Refused => {
                let v = e
                    .verdict
                    .as_ref()
                    .map(|v| v.tag())
                    .unwrap_or(match e.state {
                        ExperimentState::Crashed => "crash",
                        ExperimentState::ChecksFailed => "checks_failed",
                        ExperimentState::Refused => "refused",
                        _ => "discarded",
                    });
                dead_ends
                    .entry(cat(&e.category))
                    .or_default()
                    .push(format!("#{} {v} {d} — {}", e.n, idea(e)).replace("  ", " "));
            }
            _ => {}
        }
    }
    if let Some(h) = &headline.holdout
        && h.contains("(+") != (headline.delta_pct > 0.0)
        && !h.contains("crashed")
    {
        needs_eyes.push(format!("holdout moved against the metric: {h}"));
    }
    let hours = study
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|start| {
            let end = study
                .finished_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);
            ((end - start.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0) as f64
                - study.paused_ms as f64)
                .max(0.0)
                / 3.6e6
        })
        .unwrap_or(0.0);
    FinalReport {
        slug: study.slug.clone(),
        goal: study.spec.goal.clone(),
        metric: format!("{} ({})", study.spec.metric.name, study.spec.metric.unit),
        base_sha: study.base_sha.clone().unwrap_or_default(),
        final_sha: study.incumbent_sha.clone().unwrap_or_default(),
        reason,
        headline,
        changesets,
        needs_eyes,
        dead_ends,
        experiments,
        kept,
        cost_usd: cost as f64 / 1e9,
        hours,
        keeps_by_source: BTreeMap::new(),
    }
}

fn cat(c: &str) -> String {
    if c.trim().is_empty() {
        "uncategorised".into()
    } else {
        c.trim().to_owned()
    }
}

fn idea(e: &ExperimentRecord) -> String {
    let t = if e.idea.trim().is_empty() {
        &e.description
    } else {
        &e.idea
    };
    let t = t.trim();
    if t.chars().count() > 120 {
        format!("{}…", t.chars().take(119).collect::<String>())
    } else {
        t.to_owned()
    }
}

/// [`run_with`] on the startup snapshot of the `autoresearch:` section —
/// the `Backend::finalize` seam's shape; the actor should pass its live
/// `env.cfg` / `env.root` through [`run_with`] instead (R-106).
pub async fn run(
    store: &Store,
    study: &StudyRecord,
    reason: StopReason,
    rt: &runtime::Runtime,
) -> Result<(Headline, Option<String>)> {
    let cfg: AutoresearchConfig = rt.section("autoresearch").unwrap_or_default();
    let root = cfg.root(&rt.data_dir);
    run_with(store, study, reason, cfg, &root, rt.shutdown.clone()).await
}

/// Run every step of §11 for `study`, stopped for `reason`, under the live
/// `cfg` and `root`; returns the headline and the report document id.
/// Idempotent per step. `cancel` ends a sample in flight (engine shutdown).
pub async fn run_with(
    store: &Store,
    study: &StudyRecord,
    reason: StopReason,
    cfg: AutoresearchConfig,
    root: &Path,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(Headline, Option<String>)> {
    let (Some(base), Some(final_sha), Some(worktree)) =
        (&study.base_sha, &study.incumbent_sha, &study.worktree)
    else {
        return Err(Error::refused(format!(
            "study `{}` has no worktree to finalize",
            study.slug
        )));
    };
    let cx = Ctx {
        store,
        study,
        spec: &study.spec,
        repo: PathBuf::from(&study.repo),
        worktree: PathBuf::from(worktree),
        study_dir: crate::study_dir(root, &study.slug),
        base: base.clone(),
        final_sha: final_sha.clone(),
        cancel,
        cfg,
    };
    let headline = cx.confirm().await?;
    let rows = store.list_experiments(&study.slug, None, u32::MAX).await?;
    let kept: Vec<ExperimentRecord> = {
        let mut k: Vec<ExperimentRecord> = rows
            .iter()
            .filter(|e| e.state == ExperimentState::Kept && e.commit_sha.is_some())
            .cloned()
            .collect();
        k.sort_by_key(|e| e.n);
        k
    };
    let (changesets, notes) = cx.changesets(&kept, &headline).await?;
    // R-98: the document id, or `None` when no documents actor took the report
    // (the tables still land in `<study_dir>/report.md`); never a file path.
    let doc = match cx.step_done("report").await? {
        Some(p) => p["doc"].as_str().map(str::to_owned),
        None => {
            let mut report = assemble(
                study,
                reason.clone(),
                headline.clone(),
                changesets,
                &rows,
                notes,
            );
            report.keeps_by_source =
                keeps_by_source(&kept, &store.list_ideas(&study.slug, None).await?);
            let text = report.tables();
            std::fs::create_dir_all(&cx.study_dir)?;
            let path = cx.study_dir.join("report.md");
            std::fs::write(&path, &text)?;
            let doc = create_document(&study.slug, &text).await;
            cx.record_step(
                "report",
                serde_json::json!({ "doc": doc, "report": report }),
            )
            .await?;
            doc
        }
    };
    if cx.step_done("record").await?.is_none() {
        match crate::record::write_record(store, study, &headline.text).await {
            Ok(id) => {
                cx.record_step("record", serde_json::json!({ "id": id }))
                    .await?
            }
            Err(e) => {
                tracing::warn!(
                    "autoresearch {}: knowledge record not written: {e}",
                    study.slug
                );
                cx.record_step("record", serde_json::json!({ "error": e.to_string() }))
                    .await?;
            }
        }
    }
    let _ = std::fs::remove_dir_all(cx.finalize_dir());
    Ok((headline, doc))
}

/// The report document (`Autoresearch: <slug>`) through the documents actor; `None` without it.
async fn create_document(slug: &str, markdown: &str) -> Option<String> {
    let docs = ActorRef::<document::DocumentActor>::lookup(document::DOCUMENT_ACTOR).ok()??;
    match docs
        .ask(document::CreateNote {
            title: format!("Autoresearch: {slug}"),
            markdown: markdown.to_owned(),
            by: document::Principal::Helper,
            home: document::Homing::Here,
        })
        .await
    {
        Ok(summary) => Some(summary.id.as_str().to_owned()),
        Err(e) => {
            tracing::warn!("autoresearch {slug}: report document not created: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::types::Verdict;

    fn sh(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn commit(cwd: &Path, msg: &str) -> String {
        sh(cwd, &["add", "-A"]);
        sh(
            cwd,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@x",
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
        sh(cwd, &["rev-parse", "HEAD"])
    }

    fn kept(n: u32, files: &[&str]) -> (u32, String, Vec<String>) {
        (
            n,
            format!("sha{n}"),
            files.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    /// Disjoint keeps stay apart; a shared file chains keeps into one changeset.
    #[test]
    fn group_disjoint_and_overlapping() {
        let cs = group(&[
            kept(2, &["src/a.rs"]),
            kept(5, &["src/b.rs"]),
            kept(7, &["src/a.rs", "src/c.rs"]),
            kept(9, &["docs/x.md"]),
            kept(11, &["src/c.rs"]),
        ]);
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].experiments, vec![2, 7, 11]);
        assert_eq!(cs[0].files, vec!["src/a.rs", "src/c.rs"]);
        assert_eq!((cs[0].k, cs[0].name.as_str()), (1, "src"));
        assert_eq!(
            (cs[1].experiments.clone(), cs[1].name.as_str()),
            (vec![5], "b")
        );
        assert_eq!(
            (cs[2].k, cs[2].experiments.clone(), cs[2].name.as_str()),
            (3, vec![9], "x")
        );
        assert!(group(&[]).is_empty());
        assert_eq!(
            name_of(&["lib/liquid/parse.rb".into(), "lib/liquid/render.rb".into()]),
            "liquid"
        );
        assert_eq!(name_of(&["Weird Name!.RS".into()]), "weird-name");
    }

    /// Two disjoint-by-path keeps that only apply together (a file replaced by
    /// a directory) are merged; the independent one stays alone; branches are
    /// local, scratch worktrees gone, nothing pushed.
    #[test]
    fn resolve_merges_conflicting_changesets_and_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        sh(tmp.path(), &["init", "-q", "-b", "main", "repo"]);
        std::fs::write(repo.join("d"), "file\n").unwrap();
        std::fs::write(repo.join("other.txt"), "o\n").unwrap();
        let base = commit(&repo, "base");
        let remote = tmp.path().join("origin.git");
        sh(tmp.path(), &["init", "-q", "--bare", "origin.git"]);
        sh(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        sh(&repo, &["push", "-q", "origin", "main"]);
        let wt = tmp.path().join("data/s/worktree");
        let spec = StudySpec {
            slug: "s".into(),
            ..Default::default()
        };
        git::worktree_add(&repo, &spec, &wt, &base).unwrap();
        std::fs::remove_file(wt.join("d")).unwrap();
        let a = commit(&wt, "#1 delete d");
        std::fs::write(wt.join("other.txt"), "o2\n").unwrap();
        let c = commit(&wt, "#2 other");
        std::fs::create_dir_all(wt.join("d")).unwrap();
        std::fs::write(wt.join("d/x"), "x\n").unwrap();
        let b = commit(&wt, "#3 add d/x");
        let input = vec![
            (1, a.clone(), git::files_of(&wt, &a).unwrap()),
            (2, c.clone(), git::files_of(&wt, &c).unwrap()),
            (3, b.clone(), git::files_of(&wt, &b).unwrap()),
        ];
        assert_eq!(group(&input).len(), 3, "disjoint by path");
        let dir = tmp.path().join("data/s/finalize");
        let (mut cs, notes) = resolve(&repo, &base, &dir, &input).unwrap();
        assert_eq!(cs.len(), 2, "{cs:?}");
        assert_eq!(cs[0].experiments, vec![1, 3]);
        assert_eq!(cs[0].files, vec!["d", "d/x"]);
        assert_eq!(cs[1].experiments, vec![2]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("only applies together"), "{notes:?}");
        assert!(dir.join("cs-1").exists() && dir.join("cs-2").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("cs-1/d/x")).unwrap(),
            "x\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("cs-2/other.txt")).unwrap(),
            "o2\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("cs-2/d")).unwrap(),
            "file\n",
            "changeset 2 alone on the base"
        );
        branch_all(&repo, "s", &dir, &mut cs).unwrap();
        assert_eq!(cs[0].branch, "autoresearch/s+1-d");
        assert_eq!(cs[1].branch, "autoresearch/s+2-other");
        assert!(!dir.join("cs-1").exists() && !dir.join("cs-2").exists());
        assert_eq!(
            sh(
                &repo,
                &[
                    "rev-list",
                    "--count",
                    &format!("{base}..autoresearch/s+1-d")
                ]
            ),
            "2"
        );
        assert_eq!(
            sh(
                &repo,
                &[
                    "rev-list",
                    "--count",
                    &format!("{base}..autoresearch/s+2-other")
                ]
            ),
            "1"
        );
        assert_eq!(
            sh(&repo, &["worktree", "list"]).lines().count(),
            2,
            "repo + study worktree"
        );
        assert_eq!(
            sh(&remote, &["for-each-ref"]).lines().count(),
            1,
            "nothing pushed"
        );
        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");
    }

    #[test]
    fn partner_prefers_the_newest_earlier_keep() {
        let cs = group(&[
            kept(1, &["a"]),
            kept(4, &["b"]),
            kept(6, &["c"]),
            kept(9, &["d"]),
        ]);
        let none: Vec<String> = vec![String::new()];
        assert_eq!(partner(&cs, 2, 6, &none), Some(1));
        assert_eq!(
            partner(&cs, 0, 1, &none),
            Some(1),
            "nothing earlier: the oldest later one"
        );
        assert_eq!(partner(&[cs[0].clone()], 0, 1, &none), None);
        assert_eq!(
            partner(&cs, 2, 6, &["d".into()]),
            Some(3),
            "the changeset touching the conflicting path wins"
        );
        assert_eq!(
            partner(&cs, 2, 6, &["a/sub".into()]),
            Some(0),
            "a file above the conflict counts"
        );
        let mut cs2 = cs.clone();
        merge(&mut cs2, 2, 1);
        assert_eq!(cs2.len(), 3);
        assert_eq!(cs2[1].experiments, vec![4, 6]);
        assert_eq!(cs2.iter().map(|c| c.k).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn pooled_sigma_is_within_tree() {
        assert_eq!(
            pooled_sigma(&[&[1.0, 1.0, 1.0], &[5.0, 5.0]]),
            0.0,
            "different trees do not inflate sigma"
        );
        let s = pooled_sigma(&[&[1.0, 3.0], &[10.0, 12.0]]);
        assert!((s - 2f64.sqrt()).abs() < 1e-9, "{s}");
        assert_eq!(pooled_sigma(&[&[1.0]]), 0.0);
        assert_eq!(better(Direction::Lower, 10.0, 8.0), 2.0);
        assert_eq!(better(Direction::Higher, 10.0, 8.0), -2.0);
    }

    /// The report counts closed experiments, keeps, dead ends by category, and
    /// lists what needs eyes; the tables name branches and the only-with note.
    #[test]
    fn assemble_and_tables() {
        let mut study = StudyRecord::new(
            StudySpec {
                slug: "s".into(),
                goal: "faster".into(),
                ..Default::default()
            },
            None,
        );
        study.base_sha = Some("1234567890".into());
        study.incumbent_sha = Some("abcdefabcdef".into());
        study.started_at = Some("2026-10-08T00:00:00Z".into());
        study.finished_at = Some("2026-10-08T03:00:00Z".into());
        study.paused_ms = 3_600_000;
        let mut rows = Vec::new();
        let mut e = ExperimentRecord::open("s", 1, 0, "x");
        e.state = ExperimentState::Kept;
        e.delta_pct = Some(-3.0);
        e.cost_nanos = 500_000_000;
        e.deviations = vec!["README.md".into()];
        e.review = Some(ReviewVerdict::Clean {
            reason: "fine".into(),
        });
        rows.push(e);
        let mut e = ExperimentRecord::open("s", 2, 0, "x");
        e.state = ExperimentState::Discarded;
        e.verdict = Some(Verdict::NoGain);
        e.category = "caching".into();
        e.idea = "memoise".into();
        e.delta_pct = Some(-0.2);
        e.cost_nanos = 500_000_000;
        rows.push(e);
        let mut e = ExperimentRecord::open("s", 3, 0, "x");
        e.state = ExperimentState::RejectedByReview;
        e.review = Some(ReviewVerdict::Reverted {
            reason: "skips work".into(),
        });
        e.category = "caching".into();
        rows.push(e);
        rows.push(ExperimentRecord::open("s", 4, 0, "x"));
        let headline = Headline {
            text: "t 10 → 9 (−10.0 %)".into(),
            base_mean: 10.0,
            final_mean: 9.0,
            delta_pct: -10.0,
            holdout: Some("5 → 6 (+20.0 %)".into()),
            sigma: 0.1,
            n: (3, 3),
        };
        let cs = vec![Changeset {
            k: 1,
            name: "lib".into(),
            branch: "autoresearch/s+1-lib".into(),
            files: vec!["src/lib.rs".into()],
            experiments: vec![1],
            delta_alone_pct: Some(-0.1),
            delta_together_pct: Some(-3.0),
            only_with: vec![2],
            review: None,
        }];
        let r = assemble(
            &study,
            StopReason::Plateau,
            headline,
            cs,
            &rows,
            vec!["note".into()],
        );
        assert_eq!((r.experiments, r.kept), (3, 1));
        assert!((r.cost_usd - 1.0).abs() < 1e-9);
        assert!((r.hours - 2.0).abs() < 1e-9, "{}", r.hours);
        assert_eq!(r.dead_ends["caching"].len(), 2);
        assert!(
            r.needs_eyes
                .iter()
                .any(|l| l.contains("#3 reverted by review: skips work")),
            "{:?}",
            r.needs_eyes
        );
        assert!(
            r.needs_eyes
                .iter()
                .any(|l| l.contains("#1 edited outside scope (README.md) — no justification")),
            "{:?}",
            r.needs_eyes
        );
        assert!(
            r.needs_eyes
                .iter()
                .any(|l| l.contains("holdout moved against")),
            "{:?}",
            r.needs_eyes
        );
        let t = r.tables();
        assert!(t.contains("| 1 | lib | -0.1 % (only with #2) | -3.0 % | src/lib.rs | #1 | — | `autoresearch/s+1-lib` |"), "{t}");
        assert!(
            t.contains("### caching") && t.contains("#2 no_gain -0.2 % — memoise"),
            "{t}"
        );
        assert!(
            t.contains("git merge --no-ff autoresearch/s+1-lib") && t.contains("stopped: plateau")
        );
        let back: FinalReport = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    /// 159 W5 §10 T: the summary counts keeps by the source of the idea they tried
    /// (`tried_in`); a keep that tried no backlog idea is not counted; no match, no line.
    #[test]
    fn keeps_are_counted_by_idea_source() {
        use crate::types::IdeaSource;
        let keep = |n: u32| {
            let mut e = ExperimentRecord::open("s", n, 0, "x");
            e.state = ExperimentState::Kept;
            e
        };
        let idea = |source: IdeaSource, tried_in: &[u32]| Idea {
            slug: "s".into(),
            text: "an idea".into(),
            source,
            tried_in: tried_in.to_vec(),
            ..Idea::default()
        };
        let kept = [keep(1), keep(2), keep(3), keep(4), keep(9)];
        let ideas = [
            idea(IdeaSource::Literature, &[1, 2]),
            idea(IdeaSource::Model, &[3]),
            idea(IdeaSource::User, &[4, 7]),
            idea(IdeaSource::Model, &[8]),
        ];
        let by = keeps_by_source(&kept, &ideas);
        assert_eq!(
            by.iter().map(|(s, n)| (s.as_str(), *n)).collect::<Vec<_>>(),
            vec![("literature", 2), ("model", 1), ("user", 1)]
        );
        let mut r = assemble(
            &StudyRecord::new(StudySpec::default(), None),
            StopReason::Plateau,
            Headline::default(),
            vec![],
            &kept,
            vec![],
        );
        assert!(!r.tables().contains("Keeps by idea source"));
        r.keeps_by_source = by;
        assert!(
            r.tables()
                .contains("stopped: plateau\n\nKeeps by idea source: literature 2 · model 1 · user 1\n\n## Changesets"),
            "{}",
            r.tables()
        );
        assert!(keeps_by_source(&kept, &[]).is_empty());
    }

    /// 159 R-W4-1 / R-SEAMS-26: the re-measure decides. A null re-measure (3 + 3 fresh
    /// samples, a gain of 1.5 se — past the ±1.96 σ CI's half-width in the headline's own
    /// terms) reads `unconfirmed`; 5 se (past t₄(0.99) = 3.75) reads `confirmed`; a
    /// deterministic metric confirms on the sign alone.
    #[test]
    fn a_null_re_measure_is_unconfirmed_and_a_clear_gain_confirmed() {
        let m = MetricSpec {
            name: "time_ms".into(),
            unit: "ms".into(),
            direction: Direction::Lower,
            min_effect: "0.5%".into(),
        };
        let line = |gain_se: f64, sigma: f64| {
            let se = sigma * (2.0f64 / 3.0).sqrt();
            let f = 100.0 - gain_se * se;
            scalar_text(&m, 100.0, f, f - 100.0, sigma, 3, 3)
        };
        let t = line(1.5, 1.0);
        assert!(t.ends_with(" · unconfirmed"), "{t}");
        let t = line(3.5, 1.0);
        assert!(t.ends_with(" · unconfirmed"), "z 3.5 is not t₄(0.99): {t}");
        let t = line(5.0, 1.0);
        assert!(t.ends_with(" · confirmed"), "{t}");
        assert!(t.contains("CI ±"), "{t}");
        // σ = 0: the sign alone, no CI.
        let t = scalar_text(&m, 100.0, 99.0, -1.0, 0.0, 3, 3);
        assert!(t.ends_with(" %) · confirmed"), "{t}");
        let t = scalar_text(&m, 100.0, 101.0, 1.0, 0.0, 3, 3);
        assert!(t.ends_with(" · unconfirmed"), "{t}");
        // A higher-is-better loss is never confirmed.
        let mut up = m.clone();
        up.direction = Direction::Higher;
        assert!(scalar_text(&up, 100.0, 90.0, -10.0, 0.5, 3, 3).ends_with(" · unconfirmed"));
    }

    /// 159 R-W2-4 / R-SEAMS-3: a study scored by its kind (`scoring: cases`, `bench.command =
    /// "cases"`) reaches `changesets()` without the bench ever running the word `cases` as a
    /// program: the note says the kind's gate measured it, no sample row, no `runs/final` log.
    #[tokio::test]
    async fn changesets_skip_the_bench_for_engine_side_scoring() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        sh(tmp.path(), &["init", "-q", "-b", "main", "repo"]);
        std::fs::write(repo.join("prompt.md"), "v1\n").unwrap();
        let base = commit(&repo, "base");
        let root = tmp.path().join("data");
        let wt = root.join("s/worktree");
        let mut spec = StudySpec {
            slug: "s".into(),
            ..Default::default()
        };
        spec.axes.scoring = Scoring::Cases;
        spec.bench.command = "cases".into();
        git::worktree_add(&repo, &spec, &wt, &base).unwrap();
        std::fs::write(wt.join("prompt.md"), "v2\n").unwrap();
        let keep = commit(&wt, "#1 prompt");
        let store = Store::open(&root).await.unwrap();
        let mut study = StudyRecord::new(spec, None);
        study.repo = repo.display().to_string();
        study.base_sha = Some(base.clone());
        study.incumbent_sha = Some(keep.clone());
        study.worktree = Some(wt.display().to_string());
        let mut e = ExperimentRecord::open("s", 1, 0, "x");
        e.state = ExperimentState::Kept;
        e.commit_sha = Some(keep.clone());
        e.delta_pct = Some(10.0);
        let cx = Ctx {
            store: &store,
            study: &study,
            spec: &study.spec,
            repo: repo.clone(),
            worktree: wt.clone(),
            study_dir: root.join("s"),
            base: base.clone(),
            final_sha: keep.clone(),
            cancel: CancellationToken::new(),
            cfg: AutoresearchConfig::default(),
        };
        let headline = Headline {
            base_mean: 0.5,
            final_mean: 0.8,
            sigma: 0.1,
            n: (3, 3),
            ..Headline::default()
        };
        let (cs, notes) = cx.changesets(&[e], &headline).await.unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].delta_alone_pct, None, "not measured alone");
        assert_eq!(cs[0].delta_together_pct, Some(10.0));
        assert!(
            notes
                .iter()
                .any(|n| n == "changeset 1: measured by the kind's gate only"),
            "{notes:?}"
        );
        assert!(
            !root.join("s/runs/final").exists(),
            "the bench runner never ran `cases`"
        );
        let samples = store.list_samples("s", None).await.unwrap();
        assert!(samples.is_empty(), "{samples:?}");
    }
}
