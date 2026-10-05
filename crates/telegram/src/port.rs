//! The asks the channel makes of other plugins, behind a trait so routing,
//! rendering and commands are testable without live actors (design doc 98
//! §13; `crates/slack/src/channel.rs` `trait Port` plus `ancestors` and
//! `session_title`).

use std::path::PathBuf;

use agent::{
    AGENT_ACTOR, AgentInfo, Agents, Ancestors, Cancel, ListAgents, ListSessions, LoadSession,
    Loaded, NewRootSession, SendInput, SessionSummary, Steer, communication::PeerScope,
};
use async_trait::async_trait;
use kameo::actor::ActorRef;
use runtime::{InputId, SessionId};
use tools::{APPROVAL_ACTOR, Answer, Approval, ApprovalId, AskId, Decision, Resolve};

/// What the channel asks of the agent and approval actors.
#[async_trait]
pub trait Port: Send + Sync {
    /// `agent::SendInput`; `session: None` creates a root with `scope` first
    /// (`NewRootSession`, 50 §2.3) so the thread's root carries the owner's
    /// scope from birth. Returns the session the input went to.
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        scope: PeerScope,
    ) -> Result<SessionId, String>;
    /// `agent::NewRootSession`: an empty root with an explicit scope.
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String>;
    /// `agent::Steer` with a fresh `InputId`: lands after the current tool batch.
    async fn steer(&self, session: SessionId, text: String, files: Vec<PathBuf>) -> Result<(), String>;
    /// `agent::Cancel`.
    async fn cancel(&self, session: SessionId) -> Result<(), String>;
    /// `agent::ListSessions`.
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String>;
    /// `agent::LoadSession` (everything, settled or not).
    async fn load_session(&self, session: SessionId) -> Result<Loaded, String>;
    /// `tools::Resolve`: `true` when the id was pending.
    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String>;
    /// `agent::answer_ask`: `true` when the id was pending; `None` dismisses (cancels the turn).
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String>;
    /// `agent::ListAgents`: the subtree under `session`.
    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String>;
    /// `session`, then its parent, …, the root (24 §2); `[session]` when unknown.
    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String>;
    /// The session's title from `ListSessions`; `None` when unlisted.
    async fn session_title(&self, session: SessionId) -> Result<Option<String>, String>;
}

/// [`Port`] over the registered `agent` and `approval` actors.
pub struct LivePort;

fn agents() -> Result<ActorRef<Agents>, String> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "agent actor is not running".to_string())
}

#[async_trait]
impl Port for LivePort {
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        scope: PeerScope,
    ) -> Result<SessionId, String> {
        let session = match session {
            Some(s) => s,
            None => self.new_root(scope).await?,
        };
        agents()?
            .ask(SendInput {
                session: Some(session),
                text,
                files,
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
        agents()?
            .ask(NewRootSession { cwd: None, scope })
            .await
            .map_err(|e| e.to_string())
    }

    async fn steer(&self, session: SessionId, text: String, files: Vec<PathBuf>) -> Result<(), String> {
        agents()?
            .ask(Steer {
                session,
                id: InputId::new(),
                text,
                files,
                delivery: agent::Delivery::default(),
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn cancel(&self, session: SessionId) -> Result<(), String> {
        agents()?
            .ask(Cancel { session, keep: None })
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        agents()?.ask(ListSessions).await.map_err(|e| e.to_string())
    }

    async fn load_session(&self, session: SessionId) -> Result<Loaded, String> {
        agents()?
            .ask(LoadSession {
                session,
                before: None,
                limit: None,
                settled_only: false,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String> {
        ActorRef::<Approval>::lookup(APPROVAL_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "approval actor is not running".to_string())?
            .ask(Resolve { id, decision })
            .await
            .map_err(|e| e.to_string())
    }

    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String> {
        agent::answer_ask(id, answers)
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String> {
        agents()?
            .ask(ListAgents { session })
            .await
            .map_err(|e| e.to_string())
    }

    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String> {
        agents()?
            .ask(Ancestors { session })
            .await
            .map(|chain| chain.into_iter().map(|a| a.id).collect())
            .map_err(|e| e.to_string())
    }

    async fn session_title(&self, session: SessionId) -> Result<Option<String>, String> {
        Ok(self
            .list_sessions()
            .await?
            .into_iter()
            .find(|s| s.id == session)
            .map(|s| s.title))
    }
}
