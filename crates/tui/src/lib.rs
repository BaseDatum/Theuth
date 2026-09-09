//! Terminal UI plugin. Subscribes to `agent::AgentEvent` and
//! `tools::ApprovalRequested`; sends `SendInput` / `Steer` / `Cancel` /
//! `Resolve` by `ask`. Cancels `rt.shutdown` on quit. See design doc §8.

use runtime::SessionId;

/// The plugin.
pub struct TuiPlugin {
    /// Session to open on start (`wth --resume <id>`); `None` starts fresh.
    pub resume: Option<SessionId>,
}

#[async_trait::async_trait]
impl runtime::Plugin for TuiPlugin {
    fn name(&self) -> &'static str {
        "tui"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let _ = rt;
        todo!("tui implementation")
    }
}
