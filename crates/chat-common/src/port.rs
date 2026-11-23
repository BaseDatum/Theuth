//! The asks the chat channels make of the `agent` and `approval` actors; the
//! `Err` side is the display text of the failure.

use std::path::PathBuf;

use agent::{AGENT_ACTOR, Agents, ChannelOrigin, NewRootSession, PeerScope, SendInput, Steer};
use kameo::actor::ActorRef;
use runtime::{InputId, SessionId};
use tools::{APPROVAL_ACTOR, Answer, Approval, ApprovalId, AskId, Decision, Resolve};

/// The registered `agent` actor.
pub fn agents() -> Result<ActorRef<Agents>, String> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "agent actor is not running".to_string())
}

/// `agent::NewRootSession`: an empty root with an explicit scope.
pub async fn new_root(scope: PeerScope) -> Result<SessionId, String> {
    agents()?
        .ask(NewRootSession { cwd: None, scope })
        .await
        .map_err(|e| e.to_string())
}

/// `agent::SendInput` with the default delivery; `session: None` starts a root.
pub async fn send_input(
    session: Option<SessionId>,
    text: String,
    files: Vec<PathBuf>,
    channel: ChannelOrigin,
) -> Result<SessionId, String> {
    agents()?
        .ask(SendInput {
            session,
            text,
            files,
            delivery: agent::Delivery::default(),
            id: None,
            channel: Some(channel),
        })
        .await
        .map_err(|e| e.to_string())
}

/// `agent::Steer` with the default delivery.
pub async fn steer(
    session: SessionId,
    id: InputId,
    text: String,
    files: Vec<PathBuf>,
    channel: ChannelOrigin,
) -> Result<(), String> {
    agents()?
        .ask(Steer {
            session,
            id,
            text,
            files,
            delivery: agent::Delivery::default(),
            channel: Some(channel),
        })
        .await
        .map_err(|e| e.to_string())
}

/// `tools::Resolve` on the `approval` actor: `true` when the id was pending.
pub async fn resolve(id: ApprovalId, decision: Decision) -> Result<bool, String> {
    ActorRef::<Approval>::lookup(APPROVAL_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "approval actor is not running".to_string())?
        .ask(Resolve { id, decision })
        .await
        .map_err(|e| e.to_string())
}

/// `agent::answer_ask`: `true` when the id was pending; `None` dismisses.
pub async fn answer_ask(id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String> {
    agent::answer_ask(id, answers)
        .await
        .map_err(|e| e.to_string())
}
