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
//! - [`mesh`]: the `theuth/double/1` seam (§8.4).
//!
//! Wave C (this file's shape) is the contract; the behaviour lands in the
//! modules the `// 126 W<n>` markers name.

use runtime::Runtime;

pub mod class;
pub mod config;
pub mod events;
pub mod mesh;
pub mod store;
pub mod train_b;
pub mod types;

pub use class::{Card, CardKind, DraftKind, Requester, class_key};
pub use config::DoubleConfig;
pub use events::{
    DoubleAnswered, DoublePredicted, DoubleTrainingState, DoubleVersionActivated, Explain,
    Neighbour, Parts,
};
pub use store::Store;
pub use types::*;

/// Registry name of the double actor (`DOUBLE_ACTOR`, §4).
pub const DOUBLE_ACTOR: &str = "double";

// 126 W1: `pub mod actor;` (the `Double` kameo actor: subscriptions, predict,
// grace timers, `TakeOver`), `pub mod labels;`, `pub mod predict;`,
// `pub mod ladder;` — and the RPC stubs in crates/rpc/src/api.rs become
// `ask::<actor::Double, _>(DOUBLE_ACTOR, …)` calls.
// 126 W2: `pub mod train_a;`, `pub mod sched;`, `pub mod precond;`,
// `pub mod venue;`, `pub mod versions;`.
// 126 W3: `train_b` (above) — the Tier B trainer, `t double train-b`, `TierB::local_allowed`.
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
        let _ = (store, cfg);
        // 126 W1: spawn `actor::Double` with `runtime::subscriber_mailbox()`,
        // `register(DOUBLE_ACTOR)`, `rt.subscribe::<_, tools::ApprovalRequested>`,
        // `ApprovalResolved`, `AskRequested`, `AskResolved`, `agent::AgentEvent`.
        // 126 W2: `sched::start(store, cfg.training, rt.clone())`.
        // 126 W5: register `tool::*` once `rt.lookup_when::<ToolRegistry>` answers.
        Ok(())
    }
}
