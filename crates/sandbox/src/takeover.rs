//! Take over / hand back the desktop (79 §6.5): steer every session with a
//! running turn on the VM's engine with one input, so the agent stops (or
//! resumes) `computer_*`. The steer is `agent::Steer` on this engine's
//! supervisor — it reaches sessions this engine owns (`SessionSummary.here`);
//! sessions owned by another engine are only announced through
//! `SandboxEvent::Takeover` (the RPC layer's `forward::route` is the path a
//! remote steer would take; see the as-built).

use kameo::actor::ActorRef;
use runtime::{InputId, Runtime};

use crate::types::SandboxEvent;

/// The input a takeover injects.
pub const TAKEOVER: &str = "<desktop_takeover by=\"user\"/>";
/// The input a hand-back injects.
pub const HANDBACK: &str = "<desktop_handback/>";

/// Sessions with a running turn whose owner is `engine` (or, with
/// `engine == ""`, every local running session — the guest's own view).
pub async fn running_sessions(engine: &str) -> Vec<(runtime::SessionId, bool)> {
    let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
        return Vec::new();
    };
    let Ok(rows) = agents.ask(agent::ListSessions).await else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|s| s.running && (engine.is_empty() || s.owner == engine))
        .map(|s| (s.id, s.here))
        .collect()
}

/// Steer every running session of `engine` and publish the event. Returns how
/// many sessions were steered (local ones; remote ones count as announced only).
pub async fn steer(rt: &Runtime, name: &str, engine: &str, on: bool) -> u32 {
    let text = if on { TAKEOVER } else { HANDBACK };
    let mut n = 0u32;
    if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        for (session, here) in running_sessions(engine).await {
            if !here {
                tracing::warn!(
                    "sandbox takeover: session {session} is owned elsewhere; not steered"
                );
                continue;
            }
            let r = agents
                .ask(agent::Steer {
                    session,
                    id: InputId::new(),
                    text: text.to_owned(),
                    files: Vec::new(),
                    delivery: agent::Delivery::Steer,
                    channel: None,
                })
                .await;
            match r {
                Ok(()) => n += 1,
                Err(e) => tracing::warn!("sandbox takeover: steer {session}: {e}"),
            }
        }
    }
    rt.publish(SandboxEvent::Takeover {
        name: name.to_owned(),
        engine: engine.to_owned(),
        on,
    })
    .await;
    n
}
