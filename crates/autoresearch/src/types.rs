//! The wire types of design doc 140 (§4.1 the spec, §4.2–4.3 states, §5.2 the
//! gate, §7 the brief, §12.3 messages and events, §12.4 tool arguments and
//! reports, §12.5 RPC shapes). Every type here derives Serialize /
//! Deserialize / JsonSchema and `ts_rs::TS` behind `feature = "ts"`; the
//! actor messages live at the bottom. Other modules add nothing public to
//! the wire: a new field is a change here, announced to every worker.

use std::collections::BTreeMap;

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// `#[derive]` set every wire type carries (`Default` added per type).
macro_rules! wire {
    ($(#[$m:meta])* pub struct $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub struct $name { $($body)* }
    };
    ($(#[$m:meta])* pub enum $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub enum $name { $($body)* }
    };
}

/// Idea row id (`ideas.id`).
pub type IdeaId = i64;
/// Steer row id (`steers.id`).
pub type SteerId = i64;

// ───────────────────────────── §4.1 the specification ─────────────────────────────

wire! {
    /// The study specification the user approves on the draft card (140 §4.1).
    /// Durations are humantime strings (`600s`, `5m`) as in the YAML of §4.1.
    #[serde(default)]
    pub struct StudySpec {
        /// `[a-z0-9-]{1,32}`; schedule = `autoresearch-<slug>`, branch `autoresearch/<slug>`.
        pub slug: String,
        /// What the user wants, one sentence.
        pub goal: String,
        /// Absolute path of the repository.
        pub repo: String,
        /// Ref resolved to a sha at Start (`HEAD` default).
        pub base: String,
        /// The primary metric.
        pub metric: MetricSpec,
        /// Secondaries a keep may not regress past their limit.
        pub secondary_guards: Vec<Guard>,
        /// The benchmark.
        pub bench: BenchSpec,
        /// Pass/fail correctness check, run only on would-be keeps.
        pub checks: Option<CommandSpec>,
        /// The holdout the study session never sees (140 §5.4).
        pub holdout: Option<HoldoutSpec>,
        /// Repo files the evaluator reads: custody + hashes (140 §5.3).
        pub evaluator_paths: Vec<String>,
        /// Globs where edits are expected.
        pub scope: Vec<String>,
        /// Globs that can never be kept.
        pub off_limits: Vec<String>,
        /// Run once at Start, unscored.
        pub warm: Option<String>,
        /// Money, time and count limits.
        pub budget: BudgetSpec,
        /// Candidates per fire; `> 1` = waves (140 §10).
        pub parallel: u8,
        /// Per-study model overrides; `None` = defaults (140 §9.3, R1-D5).
        pub models: ModelsSpec,
        /// A 40 `when:` sensor confining the study to an idle window (140 §5.5).
        pub window: Option<String>,
        /// §6 "include my uncommitted changes": a dirty checkout is snapshotted onto
        /// the study branch (base must be HEAD) instead of refusing Start.
        pub include_dirty: bool,
        // ── 159 §5.1: every field below is `default` and skipped at its default, so a 140 spec
        // serialises byte for byte as before and `DraftArgs` gains only optional keys.
        /// The six axes of 159 §3; `Default` = the 140 study.
        #[serde(skip_serializing_if = "Axes::is_default")]
        #[serde(default)]
        pub axes: Axes,
        /// Objectives with roles (159 §5.2); empty = one primary from `metric` plus the guards.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        #[serde(default)]
        pub objectives: Vec<Objective>,
        /// W2, W3: the case set.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub cases: Option<CaseSetSpec>,
        /// W2, W3: the judge.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub judge: Option<JudgeSpec>,
        /// W4: cells, islands, descriptors.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub population: Option<PopulationSpec>,
        /// W5: literature search.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub literature: Option<LiteratureSpec>,
        /// W5: before/after hooks.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub hooks: Option<HooksSpec>,
        /// W6: deploy, measure, rollback, exposure, windows.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub world: Option<WorldSpec>,
        /// W7: the fixture suite.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub suite: Option<SuiteSpec>,
        /// W5, W6: registry tools the plugin may call for this study (read-only risk only, W-d).
        #[serde(skip_serializing_if = "Vec::is_empty")]
        #[serde(default)]
        pub calls: Vec<String>,
    }
}

impl Default for StudySpec {
    fn default() -> Self {
        Self {
            slug: String::new(),
            goal: String::new(),
            repo: String::new(),
            base: "HEAD".into(),
            metric: MetricSpec::default(),
            secondary_guards: Vec::new(),
            bench: BenchSpec::default(),
            checks: None,
            holdout: None,
            evaluator_paths: Vec::new(),
            scope: Vec::new(),
            off_limits: Vec::new(),
            warm: None,
            budget: BudgetSpec::default(),
            parallel: 1,
            models: ModelsSpec::default(),
            window: None,
            include_dirty: false,
            axes: Axes::default(),
            objectives: Vec::new(),
            cases: None,
            judge: None,
            population: None,
            literature: None,
            hooks: None,
            world: None,
            suite: None,
            calls: Vec::new(),
        }
    }
}

// ───────────────────────────── 159 §5.1–§5.2: axes and objectives ─────────────────────────────

wire! {
    /// The six axes of 159 §3 (idea sources are the opt-in blocks `literature` and `hooks`;
    /// the measurement rule follows from `objectives` × `scoring`, see [`StudySpec::rule`]).
    #[derive(Copy, Default, Eq)]
    #[serde(default)]
    pub struct Axes {
        /// What counts as better.
        pub objectives: ObjectivesMode,
        /// How a tree becomes numbers.
        pub scoring: Scoring,
        /// What the next candidate is cut from (`ratchet` + `parallel > 1` = waves).
        pub search: Search,
        /// What the session edits.
        pub artefact: Artefact,
    }
}

impl Axes {
    /// The 140 study: `single · bench · ratchet · repo`.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

wire! {
    /// `axes.objectives`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum ObjectivesMode {
        /// One primary decides alone (140).
        #[default]
        Single,
        /// 2–5 competing objectives, Pareto keeps (W1).
        Pareto,
        /// A judged preference, no scalar (W3).
        Judged,
    }
}

wire! {
    /// `axes.scoring`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Scoring {
        /// The bench prints `METRIC` and `CASE` lines (140).
        #[default]
        Bench,
        /// An engine-side runner scores each case by checker or judge (W2).
        Cases,
        /// A blind judge of two outputs (W3).
        Pairwise,
        /// Deploy, wait, read both arms (W6).
        World,
        /// Inner fixture studies (W7).
        Suite,
    }
}

wire! {
    /// `axes.search`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Search {
        /// One incumbent; waves cut slots from it (140).
        #[default]
        Ratchet,
        /// Parents from the Pareto front (W1).
        Front,
        /// Cells, islands, sampled parents (W4).
        Population,
    }
}

wire! {
    /// `axes.artefact`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Artefact {
        /// Code in a repo worktree (140).
        #[default]
        Repo,
        /// A text file, in a repo or a plugin-made scratch repo (W2).
        File,
        /// The study mechanism files (W7).
        Mechanism,
    }
}

wire! {
    /// One objective (159 §5.2): a `METRIC` name, a case score name, or a judged label.
    #[derive(Default)]
    #[serde(default)]
    pub struct Objective {
        /// `METRIC <name>=`, the case score name, or the judged label.
        pub name: String,
        /// Unit for display.
        pub unit: String,
        /// Which way is better.
        pub direction: Direction,
        /// What the objective does in the decision.
        pub role: ObjectiveRole,
        /// Effect grammar; `primary` and `objective` roles.
        pub min_effect: String,
        /// Effect grammar; a guard's limit, or an objective's tolerated loss (W1).
        pub max_regression: String,
        /// Where its value comes from.
        pub source: ObjectiveSource,
    }
}

wire! {
    /// A role of 159 §5.2.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum ObjectiveRole {
        /// Decides alone (`single`).
        #[default]
        Primary,
        /// Competes (`pareto`, 2–5).
        Objective,
        /// Blocks a keep past its limit, in its own direction (gate.rs `guard_failures`).
        Guard,
        /// Stored and charted; never decides.
        Report,
    }
}

wire! {
    /// Where an objective's value comes from.
    #[derive(Default, Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum ObjectiveSource {
        /// A `METRIC` line of the bench (140).
        #[default]
        Metric,
        /// The mean of a case score over the set (W2).
        CaseMean {
            /// The CASE v2 pair name.
            name: String,
        },
        /// The share of cases whose score passes (W2).
        CasePass {
            /// The CASE v2 pair name.
            name: String,
        },
        /// The blind judge's win share (W3).
        Judged,
        /// An arm metric read from the world (W6).
        World {
            /// The `ARM … <name>=` field.
            name: String,
        },
    }
}

impl Objective {
    /// The 140 primary: `metric` as an objective.
    pub fn primary(m: &MetricSpec) -> Self {
        Self {
            name: m.name.clone(),
            unit: m.unit.clone(),
            direction: m.direction,
            role: ObjectiveRole::Primary,
            min_effect: m.min_effect.clone(),
            max_regression: String::new(),
            source: ObjectiveSource::Metric,
        }
    }

    /// A 140 guard as an objective.
    pub fn guard(g: &Guard) -> Self {
        Self {
            name: g.name.clone(),
            unit: String::new(),
            direction: g.direction,
            role: ObjectiveRole::Guard,
            min_effect: String::new(),
            max_regression: g.max_regression.clone(),
            source: ObjectiveSource::Metric,
        }
    }
}

macro_rules! block_specs {
    ($($(#[$m:meta])* $name:ident;)*) => {$(
        wire! {
            $(#[$m])*
            #[derive(Default, Eq)]
            #[serde(default)]
            pub struct $name {}
        }
    )*};
}

block_specs! {
    /// W2, W3: `judge: {rubric, …}` — W3's addendum fills it.
    JudgeSpec;
    /// W4: `population: {islands, descriptors, migrate_every}` — W4's addendum fills it.
    PopulationSpec;
    /// W5: `literature: {seed, every, via, max_queries}` — W5's addendum fills it.
    LiteratureSpec;
    /// W5: `hooks: {before, after}` — W5's addendum fills it.
    HooksSpec;
    /// W6: `world: {deploy, rollback, measure, exposure, window, …}` — W6's addendum fills it.
    WorldSpec;
    /// W7: `suite: {training, held_out, seeds, inner_budget}` — W7's addendum fills it.
    SuiteSpec;
}

wire! {
    /// Which decision rule a combination of axes uses (159 §5.6 table, C-a).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum GateRule {
        /// The sequential z gate on independent samples (140).
        #[default]
        ZGate,
        /// Corrected dominance (W1).
        Dominance,
        /// Paired per-case t plus margin (W2; W7 with fixtures as cases).
        Paired,
        /// Wilson lower bound of the win share (W3).
        WinBound,
        /// Always-valid mSPRT over windows (W6).
        Msprt,
    }
}

impl StudySpec {
    /// `objectives` when non-empty; else one primary from `metric` and one guard per
    /// `secondary_guards` entry (same names, directions and limits). Never empty for a valid spec.
    pub fn objectives_in_force(&self) -> Vec<Objective> {
        if !self.objectives.is_empty() {
            return self.objectives.clone();
        }
        std::iter::once(Objective::primary(&self.metric))
            .chain(self.secondary_guards.iter().map(Objective::guard))
            .collect()
    }

    /// The objective every scalar surface shows: the first primary, else the first objective,
    /// else the first of any role.
    pub fn headline(&self) -> Objective {
        let all = self.objectives_in_force();
        all.iter()
            .find(|o| o.role == ObjectiveRole::Primary)
            .or_else(|| all.iter().find(|o| o.role == ObjectiveRole::Objective))
            .or_else(|| all.first())
            .cloned()
            .unwrap_or_default()
    }

    /// Before every save: mirror `headline()` into `metric` and the guards into
    /// `secondary_guards`, so 151 rows, the card's first line and the probe keep reading `metric`.
    /// A spec with no `objectives` is unchanged.
    pub fn normalise(&mut self) {
        if self.objectives.is_empty() {
            return;
        }
        let h = self.headline();
        self.metric = MetricSpec {
            name: h.name,
            unit: h.unit,
            direction: h.direction,
            min_effect: h.min_effect,
        };
        self.secondary_guards = self
            .objectives
            .iter()
            .filter(|o| o.role == ObjectiveRole::Guard)
            .map(|o| Guard {
                name: o.name.clone(),
                max_regression: o.max_regression.clone(),
                direction: o.direction,
            })
            .collect();
    }

    /// The decision rule of the 159 §5.6 table; `None` for a combination no wave has built.
    pub fn rule(&self) -> Option<GateRule> {
        use ObjectivesMode as O;
        use Scoring as S;
        Some(match (self.axes.objectives, self.axes.scoring) {
            (O::Single, S::Bench) => GateRule::ZGate,
            (O::Pareto, S::Bench) => GateRule::Dominance,
            (O::Single | O::Pareto, S::Cases) => GateRule::Paired,
            (O::Judged, S::Pairwise) => GateRule::WinBound,
            (O::Single, S::World) => GateRule::Msprt,
            (O::Single, S::Suite) => GateRule::Paired,
            _ => return None,
        })
    }

    /// The kind word of 159 §3 (C-b), by precedence: artefact `mechanism` → meta; scoring
    /// `world` → real world; search `population` → population; objectives `judged` → judged;
    /// scoring `cases` → prompt study; objectives `pareto` → several objectives; else one metric.
    pub fn kind(&self) -> &'static str {
        let a = &self.axes;
        if a.artefact == Artefact::Mechanism {
            KIND_META
        } else if a.scoring == Scoring::World {
            KIND_WORLD
        } else if a.search == Search::Population {
            KIND_POPULATION
        } else if a.objectives == ObjectivesMode::Judged {
            KIND_JUDGED
        } else if a.scoring == Scoring::Cases {
            KIND_PROMPT
        } else if a.objectives == ObjectivesMode::Pareto {
            KIND_PARETO
        } else {
            KIND_SCALAR
        }
    }
}

/// The kind words of 159 §3 (`StudySpec::kind`, `StudyInfo.kind`).
pub const KIND_SCALAR: &str = "one metric";
/// W1.
pub const KIND_PARETO: &str = "several objectives";
/// W2.
pub const KIND_PROMPT: &str = "prompt study";
/// W3.
pub const KIND_JUDGED: &str = "judged";
/// W4.
pub const KIND_POPULATION: &str = "population";
/// W6.
pub const KIND_WORLD: &str = "real world";
/// W7.
pub const KIND_META: &str = "meta";

wire! {
    /// `metric:` of §4.1.
    #[derive(Default)]
    #[serde(default)]
    pub struct MetricSpec {
        /// The `METRIC <name>=` line that is the primary.
        pub name: String,
        /// Unit for display (`ms`, `MB`, `tokens`); a time or rate unit makes `exclusive` default on.
        pub unit: String,
        /// Which way is better.
        pub direction: Direction,
        /// Smallest effect worth keeping: relative (`0.5%`) or absolute in metric units (`0.3`). See [`Effect`].
        pub min_effect: String,
    }
}

wire! {
    /// Which way a metric improves.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Direction as "MetricDirection" {
        /// Smaller is better (time, size, loss).
        #[default]
        Lower,
        /// Larger is better (throughput, score).
        Higher,
    }
}

wire! {
    /// One `secondary_guards:` entry.
    #[serde(default)]
    #[derive(Default)]
    pub struct Guard as "MetricGuard" {
        /// The secondary's `METRIC` name.
        pub name: String,
        /// Largest allowed regression, relative (`5%`) or absolute; see [`Effect`].
        pub max_regression: String,
        /// Which way this secondary improves (default `lower`: memory, latency);
        /// independent of the primary's direction (R-19).
        pub direction: Direction,
    }
}

wire! {
    /// `bench:` of §4.1 (140 §5.1).
    #[serde(default)]
    pub struct BenchSpec {
        /// Custody script or repo command; cwd = the worktree, `/bin/bash -lc`.
        pub command: String,
        /// One sample's limit (humantime); kill = crash.
        pub timeout: String,
        /// Baseline samples (and final confirmation).
        pub repeats: u8,
        /// Take the machine bench slot per scored sample.
        pub exclusive: bool,
        /// Seconds-long pre-check before the first scored sample; non-zero = crash.
        pub smoke: Option<String>,
    }
}

impl Default for BenchSpec {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout: "600s".into(),
            repeats: 3,
            exclusive: true,
            smoke: None,
        }
    }
}

wire! {
    /// `checks:` of §4.1: a command and its timeout.
    #[serde(default)]
    pub struct CommandSpec {
        /// Same environment as the benchmark; exit 0 = pass.
        pub command: String,
        /// humantime.
        pub timeout: String,
    }
}

impl Default for CommandSpec {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout: "300s".into(),
        }
    }
}

wire! {
    /// `holdout:` of §4.1 (140 §5.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct HoldoutSpec {
        /// Same contract as the benchmark, other inputs.
        pub command: String,
        /// Run on the incumbent every this many keeps (and at finalize).
        pub every: u32,
        /// Samples per holdout run; `None` = `bench.repeats`.
        pub repeats: Option<u8>,
    }
}

wire! {
    /// `budget:` of §4.1 (R1-D1 defaults).
    #[serde(default)]
    pub struct BudgetSpec {
        /// Study total, USD.
        pub usd: f64,
        /// Per fire (136 `budget:` → `SetBudget`).
        pub per_experiment_usd: f64,
        /// Wall hours since Start, excluding paused time.
        pub hours: f64,
        /// Experiment count.
        pub experiments: u32,
    }
}

impl Default for BudgetSpec {
    fn default() -> Self {
        Self {
            usd: 20.0,
            per_experiment_usd: 1.5,
            hours: 10.0,
            experiments: 120,
        }
    }
}

wire! {
    /// `models:` of §4.1; `None` = defaults (R1-D5).
    #[derive(Default, Eq)]
    #[serde(default)]
    pub struct ModelsSpec {
        /// The study session; default `agent.model`.
        pub executor: Option<String>,
        /// The judge; `"off"` disables; default another vendor.
        pub review: Option<String>,
        /// The plateau consult.
        pub advisor: Option<String>,
        /// 159: the model a prompt artefact runs on (W2); default `agent.model`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target: Option<String>,
        /// 159: the case or pair judge (W2, W3); default the reviewer's other-vendor rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub judge: Option<String>,
    }
}

/// A parsed `min_effect` / `max_regression` (140 §5.2 step 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// A fraction of the incumbent's mean (`0.5%` → `0.005`).
    Relative(f64),
    /// Metric units.
    Absolute(f64),
}

impl Effect {
    /// `"0.5%"` → `Relative(0.005)`, `"0.3"` → `Absolute(0.3)`, `""` → `Absolute(0.0)`.
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(Self::Absolute(0.0));
        }
        let (num, rel) = match s.strip_suffix('%') {
            Some(n) => (n.trim(), true),
            None => (s, false),
        };
        let v: f64 = num
            .parse()
            .map_err(|_| format!("effect `{s}`: not a number"))?;
        if !v.is_finite() || v < 0.0 {
            return Err(format!(
                "effect `{s}`: must be a finite non-negative number"
            ));
        }
        Ok(if rel {
            Self::Relative(v / 100.0)
        } else {
            Self::Absolute(v)
        })
    }

    /// The threshold in metric units against `reference` (the incumbent's mean).
    pub fn against(self, reference: f64) -> f64 {
        match self {
            Self::Relative(f) => f * reference.abs(),
            Self::Absolute(v) => v,
        }
    }
}

// ───────────────────────────── §4.2 lifecycle ─────────────────────────────

wire! {
    /// Study lifecycle (140 §4.2).
    #[derive(Default, Eq)]
    #[serde(tag = "state", rename_all = "snake_case")]
    pub enum StudyState {
        /// The card is shown; nothing on disk but the draft.
        #[default]
        Draft,
        /// Worktree, custody, warm, baseline.
        Preparing,
        /// Fires back to back.
        Running,
        /// `enabled: false` on the schedule file; the experiment in flight finishes.
        Paused {
            /// Why.
            reason: PauseReason,
        },
        /// §11 in progress.
        Finalizing,
        /// Report delivered; worktree kept until landed or swept.
        Finished,
        /// Could not start or continue.
        Failed {
            /// Why.
            reason: String,
        },
        /// The user discarded it (draft or finished).
        Discarded,
    }
}

impl StudyState {
    /// The serde tag (`"paused"`), the `studies.state` column.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Paused { .. } => "paused",
            Self::Finalizing => "finalizing",
            Self::Finished => "finished",
            Self::Failed { .. } => "failed",
            Self::Discarded => "discarded",
        }
    }
}

wire! {
    /// Why a study paused (140 §4.2, §8.4).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum PauseReason {
        /// A button, slash command or `autoresearch_control`.
        User,
        /// `pause.crash_streak` crashes in a row.
        CrashStreak,
        /// `pause.custody_refusals` refusals in a row.
        CustodyRefusals,
        /// Review flags ≥ `review.pause_after`.
        ReviewFlags,
        /// The draft's budget is above `confirm_budget_usd`.
        BudgetConfirm,
        /// 159 W6: the study waits for a launch press on experiment `n`; paused time for `budget.hours`.
        LaunchApproval {
            /// The experiment.
            n: u32,
        },
        /// Anything else, in words.
        Other {
            /// Why.
            reason: String,
        },
    }
}

wire! {
    /// Why a study stopped (140 §8.4).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum StopReason {
        /// Stop after this experiment.
        User,
        /// Stop now (the experiment in flight was discarded).
        UserNow,
        /// `budget.experiments` reached.
        Experiments,
        /// Study spend ≥ `budget.usd`.
        Budget,
        /// `budget.hours` elapsed.
        Hours,
        /// `deadline:` reached.
        Deadline,
        /// Two plateau windows in a row.
        Plateau,
        /// The holdout says the search overfit.
        Overfit,
        /// Unrecoverable (worktree gone, …).
        Failed {
            /// Why.
            reason: String,
        },
    }
}

// ───────────────────────────── §4.3 one experiment ─────────────────────────────

wire! {
    /// Experiment lifecycle (140 §4.3, §8.5).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum ExperimentState {
        /// `experiment_begin` ran.
        Open,
        /// `experiment_run` is measuring.
        Measuring,
        /// A verdict was returned; waiting for `experiment_log`.
        Measured,
        /// Committed; incumbent moved.
        Kept,
        /// Reverted after `no_gain` / `worse` / a model `discard`.
        Discarded,
        /// The benchmark failed.
        Crashed,
        /// Would have kept, but the check failed.
        ChecksFailed,
        /// Refused before a sample (custody, duplicate, off-limits).
        Refused,
        /// The reviewer called it suspicious; reverted.
        RejectedByReview,
        /// The turn ended without `experiment_log`.
        Abandoned,
        /// Engine restart or stop-now mid-experiment.
        Interrupted,
        /// The run hit `run_timeout`.
        Timeout,
        /// 159 W-a: between fires — a W6 launch and its windows, W7's inner studies. Not dangling.
        Waiting,
    }
}

impl ExperimentState {
    /// The serde tag (`"rejected_by_review"`), the chart's colour vocabulary.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Measuring => "measuring",
            Self::Measured => "measured",
            Self::Kept => "kept",
            Self::Discarded => "discarded",
            Self::Crashed => "crashed",
            Self::ChecksFailed => "checks_failed",
            Self::Refused => "refused",
            Self::RejectedByReview => "rejected_by_review",
            Self::Abandoned => "abandoned",
            Self::Interrupted => "interrupted",
            Self::Timeout => "timeout",
            Self::Waiting => "waiting",
        }
    }

    /// 159 W-q: not closed — `Open`, `Measuring`, `Measured` or `Waiting`. The one predicate every
    /// "open, measuring or measured" site routes through.
    pub fn is_open(&self) -> bool {
        matches!(
            self,
            Self::Open | Self::Measuring | Self::Measured | Self::Waiting
        )
    }

    /// What `TurnFinished` and `begin`'s repair may close: `Open`, `Measuring`, `Measured`.
    /// A waiting experiment is never dangling (`store::dangling_experiment` keeps these three).
    pub fn is_dangling(&self) -> bool {
        matches!(self, Self::Open | Self::Measuring | Self::Measured)
    }

    /// `!is_open()`.
    pub fn is_closed(&self) -> bool {
        !self.is_open()
    }
}

wire! {
    /// The gate's verdict (140 §5.2 step 5).
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum Verdict as "ExperimentVerdict" {
        /// Cleared the gate.
        Improved,
        /// Not significantly better.
        NoGain,
        /// Significantly worse.
        Worse,
        /// Non-zero exit, timeout, missing primary, NaN.
        Crash,
        /// Would keep, check failed.
        ChecksFailed,
        /// A secondary regressed past its limit.
        GuardFailed,
        /// Refused before a sample was spent.
        Refused {
            /// The refusal text the model sees.
            reason: String,
        },
        /// A keep-candidate flagged for review (jump above `z_suspicious` or 3× the largest step).
        Suspicious,
        /// Granted keep-simpler (not worse, fewer lines, checks pass).
        KeepSimpler,
        /// 159 W1: entered the front without dominating its parent; a keep candidate.
        Traded,
        /// 159 W6: a world launch waiting for its press or window; not a keep candidate.
        Pending {
            /// `"your approval"` or the window's end, RFC 3339.
            until: String,
        },
    }
}

impl Verdict {
    /// The serde tag, for the ledger column and the chart colour.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::NoGain => "no_gain",
            Self::Worse => "worse",
            Self::Crash => "crash",
            Self::ChecksFailed => "checks_failed",
            Self::GuardFailed => "guard_failed",
            Self::Refused { .. } => "refused",
            Self::Suspicious => "suspicious",
            Self::KeepSimpler => "keep_simpler",
            Self::Traded => "traded",
            Self::Pending { .. } => "pending",
        }
    }

    /// Would the harness keep this (before checks and review)?
    pub fn is_keep_candidate(&self) -> bool {
        matches!(
            self,
            Self::Improved | Self::Suspicious | Self::KeepSimpler | Self::Traded
        )
    }
}

wire! {
    /// Where a sample came from (`samples.role`).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SampleRole {
        /// The experiment's tree.
        Candidate,
        /// A re-measure of the incumbent.
        Incumbent,
        /// A segment's baseline.
        Baseline,
        /// The holdout command.
        Holdout,
        /// Finalize's interleaved confirmation.
        Final,
    }
}

wire! {
    /// One benchmark run (`samples` row, 140 §5.1, §12.6).
    #[derive(Default)]
    #[serde(default)]
    pub struct Sample as "BenchSample" {
        /// Experiment number (0 = a baseline/holdout outside an experiment).
        pub n: u32,
        /// Sample index within the experiment.
        pub k: u32,
        /// The tree measured.
        pub tree_sha: String,
        /// Role.
        pub role: Option<SampleRole>,
        /// The primary metric; `None` on crash/lost.
        pub value: Option<f64>,
        /// Other `METRIC` lines.
        pub secondaries: BTreeMap<String, f64>,
        /// `CASE <id> <name>=<v>` lines, keyed `id`.
        pub cases: BTreeMap<String, f64>,
        /// `ASI <key>=<text>` lines (≤ 2 KiB).
        pub asi: BTreeMap<String, String>,
        /// Process exit code; `None` when killed/lost.
        pub exit: Option<i32>,
        /// Wall time.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub wall_ms: u64,
        /// 1-minute load ÷ cores at start.
        pub load: Option<f64>,
        /// Load precondition missed: excluded from σ.
        pub noisy: bool,
        /// The engine died mid-sample.
        pub lost: bool,
        /// `runs/<n>/sample-<k>.log`, root-relative.
        pub log_path: Option<String>,
        /// RFC 3339.
        pub at: String,
        /// Owners of the machine's build slots at start (`pid label epoch argv` lines; §5.5).
        pub slot_holders: Vec<String>,
        /// Why there is no value: the runner's reason (`exit 1`, `bad number time_ms=nan`,
        /// `no METRIC time_ms line`, `killed`).
        pub failure: Option<String>,
        /// 159 §5.3 CASE v2: `id → name → value`, names kept; `cases` stays the id's last value.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub case_values: BTreeMap<String, BTreeMap<String, f64>>,
        /// 159 §5.3: Σ `SPEND` lines of the run, USD nanos (the Bench spend channel).
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub spend_nanos: u64,
    }
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

// ───────────────────────────── §5.2 the gate ─────────────────────────────

/// The gate's parameters for one study (140 §5.2): config `gate:` plus the spec's effect sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct GateCfg {
    /// Direction of the primary.
    pub direction: Direction,
    /// Confirmed keep threshold on means.
    pub z: f64,
    /// One-sample keep.
    pub z_fast: f64,
    /// One-sample discard below this.
    pub z_discard: f64,
    /// Flag for review at or above this.
    pub z_suspicious: f64,
    /// Extra samples in the band.
    pub confirm_runs: u8,
    /// Incumbent sample when its newest is older than this many experiments.
    pub remeasure_after: u32,
    /// Smallest effect worth keeping.
    pub min_effect: Effect,
    /// Secondary limits: `(name, largest regression, the secondary's own direction)`.
    pub guards: Vec<(String, Effect, Direction)>,
}

/// What the gate sees after each sample (140 §5.2 steps 1–3).
#[derive(Clone, Debug, PartialEq)]
pub struct GateInput {
    /// Candidate samples so far (primary values, raw units).
    pub candidate: Vec<f64>,
    /// The incumbent's samples since it became incumbent (plus re-measures).
    pub incumbent: Vec<f64>,
    /// Pooled within-tree σ of the segment (0 = deterministic).
    pub sigma: f64,
    /// Largest accepted Δ earlier in the segment (the 3× rule).
    pub largest_accepted_delta: Option<f64>,
    /// Experiments since the incumbent's newest sample.
    pub incumbent_age: u32,
    /// The model asked for `keep_simpler`.
    pub keep_simpler: bool,
    /// Secondary means: `(name, candidate_mean, incumbent_mean)`.
    pub secondaries: Vec<(String, f64, f64)>,
}

/// One step of the sequential decision (140 §5.2 step 3).
#[derive(Clone, Debug, PartialEq)]
pub enum GateStep {
    /// Take more samples before deciding.
    Confirm {
        /// Candidate samples still to take.
        more: u8,
        /// Take one incumbent sample too (drift).
        incumbent_sample: bool,
    },
    /// Decided.
    Decide {
        /// The verdict.
        verdict: Verdict,
        /// Direction-normalised Δ in metric units (positive = better).
        delta: f64,
        /// Δ as a percentage of the incumbent's mean.
        delta_pct: f64,
        /// The z-score behind it (`None` when σ = 0).
        z: Option<f64>,
        /// 159 W-e: the rule's full decision; `None` = the 140 scalar rule.
        detail: Option<GateDecision>,
    },
    /// 159 W6, W7: decide after a later window; the experiment goes `Waiting`.
    Wait {
        /// RFC 3339, or `"your approval"`.
        until: String,
        /// Why, in words.
        why: String,
    },
}

wire! {
    /// 159 §5.6: what a rule decided, per objective (`experiments.gate`, `{"v": 2, …}`).
    #[derive(Default)]
    #[serde(default)]
    pub struct GateDecision {
        /// Schema version, 2.
        pub v: u8,
        /// The rule.
        pub rule: GateRule,
        /// One per objective in force.
        pub objectives: Vec<ObjectiveDelta>,
        /// The rule's statistic (z_k, t, Wilson bound, Λ).
        pub stat: Option<f64>,
        /// What `stat` had to clear.
        pub threshold: Option<f64>,
        /// Samples, cases, pairs or units behind it.
        pub n: u32,
    }
}

impl GateDecision {
    /// The schema version every row carries.
    pub const V: u8 = 2;
}

wire! {
    /// One objective's comparison inside a [`GateDecision`].
    #[derive(Default)]
    #[serde(default)]
    pub struct ObjectiveDelta {
        /// The objective.
        pub name: String,
        /// Its role.
        pub role: ObjectiveRole,
        /// Candidate mean.
        pub candidate: Option<f64>,
        /// Reference mean (incumbent, parent or elite).
        pub reference: Option<f64>,
        /// Direction-normalised Δ (positive = better).
        pub delta: Option<f64>,
        /// Raw relative change (gate.rs#L72-L75 convention).
        pub delta_pct: Option<f64>,
        /// The z behind it, when the rule has one.
        pub z: Option<f64>,
        /// What the rule called it.
        pub call: Call,
    }
}

wire! {
    /// A per-objective call.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Call {
        /// Significantly better and past `min_effect`.
        Better,
        /// Within noise.
        #[default]
        Same,
        /// Significantly worse past the tolerated loss.
        Worse,
        /// A guard past its limit.
        GuardFailed,
    }
}

wire! {
    /// 159 §5.9: why and until when an experiment waits (`experiments.wait`).
    #[derive(Default)]
    #[serde(default)]
    pub struct WaitInfo {
        /// RFC 3339, or `"your approval"`.
        pub until: String,
        /// Why.
        pub why: WaitWhy,
        /// Windows measured so far (W6).
        pub windows: u32,
    }
}

wire! {
    /// Why an experiment waits.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum WaitWhy {
        /// A launch press (W6).
        #[default]
        Approval,
        /// A measurement window (W6).
        Window,
        /// Inner studies (W7).
        Inner,
    }
}

wire! {
    /// 159 §5.8: where study money goes.
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SpendChannel {
        /// Model calls in the study session tree, engine-side SideCalls included.
        Session,
        /// Σ `samples.spend_nanos`, self-reported by benches (W-m).
        Bench,
        /// Money a world study spends; always 0 under D4.
        World,
    }
}

impl SpendChannel {
    /// The `by_channel` key.
    pub fn key(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Bench => "bench",
            Self::World => "world",
        }
    }
}

// ───────────────────────────── §12.4 tool arguments and reports ─────────────────────────────

wire! {
    /// `experiment_run` arguments (140 §12.4).
    #[derive(Default)]
    pub struct RunArgs {
        /// The idea in one line (≤ 300).
        pub idea: String,
        /// A short category (`caching`, `algorithm`, `allocation`; ≤ 40).
        pub category: String,
        /// Why it should help (≤ 300).
        pub hypothesis: String,
        /// Override the novelty refusal (needs `reason`).
        #[serde(default)]
        pub force: bool,
        /// Why a retry of a similar idea is legitimate.
        #[serde(default)]
        pub reason: Option<String>,
    }
}

wire! {
    /// `experiment_run` result (140 §12.4).
    #[serde(default)]
    pub struct RunReport {
        /// The decision.
        pub verdict: Verdict,
        /// The verdict in words and numbers (140 §5.2 step 5).
        pub line: String,
        /// Samples taken this call.
        pub samples: Vec<Sample>,
        /// Direction-normalised Δ in metric units.
        pub delta: Option<f64>,
        /// Δ as a percentage.
        pub delta_pct: Option<f64>,
        /// z-score.
        pub z: Option<f64>,
        /// The check, when run.
        pub checks: Option<CheckResult>,
        /// Paths touched outside `scope`.
        pub deviations: Vec<String>,
        /// Past experiments the novelty check found similar.
        pub similar: Vec<SimilarExperiment>,
        /// Last 30 lines / 4 KiB of a crashed sample.
        pub tail: Option<String>,
        /// Per-case deltas that moved most.
        pub cases_moved: Vec<CaseDelta>,
        /// `experiment_run` calls left after a crash.
        pub retries_left: u8,
    }
}

impl Default for RunReport {
    fn default() -> Self {
        Self {
            verdict: Verdict::Crash,
            line: String::new(),
            samples: Vec::new(),
            delta: None,
            delta_pct: None,
            z: None,
            checks: None,
            deviations: Vec::new(),
            similar: Vec::new(),
            tail: None,
            cases_moved: Vec::new(),
            retries_left: 0,
        }
    }
}

wire! {
    /// The correctness check's outcome.
    #[derive(Default)]
    #[serde(default)]
    pub struct CheckResult {
        /// Exit 0.
        pub passed: bool,
        /// Tail of the output when it failed.
        pub tail: Option<String>,
        /// Wall time.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub wall_ms: u64,
    }
}

wire! {
    /// A past experiment the novelty check matched (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct SimilarExperiment {
        /// Its number.
        pub n: u32,
        /// Its verdict tag.
        pub verdict: String,
        /// Its Δ %.
        pub delta_pct: Option<f64>,
        /// Its idea.
        pub idea: String,
    }
}

wire! {
    /// One `CASE` id's movement between the incumbent and the candidate.
    #[derive(Default)]
    #[serde(default)]
    pub struct CaseDelta {
        /// The case id.
        pub id: String,
        /// Incumbent value.
        pub before: Option<f64>,
        /// Candidate value.
        pub after: f64,
        /// Relative change, %.
        pub delta_pct: Option<f64>,
    }
}

wire! {
    /// `experiment_log` arguments (140 §12.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct LogArgs {
        /// What you did (≤ 500).
        pub description: String,
        /// What the result teaches (≤ 500).
        pub learned: String,
        /// What to try next (≤ 300).
        pub next: Option<String>,
        /// Downgrade or keep-simpler; `auto` applies the verdict.
        pub decision: Decision,
        /// Required when a keep touches paths outside `scope`.
        pub justification: Option<String>,
        /// Ideas this experiment closes.
        #[cfg_attr(feature = "ts", ts(type = "number[]"))]
        pub closes: Vec<IdeaId>,
    }
}

wire! {
    /// `experiment_log.decision`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Decision as "LogDecision" {
        /// Apply the verdict.
        #[default]
        Auto,
        /// Throw away an improvement you distrust.
        Discard,
        /// Keep a simplification at equal performance (the harness verifies).
        KeepSimpler,
    }
}

wire! {
    /// `experiment_log` result (140 §12.4).
    #[serde(default)]
    pub struct LogReport {
        /// The experiment's final state.
        pub applied: ExperimentState,
        /// The keep's commit.
        pub commit: Option<String>,
        /// The reviewer's word on a keep.
        pub review: Option<ReviewVerdict>,
        /// The incumbent after this experiment.
        pub incumbent: Incumbent,
        /// What is left.
        pub budget_left: BudgetLeft,
        /// Set when this experiment was the last.
        pub stop: Option<StopReason>,
    }
}

impl Default for LogReport {
    fn default() -> Self {
        Self {
            applied: ExperimentState::Abandoned,
            commit: None,
            review: None,
            incumbent: Incumbent::default(),
            budget_left: BudgetLeft::default(),
            stop: None,
        }
    }
}

wire! {
    /// The incumbent, in short.
    #[derive(Default)]
    #[serde(default)]
    pub struct Incumbent as "StudyIncumbent" {
        /// Its commit.
        pub sha: String,
        /// Its mean of the primary.
        pub mean: Option<f64>,
        /// Experiments kept so far.
        pub kept: u32,
    }
}

wire! {
    /// Budget left (140 §7.2 "Budget left" line).
    #[derive(Default)]
    #[serde(default)]
    pub struct BudgetLeft {
        /// USD.
        pub usd: f64,
        /// Hours.
        pub hours: f64,
        /// Experiments.
        pub experiments: u32,
    }
}

wire! {
    /// Budget used so far.
    #[derive(Default)]
    #[serde(default)]
    pub struct BudgetUsed {
        /// USD (sum of `model_calls.cost` over the study session tree).
        pub usd: f64,
        /// Hours since Start, excluding paused time.
        pub hours: f64,
        /// Experiments closed.
        pub experiments: u32,
        /// 159 §5.8: USD per [`SpendChannel`] key (`session`, `bench`, `world`).
        #[serde(default)]
        pub by_channel: BTreeMap<String, f64>,
    }
}

wire! {
    /// The reviewer's word (140 §9.1).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum ReviewVerdict {
        /// `CLEAN: <reason>`; nothing visible.
        Clean {
            /// The reviewer's sentence.
            reason: String,
        },
        /// `SUSPICIOUS: <reason>` with `on_suspicious: revert` — the keep was undone.
        Reverted {
            /// The reviewer's sentence.
            reason: String,
        },
        /// `SUSPICIOUS: <reason>` with `on_suspicious: flag` — kept, marked.
        Flagged {
            /// The reviewer's sentence.
            reason: String,
        },
        /// Two malformed replies; the keep stands unreviewed.
        Error {
            /// What went wrong.
            reason: String,
        },
    }
}

wire! {
    /// `experiment_notes` arguments (140 §12.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct NotesArgs {
        /// Replaces the playbook whole (≤ `brief.playbook_bytes`).
        pub playbook: Option<String>,
        /// New backlog ideas.
        pub add_ideas: Vec<IdeaDraft>,
    }
}

wire! {
    /// One idea to add.
    #[derive(Default)]
    #[serde(default)]
    pub struct IdeaDraft {
        /// The idea.
        pub text: String,
        /// Its category.
        pub category: String,
        /// 0–3; model ideas default 1, user ideas are always 3.
        pub priority: Option<u8>,
    }
}

wire! {
    /// `experiment_notes` result.
    #[derive(Default)]
    #[serde(default)]
    pub struct NotesReport {
        /// Playbook size after the call.
        pub playbook_bytes: u32,
        /// Ideas added.
        pub ideas_added: u32,
    }
}

wire! {
    /// `experiment_begin` result (140 §7.2, §10 step 1).
    #[derive(Default)]
    #[serde(default)]
    pub struct Brief as "StudyBrief" {
        /// The rendered `<study …>…</study>` block.
        pub text: String,
        /// This experiment's number.
        pub n: u32,
        /// The segment.
        pub segment: u32,
        /// Wave slots (`parallel > 1` only).
        pub slots: Option<Vec<Slot>>,
    }
}

wire! {
    /// One wave slot (140 §10 step 1).
    #[derive(Default)]
    #[serde(default)]
    pub struct Slot as "WaveSlot" {
        /// Slot number.
        pub k: u8,
        /// Candidate worktree path.
        pub worktree: String,
        /// The category assigned to this slot.
        pub category: String,
        /// A backlog idea assigned to it.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub idea_id: Option<IdeaId>,
        /// The experiment number allocated to it.
        pub n: u32,
    }
}

// ───────────────────────────── §12.3 control, ideas, steers ─────────────────────────────

wire! {
    /// `autoresearch_control` / `autoresearch.control` operations (140 §12.3).
    #[derive(Eq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum ControlOp {
        /// `enabled: false`; the experiment in flight finishes.
        Pause,
        /// `enabled: true`.
        Resume,
        /// Finalize after this experiment.
        StopAfter,
        /// Cancel the tree, reset the worktree, finalize.
        StopNow,
        /// Fire once while paused.
        RunOnce,
        /// New custody + baseline; σ restarts (140 §5.3).
        Segment {
            /// Why the user re-baselined.
            reason: String,
        },
        /// Finalize now.
        Finalize,
        /// Re-apply a reverted keep as the next experiment (140 §9.1).
        Restore {
            /// The reverted experiment.
            n: u32,
        },
        /// Keep a review revert as it is: dismiss its needs-your-eyes item and attention row (140 §9.1).
        KeepReverted {
            /// The reverted experiment.
            n: u32,
        },
        /// `git merge --no-ff` a finalize changeset into the repo's current branch (140 §11.5);
        /// refused when the user's tree is dirty. Never pushes.
        Merge {
            /// The changeset's `k`.
            changeset: u32,
        },
        /// Remove worktree and branches.
        Discard,
    }
}

wire! {
    /// When a created study starts (the draft card's two Start buttons).
    #[derive(Default, Eq)]
    #[serde(tag = "when", rename_all = "snake_case")]
    pub enum StartWhen {
        /// Now.
        #[default]
        Now,
        /// Only when the sensor says so (`window:`).
        Window {
            /// A 40 `when:` sensor (`sensors/idle.py`).
            sensor: String,
        },
    }
}

wire! {
    /// Who did it (140 §12.3 `by`).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum By as "StudyActor" {
        /// The user: a button, a slash command, a root chat.
        #[default]
        User,
        /// The study session (model).
        Model,
        /// The advisor consult.
        Advisor,
        /// The setup conversation.
        Setup,
        /// The plugin itself (stop rules, pauses).
        Plugin,
    }
}

wire! {
    /// `autoresearch.idea` operations.
    #[derive(Eq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum IdeaChange {
        /// Add one (user ideas get priority 3 and the ★).
        Add {
            /// The idea.
            text: String,
            /// Its category.
            category: String,
            /// 0–3.
            priority: Option<u8>,
        },
        /// Reprioritise.
        Priority {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
            /// 0–3.
            priority: u8,
        },
        /// Close without trying.
        Close {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
        },
        /// Reopen.
        Reopen {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
        },
    }
}

wire! {
    /// Where an idea came from (`ideas.source`).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum IdeaSource {
        /// The user.
        User,
        /// The study session.
        #[default]
        Model,
        /// The plateau consult.
        Advisor,
        /// The setup conversation.
        Setup,
        /// 159 W5: a cited literature idea (`citations` row).
        Literature,
    }
}

wire! {
    /// `ideas.status`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum IdeaStatus {
        /// In the backlog.
        #[default]
        Open,
        /// An experiment matched it.
        Tried,
        /// Closed by the model or the user.
        Closed,
    }
}

wire! {
    /// One `ideas` row (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct Idea as "StudyIdea" {
        /// Row id.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub id: IdeaId,
        /// The study.
        pub slug: String,
        /// The idea.
        pub text: String,
        /// Its category.
        pub category: String,
        /// Who added it.
        pub source: IdeaSource,
        /// 0–3.
        pub priority: u8,
        /// Status.
        pub status: IdeaStatus,
        /// Experiments that tried it.
        pub tried_in: Vec<u32>,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// One `steers` row (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct Steer as "StudySteer" {
        /// Row id.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub id: SteerId,
        /// The study.
        pub slug: String,
        /// The text.
        pub text: String,
        /// Who.
        pub by: By,
        /// RFC 3339.
        pub at: String,
        /// The first experiment whose brief carried it.
        pub seen_in: Option<u32>,
    }
}

wire! {
    /// Where an experiment is (`ExperimentProgress.phase`, 140 §12.3).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Phase as "ExperimentPhase" {
        /// Duplicate check.
        Novelty,
        /// Evaluator hashes.
        Custody,
        /// Bench slot or load precondition.
        Waiting,
        /// A benchmark sample is running.
        Sampling,
        /// The correctness check.
        Checks,
        /// The reviewer call.
        Review,
        /// 159 W2: the engine-side case runner.
        Cases,
        /// 159 W3: the blind pair judge.
        Judging,
        /// 159 W6: a measurement window.
        Window,
    }
}

// ───────────────────────────── §12.5 RPC shapes ─────────────────────────────

wire! {
    /// One study in the list (140 §12.5 `autoresearch.list`, the TUI line, the routines badge).
    #[derive(Default)]
    #[serde(default)]
    pub struct StudyInfo {
        /// The slug.
        pub slug: String,
        /// The goal.
        pub goal: String,
        /// The repo path.
        pub repo: String,
        /// State.
        pub state: StudyState,
        /// The primary metric.
        pub metric: MetricSpec,
        /// Base sha (after Start).
        pub base_sha: Option<String>,
        /// The incumbent.
        pub incumbent: Incumbent,
        /// The study base's measured mean (segment 0's baseline); `delta_pct` is vs this.
        pub baseline_mean: Option<f64>,
        /// Pooled σ of the current segment.
        pub sigma: Option<f64>,
        /// Current segment.
        pub segment: u32,
        /// Experiments closed.
        pub experiments: u32,
        /// Incumbent vs base, %.
        pub delta_pct: Option<f64>,
        /// One line: "Optimising time_ms in liquid · 34 experiments · −14.2 % · running".
        pub headline: String,
        /// Budget spent.
        pub budget_used: BudgetUsed,
        /// Budget limits.
        pub budget: BudgetSpec,
        /// The study session once bound.
        pub session: Option<SessionId>,
        /// 151 D7: the chat the draft was set up in (the setup pane; start and result cards go there).
        pub origin: Option<SessionId>,
        /// The experiment in flight.
        pub current: Option<u32>,
        /// Its phase.
        pub phase: Option<Phase>,
        /// RFC 3339.
        pub started_at: Option<String>,
        /// RFC 3339.
        pub finished_at: Option<String>,
        /// Why it stopped.
        pub stop_reason: Option<StopReason>,
        /// The report document id once finished.
        pub report_doc: Option<String>,
        /// 159 §5.7: the kind word of §3 (`one metric`, `several objectives`, …).
        pub kind: String,
    }
}

wire! {
    /// `autoresearch.get` (140 §12.5): everything the Study view needs.
    #[derive(Default)]
    #[serde(default)]
    pub struct StudyDetail {
        /// The list row.
        pub info: StudyInfo,
        /// The approved spec.
        pub spec: StudySpec,
        /// The playbook text.
        pub playbook: String,
        /// Chart series.
        pub series: ChartSeries,
        /// Ideas, open first.
        pub ideas: Vec<Idea>,
        /// Steers, newest first.
        pub steers: Vec<Steer>,
        /// Finalize's changesets (finished studies).
        pub changesets: Vec<Changeset>,
        /// Attention items: reverted, deviations, overfit, drift (one line each).
        pub needs_eyes: Vec<String>,
        /// The last 30 lines / 4 KiB of the running sample's log while an experiment is measuring.
        pub tail: Option<String>,
        /// Finalize's holdout result (`61.0 → 49.7 (−18.5 %)`, §11.1) once finished.
        pub final_holdout: Option<String>,
        /// 159 §5.7: what the Study view switches on; `Scalar` = `series` as today.
        pub view: KindView,
    }
}

// ───────────────────────────── 159 §5.7 the kind views ─────────────────────────────

wire! {
    /// 159 §5.7: the per-kind view the Study view switches on.
    #[derive(Default)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum KindView {
        /// `StudyDetail.series` as today (140).
        #[default]
        Scalar,
        /// W1: a scatter on two objectives with the front as a staircase.
        Front {
            /// The two objectives plotted.
            axes: [String; 2],
            /// Every experiment.
            points: Vec<FrontPoint>,
            /// Experiment numbers of the current front members.
            members: Vec<u32>,
        },
        /// W2: the case × experiment heat strip.
        Cases {
            /// One per case.
            rows: Vec<CaseRow>,
            /// Cases per split.
            split: SplitCounts,
        },
        /// W3: the rating ladder and the agreement figure.
        Ratings {
            /// Kept versions, best first.
            ladder: Vec<RatingRow>,
            /// Judge–human agreement, once a pair was answered.
            agreement: Option<Agreement>,
        },
        /// W4: the archive grid.
        Archive {
            /// The two descriptors.
            axes: [String; 2],
            /// Every live elite.
            cells: Vec<CellRow>,
            /// Island count.
            islands: u8,
        },
        /// W6: both arms per window, Λ, the launch.
        World {
            /// One per arm per window.
            windows: Vec<ArmWindow>,
            /// The current launch, if any.
            launch: Option<LaunchRow>,
            /// Λ after each window.
            lambda: Vec<f64>,
        },
        /// W7: per fixture, incumbent against candidate mechanism.
        Suite {
            /// One per fixture.
            fixtures: Vec<SuiteRow>,
        },
    }
}

wire! {
    /// One experiment on the front chart (W1).
    #[derive(Default)]
    #[serde(default)]
    pub struct FrontPoint {
        /// Experiment.
        pub n: u32,
        /// Its tree.
        pub sha: String,
        /// Objective means.
        pub scores: BTreeMap<String, f64>,
        /// On the front now.
        pub member: bool,
    }
}

wire! {
    /// One case across experiments (W2; `autoresearch.cases`).
    #[derive(Default)]
    #[serde(default)]
    pub struct CaseRow {
        /// Case id.
        pub id: String,
        /// `train` | `val` | `holdout`.
        pub split: String,
        /// Score per experiment number (0 = the base).
        pub values: BTreeMap<u32, f64>,
    }
}

wire! {
    /// Cases per split (W2).
    #[derive(Copy, Default, Eq)]
    #[serde(default)]
    pub struct SplitCounts {
        /// Train.
        pub train: u32,
        /// Val.
        pub val: u32,
        /// Holdout.
        pub holdout: u32,
    }
}

wire! {
    /// One rung of the Bradley–Terry ladder (W3).
    #[derive(Default)]
    #[serde(default)]
    pub struct RatingRow {
        /// The version's tree.
        pub sha: String,
        /// Strength.
        pub theta: f64,
        /// Its standard error.
        pub se: f64,
        /// Pairs judged.
        pub games: u32,
        /// `θ × 400/ln 10 + 1000`.
        pub elo: f64,
        /// Experiment number, when it was a keep.
        pub n: Option<u32>,
    }
}

wire! {
    /// Judge–human agreement (W3, D14).
    #[derive(Copy, Default, Eq)]
    #[serde(default)]
    pub struct Agreement {
        /// Human answers that matched the judge's double verdict.
        pub agreed: u32,
        /// Human pairs answered.
        pub asked: u32,
    }
}

wire! {
    /// One live elite of the archive (W4).
    #[derive(Default)]
    #[serde(default)]
    pub struct CellRow {
        /// `i<island>:<bin>.<bin>`.
        pub key: String,
        /// Island.
        pub island: Option<u8>,
        /// The elite's tree.
        pub sha: String,
        /// The experiment that landed it.
        pub n: u32,
        /// Objective means.
        pub scores: BTreeMap<String, f64>,
        /// Descriptor values.
        pub descriptor: BTreeMap<String, f64>,
        /// Children sampled from it.
        pub children: u32,
        /// The champion.
        pub champion: bool,
    }
}

wire! {
    /// One arm in one window (W6; `observations` row).
    #[derive(Default)]
    #[serde(default)]
    pub struct ArmWindow {
        /// Experiment.
        pub n: u32,
        /// `control` | `candidate`.
        pub arm: String,
        /// Window index.
        pub win: u32,
        /// RFC 3339.
        pub start_at: String,
        /// RFC 3339.
        pub end_at: String,
        /// Units observed.
        pub units: u32,
        /// Mean outcome.
        pub mean: Option<f64>,
        /// Variance.
        pub var: Option<f64>,
    }
}

wire! {
    /// One launch (W6; `launches` row without the token hash).
    #[derive(Default)]
    #[serde(default)]
    pub struct LaunchRow {
        /// Study.
        pub slug: String,
        /// Experiment.
        pub n: u32,
        /// `requested` | `approved` | `refused` | `deployed` | `rolled_back`.
        pub state: String,
        /// Share of traffic both arms take.
        pub exposure: f64,
        /// RFC 3339.
        pub requested_at: String,
        /// RFC 3339.
        pub decided_at: Option<String>,
        /// Who pressed.
        pub by: Option<String>,
    }
}

wire! {
    /// One fixture of the suite (W7).
    #[derive(Default)]
    #[serde(default)]
    pub struct SuiteRow {
        /// Fixture name.
        pub fixture: String,
        /// In the held-out set.
        pub held_out: bool,
        /// Normalised holdout gain under the incumbent mechanism.
        pub incumbent: Option<f64>,
        /// Under the candidate mechanism.
        pub candidate: Option<f64>,
    }
}

// ───────────────────────────── 159 §5.4–§5.5, §5.11 ledger rows and RPC replies ─────────────────────────────

wire! {
    /// `archive` row (159 §5.4): a W1 front member or a W4 cell elite.
    #[derive(Default)]
    #[serde(default)]
    pub struct ArchiveRow {
        /// Study.
        pub slug: String,
        /// `front` | `cell`.
        pub kind: String,
        /// `f<n>` or `i<island>:<bin>.<bin>`.
        pub key: String,
        /// The tree.
        pub sha: String,
        /// The experiment that landed it.
        pub n: u32,
        /// `refs/autoresearch/<slug>/<kind>/<key>`.
        #[serde(rename = "ref")]
        pub ref_name: String,
        /// Objective means.
        pub scores: BTreeMap<String, f64>,
        /// Descriptor values (W4).
        pub descriptor: BTreeMap<String, f64>,
        /// Island (W4).
        pub island: Option<u8>,
        /// Children sampled from it.
        pub children: u32,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// `judgments` row (159 §5.4): a W2 case score (`b_sha` NULL) or a W3 pair.
    #[derive(Default)]
    #[serde(default)]
    pub struct JudgmentRow {
        /// Row id (0 before insert).
        pub id: i64,
        /// Study.
        pub slug: String,
        /// Experiment.
        pub n: u32,
        /// Case.
        pub case_id: String,
        /// `train` | `val` | `holdout`.
        pub split: String,
        /// The tree scored, or the pair's first tree.
        pub a_sha: String,
        /// The pair's second tree (W3).
        pub b_sha: Option<String>,
        /// Whether A was shown first (W3).
        pub a_first: Option<bool>,
        /// The judge model.
        pub model: String,
        /// 0–1 (W2).
        pub score: Option<f64>,
        /// `a` | `b` | `tie` (W3).
        pub winner: Option<String>,
        /// The judge's reason.
        pub reason: String,
        /// Answered by a human.
        pub human: bool,
        /// USD nanos.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub cost_nanos: u64,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// `ratings` row (159 §5.4, W3).
    #[derive(Default)]
    #[serde(default)]
    pub struct RatingRecord {
        /// Study.
        pub slug: String,
        /// Segment.
        pub segment: u32,
        /// The version's tree.
        pub sha: String,
        /// Strength.
        pub theta: f64,
        /// Standard error.
        pub se: f64,
        /// Pairs judged.
        pub games: u32,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// `citations` row (159 §5.4, W5).
    #[derive(Default)]
    #[serde(default)]
    pub struct CitationRow {
        /// The `ideas.id` it cites.
        pub idea_id: i64,
        /// Source URL.
        pub url: String,
        /// Page title.
        pub title: String,
        /// ≤ 300 characters, verified against the fetched text.
        pub quote: String,
        /// The quote occurred in the page.
        pub verified: bool,
        /// RFC 3339.
        pub fetched_at: String,
    }
}

wire! {
    /// `launches` row (159 §5.4, W6); the token hash never leaves the store as a `LaunchRow`.
    #[derive(Default)]
    #[serde(default)]
    pub struct LaunchRecord {
        /// Study.
        pub slug: String,
        /// Experiment.
        pub n: u32,
        /// `requested` | `approved` | `refused` | `deployed` | `rolled_back`.
        pub state: String,
        /// sha256 of the press token.
        pub token_hash: String,
        /// Share of traffic both arms take.
        pub exposure: f64,
        /// RFC 3339.
        pub requested_at: String,
        /// RFC 3339.
        pub decided_at: Option<String>,
        /// Who pressed.
        pub by: Option<String>,
    }
}

impl LaunchRecord {
    /// The view without the token hash.
    pub fn row(&self) -> LaunchRow {
        LaunchRow {
            slug: self.slug.clone(),
            n: self.n,
            state: self.state.clone(),
            exposure: self.exposure,
            requested_at: self.requested_at.clone(),
            decided_at: self.decided_at.clone(),
            by: self.by.clone(),
        }
    }
}

wire! {
    /// `observations` row (159 §5.4, W6).
    #[derive(Default)]
    #[serde(default)]
    pub struct ObservationRow {
        /// Row id (0 before insert).
        pub id: i64,
        /// Study.
        pub slug: String,
        /// Experiment.
        pub n: u32,
        /// `control` | `candidate`.
        pub arm: String,
        /// Window index.
        pub win: u32,
        /// RFC 3339.
        pub start_at: String,
        /// RFC 3339.
        pub end_at: String,
        /// Units observed.
        pub units: u32,
        /// Mean outcome.
        pub mean: Option<f64>,
        /// Variance.
        pub var: Option<f64>,
        /// The source's raw fields.
        pub raw: serde_json::Value,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// `autoresearch.mechanism` op (159 §5.11, W7).
    #[derive(Eq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum MechanismOp {
        /// Every version.
        List,
        /// One version's files.
        Show {
            /// Version; `None` = the active one.
            version: Option<u32>,
        },
        /// A press: new studies pin this version.
        Adopt {
            /// Version.
            version: u32,
        },
    }
}

wire! {
    /// `autoresearch.mechanism` reply (W7).
    #[derive(Default)]
    #[serde(default)]
    pub struct MechanismInfo {
        /// The active version; `None` = the compiled defaults.
        pub active: Option<u32>,
        /// Known versions, newest first.
        pub versions: Vec<MechanismVersion>,
        /// `Show`: file name → text.
        pub files: BTreeMap<String, String>,
    }
}

wire! {
    /// One mechanism version (W7).
    #[derive(Default)]
    #[serde(default)]
    pub struct MechanismVersion {
        /// Version.
        pub version: u32,
        /// Git sha of `<root>/mechanism/`.
        pub sha: String,
        /// RFC 3339.
        pub at: String,
        /// The adopting study's headline, or "compiled defaults".
        pub summary: String,
    }
}

// ───────────────────────────── 151 setup: the living draft ─────────────────────────────

wire! {
    /// 151 §5: the state of one draft row, computed by the engine, never by the UI.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum FieldState as "DraftFieldState" {
        /// Engine evidence matches the current spec (repo resolves; a probe whose fingerprint matches).
        Verified,
        /// The agent listed it as agreed, or the user set / OK'd it in the panel.
        Confirmed,
        /// Present, not confirmed: "Needs your OK".
        #[default]
        Proposed,
        /// Equal to the config default and untouched (budget, parallel, models).
        Default,
        /// Required and absent.
        Missing,
        /// Optional and absent (holdout, window, guards, warm, evaluator paths).
        Optional,
    }
}

wire! {
    /// One row of the live draft panel (151 §4.3 rows).
    #[derive(Default)]
    #[serde(default)]
    pub struct DraftField {
        /// Row name: `goal | repo | metric | bench | checks | holdout | scope | budget | window` then the advanced rows.
        pub name: String,
        /// The engine's one-line rendering of the row's value (empty when absent).
        pub value: String,
        /// The state.
        pub state: FieldState,
        /// Evidence line for `verified` (`584660bf · clean tree`, `5 runs · 412 ms · ±1.6%`).
        pub evidence: Option<String>,
    }
}

wire! {
    /// `autoresearch.draft {draft_id}` (151 §4.1): the panel's data.
    #[derive(Default)]
    #[serde(default)]
    pub struct DraftView {
        /// The draft id (the spec's slug).
        pub draft_id: String,
        /// The spec as last written.
        pub spec: StudySpec,
        /// The setup session.
        pub origin: Option<SessionId>,
        /// Rows in fixed order, main nine then advanced.
        pub fields: Vec<DraftField>,
        /// Main rows not `proposed` / `missing`.
        pub settled: u32,
        /// Main rows (9).
        pub total: u32,
        /// No required row missing or proposed, `window` not proposed, `validate_spec` passes.
        pub ready: bool,
        /// What blocks Start, in row order (the panel footer).
        pub blocking: Vec<String>,
        /// The repo's tree is dirty: offer "Start with my uncommitted changes".
        pub dirty: bool,
        /// The last `autoresearch_draft` was `partial: true`.
        pub partial: bool,
        /// Bumped by every write of spec.json.
        pub version: u32,
        /// RFC 3339 of the last write.
        pub updated_at: String,
        /// Probe results whose fingerprint still matches the spec (the sparkline).
        pub probes: Vec<ProbeResult>,
        /// A study with this slug exists: the panel flips to the status card.
        pub started: bool,
    }
}

wire! {
    /// `autoresearch.drafts` (151 D6): one setting-up row of the rail.
    #[derive(Default)]
    #[serde(default)]
    pub struct DraftSummary {
        /// The draft id (the spec's slug).
        pub draft_id: String,
        /// The goal so far.
        pub goal: String,
        /// The repo path.
        pub repo: String,
        /// The setup session (`/autoresearch/new?chat=<origin>`).
        pub origin: Option<SessionId>,
        /// Main rows settled.
        pub settled: u32,
        /// Main rows (9).
        pub total: u32,
        /// Start would be accepted.
        pub ready: bool,
        /// RFC 3339 of the last write.
        pub updated_at: String,
    }
}

wire! {
    /// What `autoresearch_probe` measures (151 D4).
    #[derive(Copy, Default, Eq, PartialOrd, Ord)]
    #[serde(rename_all = "snake_case")]
    pub enum ProbeKind {
        /// `bench.command`, `repeats` samples (clamped 3..=10).
        #[default]
        Bench,
        /// `checks.command` once.
        Check,
        /// `holdout.command`, `holdout.repeats` samples.
        Holdout,
    }
}

wire! {
    /// One probe's result (`<root>/drafts/<id>/probe.json`, 151 §6.2); `verified` only while `fingerprint` matches.
    #[derive(Default)]
    #[serde(default)]
    pub struct ProbeResult {
        /// Which command.
        pub kind: ProbeKind,
        /// Every sample exited 0 with the primary present (check: exit 0).
        pub ok: bool,
        /// The primary's values, one per clean sample (empty for check).
        pub values: Vec<f64>,
        /// Median of `values`.
        pub median: Option<f64>,
        /// `100 × pooled σ / |mean|` — the baseline's estimator (`gate::pooled_sigma`).
        pub noise_pct: Option<f64>,
        /// `metric.unit`.
        pub unit: String,
        /// Samples taken (including crashes).
        pub samples: u32,
        /// Wall time of the whole probe.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub duration_ms: u64,
        /// Exit code of the last (or failing) command.
        pub exit: Option<i32>,
        /// Last 30 lines / 4 KiB of the failing command's log (empty when `ok`).
        pub log_tail: String,
        /// RFC 3339.
        pub at: String,
        /// sha256 over every input the result depends on (151 §6.2 step 5).
        pub fingerprint: String,
    }
}

wire! {
    /// `autoresearch_draft` arguments (151 D5): the spec plus the living-draft flags.
    #[derive(Default)]
    #[serde(default)]
    pub struct DraftArgs as "StudyDraftArgs" {
        /// The spec (every field of 140 §4.1, flattened).
        #[serde(flatten)]
        pub spec: StudySpec,
        /// Relaxed validation (slug + repo only); compact card instead of the full card.
        pub partial: bool,
        /// Rows the agent lists as agreed with the user (151 §4.3 row names).
        pub confirmed: Vec<String>,
    }
}

wire! {
    /// The chart (140 §13.1): dots, staircase, holdout diamonds, drift markers.
    #[derive(Default)]
    #[serde(default)]
    pub struct ChartSeries as "StudyChartSeries" {
        /// One per experiment.
        pub experiments: Vec<ChartPoint>,
        /// The incumbent staircase.
        pub incumbent: Vec<ChartPoint>,
        /// Holdout points (second axis).
        pub holdout: Vec<ChartPoint>,
        /// Drift events.
        pub drift: Vec<ChartPoint>,
        /// Baseline mean of the current segment.
        pub baseline_mean: Option<f64>,
        /// σ of the current segment (the band).
        pub sigma: Option<f64>,
    }
}

wire! {
    /// One chart point.
    #[derive(Default)]
    #[serde(default)]
    pub struct ChartPoint as "StudyChartPoint" {
        /// Experiment number.
        pub n: u32,
        /// Verdict tag (colour); empty for staircase/holdout/drift points.
        pub verdict: String,
        /// Metric value.
        pub metric: Option<f64>,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// One finalize changeset (140 §11.2).
    #[derive(Default)]
    #[serde(default)]
    pub struct Changeset as "StudyChangeset" {
        /// Index.
        pub k: u32,
        /// Short name.
        pub name: String,
        /// `autoresearch/<slug>+<k>-<name>` (git cannot hold `autoresearch/<slug>/…` beside the study branch).
        pub branch: String,
        /// Files touched.
        pub files: Vec<String>,
        /// Experiments in it.
        pub experiments: Vec<u32>,
        /// Δ % measured alone on the base.
        pub delta_alone_pct: Option<f64>,
        /// Δ % in the final incumbent.
        pub delta_together_pct: Option<f64>,
        /// Changesets it only helps together with.
        pub only_with: Vec<u32>,
        /// Review verdicts of its keeps (worst first).
        pub review: Option<ReviewVerdict>,
    }
}

wire! {
    /// One run-table row (140 §12.5 `autoresearch.experiments`, §13.1).
    #[derive(Default)]
    #[serde(default)]
    pub struct ExperimentRow {
        /// The study.
        pub slug: String,
        /// Number.
        pub n: u32,
        /// Wave (fire) number; `None` sequential.
        pub wave: Option<u32>,
        /// Slot in the wave.
        pub slot: Option<u8>,
        /// Segment.
        pub segment: u32,
        /// State.
        pub state: Option<ExperimentState>,
        /// Verdict.
        pub verdict: Option<Verdict>,
        /// The idea.
        pub idea: String,
        /// Category.
        pub category: String,
        /// Candidate mean.
        pub metric: Option<f64>,
        /// Direction-normalised Δ.
        pub delta: Option<f64>,
        /// Δ %.
        pub delta_pct: Option<f64>,
        /// z.
        pub z: Option<f64>,
        /// Samples spent.
        pub n_samples: u32,
        /// Keep commit.
        pub commit_sha: Option<String>,
        /// Paths outside scope.
        pub deviations: Vec<String>,
        /// Review.
        pub review: Option<ReviewVerdict>,
        /// Model cost, USD.
        pub cost_usd: f64,
        /// RFC 3339.
        pub started_at: String,
        /// RFC 3339.
        pub finished_at: Option<String>,
    }
}

wire! {
    /// `autoresearch.experiment` (140 §12.5): the row plus everything behind it.
    #[derive(Default)]
    #[serde(default)]
    pub struct ExperimentDetail {
        /// The row.
        pub row: ExperimentRow,
        /// Hypothesis.
        pub hypothesis: String,
        /// Description from `experiment_log`.
        pub description: String,
        /// Learned.
        pub learned: String,
        /// Next.
        pub next: Option<String>,
        /// Decision asked.
        pub decision: Option<Decision>,
        /// Justification for deviations.
        pub justification: Option<String>,
        /// Incumbent sha at the start.
        pub parent_sha: String,
        /// Normalised patch hash.
        pub patch_hash: Option<String>,
        /// Samples.
        pub samples: Vec<Sample>,
        /// The verdict line.
        pub line: String,
        /// The diff / patch text (≤ 256 KiB).
        pub diff: Option<String>,
        /// Log tails per sample.
        pub tails: Vec<String>,
        /// Cases that moved most.
        pub cases_moved: Vec<CaseDelta>,
    }
}

// ───────────────────────────── §12.3 events ─────────────────────────────

wire! {
    /// The plugin's bus events (140 §12.3), bridged to RPC clients.
    #[serde(tag = "type")]
    pub enum StudyEvent {
        /// A study changed state.
        StudyChanged {
            /// Which.
            slug: String,
            /// New state.
            state: StudyState,
            /// Why, when there is more to say.
            reason: Option<String>,
        },
        /// `experiment_begin` ran.
        ExperimentStarted {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// Wave slot.
            slot: Option<u8>,
        },
        /// Measuring progress.
        ExperimentProgress {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// Where.
            phase: Phase,
            /// Sample index.
            sample: u8,
            /// Samples planned.
            of: u8,
            /// Since `experiment_begin`.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            elapsed_ms: u64,
        },
        /// `experiment_log` applied.
        ExperimentFinished {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// The verdict.
            verdict: Verdict,
            /// Candidate mean.
            metric: Option<f64>,
            /// Δ %.
            delta_pct: Option<f64>,
            /// z.
            z: Option<f64>,
        },
        /// Finalize delivered.
        StudyFinished {
            /// Which study.
            slug: String,
            /// Why.
            reason: StopReason,
            /// `time_ms 52.1 → 41.2 (−20.9 %)`.
            headline: String,
            /// The report document id.
            report_doc: String,
        },
    }
}

// ───────────────────────────── §12.3 actor messages ─────────────────────────────

/// `Create` → `StudyInfo` (state `preparing`).
#[derive(Clone, Debug)]
pub struct Create {
    /// The approved spec.
    pub spec: StudySpec,
    /// The chat the draft card lives in (start and result cards go there).
    pub origin: SessionId,
    /// Now or in the idle window.
    pub start: StartWhen,
}

/// `StartDraft` → `StudyInfo`: the draft card's press (`autoresearch.create {draft_id, start}`).
#[derive(Clone, Debug)]
pub struct StartDraft {
    /// The `app` draft id.
    pub draft_id: String,
    /// Now or in the idle window.
    pub start: StartWhen,
}

/// `Begin` → `Brief`.
#[derive(Clone, Debug)]
pub struct Begin {
    /// The calling session (study session, or a wave child's parent).
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
}

/// `Run` → `RunReport` (blocks while measuring; cancel-safe).
#[derive(Clone, Debug)]
pub struct Run {
    /// The calling session.
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
    /// Arguments.
    pub args: RunArgs,
}

/// `Log` → `LogReport`.
#[derive(Clone, Debug)]
pub struct Log {
    /// The calling session.
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
    /// Arguments.
    pub args: LogArgs,
}

/// `Notes` → `NotesReport`.
#[derive(Clone, Debug)]
pub struct Notes {
    /// The calling session.
    pub session: SessionId,
    /// Arguments.
    pub args: NotesArgs,
}

/// `Control` → `StudyInfo`.
#[derive(Clone, Debug)]
pub struct Control {
    /// Which study.
    pub slug: String,
    /// What.
    pub op: ControlOp,
    /// Who.
    pub by: By,
}

/// `Steer` → `SteerId`.
#[derive(Clone, Debug)]
pub struct SteerMsg {
    /// Which study.
    pub slug: String,
    /// The text.
    pub text: String,
    /// Who.
    pub by: By,
}

/// `IdeaOp` → `Idea`.
#[derive(Clone, Debug)]
pub struct IdeaOp {
    /// Which study.
    pub slug: String,
    /// What.
    pub op: IdeaChange,
    /// Who.
    pub by: By,
}

/// `Status` → `Vec<StudyInfo>` (one, or all newest first).
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// One study, or every study.
    pub slug: Option<String>,
}

/// `Detail` → `StudyDetail` (`autoresearch.get`).
#[derive(Clone, Debug)]
pub struct Detail {
    /// Which study.
    pub slug: String,
}

/// `Experiments` → `Vec<ExperimentRow>` newest first.
#[derive(Clone, Debug)]
pub struct Experiments {
    /// Which study.
    pub slug: String,
    /// Rows with `n < before`.
    pub before: Option<u32>,
    /// Page size.
    pub limit: u32,
}

/// `Experiment` → `ExperimentDetail`.
#[derive(Clone, Debug)]
pub struct Experiment {
    /// Which study.
    pub slug: String,
    /// Number.
    pub n: u32,
}

/// `Settings` → `AutoresearchConfig` (`autoresearch.settings.get`).
#[derive(Clone, Debug, Default)]
pub struct Settings;

/// `SetSettings` → `AutoresearchConfig` persisted and applied (`autoresearch.settings.set`).
#[derive(Clone, Debug)]
pub struct SetSettings(pub crate::config::AutoresearchConfig);

/// `DraftRoot` → the drafts' root (`<data_dir>/autoresearch` or `root:`), for the 151 draft RPCs.
#[derive(Clone, Debug, Default)]
pub struct DraftRoot;

/// `StudySlugs` → every study's slug (a draft with a study is "started"; the rail skips it).
#[derive(Clone, Debug, Default)]
pub struct StudySlugs;

// ───────────────────────────── 159 §7 W2: the case set ─────────────────────────────
// W2-b: the one typed block W2 fills in this frozen file (§5.1 "each wave fills its own");
// kept at the end so the other waves' blocks never touch the same lines.

wire! {
    /// Where a prompt study's cases come from (159 §7).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum CaseSource {
        /// JSONL `{id, input, ideal?, check?, tags}` at `CaseSetSpec.path`, repo-relative.
        #[default]
        File,
        /// `dreaming/cases.jsonl` filtered by `skill_hint` (159 §13.4).
        Dreaming,
    }
}

wire! {
    /// 159 §7 W2: `cases: {source, path, skill_hint, artefact, render, split, repeats,
    /// max_cases, train_shown}` — the case set a prompt study scores each tree on. Every
    /// field defaults and is skipped at its default.
    #[derive(Default, Eq)]
    #[serde(default)]
    pub struct CaseSetSpec {
        /// `file` | `dreaming`.
        pub source: CaseSource,
        /// The JSONL file for `file`, repo-relative (list it in `evaluator_paths` too).
        #[serde(skip_serializing_if = "String::is_empty")]
        pub path: String,
        /// The `skill_hint` filter for `dreaming`.
        #[serde(skip_serializing_if = "String::is_empty")]
        pub skill_hint: String,
        /// The text file the session edits, repo-relative: the system prompt, or what
        /// `render`'s `{{artefact}}` becomes.
        #[serde(skip_serializing_if = "String::is_empty")]
        pub artefact: String,
        /// A system-prompt template holding `{{artefact}}`; empty = the artefact is the prompt.
        #[serde(skip_serializing_if = "String::is_empty")]
        pub render: String,
        /// Percent shares `train/val/holdout`; empty = `40/40/20` (159 W-g).
        #[serde(skip_serializing_if = "String::is_empty")]
        pub split: String,
        /// Target calls per case per sample; 0 = 1.
        #[serde(skip_serializing_if = "is_zero_u8")]
        pub repeats: u8,
        /// Cap on cases read from the source after filtering; 0 = all.
        #[serde(skip_serializing_if = "is_zero_u32")]
        pub max_cases: u32,
        /// Train cases replayed per sample for the brief's feedback; 0 = 16.
        #[serde(skip_serializing_if = "is_zero_u32")]
        pub train_shown: u32,
    }
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_parses_relative_and_absolute() {
        assert_eq!(Effect::parse("0.5%"), Ok(Effect::Relative(0.005)));
        assert_eq!(Effect::parse(" 5 % "), Ok(Effect::Relative(0.05)));
        assert_eq!(Effect::parse("0.3"), Ok(Effect::Absolute(0.3)));
        assert_eq!(Effect::parse(""), Ok(Effect::Absolute(0.0)));
        assert!(Effect::parse("abc").is_err());
        assert!(Effect::parse("-1%").is_err());
        assert_eq!(Effect::Relative(0.01).against(-200.0), 2.0);
        assert_eq!(Effect::Absolute(0.7).against(42.0), 0.7);
    }

    /// The tagged enums round-trip and carry the design's tags.
    #[test]
    fn tagged_enums_round_trip() {
        let s = StudyState::Paused {
            reason: PauseReason::CrashStreak,
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "paused");
        assert_eq!(j["reason"]["kind"], "crash_streak");
        assert_eq!(serde_json::from_value::<StudyState>(j).unwrap(), s);
        let v = Verdict::Refused {
            reason: "duplicate of #12".into(),
        };
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["kind"], "refused");
        assert_eq!(v.tag(), "refused");
        assert_eq!(serde_json::from_value::<Verdict>(j).unwrap(), v);
        let e = StudyEvent::ExperimentProgress {
            slug: "s".into(),
            n: 3,
            phase: Phase::Sampling,
            sample: 2,
            of: 3,
            elapsed_ms: 10,
        };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["type"], "ExperimentProgress");
        assert_eq!(j["phase"], "sampling");
        assert_eq!(serde_json::from_value::<StudyEvent>(j).unwrap(), e);
        let op: ControlOp = serde_json::from_str(r#"{"op":"restore","n":4}"#).unwrap();
        assert_eq!(op, ControlOp::Restore { n: 4 });
    }

    /// 151 D5: `autoresearch_draft` keeps taking a bare spec (`partial` false, nothing
    /// confirmed); the flattened spec and the flags read from one object, and the
    /// enums wire as snake_case.
    #[test]
    fn draft_args_flatten_the_spec_with_default_flags() {
        let a: DraftArgs = serde_json::from_str(r#"{"slug":"x","goal":"g","repo":"/r"}"#).unwrap();
        assert_eq!(
            (a.spec.slug.as_str(), a.partial, a.confirmed.len()),
            ("x", false, 0)
        );
        assert_eq!(a.spec.budget, BudgetSpec::default());
        let a: DraftArgs = serde_json::from_str(
            r#"{"slug":"x","repo":"/r","partial":true,"confirmed":["goal","repo"],"metric":{"name":"t"}}"#,
        )
        .unwrap();
        assert_eq!(
            (
                a.partial,
                a.confirmed.as_slice(),
                a.spec.metric.name.as_str()
            ),
            (true, &["goal".to_owned(), "repo".to_owned()][..], "t")
        );
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["slug"], "x", "{v}");
        assert_eq!(v["partial"], true, "{v}");
        assert_eq!(
            serde_json::to_value([FieldState::Verified, FieldState::Missing]).unwrap(),
            serde_json::json!(["verified", "missing"])
        );
        assert_eq!(
            serde_json::from_str::<ProbeKind>(r#""holdout""#).unwrap(),
            ProbeKind::Holdout
        );
    }

    /// A spec with only the required keys fills the R1 defaults; the §4.1 YAML parses.
    #[test]
    fn spec_defaults_and_the_design_yaml() {
        let s: StudySpec = serde_json::from_str(r#"{"slug":"x","goal":"g","repo":"/r"}"#).unwrap();
        assert_eq!(s.budget, BudgetSpec::default());
        assert_eq!(s.bench.timeout, "600s");
        assert!(s.bench.exclusive);
        let yaml = r#"
slug: sort-speed
goal: "Make parse+render faster."
repo: /Users/x/liquid
base: HEAD
metric: {name: time_ms, unit: ms, direction: lower, min_effect: 0.5%}
secondary_guards:
  - {name: alloc_mb, max_regression: 5%}
bench: {command: bench.sh, timeout: 600s, repeats: 3, exclusive: true}
checks: {command: "bundle exec rake test", timeout: 300s}
holdout: {command: bench-holdout.sh, every: 5}
evaluator_paths: [fixtures/, test/, Rakefile]
scope: [lib/liquid/**]
off_limits: [fixtures/**, test/**]
warm: "bundle install"
budget: {usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120}
parallel: 1
models: {executor: null, review: null, advisor: null}
window: null
"#;
        let s: StudySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(s.metric.direction, Direction::Lower);
        assert_eq!(
            Effect::parse(&s.metric.min_effect),
            Ok(Effect::Relative(0.005))
        );
        assert_eq!(s.holdout.as_ref().unwrap().every, 5);
        let back: StudySpec = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    /// 159 §5.12 item 2: a 140 spec, its flattened draft arguments and a 151 draft serialise
    /// byte for byte as before C — no new key appears while every new field is at its default.
    #[test]
    fn a_140_spec_serialises_without_any_159_key() {
        let s = StudySpec {
            slug: "x".into(),
            goal: "g".into(),
            repo: "/r".into(),
            ..StudySpec::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        for key in [
            "axes",
            "objectives",
            "cases",
            "judge",
            "population",
            "literature",
            "hooks",
            "world",
            "suite",
            "calls",
            "target",
        ] {
            assert!(!json.contains(&format!("\"{key}\"")), "{key} in {json}");
        }
        let args = DraftArgs {
            spec: s.clone(),
            ..DraftArgs::default()
        };
        let flat = serde_json::to_string(&args).unwrap();
        assert!(!flat.contains("\"axes\"") && !flat.contains("\"objectives\""));
        // The 140 shape (every key of the 140 struct, nothing else) parses and round-trips.
        let back: StudySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert!(s.axes.is_default());
        assert_eq!(s.kind(), KIND_SCALAR);
        assert_eq!(s.rule(), Some(GateRule::ZGate));
        // And a spec with a 159 field set writes it.
        let mut p = s.clone();
        p.axes.objectives = ObjectivesMode::Pareto;
        let pj = serde_json::to_string(&p).unwrap();
        assert!(pj.contains("\"axes\"") && pj.contains("\"pareto\""));
        assert_eq!(p.kind(), KIND_PARETO);
        assert_eq!(p.rule(), Some(GateRule::Dominance));
    }

    /// 159 §5.12 item 4: `objectives_in_force()` from `metric` + guards; `normalise()` mirrors
    /// the headline and the guards back; the round trip is the identity on a 140 spec.
    #[test]
    fn objectives_in_force_and_normalise() {
        let mut s = StudySpec {
            metric: MetricSpec {
                name: "time_ms".into(),
                unit: "ms".into(),
                direction: Direction::Lower,
                min_effect: "0.5%".into(),
            },
            secondary_guards: vec![Guard {
                name: "alloc_mb".into(),
                max_regression: "5%".into(),
                direction: Direction::Lower,
            }],
            ..StudySpec::default()
        };
        let before = s.clone();
        let o = s.objectives_in_force();
        assert_eq!(o.len(), 2);
        assert_eq!(
            (o[0].name.as_str(), o[0].role),
            ("time_ms", ObjectiveRole::Primary)
        );
        assert_eq!(o[0].min_effect, "0.5%");
        assert_eq!(
            (o[1].name.as_str(), o[1].role),
            ("alloc_mb", ObjectiveRole::Guard)
        );
        assert_eq!(o[1].max_regression, "5%");
        assert_eq!(s.headline().name, "time_ms");
        s.normalise();
        assert_eq!(s, before, "normalise is the identity without `objectives`");

        // With objectives: the first primary becomes `metric`, the guards `secondary_guards`.
        s.objectives = vec![
            Objective {
                name: "size_kb".into(),
                unit: "kB".into(),
                direction: Direction::Lower,
                role: ObjectiveRole::Guard,
                max_regression: "10%".into(),
                ..Objective::default()
            },
            Objective {
                name: "score".into(),
                unit: "".into(),
                direction: Direction::Higher,
                role: ObjectiveRole::Primary,
                min_effect: "1%".into(),
                ..Objective::default()
            },
            Objective {
                name: "tokens".into(),
                role: ObjectiveRole::Report,
                ..Objective::default()
            },
        ];
        s.normalise();
        assert_eq!(s.metric.name, "score");
        assert_eq!(s.metric.direction, Direction::Higher);
        assert_eq!(s.metric.min_effect, "1%");
        assert_eq!(s.secondary_guards.len(), 1);
        assert_eq!(s.secondary_guards[0].name, "size_kb");
        assert_eq!(s.secondary_guards[0].max_regression, "10%");
        assert_eq!(s.objectives_in_force(), s.objectives);
        let mut again = s.clone();
        again.normalise();
        assert_eq!(again, s, "normalise is idempotent");
    }

    /// 159 §5.12 item 4: `is_open()` covers `Waiting`; `is_dangling()` does not (W-q).
    #[test]
    fn is_open_covers_waiting_and_dangling_does_not() {
        use ExperimentState as S;
        for s in [S::Open, S::Measuring, S::Measured] {
            assert!(s.is_open() && s.is_dangling() && !s.is_closed(), "{s:?}");
        }
        assert!(S::Waiting.is_open());
        assert!(!S::Waiting.is_dangling());
        assert!(!S::Waiting.is_closed());
        for s in [
            S::Kept,
            S::Discarded,
            S::Crashed,
            S::ChecksFailed,
            S::Refused,
            S::RejectedByReview,
            S::Abandoned,
            S::Interrupted,
            S::Timeout,
        ] {
            assert!(!s.is_open() && !s.is_dangling() && s.is_closed(), "{s:?}");
        }
        assert_eq!(S::Waiting.tag(), "waiting");
        assert_eq!(serde_json::to_string(&S::Waiting).unwrap(), "\"waiting\"");
    }

    /// 159 §5.6: the rule table, and the combinations no wave has built.
    #[test]
    fn rule_table_and_kind_words() {
        let mk = |o: ObjectivesMode, s: Scoring, se: Search, a: Artefact| StudySpec {
            axes: Axes {
                objectives: o,
                scoring: s,
                search: se,
                artefact: a,
            },
            ..StudySpec::default()
        };
        use {Artefact as A, ObjectivesMode as O, Scoring as S, Search as Se};
        assert_eq!(
            mk(O::Single, S::Bench, Se::Ratchet, A::Repo).rule(),
            Some(GateRule::ZGate)
        );
        assert_eq!(
            mk(O::Pareto, S::Bench, Se::Front, A::Repo).rule(),
            Some(GateRule::Dominance)
        );
        assert_eq!(
            mk(O::Single, S::Cases, Se::Ratchet, A::File).rule(),
            Some(GateRule::Paired)
        );
        assert_eq!(
            mk(O::Pareto, S::Cases, Se::Ratchet, A::File).rule(),
            Some(GateRule::Paired)
        );
        assert_eq!(
            mk(O::Judged, S::Pairwise, Se::Ratchet, A::File).rule(),
            Some(GateRule::WinBound)
        );
        assert_eq!(
            mk(O::Single, S::World, Se::Ratchet, A::Repo).rule(),
            Some(GateRule::Msprt)
        );
        assert_eq!(
            mk(O::Single, S::Suite, Se::Ratchet, A::Mechanism).rule(),
            Some(GateRule::Paired)
        );
        assert_eq!(mk(O::Judged, S::Bench, Se::Ratchet, A::Repo).rule(), None);
        assert_eq!(mk(O::Pareto, S::World, Se::Ratchet, A::Repo).rule(), None);
        assert_eq!(
            mk(O::Single, S::Suite, Se::Ratchet, A::Mechanism).kind(),
            KIND_META
        );
        assert_eq!(
            mk(O::Single, S::World, Se::Ratchet, A::Repo).kind(),
            KIND_WORLD
        );
        assert_eq!(
            mk(O::Single, S::Bench, Se::Population, A::Repo).kind(),
            KIND_POPULATION
        );
        assert_eq!(
            mk(O::Judged, S::Pairwise, Se::Ratchet, A::File).kind(),
            KIND_JUDGED
        );
        assert_eq!(
            mk(O::Single, S::Cases, Se::Ratchet, A::File).kind(),
            KIND_PROMPT
        );
        assert_eq!(
            mk(O::Pareto, S::Bench, Se::Front, A::Repo).kind(),
            KIND_PARETO
        );
        // The tagged forms every wave and the UI read.
        let v = serde_json::to_value(KindView::Scalar).unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "scalar" }));
        let d = serde_json::to_value(GateDecision {
            v: GateDecision::V,
            rule: GateRule::Dominance,
            ..GateDecision::default()
        })
        .unwrap();
        assert_eq!(d["v"], 2);
        assert_eq!(d["rule"], "dominance");
        let w = serde_json::to_value(WaitInfo::default()).unwrap();
        assert_eq!(w["why"], "approval");
    }
}
