//! The `autoresearch:` section of config.yaml (design doc 140 §12.2, R1
//! defaults). Read once at start like every section; `autoresearch.settings.set`
//! rewrites it whole ([`persist`]). Every field has a default, so an absent
//! section is the design's defaults. `models.executor: null` means
//! `agent.model` (R1-D5) and is resolved at use time, never stored.

use std::path::Path;

use serde::{Deserialize, Serialize};

macro_rules! wire {
    ($(#[$m:meta])* pub struct $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        #[serde(default)]
        pub struct $name { $($body)* }
    };
}

wire! {
    /// `autoresearch:` (140 §12.2).
    pub struct AutoresearchConfig {
        /// `false`: no tools, no prompt block, no RPC beyond "disabled"; existing study files stay paused.
        pub enabled: bool,
        /// `None` = `<data_dir>/autoresearch`.
        pub root: Option<String>,
        /// Pre-filled values of a new draft (R1-D1).
        pub defaults: Defaults,
        /// The sequential gate (140 §5.2).
        pub gate: GateConfig,
        /// Baseline noise acceptance (140 §4.2).
        pub noise: Noise,
        /// Plateau detection (140 §8.4).
        pub plateau: Plateau,
        /// Pause rules (140 §8.4).
        pub pause: Pause,
        /// The reviewer (140 §9.1).
        pub review: Review,
        /// Model overrides per role (140 §9.3 as amended by R1-D5); `None` = default.
        /// 194: each role's effort sits beside its model (`models.*_effort`, `review.effort`).
        pub models: Models,
        /// The brief's caps (140 §7).
        pub brief: BriefConfig,
        /// The novelty check (140 §7.3).
        pub novelty: Novelty,
        /// The bench slot and load precondition (140 §5.5).
        pub bench: Bench,
        /// Largest `parallel:` a spec may ask for (140 §10).
        pub max_parallel: u8,
        /// Draft cards above this `budget.usd` need the user to type the amount.
        pub confirm_budget_usd: f64,
        /// Worktrees of finished studies are removed after this many days.
        pub keep_finished_days: u32,
        /// Git author for the plugin's commits (140 §6).
        pub git: Git,
        /// Channel pings (R1-D6: off).
        pub notify: Notify,
    }
}

wire! {
    /// `defaults:` — what a new draft is pre-filled with.
    pub struct Defaults as "AutoresearchDefaults" {
        /// `bench.repeats`.
        pub repeats: u8,
        /// `bench.timeout` (humantime).
        pub bench_timeout: String,
        /// `checks.timeout` (humantime).
        pub checks_timeout: String,
        /// `experiment_run` retries after a crash within one experiment (140 §4.3).
        pub crash_retries: u8,
        /// Budget pre-fill (R1-D1).
        pub budget: crate::types::BudgetSpec,
        /// `parallel:`.
        pub parallel: u8,
    }
}

wire! {
    /// `gate:` (140 §5.2 step 3).
    pub struct GateConfig as "AutoresearchGateConfig" {
        /// Confirmed keep threshold on means.
        pub z: f64,
        /// One-sample keep.
        pub z_fast: f64,
        /// One-sample discard below this.
        pub z_discard: f64,
        /// A keep-candidate at or above this is re-measured and reviewed as suspicious.
        pub z_suspicious: f64,
        /// Extra samples in the band.
        pub confirm_runs: u8,
        /// Take an incumbent sample when its newest is older than this many experiments.
        pub remeasure_after: u32,
        /// Every this many keeps, one incumbent sample at the next experiment's start.
        pub remeasure_every: u32,
        /// A keep whose |Δ| is at least this many σ opens a new segment automatically
        /// (§5.3 Revision 2); 0 disables.
        pub regime_sigma: f64,
    }
}

wire! {
    /// `noise:`.
    pub struct Noise as "AutoresearchNoise" {
        /// Baseline σ / mean above this refuses to start the study.
        pub max_cv: f64,
    }
}

wire! {
    /// `plateau:`.
    pub struct Plateau as "AutoresearchPlateau" {
        /// Experiments without a keep before explore mode (then finalize).
        pub window: u32,
    }
}

wire! {
    /// `pause:`.
    pub struct Pause as "AutoresearchPause" {
        /// Consecutive crashes that pause the study.
        pub crash_streak: u32,
        /// Consecutive custody refusals that pause the study.
        pub custody_refusals: u32,
    }
}

wire! {
    /// `review:` (140 §9.1).
    pub struct Review as "AutoresearchReview" {
        /// `None` = other-vendor default; `"off"` disables.
        pub model: Option<String>,
        /// 194: the reviewer's effort beside `model`; `None` = Default (no reasoning setting).
        pub effort: Option<String>,
        /// What a SUSPICIOUS verdict does.
        pub on_suspicious: OnSuspicious,
        /// Review flags in the study that pause it.
        pub pause_after: u32,
    }
}

/// `review.on_suspicious`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "AutoresearchOnSuspicious")
)]
#[serde(rename_all = "snake_case")]
pub enum OnSuspicious {
    /// Revert the keep, keep the patch, raise an attention row with Restore (R1-D2).
    #[default]
    Revert,
    /// Keep the change and only mark it.
    Flag,
}

wire! {
    /// `models:` — `provider/model` per role; `None` = default (R1-D5).
    #[derive(Default)]
    pub struct Models as "AutoresearchModels" {
        /// Default `agent.model`.
        pub executor: Option<String>,
        /// 194: the executor session's effort beside `executor`; `None` = `agent.effort`.
        pub executor_effort: Option<String>,
        /// Default 125's advisor when configured, else the executor.
        pub advisor: Option<String>,
        /// 194: the plateau consult's effort beside `advisor`; `None` = Default.
        pub advisor_effort: Option<String>,
    }
}

wire! {
    /// `brief:` caps (140 §7.2).
    pub struct BriefConfig as "AutoresearchBriefConfig" {
        /// Whole brief, bytes.
        pub max_bytes: u32,
        /// Recent experiment rows.
        pub recent: u32,
        /// Dead-end lines.
        pub dead_ends: u32,
        /// Open ideas.
        pub ideas: u32,
        /// Playbook cap, bytes (`experiment_notes` refuses above it).
        pub playbook_bytes: u32,
        /// `code_map` budget, tokens.
        pub code_map_tokens: u32,
    }
}

wire! {
    /// `novelty:`.
    pub struct Novelty as "AutoresearchNovelty" {
        /// FTS similarity above which a past discarded experiment counts as similar.
        pub threshold: f64,
    }
}

wire! {
    /// `bench:` (140 §5.5).
    pub struct Bench as "AutoresearchBench" {
        /// `builtin` (mkdir slot under `/tmp/theuth-bench-slots/0`) or a wrapper command.
        pub lock: String,
        /// 1-minute load average ÷ cores a scored sample waits for.
        pub max_load: f64,
        /// Longest wait per experiment (humantime); past it the sample runs flagged `noisy`.
        pub wait: String,
    }
}

wire! {
    /// `git:`.
    #[derive(Default)]
    pub struct Git as "AutoresearchGit" {
        /// `None` = `theuth autoresearch <noreply@theuth.local>`.
        pub author: Option<String>,
    }
}

wire! {
    /// `notify:`.
    #[derive(Default)]
    pub struct Notify as "AutoresearchNotify" {
        /// Ping channels on a new best (R1-D6: off).
        pub new_best: bool,
    }
}

impl Default for AutoresearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root: None,
            defaults: Defaults::default(),
            gate: GateConfig::default(),
            noise: Noise::default(),
            plateau: Plateau::default(),
            pause: Pause::default(),
            review: Review::default(),
            models: Models::default(),
            brief: BriefConfig::default(),
            novelty: Novelty::default(),
            bench: Bench::default(),
            max_parallel: 4,
            confirm_budget_usd: 50.0,
            keep_finished_days: 14,
            git: Git::default(),
            notify: Notify::default(),
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            repeats: 3,
            bench_timeout: "600s".into(),
            checks_timeout: "300s".into(),
            crash_retries: 2,
            budget: crate::types::BudgetSpec::default(),
            parallel: 1,
        }
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            z: 2.0,
            z_fast: 3.5,
            z_discard: 1.0,
            z_suspicious: 8.0,
            confirm_runs: 2,
            remeasure_after: 6,
            remeasure_every: 5,
            regime_sigma: 10.0,
        }
    }
}

impl Default for Noise {
    fn default() -> Self {
        Self { max_cv: 0.05 }
    }
}

impl Default for Plateau {
    fn default() -> Self {
        Self { window: 12 }
    }
}

impl Default for Pause {
    fn default() -> Self {
        Self {
            crash_streak: 4,
            custody_refusals: 2,
        }
    }
}

impl Default for Review {
    fn default() -> Self {
        Self {
            model: None,
            effort: None,
            on_suspicious: OnSuspicious::Revert,
            pause_after: 3,
        }
    }
}

impl Default for BriefConfig {
    fn default() -> Self {
        Self {
            max_bytes: 12288,
            recent: 8,
            dead_ends: 30,
            ideas: 12,
            playbook_bytes: 4096,
            code_map_tokens: 1200,
        }
    }
}

impl Default for Novelty {
    fn default() -> Self {
        Self { threshold: 0.6 }
    }
}

impl Default for Bench {
    fn default() -> Self {
        Self {
            lock: "builtin".into(),
            max_load: 0.5,
            wait: "20m".into(),
        }
    }
}

/// What the `agent:` section offers as role defaults (R1-D5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDefaults {
    /// `agent.model` — the executor default.
    pub model: String,
    /// `agent.subagents.model`, `agent.title_model`, `agent.model` in that
    /// order, non-empty only — the reviewer picks the first of another vendor
    /// (R1-D5: `agent.model` is in the chain so an executor overridden to
    /// another vendor still gets the session model as its judge).
    pub others: Vec<String>,
    /// `agent.communication` — `spawn_subagent` exists only when it is on (50 §2.1, a
    /// ceiling no session overrides); a wave study (`parallel > 1`) needs it (140-P live).
    pub communication: bool,
}

impl AgentDefaults {
    /// Read the `agent:` section (defaults when absent). This is another
    /// plugin's section (plugins.md §3.6), read from the startup snapshot as
    /// dreaming does for the same R1-D5 defaults; the `agent` actor exposes no
    /// query for its model roles, and a live change of `agent.model` reaches
    /// the next study through a restart.
    /// `paired`: this engine has a paired machine, which turns an absent
    /// `agent.communication` on (183 §6.1, D6).
    pub fn from_runtime(rt: &runtime::Runtime, paired: bool) -> Self {
        let a: agent::AgentConfig = rt.section("agent").unwrap_or_default();
        let others = [
            a.subagents.model.clone(),
            a.title_model.clone(),
            Some(a.model.clone()),
        ]
        .into_iter()
        .flatten()
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
        .collect();
        Self {
            model: a.model,
            others,
            communication: a.communication.unwrap_or(paired),
        }
    }
}

/// `provider` of `provider/model`.
fn vendor(model: &str) -> &str {
    model.split('/').next().unwrap_or(model)
}

impl AutoresearchConfig {
    /// `<data_dir>/autoresearch` or `root:`.
    pub fn root(&self, data_dir: &Path) -> std::path::PathBuf {
        match &self.root {
            Some(r) if !r.trim().is_empty() => Path::new(r).to_path_buf(),
            _ => data_dir.join(crate::DIR),
        }
    }

    /// The executor model (R1-D5): the study's pick, else the section's, else `agent.model`.
    pub fn executor(&self, spec: &crate::types::ModelsSpec, agent: &AgentDefaults) -> String {
        pick(&spec.executor)
            .or_else(|| pick(&self.models.executor))
            .unwrap_or(&agent.model)
            .to_owned()
    }

    /// The reviewer model (R1-D5): the study's pick, else `review.model`, else
    /// the first `agent:` model of another vendor than the executor, else the
    /// executor. `None` when reviewing is `off`.
    pub fn reviewer(
        &self,
        spec: &crate::types::ModelsSpec,
        agent: &AgentDefaults,
    ) -> Option<String> {
        let executor = self.executor(spec, agent);
        let picked = pick(&spec.review).or_else(|| pick(&self.review.model));
        match picked {
            Some("off") => None,
            Some(m) => Some(m.to_owned()),
            None => Some(
                agent
                    .others
                    .iter()
                    .find(|m| vendor(m) != vendor(&executor))
                    .cloned()
                    .unwrap_or(executor),
            ),
        }
    }

    /// The advisor model: the study's pick, else `models.advisor`, else `None`
    /// (= 125's advisor when configured, else the executor — resolved by the caller).
    pub fn advisor(&self, spec: &crate::types::ModelsSpec) -> Option<String> {
        pick(&spec.advisor)
            .or_else(|| pick(&self.models.advisor))
            .map(str::to_owned)
    }

    /// 194 D4a/d: the executor's effort goes with the model it was picked with — the study's
    /// `models.executor_effort` with its `executor`, else the section's; `None` (= `agent.effort`)
    /// when neither picks a model.
    pub fn executor_effort(&self, spec: &crate::types::ModelsSpec) -> Option<String> {
        with_model(&[
            (&spec.executor, &spec.executor_effort),
            (&self.models.executor, &self.models.executor_effort),
        ])
    }

    /// 194: the reviewer's effort, paired like [`Self::executor_effort`] (`review.effort`);
    /// `None` for the other-vendor default and for `off`.
    pub fn reviewer_effort(&self, spec: &crate::types::ModelsSpec) -> Option<String> {
        if pick(&spec.review).or_else(|| pick(&self.review.model)) == Some("off") {
            return None;
        }
        with_model(&[
            (&spec.review, &spec.review_effort),
            (&self.review.model, &self.review.effort),
        ])
    }

    /// 194: the plateau consult's effort, paired like [`Self::executor_effort`].
    pub fn advisor_effort(&self, spec: &crate::types::ModelsSpec) -> Option<String> {
        with_model(&[
            (&spec.advisor, &spec.advisor_effort),
            (&self.models.advisor, &self.models.advisor_effort),
        ])
    }

    /// 194: a prompt artefact's target effort: `models.target_effort` with its `target`, else
    /// the executor's (the target defaults to the executor).
    pub fn target_effort(&self, spec: &crate::types::ModelsSpec) -> Option<String> {
        with_model(&[
            (&spec.target, &spec.target_effort),
            (&spec.executor, &spec.executor_effort),
            (&self.models.executor, &self.models.executor_effort),
        ])
    }
}

/// 194: the effort beside the first `(model, effort)` pair whose model is picked; `None` when none is.
pub(crate) fn with_model(pairs: &[(&Option<String>, &Option<String>)]) -> Option<String> {
    pairs
        .iter()
        .find(|(m, _)| pick(m).is_some())
        .and_then(|(_, e)| pick(e).map(str::to_owned))
}

fn pick(m: &Option<String>) -> Option<&str> {
    m.as_deref().map(str::trim).filter(|m| !m.is_empty())
}

/// 194 D4b–d: a role's effort as `autoresearch.settings.set` stores it, checked against the
/// role's model (`was` = the model before the set); `off` and Default carry none.
pub async fn settle_role(
    model: &Option<String>,
    effort: &Option<String>,
    was: &Option<String>,
) -> std::result::Result<Option<String>, String> {
    let m = pick(model).filter(|m| *m != "off");
    let info = agent::lifecycle::effort_model(m).await;
    agent::lifecycle::settle_effort(
        m,
        info.as_ref(),
        effort.as_deref(),
        pick(model) != pick(was),
    )
}

/// Persist the whole `autoresearch:` section of `config.yaml` (the block is
/// replaced by `cfg` serialised; the rest of the file is byte-identical),
/// through a sibling temp file and a rename — `dreaming::night::persist`'s pattern.
/// The block starts at the first line beginning `autoresearch:` — a bare key,
/// an inline map (`autoresearch: {}`) or a key with a trailing comment — so a
/// second block (a duplicate key the next start cannot parse) is never appended.
pub fn persist(path: &Path, cfg: &AutoresearchConfig) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let body = serde_yaml::to_string(cfg).map_err(std::io::Error::other)?;
    let mut block: Vec<String> = vec!["autoresearch:".to_owned()];
    block.extend(body.lines().map(|l| format!("  {l}")));
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines.iter().position(|l| l.starts_with("autoresearch:")) {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.starts_with(|c: char| !c.is_whitespace()))
                .map_or(lines.len(), |i| start + 1 + i);
            lines.splice(start..end, block);
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("yaml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, lines.join("\n") + "\n")?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty section is the R1 defaults; a partial one keeps the rest.
    #[test]
    fn defaults_match_the_design() {
        let c: AutoresearchConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(c, AutoresearchConfig::default());
        assert_eq!(c.defaults.budget.usd, 20.0);
        assert_eq!(c.defaults.budget.experiments, 120);
        assert_eq!(c.gate.z_fast, 3.5);
        assert_eq!(c.gate.regime_sigma, 10.0);
        let c: AutoresearchConfig =
            serde_yaml::from_str("gate: {z: 2.5}\nreview: {model: off}").unwrap();
        assert_eq!(c.gate.z, 2.5);
        assert_eq!(c.gate.z_fast, 3.5);
        assert_eq!(c.review.model.as_deref(), Some("off"));
    }

    /// 194 rows 19/20: each role's effort goes with the model it was picked with — the study's
    /// pair first, then the section's; a Default model (or `review: off`) carries none, and a
    /// section written before 194 (with the old reserved `effort:` block) loads as Default.
    #[test]
    fn role_efforts_ride_with_their_models() {
        let old: AutoresearchConfig = serde_yaml::from_str(
            "effort: {executor: high, review: low, advisor: high}\nmodels: {executor: a/b}",
        )
        .unwrap();
        let none = crate::types::ModelsSpec::default();
        assert_eq!(old.executor_effort(&none), None, "Default until picked");
        let cfg: AutoresearchConfig = serde_yaml::from_str(
            "models: {executor: a/b, executor_effort: low, advisor_effort: high}\nreview: {model: c/d, effort: high}",
        )
        .unwrap();
        assert_eq!(cfg.executor_effort(&none).as_deref(), Some("low"));
        assert_eq!(cfg.reviewer_effort(&none).as_deref(), Some("high"));
        assert_eq!(
            cfg.advisor_effort(&none),
            None,
            "no advisor model: no effort"
        );
        assert_eq!(
            cfg.target_effort(&none).as_deref(),
            Some("low"),
            "the target is the executor"
        );
        let spec = crate::types::ModelsSpec {
            executor: Some("x/y".into()),
            target: Some("t/u".into()),
            target_effort: Some("high".into()),
            review: Some("off".into()),
            review_effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            cfg.executor_effort(&spec),
            None,
            "the study's model, the study's effort"
        );
        assert_eq!(cfg.target_effort(&spec).as_deref(), Some("high"));
        assert_eq!(cfg.reviewer_effort(&spec), None, "off");
        let back: crate::types::ModelsSpec = serde_json::from_str(r#"{"executor":"x/y"}"#).unwrap();
        assert_eq!(back.executor_effort, None, "an old draft loads");
    }

    /// 194 D4b–d at `autoresearch.settings.set`: an unknown word is refused on the same model,
    /// dropped when the model changed; Default and `off` keep none.
    #[tokio::test]
    async fn settle_role_refuses_drops_and_clears() {
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(
            settle_role(&s("a/b"), &s("high"), &s("a/b")).await,
            Ok(s("high"))
        );
        assert_eq!(
            settle_role(&s("a/b"), &s("loud"), &s("a/b")).await,
            Err("unknown effort loud".into())
        );
        assert_eq!(
            settle_role(&s("c/d"), &s("loud"), &s("a/b")).await,
            Ok(None)
        );
        assert_eq!(settle_role(&None, &s("high"), &None).await, Ok(None));
        assert_eq!(
            settle_role(&s("off"), &s("high"), &s("off")).await,
            Ok(None)
        );
    }

    /// R1-D5: executor = `agent.model`; reviewer = the first other-vendor model; `off` disables.
    #[test]
    fn model_roles_resolve_per_r1_d5() {
        let cfg = AutoresearchConfig::default();
        let agent = AgentDefaults {
            model: "openai/gpt-5.6-sol".into(),
            others: vec![
                "openai/gpt-5-mini".into(),
                "anthropic/claude-fable-5-1".into(),
            ],
            communication: false,
        };
        let spec = crate::types::ModelsSpec::default();
        assert_eq!(cfg.executor(&spec, &agent), "openai/gpt-5.6-sol");
        assert_eq!(
            cfg.reviewer(&spec, &agent).as_deref(),
            Some("anthropic/claude-fable-5-1")
        );
        let same = AgentDefaults {
            model: "openai/gpt-5.6-sol".into(),
            others: vec!["openai/gpt-5-mini".into()],
            communication: false,
        };
        assert_eq!(
            cfg.reviewer(&spec, &same).as_deref(),
            Some("openai/gpt-5.6-sol")
        );
        let off = crate::types::ModelsSpec {
            review: Some("off".into()),
            ..Default::default()
        };
        assert_eq!(cfg.reviewer(&off, &agent), None);
    }

    /// R-84: with `models.executor` overridden to another vendor, the reviewer
    /// falls back to `agent.model` (the last link of the R1-D5 chain), not to
    /// the executor itself.
    #[test]
    fn reviewer_falls_back_to_agent_model_when_executor_is_overridden() {
        let cfg = AutoresearchConfig {
            models: Models {
                executor: Some("anthropic/claude-fable-5-1".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let agent = AgentDefaults {
            model: "openai/gpt-5.6-sol".into(),
            // what `from_runtime` builds: subagents, title, then `agent.model`.
            others: vec![
                "anthropic/claude-fable-5-1".into(),
                "openai/gpt-5.6-sol".into(),
            ],
            communication: false,
        };
        let spec = crate::types::ModelsSpec::default();
        assert_eq!(cfg.executor(&spec, &agent), "anthropic/claude-fable-5-1");
        assert_eq!(
            cfg.reviewer(&spec, &agent).as_deref(),
            Some("openai/gpt-5.6-sol")
        );
    }

    /// R-116: an inline or commented `autoresearch:` line is the block; the
    /// file keeps one key and parses afterwards.
    #[test]
    fn persist_replaces_an_inline_or_commented_block() {
        let dir = tempfile::tempdir().unwrap();
        for head in ["autoresearch: {}", "autoresearch:  # mine"] {
            let path = dir.path().join("config.yaml");
            std::fs::write(
                &path,
                format!("agent:\n  model: x\n\n{head}\n\nweb:\n  a: 1\n"),
            )
            .unwrap();
            let cfg = AutoresearchConfig {
                enabled: false,
                ..Default::default()
            };
            persist(&path, &cfg).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert_eq!(text.matches("autoresearch:").count(), 1, "{head}: {text}");
            assert!(text.ends_with("web:\n  a: 1\n"), "{head}: {text}");
            let back: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            let section: AutoresearchConfig =
                serde_yaml::from_value(back["autoresearch"].clone()).unwrap();
            assert_eq!(section, cfg);
        }
    }

    /// `persist` replaces only the section and keeps the rest of the file.
    #[test]
    fn persist_replaces_only_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "agent:\n  model: x\n\nautoresearch:\n  enabled: true\n\nweb:\n  a: 1\n",
        )
        .unwrap();
        let cfg = AutoresearchConfig {
            enabled: false,
            ..Default::default()
        };
        persist(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("agent:\n  model: x\n\nautoresearch:\n  enabled: false\n"));
        assert!(text.ends_with("web:\n  a: 1\n"));
        let back: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let section: AutoresearchConfig =
            serde_yaml::from_value(back["autoresearch"].clone()).unwrap();
        assert_eq!(section, cfg);
    }
}
