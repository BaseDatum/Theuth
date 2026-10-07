//! The `double_*` chat runtime functions (126 §10): the reasoning model's view of
//! the owner's double — status, classes, training, pausing, forgetting, rollback,
//! caps. They never answer a card. Registered by `DoublePlugin::start` once the
//! tool registry is up, the way `knowledge::tools::register` and
//! `decision::tool::register` are, together with the `## Double` prompt paragraph.
//!
//! Every tool calls the same seam the matching `double_*` RPC method calls
//! ([`Seams`], one method per RPC signature in `crates/rpc/src/api.rs`); W1's
//! actor implements it, so the tools never touch the store.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::types::{
    DoubleClass, DoubleClassDetail, DoubleStatus, ForgetTarget, RunId, Tier, Venue,
};

/// The `double_*` seams, one per RPC method (126 §13); the tools and the RPC
/// handlers call the same thing.
#[async_trait::async_trait]
pub trait Seams: Send + Sync + 'static {
    /// `double_status`.
    async fn status(&self) -> anyhow::Result<DoubleStatus>;
    /// `double_classes`.
    async fn classes(&self) -> anyhow::Result<Vec<DoubleClass>>;
    /// `double_class(key)`.
    async fn class(&self, key: &str) -> anyhow::Result<DoubleClassDetail>;
    /// `double_train(tier, venue)`.
    async fn train(&self, tier: Tier, venue: Venue) -> anyhow::Result<RunId>;
    /// `double_pause(on)`.
    async fn pause(&self, on: bool) -> anyhow::Result<DoubleStatus>;
    /// `double_forget(target)`; the count removed.
    async fn forget(&self, target: ForgetTarget) -> anyhow::Result<u32>;
    /// `double_rollback(version)`.
    async fn rollback(&self, version: &str) -> anyhow::Result<DoubleStatus>;
    /// `double_set_cap(class, rung, mutating_opt_in)`.
    async fn set_cap(
        &self,
        class: &str,
        rung: u8,
        mutating_opt_in: bool,
    ) -> anyhow::Result<DoubleClass>;
}

/// The seams before the actor exists (126 W1): reads answer from the config,
/// mutations fail with a message that names the wave — the RPC stubs' shape.
pub struct NotYet(pub crate::DoubleConfig);

fn not_yet(what: &str, wave: &str) -> anyhow::Error {
    anyhow::anyhow!("double: {what} is not available yet (126 {wave})")
}

#[async_trait::async_trait]
impl Seams for NotYet {
    async fn status(&self) -> anyhow::Result<DoubleStatus> {
        Ok(DoubleStatus {
            enabled: self.0.enabled,
            venue: self.0.training.venue.clone(),
            ..DoubleStatus::default()
        })
    }
    async fn classes(&self) -> anyhow::Result<Vec<DoubleClass>> {
        Ok(Vec::new())
    }
    async fn class(&self, key: &str) -> anyhow::Result<DoubleClassDetail> {
        Err(not_yet(&format!("class {key:?}"), "W1"))
    }
    async fn train(&self, tier: Tier, venue: Venue) -> anyhow::Result<RunId> {
        Err(not_yet(
            &format!("train tier {} on {}", tier.as_str(), venue.as_config()),
            "W2",
        ))
    }
    async fn pause(&self, _: bool) -> anyhow::Result<DoubleStatus> {
        Err(not_yet("pause", "W1"))
    }
    async fn forget(&self, _: ForgetTarget) -> anyhow::Result<u32> {
        Err(not_yet("forget", "W1"))
    }
    async fn rollback(&self, version: &str) -> anyhow::Result<DoubleStatus> {
        Err(not_yet(&format!("rollback to {version:?}"), "W2"))
    }
    async fn set_cap(&self, class: &str, _: u8, _: bool) -> anyhow::Result<DoubleClass> {
        Err(not_yet(&format!("set cap on {class:?}"), "W1"))
    }
}

/// Register the prompt paragraph and the eight tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, seams: Arc<dyn Seams>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "double",
            text: PROMPT.to_owned(),
        })
        .await;
    let s = &seams;
    let _ = reg.ask(RegisterTool(Arc::new(StatusTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ClassesTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ClassTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(TrainTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(PauseTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ForgetTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(RollbackTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(CapTool(seams)))).await;
}

/// Text for the transcript, the same data as a Python value for the runtime.
fn structured<T: serde::Serialize>(text: String, data: &T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// A seam failure as content the model reads, never an `Err`.
fn out<T: serde::Serialize>(r: anyhow::Result<T>, text: impl FnOnce(&T) -> String) -> ToolOutput {
    match r {
        Ok(v) => structured(text(&v), &v),
        Err(e) => ToolOutput::err(format!("{e:#}")),
    }
}

fn pct(x: f32) -> String {
    format!("{:.0} %", x * 100.0)
}

fn status_line(s: &DoubleStatus) -> String {
    let mut t = format!(
        "double: {}{}; version {}; venue {}; last 30 d: n={} acc {} (baseline {}) ece {:.3} asks avoided {} minutes saved {:.0}",
        if s.enabled { "enabled" } else { "disabled" },
        if s.paused { ", learning paused" } else { "" },
        s.active_version.as_deref().unwrap_or("baseline"),
        s.venue,
        s.kpis.n,
        pct(s.kpis.accuracy),
        pct(s.baseline.accuracy),
        s.kpis.ece,
        s.kpis.asks_avoided,
        s.kpis.minutes_saved,
    );
    if let Some(r) = &s.last_run {
        t.push_str(&format!(
            "; last run {} tier {} {} {}",
            r.id.0,
            r.tier.as_str(),
            r.state.as_str(),
            r.result.as_deref().unwrap_or("")
        ));
    }
    if let Some(w) = &s.next_window {
        t.push_str(&format!("; next window {w}"));
    }
    t
}

fn class_line(c: &DoubleClass) -> String {
    format!(
        "{}  L{} (cap {}{})  n={}  acc {}  ece {:.3}{}",
        c.key,
        c.rung,
        c.cap,
        c.cap_reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default(),
        c.n,
        pct(c.kpis.accuracy),
        c.kpis.ece,
        if c.mutating_opt_in { "  opt-in" } else { "" },
    )
}

/// No arguments.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// `double_status`.
pub struct StatusTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for StatusTool {
    const NAME: &'static str = "double_status";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = NoArgs;

    fn description(&self) -> String {
        "The owner's double: enabled/paused, active version, last training run, next window, venue, \
         and the last-30-days numbers (accuracy, ECE, asks avoided, minutes saved) vs the frozen baseline. \
         Read-only; it never answers a card."
            .into()
    }

    async fn call(&self, _: NoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(out(self.0.status().await, status_line))
    }
}

/// `double_classes`.
pub struct ClassesTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for ClassesTool {
    const NAME: &'static str = "double_classes";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    const CONSUMES_SECRETS: bool = false;
    type Args = NoArgs;

    fn description(&self) -> String {
        "Every decision class the double has seen (key `kind:shape:requester`, e.g. \
         `approval:shell:mutating:git push:root`) with its rung L0–L3, cap and reason, example count and \
         last-30-days accuracy/ECE. Use the key with double_class, double_forget and double_cap."
            .into()
    }

    async fn call(&self, _: NoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(out(self.0.classes().await, |cs| {
            if cs.is_empty() {
                "double: no classes yet".to_owned()
            } else {
                cs.iter().map(class_line).collect::<Vec<_>>().join("\n")
            }
        }))
    }
}

/// One class by key.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClassArgs {
    /// The class key as listed by `double_classes`.
    pub key: String,
}

/// `double_class`.
pub struct ClassTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for ClassTool {
    const NAME: &'static str = "double_class";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = ClassArgs;

    fn description(&self) -> String {
        "One decision class in detail: the row, the weekly learning curve (current vs baseline), the last 50 \
         scrubbed examples (what the owner chose, what the double predicted) and what would promote it."
            .into()
    }

    async fn call(&self, a: ClassArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(out(self.0.class(&a.key).await, |d| {
            format!(
                "{}\n{}\n{} curve points, {} examples",
                class_line(&d.class),
                d.promote_hint,
                d.curve.len(),
                d.examples.len()
            )
        }))
    }
}

/// `double_train` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TrainArgs {
    /// `a` (temperatures, k-NN, heads; seconds, in-process) or `b` (LoRA adapter; a gated run).
    /// `c` is a full fine-tune the owner starts from Settings, not from chat.
    pub tier: Tier,
    /// `auto` | `local` | `node:<name>`; default the configured venue.
    #[serde(default)]
    pub venue: Option<String>,
}

/// `double_train`.
pub struct TrainTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for TrainTool {
    const NAME: &'static str = "double_train";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "str";
    const CONSUMES_SECRETS: bool = false;
    type Args = TrainArgs;

    fn description(&self) -> String {
        "Start a training run of the double now instead of waiting for the nightly window: tier `a` \
         (fast, deterministic) or `b` (LoRA; needs idle + power + a GPU venue). Returns the run id; a new \
         version activates only if it beats the active one on the gate. Only when the owner asks."
            .into()
    }

    async fn call(&self, a: TrainArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if a.tier == Tier::C || a.tier == Tier::Baseline {
            return Ok(ToolOutput::err(format!(
                "double: tier {} is not started from chat (a or b)",
                a.tier.as_str()
            )));
        }
        let venue = match a.venue.as_deref() {
            None => Venue::Auto,
            Some(v) => match Venue::parse(v) {
                Some(Venue::Off) | None => {
                    return Ok(ToolOutput::err(format!(
                        "double: venue {v:?} is not auto, local or node:<name>"
                    )));
                }
                Some(v) => v,
            },
        };
        Ok(out(self.0.train(a.tier, venue).await, |id| {
            format!("double: training run {} started", id.0)
        }))
    }
}

/// `double_pause` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PauseArgs {
    /// `true` pauses learning (predictions and rungs continue, labels stop); `false` resumes.
    pub on: bool,
}

/// `double_pause`.
pub struct PauseTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for PauseTool {
    const NAME: &'static str = "double_pause";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = PauseArgs;

    fn description(&self) -> String {
        "Pause or resume the double's learning. Paused: it still predicts and keeps its rungs, but no new \
         labels accrue (the curve shows the gap). Only when the owner asks."
            .into()
    }

    async fn call(&self, a: PauseArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(out(self.0.pause(a.on).await, status_line))
    }
}

/// `double_forget` arguments: a class key or example ids, not both.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForgetArgs {
    /// Forget every example of this class; the class drops back to L0.
    #[serde(default)]
    pub class: Option<String>,
    /// Forget these `examples.id`s (from `double_class`).
    #[serde(default)]
    pub example_ids: Vec<String>,
}

/// `double_forget`.
pub struct ForgetTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for ForgetTool {
    const NAME: &'static str = "double_forget";
    const RISK: Risk = Risk::Destructive;
    const RETURNS: &'static str = "int";
    const CONSUMES_SECRETS: bool = false;
    type Args = ForgetArgs;

    fn description(&self) -> String {
        "Make the double forget: pass `class` (every example, label and prediction of that class; it goes \
         back to L0) or `example_ids` (specific examples from double_class). Returns how many were removed. \
         Irreversible; only when the owner asks."
            .into()
    }

    async fn call(&self, a: ForgetArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let target = match (a.class, a.example_ids.is_empty()) {
            (Some(key), true) => ForgetTarget::Class { key },
            (None, false) => ForgetTarget::Examples { ids: a.example_ids },
            _ => {
                return Ok(ToolOutput::err(
                    "double_forget: pass exactly one of `class` or `example_ids`",
                ));
            }
        };
        Ok(out(self.0.forget(target).await, |n| {
            format!("double: forgot {n} example(s)")
        }))
    }
}

/// `double_rollback` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RollbackArgs {
    /// The version id to make active (`baseline` or a `versions.id` from double_status).
    pub version: String,
}

/// `double_rollback`.
pub struct RollbackTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for RollbackTool {
    const NAME: &'static str = "double_rollback";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = RollbackArgs;

    fn description(&self) -> String {
        "Make an earlier version of the double active (`baseline` is always available). The next prediction \
         uses it; nothing is deleted, so rolling forward is the same call. Only when the owner asks."
            .into()
    }

    async fn call(&self, a: RollbackArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(out(self.0.rollback(&a.version).await, status_line))
    }
}

/// `double_cap` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CapArgs {
    /// The class key.
    pub class: String,
    /// Highest rung the class may reach, 0..=3. Destructive classes are clamped to 1 whatever is passed.
    pub rung: u8,
    /// Per-class opt-in letting a mutating approval class act at L2 (default off).
    #[serde(default)]
    pub mutating_opt_in: bool,
}

/// `double_cap`.
pub struct CapTool(pub Arc<dyn Seams>);

#[async_trait::async_trait]
impl Tool for CapTool {
    const NAME: &'static str = "double_cap";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = CapArgs;

    fn description(&self) -> String {
        "Set the highest rung a class may climb to (0 observe, 1 suggest, 2 act after a 30 s grace, 3 silent) \
         and, for mutating approval classes, the opt-in that allows L2. Lower caps take effect at once; \
         destructive classes never exceed L1. Only when the owner asks."
            .into()
    }

    async fn call(&self, a: CapArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if a.rung > 3 {
            return Ok(ToolOutput::err("double_cap: rung is 0..=3"));
        }
        Ok(out(
            self.0.set_cap(&a.class, a.rung, a.mutating_opt_in).await,
            class_line,
        ))
    }
}

/// The system-prompt paragraph (`RegisterPrompt { plugin: "double" }`, 126 §11):
/// present only while `double.enabled`, the 92 rule.
pub const PROMPT: &str = "## Double\n\
The owner has a double: a local model of their own past decisions that predicts their answer to every approval and ask card before they see it. \
A chip `double · Allow · 91 %` on a card is its prediction; `none` means it is unsure and the owner decides. \
At L2 it answers for the owner after a 30 s grace; an answer marked `double` is the owner's answer — act on it and never argue with it or ask again. \
When you disagree with an L1 suggestion, say so in one line and let the owner decide. \
`double_status()`, `double_classes()`, `double_class(key)` are its numbers; `double_pause`, `double_train`, `double_forget`, `double_rollback`, `double_cap` change it and are only for when the owner asks. \
Load the `double` skill for the ladder and the controls.";

/// The bundled `double` skill (126 §11), written to `<data_dir>/skills/double.md` by `first_run`.
pub const DOUBLE_SKILL: &str = include_str!("../double.md");

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    #[async_trait::async_trait]
    impl Seams for Fake {
        async fn status(&self) -> anyhow::Result<DoubleStatus> {
            Ok(DoubleStatus {
                enabled: true,
                paused: true,
                venue: "local".into(),
                ..DoubleStatus::default()
            })
        }
        async fn classes(&self) -> anyhow::Result<Vec<DoubleClass>> {
            Ok(Vec::new())
        }
        async fn class(&self, key: &str) -> anyhow::Result<DoubleClassDetail> {
            anyhow::bail!("no class {key}")
        }
        async fn train(&self, tier: Tier, venue: Venue) -> anyhow::Result<RunId> {
            Ok(RunId(format!("{}@{}", tier.as_str(), venue.as_config())))
        }
        async fn pause(&self, _: bool) -> anyhow::Result<DoubleStatus> {
            self.status().await
        }
        async fn forget(&self, target: ForgetTarget) -> anyhow::Result<u32> {
            Ok(match target {
                ForgetTarget::Class { .. } => 7,
                ForgetTarget::Examples { ids } => ids.len() as u32,
            })
        }
        async fn rollback(&self, _: &str) -> anyhow::Result<DoubleStatus> {
            self.status().await
        }
        async fn set_cap(&self, class: &str, rung: u8, opt: bool) -> anyhow::Result<DoubleClass> {
            Ok(DoubleClass {
                key: class.into(),
                kind: crate::CardKind::Approval,
                n: 1,
                rung: 0,
                cap: rung,
                cap_reason: None,
                mutating_opt_in: opt,
                median_answer_ms: 0,
                kpis: Default::default(),
                baseline: Default::default(),
                last_decision_at: None,
            })
        }
    }

    fn seams() -> Arc<dyn Seams> {
        Arc::new(Fake)
    }

    fn cx() -> ToolCx {
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: Arc::new(tools::SessionResources {
                root: std::env::temp_dir(),
                retention: tools::Retention::Durable,
                generation: 1,
            }),
        }
    }

    #[test]
    fn names_risks_and_schemas() {
        let s = seams();
        let spec = |t: &dyn tools::ErasedTool| t.spec();
        let specs = [
            spec(&StatusTool(s.clone())),
            spec(&ClassesTool(s.clone())),
            spec(&ClassTool(s.clone())),
            spec(&TrainTool(s.clone())),
            spec(&PauseTool(s.clone())),
            spec(&ForgetTool(s.clone())),
            spec(&RollbackTool(s.clone())),
            spec(&CapTool(s)),
        ];
        let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "double_status",
                "double_classes",
                "double_class",
                "double_train",
                "double_pause",
                "double_forget",
                "double_rollback",
                "double_cap"
            ]
        );
        let risks: Vec<_> = specs.iter().map(|s| s.risk).collect();
        assert_eq!(
            risks,
            [
                Risk::ReadOnly,
                Risk::ReadOnly,
                Risk::ReadOnly,
                Risk::Mutating,
                Risk::Mutating,
                Risk::Destructive,
                Risk::Mutating,
                Risk::Mutating
            ]
        );
        for s in &specs {
            assert_eq!(s.schema["type"], "object", "{}", s.name);
            assert!(!s.description.is_empty());
        }
        let req = |i: usize| specs[i].schema["required"].as_array().cloned().unwrap_or_default();
        assert_eq!(req(2), vec!["key"]);
        assert_eq!(req(3), vec!["tier"]);
        assert_eq!(req(7), vec!["class", "rung"]);
        assert!(specs[5].schema["properties"]["class"].is_object());
        assert!(specs[5].schema["properties"]["example_ids"].is_object());
    }

    #[tokio::test]
    async fn calls_route_and_validate() {
        let s = seams();
        let o = StatusTool(s.clone()).call(NoArgs {}, cx()).await.unwrap();
        assert!(o.content.starts_with("double: enabled, learning paused; version baseline; venue local"));
        assert_eq!(o.structured.as_ref().unwrap()["paused"], true);

        let o = ClassTool(s.clone())
            .call(ClassArgs { key: "x".into() }, cx())
            .await
            .unwrap();
        assert!(o.is_error && o.content.contains("no class x"));

        let t = TrainTool(s.clone());
        let o = t
            .call(TrainArgs { tier: Tier::A, venue: Some("node:gpu".into()) }, cx())
            .await
            .unwrap();
        assert_eq!(o.content, "double: training run a@node:gpu started");
        for (tier, venue) in [(Tier::C, None), (Tier::A, Some("off".into())), (Tier::A, Some("mars".into()))] {
            assert!(t.call(TrainArgs { tier, venue }, cx()).await.unwrap().is_error);
        }

        let f = ForgetTool(s.clone());
        let both = ForgetArgs { class: Some("k".into()), example_ids: vec!["1".into()] };
        assert!(f.call(both, cx()).await.unwrap().is_error);
        let none = ForgetArgs { class: None, example_ids: vec![] };
        assert!(f.call(none, cx()).await.unwrap().is_error);
        let o = f
            .call(ForgetArgs { class: Some("k".into()), example_ids: vec![] }, cx())
            .await
            .unwrap();
        assert_eq!(o.content, "double: forgot 7 example(s)");
        assert_eq!(o.structured.unwrap(), 7);

        let c = CapTool(s);
        let bad = CapArgs { class: "k".into(), rung: 4, mutating_opt_in: false };
        assert!(c.call(bad, cx()).await.unwrap().is_error);
        let o = c
            .call(CapArgs { class: "k".into(), rung: 2, mutating_opt_in: true }, cx())
            .await
            .unwrap();
        assert!(o.content.starts_with("k  L0 (cap 2)"), "{}", o.content);
        assert!(o.content.ends_with("opt-in"));
    }

    #[tokio::test]
    async fn not_yet_answers_reads_and_refuses_writes() {
        let n = NotYet(crate::DoubleConfig::default());
        assert!(!n.status().await.unwrap().enabled);
        assert!(n.classes().await.unwrap().is_empty());
        let e = n.pause(true).await.unwrap_err().to_string();
        assert_eq!(e, "double: pause is not available yet (126 W1)");
    }

    #[test]
    fn prompt_and_skill_shape() {
        assert!(PROMPT.starts_with("## Double\n"));
        assert!(PROMPT.lines().count() <= 8, "{}", PROMPT.lines().count());
        assert!(DOUBLE_SKILL.starts_with("---\nname: double\ndescription: "));
        let body = DOUBLE_SKILL
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .expect("closed front matter")
            .1;
        assert!(body.lines().count() <= 40, "{}", body.lines().count());
        for must in ["never argue", "owner's answer", "`none`", "30 s"] {
            assert!(DOUBLE_SKILL.contains(must), "skill lacks {must:?}");
        }
    }
}
