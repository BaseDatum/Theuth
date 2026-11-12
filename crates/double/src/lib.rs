//! Double (design doc 126): a local model of the owner's own decisions that
//! measurably improves with use. A plugin crate (00 §1 item 1 — named by its
//! function: the owner's double) that is a *client* of the runtime bus, not a
//! hook: it subscribes to `ApprovalRequested` / `AskRequested` the way the
//! RPC bridge does, predicts, writes the prequential row, and at L2/L3
//! answers through the same actor messages a UI sends (`tools::ResolveBy`,
//! `agent::answer_ask_by`). Nothing inside the agent loop knows it exists.
//!
//! - [`class`]: the deterministic class key (§3).
//! - [`store`]: `<data_dir>/double/double.sqlite` (§5).
//! - [`config`]: the `double:` section (§12).
//! - [`events`]: the four bus events (§13).
//! - [`types`]: the `double_*` RPC wire types (§13).
//! - [`mesh`]: the `theuth/double/1` bundle protocol (§8.4).
//! - [`train_a`]: Tier A — temperatures, k-NN index, logistic heads (§7.2).
//! - [`sched`] + [`precond`]: the training cycle and its preconditions (§8.1–8.3).
//! - [`venue`]: `auto | local | node:<name> | off` (§8.4).
//! - [`tier_b`]: Tier B glued to the scheduler and the mesh (local / node trainers, the node runner).
//! - [`versions`]: `models/<version>/`, the `active` pointer, rollback (§8.5).
//!
//! Wave C (this file's shape) is the contract; the behaviour lands in the
//! modules the `// 126 W<n>` markers name.

use runtime::Runtime;

pub mod actor;
pub mod chip;
pub mod class;
pub mod config;
pub mod events;
pub mod labels;
pub mod ladder;
pub mod mesh;
pub mod precond;
pub mod predict;
pub mod sched;
pub mod store;
pub mod tier_b;
pub mod tool;
pub mod train_a;
pub mod train_b;
pub mod types;
pub mod venue;
pub mod versions;

pub use class::{Card, CardKind, DraftKind, Requester, class_key};
pub use config::DoubleConfig;
pub use events::{
    DoubleAnswered, DoublePredicted, DoubleTrainingState, DoubleVersionActivated, Explain,
    Neighbour, Parts,
};
pub use store::Store;
pub use types::*;
pub use versions::ActiveVersion;

/// Registry name of the double actor (`DOUBLE_ACTOR`, §4).
pub const DOUBLE_ACTOR: &str = "double";

// 126 W1 (built): `actor`, `labels`, `predict`, `ladder`; the RPC methods in
// crates/rpc/src/api.rs are `ask::<actor::Double, _>(DOUBLE_ACTOR, …)` calls.
// 126 W2 (built): `train_a`, `sched`, `precond`, `venue`, `versions`.
// 126 W3 (built): `train_b` — the Tier B trainer, `t double train-b`, `TierB::local_allowed`.
// 126 W5 (built): `tool` (the `double_*` chat runtime functions, the `## Double`
// prompt paragraph and the `double` skill) and `chip` (the surfaces' text).

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<DoublePredicted>().await?;
    rt.register_wire_event::<DoubleAnswered>().await?;
    rt.register_wire_event::<DoubleTrainingState>().await?;
    rt.register_wire_event::<DoubleVersionActivated>().await
}

/// The plugin. Reads `double:`, and when enabled opens the store under
/// `<data_dir>/double/`; the actor, scheduler and tools are added at the
/// markers above.
pub struct DoublePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for DoublePlugin {
    fn name(&self) -> &'static str {
        "double"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: DoubleConfig = rt.section("double").unwrap_or_else(|e| {
            tracing::debug!("no [double] config section, using defaults: {e}");
            DoubleConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("double disabled by config");
            return Ok(());
        }
        let store = match Store::open(&store::path(&rt.data_dir)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("double store unavailable: {e:#}");
                return Ok(());
            }
        };
        let oracle = std::sync::Arc::new(predict::EngineOracle);
        let handle = match actor::start(rt, store.clone(), cfg.clone(), oracle).await {
            Ok(a) => Some(actor::Handle(a)),
            Err(e) => {
                tracing::warn!("double actor not started: {e:#}");
                None
            }
        };
        // 126 W2 + W3: the scheduler (Tier A locally, Tier B locally or on a mesh
        // node) and the `theuth/double/1` node side.
        let decision_cfg: decision::DecisionConfig = rt.section("decision").unwrap_or_default();
        let ckpt = tier_b::checkpoint_dir(&decision_cfg, &rt.data_dir);
        let base_hash = tier_b::base_hash(&ckpt).unwrap_or_else(|e| {
            tracing::warn!(
                "double: checkpoint hash unavailable ({e:#}); using the checkpoint name"
            );
            cfg.model.checkpoint.clone()
        });
        let factory = tier_b::factory(
            rt.clone(),
            store.clone(),
            versions::models_dir(&rt.data_dir),
            cfg.clone(),
            rt.data_dir.clone(),
            decision_cfg.clone(),
            base_hash,
        );
        if let Err(e) = sched::start(rt.clone(), store.clone(), cfg.clone(), factory) {
            tracing::warn!("double scheduler not started: {e:#}");
        }
        tokio::spawn(mesh::serve(
            rt.clone(),
            std::sync::Arc::new(tier_b::NodeRunner::new(
                rt.data_dir.clone(),
                cfg.clone(),
                decision_cfg,
            )),
        ));
        // The chat tools call the actor through `tool::Seams`; the roster inbox pulls
        // the "answers in 14 s — take over" rows through `attention::Source`.
        let seams: std::sync::Arc<dyn tool::Seams> = match &handle {
            Some(h) => std::sync::Arc::new(h.clone()),
            None => std::sync::Arc::new(tool::NotYet(cfg)),
        };
        let rt_tools = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt_tools
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            {
                tool::register(&reg, seams).await;
            }
        });
        if let Some(h) = handle {
            let rt = rt.clone();
            tokio::spawn(async move {
                if let Some(inbox) = rt
                    .lookup_when::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
                    .await
                {
                    let _ = inbox
                        .tell(roster::attention::RegisterSource(std::sync::Arc::new(h)))
                        .await;
                }
            });
        }
        Ok(())
    }
}
