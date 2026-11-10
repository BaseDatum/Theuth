//! The asks the channel makes of other plugins, behind a trait so routing,
//! rendering and commands are testable without live actors (design doc 98
//! §13; `crates/slack/src/channel.rs` `trait Port` plus `ancestors` and
//! `session_title`).

use std::fmt::Display;
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;

use agent::{
    AGENT_ACTOR, AgentInfo, Agents, Ancestors, Cancel, ChannelOrigin, ListAgents, ListSessions,
    LoadSession, Loaded, NewRootSession, SendInput, SessionSummary, Steer,
    communication::PeerScope,
};
use async_trait::async_trait;
use kameo::actor::ActorRef;
use runtime::{InputId, SessionId};
use tokio::time::Instant;
use tools::{APPROVAL_ACTOR, Answer, Approval, ApprovalId, AskId, Decision, Resolve};

/// Longest wait on another actor (98c). Past it the ask answers
/// [`ask_timed_out`] instead of pinning the dispatch task that made it.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(20);
/// An ask still unanswered after this long is logged at WARN while it waits.
pub const ASK_SLOW: Duration = Duration::from_secs(5);

/// The `Err` text of an ask that hit [`ASK_TIMEOUT`].
#[must_use]
pub fn ask_timed_out() -> String {
    format!("engine did not answer in {} s", ASK_TIMEOUT.as_secs())
}

/// `ask` under [`ASK_TIMEOUT`]; a WARN at [`ASK_SLOW`] while still waiting
/// and another on timeout. `what` names the ask in the log.
pub async fn timed<T, E: Display>(
    what: &'static str,
    ask: impl IntoFuture<Output = Result<T, E>>,
) -> Result<T, String> {
    let started = Instant::now();
    let mut fut = pin!(tokio::time::timeout(ASK_TIMEOUT, ask.into_future()));
    let r = tokio::select! {
        r = &mut fut => r,
        () = tokio::time::sleep(ASK_SLOW) => {
            tracing::warn!(ask = what, waited_s = ASK_SLOW.as_secs(), "telegram: engine ask still unanswered");
            fut.await
        }
    };
    match r {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => {
            tracing::warn!(
                ask = what,
                waited_s = started.elapsed().as_secs(),
                "telegram: engine ask timed out"
            );
            Err(ask_timed_out())
        }
    }
}

/// What the channel asks of the agent and approval actors.
#[async_trait]
pub trait Port: Send + Sync {
    /// `agent::SendInput`; `session: None` creates a root with `scope` first
    /// (`NewRootSession`, 50 §2.3) so the thread's root carries the owner's
    /// scope from birth. Returns the session the input went to. `channel`
    /// (172) rides beside the text: the row's typed origin.
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        scope: PeerScope,
        channel: ChannelOrigin,
    ) -> Result<SessionId, String>;
    /// `agent::NewRootSession`: an empty root with an explicit scope.
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String>;
    /// `agent::Steer` with a fresh `InputId`: lands after the current tool batch.
    async fn steer(
        &self,
        session: SessionId,
        text: String,
        files: Vec<PathBuf>,
        channel: ChannelOrigin,
    ) -> Result<(), String>;
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
        channel: ChannelOrigin,
    ) -> Result<SessionId, String> {
        let session = match session {
            Some(s) => s,
            None => self.new_root(scope).await?,
        };
        timed(
            "SendInput",
            agents()?.ask(SendInput {
                session: Some(session),
                text,
                files,
                delivery: agent::Delivery::default(),
                id: None,
                channel: Some(channel),
            }),
        )
        .await
    }

    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
        timed(
            "NewRootSession",
            agents()?.ask(NewRootSession { cwd: None, scope }),
        )
        .await
    }

    async fn steer(
        &self,
        session: SessionId,
        text: String,
        files: Vec<PathBuf>,
        channel: ChannelOrigin,
    ) -> Result<(), String> {
        timed(
            "Steer",
            agents()?.ask(Steer {
                session,
                id: InputId::new(),
                text,
                files,
                delivery: agent::Delivery::default(),
                channel: Some(channel),
            }),
        )
        .await
    }

    async fn cancel(&self, session: SessionId) -> Result<(), String> {
        timed(
            "Cancel",
            agents()?.ask(Cancel {
                session,
                keep: None,
                scope: agent::CancelScope::Root,
                reason: agent::CancelReason::Stop,
            }),
        )
        .await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        timed("ListSessions", agents()?.ask(ListSessions)).await
    }

    async fn load_session(&self, session: SessionId) -> Result<Loaded, String> {
        timed(
            "LoadSession",
            agents()?.ask(LoadSession {
                session,
                before: None,
                limit: None,
                settled_only: false,
            }),
        )
        .await
    }

    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String> {
        let approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "approval actor is not running".to_string())?;
        timed("Resolve", approval.ask(Resolve { id, decision })).await
    }

    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String> {
        timed("answer_ask", agent::answer_ask(id, answers)).await
    }

    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String> {
        timed("ListAgents", agents()?.ask(ListAgents { session })).await
    }

    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String> {
        timed("Ancestors", agents()?.ask(Ancestors { session }))
            .await
            .map(|chain| chain.into_iter().map(|a| a.id).collect())
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
