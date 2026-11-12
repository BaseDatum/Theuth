//! Read replies aloud (design doc 43): Pocket TTS runs in the harness via
//! `ptts`; audio leaves as [`VoiceEvent::SpeechAudio`] — one Opus packet per
//! 80 ms Mimi frame, base64 in JSON — over the bus. Voice notes (design doc
//! 45): [`Transcriber`] turns a recorded file into text for the channels, and
//! [`Synthesize`] renders one prose block into an MP3 [`Clip`] they can send back.
//! Dictation (design doc 54): [`Dictation`] owns a streaming Nemotron session fed
//! by the microphone or by PCM a remote client pushes ([`PushAudio`]); text lands
//! at the frontend's caret as [`VoiceEvent::DictationText`]. Talk (design doc 76,
//! steps 1–4): the [`Voice`] actor gains a FIFO behind the current utterance
//! ([`Speak::queue`]) and a device sink ([`Sink::Device`] → [`out::Player`], the
//! daemon's own speaker); the echo-cancelled mic lives in [`mic`]; the resident
//! `Utterer` can be leased from [`Dictation`] ([`Lease`]/[`Return`]).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use out::{Played, Player};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tools::{CloseAck, CloseTree, RegisterLifecycle};

mod clip;
mod dictation;
pub mod dictionary;
pub mod history;
pub mod live;
pub mod mic;
pub mod model;
pub mod out;
pub mod shape;
mod speak;
mod stt;
pub mod vad;

pub use dictation::{
    AnswerSuggestion, AudioSource, Correction as DictationCorrection, CurrentSettings, Dictation,
    DictationState, DictationStatus, DictionaryOp, DictionaryReply, HistoryStore, InputDevicePrefs,
    Lease, Prewarm, PushAudio, Return, Setting, StartDictation, StopDictation,
};
pub use dictionary::{Dictionary, Entry, EntryId, Kind, Match, Negative, Scope, Source, Variant};
pub use history::{Correction, HistoryRow, Store, SuggestionAnswer, SuggestionId};
use kameo::reply::ReplySender;
pub use shape::{
    Action, EndCommand, EndPhrases, Preset, Replaced, RuleId, ShapeConfig, ShapePresets, Shaped,
    Shaper,
};
use speak::{Out, Progress, Report, Synthesizer};
pub use stt::{
    Engine, PIECE, Piece, Stream, TranscribeWords, Transcriber, WINDOW, Word, WordTranscript,
    decode, resample, resample_block, speech_sample,
};

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

/// Render one prose block (markdown) into a [`Clip`] (45 §4.3) in the
/// configured voice. Waits for the model to load if it has not; `Ok(None)`
/// when there is nothing to say.
#[derive(Clone, Debug)]
pub struct Synthesize {
    /// Markdown; `markdown::spoken` turns it into sentences.
    pub text: String,
}

/// Render a dialogue into one [`Clip`] (144 §10): one synthesis per turn in
/// the turn's voice (`None` = the configured voice), [`DIALOGUE_GAP_MS`] of
/// silence between turns, encoded once. Turns with nothing to say are
/// skipped; `Ok(None)` when none remains. Reply like [`Synthesize`].
#[derive(Clone, Debug)]
pub struct SynthesizeDialogue {
    /// Speaker turns in order.
    pub turns: Vec<DialogueTurn>,
}

/// One speaker turn of a [`SynthesizeDialogue`].
#[derive(Clone, Debug)]
pub struct DialogueTurn {
    /// Voice name (`voice_status` lists them); `None` = the configured voice.
    pub voice: Option<String>,
    /// Markdown; `markdown::spoken` turns it into sentences.
    pub text: String,
}

/// Silence between two speaker turns of a dialogue clip.
pub const DIALOGUE_GAP_MS: u32 = clip::GAP_MS;

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

/// Where an utterance's audio goes (76 §0.3).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Sink {
    /// Opus packets over the bus as [`VoiceEvent::SpeechAudio`]; the frontend plays (43).
    #[default]
    Client,
    /// The daemon's speaker ([`out::Player`]); no `SpeechAudio` is published.
    Device,
}

/// Speak `text` (markdown) in `session`'s name. `queue: false` stops the
/// current utterance and drops the queue first (43); `queue: true` waits its
/// turn behind them (76 §0.4). Replies at once with the id; audio follows as
/// events, or goes to the speaker for [`Sink::Device`].
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Speak {
    /// Session the utterance belongs to (events carry it for subscription filters).
    pub session: SessionId,
    /// Markdown to read; `markdown::spoken` turns it into sentences.
    pub text: String,
    /// Append behind what is speaking instead of replacing it.
    #[serde(default)]
    pub queue: bool,
    /// Where the audio goes.
    #[serde(default)]
    pub sink: Sink,
}

/// Stop `id`, or — with `None` — whatever is speaking *and* everything
/// queued (76 §0.4). Each dropped utterance gets `SpeechFinished { Stopped }`.
/// Idempotent.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stop {
    /// Utterance to stop; `None` stops the current one and clears the queue.
    pub id: Option<SpeechId>,
}

/// Silence the speaker from the next callback, keeping the queue and the
/// ring (76 §0.6 stage 1). Device sink only: `Err` when none is open.
/// Replies with where playback was.
#[derive(Clone, Copy, Debug)]
pub struct Pause;

/// Continue from the paused sample. Device sink only.
#[derive(Clone, Copy, Debug)]
pub struct Resume;

/// Where the speaker is now: [`Played`]. Device sink only.
#[derive(Clone, Copy, Debug)]
pub struct Position;

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
    /// The ONNX Runtime execution provider STT/dictation sessions run on
    /// (`models::onnx::ep`: `DmlExecutionProvider` | `CPUExecutionProvider`).
    #[serde(default)]
    pub accelerator: String,
    /// The configured voice.
    pub voice: String,
    /// Every voice the checkpoint ships.
    pub voices: Vec<String>,
    /// The utterance in progress, if any.
    pub speaking: Option<SpeechId>,
    /// Utterances waiting behind it, in order (76 §0.4).
    #[serde(default)]
    pub queued: Vec<SpeechId>,
    /// `voice.input_devices` (153 §8.1): the ordered mic preference the companions read too.
    #[serde(default)]
    pub input_devices: Vec<String>,
}

/// Which client surface a dictation feeds (153 §4.5); `None` in [`DictationOpts`] = `Composer`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// The desktop Composer and the per-field mics.
    #[default]
    Composer,
    /// The terminal UI.
    Tui,
    /// The companion's own fields (Ask bar, panel, rows).
    Companion,
    /// The phone webview.
    Phone,
    /// Another application, through the companion's delivery (153 §13).
    AnyApp,
}

/// Who reads the dictated text (153 §9.1); decides whether polish may run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// The agent reads it: never polished.
    AgentPrompt,
    /// A document a human reads.
    Document,
    /// A chat/mail reply a human reads.
    MessageReply,
    /// Another app, named by bundle id / exe.
    AnyApp {
        /// Bundle id (macOS) or exe name (Windows).
        app: String,
    },
}

/// Trailing optional argument of `dictation_start` (153 §14.1); `None` = today's behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DictationOpts {
    /// Picks the shaping preset (`voice.shape.<surface>`); `None` = `Composer`.
    pub surface: Option<Surface>,
    /// Who reads the text; `None` = `AgentPrompt`.
    pub destination: Option<Destination>,
    /// §13's per-app row for shaping (terminal spacing, commands); the engine never persists it.
    pub app: Option<AppProfile>,
    /// Slash commands the surface accepts (stage 4); empty = none recognised.
    pub commands: Vec<String>,
    /// §8.7: `now − press` on the caller's clock at the moment of the call.
    pub press_ago_ms: Option<u32>,
}

/// A companion-side per-app row (153 §13), sent with [`DictationOpts`] for any-app dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AppProfile {
    /// Bundle id (macOS) or exe name (Windows).
    pub id: String,
    /// Display name.
    pub name: String,
    /// A terminal: trailing-space / autocomplete spacing rules apply.
    pub terminal: bool,
    /// Polish may run for this app.
    pub polish: bool,
    /// Observe the target for corrections after a paste (§6.6).
    pub learn: bool,
    /// Per-app polish instruction, appended to the system prompt.
    pub polish_prompt: Option<String>,
    /// Per-app rewrite instruction, appended to the system prompt.
    pub rewrite_prompt: Option<String>,
    /// Slash commands this app accepts.
    pub commands: Vec<String>,
}

/// Latency numbers of one dictation relative to the key press (153 §8.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DictationTimings {
    /// Which surface dictated.
    pub surface: Surface,
    /// Press → first PCM chunk from the mic.
    pub press_to_first_pcm_ms: Option<u32>,
    /// Press → first partial text.
    pub press_to_first_partial_ms: Option<u32>,
    /// Press → first closed utterance.
    pub press_to_first_final_ms: Option<u32>,
    /// Stop request → `DictationFinished`.
    pub stop_to_final_ms: Option<u32>,
    /// Polish round trip, when it ran (§9.5).
    pub polish_ms: Option<u32>,
    /// The input device that opened.
    pub device: Option<String>,
}

/// What happened to the mic mid-dictation (153 §8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Re-armed on `device`.
    Changed,
    /// No device could be opened; the dictation ends after this.
    Lost,
}

/// One enumerated microphone (153 §8.1), the `list_input_devices` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InputDevice {
    /// Host name of the device.
    pub name: String,
    /// The system default input.
    pub default: bool,
}

/// `voice.duck_media` (153 §8.6): what the companion does to media while dictating.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Duck {
    /// Leave media alone.
    #[default]
    Off,
    /// Output volume −50 % for the dictation.
    Duck,
    /// One media play/pause at start and end.
    Pause,
}

/// `voice.history` (153 §6.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct HistoryConfig {
    /// Write dictation rows and audio at all.
    pub enabled: bool,
    /// Rows kept; older rows are deleted.
    pub rows: u32,
    /// Audio budget in MiB; oldest files unlinked first.
    pub audio_mib: u32,
    /// Learn corrections (§6.3) from `dictation_correction`.
    pub learn: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rows: 200,
            audio_mib: 64,
            learn: true,
        }
    }
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
        /// Where the audio goes; `SpeechAudio` follows only for `client`.
        #[serde(default)]
        sink: Sink,
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
        /// Where the audio went.
        #[serde(default)]
        sink: Sink,
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
        /// `text` after shaping (153 §4); what the surface shows and sends.
        #[serde(default)]
        shaped: String,
        /// A spoken end command armed/fired on this piece (153 §7).
        #[serde(default)]
        command: Option<EndCommand>,
        /// Shaping actions ("new line", "tab") the surface applies after `shaped`.
        #[serde(default)]
        actions: Vec<Action>,
    },
    /// Mic closed. `Done` after a stop, `Stopped` after the max-length stop, `Error` for mic/model failure.
    DictationFinished {
        /// Dictation.
        id: DictationId,
        /// How it ended.
        reason: FinishReason,
    },
    /// The input device changed mid-dictation (153 §8.2): re-armed on `device`, or lost.
    DictationDevice {
        /// Dictation.
        id: DictationId,
        /// The device now open; `None` when lost.
        device: Option<String>,
        /// What happened.
        state: DeviceState,
    },
    /// A dictionary suggestion is due (153 §6.3): the surface shows the card.
    DictionarySuggestion {
        /// Suggestion; answered with `dictionary_suggestion_answer`.
        id: SuggestionId,
        /// What the model heard.
        heard: String,
        /// What the user typed instead.
        typed: String,
        /// How often the pair was seen.
        occurrences: u32,
        /// Where it was dictated.
        surface: Surface,
        /// Audio of the utterance is available (`dictation_audio_get`).
        has_audio: bool,
    },
}

impl wire::WireEvent for VoiceEvent {
    const TOPIC: &'static str = "voice.events";
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<VoiceEvent>().await
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
    /// Speaker ring: how far ahead of playback synthesis may run (76 §4.3).
    pub player_buffer_ms: u32,
    /// Output device by name; `None` → the default output.
    pub player_device: Option<String>,
    /// Shaping preset per surface (153 §4.5).
    pub shape: ShapePresets,
    /// Spoken end phrases (153 §7.2).
    pub end_commands: EndPhrases,
    /// Ordered input device names; `[]` = system default only (153 §8.1).
    pub input_devices: Vec<String>,
    /// Keep the mic open this long after a dictation ends; `0` = close at once (153 §8.4).
    pub standby_secs: u64,
    /// What the companion does to media while dictating (153 §8.6).
    pub duck_media: Duck,
    /// Polish model id; `None` = title model, then the session's model (153 §9.2).
    pub polish_model: Option<String>,
    /// Per-destination polish instruction overrides, keyed by destination kind (153 §9.1).
    pub polish_profiles: BTreeMap<String, String>,
    /// System-prompt template for rewrite-on-selection (153 §13.6); `None` = the built-in.
    pub rewrite_template: Option<String>,
    /// Dictation history store (153 §6.5).
    pub history: HistoryConfig,
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
            player_buffer_ms: 200,
            player_device: None,
            shape: ShapePresets::default(),
            end_commands: EndPhrases::default(),
            input_devices: Vec::new(),
            standby_secs: 8,
            duck_media: Duck::Off,
            polish_model: None,
            polish_profiles: BTreeMap::new(),
            rewrite_template: None,
            history: HistoryConfig::default(),
        }
    }
}

/// The `voice_settings` reply (153 F1): the user-facing `voice.*` keys as the engine
/// holds them now (every setter has a reader; clients remember nothing).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VoiceSettings {
    /// `voice.shape` (§4.5).
    pub shape: ShapePresets,
    /// `voice.end_commands` (§7.2).
    pub end_commands: EndPhrases,
    /// `voice.input_devices` (§8.1).
    pub input_devices: Vec<String>,
    /// `voice.standby_secs` (§8.4).
    pub standby_secs: u64,
    /// `voice.duck_media` (§8.6).
    pub duck_media: Duck,
    /// `voice.polish_model` (§9.2).
    pub polish_model: Option<String>,
    /// `voice.polish_profiles` (§9.1).
    pub polish_profiles: BTreeMap<String, String>,
    /// `voice.history` (§6.5).
    pub history: HistoryConfig,
}

impl From<&VoiceConfig> for VoiceSettings {
    fn from(c: &VoiceConfig) -> Self {
        Self {
            shape: c.shape,
            end_commands: c.end_commands.clone(),
            input_devices: c.input_devices.clone(),
            standby_secs: c.standby_secs,
            duck_media: c.duck_media,
            polish_model: c.polish_model.clone(),
            polish_profiles: c.polish_profiles.clone(),
            history: c.history.clone(),
        }
    }
}

/// The utterance in progress: a running task, or one waiting for the model.
struct Current {
    id: SpeechId,
    session: SessionId,
    sink: Sink,
    /// `Some` once synthesis started; cancelling it drops the ptts stream.
    cancel: Option<CancellationToken>,
    /// Markdown still to speak, while the model is loading.
    text: Option<String>,
}

/// An utterance waiting behind [`Current`] (76 §0.4).
struct Queued {
    id: SpeechId,
    session: SessionId,
    text: String,
    sink: Sink,
}

/// `tell` from the load task: what `model::load` produced.
struct Loaded(anyhow::Result<ptts::synth::Synth>);

/// The [`VOICE_ACTOR`] actor: owns the model, the one utterance at a time
/// (43 §0.6), the FIFO behind it and the speaker (76 §0.3, §0.4).
#[derive(kameo::Actor)]
pub struct Voice {
    rt: Runtime,
    cfg: VoiceConfig,
    /// Resident once loaded; never unloaded (43 §4.3).
    model: Option<Arc<dyn Synthesizer>>,
    model_state: ModelState,
    voices: Vec<String>,
    current: Option<Current>,
    queue: VecDeque<Queued>,
    /// The speaker; opened on the first `Sink::Device` utterance, closed by
    /// `Stop { id: None }` (a talk's last word) so the device is held only while one runs.
    player: Option<Arc<Player>>,
    /// [`Synthesize`]/[`SynthesizeDialogue`] jobs (`(voice, spoken)` turns) that arrived before the model loaded (45 §4.3).
    pending_clips: Vec<(Vec<(String, String)>, ClipReply)>,
    /// One clip renders at a time; two channels talking at once share the CPU in turn, not in halves.
    clip_gate: Arc<tokio::sync::Semaphore>,
}

type ClipReply = ReplySender<Result<Option<Clip>, String>>;

impl Voice {
    fn new(rt: Runtime, cfg: VoiceConfig, model: Option<Arc<dyn Synthesizer>>) -> Self {
        Self {
            rt,
            cfg,
            model_state: if model.is_some() {
                ModelState::Ready
            } else {
                ModelState::Missing
            },
            model,
            voices: Vec::new(),
            current: None,
            queue: VecDeque::new(),
            player: None,
            pending_clips: Vec::new(),
            clip_gate: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    async fn finished(&self, id: SpeechId, session: SessionId, sink: Sink, reason: FinishReason) {
        self.rt
            .publish(VoiceEvent::SpeechFinished {
                id,
                session,
                reason,
                sink,
            })
            .await;
    }

    /// Publish `Stopped` for the current utterance (running or pending) and
    /// forget it; a device-sink utterance is also cut from the speaker ring.
    async fn stop_current(&mut self) {
        let Some(cur) = self.current.take() else {
            return;
        };
        if let Some(cancel) = cur.cancel {
            cancel.cancel();
        }
        if cur.sink == Sink::Device
            && let Some(p) = &self.player
        {
            p.flush();
        }
        self.finished(cur.id, cur.session, cur.sink, FinishReason::Stopped)
            .await;
    }

    /// Publish `Stopped` for every queued utterance and forget them.
    async fn clear_queue(&mut self) {
        for q in std::mem::take(&mut self.queue) {
            self.finished(q.id, q.session, q.sink, FinishReason::Stopped)
                .await;
        }
    }

    /// `current` is empty: start the next queued utterance, if any. Without
    /// the model it parks as `current` and the load begins.
    async fn advance(&mut self, me: ActorRef<Voice>) {
        while self.current.is_none() {
            let Some(q) = self.queue.pop_front() else {
                return;
            };
            match self.model.clone() {
                Some(synth) => {
                    self.start(synth, q.id, q.session, q.text, q.sink, me.clone())
                        .await;
                }
                None => {
                    self.current = Some(Current {
                        id: q.id,
                        session: q.session,
                        sink: q.sink,
                        cancel: None,
                        text: Some(q.text),
                    });
                    if self.model_state != ModelState::Loading {
                        self.load(me.clone()).await;
                    }
                }
            }
        }
    }

    /// Render `turns` (`(voice, spoken)`) into a clip on the loaded model, off the actor, one at a time.
    fn clip(&self, synth: Arc<dyn Synthesizer>, turns: Vec<(String, String)>, tx: ClipReply) {
        let gate = self.clip_gate.clone();
        tokio::spawn(async move {
            let Ok(_permit) = gate.acquire_owned().await else {
                return;
            };
            let r = tokio::task::spawn_blocking(move || clip::render(&*synth, &turns))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("clip task panicked: {e}")));
            tx.send(r.map(Some).map_err(|e| format!("{e:#}")));
        });
    }

    /// Queue a clip job: render now when the model is loaded, else park it and start the load.
    async fn clip_or_pend(
        &mut self,
        turns: Vec<(String, String)>,
        tx: ClipReply,
        me: ActorRef<Voice>,
    ) {
        match self.model.clone() {
            Some(synth) => self.clip(synth, turns, tx),
            None => {
                self.pending_clips.push((turns, tx));
                if self.model_state != ModelState::Loading {
                    self.load(me).await;
                }
            }
        }
    }

    /// Start synthesizing `text` for the utterance `id` on the loaded model.
    /// A speaker that will not open ends the utterance with `Error` at once.
    async fn start(
        &mut self,
        synth: Arc<dyn Synthesizer>,
        id: SpeechId,
        session: SessionId,
        text: String,
        sink: Sink,
        me: ActorRef<Voice>,
    ) {
        let out = match sink {
            Sink::Client => Out::Client,
            Sink::Device => match self.player() {
                Ok(p) => Out::Device(p),
                Err(message) => {
                    tracing::warn!("voice: {message}");
                    self.finished(id, session, sink, FinishReason::Error { message })
                        .await;
                    return;
                }
            },
        };
        let cancel = CancellationToken::new();
        self.current = Some(Current {
            id,
            session,
            sink,
            cancel: Some(cancel.clone()),
            text: None,
        });
        let voice = self.cfg.voice.clone();
        tokio::task::spawn_blocking(move || speak::run(synth, voice, id, text, out, cancel, me));
    }

    /// The open speaker, opening it on first use.
    fn player(&mut self) -> Result<Arc<Player>, String> {
        if let Some(p) = &self.player {
            return Ok(p.clone());
        }
        let p = Player::open(self.cfg.player_device.as_deref(), self.cfg.player_buffer_ms)
            .map_err(|e| format!("{e:#}"))?;
        tracing::info!(device = %p.device, rate = p.rate, channels = p.channels, "voice: speaker open");
        let p = Arc::new(p);
        self.player = Some(p.clone());
        Ok(p)
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
        let turns = vec![(self.cfg.voice.clone(), spoken)];
        self.clip_or_pend(turns, tx, ctx.actor_ref().clone()).await;
        reply
    }
}

impl Message<SynthesizeDialogue> for Voice {
    type Reply = kameo::reply::DelegatedReply<Result<Option<Clip>, String>>;
    async fn handle(
        &mut self,
        m: SynthesizeDialogue,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let turns: Vec<(String, String)> = m
            .turns
            .into_iter()
            .filter_map(|t| {
                let spoken = markdown::spoken(&t.text);
                (!spoken.is_empty())
                    .then(|| (t.voice.unwrap_or_else(|| self.cfg.voice.clone()), spoken))
            })
            .collect();
        if turns.is_empty() {
            tx.send(Ok(None));
            return reply;
        }
        self.clip_or_pend(turns, tx, ctx.actor_ref().clone()).await;
        reply
    }
}

impl Message<Speak> for Voice {
    type Reply = Result<SpeechId, String>;
    async fn handle(&mut self, m: Speak, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = SpeechId::new();
        if !m.queue {
            self.clear_queue().await;
            self.stop_current().await;
        }
        self.queue.push_back(Queued {
            id,
            session: m.session,
            text: m.text,
            sink: m.sink,
        });
        self.advance(ctx.actor_ref().clone()).await;
        Ok(id)
    }
}

impl Message<Stop> for Voice {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: Stop, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match m.id {
            None => {
                self.clear_queue().await;
                self.stop_current().await;
                // A talk's last word: release the speaker.
                self.player = None;
            }
            Some(id) if self.current.as_ref().is_some_and(|c| c.id == id) => {
                self.stop_current().await;
                self.advance(ctx.actor_ref().clone()).await;
            }
            Some(id) => {
                if let Some(at) = self.queue.iter().position(|q| q.id == id)
                    && let Some(q) = self.queue.remove(at)
                {
                    self.finished(q.id, q.session, q.sink, FinishReason::Stopped)
                        .await;
                }
            }
        }
        Ok(())
    }
}

impl Message<Pause> for Voice {
    type Reply = Result<Played, String>;
    async fn handle(&mut self, _: Pause, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.player
            .as_ref()
            .map(|p| p.pause())
            .ok_or_else(|| "no device sink open".to_owned())
    }
}

impl Message<Resume> for Voice {
    type Reply = Result<(), String>;
    async fn handle(&mut self, _: Resume, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.player
            .as_ref()
            .map(|p| p.resume())
            .ok_or_else(|| "no device sink open".to_owned())
    }
}

impl Message<Position> for Voice {
    type Reply = Result<Played, String>;
    async fn handle(&mut self, _: Position, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.player
            .as_ref()
            .map(|p| p.position())
            .ok_or_else(|| "no device sink open".to_owned())
    }
}

impl Message<CloseTree> for Voice {
    type Reply = CloseAck;

    /// A closing tree (51 §4.5): stop the utterances that belong to a
    /// closing session (same path as [`Stop`]). Voice keeps nothing else per
    /// session and writes no session files; clips are returned as bytes.
    async fn handle(&mut self, m: CloseTree, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (mine, others): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.queue)
            .into_iter()
            .partition(|q| m.subtree.contains(&q.session));
        self.queue = others;
        for q in mine {
            self.finished(q.id, q.session, q.sink, FinishReason::Stopped)
                .await;
        }
        if self
            .current
            .as_ref()
            .is_some_and(|c| m.subtree.contains(&c.session))
        {
            self.stop_current().await;
            self.advance(ctx.actor_ref().clone()).await;
        }
        CloseAck::ok("voice")
    }
}

impl Message<Status> for Voice {
    type Reply = VoiceStatus;
    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        VoiceStatus {
            model: self.model_state.clone(),
            accelerator: models::onnx::ep().to_owned(),
            voice: self.cfg.voice.clone(),
            voices: self.voices.clone(),
            speaking: self.current.as_ref().map(|c| c.id),
            queued: self.queue.iter().map(|q| q.id).collect(),
            input_devices: self.cfg.input_devices.clone(),
        }
    }
}

impl Message<Loaded> for Voice {
    type Reply = ();
    async fn handle(&mut self, m: Loaded, ctx: &mut Context<Self, Self::Reply>) {
        match m.0 {
            Ok(synth) => {
                self.voices = synth.voices();
                let synth: Arc<dyn Synthesizer> = Arc::new(synth);
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
                        cur.sink,
                        ctx.actor_ref().clone(),
                    )
                    .await;
                }
                // A speaker that failed to open left `current` empty: move on.
                self.advance(ctx.actor_ref().clone()).await;
                for (turns, tx) in std::mem::take(&mut self.pending_clips) {
                    self.clip(synth.clone(), turns, tx);
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
                let failed = FinishReason::Error { message };
                if let Some(cur) = self.current.take() {
                    self.finished(cur.id, cur.session, cur.sink, failed.clone())
                        .await;
                }
                for q in std::mem::take(&mut self.queue) {
                    self.finished(q.id, q.session, q.sink, failed.clone()).await;
                }
            }
        }
    }
}

impl Message<Report> for Voice {
    type Reply = ();
    async fn handle(&mut self, m: Report, ctx: &mut Context<Self, Self::Reply>) {
        // Late frames after `Stop` or a replacing `Speak`.
        let Some(cur) = self.current.as_ref().filter(|c| c.id == m.id) else {
            return;
        };
        let (id, session, sink) = (cur.id, cur.session, cur.sink);
        let event = match m.progress {
            Progress::Started { chars } => VoiceEvent::SpeechStarted {
                id,
                session,
                sample_rate: speak::SAMPLE_RATE,
                chars,
                sink,
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
                    sink,
                }
            }
        };
        self.rt.publish(event).await;
        if self.current.is_none() {
            self.advance(ctx.actor_ref().clone()).await;
        }
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
        declare_wire(rt).await?;
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
        let actor = Voice::spawn(Voice::new(rt.clone(), cfg, None));
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
    use std::time::Instant;

    use super::*;

    #[test]
    fn note_marker() {
        assert_eq!(note_text("hi", 12_400), "[voice note, 13 s] hi");
        assert_eq!(note_text("", 0), "[voice note, 1 s] ");
        assert!(is_note("  [voice note, 3 s] x"));
        assert!(!is_note("[attachment: a.m4a]"));
    }

    /// 153 F1: `voice_settings` mirrors the config section field for field and survives serde.
    #[test]
    fn voice_settings_round_trip_defaults() {
        let cfg = VoiceConfig::default();
        let settings = VoiceSettings::from(&cfg);
        assert_eq!(settings.shape, cfg.shape);
        assert_eq!(settings.end_commands, cfg.end_commands);
        assert_eq!(settings.standby_secs, 8);
        assert_eq!(settings.duck_media, Duck::Off);
        assert_eq!(settings.history, HistoryConfig::default());
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["shape"]["tui"], "full");
        assert_eq!(json["end_commands"]["send"][0], "send it");
        let back: VoiceSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back, settings);
    }

    /// 144 §10: a dialogue clip is one MP3 of every turn (5 frames each on the
    /// fake model) joined with 350 ms gaps; an empty turn is skipped and an
    /// all-empty dialogue is nothing to say.
    #[tokio::test]
    async fn dialogue_clip_joins_turns_with_gaps() {
        let (actor, _rx) = fake_voice().await;
        let turn = |voice: &str, text: &str| DialogueTurn {
            voice: Some(voice.into()),
            text: text.into(),
        };
        let clip = actor
            .ask(SynthesizeDialogue {
                turns: vec![
                    turn("alba", "**Alba:** Hello."),
                    turn("peter_yearsley", ""),
                    turn("peter_yearsley", "Hi there."),
                ],
            })
            .await
            .unwrap()
            .expect("a clip");
        assert_eq!(clip.mime, "audio/mpeg");
        assert_eq!(clip.bytes[0], 0xFF);
        // Two spoken turns × 5 × 1920 samples + one 350 ms gap, at 24 kHz.
        assert_eq!(clip.duration_ms, (2 * 5 * 1920 * 1000 / 24_000) + 350);
        let none = actor
            .ask(SynthesizeDialogue {
                turns: vec![turn("alba", "")],
            })
            .await
            .unwrap();
        assert!(none.is_none());
    }

    /// A model that says nothing audible: `frames` Mimi frames of silence, one every `pace`.
    struct FakeSynth {
        frames: usize,
        pace: Duration,
    }

    impl Synthesizer for FakeSynth {
        fn stream(&self, _: &str, _: &str) -> anyhow::Result<speak::Pcm> {
            let pace = self.pace;
            Ok(Box::new((0..self.frames).map(move |_| {
                std::thread::sleep(pace);
                Ok(vec![0.0; 1920])
            })))
        }
    }

    /// Every `VoiceEvent` published on the runtime, in order.
    #[derive(kameo::Actor)]
    struct Events(tokio::sync::mpsc::UnboundedSender<VoiceEvent>);

    impl Message<VoiceEvent> for Events {
        type Reply = ();
        async fn handle(&mut self, e: VoiceEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    /// An actor on the fake model (each utterance ≈ 100 ms) and the events it publishes.
    async fn fake_voice() -> (
        ActorRef<Voice>,
        tokio::sync::mpsc::UnboundedReceiver<VoiceEvent>,
    ) {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = Events::spawn_with_mailbox(Events(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Events, VoiceEvent>(&sink).await;
        let synth: Arc<dyn Synthesizer> = Arc::new(FakeSynth {
            frames: 5,
            pace: Duration::from_millis(20),
        });
        let actor = Voice::spawn(Voice::new(rt, VoiceConfig::default(), Some(synth)));
        (actor, rx)
    }

    /// Next `SpeechStarted`/`SpeechFinished` (audio packets skipped), within a second.
    async fn next_edge(rx: &mut tokio::sync::mpsc::UnboundedReceiver<VoiceEvent>) -> VoiceEvent {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let e = tokio::time::timeout_at(deadline.into(), rx.recv())
                .await
                .expect("an event within a second")
                .expect("bus open");
            if !matches!(e, VoiceEvent::SpeechAudio { .. }) {
                return e;
            }
        }
    }

    fn speak(session: SessionId, queue: bool) -> Speak {
        Speak {
            session,
            text: "Hello there.".into(),
            queue,
            sink: Sink::Client,
        }
    }

    /// 76 §10 step 2: three queued speaks start in order, each finishing `Done`.
    #[tokio::test]
    async fn queued_speaks_play_in_order() {
        let (actor, mut rx) = fake_voice().await;
        let s = SessionId::new();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(actor.ask(speak(s, true)).await.expect("speak"));
        }
        let status = actor.ask(Status).await.expect("ask");
        assert_eq!(status.speaking, Some(ids[0]));
        assert_eq!(status.queued, ids[1..].to_vec());
        for &id in &ids {
            let started = next_edge(&mut rx).await;
            assert!(
                matches!(started, VoiceEvent::SpeechStarted { id: got, sink: Sink::Client, .. } if got == id),
                "{started:?}"
            );
            let finished = next_edge(&mut rx).await;
            assert!(
                matches!(finished, VoiceEvent::SpeechFinished { id: got, reason: FinishReason::Done, .. } if got == id),
                "{finished:?}"
            );
        }
        assert_eq!(actor.ask(Status).await.expect("ask").speaking, None);
    }

    /// `Stop { id: None }` mid-second: `Stopped` for the second and third, nothing more for the first.
    #[tokio::test]
    async fn stop_all_clears_the_queue() {
        let (actor, mut rx) = fake_voice().await;
        let s = SessionId::new();
        let ids: Vec<SpeechId> = {
            let mut v = Vec::new();
            for _ in 0..3 {
                v.push(actor.ask(speak(s, true)).await.expect("speak"));
            }
            v
        };
        // Wait for the second to start.
        loop {
            if matches!(next_edge(&mut rx).await, VoiceEvent::SpeechStarted { id, .. } if id == ids[1])
            {
                break;
            }
        }
        actor.ask(Stop { id: None }).await.expect("stop");
        let mut stopped = Vec::new();
        for _ in 0..2 {
            match next_edge(&mut rx).await {
                VoiceEvent::SpeechFinished {
                    id,
                    reason: FinishReason::Stopped,
                    ..
                } => stopped.push(id),
                other => panic!("expected Stopped, got {other:?}"),
            }
        }
        stopped.sort_by_key(|id| id.0);
        let mut expect = ids[1..].to_vec();
        expect.sort_by_key(|id| id.0);
        assert_eq!(stopped, expect);
        let status = actor.ask(Status).await.expect("ask");
        assert_eq!((status.speaking, status.queued.len()), (None, 0));
        // Nothing else follows: no late frame of the cancelled utterance, nothing for the first.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(rx.try_recv().is_err(), "unexpected event after Stop");
    }

    /// A replacing `Speak { queue: false }` drops the queue (today's semantics) and speaks itself.
    #[tokio::test]
    async fn replacing_speak_clears_the_queue() {
        let (actor, mut rx) = fake_voice().await;
        let s = SessionId::new();
        let a = actor.ask(speak(s, true)).await.expect("speak");
        let b = actor.ask(speak(s, true)).await.expect("speak");
        let c = actor.ask(speak(s, true)).await.expect("speak");
        let d = actor.ask(speak(s, false)).await.expect("speak");
        let mut seen = Vec::new();
        loop {
            match next_edge(&mut rx).await {
                VoiceEvent::SpeechFinished { id, reason, .. } => seen.push((id, reason)),
                VoiceEvent::SpeechStarted { id, .. } if id == d => break,
                VoiceEvent::SpeechStarted { id, .. } => {
                    assert_eq!(id, a, "only the first ever started")
                }
                other => panic!("{other:?}"),
            }
        }
        let stopped: Vec<SpeechId> = seen
            .iter()
            .filter(|(_, r)| *r == FinishReason::Stopped)
            .map(|(id, _)| *id)
            .collect();
        assert!(stopped.contains(&b) && stopped.contains(&c), "{seen:?}");
        assert!(
            stopped.contains(&a)
                || seen
                    .iter()
                    .any(|(id, r)| *id == a && *r == FinishReason::Done)
        );
        assert_eq!(actor.ask(Status).await.expect("ask").speaking, Some(d));
    }

    /// `Stop { id }` of a queued utterance removes just it; of the current one, the next starts.
    #[tokio::test]
    async fn stop_one() {
        let (actor, mut rx) = fake_voice().await;
        let s = SessionId::new();
        let a = actor.ask(speak(s, true)).await.expect("speak");
        let b = actor.ask(speak(s, true)).await.expect("speak");
        let c = actor.ask(speak(s, true)).await.expect("speak");
        assert!(
            matches!(next_edge(&mut rx).await, VoiceEvent::SpeechStarted { id, .. } if id == a)
        );
        actor.ask(Stop { id: Some(b) }).await.expect("stop");
        assert_eq!(actor.ask(Status).await.expect("ask").queued, vec![c]);
        actor.ask(Stop { id: Some(a) }).await.expect("stop");
        assert_eq!(actor.ask(Status).await.expect("ask").speaking, Some(c));
        let mut order = Vec::new();
        for _ in 0..3 {
            order.push(match next_edge(&mut rx).await {
                VoiceEvent::SpeechStarted { id, .. } => (id, None),
                VoiceEvent::SpeechFinished { id, reason, .. } => (id, Some(reason)),
                other => panic!("{other:?}"),
            });
        }
        assert_eq!(order[0], (b, Some(FinishReason::Stopped)));
        assert_eq!(order[1], (a, Some(FinishReason::Stopped)));
        assert_eq!(order[2], (c, None));
    }

    /// Pause/Resume/Position without a speaker open answer `Err`, not a panic.
    #[tokio::test]
    async fn device_controls_need_a_player() {
        let (actor, _rx) = fake_voice().await;
        assert!(actor.ask(Pause).await.is_err());
        assert!(actor.ask(Resume).await.is_err());
        assert!(actor.ask(Position).await.is_err());
    }

    /// An actor with `current` set to an utterance of `session` waiting for
    /// the model (no synthesis task to cancel, no model to load).
    fn speaking(session: SessionId) -> (ActorRef<Voice>, SpeechId) {
        let id = SpeechId::new();
        let mut voice = Voice::new(
            Runtime::new(figment::Figment::new(), std::env::temp_dir()),
            VoiceConfig::default(),
            None,
        );
        voice.current = Some(Current {
            id,
            session,
            sink: Sink::Client,
            cancel: None,
            text: Some("hello".to_owned()),
        });
        (Voice::spawn(voice), id)
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
