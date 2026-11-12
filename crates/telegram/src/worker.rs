//! The per-thread turn renderer (design doc 98 §8–§9): one task per bound
//! thread turns this session's `AgentEvent`s into drafts, bubbles, cards,
//! reactions, the board and the topic title. Every outbound call goes through
//! `ctx.api`; the pacer inside `Api` is the only cross-thread coupling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::AgentEvent;
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::ToolsMode;
use crate::api::{MsgRef, OutFile, Sender, Target};
use crate::board::Board;
use crate::bubble::{Bubble, clip, footer, footer_line, report, send_painted, send_rich_or_plain};
use crate::cards::Card;
use crate::port::Port;
use crate::threads::{OpenMsg, ThreadKey, Topic};

/// What the worker renders (98 §12 `tools`, `reasoning`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderConfig {
    /// Activity cards per tool batch.
    pub tools: ToolsMode,
    /// Reasoning text in the expandable quote.
    pub reasoning: bool,
    /// Picture fences as PNGs (164 D10); `false` = D7 text.
    pub pictures: bool,
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
    /// One of this session's events (boxed: `AgentEvent` dwarfs the other variants).
    Agent(Box<AgentEvent>),
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

/// The flush tick.
pub const TICK: Duration = Duration::from_secs(1);
/// `typing` while the turn runs and nothing else was sent for this long (98 §8.7).
pub const TYPING_GAP: Duration = Duration::from_secs(4);
/// Tool outputs are kept for `out:` presses this long (98 §8.4).
pub const OUTPUT_TTL: Duration = Duration::from_secs(3600);
/// `MessageDelivered` body cap (98 §8.9).
pub const DELIVERED_MAX: usize = 32_000;
/// `editForumTopic` name cap.
pub const TITLE_MAX: usize = 128;

/// Reactions on the user's message (98 §8.7): accepted, done, cancelled, failed.
pub const REACTIONS: [(&str, &str); 4] = [
    ("accepted", "👀"),
    ("done", "👌"),
    ("cancelled", "🤷"),
    ("failed", "😱"),
];

fn reaction(moment: &str) -> &'static str {
    REACTIONS
        .iter()
        .find(|(m, _)| *m == moment)
        .map_or("👀", |(_, e)| e)
}

/// `**name** · ` on the first line of a child's messages in a flat chat (98 §9.2).
// NEEDS(ROUTE): reply-threading of a child's messages to its marker (98 §9.2) — ROUTE owns the marker ids.
#[must_use]
pub fn brand(ctx: &WorkerCtx, md: &str) -> String {
    match &ctx.child_name {
        Some(name) => format!("**{name}** · {md}"),
        None => md.to_owned(),
    }
}

/// `s` cut to `max` chars, the last one an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some(_) => {
            let cut = s.char_indices().nth(max - 1).map_or(0, |(i, _)| i);
            format!("{}…", &s[..cut])
        }
        None => s.to_owned(),
    }
}

/// `TurnStarted..TurnFinished` (98 §8).
struct Turn {
    started: Instant,
    model: String,
    voice: bool,
    clip_tail: Option<JoinHandle<()>>,
    bubble: Option<Bubble>,
    card: Option<Card>,
    draft_seq: i64,
    tools: u32,
    /// Markdown streamed since the last clip (45 §4.5).
    unspoken: String,
    /// Tool names by call id (`ToolCallFinished` carries none).
    names: HashMap<String, String>,
}

impl Turn {
    fn new(model: String, voice: bool) -> Self {
        Self {
            started: Instant::now(),
            model,
            voice,
            clip_tail: None,
            bubble: None,
            card: None,
            draft_seq: 0,
            tools: 0,
            unspoken: String::new(),
            names: HashMap::new(),
        }
    }

    /// The open bubble, opened now if there is none.
    fn bubble(&mut self, ctx: &WorkerCtx) -> &mut Bubble {
        let seq = &mut self.draft_seq;
        self.bubble.get_or_insert_with(|| {
            *seq += 1;
            Bubble::open(ctx, *seq)
        })
    }

    fn end_reasoning(&mut self, ms: Option<u64>) {
        if let Some(c) = &mut self.card {
            c.end_reasoning(ms);
        }
        if let Some(b) = &mut self.bubble {
            b.end_reasoning(ms);
        }
    }

    /// End of a prose block on a voice turn: synthesise it and send the clip
    /// after the previous one (45 §4.5.4). Picture fences are never read (164 D9).
    fn speak(&mut self, ctx: &WorkerCtx) {
        let text = surface::strip_pictures(&std::mem::take(&mut self.unspoken)).into_owned();
        let text = surface::cell_link::strip_run_links(&text).into_owned(); // 171:A1
        if !self.voice || text.trim().is_empty() {
            return;
        }
        let Some(voice) = ctx.voice.voice.clone() else {
            return;
        };
        let prev = self.clip_tail.take();
        let api = Arc::clone(&ctx.api);
        let to = ctx.to;
        self.clip_tail = Some(tokio::spawn(async move {
            let clip = voice.ask(voice::Synthesize { text }).await;
            if let Some(p) = prev {
                let _ = p.await;
            }
            let c = match clip.map_err(|e| e.to_string()) {
                Ok(Some(c)) => c,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "telegram voice clip synthesis failed");
                    return;
                }
            };
            let file = OutFile {
                name: "reply.mp3".to_owned(),
                mime: c.mime.to_owned(),
                bytes: c.bytes,
            };
            if let Err(e) = api.send_file(&to, &file, None, None).await {
                tracing::warn!(error = %e, "telegram voice clip send failed");
            }
        }));
    }

    /// Closes the open bubble (block boundary); `true` when something was persisted.
    async fn close_bubble(&mut self, ctx: &WorkerCtx, footer: Option<&str>) -> bool {
        let Some(b) = self.bubble.take() else {
            return false;
        };
        let placed = b.close(ctx, footer).await;
        self.speak(ctx);
        placed
    }

    /// Final edit of the card (batch end).
    async fn close_card(&mut self, ctx: &WorkerCtx, footer: Option<&str>) {
        if let Some(mut c) = self.card.take() {
            c.flush(ctx, true, footer).await;
        }
    }
}

/// Per-thread state (98 §8 `Worker`).
struct Worker {
    ctx: WorkerCtx,
    turn: Option<Turn>,
    /// The user's message(s) of this turn: 👀 → 👌/🤷/😱.
    marks: Vec<i64>,
    /// The last mark was a voice note.
    voice: bool,
    title_sent: Option<String>,
    context_size: Option<u32>,
    named_by_user: bool,
    board: Board,
    /// Oversized tool outputs for `out:` presses.
    outputs: HashMap<String, (Instant, String)>,
    last_typing: Instant,
}

impl Worker {
    fn new(ctx: WorkerCtx) -> Self {
        Self {
            named_by_user: ctx.topic.is_some_and(|t| t.named_by_user),
            ctx,
            turn: None,
            marks: Vec::new(),
            voice: false,
            title_sent: None,
            context_size: None,
            board: Board::default(),
            outputs: HashMap::new(),
            last_typing: Instant::now() - TYPING_GAP,
        }
    }

    /// One mailbox message; `false` = exit.
    async fn apply(&mut self, msg: WorkerMsg) -> bool {
        match msg {
            WorkerMsg::Agent(ev) => self.event(*ev).await,
            WorkerMsg::Mark { message_id, voice } => {
                self.marks.push(message_id);
                self.voice = voice;
                if let Some(t) = &mut self.turn {
                    t.voice = voice;
                }
                self.react(&[message_id], reaction("accepted")).await;
            }
            WorkerMsg::Stopped { draft_id } => {
                let live = self
                    .turn
                    .as_ref()
                    .and_then(|t| t.bubble.as_ref())
                    .and_then(Bubble::draft_id);
                if live == Some(draft_id)
                    && let Err(e) = self.ctx.port.cancel(self.ctx.session).await
                {
                    tracing::warn!(error = %e, "telegram stop: cancel failed");
                }
            }
            WorkerMsg::ChildRow { child, row, idle } => self.board.update(child, row, idle),
            WorkerMsg::TopicNamedByUser(v) => self.named_by_user = v,
            WorkerMsg::Output { call_id, reply } => {
                let out = self
                    .outputs
                    .get(&call_id)
                    .filter(|(at, _)| at.elapsed() < OUTPUT_TTL)
                    .map(|(_, s)| s.clone());
                let _ = reply.send(out);
            }
            WorkerMsg::Shutdown => {
                self.shutdown().await;
                return false;
            }
        }
        true
    }

    async fn event(&mut self, ev: AgentEvent) {
        let ctx = &self.ctx;
        match ev {
            AgentEvent::TurnStarted { model, .. } => {
                if let Some(t) = self.turn.take() {
                    self.finish_turn(t, None).await;
                }
                self.turn = Some(Turn::new(model, self.voice));
                if let Err(e) = self.ctx.api.typing(&self.ctx.to).await {
                    report(&self.ctx, &e, "typing");
                }
                self.last_typing = Instant::now();
            }
            AgentEvent::TextDelta { text, .. } => {
                let t = self
                    .turn
                    .get_or_insert_with(|| Turn::new(String::new(), false));
                t.end_reasoning(None);
                t.close_card(ctx, None).await;
                // Fresh = no prose shown yet: the first draft goes out at once.
                let mut fresh = t.bubble.as_ref().is_none_or(|b| b.text.raw().is_empty());
                let text =
                    surface::cell_link::run_links(&t.unspoken, &text, &ctx.session.to_string()); // 171:A1
                let b = t.bubble(ctx);
                b.push(&text);
                t.unspoken.push_str(&text);
                // Block boundaries (a closed picture fence, 164 D9; the soft cap, 98 §8.5):
                // persist up to each one, continue in a new bubble. An empty tail still
                // opens one so the turn footer has a bubble to land in.
                while let Some((head, tail)) = t.bubble.as_ref().and_then(Bubble::split) {
                    if let Some(mut old) = t.bubble.take() {
                        old.truncate_to(&head);
                        old.close(ctx, None).await;
                    }
                    t.speak(ctx);
                    fresh = true;
                    let b = t.bubble(ctx);
                    if tail.is_empty() {
                        break;
                    }
                    b.push(&tail);
                }
                if fresh && let Some(b) = &mut t.bubble {
                    b.flush(ctx, true, false).await;
                }
            }
            AgentEvent::ReasoningDelta { text, .. } => {
                let keep = ctx.cfg.reasoning;
                let t = self
                    .turn
                    .get_or_insert_with(|| Turn::new(String::new(), false));
                if let Some(c) = &mut t.card {
                    c.think(&text, keep);
                } else {
                    let fresh = t.bubble.is_none();
                    let b = t.bubble(ctx);
                    b.think(&text, keep);
                    if fresh {
                        b.flush(ctx, true, false).await;
                    }
                }
            }
            AgentEvent::ReasoningDone { ms, .. } => {
                if let Some(t) = &mut self.turn {
                    t.end_reasoning(Some(ms));
                }
            }
            AgentEvent::ToolCallPending { call_id, name, .. } => {
                let t = self
                    .turn
                    .get_or_insert_with(|| Turn::new(String::new(), false));
                t.end_reasoning(None);
                t.names.insert(call_id.clone(), name.clone());
                if ctx.cfg.tools != ToolsMode::All {
                    return;
                }
                if t.card.is_none() {
                    Self::open_card(t, ctx).await;
                }
                let card = t.card.get_or_insert_with(Card::new);
                card.pending(&call_id, &name);
                if card.msg.is_none() {
                    card.flush(ctx, false, None).await;
                }
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => {
                let t = self
                    .turn
                    .get_or_insert_with(|| Turn::new(String::new(), false));
                t.tools += 1;
                t.names.insert(call_id.clone(), name.clone());
                if ctx.cfg.tools != ToolsMode::All {
                    return;
                }
                if t.card.is_none() {
                    Self::open_card(t, ctx).await;
                }
                let card = t.card.get_or_insert_with(Card::new);
                card.started(&call_id, &name, &args);
                if card.msg.is_none() {
                    card.flush(ctx, false, None).await;
                }
            }
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                let t = self
                    .turn
                    .get_or_insert_with(|| Turn::new(String::new(), false));
                let name = t.names.remove(&call_id).unwrap_or_default();
                let show = match ctx.cfg.tools {
                    ToolsMode::All => true,
                    ToolsMode::Errors => is_error,
                    ToolsMode::None => false,
                };
                if !show {
                    return;
                }
                if t.card.is_none() {
                    Self::open_card(t, ctx).await;
                }
                let card = t.card.get_or_insert_with(Card::new);
                let more = card.finished(&call_id, &name, &output, is_error);
                if more > 0 {
                    self.outputs.insert(call_id, (Instant::now(), output));
                }
                if card.msg.is_none() {
                    card.flush(ctx, false, None).await;
                }
            }
            AgentEvent::TurnFinished {
                stop_reason,
                usage,
                context,
                ..
            } => {
                let Some(t) = self.turn.take() else {
                    return;
                };
                let line = footer_line(
                    &t.model,
                    usage,
                    t.started.elapsed(),
                    t.tools,
                    self.context_size.map(|size| (context, size)),
                );
                self.finish_turn(t, Some(footer(&line))).await;
                let moment = match stop_reason.as_str() {
                    "cancelled" => "cancelled",
                    "error" => "failed",
                    _ => "done",
                };
                let marks = std::mem::take(&mut self.marks);
                self.react(&marks, reaction(moment)).await;
                self.title().await;
            }
            AgentEvent::Error { message, .. } => {
                if let Some(mut t) = self.turn.take() {
                    t.end_reasoning(None);
                    t.close_bubble(ctx, None).await;
                    t.close_card(ctx, None).await;
                    self.turn = Some(t);
                }
                let _ = send_rich_or_plain(ctx, &brand(ctx, &format!("⚠️ {message}"))).await;
                let marks = self.marks.clone();
                self.react(&marks, reaction("failed")).await;
            }
            // 164 D6: a peer's text is painted too (pictures in place, fallback text otherwise).
            AgentEvent::MessageDelivered { from, text, .. } => {
                let lead = format!("⟵ **{from}**\n\n");
                send_painted(ctx, &lead, &clip(&text, DELIVERED_MAX)).await;
            }
            // 125 §9: a concern or blocker is one footer-styled line; nits post nothing.
            AgentEvent::AdvisoryAdded { advisory, .. } => {
                if let Some(line) = advisory.channel_line() {
                    let _ = send_rich_or_plain(ctx, &brand(ctx, &footer(&line))).await;
                }
            }
            AgentEvent::Compacted { epoch, folded, .. } => {
                let md = format!("⟲ compacted → epoch {epoch} ({folded} messages folded)");
                let _ = send_rich_or_plain(ctx, &brand(ctx, &md)).await;
            }
            AgentEvent::ContextUsed {
                size: Some(size), ..
            } => self.context_size = Some(size),
            _ => {}
        }
    }

    /// The first card of a batch: close the bubble (a reasoning-only bubble
    /// moves into the card as a `thinking` entry).
    async fn open_card(t: &mut Turn, ctx: &WorkerCtx) {
        let mut card = Card::new();
        if let Some(mut b) = t.bubble.take() {
            if b.text.raw().trim().is_empty() {
                if let Some(r) = b.take_reasoning() {
                    card.adopt(r);
                }
                b.close(ctx, None).await;
            } else {
                b.close(ctx, None).await;
                t.speak(ctx);
            }
        }
        t.card = Some(card);
    }

    /// `TurnFinished` / a superseded turn: persist what is open, footer on
    /// the last bubble, else on the card.
    async fn finish_turn(&mut self, mut t: Turn, footer: Option<String>) {
        t.end_reasoning(None);
        let placed = t.close_bubble(&self.ctx, footer.as_deref()).await;
        t.close_card(&self.ctx, if placed { None } else { footer.as_deref() })
            .await;
    }

    async fn react(&self, marks: &[i64], emoji: &str) {
        for id in marks {
            let at = MsgRef {
                chat: self.ctx.to.chat,
                id: *id,
            };
            if let Err(e) = self.ctx.api.react(&at, Some(emoji)).await {
                tracing::debug!(key = %self.ctx.key, error = %e, "telegram reaction refused");
            }
        }
    }

    /// After `TurnFinished`: follow the session title on a topic the user did not name.
    async fn title(&mut self) {
        let Some(topic) = self.ctx.topic else {
            return;
        };
        if self.named_by_user {
            return;
        }
        let Ok(Some(title)) = self.ctx.port.session_title(self.ctx.session).await else {
            return;
        };
        if title.trim().is_empty() || self.title_sent.as_deref() == Some(title.as_str()) {
            return;
        }
        let name = match &self.ctx.child_name {
            Some(n) => format!("↳ {n} · {title}"),
            None => title.clone(),
        };
        let name = truncate_chars(&name, TITLE_MAX);
        match self
            .ctx
            .api
            .rename_topic(self.ctx.to.chat, topic.id, &name)
            .await
        {
            Ok(()) => {
                self.title_sent = Some(title.clone());
                let _ = self.ctx.notes.send(WorkerNote::Renamed {
                    key: self.ctx.key.clone(),
                    title,
                });
            }
            Err(e) => report(&self.ctx, &e, "rename_topic"),
        }
    }

    /// The 1 s tick: flush the bubble, the card, the board; typing; prune outputs.
    async fn tick(&mut self) {
        if let Some(t) = &mut self.turn {
            let card_live = t.card.is_some();
            if let Some(b) = &mut t.bubble {
                b.flush(&self.ctx, true, card_live).await;
            }
            if let Some(c) = &mut t.card {
                c.flush(&self.ctx, false, None).await;
            }
            let touched = t
                .bubble
                .as_ref()
                .map(|b| b.last_call)
                .into_iter()
                .chain(t.card.as_ref().map(|c| c.last_edit))
                .max()
                .unwrap_or(t.started);
            if touched.elapsed() >= TYPING_GAP && self.last_typing.elapsed() >= TYPING_GAP {
                if let Err(e) = self.ctx.api.typing(&self.ctx.to).await {
                    report(&self.ctx, &e, "typing");
                }
                self.last_typing = Instant::now();
            }
        }
        self.board.flush(&self.ctx).await;
        self.outputs.retain(|_, (at, _)| at.elapsed() < OUTPUT_TTL);
    }

    async fn shutdown(&mut self) {
        if let Some(t) = self.turn.take() {
            self.finish_turn(t, None).await;
        }
        self.board.close(&self.ctx).await;
    }
}

/// Spawns the worker task; the returned sender is its mailbox.
pub fn spawn(ctx: WorkerCtx) -> mpsc::UnboundedSender<WorkerMsg> {
    let (tx, mut rx) = mpsc::unbounded_channel::<WorkerMsg>();
    tokio::spawn(async move {
        let mut w = Worker::new(ctx);
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(m) => {
                        if !w.apply(m).await {
                            break;
                        }
                    }
                    None => {
                        w.shutdown().await;
                        break;
                    }
                },
                _ = tick.tick() => w.tick().await,
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use agent::communication::PeerScope;
    use agent::{AgentInfo, Loaded, SessionSummary, Usage};
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use runtime::TurnId;
    use tools::{Answer, ApprovalId, AskId, Decision};

    use super::*;
    use crate::api::ApiError;
    use crate::api::mock::{Call, MockSender};
    use crate::bubble::BUBBLE_SOFT_MAX;
    use crate::cards::TOOL_OUTPUT_CHARS;

    /// A `Port` that only knows the session title.
    struct TitlePort(Mutex<Option<String>>);

    #[async_trait]
    impl Port for TitlePort {
        async fn send_input(
            &self,
            _: Option<SessionId>,
            _: String,
            _: Vec<PathBuf>,
            _: PeerScope,
            _: agent::ChannelOrigin,
        ) -> Result<SessionId, String> {
            Err("no".into())
        }
        async fn new_root(&self, _: PeerScope) -> Result<SessionId, String> {
            Err("no".into())
        }
        async fn steer(
            &self,
            _: SessionId,
            _: String,
            _: Vec<PathBuf>,
            _: agent::ChannelOrigin,
        ) -> Result<(), String> {
            Err("no".into())
        }
        async fn cancel(&self, _: SessionId) -> Result<(), String> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
            Ok(Vec::new())
        }
        async fn load_session(&self, _: SessionId) -> Result<Loaded, String> {
            Err("no".into())
        }
        async fn resolve(&self, _: ApprovalId, _: Decision) -> Result<bool, String> {
            Ok(false)
        }
        async fn answer_ask(&self, _: AskId, _: Option<Vec<Answer>>) -> Result<bool, String> {
            Ok(false)
        }
        async fn list_agents(&self, _: SessionId) -> Result<Vec<AgentInfo>, String> {
            Ok(Vec::new())
        }
        async fn ancestors(&self, s: SessionId) -> Result<Vec<SessionId>, String> {
            Ok(vec![s])
        }
        async fn session_title(&self, _: SessionId) -> Result<Option<String>, String> {
            Ok(self.0.lock().clone())
        }
    }

    struct Rig {
        w: Worker,
        api: Arc<MockSender>,
        notes: mpsc::UnboundedReceiver<WorkerNote>,
        title: Arc<TitlePort>,
    }

    fn rig(chat: i64, topic: Option<i64>, cfg: RenderConfig, child: Option<&str>) -> Rig {
        let api = MockSender::new();
        let (notes_tx, notes) = mpsc::unbounded_channel();
        let title = Arc::new(TitlePort(Mutex::new(None)));
        let ctx = WorkerCtx {
            key: crate::threads::key(chat, topic),
            to: Target {
                chat,
                thread: topic,
            },
            user: 7,
            session: SessionId::new(),
            api: api.clone(),
            port: title.clone(),
            rt: Runtime::new(figment::Figment::new(), std::env::temp_dir()),
            cfg,
            voice: VoiceHandles::default(),
            topic: topic.map(|id| Topic {
                id,
                named_by_user: false,
            }),
            notes: notes_tx,
            child_name: child.map(str::to_owned),
        };
        Rig {
            w: Worker::new(ctx),
            api,
            notes,
            title,
        }
    }

    const ALL: RenderConfig = RenderConfig {
        tools: ToolsMode::All,
        reasoning: true,
        pictures: true,
    };

    impl Rig {
        fn session(&self) -> SessionId {
            self.w.ctx.session
        }
        async fn ev(&mut self, ev: AgentEvent) {
            self.w.event(ev).await;
        }
        async fn start(&mut self) {
            self.ev(AgentEvent::TurnStarted {
                session: self.session(),
                turn: TurnId::new(),
                text: "hi".into(),
                model: "m1".into(),
                turn_seq: 1,
                channel: None,
            })
            .await;
        }
        async fn delta(&mut self, s: &str) {
            self.ev(AgentEvent::TextDelta {
                session: self.session(),
                turn: TurnId::new(),
                text: Arc::from(s),
            })
            .await;
        }
        async fn finish(&mut self, stop: &str) {
            self.ev(AgentEvent::TurnFinished {
                session: self.session(),
                turn: TurnId::new(),
                stop_reason: stop.into(),
                usage: Usage {
                    prompt: 12_300,
                    completion: 1_800,
                    cached: 0,
                },
                context: 41_000,
            })
            .await;
        }
        async fn pending(&mut self, id: &str, name: &str) {
            self.ev(AgentEvent::ToolCallPending {
                session: self.session(),
                turn: TurnId::new(),
                call_id: id.into(),
                name: name.into(),
            })
            .await;
        }
        async fn started(&mut self, id: &str, name: &str, args: serde_json::Value) {
            self.ev(AgentEvent::ToolCallStarted {
                session: self.session(),
                turn: TurnId::new(),
                call_id: id.into(),
                name: name.into(),
                args,
            })
            .await;
        }
        async fn finished(&mut self, id: &str, out: &str, err: bool) {
            self.ev(AgentEvent::ToolCallFinished {
                session: self.session(),
                turn: TurnId::new(),
                call_id: id.into(),
                output: out.into(),
                is_error: err,
                files: Vec::new(),
            })
            .await;
        }
        async fn tick(&mut self) {
            tokio::time::advance(TICK).await;
            self.w.tick().await;
        }
    }

    fn drafts(calls: &[Call]) -> Vec<(i64, String, bool)> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::Draft {
                    draft_id,
                    md,
                    can_stop,
                    ..
                } => Some((*draft_id, md.clone(), *can_stop)),
                _ => None,
            })
            .collect()
    }

    fn sends(calls: &[Call]) -> Vec<String> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::SendRich { md, .. } => Some(md.clone()),
                _ => None,
            })
            .collect()
    }

    fn edits(calls: &[Call]) -> Vec<String> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::EditRich { md, .. } => Some(md.clone()),
                _ => None,
            })
            .collect()
    }

    fn reactions(calls: &[Call]) -> Vec<String> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::React { emoji, .. } => emoji.clone(),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn draft_opens_on_first_delta_and_resends_full_text_on_tick() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        assert!(matches!(r.api.calls().as_slice(), [Call::Typing(_)]));
        r.delta("Hello").await;
        let d = drafts(&r.api.calls());
        assert_eq!(d, vec![(1, "Hello".to_owned(), true)]);
        r.delta(" world").await;
        assert_eq!(
            drafts(&r.api.calls()).len(),
            1,
            "deltas coalesce until the tick"
        );
        r.tick().await;
        let d = drafts(&r.api.calls());
        assert_eq!(d.last().unwrap().1, "Hello world");
        r.tick().await;
        assert_eq!(drafts(&r.api.calls()).len(), 2, "nothing new, no call");
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_resends_once_after_25s_silence() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.delta("Hello").await;
        for _ in 0..24 {
            r.tick().await;
        }
        assert_eq!(drafts(&r.api.calls()).len(), 1);
        r.tick().await;
        let d = drafts(&r.api.calls());
        assert_eq!(d.len(), 2);
        assert_eq!(d[1].1, "Hello");
        for _ in 0..10 {
            r.tick().await;
        }
        assert_eq!(drafts(&r.api.calls()).len(), 2, "one keepalive per 25 s");
    }

    #[tokio::test(start_paused = true)]
    async fn tool_pending_closes_bubble_and_card_progresses() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.delta("one ").await;
        r.delta("two").await;
        r.api.take_calls();
        r.pending("c1", "shell").await;
        let calls = r.api.take_calls();
        let s = sends(&calls);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "one two");
        assert_eq!(s[1], "⏳ shell");
        r.started("c1", "shell", serde_json::json!({"command": "ls -la"}))
            .await;
        r.tick().await;
        r.tick().await;
        let e = edits(&r.api.take_calls());
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("<details open><summary>⚙ shell · ls -la · running"));
        assert!(e[0].contains("```sh\nls -la\n```"));
        let big = format!("first line\n{}", "x".repeat(TOOL_OUTPUT_CHARS));
        r.finished("c1", &big, false).await;
        r.delta("after").await;
        let calls = r.api.take_calls();
        let e = edits(&calls);
        assert_eq!(e.len(), 1, "batch end = final edit");
        assert!(e[0].starts_with("<details><summary>✓ shell · ls -la · "));
        assert!(e[0].contains("```text\nfirst line\n"));
        assert!(e[0].contains("… 11 more chars"));
        assert!(e[0].contains("data=\"out:c1\">full output · "));
        assert_eq!(drafts(&calls), vec![(2, "after".to_owned(), true)]);
        let (tx, rx) = oneshot::channel();
        r.w.apply(WorkerMsg::Output {
            call_id: "c1".into(),
            reply: tx,
        })
        .await;
        assert_eq!(rx.await.unwrap().unwrap(), big);
        r.finish("end_turn").await;
        let last = sends(&r.api.calls()).pop().unwrap();
        assert!(last.starts_with("after\n\n<footer>m1 · 12.3k in / 1.8k out · "));
        assert!(last.contains(" · 1 tools</footer>"));
    }

    #[tokio::test(start_paused = true)]
    async fn errors_mode_opens_card_only_for_an_error() {
        let cfg = RenderConfig {
            tools: ToolsMode::Errors,
            reasoning: true,
            pictures: true,
        };
        let mut r = rig(10, None, cfg, None);
        r.start().await;
        r.pending("c1", "shell").await;
        r.started("c1", "shell", serde_json::json!({"command": "ls"}))
            .await;
        r.finished("c1", "ok", false).await;
        assert!(sends(&r.api.calls()).is_empty());
        r.pending("c2", "edit").await;
        r.finished("c2", "old text not found", true).await;
        let s = sends(&r.api.calls());
        assert_eq!(s.len(), 1);
        assert!(s[0].starts_with("<details><summary>✗ edit · "));
        assert!(s[0].contains("old text not found"));
    }

    #[tokio::test(start_paused = true)]
    async fn long_turn_rotates_at_block_boundary_and_reopens_fence() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        let para = format!("{}\n\n", "p".repeat(999));
        for _ in 0..40 {
            r.delta(&para).await;
        }
        let s = sends(&r.api.calls());
        assert_eq!(s.len(), 1);
        assert!(s[0].len() <= BUBBLE_SOFT_MAX);
        // The persisted text is painted (164): trailing whitespace trimmed.
        assert!(s[0].ends_with('p'), "closed at a block boundary");
        assert!(
            (s[0].len() + 2).is_multiple_of(1001),
            "whole paragraphs only"
        );
        r.api.take_calls();
        r.delta("```rust\n").await;
        for _ in 0..3200 {
            r.delta("let x = 1;\n").await;
        }
        let s = sends(&r.api.calls());
        assert_eq!(s.len(), 2, "paragraph boundary, then the fence cut");
        assert!(s[0].ends_with('p') && !s[0].contains("```"));
        assert!(s[1].starts_with("```rust\n"));
        assert!(s[1].ends_with("\n```"), "fence closed at the cut");
        let d = drafts(&r.api.calls());
        assert!(
            d.last().unwrap().1.starts_with("```rust\n"),
            "fence re-opened"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn draft_failure_switches_to_edit_mode_within_committed() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.api.fail_next(
            "draft",
            ApiError::Api {
                code: 400,
                description: "Bad Request: nope".into(),
            },
        );
        r.delta("para one\n\n").await;
        r.delta("tail").await;
        assert!(sends(&r.api.calls()).is_empty(), "no committed block yet");
        r.tick().await;
        let calls = r.api.take_calls();
        assert_eq!(sends(&calls), vec!["para one\n\n".to_owned()]);
        assert!(matches!(
            r.notes.try_recv(),
            Ok(WorkerNote::Fallback { reason, .. }) if reason.starts_with("draft:")
        ));
        assert!(matches!(
            r.notes.try_recv(),
            Ok(WorkerNote::OpenMsg { open: true, .. })
        ));
        r.delta(" more\n\nnext").await;
        r.tick().await;
        let calls = r.api.take_calls();
        assert!(drafts(&calls).is_empty());
        assert_eq!(edits(&calls), vec!["para one\n\ntail more\n\n".to_owned()]);
        r.api.fail_next(
            "edit_rich",
            ApiError::Api {
                code: 400,
                description: format!("{}: x", crate::api::NOT_MODIFIED_PREFIX),
            },
        );
        r.delta("\n\nx").await;
        r.tick().await;
        r.finish("end_turn").await;
        let e = edits(&r.api.take_calls());
        assert_eq!(e.len(), 2);
        assert!(e[1].starts_with("para one\n\ntail more\n\nnext\n\nx\n\n<footer>m1"));
        assert!(matches!(
            r.notes.try_recv(),
            Ok(WorkerNote::OpenMsg { open: false, .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn groups_use_edit_mode_and_child_prefix() {
        let mut r = rig(-100, None, ALL, Some("grep-logs"));
        r.start().await;
        r.delta("a\n\nb").await;
        assert!(drafts(&r.api.calls()).is_empty());
        r.tick().await;
        assert_eq!(
            sends(&r.api.calls()),
            vec!["**grep-logs** · a\n\n".to_owned()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_quote_and_switch() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.ev(AgentEvent::ReasoningDelta {
            session: r.session(),
            turn: TurnId::new(),
            text: Arc::from("hmm"),
        })
        .await;
        let d = drafts(&r.api.calls());
        assert!(
            d[0].1
                .starts_with("<tg-thinking>thinking · 0 s</tg-thinking>\n")
        );
        assert!(
            d[0].1
                .contains("<blockquote expandable>\nhmm\n</blockquote>")
        );
        r.ev(AgentEvent::ReasoningDone {
            session: r.session(),
            turn: TurnId::new(),
            ms: 9_000,
        })
        .await;
        r.delta("prose").await;
        r.finish("end_turn").await;
        let s = sends(&r.api.calls()).pop().unwrap();
        assert!(
            s.starts_with("**thought 9 s**\n<blockquote expandable>\nhmm\n</blockquote>\n\nprose")
        );

        let mut r = rig(
            10,
            None,
            RenderConfig {
                tools: ToolsMode::All,
                reasoning: false,
                pictures: true,
            },
            None,
        );
        r.start().await;
        r.ev(AgentEvent::ReasoningDelta {
            session: r.session(),
            turn: TurnId::new(),
            text: Arc::from("hmm"),
        })
        .await;
        let d = drafts(&r.api.calls());
        assert_eq!(d[0].1, "<tg-thinking>thinking · 0 s</tg-thinking>\n");
        r.delta("prose").await;
        r.finish("end_turn").await;
        let s = sends(&r.api.calls()).pop().unwrap();
        assert!(s.starts_with("prose\n\n<footer>"));
    }

    #[tokio::test(start_paused = true)]
    async fn reactions_follow_the_turn() {
        for (stop, emoji) in [("end_turn", "👌"), ("cancelled", "🤷"), ("error", "😱")] {
            let mut r = rig(10, None, ALL, None);
            r.w.apply(WorkerMsg::Mark {
                message_id: 5,
                voice: false,
            })
            .await;
            r.start().await;
            r.delta("x").await;
            r.finish(stop).await;
            assert_eq!(reactions(&r.api.calls()), vec!["👀", emoji]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn topic_renamed_once_per_title_and_never_after_user_named() {
        let mut r = rig(10, Some(77), ALL, None);
        r.start().await;
        r.finish("end_turn").await;
        assert!(
            !r.api
                .calls()
                .iter()
                .any(|c| matches!(c, Call::RenameTopic { .. }))
        );
        *r.title.0.lock() = Some("Fix the tests".into());
        r.start().await;
        r.finish("end_turn").await;
        r.start().await;
        r.finish("end_turn").await;
        let renames: Vec<_> = r
            .api
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::RenameTopic { topic, name, .. } => Some((topic, name)),
                _ => None,
            })
            .collect();
        assert_eq!(renames, vec![(77, "Fix the tests".to_owned())]);
        assert!(matches!(
            r.notes.try_recv(),
            Ok(WorkerNote::Renamed { title, .. }) if title == "Fix the tests"
        ));
        r.w.apply(WorkerMsg::TopicNamedByUser(true)).await;
        *r.title.0.lock() = Some("Other".into());
        r.start().await;
        r.finish("end_turn").await;
        assert_eq!(
            r.api
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::RenameTopic { .. }))
                .count(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn delivered_message_is_its_own_message() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.delta("live").await;
        r.ev(AgentEvent::MessageDelivered {
            session: r.session(),
            from: "grep-logs".into(),
            text: "found it".into(),
        })
        .await;
        assert_eq!(
            sends(&r.api.calls()),
            vec!["⟵ **grep-logs**\n\nfound it".to_owned()]
        );
        assert_eq!(drafts(&r.api.calls()).len(), 1, "the draft is untouched");
    }

    /// 125 §9: a concern is one footer-styled line (its first line); a nit posts nothing.
    #[tokio::test(start_paused = true)]
    async fn advisory_concern_is_one_footer_line_and_nit_is_silent() {
        use agent::{AdvisorSeverity, Advisory, AdvisoryNote};
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        let session = r.session();
        let card = |severity: AdvisorSeverity, note: &str| AgentEvent::AdvisoryAdded {
            session,
            seq: 5,
            advisory: Advisory {
                advisor: SessionId::new(),
                step: 2,
                turn: None,
                at_ms: 0,
                notes: vec![AdvisoryNote {
                    severity,
                    note: note.into(),
                }],
                kind: Default::default(),
            },
        };
        r.ev(card(AdvisorSeverity::Nit, "nit: rename x")).await;
        assert!(sends(&r.api.calls()).is_empty());
        r.ev(card(
            AdvisorSeverity::Concern,
            "claims done\nbut no check ran",
        ))
        .await;
        assert_eq!(
            sends(&r.api.calls()),
            vec!["<footer>advisor · concern — claims done</footer>".to_owned()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_press_cancels_only_the_live_draft() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.delta("x").await;
        r.w.apply(WorkerMsg::Stopped { draft_id: 1 }).await;
        r.w.apply(WorkerMsg::Stopped { draft_id: 9 }).await;
        // `TitlePort::cancel` is `Ok`: the only observable is that nothing panicked and no call was made.
        assert_eq!(drafts(&r.api.calls()).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn board_edits_two_seconds_apart_and_ends_all_idle() {
        let mut r = rig(10, None, ALL, None);
        let row = |state: &str| Row {
            glyph: "⠹",
            name: "grep-logs".into(),
            state: state.into(),
            detail: "read_file x.rs".into(),
            age_secs: 92,
        };
        let child = SessionId::new();
        r.w.apply(WorkerMsg::ChildRow {
            child,
            row: row("running"),
            idle: false,
        })
        .await;
        r.tick().await;
        let s = sends(&r.api.calls());
        assert_eq!(s.len(), 1);
        assert!(s[0].starts_with("**agents**\n|  | name | state | detail | age |\n"));
        assert!(s[0].contains("| ⠹ | grep-logs | running | read_file x.rs | 1m32s |"));
        r.w.apply(WorkerMsg::ChildRow {
            child,
            row: row("awaiting"),
            idle: false,
        })
        .await;
        r.tick().await;
        assert!(edits(&r.api.calls()).is_empty(), "< 2 s since the send");
        r.tick().await;
        assert_eq!(edits(&r.api.calls()).len(), 1);
        r.w.apply(WorkerMsg::ChildRow {
            child,
            row: row("idle"),
            idle: true,
        })
        .await;
        r.tick().await;
        r.tick().await;
        let e = edits(&r.api.calls());
        assert_eq!(e.last().unwrap(), "**agents** · all idle");
    }

    // -- 164: pictures ---------------------------------------------------------

    use crate::api::FileKind;
    use crate::api::mock::png;
    use artifact::snapshot;

    const GRAPH: &str = "{\"root\":\"g\",\"elements\":{\"g\":{\"type\":\"Graph\",\"props\":{\"title\":\"Data flow\",\"nodes\":[{\"id\":\"a\",\"label\":\"Alpha node\"},{\"id\":\"b\",\"label\":\"Beta node\"}],\"edges\":[{\"from\":\"a\",\"to\":\"b\"}]}}}}";

    /// Answers every snapshot request with one PNG and counts them.
    #[derive(kameo::Actor)]
    struct FakeRenderer {
        png: Vec<u8>,
        requests: Arc<Mutex<usize>>,
    }

    impl kameo::message::Message<snapshot::FenceSnapshotRequested> for FakeRenderer {
        type Reply = ();
        async fn handle(
            &mut self,
            m: snapshot::FenceSnapshotRequested,
            _: &mut kameo::message::Context<Self, ()>,
        ) {
            *self.requests.lock() += 1;
            snapshot::answer(
                m.id,
                snapshot::FenceSnapshotAnswer::done(
                    &self.png,
                    1120,
                    600,
                    artifact::preview::Client::Desktop,
                ),
            );
        }
    }

    impl Rig {
        /// A fake desktop renderer on this rig's bus; the count of requests it saw.
        async fn renderer(&self, png: Vec<u8>) -> Arc<Mutex<usize>> {
            artifact::preview::watch_feed(Arc::new(|| 1));
            let requests = Arc::new(Mutex::new(0));
            let actor = kameo::actor::Spawn::spawn_with_mailbox(
                FakeRenderer {
                    png,
                    requests: Arc::clone(&requests),
                },
                runtime::subscriber_mailbox(),
            );
            self.w
                .ctx
                .rt
                .subscribe::<FakeRenderer, snapshot::FenceSnapshotRequested>(&actor)
                .await;
            requests
        }
    }

    /// Every text Telegram saw, in order: drafts, rich sends, edits, plain sends.
    fn texts(calls: &[Call]) -> Vec<String> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::Draft { md, .. } | Call::SendRich { md, .. } | Call::EditRich { md, .. } => {
                    Some(md.clone())
                }
                Call::SendText { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn files(calls: &[Call]) -> Vec<(String, String, Vec<u8>, Option<String>)> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::SendFile {
                    name,
                    mime,
                    bytes,
                    caption,
                    ..
                } => Some((name.clone(), mime.clone(), bytes.clone(), caption.clone())),
                _ => None,
            })
            .collect()
    }

    /// No byte of the fence body in any text that went out.
    fn assert_no_body(calls: &[Call]) {
        for t in texts(calls) {
            assert!(
                !t.contains("\"root\"") && !t.contains("\"label\"") && !t.contains("```json"),
                "fence body leaked: {t}"
            );
        }
    }

    /// (a) 164 D6/D9: prose, a Graph fence, prose → rich text, the photo (the
    /// renderer's PNG, caption = title), rich text, in order; no raw JSON.
    #[tokio::test(start_paused = true)]
    async fn picture_fence_is_a_block_boundary_with_the_photo_in_place() {
        let mut r = rig(10, None, ALL, None);
        let image = png(1120, 600);
        let requests = r.renderer(image.clone()).await;
        r.start().await;
        r.delta("Before\n\n").await;
        r.delta(&format!("```json-render\n{GRAPH}\n```\n\nAfter"))
            .await;
        let calls = r.api.take_calls();
        assert_eq!(sends(&calls), vec!["Before".to_owned()]);
        assert_eq!(
            files(&calls),
            vec![(
                "data-flow.png".to_owned(),
                "image/png".to_owned(),
                image,
                Some("Data flow".to_owned())
            )]
        );
        let order: Vec<&str> = calls
            .iter()
            .filter_map(|c| match c {
                Call::SendRich { .. } => Some("text"),
                Call::SendFile { .. } => Some("photo"),
                Call::Draft { .. } => Some("draft"),
                _ => None,
            })
            .collect();
        assert_eq!(order, vec!["draft", "text", "photo", "draft"]);
        assert_eq!(drafts(&calls).last().unwrap().1, "After");
        assert_no_body(&calls);
        r.finish("end_turn").await;
        let calls = r.api.take_calls();
        let s = sends(&calls);
        assert_eq!(s.len(), 1);
        assert!(s[0].starts_with("After\n\n<footer>m1 · "), "{}", s[0]);
        assert_no_body(&calls);
        assert_eq!(*requests.lock(), 1);
    }

    /// A turn ending on a picture: photo, then the footer as its own message.
    #[tokio::test(start_paused = true)]
    async fn turn_ending_on_a_picture_keeps_its_footer() {
        let mut r = rig(10, None, ALL, None);
        r.renderer(png(1120, 600)).await;
        r.start().await;
        r.delta(&format!("Look:\n\n```json-render\n{GRAPH}\n```\n"))
            .await;
        r.finish("end_turn").await;
        let calls = r.api.calls();
        let order: Vec<String> = calls
            .iter()
            .filter_map(|c| match c {
                Call::SendRich { md, .. } => Some(md.clone()),
                Call::SendFile { name, .. } => Some(format!("photo {name}")),
                _ => None,
            })
            .collect();
        assert_eq!(order.len(), 3, "{order:?}");
        assert_eq!(order[0], "Look:");
        assert_eq!(order[1], "photo data-flow.png");
        assert!(order[2].starts_with("<footer>m1 · "), "{}", order[2]);
        assert_no_body(&calls);
    }

    /// Edit mode (groups): the committed message is edited to the prose before
    /// the fence, the photo follows, the prose after is a new message.
    #[tokio::test(start_paused = true)]
    async fn group_edit_mode_places_the_photo_between_messages() {
        let mut r = rig(-100, None, ALL, Some("grep-logs"));
        r.renderer(png(1120, 600)).await;
        r.start().await;
        r.delta("Before\n\nstill").await;
        r.tick().await;
        assert_eq!(
            sends(&r.api.take_calls()),
            vec!["**grep-logs** · Before\n\n".to_owned()]
        );
        r.delta(&format!("\n\n```json-render\n{GRAPH}\n```\n\nAfter"))
            .await;
        r.finish("end_turn").await;
        let calls = r.api.calls();
        assert_eq!(
            edits(&calls),
            vec!["**grep-logs** · Before\n\nstill".to_owned()]
        );
        assert_eq!(files(&calls).len(), 1);
        let s = sends(&calls);
        assert_eq!(s.len(), 1);
        assert!(
            s[0].starts_with("**grep-logs** · After\n\n<footer>"),
            "{}",
            s[0]
        );
        assert!(
            calls
                .iter()
                .position(|c| matches!(c, Call::EditRich { .. }))
                < calls
                    .iter()
                    .position(|c| matches!(c, Call::SendFile { .. })),
            "edit before the photo"
        );
        assert_no_body(&calls);
    }

    /// (b) 164 D7: no renderer → the fence's text fallback, no file, no raw JSON.
    #[tokio::test(start_paused = true)]
    async fn no_renderer_sends_the_text_fallback() {
        let mut r = rig(10, None, ALL, None);
        r.start().await;
        r.delta(&format!("Before\n\n```json-render\n{GRAPH}\n```\n\nAfter"))
            .await;
        r.finish("end_turn").await;
        let calls = r.api.calls();
        assert!(files(&calls).is_empty());
        let s = sends(&calls);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].starts_with("Before\n"), "{}", s[0]);
        assert!(s[0].len() > "Before\n\n".len(), "fallback text: {}", s[0]);
        assert!(s[1].starts_with("After\n\n<footer>"), "{}", s[1]);
        assert_no_body(&calls);
    }

    /// (c) 164 D10: `pictures: false` → fallback text and no request at all.
    #[tokio::test(start_paused = true)]
    async fn pictures_off_falls_back_without_a_request() {
        let cfg = RenderConfig {
            pictures: false,
            ..ALL
        };
        let mut r = rig(10, None, cfg, None);
        let requests = r.renderer(png(1120, 600)).await;
        r.start().await;
        r.delta(&format!("Before\n\n```json-render\n{GRAPH}\n```\n\nAfter"))
            .await;
        r.finish("end_turn").await;
        let calls = r.api.calls();
        assert!(files(&calls).is_empty());
        assert_eq!(*requests.lock(), 0);
        let s = sends(&calls);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].len() > "Before\n\n".len(), "fallback text: {}", s[0]);
        assert_no_body(&calls);
    }

    /// (d) 164 D8: a fence streamed byte by byte never shows a byte of its
    /// body in any draft; `_drawing…_` stands in while it streams.
    #[tokio::test(start_paused = true)]
    async fn streamed_fence_never_shows_its_body_in_a_draft() {
        let mut r = rig(10, None, ALL, None);
        r.renderer(png(1120, 600)).await;
        r.start().await;
        let reply = format!("Intro\n\n```json-render\n{GRAPH}\n```\n\nOutro");
        let mut drawing = false;
        let mut photos = 0;
        for ch in reply.chars() {
            r.delta(&ch.to_string()).await;
            r.tick().await;
            let calls = r.api.take_calls();
            assert_no_body(&calls);
            photos += files(&calls).len();
            for (_, md, _) in drafts(&calls) {
                assert!(
                    !md.contains("```j") && !md.contains("Alpha") && !md.contains("Beta"),
                    "fence content in a draft: {md}"
                );
                if md.contains(surface::DRAWING) {
                    drawing = true;
                    assert!(md.starts_with("Intro\n\n"), "{md}");
                }
            }
        }
        assert!(
            drawing,
            "the drawing line was shown while the fence streamed"
        );
        assert_eq!(photos, 1, "the photo went out when the fence closed");
        r.finish("end_turn").await;
        let calls = r.api.calls();
        assert!(files(&calls).is_empty());
        assert!(sends(&calls)[0].starts_with("Outro\n\n<footer>"));
        assert_no_body(&calls);
    }

    /// (e) 164 D9: a PNG past Telegram's photo limits goes out as a document
    /// (`file_kind` on the very bytes the mock recorded).
    #[tokio::test(start_paused = true)]
    async fn oversize_picture_goes_out_as_a_document() {
        let mut r = rig(10, None, ALL, None);
        let wide = png(12_000, 100);
        r.renderer(wide.clone()).await;
        r.start().await;
        r.delta(&format!("```json-render\n{GRAPH}\n```\n")).await;
        let f = files(&r.api.calls());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].2, wide);
        assert_eq!(crate::api::file_kind(&f[0].1, &f[0].2), FileKind::Document);
        assert_eq!(
            crate::api::file_kind("image/png", &png(1120, 600)),
            FileKind::Photo
        );
    }

    /// 164 D6: a peer's delivered text is painted too.
    #[tokio::test(start_paused = true)]
    async fn delivered_message_carries_its_picture() {
        let mut r = rig(10, None, ALL, None);
        let image = png(1120, 600);
        r.renderer(image.clone()).await;
        r.start().await;
        r.ev(AgentEvent::MessageDelivered {
            session: r.session(),
            from: "grep-logs".into(),
            text: format!("found it\n\n```json-render\n{GRAPH}\n```"),
        })
        .await;
        let calls = r.api.calls();
        assert_eq!(
            sends(&calls),
            vec!["⟵ **grep-logs**\n\nfound it".to_owned()]
        );
        assert_eq!(files(&calls)[0].2, image);
        assert_no_body(&calls);
    }

    /// 164 D9: the voice clip never reads a fence body.
    #[test]
    fn spoken_text_drops_picture_fences() {
        let t = format!("Intro\n\n```json-render\n{GRAPH}\n```\n\nOutro");
        let spoken = surface::strip_pictures(&t);
        assert!(!spoken.contains("root") && spoken.contains("Intro") && spoken.contains("Outro"));
    }
}
