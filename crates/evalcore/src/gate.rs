//! The shared gate (159 W-r, §13.1): dreaming's margin rule plus a paired t-test when there are
//! at least 10 pairs; below 10 pairs the margin rule alone, so a dreaming night behaves as today.
//! Owner: W2 (`margin` moves from crates/dreaming/src/replay.rs#L287-L325; `paired` is new, W-n).
//! W7 consumes `paired` and `decide` with one fixture × seed as one pair (W-k).

use serde::{Deserialize, Serialize};

/// One case scored on both trees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    /// Case id.
    pub id: String,
    /// The incumbent's (baseline's) score.
    pub baseline: f64,
    /// The candidate's score.
    pub candidate: f64,
}

/// The margin rule's parameters (dreaming's `gate.*` config; a prompt study's `min_effect` and
/// `max_regression`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cfg {
    /// Accept needs mean Δ at or above this (dreaming 0.1; prompt studies 0.05).
    pub min_mean_delta: f64,
    /// Accept needs min Δ at or above minus this (0.2).
    pub max_case_regression: f64,
    /// Fewer pairs than this is `Unverified` (dreaming 3; prompt studies 10).
    pub min_pairs: usize,
}

/// What the gate said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Keep.
    Accept,
    /// Do not keep; the reason names the failing rule.
    Reject(String),
    /// Too few pairs to say.
    Unverified(String),
}

/// The paired t statistic over the pairs' deltas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paired {
    /// Pairs.
    pub n: usize,
    /// Mean delta.
    pub mean: f64,
    /// Sample standard deviation of the deltas.
    pub sd: f64,
    /// `mean / (sd / √n)`.
    pub t: f64,
    /// `c_n`: 2.0 from 30 pairs up, the one-sided 97.7 % Student-t quantile at n − 1 below (W-n).
    pub critical: f64,
    /// The smallest delta.
    pub min_delta: f64,
}

/// Dreaming's margin rule: accept iff mean Δ ≥ `min_mean_delta` and min Δ ≥ −`max_case_regression`
/// over at least `min_pairs` pairs.
pub fn margin(pairs: &[Pair], cfg: &Cfg) -> Decision {
    let _ = (pairs, cfg);
    Decision::Unverified("not built (W2)".into())
}

/// The paired t over the deltas; `None` below 2 pairs.
pub fn paired(pairs: &[Pair]) -> Option<Paired> {
    let _ = pairs;
    None
}

/// `margin`, plus `paired` (t ≥ critical) when there are at least 10 pairs (W-r).
pub fn decide(pairs: &[Pair], cfg: &Cfg) -> Decision {
    margin(pairs, cfg)
}
