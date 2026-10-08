//! 137 R1-D6: the production [`Memory`] — the meeting's bridge to
//! `agent::endless::Endless` over the two `Agents` messages W2 adds
//! (consistent with `SetCompaction` / `SetToolset`):
//!
//! - `agent::SetSessionMemory { session, mode: MemoryMode, filter: UnitFilter }`
//!   → `anyhow::Result<agent::SetMode>` (the agent passes `epoch` itself: a meeting
//!   session has no epoch head to write)
//! - `agent::RenderMemoryView { session, bytes: u32 }` → `anyhow::Result<Vec<agent::ViewLine>>`
//!   (what `Endless::render_with_budget` returns; `built` tells verbatim from summarized)
//!
//! Enabled by the closer once W2's commit is on the branch: `mod endless_agent;`
//! in `lib.rs` and `endless_agent::install()` in `MeetingPlugin::start`
//! (137-as-built-W4.md §4 item 1).

use std::sync::Arc;

use agent::{MemoryMode, ViewLine};
use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::endless::Memory;

/// The bridge: every call is one `ask` on the agent actor.
pub struct AgentsMemory;

fn agents() -> Result<ActorRef<agent::Agents>, String> {
    match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(a)) => Ok(a),
        _ => Err("agent is not running".into()),
    }
}

#[async_trait::async_trait]
impl Memory for AgentsMemory {
    async fn enable_meeting(&self, session: SessionId) -> Result<(), String> {
        agents()?
            .ask(agent::SetSessionMemory {
                session,
                mode: MemoryMode::Endless,
                filter: agent::UnitFilter::Meeting,
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn render(&self, session: SessionId, bytes: u32) -> Result<Vec<ViewLine>, String> {
        agents()?
            .ask(agent::RenderMemoryView { session, bytes })
            .await
            .map_err(|e| e.to_string())
    }
}

/// Install the bridge as the meeting's memory port (idempotent).
pub fn install() {
    crate::endless::install(Arc::new(AgentsMemory));
}
