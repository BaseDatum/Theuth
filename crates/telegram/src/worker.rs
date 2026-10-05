//! NEEDS(RENDER): the per-thread turn renderer (design doc 98 §8–§9). Stage 1
//! lands the contract types and a task that drains its mailbox; RENDER
//! replaces the body of [`spawn`] and keeps every signature here.

use std::sync::Arc;

use agent::AgentEvent;
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use tokio::sync::{mpsc, oneshot};

use crate::ToolsMode;
use crate::api::{Sender, Target};
use crate::port::Port;
use crate::threads::{OpenMsg, ThreadKey, Topic};

/// What the worker renders (98 §12 `tools`, `reasoning`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderConfig {
    /// Activity cards per tool batch.
    pub tools: ToolsMode,
    /// Reasoning text in the expandable quote.
    pub reasoning: bool,
}

/// Voice synthesis and transcription actors, when the voice plugin is up (45 §4.5).
#[derive(Clone, Default)]
pub struct VoiceHandles {
    /// `Synthesize`.
    pub voice: Option<ActorRef<voice::Voice>>,
    /// `Transcribe`.
    pub stt: Option<ActorRef<voice::Transcriber>>,
}

/// One board row (98 §9.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// State glyph.
    pub glyph: &'static str,
    /// Child name.
    pub name: String,
    /// State word.
    pub state: String,
    /// Last tool or message.
    pub detail: String,
    /// Seconds since the last event.
    pub age_secs: u64,
}

/// Everything a worker needs at spawn.
pub struct WorkerCtx {
    /// The thread key this worker renders.
    pub key: ThreadKey,
    /// Where its messages go.
    pub to: Target,
    /// The thread's user.
    pub user: u64,
    /// The session it renders.
    pub session: SessionId,
    /// Bot API.
    pub api: Arc<dyn Sender>,
    /// Agent asks.
    pub port: Arc<dyn Port>,
    /// Runtime (data dir for `inbox/`, shutdown).
    pub rt: Runtime,
    /// Rendering switches.
    pub cfg: RenderConfig,
    /// Voice actors.
    pub voice: VoiceHandles,
    /// The topic, in topics chats.
    pub topic: Option<Topic>,
    /// Worker → channel notes.
    pub notes: mpsc::UnboundedSender<WorkerNote>,
    /// The subagent's name when this worker renders a child session (98 §9).
    pub child_name: Option<String>,
}

/// Channel → worker.
#[derive(Debug)]
pub enum WorkerMsg {
    /// One of this session's events.
    Agent(AgentEvent),
    /// The user's message that started or steered the turn (👀 now, the terminal mark later).
    Mark {
        /// Its id.
        message_id: i64,
        /// It was a voice note (45 §4.5).
        voice: bool,
    },
    /// A Stop press for a draft this worker owns.
    Stopped {
        /// The draft.
        draft_id: i64,
    },
    /// A descendant's board row changed.
    ChildRow {
        /// The child.
        child: SessionId,
        /// Its row.
        row: Row,
        /// The child is idle.
        idle: bool,
    },
    /// The user renamed the topic: stop following the title.
    TopicNamedByUser(bool),
    /// `out:<call_id>` press: the full tool output this worker holds, `None` when expired (98 §8.4).
    Output {
        /// The tool call.
        call_id: String,
        /// Answer here.
        reply: oneshot::Sender<Option<String>>,
    },
    /// Thread unbound or shutdown: finish and exit.
    Shutdown,
}

/// Worker → channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerNote {
    /// An edit-mode message entered (`open`) or left flight (98 §11).
    OpenMsg {
        /// The message.
        msg: OpenMsg,
        /// In flight.
        open: bool,
    },
    /// The topic was renamed after the session title.
    Renamed {
        /// Thread.
        key: ThreadKey,
        /// New title.
        title: String,
    },
    /// A bubble left Draft mode or rich → plain (98 §8.3).
    Fallback {
        /// Session.
        session: SessionId,
        /// Why.
        reason: String,
    },
    /// The chat cannot be written to (98 §4.3).
    Unreachable {
        /// Chat.
        chat: i64,
        /// Why.
        reason: String,
    },
}

/// Spawns the worker task; the returned sender is its mailbox.
// NEEDS(RENDER): the real renderer. Stage 1 drains and logs.
pub fn spawn(ctx: WorkerCtx) -> mpsc::UnboundedSender<WorkerMsg> {
    let (tx, mut rx) = mpsc::unbounded_channel::<WorkerMsg>();
    let key = ctx.key.clone();
    let session = ctx.session;
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                WorkerMsg::Shutdown => break,
                WorkerMsg::Output { reply, .. } => {
                    let _ = reply.send(None);
                }
                other => {
                    tracing::debug!(%key, %session, msg = ?other, "telegram worker stub: dropped")
                }
            }
        }
    });
    tx
}
