//! One session's model loop: stream, dispatch tool calls, steer, cancel.
//!
//! The [`Session`] actor owns the history between turns; a running turn owns
//! it inside a spawned task and hands it back via [`TurnDone`], so the actor's
//! mailbox never blocks on network I/O.

use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::Client;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, MessageContent, ToolCall, ToolResponse,
};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use models::Resolved;
use runtime::{Runtime, SessionId, TurnId};
use tokio_util::sync::CancellationToken;
use tools::{Invoke, ListTools, REGISTRY_ACTOR, ToolRegistry};

use crate::peers::SpawnSubagentTool;
use crate::skills;
use crate::store::Store;
use crate::supervisor::{Auth, PeerContext};
use crate::{AGENT_ACTOR, AgentConfig, AgentEvent, Agents, Usage};

/// Characters of tool output shown in [`AgentEvent::ToolCallFinished`].
const DISPLAY_OUTPUT_CHARS: usize = 2000;
/// Subscription tokens are only honoured when the system prompt opens with
/// the client's identity line (design doc 05 §2.3).
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// One model call's events, from genai or the Cursor backend.
type EventStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, String>> + Send>>;

/// `tell`: user text. Starts a turn when idle, otherwise queued and injected
/// after the current tool batch (steering).
pub struct UserInput(pub String);

/// `tell`: abort the in-flight turn, if any.
pub struct CancelTurn;

/// `ask` → whether a turn is in flight.
pub struct IsRunning;

/// `tell`: switch the model; takes effect on the next model call, mid-turn included.
pub struct SwitchModel(pub Resolved);

/// `tell` (from the turn task to its own session): the turn ended.
pub struct TurnDone(Vec<ChatMessage>);

/// One conversation. Spawn with `Session::spawn(session)`.
#[derive(kameo::Actor)]
pub struct Session {
    /// Everything a turn task needs, cloned per turn.
    shared: Shared,
    /// Messages so far; taken by the running turn and returned in [`TurnDone`].
    history: Vec<ChatMessage>,
    /// Cancels the in-flight turn; `None` when idle.
    inflight: Option<CancellationToken>,
    /// Whether `sessions.title` is set.
    titled: bool,
}

/// Turn-invariant state; cheap to clone.
#[derive(Clone)]
struct Shared {
    id: SessionId,
    depth: u8,
    /// Current model plus the resolution error of the configured id, if any;
    /// an error makes the next round stop instead of calling genai. Shared
    /// with the running turn so [`SwitchModel`] applies mid-turn.
    model: Arc<Mutex<(Resolved, Option<String>)>>,
    cfg: Arc<AgentConfig>,
    store: Store,
    client: Client,
    auth: Auth,
    rt: Runtime,
    steer: Arc<Mutex<Vec<String>>>,
}

impl Session {
    /// Build a session; `history` is empty for new sessions and the stored
    /// messages for resumed ones. `depth` ≥ `subagents.max_depth` hides `spawn_subagent`.
    /// `model` is the resolved id and, when resolution failed, the message to surface.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: SessionId,
        depth: u8,
        model: (Resolved, Option<String>),
        history: Vec<ChatMessage>,
        cfg: Arc<AgentConfig>,
        store: Store,
        client: Client,
        auth: Auth,
        rt: Runtime,
    ) -> Self {
        let titled = !history.is_empty();
        Self {
            shared: Shared {
                id,
                depth,
                model: Arc::new(Mutex::new(model)),
                cfg,
                store,
                client,
                auth,
                rt,
                steer: Arc::default(),
            },
            history,
            inflight: None,
            titled,
        }
    }

    async fn start_turn(&mut self, inputs: Vec<String>, me: &ActorRef<Self>) {
        let turn = TurnId::new();
        let cancel = self.shared.rt.shutdown.child_token();
        self.inflight = Some(cancel.clone());
        if !self.titled {
            self.titled = true;
            if let Some(first) = inputs.first()
                && let Err(e) = self
                    .shared
                    .store
                    .set_title(
                        self.shared.id,
                        &first.trim().chars().take(60).collect::<String>(),
                    )
                    .await
            {
                tracing::warn!(session = %self.shared.id, "set_title: {e:#}");
            }
        }
        let model = self
            .shared
            .model
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .id
            .clone();
        self.shared
            .rt
            .publish(AgentEvent::TurnStarted {
                session: self.shared.id,
                turn,
                text: inputs.join("\n"),
                model,
            })
            .await;
        let task = Turn {
            shared: self.shared.clone(),
            turn,
            cancel,
            peers: String::new(),
        };
        let history = std::mem::take(&mut self.history);
        let me = me.clone();
        tokio::spawn(async move {
            let history = task.run(history, inputs).await;
            let _ = me.tell(TurnDone(history)).await;
        });
    }
}

impl Message<UserInput> for Session {
    type Reply = ();

    async fn handle(&mut self, UserInput(text): UserInput, ctx: &mut Context<Self, Self::Reply>) {
        if self.inflight.is_some() {
            self.shared
                .steer
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(text);
        } else {
            let me = ctx.actor_ref().clone();
            self.start_turn(vec![text], &me).await;
        }
    }
}

impl Message<CancelTurn> for Session {
    type Reply = ();

    async fn handle(&mut self, _: CancelTurn, _: &mut Context<Self, Self::Reply>) {
        if let Some(c) = &self.inflight {
            c.cancel();
        }
    }
}

impl Message<IsRunning> for Session {
    type Reply = bool;

    async fn handle(&mut self, _: IsRunning, _: &mut Context<Self, Self::Reply>) -> bool {
        self.inflight.is_some()
    }
}

impl Message<SwitchModel> for Session {
    type Reply = ();

    async fn handle(
        &mut self,
        SwitchModel(model): SwitchModel,
        _: &mut Context<Self, Self::Reply>,
    ) {
        *self.shared.model.lock().unwrap_or_else(|p| p.into_inner()) = (model, None);
    }
}

impl Message<TurnDone> for Session {
    type Reply = ();

    async fn handle(&mut self, TurnDone(history): TurnDone, ctx: &mut Context<Self, Self::Reply>) {
        self.history = history;
        self.inflight = None;
        let queued =
            std::mem::take(&mut *self.shared.steer.lock().unwrap_or_else(|p| p.into_inner()));
        if !queued.is_empty() {
            let me = ctx.actor_ref().clone();
            self.start_turn(queued, &me).await;
        }
    }
}

/// One turn: model call, tool batch, steer injection, repeat.
struct Turn {
    shared: Shared,
    turn: TurnId,
    cancel: CancellationToken,
    /// §2.5 prompt block, fetched once per turn.
    peers: String,
}

impl Turn {
    async fn publish(&self, event: AgentEvent) {
        self.shared.rt.publish(event).await;
    }

    async fn error(&self, message: String) {
        tracing::warn!(session = %self.shared.id, turn = %self.turn, "{message}");
        self.publish(AgentEvent::Error {
            session: self.shared.id,
            turn: Some(self.turn),
            message,
        })
        .await;
    }

    async fn persist(&self, history: &mut Vec<ChatMessage>, msg: ChatMessage) {
        if let Err(e) = self.shared.store.append(self.shared.id, &msg).await {
            self.error(format!("store: {e:#}")).await;
        }
        history.push(msg);
    }

    /// Runs to completion (or cancel/error) and returns the history to hand back.
    async fn run(mut self, mut history: Vec<ChatMessage>, inputs: Vec<String>) -> Vec<ChatMessage> {
        for text in inputs {
            self.persist(&mut history, ChatMessage::user(text)).await;
        }
        if let Ok(Some(agents)) = ActorRef::<Agents>::lookup(AGENT_ACTOR) {
            self.peers = agents
                .ask(PeerContext {
                    session: self.shared.id,
                })
                .await
                .unwrap_or_default();
        }
        let mut usage = Usage::default();
        let mut context = 0u32;
        let mut rounds = 0u32;
        let stop_reason = loop {
            match self.round(&mut history, &mut usage, &mut context).await {
                Round::Tools => {
                    rounds += 1;
                    if rounds >= self.shared.cfg.max_tool_rounds {
                        self.error(format!("stopped after {rounds} tool rounds"))
                            .await;
                        break "max_tool_rounds".to_string();
                    }
                    let queued = std::mem::take(
                        &mut *self.shared.steer.lock().unwrap_or_else(|p| p.into_inner()),
                    );
                    for text in queued {
                        self.persist(&mut history, ChatMessage::user(text)).await;
                    }
                }
                Round::Stop(reason) => break reason,
            }
        };
        self.publish(AgentEvent::TurnFinished {
            session: self.shared.id,
            turn: self.turn,
            stop_reason,
            usage,
            context,
        })
        .await;
        history
    }

    /// One model call plus its tool batch. `context` receives this call's
    /// prompt + completion tokens.
    async fn round(
        &self,
        history: &mut Vec<ChatMessage>,
        usage: &mut Usage,
        context: &mut u32,
    ) -> Round {
        let (model, error) = self
            .shared
            .model
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(e) = error {
            self.error(format!("model: {e}")).await;
            return Round::Stop("error".into());
        }
        let Some(registry) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten()
        else {
            self.error("tool registry not running".into()).await;
            return Round::Stop("error".into());
        };
        let tools = match registry.ask(ListTools).await {
            Ok(specs) => specs
                .into_iter()
                .filter(|s| {
                    s.name != <SpawnSubagentTool as tools::Tool>::NAME
                        || self.shared.depth < self.shared.cfg.subagents.max_depth
                })
                .map(|s| {
                    genai::chat::Tool::new(s.name)
                        .with_description(s.description)
                        .with_schema(s.schema)
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                self.error(format!("list tools: {e}")).await;
                Vec::new()
            }
        };
        // The namespace picks the transport: genai for every adapter it knows,
        // the `cursor` crate for a Cursor login.
        let ns = model.genai.split_once("::").map_or("", |(n, _)| n);
        let mut system = system_prompt(&self.shared.rt.data_dir, &self.peers);
        if ns == "anthropic" && self.shared.auth.login("anthropic") {
            system = format!("{CLAUDE_CODE_IDENTITY}\n\n{system}");
        }
        let req = ChatRequest::new(history.clone())
            .with_system(system)
            .with_tools(tools);
        let opts = ChatOptions::default()
            .with_capture_tool_calls(true)
            .with_capture_usage(true)
            .with_capture_content(true);

        let open = async {
            if ns == "cursor" {
                let t = self.shared.auth.oauth.access(&oauth::CURSOR).await?;
                let slug = model
                    .id
                    .split_once('/')
                    .map_or(model.id.as_str(), |(_, m)| m);
                cursor::chat_stream(&t.access_token, slug, req, &self.shared.id.to_string()).await
            } else {
                self.shared
                    .client
                    .exec_chat_stream(&model.genai, req, Some(&opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(|ev| ev.map_err(|e| e.to_string())))
                    })
                    .map_err(|e| e.to_string())
            }
        };
        let stream = tokio::select! {
            _ = self.cancel.cancelled() => return Round::Stop("cancelled".into()),
            r = open => match r {
                Ok(s) => s,
                Err(e) => {
                    self.error(format!("model: {e}")).await;
                    return Round::Stop("error".into());
                }
            },
        };
        let mut stream = stream;
        let mut pending: Vec<String> = Vec::new();
        let end = loop {
            let ev = tokio::select! {
                _ = self.cancel.cancelled() => return Round::Stop("cancelled".into()),
                ev = stream.next() => ev,
            };
            match ev {
                None => {
                    self.error("stream ended without End event".into()).await;
                    return Round::Stop("error".into());
                }
                Some(Err(e)) => {
                    self.error(format!("model: {e}")).await;
                    return Round::Stop("error".into());
                }
                Some(Ok(ChatStreamEvent::Chunk(c))) => {
                    self.publish(AgentEvent::TextDelta {
                        session: self.shared.id,
                        turn: self.turn,
                        text: c.content.into(),
                    })
                    .await
                }
                Some(Ok(ChatStreamEvent::ReasoningChunk(c))) => {
                    self.publish(AgentEvent::ReasoningDelta {
                        session: self.shared.id,
                        turn: self.turn,
                        text: c.content.into(),
                    })
                    .await
                }
                // Arguments arrive whole only at End (ToolCallStarted is
                // published at dispatch); the first chunk names the tool.
                Some(Ok(ChatStreamEvent::ToolCallChunk(c))) => {
                    let tc = c.tool_call;
                    if !tc.fn_name.is_empty() && !pending.contains(&tc.call_id) {
                        pending.push(tc.call_id.clone());
                        self.publish(AgentEvent::ToolCallPending {
                            session: self.shared.id,
                            turn: self.turn,
                            call_id: tc.call_id,
                            name: tc.fn_name,
                        })
                        .await;
                    }
                }
                Some(Ok(
                    ChatStreamEvent::Start
                    | ChatStreamEvent::Heartbeat
                    | ChatStreamEvent::ThoughtSignatureChunk(_),
                )) => {}
                Some(Ok(ChatStreamEvent::End(end))) => break end,
            }
        };

        if let Some(u) = &end.captured_usage {
            let prompt = u.prompt_tokens.unwrap_or(0).max(0) as u32;
            let completion = u.completion_tokens.unwrap_or(0).max(0) as u32;
            usage.prompt += prompt;
            usage.completion += completion;
            *context = prompt + completion;
        }
        let stop = end
            .captured_stop_reason
            .as_ref()
            .map(|s| s.raw().to_string())
            .unwrap_or_else(|| "end_turn".into());
        let calls: Vec<ToolCall> = end
            .captured_tool_calls()
            .map(|v| v.into_iter().cloned().collect())
            .unwrap_or_default();
        if calls.is_empty() {
            if let Some(content) = end.captured_content.filter(|c| !c.is_empty()) {
                self.persist(history, ChatMessage::assistant(content)).await;
            }
            return Round::Stop(stop);
        }
        let assistant = end
            .into_assistant_message_for_tool_use()
            .unwrap_or_else(|| ChatMessage::from(calls.clone()));
        self.persist(history, assistant).await;

        for call in &calls {
            self.publish(AgentEvent::ToolCallStarted {
                session: self.shared.id,
                turn: self.turn,
                call_id: call.call_id.clone(),
                name: call.fn_name.clone(),
                args: call.fn_arguments.clone(),
            })
            .await;
        }
        let outputs = tokio::select! {
            _ = self.cancel.cancelled() => {
                let responses = calls.iter().map(|c| ToolResponse::new(c.call_id.clone(), "cancelled by user")).collect::<Vec<_>>();
                self.persist(history, ChatMessage::from(responses)).await;
                return Round::Stop("cancelled".into());
            }
            outs = join_all(calls.iter().map(|c| {
                registry.ask(Invoke(tools::ToolCall {
                    id: c.call_id.clone(),
                    name: c.fn_name.clone(),
                    args: c.fn_arguments.clone(),
                    session: self.shared.id,
                })).send()
            })) => outs,
        };
        let mut responses = Vec::with_capacity(calls.len());
        for (call, out) in calls.iter().zip(outputs) {
            let out = out.unwrap_or_else(|e| tools::ToolOutput::err(format!("tool registry: {e}")));
            self.publish(AgentEvent::ToolCallFinished {
                session: self.shared.id,
                turn: self.turn,
                call_id: call.call_id.clone(),
                output: truncate(&out.content, DISPLAY_OUTPUT_CHARS),
                is_error: out.is_error,
            })
            .await;
            let content = truncate(&out.content, self.shared.cfg.max_tool_output_chars);
            let content = if out.is_error {
                format!("ERROR: {content}")
            } else {
                content
            };
            responses.push(ToolResponse::new(call.call_id.clone(), content));
        }
        // One tool message carrying every response (genai: `ChatMessage::from(Vec<ToolResponse>)`).
        self.persist(history, ChatMessage::from(responses)).await;
        Round::Tools
    }
}

/// The system prompt (design doc 04 §1.1), rebuilt from disk per call:
/// built-in header, `<data_dir>/prompt.md`, `always` skill bodies, skills
/// index, `peers`; empty parts skipped, one blank line between the rest.
pub(crate) fn system_prompt(data_dir: &Path, peers: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let header = format!(
        "You are a fast, direct assistant that does things for the user on their machine.\nWorking directory: {cwd}\nDate: {}",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let standing = std::fs::read_to_string(data_dir.join("prompt.md")).unwrap_or_default();
    let skills = skills::index(&skills::dir(data_dir));
    [
        header.as_str(),
        &standing,
        &skills::always_bodies(&skills),
        &skills::prompt_index(&skills),
        peers,
    ]
    .iter()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// Outcome of one model call.
enum Round {
    /// Tool calls ran; call the model again.
    Tools,
    /// The turn is over with this stop reason.
    Stop(String),
}

/// Char-boundary-safe prefix with a marker when cut.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…[truncated {} chars]", &s[..i], s.len() - i),
        None => s.to_string(),
    }
}

/// Text parts of a message joined, for history rendering.
pub(crate) fn text_of(content: &MessageContent) -> String {
    content.texts().join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_parts_in_order_with_blank_lines() {
        let data_dir = std::env::temp_dir().join(format!("wth-prompt-{}", uuid::Uuid::new_v4()));
        let skills_dir = skills::dir(&data_dir);
        std::fs::create_dir_all(&skills_dir).unwrap();
        let bare = system_prompt(&data_dir, "");
        assert!(bare.starts_with("You are a fast"));
        assert!(!bare.contains("## Skills"));

        std::fs::write(data_dir.join("prompt.md"), "Standing rule.\n\n").unwrap();
        std::fs::write(
            skills_dir.join("rules.md"),
            "---\nalways: true\n---\nAlways rule.\n",
        )
        .unwrap();
        std::fs::write(skills_dir.join("later.md"), "Listed skill\n").unwrap();
        let full = system_prompt(&data_dir, "## Peers\npeer text");
        std::fs::remove_dir_all(&data_dir).unwrap();

        let pos = |needle: &str| {
            full.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} in {full}"))
        };
        assert!(pos("Date: ") < pos("\n\nStanding rule.\n\nAlways rule.\n\n## Skills\n"));
        assert!(pos("- later: Listed skill\n\n## Peers\npeer text") > pos("## Skills"));
        assert!(!full.contains("rules"));
        assert!(full.ends_with("peer text"));
    }
}
