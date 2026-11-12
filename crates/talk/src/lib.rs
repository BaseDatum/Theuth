//! Talk (design doc 76): one live, interruptible spoken conversation between
//! the user and one session. Audio → `voice::live::Utterer` → the session as
//! `[spoken, N s]` text → committed blocks → sentences → Pocket TTS → speaker;
//! a barge-in is an explicit event that cuts playback, the queue and the model
//! turn at once and keeps only what the user heard (76 §0.6, §0.7).
//!
//! Public surface consumed by other plugins and the frontends:
//! - commands on the [`TALK_ACTOR`] actor ([`Talk`]): [`StartTalk`],
//!   [`PushAudio`], [`StopTalk`], [`TalkStatus`]
//! - events: [`TalkEvent`]
//! - config: [`TalkConfig`] (`talk:`)
//!
//! The types, the config and the RPC shape are the contract (76 §4.1,
//! §4.9–§4.11, §5.1, §6); the behaviour is [`Talk`] over the pure
//! [`policy::Policy`]: the actor owns the devices, the lease, the timers and
//! the bus, and does exactly what the policy tells it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use agent::AgentEvent;
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tools::{CloseAck, CloseTree, RegisterLifecycle};
use voice::live::Utterer;
use voice::mic::{Capture, Chunk};
use voice::{ModelState, SpeechId, VoiceEvent};

use policy::{Action, Input, Policy, Timer};

mod policy;
pub mod rpc_support;

/// Registry name of the [`Talk`] actor.
pub const TALK_ACTOR: &str = "talk";

/// One talk (start → stop). Time-ordered like [`SpeechId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct TalkId(pub uuid::Uuid);

impl TalkId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for TalkId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TalkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for TalkId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// Where the agent's speech goes and where audio comes from (76 §0.3):
/// `voice`'s type, so a talk and a `Speak` name the same sink. Wire:
/// `"client" | "device"`.
pub use voice::Sink;
/// Echo cancellation the talk runs with (76 §0.5): reported, not chosen.
/// Wire: `"vpio" | "none"`.
pub use voice::mic::Aec;

/// The talk's state machine (76 §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No talk, or the one just started with nothing heard yet.
    Idle,
    /// The user may speak; the gate decides utterances, the turn policy commits.
    Listening,
    /// A turn was submitted; no audio yet.
    Thinking,
    /// 145 §0.2: a tool call of the turn runs; nothing queued or playing.
    Working,
    /// The agent is being heard.
    Speaking,
    /// Stage 1 of a barge-in: playback paused, queue kept, decision pending.
    Holding,
    /// Stage 2 committed: everything cut, the heard prefix kept.
    Interrupted,
}

/// What stage 1 of a barge-in does to the player (76 §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Pause playback while the classifier decides (default).
    Pause,
    /// Keep speaking until the decision — for measurement, not a preference.
    None,
}

/// Which echo cancellation to ask the mic for (76 §4.10 `talk.aec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AecMode {
    /// VoiceProcessingIO where it exists, plain mic with [`TalkDegradation::MicNoAec`] elsewhere.
    Prefer,
    /// Plain mic even on macOS; for measurements.
    Off,
}

/// End-of-turn model (76 §0.8, §10 step 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TurnModel {
    /// The silence + punctuation rule only.
    None,
    /// Smart Turn v3 on ONNX Runtime; refused until `<data_dir>/models/smart-turn` exists.
    SmartTurn,
}

/// `talk:` config section (76 §4.10).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct TalkConfig {
    /// `false`: no actor, `talk_*` RPC answers "talk is disabled".
    pub enabled: bool,
    /// Stage 1: the gate must stay open this long before playback pauses.
    pub barge_in_min_ms: u64,
    /// Stage 2 fallback: onset → decision deadline when no partial has arrived
    /// (76 §4.4 says 900; raise it where the STT window lands later, e.g. debug builds).
    pub decide_ms: u64,
    /// VAD probability a frame must exceed to count as speech while the agent
    /// speaks (`Speaking`/`Holding`); `Listening`/`Thinking` keep the gate's 0.5.
    pub barge_in_vad: f32,
    /// The turn commits this long after an utterance closes without terminal punctuation.
    pub end_of_turn_ms: u64,
    /// The same, when the utterance ends in `.`, `?` or `!`.
    pub end_of_turn_punct_ms: u64,
    /// Whether stage 1 pauses the player.
    pub hold: Hold,
    /// Words that, alone, are an acknowledgement rather than a cut-in; lowercase, no punctuation.
    pub backchannels: Vec<String>,
    /// Levenshtein ratio at or above which a candidate that prefixes the last two
    /// seconds of the agent's speech is treated as echo.
    pub echo_ratio: f32,
    /// Echo cancellation to ask the mic for.
    pub aec: AecMode,
    /// End-of-turn model.
    pub turn_model: TurnModel,
    /// 145 §0.4: inline tool calls promote to a job after this long; 0 = never.
    pub promote_after_ms: u64,
}

impl Default for TalkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            barge_in_min_ms: 400,
            decide_ms: 900,
            barge_in_vad: 0.7,
            end_of_turn_ms: 700,
            end_of_turn_punct_ms: 250,
            hold: Hold::Pause,
            backchannels: [
                "yeah", "yep", "yes", "mhm", "mm", "mm-hmm", "uh-huh", "ok", "okay", "right",
                "sure", "hm", "hmm", "i see", "got it",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            echo_ratio: 0.8,
            aec: AecMode::Prefer,
            turn_model: TurnModel::None,
            promote_after_ms: 5000,
        }
    }
}

/// Start a talk on `session` (76 §4.1). One per daemon: a second answers
/// `"already talking"`. `sample_rate` is the remote client's PCM rate
/// ([`Sink::Client`] only); `None` = the daemon's microphone.
/// `ask` → `Result<TalkId, String>`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StartTalk {
    /// The session the conversation is with.
    pub session: SessionId,
    /// Where speech goes and where audio comes from.
    pub sink: Sink,
    /// Rate of the PCM the client will push; a multiple of 50 (20 ms chunks).
    pub sample_rate: Option<u32>,
}

/// The remote leg (76 §5.3): mono i16 PCM at the rate given at start, any
/// length, and the highest `SpeechAudio.seq` whose playback ended on the
/// client — the heard-prefix ledger's position for [`Sink::Client`].
/// `ask` → `Result<(), String>`.
#[derive(Clone, Debug)]
pub struct PushAudio {
    /// The talk the audio belongs to.
    pub id: TalkId,
    /// The samples.
    pub pcm: Vec<i16>,
    /// Highest `seq` of the current utterance the client has finished playing.
    pub played_seq: Option<u32>,
}

/// Stop `id`: stops speech, ends the audio task, returns the engine lease.
/// Idempotent. `ask` → `Result<(), String>`.
#[derive(Clone, Copy, Debug)]
pub struct StopTalk {
    /// The talk to stop.
    pub id: TalkId,
}

/// The running talk and the streaming model's state. `ask` → [`TalkState`].
#[derive(Clone, Copy, Debug)]
pub struct TalkStatus;

/// The one live talk, as [`TalkStatus`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Active {
    /// The talk.
    pub id: TalkId,
    /// Its session.
    pub session: SessionId,
    /// Where the state machine is.
    pub state: State,
    /// Echo cancellation it runs with.
    pub aec: Aec,
}

/// Reply to [`TalkStatus`].
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TalkState {
    /// The talk running, if any (one per daemon).
    pub active: Option<Active>,
    /// The streaming STT model's lifecycle (the `Utterer` the talk leases).
    pub model: ModelState,
}

/// Why a hold ended in playback resuming (76 §4.4 stage 2 a/b, and noise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ResumeReason {
    /// Every word of the candidate is in `talk.backchannels`.
    Backchannel,
    /// The candidate is a fuzzy prefix of what the agent just said.
    Echo,
    /// The gate opened on something that never became a word.
    Noise,
}

/// What a talk runs without (76 §0.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TalkDegradation {
    /// No echo cancellation on this host; headphones recommended.
    MicNoAec,
}

/// Why a talk ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum EndReason {
    /// `StopTalk`.
    Stopped,
    /// The frontend connection that started it closed.
    Disconnected,
    /// The session closed (`CloseTree`).
    SessionClosed,
    /// Devices, model or the session failed.
    Error {
        /// What went wrong.
        message: String,
    },
}

/// Bus events (76 §4.9); the RPC bridge forwards them to subscribers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum TalkEvent {
    /// The lease is held and the devices are open.
    TalkStarted {
        /// Talk.
        id: TalkId,
        /// Its session.
        session: SessionId,
        /// Where speech goes.
        sink: Sink,
        /// Echo cancellation in effect.
        aec: Aec,
        /// Milliseconds since the talk started (0 here; the marker's clock).
        since_ms: u64,
    },
    /// Every transition of the state machine (76 §4.2).
    TalkState {
        /// Talk.
        id: TalkId,
        /// New state.
        state: State,
    },
    /// RMS per 20 ms of the last 160 ms of mic audio, 0.0–1.0, linear, oldest first.
    /// Named apart from `MeetingEvent::Level`: both ride the one UI event stream.
    TalkLevel {
        /// Talk.
        id: TalkId,
        /// Eight values, ~6 events/s.
        rms: [f32; 8],
    },
    /// Text of utterance `utterance` so far; `final: true` once, with the closed
    /// utterance's whole text. Partials are for the frontend only.
    Heard {
        /// Talk.
        id: TalkId,
        /// Utterance index within the talk.
        utterance: u32,
        /// Whole utterance so far.
        text: String,
        /// The gate closed this utterance; no more text for it.
        #[serde(rename = "final")]
        is_final: bool,
    },
    /// A sentence of the agent's started playing — the agent subtitle.
    Saying {
        /// Talk.
        id: TalkId,
        /// The sentence's utterance.
        speech: SpeechId,
        /// The sentence, spoken form.
        text: String,
    },
    /// The input the session received, marker included.
    Submitted {
        /// Talk.
        id: TalkId,
        /// `[spoken, N s] …` or `[spoken, N s, interrupted] …`.
        text: String,
        /// The previous turn was cut off by this one.
        interrupted: bool,
    },
    /// Stage 1: the gate has been open `barge_in_min_ms`; playback paused.
    Holding {
        /// Talk.
        id: TalkId,
        /// How far into the agent's speech the hold began.
        since_speech_ms: u64,
    },
    /// Stage 2 (a)/(b): playback resumed from the paused sample.
    Resumed {
        /// Talk.
        id: TalkId,
        /// Why.
        reason: ResumeReason,
    },
    /// Stage 2 (c): the five cuts committed (76 §4.5).
    Interrupted {
        /// Talk.
        id: TalkId,
        /// Sentences of the turn the user heard (kept in history).
        heard_sentences: u32,
        /// Sentences generated or queued but never heard (dropped).
        dropped_sentences: u32,
    },
    /// Remote sink: cut every buffered packet now.
    Flush {
        /// Talk.
        id: TalkId,
    },
    /// At start, on hosts without echo cancellation.
    Degraded {
        /// Talk.
        id: TalkId,
        /// What is missing.
        degradation: TalkDegradation,
    },
    /// The talk ended; devices closed, lease returned.
    TalkFinished {
        /// Talk.
        id: TalkId,
        /// Why.
        reason: EndReason,
    },
    /// 145 §0.6: a harness sentence the talk spoke (approval announce).
    Announced {
        /// Talk.
        id: TalkId,
        /// The sentence, spoken form.
        text: String,
    },
}

impl wire::WireEvent for TalkEvent {
    const TOPIC: &'static str = "talk.events";
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<TalkEvent>().await
}

/// Prompt block registered as `RegisterPrompt { plugin: "talk" }` (76 §0.11).
const PROMPT: &str = "## Live talk\n\
You are talking out loud. Your reply is synthesised to speech and played on the user's speaker \
the moment you write it, so the user and anyone with them hears you aloud, in the room. When the \
user asks you to say something to a person present (\"say hello to my friend\"), just say it in \
your reply — they are listening. Do not send messages, DM, or look up agents or contacts for \
people the user mentions unless they explicitly ask you to send a message elsewhere; you are a \
voice in the room, not a messenger.\n\
A user message that starts with `[spoken, N s]` was said aloud to you just now, in a live \
conversation: what follows is a transcript of N seconds of speech, so read through recognition \
slips and never quote them back. Your reply is spoken to the user sentence by sentence as you \
write it. Write for the ear: short spoken sentences, short paragraphs, the answer first. No \
markdown, lists, headings, tables, code blocks, URLs or file paths; say names the way you would \
say them aloud. Before a batch of tool calls say one short sentence about what you are about to \
do — that sentence is what the user hears while the tools run, and their chance to stop you. \
The user can cut you off at any time. When that happens, your history keeps only the sentences \
they actually heard — your own message is truncated to them — and the next user message starts \
with `[spoken, N s, interrupted]`; carry on from what they heard, do not repeat the rest. Keep \
this spoken style while the messages carry the `[spoken …]` marker.\n\
Aloud, one or two sentences; if there is more, say the rest is on screen. Never say tag names or \
tool identifiers aloud — no angle-bracket tags, no snake_case names; say what happened in plain \
words. A result that arrives while you talk (a job result, a reminder, a message from another \
agent) gets one sentence aloud and an offer of the detail; several results in one message get one \
sentence for all of them. A tool result that says it was promoted to a job means the work goes on \
in the background: say one short sentence such as \"I'll tell you when it's done\", then stop — \
never wait for it or poll. Progress on running work is visual; speak it only when it matters, at \
most one sentence per minute per job — silence is fine. A remark that arrives while you are \
working joins the running turn: acknowledge it in one clause and continue, unless it tells you to \
stop.";

/// Chunks per [`TalkEvent::TalkLevel`] (160 ms).
const LEVELS: usize = 8;
/// How long the audio task waits for a chunk before rechecking cancel.
const POLL: Duration = Duration::from_millis(50);

/// The one talk (76 §4.1).
struct Live {
    id: TalkId,
    session: SessionId,
    sink: Sink,
    aec: Aec,
    policy: Policy,
    /// Ends the audio task.
    cancel: Arc<AtomicBool>,
    /// The gate's speech threshold (`f32` bits), read by the task each chunk.
    vad_threshold: Arc<AtomicU32>,
    /// The remote PCM inbox ([`Sink::Client`]).
    remote: Option<Remote>,
    /// Generation per [`Timer`]; a stale [`Fired`] is dropped.
    timers: [u64; 3],
    /// `Voice::Pause` is in effect.
    paused: bool,
    /// Stop requested; `TalkFinished` follows when the task hands the engine back.
    ending: Option<EndReason>,
    /// The last `TurnStarted`, for the first-sentence latency log (76 §4.8).
    turn_started: Option<Instant>,
    /// Tool calls of the live session's turn still running (145 §0.2).
    tools: u32,
}

/// The actor's end of a client talk's audio: pushed PCM rechunked into 20 ms
/// chunks for the task (the dictation shape).
struct Remote {
    tx: Sender<Chunk>,
    pending: Vec<f32>,
    /// Samples per chunk: `sample_rate / 50`.
    chunk: usize,
}

/// What the audio task tells the actor.
enum Progress {
    /// The gate opened.
    Onset,
    /// The gate closed.
    Close,
    /// A text piece.
    Text {
        utterance: u32,
        text: String,
        is_final: bool,
    },
    /// RMS per chunk of the last 160 ms.
    Level([f32; LEVELS]),
    /// The task ended. `None` engine: lost to a model failure.
    Finished {
        reason: EndReason,
        engine: Box<Option<Utterer>>,
    },
}

/// `tell` to the actor from the task.
struct Report {
    id: TalkId,
    progress: Progress,
}

/// `tell` from a timer task.
struct Fired {
    id: TalkId,
    timer: Timer,
    generation: u64,
}

fn slot(t: Timer) -> usize {
    match t {
        Timer::EndOfTurn => 0,
        Timer::Hold => 1,
        Timer::Decide => 2,
    }
}

fn voice() -> Option<ActorRef<voice::Voice>> {
    ActorRef::lookup(voice::VOICE_ACTOR).ok().flatten()
}

fn dictation() -> Option<ActorRef<voice::Dictation>> {
    ActorRef::lookup(voice::DICTATION_ACTOR).ok().flatten()
}

fn agents() -> Option<ActorRef<agent::Agents>> {
    ActorRef::lookup(agent::AGENT_ACTOR).ok().flatten()
}

/// The [`TALK_ACTOR`] actor: owns the one talk (76 §4.1) — its lease, its
/// devices, its timers — and carries out the [`Policy`]'s decisions.
#[derive(kameo::Actor)]
pub struct Talk {
    rt: Runtime,
    cfg: TalkConfig,
    active: Option<Live>,
}

impl Talk {
    /// No talk running.
    pub fn new(rt: Runtime, cfg: TalkConfig) -> Self {
        Self {
            rt,
            cfg,
            active: None,
        }
    }

    /// Feed the policy one input and do what it says.
    async fn step(&mut self, me: &ActorRef<Self>, input: Input) {
        let now = Instant::now();
        let Some(live) = self.active.as_mut() else {
            return;
        };
        let actions = live.policy.step(input, now);
        self.run(me, actions).await;
    }

    /// Carry out `actions` in order (76 §4.5: one handler, nothing else between them).
    async fn run(&mut self, me: &ActorRef<Self>, actions: Vec<Action>) {
        let mut queue: std::collections::VecDeque<Action> = actions.into();
        while let Some(action) = queue.pop_front() {
            let Some(live) = self.active.as_mut() else {
                return;
            };
            let (id, session, sink) = (live.id, live.session, live.sink);
            // `RUST_LOG=talk=debug`: the state machine's every decision, the five cuts in order.
            tracing::debug!(?action, "talk");
            match action {
                Action::State(state) => self.rt.publish(TalkEvent::TalkState { id, state }).await,
                Action::Heard {
                    utterance,
                    text,
                    is_final,
                } => {
                    self.rt
                        .publish(TalkEvent::Heard {
                            id,
                            utterance,
                            text,
                            is_final,
                        })
                        .await;
                }
                Action::Speak { index, text } => {
                    if index == 0
                        && let Some(t) = live.turn_started
                    {
                        tracing::info!(target: "talk", "talk: first sentence spoken after {} ms (turn started → first Speak)", t.elapsed().as_millis());
                    }
                    let spoke = match voice() {
                        Some(v) => v
                            .ask(voice::Speak {
                                session,
                                text,
                                queue: true,
                                sink,
                            })
                            .await
                            .map_err(|e| e.to_string()),
                        None => Err("voice is disabled".into()),
                    };
                    match spoke {
                        Ok(speech) => live.policy.spoken(index, speech),
                        Err(e) => {
                            tracing::warn!("talk: speak failed: {e}");
                            let more = live.policy.failed(index);
                            queue.extend(more);
                        }
                    }
                }
                Action::Saying { speech, text } => {
                    if let (Some(t), Some(first)) =
                        (live.turn_started, live.policy.ledger.sentences.first())
                        && first.speech == Some(speech)
                    {
                        tracing::info!(target: "talk", "talk: first audio after {} ms", t.elapsed().as_millis());
                    }
                    self.rt
                        .publish(TalkEvent::Saying { id, speech, text })
                        .await;
                }
                Action::Submit { text, interrupted } => {
                    let sent = match agents() {
                        Some(a) => a
                            .ask(agent::SendInput {
                                session: Some(session),
                                text: text.clone(),
                                files: Vec::new(),
                                delivery: agent::Delivery::NextTurn,
                                id: None,
                                channel: None,
                            })
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                        None => Err("agent is not running".into()),
                    };
                    match sent {
                        Ok(()) => {
                            self.rt
                                .publish(TalkEvent::Submitted {
                                    id,
                                    text,
                                    interrupted,
                                })
                                .await;
                        }
                        Err(e) => {
                            tracing::warn!("talk: send_input failed: {e}");
                            // Nothing will answer: back to listening.
                            let more = live
                                .policy
                                .step(Input::TurnFinished("error".into()), Instant::now());
                            queue.extend(more);
                        }
                    }
                }
                Action::Cancel(keep) => {
                    if let Some(a) = agents() {
                        let _ = a
                            .ask(agent::Cancel {
                                session,
                                keep,
                                scope: agent::CancelScope::Root,
                                reason: agent::CancelReason::default(),
                            })
                            .await;
                    }
                }
                Action::Trim { turn, keep } => {
                    if let Some(a) = agents()
                        && let Err(e) = a
                            .ask(agent::TrimAssistant {
                                session,
                                turn,
                                keep,
                            })
                            .await
                    {
                        tracing::warn!(%session, turn, "trim assistant: {e}");
                    }
                }
                Action::StopSpeech => {
                    if sink == Sink::Client {
                        self.rt.publish(TalkEvent::Flush { id }).await;
                    }
                    if let Some(v) = voice() {
                        let _ = v.ask(voice::Stop { id: None }).await;
                    }
                    live.paused = false;
                }
                Action::Pause => {
                    if let Some(v) = voice()
                        && let Ok(played) = v.ask(voice::Pause).await
                    {
                        live.policy.paused(played);
                        live.paused = true;
                    }
                }
                Action::Resume { reason, text } => {
                    if live.paused
                        && let Some(v) = voice()
                    {
                        let _ = v.ask(voice::Resume).await;
                    }
                    live.paused = false;
                    tracing::info!(target: "talk", ?reason, text = text.as_deref().unwrap_or(""), "resumed");
                    self.rt.publish(TalkEvent::Resumed { id, reason }).await;
                }
                Action::Holding { since_speech_ms } => {
                    tracing::info!(target: "talk", since_speech_ms, "holding");
                    self.rt
                        .publish(TalkEvent::Holding {
                            id,
                            since_speech_ms,
                        })
                        .await;
                }
                Action::Commit => {
                    // Where the player is, unless the hold's pause already said.
                    let played = match (sink, live.paused, voice()) {
                        (Sink::Device, false, Some(v)) => v.ask(voice::Position).await.ok(),
                        _ => None,
                    };
                    let cuts = live.policy.commit(played);
                    for a in cuts.into_iter().rev() {
                        queue.push_front(a);
                    }
                }
                Action::Interrupted {
                    heard,
                    dropped,
                    text,
                } => {
                    tracing::info!(target: "talk", heard, dropped, text = text.as_deref().unwrap_or(""), "interrupted");
                    self.rt
                        .publish(TalkEvent::Interrupted {
                            id,
                            heard_sentences: heard,
                            dropped_sentences: dropped,
                        })
                        .await;
                }
                Action::VadThreshold(t) => live.vad_threshold.store(t.to_bits(), Ordering::Relaxed),
                Action::Timer(timer, after) => {
                    let g = &mut live.timers[slot(timer)];
                    *g += 1;
                    if let Some(after) = after {
                        let (me, generation) = (me.clone(), *g);
                        tokio::spawn(async move {
                            tokio::time::sleep(after).await;
                            let _ = me
                                .tell(Fired {
                                    id,
                                    timer,
                                    generation,
                                })
                                .await;
                        });
                    }
                }
                Action::Steer(text) => {
                    let sent = match agents() {
                        Some(a) => a
                            .ask(agent::Steer {
                                session,
                                id: runtime::InputId::new(),
                                text,
                                files: Vec::new(),
                                delivery: agent::Delivery::Steer,
                                channel: None,
                            })
                            .await
                            .map_err(|e| e.to_string()),
                        None => Err("agent is not running".into()),
                    };
                    if let Err(e) = sent {
                        tracing::warn!("talk: steer failed: {e}");
                    }
                }
                Action::Window(open) => self.set_talk(session, Some(open)).await,
                Action::Announced(text) => {
                    tracing::info!(target: "talk", %text, "announced");
                    self.rt.publish(TalkEvent::Announced { id, text }).await;
                }
            }
        }
    }

    /// Stop speech, end the task; the lease returns and `TalkFinished` is
    /// published when the task hands the engine back.
    async fn end(&mut self, reason: EndReason) {
        let Some(live) = self.active.as_mut() else {
            return;
        };
        if live.ending.is_some() {
            return;
        }
        live.ending = Some(reason);
        live.cancel.store(true, Ordering::Relaxed);
        for g in &mut live.timers {
            *g += 1;
        }
        let (id, session) = (live.id, live.session);
        if let Some(v) = voice() {
            let _ = v.ask(voice::Stop { id: None }).await;
        }
        self.set_talk(session, None).await;
        self.rt
            .publish(TalkEvent::TalkState {
                id,
                state: State::Idle,
            })
            .await;
    }

    /// 145 §0.3–0.4: tell the agent the talk's spoken window (`Some(open)`)
    /// or that the talk ended (`None`).
    async fn set_talk(&self, session: SessionId, open: Option<bool>) {
        let live = open.map(|open| agent::TalkLive {
            open,
            promote_after_ms: (self.cfg.promote_after_ms > 0).then_some(self.cfg.promote_after_ms),
        });
        let Some(a) = agents() else {
            tracing::warn!("talk: set_talk skipped: agent is not running");
            return;
        };
        if let Err(e) = a.ask(agent::SetTalk { session, live }).await {
            tracing::warn!("talk: set_talk failed: {e}");
        }
    }
}

impl Message<StartTalk> for Talk {
    type Reply = Result<TalkId, String>;
    async fn handle(&mut self, m: StartTalk, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.active.is_some() {
            return Err("already talking".into());
        }
        let Some(dictation) = dictation().filter(|_| voice().is_some()) else {
            return Err("voice is disabled".into());
        };
        let rate = match (m.sink, m.sample_rate) {
            (Sink::Device, _) => voice::mic::RATE,
            (Sink::Client, Some(rate)) => rate,
            (Sink::Client, None) => return Err("sink: client needs sample_rate".into()),
        };
        // The resident `Utterer` changes hands (76 §0.10); this waits for a load.
        let mut engine = dictation
            .ask(voice::Lease)
            .await
            .map_err(|e| e.to_string())?;
        let opened = engine
            .set_rate(rate)
            .map_err(|e| format!("resampler: {e}"))
            .and_then(|()| match m.sink {
                Sink::Device => {
                    let want = match self.cfg.aec {
                        AecMode::Prefer => Aec::Vpio,
                        AecMode::Off => Aec::None,
                    };
                    Capture::open_with(want)
                        .map(|(cap, rx, aec)| (Some(cap), rx, None, aec))
                        .map_err(|e| format!("microphone: {e:#}"))
                }
                Sink::Client => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let remote = Remote {
                        tx,
                        pending: Vec::new(),
                        chunk: rate as usize / 50,
                    };
                    Ok((None, rx, Some(remote), Aec::None))
                }
            });
        let (cap, rx, remote, aec) = match opened {
            Ok(o) => o,
            Err(e) => {
                let _ = dictation.tell(voice::Return(Some(engine))).await;
                return Err(e);
            }
        };
        let id = TalkId::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let vad_threshold = Arc::new(AtomicU32::new(voice::vad::Gate::THRESHOLD.to_bits()));
        let me = ctx.actor_ref().clone();
        let task = Task {
            me: me.clone(),
            id,
            live: *engine,
            levels: Vec::with_capacity(LEVELS),
            vad_threshold: vad_threshold.clone(),
            applied: voice::vad::Gate::THRESHOLD,
        };
        let stop = cancel.clone();
        tokio::spawn(async move {
            let (reason, engine) = tokio::task::spawn_blocking(move || task.run(cap, rx, &stop))
                .await
                .unwrap_or_else(|e| {
                    (
                        EndReason::Error {
                            message: format!("audio task panicked: {e}"),
                        },
                        None,
                    )
                });
            let _ = me
                .tell(Report {
                    id,
                    progress: Progress::Finished {
                        reason,
                        engine: Box::new(engine),
                    },
                })
                .await;
        });
        self.active = Some(Live {
            id,
            session: m.session,
            sink: m.sink,
            aec,
            policy: Policy::new(self.cfg.clone(), m.sink),
            cancel,
            vad_threshold,
            remote,
            timers: [0; 3],
            paused: false,
            ending: None,
            turn_started: None,
            tools: 0,
        });
        self.rt
            .publish(TalkEvent::TalkStarted {
                id,
                session: m.session,
                sink: m.sink,
                aec,
                since_ms: 0,
            })
            .await;
        if m.sink == Sink::Device && aec == Aec::None {
            self.rt
                .publish(TalkEvent::Degraded {
                    id,
                    degradation: TalkDegradation::MicNoAec,
                })
                .await;
        }
        self.rt
            .publish(TalkEvent::TalkState {
                id,
                state: State::Listening,
            })
            .await;
        let opened = self
            .active
            .as_mut()
            .map(|l| l.policy.start())
            .unwrap_or_default();
        let me = ctx.actor_ref().clone();
        self.run(&me, opened).await;
        Ok(id)
    }
}

impl Message<PushAudio> for Talk {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: PushAudio, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(live) = self.active.as_mut().filter(|l| l.id == m.id) else {
            return Err("not talking".into());
        };
        let Some(remote) = live.remote.as_mut() else {
            return Err("not a client talk".into());
        };
        remote
            .pending
            .extend(m.pcm.iter().map(|&s| f32::from(s) / 32768.0));
        while remote.pending.len() >= remote.chunk {
            let chunk: Vec<f32> = remote.pending.drain(..remote.chunk).collect();
            remote
                .tx
                .send(Ok(chunk))
                .map_err(|_| String::from("not talking"))?;
        }
        if let Some(seq) = m.played_seq {
            let me = ctx.actor_ref().clone();
            self.step(&me, Input::PlayedSeq(seq)).await;
        }
        Ok(())
    }
}

impl Message<StopTalk> for Talk {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: StopTalk, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.active.as_ref().is_some_and(|l| l.id == m.id) {
            self.end(EndReason::Stopped).await;
        }
        Ok(())
    }
}

impl Message<TalkStatus> for Talk {
    type Reply = TalkState;
    async fn handle(&mut self, _: TalkStatus, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // The streaming model is the `Dictation` actor's; the talk only leases it.
        let model = match dictation() {
            Some(d) => d
                .ask(voice::DictationStatus)
                .await
                .map(|s| s.model)
                .unwrap_or_default(),
            None => ModelState::Missing,
        };
        TalkState {
            active: self.active.as_ref().map(|l| Active {
                id: l.id,
                session: l.session,
                state: if l.ending.is_some() {
                    State::Idle
                } else {
                    l.policy.state
                },
                aec: l.aec,
            }),
            model,
        }
    }
}

impl Message<Report> for Talk {
    type Reply = ();
    async fn handle(&mut self, m: Report, ctx: &mut Context<Self, Self::Reply>) {
        let Some(live) = self.active.as_ref().filter(|l| l.id == m.id) else {
            return;
        };
        // A stopping talk's flush still yields text; only `Finished` matters now.
        if live.ending.is_some() && !matches!(m.progress, Progress::Finished { .. }) {
            return;
        }
        let me = ctx.actor_ref().clone();
        match m.progress {
            Progress::Onset => self.step(&me, Input::Onset).await,
            Progress::Close => self.step(&me, Input::Close).await,
            Progress::Text {
                utterance,
                text,
                is_final,
            } => {
                self.step(
                    &me,
                    Input::Text {
                        utterance,
                        text,
                        is_final,
                    },
                )
                .await;
            }
            Progress::Level(rms) => {
                self.rt
                    .publish(TalkEvent::TalkLevel { id: m.id, rms })
                    .await
            }
            Progress::Finished { reason, engine } => {
                let live = self.active.take().expect("checked");

                // The engine goes back; `None` (the model died under the lease)
                // frees the slot and marks the model missing so the next use reloads.
                if let Some(d) = dictation() {
                    let _ = d.tell(voice::Return((*engine).map(Box::new))).await;
                }
                if let Some(v) = voice() {
                    let _ = v.ask(voice::Stop { id: None }).await;
                }
                self.rt
                    .publish(TalkEvent::TalkFinished {
                        id: live.id,
                        reason: live.ending.unwrap_or(reason),
                    })
                    .await;
            }
        }
    }
}

impl Message<Fired> for Talk {
    type Reply = ();
    async fn handle(&mut self, m: Fired, ctx: &mut Context<Self, Self::Reply>) {
        let live = self
            .active
            .as_ref()
            .filter(|l| l.id == m.id && l.ending.is_none());
        if live.is_some_and(|l| l.timers[slot(m.timer)] == m.generation) {
            let me = ctx.actor_ref().clone();
            self.step(&me, Input::Timer(m.timer)).await;
        }
    }
}

impl Message<AgentEvent> for Talk {
    type Reply = ();
    async fn handle(&mut self, ev: AgentEvent, ctx: &mut Context<Self, Self::Reply>) {
        let Some(live) = self.active.as_mut().filter(|l| l.ending.is_none()) else {
            return;
        };
        let input = match ev {
            AgentEvent::TextDelta { session, text, .. } if session == live.session => {
                Input::Delta(text.to_string())
            }
            AgentEvent::ToolCallPending { session, .. } if session == live.session => {
                Input::BlockEnd
            }
            // 145 §0.2: outstanding calls of the turn, as a running count — a
            // `Finished` for a call that began before the talk cannot go below 0.
            AgentEvent::ToolCallStarted { session, .. } if session == live.session => {
                live.tools += 1;
                Input::Tools(live.tools)
            }
            AgentEvent::ToolCallFinished { session, .. } if session == live.session => {
                live.tools = live.tools.saturating_sub(1);
                Input::Tools(live.tools)
            }
            AgentEvent::TurnStarted {
                session, turn_seq, ..
            } if session == live.session => {
                live.turn_started = Some(Instant::now());
                live.tools = 0;
                Input::TurnStarted(turn_seq)
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } if session == live.session => {
                live.tools = 0;
                Input::TurnFinished(stop_reason)
            }
            _ => return,
        };
        let me = ctx.actor_ref().clone();
        self.step(&me, input).await;
    }
}

/// 145 §0.6: an approval on the talk's session is announced, not answered.
/// The sentence names the tool and, for `shell`, the command cut to 80 chars,
/// after the secret scrubber.
impl Message<tools::ApprovalRequested> for Talk {
    type Reply = ();
    async fn handle(&mut self, m: tools::ApprovalRequested, ctx: &mut Context<Self, Self::Reply>) {
        if !self
            .active
            .as_ref()
            .is_some_and(|l| l.ending.is_none() && l.session == m.session)
        {
            return;
        }
        let me = ctx.actor_ref().clone();
        self.step(&me, Input::Approval(approval_summary(&m.tool, &m.args)))
            .await;
    }
}

/// `"{tool}"`, plus `": {command}"` for `shell` (≤ 80 chars), secrets redacted.
fn approval_summary(tool: &str, args: &serde_json::Value) -> String {
    let mut s = tool.to_owned();
    if tool == "shell"
        && let Some(cmd) = args.get("command").and_then(|c| c.as_str())
    {
        let cmd = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut cut: String = cmd.chars().take(80).collect();
        if cut.len() < cmd.len() {
            cut.push('…');
        }
        s = format!("{s}: {cut}");
    }
    tools::secrets::redact(&s).into_owned()
}

impl Message<VoiceEvent> for Talk {
    type Reply = ();
    async fn handle(&mut self, ev: VoiceEvent, ctx: &mut Context<Self, Self::Reply>) {
        let Some(live) = self.active.as_ref().filter(|l| l.ending.is_none()) else {
            return;
        };
        let input = match ev {
            VoiceEvent::SpeechStarted { id, session, .. } if session == live.session => {
                Input::SpeechStarted(id)
            }
            VoiceEvent::SpeechFinished {
                id,
                session,
                reason,
                ..
            } if session == live.session => Input::SpeechFinished {
                speech: id,
                done: reason == voice::FinishReason::Done,
            },
            VoiceEvent::SpeechAudio {
                id, session, seq, ..
            } if session == live.session && live.sink == Sink::Client => {
                Input::Packet { speech: id, seq }
            }
            _ => return,
        };
        let me = ctx.actor_ref().clone();
        self.step(&me, input).await;
    }
}

impl Message<CloseTree> for Talk {
    type Reply = CloseAck;
    /// The talk's session is closing (76 §0.10): end it.
    async fn handle(&mut self, m: CloseTree, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self
            .active
            .as_ref()
            .is_some_and(|l| m.subtree.contains(&l.session))
        {
            self.end(EndReason::SessionClosed).await;
        }
        CloseAck::ok("talk")
    }
}

/// The audio task (76 §4.2): one blocking thread reading the source, the VAD
/// running in every state, edges and text told to the actor.
struct Task {
    me: ActorRef<Talk>,
    id: TalkId,
    live: Utterer,
    levels: Vec<f32>,
    /// The actor's wish (`f32` bits): `talk.barge_in_vad` while the agent speaks.
    vad_threshold: Arc<AtomicU32>,
    /// What the gate has; the lease goes back at [`voice::vad::Gate::THRESHOLD`].
    applied: f32,
}

impl Task {
    fn report(&self, progress: Progress) {
        let _ = self
            .me
            .tell(Report {
                id: self.id,
                progress,
            })
            .blocking_send();
    }

    /// Read until cancel or failure; flush; hand the engine back.
    fn run(
        mut self,
        cap: Option<Capture>,
        rx: Receiver<Chunk>,
        cancel: &AtomicBool,
    ) -> (EndReason, Option<Utterer>) {
        let reason = loop {
            if cancel.load(Ordering::Relaxed) || !self.me.is_alive() {
                break Ok(EndReason::Stopped);
            }
            let chunk = match rx.recv_timeout(POLL) {
                Ok(Ok(c)) => c,
                Ok(Err(m)) => {
                    break Ok(EndReason::Error {
                        message: format!("microphone: {m}"),
                    });
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    break Ok(EndReason::Error {
                        message: "audio source closed".into(),
                    });
                }
            };
            if let Err(e) = self.chunk(&chunk) {
                break Err(e);
            }
        };
        drop(cap);
        self.live.set_vad_threshold(voice::vad::Gate::THRESHOLD);
        match reason.and_then(|r| self.live.close().map(|texts| (r, texts))) {
            Ok((reason, texts)) => {
                self.texts(texts);
                self.live.reset();
                (reason, Some(self.live))
            }
            Err(e) => {
                let message = format!("{e:#}");
                tracing::warn!("talk: audio: {message}");
                (EndReason::Error { message }, None)
            }
        }
    }

    /// One 20 ms chunk: level, edges, text.
    fn chunk(&mut self, pcm: &[f32]) -> anyhow::Result<()> {
        let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
        self.levels.push(rms.min(1.0));
        if self.levels.len() == LEVELS {
            let levels = std::mem::take(&mut self.levels);
            if let Ok(rms) = <[f32; LEVELS]>::try_from(levels) {
                self.report(Progress::Level(rms));
            }
        }
        let wanted = f32::from_bits(self.vad_threshold.load(Ordering::Relaxed));
        if wanted != self.applied {
            self.live.set_vad_threshold(wanted);
            self.applied = wanted;
            tracing::debug!(threshold = wanted, "talk: vad threshold");
        }
        let was_open = self.live.is_open();
        let texts = self.live.chunk(pcm)?;
        // An utterance is at least ONSET + CLOSE long: one edge per chunk at most.
        match (was_open, self.live.is_open()) {
            (false, true) => self.report(Progress::Onset),
            (true, false) => self.report(Progress::Close),
            _ => {}
        }
        self.texts(texts);
        Ok(())
    }

    fn texts(&self, texts: Vec<voice::live::Text>) {
        for t in texts {
            self.report(Progress::Text {
                utterance: t.utterance,
                text: t.text,
                is_final: t.is_final,
            });
        }
    }
}

/// The plugin (76 §4): registers [`Talk`] as [`TALK_ACTOR`], subscribes it
/// to the agent's and voice's events, joins the session lifecycle and
/// registers the `[spoken, N s]` prompt block when `talk.enabled`.
pub struct TalkPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for TalkPlugin {
    fn name(&self) -> &'static str {
        "talk"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: TalkConfig = rt.section("talk").unwrap_or_else(|e| {
            tracing::debug!("no [talk] config section, using defaults: {e}");
            TalkConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("talk disabled by config");
            return Ok(());
        }
        let actor =
            Talk::spawn_with_mailbox(Talk::new(rt.clone(), cfg), runtime::subscriber_mailbox());
        actor.register(TALK_ACTOR)?;
        rt.subscribe::<Talk, AgentEvent>(&actor).await;
        rt.subscribe::<Talk, VoiceEvent>(&actor).await;
        rt.subscribe::<Talk, tools::ApprovalRequested>(&actor).await;
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(RegisterLifecycle {
                        owner: "talk",
                        opened: None,
                        close: actor.reply_recipient(),
                    })
                    .await;
                let _ = reg
                    .ask(tools::RegisterPrompt {
                        plugin: "talk",
                        text: PROMPT.to_owned(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape frontends code against (76 §4.9, §5.1): tagged events,
    /// snake_case enums, `final` on `Heard`, transparent ids.
    #[test]
    fn wire_shape() {
        let id = TalkId::new();
        let v = serde_json::to_value(TalkEvent::Heard {
            id,
            utterance: 2,
            text: "hi".into(),
            is_final: true,
        })
        .unwrap();
        assert_eq!(v["type"], "Heard");
        assert_eq!(v["final"], true);
        assert_eq!(v["id"], id.to_string());

        let v = serde_json::to_value(TalkEvent::TalkFinished {
            id,
            reason: EndReason::Error {
                message: "mic".into(),
            },
        })
        .unwrap();
        assert_eq!(v["reason"]["reason"], "error");
        assert_eq!(v["reason"]["message"], "mic");

        let s: StartTalk = serde_json::from_str(&format!(
            r#"{{"session":"{}","sink":"client","sample_rate":48000}}"#,
            SessionId::new()
        ))
        .unwrap();
        assert_eq!(s.sink, Sink::Client);
        assert_eq!(serde_json::to_value(Sink::Device).unwrap(), "device");
        assert_eq!(serde_json::to_value(State::Listening).unwrap(), "listening");
        assert_eq!(serde_json::to_value(Aec::None).unwrap(), "none");
        assert_eq!(serde_json::to_value(Aec::Vpio).unwrap(), "vpio");
        assert_eq!(id.to_string().parse::<TalkId>().unwrap(), id);
        // 145 §2.3.
        assert_eq!(serde_json::to_value(State::Working).unwrap(), "working");
        let v = serde_json::to_value(TalkEvent::Announced {
            id,
            text: "Waiting.".into(),
        })
        .unwrap();
        assert_eq!(v["type"], "Announced");
        assert_eq!(v["text"], "Waiting.");
    }

    /// Every §4.10 enum key deserializes and the defaults are the doc's.
    #[test]
    fn config_defaults() {
        let cfg: TalkConfig =
            serde_json::from_str(r#"{"hold":"none","aec":"off","turn_model":"smart-turn"}"#)
                .unwrap();
        assert_eq!(
            (cfg.hold, cfg.aec, cfg.turn_model),
            (Hold::None, AecMode::Off, TurnModel::SmartTurn)
        );
        assert_eq!(cfg.barge_in_min_ms, 400);
        assert_eq!((cfg.end_of_turn_ms, cfg.end_of_turn_punct_ms), (700, 250));
        assert_eq!(cfg.barge_in_vad, 0.7);
        assert_eq!(cfg.backchannels.len(), 15);
        assert!(cfg.enabled);
        assert_eq!(cfg.promote_after_ms, 5000);
    }

    /// FIX-1: the model is a voice in the room — it says things, it does not
    /// message people the user mentions.
    #[test]
    fn prompt_says_it_is_heard_aloud_and_not_a_messenger() {
        assert!(PROMPT.starts_with("## Live talk\n"));
        assert!(PROMPT.contains("aloud"));
        assert!(PROMPT.contains("Do not send messages"));
        // 145 §0.7–0.9 and the prompt lines of §0.2–0.4.
        for rule in [
            "the rest is on screen",
            "Never say tag names or tool identifiers aloud",
            "one sentence for all of them",
            "promoted to a job",
            "never wait for it or poll",
            "one sentence per minute per job",
            "joins the running turn",
        ] {
            assert!(PROMPT.contains(rule), "{rule}");
        }
    }

    /// 145 §0.6: the announce names the tool and, for `shell`, the command
    /// cut to 80 chars with whitespace collapsed; other tools' args stay private.
    #[test]
    fn approval_summary_names_tool_and_short_command() {
        assert_eq!(
            approval_summary("write_file", &serde_json::json!({"path": "/etc/x"})),
            "write_file"
        );
        assert_eq!(
            approval_summary(
                "shell",
                &serde_json::json!({"command": "git  push\n origin main"})
            ),
            "shell: git push origin main"
        );
        let long = "x".repeat(100);
        let s = approval_summary("shell", &serde_json::json!({ "command": long }));
        assert_eq!(s.chars().count(), "shell: ".len() + 81);
        assert!(s.ends_with('…'));
        assert_eq!(approval_summary("shell", &serde_json::json!({})), "shell");
    }
}
