//! `Agents` supervisor: registered as [`crate::AGENT_ACTOR`]; owns one
//! session actor per live session and the sqlite store.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use genai::Client;
use genai::chat::{ChatMessage, ChatRole};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use tools::{REGISTRY_ACTOR, RegisterTool, ToolRegistry};

use crate::loop_::{CancelTurn, Session, UserInput, text_of};
use crate::skills::{LoadSkillTool, Skills};
use crate::store::Store;
use crate::subagent::SpawnSubagentTool;
use crate::{
    AGENT_ACTOR, AgentConfig, Cancel, HistoryEntry, ListSessions, LoadSession, SendInput,
    SessionSummary, Steer,
};

/// The supervisor actor. Look up with `ActorRef::<Agents>::lookup(AGENT_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Agents {
    rt: Runtime,
    cfg: Arc<AgentConfig>,
    store: Store,
    skills: Arc<Skills>,
    client: Client,
    sessions: HashMap<SessionId, ActorRef<Session>>,
}

/// `ask` → `Option<SessionId>` (internal, used by the `spawn_subagent` tool):
/// create an idle child session at depth 1; feed it with [`SendInput`].
#[derive(Clone, Copy, Debug)]
pub struct SpawnChild {
    /// Owning session.
    pub parent: SessionId,
}

impl Agents {
    /// Live actor for `id`, resuming from the store (or creating the row) when needed.
    async fn session(
        &mut self,
        id: Option<SessionId>,
        parent: Option<SessionId>,
    ) -> anyhow::Result<(SessionId, ActorRef<Session>)> {
        let id = id.unwrap_or_default();
        if let Some(actor) = self.sessions.get(&id).filter(|a| a.is_alive()) {
            return Ok((id, actor.clone()));
        }
        let depth = u8::from(parent.is_some());
        let (model, history) = match self.store.session(id).await? {
            Some(row) => (row.model, self.store.messages(id).await?),
            None => {
                let model = match parent {
                    Some(_) => self
                        .cfg
                        .subagent_model
                        .clone()
                        .unwrap_or_else(|| self.cfg.model.clone()),
                    None => self.cfg.model.clone(),
                };
                self.store.create_session(id, &model, parent).await?;
                (model, Vec::new())
            }
        };
        let actor = Session::spawn(Session::new(
            id,
            depth,
            model,
            history,
            self.cfg.clone(),
            self.store.clone(),
            self.client.clone(),
            self.rt.clone(),
            self.skills.clone(),
        ));
        self.sessions.insert(id, actor.clone());
        Ok((id, actor))
    }

    async fn input(&mut self, session: Option<SessionId>, text: String) -> SessionId {
        match self.session(session, None).await {
            Ok((id, actor)) => {
                let _ = actor.tell(UserInput(text)).await;
                id
            }
            Err(e) => {
                let id = session.unwrap_or_default();
                tracing::error!(session = %id, "session: {e:#}");
                self.rt
                    .publish(crate::AgentEvent::Error {
                        session: id,
                        turn: None,
                        message: format!("{e:#}"),
                    })
                    .await;
                id
            }
        }
    }
}

impl Message<SendInput> for Agents {
    type Reply = Result<SessionId, kameo::error::Infallible>;

    async fn handle(
        &mut self,
        m: SendInput,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SessionId, kameo::error::Infallible> {
        Ok(self.input(m.session, m.text).await)
    }
}

impl Message<Steer> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Steer, _: &mut Context<Self, Self::Reply>) {
        self.input(Some(m.session), m.text).await;
    }
}

impl Message<Cancel> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Cancel, _: &mut Context<Self, Self::Reply>) {
        if let Some(actor) = self.sessions.get(&m.session) {
            let _ = actor.tell(CancelTurn).await;
        }
    }
}

impl Message<SpawnChild> for Agents {
    type Reply = Option<SessionId>;

    async fn handle(
        &mut self,
        m: SpawnChild,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<SessionId> {
        match self.session(None, Some(m.parent)).await {
            Ok((id, _)) => Some(id),
            Err(e) => {
                tracing::error!(parent = %m.parent, "spawn child: {e:#}");
                None
            }
        }
    }
}

impl Message<ListSessions> for Agents {
    type Reply = Vec<SessionSummary>;

    async fn handle(
        &mut self,
        _: ListSessions,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<SessionSummary> {
        self.store.list_sessions().await.unwrap_or_else(|e| {
            tracing::error!("list sessions: {e:#}");
            Vec::new()
        })
    }
}

impl Message<LoadSession> for Agents {
    type Reply = Vec<HistoryEntry>;

    async fn handle(
        &mut self,
        m: LoadSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<HistoryEntry> {
        match self.store.messages(m.session).await {
            Ok(messages) => render(&messages),
            Err(e) => {
                tracing::error!(session = %m.session, "load session: {e:#}");
                Vec::new()
            }
        }
    }
}

/// Collapse stored messages into UI entries; a tool call and its response
/// become one `Tool` entry.
fn render(messages: &[ChatMessage]) -> Vec<HistoryEntry> {
    let mut out = Vec::new();
    let mut pending: HashMap<&str, (&str, String)> = HashMap::new();
    for m in messages {
        match m.role {
            ChatRole::User => out.push(HistoryEntry::User {
                text: text_of(&m.content),
            }),
            ChatRole::Assistant => {
                let text = text_of(&m.content);
                if !text.is_empty() {
                    out.push(HistoryEntry::Assistant { text });
                }
                for c in m.content.tool_calls() {
                    pending.insert(&c.call_id, (&c.fn_name, c.fn_arguments.to_string()));
                }
            }
            ChatRole::Tool => {
                for r in m.content.tool_responses() {
                    let (name, summary) = pending
                        .remove(r.call_id.as_str())
                        .unwrap_or(("?", String::new()));
                    out.push(HistoryEntry::Tool {
                        name: name.to_string(),
                        summary,
                        is_error: r.content.starts_with("ERROR: "),
                        output: r.content.clone(),
                    });
                }
            }
            ChatRole::System => {}
        }
    }
    out
}

/// Open the store, spawn and register the supervisor.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let cfg: AgentConfig = rt.section("agent").unwrap_or_else(|e| {
        tracing::debug!("no [agent] config section, using defaults: {e}");
        AgentConfig::default()
    });
    let store = Store::open(&rt.data_dir.join("sessions.sqlite")).await?;
    let skills = Arc::new(Skills::load(&rt.data_dir.join("skills")));
    let agents = Agents::spawn(Agents {
        rt: rt.clone(),
        cfg: Arc::new(cfg),
        store,
        skills: skills.clone(),
        client: Client::new()?,
        sessions: HashMap::new(),
    });
    agents.register(AGENT_ACTOR)?;

    // The tools plugin may start after us; retry the lookup briefly.
    let rt = rt.clone();
    tokio::spawn(async move {
        for _ in 0..50 {
            if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
                let _ = reg
                    .ask(RegisterTool(Arc::new(LoadSkillTool { skills })))
                    .await;
                let _ = reg
                    .ask(RegisterTool(Arc::new(SpawnSubagentTool { rt })))
                    .await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!("tool registry never came up; load_skill/spawn_subagent not registered");
    });
    Ok(())
}
