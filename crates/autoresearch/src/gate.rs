//! W1 MEASURE — noise and the sequential gate (design doc 140 §5.2): pooled
//! within-tree σ (robust variant past 12 repeated samples), the threshold
//! `T = max(z σ √(1/n_c + 1/n_i), min_effect · |x̄_i|)`, the one-sample /
//! confirm decision, the suspicious rule, guards, keep-simpler, and the
//! incumbent re-measure drift rule. Pure functions over [`GateInput`].

use crate::types::{Direction, GateCfg, GateInput, GateStep};

/// Pooled within-tree standard deviation over `trees` (each a tree's samples,
/// `noisy` ones already removed); `0.0` when no tree has two samples (140 §5.2 step 1).
pub fn pooled_sigma(_trees: &[Vec<f64>]) -> f64 {
    // stub: W1
    0.0
}

/// Direction-normalise: positive = better.
pub fn normalise(direction: Direction, candidate_mean: f64, incumbent_mean: f64) -> f64 {
    match direction {
        Direction::Lower => incumbent_mean - candidate_mean,
        Direction::Higher => candidate_mean - incumbent_mean,
    }
}

/// One step of the sequential decision after the samples in `input` (140 §5.2 step 3).
pub fn step(_cfg: &GateCfg, _input: &GateInput) -> GateStep {
    // stub: W1
    GateStep::Confirm {
        more: 0,
        incumbent_sample: false,
    }
}

/// Keep-simpler check (140 §5.2 step 3): not significantly worse (`z > −1` on
/// means), `removed > added` lines, guards hold. Checks run elsewhere.
pub fn keep_simpler(_cfg: &GateCfg, _input: &GateInput, _added: u32, _removed: u32) -> bool {
    // stub: W1
    false
}

/// Re-measure drift (140 §5.2 step 4): a new incumbent sample outside 3σ of the mean.
pub fn drifted(_sigma: f64, _incumbent_mean: f64, _sample: f64) -> bool {
    // stub: W1
    false
}

/// The verdict in words and numbers (140 §5.2 step 5), e.g.
/// `improved: 41.2 ms vs 42.3 ms (−2.6 %), 3.1σ over 3 samples, checks passed`.
pub fn line(
    _step: &GateStep,
    _unit: &str,
    _candidate_mean: f64,
    _incumbent_mean: f64,
    _n: usize,
) -> String {
    // stub: W1
    String::new()
}
