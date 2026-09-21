//! Read replies aloud (design doc 43): Pocket TTS runs in the harness via
//! `ptts`; audio leaves as [`VoiceEvent::SpeechAudio`] — one Opus packet per
//! 80 ms Mimi frame, base64 in JSON — over the bus. Voice notes (design doc
//! 45): [`Transcriber`] turns a recorded file into text for the channels, and
//! [`Synthesize`] renders one prose block into an MP3 [`Clip`] they can send back.
//! Dictation (design doc 54): [`Dictation`] owns a streaming Nemotron session fed
//! by the microphone or by PCM a remote client pushes ([`PushAudio`]); text lands
//! at the frontend's caret as [`VoiceEvent::DictationText`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use ptts::synth::Synth;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tools::{CloseAck, CloseTree, RegisterLifecycle};

mod clip;
mod dictation;
pub mod live;
pub mod mic;
pub mod model;
mod speak;
mod stt;
pub mod vad;

pub use dictation::{
    AudioSource, Dictation, DictationState, DictationStatus, Prewarm, PushAudio, StartDictation,
    StopDictation,
};
use kameo::reply::ReplySender;
use speak::{Progress, Report};
pub use stt::{Engine, PIECE, Piece, Transcriber, Word, decode, resample};

/// Actor name the RPC layer looks up for live dictation (54 §4.3).
pub const DICTATION_ACTOR: &str = "dictation";

/// The Nemotron streaming checkpoint under `<data_dir>/models/` (54 §9): the
/// dictation default, and the meeting plugin's default `live_model_dir` —
/// one checkpoint for both, without either reading the other's config section.
pub const STREAMING_MODEL_SUBDIR: &str = "nemotron-streaming-en";

/// The Parakeet TDT checkpoint under `<data_dir>/models/` (45 §4.6): the
/// `Transcriber` default and the meeting plugin's transcript-of-record engine.
pub const TDT_MODEL_SUBDIR: &str = "parakeet-tdt";

/// One dictation (mic open → mic closed). Time-ordered like [`SpeechId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct DictationId(pub uuid::Uuid);

impl DictationId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for DictationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DictationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for DictationId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// Actor name the channels look up for speech-to-text (45 §4.2).
pub const TRANSCRIBER_ACTOR: &str = "transcriber";

/// Transcribe the audio file at `path` (any container symphonia demuxes, or
/// Opus in webm/ogg). Replies once inference is done; `Err` is a message.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Transcribe {
    /// Audio file on this machine; the extension picks the demuxer.
    pub path: PathBuf,
}

/// Reply to [`Transcribe`].
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Transcript {
    /// What was said; empty when nothing was recognized.
    pub text: String,
    /// Length of the audio.
    pub duration_ms: u64,
}

/// Render one prose block (markdown) into a [`Clip`] (45 §4.3). Waits for the
/// model to load if it has not; `Ok(None)` when there is nothing to say.
#[derive(Clone, Debug)]
pub struct Synthesize {
    /// Markdown; `markdown::spoken` turns it into sentences.
    pub text: String,
}

/// One voice message: MP3, 24 kHz mono, 48 kbit/s.
#[derive(Clone, Debug, kameo::Reply)]
pub struct Clip {
    /// The encoded file.
    pub bytes: Vec<u8>,
    /// Always `audio/mpeg`.
    pub mime: &'static str,
    /// Length of the audio.
    pub duration_ms: u64,
}

/// The user line a channel sends for a transcribed note (45 §4.4):
/// `[voice note, 12 s] <transcript>`. The prompt block explains the prefix.
pub fn note_text(transcript: &str, duration_ms: u64) -> String {
    format!(
        "[voice note, {} s] {transcript}",
        duration_ms.div_ceil(1000).max(1)
    )
}

/// Whether a user line came from [`note_text`].
pub fn is_note(text: &str) -> bool {
    text.trim_start().starts_with("[voice note")
}

/// Prompt block registered as `RegisterPrompt { plugin: "voice" }` (45 §4.4).
const PROMPT: &str = "## Voice notes\n\
A user message that starts with `[voice note, N s]` was spoken, not typed; what follows is a \
transcript, so read through recognition slips and do not quote them back. Your prose goes back to \
the user as speech on the same channel, block by block: everything you write before a batch of \
tool calls is delivered the moment those calls start, and the rest when the turn ends. Write for \
the ear — short and conversational, a few sentences, just enough to carry the point. No markdown, \
lists, headings, tables, code blocks, URLs or file paths; say names the way you would say them \
aloud. On work that takes several steps, say one short sentence about what you are about to do \
before each batch of tool calls, so the user hears progress instead of silence. Keep this spoken \
style while the user keeps sending voice notes; go back to your normal style when they type.";

/// Actor name the RPC layer looks up.
pub const VOICE_ACTOR: &str = "voice";

/// One utterance. Time-ordered like [`runtime::InputId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SpeechId(pub uuid::Uuid);

impl SpeechId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for SpeechId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SpeechId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for SpeechId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// Speak `text` (markdown) in `session`'s name. Stops the current utterance
/// first. Replies at once with the id; audio follows as events.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Speak {
    /// Session the utterance belongs to (events carry it for subscription filters).
    pub session: SessionId,
    /// Markdown to read; `markdown::spoken` turns it into sentences.
    pub text: String,
}

/// Stop `id`, or whatever is speaking when `None`. Idempotent.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stop {
    /// Utterance to stop; `None` stops the current one.
    pub id: Option<SpeechId>,
}

/// Model state, voices, what is speaking.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Status;

/// Reply to [`Status`].
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VoiceStatus {
    /// Where the model is in its lifecycle.
    pub model: ModelState,
    /// The configured voice.
    pub voice: String,
    /// Every voice the checkpoint ships.
    pub voices: Vec<String>,
    /// The utterance in progress, if any.
    pub speaking: Option<SpeechId>,
}

/// Model lifecycle.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    /// Not on disk.
    #[default]
    Missing,
    /// Reading weights into memory.
    Loading,
    /// Resident; `Speak` starts synthesis at once.
    Ready,
    /// The last load failed; the next `Speak` retries.
    Failed {
        /// What went wrong.
        message: String,
    },
}

/// Why an utterance ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FinishReason {
    /// Every sample has been published (playback may still have seconds to go).
    Done,
    /// Cut short by `Stop` or a replacing `Speak`.
    Stopped,
    /// Synthesis failed.
    Error {
        /// What went wrong.
        message: String,
    },
}

/// Bus events; the RPC bridge forwards them to subscribers.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum VoiceEvent {
    /// Model lifecycle; the desktop shows a toast while `Loading`.
    VoiceModel {
        /// New state.
        state: ModelState,
    },
    /// The first packet is about to arrive.
    SpeechStarted {
        /// Utterance.
        id: SpeechId,
        /// Session it belongs to.
        session: SessionId,
        /// PCM rate the packets decode to (24 000).
        sample_rate: u32,
        /// Length of the spoken text, for a progress estimate.
        chars: u32,
    },
    /// One Opus packet (RFC 6716), mono, 80 ms = 1920 samples at `sample_rate`, base64.
    SpeechAudio {
        /// Utterance.
        id: SpeechId,
        /// Session it belongs to.
        session: SessionId,
        /// Starts at 0, no gaps.
        seq: u32,
        /// Base64 of one Opus packet.
        opus: Arc<str>,
    },
    /// Synthesis ended.
    SpeechFinished {
        /// Utterance.
        id: SpeechId,
        /// Session it belongs to.
        session: SessionId,
        /// How it ended.
        reason: FinishReason,
    },
    /// Streaming (dictation) model lifecycle; distinct from `VoiceModel`, which is TTS.
    DictationModel {
        /// New state.
        state: ModelState,
    },
    /// The mic is open and the VAD is listening. Frontends switch to the
    /// dictating view on this, not on the RPC reply.
    DictationStarted {
        /// Dictation.
        id: DictationId,
    },
    /// RMS per 20 ms of the last 160 ms of mic audio, 0.0–1.0, linear. 8 values, ~6 events/s.
    DictationLevel {
        /// Dictation.
        id: DictationId,
        /// Oldest first.
        levels: Vec<f32>,
    },
    /// Text of utterance `utterance` (0-based within this dictation) so far. Append-only per
    /// utterance; `final: true` is sent once, with the closed utterance's full text (may be
    /// empty → the UI drops it).
    DictationText {
        /// Dictation.
        id: DictationId,
        /// Utterance index within the dictation.
        utterance: u32,
        /// Whole utterance so far.
        text: String,
        /// The VAD closed this utterance; no more text for it.
        #[serde(rename = "final")]
        is_final: bool,
    },
    /// Mic closed. `Done` after a stop, `Stopped` after the max-length stop, `Error` for mic/model failure.
    DictationFinished {
        /// Dictation.
        id: DictationId,
        /// How it ended.
        reason: FinishReason,
    },
}

/// `voice:` config section.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct VoiceConfig {
    /// `false`: no actor, `speak` answers "not running", the desktop shows no button.
    pub enabled: bool,
    /// Voice name; `voice_status` lists the available ones.
    pub voice: String,
    /// A local Pocket TTS checkpoint directory; `None` → `<data_dir>/models/pocket-tts`.
    pub model_dir: Option<PathBuf>,
    /// A local Parakeet TDT ONNX checkpoint (encoder-model.onnx, decoder_joint-model.onnx,
    /// vocab.txt); `None` → `<data_dir>/models/parakeet-tdt` (45 §4.6).
    pub stt_model_dir: Option<PathBuf>,
    /// Unload the speech-to-text model after this long unused; `0` keeps it resident (45 §0.7).
    pub stt_idle_secs: u64,
    /// A local Nemotron streaming ONNX checkpoint (encoder.onnx, decoder_joint.onnx,
    /// tokenizer.model); `None` → `<data_dir>/models/nemotron-streaming-en` (54 §9).
    pub dictation_model_dir: Option<PathBuf>,
    /// A running dictation stops itself after this long (54 §0.10).
    pub dictation_max_secs: u64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            voice: "peter_yearsley".into(),
            model_dir: None,
            stt_model_dir: None,
            stt_idle_secs: 600,
            dictation_model_dir: None,
            dictation_max_secs: 300,
        }
    }
}

/// The utterance in progress: a running task, or one waiting for the model.
struct Current {
    id: SpeechId,
    session: SessionId,
    /// `Some` once synthesis started; cancelling it drops the ptts stream.
    cancel: Option<CancellationToken>,
    /// Markdown still to speak, while the model is loading.
    text: Option<String>,
}

/// `tell` from the load task: what `model::load` produced.
struct Loaded(anyhow::Result<Synth>);

/// The [`VOICE_ACTOR`] actor: owns the model and the one utterance at a time (43 §0.6).
#[derive(kameo::Actor)]
pub struct Voice {
    rt: Runtime,
    cfg: VoiceConfig,
    /// Resident once loaded; never unloaded (43 §4.3).
    model: Option<Arc<Synth>>,
    model_state: ModelState,
    voices: Vec<String>,
    current: Option<Current>,
    /// [`Synthesize`] requests that arrived before the model loaded (45 §4.3).
    pending_clips: Vec<(String, ClipReply)>,
    /// One clip renders at a time; two channels talking at once share the CPU in turn, not in halves.
    clip_gate: Arc<tokio::sync::Semaphore>,
}

type ClipReply = ReplySender<Result<Option<Clip>, String>>;

impl Voice {
    /// Publish `Stopped` for the current utterance (running or pending) and forget it.
    async fn stop_current(&mut self) {
        let Some(cur) = self.current.take() else {
            return;
        };
        if let Some(cancel) = cur.cancel {
            cancel.cancel();
        }
        self.rt
            .publish(VoiceEvent::SpeechFinished {
                id: cur.id,
                session: cur.session,
                reason: FinishReason::Stopped,
            })
            .await;
    }

    /// Render `spoken` into a clip on the loaded model, off the actor, one at a time.
    fn clip(&self, synth: Arc<Synth>, spoken: String, tx: ClipReply) {
        let voice = self.cfg.voice.clone();
        let gate = self.clip_gate.clone();
        tokio::spawn(async move {
            let Ok(_permit) = gate.acquire_owned().await else {
                return;
            };
            let r = tokio::task::spawn_blocking(move || clip::render(&synth, &voice, &spoken))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("clip task panicked: {e}")));
            tx.send(r.map(Some).map_err(|e| format!("{e:#}")));
        });
    }

    /// Start synthesizing `text` for the utterance `id` on the loaded model.
    fn start(
        &mut self,
        synth: Arc<Synth>,
        id: SpeechId,
        session: SessionId,
        text: String,
        me: ActorRef<Voice>,
    ) {
        let cancel = CancellationToken::new();
        self.current = Some(Current {
            id,
            session,
            cancel: Some(cancel.clone()),
            text: None,
        });
        let voice = self.cfg.voice.clone();
        tokio::task::spawn_blocking(move || speak::run(synth, voice, id, text, cancel, me));
    }

    /// Locate and load the checkpoint off the runtime; the result comes back as [`Loaded`].
    async fn load(&mut self, me: ActorRef<Voice>) {
        self.model_state = ModelState::Loading;
        self.rt
            .publish(VoiceEvent::VoiceModel {
                state: ModelState::Loading,
            })
            .await;
        let dir = self
            .cfg
            .model_dir
            .clone()
            .unwrap_or_else(|| self.rt.data_dir.join("models/pocket-tts"));
        tokio::spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || model::load(&model::locate(&dir)?))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("model load task panicked: {e}")));
            let _ = me.tell(Loaded(loaded)).await;
        });
    }
}

impl Message<Synthesize> for Voice {
    type Reply = kameo::reply::DelegatedReply<Result<Option<Clip>, String>>;
    async fn handle(&mut self, m: Synthesize, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let spoken = markdown::spoken(&m.text);
        if spoken.is_empty() {
            tx.send(Ok(None));
            return reply;
        }
        match self.model.clone() {
            Some(synth) => self.clip(synth, spoken, tx),
            None => {
                self.pending_clips.push((spoken, tx));
                if self.model_state != ModelState::Loading {
                    self.load(ctx.actor_ref().clone()).await;
                }
            }
        }
        reply
    }
}

impl Message<Speak> for Voice {
    type Reply = Result<SpeechId, String>;
    async fn handle(&mut self, m: Speak, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = SpeechId::new();
        self.stop_current().await;
        match self.model.clone() {
            Some(synth) => self.start(synth, id, m.session, m.text, ctx.actor_ref().clone()),
            None => {
                self.current = Some(Current {
                    id,
                    session: m.session,
                    cancel: None,
                    text: Some(m.text),
                });
                if self.model_state != ModelState::Loading {
                    self.load(ctx.actor_ref().clone()).await;
                }
            }
        }
        Ok(id)
    }
}

impl Message<Stop> for Voice {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: Stop, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let speaking = self.current.as_ref().map(|c| c.id);
        if speaking.is_some() && m.id.is_none_or(|id| Some(id) == speaking) {
            self.stop_current().await;
        }
        Ok(())
    }
}

impl Message<CloseTree> for Voice {
    type Reply = CloseAck;

    /// A closing tree (51 §4.5): stop the utterance if it belongs to a
    /// closing session (same path as [`Stop`]). Voice keeps nothing else per
    /// session and writes no session files; clips are returned as bytes.
    async fn handle(&mut self, m: CloseTree, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self
            .current
            .as_ref()
            .is_some_and(|c| m.subtree.contains(&c.session))
        {
            self.stop_current().await;
        }
        CloseAck::ok("voice")
    }
}

impl Message<Status> for Voice {
    type Reply = VoiceStatus;
    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        VoiceStatus {
            model: self.model_state.clone(),
            voice: self.cfg.voice.clone(),
            voices: self.voices.clone(),
            speaking: self.current.as_ref().map(|c| c.id),
        }
    }
}

impl Message<Loaded> for Voice {
    type Reply = ();
    async fn handle(&mut self, m: Loaded, ctx: &mut Context<Self, Self::Reply>) {
        match m.0 {
            Ok(synth) => {
                self.voices = synth.voices();
                let synth = Arc::new(synth);
                self.model = Some(synth.clone());
                self.model_state = ModelState::Ready;
                self.rt
                    .publish(VoiceEvent::VoiceModel {
                        state: ModelState::Ready,
                    })
                    .await;
                if let Some(cur) = self.current.take()
                    && let Some(text) = cur.text
                {
                    self.start(
                        synth.clone(),
                        cur.id,
                        cur.session,
                        text,
                        ctx.actor_ref().clone(),
                    );
                }
                for (spoken, tx) in std::mem::take(&mut self.pending_clips) {
                    self.clip(synth.clone(), spoken, tx);
                }
            }
            Err(e) => {
                let message = format!("{e:#}");
                tracing::warn!("voice: {message}");
                for (_, tx) in self.pending_clips.drain(..) {
                    tx.send(Err(message.clone()));
                }
                self.model_state = ModelState::Failed {
                    message: message.clone(),
                };
                self.rt
                    .publish(VoiceEvent::VoiceModel {
                        state: self.model_state.clone(),
                    })
                    .await;
                if let Some(cur) = self.current.take() {
                    self.rt
                        .publish(VoiceEvent::SpeechFinished {
                            id: cur.id,
                            session: cur.session,
                            reason: FinishReason::Error { message },
                        })
                        .await;
                }
            }
        }
    }
}

impl Message<Report> for Voice {
    type Reply = ();
    async fn handle(&mut self, m: Report, _: &mut Context<Self, Self::Reply>) {
        // Late frames after `Stop` or a replacing `Speak`.
        let Some(cur) = self.current.as_ref().filter(|c| c.id == m.id) else {
            return;
        };
        let (id, session) = (cur.id, cur.session);
        let event = match m.progress {
            Progress::Started { chars } => VoiceEvent::SpeechStarted {
                id,
                session,
                sample_rate: speak::SAMPLE_RATE,
                chars,
            },
            Progress::Audio { seq, opus } => VoiceEvent::SpeechAudio {
                id,
                session,
                seq,
                opus,
            },
            Progress::Finished(reason) => {
                self.current = None;
                VoiceEvent::SpeechFinished {
                    id,
                    session,
                    reason,
                }
            }
        };
        self.rt.publish(event).await;
    }
}

/// The plugin (43 §4): registers [`Voice`] as [`VOICE_ACTOR`]. The model loads
/// on the first `Speak`, not at start.
pub struct VoicePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for VoicePlugin {
    fn name(&self) -> &'static str {
        "voice"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: VoiceConfig = rt.section("voice").unwrap_or_else(|e| {
            tracing::debug!("no [voice] config section, using defaults: {e}");
            VoiceConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("voice disabled by config");
            return Ok(());
        }
        let stt_dir = cfg
            .stt_model_dir
            .clone()
            .unwrap_or_else(|| rt.data_dir.join("models").join(TDT_MODEL_SUBDIR));
        let stt = Transcriber::spawn(Transcriber::new(
            stt_dir,
            rt.data_dir.clone(),
            Duration::from_secs(cfg.stt_idle_secs),
        ));
        stt.register(TRANSCRIBER_ACTOR)?;
        let dict_dir = cfg
            .dictation_model_dir
            .clone()
            .unwrap_or_else(|| rt.data_dir.join("models").join(STREAMING_MODEL_SUBDIR));
        let dictation = Dictation::spawn(Dictation::new(
            rt.clone(),
            dict_dir,
            Duration::from_secs(cfg.dictation_max_secs),
        ));
        dictation.register(DICTATION_ACTOR)?;
        let actor = Voice::spawn(Voice {
            rt: rt.clone(),
            cfg,
            model: None,
            model_state: ModelState::Missing,
            voices: Vec::new(),
            current: None,
            pending_clips: Vec::new(),
            clip_gate: Arc::new(tokio::sync::Semaphore::new(1)),
        });
        actor.register(VOICE_ACTOR)?;
        // What `[voice note, N s]` means (45 §4.4); static, so cache-safe.
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(RegisterLifecycle {
                        owner: "voice",
                        opened: None,
                        close: actor.reply_recipient(),
                    })
                    .await;
                let _ = reg
                    .ask(tools::RegisterPrompt {
                        plugin: "voice",
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

    #[test]
    fn note_marker() {
        assert_eq!(note_text("hi", 12_400), "[voice note, 13 s] hi");
        assert_eq!(note_text("", 0), "[voice note, 1 s] ");
        assert!(is_note("  [voice note, 3 s] x"));
        assert!(!is_note("[attachment: a.m4a]"));
    }

    /// An actor with `current` set to an utterance of `session` waiting for
    /// the model (no synthesis task to cancel, no model to load).
    fn speaking(session: SessionId) -> (ActorRef<Voice>, SpeechId) {
        let id = SpeechId::new();
        let actor = Voice::spawn(Voice {
            rt: Runtime::new(figment::Figment::new(), std::env::temp_dir()),
            cfg: VoiceConfig::default(),
            model: None,
            model_state: ModelState::Missing,
            voices: Vec::new(),
            current: Some(Current {
                id,
                session,
                cancel: None,
                text: Some("hello".to_owned()),
            }),
            pending_clips: Vec::new(),
            clip_gate: Arc::new(tokio::sync::Semaphore::new(1)),
        });
        (actor, id)
    }

    #[tokio::test]
    async fn close_tree_stops_only_the_subtree_utterance() {
        let (mine, other) = (SessionId::new(), SessionId::new());
        let (actor, id) = speaking(mine);
        let close = |subtree| CloseTree {
            subtree,
            generation: 1,
            cancel: CancellationToken::new(),
        };

        let ack = actor.ask(close(vec![other])).await.expect("ask");
        assert!(ack.ok && ack.owner == "voice");
        assert_eq!(actor.ask(Status).await.expect("ask").speaking, Some(id));

        let ack = actor.ask(close(vec![other, mine])).await.expect("ask");
        assert!(ack.ok);
        assert_eq!(actor.ask(Status).await.expect("ask").speaking, None);
    }
}
