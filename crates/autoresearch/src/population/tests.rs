//! 159 §9 P: cell keys, parent weights, acceptance, migration offers, the setup grammar; the
//! null-champion and deceptive-landscape simulations on the pure model with the real gate; one
//! wave of a population study on `fixtures/toy-route` with real git and a scripted bench.

use std::collections::{HashMap, VecDeque};
use std::process::Command;
use std::sync::Arc;

use parking_lot::Mutex;
use runtime::SessionId;

use super::*;
use crate::actor::Backend;
use crate::config::AgentDefaults;
use crate::wave::{register_child, slot_of};
use crate::{Studies, binding, gate, tools};

fn d(name: &str, bins: &[f64]) -> Descriptor {
    Descriptor {
        source: DescriptorSource::Secondary,
        name: name.into(),
        bins: bins.to_vec(),
    }
}

fn spec_with(islands: u8, descriptors: Vec<Descriptor>) -> StudySpec {
    StudySpec {
        metric: MetricSpec {
            name: "length".into(),
            unit: String::new(),
            direction: Direction::Lower,
            min_effect: "1%".into(),
        },
        parallel: 3,
        axes: Axes {
            search: Search::Population,
            ..Axes::default()
        },
        population: Some(PopulationSpec {
            islands,
            descriptors,
            ..PopulationSpec::default()
        }),
        ..StudySpec::default()
    }
}

fn sample(value: f64, secondaries: &[(&str, f64)]) -> Sample {
    Sample {
        value: Some(value),
        secondaries: secondaries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
        ..Sample::default()
    }
}

fn elite(key: &str, n: u32, score: f64) -> Elite {
    Elite {
        key: key.into(),
        island: island_of(key).unwrap(),
        sha: format!("t{n}"),
        n,
        score,
        descriptor: BTreeMap::new(),
        children: 0,
    }
}

// ───────────────────────────── cells ─────────────────────────────

#[test]
fn bins_keys_and_islands() {
    let edges = [1.0, 10.0, 100.0];
    assert_eq!(bin(0.5, &edges), 0);
    assert_eq!(bin(1.0, &edges), 1);
    assert_eq!(bin(50.0, &edges), 2);
    assert_eq!(bin(1e9, &edges), 3);
    assert_eq!(bin(f64::NAN, &edges), 0);
    assert_eq!(cell_key(2, &[1, 3]), "i2:1.3");
    assert_eq!(cell_key(0, &[4]), "i0:4");
    assert_eq!(cell_key(1, &[]), "i1:0");
    assert_eq!(island_of("i3:1.2"), Some(3));
    assert_eq!(island_of("f1"), None);
    assert_eq!(rekey("i0:1.2", 2), "i2:1.2");
    // The ref form C-f writes (`:` → `/`) decodes back to the key.
    assert_eq!(
        refs::name("s", RefKind::Cell, "i2:1.3"),
        "refs/autoresearch/s/cell/i2/1.3"
    );
}

#[test]
fn cfg_resolves_defaults_and_caps_descriptors() {
    let mut s = spec_with(
        0,
        vec![d("a", &[3.0, 1.0, 1.0]), d("b", &[1.0]), d("c", &[1.0])],
    );
    s.parallel = 7;
    let c = cfg(&s, 5);
    assert_eq!(c.islands, 4, "parallel clamped to 2–4");
    assert_eq!(c.descriptors.len(), 2, "at most two");
    assert_eq!(c.descriptors[0].bins, vec![1.0, 3.0], "sorted, deduped");
    assert_eq!((c.migrate_every, c.remeasure_after), (4, 5));
    s.population = Some(PopulationSpec {
        islands: 3,
        migrate_every: 2,
        remeasure_after: 9,
        ..PopulationSpec::default()
    });
    let c = cfg(&s, 5);
    assert_eq!((c.islands, c.migrate_every, c.remeasure_after), (3, 2, 9));
    assert!(c.descriptors.is_empty());
}

#[test]
fn descriptor_values_come_from_secondaries_asi_and_cases() {
    let mut s = spec_with(
        2,
        vec![
            d("time_ms", &[10.0]),
            Descriptor {
                source: DescriptorSource::Asi,
                name: "depth".into(),
                bins: vec![2.0],
            },
        ],
    );
    let c = cfg(&s, 0);
    let mut a = sample(100.0, &[("time_ms", 20.0)]);
    a.asi.insert("depth".into(), " 3 ".into());
    let mut b = sample(102.0, &[("time_ms", 40.0)]);
    b.asi.insert("depth".into(), "1".into());
    let mut noisy = sample(1.0, &[("time_ms", 1e6)]);
    noisy.noisy = true;
    let v = descriptor_values(&c, &s.metric, &[a, b, noisy], &BTreeMap::new());
    assert_eq!(v["time_ms"], 30.0, "noisy samples are left out");
    assert_eq!(v["depth"], 2.0);
    assert_eq!(bins_of(&c, &v), vec![1, 1]);
    // The primary itself as a descriptor, and a descriptor nobody reports (bin 0).
    s.population.as_mut().unwrap().descriptors = vec![d("length", &[101.0]), d("nope", &[1.0])];
    let c = cfg(&s, 0);
    let v = descriptor_values(
        &c,
        &s.metric,
        &[sample(100.0, &[]), sample(104.0, &[])],
        &BTreeMap::new(),
    );
    assert_eq!(v.get("length"), Some(&102.0));
    assert_eq!(v.get("nope"), None);
    assert_eq!(bins_of(&c, &v), vec![1, 0]);
    // case_best: the case (by sorted id) improved most over the base.
    s.population.as_mut().unwrap().descriptors = vec![Descriptor {
        source: DescriptorSource::CaseBest,
        name: String::new(),
        bins: vec![1.0, 2.0],
    }];
    let c = cfg(&s, 0);
    let mut x = sample(100.0, &[]);
    x.cases = BTreeMap::from([("a".into(), 10.0), ("b".into(), 5.0), ("c".into(), 9.0)]);
    let base = BTreeMap::from([("a".into(), 10.0), ("b".into(), 10.0), ("c".into(), 12.0)]);
    let v = descriptor_values(&c, &s.metric, &[x], &base);
    assert_eq!(v["case_best"], 1.0, "b improved by 5 (lower is better)");
    assert_eq!(bins_of(&c, &v), vec![1]);
}

// ───────────────────────────── the model ─────────────────────────────

#[test]
fn rank_weights_are_logistic_and_children_halve_them() {
    assert_eq!(rank_weight(0, 1), 1.0);
    let n = 5;
    assert!(rank_weight(0, n) > 0.9, "{}", rank_weight(0, n));
    assert!((rank_weight(2, n) - 0.5).abs() < 1e-12);
    assert!(rank_weight(4, n) < 0.1);
    for r in 1..n {
        assert!(rank_weight(r, n) < rank_weight(r - 1, n));
    }
    let mut a = Archive::new(1);
    a.cells = vec![
        elite("i0:0", 1, 100.0),
        elite("i0:1", 2, 90.0),
        elite("i0:2", 3, 110.0),
    ];
    a.cells[1].children = 1;
    let w = a.weights(0, Direction::Lower);
    assert_eq!(
        w.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![1, 0, 2],
        "best first"
    );
    assert!(
        (w[0].1 - rank_weight(0, 3) / 2.0).abs() < 1e-12,
        "one child halves"
    );
    assert!((w[1].1 - 0.5).abs() < 1e-12);
    assert_eq!(a.champion(Direction::Lower).unwrap().n, 2);
    assert_eq!(a.champion(Direction::Higher).unwrap().n, 3);
}

#[test]
fn pick_parent_is_seeded_counts_children_and_favours_the_top() {
    let mut a = Archive::new(1);
    a.cells = vec![
        elite("i0:0", 1, 100.0),
        elite("i0:1", 2, 90.0),
        elite("i0:2", 3, 110.0),
    ];
    let mut r1 = Rng::seeded("s/7");
    let mut r2 = Rng::seeded("s/7");
    let p1 = a.clone().pick_parent(0, Direction::Lower, &mut r1);
    let p2 = a.clone().pick_parent(0, Direction::Lower, &mut r2);
    assert_eq!(p1, p2, "the same seed picks the same parent");
    assert_eq!(
        a.pick_parent(9, Direction::Lower, &mut r1),
        None,
        "empty island"
    );
    let mut counts = [0u32; 3];
    let mut rng = Rng::seeded("many");
    for _ in 0..3000 {
        let mut b = a.clone();
        let i = b.pick_parent(0, Direction::Lower, &mut rng).unwrap();
        assert_eq!(b.cells[i].children, 1);
        counts[i] += 1;
    }
    // s = [0.88, 0.5, 0.12] (n = 3) with a 10 % uniform share: ≈ 62 %, 36 %, 11 %.
    assert!(counts[1] > counts[0] && counts[0] > counts[2], "{counts:?}");
    assert!(
        counts[2] > 150,
        "the uniform share keeps the worst elite alive: {counts:?}"
    );
}

#[test]
fn accept_enters_replaces_and_rejects() {
    let mut a = Archive::new(2);
    assert_eq!(
        a.accept(elite("i0:1", 1, 100.0), |_| false),
        Outcome::Entered
    );
    assert_eq!(
        a.accept(elite("i0:1", 2, 95.0), |h| {
            assert_eq!(h.n, 1);
            true
        }),
        Outcome::Replaced { n: 1 }
    );
    assert_eq!(a.get("i0:1").unwrap().n, 2);
    assert_eq!(
        a.accept(elite("i0:1", 3, 99.0), |_| false),
        Outcome::Rejected("cell i0:1 held by #2".into())
    );
    assert_eq!(
        a.accept(elite("i1:1", 4, 120.0), |_| false),
        Outcome::Entered
    );
    assert_eq!(a.cells.len(), 2);
}

#[test]
fn migration_offers_rekey_each_islands_best_to_the_next() {
    let mut a = Archive::new(3);
    a.cells = vec![
        elite("i0:0", 0, 100.0),
        elite("i0:1", 4, 80.0),
        elite("i1:0", 0, 100.0),
        elite("i2:2", 7, 95.0),
    ];
    a.cells[1].children = 5;
    let offers = a.offers(Direction::Lower);
    let keys: Vec<(u32, String, u8, u32)> = offers
        .iter()
        .map(|(_, e)| (e.n, e.key.clone(), e.island, e.children))
        .collect();
    assert_eq!(
        keys,
        vec![
            (4, "i1:1".into(), 1, 0),
            (0, "i2:0".into(), 2, 0),
            (7, "i0:2".into(), 0, 0)
        ]
    );
    assert!(Archive::new(1).offers(Direction::Lower).is_empty());
}

// ───────────────────────────── setup, card ─────────────────────────────

#[test]
fn setup_grammar_round_trips_and_refuses_bad_input() {
    let ds = parse_descriptors("peak_kb: 100, 200, 400; asi:depth: 2, 4").unwrap();
    assert_eq!(ds.len(), 2);
    assert_eq!(
        (ds[0].source, ds[0].name.as_str(), ds[0].bins.len()),
        (DescriptorSource::Secondary, "peak_kb", 3)
    );
    assert_eq!(
        (ds[1].source, ds[1].name.as_str()),
        (DescriptorSource::Asi, "depth")
    );
    let c = parse_descriptors("case: 2, 4").unwrap();
    assert_eq!(c[0].source, DescriptorSource::CaseBest);
    assert!(
        parse_descriptors("a: 1; b: 2; c: 3")
            .unwrap_err()
            .contains("two")
    );
    assert!(parse_descriptors("a: 3, 1").unwrap_err().contains("ascend"));
    assert!(
        parse_descriptors("a: x")
            .unwrap_err()
            .contains("not a number")
    );
    assert!(
        parse_descriptors("nothing")
            .unwrap_err()
            .contains("expected")
    );

    let mut s = StudySpec::default();
    assert_eq!(setup_value(&s, "population"), "", "no block, no value");
    setup_set(&mut s, "population.islands", "3").unwrap();
    setup_set(&mut s, "population.descriptors", "time_ms: 1, 10, 100").unwrap();
    setup_set(&mut s, "population.migrate_every", "2").unwrap();
    assert!(setup_set(&mut s, "population.islands", "9").is_err());
    assert!(setup_set(&mut s, "population.nope", "1").is_err());
    assert_eq!(
        setup_value(&s, "population"),
        "3 islands · cells over `time_ms` [1, 10, 100] · migrate every 2 waves"
    );
    assert_eq!(
        card_line(&s),
        "Population search: 3 islands, cells over `time_ms`; the champion is re-measured before it counts."
    );
    assert_eq!(SETUP_ROWS[0].row, "population");
    // The spec still writes no 159 key when the block is absent, and round-trips with it.
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"population\"") && json.contains("\"time_ms\""));
    let back: StudySpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
}

// ───────────────────────────── simulations ─────────────────────────────

fn sim_cfg() -> GateCfg {
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

/// Samples per tree (the ledger's `samples_of_tree`) for the simulations.
#[derive(Default)]
struct Trees(HashMap<String, Vec<f64>>);

impl Trees {
    fn sigma(&self) -> f64 {
        gate::pooled_sigma(&self.0.values().cloned().collect::<Vec<_>>())
    }
}

/// The 140 gate between two trees, following every `Confirm` like the actor's measure loop
/// (extra candidate draws, one reference draw when asked, at most `1 + 2·confirm_runs + 1`
/// candidate samples, then a decision on what was measured — an undecided gate is no gain);
/// `true` = the candidate is a keep candidate.
fn gate_beats(
    cfg: &GateCfg,
    rng: &mut Rng,
    trees: &mut Trees,
    cand: &str,
    cand_true: f64,
    reference: &str,
    ref_true: f64,
) -> bool {
    let draw = |rng: &mut Rng, mu: f64| mu + rng.normal();
    let max_samples = 1 + 2 * usize::from(cfg.confirm_runs) + 1;
    let mut incumbent_taken = 0usize;
    loop {
        let input = GateInput {
            candidate: trees.0[cand].clone(),
            incumbent: trees.0[reference].clone(),
            sigma: trees.sigma(),
            largest_accepted_delta: None,
            incumbent_age: 0,
            keep_simpler: false,
            secondaries: vec![],
        };
        let at_cap = trees.0[cand].len() >= max_samples;
        match gate::step(cfg, &input) {
            GateStep::Decide { verdict, .. } => return verdict.is_keep_candidate(),
            GateStep::Wait { .. } => return false,
            GateStep::Confirm { .. } if at_cap => return false,
            GateStep::Confirm {
                more,
                incumbent_sample,
            } => {
                let take_incumbent =
                    incumbent_sample && incumbent_taken < 1 + usize::from(cfg.confirm_runs);
                let more = if more == 0 && !take_incumbent {
                    1
                } else {
                    more
                };
                for _ in 0..more {
                    let v = draw(rng, cand_true);
                    trees.0.get_mut(cand).unwrap().push(v);
                }
                if take_incumbent {
                    incumbent_taken += 1;
                    let v = draw(rng, ref_true);
                    trees.0.get_mut(reference).unwrap().push(v);
                }
            }
        }
    }
}

/// 159 §9 Guard P1: a null population (every child's true effect 0) over 200 experiments, then
/// the champion re-measured the way finalize's `confirm` does it — `repeats` fresh samples per
/// side, σ pooled from those two trees — and judged (a) as the CI alone would, a gain past
/// ±1.96 σ, and (b) by `finalize::confirmed` (R-W4-1: the one-sided t at the realised dof).
/// Returns `(ci_gain, confirmed)` for this run.
fn null_run(seed: u64) -> (bool, bool) {
    let cfg = sim_cfg();
    let mut rng = Rng(seed);
    let base_true = 100.0;
    let edges = [1.0, 2.0, 3.0];
    let mut trees = Trees::default();
    trees.0.insert(
        "base".into(),
        (0..3).map(|_| base_true + rng.normal()).collect(),
    );
    let mut a = Archive::new(2);
    for island in 0..2 {
        let mut e = elite(&cell_key(island, &[0]), 0, 100.0);
        e.sha = "base".into();
        e.score = mean(&trees.0["base"]);
        a.cells.push(e);
    }
    for n in 1..=200u32 {
        let island = (n % 2) as u8;
        let Some(p) = a.pick_parent(island, Direction::Lower, &mut rng) else {
            continue;
        };
        let _parent = a.cells[p].clone();
        let sha = format!("t{n}");
        trees.0.insert(sha.clone(), vec![base_true + rng.normal()]);
        let b = bin(rng.uniform() * 4.0, &edges);
        let key = cell_key(island, &[b]);
        let holder = a.get(&key).cloned();
        let beats = match &holder {
            None => true,
            Some(h) => gate_beats(
                &cfg, &mut rng, &mut trees, &sha, base_true, &h.sha, base_true,
            ),
        };
        if beats {
            let mut child = elite(&key, n, 0.0);
            child.sha = sha.clone();
            child.score = mean(&trees.0[&sha]);
            a.accept(child, |_| true);
        }
    }
    // Every elite's recorded mean is the lucky draw that won its cell: the champion looks
    // best of ~200 null trees. The claim is made only on fresh samples.
    let champ = a.champion(Direction::Lower).unwrap().clone();
    assert!(
        champ.score < base_true,
        "the archive's champion is a lucky draw: {}",
        champ.score
    );
    let repeats = 3;
    let b: Vec<f64> = (0..repeats).map(|_| base_true + rng.normal()).collect();
    let f: Vec<f64> = (0..repeats).map(|_| base_true + rng.normal()).collect();
    let (nb, nf) = (b.len(), f.len());
    let sigma = gate::pooled_sigma(&[b.clone(), f.clone()]);
    let se = sigma * (1.0 / nb as f64 + 1.0 / nf as f64).sqrt();
    let t = (mean(&b) - mean(&f)) / se;
    (
        t > 1.96,
        crate::finalize::confirmed(
            Direction::Lower,
            mean(&b),
            mean(&f),
            sigma,
            nb as u32,
            nf as u32,
        ),
    )
}

/// The quantile the rule leans on (`evalcore::gate::t_quantile`, W2) against the table.
#[test]
fn t_quantile_matches_the_table() {
    use evalcore::gate::t_quantile;
    for (dof, p, want) in [
        (4, 0.99, 3.747),
        (10, 0.99, 2.764),
        (30, 0.99, 2.457),
        (4, 0.975, 2.776),
    ] {
        let got = t_quantile(f64::from(dof), p);
        assert!(
            (got - want).abs() < 0.01,
            "t({dof}, {p}) = {got}, table {want}"
        );
    }
}

/// R-W4-1: the champion's claim on finalize's fresh samples (`repeats` = 3 per side, 4 dof). A
/// gain past the ±1.96 σ CI alone would claim well above 1 % of null studies; `finalize::confirmed`
/// (the one-sided t at the realised dof, ≈ 3.75 at 4 dof) claims ≤ 1 %.
#[test]
fn null_population_champion_confirmed_at_most_one_percent_under_the_t_rule() {
    let runs = 1000usize;
    let (ci_gain, confirmed) = (0..runs as u64)
        .map(|s| null_run(5000 + s))
        .fold((0usize, 0usize), |(c, t), (a, b)| {
            (c + a as usize, t + b as usize)
        });
    eprintln!("null champion: CI gain {ci_gain} / {runs}, confirmed (t rule) {confirmed} / {runs}");
    assert!(
        ci_gain * 100 > runs,
        "the CI alone is not a guard: {ci_gain} / {runs}"
    );
    assert!(confirmed * 100 <= runs, "{confirmed} / {runs}");
}

/// The deceptive landscape (159 §9 Guard P2): three behaviours — the base (mean 100, fast),
/// step one (mean 102 = 2σ worse, slow), step two (mean 90 = 10σ better, slow). A child of the
/// base is step one with probability ½ (else a null variant of the base); a child of step one
/// is step two with probability ½. The descriptor is the run time: the base's cell and the slow
/// cell, so step one enters an empty cell and step two replaces it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Base,
    One,
    Two,
}

impl Step {
    fn mean(self) -> f64 {
        match self {
            Self::Base => 100.0,
            Self::One => 102.0,
            Self::Two => 90.0,
        }
    }
    fn time_ms(self) -> f64 {
        match self {
            Self::Base => 1.0,
            _ => 50.0,
        }
    }
    fn child(self, rng: &mut Rng) -> Self {
        match self {
            Self::Base if rng.uniform() < 0.5 => Self::One,
            Self::One if rng.uniform() < 0.5 => Self::Two,
            s => s,
        }
    }
}

fn deceptive_archive(seed: u64, experiments: u32) -> bool {
    let cfg = sim_cfg();
    let mut rng = Rng(seed);
    let edges = [10.0];
    let mut trees = Trees::default();
    let mut state: HashMap<String, Step> = HashMap::new();
    trees.0.insert(
        "base".into(),
        (0..3).map(|_| Step::Base.mean() + rng.normal()).collect(),
    );
    state.insert("base".into(), Step::Base);
    let mut a = Archive::new(1);
    let mut e = elite(&cell_key(0, &[bin(Step::Base.time_ms(), &edges)]), 0, 0.0);
    e.sha = "base".into();
    e.score = mean(&trees.0["base"]);
    a.cells.push(e);
    for n in 1..=experiments {
        let p = a.pick_parent(0, Direction::Lower, &mut rng).unwrap();
        let parent = a.cells[p].clone();
        let step = state[&parent.sha].child(&mut rng);
        let sha = format!("t{n}");
        trees
            .0
            .insert(sha.clone(), vec![step.mean() + rng.normal()]);
        state.insert(sha.clone(), step);
        let key = cell_key(0, &[bin(step.time_ms(), &edges)]);
        let beats = match a.get(&key).cloned() {
            None => true,
            Some(h) => {
                let ht = state[&h.sha].mean();
                gate_beats(&cfg, &mut rng, &mut trees, &sha, step.mean(), &h.sha, ht)
            }
        };
        if beats {
            let mut child = elite(&key, n, 0.0);
            child.sha = sha.clone();
            child.score = mean(&trees.0[&sha]);
            a.accept(child, |_| true);
        }
    }
    let champ = a.champion(Direction::Lower).unwrap();
    state[&champ.sha] == Step::Two
}

fn deceptive_ratchet(seed: u64, experiments: u32) -> bool {
    let cfg = sim_cfg();
    let mut rng = Rng(seed);
    let mut trees = Trees::default();
    let mut inc = ("base".to_owned(), Step::Base);
    trees.0.insert(
        "base".into(),
        (0..3).map(|_| Step::Base.mean() + rng.normal()).collect(),
    );
    for n in 1..=experiments {
        let step = inc.1.child(&mut rng);
        let sha = format!("t{n}");
        trees
            .0
            .insert(sha.clone(), vec![step.mean() + rng.normal()]);
        if gate_beats(
            &cfg,
            &mut rng,
            &mut trees,
            &sha,
            step.mean(),
            &inc.0,
            inc.1.mean(),
        ) {
            inc = (sha, step);
        }
    }
    inc.1 == Step::Two
}

#[test]
fn deceptive_landscape_archive_reaches_the_optimum_the_ratchet_does_not() {
    let seeds = 50u64;
    let archive = (0..seeds)
        .filter(|s| deceptive_archive(9000 + s, 30))
        .count();
    let ratchet = (0..seeds)
        .filter(|s| deceptive_ratchet(9000 + s, 30))
        .count();
    eprintln!("deceptive: archive {archive} / {seeds}, ratchet {ratchet} / {seeds}");
    assert!(
        archive * 100 >= seeds as usize * 80,
        "archive {archive} / {seeds}"
    );
    assert!(
        ratchet * 100 <= seeds as usize * 20,
        "ratchet {ratchet} / {seeds}"
    );
}

// ───────────────────────────── one wave on toy-route ─────────────────────────────

/// Real git; scripted bench values and secondaries; checks pass; no reviewer.
#[derive(Default)]
struct Bench {
    values: Mutex<VecDeque<(f64, f64)>>,
    commands: Mutex<Vec<String>>,
    /// R-W4-2: another session's `experiment_notes` landing while a settle runs its checks.
    notes_during_checks: Mutex<Option<(Store, String, String)>>,
    /// W4 H5: the experiment the reviewer flags.
    flag: Mutex<Option<u32>>,
}

impl Bench {
    fn with(values: &[(f64, f64)]) -> Arc<Self> {
        let b = Self::default();
        *b.values.lock() = values.iter().copied().collect();
        Arc::new(b)
    }
}

#[async_trait::async_trait]
impl Backend for Bench {
    async fn sample(
        &self,
        _: &StudySpec,
        _: &crate::config::Bench,
        req: SampleRequest<'_>,
        _: &CancellationToken,
    ) -> Result<Sample> {
        let next = self.values.lock().pop_front();
        let (value, time_ms) = next.map_or((None, 0.0), |(v, t)| (Some(v), t));
        Ok(Sample {
            n: req.n,
            k: req.k,
            tree_sha: req.tree_sha.to_owned(),
            role: Some(req.role),
            value,
            secondaries: BTreeMap::from([("time_ms".to_owned(), time_ms)]),
            exit: Some(if value.is_some() { 0 } else { 1 }),
            wall_ms: 1,
            at: now(),
            ..Sample::default()
        })
    }
    async fn command(
        &self,
        command: &str,
        _: Duration,
        _: SampleRequest<'_>,
        _: &CancellationToken,
    ) -> Result<(Option<i32>, String, u64)> {
        self.commands.lock().push(command.to_owned());
        let notes = self.notes_during_checks.lock().take();
        if let Some((store, slug, text)) = notes {
            let p = StudyPatch {
                playbook: Some(text),
                ..StudyPatch::default()
            };
            store.patch_study(&slug, &p).await?;
        }
        Ok((Some(0), String::new(), 1))
    }
    async fn review(
        &self,
        _: &Store,
        _: &StudyRecord,
        n: u32,
        _: &str,
        _: &runtime::Runtime,
    ) -> Result<ReviewVerdict> {
        Ok(if *self.flag.lock() == Some(n) {
            ReviewVerdict::Flagged {
                reason: "skips the tour check".into(),
            }
        } else {
            ReviewVerdict::Clean {
                reason: "fine".into(),
            }
        })
    }
    async fn seed_playbook(&self, _: &str, _: &runtime::Runtime) -> Result<Option<String>> {
        Ok(None)
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

fn sh(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct H {
    _tmp: tempfile::TempDir,
    studies: Studies,
    session: SessionId,
}

const SLUG: &str = "route";

/// A toy-route repo, a running population study with `parallel` slots, the study session bound.
async fn harness(bench: Arc<Bench>, parallel: u8) -> H {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/toy-route");
    std::fs::create_dir_all(repo.join("fixtures")).unwrap();
    for f in ["bench.sh", "main.py", "router.py", "test.sh", "holdout.sh"] {
        std::fs::copy(src.join(f), repo.join(f)).unwrap();
    }
    for f in ["input.json", "optimum.txt"] {
        std::fs::copy(src.join("fixtures").join(f), repo.join("fixtures").join(f)).unwrap();
    }
    sh(tmp.path(), &["init", "-q", "-b", "main", "repo"]);
    sh(&repo, &["add", "-A"]);
    sh(
        &repo,
        &[
            "-c",
            "user.name=u",
            "-c",
            "user.email=u@x",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
    let data_dir = tmp.path().join("data");
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.clone());
    let root = data_dir.join("autoresearch");
    let store = Store::open(&root).await.unwrap();
    let mut studies = Studies {
        data_dir,
        root,
        cfg: crate::config::AutoresearchConfig::default(),
        agent: AgentDefaults {
            communication: true,
            ..AgentDefaults::default()
        },
        rt,
        store,
        backend: bench,
    };
    let mut spec = spec_with(2, vec![d("time_ms", &[10.0])]);
    spec.slug = SLUG.into();
    spec.goal = "shorter tours".into();
    spec.repo = repo.display().to_string();
    spec.bench = BenchSpec {
        command: "bash bench.sh".into(),
        repeats: 3,
        exclusive: true,
        ..BenchSpec::default()
    };
    spec.checks = Some(CommandSpec {
        command: "bash test.sh".into(),
        ..CommandSpec::default()
    });
    spec.scope = vec!["router.py".into()];
    spec.budget = BudgetSpec {
        experiments: 50,
        ..BudgetSpec::default()
    };
    spec.parallel = parallel;
    tools::validate_spec(&spec).unwrap();
    actor::create(
        &mut studies,
        Create {
            spec,
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap();
    for _ in 0..300 {
        let s = studies.store.get_study(SLUG).await.unwrap().unwrap();
        if s.state == StudyState::Running {
            break;
        }
        if let StudyState::Failed { reason } = &s.state {
            panic!("study failed: {reason}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let session = SessionId::new();
    binding::on_fired(&mut studies, &format!("autoresearch-{SLUG}"), session)
        .await
        .unwrap();
    H {
        _tmp: tmp,
        studies,
        session,
    }
}

async fn study(h: &H) -> StudyRecord {
    h.studies.store.get_study(SLUG).await.unwrap().unwrap()
}

fn append(dir: &str, file: &str, text: &str) {
    let p = Path::new(dir).join(file);
    let mut s = std::fs::read_to_string(&p).unwrap();
    s.push_str(text);
    std::fs::write(p, s).unwrap();
}

/// One child: `experiment_begin{slot}`, an edit of `router.py`, `experiment_run`, `experiment_log`.
async fn child_round(h: &H, k: u8, expect: Verdict) -> (u32, String) {
    let env = h.studies.env();
    let child = SessionId::new();
    register_child(h.session, child);
    let brief = actor::begin_env(
        &env,
        Begin {
            session: child,
            slot: Some(k),
        },
    )
    .await
    .unwrap();
    assert!(brief.text.contains("<archive>"), "{}", brief.text);
    let mine = &brief.slots.as_ref().unwrap()[0];
    assert_eq!(mine.k, k);
    // Unique per experiment: a wave-2 slot cut from the base would otherwise repeat wave 1's
    // patch and be refused as a duplicate of a dead end.
    append(
        &mine.worktree,
        "router.py",
        &format!("\n# slot {k} idea #{}\n", mine.n),
    );
    let r = actor::run_env(
        &env,
        Run {
            session: child,
            slot: slot_of(child),
            args: RunArgs {
                idea: format!("idea of slot {k}"),
                category: format!("c{k}"),
                hypothesis: "h".into(),
                // The archive, not novelty, is under test: a one-line comment per slot is
                // "similar" to the previous slot's by design.
                force: true,
                reason: Some("archive test: distinct cell, same shape of edit".into()),
                ..RunArgs::default()
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(r.verdict, expect, "{}", r.line);
    actor::log_env(
        &env,
        Log {
            session: child,
            slot: slot_of(child),
            args: LogArgs {
                description: format!("slot {k} change"),
                learned: "l".into(),
                ..LogArgs::default()
            },
        },
    )
    .await
    .unwrap();
    (mine.n, mine.worktree.clone())
}

#[tokio::test]
async fn a_wave_settles_into_cells_moves_the_champion_and_cuts_the_next_wave_from_elites() {
    // baseline ×3 (length 7450, 1 ms) · slot 1 (7000, 50 ms: better, slow) · slot 2 (7600,
    // 200 ms: worse, slow — a stepping stone) · wave 2 slot samples.
    let bench = Bench::with(&[
        (7450.0, 1.0),
        (7450.0, 1.0),
        (7450.0, 1.0),
        (7000.0, 50.0),
        (7600.0, 200.0),
        (6900.0, 60.0),
        (7500.0, 2.0),
    ]);
    let mut h = harness(bench.clone(), 2).await;
    let env = h.studies.env();
    let base = study(&h).await.incumbent_sha.unwrap();
    let repo = actor::worktree_of(&study(&h).await).unwrap();

    // The supervisor's begin opens wave 1: both slots are cut from the seeded base.
    let brief = actor::begin_env(
        &env,
        Begin {
            session: h.session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let slots = brief.slots.clone().unwrap();
    assert_eq!(slots.len(), 2);
    let seeded = env.store.archive_list(SLUG, Some("cell")).await.unwrap();
    assert_eq!(
        seeded
            .iter()
            .map(|r| (r.key.as_str(), r.n, r.children))
            .collect::<Vec<_>>(),
        vec![("i0:0", 0, 1), ("i1:0", 0, 1)],
        "the base seeds every island; one child counted per island"
    );
    assert!(
        seeded
            .iter()
            .all(|r| r.sha == base && r.scores["length"] == 7450.0)
    );
    assert_eq!(
        refs::list(&repo, SLUG, RefKind::Cell).unwrap(),
        vec![
            ("i0:0".to_owned(), base.clone()),
            ("i1:0".to_owned(), base.clone())
        ]
    );
    for s in &slots {
        assert_eq!(git::head(Path::new(&s.worktree)).unwrap(), base);
    }

    let (n1, _) = child_round(&h, 1, Verdict::Improved).await;
    let e1 = env.store.get_experiment(SLUG, n1).await.unwrap().unwrap();
    assert_eq!(e1.state, ExperimentState::Kept);
    assert_eq!(
        study(&h).await.incumbent_sha.as_deref(),
        Some(base.as_str()),
        "a slot keep moves nothing yet"
    );

    // The last slot's `experiment_log` promotes the wave: `wave::promote_env` dispatches a
    // population study to `promote` (159 W4 H1).
    // Slot 2's own run is `worse`: no checks of its own, the settle's stepping stone runs them once.
    let checks_before = bench.commands.lock().len();
    // R-W4-2: slot 2's session writes notes while the settle runs the stepping stone's checks.
    *bench.notes_during_checks.lock() = Some((
        env.store.clone(),
        SLUG.to_owned(),
        "notes written during the settle".to_owned(),
    ));
    let (n2, _) = child_round(&h, 2, Verdict::Worse).await;
    assert_eq!(
        study(&h).await.playbook,
        "notes written during the settle",
        "the promotion patches its own columns and keeps the notes another session wrote"
    );
    let p = promotion_of(&env, 1).await;
    assert_eq!(
        p.promoted,
        vec![n1, n2],
        "both entered: an improvement and a stepping stone"
    );
    let c1 = e1.commit_sha.clone().unwrap();
    assert_eq!(
        p.incumbent.as_deref(),
        Some(c1.as_str()),
        "the champion is slot 1's commit"
    );
    let s = study(&h).await;
    assert_eq!(s.incumbent_sha.as_deref(), Some(c1.as_str()));
    assert_eq!(s.incumbent_mean, Some(7000.0));
    assert_eq!(
        git::head(&repo).unwrap(),
        c1,
        "the study branch points at the champion"
    );
    assert_eq!(
        bench.commands.lock().len(),
        checks_before + 1,
        "the stepping stone ran the checks once"
    );
    assert!(bench.commands.lock().last().unwrap().contains("test.sh"));

    let e1 = env.store.get_experiment(SLUG, n1).await.unwrap().unwrap();
    let e2 = env.store.get_experiment(SLUG, n2).await.unwrap().unwrap();
    assert_eq!(
        e1.parent_ref.as_deref(),
        Some("refs/autoresearch/route/cell/i0/0")
    );
    assert_eq!(
        e2.parent_ref.as_deref(),
        Some("refs/autoresearch/route/cell/i1/0")
    );
    assert!(
        e1.line.starts_with("entered an empty cell (parent i0:0)"),
        "{}",
        e1.line
    );
    assert_eq!(
        (e2.state, e2.verdict.clone()),
        (ExperimentState::Discarded, Some(Verdict::Worse)),
        "the stepping stone keeps the gate's word (R-W4-4): no 140 reader counts it as a keep"
    );
    let c2 = e2.commit_sha.clone().expect("materialised");
    assert_ne!(c2, base);
    assert!(
        e2.line.starts_with("entered an empty cell (parent i1:0)"),
        "{}",
        e2.line
    );
    let retagged = env.store.samples_of_tree(SLUG, &c2).await.unwrap();
    assert_eq!(
        retagged.len(),
        1,
        "the slot's sample follows the materialised tree"
    );
    // The materialised tree is the discarded patch on the incumbent of the time (the base).
    assert_eq!(sh(&repo, &["rev-parse", &format!("{c2}^")]), base);
    assert!(sh(&repo, &["show", &format!("{c2}:router.py")]).contains("# slot 2 idea"));

    let rows = env.store.archive_list(SLUG, Some("cell")).await.unwrap();
    let got: Vec<(String, u32, String, u32)> = rows
        .iter()
        .map(|r| (r.key.clone(), r.n, r.sha.clone(), r.children))
        .collect();
    assert_eq!(
        got,
        vec![
            ("i0:0".into(), 0, base.clone(), 1),
            ("i0:1".into(), n1, c1.clone(), 0),
            ("i1:0".into(), 0, base.clone(), 1),
            ("i1:1".into(), n2, c2.clone(), 0),
        ]
    );
    assert_eq!(rows[1].descriptor["time_ms"], 50.0);
    assert_eq!(rows[3].scores["length"], 7600.0);
    let listed = refs::list(&repo, SLUG, RefKind::Cell).unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[1], ("i0:1".to_owned(), c1.clone()));
    assert_eq!(listed[3], ("i1:1".to_owned(), c2.clone()));
    let promoted = env
        .store
        .list_events(SLUG, Some(crate::wave::EVENT_PROMOTED), 10)
        .await
        .unwrap();
    assert!(
        promoted
            .iter()
            .any(|e| e.payload["wave"] == 1 && e.payload["promoted"] == serde_json::json!([n1, n2])),
        "{promoted:?}"
    );
    let archived = env
        .store
        .list_events(SLUG, Some(EVENT_ARCHIVE), 10)
        .await
        .unwrap();
    assert_eq!(archived.len(), 2);
    for k in 1..=2 {
        assert!(
            !crate::wave::cand_dir(&env.root, SLUG, k).exists(),
            "candidate worktree {k} removed"
        );
    }
    assert!(
        !env.root.join(SLUG).join(format!("pop-child-{n2}")).exists(),
        "scratch worktree removed"
    );

    // The Study view and the brief see the archive; the champion is starred.
    let KindView::Archive {
        axes,
        cells,
        islands,
    } = view(&env.store, &s).await.unwrap()
    else {
        panic!("not an archive view");
    };
    assert_eq!((axes, islands), (["time_ms".to_owned(), String::new()], 2));
    assert_eq!(
        cells
            .iter()
            .filter(|c| c.champion)
            .map(|c| c.n)
            .collect::<Vec<_>>(),
        vec![n1]
    );
    let section = brief_section(&env.store, &s).await.unwrap();
    assert!(
        section.contains("champion (unconfirmed") && section.contains("i1:1"),
        "{section}"
    );
    assert_eq!(
        headline(&env.store, &s).await.unwrap(),
        None,
        "finalize re-measures the champion"
    );
    assert_eq!(archive(&env.store, SLUG).await.unwrap().len(), 4);

    // Wave 2: wave.rs's settle sees the promotion event and opens; each slot is cut from a
    // sampled elite of its island, counted as that elite's child.
    let brief = actor::begin_env(
        &env,
        Begin {
            session: h.session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let slots = brief.slots.clone().unwrap();
    let rows = env.store.archive_list(SLUG, Some("cell")).await.unwrap();
    for s in &slots {
        let island = s.k - 1;
        let e = env.store.get_experiment(SLUG, s.n).await.unwrap().unwrap();
        assert_eq!(e.wave, Some(2));
        let parent = rows
            .iter()
            .find(|r| r.sha == e.parent_sha && r.island == Some(island))
            .unwrap_or_else(|| {
                panic!(
                    "slot {} parent {} is not an elite of island {island}",
                    s.k, e.parent_sha
                )
            });
        assert_eq!(
            git::head(Path::new(&s.worktree)).unwrap(),
            parent.sha,
            "the slot is cut from its parent"
        );
        let before = seeded_children(&parent.key, n1, n2);
        assert_eq!(
            parent.children,
            before + 1,
            "{}: one more child",
            parent.key
        );
    }
    // No elite was stale enough for a re-measure (remeasure_after is 6 by default).
    assert_eq!(
        bench.values.lock().len(),
        2,
        "no bench sample spent at wave open"
    );
    let _ = (h.session, &mut h);
}

/// Children counted before wave 2 opened: the base's cells had one each, the new elites none.
fn seeded_children(key: &str, _n1: u32, _n2: u32) -> u32 {
    if key.ends_with(":0") { 1 } else { 0 }
}

#[tokio::test]
async fn a_stale_elite_picked_as_parent_is_re_measured_first() {
    // baseline ×3 · the re-measure sample.
    let bench = Bench::with(&[(7450.0, 1.0), (7450.0, 1.0), (7450.0, 1.0), (7460.0, 1.0)]);
    let mut h = harness(bench.clone(), 2).await;
    let env = h.studies.env();
    let mut s = study(&h).await;
    s.spec.population.as_mut().unwrap().remeasure_after = 1;
    env.store.update_study(&s).await.unwrap();
    // Pretend seven experiments have gone by: the base's newest sample (n = 0) is stale.
    for n in 1..=7 {
        let mut e = ExperimentRecord::open(SLUG, n, s.segment, s.incumbent_sha.as_deref().unwrap());
        e.state = ExperimentState::Discarded;
        env.store.insert_experiment(&e).await.unwrap();
    }
    let base = s.incumbent_sha.clone().unwrap();
    let sha = parent_of(&env, &s, Some(1)).await.unwrap();
    assert_eq!(sha, base);
    let samples = env.store.samples_of_tree(SLUG, &base).await.unwrap();
    let re: Vec<&Sample> = samples
        .iter()
        .filter(|x| x.role == Some(SampleRole::Incumbent))
        .collect();
    assert_eq!(re.len(), 1, "one fresh sample of the picked elite");
    assert_eq!((re[0].n, re[0].k, re[0].value), (8, 90, Some(7460.0)));
    assert!(bench.values.lock().is_empty());
    assert!(
        !env.root
            .join(SLUG)
            .join(format!("pop-{}-8", &base[..8]))
            .exists(),
        "scratch removed"
    );
    // Picked again at once: fresh now, no second sample.
    assert_eq!(parent_of(&env, &s, Some(1)).await.unwrap(), base);
    assert_eq!(
        env.store.samples_of_tree(SLUG, &base).await.unwrap().len(),
        4
    );
    let _ = &mut h;
}

/// What the last slot's `experiment_log` promoted (the `wave_promoted` event `promote` writes;
/// `wave::promote_env` dispatches a population study there, 159 W4 H1).
async fn promotion_of(env: &Env, wave: u32) -> Promotion {
    let ev = env
        .store
        .list_events(SLUG, Some(crate::wave::EVENT_PROMOTED), 10)
        .await
        .unwrap();
    let e = ev
        .iter()
        .find(|e| e.payload["wave"] == wave)
        .unwrap_or_else(|| panic!("wave {wave} not promoted: {ev:?}"));
    Promotion {
        promoted: serde_json::from_value(e.payload["promoted"].clone()).unwrap(),
        incumbent: e.payload["incumbent"].as_str().map(str::to_owned),
    }
}

/// R-W4-3, R-W4-4, R-W4-5 with noise (σ ≈ 10 from the baseline). Wave 1 files two stepping
/// stones (`worse`, one sample each). Wave 2 challenges both: the gate first asks for a sample
/// of the one-sample holder (R-25) and the settle takes it on the holder's tree; slot 1's
/// challenger then reads 12 σ better than its holder — `suspicious`, rejected unreviewed; slot
/// 2's reads 6.9 σ better — `improved`, replaces its holder, and stays `discarded`/`worse` as the
/// measure loop said while its commit and the archive carry the entry.
#[tokio::test]
async fn a_challenged_one_sample_elite_is_re_measured_and_suspicious_jumps_stay_out() {
    let bench = Bench::with(&[
        // baseline ×3 (σ = 10)
        (7450.0, 1.0),
        (7460.0, 1.0),
        (7440.0, 1.0),
        // wave 1: slot 1 worse, slow (→ i0:1); slot 2 worse, slower (→ i1:1)
        (7600.0, 50.0),
        (7600.0, 200.0),
        // wave 2: slot 1 no gain vs the champion (→ i0:1); slot 2 worse vs the champion (→ i1:1)
        (7450.0, 50.0),
        (7520.0, 200.0),
        // settle of wave-2 slot 1: holder #1 re-measured (R-25), then its two confirm samples
        (7600.0, 50.0),
        (7450.0, 50.0),
        (7450.0, 50.0),
        // settle of wave-2 slot 2: holder #2 re-measured, the gate decides at one sample
        (7610.0, 200.0),
    ]);
    let mut h = harness(bench.clone(), 2).await;
    let env = h.studies.env();
    let base = study(&h).await.incumbent_sha.unwrap();
    assert!((study(&h).await.sigma.unwrap() - 10.0).abs() < 1e-9);

    actor::begin_env(
        &env,
        Begin {
            session: h.session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let (n1, _) = child_round(&h, 1, Verdict::Worse).await;
    let (n2, _) = child_round(&h, 2, Verdict::Worse).await;
    let p = promotion_of(&env, 1).await;
    assert_eq!(p.promoted, vec![n1, n2], "two stepping stones");
    assert_eq!(
        p.incumbent.as_deref(),
        Some(base.as_str()),
        "the champion is still the base"
    );
    let c1 = env
        .store
        .get_experiment(SLUG, n1)
        .await
        .unwrap()
        .unwrap()
        .commit_sha
        .unwrap();
    let c2 = env
        .store
        .get_experiment(SLUG, n2)
        .await
        .unwrap()
        .unwrap()
        .commit_sha
        .unwrap();
    assert_eq!(env.store.samples_of_tree(SLUG, &c1).await.unwrap().len(), 1);

    actor::begin_env(
        &env,
        Begin {
            session: h.session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let (n3, _) = child_round(&h, 1, Verdict::NoGain).await;
    let (n4, _) = child_round(&h, 2, Verdict::Worse).await;
    let p = promotion_of(&env, 2).await;
    assert_eq!(p.promoted, vec![n4]);
    assert!(
        bench.values.lock().is_empty(),
        "every scripted sample was spent in the scripted order"
    );

    // R-W4-3: both one-sample holders got a fresh Incumbent sample on their own tree.
    for (c, n) in [(&c1, n3), (&c2, n4)] {
        let s = env.store.samples_of_tree(SLUG, c).await.unwrap();
        let re: Vec<&Sample> = s
            .iter()
            .filter(|x| x.role == Some(SampleRole::Incumbent))
            .collect();
        assert_eq!(re.len(), 1, "{c}: {s:?}");
        assert_eq!((re[0].n, re[0].k), (n, 80));
    }
    // R-W4-5: the 12 σ jump is suspicious and stays out; its holder keeps the cell.
    let e3 = env.store.get_experiment(SLUG, n3).await.unwrap().unwrap();
    assert!(e3.line.contains("suspicious; not reviewed"), "{}", e3.line);
    assert_eq!(e3.state, ExperimentState::Discarded);
    let rows = env.store.archive_list(SLUG, Some("cell")).await.unwrap();
    let by_key = |k: &str| rows.iter().find(|r| r.key == k).unwrap();
    assert_eq!(by_key("i0:1").n, n1);
    // R-W4-4: the replacement is archived with its commit, the gate's word stands on the row.
    let e4 = env.store.get_experiment(SLUG, n4).await.unwrap().unwrap();
    assert!(
        e4.line.starts_with(&format!("replaced #{n2} in its cell")),
        "{}",
        e4.line
    );
    assert_eq!(
        (e4.state, e4.verdict.clone()),
        (ExperimentState::Discarded, Some(Verdict::Worse))
    );
    let c4 = e4.commit_sha.clone().expect("materialised");
    assert_eq!(by_key("i1:1").n, n4);
    assert_eq!(by_key("i1:1").sha, c4);
    assert_eq!(
        refs::list(&repo_of(&h).await, SLUG, RefKind::Cell)
            .unwrap()
            .iter()
            .find(|(k, _)| k == "i1:1")
            .unwrap()
            .1,
        c4
    );
    // The replaced elite's commit counted as a challenger's extra samples never: #2's tree has
    // its slot sample and the re-measure only.
    assert_eq!(env.store.samples_of_tree(SLUG, &c2).await.unwrap().len(), 2);
    let kept = env
        .store
        .list_experiments(SLUG, None, u32::MAX)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .count();
    assert_eq!(kept, 0, "stepping stones and replacements are not keeps");
    let _ = &mut h;
}

async fn repo_of(h: &H) -> PathBuf {
    actor::worktree_of(&study(h).await).unwrap()
}

/// W4 H5 (W4-f): the reviewer runs at archive acceptance. Slot 1's keep (`improved`, would enter
/// `i0:1` and become the champion) is flagged under `on_suspicious: revert`: the acceptance is
/// undone (no cell, no ref), the row says `rejected_by_review` with the saved patch and the
/// `rejected/<n>` ref, the champion stays the base; slot 2's stepping stone is not reviewed.
#[tokio::test]
async fn a_flagged_keep_is_reverted_at_acceptance_and_its_cell_stays_empty() {
    let bench = Bench::with(&[
        (7450.0, 1.0),
        (7450.0, 1.0),
        (7450.0, 1.0),
        (7000.0, 50.0),
        (7600.0, 200.0),
    ]);
    let mut h = harness(bench.clone(), 2).await;
    let env = h.studies.env();
    let base = study(&h).await.incumbent_sha.unwrap();
    let repo = repo_of(&h).await;
    actor::begin_env(
        &env,
        Begin {
            session: h.session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let (n1, _) = child_round(&h, 1, Verdict::Improved).await;
    *bench.flag.lock() = Some(n1);
    let c1 = env
        .store
        .get_experiment(SLUG, n1)
        .await
        .unwrap()
        .unwrap()
        .commit_sha
        .unwrap();
    let (n2, _) = child_round(&h, 2, Verdict::Worse).await;
    let p = promotion_of(&env, 1).await;
    assert_eq!(p.promoted, vec![n2], "the flagged keep is not promoted");
    assert_eq!(p.incumbent.as_deref(), Some(base.as_str()));
    assert_eq!(git::head(&repo).unwrap(), base);

    let e1 = env.store.get_experiment(SLUG, n1).await.unwrap().unwrap();
    assert_eq!(e1.state, ExperimentState::RejectedByReview);
    assert!(
        matches!(&e1.review, Some(ReviewVerdict::Reverted { reason }) if reason == "skips the tour check"),
        "{:?}",
        e1.review
    );
    assert!(
        e1.line
            .starts_with("rejected by review (skips the tour check); entered an empty cell"),
        "{}",
        e1.line
    );
    let patch = env.root.join(e1.patch_path.as_deref().unwrap());
    assert!(
        std::fs::read_to_string(&patch)
            .unwrap()
            .contains("# slot 1 idea"),
        "the saved patch is the keep's diff"
    );
    assert_eq!(
        sh(&repo, &["rev-parse", &git::rejected_ref(SLUG, n1)]),
        c1,
        "Restore finds the commit"
    );
    let keys: Vec<String> = env
        .store
        .archive_list(SLUG, Some("cell"))
        .await
        .unwrap()
        .iter()
        .map(|r| r.key.clone())
        .collect();
    assert_eq!(
        keys,
        vec!["i0:0", "i1:0", "i1:1"],
        "no cell for the flagged keep"
    );
    assert_eq!(refs::list(&repo, SLUG, RefKind::Cell).unwrap().len(), 3);
    let archived = env
        .store
        .list_events(SLUG, Some(EVENT_ARCHIVE), 10)
        .await
        .unwrap();
    assert!(
        archived
            .iter()
            .any(|e| e.payload["n"] == n1 && e.payload["outcome"] == "rejected_by_review"),
        "{archived:?}"
    );
    let reviews = env
        .store
        .list_events(SLUG, Some("review"), 10)
        .await
        .unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].payload["reverted"], true);
    let e2 = env.store.get_experiment(SLUG, n2).await.unwrap().unwrap();
    assert_eq!(e2.review, None, "a stepping stone is not reviewed");
    let _ = &mut h;
}
