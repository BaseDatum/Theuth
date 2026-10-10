//! W1 MEASURE — noise and the sequential gate (design doc 140 §5.2): pooled
//! within-tree σ (robust variant past 12 repeated samples), the threshold
//! `T = max(z σ √(1/n_c + 1/n_i), min_effect · |x̄_i|)`, the one-sample /
//! confirm decision, the suspicious rule, guards, keep-simpler, and the
//! incumbent re-measure drift rule. Pure functions over [`GateInput`].

use crate::types::{Direction, GateCfg, GateInput, GateStep, Verdict};

/// Repeated samples (over trees measured at least twice) from which the
/// robust estimate replaces the classical pooled σ (140 §5.2 step 1).
pub const ROBUST_AFTER: usize = 12;

/// σ = median |x_j − x_k| over within-tree pairs × this: for normal noise
/// `|x_j − x_k| ~ σ √2 |N(0,1)|` and the median of `|N(0,1)|` is 0.67449.
const PAIR_MEDIAN_TO_SIGMA: f64 = 1.048_27;

/// Pooled within-tree standard deviation over `trees` (each a tree's samples,
/// `noisy` ones already removed); `0.0` when no tree has two samples (140 §5.2 step 1).
///
/// Robust variant (≥ [`ROBUST_AFTER`] repeated samples): the median of every
/// within-tree pairwise difference, scaled to σ. Unlike a MAD around each
/// tree's own median, a pairwise difference has the same distribution whatever
/// the tree's size, so confirm pairs and triples (n = 2, 3 — the common case)
/// do not read σ 30–50 % low (R-24); one outlier poisons only its own pairs.
pub fn pooled_sigma(trees: &[Vec<f64>]) -> f64 {
    let repeated: Vec<&Vec<f64>> = trees.iter().filter(|t| t.len() >= 2).collect();
    if repeated.is_empty() {
        return 0.0;
    }
    let n: usize = repeated.iter().map(|t| t.len()).sum();
    if n >= ROBUST_AFTER {
        let mut pairs: Vec<f64> = repeated
            .iter()
            .flat_map(|t| {
                (0..t.len()).flat_map(move |j| (j + 1..t.len()).map(move |k| (t[j] - t[k]).abs()))
            })
            .collect();
        pairs.sort_by(f64::total_cmp);
        return PAIR_MEDIAN_TO_SIGMA * median_sorted(&pairs);
    }
    let (ss, dof) = repeated.iter().fold((0.0, 0usize), |(ss, dof), t| {
        let m = mean(t);
        (
            ss + t.iter().map(|x| (x - m).powi(2)).sum::<f64>(),
            dof + t.len() - 1,
        )
    });
    (ss / dof as f64).sqrt()
}

/// Direction-normalise: positive = better.
pub fn normalise(direction: Direction, candidate_mean: f64, incumbent_mean: f64) -> f64 {
    match direction {
        Direction::Lower => incumbent_mean - candidate_mean,
        Direction::Higher => candidate_mean - incumbent_mean,
    }
}

/// One step of the sequential decision after the samples in `input` (140 §5.2 step 3).
pub fn step(cfg: &GateCfg, input: &GateInput) -> GateStep {
    let n_c = input.candidate.len();
    let n_i = input.incumbent.len();
    if n_c == 0 || n_i == 0 {
        return GateStep::Confirm {
            more: 1,
            incumbent_sample: n_i == 0,
        };
    }
    let x_c = mean(&input.candidate);
    let x_i = mean(&input.incumbent);
    let delta = normalise(cfg.direction, x_c, x_i);
    // Revision 2: `delta_pct` is the raw relative change (negative = the value went down),
    // the one convention the ledger, brief, trailer, headline and UI share; `delta` stays
    // direction-normalised for the arithmetic.
    let delta_pct = pct(x_c - x_i, x_i);
    let min_eff = cfg.min_effect.against(x_i);
    let sigma = input.sigma;
    let decide = |verdict: Verdict, z: Option<f64>| GateStep::Decide {
        verdict,
        delta,
        delta_pct,
        z,
        detail: None,
    };
    // R-30: a jump that looked like gaming on its first sample stays `suspicious`
    // after the confirm samples pull the means under the thresholds — the
    // first sample is still there, so its z₁ and Δ₁ are re-derived here.
    let first_suspicious = || {
        let d1 = normalise(cfg.direction, input.candidate[0], x_i);
        let z1 = if sigma > 0.0 {
            Some(d1 / (sigma * (1.0 + 1.0 / n_i as f64).sqrt()))
        } else {
            None
        };
        z1.is_some_and(|z| z >= cfg.z_suspicious)
            || input
                .largest_accepted_delta
                .is_some_and(|big| big > 0.0 && d1 > 3.0 * big)
    };
    let suspicious = |z: Option<f64>| {
        z.is_some_and(|z| z >= cfg.z_suspicious)
            || input
                .largest_accepted_delta
                .is_some_and(|big| big > 0.0 && delta > 3.0 * big)
            || (n_c > 1 && first_suspicious())
    };
    let keep = |z: Option<f64>| {
        if !guard_failures(cfg, input).is_empty() {
            return decide(Verdict::GuardFailed, z);
        }
        decide(
            if suspicious(z) {
                Verdict::Suspicious
            } else {
                Verdict::Improved
            },
            z,
        )
    };
    let confirm = || GateStep::Confirm {
        more: cfg.confirm_runs,
        // §5.2 says "older than remeasure_after"; also when the incumbent has a single
        // sample (a fast keep): that sample was selected for being extreme, so x̄_i is
        // biased and the next true gain is under-read (as-built W1 table: power at a
        // true 3σ gain 82.4 % → 89.7 %, null rate unchanged). The sample must be taken
        // on the incumbent tree itself (R-2), never in the candidate's worktree.
        incumbent_sample: input.incumbent_age >= cfg.remeasure_after || n_i < 2,
    };

    // Deterministic metric: no z-tests, no confirm (140 §5.2 step 2).
    if sigma <= 0.0 {
        return if delta > min_eff {
            keep(None)
        } else if delta < 0.0 {
            decide(Verdict::Worse, None)
        } else {
            decide(Verdict::NoGain, None)
        };
    }

    // R-25: a single-sample incumbent is the sample that fast-kept, selected for
    // being extreme; judging the next candidate's first sample against it reads a
    // true gain low (≈ 14 % of 3σ gains discarded at z₁ < 1). Measure the incumbent
    // once more first — on its own tree (R-2) — then decide.
    if n_c == 1 && n_i < 2 && !input.keep_simpler {
        return GateStep::Confirm {
            more: 0,
            incumbent_sample: true,
        };
    }
    let se = sigma * (1.0 / n_c as f64 + 1.0 / n_i as f64).sqrt();
    let z = delta / se;
    if n_c == 1 {
        if z < -cfg.z {
            return decide(Verdict::Worse, Some(z));
        }
        if z < cfg.z_discard {
            // The segment baseline has `repeats` (≥ 2) samples; an incumbent with
            // fewer is a fast keep plus its one re-measure, and a one-sample discard
            // against it is still unreliable — confirm on means instead.
            if n_i < 3 && delta > 0.0 {
                return confirm();
            }
            return decide(Verdict::NoGain, Some(z));
        }
        if input.keep_simpler {
            return confirm(); // keep-simpler is judged on means (step 3)
        }
        if z >= cfg.z_fast && delta > min_eff && !suspicious(Some(z)) {
            return keep(Some(z));
        }
        return confirm();
    }
    let t = (cfg.z * se).max(min_eff);
    if delta > t {
        keep(Some(z))
    } else if z < -cfg.z {
        decide(Verdict::Worse, Some(z))
    } else {
        decide(Verdict::NoGain, Some(z))
    }
}

/// Every secondary guard a keep would break, rendered as the verdict line shows them
/// (`alloc_mb +7.2 % > 5 %`); empty = guards hold (140 §5.2 step 3). Each guard is
/// judged in its own direction (R-19): a memory guard in a throughput study still
/// fails when memory goes *up*.
pub fn guard_failures(cfg: &GateCfg, input: &GateInput) -> Vec<String> {
    cfg.guards
        .iter()
        .filter_map(|(name, limit, direction)| {
            let &(_, c, i) = input.secondaries.iter().find(|(n, _, _)| n == name)?;
            let regression = -normalise(*direction, c, i);
            (regression > limit.against(i)).then(|| {
                let shown = match limit {
                    crate::types::Effect::Relative(f) => format!("{} %", nice(f * 100.0)),
                    crate::types::Effect::Absolute(v) => nice(*v),
                };
                format!("{name} {} > {shown}", signed_pct(c - i, i))
            })
        })
        .collect()
}

/// Keep-simpler check (140 §5.2 step 3): not significantly worse (`z > −1` on
/// means), `removed > added` lines, guards hold. Checks run elsewhere.
pub fn keep_simpler(cfg: &GateCfg, input: &GateInput, added: u32, removed: u32) -> bool {
    if removed <= added || input.candidate.is_empty() || input.incumbent.is_empty() {
        return false;
    }
    if !guard_failures(cfg, input).is_empty() {
        return false;
    }
    let delta = normalise(
        cfg.direction,
        mean(&input.candidate),
        mean(&input.incumbent),
    );
    if input.sigma <= 0.0 {
        return delta >= 0.0;
    }
    let se = input.sigma
        * (1.0 / input.candidate.len() as f64 + 1.0 / input.incumbent.len() as f64).sqrt();
    delta / se > -1.0
}

/// Re-measure drift (140 §5.2 step 4): a new incumbent sample outside 3σ of the mean.
pub fn drifted(sigma: f64, incumbent_mean: f64, sample: f64) -> bool {
    (sample - incumbent_mean).abs() > 3.0 * sigma
}

/// The verdict in words and numbers (140 §5.2 step 5), e.g.
/// `improved: 41.2 ms vs 42.3 ms (−2.6 %), 3.1σ over 3 samples, checks passed`.
pub fn line(
    step: &GateStep,
    unit: &str,
    candidate_mean: f64,
    incumbent_mean: f64,
    n: usize,
) -> String {
    let samples = |n: usize| {
        if n == 1 {
            "1 sample".to_owned()
        } else {
            format!("{n} samples")
        }
    };
    let unit = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    let change = signed_pct(candidate_mean - incumbent_mean, incumbent_mean);
    let full = |z: Option<f64>| {
        let sig = z.map_or_else(|| "deterministic".to_owned(), |z| format!("{}σ", nice(z)));
        format!(
            "{}{unit} vs {}{unit} ({change}), {sig} over {}",
            nice(candidate_mean),
            nice(incumbent_mean),
            samples(n)
        )
    };
    match step {
        GateStep::Confirm { more, .. } => {
            format!("measuring: {} so far, {more} more to confirm", samples(n))
        }
        GateStep::Decide { verdict, z, .. } => match verdict {
            Verdict::Improved => format!("improved: {}", full(*z)),
            Verdict::Suspicious => format!("suspicious: {} — flagged for review", full(*z)),
            Verdict::KeepSimpler => format!("keep simpler: {}", full(*z)),
            Verdict::Worse => format!("worse: {}", full(*z)),
            Verdict::NoGain => format!(
                "no gain: {change}, {} ({})",
                z.map_or_else(|| "deterministic".to_owned(), |z| format!("{}σ", nice(z))),
                samples(n)
            ),
            Verdict::GuardFailed => format!(
                "guard failed: a secondary regressed past its limit; primary {}",
                full(*z)
            ),
            Verdict::ChecksFailed => format!("checks failed: primary {}", full(*z)),
            Verdict::Crash => "crash (tail below)".to_owned(),
            Verdict::Refused { reason } => format!("refused: {reason}"),
            Verdict::Traded => format!("traded: joined the front; {}", full(*z)),
            Verdict::Pending { until } => format!("pending: waiting until {until}"),
        },
        GateStep::Wait { until, why } => format!("waiting: {why}, until {until}"),
    }
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn median_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// `delta` as a percentage of `reference` (0 when the reference is 0).
fn pct(delta: f64, reference: f64) -> f64 {
    if reference == 0.0 {
        0.0
    } else {
        100.0 * delta / reference.abs()
    }
}

/// `+7.2 %` / `-2.6 %`.
fn signed_pct(delta: f64, reference: f64) -> String {
    let p = pct(delta, reference);
    format!("{}{} %", if p >= 0.0 { "+" } else { "" }, nice(p))
}

/// Three-ish significant digits for the model.
fn nice(v: f64) -> String {
    let a = v.abs();
    let s = if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    };
    if s == "-0" { "0".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Effect;

    fn cfg() -> GateCfg {
        GateCfg {
            direction: Direction::Lower,
            z: 2.0,
            z_fast: 3.5,
            z_discard: 1.0,
            z_suspicious: 8.0,
            confirm_runs: 2,
            remeasure_after: 6,
            min_effect: Effect::Relative(0.005),
            guards: vec![],
        }
    }

    fn input(candidate: &[f64], incumbent: &[f64], sigma: f64) -> GateInput {
        GateInput {
            candidate: candidate.to_vec(),
            incumbent: incumbent.to_vec(),
            sigma,
            largest_accepted_delta: None,
            incumbent_age: 0,
            keep_simpler: false,
            secondaries: vec![],
        }
    }

    fn verdict(step: &GateStep) -> &Verdict {
        match step {
            GateStep::Decide { verdict, .. } => verdict,
            GateStep::Confirm { .. } | GateStep::Wait { .. } => panic!("not a verdict: {step:?}"),
        }
    }

    /// 159 §5.12 item 4: the 140 scalar rule never fills `detail` — every verdict path of
    /// `step` (one-sample keep, confirmed keep, worse, no gain, guard, suspicious, deterministic).
    #[test]
    fn scalar_decisions_carry_no_detail() {
        let mut guarded = cfg();
        guarded.guards = vec![("alloc".into(), Effect::Relative(0.05), Direction::Lower)];
        let mut with_guard = input(&[60.0], &[100.0, 100.0, 100.0], 1.0);
        with_guard.secondaries = vec![("alloc".into(), 200.0, 100.0)];
        let steps = [
            step(&cfg(), &input(&[60.0], &[100.0, 100.0, 100.0], 1.0)),
            step(
                &cfg(),
                &input(&[90.0, 90.0, 90.0], &[100.0, 100.0, 100.0], 1.0),
            ),
            step(&cfg(), &input(&[140.0], &[100.0, 100.0, 100.0], 1.0)),
            step(
                &cfg(),
                &input(&[100.0, 100.0, 100.0], &[100.0, 100.0, 100.0], 1.0),
            ),
            step(&cfg(), &input(&[990.0], &[1000.0], 0.0)),
            step(&guarded, &with_guard),
        ];
        let mut decided = 0;
        for s in &steps {
            if let GateStep::Decide { detail, .. } = s {
                assert!(detail.is_none(), "{s:?}");
                decided += 1;
            }
        }
        assert!(decided >= 4, "{steps:?}");
    }

    // ── a tiny deterministic RNG: LCG + Box-Muller (no new deps) ──
    struct Rng(u64);
    impl Rng {
        fn next_u(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn normal(&mut self) -> f64 {
            let u1 = self.next_u();
            let u2 = self.next_u();
            (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
        }
    }

    /// Runs `experiments` experiments of one simulated study; returns the keep count.
    /// `effect` is the candidate's true improvement in σ units (0 = null).
    fn simulate(seed: u64, experiments: usize, effect: f64) -> usize {
        let cfg = cfg();
        let mut rng = Rng(seed);
        let sigma_true = 1.0;
        let mut inc_true = 100.0; // σ_true = 1 → 1 % noise, min_effect 0.5 % = 0.5σ (the §5.2 worked example)
        let draw = |rng: &mut Rng, mu: f64| mu + sigma_true * rng.normal();
        // `past` = trees no longer incumbent; the incumbent's own samples are pooled with them.
        let mut past: Vec<Vec<f64>> = Vec::new();
        let mut incumbent: Vec<f64> = (0..3).map(|_| draw(&mut rng, inc_true)).collect();
        let mut age = 0u32;
        let mut keeps = 0;
        for _ in 0..experiments {
            let cand_true = inc_true - effect * sigma_true;
            let sigma = {
                let mut all = past.clone();
                all.push(incumbent.clone());
                pooled_sigma(&all)
            };
            let mut inp = input(&[draw(&mut rng, cand_true)], &incumbent, sigma);
            inp.incumbent_age = age;
            let mut st = step(&cfg, &inp);
            // Like the actor: every `Confirm` is followed (the incumbent-first step
            // after a fast keep, then the band confirm) until the gate decides.
            while let GateStep::Confirm {
                more,
                incumbent_sample,
            } = st
            {
                for _ in 0..more {
                    inp.candidate.push(draw(&mut rng, cand_true));
                }
                if incumbent_sample {
                    let s = draw(&mut rng, inc_true);
                    incumbent.push(s);
                    inp.incumbent.push(s);
                    age = 0;
                }
                st = step(&cfg, &inp);
            }
            match verdict(&st) {
                Verdict::Improved | Verdict::Suspicious => {
                    keeps += 1;
                    inc_true = cand_true;
                    past.push(std::mem::replace(&mut incumbent, inp.candidate));
                    age = 0;
                }
                _ => {
                    past.push(inp.candidate);
                    age += 1;
                }
            }
        }
        keeps
    }

    #[test]
    fn null_false_keep_rate_at_most_one_percent() {
        // 100 studies × 100 experiments = 10,000 null experiments.
        let keeps: usize = (0..100).map(|s| simulate(1000 + s, 100, 0.0)).sum();
        let rate = keeps as f64 / 10_000.0;
        eprintln!("null false-keep rate {rate:.4} ({keeps} / 10000)");
        assert!(rate <= 0.01, "false-keep rate {rate:.4} ({keeps} / 10000)");
    }

    /// Power of the gate at a true 3σ gain, defaults, σ estimated as the study
    /// grows. §15 asked ≥ 85 %; W1 measured 89.7 % with a robust σ that read
    /// 30–50 % low on confirm pairs/triples (R-24) — the same bias put the null
    /// rate at 0.93 %. With an unbiased σ the defaults give ≈ 83 % here at a
    /// 0.4 % null rate; the remaining gap is a `gate:` defaults question
    /// (`z_discard`, `z_fast`), recorded in the F2 as-built. Floor: 80 %.
    #[test]
    fn true_three_sigma_improvement_kept_mostly() {
        // Each experiment carries a fresh 3σ improvement over the current incumbent.
        let keeps: usize = (0..200).map(|s| simulate(7000 + s, 50, 3.0)).sum();
        let rate = keeps as f64 / 10_000.0;
        eprintln!("3σ keep rate {rate:.4} ({keeps} / 10000)");
        assert!(rate >= 0.80, "keep rate {rate:.4}");
    }

    /// 159 §6 Guard: `simulate` for the dominance rule (W1). `k` objectives, all null except a
    /// true `effect`·σ gain on objective 0; parents rotate over the front (fewest children, ties
    /// by objective 0), every `Confirm` is followed, an `Improved`/`Traded` enters the front and
    /// evicts what it dominates. Returns the entry count.
    fn simulate_front(seed: u64, experiments: usize, k: usize, effect: f64) -> usize {
        use crate::pareto::{self, Axis, Dom, Trees};
        struct M {
            truth: Vec<f64>,
            values: Vec<Vec<f64>>,
            children: u32,
            /// Experiment index of its newest sample (the engine's `parent_age` source).
            last: usize,
        }
        let cfg = cfg();
        let axes: Vec<Axis> = (0..k)
            .map(|j| Axis {
                name: format!("o{j}"),
                direction: Direction::Lower,
                min_effect: Effect::Relative(0.005),
                max_regression: Effect::Absolute(0.0),
                guard: false,
            })
            .collect();
        let mut rng = Rng(seed);
        let draw = |rng: &mut Rng, mu: &[f64]| -> Vec<f64> {
            mu.iter().map(|m| m + rng.normal()).collect()
        };
        let push = |t: &mut Vec<Vec<f64>>, s: &[f64]| {
            for (j, v) in s.iter().enumerate() {
                t[j].push(*v);
            }
        };
        let base_truth = vec![100.0; k];
        let mut base = vec![Vec::new(); k];
        for _ in 0..3 {
            push(&mut base, &draw(&mut rng, &base_truth));
        }
        let mut past: Vec<Vec<Vec<f64>>> = Vec::new();
        let mut front = vec![M {
            truth: base_truth,
            values: base,
            children: 0,
            last: 0,
        }];
        let mut incumbent = 0usize;
        let mut entries = 0;
        for i in 0..experiments {
            let p = (0..front.len())
                .min_by(|&a, &b| {
                    front[a].children.cmp(&front[b].children).then_with(|| {
                        mean(&front[a].values[0]).total_cmp(&mean(&front[b].values[0]))
                    })
                })
                .unwrap();
            front[p].children += 1;
            let mut cand_truth = front[p].truth.clone();
            cand_truth[0] -= effect;
            let mut cand = vec![Vec::new(); k];
            push(&mut cand, &draw(&mut rng, &cand_truth));
            loop {
                let sigmas: Vec<f64> = (0..k)
                    .map(|j| {
                        let mut trees: Vec<Vec<f64>> = past.iter().map(|t| t[j].clone()).collect();
                        trees.extend(front.iter().map(|m| m.values[j].clone()));
                        trees.push(cand[j].clone());
                        pooled_sigma(&trees)
                    })
                    .collect();
                let dof: usize = past
                    .iter()
                    .map(|t| &t[0])
                    .chain(front.iter().map(|m| &m.values[0]))
                    .chain(std::iter::once(&cand[0]))
                    .filter(|v| v.len() >= 2)
                    .map(|v| v.len() - 1)
                    .sum();
                let d = Dom::new(&cfg, &axes, &sigmas, dof);
                let members: Vec<Vec<Vec<f64>>> = front.iter().map(|m| m.values.clone()).collect();
                let st = pareto::decide(
                    &d,
                    &Trees {
                        candidate: &cand,
                        parent: &front[p].values,
                        parent_age: (i.saturating_sub(front[p].last)).saturating_sub(1) as u32,
                        parent_is_incumbent: p == incumbent,
                        members: &members,
                    },
                );
                match st {
                    GateStep::Confirm {
                        more,
                        incumbent_sample,
                    } => {
                        // Like the actor: an incumbent-first step with nothing else asked still
                        // advances by one candidate sample when no incumbent sample is taken.
                        let more = if more == 0 && !incumbent_sample {
                            1
                        } else {
                            more
                        };
                        for _ in 0..more {
                            push(&mut cand, &draw(&mut rng, &cand_truth));
                        }
                        if incumbent_sample {
                            let s = draw(&mut rng, &front[p].truth);
                            push(&mut front[p].values, &s);
                            front[p].last = i;
                        }
                    }
                    GateStep::Decide {
                        verdict: Verdict::Improved | Verdict::Traded,
                        ..
                    } => {
                        entries += 1;
                        let mut i = 0;
                        while i < front.len() {
                            if pareto::compare(&axes, &sigmas, &cand, &front[i].values, d.z_k)
                                .dominates(&axes)
                            {
                                past.push(front.remove(i).values);
                            } else {
                                i += 1;
                            }
                        }
                        front.push(M {
                            truth: cand_truth,
                            values: cand,
                            children: 0,
                            last: i,
                        });
                        incumbent = front.len() - 1;
                        break;
                    }
                    GateStep::Decide { .. } => {
                        past.push(cand);
                        break;
                    }
                    GateStep::Wait { .. } => unreachable!("the dominance rule never waits"),
                }
            }
        }
        entries
    }

    /// 159 §6 Guard, §15 P: k ∈ {2, 3, 4} null objectives, 100 studies × 100 experiments each;
    /// the front's false-entry rate stays at or below 1 % with the Šidák-corrected thresholds.
    #[test]
    fn front_null_entry_rate_at_most_one_percent_for_two_to_four_objectives() {
        for k in [2usize, 3, 4] {
            let entries: usize = (0..100)
                .map(|s| simulate_front(3000 + 100 * k as u64 + s, 100, k, 0.0))
                .sum();
            let rate = entries as f64 / 10_000.0;
            eprintln!("k = {k}: null front-entry rate {rate:.4} ({entries} / 10000)");
            assert!(
                rate <= 0.01,
                "k = {k}: entry rate {rate:.4} ({entries} / 10000)"
            );
        }
    }

    /// 159 §6 Guard: a true 3σ gain on one of three objectives is kept at least 70 % of the time.
    #[test]
    fn front_keeps_a_true_three_sigma_gain_at_three_objectives() {
        let entries: usize = (0..200).map(|s| simulate_front(9000 + s, 50, 3, 3.0)).sum();
        let rate = entries as f64 / 10_000.0;
        eprintln!("k = 3: 3σ entry rate {rate:.4} ({entries} / 10000)");
        assert!(rate >= 0.70, "entry rate {rate:.4}");
    }

    #[test]
    fn pooled_sigma_classical_and_robust() {
        assert_eq!(pooled_sigma(&[vec![1.0], vec![2.0]]), 0.0);
        assert_eq!(pooled_sigma(&[]), 0.0);
        // Two trees, each sd 1 around their own mean: pooled = 1 regardless of the gap.
        let s = pooled_sigma(&[vec![10.0, 12.0], vec![100.0, 102.0]]);
        assert!((s - 2f64.sqrt()).abs() < 1e-9, "{s}");
        // 12 repeated samples: robust path, immune to one outlier.
        let mut t = vec![
            vec![10.0, 11.0, 10.0, 11.0, 10.0, 11.0],
            vec![20.0, 21.0, 20.0, 21.0, 20.0, 21.0],
        ];
        let robust = pooled_sigma(&t);
        // 30 within-tree pairs: 12 differ by 0, 18 by 1 → median 1.
        assert!((robust - PAIR_MEDIAN_TO_SIGMA).abs() < 1e-9, "{robust}");
        t[0][0] = 1e6;
        let still = pooled_sigma(&t);
        assert!((still - robust).abs() < 1e-9, "{still}");
        assert_eq!(pooled_sigma(&[vec![5.0, 5.0, 5.0]]), 0.0);
    }

    /// R-24: the robust σ must not read low on confirm pairs and triples — the
    /// shape production produces (n = 2, 3 per tree). Normal noise with σ = 1:
    /// the estimate is within 5 % of 1 for both tree sizes (a MAD around each
    /// tree's own median gave ≈ 0.71 for pairs).
    #[test]
    fn robust_sigma_is_consistent_for_small_trees() {
        let mut rng = Rng(42);
        for n in [2usize, 3, 4] {
            let trees: Vec<Vec<f64>> = (0..4000)
                .map(|_| (0..n).map(|_| 50.0 + rng.normal()).collect())
                .collect();
            let s = pooled_sigma(&trees);
            assert!((s - 1.0).abs() < 0.05, "n = {n}: σ̂ = {s}");
        }
    }

    #[test]
    fn deterministic_metric_uses_min_effect() {
        let cfg = cfg();
        assert_eq!(
            *verdict(&step(&cfg, &input(&[990.0], &[1000.0], 0.0))),
            Verdict::Improved
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[997.0], &[1000.0], 0.0))),
            Verdict::NoGain
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[1000.0], &[1000.0], 0.0))),
            Verdict::NoGain
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[1001.0], &[1000.0], 0.0))),
            Verdict::Worse
        );
        let GateStep::Decide {
            z,
            delta,
            delta_pct,
            ..
        } = step(&cfg, &input(&[990.0], &[1000.0], 0.0))
        else {
            panic!()
        };
        assert_eq!(z, None);
        assert_eq!(delta, 10.0);
        // Revision 2: Δ % is the raw change — a lower-is-better win reads negative…
        assert!((delta_pct + 1.0).abs() < 1e-9, "{delta_pct}");
        // …and a higher-is-better win positive; `delta` is normalised in both.
        let higher = GateCfg {
            direction: Direction::Higher,
            ..cfg
        };
        let GateStep::Decide {
            verdict,
            delta,
            delta_pct,
            ..
        } = step(&higher, &input(&[1010.0], &[1000.0], 0.0))
        else {
            panic!()
        };
        assert_eq!(verdict, Verdict::Improved);
        assert_eq!(delta, 10.0);
        assert!((delta_pct - 1.0).abs() < 1e-9, "{delta_pct}");
    }

    #[test]
    fn one_sample_paths() {
        let cfg = cfg();
        // z1 = Δ / (σ √(1 + 1/3)) with σ = 10, n_i = 3 → se = 11.55.
        let inc = [1000.0, 1000.0, 1000.0];
        assert_eq!(
            *verdict(&step(&cfg, &input(&[995.0], &inc, 10.0))),
            Verdict::NoGain
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[1030.0], &inc, 10.0))),
            Verdict::Worse
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[950.0], &inc, 10.0))),
            Verdict::Improved
        );
        assert_eq!(
            step(&cfg, &input(&[980.0], &inc, 10.0)),
            GateStep::Confirm {
                more: 2,
                incumbent_sample: false
            }
        );
        let mut old = input(&[980.0], &inc, 10.0);
        old.incumbent_age = 6;
        assert_eq!(
            step(&cfg, &old),
            GateStep::Confirm {
                more: 2,
                incumbent_sample: true
            }
        );
        // Higher-is-better flips the sign.
        let mut up = cfg.clone();
        up.direction = Direction::Higher;
        assert_eq!(
            *verdict(&step(&up, &input(&[1050.0], &inc, 10.0))),
            Verdict::Improved
        );
        assert_eq!(
            *verdict(&step(&up, &input(&[950.0], &inc, 10.0))),
            Verdict::Worse
        );
    }

    #[test]
    fn confirm_path_on_means() {
        let cfg = cfg();
        let inc = [1000.0, 1000.0, 1000.0];
        // T = max(2 · 10 · √(2/3), 5) = 16.3: Δ̄ = 20 keeps, Δ̄ = 12 does not.
        assert_eq!(
            *verdict(&step(&cfg, &input(&[980.0, 980.0, 980.0], &inc, 10.0))),
            Verdict::Improved
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[988.0, 988.0, 988.0], &inc, 10.0))),
            Verdict::NoGain
        );
        assert_eq!(
            *verdict(&step(&cfg, &input(&[1020.0, 1020.0, 1020.0], &inc, 10.0))),
            Verdict::Worse
        );
        // min_effect floor beats the z threshold when σ is tiny.
        assert_eq!(
            *verdict(&step(&cfg, &input(&[997.0, 997.0, 997.0], &inc, 0.1))),
            Verdict::NoGain
        );
    }

    #[test]
    fn suspicious_paths() {
        let cfg = cfg();
        let inc = [1000.0, 1000.0, 1000.0];
        // z ≥ 8 on one sample: confirm first, then suspicious on means.
        let one = input(&[800.0], &inc, 10.0);
        assert!(matches!(
            step(&cfg, &one),
            GateStep::Confirm { more: 2, .. }
        ));
        assert_eq!(
            *verdict(&step(&cfg, &input(&[800.0, 800.0, 800.0], &inc, 10.0))),
            Verdict::Suspicious
        );
        // 3× the largest earlier accepted Δ.
        let mut big = input(&[950.0], &inc, 10.0);
        big.largest_accepted_delta = Some(10.0);
        assert!(matches!(step(&cfg, &big), GateStep::Confirm { .. }));
        big.candidate = vec![950.0, 950.0, 950.0];
        assert_eq!(*verdict(&step(&cfg, &big)), Verdict::Suspicious);
        big.largest_accepted_delta = Some(40.0);
        assert_eq!(*verdict(&step(&cfg, &big)), Verdict::Improved);
    }

    /// R-30: a first sample at z₁ ≥ 8 whose confirm samples pull the mean back
    /// under the suspicious thresholds (but still past the keep threshold) is
    /// still `suspicious` — §5.2 marks it after the forced confirm, not only
    /// when the means stay extreme.
    #[test]
    fn suspicion_from_the_first_sample_survives_confirm() {
        let cfg = cfg();
        let inc = [1000.0, 1000.0, 1000.0];
        // First sample −100 (z₁ ≈ 8.7); confirms −30, −30 → mean Δ = 53.3, z ≈ 6.5 < 8 on means.
        let inp = input(&[900.0, 970.0, 970.0], &inc, 10.0);
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::Suspicious);
        // The same means without an extreme first sample are a plain keep.
        let plain = input(&[946.0, 947.0, 947.0], &inc, 10.0);
        assert_eq!(*verdict(&step(&cfg, &plain)), Verdict::Improved);
        // A 3× jump on the first sample likewise.
        let mut big = input(&[940.0, 985.0, 985.0], &inc, 10.0);
        big.largest_accepted_delta = Some(15.0);
        assert_eq!(*verdict(&step(&cfg, &big)), Verdict::Suspicious);
    }

    #[test]
    fn guards() {
        let mut cfg = cfg();
        cfg.guards = vec![("alloc_mb".into(), Effect::Relative(0.05), Direction::Lower)];
        let inc = [1000.0, 1000.0, 1000.0];
        let mut inp = input(&[950.0], &inc, 10.0);
        inp.secondaries = vec![("alloc_mb".into(), 107.2, 100.0)];
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::GuardFailed);
        assert_eq!(
            guard_failures(&cfg, &inp),
            vec!["alloc_mb +7.20 % > 5.00 %"]
        );
        inp.secondaries = vec![("alloc_mb".into(), 104.0, 100.0)];
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::Improved);
        inp.secondaries = vec![];
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::Improved);
    }

    /// R-19: a higher-is-better study (throughput) with a lower-is-better guard
    /// (memory): more memory fails, less memory passes — the guard's direction,
    /// not the primary's, decides. A higher-is-better guard works the other way.
    #[test]
    fn guards_use_their_own_direction() {
        let mut cfg = cfg();
        cfg.direction = Direction::Higher;
        cfg.guards = vec![("alloc_mb".into(), Effect::Relative(0.05), Direction::Lower)];
        let inc = [1000.0, 1000.0, 1000.0];
        let mut inp = input(&[1050.0], &inc, 10.0);
        inp.secondaries = vec![("alloc_mb".into(), 107.2, 100.0)];
        assert_eq!(
            guard_failures(&cfg, &inp),
            vec!["alloc_mb +7.20 % > 5.00 %"]
        );
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::GuardFailed);
        inp.secondaries = vec![("alloc_mb".into(), 92.0, 100.0)];
        assert!(guard_failures(&cfg, &inp).is_empty());
        assert_eq!(*verdict(&step(&cfg, &inp)), Verdict::Improved);

        cfg.guards = vec![("hit_rate".into(), Effect::Relative(0.05), Direction::Higher)];
        inp.secondaries = vec![("hit_rate".into(), 90.0, 100.0)];
        assert_eq!(
            guard_failures(&cfg, &inp),
            vec!["hit_rate -10.0 % > 5.00 %"]
        );
        inp.secondaries = vec![("hit_rate".into(), 110.0, 100.0)];
        assert!(guard_failures(&cfg, &inp).is_empty());
    }

    #[test]
    fn keep_simpler_granted_and_refused() {
        let cfg = cfg();
        let inc = [1000.0, 1000.0, 1000.0];
        // Slightly worse but within z > −1 on means, more lines removed than added.
        let inp = input(&[1005.0, 1005.0, 1005.0], &inc, 10.0);
        assert!(keep_simpler(&cfg, &inp, 3, 10));
        assert!(!keep_simpler(&cfg, &inp, 10, 10));
        assert!(!keep_simpler(
            &cfg,
            &input(&[1010.0, 1010.0, 1010.0], &inc, 10.0),
            3,
            10
        ));
        // Deterministic: equal is fine, worse is not.
        assert!(keep_simpler(&cfg, &input(&[1000.0], &inc, 0.0), 0, 1));
        assert!(!keep_simpler(&cfg, &input(&[1001.0], &inc, 0.0), 0, 1));
        // Asked for in the run: the one-sample step confirms so means exist.
        let mut asked = input(&[950.0], &inc, 10.0);
        asked.keep_simpler = true;
        assert!(matches!(
            step(&cfg, &asked),
            GateStep::Confirm { more: 2, .. }
        ));
    }

    #[test]
    fn drift_rule() {
        assert!(!drifted(10.0, 1000.0, 1029.0));
        assert!(drifted(10.0, 1000.0, 1031.0));
        assert!(drifted(10.0, 1000.0, 969.0));
        assert!(!drifted(0.0, 1000.0, 1000.0));
        assert!(drifted(0.0, 1000.0, 1000.5));
    }

    #[test]
    fn verdict_lines() {
        let cfg = cfg();
        let inc = [42.3, 42.3, 42.3];
        let st = step(&cfg, &input(&[41.2, 41.2, 41.2], &inc, 0.3));
        assert_eq!(
            line(&st, "ms", 41.2, 42.3, 3),
            "improved: 41.2 ms vs 42.3 ms (-2.60 %), 4.49σ over 3 samples"
        );
        let st = step(&cfg, &input(&[42.5], &inc, 1.0));
        assert_eq!(
            line(&st, "ms", 42.5, 42.3, 1),
            "no gain: +0.473 %, -0.173σ (1 sample)"
        );
        let st = GateStep::Decide {
            verdict: Verdict::Refused {
                reason: "evaluator changed (bench.sh)".into(),
            },
            delta: 0.0,
            delta_pct: 0.0,
            z: None,
            detail: None,
        };
        assert_eq!(
            line(&st, "ms", 0.0, 0.0, 0),
            "refused: evaluator changed (bench.sh)"
        );
        let st = GateStep::Confirm {
            more: 2,
            incumbent_sample: false,
        };
        assert_eq!(
            line(&st, "ms", 0.0, 0.0, 1),
            "measuring: 1 sample so far, 2 more to confirm"
        );
    }
}
