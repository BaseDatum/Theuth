//! W3 LOOP — binding a session to a study (design doc 140 §8.2, §8.3, §12.3
//! Subscriptions): on `ScheduleEvent::Fired { name: "autoresearch-<slug>", session }`
//! record `studies.session` and send `agent::SetToolset` with the study
//! toolset; on `AgentEvent::TurnFinished` for a bound session close a
//! dangling experiment as `abandoned` and evaluate the stop rules; schedule
//! run-closed rows carry timeouts.

use crate::Studies;
use crate::error::{Error, Result};

/// The study session's `Toolset` (140 §8.3): the four experiment tools, the
/// coding tools, knowledge read tools, `todo`; `spawn_subagent` only when `parallel > 1`.
pub fn toolset(_parallel: u8) -> agent::Toolset {
    // stub: W3
    agent::Toolset {
        tools: Vec::new(),
        prompts: vec![crate::PLUGIN.to_owned()],
        runtime: false,
        hint_limit: Some(0),
    }
}

/// `ScheduleEvent::Fired` for one of ours → bind (140 §8.2).
pub async fn on_fired(_a: &mut Studies, _name: &str, _session: runtime::SessionId) -> Result<()> {
    Err(Error::NotBuilt("W3"))
}

/// `AgentEvent::TurnFinished` on a bound session → close dangling, stop rules (140 §8.4, §8.5).
pub async fn on_turn_finished(_a: &mut Studies, _session: runtime::SessionId) -> Result<()> {
    Err(Error::NotBuilt("W3"))
}

/// The slug of a schedule name `autoresearch-<slug>`, if it is one of ours.
pub fn slug_of(name: &str) -> Option<&str> {
    name.strip_prefix(crate::SCHEDULE_PREFIX).filter(|s| !s.is_empty())
}
