//! 159 W7 — the outer study's `suite` scoring (W-j): one inner fixture study per training
//! fixture and seed under the mechanism being measured, the normalised holdout gain of each
//! as one case, decided with W2's paired gate (W-k). Owner: W7. Signatures frozen by C.
//!
//! One outer sample = one suite run. `sample` materialises each fixture into its own git
//! repo under `<root>/<slug>/suite/`, creates the inner studies (`created_by = meta:<slug>`,
//! pinned to the candidate mechanism), lets the engine run them and waits for them to finish
//! (W7-b: inside the sample, not through W6's `Waiting` — see the as-built), then reads each
//! one's holdout rows. Scores come from the fixtures' holdouts only — never the search metric,
//! never the keep count — and the held-out fixtures run only as the outer study's holdout, so
//! their numbers never reach the outer brief.
//!
//! The suite is `spec.suite` (`SuiteSpec`, R1): training and held-out fixture names (empty =
//! the §14 suite), seeds and experiments per inner study (0 = the §12 constants, 2 and 8), the
//! inner executor. `bench.repeats` is still the incumbent's suite runs at Start and
//! `holdout.every` the held-out cadence; their `command`s are unused for this kind.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use evalcore::gate::{Cfg as PairCfg, Decision, Pair, Paired};
use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{Store, StudyRecord};
use crate::types::{
    BenchSpec, BudgetSpec, Call, CommandSpec, Direction, GateCfg, GateDecision, GateInput,
    GateRule, GateStep, Guard, HoldoutSpec, KindView, MetricSpec, ModelsSpec, ObjectiveDelta,
    ObjectiveRole, Sample, SampleRole, StartWhen, StudySpec, StudyState, SuiteRow, SuiteSpec,
    Verdict,
};

/// The §14 training suite.
pub const TRAINING: [&str; 4] = ["toy-sort", "toy-dedupe", "toy-wordfreq", "toy-pareto"];
/// The §14 held-out suite.
pub const HELD_OUT: [&str; 3] = ["toy-route", "toy-json", "toy-lru"];
/// Inner studies per fixture (replicates; the session is stochastic).
pub const SEEDS: u32 = 2;
/// The inner budget (§12): 8 experiments, $2, `parallel: 1`.
pub const INNER_EXPERIMENTS: u32 = 8;
/// The inner budget in USD.
pub const INNER_USD: f64 = 2.0;
/// The outer metric's name: the mean normalised holdout gain over the suite.
pub const METRIC: &str = "suite_gain";
/// What one inner study costs on a cheap executor (§12 Cost), for the card.
const INNER_COST_USD: f64 = 1.2;
const INNER_PER_EXPERIMENT_USD: f64 = 0.3;
const INNER_HOURS: f64 = 3.0;
/// Inner studies in flight at once (§12 Cost: "two at a time").
const INNER_CONCURRENCY: usize = 2;
/// How long a sample waits for one inner study before stopping it: the inner budget's hours
/// plus slack; seconds under test, so a driver that never fires fails fast instead of hanging.
#[cfg(not(test))]
const INNER_DEADLINE: Duration = Duration::from_secs((INNER_HOURS * 3600.0) as u64 + 1800);
#[cfg(test)]
const INNER_DEADLINE: Duration = Duration::from_secs(20);
/// How often a waiting sample looks at its inner studies.
#[cfg(not(test))]
const POLL: Duration = Duration::from_secs(2);
#[cfg(test)]
const POLL: Duration = Duration::from_millis(30);
/// A fixture's holdout score may fall at most this much under a candidate mechanism
/// (dreaming's `max_case_regression`, in normalised gain).
const MAX_CASE_REGRESSION: f64 = 0.25;
/// The incumbent mechanism's suite re-runs when its newest sample is this many outer
/// experiments old (§12: "every third outer experiment").
const REMEASURE_AGE: u32 = 2;
/// The reference solution's holdout gets this long.
const REFERENCE_TIMEOUT: Duration = Duration::from_secs(120);

// ───────────────────────────── the fixtures, embedded ─────────────────────────────

/// One suite fixture: the files of `crates/autoresearch/fixtures/<name>/`, embedded so an
/// engine without a theuth checkout can materialise it.
pub struct Fixture {
    /// Directory name under `fixtures/`.
    pub name: &'static str,
    /// The one file the inner study may edit.
    pub scope: &'static str,
    /// The inner study's goal.
    pub goal: &'static str,
    /// `(path, text, executable)`.
    pub files: &'static [(&'static str, &'static str, bool)],
}

macro_rules! fixture_files {
    ($dir:literal, $scope:literal $(, $extra:expr)*) => {
        &[
            ("bench.sh", include_str!(concat!("../fixtures/", $dir, "/bench.sh")), true),
            ("test.sh", include_str!(concat!("../fixtures/", $dir, "/test.sh")), true),
            ("holdout.sh", include_str!(concat!("../fixtures/", $dir, "/holdout.sh")), true),
            ("main.py", include_str!(concat!("../fixtures/", $dir, "/main.py")), false),
            (".gitignore", "__pycache__/\n", false),
            ($scope, include_str!(concat!("../fixtures/", $dir, "/", $scope)), false),
            $($extra,)*
        ]
    };
}

/// toy-sort's holdout with the seed offset of R-W7-6 (the fixture dir is C's; the same text is
/// handed off for `fixtures/toy-sort/holdout.sh`). Identical to the shipped script at seed 0.
const TOY_SORT_HOLDOUT: &str = r#"#!/bin/bash
# The holdout (140 §5.4): the same objective on sizes and seeds the search never
# scores, with a correctness check of its own (a cheat that returns the input
# unsorted is caught here even if the benchmark never looks).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import os, random, sys, time
from sorter import sort

# 159 R-W7-6: the suite runner draws the seed per outer study and passes it in the
# environment; it is never in the repo, so a session tuned to the holdout cannot see it.
SEED = int(os.environ.get("AUTORESEARCH_HOLDOUT_SEED", "0"))
total = 0.0
for n, seed in ((2500, 101), (6000, 202), (10000, 303), (7000, 404)):
    rng = random.Random(seed + SEED)
    xs = [rng.randrange(-10**9, 10**9) for _ in range(n)]
    t0 = time.perf_counter()
    out = sort(xs)
    ms = (time.perf_counter() - t0) * 1000.0
    if out != sorted(xs):
        print(f"holdout: wrong output for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed + SEED} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
"#;

/// The fixtures this build can materialise. toy-pareto (W1) and toy-route (W4) are not in
/// this tree; Z adds their rows (as-built W7, Hand-offs). toy-sort ships no `reference/`, so
/// its reference (`sorted`) is embedded here. `reference/*` entries feed `best_known` only
/// and are never written into an inner repo (R-W7-1).
pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "toy-sort",
        scope: "sorter.py",
        goal: "Make sorting faster without changing output.",
        files: &[
            (
                "bench.sh",
                include_str!("../fixtures/toy-sort/bench.sh"),
                true,
            ),
            (
                "test.sh",
                include_str!("../fixtures/toy-sort/test.sh"),
                true,
            ),
            ("holdout.sh", TOY_SORT_HOLDOUT, true),
            (
                "main.py",
                include_str!("../fixtures/toy-sort/main.py"),
                false,
            ),
            (".gitignore", "__pycache__/\n", false),
            (
                "sorter.py",
                include_str!("../fixtures/toy-sort/sorter.py"),
                false,
            ),
            (
                "fixtures/input.txt",
                include_str!("../fixtures/toy-sort/fixtures/input.txt"),
                false,
            ),
            (
                "fixtures/expected.txt",
                include_str!("../fixtures/toy-sort/fixtures/expected.txt"),
                false,
            ),
            (
                "reference/sorter.py",
                "def sort(xs):\n    return sorted(xs)\n",
                false,
            ),
        ],
    },
    Fixture {
        name: "toy-dedupe",
        scope: "dedupe.py",
        goal: "Make `dedupe` faster without changing output.",
        files: fixture_files!(
            "toy-dedupe",
            "dedupe.py",
            (
                "fixtures/input.txt",
                include_str!("../fixtures/toy-dedupe/fixtures/input.txt"),
                false
            ),
            (
                "fixtures/expected.txt",
                include_str!("../fixtures/toy-dedupe/fixtures/expected.txt"),
                false
            ),
            (
                "reference/dedupe.py",
                include_str!("../fixtures/toy-dedupe/reference/dedupe.py"),
                false
            )
        ),
    },
    Fixture {
        name: "toy-wordfreq",
        scope: "wordfreq.py",
        goal: "Make `top` faster without changing output.",
        files: fixture_files!(
            "toy-wordfreq",
            "wordfreq.py",
            (
                "fixtures/input.txt",
                include_str!("../fixtures/toy-wordfreq/fixtures/input.txt"),
                false
            ),
            (
                "fixtures/expected.txt",
                include_str!("../fixtures/toy-wordfreq/fixtures/expected.txt"),
                false
            ),
            (
                "reference/wordfreq.py",
                include_str!("../fixtures/toy-wordfreq/reference/wordfreq.py"),
                false
            )
        ),
    },
    Fixture {
        name: "toy-json",
        scope: "jsonparse.py",
        goal: "Make `parse` faster without changing output.",
        files: fixture_files!(
            "toy-json",
            "jsonparse.py",
            (
                "fixtures/input.json",
                include_str!("../fixtures/toy-json/fixtures/input.json"),
                false
            ),
            (
                "fixtures/expected.txt",
                include_str!("../fixtures/toy-json/fixtures/expected.txt"),
                false
            ),
            (
                "reference/jsonparse.py",
                include_str!("../fixtures/toy-json/reference/jsonparse.py"),
                false
            )
        ),
    },
    Fixture {
        name: "toy-lru",
        scope: "lru.py",
        goal: "Make `replay` faster without changing output.",
        files: fixture_files!(
            "toy-lru",
            "lru.py",
            (
                "fixtures/input.txt",
                include_str!("../fixtures/toy-lru/fixtures/input.txt"),
                false
            ),
            (
                "fixtures/expected.txt",
                include_str!("../fixtures/toy-lru/fixtures/expected.txt"),
                false
            ),
            (
                "reference/lru.py",
                include_str!("../fixtures/toy-lru/reference/lru.py"),
                false
            )
        ),
    },
];

/// The fixture named `name`, when this build has it.
pub fn fixture(name: &str) -> Option<&'static Fixture> {
    FIXTURES.iter().find(|f| f.name == name)
}

/// Write `f` under `dir` and commit it as the base of a fresh git repo. The `reference/`
/// entries stay out (R-W7-1): an inner session reads its worktree, and the answer key would
/// make "copy the reference" the first outer keep. `best_known` applies the reference itself.
pub fn materialise(f: &Fixture, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (path, text, exec) in f
        .files
        .iter()
        .filter(|(p, ..)| !p.starts_with("reference/"))
    {
        let p = dir.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, text)?;
        #[cfg(unix)]
        if *exec {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    crate::git::plumbing(dir, &["init", "-q"])?;
    crate::git::plumbing(dir, &["add", "-A"])?;
    crate::git::plumbing(dir, &["commit", "-q", "-m", "base"])?;
    Ok(())
}

// ───────────────────────────── the suite in the spec ─────────────────────────────

/// What the outer spec says about the suite: `spec.suite` with the §12/§14 defaults filled.
#[derive(Clone, Debug, PartialEq)]
pub struct Suite {
    /// Training fixtures (the bench).
    pub training: Vec<String>,
    /// Held-out fixtures (the outer holdout).
    pub held_out: Vec<String>,
    /// Inner studies per fixture.
    pub seeds: u32,
    /// Experiments per inner study.
    pub inner_experiments: u32,
    /// The inner studies' executor; `None` = the outer study's.
    pub executor: Option<String>,
}

fn words(s: &str) -> Vec<String> {
    s.split([' ', ','])
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The suite of `spec`: `spec.suite` (R1), empty lists = §14's, zeros = §12's constants.
pub fn suite_of(spec: &StudySpec) -> Suite {
    let s = spec.suite.clone().unwrap_or_default();
    Suite {
        training: if s.training.is_empty() {
            TRAINING.iter().map(|x| (*x).to_owned()).collect()
        } else {
            s.training
        },
        held_out: if s.held_out.is_empty() {
            HELD_OUT.iter().map(|x| (*x).to_owned()).collect()
        } else {
            s.held_out
        },
        seeds: if s.seeds == 0 { SEEDS } else { s.seeds },
        inner_experiments: if s.inner_experiments == 0 {
            INNER_EXPERIMENTS
        } else {
            s.inner_experiments
        },
        executor: s.executor,
    }
}

/// The spec's `suite` block, created on first write.
fn suite_mut(spec: &mut StudySpec) -> &mut SuiteSpec {
    spec.suite.get_or_insert_with(SuiteSpec::default)
}

/// The meta preset the setup chat starts from (`/autoresearch meta`, 159 §12): the §14 suite,
/// $60, an idle window; the agent asks only for the budget, the window and the suite.
pub fn preset(repo: &str) -> StudySpec {
    StudySpec {
        slug: "meta".into(),
        goal: "Improve how theuth runs studies: the protocol, the fire prompt, the advisor's \
               prompt and the brief's caps, measured by inner fixture studies."
            .into(),
        repo: repo.to_owned(),
        metric: MetricSpec {
            name: METRIC.into(),
            unit: String::new(),
            direction: Direction::Higher,
            min_effect: "0.02".into(),
        },
        bench: BenchSpec {
            command: "suite".into(),
            timeout: "4h".into(),
            repeats: 2,
            exclusive: false,
            smoke: None,
        },
        holdout: Some(HoldoutSpec {
            command: "suite".into(),
            every: 2,
            repeats: Some(1),
        }),
        suite: Some(SuiteSpec {
            training: TRAINING.iter().map(|x| (*x).to_owned()).collect(),
            held_out: HELD_OUT.iter().map(|x| (*x).to_owned()).collect(),
            seeds: SEEDS,
            inner_experiments: INNER_EXPERIMENTS,
            executor: None,
        }),
        evaluator_paths: Vec::new(),
        scope: crate::mechanism::FILES
            .iter()
            .map(|f| (*f).to_owned())
            .collect(),
        off_limits: Vec::new(),
        budget: BudgetSpec {
            usd: 60.0,
            per_experiment_usd: 20.0,
            hours: 24.0,
            experiments: 6,
        },
        window: None,
        axes: crate::types::Axes {
            scoring: crate::types::Scoring::Suite,
            artefact: crate::types::Artefact::Mechanism,
            ..Default::default()
        },
        ..StudySpec::default()
    }
}

/// Draft card line 1 for a meta study.
pub fn card_line(spec: &StudySpec) -> String {
    let s = suite_of(spec);
    let inner = s.training.len() as u32 * s.seeds;
    let cost = inner as f64 * INNER_COST_USD * 4.0 / 3.0;
    format!(
        "Improve the study mechanism on {} training fixtures ({} held out); each outer experiment runs {inner} inner studies, about ${cost:.0}.",
        s.training.len(),
        s.held_out.len()
    )
}

/// Setup rows this kind adds to the draft panel.
pub const SETUP_ROWS: &[SetupRow] = &[SetupRow {
    row: "suite",
    label: "Suite",
    leaves: &[
        "suite.training",
        "suite.held_out",
        "suite.seeds",
        "suite.inner_experiments",
        "suite.executor",
    ],
}];

/// One-line value of a kind row for the draft panel.
pub fn setup_value(spec: &StudySpec, row: &str) -> String {
    if row != "suite" {
        return String::new();
    }
    let s = suite_of(spec);
    let mut line = format!(
        "training {} · held out {} · {} seeds · {} inner experiments at ${INNER_USD} each",
        s.training.join(", "),
        s.held_out.join(", "),
        s.seeds,
        s.inner_experiments
    );
    if let Some(e) = s.executor {
        line.push_str(&format!(" on {e}"));
    }
    line
}

fn known(names: &[String]) -> std::result::Result<(), String> {
    match names.iter().find(|n| fixture(n).is_none()) {
        Some(n) => Err(format!(
            "unknown fixture `{n}` (this build has {})",
            FIXTURES
                .iter()
                .map(|f| f.name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => Ok(()),
    }
}

/// Set one kind leaf from the panel's text.
pub fn setup_set(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    let count = |what: &str, max: u32| -> std::result::Result<u32, String> {
        let n: u32 = v
            .trim()
            .parse()
            .map_err(|_| format!("{what}: not a whole number"))?;
        if n == 0 || n > max {
            return Err(format!("{what}: {n} is outside 1..={max}"));
        }
        Ok(n)
    };
    match leaf {
        "suite.training" => {
            let names = words(v);
            if names.is_empty() {
                return Err("suite.training: name at least one fixture".into());
            }
            known(&names)?;
            suite_mut(spec).training = names;
        }
        "suite.held_out" => {
            let names = words(v);
            known(&names)?;
            suite_mut(spec).held_out = names;
            if spec.holdout.is_none() {
                spec.holdout = Some(HoldoutSpec {
                    command: "suite".into(),
                    every: 2,
                    repeats: Some(1),
                });
            }
        }
        "suite.seeds" => suite_mut(spec).seeds = count("suite.seeds", 5)?,
        "suite.inner_experiments" => {
            suite_mut(spec).inner_experiments = count("suite.inner_experiments", 40)?;
        }
        "suite.executor" => {
            let v = v.trim();
            suite_mut(spec).executor = (!v.is_empty()).then(|| v.to_owned());
        }
        other => return Err(format!("unknown field `{other}`")),
    }
    Ok(())
}

// ───────────────────────────── the inner studies ─────────────────────────────

/// The case id of fixture × seed.
fn case_id(fixture: &str, seed: u32) -> String {
    format!("{fixture}/{seed}")
}

/// A slug at most 32 chars: `<outer>-<n><role><k>-<fixture>-s<seed>`, the outer part cut first.
fn inner_slug(outer: &str, n: u32, role: SampleRole, k: u32, fixture: &str, seed: u32) -> String {
    let role = match role {
        SampleRole::Candidate => 'c',
        SampleRole::Incumbent => 'i',
        SampleRole::Baseline => 'b',
        SampleRole::Holdout => 'h',
        SampleRole::Final => 'f',
    };
    let tail = format!(
        "-{n}{role}{k}-{}-s{seed}",
        fixture.trim_start_matches("toy-")
    );
    let room = 32usize.saturating_sub(tail.len());
    let head: String = outer.chars().take(room).collect();
    format!("{}{tail}", head.trim_end_matches('-'))
}

/// The inner spec: the fixture's README spec with the §12 budget, the outer executor and the
/// outer study's holdout seed.
fn inner_spec(
    f: &Fixture,
    slug: &str,
    repo: &Path,
    suite: &Suite,
    outer: &StudySpec,
    hseed: u32,
) -> StudySpec {
    StudySpec {
        slug: slug.to_owned(),
        goal: f.goal.to_owned(),
        repo: repo.display().to_string(),
        metric: MetricSpec {
            name: "time_ms".into(),
            unit: "ms".into(),
            direction: Direction::Lower,
            min_effect: "1%".into(),
        },
        secondary_guards: vec![Guard {
            name: "peak_kb".into(),
            max_regression: "50%".into(),
            direction: Direction::Lower,
        }],
        bench: BenchSpec {
            command: "bash bench.sh".into(),
            timeout: "60s".into(),
            repeats: 3,
            exclusive: true,
            smoke: None,
        },
        checks: Some(CommandSpec {
            command: "bash test.sh".into(),
            timeout: "30s".into(),
        }),
        // R-W7-6: the seed is in the command the plugin runs (the ledger), not in the repo.
        holdout: Some(HoldoutSpec {
            command: format!("AUTORESEARCH_HOLDOUT_SEED={hseed} bash holdout.sh"),
            every: 1,
            repeats: None,
        }),
        evaluator_paths: ["bench.sh", "test.sh", "holdout.sh", "fixtures/"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        scope: vec![f.scope.to_owned()],
        off_limits: [
            "fixtures/**",
            "test.sh",
            "bench.sh",
            "holdout.sh",
            "main.py",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
        budget: BudgetSpec {
            usd: INNER_USD,
            per_experiment_usd: INNER_PER_EXPERIMENT_USD,
            hours: INNER_HOURS,
            experiments: suite.inner_experiments,
        },
        parallel: 1,
        models: ModelsSpec {
            executor: suite
                .executor
                .clone()
                .or_else(|| outer.models.executor.clone()),
            review: outer.models.review.clone(),
            advisor: outer.models.advisor.clone(),
            ..ModelsSpec::default()
        },
        ..StudySpec::default()
    }
}

/// One inner study as the outer sample sees it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Inner {
    /// The inner study.
    pub slug: String,
    /// Its fixture.
    pub fixture: String,
    /// Its seed (replicate).
    pub seed: u32,
    /// Base holdout mean (`time_ms`).
    pub base: Option<f64>,
    /// Final incumbent's holdout mean.
    pub last: Option<f64>,
    /// The reference solution's holdout mean, when measured.
    pub best: Option<f64>,
    /// The normalised holdout gain.
    pub score: Option<f64>,
    /// Keeps the inner study made (reported, never scored).
    pub keeps: u32,
    /// The inner study's final state word.
    pub state: String,
    /// What it spent, rolled into the outer sample's bench channel.
    pub spend_usd: f64,
}

/// The state's serde tag (`running`, `finished`, …).
fn state_word(state: &StudyState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.get("state")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The normalised holdout gain s = (base − final) / (base − best_known), with the search
/// direction applied; without a reference (or one no better than base) the plain relative
/// gain (base − final) / base. Lower-is-better fixtures only, which the suite's all are.
pub fn score(base: f64, last: f64, best: Option<f64>) -> Option<f64> {
    if !base.is_finite() || !last.is_finite() {
        return None;
    }
    let denom = match best {
        Some(b) if b.is_finite() && base - b > 1e-9 => base - b,
        _ if base.abs() > 1e-9 => base.abs(),
        _ => return None,
    };
    Some((base - last) / denom)
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Read an inner study's holdout rows: base = the base tree's, final = the newest keep's
/// (the highest `n > 0`), else base. Only `role = holdout` rows count (§12: never the search
/// metric).
pub async fn score_inner(store: &Store, inner: &StudyRecord, best: Option<f64>) -> Result<Inner> {
    let samples = store.list_samples(&inner.slug, None).await?;
    let hold: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.role == Some(SampleRole::Holdout) && !s.lost && s.value.is_some())
        .collect();
    let base_sha = inner.base_sha.clone().unwrap_or_default();
    let base = mean(
        &hold
            .iter()
            .filter(|s| s.tree_sha == base_sha)
            .filter_map(|s| s.value)
            .collect::<Vec<_>>(),
    );
    let newest = hold.iter().filter(|s| s.n > 0).map(|s| s.n).max();
    let last = match newest {
        Some(n) => mean(
            &hold
                .iter()
                .filter(|s| s.n == n)
                .filter_map(|s| s.value)
                .collect::<Vec<_>>(),
        ),
        None => base,
    };
    let keeps = store
        .list_experiments(&inner.slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| e.state == crate::types::ExperimentState::Kept)
        .count() as u32;
    let (fixture, seed) = inner
        .created_by
        .as_deref()
        .and_then(|c| c.split_once('/'))
        .map(|(_, fs)| fs)
        .and_then(|fs| fs.rsplit_once('/'))
        .map(|(f, s)| (f.to_owned(), s.parse().unwrap_or(0)))
        .unwrap_or_default();
    Ok(Inner {
        slug: inner.slug.clone(),
        fixture,
        seed,
        base,
        last,
        best,
        score: match (base, last) {
            (Some(b), Some(l)) => score(b, l, best),
            _ => None,
        },
        keeps,
        state: state_word(&inner.state),
        spend_usd: 0.0,
    })
}

/// `created_by` of an inner study: `meta:<outer>/<fixture>/<seed>`.
fn created_by(outer: &str, fixture: &str, seed: u32) -> String {
    format!("meta:{outer}/{fixture}/{seed}")
}

/// Create one inner study (the `create` path on an [`Env`]) and run its preparation to
/// `running` or `failed`. The schedule actor fires its sessions from there.
async fn start_inner(
    env: &Env,
    mut spec: StudySpec,
    created_by: String,
    sha: &str,
) -> Result<StudyRecord> {
    crate::tools::validate_spec(&spec).map_err(Error::refused)?;
    spec.normalise();
    let slug = spec.slug.clone();
    let mut study = StudyRecord::new(spec, None);
    study.state = StudyState::Preparing;
    study.created_by = Some(created_by);
    study.mechanism = Some(sha.to_owned());
    study.schedule = Some(crate::schedule_file::name(&slug));
    env.store.create_study(&study).await?;
    env.rt
        .publish(crate::types::StudyEvent::StudyChanged {
            slug: slug.to_owned(),
            state: StudyState::Preparing,
            reason: None,
        })
        .await;
    prepare_boxed(env.clone(), study.clone()).await;
    Ok(study)
}

/// `actor::prepare` behind a concrete `dyn Future` type: `prepare` reaches `kind_sample` and
/// so [`sample`] again, and the boxing in a plain fn is what keeps the two opaque future
/// types from depending on each other.
fn prepare_boxed(env: Env, study: StudyRecord) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(crate::actor::prepare(env, study, StartWhen::Now))
}

/// `actor::control_env` the same way (`StopNow` reaches the measure path and [`sample`]).
fn control_boxed(
    env: Env,
    m: crate::types::Control,
) -> Pin<Box<dyn Future<Output = Result<crate::types::StudyInfo>> + Send>> {
    Box::pin(async move { crate::actor::control_env(&env, m).await })
}

/// Wait for `slug` to reach a terminal state; `Err` when cancelled or past the inner budget's
/// hours plus slack.
async fn wait_terminal(env: &Env, slug: &str, cancel: &CancellationToken) -> Result<StudyRecord> {
    let deadline = Instant::now() + INNER_DEADLINE;
    loop {
        let s = env
            .store
            .get_study(slug)
            .await?
            .ok_or_else(|| Error::refused(format!("inner study `{slug}` vanished")))?;
        if crate::actor::is_terminal(&s.state) {
            return Ok(s);
        }
        if cancel.is_cancelled() {
            return Err(Error::refused("cancelled while inner studies ran"));
        }
        if Instant::now() > deadline {
            return Err(Error::refused(format!(
                "inner study `{slug}` did not finish within its budget"
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The candidate mechanism of this sample: a snapshot of the outer worktree for a candidate
/// (refused here when invalid, before any inner study starts); the tree's sha otherwise.
fn mechanism_sha(study: &StudyRecord, req: &SampleRequest<'_>) -> Result<String> {
    if req.role == SampleRole::Candidate && crate::git::is_git_repo(req.worktree) {
        return crate::mechanism::snapshot(req.worktree, &study.slug, req.n);
    }
    Ok(req.tree_sha.to_owned())
}

/// The outer study's holdout seed (R-W7-6): drawn once per outer study, kept in
/// `suite/holdout_seed` (never in any inner repo) and passed to every inner holdout and to the
/// reference measurement as `AUTORESEARCH_HOLDOUT_SEED`, so every tree is scored on the same
/// held-out inputs and no fixture file names them.
pub fn holdout_seed(dir: &Path) -> u32 {
    let path = dir.join("holdout_seed");
    if let Some(s) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
    {
        return s;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    // 1..=999_999: never 0, which the scripts read as "the shipped seeds".
    let seed = nanos % 999_999 + 1;
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, seed.to_string());
    seed
}

/// `stop_now` on an inner study the sample gives up on (cancelled, over its hours, failed to
/// score): a finished or failed study is left alone; errors are logged, never fatal.
async fn stop_now(env: &Env, slug: &str) {
    match env.store.get_study(slug).await {
        Ok(Some(s)) if !crate::actor::is_terminal(&s.state) => {
            let m = crate::types::Control {
                slug: slug.to_owned(),
                op: crate::types::ControlOp::StopNow,
                by: crate::types::By::Plugin,
            };
            if let Err(e) = control_boxed(env.clone(), m).await {
                tracing::warn!(%slug, "inner study not stopped: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%slug, "inner study not read: {e}"),
    }
}

/// `best_known` per fixture: the reference solution's holdout on this machine, measured once
/// per outer study (cached in `suite/best_known.json`) through `plugin_command`, on the same
/// holdout seed the inner studies get.
async fn best_known(
    env: &Env,
    study: &StudyRecord,
    names: &[String],
    dir: &Path,
    hseed: u32,
) -> BTreeMap<String, f64> {
    let cache = dir.join("best_known.json");
    let mut known: BTreeMap<String, f64> = std::fs::read(&cache)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for name in names {
        if known.contains_key(name) {
            continue;
        }
        let Some(f) = fixture(name) else { continue };
        let Some(reference) = f.files.iter().find(|(p, ..)| p.starts_with("reference/")) else {
            continue;
        };
        let copy = dir.join("reference").join(name);
        let ok = (|| -> Result<()> {
            materialise(f, &copy)?;
            std::fs::write(copy.join(f.scope), reference.1)?;
            Ok(())
        })();
        if let Err(e) = ok {
            tracing::warn!(%name, "reference copy not written: {e}");
            continue;
        }
        let run = env
            .backend
            .plugin_command(
                study,
                &format!("reference-{name}"),
                &format!("bash {}", copy.join("holdout.sh").display()),
                REFERENCE_TIMEOUT,
                &[
                    (
                        "AUTORESEARCH_WORKTREE".to_owned(),
                        copy.display().to_string(),
                    ),
                    ("AUTORESEARCH_HOLDOUT_SEED".to_owned(), hseed.to_string()),
                ],
            )
            .await;
        match run {
            Ok(r) if r.exit == Some(0) => {
                if let Some(v) = r.scanned.metrics.get("time_ms") {
                    known.insert(name.clone(), *v);
                }
            }
            Ok(r) => tracing::warn!(%name, exit = ?r.exit, "reference holdout failed: {}", r.tail),
            Err(e) => tracing::warn!(%name, "reference holdout not run: {e}"),
        }
    }
    if let Ok(b) = serde_json::to_vec_pretty(&known) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&cache, b);
    }
    known
}

/// Engine-side scoring for this kind (`Scoring` dispatch in actor.rs): one `Sample` per
/// suite run. The caller inserted the pending row and stores what comes back.
pub async fn sample(
    env: &Env,
    study: &StudyRecord,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    let started = Instant::now();
    let suite = suite_of(&study.spec);
    let held_out = req.role == SampleRole::Holdout;
    let names = if held_out {
        &suite.held_out
    } else {
        &suite.training
    };
    // A fixture another wave owns may be absent from this build (toy-pareto, toy-route until
    // W1/W4 land): the suite runs on what it has and says what it skipped.
    let missing: Vec<&String> = names.iter().filter(|n| fixture(n).is_none()).collect();
    if missing.len() == names.len() {
        known(names).map_err(Error::refused)?;
    }
    if !missing.is_empty() {
        tracing::warn!(slug = %study.slug, ?missing, "suite fixtures not in this build; skipped");
    }
    // R-W7-2: the inner spend of one outer sample is bounded by `per_experiment_usd` before
    // anything starts (the setup panel caps `seeds`, a hand-written spec may not).
    let planned = (names.len() - missing.len()) as f64 * f64::from(suite.seeds) * INNER_USD;
    if planned > study.spec.budget.per_experiment_usd {
        return Err(Error::refused(format!(
            "a suite run of {} fixtures × {} seeds × ${INNER_USD} = ${planned:.0} is over budget.per_experiment_usd ${}; fewer seeds or a larger cap",
            names.len() - missing.len(),
            suite.seeds,
            study.spec.budget.per_experiment_usd
        )));
    }
    let sha = mechanism_sha(study, &req)?;
    let dir = crate::study_dir(&env.root, &study.slug).join("suite");
    let hseed = holdout_seed(&dir);
    let best = best_known(env, study, names, &dir, hseed).await;
    let run_dir = dir.join(format!(
        "{}-{}-{}",
        req.n,
        match req.role {
            SampleRole::Candidate => "candidate",
            SampleRole::Incumbent => "incumbent",
            SampleRole::Baseline => "baseline",
            SampleRole::Holdout => "holdout",
            SampleRole::Final => "final",
        },
        req.k
    ));

    // Start and wait, at most INNER_CONCURRENCY in flight. Every inner study started is
    // remembered: whatever happens to this sample, each one is stopped if still running and
    // its spend lands in the sample row (R-W7-2).
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(INNER_CONCURRENCY));
    let started_slugs = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let mut set = tokio::task::JoinSet::new();
    for name in names {
        let Some(f) = fixture(name) else { continue };
        for seed in 1..=suite.seeds {
            let mut slug = inner_slug(&study.slug, req.n, req.role, req.k, f.name, seed);
            let mut i = 2;
            while env.store.get_study(&slug).await?.is_some() {
                slug = inner_slug(&study.slug, req.n, req.role, req.k * 100 + i, f.name, seed);
                i += 1;
            }
            let repo = run_dir.join(format!("{}-s{seed}", f.name));
            let (env, outer, suite, sha, sem, cancel, started_slugs) = (
                env.clone(),
                study.clone(),
                suite.clone(),
                sha.clone(),
                sem.clone(),
                cancel.clone(),
                started_slugs.clone(),
            );
            set.spawn(async move {
                let _permit = sem.acquire().await;
                let started = async {
                    if cancel.is_cancelled() {
                        return Err(Error::refused("cancelled before the inner study started"));
                    }
                    materialise(f, &repo)?;
                    let spec = inner_spec(f, &slug, &repo, &suite, &outer.spec, hseed);
                    started_slugs.lock().push(slug.clone());
                    start_inner(&env, spec, created_by(&outer.slug, f.name, seed), &sha).await
                }
                .await;
                let r = match started {
                    Ok(_) => wait_terminal(&env, &slug, &cancel).await,
                    Err(e) => Err(e),
                };
                if r.is_err() {
                    stop_now(&env, &slug).await;
                }
                (f.name, seed, slug, r)
            });
        }
    }
    let mut inner: Vec<Inner> = Vec::new();
    while let Some(joined) = set.join_next().await {
        let Ok((fixture_name, seed, slug, r)) = joined else {
            continue;
        };
        let scored = match r {
            Ok(rec) => score_inner(&env.store, &rec, best.get(fixture_name).copied()).await,
            Err(e) => Err(e),
        };
        inner.push(match scored {
            Ok(sc) => sc,
            Err(e) => {
                tracing::warn!(%slug, "inner study did not score: {e}");
                Inner {
                    slug,
                    fixture: fixture_name.to_owned(),
                    seed,
                    state: "failed".into(),
                    ..Inner::default()
                }
            }
        });
    }
    // Spend of every inner study this sample started, scored or not.
    let started_slugs = started_slugs.lock().clone();
    for i in &mut inner {
        if let Ok(Some(rec)) = env.store.get_study(&i.slug).await {
            i.spend_usd = crate::actor::study_spend(env, &rec).await.unwrap_or(0.0);
        }
    }
    let spend_usd: f64 = inner
        .iter()
        .filter(|i| started_slugs.contains(&i.slug))
        .map(|i| i.spend_usd)
        .sum();
    let spend_nanos = (spend_usd * 1e9).round().max(0.0) as u64;
    if cancel.is_cancelled() {
        // Interrupted: the row still carries what the inner studies spent.
        return Ok(Sample {
            n: req.n,
            k: req.k,
            tree_sha: req.tree_sha.to_owned(),
            role: Some(req.role),
            lost: true,
            wall_ms: started.elapsed().as_millis() as u64,
            failure: Some("cancelled while inner studies ran".into()),
            at: crate::store::now(),
            asi: BTreeMap::from([("mechanism".to_owned(), sha)]),
            spend_nanos,
            ..Sample::default()
        });
    }
    inner.sort_by(|a, b| (&a.fixture, a.seed).cmp(&(&b.fixture, b.seed)));

    let mut cases = BTreeMap::new();
    let mut case_values = BTreeMap::new();
    for i in &inner {
        let Some(s) = i.score else { continue };
        let id = case_id(&i.fixture, i.seed);
        let mut v = BTreeMap::from([("gain".to_owned(), s)]);
        if let Some(b) = i.base {
            v.insert("base".into(), b);
        }
        if let Some(l) = i.last {
            v.insert("final".into(), l);
        }
        if let Some(b) = i.best {
            v.insert("best".into(), b);
        }
        v.insert("keeps".into(), f64::from(i.keeps));
        case_values.insert(id.clone(), v);
        cases.insert(id, s);
    }
    let value = mean(&cases.values().copied().collect::<Vec<_>>());
    env.store
        .append_event(
            &study.slug,
            "suite",
            &serde_json::json!({
                "n": req.n, "k": req.k, "role": req.role, "mechanism": sha,
                "held_out": held_out, "value": value, "spend_usd": spend_usd, "inner": inner,
                "missing": missing,
            }),
        )
        .await?;
    let secondaries = BTreeMap::from([(
        "inner_keeps".to_owned(),
        f64::from(inner.iter().map(|i| i.keeps).sum::<u32>()),
    )]);
    Ok(Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        value,
        secondaries,
        cases,
        case_values,
        asi: BTreeMap::from([("mechanism".to_owned(), sha)]),
        exit: Some(if value.is_some() { 0 } else { 1 }),
        wall_ms: started.elapsed().as_millis() as u64,
        failure: value
            .is_none()
            .then(|| "no inner study produced a holdout score".to_owned()),
        at: crate::store::now(),
        spend_nanos,
        ..Sample::default()
    })
}

// ───────────────────────────── the decision ─────────────────────────────

/// The paired gate as a seam: evalcore's in production, a fake in tests (W2's bodies are
/// neutral stubs in this tree).
pub struct PairedGate {
    /// `evalcore::gate::paired`.
    pub paired: fn(&[Pair]) -> Option<Paired>,
    /// `evalcore::gate::decide`.
    pub decide: fn(&[Pair], &PairCfg) -> Decision,
}

static GATE: RwLock<PairedGate> = RwLock::new(PairedGate {
    paired: evalcore::gate::paired,
    decide: evalcore::gate::decide,
});

/// Swap the paired gate (tests).
#[cfg(test)]
pub(crate) fn set_gate(g: PairedGate) {
    *GATE.write().unwrap_or_else(|e| e.into_inner()) = g;
}

/// Per-case means over a tree's samples.
fn per_case(samples: &[Sample]) -> BTreeMap<String, f64> {
    let mut acc: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in samples.iter().filter(|s| !s.lost && s.value.is_some()) {
        for (id, v) in &s.cases {
            acc.entry(id.clone()).or_default().push(*v);
        }
    }
    acc.into_iter()
        .filter_map(|(id, vs)| mean(&vs).map(|m| (id, m)))
        .collect()
}

/// Pairs over the cases both trees scored.
pub fn pairs(candidate: &[Sample], reference: &[Sample]) -> Vec<Pair> {
    let c = per_case(candidate);
    let r = per_case(reference);
    c.into_iter()
        .filter_map(|(id, cand)| {
            r.get(&id).map(|base| Pair {
                id,
                baseline: *base,
                candidate: cand,
            })
        })
        .collect()
}

/// The kind's decision rule (`GateRule` dispatch in actor.rs): W2's paired gate over the
/// fixture × seed pairs; the incumbent's suite re-runs every third outer experiment.
pub async fn step(
    _env: &Env,
    study: &StudyRecord,
    cfg: &GateCfg,
    input: &GateInput,
    candidate: &[Sample],
    reference: &[Sample],
) -> Result<GateStep> {
    let fresh_incumbent = reference.iter().any(|s| {
        s.role == Some(SampleRole::Incumbent) && s.n == candidate.first().map_or(0, |c| c.n)
    });
    if input.incumbent_age >= REMEASURE_AGE && !fresh_incumbent {
        return Ok(GateStep::Confirm {
            more: 0,
            incumbent_sample: true,
        });
    }
    let pairs = pairs(candidate, reference);
    let ref_mean = mean(&pairs.iter().map(|p| p.baseline).collect::<Vec<_>>()).unwrap_or(0.0);
    let cand_mean = mean(&pairs.iter().map(|p| p.candidate).collect::<Vec<_>>()).unwrap_or(0.0);
    let pcfg = PairCfg {
        min_mean_delta: cfg.min_effect.against(ref_mean),
        max_case_regression: MAX_CASE_REGRESSION,
        min_pairs: 2,
    };
    let (paired, decision) = {
        let g = GATE.read().unwrap_or_else(|e| e.into_inner());
        ((g.paired)(&pairs), (g.decide)(&pairs, &pcfg))
    };
    let delta = cand_mean - ref_mean;
    let delta_pct = if ref_mean.abs() > 1e-12 {
        delta / ref_mean.abs() * 100.0
    } else {
        0.0
    };
    let t = paired.map(|p| p.t);
    let verdict = match &decision {
        Decision::Accept => Verdict::Improved,
        Decision::Reject(_) => {
            if paired.is_some_and(|p| p.t <= -p.critical) {
                Verdict::Worse
            } else {
                Verdict::NoGain
            }
        }
        Decision::Unverified(_) => Verdict::NoGain,
    };
    let call = match verdict {
        Verdict::Improved => Call::Better,
        Verdict::Worse => Call::Worse,
        _ => Call::Same,
    };
    let primary = study.spec.headline();
    Ok(GateStep::Decide {
        verdict,
        delta,
        delta_pct,
        z: t,
        detail: Some(GateDecision {
            v: GateDecision::V,
            rule: GateRule::Paired,
            objectives: vec![ObjectiveDelta {
                name: primary.name,
                role: ObjectiveRole::Primary,
                candidate: Some(cand_mean),
                reference: Some(ref_mean),
                delta: Some(delta),
                delta_pct: Some(delta_pct),
                z: t,
                call,
            }],
            stat: t,
            threshold: paired.map(|p| p.critical),
            n: pairs.len() as u32,
        }),
    })
}

// ───────────────────────────── views ─────────────────────────────

/// Mean per fixture over the given samples' cases.
fn per_fixture(samples: &[Sample]) -> BTreeMap<String, f64> {
    let mut acc: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (id, v) in per_case(samples) {
        let f = id.split('/').next().unwrap_or(&id).to_owned();
        acc.entry(f).or_default().push(v);
    }
    acc.into_iter()
        .filter_map(|(f, vs)| mean(&vs).map(|m| (f, m)))
        .collect()
}

struct Scores {
    /// Training: the incumbent mechanism's suite (its tree's samples).
    train_inc: BTreeMap<String, f64>,
    /// Training: the newest candidate experiment's suite.
    train_cand: BTreeMap<String, f64>,
    /// Held out: the base mechanism's holdout suite.
    held_base: BTreeMap<String, f64>,
    /// Held out: the newest holdout suite (the latest keep's), when one ran.
    held_last: BTreeMap<String, f64>,
    base_mean: Option<f64>,
    inc_mean: Option<f64>,
    n_base: u32,
    n_inc: u32,
}

async fn scores(store: &Store, study: &StudyRecord) -> Result<Scores> {
    let samples = store.list_samples(&study.slug, None).await?;
    let base = study.base_sha.clone().unwrap_or_default();
    let inc = study.incumbent_sha.clone().unwrap_or_default();
    let clean = |s: &&Sample| !s.lost && s.value.is_some();
    let hold: Vec<Sample> = samples
        .iter()
        .filter(|s| s.role == Some(SampleRole::Holdout))
        .filter(clean)
        .cloned()
        .collect();
    let search: Vec<Sample> = samples
        .iter()
        .filter(|s| s.role != Some(SampleRole::Holdout))
        .filter(clean)
        .cloned()
        .collect();
    let base_s: Vec<Sample> = search
        .iter()
        .filter(|s| s.tree_sha == base)
        .cloned()
        .collect();
    let inc_s: Vec<Sample> = search
        .iter()
        .filter(|s| s.tree_sha == inc)
        .cloned()
        .collect();
    let newest_cand = search
        .iter()
        .filter(|s| s.role == Some(SampleRole::Candidate))
        .map(|s| s.n)
        .max();
    let cand_s: Vec<Sample> = search
        .iter()
        .filter(|s| s.role == Some(SampleRole::Candidate) && Some(s.n) == newest_cand)
        .cloned()
        .collect();
    let held_base_s: Vec<Sample> = hold
        .iter()
        .filter(|s| s.tree_sha == base)
        .cloned()
        .collect();
    let newest_hold = hold.iter().filter(|s| s.n > 0).map(|s| s.n).max();
    let held_last_s: Vec<Sample> = hold
        .iter()
        .filter(|s| Some(s.n) == newest_hold)
        .cloned()
        .collect();
    Ok(Scores {
        train_inc: per_fixture(&inc_s),
        train_cand: per_fixture(&cand_s),
        held_base: per_fixture(&held_base_s),
        held_last: per_fixture(&held_last_s),
        base_mean: mean(&base_s.iter().filter_map(|s| s.value).collect::<Vec<_>>()),
        inc_mean: mean(&inc_s.iter().filter_map(|s| s.value).collect::<Vec<_>>()),
        n_base: base_s.len() as u32,
        n_inc: inc_s.len() as u32,
    })
}

/// `KindView::Suite` for the Study view: per fixture, the incumbent mechanism's score and
/// the newest candidate's; held-out rows carry the base holdout and the latest holdout.
pub async fn view(store: &Store, study: &StudyRecord) -> Result<KindView> {
    let suite = suite_of(&study.spec);
    let sc = scores(store, study).await?;
    let mut fixtures: Vec<SuiteRow> = suite
        .training
        .iter()
        .map(|f| SuiteRow {
            fixture: f.clone(),
            held_out: false,
            incumbent: sc.train_inc.get(f).copied(),
            candidate: sc.train_cand.get(f).copied(),
        })
        .collect();
    fixtures.extend(suite.held_out.iter().map(|f| SuiteRow {
        fixture: f.clone(),
        held_out: true,
        incumbent: sc.held_base.get(f).copied(),
        candidate: sc.held_last.get(f).copied(),
    }));
    Ok(KindView::Suite { fixtures })
}

/// The brief's per-kind section: the training scores and what the mechanism files are.
/// Held-out scores never appear here.
pub async fn brief_section(store: &Store, study: &StudyRecord) -> Result<String> {
    let suite = suite_of(&study.spec);
    let sc = scores(store, study).await?;
    let mut out = String::from(
        "\nSuite (training fixtures; score = normalised holdout gain of the inner studies, 1.0 = the reference solution):\n",
    );
    for f in &suite.training {
        let inc = sc
            .train_inc
            .get(f)
            .map_or("—".to_owned(), |v| format!("{v:.2}"));
        let cand = sc
            .train_cand
            .get(f)
            .map_or(String::new(), |v| format!(" · last candidate {v:.2}"));
        out.push_str(&format!("- {f}: incumbent {inc}{cand}\n"));
    }
    out.push_str(
        "The mechanism is four files in the worktree: protocol.md (the brief's protocol; `{crash_retries}` is filled), \
         fire.md (the fire prompt; `{slug}` is filled), advisor.md (the advisor's system prompt) and mechanism.yaml \
         (version, brief caps at most the compiled ones, plateau.window 4–48, novelty.threshold 0.3–0.95, \
         defaults.crash_retries 0–5, categories). Unknown keys are refused before any inner study runs: the gate, \
         scoring, custody and budgets are compiled. Each outer experiment runs the whole training suite, so make one \
         deliberate change and bump `version`.\n",
    );
    Ok(out)
}

/// Finalize's headline: `suite +0.12 over mechanism v1 (held out +0.07)`; a kept mechanism is
/// registered as the next version, ready for the adopt press.
pub async fn headline(store: &Store, study: &StudyRecord) -> Result<Option<Headline>> {
    let sc = scores(store, study).await?;
    let base = sc.base_mean.unwrap_or(0.0);
    let last = sc.inc_mean.unwrap_or(base);
    let gain = last - base;
    let held = {
        let ids: Vec<&String> = sc.held_base.keys().collect();
        let b = mean(
            &ids.iter()
                .filter_map(|f| sc.held_base.get(*f))
                .copied()
                .collect::<Vec<_>>(),
        );
        let l = mean(
            &ids.iter()
                .filter_map(|f| sc.held_last.get(*f))
                .copied()
                .collect::<Vec<_>>(),
        );
        match (b, l) {
            (Some(b), Some(l)) => Some((b, l)),
            _ => None,
        }
    };
    let repo = Path::new(&study.repo);
    let root = repo
        .parent()
        .filter(|_| repo.file_name().is_some_and(|f| f == crate::mechanism::DIR));
    let against = root
        .and_then(crate::mechanism::active)
        .map_or("the compiled mechanism".to_owned(), |(v, _)| {
            format!("mechanism v{v}")
        });
    let mut text = format!("suite {gain:+.2} over {against}");
    if let Some((b, l)) = held {
        text.push_str(&format!(" (held out {:+.2})", l - b));
    }
    let kept = study
        .incumbent_sha
        .as_deref()
        .filter(|s| Some(*s) != study.base_sha.as_deref());
    if let (Some(root), Some(sha)) = (root, kept) {
        match crate::mechanism::register(root, sha, &text) {
            Ok(v) => text.push_str(&format!(" · mechanism v{v} ready to adopt")),
            Err(e) => tracing::warn!(slug = %study.slug, "mechanism not registered: {e}"),
        }
    } else if kept.is_none() {
        text.push_str(" · nothing adopted: no outer keep");
    }
    Ok(Some(Headline {
        text,
        base_mean: base,
        final_mean: last,
        delta_pct: if base.abs() > 1e-12 {
            gain / base.abs() * 100.0
        } else {
            0.0
        },
        holdout: held.map(|(b, l)| format!("{b:.2} → {l:.2} ({:+.2})", l - b)),
        sigma: 0.0,
        n: (sc.n_base, sc.n_inc),
    }))
}

#[cfg(test)]
mod tests {
    //! P: the score, the pairs and the step through a fake paired gate, the setup rows; T
    //! (automated): an outer suite study driven through a test `Backend` — real inner
    //! studies in the real ledger, scripted samples, a fake scheduler in place of the
    //! schedule actor — baseline, a candidate keep, the held-out holdout, the headline and
    //! the version registered for the adopt press.

    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use parking_lot::Mutex;
    use runtime::SessionId;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::Studies;
    use crate::actor::{Backend, Env};
    use crate::config::{AgentDefaults, AutoresearchConfig};
    use crate::custody::ScopeDiff;
    use crate::git::KeepMeta;
    use crate::novelty::Novelty;
    use crate::runner::CommandRun;
    use crate::store::{Store, StudyPatch};
    use crate::types::{
        Begin, Brief, Decision as LogDecision, ExperimentState, IdeaDraft, Log, LogArgs,
        ReviewVerdict, Run, RunArgs, StopReason, StudyState,
    };

    fn s(id: &str, cases: &[(&str, f64)], role: SampleRole, n: u32) -> Sample {
        Sample {
            n,
            k: 0,
            tree_sha: id.into(),
            role: Some(role),
            value: Some(cases.iter().map(|c| c.1).sum::<f64>() / cases.len() as f64),
            cases: cases.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            exit: Some(0),
            ..Sample::default()
        }
    }

    /// A paired gate that accepts when the mean delta clears `min_mean_delta`, with t = mean/sd·√n.
    fn fake_gate() -> PairedGate {
        fn paired(pairs: &[Pair]) -> Option<Paired> {
            if pairs.len() < 2 {
                return None;
            }
            let d: Vec<f64> = pairs.iter().map(|p| p.candidate - p.baseline).collect();
            let n = d.len() as f64;
            let mean = d.iter().sum::<f64>() / n;
            let sd = (d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
            Some(Paired {
                n: pairs.len(),
                mean,
                sd,
                t: if sd > 0.0 {
                    mean / (sd / n.sqrt())
                } else {
                    mean.signum() * 99.0
                },
                critical: 2.0,
                min_delta: d.iter().copied().fold(f64::INFINITY, f64::min),
            })
        }
        fn decide(pairs: &[Pair], cfg: &PairCfg) -> Decision {
            match paired(pairs) {
                None => Decision::Unverified("few".into()),
                Some(p)
                    if p.mean >= cfg.min_mean_delta && p.min_delta >= -cfg.max_case_regression =>
                {
                    Decision::Accept
                }
                Some(_) => Decision::Reject("no".into()),
            }
        }
        PairedGate { paired, decide }
    }

    #[test]
    fn score_normalises_the_holdout_gain_against_the_reference() {
        assert!((score(100.0, 50.0, Some(10.0)).unwrap() - 50.0 / 90.0).abs() < 1e-12);
        assert!(
            (score(100.0, 50.0, None).unwrap() - 0.5).abs() < 1e-12,
            "no reference: relative"
        );
        assert!(
            (score(100.0, 50.0, Some(120.0)).unwrap() - 0.5).abs() < 1e-12,
            "a worse reference is ignored"
        );
        assert!(
            (score(100.0, 120.0, Some(10.0)).unwrap() + 20.0 / 90.0).abs() < 1e-12,
            "a regression is negative"
        );
        assert_eq!(score(0.0, 0.0, None), None);
        assert_eq!(score(f64::NAN, 1.0, None), None);
    }

    #[test]
    fn pairs_use_the_cases_both_trees_scored_and_average_the_reference() {
        let cand = vec![s(
            "exp:1",
            &[("a/1", 0.6), ("a/2", 0.7)],
            SampleRole::Candidate,
            1,
        )];
        let refs = vec![
            s(
                "base",
                &[("a/1", 0.4), ("a/2", 0.5), ("b/1", 0.1)],
                SampleRole::Baseline,
                0,
            ),
            s(
                "base",
                &[("a/1", 0.6), ("a/2", 0.5)],
                SampleRole::Baseline,
                0,
            ),
        ];
        let p = pairs(&cand, &refs);
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].id.as_str(), p[0].baseline, p[0].candidate),
            ("a/1", 0.5, 0.6)
        );
        assert_eq!(
            (p[1].id.as_str(), p[1].baseline, p[1].candidate),
            ("a/2", 0.5, 0.7)
        );
    }

    #[test]
    fn setup_rows_round_trip_and_refuse_unknown_fixtures() {
        let mut spec = preset("/x/mechanism");
        assert_eq!(spec.kind(), crate::types::KIND_META);
        crate::tools::validate_spec(&spec).unwrap();
        assert!(setup_value(&spec, "suite").contains("training toy-sort, toy-dedupe"));
        setup_set(&mut spec, "suite.training", "toy-dedupe, toy-lru").unwrap();
        assert_eq!(suite_of(&spec).training, vec!["toy-dedupe", "toy-lru"]);
        let e = setup_set(&mut spec, "suite.training", "toy-nope").unwrap_err();
        assert!(e.contains("unknown fixture `toy-nope`"), "{e}");
        setup_set(&mut spec, "suite.held_out", "toy-json").unwrap();
        assert_eq!(suite_of(&spec).held_out, vec!["toy-json"]);
        assert_eq!(spec.holdout.as_ref().map(|h| h.every), Some(2));
        setup_set(&mut spec, "suite.executor", "cheap-model").unwrap();
        assert_eq!(suite_of(&spec).executor.as_deref(), Some("cheap-model"));
        assert!(setup_value(&spec, "suite").ends_with("on cheap-model"));
        assert!(setup_set(&mut spec, "suite.bogus", "x").is_err());
        setup_set(&mut spec, "suite.seeds", "3").unwrap();
        setup_set(&mut spec, "suite.inner_experiments", "4").unwrap();
        assert!(setup_set(&mut spec, "suite.seeds", "0").is_err());
        assert!(setup_set(&mut spec, "suite.inner_experiments", "many").is_err());
        let s = suite_of(&spec);
        assert_eq!((s.seeds, s.inner_experiments), (3, 4));
        assert_eq!(
            spec.suite.as_ref().unwrap().training,
            vec!["toy-dedupe", "toy-lru"]
        );
        // The block survives the wire and a 140 spec never carries it.
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(
            serde_json::from_str::<StudySpec>(&json).unwrap().suite,
            spec.suite
        );
        assert!(
            !serde_json::to_string(&StudySpec::default())
                .unwrap()
                .contains("suite")
        );
        // Zeros and empty lists read as the §12/§14 defaults.
        let bare = StudySpec {
            suite: Some(SuiteSpec::default()),
            ..preset("/x")
        };
        assert_eq!(suite_of(&bare).training, TRAINING.to_vec());
        assert_eq!(
            (suite_of(&bare).seeds, suite_of(&bare).inner_experiments),
            (SEEDS, INNER_EXPERIMENTS)
        );
        assert!(
            card_line(&preset("/x"))
                .starts_with("Improve the study mechanism on 4 training fixtures (3 held out)")
        );
        assert!(card_line(&preset("/x")).contains("8 inner studies"));
    }

    #[test]
    fn inner_slugs_fit_the_slug_rule() {
        let long = "a-very-long-outer-study-slug-xx";
        for role in [
            SampleRole::Candidate,
            SampleRole::Holdout,
            SampleRole::Baseline,
        ] {
            let slug = inner_slug(long, 12, role, 3, "toy-wordfreq", 2);
            crate::tools::check_slug(&slug).unwrap();
            assert!(slug.ends_with("-wordfreq-s2"), "{slug}");
        }
        assert_eq!(
            inner_slug("meta", 1, SampleRole::Candidate, 0, "toy-dedupe", 1),
            "meta-1c0-dedupe-s1"
        );
    }

    /// R-W7-1 and R-W7-6: an inner repo carries the fixture but never `reference/`; every
    /// fixture has a reference for `best_known`; the holdout reads its seed from the
    /// environment, so two outer studies score on different held-out inputs.
    #[test]
    fn inner_repos_carry_no_answer_key_and_the_holdout_seed_lives_outside_them() {
        let d = tempfile::tempdir().unwrap();
        for f in FIXTURES {
            let dir = d.path().join(f.name);
            materialise(f, &dir).unwrap();
            assert!(dir.join(".git").exists());
            assert!(dir.join(f.scope).exists());
            assert!(
                !dir.join("reference").exists(),
                "{}: the answer key is not in the inner repo",
                f.name
            );
            assert!(
                crate::git::read(&dir, &["ls-files"])
                    .unwrap()
                    .lines()
                    .all(|l| !l.starts_with("reference/")),
                "{}: nor in its history",
                f.name
            );
            assert!(
                f.files
                    .iter()
                    .any(|(p, ..)| *p == format!("reference/{}", f.scope)),
                "{} has a reference for best_known",
                f.name
            );
            let holdout = f.files.iter().find(|(p, ..)| *p == "holdout.sh").unwrap().1;
            assert!(holdout.contains("AUTORESEARCH_HOLDOUT_SEED"), "{}", f.name);
            for name in ["bench.sh", "test.sh", "holdout.sh"] {
                assert!(dir.join(name).exists());
            }
        }
        assert_eq!(
            fixture("toy-pareto").map(|f| f.name),
            None,
            "W1's fixture is a hand-off row"
        );
        // Custody: every fixture file the inner session could game is an evaluator path and
        // off limits, so an inner keep that edits one is refused (140 §5.3); the holdout seed
        // is in the command the plugin runs, not in any file of the repo.
        let f = fixture("toy-lru").unwrap();
        let spec = inner_spec(
            f,
            "x",
            d.path(),
            &suite_of(&preset("/x")),
            &preset("/x"),
            4242,
        );
        for p in ["bench.sh", "test.sh", "holdout.sh", "fixtures/"] {
            assert!(spec.evaluator_paths.iter().any(|e| e == p), "{p}");
        }
        assert!(!spec.evaluator_paths.iter().any(|e| e.contains("reference")));
        assert_eq!(
            spec.holdout.as_ref().unwrap().command,
            "AUTORESEARCH_HOLDOUT_SEED=4242 bash holdout.sh"
        );
        assert_eq!(spec.scope, vec!["lru.py"]);
        crate::tools::validate_spec(&spec).unwrap();
        // The seed is drawn once per outer study and reread.
        let s1 = holdout_seed(d.path());
        assert!(s1 > 0 && s1 == holdout_seed(d.path()));
        // Two seeds, two different held-out inputs (python3, as every fixture run here).
        let run = |seed: &str| {
            let out = std::process::Command::new("bash")
                .arg("holdout.sh")
                .current_dir(d.path().join("toy-dedupe"))
                .env("AUTORESEARCH_HOLDOUT_SEED", seed)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.starts_with("CASE "))
                .map(|l| l.split(' ').nth(1).unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let (a, b) = (run("0"), run("17"));
        assert_eq!(a.len(), 4);
        assert_ne!(a, b, "a different seed is a different holdout");
        assert!(
            b.iter().all(|c| c.ends_with("118")
                || c.ends_with("219")
                || c.ends_with("320")
                || c.ends_with("421")),
            "{b:?}"
        );
    }

    // ───────────────────────── the double and the harness ─────────────────────────

    /// What the scripted engine answers: inner samples by slug and role, keeps for every
    /// inner experiment, a growing session cost so spend rolls up, the reference holdout.
    #[derive(Default)]
    struct Double {
        cost: Mutex<BTreeMap<String, u32>>,
    }

    impl Double {
        /// Inner benches: 100 for base trees, 80 for candidates. Inner holdouts: 100 at the base
        /// (n = 0); after a keep 40 under a candidate mechanism (slug marker `c`), 50 otherwise.
        fn value(slug: &str, req: &SampleRequest<'_>) -> f64 {
            let candidate_mech = slug.split('-').nth(1).is_some_and(|t| t.contains('c'));
            match req.role {
                SampleRole::Holdout if req.n == 0 => 100.0,
                SampleRole::Holdout if candidate_mech => 40.0,
                SampleRole::Holdout => 50.0,
                SampleRole::Candidate => 80.0,
                _ => 100.0,
            }
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
            base_sha: &str,
        ) -> Result<String> {
            std::fs::create_dir_all(worktree)?;
            Ok(base_sha.to_owned())
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
            Ok(format!("keep{:04}", meta.n))
        }
        fn discard(&self, _: &Path, _: &str, patch_path: &Path) -> Result<String> {
            std::fs::write(patch_path, "diff")?;
            Ok("patchhash".into())
        }
        fn apply_patch(&self, _: &Path, _: &Path) -> Result<()> {
            Ok(())
        }
        fn discard_study(&self, _: &Path, _: &str, _: &Path) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn head(&self, _: &Path) -> Result<String> {
            Ok("base0000".into())
        }
        fn commit_parent(&self, _: &Path, _: &str) -> Result<String> {
            Ok("base0000".into())
        }
        fn repair(&self, _: &Path, _: &StudySpec, _: &str, _: &Path) -> Result<Option<String>> {
            Ok(None)
        }
        fn reject(&self, _: &Path, _: &str, _: u32, _: &str) -> Result<()> {
            Ok(())
        }
        fn restore_rejected(&self, _: &Path, _: &str, _: u32) -> Result<()> {
            Ok(())
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
            let value = Some(Self::value(req.slug, &req));
            Ok(Sample {
                n: req.n,
                k: req.k,
                tree_sha: req.tree_sha.to_owned(),
                role: Some(req.role),
                value,
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
        fn tail(&self, _: &Path) -> String {
            String::new()
        }
        fn custody_take(
            &self,
            _: &StudySpec,
            _: &Path,
            dir: &Path,
        ) -> Result<BTreeMap<String, String>> {
            std::fs::create_dir_all(dir)?;
            Ok(BTreeMap::from([("bench.sh".to_owned(), "abc".to_owned())]))
        }
        fn custody_verify(&self, _: &BTreeMap<String, String>, _: &Path) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn scope_diff(&self, spec: &StudySpec, _: &Path) -> Result<ScopeDiff> {
            // Every candidate changed its first scope file by one line.
            Ok(ScopeDiff {
                changed: vec![spec.scope.first().cloned().unwrap_or_default()],
                added: 1,
                ..ScopeDiff::default()
            })
        }
        fn gate_step(&self, _: &GateCfg, _: &GateInput) -> GateStep {
            GateStep::Decide {
                verdict: Verdict::Improved,
                delta: 20.0,
                delta_pct: -20.0,
                z: Some(4.0),
                detail: None,
            }
        }
        fn keep_simpler(&self, _: &GateCfg, _: &GateInput, _: u32, _: u32) -> bool {
            false
        }
        fn drifted(&self, _: f64, _: f64, _: f64) -> bool {
            false
        }
        fn gate_line(&self, _: &GateStep, _: &str, _: f64, _: f64, _: usize) -> String {
            String::new()
        }
        async fn brief(
            &self,
            _: &Store,
            study: &StudyRecord,
            _: &crate::config::BriefConfig,
            cx: &crate::brief::Context,
        ) -> Result<Brief> {
            Ok(Brief {
                text: format!("<study slug=\"{}\">", study.slug),
                n: cx.n,
                segment: study.segment,
                slots: None,
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
        ) -> Result<Vec<IdeaDraft>> {
            Ok(vec![])
        }
        async fn finalize(
            &self,
            _: &Store,
            study: &StudyRecord,
            _: StopReason,
            _: &AutoresearchConfig,
            _: &Path,
            _: &runtime::Runtime,
        ) -> Result<(Headline, Option<String>)> {
            Ok((
                Headline {
                    text: format!("{} done", study.slug),
                    ..Headline::default()
                },
                None,
            ))
        }
        async fn session_cost(&self, session: Option<SessionId>) -> Result<f64> {
            // Every look at a session costs it a cent, so each closed inner experiment carries
            // some spend without any study ever reaching its $2.
            let Some(session) = session else {
                return Ok(0.0);
            };
            let mut cost = self.cost.lock();
            let n = cost.entry(session.to_string()).or_insert(0);
            *n += 1;
            Ok(f64::from(*n) * 0.01)
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
        async fn plugin_command(
            &self,
            _: &StudyRecord,
            what: &str,
            _: &str,
            _: Duration,
            extra_env: &[(String, String)],
        ) -> Result<CommandRun> {
            assert!(what.starts_with("reference-"), "{what}");
            assert!(
                extra_env
                    .iter()
                    .any(|(k, v)| k == "AUTORESEARCH_WORKTREE" && v.contains("/reference/"))
            );
            Ok(CommandRun {
                exit: Some(0),
                scanned: crate::scanner::Scanned {
                    metrics: BTreeMap::from([("time_ms".to_owned(), 10.0)]),
                    ..Default::default()
                },
                ..CommandRun::default()
            })
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        studies: Studies,
    }

    async fn harness(double: Arc<Double>) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let root = dir.path().join("autoresearch");
        let store = Store::open(&root).await.unwrap();
        let studies = Studies {
            data_dir: dir.path().to_path_buf(),
            root,
            cfg: AutoresearchConfig::default(),
            agent: AgentDefaults::default(),
            rt,
            store,
            backend: double,
        };
        Harness { _dir: dir, studies }
    }

    fn is_inner(s: &StudyRecord) -> bool {
        s.created_by
            .as_deref()
            .is_some_and(|c| c.starts_with("meta:"))
    }

    /// The schedule actor's part: every running inner study gets a session and one
    /// experiment per pass, until `stop`.
    async fn fake_scheduler(env: Env, stop: Arc<AtomicBool>) {
        let mut i = 0u32;
        while !stop.load(Ordering::SeqCst) {
            let mut progressed = false;
            for st in env.store.list_studies().await.unwrap() {
                if !is_inner(&st) || st.state != StudyState::Running {
                    continue;
                }
                let session = match st.session {
                    Some(x) => x,
                    None => {
                        let x = SessionId::new();
                        env.store
                            .patch_study(
                                &st.slug,
                                &StudyPatch {
                                    session: Some(Some(x)),
                                    ..StudyPatch::default()
                                },
                            )
                            .await
                            .unwrap();
                        x
                    }
                };
                if crate::actor::begin_env(
                    &env,
                    Begin {
                        session,
                        slot: None,
                    },
                )
                .await
                .is_err()
                {
                    continue;
                }
                i += 1;
                let run = Run {
                    session,
                    slot: None,
                    args: RunArgs {
                        idea: format!("idea {i}"),
                        category: "c".into(),
                        ..RunArgs::default()
                    },
                };
                if crate::actor::run_env(&env, run).await.is_ok() {
                    let log = Log {
                        session,
                        slot: None,
                        args: LogArgs {
                            description: "d".into(),
                            learned: "l".into(),
                            decision: LogDecision::Auto,
                            ..LogArgs::default()
                        },
                    };
                    let _ = crate::actor::log_env(&env, log).await;
                }
                // The fire's turn ends here: a deferred finalize (budget reached) runs now, as
                // `binding::on_turn_finished` would start it.
                if let Ok(Some(st)) = env.store.get_study(&st.slug).await
                    && st.state == StudyState::Finalizing
                {
                    let _ = crate::actor::begin_finalize(&env, st, StopReason::User).await;
                }
                progressed = true;
            }
            if !progressed {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    async fn wait_state(
        store: &Store,
        slug: &str,
        pred: impl Fn(&StudyState) -> bool,
    ) -> StudyRecord {
        for _ in 0..3000 {
            let s = store.get_study(slug).await.unwrap().unwrap();
            if pred(&s.state) {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut dump = Vec::new();
        for s in store.list_studies().await.unwrap() {
            let exps = store
                .list_experiments(&s.slug, None, u32::MAX)
                .await
                .unwrap();
            let events = store.list_events(&s.slug, None, 3).await.unwrap();
            dump.push(format!(
                "{} {:?} exps={} session={:?} last_events={:?}",
                s.slug,
                s.state,
                exps.len(),
                s.session,
                events
                    .iter()
                    .map(|e| (e.kind.clone(), e.payload.to_string()))
                    .collect::<Vec<_>>()
            ));
        }
        panic!(
            "study `{slug}` never reached the state:\n{}",
            dump.join("\n")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn outer_suite_study_runs_inner_studies_decides_and_registers_a_version() {
        tokio::time::timeout(Duration::from_secs(120), outer_suite_study_body())
            .await
            .expect("the outer suite study hung");
    }

    async fn outer_suite_study_body() {
        set_gate(fake_gate());
        let double = Arc::new(Double::default());
        let mut h = harness(double.clone()).await;
        let env = h.studies.env();
        let repo = crate::mechanism::ensure(&env.root).unwrap();

        let mut spec = preset(&repo.display().to_string());
        setup_set(&mut spec, "suite.training", "toy-dedupe").unwrap();
        setup_set(&mut spec, "suite.held_out", "toy-lru").unwrap();
        spec.holdout.as_mut().unwrap().every = 1;
        spec.budget.experiments = 2;
        crate::tools::validate_spec(&spec).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let sched = tokio::spawn(fake_scheduler(env.clone(), stop.clone()));

        let origin = SessionId::new();
        crate::actor::create(
            &mut h.studies,
            crate::types::Create {
                spec,
                origin,
                start: StartWhen::Now,
            },
        )
        .await
        .unwrap();
        let outer = wait_state(&env.store, "meta", |s| *s == StudyState::Running).await;
        assert_eq!(outer.incumbent_sha.as_deref(), Some("base0000"));
        // Baseline: 2 suite runs × (1 fixture × 2 seeds) at the compiled mechanism, each
        // inner study's holdout 100 → 50 against a reference of 10: s = 50/90.
        let base_samples: Vec<Sample> = env.store.list_samples("meta", Some(0)).await.unwrap();
        let baseline: Vec<&Sample> = base_samples
            .iter()
            .filter(|s| s.role == Some(SampleRole::Baseline))
            .collect();
        assert_eq!(baseline.len(), 2);
        for b in &baseline {
            assert_eq!(b.cases.len(), 2, "{:?}", b.cases);
            assert!((b.cases["toy-dedupe/1"] - 50.0 / 90.0).abs() < 1e-9);
            assert!((b.value.unwrap() - 50.0 / 90.0).abs() < 1e-9);
            assert_eq!(b.asi["mechanism"], "base0000");
        }
        let held: Vec<&Sample> = base_samples
            .iter()
            .filter(|s| s.role == Some(SampleRole::Holdout))
            .collect();
        assert_eq!(held.len(), 1, "holdout.repeats = 1");
        assert!(held[0].cases.contains_key("toy-lru/1"));

        // The outer session: experiment 1 is kept by the paired gate (d = +0.11 on every
        // fixture × seed); experiment 2 measures the same candidate against the new incumbent
        // (d = 0) and is no gain; then the budget stops the study.
        let session = SessionId::new();
        env.store
            .patch_study(
                "meta",
                &StudyPatch {
                    session: Some(Some(session)),
                    ..StudyPatch::default()
                },
            )
            .await
            .unwrap();
        for n in 1..=2u32 {
            crate::actor::begin_env(
                &env,
                Begin {
                    session,
                    slot: None,
                },
            )
            .await
            .unwrap();
            let report = crate::actor::run_env(
                &env,
                Run {
                    session,
                    slot: None,
                    args: RunArgs {
                        idea: format!("bolder protocol {n}"),
                        category: "protocol".into(),
                        ..RunArgs::default()
                    },
                },
            )
            .await
            .unwrap();
            let want = if n == 1 {
                Verdict::Improved
            } else {
                Verdict::NoGain
            };
            assert_eq!(report.verdict, want, "{report:?}");
            let lr = crate::actor::log_env(
                &env,
                Log {
                    session,
                    slot: None,
                    args: LogArgs {
                        description: "d".into(),
                        learned: "l".into(),
                        decision: LogDecision::Auto,
                        ..LogArgs::default()
                    },
                },
            )
            .await
            .unwrap();
            let state = if n == 1 {
                ExperimentState::Kept
            } else {
                ExperimentState::Discarded
            };
            assert_eq!(lr.applied, state, "{lr:?}");
        }
        // The outer fire's turn ends: the deferred finalize (budget reached) runs.
        let finalizing = wait_state(&env.store, "meta", |s| *s == StudyState::Finalizing).await;
        crate::actor::begin_finalize(&env, finalizing, StopReason::User)
            .await
            .unwrap();
        let outer = wait_state(&env.store, "meta", |s| *s == StudyState::Finished).await;
        stop.store(true, Ordering::SeqCst);
        sched.await.unwrap();

        // The ledger: the kept experiment carries the paired decision over 2 pairs.
        let e1 = env.store.get_experiment("meta", 1).await.unwrap().unwrap();
        assert_eq!(e1.state, ExperimentState::Kept);
        let gate = e1.gate.expect("a paired decision");
        assert_eq!((gate.rule, gate.n, gate.v), (GateRule::Paired, 2, 2));
        assert!(
            (gate.objectives[0].delta.unwrap() - 10.0 / 90.0).abs() < 1e-9,
            "{gate:?}"
        );
        let cand: Vec<Sample> = env.store.list_samples("meta", Some(1)).await.unwrap();
        let c = cand
            .iter()
            .find(|s| s.role == Some(SampleRole::Candidate))
            .unwrap();
        assert!((c.cases["toy-dedupe/2"] - 60.0 / 90.0).abs() < 1e-9);
        assert_eq!(
            c.asi["mechanism"], "exp:1",
            "the candidate tree is what the inner studies pinned"
        );
        assert!(
            c.spend_nanos > 0,
            "inner spend rolls into the outer bench channel"
        );
        assert!(env.store.bench_spend("meta").await.unwrap() > 0);
        assert_eq!(c.case_values["toy-dedupe/1"]["best"], 10.0);

        // Inner studies: ordinary, finished, pinned, not the user's.
        let inner: Vec<StudyRecord> = env
            .store
            .list_studies()
            .await
            .unwrap()
            .into_iter()
            .filter(is_inner)
            .collect();
        assert!(inner.len() >= 12, "{}", inner.len());
        assert!(
            inner.iter().all(|s| s.state == StudyState::Finished),
            "{:?}",
            inner
                .iter()
                .map(|s| (&s.slug, &s.state))
                .collect::<Vec<_>>()
        );
        assert!(
            inner
                .iter()
                .all(|s| s.spec.axes == crate::types::Axes::default()
                    && s.spec.budget.experiments == INNER_EXPERIMENTS)
        );
        let pinned: std::collections::BTreeSet<&str> = inner
            .iter()
            .filter_map(|s| s.mechanism.as_deref())
            .collect();
        assert!(
            pinned.contains("base0000") && pinned.contains("exp:1"),
            "{pinned:?}"
        );
        assert!(inner.iter().all(|s| {
            s.created_by
                .as_deref()
                .unwrap()
                .starts_with("meta:meta/toy-")
        }));

        // Views: training rows for the session's eyes, held-out rows for the Study view only.
        let KindView::Suite { fixtures } = view(&env.store, &outer).await.unwrap() else {
            panic!()
        };
        assert_eq!(fixtures.len(), 2);
        assert!(!fixtures[0].held_out && fixtures[0].fixture == "toy-dedupe");
        assert!((fixtures[0].candidate.unwrap() - 60.0 / 90.0).abs() < 1e-9);
        assert!(fixtures[1].held_out && fixtures[1].fixture == "toy-lru");
        assert!(fixtures[1].incumbent.is_some() && fixtures[1].candidate.is_some());
        let brief = brief_section(&env.store, &outer).await.unwrap();
        assert!(brief.contains("toy-dedupe: incumbent") && brief.contains("mechanism.yaml"));
        assert!(
            !brief.contains("toy-lru"),
            "held-out never reaches the brief: {brief}"
        );

        // The headline registers the kept mechanism as v2, ready for the adopt press.
        let hl = headline(&env.store, &outer).await.unwrap().unwrap();
        assert!(
            hl.text
                .starts_with("suite +0.11 over the compiled mechanism (held out"),
            "{}",
            hl.text
        );
        assert!(
            hl.text.ends_with("mechanism v2 ready to adopt"),
            "{}",
            hl.text
        );
        let info = crate::mechanism::rpc(&env.root, crate::types::MechanismOp::List)
            .await
            .unwrap();
        assert_eq!(info.versions[0].version, 2);
        assert_eq!(info.versions[0].sha, outer.incumbent_sha.unwrap());
        assert_eq!(info.active, None, "adoption is a press, not the finish");
    }

    /// A stored outer meta study and a candidate sample request on it.
    async fn outer_for_sample(h: &Harness, seeds: u32, inner_experiments: u32) -> StudyRecord {
        let env = h.studies.env();
        let repo = crate::mechanism::ensure(&env.root).unwrap();
        let mut spec = preset(&repo.display().to_string());
        setup_set(&mut spec, "suite.training", "toy-dedupe").unwrap();
        setup_set(&mut spec, "suite.held_out", "toy-lru").unwrap();
        spec.suite.as_mut().unwrap().seeds = seeds;
        spec.suite.as_mut().unwrap().inner_experiments = inner_experiments;
        let mut study = StudyRecord::new(spec, None);
        study.state = StudyState::Running;
        study.base_sha = Some("base0000".into());
        study.incumbent_sha = Some("base0000".into());
        env.store.create_study(&study).await.unwrap();
        study
    }

    /// R-W7-2: a suite run whose inner budget exceeds `per_experiment_usd` is refused before
    /// any inner study exists.
    #[tokio::test]
    async fn a_suite_run_over_the_per_experiment_cap_starts_nothing() {
        tokio::time::timeout(Duration::from_secs(10), cap_body())
            .await
            .expect("the cap test hung: inner studies were started");
    }

    async fn cap_body() {
        let h = harness(Arc::new(Double::default())).await;
        let env = h.studies.env();
        let study = outer_for_sample(&h, 50, 8).await; // 1 fixture × 50 seeds × $2 = $100 > $20
        let dir = h._dir.path().join("wt");
        std::fs::create_dir_all(&dir).unwrap();
        let req = SampleRequest {
            slug: "meta",
            n: 1,
            k: 0,
            tree_sha: "exp:1",
            role: SampleRole::Candidate,
            worktree: &dir,
            evaluator: &dir,
            runs_dir: &dir,
        };
        let e = sample(&env, &study, req, &CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("over budget.per_experiment_usd"), "{e}");
        assert!(
            env.store
                .list_studies()
                .await
                .unwrap()
                .iter()
                .all(|s| !is_inner(s)),
            "no inner study was created"
        );
    }

    /// R-W7-2: a cancelled suite sample stops the inner studies it started and still returns
    /// a (lost) row carrying what they spent, so the outer money stop sees it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cancelled_suite_sample_stops_its_inner_studies_and_reports_their_spend() {
        tokio::time::timeout(Duration::from_secs(60), cancelled_body())
            .await
            .expect("the cancelled sample hung");
    }

    async fn cancelled_body() {
        let double = Arc::new(Double::default());
        let h = harness(double.clone()).await;
        let env = h.studies.env();
        // 40 inner experiments each: far more than the scheduler can finish before the cancel.
        let study = outer_for_sample(&h, 2, 40).await;
        let stop = Arc::new(AtomicBool::new(false));
        let sched = tokio::spawn(fake_scheduler(env.clone(), stop.clone()));
        let cancel = CancellationToken::new();
        let canceller = {
            let (cancel, env) = (cancel.clone(), env.clone());
            tokio::spawn(async move {
                // Cancel once both inner studies run and have closed an experiment.
                for _ in 0..500 {
                    let inner: Vec<StudyRecord> = env
                        .store
                        .list_studies()
                        .await
                        .unwrap()
                        .into_iter()
                        .filter(is_inner)
                        .collect();
                    let mut closed = 0;
                    for s in &inner {
                        closed += env
                            .store
                            .list_experiments(&s.slug, None, u32::MAX)
                            .await
                            .unwrap()
                            .iter()
                            .filter(|e| e.state == ExperimentState::Kept)
                            .count();
                    }
                    if inner.len() == 2 && closed >= 2 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                cancel.cancel();
            })
        };
        let dir = h._dir.path().join("wt");
        std::fs::create_dir_all(&dir).unwrap();
        let req = SampleRequest {
            slug: "meta",
            n: 1,
            k: 0,
            tree_sha: "exp:1",
            role: SampleRole::Candidate,
            worktree: &dir,
            evaluator: &dir,
            runs_dir: &dir,
        };
        let s = sample(&env, &study, req, &cancel).await.unwrap();
        canceller.await.unwrap();
        stop.store(true, Ordering::SeqCst);
        sched.await.unwrap();
        assert!(s.lost && s.value.is_none() && s.exit.is_none(), "{s:?}");
        assert!(s.spend_nanos > 0, "the inner spend is on the row: {s:?}");
        let inner: Vec<StudyRecord> = env
            .store
            .list_studies()
            .await
            .unwrap()
            .into_iter()
            .filter(is_inner)
            .collect();
        assert_eq!(inner.len(), 2);
        for s in &inner {
            assert!(
                !matches!(
                    s.state,
                    StudyState::Running | StudyState::Preparing | StudyState::Paused { .. }
                ),
                "{} was stopped: {:?}",
                s.slug,
                s.state
            );
        }
        assert_eq!(
            env.store
                .list_events("meta", Some("suite"), 1)
                .await
                .unwrap()
                .len(),
            0,
            "no suite event for a lost sample"
        );
    }

    #[tokio::test]
    async fn step_asks_for_the_incumbent_rerun_and_maps_the_decision() {
        set_gate(fake_gate());
        let h = harness(Arc::new(Double::default())).await;
        let env = h.studies.env();
        let study = StudyRecord::new(preset("/x/mechanism"), None);
        let cfg = crate::actor::gate_cfg(&env.cfg, &study.spec).unwrap();
        let cand = vec![s(
            "exp:3",
            &[("a/1", 0.6), ("a/2", 0.7), ("b/1", 0.5)],
            SampleRole::Candidate,
            3,
        )];
        let refs = vec![s(
            "base",
            &[("a/1", 0.5), ("a/2", 0.5), ("b/1", 0.5)],
            SampleRole::Baseline,
            0,
        )];
        let input = |age: u32| GateInput {
            candidate: vec![0.6],
            incumbent: vec![0.5],
            sigma: 0.0,
            largest_accepted_delta: None,
            incumbent_age: age,
            keep_simpler: false,
            secondaries: vec![],
        };
        // Stale incumbent: one re-run of its suite first.
        assert_eq!(
            step(&env, &study, &cfg, &input(2), &cand, &refs)
                .await
                .unwrap(),
            GateStep::Confirm {
                more: 0,
                incumbent_sample: true
            }
        );
        // Fresh: decide. Mean d = 0.1 ≥ 0.02, no regression → Improved with the paired t.
        let GateStep::Decide {
            verdict,
            delta,
            z,
            detail,
            ..
        } = step(&env, &study, &cfg, &input(0), &cand, &refs)
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(verdict, Verdict::Improved);
        assert!((delta - 0.1).abs() < 1e-9);
        let d = detail.unwrap();
        assert_eq!((d.rule, d.n), (GateRule::Paired, 3));
        assert_eq!(d.objectives[0].name, METRIC);
        assert_eq!(d.objectives[0].call, Call::Better);
        assert!(z.is_some() && d.threshold == Some(2.0));
        // A regression on one fixture past the case limit: rejected, not kept.
        let worse = vec![s(
            "exp:4",
            &[("a/1", 0.9), ("a/2", 0.9), ("b/1", 0.1)],
            SampleRole::Candidate,
            4,
        )];
        let GateStep::Decide { verdict, .. } = step(&env, &study, &cfg, &input(0), &worse, &refs)
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(verdict, Verdict::NoGain);
        // No shared cases: no gain, n = 0.
        let none = vec![s("exp:5", &[("z/1", 0.9)], SampleRole::Candidate, 5)];
        let GateStep::Decide {
            verdict, detail, ..
        } = step(&env, &study, &cfg, &input(0), &none, &refs)
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!((verdict, detail.unwrap().n), (Verdict::NoGain, 0));
    }
}
