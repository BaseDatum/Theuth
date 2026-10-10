//! 159 W1 — several objectives: the dominance gate, the front archive and the front chart (159 §6).
//! Owner: W1. The skeleton (C) froze the signatures below and the registration lines that call
//! them (`// 159:W1` in actor.rs, brief.rs, setup.rs, finalize.rs, study_draft.rs, rpc_impl.rs,
//! StudyView.tsx); every body here is W1's. Nothing in this file runs for a 140 study.
//!
//! Shape. The front is never stored as state: [`front`] replays every `Kept` experiment, in
//! order, through the entry rule over the ledger's own samples (the base enters first; a keep
//! evicts the members it dominates; past 8 the least crowded leaves). Every surface — the parent
//! choice, the Study view, the brief, the headline, `autoresearch.archive` — reads that replay,
//! and [`parent_of`] mirrors it into the `archive` table, the `refs/autoresearch/<slug>/front/f<n>`
//! refs and `studies.scores`. Idempotent, so a crash between a keep and the next fire loses nothing.
//!
//! The ratchet's incumbent keeps the engine's meaning (the latest keep: HEAD of the study
//! worktree, what holdouts and the drift repair read); the parent of an experiment is a front
//! member, checked out by [`parent_of`] before the brief. `step` compares the candidate with its
//! parent's samples, not the incumbent's, and only asks for an in-place re-measure when the two
//! coincide (the engine samples the incumbent, not the parent).

use std::collections::BTreeMap;
use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::refs::{self, RefKind};
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{ExperimentRecord, Store, StudyPatch, StudyRecord};
use crate::types::{
    ArchiveRow, Call, Direction, Effect, ExperimentState, FrontPoint, GateCfg, GateDecision,
    GateInput, GateRule, GateStep, KindView, Objective, ObjectiveDelta, ObjectiveRole,
    ObjectivesMode, Sample, SampleRole, Search, StudySpec, Verdict,
};
use crate::{gate, git};

/// Not reached by a 140 study.
const OWNER: &str = "W1";

/// At most this many front members (159 §6 "Cap").
pub const FRONT_CAP: usize = 8;

/// `archive.kind` of a front member.
pub const KIND: &str = "front";

// ───────────────────────────── thresholds (159 §4) ─────────────────────────────

/// Upper tail of the standard normal, `P(Z > z)` (Abramowitz–Stegun 7.1.26, |ε| < 1.5e-7).
fn norm_tail(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erfc = poly * (-x * x).exp();
    if z >= 0.0 {
        erfc / 2.0
    } else {
        1.0 - erfc / 2.0
    }
}

/// Šidák's per-objective threshold for `k` competing objectives: the `z_k` whose one-sided tail
/// is `1 − (1 − tail(z))^(1/k)`, so the family-wise false-keep rate stays at one objective's
/// (159 §4 table: 2.00 → 2.28, 2.43, 2.53, 2.61 for k = 2…5).
pub fn sidak_z(z: f64, k: usize) -> f64 {
    if k <= 1 {
        return z;
    }
    let target = 1.0 - (1.0 - norm_tail(z)).powf(1.0 / k as f64);
    let (mut lo, mut hi) = (z, z + 3.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if norm_tail(mid) > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

// ───────────────────────────── objectives and their values ─────────────────────────────

/// One objective as the gate sees it: competing (`guard: false`) or a guard. Report objectives
/// never enter.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// `METRIC` name.
    pub name: String,
    /// Which way is better.
    pub direction: Direction,
    /// Smallest gain that counts as better.
    pub min_effect: Effect,
    /// Largest significant loss still called the same (`""` = none).
    pub max_regression: Effect,
    /// A guard blocks a keep past `max_regression` and never competes.
    pub guard: bool,
}

/// The competing objectives first, then the guards (159 §5.2 roles); `Effect` strings parsed.
pub fn axes_of(spec: &StudySpec) -> Result<Vec<Axis>> {
    let mut out = Vec::new();
    for role in [ObjectiveRole::Objective, ObjectiveRole::Guard] {
        for o in spec.objectives_in_force().iter().filter(|o| o.role == role) {
            out.push(Axis {
                name: o.name.trim().to_owned(),
                direction: o.direction,
                min_effect: Effect::parse(&o.min_effect).map_err(Error::refused)?,
                max_regression: Effect::parse(&o.max_regression).map_err(Error::refused)?,
                guard: role == ObjectiveRole::Guard,
            });
        }
    }
    Ok(out)
}

fn competing(axes: &[Axis]) -> usize {
    axes.iter().filter(|a| !a.guard).count()
}

/// A benchmark sample the gate may read (140 R-3: never holdout rows, never noisy or lost ones).
fn clean(s: &Sample) -> bool {
    !s.noisy && !s.lost && s.value.is_some() && s.role != Some(SampleRole::Holdout)
}

/// One objective's value in a sample: the headline is `Sample.value`, the rest are `METRIC`
/// secondaries (runner.rs removes the primary from the map).
fn value_of(s: &Sample, name: &str, headline: &str) -> Option<f64> {
    if name == headline {
        s.value
    } else {
        s.secondaries.get(name).copied()
    }
}

/// Per-axis values of one tree's clean samples.
pub fn tree_values(samples: &[Sample], axes: &[Axis], headline: &str) -> Vec<Vec<f64>> {
    axes.iter()
        .map(|a| {
            samples
                .iter()
                .filter(|s| clean(s))
                .filter_map(|s| value_of(s, &a.name, headline))
                .collect()
        })
        .collect()
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// `delta` as a percentage of `reference` (0 when the reference is 0) — gate.rs' convention.
fn pct(delta: f64, reference: f64) -> f64 {
    if reference == 0.0 {
        0.0
    } else {
        100.0 * delta / reference
    }
}

// ───────────────────────────── comparing two trees (159 §6) ─────────────────────────────

/// Candidate against one reference, every axis called at `z_k`.
#[derive(Clone, Debug, PartialEq)]
pub struct Compare {
    /// One per axis, in `axes` order.
    pub deltas: Vec<ObjectiveDelta>,
    /// The strongest competing z (`None` when every σ is 0 or a side has no samples).
    pub z_max: Option<f64>,
}

impl Compare {
    fn calls<'a>(
        &'a self,
        axes: &'a [Axis],
    ) -> impl Iterator<Item = (&'a Axis, &'a ObjectiveDelta)> {
        axes.iter().zip(&self.deltas)
    }
    /// Better on at least one competing objective.
    pub fn better_any(&self, axes: &[Axis]) -> bool {
        self.calls(axes)
            .any(|(a, d)| !a.guard && d.call == Call::Better)
    }
    /// Worse on at least one competing objective.
    pub fn worse_any(&self, axes: &[Axis]) -> bool {
        self.calls(axes)
            .any(|(a, d)| !a.guard && d.call == Call::Worse)
    }
    /// A guard past its limit.
    pub fn guard_failed(&self) -> bool {
        self.deltas.iter().any(|d| d.call == Call::GuardFailed)
    }
    /// 159 §6 "Dominance under noise": better on ≥ 1 competing objective, worse on none.
    pub fn dominates(&self, axes: &[Axis]) -> bool {
        self.better_any(axes) && !self.worse_any(axes)
    }
}

/// Candidate `c` against reference `r` (per-axis sample vectors), with per-axis pooled σ and
/// the corrected threshold `z_k`. Δ is direction-normalised (positive = better);
/// `z = Δ / (σ √(1/n_c + 1/n_r))`; better needs `z ≥ z_k` and `Δ > min_effect`; worse needs
/// `z ≤ −z_k` and a loss past `max_regression`; σ = 0 is deterministic (effects alone). A guard
/// is judged in its own direction against its limit and never called better.
pub fn compare(axes: &[Axis], sigmas: &[f64], c: &[Vec<f64>], r: &[Vec<f64>], z_k: f64) -> Compare {
    let mut z_max: Option<f64> = None;
    let deltas = axes
        .iter()
        .enumerate()
        .map(|(j, a)| {
            let (cv, rv) = (&c[j], &r[j]);
            let role = if a.guard {
                ObjectiveRole::Guard
            } else {
                ObjectiveRole::Objective
            };
            if cv.is_empty() || rv.is_empty() {
                return ObjectiveDelta {
                    name: a.name.clone(),
                    role,
                    candidate: (!cv.is_empty()).then(|| mean(cv)),
                    reference: (!rv.is_empty()).then(|| mean(rv)),
                    ..ObjectiveDelta::default()
                };
            }
            let (xc, xr) = (mean(cv), mean(rv));
            let delta = gate::normalise(a.direction, xc, xr);
            let sigma = sigmas.get(j).copied().unwrap_or(0.0);
            let z = (sigma > 0.0)
                .then(|| delta / (sigma * (1.0 / cv.len() as f64 + 1.0 / rv.len() as f64).sqrt()));
            let significant_gain = z.is_none_or(|z| z >= z_k);
            let significant_loss = z.is_none_or(|z| z <= -z_k);
            let loss_past = -delta > a.max_regression.against(xr) && delta < 0.0;
            let call = if a.guard {
                if significant_loss && loss_past {
                    Call::GuardFailed
                } else {
                    Call::Same
                }
            } else if significant_gain && delta > a.min_effect.against(xr) {
                Call::Better
            } else if significant_loss && loss_past {
                Call::Worse
            } else {
                Call::Same
            };
            if !a.guard
                && let Some(z) = z
            {
                z_max = Some(z_max.map_or(z, |m| m.max(z)));
            }
            ObjectiveDelta {
                name: a.name.clone(),
                role,
                candidate: Some(xc),
                reference: Some(xr),
                delta: Some(delta),
                delta_pct: Some(pct(xc - xr, xr)),
                z,
                call,
            }
        })
        .collect();
    Compare { deltas, z_max }
}

// ───────────────────────────── the sequential dominance step ─────────────────────────────

/// What one dominance decision needs beyond the samples: the study's objectives, the segment's
/// per-axis σ and the gate's thresholds, `z`/`z_fast` already Šidák-corrected.
#[derive(Clone, Debug, PartialEq)]
pub struct Dom<'a> {
    /// Competing objectives, then guards.
    pub axes: &'a [Axis],
    /// Pooled within-tree σ per axis.
    pub sigmas: &'a [f64],
    /// Corrected keep threshold on means.
    pub z_k: f64,
    /// Corrected one-sample keep threshold.
    pub z_fast_k: f64,
    /// One-sample discard below this (uncorrected, as in 140).
    pub z_discard: f64,
    /// Extra candidate samples in the band.
    pub confirm_runs: u8,
    /// Re-measure the parent when its newest sample is older than this many experiments.
    pub remeasure_after: u32,
}

impl<'a> Dom<'a> {
    /// From the study's `GateCfg` (thresholds, confirm and staleness; its direction and
    /// `min_effect` are the headline's and unused here) and the σ pool's degrees of freedom.
    /// `z` and `z_fast` are Šidák-corrected for the objective count, then widened to the
    /// Student-t quantile at `dof` ([`t_of`]): with k per-objective σ estimates on a few
    /// repeated samples, normal thresholds under-correct (the family-wise rate measured 1.05 %
    /// at k = 4 without it). `z_discard` stays as configured (W-p).
    pub fn new(cfg: &GateCfg, axes: &'a [Axis], sigmas: &'a [f64], dof: usize) -> Self {
        let k = competing(axes);
        Self {
            axes,
            sigmas,
            z_k: t_of(sidak_z(cfg.z, k), dof),
            z_fast_k: t_of(sidak_z(cfg.z_fast, k), dof),
            z_discard: cfg.z_discard,
            confirm_runs: cfg.confirm_runs,
            remeasure_after: cfg.remeasure_after,
        }
    }
}

/// The Student-t quantile with the same tail as the normal quantile `z`, at `dof` degrees of
/// freedom (the Cornish–Fisher expansion, exact as `dof → ∞`; within 2 % of the table for
/// `dof ≥ 4`). `dof = 0` (no repeated tree yet) is treated as 2.
pub fn t_of(z: f64, dof: usize) -> f64 {
    let v = dof.max(2) as f64;
    let g1 = (z.powi(3) + z) / 4.0;
    let g2 = (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / 96.0;
    let g3 = (3.0 * z.powi(7) + 19.0 * z.powi(5) + 17.0 * z.powi(3) - 15.0 * z) / 384.0;
    z + g1 / v + g2 / v.powi(2) + g3 / v.powi(3)
}

/// Degrees of freedom of the pooled σ: Σ (n − 1) over trees measured at least twice, on the
/// headline axis.
pub fn dof_of(trees: &BTreeMap<String, Vec<Vec<f64>>>) -> usize {
    trees
        .values()
        .filter_map(|t| t.first())
        .filter(|v| v.len() >= 2)
        .map(|v| v.len() - 1)
        .sum()
}

/// The trees one decision compares: the candidate, its parent, and the front's members
/// (each a per-axis sample vector; the parent may be among the members).
#[derive(Clone, Debug, PartialEq)]
pub struct Trees<'a> {
    /// The candidate's samples so far.
    pub candidate: &'a [Vec<f64>],
    /// The parent's samples.
    pub parent: &'a [Vec<f64>],
    /// Experiments since the parent's newest sample.
    pub parent_age: u32,
    /// The parent is the incumbent, so an in-place re-measure samples the right tree.
    pub parent_is_incumbent: bool,
    /// Every front member's samples (the parent may be among them).
    pub members: &'a [Vec<Vec<f64>>],
}

fn n_of(t: &[Vec<f64>]) -> usize {
    t.iter().map(Vec::len).min().unwrap_or(0)
}

/// One step of the sequential dominance decision (159 §6 "Comparing two trees", "The entry
/// rule", "Sequential sampling"), pure. One sample: `Worse` at `z ≤ −z_k` on any objective; a
/// fast entry needs `z_fast,k` on some objective; nothing above `z_discard` is `NoGain`;
/// anything between takes `confirm_runs` more samples. On means, every call is at `z_k`. A
/// keep candidate (better on ≥ 1 objective vs its parent) then meets the entry rule: no member
/// dominates it, it beats every member on some objective, every guard holds — `Improved` when
/// it dominates its parent, `Traded` otherwise.
pub fn decide(d: &Dom, t: &Trees) -> GateStep {
    let (n_c, n_p) = (n_of(t.candidate), n_of(t.parent));
    if n_c == 0 || n_p == 0 {
        return GateStep::Confirm {
            more: 1,
            incumbent_sample: n_p == 0 && t.parent_is_incumbent,
        };
    }
    // The parent is re-measured alongside a confirm when it is stale, or when it still has
    // fewer samples than a baseline (a fast keep plus one re-measure: its mean was selected for
    // being extreme, so a true gain over it reads low under the corrected threshold).
    let confirm = || GateStep::Confirm {
        more: d.confirm_runs,
        incumbent_sample: t.parent_is_incumbent && (t.parent_age >= d.remeasure_after || n_p < 3),
    };
    let deterministic = d.sigmas.iter().all(|s| *s <= 0.0);
    // R-25 as in 140: a single-sample incumbent was selected for being extreme; measure it once
    // more before judging a first candidate sample against it.
    if !deterministic && n_c == 1 && n_p < 2 && t.parent_is_incumbent {
        return GateStep::Confirm {
            more: 0,
            incumbent_sample: true,
        };
    }
    let vs_parent = compare(d.axes, d.sigmas, t.candidate, t.parent, d.z_k);
    let mut threshold = d.z_k;
    if !deterministic && n_c == 1 {
        if !vs_parent.better_any(d.axes) {
            if vs_parent.worse_any(d.axes) {
                return finish(&vs_parent, Verdict::Worse, threshold, n_c);
            }
            if vs_parent.z_max.unwrap_or(0.0) < d.z_discard {
                // A parent with fewer than the baseline's samples is a fast keep plus its
                // re-measure; a one-sample discard against it is still unreliable.
                let promising = vs_parent
                    .deltas
                    .iter()
                    .any(|x| x.delta.is_some_and(|v| v > 0.0));
                if n_p < 3 && promising {
                    return confirm();
                }
                return finish(&vs_parent, Verdict::NoGain, threshold, n_c);
            }
            return confirm();
        }
        // A possible trade (better here, worse there) is never entered on one sample.
        let fast = !vs_parent.worse_any(d.axes)
            && d.axes.iter().zip(&vs_parent.deltas).any(|(a, x)| {
                !a.guard && x.call == Call::Better && x.z.is_some_and(|z| z >= d.z_fast_k)
            });
        if !fast {
            return confirm();
        }
        threshold = d.z_fast_k;
    }
    if !vs_parent.better_any(d.axes) {
        let v = if vs_parent.worse_any(d.axes) {
            Verdict::Worse
        } else {
            Verdict::NoGain
        };
        return finish(&vs_parent, v, threshold, n_c);
    }
    if vs_parent.guard_failed() {
        return finish(&vs_parent, Verdict::GuardFailed, threshold, n_c);
    }
    for m in t.members {
        let vs_m = compare(d.axes, d.sigmas, t.candidate, m, d.z_k);
        let dominated = vs_m.worse_any(d.axes) && !vs_m.better_any(d.axes);
        if dominated {
            let v = if vs_parent.worse_any(d.axes) {
                Verdict::Worse
            } else {
                Verdict::NoGain
            };
            return finish(&vs_parent, v, threshold, n_c);
        }
        if !vs_m.better_any(d.axes) {
            // Condition 2: a noise-level clone of a member stays out.
            return finish(&vs_parent, Verdict::NoGain, threshold, n_c);
        }
    }
    let v = if vs_parent.worse_any(d.axes) {
        Verdict::Traded
    } else {
        Verdict::Improved
    };
    finish(&vs_parent, v, threshold, n_c)
}

/// `Decide` with the headline objective's scalar fields (axis 0 is the first competing
/// objective, which `StudySpec::headline` picks too) and the full per-objective detail.
fn finish(cmp: &Compare, verdict: Verdict, threshold: f64, n: usize) -> GateStep {
    let head = cmp.deltas.first();
    GateStep::Decide {
        verdict,
        delta: head.and_then(|x| x.delta).unwrap_or(0.0),
        delta_pct: head.and_then(|x| x.delta_pct).unwrap_or(0.0),
        z: head.and_then(|x| x.z),
        detail: Some(GateDecision {
            v: GateDecision::V,
            rule: GateRule::Dominance,
            objectives: cmp.deltas.clone(),
            stat: cmp.z_max,
            threshold: Some(threshold),
            n: n as u32,
        }),
    }
}

// ───────────────────────────── the front, replayed from the ledger ─────────────────────────────

/// One front member.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// `f<n>` (`f0` = the base).
    pub key: String,
    /// Its tree (a keep commit, or the base).
    pub sha: String,
    /// The experiment that landed it (0 = the base).
    pub n: u32,
    /// Objective means (competing objectives and guards).
    pub scores: BTreeMap<String, f64>,
    /// Per-axis clean sample values.
    pub values: Vec<Vec<f64>>,
    /// Experiments cut from it.
    pub children: u32,
}

/// The replayed front and what every surface shows of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Front {
    /// Competing objectives, then guards.
    pub axes: Vec<Axis>,
    /// Pooled within-tree σ per axis over every clean sample of the study.
    pub sigmas: Vec<f64>,
    /// Degrees of freedom behind `sigmas` ([`dof_of`]).
    pub dof: usize,
    /// The corrected keep threshold for this study's objective count.
    pub z_k: f64,
    /// The corrected one-sample threshold.
    pub z_fast_k: f64,
    /// Members, oldest first.
    pub members: Vec<Member>,
    /// Index into `members` of the best member on the headline objective.
    pub headline: usize,
    /// Every measured experiment (and the base as n = 0) with its objective means.
    pub points: Vec<FrontPoint>,
}

impl Front {
    /// The member whose tree is `sha`.
    pub fn member_of(&self, sha: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.sha == sha)
    }
    /// Member n's, oldest first.
    pub fn member_ns(&self) -> Vec<u32> {
        self.members.iter().map(|m| m.n).collect()
    }
    fn headline_member(&self) -> Option<&Member> {
        self.members.get(self.headline)
    }
    /// The two charted objectives: the first two competing ones.
    pub fn chart_axes(&self) -> [String; 2] {
        let mut it = self
            .axes
            .iter()
            .filter(|a| !a.guard)
            .map(|a| a.name.clone());
        let x = it.next().unwrap_or_default();
        let y = it.next().unwrap_or_else(|| x.clone());
        [x, y]
    }
    /// Archive rows (`kind = front`), one per member.
    pub fn rows(&self, slug: &str) -> Vec<ArchiveRow> {
        self.members
            .iter()
            .map(|m| ArchiveRow {
                slug: slug.to_owned(),
                kind: KIND.to_owned(),
                key: m.key.clone(),
                sha: m.sha.clone(),
                n: m.n,
                ref_name: refs::name(slug, RefKind::Front, &m.key),
                scores: m.scores.clone(),
                children: m.children,
                at: crate::store::now(),
                ..ArchiveRow::default()
            })
            .collect()
    }
}

/// NSGA-II crowding distance per member: the normalised gap between its neighbours along
/// each objective, summed; the extremes of every objective are infinite.
pub fn crowding(scores: &[Vec<f64>]) -> Vec<f64> {
    let n = scores.len();
    let mut dist = vec![0.0; n];
    let k = scores.first().map_or(0, Vec::len);
    for j in 0..k {
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| scores[a][j].total_cmp(&scores[b][j]));
        let (lo, hi) = (scores[order[0]][j], scores[order[n - 1]][j]);
        dist[order[0]] = f64::INFINITY;
        dist[order[n - 1]] = f64::INFINITY;
        if hi > lo {
            for w in order.windows(3) {
                dist[w[1]] += (scores[w[2]][j] - scores[w[0]][j]) / (hi - lo);
            }
        }
    }
    dist
}

/// Replay every kept experiment through the entry rule: the base enters first; a keep evicts
/// the members it dominates (at `z_k`, on each member's own samples); past [`FRONT_CAP`] the
/// member with the smallest crowding distance leaves. A keep whose tree has no clean sample
/// (never, after `record_keep` retags) is skipped.
pub fn replay(
    axes: &[Axis],
    sigmas: &[f64],
    z_k: f64,
    base: (&str, Vec<Vec<f64>>),
    keeps: &[(u32, String, Vec<Vec<f64>>)],
) -> Vec<Member> {
    let member = |key: String, sha: &str, n: u32, values: Vec<Vec<f64>>| Member {
        key,
        sha: sha.to_owned(),
        n,
        scores: axes
            .iter()
            .zip(&values)
            .filter(|(_, v)| !v.is_empty())
            .map(|(a, v)| (a.name.clone(), mean(v)))
            .collect(),
        values,
        children: 0,
    };
    let mut front: Vec<Member> = vec![member("f0".into(), base.0, 0, base.1)];
    for (n, sha, values) in keeps {
        if n_of(values) == 0 {
            continue;
        }
        front.retain(|m| !compare(axes, sigmas, values, &m.values, z_k).dominates(axes));
        front.push(member(format!("f{n}"), sha, *n, values.clone()));
        while front.len() > FRONT_CAP {
            let scores: Vec<Vec<f64>> = front
                .iter()
                .map(|m| {
                    axes.iter()
                        .filter(|a| !a.guard)
                        .map(|a| m.scores.get(&a.name).copied().unwrap_or(0.0))
                        .collect()
                })
                .collect();
            let d = crowding(&scores);
            let (victim, _) = d
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .expect("front over the cap is non-empty");
            front.remove(victim);
        }
    }
    front
}

/// The best member on the headline objective (axis 0, in its direction).
fn best_on_headline(axes: &[Axis], members: &[Member]) -> usize {
    let Some(a) = axes.first() else { return 0 };
    members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.scores.get(&a.name).map(|v| (i, *v)))
        .max_by(|x, y| {
            let (p, q) = match a.direction {
                Direction::Lower => (-x.1, -y.1),
                Direction::Higher => (x.1, y.1),
            };
            p.total_cmp(&q)
        })
        .map_or(0, |(i, _)| i)
}

/// Per-axis pooled within-tree σ over `trees` (tree → per-axis values), the 140 estimator.
fn sigmas_of(axes: &[Axis], trees: &BTreeMap<String, Vec<Vec<f64>>>) -> Vec<f64> {
    (0..axes.len())
        .map(|j| {
            let per_tree: Vec<Vec<f64>> = trees.values().map(|t| t[j].clone()).collect();
            gate::pooled_sigma(&per_tree)
        })
        .collect()
}

/// Every clean sample of the study grouped by tree, as per-axis values.
fn trees_of(samples: &[Sample], axes: &[Axis], headline: &str) -> BTreeMap<String, Vec<Vec<f64>>> {
    let mut by_tree: BTreeMap<String, Vec<&Sample>> = BTreeMap::new();
    for s in samples.iter().filter(|s| clean(s)) {
        by_tree.entry(s.tree_sha.clone()).or_default().push(s);
    }
    by_tree
        .into_iter()
        .map(|(sha, ss)| {
            let owned: Vec<Sample> = ss.into_iter().cloned().collect();
            (sha, tree_values(&owned, axes, headline))
        })
        .collect()
}

/// The study's `gate.z` / `gate.z_fast`; a `Store`-only surface (view, brief, headline, RPC)
/// cannot reach the config and passes the defaults (W1-i).
pub const DEFAULT_Z: (f64, f64) = (2.0, 3.5);

/// The front of a study, replayed from its ledger (see the module doc). `(z, z_fast)` are the
/// uncorrected gate thresholds; the replay evicts at `sidak_z(z, k)`.
pub async fn front(store: &Store, study: &StudyRecord, (z, z_fast): (f64, f64)) -> Result<Front> {
    let slug = &study.slug;
    let axes = axes_of(&study.spec)?;
    let headline_name = study.spec.headline().name;
    let samples = store.list_samples(slug, None).await?;
    let trees = trees_of(&samples, &axes, &headline_name);
    let sigmas = sigmas_of(&axes, &trees);
    let dof = dof_of(&trees);
    let k = competing(&axes);
    let (z_k, z_fast_k) = (t_of(sidak_z(z, k), dof), t_of(sidak_z(z_fast, k), dof));

    let mut exps = store.list_experiments(slug, None, u32::MAX).await?;
    exps.sort_by_key(|e| e.n);
    let base_sha = study
        .base_sha
        .clone()
        .or_else(|| {
            samples
                .iter()
                .find(|s| s.role == Some(SampleRole::Baseline))
                .map(|s| s.tree_sha.clone())
        })
        .unwrap_or_default();
    let base_values = trees
        .get(&base_sha)
        .cloned()
        .unwrap_or_else(|| vec![Vec::new(); axes.len()]);
    let keeps: Vec<(u32, String, Vec<Vec<f64>>)> = exps
        .iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .filter_map(|e| {
            let sha = e.commit_sha.clone()?;
            let values = trees.get(&sha)?.clone();
            Some((e.n, sha, values))
        })
        .collect();
    let mut members = replay(
        &axes,
        &sigmas,
        z_k,
        (&base_sha, base_values.clone()),
        &keeps,
    );
    for m in &mut members {
        m.children = exps.iter().filter(|e| e.parent_sha == m.sha).count() as u32;
    }
    let headline = best_on_headline(&axes, &members);

    // Points: the base, then every experiment with a measured candidate.
    let mut points = Vec::new();
    let base_scores: BTreeMap<String, f64> = axes
        .iter()
        .zip(&base_values)
        .filter(|(_, v)| !v.is_empty())
        .map(|(a, v)| (a.name.clone(), mean(v)))
        .collect();
    if !base_scores.is_empty() {
        points.push(FrontPoint {
            n: 0,
            sha: base_sha.clone(),
            scores: base_scores,
            member: members.iter().any(|m| m.n == 0),
        });
    }
    let mut by_n: BTreeMap<u32, Vec<&Sample>> = BTreeMap::new();
    for s in samples
        .iter()
        .filter(|s| clean(s) && s.n > 0 && s.role == Some(SampleRole::Candidate))
    {
        by_n.entry(s.n).or_default().push(s);
    }
    for (n, ss) in by_n {
        let owned: Vec<Sample> = ss.iter().map(|s| (*s).clone()).collect();
        let values = tree_values(&owned, &axes, &headline_name);
        let scores: BTreeMap<String, f64> = axes
            .iter()
            .zip(&values)
            .filter(|(_, v)| !v.is_empty())
            .map(|(a, v)| (a.name.clone(), mean(v)))
            .collect();
        if scores.is_empty() {
            continue;
        }
        let sha = exps
            .iter()
            .find(|e| e.n == n)
            .and_then(|e| e.commit_sha.clone())
            .unwrap_or_else(|| owned[0].tree_sha.clone());
        points.push(FrontPoint {
            n,
            sha,
            scores,
            member: members.iter().any(|m| m.n == n),
        });
    }
    Ok(Front {
        axes,
        sigmas,
        dof,
        z_k,
        z_fast_k,
        members,
        headline,
        points,
    })
}

/// Mirror the replayed front into the `archive` table, the front refs (when the worktree is a
/// git repository) and `studies.scores` (the headline member's means).
pub async fn sync(store: &Store, study: &StudyRecord, f: &Front) -> Result<()> {
    let slug = &study.slug;
    let want = f.rows(slug);
    let have = store.archive_list(slug, Some(KIND)).await?;
    let repo = study
        .worktree
        .as_deref()
        .map(Path::new)
        .filter(|p| git::is_git_repo(p));
    for h in &have {
        if !want.iter().any(|w| w.key == h.key) {
            store.archive_remove(slug, KIND, &h.key).await?;
            if let Some(repo) = repo {
                refs::delete(repo, slug, RefKind::Front, &h.key)?;
            }
        }
    }
    for w in &want {
        let same = have.iter().any(|h| {
            h.key == w.key && h.sha == w.sha && h.children == w.children && h.scores == w.scores
        });
        if !same {
            store.archive_put(w).await?;
        }
        if let Some(repo) = repo {
            refs::update(repo, slug, RefKind::Front, &w.key, &w.sha)?;
        }
    }
    if let Some(h) = f.headline_member()
        && h.scores != study.scores
    {
        store
            .patch_study(
                slug,
                &StudyPatch {
                    scores: Some(h.scores.clone()),
                    ..StudyPatch::default()
                },
            )
            .await?;
    }
    Ok(())
}

// ───────────────────────────── the plug-ins (159 §18.4) ─────────────────────────────

/// `GateRule::Dominance`: the candidate against its parent's own samples (not the incumbent's),
/// then the entry rule against the replayed front. `Decide { detail: Some(rule = dominance) }`.
pub async fn step(
    env: &Env,
    study: &StudyRecord,
    cfg: &GateCfg,
    _input: &GateInput,
    candidate: &[Sample],
    _reference: &[Sample],
) -> Result<GateStep> {
    let Some(first) = candidate.first() else {
        return Ok(GateStep::Confirm {
            more: 1,
            incumbent_sample: false,
        });
    };
    let n = first.n;
    let f = front(&env.store, study, (cfg.z, cfg.z_fast)).await?;
    let headline_name = study.spec.headline().name;
    let parent_sha = env
        .store
        .get_experiment(&study.slug, n)
        .await?
        .map(|e| e.parent_sha)
        .ok_or_else(|| Error::refused(format!("experiment {n} has no row")))?;
    let parent_samples = env.store.samples_of_tree(&study.slug, &parent_sha).await?;
    let parent = tree_values(&parent_samples, &f.axes, &headline_name);
    let parent_age = parent_samples
        .iter()
        .filter(|s| clean(s))
        .map(|s| s.n)
        .max()
        .map_or(n, |newest| n.saturating_sub(newest).saturating_sub(1));
    let cand = tree_values(candidate, &f.axes, &headline_name);
    let members: Vec<Vec<Vec<f64>>> = f.members.iter().map(|m| m.values.clone()).collect();
    // `front` pooled σ over every stored tree; the candidate's samples are in the ledger
    // before the rule runs (`scored_sample` inserts the row first).
    let d = Dom::new(cfg, &f.axes, &f.sigmas, f.dof);
    let t = Trees {
        candidate: &cand,
        parent: &parent,
        parent_age,
        parent_is_incumbent: study.incumbent_sha.as_deref() == Some(parent_sha.as_str()),
        members: &members,
    };
    Ok(decide(&d, &t))
}

/// `Search::Front`: the member with the fewest children, ties by the headline objective; slot
/// `k` of a wave takes the k-th in that order. The front is mirrored into the ledger and the
/// refs here, and the study worktree is reset to the parent before the brief is rendered.
pub async fn parent_of(env: &Env, study: &StudyRecord, slot: Option<u8>) -> Result<String> {
    let f = front(&env.store, study, (env.cfg.gate.z, env.cfg.gate.z_fast)).await?;
    sync(&env.store, study, &f).await?;
    if f.members.is_empty() {
        return Err(Error::refused(format!(
            "study `{}` has no front yet",
            study.slug
        )));
    }
    let head_axis = f.axes.first();
    let mut order: Vec<&Member> = f.members.iter().collect();
    order.sort_by(|a, b| {
        a.children.cmp(&b.children).then_with(|| {
            let Some(ax) = head_axis else {
                return std::cmp::Ordering::Equal;
            };
            let (x, y) = (
                a.scores.get(&ax.name).copied().unwrap_or(0.0),
                b.scores.get(&ax.name).copied().unwrap_or(0.0),
            );
            match ax.direction {
                Direction::Lower => x.total_cmp(&y),
                Direction::Higher => y.total_cmp(&x),
            }
        })
    });
    let pick = order[usize::from(slot.unwrap_or(0)) % order.len()];
    let parent = pick.sha.clone();
    if slot.is_none() {
        let worktree = crate::actor::worktree_of(study)?;
        if env.backend.head(&worktree)? != parent {
            let n = env.store.next_experiment_n(&study.slug).await?;
            let runs = env.runs_dir(&study.slug, n);
            std::fs::create_dir_all(&runs)?;
            env.backend
                .discard(&worktree, &parent, &runs.join("parent.patch"))?;
        }
    }
    Ok(parent)
}

/// After a decision: the front is a replay, so settling is mirroring (`sync`); kept for the
/// frozen surface and for W4.
pub async fn settle(env: &Env, study: &StudyRecord, _exp: &ExperimentRecord) -> Result<()> {
    let f = front(&env.store, study, (env.cfg.gate.z, env.cfg.gate.z_fast)).await?;
    sync(&env.store, study, &f).await
}

/// `KindView::Front` for the Study view.
pub async fn view(store: &Store, study: &StudyRecord) -> Result<KindView> {
    let f = front(store, study, DEFAULT_Z).await?;
    Ok(KindView::Front {
        axes: f.chart_axes(),
        members: f.member_ns(),
        points: f.points,
    })
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

fn nice(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let digits = (3 - v.abs().log10().floor() as i32 - 1).clamp(0, 6) as usize;
    format!("{v:.digits$}")
}

/// The brief's "Front so far" section.
pub async fn brief_section(store: &Store, study: &StudyRecord) -> Result<String> {
    let f = front(store, study, DEFAULT_Z).await?;
    if f.members.is_empty() {
        return Ok(String::new());
    }
    let mut out = format!(
        "\nFront so far ({} member{}; objectives {}):\n",
        f.members.len(),
        if f.members.len() == 1 { "" } else { "s" },
        f.axes
            .iter()
            .filter(|a| !a.guard)
            .map(|a| {
                format!(
                    "{} ({} is better)",
                    a.name,
                    match a.direction {
                        Direction::Lower => "lower",
                        Direction::Higher => "higher",
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    for m in &f.members {
        let scores: Vec<String> = f
            .axes
            .iter()
            .filter_map(|a| {
                m.scores
                    .get(&a.name)
                    .map(|v| format!("{} {}", a.name, nice(*v)))
            })
            .collect();
        out.push_str(&format!(
            "- {} {} · {} · {} experiment{} cut from it{}\n",
            m.key,
            short(&m.sha),
            scores.join(" · "),
            m.children,
            if m.children == 1 { "" } else { "s" },
            if Some(m) == f.headline_member() {
                " · best on the headline objective"
            } else {
                ""
            }
        ));
    }
    let open = store
        .list_experiments(&study.slug, None, 1)
        .await?
        .into_iter()
        .next()
        .filter(|e| e.state == ExperimentState::Open);
    if let Some(e) = open
        && let Some(m) = f.member_of(&e.parent_sha)
    {
        out.push_str(&format!(
            "Your change is cut from {} ({}). It is kept when it is better than that parent on at least one objective \
             (z ≥ {:.2} on means, {:.2} on one sample) and no front member beats it on every objective it does not lose on; \
             a trade (better on one, worse on another within the tolerated loss) joins the front as a new member.\n",
            m.key,
            short(&m.sha),
            f.z_k,
            f.z_fast_k
        ));
    }
    Ok(out)
}

/// Finalize's headline: `front of 3 · time_ms −21 % · size_kb −12 %` — per objective, the best
/// member's change against the base. Means come from the ledger's samples (W1-e: no fresh
/// per-member confirmation).
pub async fn headline(store: &Store, study: &StudyRecord) -> Result<Option<Headline>> {
    let f = front(store, study, DEFAULT_Z).await?;
    let Some(h) = f.headline_member() else {
        return Ok(None);
    };
    let base_sha = study.base_sha.clone().unwrap_or_default();
    let base_samples = store.samples_of_tree(&study.slug, &base_sha).await?;
    let base = tree_values(&base_samples, &f.axes, &study.spec.headline().name);
    let mut parts = vec![format!("front of {}", f.members.len())];
    for (j, a) in f.axes.iter().enumerate().filter(|(_, a)| !a.guard) {
        let b = mean(&base[j]);
        if base[j].is_empty() || b == 0.0 {
            continue;
        }
        let best = f
            .members
            .iter()
            .filter_map(|m| m.scores.get(&a.name).map(|v| pct(v - b, b)))
            .min_by(|x, y| {
                let (p, q) = match a.direction {
                    Direction::Lower => (*x, *y),
                    Direction::Higher => (-*x, -*y),
                };
                p.total_cmp(&q)
            });
        if let Some(p) = best {
            parts.push(format!(
                "{} {}{} %",
                a.name,
                if p < 0.0 { "−" } else { "+" },
                nice(p.abs())
            ));
        }
    }
    let base_mean = mean(&base[0]);
    let final_mean = h.scores.get(&f.axes[0].name).copied().unwrap_or(base_mean);
    Ok(Some(Headline {
        text: parts.join(" · "),
        base_mean,
        final_mean,
        delta_pct: pct(final_mean - base_mean, base_mean),
        holdout: None,
        sigma: f.sigmas.first().copied().unwrap_or(0.0),
        n: (
            base[0].len() as u32,
            h.values.first().map_or(0, Vec::len) as u32,
        ),
    }))
}

/// Draft card line 1: "Find the trade-offs between `time_ms` (lower) and `size_kb` (lower) of …".
/// Past three objectives the line carries the warning (each one raises the bar for every keep).
pub fn card_line(spec: &StudySpec) -> String {
    let dir = |d: Direction| match d {
        Direction::Lower => "lower",
        Direction::Higher => "higher",
    };
    let objs: Vec<String> = spec
        .objectives
        .iter()
        .filter(|o| o.role == ObjectiveRole::Objective)
        .map(|o| format!("`{}` ({})", o.name.trim(), dir(o.direction)))
        .collect();
    let joined = match objs.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => "the objectives".to_owned(),
    };
    let repo = Path::new(&spec.repo)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| spec.repo.clone());
    let mut line = format!(
        "Find the trade-offs between {joined} of `{}` in {repo} (base {}{}); every non-dominated version is kept on the front",
        spec.bench.command.trim(),
        spec.base.trim(),
        if spec.include_dirty {
            " + your uncommitted changes"
        } else {
            ""
        }
    );
    if objs.len() > 3 {
        line.push_str(" · warning: more than three objectives raise the bar for every keep");
    }
    line
}

/// `autoresearch.archive` rows of kind `front`: the replayed front (fresher than the mirrored
/// table between a keep and the next fire).
pub async fn archive(store: &Store, slug: &str) -> Result<Vec<ArchiveRow>> {
    let study = store
        .get_study(slug)
        .await?
        .ok_or_else(|| Error::refused(format!("no study `{slug}`")))?;
    Ok(front(store, &study, DEFAULT_Z).await?.rows(slug))
}

/// `autoresearch.archive {slug}` on the actor: the front (W1) or the cells (W4) by the study's
/// search axis. Lives here because the `Studies` message set is frozen after the skeleton.
pub struct ArchiveQuery {
    /// The study.
    pub slug: String,
}

impl kameo::message::Message<ArchiveQuery> for crate::Studies {
    type Reply = std::result::Result<Vec<ArchiveRow>, String>;

    async fn handle(
        &mut self,
        m: ArchiveQuery,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let study = self
            .store
            .get_study(&m.slug)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no study `{}`", m.slug))?;
        let rows = if study.spec.axes.search == Search::Population {
            crate::population::archive(&self.store, &m.slug).await
        } else {
            archive(&self.store, &m.slug).await
        };
        rows.map_err(|e| e.to_string())
    }
}

// ───────────────────────────── setup rows (151 panel) ─────────────────────────────

/// The one kind row: `objectives`, edited as lines of `name lower|higher min_effect
/// [max_regression] [unit]` (W1-a: a line grammar like `secondary_guards`, not indexed leaves).
pub const SETUP_ROWS: &[SetupRow] = &[SetupRow {
    row: "objectives",
    label: "Objectives",
    leaves: &["objectives"],
}];

/// One-line value of the `objectives` row.
pub fn setup_value(spec: &StudySpec, row: &str) -> String {
    if row != "objectives" {
        return String::new();
    }
    spec.objectives
        .iter()
        .filter(|o| o.role == ObjectiveRole::Objective)
        .map(|o| {
            let mut s = format!(
                "{} ({} is better; keep from {}",
                o.name.trim(),
                match o.direction {
                    Direction::Lower => "lower",
                    Direction::Higher => "higher",
                },
                if o.min_effect.trim().is_empty() {
                    "any gain"
                } else {
                    o.min_effect.trim()
                }
            );
            if !o.max_regression.trim().is_empty() {
                s.push_str(&format!(", give up to {}", o.max_regression.trim()));
            }
            if !o.unit.trim().is_empty() {
                s.push_str(&format!(", {}", o.unit.trim()));
            }
            s.push(')');
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Set `objectives` from the panel's lines. Guards and reports already in the spec stay; with
/// two or more objectives the axes become `pareto` · `front`.
pub fn setup_set(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    if leaf != "objectives" {
        return Err(format!("unknown field `{leaf}`"));
    }
    let mut objs = Vec::new();
    for l in v.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let t: Vec<&str> = l.split_whitespace().collect();
        let (name, dir, min) = match t.as_slice() {
            [n, d, m, ..] => (*n, *d, *m),
            _ => {
                return Err(format!(
                    "objective `{l}`: expected `name lower|higher min_effect [max_regression] [unit]`"
                ));
            }
        };
        let direction = match dir {
            "lower" => Direction::Lower,
            "higher" => Direction::Higher,
            _ => {
                return Err(format!(
                    "objective `{l}`: direction must be `lower` or `higher`"
                ));
            }
        };
        Effect::parse(min).map_err(|e| format!("objective `{l}`: min_effect {e}"))?;
        let mut rest = t[3..].iter();
        let mut max_regression = String::new();
        let mut unit = Vec::new();
        if let Some(x) = rest.next() {
            if Effect::parse(x).is_ok() && !x.trim().is_empty() {
                max_regression = (*x).to_owned();
            } else {
                unit.push(*x);
            }
        }
        unit.extend(rest.copied());
        objs.push(Objective {
            name: name.to_owned(),
            unit: unit.join(" "),
            direction,
            role: ObjectiveRole::Objective,
            min_effect: min.to_owned(),
            max_regression,
            source: Default::default(),
        });
    }
    spec.objectives
        .retain(|o| o.role != ObjectiveRole::Objective && o.role != ObjectiveRole::Primary);
    spec.objectives.splice(0..0, objs.iter().cloned());
    if objs.len() >= 2 {
        spec.axes.objectives = ObjectivesMode::Pareto;
        spec.axes.search = Search::Front;
    }
    spec.normalise();
    Ok(())
}

/// `Scoring` is `bench` for W1; the scoring dispatch never names this module. Kept so every
/// wave module has the same surface.
pub async fn sample(
    _env: &Env,
    _study: &StudyRecord,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    Err(Error::NotBuilt(OWNER))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use parking_lot::Mutex;
    use runtime::SessionId;

    use super::*;
    use crate::actor::{Backend, begin, create, detail, log, run};
    use crate::config::{AgentDefaults, AutoresearchConfig};
    use crate::custody::ScopeDiff;
    use crate::git::KeepMeta;
    use crate::novelty::Novelty;
    use crate::types::{
        Begin, BenchSpec, BudgetSpec, Create, Decision, Detail, ExperimentState, GateRule, Log,
        LogArgs, ReviewVerdict, Run, RunArgs, StartWhen, StudyState,
    };
    use crate::{Studies, binding};

    fn axis(name: &str, min: &str, max: &str) -> Axis {
        Axis {
            name: name.into(),
            direction: Direction::Lower,
            min_effect: Effect::parse(min).unwrap(),
            max_regression: Effect::parse(max).unwrap(),
            guard: false,
        }
    }

    fn two() -> Vec<Axis> {
        vec![axis("time_ms", "1%", ""), axis("size_kb", "1%", "10%")]
    }

    fn tree(t: &[f64], s: &[f64]) -> Vec<Vec<f64>> {
        vec![t.to_vec(), s.to_vec()]
    }

    fn dom<'a>(axes: &'a [Axis], sigmas: &'a [f64]) -> Dom<'a> {
        let cfg = GateCfg {
            direction: Direction::Lower,
            z: 2.0,
            z_fast: 3.5,
            z_discard: 1.0,
            z_suspicious: 8.0,
            confirm_runs: 2,
            remeasure_after: 6,
            min_effect: Effect::Relative(0.01),
            guards: vec![],
        };
        Dom::new(&cfg, axes, sigmas, 1_000_000)
    }

    fn verdict(s: &GateStep) -> Verdict {
        match s {
            GateStep::Decide { verdict, .. } => verdict.clone(),
            other => panic!("not a decision: {other:?}"),
        }
    }

    /// 159 §4 table: Šidák-corrected keep thresholds for k = 1…5 competing objectives.
    #[test]
    fn sidak_matches_the_design_table() {
        let want = [(1, 2.00), (2, 2.28), (3, 2.43), (4, 2.53), (5, 2.61)];
        for (k, z) in want {
            let got = sidak_z(2.0, k);
            assert!((got - z).abs() < 0.01, "k = {k}: {got}");
        }
        assert!((sidak_z(3.5, 2) - 3.68).abs() < 0.01);
        assert!((sidak_z(3.5, 5) - 3.91).abs() < 0.01);
        assert!((norm_tail(0.0) - 0.5).abs() < 1e-6);
        assert!((norm_tail(1.96) - 0.025).abs() < 1e-4);
    }

    /// §6 "Comparing two trees": better needs z ≥ z_k and the effect; worse needs z ≤ −z_k and a
    /// loss past `max_regression`; a deterministic objective is judged on effects alone.
    #[test]
    fn compare_calls_better_same_worse() {
        let axes = two();
        let r = tree(&[100.0, 100.0, 100.0], &[50.0, 50.0, 50.0]);
        // time clearly better, size 4 % worse but within the 10 % tolerated loss.
        let c = tree(&[90.0, 90.0, 90.0], &[52.0, 52.0, 52.0]);
        let cmp = compare(&axes, &[1.0, 1.0], &c, &r, 2.28);
        assert_eq!(cmp.deltas[0].call, Call::Better);
        assert_eq!(cmp.deltas[1].call, Call::Same);
        assert!(cmp.dominates(&axes));
        assert!((cmp.deltas[0].delta.unwrap() - 10.0).abs() < 1e-9);
        assert!((cmp.deltas[0].delta_pct.unwrap() + 10.0).abs() < 1e-9);
        // size 20 % worse: past the tolerated loss → worse; a trade, not a domination.
        let c = tree(&[90.0, 90.0, 90.0], &[60.0, 60.0, 60.0]);
        let cmp = compare(&axes, &[1.0, 1.0], &c, &r, 2.28);
        assert_eq!(cmp.deltas[1].call, Call::Worse);
        assert!(cmp.better_any(&axes) && cmp.worse_any(&axes) && !cmp.dominates(&axes));
        // Within noise on both.
        let c = tree(&[99.5, 100.5, 100.0], &[50.0, 50.0, 50.0]);
        let cmp = compare(&axes, &[1.0, 1.0], &c, &r, 2.28);
        assert!(cmp.deltas.iter().all(|d| d.call == Call::Same));
        // Deterministic size (σ = 0): a 1 % gain is better by effect alone, no z.
        let c = tree(&[100.0, 100.0, 100.0], &[49.0, 49.0, 49.0]);
        let cmp = compare(&axes, &[1.0, 0.0], &c, &r, 2.28);
        assert_eq!(cmp.deltas[1].call, Call::Better);
        assert_eq!(cmp.deltas[1].z, None);
        // A guard is judged in its own direction and never called better.
        let mut guarded = two();
        guarded.push(Axis {
            name: "peak_kb".into(),
            direction: Direction::Lower,
            min_effect: Effect::Absolute(0.0),
            max_regression: Effect::Relative(0.05),
            guard: true,
        });
        let r3 = vec![vec![100.0; 3], vec![50.0; 3], vec![10.0; 3]];
        let c3 = vec![vec![90.0; 3], vec![50.0; 3], vec![12.0; 3]];
        let cmp = compare(&guarded, &[1.0, 1.0, 0.1], &c3, &r3, 2.28);
        assert_eq!(cmp.deltas[2].call, Call::GuardFailed);
        assert!(cmp.guard_failed());
        let c3 = vec![vec![90.0; 3], vec![50.0; 3], vec![5.0; 3]];
        let cmp = compare(&guarded, &[1.0, 1.0, 0.1], &c3, &r3, 2.28);
        assert_eq!(cmp.deltas[2].call, Call::Same);
    }

    /// §6 "Sequential sampling" and "The entry rule" on one decision each.
    #[test]
    fn decide_sequential_and_entry_rule() {
        let axes = two();
        let sig = [1.0, 1.0];
        let d = dom(&axes, &sig);
        let parent = tree(&[100.0, 100.0, 100.0], &[50.0, 50.0, 50.0]);
        let members = [parent.clone()];
        fn trees<'a>(
            cand: &'a [Vec<f64>],
            parent: &'a [Vec<f64>],
            members: &'a [Vec<Vec<f64>>],
        ) -> Trees<'a> {
            Trees {
                candidate: cand,
                parent,
                parent_age: 0,
                parent_is_incumbent: false,
                members,
            }
        }
        // One sample, 10σ better on time, same size: a fast entry that dominates its parent.
        let s = decide(&d, &trees(&tree(&[90.0], &[50.0]), &parent, &members));
        assert_eq!(verdict(&s), Verdict::Improved);
        let GateStep::Decide {
            detail: Some(g), z, ..
        } = &s
        else {
            panic!()
        };
        assert_eq!(g.rule, GateRule::Dominance);
        assert_eq!(g.objectives.len(), 2);
        assert!((g.threshold.unwrap() - d.z_fast_k).abs() < 1e-9);
        assert!(z.unwrap() > 3.68);
        // One sample, a possible trade (faster, 30 % larger): never entered on one sample.
        let s = decide(&d, &trees(&tree(&[90.0], &[65.0]), &parent, &members));
        assert!(matches!(
            s,
            GateStep::Confirm {
                more: 2,
                incumbent_sample: false
            }
        ));
        // Means: the trade enters as `Traded` — better than the only member on time.
        let s = decide(
            &d,
            &trees(
                &tree(&[90.0, 90.0, 90.0], &[65.0, 65.0, 65.0]),
                &parent,
                &members,
            ),
        );
        assert_eq!(verdict(&s), Verdict::Traded);
        // One sample 4σ worse on time, nothing better: worse at once.
        let s = decide(&d, &trees(&tree(&[104.0], &[50.0]), &parent, &members));
        assert_eq!(verdict(&s), Verdict::Worse);
        // One sample within z_discard on both: no gain.
        let s = decide(&d, &trees(&tree(&[100.5], &[50.2]), &parent, &members));
        assert_eq!(verdict(&s), Verdict::NoGain);
        // One sample between z_discard and z_fast: confirm.
        let s = decide(&d, &trees(&tree(&[97.0], &[50.0]), &parent, &members));
        assert!(matches!(s, GateStep::Confirm { more: 2, .. }));
        // Condition 1: a member (not the parent) dominates the candidate → no entry.
        let elite = tree(&[80.0, 80.0, 80.0], &[40.0, 40.0, 40.0]);
        let s = decide(
            &d,
            &trees(
                &tree(&[90.0, 90.0, 90.0], &[50.0, 50.0, 50.0]),
                &parent,
                &[parent.clone(), elite.clone()],
            ),
        );
        assert_eq!(verdict(&s), Verdict::NoGain);
        // Condition 2: a noise-level clone of a member stays out.
        let s = decide(
            &d,
            &trees(
                &tree(&[90.0, 90.0, 90.0], &[45.0, 45.0, 45.0]),
                &parent,
                &[parent.clone(), tree(&[90.0; 3], &[45.0; 3])],
            ),
        );
        assert_eq!(verdict(&s), Verdict::NoGain);
        // Condition 3: a guard past its limit blocks the keep.
        let mut guarded = two();
        guarded.push(Axis {
            name: "peak_kb".into(),
            direction: Direction::Lower,
            min_effect: Effect::Absolute(0.0),
            max_regression: Effect::Relative(0.05),
            guard: true,
        });
        let sig3 = [1.0, 1.0, 0.0];
        let d3 = dom(&guarded, &sig3);
        let p3 = vec![vec![100.0; 3], vec![50.0; 3], vec![10.0; 3]];
        let c3 = vec![vec![90.0; 3], vec![50.0; 3], vec![12.0; 3]];
        let m3 = [p3.clone()];
        let s = decide(
            &d3,
            &Trees {
                candidate: &c3,
                parent: &p3,
                parent_age: 0,
                parent_is_incumbent: false,
                members: &m3,
            },
        );
        assert_eq!(verdict(&s), Verdict::GuardFailed);
        // A stale parent that is the incumbent asks for its re-measure; another member never does.
        let s = decide(
            &d,
            &Trees {
                candidate: &tree(&[97.0], &[50.0]),
                parent: &parent,
                parent_age: 7,
                parent_is_incumbent: true,
                members: &members,
            },
        );
        assert!(matches!(
            s,
            GateStep::Confirm {
                incumbent_sample: true,
                ..
            }
        ));
        let s = decide(
            &d,
            &Trees {
                candidate: &tree(&[97.0], &[50.0]),
                parent: &parent,
                parent_age: 7,
                parent_is_incumbent: false,
                members: &members,
            },
        );
        assert!(matches!(
            s,
            GateStep::Confirm {
                incumbent_sample: false,
                ..
            }
        ));
        // Deterministic objectives decide on the first sample by effect alone.
        let sig0 = [0.0, 0.0];
        let d0 = dom(&axes, &sig0);
        let s = decide(&d0, &trees(&tree(&[98.0], &[50.0]), &parent, &members));
        assert_eq!(verdict(&s), Verdict::Improved);
    }

    /// §6 "The entry rule" (members a keep dominates leave) and "Cap" (crowding distance).
    #[test]
    fn replay_evicts_dominated_and_caps_by_crowding() {
        let axes = two();
        let sig = [1.0, 1.0];
        let base = ("base", tree(&[100.0; 3], &[50.0; 3]));
        let keeps = vec![
            (1, "k1".to_owned(), tree(&[90.0; 3], &[70.0; 3])), // trade vs base
            (2, "k2".to_owned(), tree(&[95.0; 3], &[45.0; 3])), // dominates base
            (3, "k3".to_owned(), tree(&[80.0; 3], &[60.0; 3])), // dominates k1
        ];
        let f = replay(&axes, &sig, 2.28, (base.0, base.1.clone()), &keeps);
        let keys: Vec<&str> = f.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["f2", "f3"]);
        assert_eq!(f[0].scores["time_ms"], 95.0);
        // Ten trades along one line (no tolerated loss, so none dominates another): the cap
        // keeps the extremes and drops the most crowded first (k4, whose neighbour k5 sits
        // 0.2 closer to it).
        let strict = vec![axis("time_ms", "1%", ""), axis("size_kb", "1%", "")];
        let keeps: Vec<(u32, String, Vec<Vec<f64>>)> = (1..=10)
            .map(|i| {
                let x = 100.0 - 5.0 * i as f64;
                let crowd = if i == 5 { 0.2 } else { 0.0 };
                (
                    i,
                    format!("k{i}"),
                    tree(&[x + crowd; 3], &[50.0 + 5.0 * i as f64; 3]),
                )
            })
            .collect();
        let f = replay(&strict, &sig, 2.28, (base.0, base.1.clone()), &keeps);
        let keys: Vec<&str> = f.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(f.len(), FRONT_CAP, "{keys:?}");
        assert!(
            keys.contains(&"f0") && keys.contains(&"f10") && !keys.contains(&"f4"),
            "{keys:?}"
        );
        let d = crowding(&[vec![0.0, 0.0], vec![1.0, 1.0], vec![2.0, 2.0]]);
        assert!(d[0].is_infinite() && d[2].is_infinite() && (d[1] - 2.0).abs() < 1e-9);
    }

    /// The panel's `objectives` line grammar and the card's first line.
    #[test]
    fn setup_and_card() {
        let mut spec = StudySpec {
            repo: "/tmp/codec".into(),
            bench: BenchSpec {
                command: "bash bench.sh".into(),
                ..BenchSpec::default()
            },
            ..StudySpec::default()
        };
        setup_set(
            &mut spec,
            "objectives",
            "time_ms lower 1%\nsize_kb lower 2% 10% kB\n",
        )
        .unwrap();
        assert_eq!(spec.axes.objectives, ObjectivesMode::Pareto);
        assert_eq!(spec.axes.search, Search::Front);
        assert_eq!(spec.objectives.len(), 2);
        assert_eq!(spec.objectives[1].max_regression, "10%");
        assert_eq!(spec.objectives[1].unit, "kB");
        assert_eq!(
            spec.metric.name, "time_ms",
            "normalise mirrors the headline"
        );
        assert_eq!(spec.kind(), crate::types::KIND_PARETO);
        let v = setup_value(&spec, "objectives");
        assert!(
            v.contains("time_ms (lower is better; keep from 1%)") && v.contains("give up to 10%"),
            "{v}"
        );
        assert!(setup_set(&mut spec, "objectives", "time_ms sideways 1%").is_err());
        assert!(setup_set(&mut spec, "objectives", "time_ms").is_err());
        assert!(setup_set(&mut spec, "nope", "x").is_err());
        let line = card_line(&spec);
        assert!(line.starts_with("Find the trade-offs between `time_ms` (lower) and `size_kb` (lower) of `bash bench.sh` in codec"), "{line}");
        assert!(!line.contains("warning"));
        setup_set(
            &mut spec,
            "objectives",
            "a lower 1%\nb lower 1%\nc lower 1%\nd higher 1%",
        )
        .unwrap();
        assert!(card_line(&spec).contains("more than three objectives"));
    }

    // ───────────────────────────── the scripted toy-pareto study ─────────────────────────────

    /// The test backend for a front study: scripted `(time_ms, size_kb)` samples, no git, no
    /// models; `head` follows keeps and resets like a worktree would.
    struct Double {
        values: Mutex<VecDeque<(f64, f64)>>,
        head: Mutex<String>,
        resets: Mutex<Vec<String>>,
    }

    impl Double {
        fn feed(&self, v: &[(f64, f64)]) {
            self.values.lock().extend(v.iter().copied());
        }
    }

    #[async_trait::async_trait]
    impl Backend for Double {
        fn is_git_repo(&self, _: &Path) -> bool {
            true
        }
        fn resolve_base(&self, _: &Path, _: &str, _: bool) -> Result<String> {
            Ok("base0000".into())
        }
        fn worktree_add(
            &self,
            _: &Path,
            _: &StudySpec,
            worktree: &Path,
            base: &str,
        ) -> Result<String> {
            std::fs::create_dir_all(worktree)?;
            Ok(base.to_owned())
        }
        fn worktree_remove(&self, _: &Path, _: &Path) -> Result<()> {
            Ok(())
        }
        fn keep(
            &self,
            _: &Path,
            _: &StudySpec,
            meta: &KeepMeta,
            _: Option<&str>,
        ) -> Result<String> {
            let sha = format!("keep{:04}", meta.n);
            *self.head.lock() = sha.clone();
            Ok(sha)
        }
        fn discard(&self, _: &Path, sha: &str, patch_path: &Path) -> Result<String> {
            std::fs::write(patch_path, "diff")?;
            *self.head.lock() = sha.to_owned();
            self.resets.lock().push(sha.to_owned());
            Ok("patchhash".into())
        }
        fn apply_patch(&self, _: &Path, _: &Path) -> Result<()> {
            Ok(())
        }
        fn head(&self, _: &Path) -> Result<String> {
            Ok(self.head.lock().clone())
        }
        fn commit_parent(&self, _: &Path, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn repair(&self, _: &Path, _: &StudySpec, _: &str, _: &Path) -> Result<Option<String>> {
            Ok(None)
        }
        fn patch_hash(&self, _: &Path, _: &str) -> Result<String> {
            Ok("ph".into())
        }
        fn diff_text(&self, _: &Path, _: &str, _: &str, _: usize) -> Result<String> {
            Ok("diff".into())
        }
        async fn sample(
            &self,
            _: &StudySpec,
            _: &crate::config::Bench,
            req: SampleRequest<'_>,
            _: &CancellationToken,
        ) -> Result<Sample> {
            let (t, s) = self.values.lock().pop_front().unwrap_or_else(|| {
                panic!(
                    "no scripted sample for n={} k={} {:?}",
                    req.n, req.k, req.role
                )
            });
            Ok(Sample {
                n: req.n,
                k: req.k,
                tree_sha: req.tree_sha.to_owned(),
                role: Some(req.role),
                value: Some(t),
                secondaries: [("size_kb".to_owned(), s)].into_iter().collect(),
                exit: Some(0),
                wall_ms: 1,
                at: crate::store::now(),
                ..Sample::default()
            })
        }
        async fn command(
            &self,
            _: &str,
            _: Duration,
            _: SampleRequest<'_>,
            _: &CancellationToken,
        ) -> Result<(Option<i32>, String, u64)> {
            Ok((Some(0), String::new(), 1))
        }
        fn custody_take(
            &self,
            _: &StudySpec,
            _: &Path,
            dir: &Path,
        ) -> Result<BTreeMap<String, String>> {
            std::fs::create_dir_all(dir)?;
            Ok([("bench.sh".to_owned(), "abc".to_owned())]
                .into_iter()
                .collect())
        }
        fn custody_verify(&self, _: &BTreeMap<String, String>, _: &Path) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn scope_diff(&self, _: &StudySpec, _: &Path) -> Result<ScopeDiff> {
            Ok(ScopeDiff {
                changed: vec!["codec.py".into()],
                ..ScopeDiff::default()
            })
        }
        async fn novelty(
            &self,
            _: &Store,
            _: &str,
            _: &RunArgs,
            _: &str,
            _: f64,
            _: bool,
        ) -> Result<Novelty> {
            Ok(Novelty::default())
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
                reason: "fine".into(),
            })
        }
        async fn advise(
            &self,
            _: &Store,
            _: &StudyRecord,
            _: Option<&str>,
            _: &runtime::Runtime,
        ) -> Result<Vec<crate::types::IdeaDraft>> {
            Ok(vec![])
        }
    }

    async fn wait_state(store: &Store, slug: &str, want: StudyState) -> StudyRecord {
        for _ in 0..300 {
            let s = store.get_study(slug).await.unwrap().unwrap();
            if s.state == want {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("study `{slug}` never reached {want:?}");
    }

    fn toy_pareto_spec(repo: &str) -> StudySpec {
        let obj = |name: &str, unit: &str| Objective {
            name: name.into(),
            unit: unit.into(),
            direction: Direction::Lower,
            role: ObjectiveRole::Objective,
            min_effect: "2%".into(),
            max_regression: String::new(),
            source: Default::default(),
        };
        let mut spec = StudySpec {
            slug: "toy-pareto".into(),
            goal: "find the codec trade-offs".into(),
            repo: repo.into(),
            axes: crate::types::Axes {
                objectives: ObjectivesMode::Pareto,
                search: Search::Front,
                ..Default::default()
            },
            objectives: vec![obj("time_ms", "ms"), obj("size_kb", "kB")],
            bench: BenchSpec {
                command: "bash bench.sh".into(),
                repeats: 3,
                ..BenchSpec::default()
            },
            budget: BudgetSpec {
                experiments: 50,
                ..BudgetSpec::default()
            },
            ..StudySpec::default()
        };
        spec.normalise();
        crate::tools::validate_spec(&spec).unwrap();
        spec
    }

    /// 159 §6 "Fixture study and proof", P, end to end on toy-pareto's measured numbers
    /// (README): base = the RLE codec (52 ms · 79 KB); #1 drops the pass (6 ms · 78.6 KB,
    /// dominates the base); #2 bz2 (20 ms · 48 KB, a trade); #3 a bz2 clone (no gain); #4 store
    /// (0.3 ms · 266 KB, a trade cut from f1); #5 slower and bigger (worse). Proves the parent
    /// choice, the in-place re-measure only when the parent is the incumbent, the verdicts and
    /// their stored detail, the archive mirror, the view, the brief section and the headline.
    #[tokio::test]
    async fn toy_pareto_front_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let root = dir.path().join("autoresearch");
        let store = Store::open(&root).await.unwrap();
        let double = Arc::new(Double {
            values: Mutex::new(VecDeque::from([(52.0, 79.0), (52.4, 79.0), (51.6, 79.0)])),
            head: Mutex::new("base0000".into()),
            resets: Default::default(),
        });
        let mut st = Studies {
            data_dir: dir.path().to_path_buf(),
            root,
            cfg: AutoresearchConfig::default(),
            agent: AgentDefaults::default(),
            rt,
            store,
            backend: double.clone(),
        };
        let spec = toy_pareto_spec(&dir.path().display().to_string());
        create(
            &mut st,
            Create {
                spec,
                origin: SessionId::new(),
                start: StartWhen::Now,
            },
        )
        .await
        .unwrap();
        wait_state(&st.store, "toy-pareto", StudyState::Running).await;
        let session = SessionId::new();
        binding::on_fired(&mut st, "autoresearch-toy-pareto", session)
            .await
            .unwrap();
        let b = || Begin {
            session,
            slot: None,
        };
        let r = |idea: &str| Run {
            session,
            slot: None,
            args: RunArgs {
                idea: idea.into(),
                category: "codec".into(),
                ..RunArgs::default()
            },
        };
        let l = || Log {
            session,
            slot: None,
            args: LogArgs {
                description: "d".into(),
                learned: "l".into(),
                decision: Decision::Auto,
                ..LogArgs::default()
            },
        };
        let exp = |store: &Store, n: u32| {
            let store = store.clone();
            async move {
                store
                    .get_experiment("toy-pareto", n)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };

        // #1: drop the RLE pass — one sample, far better on time, same size: a fast `Improved`.
        let brief = begin(&mut st, b()).await.unwrap();
        assert!(
            brief.text.contains("Front so far (1 member") && brief.text.contains("f0 base000"),
            "{}",
            brief.text
        );
        double.feed(&[(6.0, 78.6)]);
        let rep = run(&mut st, r("drop the RLE pass")).await.unwrap();
        assert_eq!(rep.verdict, Verdict::Improved, "{}", rep.line);
        // 52 → 6 ms is a regime change (140 §5.3 Revision 2): the keep opens a segment and,
        // with one clean sample, spends the R-25 re-measure now.
        double.feed(&[(6.3, 78.6)]);
        let lr = log(&mut st, l()).await.unwrap();
        assert_eq!(lr.applied, ExperimentState::Kept);
        assert!(
            double.values.lock().is_empty(),
            "the regime re-measure was taken"
        );
        let e1 = exp(&st.store, 1).await;
        assert_eq!(e1.parent_sha, "base0000");
        let g = e1.gate.expect("dominance detail stored");
        assert_eq!(g.rule, GateRule::Dominance);
        assert_eq!(
            g.objectives.iter().map(|o| o.call).collect::<Vec<_>>(),
            [Call::Better, Call::Same]
        );
        // One sample against the base: the fast threshold, Šidák-corrected for two objectives
        // and widened to the t quantile at the σ pool's 2 degrees of freedom.
        assert!(
            (g.threshold.unwrap() - t_of(sidak_z(3.5, 2), 2)).abs() < 1e-9,
            "{:?}",
            g.threshold
        );

        // #2: bz2 — a trade on the first sample (slower, much smaller) confirms; the parent f1
        // is the incumbent with two samples, so the confirm re-measures it in place once (6.1).
        begin(&mut st, b()).await.unwrap();
        double.feed(&[(20.0, 48.0), (6.1, 78.6), (20.4, 48.0), (19.8, 48.0)]);
        let rep = run(&mut st, r("bz2")).await.unwrap();
        assert_eq!(rep.verdict, Verdict::Traded, "{}", rep.line);
        assert!(
            double.values.lock().is_empty(),
            "every scripted sample consumed"
        );
        let lr = log(&mut st, l()).await.unwrap();
        assert_eq!(lr.applied, ExperimentState::Kept);
        let e2 = exp(&st.store, 2).await;
        assert_eq!(e2.parent_sha, "keep0001");
        let g = e2.gate.unwrap();
        assert_eq!(
            g.objectives.iter().map(|o| o.call).collect::<Vec<_>>(),
            [Call::Worse, Call::Better]
        );
        assert_eq!(g.n, 3);

        // #3: a bz2 clone, cut from f2 (fewest children): no gain on one sample.
        let brief = begin(&mut st, b()).await.unwrap();
        assert!(
            brief.text.contains("Front so far (2 members") && brief.text.contains("cut from f2"),
            "{}",
            brief.text
        );
        assert_eq!(exp(&st.store, 3).await.parent_sha, "keep0002");
        double.feed(&[(20.1, 48.0)]);
        let rep = run(&mut st, r("bz2 again")).await.unwrap();
        assert_eq!(rep.verdict, Verdict::NoGain, "{}", rep.line);
        assert_eq!(
            log(&mut st, l()).await.unwrap().applied,
            ExperimentState::Discarded
        );

        // #4: store — f1 and f2 tie on children, the headline (time) picks f1; the worktree is
        // reset to f1 (not the incumbent keep0002); no in-place re-measure because the parent
        // is not the incumbent; the trade enters.
        let resets_before = double.resets.lock().len();
        begin(&mut st, b()).await.unwrap();
        assert_eq!(exp(&st.store, 4).await.parent_sha, "keep0001");
        assert_eq!(
            double.resets.lock()[resets_before..],
            ["keep0001".to_owned()],
            "the worktree is reset to the parent"
        );
        double.feed(&[(0.3, 266.0), (0.35, 266.0), (0.3, 266.0)]);
        let rep = run(&mut st, r("store, no compression")).await.unwrap();
        assert_eq!(rep.verdict, Verdict::Traded, "{}", rep.line);
        assert!(
            double.values.lock().is_empty(),
            "no incumbent sample was taken for a non-incumbent parent"
        );
        assert_eq!(
            log(&mut st, l()).await.unwrap().applied,
            ExperimentState::Kept
        );

        // #5: slower and bigger, cut from f4: worse on one sample.
        begin(&mut st, b()).await.unwrap();
        assert_eq!(exp(&st.store, 5).await.parent_sha, "keep0004");
        double.feed(&[(30.0, 300.0)]);
        let rep = run(&mut st, r("a slow big codec")).await.unwrap();
        assert_eq!(rep.verdict, Verdict::Worse, "{}", rep.line);
        assert_eq!(
            log(&mut st, l()).await.unwrap().applied,
            ExperimentState::Discarded
        );

        // #6 opens: the archive mirrors the front as it stood before #6 (f1, f2, f4; the base
        // left when f1 dominated it); f2 and f4 tie on children, the headline picks f4.
        begin(&mut st, b()).await.unwrap();
        let rows = st
            .store
            .archive_list("toy-pareto", Some(KIND))
            .await
            .unwrap();
        let keys: Vec<(&str, &str, u32)> = rows
            .iter()
            .map(|r| (r.key.as_str(), r.sha.as_str(), r.children))
            .collect();
        assert_eq!(
            keys,
            [
                ("f1", "keep0001", 2),
                ("f2", "keep0002", 1),
                ("f4", "keep0004", 1)
            ]
        );
        assert_eq!(rows[0].ref_name, "refs/autoresearch/toy-pareto/front/f1");
        assert_eq!(
            exp(&st.store, 6).await.parent_sha,
            "keep0004",
            "fewest children, then the headline"
        );
        let study = st.store.get_study("toy-pareto").await.unwrap().unwrap();
        assert!(
            (study.scores["time_ms"] - 0.3167).abs() < 1e-3,
            "{:?}",
            study.scores
        );
        assert_eq!(
            study.incumbent_sha.as_deref(),
            Some("keep0004"),
            "the engine's incumbent is the latest keep (W1-b)"
        );

        // The view, the RPC rows and the headline read the same replay.
        let d = detail(
            &mut st,
            Detail {
                slug: "toy-pareto".into(),
            },
        )
        .await
        .unwrap();
        let KindView::Front {
            axes,
            points,
            members,
        } = d.view
        else {
            panic!("{:?}", d.view)
        };
        assert_eq!(axes, ["time_ms".to_owned(), "size_kb".to_owned()]);
        assert_eq!(members, [1, 2, 4]);
        assert_eq!(
            points.iter().map(|p| p.n).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert!(
            points[0].member == false
                && points[1].member
                && (points[2].scores["size_kb"] - 48.0).abs() < 1e-9
        );
        let rows = archive(&st.store, "toy-pareto").await.unwrap();
        assert_eq!(rows.len(), 3);
        let h = headline(&st.store, &study).await.unwrap().unwrap();
        assert_eq!(
            h.text, "front of 3 · time_ms −99.4 % · size_kb −39.2 %",
            "{}",
            h.text
        );
        assert!((h.base_mean - 52.0).abs() < 1e-9 && h.n.0 == 3);
        // Every stored dominance decision carries its corrected threshold (the realised entry
        // rate of the as-built reads these rows).
        for n in 1..=5 {
            let g = exp(&st.store, n)
                .await
                .gate
                .expect("detail on every decided row");
            assert!(
                g.threshold.unwrap() >= sidak_z(2.0, 2),
                "n = {n}: {:?}",
                g.threshold
            );
        }
    }
}
