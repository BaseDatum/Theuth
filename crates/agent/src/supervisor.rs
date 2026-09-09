//! `Agents` supervisor: registered as [`crate::AGENT_ACTOR`]; owns one
//! session actor per live session, the sqlite store, and peer routing
//! (design doc 01 §2.3): child results and peer messages are injected as
//! [`UserInput`] into the addressee.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRole};
use genai::resolver::AuthData;
use genai::{Client, ModelIden};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use models::{Catalog, Resolved};
use runtime::{Runtime, SessionId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::task::AbortHandle;
use tools::{REGISTRY_ACTOR, RegisterTool, ToolRegistry};

use crate::loop_::{CancelTurn, Session, SwitchModel, UserInput, text_of};
use crate::peers::{SendMessageTool, SpawnSubagentTool};
use crate::skills::{LoadSkillTool, Skills};
use crate::store::Store;
use crate::{
    AGENT_ACTOR, AgentConfig, AgentEvent, Cancel, HistoryEntry, ListChildren, ListModels,
    ListSessions, LoadSession, ModelInfo, SendInput, SessionSummary, SetModel, Steer,
};

/// The supervisor actor. Look up with `ActorRef::<Agents>::lookup(AGENT_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Agents {
    rt: Runtime,
    cfg: Arc<AgentConfig>,
    /// `cfg.model` as last set by [`SetModel`]; new sessions start on it.
    default_model: String,
    catalog: Arc<Catalog>,
    /// `configured(genai_namespace)`: whether genai can authenticate it.
    configured: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    store: Store,
    skills: Arc<Skills>,
    client: Client,
    sessions: HashMap<SessionId, Live>,
    /// Sessions with a turn in flight (from `TurnStarted`/`TurnFinished`).
    running: HashSet<SessionId>,
    /// `(waiter, from)` → resolves with `from`'s next message to `waiter`.
    waiters: HashMap<(SessionId, SessionId), oneshot::Sender<String>>,
    /// Concurrent child turns.
    permits: Arc<Semaphore>,
    /// Child turns started by [`SpawnChild`]: permit + runtime timer.
    child_turns: HashMap<SessionId, ChildTurn>,
    /// Children whose current turn was cancelled by the runtime cap.
    timed_out: HashSet<SessionId>,
    next_msg: u64,
}

/// A live session actor plus its peer identity.
struct Live {
    actor: ActorRef<Session>,
    /// `(parent, name)` for subagents.
    parent: Option<(SessionId, String)>,
}

struct ChildTurn {
    _permit: OwnedSemaphorePermit,
    timer: AbortHandle,
}

/// `ask` → `Result<(name, child), String>`: create a child session and start
/// it on `task` (queued behind the concurrency cap). Returns before the child
/// runs; its result reaches the parent as a `<subagent_result>` input.
#[derive(Clone, Debug)]
pub struct SpawnChild {
    /// Owning session.
    pub parent: SessionId,
    /// Requested peer name; generated when `None`, uniquified within the parent.
    pub name: Option<String>,
    /// First input of the child.
    pub task: String,
}

/// `ask` → `Result<Option<oneshot::Receiver<String>>, String>`: route a peer
/// message. `Err` = unknown addressee. `Some(rx)` when `await_reply`: resolves
/// with the addressee's next message to `from`.
#[derive(Clone, Debug)]
pub struct Deliver {
    /// Sender.
    pub from: SessionId,
    /// `"parent"` or a child name.
    pub to: String,
    /// Body.
    pub text: String,
    /// Register a waiter for the answer.
    pub await_reply: bool,
    /// Id of the `<message>` being answered.
    pub reply_to: Option<String>,
}

/// `ask` → `String`: the §2.5 prompt block for `session` (empty when it has
/// no peers and cannot spawn).
#[derive(Clone, Copy, Debug)]
pub struct PeerContext {
    /// Session.
    pub session: SessionId,
}

/// `tell` (internal): a permit was acquired; run the child's first turn.
struct StartChildTurn {
    child: SessionId,
    permit: OwnedSemaphorePermit,
    text: String,
}

/// `tell` (internal): a child turn hit `max_runtime_secs`.
struct ChildTimedOut(SessionId);

const ADJECTIVES: [&str; 8] = [
    "quick", "calm", "bold", "keen", "brisk", "neat", "wise", "swift",
];
const NOUNS: [&str; 8] = [
    "otter", "heron", "fox", "lynx", "finch", "hare", "wren", "elk",
];

impl Agents {
    /// Resolve a config/row id; on failure keep it raw and carry the message
    /// for the session's next round to surface.
    fn resolve_model(&self, id: &str) -> (Resolved, Option<String>) {
        match self.catalog.resolve(id, &*self.configured) {
            Ok(r) => (r, None),
            Err(e) => (
                Resolved {
                    id: id.to_owned(),
                    genai: id.to_owned(),
                    provider: None,
                    model: None,
                },
                Some(e),
            ),
        }
    }

    /// Live actor for `id`, resuming from the store (or creating the row) when needed.
    async fn session(
        &mut self,
        id: Option<SessionId>,
        parent: Option<(SessionId, &str)>,
    ) -> anyhow::Result<(SessionId, ActorRef<Session>)> {
        let id = id.unwrap_or_default();
        if let Some(live) = self.sessions.get(&id).filter(|l| l.actor.is_alive()) {
            return Ok((id, live.actor.clone()));
        }
        let (model, history, parent) = match self.store.session(id).await? {
            Some(row) => (
                self.resolve_model(&row.model),
                self.store.messages(id).await?,
                row.parent.zip(row.name),
            ),
            None => {
                let sub = &self.cfg.subagents;
                let model = match parent {
                    Some(_) => sub.model.as_deref().unwrap_or(&self.default_model),
                    None => &self.default_model,
                };
                let model = self.resolve_model(model);
                self.store
                    .create_session(id, &model.0.id, parent.map(|p| p.0), parent.map(|p| p.1))
                    .await?;
                (model, Vec::new(), parent.map(|(p, n)| (p, n.to_string())))
            }
        };
        let depth = self.store.depth(id).await?;
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
        self.sessions.insert(
            id,
            Live {
                actor: actor.clone(),
                parent,
            },
        );
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
                    .publish(AgentEvent::Error {
                        session: id,
                        turn: None,
                        message: format!("{e:#}"),
                    })
                    .await;
                id
            }
        }
    }

    /// Inject `text` (already wrapped) into `to` and announce it.
    async fn inject(&mut self, to: SessionId, from: &str, text: String, wrapped: String) {
        self.input(Some(to), wrapped).await;
        self.rt
            .publish(AgentEvent::MessageDelivered {
                session: to,
                from: from.to_string(),
                text,
            })
            .await;
    }

    /// Resolve `to` relative to `from`: `"parent"` or a child name.
    async fn resolve(&mut self, from: SessionId, to: &str) -> Option<(SessionId, String)> {
        if to == "parent" {
            let (parent, name) = self.sessions.get(&from)?.parent.clone()?;
            return Some((parent, name));
        }
        let child = self.store.child_by_name(from, to).await.ok()??;
        Some((child, "parent".into()))
    }

    async fn child_finished(&mut self, child: SessionId, stop_reason: &str) {
        let Some((parent, name)) = self.sessions.get(&child).and_then(|l| l.parent.clone()) else {
            return;
        };
        let mut text = self.store.last_turn_text(child).await.unwrap_or_else(|e| {
            tracing::error!(session = %child, "last_turn_text: {e:#}");
            String::new()
        });
        if self.timed_out.remove(&child) {
            text = format!(
                "[timed out after {}s]\n{text}",
                self.cfg.subagents.max_runtime_secs
            );
        }
        if let Some(tx) = self.waiters.remove(&(parent, child)) {
            match tx.send(text) {
                Ok(()) => return,
                Err(t) => text = t, // waiter gave up (timeout); fall through to inject
            }
        }
        let wrapped = format!(
            "<subagent_result name=\"{name}\" session=\"{child}\" stop=\"{stop_reason}\">\n{text}\n</subagent_result>"
        );
        self.inject(parent, &name, text, wrapped).await;
    }
}

impl Message<AgentEvent> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) {
        match ev {
            AgentEvent::TurnStarted { session, .. } => {
                self.running.insert(session);
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                self.running.remove(&session);
                if let Some(ct) = self.child_turns.remove(&session) {
                    ct.timer.abort();
                }
                self.child_finished(session, &stop_reason).await;
            }
            _ => {}
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
        let children = self.store.children(m.session).await.unwrap_or_default();
        for id in std::iter::once(m.session).chain(children.into_iter().map(|c| c.id)) {
            if let Some(live) = self
                .sessions
                .get(&id)
                .filter(|_| id == m.session || self.running.contains(&id))
            {
                let _ = live.actor.tell(CancelTurn).await;
            }
        }
    }
}

impl Message<SpawnChild> for Agents {
    type Reply = Result<(String, SessionId), String>;

    async fn handle(
        &mut self,
        m: SpawnChild,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(String, SessionId), String> {
        let sub = &self.cfg.subagents;
        let depth = self
            .store
            .depth(m.parent)
            .await
            .map_err(|e| e.to_string())?;
        if depth >= sub.max_depth {
            return Err(format!("subagents are not available at depth {depth}"));
        }
        let id = SessionId::new();
        let base = m.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let b = id.0.as_bytes();
            format!(
                "{}-{}",
                ADJECTIVES[b[14] as usize % ADJECTIVES.len()],
                NOUNS[b[15] as usize % NOUNS.len()]
            )
        });
        let mut name = base.clone();
        let mut n = 1;
        while self
            .store
            .child_by_name(m.parent, &name)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            n += 1;
            name = format!("{base}-{n}");
        }
        self.session(Some(id), Some((m.parent, &name)))
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.rt
            .publish(AgentEvent::SubagentSpawned {
                parent: m.parent,
                child: id,
                name: name.clone(),
            })
            .await;
        let me = ctx.actor_ref().clone();
        let permits = self.permits.clone();
        let text = m.task;
        tokio::spawn(async move {
            let Ok(permit) = permits.acquire_owned().await else {
                return;
            };
            let _ = me
                .tell(StartChildTurn {
                    child: id,
                    permit,
                    text,
                })
                .await;
        });
        Ok((name, id))
    }
}

impl Message<StartChildTurn> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: StartChildTurn, ctx: &mut Context<Self, Self::Reply>) {
        let child = m.child;
        self.input(Some(child), m.text).await;
        let me = ctx.actor_ref().clone();
        let after = Duration::from_secs(self.cfg.subagents.max_runtime_secs);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = me.tell(ChildTimedOut(child)).await;
        })
        .abort_handle();
        self.child_turns.insert(
            child,
            ChildTurn {
                _permit: m.permit,
                timer,
            },
        );
    }
}

impl Message<ChildTimedOut> for Agents {
    type Reply = ();

    async fn handle(
        &mut self,
        ChildTimedOut(child): ChildTimedOut,
        _: &mut Context<Self, Self::Reply>,
    ) {
        if self.child_turns.contains_key(&child)
            && let Some(live) = self.sessions.get(&child)
        {
            self.timed_out.insert(child);
            let _ = live.actor.tell(CancelTurn).await;
        }
    }
}

impl Message<Deliver> for Agents {
    type Reply = Result<Option<oneshot::Receiver<String>>, String>;

    async fn handle(
        &mut self,
        m: Deliver,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<oneshot::Receiver<String>>, String> {
        let Some((to, from_name)) = self.resolve(m.from, &m.to).await else {
            return Err(format!("unknown peer `{}`", m.to));
        };
        // A waiter that gave up (timeout) has dropped its receiver; fall through to inject.
        let unresolved = match self.waiters.remove(&(to, m.from)) {
            Some(tx) => tx.send(m.text).err(),
            None => Some(m.text),
        };
        if let Some(text) = unresolved {
            self.next_msg += 1;
            let reply_to = m
                .reply_to
                .map(|r| format!(" reply_to=\"{r}\""))
                .unwrap_or_default();
            let wrapped = format!(
                "<message from=\"{from_name}\" id=\"m{}\" reply_expected=\"{}\"{reply_to}>\n{text}\n</message>",
                self.next_msg, m.await_reply
            );
            self.inject(to, &from_name, text, wrapped).await;
        }
        Ok(m.await_reply.then(|| {
            let (tx, rx) = oneshot::channel();
            self.waiters.insert((m.from, to), tx);
            rx
        }))
    }
}

impl Message<PeerContext> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: PeerContext, _: &mut Context<Self, Self::Reply>) -> String {
        let mut s = String::new();
        if let Some((parent, name)) = self
            .sessions
            .get(&m.session)
            .and_then(|l| l.parent.as_ref())
        {
            s.push_str(&format!(
                "## Peers\nYou are `{name}`, a subagent of session {parent} (address it as `parent`).\nYour final message is delivered to the parent when your turn ends."
            ));
        }
        let children = self.store.children(m.session).await.unwrap_or_default();
        let can_spawn =
            self.store.depth(m.session).await.unwrap_or(u8::MAX) < self.cfg.subagents.max_depth;
        if children.is_empty() && !can_spawn {
            return s;
        }
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str("## Subagents\n");
        for c in children.iter().rev() {
            let status = if self.running.contains(&c.id) {
                "running"
            } else {
                "idle"
            };
            s.push_str(&format!(
                "- {}  {status}\n",
                c.name.as_deref().unwrap_or("?")
            ));
        }
        if can_spawn {
            s.push_str("Spawn with spawn_subagent (put all shared background in `task`; the subagent sees nothing else); steer or ask with send_message. Results arrive as <subagent_result> messages; you do not need to poll.");
        } else {
            s.push_str("Steer or ask with send_message. Results arrive as <subagent_result> messages; you do not need to poll.");
        }
        s
    }
}

impl Message<ListChildren> for Agents {
    type Reply = Vec<SessionSummary>;

    async fn handle(
        &mut self,
        m: ListChildren,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<SessionSummary> {
        self.store.children(m.parent).await.unwrap_or_else(|e| {
            tracing::error!(parent = %m.parent, "list children: {e:#}");
            Vec::new()
        })
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

impl Message<ListModels> for Agents {
    type Reply = Vec<ModelInfo>;

    async fn handle(
        &mut self,
        _: ListModels,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ModelInfo> {
        let mut out: Vec<ModelInfo> = self
            .catalog
            .providers()
            .filter(|p| (self.configured)(Catalog::adapter(&p.id)))
            .flat_map(|p| p.models.values().map(|m| ModelInfo::new(&p.id, m)))
            .collect();
        out.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        out
    }
}

impl Message<SetModel> for Agents {
    type Reply = Result<ModelInfo, String>;

    async fn handle(
        &mut self,
        m: SetModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ModelInfo, String> {
        let resolved = self.catalog.resolve(&m.model, &*self.configured)?;
        if let Some(id) = m.session {
            if self
                .store
                .session(id)
                .await
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err(format!("unknown session {id}"));
            }
            let (_, actor) = self
                .session(Some(id), None)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let _ = actor.tell(SwitchModel(resolved.clone())).await;
            if let Err(e) = self.store.set_model(id, &resolved.id).await {
                tracing::warn!(session = %id, "set_model: {e:#}");
            }
            self.rt
                .publish(AgentEvent::ModelChanged {
                    session: id,
                    model: resolved.id.clone(),
                })
                .await;
        }
        self.default_model = resolved.id.clone();
        // ponytail: writes <data_dir>/config.yaml; a `--config <other>` file is not written back.
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "agent", "model", &resolved.id) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(ModelInfo::from(&resolved))
    }
}

impl ModelInfo {
    fn new(provider: &str, m: &models::Model) -> Self {
        Self {
            id: format!("{provider}/{}", m.id),
            name: m.name.clone(),
            provider: provider.to_owned(),
            context: m.limit.context,
            reasoning: m.reasoning,
        }
    }
}

impl From<&Resolved> for ModelInfo {
    fn from(r: &Resolved) -> Self {
        match (&r.provider, &r.model) {
            (Some(p), Some(m)) => Self::new(p, m),
            _ => Self {
                id: r.id.clone(),
                name: r.id.clone(),
                provider: r.provider.clone().unwrap_or_default(),
                context: 0,
                reasoning: false,
            },
        }
    }
}

/// Collapse stored messages into UI entries; a tool call and its response
/// become one `Tool` entry.
fn render(messages: &[ChatMessage]) -> Vec<HistoryEntry> {
    let mut out = Vec::new();
    let mut pending: HashMap<&str, (&str, &serde_json::Value)> = HashMap::new();
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
                    pending.insert(&c.call_id, (&c.fn_name, &c.fn_arguments));
                }
            }
            ChatRole::Tool => {
                for r in m.content.tool_responses() {
                    let (name, args) = pending
                        .remove(r.call_id.as_str())
                        .unwrap_or(("?", &serde_json::Value::Null));
                    out.push(HistoryEntry::Tool {
                        name: name.to_string(),
                        args: args.clone(),
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

/// `<data_dir>/credentials.yaml` as `genai_namespace: key` (design doc 01 §1.3).
fn credentials(data_dir: &Path) -> HashMap<String, String> {
    let path = data_dir.join("credentials.yaml");
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(md) = std::fs::metadata(&path)
                    && md.permissions().mode() & 0o077 != 0
                {
                    tracing::warn!("{} is readable by others; chmod 600 it", path.display());
                }
            }
            serde_yaml::from_str::<Option<HashMap<String, String>>>(&s)
                .unwrap_or_else(|e| {
                    tracing::warn!("{}: {e}; ignoring", path.display());
                    None
                })
                .unwrap_or_default()
        }
        Err(_) => HashMap::new(),
    }
}

/// genai client whose keys come from `creds`, falling back to each adapter's env var.
fn client(creds: Arc<HashMap<String, String>>) -> anyhow::Result<Client> {
    Ok(Client::builder()
        .with_auth_resolver_fn(move |m: ModelIden| {
            Ok(match creds.get(m.adapter_kind.as_lower_str()) {
                Some(k) => Some(AuthData::Key(k.clone())),
                None => AdapterKind::default_key_env_name(&m.adapter_kind).map(AuthData::from_env),
            })
        })
        .build()?)
}

/// The auth-resolver rule as a predicate over genai namespaces: a key in
/// `creds`, the adapter's default env var set, or an adapter needing no key (`ollama`).
fn configured(creds: Arc<HashMap<String, String>>) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |ns: &str| {
        creds.contains_key(ns)
            || AdapterKind::from_lower_str(ns).is_some_and(|k| {
                k.default_key_env_name()
                    .is_none_or(|env| std::env::var_os(env).is_some_and(|v| !v.is_empty()))
            })
    })
}

/// Open the store, spawn and register the supervisor.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let cfg: AgentConfig = rt.section("agent").unwrap_or_else(|e| {
        tracing::debug!("no [agent] config section, using defaults: {e}");
        AgentConfig::default()
    });
    let cfg = Arc::new(cfg);
    let store = Store::open(&rt.data_dir.join("sessions.sqlite")).await?;
    let skills = Arc::new(Skills::load(&rt.data_dir.join("skills")));
    let creds = Arc::new(credentials(&rt.data_dir));
    let agents = Agents::spawn_with_mailbox(
        Agents {
            rt: rt.clone(),
            cfg: cfg.clone(),
            default_model: cfg.model.clone(),
            catalog: Arc::new(Catalog::load(&rt.data_dir)),
            configured: configured(creds.clone()),
            store,
            skills: skills.clone(),
            client: client(creds)?,
            sessions: HashMap::new(),
            running: HashSet::new(),
            waiters: HashMap::new(),
            permits: Arc::new(Semaphore::new(cfg.subagents.max_concurrent.max(1))),
            child_turns: HashMap::new(),
            timed_out: HashSet::new(),
            next_msg: 0,
        },
        runtime::subscriber_mailbox(),
    );
    agents.register(AGENT_ACTOR)?;
    rt.subscribe::<Agents, AgentEvent>(&agents).await;

    // The tools plugin may start after us; retry the lookup briefly.
    tokio::spawn(async move {
        for _ in 0..50 {
            if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
                let _ = reg
                    .ask(RegisterTool(Arc::new(LoadSkillTool { skills })))
                    .await;
                let _ = reg.ask(RegisterTool(Arc::new(SpawnSubagentTool))).await;
                let _ = reg
                    .ask(RegisterTool(Arc::new(SendMessageTool { cfg })))
                    .await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!(
            "tool registry never came up; load_skill/spawn_subagent/send_message not registered"
        );
    });
    Ok(())
}
