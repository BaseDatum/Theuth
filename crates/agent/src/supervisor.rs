//! `Agents` supervisor: registered as [`crate::AGENT_ACTOR`]; owns one
//! session actor per live session, the sqlite store, and peer routing
//! (design doc 01 §2.3): child results, peer messages and finished jobs
//! (14 §3) are injected as [`UserInput`] into the addressee.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRole};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, Headers, ModelIden, ServiceTarget};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use models::{Catalog, Resolved};
use runtime::{InputId, Runtime, SessionId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::task::AbortHandle;
use tools::{
    ApprovalId, ApprovalRequested, ApprovalResolved, JobEvent, JobExit, JobId, ListPrompts,
    ListTools, REGISTRY_ACTOR, RegisterPrompt, RegisterTool, SetParent, ToolRegistry,
};

use crate::acp_child::AcpChild;
use crate::loop_::{
    CancelTurn, CompactNow, HarvestNow, IsRunning, QueuedInputs, RedoTurn, Session, SessionState,
    SetPendingEffort, SwitchModel, UndoTurn, UserInput, WithdrawInput, text_of,
};
use crate::peers::{SendMessageTool, SpawnSubagentTool};
use crate::skills::{
    self, AddSkillRepoTool, AddSkillTool, LoadSkillTool, RemoveSkillRepoTool, RemoveSkillTool,
    SyncSkillsTool,
};
use crate::store::{Epoch, Store};
use crate::{
    AGENT_ACTOR, AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentEvent, AgentInfo,
    AgentState, Cancel, Compact, CompactStyle, GetCompactStyle, GetTitleModel, Harvest,
    HistoryEntry, ListAcp, ListAgents, ListLogins, ListModels, ListProviders, ListQueued,
    ListRegistry, ListSessions, ListSkills, LoadSession, LoadSkill, Loaded, Login, LoginFinished,
    LoginInfo, Logout, ModelInfo, NewSession, ProviderInfo, QueuedInput, Redo, RemoveAcp,
    RemoveProvider, RemoveSkill, RemoveSkillRepo, SendInput, SessionSummary, SetCompactStyle,
    SetEffort, SetModel, SetTitleModel, ShowPrompt, SkillBody, SkillMeta, Steer, SyncSkills, Undo,
    Withdraw, effort_of,
};
use crate::{frame, repl, spill};

/// The supervisor actor. Look up with `ActorRef::<Agents>::lookup(AGENT_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Agents {
    rt: Runtime,
    cfg: Arc<AgentConfig>,
    /// `cfg.model` as last set by [`SetModel`]; new sessions start on it.
    default_model: String,
    /// `cfg.effort` as last set by [`SetEffort`]; new sessions start on it.
    default_effort: Option<String>,
    /// Live default compaction style; cloned into each session so [`SetCompactStyle`] is in-process.
    compact_style: Arc<RwLock<CompactStyle>>,
    /// Live title model (07 §15); `None` = each session's own model. Shared with every session.
    title_model: Arc<RwLock<Option<Resolved>>>,
    catalog: Arc<Catalog>,
    /// Python runtime workers; `None` when the `monty` binary was not found (design doc 08 §9).
    pool: Option<Arc<monty_pool::Pool>>,
    /// `configured(genai_namespace)`: whether genai can authenticate it.
    configured: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    store: Store,
    client: Client,
    auth: Auth,
    sessions: HashMap<SessionId, Live>,
    /// Sessions with a turn in flight (from `TurnStarted`/`TurnFinished`).
    running: HashSet<SessionId>,
    /// `(waiter, from)` → resolves with `from`'s next message to `waiter`.
    waiters: HashMap<(SessionId, SessionId), oneshot::Sender<String>>,
    /// Concurrent child turns.
    permits: Arc<Semaphore>,
    /// Every running child turn: `started_at` always; permit + runtime timer
    /// only for the spawn turn started by [`SpawnChild`].
    child_turns: HashMap<SessionId, ChildTurn>,
    /// Children whose current turn was cancelled by the runtime cap.
    timed_out: HashSet<SessionId>,
    /// Last `TurnFinished` per session this process saw: `(stop_reason, when)`.
    last_stop: HashMap<SessionId, (String, DateTime<Utc>)>,
    /// Unanswered approvals per session: `(id, tool)` (design doc 19 §2.1).
    approvals: HashMap<SessionId, (ApprovalId, String)>,
    next_msg: u64,
}

/// A live session actor plus its peer identity.
struct Live {
    actor: LiveActor,
    /// `(parent, name)` for subagents.
    parent: Option<(SessionId, String)>,
}

/// What runs a session's turns: wth's own loop, or an external ACP harness
/// (design doc 06 §3.2). Both take [`UserInput`], [`CancelTurn`] and [`IsRunning`].
#[derive(Clone)]
enum LiveActor {
    Native(ActorRef<Session>),
    Acp(ActorRef<AcpChild>),
}

/// The refusal for every `Session`-only command on an ACP child.
const NOT_NATIVE: &str = "not available for an external harness";

impl LiveActor {
    fn is_alive(&self) -> bool {
        match self {
            Self::Native(a) => a.is_alive(),
            Self::Acp(a) => a.is_alive(),
        }
    }

    async fn tell_input(&self, input: UserInput) {
        match self {
            Self::Native(a) => {
                let _ = a.tell(input).await;
            }
            Self::Acp(a) => {
                let _ = a.tell(input).await;
            }
        }
    }

    async fn tell_cancel(&self) {
        match self {
            Self::Native(a) => {
                let _ = a.tell(CancelTurn).await;
            }
            Self::Acp(a) => {
                let _ = a.tell(CancelTurn).await;
            }
        }
    }

    async fn is_running(&self) -> bool {
        match self {
            Self::Native(a) => a.ask(IsRunning).await.unwrap_or(false),
            Self::Acp(a) => a.ask(IsRunning).await.unwrap_or(false),
        }
    }

    /// The wth loop behind this session; `Err` for an external harness.
    fn native(&self) -> Result<&ActorRef<Session>, String> {
        match self {
            Self::Native(a) => Ok(a),
            Self::Acp(_) => Err(NOT_NATIVE.into()),
        }
    }
}

/// One child turn in flight (design doc 19 §2.1).
struct ChildTurn {
    /// Concurrency permit; `None` for a follow-up turn (human steer, peer message).
    _permit: Option<OwnedSemaphorePermit>,
    /// `max_runtime_secs` timer; `None` for a follow-up turn.
    timer: Option<AbortHandle>,
    /// When the turn started (the picker's elapsed column).
    started_at: DateTime<Utc>,
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
    /// External harness (`agents.yaml` key); `None` runs a wth subagent (design doc 06 §3.2).
    pub agent: Option<String>,
    /// Working directory for an external harness's `session/new`; default: wth's cwd.
    pub cwd: Option<PathBuf>,
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

/// `ask` → `String`: the child's identity line only (design doc 07 §10 — the
/// roster and static guidance live in [`frame::baseline`]). Empty for a root session.
#[derive(Clone, Copy, Debug)]
pub struct PeerContext {
    /// Session.
    pub session: SessionId,
}

/// `ask` → `String`: `"Subagents: a (running), b (idle)"` or `""` — harness
/// truth for the compaction head (design doc 07 §8.4).
#[derive(Clone, Copy, Debug)]
pub struct Roster {
    /// Session whose children to list.
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

    /// Live actor for `id`, resuming from the store (or creating the row) when
    /// needed. `acp` = `(harness, cwd)` runs a new child on an external harness
    /// (design doc 06 §3.2); a row whose `agent` column is set resumes on one.
    async fn session(
        &mut self,
        id: Option<SessionId>,
        parent: Option<(SessionId, &str)>,
        acp: Option<(String, Option<PathBuf>)>,
    ) -> anyhow::Result<(SessionId, LiveActor)> {
        let id = id.unwrap_or_default();
        if let Some(live) = self.sessions.get(&id).filter(|l| l.actor.is_alive()) {
            return Ok((id, live.actor.clone()));
        }
        let row = self.store.session(id).await?;
        let harness = acp
            .as_ref()
            .map(|a| a.0.clone())
            .or_else(|| row.as_ref().and_then(|r| r.agent.clone()));
        if let Some(name) = harness {
            let actor = self
                .acp_session(id, row, parent, name, acp.and_then(|a| a.1))
                .await?;
            return Ok((id, actor));
        }
        let (model, effort, state, parent) = match row {
            Some(row) => {
                let (epoch, head, history) = self.store.frame(id).await?;
                (
                    self.resolve_model(&row.model),
                    row.effort,
                    SessionState::resume(epoch, head, history),
                    row.parent.zip(row.name),
                )
            }
            None => {
                let sub = &self.cfg.subagents;
                let (model, effort) = match parent {
                    Some(_) => (
                        sub.model.as_deref().unwrap_or(&self.default_model),
                        sub.effort.clone().or_else(|| self.default_effort.clone()),
                    ),
                    None => (self.default_model.as_str(), self.default_effort.clone()),
                };
                let model = self.resolve_model(model);
                self.store
                    .create_session(id, &model.0.id, parent.map(|p| p.0), parent.map(|p| p.1))
                    .await?;
                if let Some(e) = &effort {
                    self.store.set_effort(id, Some(e)).await?;
                }
                (
                    model,
                    effort,
                    SessionState::resume(0, None, Vec::new()),
                    parent.map(|(p, n)| (p, n.to_string())),
                )
            }
        };
        let depth = self.store.depth(id).await?;
        self.inherit_approvals(id, parent.as_ref().map(|p| p.0))
            .await;
        let actor = Session::spawn(Session::new(
            id,
            depth,
            model,
            effort.as_deref().and_then(effort_of),
            state,
            self.cfg.clone(),
            self.compact_style.clone(),
            self.title_model.clone(),
            self.store.clone(),
            self.client.clone(),
            self.auth.clone(),
            self.rt.clone(),
            self.catalog.clone(),
            self.pool.clone(),
        ));
        let actor = LiveActor::Native(actor);
        self.sessions.insert(
            id,
            Live {
                actor: actor.clone(),
                parent,
            },
        );
        Ok((id, actor))
    }

    /// Spawn an [`AcpChild`] for `id` on harness `name`: a new row (model
    /// column = the harness name) or a resume of `row` with its stored ACP
    /// session id (design doc 06 §3.2, §3.6).
    async fn acp_session(
        &mut self,
        id: SessionId,
        row: Option<SessionSummary>,
        parent: Option<(SessionId, &str)>,
        name: String,
        cwd: Option<PathBuf>,
    ) -> anyhow::Result<LiveActor> {
        let entries = acp::load(&self.rt.data_dir)?;
        let Some(entry) = entries.get(&name) else {
            anyhow::bail!("{}", acp::unknown(&name, &entries));
        };
        let (parent, cwd, resume) = match row {
            Some(row) => (
                row.parent.zip(row.name),
                row.cwd.map(PathBuf::from),
                self.store.acp_session(id).await?,
            ),
            None => {
                self.store
                    .create_session(id, &name, parent.map(|p| p.0), parent.map(|p| p.1))
                    .await?;
                self.store.set_acp(id, &name, cwd.as_deref()).await?;
                (parent.map(|(p, n)| (p, n.to_string())), cwd, None)
            }
        };
        let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.inherit_approvals(id, parent.as_ref().map(|p| p.0))
            .await;
        let actor = LiveActor::Acp(AcpChild::spawn(AcpChild::new(
            id,
            name,
            entry,
            cwd,
            resume,
            self.store.clone(),
            self.rt.clone(),
        )));
        self.sessions.insert(
            id,
            Live {
                actor: actor.clone(),
                parent,
            },
        );
        Ok(actor)
    }

    /// Subagents inherit the parent's approval switch.
    async fn inherit_approvals(&self, id: SessionId, parent: Option<SessionId>) {
        if let Some(parent) = parent
            && let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
        {
            let _ = reg
                .tell(SetParent {
                    session: id,
                    parent,
                })
                .await;
        }
    }

    /// Route `text` (plus image `files`) into `session` (created when `None`);
    /// errors surface as `AgentEvent::Error`. A rejected attachment drops the
    /// whole input (17 §3.1).
    async fn input(
        &mut self,
        session: Option<SessionId>,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> SessionId {
        let routed = match check_files(&files) {
            Ok(()) => self.session(session, None, None).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        match routed {
            Ok((sid, actor)) => {
                actor.tell_input(UserInput { id, text, files }).await;
                sid
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

    /// [`SendInput`] / [`Steer`] from a human (design doc 19 §2.3): a session
    /// blocked in `send_message{await_reply}` — a pending `(session, _)`
    /// waiter — gets `text` as the tool's return value instead of a queued
    /// message, announced as `MessageDelivered { from: "you" }`. Harness
    /// inputs ([`Agents::inject`]) never take this path. Attachments cannot
    /// answer a tool call, so an input with `files` is always routed.
    async fn human_input(
        &mut self,
        session: Option<SessionId>,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> SessionId {
        let waiter = session
            .filter(|_| files.is_empty())
            .and_then(|s| self.waiters.keys().find(|(w, _)| *w == s).copied());
        let text = match waiter.and_then(|k| self.waiters.remove(&k)) {
            // A waiter that gave up (timeout) has dropped its receiver; fall through to route.
            Some(tx) => match tx.send(text.clone()) {
                Ok(()) => {
                    let target = session.unwrap_or_default();
                    self.rt
                        .publish(AgentEvent::MessageDelivered {
                            session: target,
                            from: "you".into(),
                            text,
                        })
                        .await;
                    return target;
                }
                Err(_) => text,
            },
            None => text,
        };
        self.input(session, id, text, files).await
    }

    /// The live (or resumed) actor for `session`, with the lookup error as the
    /// caller's `Err` string.
    async fn actor(&mut self, session: SessionId) -> Result<LiveActor, String> {
        self.session(Some(session), None, None)
            .await
            .map(|(_, actor)| actor)
            .map_err(|e| format!("{e:#}"))
    }

    /// The live actor for `session`, if any; a session with no actor has nothing queued.
    fn live(&self, session: SessionId) -> Option<&LiveActor> {
        self.sessions
            .get(&session)
            .filter(|l| l.actor.is_alive())
            .map(|l| &l.actor)
    }

    /// Inject `text` (already wrapped) into `to` and announce it.
    async fn inject(&mut self, to: SessionId, from: &str, text: String, wrapped: String) {
        self.input(Some(to), None, wrapped, Vec::new()).await;
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
                // A follow-up turn (steer, peer message) has no permit or
                // timer but still needs a start time for the picker.
                if self
                    .sessions
                    .get(&session)
                    .is_some_and(|l| l.parent.is_some())
                {
                    self.child_turns
                        .entry(session)
                        .or_insert_with(|| ChildTurn {
                            _permit: None,
                            timer: None,
                            started_at: Utc::now(),
                        });
                }
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                self.running.remove(&session);
                if let Some(timer) = self.child_turns.remove(&session).and_then(|ct| ct.timer) {
                    timer.abort();
                }
                self.last_stop
                    .insert(session, (stop_reason.clone(), Utc::now()));
                self.child_finished(session, &stop_reason).await;
            }
            _ => {}
        }
    }
}

/// Approvals (design doc 19 §2.1): an unanswered request shows the session
/// as `AgentState::Approval`.
impl Message<ApprovalRequested> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalRequested, _: &mut Context<Self, Self::Reply>) {
        self.approvals.insert(ev.session, (ev.id, ev.tool));
    }
}

impl Message<ApprovalResolved> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalResolved, _: &mut Context<Self, Self::Reply>) {
        self.approvals.retain(|_, (id, _)| *id != ev.id);
    }
}

/// Finished jobs (14 §3): `Finished` becomes a `<job_result>` harness input
/// via [`Agents::inject`] — an idle session starts a new turn, a turn in
/// flight queues it after the current tool batch. A session with no live
/// actor is resumed from the store by `input`, so a job that outlives its
/// actor still wakes the session. `Started` / `Output` are for the UIs.
impl Message<JobEvent> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: JobEvent, _: &mut Context<Self, Self::Reply>) {
        let JobEvent::Finished {
            session,
            id,
            command,
            exit,
            secs,
            output,
            inline,
        } = ev
        else {
            return;
        };
        // An inline result was the `shell` tool's own reply; injecting it
        // too would hand the model the same output twice.
        if inline {
            return;
        }
        // Same cap and dir as tool results in the loop (07 §6).
        let text = spill::cap(
            session,
            &format!("job-{id}"),
            &output,
            self.cfg.max_tool_output_chars,
            &self.rt.data_dir.join("spill"),
        );
        let wrapped = job_result(&id, &command, &exit, secs, &text);
        self.inject(session, &format!("job {id}"), text, wrapped)
            .await;
    }
}

/// The `<job_result>` wrapper (14 §1.2): `command` cut to 120 chars and
/// attribute-escaped; `exit` is the code, `timeout` or `killed`.
pub(crate) fn job_result(
    id: &JobId,
    command: &str,
    exit: &JobExit,
    secs: u64,
    text: &str,
) -> String {
    let cmd: String = command.chars().take(120).collect();
    let exit = match exit {
        JobExit::Exited(code) => code.to_string(),
        JobExit::Timeout => "timeout".into(),
        JobExit::Killed => "killed".into(),
    };
    format!(
        "<job_result id=\"{id}\" command=\"{}\" exit=\"{exit}\" secs=\"{secs}\">\n{text}\n</job_result>",
        attr(&cmd)
    )
}

/// XML attribute escape.
fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Message<SendInput> for Agents {
    type Reply = Result<SessionId, kameo::error::Infallible>;

    async fn handle(
        &mut self,
        m: SendInput,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SessionId, kameo::error::Infallible> {
        Ok(self.human_input(m.session, None, m.text, m.files).await)
    }
}

impl Message<Steer> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Steer, _: &mut Context<Self, Self::Reply>) {
        self.human_input(Some(m.session), Some(m.id), m.text, m.files)
            .await;
    }
}

impl Message<Withdraw> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Withdraw, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(actor) = self.live(m.session) else {
            return Err("not queued".into());
        };
        // `SendError::HandlerError` displays as the inner `String`.
        actor
            .native()?
            .ask(WithdrawInput(m.id))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<ListQueued> for Agents {
    type Reply = Vec<QueuedInput>;

    async fn handle(&mut self, m: ListQueued, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match self.live(m.session).and_then(|a| a.native().ok()) {
            Some(actor) => actor.ask(QueuedInputs).await.unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl Message<Undo> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Undo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.actor(m.session)
            .await?
            .native()?
            .ask(UndoTurn)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<Redo> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Redo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.actor(m.session)
            .await?
            .native()?
            .ask(RedoTurn)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<Cancel> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Cancel, _: &mut Context<Self, Self::Reply>) {
        // Every running descendant, not one level (design doc 19 §2.3).
        let tree = self.store.descendants(m.session).await.unwrap_or_default();
        for id in std::iter::once(m.session).chain(tree.into_iter().map(|(_, s)| s.id)) {
            if let Some(live) = self
                .sessions
                .get(&id)
                .filter(|_| id == m.session || self.running.contains(&id))
            {
                live.actor.tell_cancel().await;
            }
        }
    }
}

impl Message<Compact> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<u32, String>>;

    async fn handle(&mut self, m: Compact, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let actor = match self
            .actor(m.session)
            .await
            .and_then(|a| a.native().cloned())
        {
            Ok(actor) => actor,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // Off the mailbox: `compact_now` asks us for the `Roster`, so awaiting
        // here would deadlock. `SendError::HandlerError` displays as the inner `String`.
        tokio::spawn(async move {
            tx.send(
                actor
                    .ask(CompactNow { style: m.style })
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

impl Message<SetCompactStyle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetCompactStyle,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        *self
            .compact_style
            .write()
            .unwrap_or_else(|p| p.into_inner()) = m.style;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "compact_style", &m.style.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<GetCompactStyle> for Agents {
    type Reply = CompactStyle;

    async fn handle(
        &mut self,
        _: GetCompactStyle,
        _: &mut Context<Self, Self::Reply>,
    ) -> CompactStyle {
        *self.compact_style.read().unwrap_or_else(|p| p.into_inner())
    }
}

impl Message<SetTitleModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetTitleModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let resolved = match &m.model {
            Some(id) => Some(self.catalog.resolve(id, &*self.configured)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        *self.title_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        if let Err(e) = runtime::write_config_value(&path, "agent", "title_model", value) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(info)
    }
}

impl Message<GetTitleModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: GetTitleModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.title_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Message<Harvest> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<knowledge::HarvestResult, String>>;

    async fn handle(&mut self, m: Harvest, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let actor = match self
            .actor(m.session)
            .await
            .and_then(|a| a.native().cloned())
        {
            Ok(actor) => actor,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // Off the mailbox for the same reason as `Compact`: the harvest is a
        // model call and may ask us back.
        tokio::spawn(async move {
            tx.send(actor.ask(HarvestNow).await.map_err(|e| e.to_string()));
        });
        reply
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
        self.session(
            Some(id),
            Some((m.parent, &name)),
            m.agent.map(|a| (a, m.cwd)),
        )
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
        self.input(Some(child), None, m.text, Vec::new()).await;
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
                _permit: Some(m.permit),
                timer: Some(timer),
                started_at: Utc::now(),
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
        if self
            .child_turns
            .get(&child)
            .is_some_and(|ct| ct.timer.is_some())
            && let Some(live) = self.sessions.get(&child)
        {
            self.timed_out.insert(child);
            live.actor.tell_cancel().await;
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

impl Agents {
    /// Identity lines of a subagent (design doc 07 §10); `""` for a root session.
    fn peer_context(&self, session: SessionId) -> String {
        match self.sessions.get(&session).and_then(|l| l.parent.as_ref()) {
            Some((parent, name)) => format!(
                "## Peers\nYou are `{name}`, a subagent of session {parent} (address it as `parent`).\nYour final message is delivered to the parent when your turn ends."
            ),
            None => String::new(),
        }
    }

    /// Whether the Claude Code identity line leads the system prompt for `model`
    /// (design doc 05 §2.3): Anthropic namespace with a login in force.
    fn claude_code(&self, model: &Resolved) -> bool {
        model
            .genai
            .split_once("::")
            .is_some_and(|(ns, _)| ns == "anthropic")
            && self.auth.login("anthropic")
    }

    fn skills_dir(&self) -> std::path::PathBuf {
        skills::dir(&self.rt.data_dir)
    }
}

impl Message<PeerContext> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: PeerContext, _: &mut Context<Self, Self::Reply>) -> String {
        self.peer_context(m.session)
    }
}

impl Agents {
    /// [`AgentInfo`] for every descendant of `session`, store rows in
    /// pre-order plus live state from this actor (design doc 19 §2.1).
    async fn agents(&self, session: SessionId) -> Vec<AgentInfo> {
        let rows = match self.store.descendants(session).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(session = %session, "descendants: {e:#}");
                return Vec::new();
            }
        };
        let names: HashMap<SessionId, &str> = rows
            .iter()
            .filter_map(|(_, s)| Some((s.id, s.name.as_deref()?)))
            .collect();
        let mut out = Vec::with_capacity(rows.len());
        for (depth, row) in &rows {
            let id = row.id;
            let parent = row.parent.unwrap_or(session);
            let state = if let Some((_, tool)) = self.approvals.get(&id) {
                AgentState::Approval { tool: tool.clone() }
            } else if let Some((_, from)) = self.waiters.keys().find(|(w, _)| *w == id) {
                let from = if *from == parent {
                    "parent".to_owned()
                } else {
                    names
                        .get(from)
                        .map(|n| (*n).to_owned())
                        .or_else(|| Some(self.sessions.get(from)?.parent.as_ref()?.1.clone()))
                        .unwrap_or_else(|| from.to_string())
                };
                AgentState::Awaiting { from }
            } else if self.running.contains(&id) {
                AgentState::Running
            } else {
                match self.last_stop.get(&id).map(|(reason, _)| reason.as_str()) {
                    Some("error") => AgentState::Error,
                    Some("cancelled") => AgentState::Cancelled,
                    _ => AgentState::Idle,
                }
            };
            // ponytail: one message replay per row; a `last_text` column if the picker gets slow.
            let last = self.store.last_turn_text(id).await.unwrap_or_else(|e| {
                tracing::error!(session = %id, "last_turn_text: {e:#}");
                String::new()
            });
            out.push(AgentInfo {
                id,
                name: row.name.clone().unwrap_or_default(),
                parent,
                depth: *depth,
                agent: row.agent.clone(),
                model: row.model.clone(),
                state,
                started_at: self.child_turns.get(&id).map(|ct| ct.started_at),
                finished_at: self.last_stop.get(&id).map(|(_, at)| *at),
                last: first_line(&last),
            });
        }
        out
    }
}

/// First non-empty line of `text`, trimmed, cut to 80 chars with `…`.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() > 80 {
        let mut s: String = line.chars().take(79).collect();
        s.push('…');
        s
    } else {
        line.to_owned()
    }
}

impl Message<ListAgents> for Agents {
    type Reply = Vec<AgentInfo>;

    async fn handle(&mut self, m: ListAgents, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.agents(m.session).await
    }
}

impl Message<Roster> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: Roster, _: &mut Context<Self, Self::Reply>) -> String {
        let agents = self.agents(m.session).await;
        if agents.is_empty() {
            return String::new();
        }
        let mut s = String::from("Subagents: ");
        for (i, a) in agents.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&a.name);
            s.push_str(" (");
            s.push_str(a.state.word());
            s.push(')');
        }
        s
    }
}

impl Message<ShowPrompt> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: ShowPrompt, _: &mut Context<Self, Self::Reply>) -> String {
        let peers = m.session.map(|s| self.peer_context(s)).unwrap_or_default();
        let (model, _) = self.resolve_model(&self.default_model);
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten();
        let mut blocks = String::new();
        if let Some(reg) = &registry
            && let Ok(b) = reg.ask(ListPrompts).await
        {
            blocks = b.join("\n\n");
        }
        let mut prompt =
            frame::baseline(&self.rt.data_dir, &peers, &blocks, self.claude_code(&model));
        if self.pool.is_some()
            && let Some(reg) = registry
            && let Ok(specs) = reg.ask(ListTools(m.session)).await
        {
            prompt.push_str("\n\n");
            prompt.push_str(&repl::prompt_block(&specs));
        }
        prompt
    }
}

impl Message<ListSkills> for Agents {
    type Reply = Vec<SkillMeta>;

    async fn handle(
        &mut self,
        _: ListSkills,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<SkillMeta> {
        skills::index(&self.skills_dir())
    }
}

impl Message<LoadSkill> for Agents {
    type Reply = Result<SkillBody, String>;

    async fn handle(
        &mut self,
        m: LoadSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SkillBody, String> {
        skills::load(&self.skills_dir(), &m.name)
    }
}

impl Message<AddSkill> for Agents {
    type Reply = Result<SkillMeta, String>;

    async fn handle(
        &mut self,
        m: AddSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SkillMeta, String> {
        skills::add(&self.skills_dir(), m)
    }
}

impl Message<RemoveSkill> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        skills::remove(&self.skills_dir(), &m.name)
    }
}

impl Message<AddSkillRepo> for Agents {
    type Reply = Result<Vec<SkillMeta>, String>;

    async fn handle(
        &mut self,
        m: AddSkillRepo,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Vec<SkillMeta>, String> {
        skills::add_repo(&self.skills_dir(), &m.url, m.name).await
    }
}

impl Message<RemoveSkillRepo> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveSkillRepo,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        skills::remove_repo(&self.skills_dir(), &m.name)
    }
}

impl Message<SyncSkills> for Agents {
    type Reply = Vec<tools::git::SyncResult>;

    async fn handle(
        &mut self,
        _: SyncSkills,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<tools::git::SyncResult> {
        tools::git::sync_all(&self.skills_dir()).await
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
    type Reply = Loaded;

    async fn handle(&mut self, m: LoadSession, _: &mut Context<Self, Self::Reply>) -> Loaded {
        let store = &self.store;
        let loaded = async {
            anyhow::Ok((
                store.session(m.session).await?,
                store.messages(m.session).await?,
                store.epochs(m.session).await?,
            ))
        }
        .await;
        let (row, history) = match loaded {
            Ok((row, messages, epochs)) => {
                // A boundary sits right after the last entry completed at or
                // before `at_seq`, i.e. before the first entry past it.
                let mut epochs = epochs.into_iter().peekable();
                let mut out = Vec::new();
                for (seq, entry) in render(&messages) {
                    while let Some(e) = epochs.next_if(|e| e.at_seq < seq) {
                        out.push(compacted(e));
                    }
                    out.push(entry);
                }
                out.extend(epochs.map(compacted));
                (row, out)
            }
            Err(e) => {
                tracing::error!(session = %m.session, "load session: {e:#}");
                (None, Vec::new())
            }
        };
        let (running, queued) = match self.live(m.session) {
            Some(actor) => (
                actor.is_running().await,
                match actor.native() {
                    Ok(a) => a.ask(QueuedInputs).await.unwrap_or_default(),
                    Err(_) => Vec::new(),
                },
            ),
            None => (false, Vec::new()),
        };
        let undone = self
            .store
            .hidden_turns(m.session)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(session = %m.session, "hidden_turns: {e:#}");
                0
            });
        let (model, effort, agent, name, parent) = row
            .map(|r| (r.model, r.effort, r.agent, r.name, r.parent))
            .unwrap_or_default();
        Loaded {
            history,
            running,
            model,
            effort,
            queued,
            undone,
            agent,
            name,
            parent,
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
        // Cursor is not in models.dev: ask the backend what the account may use.
        if self.auth.login("cursor") {
            match self.auth.oauth.access(&oauth::CURSOR).await {
                Ok(t) => match crate::backend::cursor_models(&t.access_token).await {
                    Ok(models) => out.extend(models.into_iter().map(|m| {
                        let id = format!("cursor/{}", m.id);
                        let near = self.catalog.closest(&id);
                        ModelInfo {
                            context: near.map_or(0, |n| n.limit.context),
                            reasoning: near.is_some_and(|n| n.reasoning),
                            image: near.is_some_and(|n| n.image),
                            efforts: near.map(|n| n.efforts()).unwrap_or_default(),
                            budget_min: near.and_then(|n| n.budget()).map(|b| b.0),
                            budget_max: near.and_then(|n| n.budget()).and_then(|b| b.1),
                            id,
                            name: m.name,
                            provider: "cursor".into(),
                        }
                    })),
                    Err(e) => tracing::warn!("cursor models: {e}"),
                },
                Err(e) => tracing::warn!("{e}"),
            }
        }
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
            let Some(row) = self.store.session(id).await.map_err(|e| e.to_string())? else {
                return Err(format!("unknown session {id}"));
            };
            let actor = self.actor(id).await?.native()?.clone();
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
            // An effort the new model does not take would 400 its next call: drop it.
            if let Some(e) = &row.effort
                && resolved
                    .model
                    .as_ref()
                    .is_some_and(|m| m.validate_effort(e).is_err())
            {
                self.apply_effort(id, &actor, None).await;
            }
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

impl Agents {
    /// Switch `id`'s effort from its next turn, persist it, and announce it.
    async fn apply_effort(&self, id: SessionId, actor: &ActorRef<Session>, effort: Option<String>) {
        let _ = actor
            .tell(SetPendingEffort(effort.as_deref().and_then(effort_of)))
            .await;
        if let Err(e) = self.store.set_effort(id, effort.as_deref()).await {
            tracing::warn!(session = %id, "set_effort: {e:#}");
        }
        self.rt
            .publish(AgentEvent::EffortChanged {
                session: id,
                effort,
            })
            .await;
    }
}

impl Message<SetEffort> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let target = match m.session {
            Some(id) => {
                self.store
                    .session(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown session {id}"))?
                    .model
            }
            None => self.default_model.clone(),
        };
        if let Some(e) = &m.effort {
            let known = self.catalog.resolve(&target, &*self.configured).ok();
            match known.and_then(|r| r.model) {
                Some(model) => model.validate_effort(e)?,
                None if effort_of(e).is_none() => return Err(format!("unknown effort {e}")),
                None => {}
            }
        }
        if let Some(id) = m.session {
            let actor = self.actor(id).await?.native()?.clone();
            self.apply_effort(id, &actor, m.effort.clone()).await;
        }
        self.default_effort = m.effort.clone();
        // Quoted so a bare budget (`"8000"`) stays a YAML string; `~` clears.
        let value = m
            .effort
            .as_ref()
            .map_or_else(|| "~".to_owned(), |e| format!("\"{e}\""));
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "agent", "effort", &value) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<ListLogins> for Agents {
    type Reply = Vec<LoginInfo>;

    async fn handle(
        &mut self,
        _: ListLogins,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<LoginInfo> {
        oauth::PROVIDERS
            .iter()
            .map(|p| {
                let t = self.auth.oauth.load(&p.ns);
                LoginInfo {
                    name: p.ns.to_string(),
                    logged_in: t.is_some(),
                    expires_at: t.as_ref().map(|t| t.expires_at),
                    account: t.and_then(|t| t.account_id),
                }
            })
            .collect()
    }
}

impl Message<Login> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Login, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = login_provider(&m.provider)?;
        let (url, finish) = self.auth.oauth.login(p).await?;
        oauth::open_browser(&url);
        let rt = self.rt.clone();
        tokio::spawn(async move {
            let error = finish.await.err();
            match &error {
                Some(e) => tracing::warn!("login {}: {e}", m.provider),
                None => tracing::info!("login {}: ok", m.provider),
            }
            rt.publish(LoginFinished {
                provider: m.provider,
                error,
            })
            .await;
        });
        Ok(url)
    }
}

impl Message<Logout> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Logout, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = login_provider(&m.provider)?;
        self.auth.oauth.remove(&p.ns).map_err(|e| e.to_string())
    }
}

impl Message<ListProviders> for Agents {
    type Reply = Vec<ProviderInfo>;

    async fn handle(
        &mut self,
        _: ListProviders,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ProviderInfo> {
        let gws = self.auth.gateways.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ProviderInfo> = gws
            .iter()
            .map(|(name, g)| ProviderInfo {
                models: self
                    .catalog
                    .providers()
                    .find(|p| p.id == *name)
                    .map_or(0, |p| p.models.len() as u32),
                name: name.clone(),
                base_url: g.base_url.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

impl Message<AddProvider> for Agents {
    type Reply = Result<ProviderInfo, String>;

    async fn handle(
        &mut self,
        m: AddProvider,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ProviderInfo, String> {
        crate::providers::validate_name(&m.name)?;
        let base_url = crate::providers::normalize_url(&m.base_url)?;
        let key = m.key.trim();
        if key.is_empty() {
            return Err("api key is empty".into());
        }
        let models = match self
            .client
            .all_model_names(
                AdapterKind::OpenAI,
                (
                    Endpoint::from_owned(base_url.clone()),
                    AuthData::Key(key.to_owned()),
                ),
            )
            .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(provider = %m.name, "listing models: {e}");
                Vec::new()
            }
        };
        crate::providers::save(&self.rt.data_dir, &m.name, &base_url, key, &models)?;
        self.auth
            .gateways
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                m.name.clone(),
                crate::providers::Gateway {
                    base_url: base_url.clone(),
                    key: key.to_owned(),
                },
            );
        self.catalog = Arc::new(Catalog::load(&self.rt.data_dir));
        Ok(ProviderInfo {
            name: m.name,
            base_url,
            models: models.len() as u32,
        })
    }
}

impl Message<RemoveProvider> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveProvider,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        crate::providers::remove(&self.rt.data_dir, &m.name)?;
        self.auth
            .gateways
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&m.name);
        self.catalog = Arc::new(Catalog::load(&self.rt.data_dir));
        Ok(())
    }
}

impl Message<NewSession> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: NewSession, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (id, _) = self
            .session(None, None, None)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some(cwd) = &m.cwd {
            self.store
                .set_cwd(id, cwd)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(id)
    }
}

/// One `/acp` row, unprobed.
fn acp_info(name: String, e: &acp::Entry) -> acp::EntryInfo {
    acp::EntryInfo {
        name,
        command: e.command.clone(),
        args: e.args.clone(),
        description: e.description.clone(),
        status: None,
    }
}

impl Message<ListAcp> for Agents {
    type Reply = kameo::reply::DelegatedReply<Vec<acp::EntryInfo>>;

    async fn handle(&mut self, m: ListAcp, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        // Probing spawns every harness (up to 10 s each): off the mailbox.
        tokio::spawn(async move {
            let entries = acp::load(&data_dir).unwrap_or_else(|e| {
                tracing::warn!("acp/agents.yaml: {e:#}");
                Default::default()
            });
            let logs = data_dir.join("logs");
            let logs = &logs;
            let probes = entries.iter().map(|(name, e)| async move {
                let mut info = acp_info(name.clone(), e);
                if m.probe {
                    let r =
                        acp::harness::probe(e.config(), logs.join(format!("acp-{name}.log"))).await;
                    info.status = Some(r.status);
                }
                info
            });
            tx.send(futures::future::join_all(probes).await);
        });
        reply
    }
}

impl Message<AddAcp> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<acp::EntryInfo, String>>;

    async fn handle(&mut self, m: AddAcp, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        // Network + download: off the mailbox.
        tokio::spawn(async move {
            let result = async {
                let agents = acp::registry::fetch(&data_dir)
                    .await
                    .map_err(|e| format!("registry: {e:#}"))?;
                let agent = agents
                    .iter()
                    .find(|a| a.id == m.id)
                    .ok_or_else(|| format!("unknown registry id `{}`", m.id))?;
                let entry = acp::registry::install(&data_dir, agent)
                    .await
                    .map_err(|e| format!("install {}: {e:#}", m.id))?;
                let mut entries = acp::load(&data_dir).map_err(|e| format!("{e:#}"))?;
                entries.insert(m.id.clone(), entry.clone());
                acp::save(&data_dir, &entries).map_err(|e| format!("{e:#}"))?;
                Ok(acp_info(m.id, &entry))
            }
            .await;
            tx.send(result);
        });
        reply
    }
}

impl Message<RemoveAcp> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RemoveAcp, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let data_dir = &self.rt.data_dir;
        let mut entries = acp::load(data_dir).map_err(|e| format!("{e:#}"))?;
        if entries.remove(&m.name).is_none() {
            return Err(acp::unknown(&m.name, &entries));
        }
        acp::save(data_dir, &entries).map_err(|e| format!("{e:#}"))?;
        acp::registry::remove_bin(data_dir, &m.name).map_err(|e| format!("bin/{}: {e}", m.name))
    }
}

impl Message<ListRegistry> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<Vec<acp::registry::RegistryAgent>, String>>;

    async fn handle(
        &mut self,
        _: ListRegistry,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        tokio::spawn(async move {
            tx.send(
                acp::registry::fetch(&data_dir)
                    .await
                    .map_err(|e| format!("registry: {e:#}")),
            );
        });
        reply
    }
}

/// [`oauth::provider`] with a user-facing error listing the choices.
fn login_provider(ns: &str) -> Result<&'static oauth::Provider, String> {
    oauth::provider(ns).ok_or_else(|| {
        let all: Vec<&str> = oauth::PROVIDERS.iter().map(|p| &*p.ns).collect();
        format!("unknown provider `{ns}`; providers: {}", all.join(", "))
    })
}

impl ModelInfo {
    fn new(provider: &str, m: &models::Model) -> Self {
        let budget = m.budget();
        Self {
            id: format!("{provider}/{}", m.id),
            name: m.name.clone(),
            provider: provider.to_owned(),
            context: m.limit.context,
            reasoning: m.reasoning,
            image: m.image,
            efforts: m.efforts(),
            budget_min: budget.map(|b| b.0),
            budget_max: budget.and_then(|b| b.1),
        }
    }
}

impl From<&Resolved> for ModelInfo {
    fn from(r: &Resolved) -> Self {
        // `r.model` may be a name-matched neighbour: keep the id the user chose.
        let budget = r.model.as_ref().and_then(|m| m.budget());
        Self {
            id: r.id.clone(),
            name: r
                .model
                .as_ref()
                .map_or_else(|| r.id.clone(), |m| m.name.clone()),
            provider: r.provider.clone().unwrap_or_default(),
            context: r.model.as_ref().map_or(0, |m| m.limit.context),
            reasoning: r.model.as_ref().is_some_and(|m| m.reasoning),
            image: r.model.as_ref().is_some_and(|m| m.image),
            efforts: r.model.as_ref().map(|m| m.efforts()).unwrap_or_default(),
            budget_min: budget.map(|b| b.0),
            budget_max: budget.and_then(|b| b.1),
        }
    }
}

/// Collapse stored messages into UI entries tagged with the seq of the
/// message that completed them; a tool call and its response become one
/// `Tool` entry (tagged with the response's seq).
fn render(messages: &[(i64, ChatMessage)]) -> Vec<(i64, HistoryEntry)> {
    let mut out = Vec::new();
    let mut pending: HashMap<&str, (&str, &serde_json::Value)> = HashMap::new();
    for (seq, m) in messages {
        match m.role {
            ChatRole::User => out.push((
                *seq,
                HistoryEntry::User {
                    text: text_of(&m.content),
                },
            )),
            ChatRole::Assistant => {
                let text = text_of(&m.content);
                if !text.is_empty() {
                    out.push((*seq, HistoryEntry::Assistant { text }));
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
                    out.push((
                        *seq,
                        HistoryEntry::Tool {
                            name: name.to_string(),
                            args: args.clone(),
                            is_error: r.content.starts_with("ERROR: "),
                            output: r.content.clone(),
                        },
                    ));
                }
            }
            ChatRole::System => {}
        }
    }
    out
}

/// The UI divider for one epoch: the head text without its
/// `<context_summary …>` wrapper (design doc 07 §8.4).
fn compacted(e: Epoch) -> HistoryEntry {
    let text = text_of(&e.head.content);
    let style = crate::compact::style_of_head(&text);
    let summary = text
        .strip_prefix("<context_summary")
        .and_then(|s| s.split_once('>'))
        .and_then(|(_, s)| s.strip_suffix("</context_summary>"))
        .map_or(text.as_str(), str::trim)
        .to_owned();
    HistoryEntry::Compacted {
        epoch: e.n,
        summary,
        style,
    }
}

/// Key sources in precedence order (design doc 05 §2.2): `credentials.yaml`,
/// then a `/login`, then the adapter's env var (left to genai).
#[derive(Clone)]
pub(crate) struct Auth {
    pub(crate) creds: Arc<HashMap<String, String>>,
    pub(crate) oauth: oauth::Store,
    /// Named OpenAI-compatible gateways; interior-mutable so `/provider add` is live.
    pub(crate) gateways: Arc<RwLock<HashMap<String, crate::providers::Gateway>>>,
}

impl Auth {
    /// A login is in force for genai namespace `ns`: no key in
    /// `credentials.yaml` and tokens on disk.
    pub(crate) fn login(&self, ns: &str) -> bool {
        !self.creds.contains_key(ns)
            && oauth::provider(ns).is_some()
            && self.oauth.load(ns).is_some()
    }

    /// OpenAI-compatible gateway for user-facing provider id `ns`.
    pub(crate) fn gateway(&self, ns: &str) -> Option<crate::providers::Gateway> {
        self.gateways.read().ok()?.get(ns).cloned()
    }
}

/// Largest attachment a provider accepts (17 §1: Anthropic's 5 MB is the binding cap).
const MAX_ATTACHMENT: u64 = 5 * 1024 * 1024;

/// Rejects what a provider would (17 §3.1): missing/non-regular file,
/// non-`image/*` mime, over 5 MB.
fn check_files(files: &[PathBuf]) -> Result<(), String> {
    for f in files {
        let name = f.file_name().unwrap_or(f.as_os_str()).to_string_lossy();
        let meta = match std::fs::metadata(f) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Err(format!("attachment {name}: not a regular file")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("attachment {name}: not found"));
            }
            Err(e) => return Err(format!("attachment {name}: {e}")),
        };
        let mime = mime_guess::from_path(f).first_or_octet_stream();
        if mime.type_() != mime_guess::mime::IMAGE {
            return Err(format!("attachment {name}: {mime} is not an image"));
        }
        if meta.len() > MAX_ATTACHMENT {
            return Err(format!(
                "attachment {name}: {:.1} MB exceeds 5 MB",
                meta.len() as f64 / (1024.0 * 1024.0)
            ));
        }
    }
    Ok(())
}

/// genai client whose keys come from `credentials.yaml`, falling back to each
/// adapter's env var. A login in force (design doc 05 §2.3) rewrites the
/// target instead: OpenAI → the ChatGPT Codex backend; Anthropic → the same
/// Messages API with the Claude Code bearer and betas.
fn client(auth: Auth) -> anyhow::Result<Client> {
    let keys = auth.creds.clone();
    Ok(Client::builder()
        .with_auth_resolver_fn(move |m: ModelIden| {
            Ok(match keys.get(m.adapter_kind.as_lower_str()) {
                Some(k) => Some(AuthData::Key(k.clone())),
                None => AdapterKind::default_key_env_name(&m.adapter_kind).map(AuthData::from_env),
            })
        })
        .with_service_target_resolver(ServiceTargetResolver::from_resolver_async_fn(
            move |mut target: ServiceTarget| -> Pin<
                Box<dyn Future<Output = genai::resolver::Result<ServiceTarget>> + Send>,
            > {
                let auth = auth.clone();
                Box::pin(async move {
                    match target.model.adapter_kind {
                        AdapterKind::OpenAI | AdapterKind::OpenAIResp if auth.login("openai") => {
                            let t = auth
                                .oauth
                                .access(&oauth::OPENAI)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.model.adapter_kind = AdapterKind::OpenAIResp;
                            target.endpoint =
                                Endpoint::from_static("https://chatgpt.com/backend-api/codex/");
                            target.auth = AuthData::RequestOverride {
                                url: "https://chatgpt.com/backend-api/codex/responses".into(),
                                headers: Headers::from(vec![
                                    ("Authorization", format!("Bearer {}", t.access_token)),
                                    ("chatgpt-account-id", t.account_id.unwrap_or_default()),
                                    ("OpenAI-Beta", "responses=v1".to_owned()),
                                    ("originator", "codex_cli_rs".to_owned()),
                                ]),
                            };
                        }
                        AdapterKind::Anthropic if auth.login("anthropic") => {
                            let t = auth
                                .oauth
                                .access(&oauth::ANTHROPIC)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.auth = AuthData::RequestOverride {
                                url: "https://api.anthropic.com/v1/messages?beta=true".into(),
                                headers: Headers::from(vec![
                                    ("Authorization", format!("Bearer {}", t.access_token)),
                                    ("anthropic-version", "2023-06-01".to_owned()),
                                    ("anthropic-beta", ANTHROPIC_BETAS.to_owned()),
                                    ("User-Agent", CLAUDE_CODE_UA.to_owned()),
                                    ("x-app", "cli".to_owned()),
                                    (
                                        "anthropic-dangerous-direct-browser-access",
                                        "true".to_owned(),
                                    ),
                                ]),
                            };
                        }
                        AdapterKind::Xai if auth.login("xai") => {
                            let t = auth
                                .oauth
                                .access(&oauth::XAI)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.auth = AuthData::Key(t.access_token);
                        }
                        _ => {}
                    }
                    Ok(target)
                })
            },
        ))
        .build()?)
}

/// Betas a subscription token is accepted with; `oauth-2025-04-20` is the
/// one that turns a 403 into a 200.
const ANTHROPIC_BETAS: &str =
    "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14";
/// The client the subscription token was minted for.
const CLAUDE_CODE_UA: &str = "claude-cli/2.1.257 (external, cli)";

/// The auth-resolver rule as a predicate over genai namespaces: a key in
/// `creds`, a login on disk, the adapter's default env var set, or an
/// adapter needing no key (`ollama`). `cursor` is not a genai adapter: only
/// a login configures it.
fn configured(auth: Auth) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |ns: &str| {
        auth.creds.contains_key(ns)
            || auth.gateway(ns).is_some()
            || auth.login(ns)
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
    let creds = runtime::credentials(&rt.data_dir);
    let gateways = crate::providers::load(&rt.data_dir, &creds);
    let auth = Auth {
        creds: Arc::new(creds),
        oauth: oauth::Store::new(&rt.data_dir),
        gateways: Arc::new(RwLock::new(gateways)),
    };
    // One worker per concurrent child turn plus the root session (design doc 08 §9).
    // The worker is this binary (08 §9).
    let pool = match std::env::current_exe() {
        Ok(exe) => repl::open_pool(&exe, cfg.subagents.max_concurrent.max(1) + 1).await,
        Err(e) => {
            tracing::warn!("cannot locate own executable ({e}); python runtime disabled");
            None
        }
    };
    spill::sweep(
        &rt.data_dir.join("spill"),
        Duration::from_secs(7 * 24 * 3600),
    );
    let catalog = Arc::new(Catalog::load(&rt.data_dir));
    let configured = configured(auth.clone());
    // Unresolvable → warn and fall back to the session model rather than fail every title.
    let title_model = cfg.title_model.as_deref().and_then(|id| {
        catalog
            .resolve(id, &*configured)
            .map_err(|e| tracing::warn!("agent.title_model: {e}"))
            .ok()
    });
    let agents = Agents::spawn_with_mailbox(
        Agents {
            rt: rt.clone(),
            cfg: cfg.clone(),
            default_model: cfg.model.clone(),
            default_effort: cfg.effort.clone(),
            compact_style: Arc::new(RwLock::new(cfg.compact_style)),
            title_model: Arc::new(RwLock::new(title_model)),
            catalog,
            pool,
            configured,
            store,
            client: client(auth.clone())?,
            auth,
            sessions: HashMap::new(),
            running: HashSet::new(),
            waiters: HashMap::new(),
            permits: Arc::new(Semaphore::new(cfg.subagents.max_concurrent.max(1))),
            child_turns: HashMap::new(),
            timed_out: HashSet::new(),
            last_stop: HashMap::new(),
            approvals: HashMap::new(),
            next_msg: 0,
        },
        runtime::subscriber_mailbox(),
    );
    agents.register(AGENT_ACTOR)?;
    rt.subscribe::<Agents, AgentEvent>(&agents).await;
    rt.subscribe::<Agents, JobEvent>(&agents).await;
    rt.subscribe::<Agents, ApprovalRequested>(&agents).await;
    rt.subscribe::<Agents, ApprovalResolved>(&agents).await;

    // First use of the registry (00 §4, 12 §2).
    let data_dir = rt.data_dir.clone();
    let rt = rt.clone();
    tokio::spawn(async move {
        let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await else {
            return;
        };
        let _ = reg
            .ask(RegisterPrompt {
                plugin: "agent",
                text: format!(
                    "This harness is `wth`; its state is under {d}: config.yaml (every plugin's section: \
                     `agent.model` = default `provider/model`, `tools.policy`, `rpc`, `web`, `knowledge`, `signal`), \
                     credentials.yaml (provider -> API key, mode 0600), prompt.md (standing instructions, read every call), \
                     skills/ (markdown skills; this index), knowledge/, sessions.sqlite, logs/wth.log.<date>. \
                     Load the `harness` skill before editing any of them; it has the full layout and every key. \
                     config.yaml and credentials.yaml are read once at startup (mcp/, skills/ and prompt.md are live); \
                     the user switches the running model with /model.",
                    d = data_dir.display()
                ),
            })
            .await;
        let dir = skills::dir(&data_dir);
        let _ = reg
            .ask(RegisterTool(Arc::new(LoadSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(AddSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(RemoveSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(AddSkillRepoTool {
                dir: dir.clone(),
            })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(RemoveSkillRepoTool {
                dir: dir.clone(),
            })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SyncSkillsTool { dir })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SpawnSubagentTool { data_dir })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SendMessageTool { cfg })))
            .await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_result_wrapper_escapes_and_names_exit() {
        let id = JobId("j3".into());
        let w = job_result(&id, "echo \"a\" <b>", &JobExit::Exited(0), 7, "out");
        assert_eq!(
            w,
            "<job_result id=\"j3\" command=\"echo &quot;a&quot; &lt;b&gt;\" exit=\"0\" secs=\"7\">\nout\n</job_result>"
        );
        let w = job_result(&id, "sleep 9", &JobExit::Timeout, 1800, "");
        assert!(w.contains(" exit=\"timeout\" "));
        let w = job_result(&id, "x", &JobExit::Killed, 1, "");
        assert!(w.contains(" exit=\"killed\" "));
        let long = "a".repeat(200);
        let w = job_result(&id, &long, &JobExit::Exited(1), 0, "");
        assert!(w.contains(&format!(" command=\"{}\" ", "a".repeat(120))));
        assert!(!w.contains(&"a".repeat(121)));
    }

    #[test]
    fn first_line_skips_blanks_and_cuts_by_char() {
        assert_eq!(first_line("\n  \n  hello world  \nmore"), "hello world");
        assert_eq!(first_line(""), "");
        let long = "é".repeat(100);
        let cut = first_line(&long);
        assert_eq!(cut.chars().count(), 80);
        assert!(cut.ends_with('…'));
        assert_eq!(first_line(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn check_files_rejects_what_a_provider_would() {
        let dir = std::env::temp_dir().join(format!("wth-attach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"hi").unwrap();
        let big = dir.join("big.jpg");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_ATTACHMENT + 1)
            .unwrap();

        assert_eq!(check_files(&[]), Ok(()));
        assert_eq!(check_files(std::slice::from_ref(&png)), Ok(()));
        assert_eq!(
            check_files(&[png, txt]).unwrap_err(),
            "attachment notes.txt: text/plain is not an image"
        );
        assert_eq!(
            check_files(&[dir.join("gone.png")]).unwrap_err(),
            "attachment gone.png: not found"
        );
        assert_eq!(
            check_files(std::slice::from_ref(&dir)).unwrap_err(),
            format!(
                "attachment {}: not a regular file",
                dir.file_name().unwrap().to_string_lossy()
            )
        );
        assert_eq!(
            check_files(&[big]).unwrap_err(),
            "attachment big.jpg: 5.0 MB exceeds 5 MB"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
