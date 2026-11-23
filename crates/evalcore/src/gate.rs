//! The shared gate (159 W-r, §13.1): dreaming's margin rule plus a paired t-test when there are
//! at least 10 pairs; below 10 pairs the margin rule alone, so a dreaming night behaves as today.
//! Owner: W2 (`margin` moves from crates/dreaming/src/replay.rs#L287-L325; `paired` is new, W-n).
//! W7 consumes `paired` and `decide` with one fixture × seed as one pair (W-k).

use serde::{Deserialize, Serialize};
use statrs::distribution::{ContinuousCDF, StudentsT};

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

impl Pair {
    /// `candidate − baseline`.
    pub fn delta(&self) -> f64 {
        self.candidate - self.baseline
    }
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

/// Pairs from 30 up use `z = 2.0` (140's confirmed keep); below, the t quantile at the same
/// one-sided tail, so `c_n` is continuous at 30.
pub const PAIRS_FOR_Z: usize = 30;
/// Pairs from here up add the paired t to the margin rule (W-r).
pub const PAIRS_FOR_T: usize = 10;
/// Φ(2.0): the one-sided tail 140's `z = 2.0` keeps.
const TAIL_P: f64 = 0.977_249_868_051_820_8;

/// Mean of `v`; 0 when empty.
fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Dreaming's margin rule: accept iff mean Δ ≥ `min_mean_delta` and min Δ ≥ −`max_case_regression`
/// over at least `min_pairs` pairs.
pub fn margin(pairs: &[Pair], cfg: &Cfg) -> Decision {
    if pairs.len() < cfg.min_pairs {
        return Decision::Unverified(format!(
            "{} val pairs, {} needed",
            pairs.len(),
            cfg.min_pairs
        ));
    }
    let deltas: Vec<f64> = pairs.iter().map(Pair::delta).collect();
    let m = mean(&deltas);
    let min_delta = deltas.iter().copied().fold(f64::INFINITY, f64::min);
    if m < cfg.min_mean_delta {
        return Decision::Reject(format!(
            "mean Δ {m:+.3} below the margin {:.3}",
            cfg.min_mean_delta
        ));
    }
    if min_delta < -cfg.max_case_regression {
        return Decision::Reject(format!(
            "a case regressed by {:.3}, more than {:.3}",
            -min_delta, cfg.max_case_regression
        ));
    }
    Decision::Accept
}

/// The paired t over the deltas; `None` below 2 pairs. With zero spread `t` is `±∞` (or `0`
/// when the mean is 0 too), so a deterministic set still decides.
pub fn paired(pairs: &[Pair]) -> Option<Paired> {
    let n = pairs.len();
    if n < 2 {
        return None;
    }
    let deltas: Vec<f64> = pairs.iter().map(Pair::delta).collect();
    let m = mean(&deltas);
    let var = deltas.iter().map(|d| (d - m).powi(2)).sum::<f64>() / (n - 1) as f64;
    let sd = var.sqrt();
    let t = if sd > 0.0 {
        m / (sd / (n as f64).sqrt())
    } else if m == 0.0 {
        0.0
    } else {
        m.signum() * f64::INFINITY
    };
    Some(Paired {
        n,
        mean: m,
        sd,
        t,
        critical: critical(n),
        min_delta: deltas.iter().copied().fold(f64::INFINITY, f64::min),
    })
}

/// `c_n` (W-n): 2.0 from [`PAIRS_FOR_Z`] pairs up, else the one-sided Student-t quantile at
/// n − 1 degrees of freedom for the same tail.
pub fn critical(n: usize) -> f64 {
    if n >= PAIRS_FOR_Z {
        2.0
    } else {
        t_quantile(n.saturating_sub(1).max(1) as f64, TAIL_P)
    }
}

/// `margin`, plus `paired` (t ≥ critical) when there are at least [`PAIRS_FOR_T`] pairs (W-r).
pub fn decide(pairs: &[Pair], cfg: &Cfg) -> Decision {
    let m = margin(pairs, cfg);
    if m != Decision::Accept || pairs.len() < PAIRS_FOR_T {
        return m;
    }
    match paired(pairs) {
        Some(p) if p.t < p.critical => Decision::Reject(format!(
            "paired t {:.2} below c_{} = {:.2} (mean Δ {:+.3}, sd {:.3})",
            p.t, p.n, p.critical, p.mean, p.sd
        )),
        _ => Decision::Accept,
    }
}

// ── Student t ─────────────────────────────────────────────────────────────────────────────────

/// The t quantile at `p` (0.5 < p < 1); `p ≤ 0.5` is `0`, `p ≥ 1` is `+∞`, results cap at 1e6,
/// and degrees of freedom that are not positive and finite give `+∞` (nothing clears it).
pub fn t_quantile(df: f64, p: f64) -> f64 {
    if p >= 1.0 {
        return f64::INFINITY;
    }
    if p.is_nan() || p <= 0.5 {
        return 0.0;
    }
    match StudentsT::new(0.0, 1.0, df) {
        Ok(t) => t.inverse_cdf(p).min(1e6),
        Err(_) => f64::INFINITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(b: &[f64], c: &[f64]) -> Vec<Pair> {
        b.iter()
            .zip(c)
            .enumerate()
            .map(|(i, (b, c))| Pair {
                id: format!("c{i}"),
                baseline: *b,
                candidate: *c,
            })
            .collect()
    }

    const DREAM: Cfg = Cfg {
        min_mean_delta: 0.1,
        max_case_regression: 0.2,
        min_pairs: 3,
    };

    /// Dreaming's three margin cases (crates/dreaming/src/replay.rs tests), unchanged.
    #[test]
    fn margin_is_dreamings_rule() {
        assert_eq!(
            margin(&pairs(&[0.4, 0.5, 0.3], &[0.7, 0.6, 0.5]), &DREAM),
            Decision::Accept
        );
        // Mean improves but one case regresses by 0.3.
        assert!(matches!(
            margin(&pairs(&[0.4, 0.5, 0.9], &[0.9, 0.9, 0.6]), &DREAM),
            Decision::Reject(r) if r.contains("regressed")
        ));
        // Mean improves by less than the margin.
        assert!(matches!(
            margin(&pairs(&[0.4, 0.5, 0.3], &[0.45, 0.55, 0.35]), &DREAM),
            Decision::Reject(r) if r.contains("margin")
        ));
        assert!(matches!(
            margin(&pairs(&[0.1, 0.1], &[0.9, 0.9]), &DREAM),
            Decision::Unverified(_)
        ));
        assert!(matches!(margin(&[], &DREAM), Decision::Unverified(_)));
    }

    /// The t quantile against the two-sided 95 % table (one-sided 0.975): df 1, 2, 3, 4, 10, 29.
    #[test]
    fn t_quantile_matches_the_table() {
        for (df, want) in [
            (1.0, 12.706),
            (2.0, 4.303),
            (3.0, 3.182),
            (4.0, 2.776),
            (10.0, 2.228),
            (29.0, 2.045),
        ] {
            let got = t_quantile(df, 0.975);
            assert!((got - want).abs() < 0.002, "df {df}: {got} vs {want}");
        }
        // c_n: continuous at 30, monotone below, 2.0 at and above.
        assert_eq!(critical(30), 2.0);
        assert_eq!(critical(300), 2.0);
        let c29 = critical(29);
        assert!(c29 > 2.0 && c29 < 2.1, "{c29}");
        assert!(critical(10) > critical(20) && critical(20) > c29);
        assert!(critical(2) > 10.0);
    }

    #[test]
    fn paired_t_and_decide() {
        assert!(paired(&pairs(&[0.1], &[0.2])).is_none());
        // Deterministic uplift on 12 pairs: t is huge (±∞ with zero spread) and passes; the margin rule still applies.
        let base: Vec<f64> = (0..12).map(|i| 0.3 + 0.01 * i as f64).collect();
        let up: Vec<f64> = base.iter().map(|b| b + 0.2).collect();
        let p = paired(&pairs(&base, &up)).unwrap();
        assert!(p.t > 100.0 && (p.mean - 0.2).abs() < 1e-9, "{p:?}");
        let study = Cfg {
            min_mean_delta: 0.05,
            max_case_regression: 0.2,
            min_pairs: 10,
        };
        assert_eq!(decide(&pairs(&base, &up), &study), Decision::Accept);
        // Noisy: mean +0.1 but the deltas swing ±0.5 — the margin passes, the t does not.
        let noisy: Vec<f64> = base
            .iter()
            .enumerate()
            .map(|(i, b)| b + if i % 2 == 0 { 0.6 } else { -0.4 } + 0.1)
            .collect();
        let d = decide(&pairs(&base, &noisy), &study);
        // min Δ = −0.3 < −0.2 → the margin rule already rejects; make the regression legal.
        assert!(matches!(d, Decision::Reject(_)));
        let noisy: Vec<f64> = base
            .iter()
            .enumerate()
            .map(|(i, b)| b + if i % 2 == 0 { 0.36 } else { -0.14 })
            .collect();
        let m = margin(&pairs(&base, &noisy), &study);
        assert_eq!(
            m,
            Decision::Accept,
            "mean +0.11, min −0.14 passes the margin"
        );
        assert!(
            matches!(decide(&pairs(&base, &noisy), &study), Decision::Reject(r) if r.contains("paired t")),
            "{:?}",
            paired(&pairs(&base, &noisy))
        );
        // Below 10 pairs `decide` is the margin rule alone (W-r): the same noisy pattern on 6 pairs passes.
        let d6 = decide(&pairs(&base[..6], &noisy[..6]), &DREAM);
        assert_eq!(d6, Decision::Accept);
        // No improvement at all: t = 0, rejected by the margin.
        assert!(matches!(
            decide(&pairs(&base, &base), &study),
            Decision::Reject(_)
        ));
    }
}
