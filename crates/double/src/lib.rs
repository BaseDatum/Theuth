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
//! - [`versions`]: `models/<version>/`, the `active` pointer, rollback (§8.5).
//!
//! Wave C (this file's shape) is the contract; the behaviour lands in the
//! modules the `// 126 W<n>` markers name.

use runtime::Runtime;

pub mod actor;
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
pub mod train_a;
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
// 126 W3: `pub mod train_b;`.
// 126 W5: `pub mod tool;` (the `double_*` chat runtime functions), the
// `double` skill and the `## Double` prompt paragraph.

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
        if let Err(e) = actor::start(rt, store.clone(), cfg.clone(), oracle).await {
            tracing::warn!("double actor not started: {e:#}");
        }
        // 126 W2: the scheduler (Tier A locally; W3 wraps `tier_a_factory` to add
        // Tier B) and the `theuth/double/1` node side (refusing until W3).
        let factory = sched::tier_a_factory(
            store.clone(),
            versions::models_dir(&rt.data_dir),
            // ponytail: the checkpoint *name* stands in for its sha256 until W3
            // (the decision crate owns the checkpoint path and its `.sha256` sidecar).
            cfg.model.checkpoint.clone(),
        );
        if let Err(e) = sched::start(rt.clone(), store.clone(), cfg.clone(), factory) {
            tracing::warn!("double scheduler not started: {e:#}");
        }
        tokio::spawn(mesh::serve(rt.clone(), std::sync::Arc::new(mesh::NotYet)));
        let _ = (store, cfg);
        // 126 W5: register `tool::*` once `rt.lookup_when::<ToolRegistry>` answers.
        Ok(())
    }
}
