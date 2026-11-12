//! Meeting recorder (design doc 53; phase 1 is `docs/design/meetings/01-capture.md`):
//! the meeting app's audio through a Core Audio process tap and the microphone
//! through Apple's VoiceProcessingIO unit (hardware-clocked AEC), encoded live
//! as three Opus tracks under `<data_dir>/meetings/<id>/`, with a manifest, an
//! index and the meeting's own agent session. Capture backends per OS (58 §6.1):
//! Core Audio taps + VoiceProcessingIO on macOS, WASAPI loopback on Windows,
//! PipeWire on Linux; the mic off macOS has no echo cancellation
//! (`Degradation::MicNoAec`, "headphones recommended").
//!
//! Public surface consumed by other plugins:
//! - commands on the [`MEETING_ACTOR`] actor ([`Meeting`]): [`ListProcesses`],
//!   [`StartMeeting`], [`PauseMeeting`], [`ResumeMeeting`], [`StopMeeting`],
//!   [`MeetingStatus`], [`ListMeetings`], [`GetMeeting`], [`DeleteMeeting`],
//!   [`CheckPermissions`]
//! - events: [`MeetingEvent`] (one stream; every per-meeting variant carries `meeting`;
//!   158's `Detected` / `DetectionCleared` / `Upcoming` precede a meeting and carry none)
//!
//! The types below are the shared contract frozen in `meetings/README.md` §3;
//! phases 2–4 add variants and fields, never rename or retype.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ::tools::{ApprovalRequested, ApprovalResolved};
use agent::AgentEvent;
use decision::training::{Amendment, ExampleId, Reaction};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{InputId, Runtime, SessionId, TurnId};
use serde::{Deserialize, Serialize};
use voice::live::Utterer;

use crate::post::diarize;

pub mod batch;
pub mod cache;
pub mod calendar;
pub mod capture;
pub mod detect;
pub mod diar;
pub mod endless;
mod endless_agent;
pub mod head;
pub mod note;
pub mod ogg;
pub mod oracle;
pub mod post;
pub mod search;
pub mod session;
pub mod speakers;
pub mod state;
pub mod store;
pub mod stt;
pub mod summary;
pub mod templates;
pub mod tools;
pub mod topics;
pub mod transcript;

pub use cache::{Cache, CachedBrief, CachedBriefPut};
pub use calendar::{
    Attendee, CalEvent, CalSource, CalendarConfig, CalendarEvent, EventId, EventMatch, EventRef,
};
pub use detect::{Cleared, DetectConfig, Detection};
pub use oracle::{OracleDecision, OracleDisposition, OracleOutcome};
pub use post::suggest::{Suggestion, SuggestionSource};
pub use speakers::{Profile, Profiles};
use state::Prompted;
pub use state::{Recording, RecordingClock};
pub use store::{Gap, Manifest, MeetingSummary, SearchHit, Stats, Stem, Store, TrackFile};
use stt::Piece;
pub use templates::{Section, Template};
use topics::{Gone, Record};
pub use transcript::{EditOp, Proposed, Segment, SpeakerLabel, Transcript, Word};

/// Registry name of the [`Meeting`] actor.
pub const MEETING_ACTOR: &str = "meeting";
/// Directory under `data_dir`: `<data_dir>/meetings/`.
pub const DIR: &str = "meetings";
/// Live transcript, one [`Utterance`] per line, inside a meeting's directory (02 §7).
pub const LIVE_LOG: &str = "live.jsonl";
/// Topic/brief records, one [`topics::Record`] per line (02 §7).
pub const BRIEFS_LOG: &str = "briefs.jsonl";
/// Most `live.jsonl` lines `LiveText` returns.
pub(crate) const LIVE_TEXT_CAP: usize = 2_000;
/// Free space under which [`Degradation::DiskLow`] is raised (01 §4.4).
const DISK_LOW_BYTES: u64 = 500 * 1024 * 1024;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            /// Time-ordered id.
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}

id_type!(
    /// One meeting: its directory name, manifest, tracks and session.
    MeetingId
);
id_type!(
    /// A topic button (phase 2).
    TopicId
);
id_type!(
    /// A brief (phase 2).
    BriefId
);
id_type!(
    /// An enrolled speaker profile (phase 4): its file name under `speakers/`.
    ProfileId
);

/// Which side of the call a signal came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Track {
    /// The user's microphone.
    Mic,
    /// The meeting app's output.
    System,
}

/// Where a meeting is in its life (01 §3; phase 4 fills `Post`..`Summarizing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum MeetingState {
    /// No meeting.
    Idle,
    /// Directory, session and devices being set up.
    Starting,
    /// Tracks growing.
    Recording,
    /// Devices open, encoder and clock stopped; a gap is being recorded.
    Paused,
    /// Threads joining, last pages being written.
    Stopping,
    /// Post pipeline running (phase 4).
    Post(PostStage),
    /// Waiting for the user's review (phase 4).
    Review,
    /// Summary turn in flight (phase 4).
    Summarizing,
    /// Finished.
    Done,
    /// `Starting` did not complete; `Manifest::failure` says why.
    Failed,
}

/// Stage of the post pipeline (phase 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PostStage {
    /// Re-transcribing the tracks with word times.
    Transcribing,
    /// Speaker diarization on the system track.
    Diarizing,
    /// Aligning words to speaker segments.
    Aligning,
}

/// Something the recording continues without (53 §0.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Degradation {
    /// No echo cancellation on the mic: the VoiceProcessingIO unit could not be opened (macOS), or
    /// the OS has none (Windows/Linux — 58 §6.1's normal case). Headphones recommended (53 §0.4).
    MicNoAec,
    /// Live transcription of one track stopped (phase 2).
    SttDown(Track),
    /// The meeting session's model is failing (phase 2).
    ModelDown,
    /// Under 500 MB free on the data volume.
    DiskLow,
    /// Ten minutes from `max_minutes`.
    NearMaxLength,
    /// A TCC prompt was denied.
    PermissionDenied(Permission),
    /// Documents run but refused the meeting's note (59 §7.4; the documents helper missing): no note, no summary note.
    NoNote,
    /// Live speaker labels stopped (07 §6): the diariser's session failed or its task
    /// panicked; system lines are `them:` from here, the post pass still diarizes.
    DiarizationDown,
    /// 173: the app's tap stayed silent through three reopens while sound played (a probe
    /// heard it, or the app ran output); the system track recorded every app on the default
    /// output (minus theuth) from `at_ms`, until a return probe found the app's tap live again
    /// at `until_ms` (`None`: to the end).
    SystemAudioFallback {
        /// Why the app's tap was given up.
        reason: CaptureFault,
        /// Recording-clock start.
        at_ms: u64,
        /// Recording-clock end, once the app's tap came back.
        until_ms: Option<u64>,
    },
}

/// 173: why a capture track went quiet (the capture watchdog's episode reason).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum CaptureFault {
    /// No chunk for 3 s.
    Stalled,
    /// Exact digital zeros for 15 s while sound played: a concurrent global-tap probe heard it
    /// (per-app tap), or the source ran output (global tap, or a probe that could not open).
    Silent,
    /// The OS refused the capture (System Audio Recording / Microphone permission).
    PermissionDenied,
    /// The stream ended with an error (device lost, terminal error, reopen failed).
    Failed,
}

/// 173: one capture track as the user should hear about it. A track is `Ok` until the first
/// automatic reopen failed to bring audio back (or at once for a permission / terminal error),
/// so genuine silence never raises it. `since`: RFC 3339, when the track went quiet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state")]
pub enum CaptureHealth {
    /// Hearing it (or genuine silence).
    #[default]
    Ok,
    /// Reopening; the first reopen did not help.
    Recovering {
        /// Why.
        reason: CaptureFault,
        /// Quiet since.
        since: String,
    },
    /// Three reopens did not help, or the OS refused: still retrying, the user should check.
    Lost {
        /// Why.
        reason: CaptureFault,
        /// Quiet since.
        since: String,
    },
    /// Recording again, but through [`Degradation::SystemAudioFallback`].
    Degraded {
        /// Why the app's tap was given up.
        reason: CaptureFault,
    },
}

/// 173: both capture tracks' [`CaptureHealth`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CaptureStatus {
    /// The microphone.
    pub mic: CaptureHealth,
    /// The meeting app's audio.
    pub system: CaptureHealth,
}

/// 58 §3.3: what this host can do for meetings, stated to the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "reason")]
pub enum Support {
    /// Per-app tap and echo-cancelled mic (macOS).
    Full,
    /// Records, with the stated limitation (Windows/Linux: no echo cancellation).
    Partial(String),
    /// Cannot record here; why (no PipeWire, no render device).
    None(String),
}

/// The two TCC services a meeting needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Permission {
    /// Microphone.
    Microphone,
    /// System Audio Recording Only (Core Audio taps).
    AudioCapture,
}

/// One process Core Audio knows about (01 §4.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProcessInfo {
    /// Its pid; what [`StartMeeting::process`] takes.
    pub pid: u32,
    /// `proc_name`, or the bundle id's last component.
    pub name: String,
    /// `kAudioProcessPropertyBundleID`, when the process has one.
    pub bundle_id: Option<String>,
    /// Currently playing audio (`kAudioProcessPropertyIsRunningOutput`).
    pub active: bool,
}

/// [`StartMeeting::process`] `None`: every app's audio on the default output device (a global
/// tap minus our own process). Pid 0 is the sentinel `capture::system::spawn` switches on.
pub const SYSTEM_AUDIO: u32 = 0;

/// The [`ProcessInfo`] a system-audio meeting's manifest records.
pub fn system_audio() -> ProcessInfo {
    ProcessInfo {
        pid: SYSTEM_AUDIO,
        name: "System audio".into(),
        bundle_id: None,
        active: true,
    }
}

fn yes() -> bool {
    true
}

/// Bus events; the RPC bridge forwards them to subscribers. Times are
/// milliseconds on the recording clock (pauses excluded).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum MeetingEvent {
    /// Recording began.
    Started {
        /// The meeting.
        meeting: MeetingId,
        /// Its agent session (project `meetings`).
        session: SessionId,
        /// The tapped process.
        process: ProcessInfo,
        /// RFC 3339.
        at: String,
    },
    /// A state change, with everything currently degraded.
    State {
        /// The meeting.
        meeting: MeetingId,
        /// New state.
        state: MeetingState,
        /// Union so far.
        degraded: Vec<Degradation>,
    },
    /// Meters, 10 Hz, not persisted.
    Level {
        /// The meeting.
        meeting: MeetingId,
        /// RMS of the last 100 ms of the mic track (linear).
        mic_rms: f32,
        /// RMS of the last 100 ms of the system track (linear).
        system_rms: f32,
    },
    /// Live transcript piece (phase 2).
    Utterance {
        /// The meeting.
        meeting: MeetingId,
        /// Which side spoke.
        track: Track,
        /// Start.
        t0_ms: u64,
        /// End.
        t1_ms: u64,
        /// What was said.
        text: String,
        /// Spoken while push-to-talk was held.
        request: bool,
        /// `false` = rolling partial of an open utterance (02 §2).
        #[serde(default = "yes")]
        r#final: bool,
        /// 07 §4: the live speaker label (`s1`..`s8`) of a system utterance once the
        /// diariser has scored it — a second `final: true` event for the same
        /// `track`/`t0_ms` carries it; `None` before, for the mic, and when off.
        #[serde(default)]
        speaker: Option<String>,
    },
    /// A topic button appeared (phase 2).
    TopicAdded {
        /// The meeting.
        meeting: MeetingId,
        /// The topic.
        topic: Topic,
    },
    /// A topic button went away (phase 2).
    TopicRemoved {
        /// The meeting.
        meeting: MeetingId,
        /// Which.
        topic: TopicId,
        /// Why.
        why: TopicGone,
    },
    /// A brief to show (phase 2).
    Brief {
        /// The meeting.
        meeting: MeetingId,
        /// The brief.
        brief: Brief,
    },
    /// Push-to-talk held or released (phase 2).
    Listening {
        /// The meeting.
        meeting: MeetingId,
        /// Held.
        held: bool,
    },
    /// The meeting session is waiting on a tool approval (phase 2).
    ApprovalWaiting {
        /// The meeting.
        meeting: MeetingId,
        /// Waiting.
        waiting: bool,
    },
    /// Post pipeline progress (phase 4).
    PostProgress {
        /// The meeting.
        meeting: MeetingId,
        /// Stage.
        stage: PostStage,
        /// 0–100.
        pct: u8,
    },
    /// The transcript is ready for review (phase 4).
    ReviewReady {
        /// The meeting.
        meeting: MeetingId,
    },
    /// Summary written, title set (phase 4).
    Summarized {
        /// The meeting.
        meeting: MeetingId,
        /// The title.
        title: String,
    },
    /// Directory, index row and session are gone.
    Deleted {
        /// The meeting.
        meeting: MeetingId,
    },
    /// 57 §3.4: the decision oracle is deciding a batch (the island's `deciding` flash).
    OracleDeciding {
        /// The meeting.
        meeting: MeetingId,
        /// The batch's `hh:mm:ss` stamp.
        batch_id: String,
    },
    /// 57 §3.4: the oracle's verdict on a batch; re-sent with `outcome` after the model turn.
    OracleDecision {
        /// The meeting.
        meeting: MeetingId,
        /// The verdict.
        decision: OracleDecision,
    },
    /// 158 §4.5 (W1): an eligible app has held the mic ≥ 15 s — the island's prompt. Not
    /// per-meeting: there is none yet. `event`/`project`/`brief` are filled by W2/W3.
    Detected(Detection),
    /// 158 §4.5 (W1): the prompt went away, and why.
    DetectionCleared {
        /// Which app's prompt.
        app_id: String,
        /// Why.
        reason: Cleared,
    },
    /// 158 §4.4 rule 7 (W1): the detected app has held no input stream for `released_for_s`;
    /// the island asks "Stop recording?". Once per release episode (W1 orchestrator ruling: there
    /// is no "keep recording" RPC); a new episode needs the app to hold the mic again first.
    AppReleased {
        /// The recording meeting.
        meeting: MeetingId,
        /// The app that let go.
        app: ProcessInfo,
        /// Seconds since it released.
        released_for_s: u32,
    },
    /// 158 §9.4 (W3): a matched-or-remembered event starts within 5 minutes — island state (c).
    Upcoming {
        /// The event.
        event: EventRef,
        /// Its project, when resolved.
        project: Option<String>,
        /// The pre-meeting brief bullets, when generated.
        brief: Option<Vec<String>>,
    },
    /// 173: a capture track's health changed (published on every change, both tracks).
    Capture {
        /// The meeting.
        meeting: MeetingId,
        /// Both tracks.
        capture: CaptureStatus,
    },
}

/// Why a topic button disappeared (phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum TopicGone {
    /// The user tapped it and saw the brief.
    Viewed,
    /// The user dismissed it.
    Dismissed,
    /// `topic_ttl_secs` passed.
    Expired,
}

/// A topic button (phase 2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Topic {
    /// Id.
    pub id: TopicId,
    /// ≤ 4 words, as spoken.
    pub label: String,
    /// When it came up.
    pub at_ms: u64,
    /// Precomputed brief, if any.
    pub brief: Option<BriefId>,
    /// Phase 5: the cache row behind the precomputed brief (the island marks these).
    #[serde(default)]
    pub cached: Option<BriefId>,
    /// 57 §7: what put it on the island — `Auto` (the model), `Cache` (the alias scan),
    /// `Oracle` (the decision oracle, marked with the Zap icon). Absent on rows written before step 4.
    #[serde(default)]
    pub source: Option<BriefSource>,
}

/// A brief: 2–3 bullets of facts (phase 2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Brief {
    /// Id.
    pub id: BriefId,
    /// The topic it answers, if any.
    pub topic: Option<TopicId>,
    /// Title.
    pub title: String,
    /// The bullets.
    pub bullets: Vec<String>,
    /// What produced it.
    pub source: BriefSource,
    /// When.
    pub at_ms: u64,
    /// The brief this one goes deeper on.
    pub parent: Option<BriefId>,
    /// Phase 5: the cache row this instance came from (or was written to).
    #[serde(default)]
    pub cached: Option<BriefId>,
}

/// What produced a brief (phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum BriefSource {
    /// The model on its own.
    Auto,
    /// A tapped topic button.
    Tap,
    /// A push-to-talk request.
    Request,
    /// "Deeper" on a brief.
    Deeper,
    /// Typed in the control center.
    Typed,
    /// Phase 5: the actor's alias scan found it in the cache, no model involved.
    Cache,
    /// 57 §3.2: the decision oracle picked it from the cache, no model involved.
    Oracle,
}

/// The brief cache changed (phase 5, 05 §0.8). The one meeting stream without
/// a `meeting`: the cache has none. `removed` = the row is gone. On the wire the
/// variant is `BriefChanged`: `ScheduleEvent` already owns `type: "Changed"` and
/// the desktop dispatches every event by that field alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum BriefEvent {
    /// A row was put or deleted.
    #[serde(rename = "BriefChanged")]
    Changed {
        /// The row.
        id: BriefId,
        /// Deleted.
        removed: bool,
    },
}

/// One closed utterance of the live transcript: a `live.jsonl` line and the
/// batcher's unit (02 §2, §7). Times are on the recording clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Utterance {
    /// Which side spoke.
    pub track: Track,
    /// Start.
    pub t0_ms: u64,
    /// End.
    pub t1_ms: u64,
    /// What was said.
    pub text: String,
    /// Closed by a push-to-talk release (53 decision 10).
    pub request: bool,
    /// 07 §4: live speaker label (`s1`..`s8`) of a system utterance; `None` for the mic,
    /// when live diarization is off, or when no speaker covered the utterance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

impl Utterance {
    /// The speaker tag of the line the model reads (07 §4): `me` for the mic, the live
    /// label's display name (`Speaker 2`) when the diariser labelled it, else `them`.
    pub fn who(&self) -> String {
        match (self.track, &self.speaker) {
            (Track::Mic, _) => "me".to_owned(),
            (Track::System, Some(s)) => transcript::default_name(s),
            (Track::System, None) => "them".to_owned(),
        }
    }
}

/// `meeting:` config section (README §3). Phase 1 reads `enabled` and
/// `max_minutes`; phase 2 the batching, model, topic keys; phase 4 `models_dir`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MeetingConfig {
    /// `false`: no actor; `meeting_*` answers "meetings are disabled".
    pub enabled: bool,
    /// Hard stop; `NearMaxLength` at `max_minutes - 10`.
    pub max_minutes: u32,
    /// 02 §4.1: per-session compaction trigger (tokens) for the meeting session, set as a
    /// `SetCompaction` override. The agent sizes the kept tail and the head as fractions of
    /// `trigger − fixed` (COMPACT-FLOOR), so with a 32k trigger and a ~7k system prompt a
    /// meeting keeps roughly the last 8–9k tokens of transcript verbatim and compacts every
    /// ~15k tokens of growth — at no model cost, since the plugin supplies the head.
    pub context_tokens: u32,
    /// Batcher cadence: a batch goes out this long after the last send if anything is pending.
    pub batch_secs: u32,
    /// Batcher size: this many pending utterances flush at once.
    pub batch_utterances: u32,
    /// Nemotron streaming directory; `None` → `<data_dir>/models/nemotron-streaming-en`
    /// (the checkpoint dictation uses, `voice::STREAMING_MODEL_SUBDIR`).
    pub live_model_dir: Option<std::path::PathBuf>,
    /// How long a topic button lives without its label being spoken again.
    pub topic_ttl_secs: u32,
    /// Most topic buttons at once; the oldest untouched expires first.
    pub max_topics: u32,
    /// Phase 4: the diarization models root (`nemotron-diarization/`, `pyannote/`);
    /// `None` → `<data_dir>/models`.
    pub models_dir: Option<std::path::PathBuf>,
    /// 07: label the other side's live lines by speaker (`Speaker N:`) while recording.
    /// `false` → today's `them:`; the post pass diarizes either way. Read at meeting start.
    pub live_diarization: bool,
    /// 06 §1.5: the registry tools the meeting session may call directly while
    /// recording; every other tool is hidden from it until stop.
    pub live_tools: Vec<String>,
    /// 06 §1.5: the plugins whose prompt blocks the meeting session sees while recording.
    pub live_prompts: Vec<String>,
    /// 06 §1.5 / 81: lines per `<knowledge>` note while recording; 0 = none.
    pub live_hint_limit: u32,
    /// 158 §6.1 (W1): the meeting session's model (`provider/model`); `None` → `agent.model`.
    /// Carried, not yet read — W1 resolves `StartMeeting.session_model.or(cfg.model)`.
    pub model: Option<String>,
    /// 194 D2: the meeting session's effort beside `model`; `None` → `agent.effort` (D4a).
    pub effort: Option<String>,
    /// 158 §6.1 (W1): the summary turn's model; `None` → `model`. Carried, not yet read.
    pub summary_model: Option<String>,
    /// 158 §6.1 (W4): default note template slug when the start picks none and nothing is remembered.
    pub template: String,
    /// 158 §6.1: transcript language (BCP-47); stored, inert until the multilingual tier (§14).
    pub language: String,
    /// 158 §6.1 (W3): the project a meeting gets when nothing is remembered (§9.3).
    pub project: Option<String>,
    /// 158 §4 (W1): detection. Carried, not yet read.
    pub detect: DetectConfig,
    /// 158 §7 (W2): calendar. Carried, not yet read.
    pub calendar: CalendarConfig,
}

/// 158 §6.2: the user-facing subset of [`MeetingConfig`] that `meeting_settings_get` /
/// `meeting_settings_set` move whole (not `batch_secs`, `context_tokens`, paths).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct MeetingSettings {
    /// `meeting.model`.
    pub model: Option<String>,
    /// `meeting.effort` (194): the meeting session's effort; cleared with `model`.
    pub effort: Option<String>,
    /// `meeting.summary_model`.
    pub summary_model: Option<String>,
    /// `meeting.template`.
    pub template: String,
    /// `meeting.language`.
    pub language: String,
    /// `meeting.live_diarization`.
    pub live_diarization: bool,
    /// `meeting.live_tools`.
    pub live_tools: Vec<String>,
    /// `meeting.project`.
    pub project: Option<String>,
    /// `meeting.detect.*`.
    pub detect: DetectConfig,
    /// `meeting.calendar.*`.
    pub calendar: CalendarConfig,
}

impl Default for MeetingSettings {
    fn default() -> Self {
        Self::from(&MeetingConfig::default())
    }
}

impl From<&MeetingConfig> for MeetingSettings {
    fn from(c: &MeetingConfig) -> Self {
        Self {
            model: c.model.clone(),
            effort: c.effort.clone(),
            summary_model: c.summary_model.clone(),
            template: c.template.clone(),
            language: c.language.clone(),
            live_diarization: c.live_diarization,
            live_tools: c.live_tools.clone(),
            project: c.project.clone(),
            detect: c.detect.clone(),
            calendar: c.calendar.clone(),
        }
    }
}

impl MeetingSettings {
    /// Every key to `config.yaml` (158 §6.2), all or nothing: the writes land on a copy that
    /// replaces the file only once each succeeded. Values are JSON scalars/arrays, which YAML
    /// reads as quoted strings, `null`, booleans, numbers and flow sequences.
    fn persist(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("yaml.meeting.tmp");
        match std::fs::copy(path, &tmp) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e) => return Err(e),
        }
        match self.write_keys(&tmp) {
            Ok(()) => std::fs::rename(&tmp, path),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    fn write_keys(&self, path: &Path) -> std::io::Result<()> {
        fn j<T: Serialize>(v: &T) -> String {
            serde_json::to_string(v).expect("settings serialize")
        }
        for (k, v) in [
            ("model", j(&self.model)),
            ("effort", j(&self.effort)),
            ("summary_model", j(&self.summary_model)),
            ("template", j(&self.template)),
            ("language", j(&self.language)),
            ("live_diarization", j(&self.live_diarization)),
            ("live_tools", j(&self.live_tools)),
            ("project", j(&self.project)),
        ] {
            runtime::write_config_value(path, "meeting", k, &v)?;
        }
        for (k, v) in [
            ("enabled", j(&self.detect.enabled)),
            ("ignore", j(&self.detect.ignore)),
            ("poll_secs", j(&self.detect.poll_secs)),
        ] {
            runtime::write_nested_config_value(path, "meeting", "detect", k, &v)?;
        }
        for (k, v) in [
            ("eventkit", j(&self.calendar.eventkit)),
            ("prebrief", j(&self.calendar.prebrief)),
            ("poll_secs", j(&self.calendar.poll_secs)),
        ] {
            runtime::write_nested_config_value(path, "meeting", "calendar", k, &v)?;
        }
        Ok(())
    }

    /// Into the live config (the `meeting_settings_set` subset only).
    fn apply_to(self, c: &mut MeetingConfig) {
        c.model = self.model;
        c.effort = self.effort;
        c.summary_model = self.summary_model;
        c.template = self.template;
        c.language = self.language;
        c.live_diarization = self.live_diarization;
        c.live_tools = self.live_tools;
        c.project = self.project;
        c.detect = self.detect;
        c.calendar = self.calendar;
    }
}

/// `ask` → [`MeetingSettings`]: the live block (what a set left behind).
#[derive(Clone, Copy, Debug, Default)]
pub struct GetSettings;

/// `ask` → `Result<MeetingSettings, String>`: persist the whole block, then apply it live
/// (the detector restarts with its new list and cadence). A failed write changes nothing.
#[derive(Clone, Debug)]
pub struct SetSettings(pub MeetingSettings);

impl Message<GetSettings> for Meeting {
    type Reply = MeetingSettings;

    async fn handle(&mut self, _: GetSettings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        MeetingSettings::from(&self.cfg)
    }
}

impl Message<SetSettings> for Meeting {
    type Reply = Result<MeetingSettings, String>;

    async fn handle(
        &mut self,
        m: SetSettings,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // 194 D4b–d: `meeting.effort` against the model it goes with.
        let mut m = m;
        let info = agent::lifecycle::effort_model(m.0.model.as_deref()).await;
        m.0.effort = agent::lifecycle::settle_effort(
            m.0.model.as_deref(),
            info.as_ref(),
            m.0.effort.as_deref(),
            m.0.model != self.cfg.model,
        )?;
        m.0.persist(&self.rt.data_dir.join("config.yaml"))
            .map_err(|e| format!("config.yaml: {e}"))?;
        m.0.apply_to(&mut self.cfg);
        self.detect.apply(&self.cfg.detect, ctx.actor_ref());
        tracing::info!("meeting: settings updated");
        Ok(MeetingSettings::from(&self.cfg))
    }
}

/// `meeting_settings_get() -> MeetingSettings` (158 §6.2, W1): the live block from the actor,
/// or the file when meetings are disabled.
pub async fn rpc_settings_get(rt: &Runtime) -> anyhow::Result<MeetingSettings> {
    if let Ok(Some(actor)) = ActorRef::<Meeting>::lookup(MEETING_ACTOR) {
        return actor
            .ask(GetSettings)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    let cfg: MeetingConfig = rt.section("meeting").unwrap_or_default();
    Ok(MeetingSettings::from(&cfg))
}

/// `meeting_settings_set(MeetingSettings) -> MeetingSettings` (158 §6.2, W1).
pub async fn rpc_settings_set(
    _rt: &Runtime,
    settings: MeetingSettings,
) -> anyhow::Result<MeetingSettings> {
    ActorRef::<Meeting>::lookup(MEETING_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("meetings are disabled"))?
        .ask(SetSettings(settings))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

impl MeetingConfig {
    /// 06 §1.5: the session's allowlist while recording.
    fn live_toolset(&self) -> agent::Toolset {
        agent::Toolset {
            tools: self.live_tools.clone(),
            prompts: self.live_prompts.clone(),
            runtime: false,
            hint_limit: Some(self.live_hint_limit),
        }
    }
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_minutes: 180,
            context_tokens: 32_000,
            batch_secs: 25,
            batch_utterances: 6,
            live_model_dir: None,
            topic_ttl_secs: 600,
            max_topics: 6,
            models_dir: None,
            live_diarization: true,
            live_tools: [
                "meeting_topic",
                "meeting_brief",
                "brief_search",
                "brief_put",
                "knowledge_search",
                "knowledge_get",
                "meeting_read",
                "meeting_search",
                "search_web",
                "read_url",
            ]
            .map(String::from)
            .to_vec(),
            live_prompts: ["meeting", "knowledge", "web"].map(String::from).to_vec(),
            live_hint_limit: 2,
            model: None,
            effort: None,
            summary_model: None,
            template: templates::DEFAULT.to_owned(),
            language: "en".to_owned(),
            project: None,
            detect: DetectConfig::default(),
            calendar: CalendarConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Messages (01 §3). All `ask`; all `Result<_, String>` with the user's text.

/// `ask` → `Result<Vec<ProcessInfo>, String>`: processes Core Audio knows,
/// active first, then by name; our own pid skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListProcesses;

/// `ask` → `Result<Started, String>`. `consent == false` → `Err`; `process:
/// None` → all system audio ([`system_audio`]); a meeting already recording →
/// `Err`. Replies once the devices are open; `State{Recording}` and `Started`
/// are published just before.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StartMeeting {
    /// Pid of the meeting app (`meeting_processes`); `None` records all system audio.
    pub process: Option<u32>,
    /// Optional title; the session is named `Meeting <yyyy-mm-dd hh:mm>` regardless.
    #[serde(default)]
    pub title: Option<String>,
    /// Free text the user typed about the meeting (phase 2 sends it to the session).
    #[serde(default)]
    pub context: Option<String>,
    /// "I acknowledge this recording is legal" (53 §0.14). Required.
    #[serde(default)]
    pub consent: bool,
    /// Model for the meeting session (phase 2); stored in the manifest.
    #[serde(default)]
    pub session_model: Option<String>,
    /// 194 D2: the effort for that session; read only with `session_model` (D4d), else
    /// `meeting.effort` goes with `meeting.model`. `None` = `agent.effort`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub session_effort: Option<String>,
    /// 137 R1-D6: the session's memory mode; `None` = `agent.endless.default_meeting`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub memory: Option<agent::MemoryMode>,
    /// 158 §15.1 (W1): how the start happened; `None` = manual. Carried, not yet read.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub source: Option<detect::Source>,
    /// 158 §8.3 (W2): the calendar event to seed from. Carried, not yet read.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub event: Option<EventId>,
    /// 158 §9.2 (W3): the meeting's project; `None` = remembered (`uid:` → `title:`) or `meeting.project`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub project: Option<String>,
    /// 158 §11.1 (W4): the note template slug; `None` = remembered per event/project, else `meeting.template`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub template: Option<String>,
}

/// Reply to [`StartMeeting`].
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Started {
    /// The meeting.
    pub meeting: MeetingId,
    /// Its session.
    pub session: SessionId,
}

/// `ask` → `Result<(), String>`: `Recording` → `Paused`; any other state → `Err(state)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PauseMeeting;

/// `ask` → `Result<(), String>`: `Paused` → `Recording`; any other state → `Err(state)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResumeMeeting;

/// `ask` → `Result<(), String>`: `Recording | Paused` → `Stopping`, then the post
/// pipeline (`Post(_)` → `Review`, phase 4) runs in the background. Replies once
/// the last Ogg pages are on disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct StopMeeting;

/// `ask` → [`Status`]: for a UI on reconnect.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeetingStatus;

/// Reply to [`MeetingStatus`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
// `MeetingStatus` on the wire: `Status.ts` is the todo item state's file.
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "MeetingStatus")
)]
pub struct Status {
    /// `Idle` when nothing is recording.
    pub state: MeetingState,
    /// The current meeting.
    pub meeting: Option<MeetingId>,
    /// Its session.
    pub session: Option<SessionId>,
    /// Recording clock (pauses excluded).
    pub elapsed_ms: u64,
    /// Everything degraded so far.
    pub degraded: Vec<Degradation>,
    /// Where the island says the user is reading (`SetCursor`); phase 2.
    #[serde(default)]
    pub cursor: Option<BriefId>,
    /// Live topic buttons; phase 2.
    #[serde(default)]
    pub topics: Vec<Topic>,
    /// The session is waiting on a tool approval; phase 2.
    #[serde(default)]
    pub approval_waiting: bool,
    /// What this host can do (58 §3.3); `None` when no loopback backend opens.
    pub supported: Support,
    /// The ONNX Runtime execution provider diarisation sessions run on
    /// (`models::onnx::ep`: `DmlExecutionProvider` | `CPUExecutionProvider`).
    #[serde(default)]
    pub accelerator: String,
    /// 158 §4.5 (W1): the current detection prompt, so a companion connecting mid-prompt
    /// renders it. `None` until W1.
    #[serde(default)]
    pub detection: Option<Detection>,
    /// 173: the capture tracks' health; all `Ok` when nothing records.
    #[serde(default)]
    pub capture: CaptureStatus,
}

/// `ask` → `Result<Vec<MeetingSummary>, String>`: from the index, newest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListMeetings {
    /// Most rows; `None` = 100.
    pub limit: Option<u32>,
    /// 158 §9.2 (W3): only meetings of this project.
    #[serde(default)]
    pub project: Option<String>,
}

/// `ask` → `Result<Manifest, String>`: the manifest as on disk.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetMeeting {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → `Result<(), String>`: directory, index row, then `agent::DeleteSession`,
/// in that order (a half-failure leaves a session pointing at nothing rather than
/// files without a session). `Err` for the meeting currently recording.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DeleteMeeting {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → [`Permissions`]. Never prompts.
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckPermissions;

/// TCC state as far as it can be known without prompting (01 §0.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PermissionState {
    /// Allowed.
    Granted,
    /// Refused (or restricted).
    Denied,
    /// Never asked.
    NotDetermined,
}

/// Reply to [`CheckPermissions`]. `microphone` is `AVCaptureDevice`'s
/// answer; `audio_capture` has no query API on macOS — it is what the last
/// tap open taught us (`NotDetermined` until a meeting has been started).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Permissions {
    /// Microphone.
    pub microphone: PermissionState,
    /// System Audio Recording Only.
    pub audio_capture: PermissionState,
}

// Phase 2 (02 §6). All `ask` → `Result<_, String>` unless noted.

/// The user tapped a topic button: `TopicRemoved{Viewed}`, the precomputed
/// brief shown as `Brief{source: Tap}`, `[meeting tapped "X"]` steered.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TapTopic {
    /// Which.
    pub topic: TopicId,
}

/// The user dismissed a topic button: `TopicRemoved{Dismissed}`, the label
/// barred for the meeting, `[meeting: user dismissed …]` steered.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DismissTopic {
    /// Which.
    pub topic: TopicId,
}

/// The user asked for more on a brief: `[meeting deeper on …]` steered.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Deeper {
    /// Which.
    pub brief: BriefId,
}

/// A request typed in the control center: `[meeting request, typed] …` steered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypedRequest {
    /// The question.
    pub text: String,
}

/// Push-to-talk pressed or released (53 decision 10): publishes `Listening`;
/// the release tags the mic utterance that closes as a spoken request.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PushToTalk {
    /// Held.
    pub held: bool,
}

/// The island's cursor, mirrored to `Status.cursor` for the control center.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SetCursor {
    /// The brief in view; `None` = the newest.
    pub brief: Option<BriefId>,
}

/// `ask` → `Result<Vec<Brief>, String>`: every brief of `meeting`, in order
/// (memory while it records, `briefs.jsonl` after).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ListBriefs {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → `Vec<Topic>`: the live topic buttons of the current meeting.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListTopics;

/// `ask` → `Result<Vec<Utterance>, String>`: `live.jsonl` of `meeting`, the last 2 000 lines.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LiveText {
    /// Which.
    pub meeting: MeetingId,
}

/// From `meeting_topic` (02 §5.1, 05 §4): `ask` → `Result<String, String>` — the
/// text the model reads back. Either `title` + `bullets` (a new brief, also
/// written to the cache) or `brief` (a cache row, registered by reference; the
/// label becomes one of its aliases).
#[derive(Clone, Debug)]
pub struct AddTopic {
    /// The calling session; must be the meeting's.
    pub session: SessionId,
    /// ≤ 4 words.
    pub label: String,
    /// Brief title.
    pub title: Option<String>,
    /// 2–3 bullets.
    pub bullets: Option<Vec<String>>,
    /// A cached brief to reuse.
    pub brief: Option<BriefId>,
}

/// From `meeting_brief` (02 §5.2): `ask` → `Result<String, String>`.
#[derive(Clone, Debug)]
pub struct AddBrief {
    /// The calling session; must be the meeting's.
    pub session: SessionId,
    /// Title.
    pub title: String,
    /// 2–3 bullets.
    pub bullets: Vec<String>,
    /// The live topic this answers.
    pub topic: Option<TopicId>,
    /// The brief this goes deeper on.
    pub parent: Option<BriefId>,
    /// Phase 5: `Some` → also cached under these aliases.
    pub aliases: Option<Vec<String>>,
}

// Phase 5 (05 §8): the brief cache. Tools and RPC both go through the actor so
// the alias list and the live meeting's bookkeeping stay consistent.

/// `ask` → `Result<Vec<CachedBrief>, String>`: FTS, best rank first (05 §4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BriefSearch {
    /// Words; OR-joined.
    pub query: String,
    /// ≤ 50; default 8.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Vec<CachedBrief>, String>`: newest `updated_at` first, optionally one tag.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BriefList {
    /// Only rows tagged this.
    pub tag: Option<String>,
    /// ≤ 50; default 50.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Vec<String>, String>`: every tag, most-used first (the `/briefs` chip row).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct BriefTags;

/// `ask` → `Result<CachedBrief, String>`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BriefGet {
    /// Which.
    pub id: BriefId,
}

/// `ask` → `Result<(CachedBrief, bool /* created */), String>`: upsert; publishes
/// `BriefEvent::Changed`; with a meeting live, re-publishes the brief if its topic is on the island.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BriefPut {
    /// The row.
    pub put: CachedBriefPut,
    /// `session:<id>` from a tool, `user` from the UI; kept when the row exists.
    pub source: String,
}

/// `ask` → `Result<(), String>`: delete a row; publishes `BriefEvent::Changed { removed: true }`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BriefDelete {
    /// Which.
    pub id: BriefId,
}

// Phase 4 (04 §5). All `ask` → `Result<_, String>`.

/// `ask` → `Result<Transcript, String>`: `transcript.json` as on disk.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetTranscript {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → `Result<Transcript, String>`: apply review edits (all or none),
/// rewrite `transcript.json`, re-index; the result is the new transcript.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditTranscript {
    /// Which.
    pub meeting: MeetingId,
    /// In order.
    pub ops: Vec<EditOp>,
}

/// `ask` → `Result<String, String>`: `[t0, t1)` of one track as a base64 16-bit
/// mono WAV (04 §0.5), for the review pane's player.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SegmentAudio {
    /// Which.
    pub meeting: MeetingId,
    /// Which file.
    pub track: Track,
    /// Start.
    pub t0_ms: u64,
    /// End.
    pub t1_ms: u64,
}

/// `ask` → `Result<Vec<Profile>, String>`: the enrolled speakers, by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSpeakers;

/// `ask` → `Result<(), String>`: remove a profile file. Transcripts keep their labels.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DeleteSpeaker {
    /// Which.
    pub profile: ProfileId,
}

/// 156 D14: whose voiceprint the chosen speech becomes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind")]
pub enum EnrollTarget {
    /// A new profile; refused when that name (case-insensitive) is enrolled.
    New {
        /// Display name.
        name: String,
        /// Knowledge entity id, if the user picked one.
        entity: Option<String>,
        /// 158 §10.4: the automatic path (a suggestion's Accept) runs the quality gates (≥ 5 s,
        /// agreement, SNR, clipping) before embedding; the manual selection enrol (156) does not.
        #[serde(default)]
        gated: bool,
    },
    /// Fold the speech into this enrolled profile ("enhance").
    Existing {
        /// Which.
        profile: ProfileId,
    },
}

/// 156 D14: what `EnrollSegments` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Enrolled {
    /// The profile as written.
    pub profile: Profile,
    /// Speech used for the voiceprint (<= 30 s).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub speech_ms: u64,
    /// Speech in the chosen segments before the cap.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub available_ms: u64,
    /// The transcript after linking (label.profile/name set, proposed refreshed); unchanged when not linked.
    pub transcript: Transcript,
}

/// 156 D14: `ask` → `Result<Enrolled, String>`: a voiceprint from the word spans of
/// `segments` (one track, first 30 s), as a new profile or folded into an enrolled one;
/// the label is linked when every segment shares one. Review and Done only. The embedding
/// runs off the mailbox; the profile write, the link and the transcript save run on it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrollSegments {
    /// Which.
    pub meeting: MeetingId,
    /// Segment ids.
    pub segments: Vec<u32>,
    /// New or existing profile.
    pub target: EnrollTarget,
}

/// Second half of [`EnrollSegments`], asked by its task once the embedding is back: the
/// profile write, the link and the save, on the actor so a concurrent edit is not lost.
struct EnrollApply {
    m: EnrollSegments,
    emb: Vec<f32>,
    speech_ms: u64,
    available_ms: u64,
}

/// `ask` → `bool`: the `meeting.live_diarization` switch (07 §6) as configured.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct LiveDiarization;

/// `ask` → `Result<bool, String>`: persist `meeting.live_diarization` in `config.yaml` and
/// apply it from the next meeting start (a running meeting keeps its mode). A failed
/// config write changes nothing.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetLiveDiarization {
    /// The new value.
    pub on: bool,
}

/// `ask` → `Result<(), String>`: `Review` → `Summarizing`; the guided form goes
/// to the session; `Summarized{title}` (and `Done`) follow when the turn ends.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Summarize {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → `Result<(), String>`: re-run the post pipeline from stage 1.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PostRetry {
    /// Which.
    pub meeting: MeetingId,
}

/// `ask` → `Result<String, String>`: `summary.md`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetSummary {
    /// Which.
    pub meeting: MeetingId,
}

/// `tell` from the 1 s interval: batcher rule (a), topic expiry.
struct Tick;

/// `tell` from the start task: the devices are open (or not), and the two
/// streaming STT sessions if they loaded.
struct Begun {
    manifest: Manifest,
    result: Result<capture::Capture, String>,
    engines: Option<(Utterer, Utterer)>,
    /// 07 §2: the live diariser's session, when enabled and its models are present.
    diar: Option<diarize::Streamer>,
    /// The note documents were still creating when the meeting started (`note::Created::Late`).
    late_note: Option<tokio::task::JoinHandle<Option<document::DocId>>>,
}

/// `tell` from the timers, the capture threads and the STT tasks: something degraded.
pub(crate) struct Degrade(pub(crate) Degradation);

/// `tell` from the late-note task: the note `meeting_start` did not wait for is there.
pub(crate) struct NoteReady {
    pub(crate) meeting: MeetingId,
    pub(crate) doc: document::DocId,
}

// ---------------------------------------------------------------------------
// Actor

/// The [`MEETING_ACTOR`] actor: one recording at a time (one mic).
#[derive(kameo::Actor)]
pub struct Meeting {
    rt: Runtime,
    cfg: MeetingConfig,
    /// 137 R1-D6: `agent.endless.{default_meeting, meeting_view_bytes}`.
    endless: endless::Config,
    pub(crate) store: Arc<Store>,
    /// The brief cache (phase 5); `None` when `briefs.sqlite` would not open — the
    /// meeting then runs on the model path only.
    cache: Option<Arc<Cache>>,
    /// The live meeting, from `Begun(Ok)` to `StopMeeting`.
    current: Option<Recording>,
    /// Set between `StartMeeting` and `Begun`; the manifest of the meeting being set up.
    starting: Option<Manifest>,
    audio_capture: PermissionState,
    /// Post pipelines in flight (phase 4); finished handles are pruned on access.
    post: Vec<post::Job>,
    /// Meetings whose summary turn is queued or running (04 §6), by id. Each is
    /// bound to its own turn through the form's input id, so a turn already in
    /// flight at `Summarize` (the stop-flush batch) is never taken for it, and
    /// two meetings may summarize at once — their sessions are distinct.
    summarizing: HashMap<MeetingId, Summarizing>,
    /// 158 §4: the mic sensor's rules engine, theuth's own voice state, the poll task.
    detect: detect::Detector,
}

/// One meeting's summary in flight: the form's input id, and the turn it opened
/// once `AgentEvent::InputInjected` echoed it.
struct Summarizing {
    manifest: Manifest,
    input: InputId,
    turn: Option<TurnId>,
    /// 158 §6.1: the model to put back on the session once the summary turn settles.
    restore: Option<String>,
}

/// Wall clock on one summary from the form's delivery to its turn's end (04 §6). The
/// turn reads the transcript in pieces and writes at most 400 words (or edits the note);
/// a batch turn ahead of it is one brief. Past this the slot is released and the meeting
/// goes back to `Review` with `failure` set, so nothing stays `Summarizing` forever.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// `tell` from the watchdog spawned by [`Meeting::begin_summary`]: `input` names the
/// attempt, so a timer from an earlier, already settled attempt is a no-op.
struct SummaryTimeout {
    meeting: MeetingId,
    input: InputId,
}

/// `tell` at plugin start for a manifest found `Summarizing`: settle on the turn the
/// history already holds, else re-send the form (04 §2).
struct Resummarize(Manifest);

impl Meeting {
    fn cache(&self) -> Result<Arc<Cache>, String> {
        self.cache
            .clone()
            .ok_or_else(|| "the brief cache is unavailable".to_owned())
    }

    /// 05 §3: aliases spoken in `text`, as `(row, label as spoken)`; empty without a cache.
    fn scan(&self, text: &str) -> Vec<(BriefId, String)> {
        match &self.cache {
            Some(c) => cache::scan(text, &c.aliases()),
            None => Vec::new(),
        }
    }

    /// Write a row and tell the world (05 §0.8). Both the tools and the RPC land here.
    async fn cache_put(
        &self,
        put: CachedBriefPut,
        source: &str,
    ) -> Result<(CachedBrief, bool), String> {
        let (row, created) = self.cache()?.put(put, source).await?;
        self.rt
            .publish(BriefEvent::Changed {
                id: row.id,
                removed: false,
            })
            .await;
        Ok((row, created))
    }

    /// `uses += 1` off the hot path.
    fn touch_use(&self, id: BriefId) {
        if let Some(c) = self.cache.clone() {
            tokio::spawn(async move {
                if let Err(e) = c.touch_use(id).await {
                    tracing::debug!("brief cache: touch: {e:#}");
                }
            });
        }
    }

    /// The hot path (05 §3): every alias hit whose row was not shown or dismissed
    /// this meeting becomes a topic with the cached brief, published before any
    /// model turn. `since` is the utterance close (`Piece.at`, so it includes the
    /// STT flush and the mailbox hop) for the latency log; the actor's own share is
    /// measured from entry.
    async fn cached_topics(&mut self, hits: Vec<(BriefId, String)>, at_ms: u64, since: Instant) {
        let entered = Instant::now();
        let Some(cache) = self.cache.clone() else {
            return;
        };
        for (row, label) in hits {
            let Some(rec) = self.current.as_mut() else {
                return;
            };
            if rec.live.topics.cached_blocked(row) {
                continue;
            }
            let row = match cache.get(row).await {
                Ok(r) => r,
                Err(_) => continue, // deleted between the scan and now
            };
            let Some(rec) = self.current.as_mut() else {
                return;
            };
            let (topic, brief, expired) = match rec.live.topics.insert_cached(
                &label,
                &row,
                BriefSource::Cache,
                Instant::now(),
                at_ms,
            ) {
                Ok(x) => x,
                Err(e) => {
                    tracing::debug!(label, "meeting: cached topic skipped: {e}");
                    continue;
                }
            };
            rec.live.log_record(&Record::Topic(topic.clone()));
            rec.live.log_record(&Record::Brief(brief));
            self.topics_gone(expired).await;
            self.topic_added(topic).await;
            tracing::debug!(
                label,
                since_t1_us = since.elapsed().as_micros(),
                actor_us = entered.elapsed().as_micros(),
                "meeting: cached topic published"
            );
            self.touch_use(row.id);
        }
    }

    /// A request named a cached subject: show the row's brief at once as
    /// `Request` (05 §3); the title goes into the request line's suffix.
    async fn cached_request(&mut self, hits: &[(BriefId, String)]) -> Option<String> {
        let cache = self.cache.clone()?;
        for (id, _) in hits {
            let Ok(row) = cache.get(*id).await else {
                continue;
            };
            let rec = self.current.as_mut()?;
            let at_ms = rec.clock.now_ms();
            let Ok(brief) = rec.live.topics.add_brief(
                &row.title,
                row.bullets.clone(),
                None,
                None,
                BriefSource::Request,
                Some(row.id),
                at_ms,
            ) else {
                continue;
            };
            self.show_brief(brief).await;
            self.touch_use(row.id);
            return Some(row.title);
        }
        None
    }
}

impl Meeting {
    async fn publish_state(&self, m: &Manifest) {
        self.rt
            .publish(MeetingEvent::State {
                meeting: m.id,
                state: m.state,
                degraded: m.degraded.clone(),
            })
            .await;
    }

    /// Rewrite the manifest (atomic) and its index row, then publish `State`.
    async fn save(&self, m: &Manifest) {
        if let Err(e) = self.store.save(m).await {
            tracing::warn!(meeting = %m.id, "manifest: {e:#}");
        }
        self.publish_state(m).await;
    }

    /// Model and profile directories for the post pipeline (04 §0.1).
    fn paths(&self) -> post::Paths {
        let models = self.rt.data_dir.join("models");
        post::Paths {
            tdt: models.join(voice::TDT_MODEL_SUBDIR),
            models: self.cfg.models_dir.clone().unwrap_or(models),
            speakers: self.rt.data_dir.join(DIR).join(speakers::DIR),
            data_dir: self.rt.data_dir.clone(),
        }
    }

    /// Start (or refuse to duplicate) the post pipeline for `manifest`.
    fn start_post(&mut self, manifest: Manifest) -> Result<(), String> {
        self.post.retain(|j| !j.handle.is_finished());
        if self.post.iter().any(|j| j.meeting == manifest.id) {
            return Err("the post pipeline is already running for that meeting".into());
        }
        self.post.push(post::spawn(
            self.rt.clone(),
            self.store.clone(),
            manifest,
            self.paths(),
        ));
        Ok(())
    }

    /// The meeting being recorded or started, if any.
    fn live_id(&self) -> Option<MeetingId> {
        self.current
            .as_ref()
            .map(|r| r.manifest.id)
            .or_else(|| self.starting.as_ref().map(|m| m.id))
    }

    /// The manifest of a meeting that is not the live one.
    fn stored(&self, id: MeetingId) -> Result<Manifest, String> {
        if self.live_id() == Some(id) {
            return Err("that meeting is recording; stop it first".into());
        }
        self.store.manifest(id).map_err(|e| format!("{e:#}"))
    }

    /// 156 D14: the manifest of a meeting whose transcript may take an enrollment
    /// (Review or Done); every other state is refused in the UI's words.
    fn enrollable(&self, id: MeetingId) -> Result<Manifest, String> {
        const RECORDING: &str = "still recording: wait for the meeting to end";
        if self.live_id() == Some(id) {
            return Err(RECORDING.into());
        }
        let m = self.store.manifest(id).map_err(|e| format!("{e:#}"))?;
        match m.state {
            MeetingState::Review | MeetingState::Done => Ok(m),
            MeetingState::Post(_) => Err("still processing: wait for the post pipeline".into()),
            MeetingState::Summarizing => Err("Summarizing: wait for it to finish".into()),
            MeetingState::Starting
            | MeetingState::Recording
            | MeetingState::Paused
            | MeetingState::Stopping => Err(RECORDING.into()),
            MeetingState::Idle | MeetingState::Failed => {
                Err(format!("{:?}: this meeting has no transcript", m.state))
            }
        }
    }

    /// The file of `track` in `manifest`'s directory.
    fn track_path(&self, manifest: &Manifest, track: Track) -> PathBuf {
        let stem = match track {
            Track::Mic => Stem::Mic,
            Track::System => Stem::System,
        };
        self.store.meeting_dir(manifest.id).join(
            manifest
                .tracks
                .iter()
                .find(|t| t.track == stem)
                .map_or(stem.file(), |t| t.file.as_str()),
        )
    }

    fn state(&self) -> MeetingState {
        match (&self.current, &self.starting) {
            (Some(r), _) => r.manifest.state,
            (None, Some(_)) => MeetingState::Starting,
            (None, None) => MeetingState::Idle,
        }
    }

    /// Timers that ride along with a recording: levels at 10 Hz, the max-length
    /// warning and stop, the disk check every 60 s.
    fn spawn_tasks(&self, id: MeetingId, rec: &mut Recording, me: ActorRef<Self>) {
        let rt = self.rt.clone();
        let mut level = rec.capture.level.clone();
        rec.tasks.push(tokio::spawn(async move {
            while level.changed().await.is_ok() {
                let (mic_rms, system_rms) = *level.borrow_and_update();
                rt.publish(MeetingEvent::Level {
                    meeting: id,
                    mic_rms,
                    system_rms,
                })
                .await;
            }
        }));
        let max = Duration::from_secs(u64::from(self.cfg.max_minutes) * 60);
        let warn = max.saturating_sub(Duration::from_secs(600));
        let actor = me.clone();
        rec.tasks.push(tokio::spawn(async move {
            tokio::time::sleep(warn).await;
            let _ = actor.tell(Degrade(Degradation::NearMaxLength)).await;
            tokio::time::sleep(max - warn).await;
            let _ = actor.ask(StopMeeting).await;
        }));
        let dir = self.rt.data_dir.clone();
        let actor = me.clone();
        rec.tasks.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if disk_low(&dir) {
                    let _ = actor.tell(Degrade(Degradation::DiskLow)).await;
                }
            }
        }));
        // 02 §0.3: the batcher's rule (a) and topic expiry, once a second.
        rec.tasks.push(tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(1));
            loop {
                every.tick().await;
                if me.tell(Tick).await.is_err() {
                    return;
                }
            }
        }));
    }

    /// The recording, if it is `Recording` or `Paused` and is `session`'s.
    fn recording_for(&mut self, session: SessionId) -> Result<&mut Recording, String> {
        self.current
            .as_mut()
            .filter(|r| r.manifest.session == session)
            .ok_or_else(|| "no meeting is recording in this session".to_owned())
    }

    /// `Steer` if the session has a turn running, else `SendInput` (02 §3).
    async fn deliver(&self, text: String) {
        let Some(rec) = self.current.as_ref() else {
            return;
        };
        let session = rec.manifest.session;
        if let Err(e) = session::deliver(session, text, rec.live.turn_running).await {
            tracing::warn!(%session, "meeting: deliver: {e}");
        }
    }

    /// 02 §4.1: re-render the state head and, when it differs from the last one pushed,
    /// hand it to the session with the trigger and hint — one ask per change, never a timer.
    /// 137 R1-D6: with the mode on, the view is re-read first (same cadence, one more
    /// section); a view that does not answer in time leaves the last one in place.
    async fn push_head(&mut self) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if rec.manifest.memory == agent::MemoryMode::Endless
            && let Some(port) = endless::port()
            && let Some(e) = endless::earlier(
                port.as_ref(),
                rec.manifest.session,
                self.endless.meeting_view_bytes,
            )
            .await
        {
            rec.live.earlier = Some(e);
        }
        let head = head::render(&head::of(rec, chrono::Utc::now(), rec.live.earlier.clone()));
        if !head::changed(rec.live.last_head.as_deref(), &head) {
            return;
        }
        rec.live.last_head = Some(head.clone());
        let session = rec.manifest.session;
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Err(e) = agents
                .ask(agent::SetCompaction {
                    session,
                    at_tokens: Some(self.cfg.context_tokens),
                    hint: Some(tools::COMPACTION_HINT.to_owned()),
                    head: Some(head),
                })
                .await
        {
            tracing::warn!(%session, "meeting: push head: {e}");
        }
    }

    /// One transcript batch, or a deferral when the session's steer queue is deep. The batch
    /// goes through the decision oracle off the mailbox (57 §3; [`oracle::run`]) and comes
    /// back as a [`oracle::Verdict`]; with the oracle off that is today's delivery, one hop later.
    async fn send_batch(&mut self, batch: Vec<Utterance>, me: &ActorRef<Self>) {
        let Some(rec) = self.current.as_ref() else {
            return;
        };
        let session = rec.manifest.session;
        if session::queued(session).await > batch::MAX_QUEUED {
            if let Some(rec) = self.current.as_mut() {
                rec.live.batcher.defer(batch);
            }
            return;
        }
        let Some(rec) = self.current.as_ref() else {
            return;
        };
        let meeting = rec.manifest.id;
        let blocked = rec.live.topics.cached_blocked_rows();
        let (rt, cache, me) = (self.rt.clone(), self.cache.clone(), me.clone());
        tokio::spawn(async move {
            let verdict = oracle::run(
                rt,
                cache,
                blocked,
                batch,
                meeting,
                session,
                oracle::side_caller(),
            )
            .await;
            let _ = me.tell(verdict).await;
        });
    }

    /// Deliver a batch to the model as today (57 §3.2 tier 3): the carried batches ride along
    /// as context, the evidence line last; what the oracle expected waits for the turn's end.
    async fn deliver_batch(
        &mut self,
        batch: Vec<Utterance>,
        evidence: Option<String>,
        pending: Option<(OracleDecision, oracle::Expected, Option<ExampleId>)>,
    ) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let carried = std::mem::take(&mut rec.live.carry);
        let text = session::batch_line_with(
            &batch,
            &rec.live.topics.batch_labels(chrono::Utc::now()),
            rec.live.topics.dismissed_labels(),
            &carried,
            evidence.as_deref(),
        );
        if let Some(p) = pending {
            rec.live.pending.push(p);
        }
        self.push_head().await;
        self.deliver(text).await;
    }

    /// The oracle kept `batch` from the model: it becomes context for the next delivered one.
    fn carry(&mut self, batch: Vec<Utterance>) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        rec.live.carry.push((oracle::batch_id(&batch), batch));
        // ponytail: a long quiet stretch would otherwise grow the next line without bound;
        // the transcript is on disk anyway (`meeting_read`).
        const MAX_CARRY: usize = 6;
        if rec.live.carry.len() > MAX_CARRY {
            rec.live.carry.drain(..rec.live.carry.len() - MAX_CARRY);
        }
    }

    /// 57 §3.4: the model turn ended — settle every pending oracle verdict against what the
    /// turn published, report the outcomes to the decision log and update the events. 57 §11.4:
    /// captured examples get an `amend { outcome }`, and the published topics are remembered
    /// so a tap or dismiss on them amends the same examples.
    async fn settle_pending(&mut self) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if rec.live.pending.is_empty() {
            rec.live.published.clear();
            rec.live.published_topics.clear();
            return;
        }
        let meeting = rec.manifest.id;
        let published = std::mem::take(&mut rec.live.published);
        let topics = std::mem::take(&mut rec.live.published_topics);
        let pending = std::mem::take(&mut rec.live.pending);
        let engine = ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR)
            .ok()
            .flatten();
        for (mut event, expected, example) in pending {
            let (outcome, reports) = oracle::outcome(&expected, &published);
            if let Some(engine) = &engine {
                for (log_id, outcome) in reports {
                    let _ = engine.tell(decision::Report { log_id, outcome }).await;
                }
            }
            if let Some(id) = example {
                oracle::amend(
                    id,
                    Amendment::Outcome {
                        published: published.first().map(|(row, title)| {
                            decision::training::Brief {
                                id: row.map(|r| r.to_string()),
                                title: title.clone(),
                            }
                        }),
                    },
                );
                rec.live.captured.extend(topics.iter().map(|t| (*t, id)));
            }
            event.outcome = Some(outcome);
            self.rt
                .publish(MeetingEvent::OracleDecision {
                    meeting,
                    decision: event,
                })
                .await;
        }
    }

    /// A topic left the live set: event + `briefs.jsonl` record.
    async fn topic_gone(&mut self, topic: TopicId, why: TopicGone) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let meeting = rec.manifest.id;
        rec.live.log_record(&Record::Removed {
            topic,
            why,
            at_ms: rec.clock.now_ms(),
        });
        self.rt
            .publish(MeetingEvent::TopicRemoved {
                meeting,
                topic,
                why,
            })
            .await;
        self.push_head().await;
    }

    /// A topic joined the live set: event + head.
    async fn topic_added(&mut self, topic: Topic) {
        let Some(rec) = self.current.as_ref() else {
            return;
        };
        let meeting = rec.manifest.id;
        self.rt
            .publish(MeetingEvent::TopicAdded { meeting, topic })
            .await;
        self.push_head().await;
    }

    async fn topics_gone(&mut self, gone: Vec<Gone>) {
        for g in gone {
            self.topic_gone(g.topic, g.why).await;
        }
    }

    /// A brief to show: event + record.
    async fn show_brief(&mut self, brief: Brief) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let meeting = rec.manifest.id;
        rec.live.log_record(&Record::Brief(brief.clone()));
        self.rt
            .publish(MeetingEvent::Brief { meeting, brief })
            .await;
        self.push_head().await;
    }

    /// A closed utterance: `live.jsonl`, the alias scan (05 §3), then the batcher
    /// (or the request line). `at` is the close stamp, for the latency log.
    async fn closed(&mut self, u: Utterance, at: Instant, me: &ActorRef<Self>) {
        let hits = self.scan(&u.text);
        if u.request {
            let cached = self.cached_request(&hits).await;
            let Some(rec) = self.current.as_mut() else {
                return;
            };
            rec.live.log_utterance(&u);
            rec.live
                .prompted(Prompted::Request { spoken: true }, u.text.clone());
            let line = session::request_line(true, &u.text, cached.as_deref());
            self.deliver(line).await;
            return;
        }
        self.cached_topics(hits, u.t1_ms, at).await;
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if u.track == Track::System && rec.live.diar.is_some() {
            // 07 §3: the line is logged and batched once the diariser has scored it.
            rec.live.held_system.push(u);
            self.release_held(me, false).await;
            return;
        }
        self.commit(u, me).await;
    }

    /// The tail of [`Meeting::closed`]: `live.jsonl`, then the batcher.
    async fn commit(&mut self, u: Utterance, me: &ActorRef<Self>) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        rec.live.log_utterance(&u);
        if let Some(batch) = rec.live.batcher.feed(u, Instant::now()) {
            self.send_batch(batch, me).await;
        }
    }

    /// 07 §3: label and commit the held system utterances the diariser's horizon has
    /// passed (all of them when `all`: the diariser is gone). A label is published as a
    /// second `final` `Utterance` event for the same row.
    async fn release_held(&mut self, me: &ActorRef<Self>, all: bool) {
        loop {
            let Some(rec) = self.current.as_mut() else {
                return;
            };
            let horizon = rec.live.labels.horizon_ms;
            if rec
                .live
                .held_system
                .first()
                .is_none_or(|u| !all && u.t1_ms > horizon)
            {
                return;
            }
            let mut u = rec.live.held_system.remove(0);
            u.speaker = diar::speaker_of(u.t0_ms, u.t1_ms, &rec.live.labels.spans);
            if u.speaker.is_some() {
                self.rt
                    .publish(MeetingEvent::Utterance {
                        meeting: rec.manifest.id,
                        track: u.track,
                        t0_ms: u.t0_ms,
                        t1_ms: u.t1_ms,
                        text: u.text.clone(),
                        request: u.request,
                        r#final: true,
                        speaker: u.speaker.clone(),
                    })
                    .await;
            }
            self.commit(u, me).await;
        }
    }

    /// `ApprovalWaiting` when the pending set flips between empty and not.
    async fn approval_waiting(&self, meeting: MeetingId, before: bool, after: bool) {
        if before != after {
            self.rt
                .publish(MeetingEvent::ApprovalWaiting {
                    meeting,
                    waiting: after,
                })
                .await;
        }
    }
}

/// A closed utterance worth keeping: at least one letter or digit (a lone `.`
/// from a breath or the far end's echo is not a line).
fn said_something(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// `t0`/`t1` of a piece on the recording clock.
fn times(clock: &RecordingClock, p: &Piece) -> (u64, u64) {
    let t1 = clock.elapsed_at(p.at);
    let t0 = t1.saturating_sub(p.behind);
    (t0.as_millis() as u64, t1.as_millis() as u64)
}

/// The last `cap` JSON lines of `path`, parsed; a missing file is empty.
pub(crate) fn read_jsonl<T: serde::de::DeserializeOwned>(
    path: &Path,
    cap: usize,
) -> Result<Vec<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let skip = lines.len().saturating_sub(cap);
    Ok(lines[skip..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Free space on `dir`'s volume under [`DISK_LOW_BYTES`] (`statvfs` on unix, `GetDiskFreeSpaceExW` on
/// Windows). Unknown → `false`.
fn disk_low(dir: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a valid NUL-terminated string; `st` is a valid out pointer.
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return false;
        }
        (st.f_bavail as u64).saturating_mul(st.f_frsize as u64) < DISK_LOW_BYTES
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::core::PCWSTR;
        let path: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_to_caller = 0u64;
        // SAFETY: `path` is a valid NUL-terminated wide string; the out pointer is valid for the call.
        let ok = unsafe {
            GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&mut free_to_caller), None, None)
        };
        ok.is_ok() && free_to_caller < DISK_LOW_BYTES
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        false
    }
}

impl Message<ListProcesses> for Meeting {
    type Reply = Result<Vec<ProcessInfo>, String>;

    async fn handle(
        &mut self,
        _: ListProcesses,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tokio::task::spawn_blocking(capture::system::processes)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<StartMeeting> for Meeting {
    type Reply = DelegatedReply<Result<Started, String>>;

    async fn handle(
        &mut self,
        m: StartMeeting,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if !m.consent {
            tx.send(Err("consent is required".into()));
            return reply;
        }
        if self.current.is_some() || self.starting.is_some() {
            tx.send(Err("a meeting is already recording".into()));
            return reply;
        }
        // 158 §5.4: a one-tap start names the prompted app; on a host whose render list
        // misses a mic-only process (Windows) the prompt's own `ProcessInfo` stands in.
        let prompted = self
            .detect
            .engine
            .prompt()
            .map(|d| d.app.clone())
            .filter(|p| Some(p.pid) == m.process);
        let process = match m.process {
            None => system_audio(),
            Some(pid) => match tokio::task::spawn_blocking(capture::system::processes).await {
                Ok(Ok(list)) => match list.into_iter().find(|p| p.pid == pid).or(prompted) {
                    Some(p) => p,
                    None => {
                        tx.send(Err(format!("pid {pid} is not playing audio")));
                        return reply;
                    }
                },
                Ok(Err(e)) => {
                    tx.send(Err(format!("{e:#}")));
                    return reply;
                }
                Err(e) => {
                    tx.send(Err(e.to_string()));
                    return reply;
                }
            },
        };
        // 158 §4.4 rule 8: any start clears the prompt.
        self.detect_starting(m.source.as_ref()).await;
        let now = chrono::Local::now();
        let mut manifest = Manifest::new(MeetingId::new(), process, now.to_rfc3339());
        manifest.title = m.title;
        manifest.context = m.context;
        // 158 §6.1: the start's pick, else `meeting.model`; `None` still falls to `agent.model`.
        // 194 D4a/d: each model brings its own effort; `None` still falls to `agent.effort`.
        (manifest.session_model, manifest.session_effort) =
            session_pick(m.session_model, m.session_effort, &self.cfg);
        // 158 §8.3 (W2): the picked calendar event seeds title, context and `manifest.event`.
        let picked = m.event.as_ref().or(match &m.source {
            Some(detect::Source::Calendar { event }) => Some(event),
            _ => None,
        });
        if let Some(id) = picked {
            match calendar::store::event(&self.store, id).await {
                Ok(Some(e)) => {
                    // W3 close: the islands' one-tap start from the Upcoming card sends the
                    // event's description as `context`; the seed carries it already.
                    let desc = e.to_ref().description;
                    if manifest.context.as_deref().map(str::trim) == desc.as_deref() {
                        manifest.context = None;
                    }
                    calendar::seed(&mut manifest, &e);
                }
                Ok(None) => tracing::warn!(event = %id, "meeting: start: unknown calendar event"),
                Err(e) => tracing::warn!("meeting: start: calendar event: {e:#}"),
            }
        }
        // 158 §9.3 (W3): the start's pick, else memory (`uid:` → `title:`), else `meeting.project`.
        // Only an explicit pick is remembered, so the default stays changeable.
        let picked_project = calendar::prebrief::clean_project(m.project);
        let (event, title) = (manifest.event.as_ref(), manifest.title.as_deref());
        manifest.project = match &picked_project {
            Some(p) => {
                if let Err(e) = calendar::prebrief::remember(&self.store, event, title, p).await {
                    tracing::warn!("meeting: start: project memory: {e:#}");
                }
                Some(p.clone())
            }
            None => {
                calendar::prebrief::lookup(&self.store, self.cfg.project.as_deref(), event, title)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("meeting: start: project memory: {e:#}");
                        None
                    })
            }
        };
        // 158 §11.1 (W4): the start's pick → remembered for the event / project → `meeting.template`.
        manifest.template = templates::select(
            &self.store,
            m.template.as_deref(),
            manifest.event.as_ref(),
            manifest.project.as_deref(),
            &self.cfg.template,
        )
        .await;
        // 137 R1-D6: the mode is the start option, else the configured default; without
        // the agent bridge installed the meeting runs as before and says so once.
        manifest.memory = self.endless.resolve(m.memory);
        if manifest.memory == agent::MemoryMode::Endless && endless::port().is_none() {
            tracing::warn!(
                meeting = %manifest.id,
                "meeting: endless memory requested but no agent bridge is installed; running in epochs"
            );
            manifest.memory = agent::MemoryMode::Epochs;
        }
        if disk_low(&self.rt.data_dir) {
            manifest.degraded.push(Degradation::DiskLow);
        }
        self.starting = Some(manifest.clone());
        self.publish_state(&manifest).await;

        let store = self.store.clone();
        let me = ctx.actor_ref().clone();
        let title = format!("Meeting {}", now.format("%Y-%m-%d %H:%M"));
        let setup = Setup {
            title,
            context_tokens: self.cfg.context_tokens,
            toolset: self.cfg.live_toolset(),
            view_bytes: self.endless.meeting_view_bytes,
            live_model_dir: self.cfg.live_model_dir.clone().unwrap_or_else(|| {
                self.rt
                    .data_dir
                    .join("models")
                    .join(voice::STREAMING_MODEL_SUBDIR)
            }),
            data_dir: self.rt.data_dir.clone(),
            diarization: self.cfg.live_diarization.then(|| self.paths().models),
        };
        tokio::spawn(async move {
            let begun = start_task(&store, manifest, &setup).await;
            let result = match &begun.result {
                Ok(_) => Ok(Started {
                    meeting: begun.manifest.id,
                    session: begun.manifest.session,
                }),
                Err(e) => Err(e.clone()),
            };
            // `ask`, so `current` is installed before the caller hears `Ok`.
            let _ = me.ask(begun).await;
            tx.send(result);
        });
        reply
    }
}

/// 158 §6.1 + 194 D4a/d: the meeting session's `(model, effort)`: the start's model with the
/// start's effort, else `meeting.model` with `meeting.effort` (a start effort without a model
/// is not read).
fn session_pick(
    model: Option<String>,
    effort: Option<String>,
    cfg: &MeetingConfig,
) -> (Option<String>, Option<String>) {
    match model {
        Some(model) => (Some(model), effort),
        None => (cfg.model.clone(), cfg.effort.clone()),
    }
}

/// What `start_task` needs from config.
struct Setup {
    title: String,
    context_tokens: u32,
    toolset: agent::Toolset,
    /// 137 R1-D6: `agent.endless.meeting_view_bytes`, the head section's budget.
    view_bytes: u32,
    live_model_dir: PathBuf,
    /// Locates the ONNX Runtime (`models::onnx::ort_lib`).
    data_dir: PathBuf,
    /// 07: the diarization models root when live diarization is on.
    diarization: Option<PathBuf>,
}

/// Session → directory + manifest → devices → STT sessions (01 §3, 02 §4, off
/// the mailbox). The session comes first because `Manifest::session` is not
/// optional; the STT sessions last because their failure only degrades.
async fn start_task(store: &Store, mut manifest: Manifest, setup: &Setup) -> Begun {
    let agents = match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(a)) => a,
        _ => return Begun::failed(manifest, "agent is not running".into()),
    };
    let open = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions {
                // 158 §9.2 W-c: the meeting's project, else the `meetings` pseudo-project.
                project: manifest.project.clone().or_else(|| Some("meetings".into())),
                model: manifest.session_model.clone(),
                effort: manifest.session_effort.clone(),
                // 137 R1-D6: never the chat default — the meeting's mode is set below with
                // the meeting filter, so the agent never starts an unfiltered tree here.
                memory: Some(agent::MemoryMode::Epochs),
                ..Default::default()
            },
            peer: None,
        })
        .await;
    let session = match open {
        Ok(open) => open.session,
        Err(e) => return Begun::failed(manifest, format!("create session: {e}")),
    };
    manifest.session = session;
    // 137 R1-D6: the summary tree over the meeting's units; a refusal degrades to epochs
    // (the head and the tools then stay as wave 81 built them).
    if manifest.memory == agent::MemoryMode::Endless {
        let enabled = match endless::port() {
            Some(port) => port.enable_meeting(session).await,
            None => Err("no agent bridge installed".into()),
        };
        if let Err(e) = enabled {
            tracing::warn!(%session, "meeting: endless memory off: {e}");
            manifest.memory = agent::MemoryMode::Epochs;
        }
    }
    let (toolset, earlier) = match manifest.memory {
        agent::MemoryMode::Endless => (
            endless::toolset(&setup.toolset),
            Some(head::Earlier {
                lines: Vec::new(),
                budget: setup.view_bytes,
            }),
        ),
        agent::MemoryMode::Epochs => (setup.toolset.clone(), None),
    };
    if let Err(e) = agents
        .ask(agent::RenameSession {
            session,
            title: setup.title.clone(),
        })
        .await
    {
        tracing::warn!(%session, "rename meeting session: {e}");
    }
    // 06 §1.7: the meeting named its session; the titler stays out until stop.
    if let Err(e) = agents
        .ask(agent::PinTitle {
            session,
            pinned: true,
        })
        .await
    {
        tracing::warn!(%session, "pin meeting session title: {e}");
    }
    // 02 §4, §4.1: meeting-sized compaction with the meeting hint and the initial state
    // head (nothing heard yet); re-pushed on every change, cleared at stop.
    if let Err(e) = agents
        .ask(agent::SetCompaction {
            session,
            at_tokens: Some(setup.context_tokens),
            hint: Some(tools::COMPACTION_HINT.to_owned()),
            head: Some(head::render(&head::initial(
                &manifest,
                &setup.title,
                earlier,
            ))),
        })
        .await
    {
        tracing::warn!(%session, "meeting session compaction: {e}");
    }
    // 06 §1.6: the lean, direct-call toolset while recording; cleared at stop.
    if let Err(e) = agents
        .ask(agent::SetToolset {
            session,
            toolset: Some(toolset),
        })
        .await
    {
        tracing::warn!(%session, "meeting session toolset: {e}");
    }
    let dir = match store.create(&manifest).await {
        Ok(dir) => dir,
        Err(e) => return Begun::failed(manifest, format!("{e:#}")),
    };
    // 59 §7.4: one note per meeting, from the first second; `None` when documents are off — or refused
    // (the helper missing), which the user must see: the meeting goes on without a note or a summary note.
    // The wait is bounded (`note::CREATE_WAIT`): a slow helper degrades the start, never hangs it; the
    // note is attached later if it arrives (`NoteReady`).
    let mut late_note = None;
    match note::create(
        manifest.id,
        manifest.title.as_deref().unwrap_or(&setup.title),
    )
    .await
    {
        note::Created::Now(n) => manifest.note = n,
        note::Created::Late(task) => late_note = Some(task),
    }
    if manifest.note.is_none() && document::Handle::lookup().is_some() {
        manifest.degraded.push(Degradation::NoNote);
    }
    let process = manifest.process.clone();
    let id = manifest.id;
    let file_title = setup.title.clone();
    let opened = tokio::task::spawn_blocking(move || {
        capture::Capture::start(&process, id, &dir, &file_title)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r.map_err(|e| format!("{e:#}")));
    let capture = match opened {
        Ok((capture, degraded)) => {
            for d in degraded {
                if !manifest.degraded.contains(&d) {
                    manifest.degraded.push(d);
                }
            }
            capture
        }
        Err(e) => return Begun::failed(manifest, e),
    };
    // Two streaming sessions, one per track (02 §2); ≈ 0.9 GB each, int8.
    let (model_dir, data_dir) = (setup.live_model_dir.clone(), setup.data_dir.clone());
    let t = Instant::now();
    let engines = tokio::task::spawn_blocking(move || -> anyhow::Result<(Utterer, Utterer)> {
        Ok((
            Utterer::open(&model_dir, &data_dir)?,
            Utterer::open(&model_dir, &data_dir)?,
        ))
    })
    .await
    .map_err(|e| anyhow::anyhow!("STT load task panicked: {e}"))
    .and_then(|r| r);
    let engines = match engines {
        Ok(e) => {
            tracing::info!(meeting = %manifest.id, "meeting: two streaming STT sessions loaded in {:?}", t.elapsed());
            Some(e)
        }
        Err(e) => {
            tracing::warn!(meeting = %manifest.id, "meeting: live STT unavailable: {e:#}");
            for d in [
                Degradation::SttDown(Track::Mic),
                Degradation::SttDown(Track::System),
            ] {
                if !manifest.degraded.contains(&d) {
                    manifest.degraded.push(d);
                }
            }
            None
        }
    };
    // 07 §2: the live diariser (≈ 300 MB). Missing models are not a degradation (labels
    // are simply absent, as with the switch off); a failed load with the switch on is.
    let diar = match setup
        .diarization
        .as_ref()
        .and_then(|dir| diarize::models(dir, &setup.data_dir))
    {
        Some(models) => {
            let t = Instant::now();
            match tokio::task::spawn_blocking(move || diar::open(&models)).await {
                Ok(Ok(s)) => {
                    tracing::info!(meeting = %manifest.id, "meeting: live diarization session loaded in {:?}", t.elapsed());
                    Some(s)
                }
                Ok(Err(e)) => {
                    tracing::warn!(meeting = %manifest.id, "meeting: live diarization unavailable: {e:#}");
                    manifest.degraded.push(Degradation::DiarizationDown);
                    None
                }
                Err(e) => {
                    tracing::warn!(meeting = %manifest.id, "meeting: live diarization load panicked: {e}");
                    manifest.degraded.push(Degradation::DiarizationDown);
                    None
                }
            }
        }
        None => {
            if setup.diarization.is_some() {
                tracing::info!(meeting = %manifest.id, "meeting: live diarization off: models missing");
            }
            None
        }
    };
    Begun {
        manifest,
        result: Ok(capture),
        engines,
        diar,
        late_note,
    }
}

impl Begun {
    fn failed(mut manifest: Manifest, reason: String) -> Self {
        let lower = reason.to_ascii_lowercase();
        if lower.contains("permission denied") {
            let which = if lower.contains("microphone") {
                Permission::Microphone
            } else {
                Permission::AudioCapture
            };
            manifest.degraded.push(Degradation::PermissionDenied(which));
        }
        manifest.state = MeetingState::Failed;
        manifest.failure = Some(reason.clone());
        Begun {
            manifest,
            result: Err(reason),
            engines: None,
            diar: None,
            late_note: None,
        }
    }
}

impl Message<Begun> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: Begun, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.starting = None;
        let mut manifest = m.manifest;
        match m.result {
            Err(_) => {
                if manifest
                    .degraded
                    .iter()
                    .any(|d| matches!(d, Degradation::PermissionDenied(Permission::AudioCapture)))
                {
                    self.audio_capture = PermissionState::Denied;
                }
                // 06 §1.6: a session created before the failure goes on as an ordinary one.
                if !manifest.session.0.is_nil()
                    && let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    && let Err(e) = agents
                        .ask(agent::SetToolset {
                            session: manifest.session,
                            toolset: None,
                        })
                        .await
                {
                    tracing::warn!(meeting = %manifest.id, "meeting: clear toolset: {e}");
                }
                // 06 §1.7: and titles itself like one.
                if !manifest.session.0.is_nil()
                    && let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    && let Err(e) = agents
                        .ask(agent::PinTitle {
                            session: manifest.session,
                            pinned: false,
                        })
                        .await
                {
                    tracing::warn!(meeting = %manifest.id, "meeting: unpin title: {e}");
                }
                // The directory exists only if the session did; `save` creates
                // the row either way so the failure is listed.
                if self.store.meeting_dir(manifest.id).exists() {
                    self.save(&manifest).await;
                } else {
                    self.publish_state(&manifest).await;
                }
                self.detect_ended();
            }
            Ok(capture) => {
                self.audio_capture = PermissionState::Granted;
                manifest.state = MeetingState::Recording;
                let id = manifest.id;
                self.detect_recording(id, &manifest.process);
                let me = ctx.actor_ref().clone();
                let dir = self.store.meeting_dir(id);
                let mut rec = Recording::new(manifest, capture, &self.cfg, &dir);
                // 137 R1-D6: the section exists from the first head; the view fills it.
                if rec.manifest.memory == agent::MemoryMode::Endless {
                    rec.live.earlier = Some(head::Earlier {
                        lines: Vec::new(),
                        budget: self.endless.meeting_view_bytes,
                    });
                }
                self.spawn_tasks(id, &mut rec, me.clone());
                if let Some((mic, system)) = m.engines
                    && let Some((mic_ring, system_ring)) = rec.capture.take_stt()
                {
                    let flags = |held| stt::Flags {
                        stop: rec.capture.stop_flag(),
                        paused: rec.capture.paused_flag(),
                        held,
                    };
                    rec.live.stt.push(stt::spawn(
                        Track::Mic,
                        mic_ring,
                        mic,
                        flags(Some(rec.live.held_flag.clone())),
                        me.clone(),
                    ));
                    rec.live.stt.push(stt::spawn(
                        Track::System,
                        system_ring,
                        system,
                        flags(None),
                        me.clone(),
                    ));
                }
                // 07 §2: the live diariser on the system feed's own ring.
                if let Some(streamer) = m.diar
                    && let Some(ring) = rec.capture.take_diar()
                {
                    rec.live.diar = Some(diar::spawn(
                        ring,
                        streamer,
                        diar::Flags {
                            stop: rec.capture.stop_flag(),
                            paused: rec.capture.paused_flag(),
                        },
                        me.clone(),
                    ));
                }
                tools::register(me).await;
                // 57 §3.5 item 5: load the oracle now so batch 1 never pays the cold load.
                tokio::spawn(oracle::warm_up());
                self.rt
                    .publish(MeetingEvent::Started {
                        meeting: id,
                        session: rec.manifest.session,
                        process: rec.manifest.process.clone(),
                        at: rec.manifest.started_at.clone(),
                    })
                    .await;
                manifest = rec.manifest.clone();
                let started = session::started_line(rec.manifest.context.as_deref());
                // 05 §3: the start form's title + context are scanned before the
                // `started.` line reaches the model.
                let seed = format!(
                    "{} {}",
                    rec.manifest.title.as_deref().unwrap_or(""),
                    rec.manifest.context.as_deref().unwrap_or("")
                );
                self.current = Some(rec);
                self.save(&manifest).await;
                if let Some(task) = m.late_note {
                    let me = ctx.actor_ref().clone();
                    tokio::spawn(async move {
                        if let Ok(Some(doc)) = task.await {
                            let _ = me.tell(NoteReady { meeting: id, doc }).await;
                        }
                    });
                }
                let hits = self.scan(&seed);
                self.cached_topics(hits, 0, Instant::now()).await;
                self.deliver(started).await;
            }
        }
    }
}

/// 173: fold one watchdog report into `manifest` (capture gaps, degradations) and `status`;
/// `(manifest changed, status changed)`.
fn apply_report(
    manifest: &mut Manifest,
    clock: &RecordingClock,
    status: &mut CaptureStatus,
    r: capture::Report,
) -> (bool, bool) {
    use capture::watchdog::Health;
    /// `at` on the wall clock, RFC 3339.
    fn wall(at: Instant) -> String {
        let ago = chrono::Duration::from_std(Instant::now().saturating_duration_since(at))
            .unwrap_or_default();
        (chrono::Local::now() - ago).to_rfc3339()
    }
    match r {
        capture::Report::Gap {
            track,
            start,
            end,
            reason,
            retarget,
        } => {
            let at_ms = clock.elapsed_at(start).as_millis() as u64;
            let dur_ms = (clock.elapsed_at(end).as_millis() as u64).saturating_sub(at_ms);
            tracing::warn!(meeting = %manifest.id, ?track, at_ms, dur_ms, ?reason, ?retarget, "meeting capture gap");
            manifest.gaps.push(Gap {
                at_ms,
                dur_ms,
                track: Some(match track {
                    Track::Mic => Stem::Mic,
                    Track::System => Stem::System,
                }),
                reason: Some(reason),
                retarget,
            });
            (true, false)
        }
        capture::Report::Degraded(d) => {
            if manifest.degraded.contains(&d) {
                return (false, false);
            }
            manifest.degraded.push(d);
            (true, false)
        }
        capture::Report::Fallback { start, reason } => {
            let at_ms = clock.elapsed_at(start).as_millis() as u64;
            tracing::warn!(meeting = %manifest.id, at_ms, ?reason, "meeting capture: system track on the global tap");
            manifest.degraded.push(Degradation::SystemAudioFallback {
                reason,
                at_ms,
                until_ms: None,
            });
            (true, false)
        }
        capture::Report::Returned { end, pid } => {
            let end_ms = clock.elapsed_at(end).as_millis() as u64;
            tracing::warn!(meeting = %manifest.id, end_ms, pid, "meeting capture: system track back on the app's tap");
            // The open fallback entry gets its end.
            let open = manifest.degraded.iter_mut().rev().find_map(|d| match d {
                Degradation::SystemAudioFallback { until_ms, .. } if until_ms.is_none() => {
                    Some(until_ms)
                }
                _ => None,
            });
            match open {
                Some(until) => {
                    *until = Some(end_ms);
                    (true, false)
                }
                None => (false, false),
            }
        }
        capture::Report::Health { track, health } => {
            let h = match health {
                Health::Ok => CaptureHealth::Ok,
                Health::Recovering(reason, at) => CaptureHealth::Recovering {
                    reason,
                    since: wall(at),
                },
                Health::Lost(reason, at) => CaptureHealth::Lost {
                    reason,
                    since: wall(at),
                },
                Health::Degraded(reason) => CaptureHealth::Degraded { reason },
            };
            tracing::warn!(meeting = %manifest.id, ?track, health = ?h, "meeting capture health changed");
            match track {
                Track::Mic => status.mic = h,
                Track::System => status.system = h,
            }
            (false, true)
        }
    }
}

impl Message<Degrade> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: Degrade, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if m.0 == Degradation::DiarizationDown && rec.live.diar.take().is_some() {
            // 07 §6: nothing more will be labelled; what is held goes out as it is.
            let me = ctx.actor_ref().clone();
            self.release_held(&me, true).await;
        }
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if rec.manifest.degraded.contains(&m.0) {
            return;
        }
        rec.manifest.degraded.push(m.0);
        let manifest = rec.manifest.clone();
        self.save(&manifest).await;
    }
}

impl Message<NoteReady> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: NoteReady, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(rec) = self
            .current
            .as_mut()
            .filter(|r| r.manifest.id == m.meeting && r.manifest.note.is_none())
        else {
            tracing::warn!(meeting = %m.meeting, doc = %m.doc, "late meeting note: meeting no longer current; left unattached");
            return;
        };
        rec.manifest.note = Some(m.doc);
        rec.manifest.degraded.retain(|d| *d != Degradation::NoNote);
        let manifest = rec.manifest.clone();
        self.save(&manifest).await;
    }
}

impl Message<PauseMeeting> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: PauseMeeting, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let state = self.state();
        let Some(rec) = self
            .current
            .as_mut()
            .filter(|r| r.manifest.state == MeetingState::Recording)
        else {
            return Err(format!("{state:?}"));
        };
        rec.clock.pause();
        rec.capture.pause();
        rec.manifest.state = MeetingState::Paused;
        let manifest = rec.manifest.clone();
        self.save(&manifest).await;
        Ok(())
    }
}

impl Message<ResumeMeeting> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        _: ResumeMeeting,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let state = self.state();
        let Some(rec) = self
            .current
            .as_mut()
            .filter(|r| r.manifest.state == MeetingState::Paused)
        else {
            return Err(format!("{state:?}"));
        };
        if let Some(gap) = rec.clock.resume() {
            rec.manifest.gaps.push(gap);
        }
        rec.capture.resume();
        rec.manifest.state = MeetingState::Recording;
        let manifest = rec.manifest.clone();
        self.save(&manifest).await;
        Ok(())
    }
}

impl Message<StopMeeting> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: StopMeeting, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let state = self.state();
        let Some(mut rec) = self.current.take_if(|r| {
            matches!(
                r.manifest.state,
                MeetingState::Recording | MeetingState::Paused
            )
        }) else {
            return Err(format!("{state:?}"));
        };
        if let Some(gap) = rec.clock.resume() {
            rec.manifest.gaps.push(gap);
        }
        rec.manifest.state = MeetingState::Stopping;
        self.save(&rec.manifest).await;
        let Recording {
            mut manifest,
            clock,
            capture,
            tasks,
            mut live,
        } = rec;
        for t in tasks {
            t.abort();
        }
        // Joins the three threads; the encoder writes the EOS page. The STT
        // tasks see the same stop flag and flush their open utterances.
        let reports = match tokio::task::spawn_blocking(move || capture.stop()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(meeting = %manifest.id, "capture stop: {e}");
                Vec::new()
            }
        };
        // 173: the episodes the stop closed (and any report the last tick missed).
        for r in reports {
            apply_report(&mut manifest, &clock, &mut live.capture, r);
        }
        // 07 §3: the diariser flushes its tail (no look-ahead) and returns its final
        // view; the held system lines and the STT flush are labelled against it.
        if let Some(task) = live.diar.take()
            && let Ok(Some(labels)) = task.await
        {
            live.labels = labels.view(clock.elapsed_at(labels.at).as_millis() as u64);
        }
        let mut finals: Vec<Utterance> = Vec::new();
        let tasks: Vec<_> = std::mem::take(&mut live.stt);
        for task in tasks {
            let Ok(pieces) = task.await else { continue };
            for p in pieces
                .into_iter()
                .filter(|p| p.is_final && said_something(&p.text))
            {
                let (t0_ms, t1_ms) = times(&clock, &p);
                finals.push(Utterance {
                    track: p.track,
                    t0_ms,
                    t1_ms,
                    text: p.text,
                    request: p.request,
                    speaker: None,
                });
            }
        }
        let held: Vec<Utterance> = std::mem::take(&mut live.held_system);
        // 02 §4: the engines are gone (the tasks owned them); pending lines go
        // out as one last input, then the session compacts like any other. 57 §3.2:
        // batches the oracle kept back ride along as context, unfiltered at stop.
        let mut pending = live.batcher.drain();
        for mut u in held.into_iter().chain(finals) {
            if u.track == Track::System && u.speaker.is_none() {
                u.speaker = diar::speaker_of(u.t0_ms, u.t1_ms, &live.labels.spans);
            }
            live.log_utterance(&u);
            self.rt
                .publish(MeetingEvent::Utterance {
                    meeting: manifest.id,
                    track: u.track,
                    t0_ms: u.t0_ms,
                    t1_ms: u.t1_ms,
                    text: u.text.clone(),
                    request: u.request,
                    r#final: true,
                    speaker: u.speaker.clone(),
                })
                .await;
            pending.push(u);
        }
        // Held system lines predate the mic lines already pending.
        pending.sort_by_key(|u| u.t0_ms);
        if !pending.is_empty() || !live.carry.is_empty() {
            let carried = std::mem::take(&mut live.carry);
            let (pending, carried) = if pending.is_empty() {
                let mut c = carried;
                let last = c.pop().map(|(_, b)| b).unwrap_or_default();
                (last, c)
            } else {
                (pending, carried)
            };
            let text = session::batch_line_with(
                &pending,
                &live.topics.batch_labels(chrono::Utc::now()),
                live.topics.dismissed_labels(),
                &carried,
                None,
            );
            if let Err(e) = session::deliver(manifest.session, text, live.turn_running).await {
                tracing::warn!(meeting = %manifest.id, "meeting: last batch: {e}");
            }
        }
        // 02 §4.1: trigger, hint and head all go; the session compacts like any other.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Err(e) = agents
                .ask(agent::SetCompaction {
                    session: manifest.session,
                    at_tokens: None,
                    hint: None,
                    head: None,
                })
                .await
        {
            tracing::warn!(meeting = %manifest.id, "meeting: clear compaction: {e}");
        }
        // 06 §1.6: the session is an ordinary one from here.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Err(e) = agents
                .ask(agent::SetToolset {
                    session: manifest.session,
                    toolset: None,
                })
                .await
        {
            tracing::warn!(meeting = %manifest.id, "meeting: clear toolset: {e}");
        }
        // 06 §1.7: post-meeting turns title the session like any other.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Err(e) = agents
                .ask(agent::PinTitle {
                    session: manifest.session,
                    pinned: false,
                })
                .await
        {
            tracing::warn!(meeting = %manifest.id, "meeting: unpin title: {e}");
        }
        tools::unregister().await;
        // 57 §2.1: the meeting checkpoint is resident only while recording.
        oracle::release().await;
        self.detect_ended();
        manifest.ended_at = Some(chrono::Local::now().to_rfc3339());
        manifest.duration_ms = Some(clock.now_ms());
        // 04 §2: the post pipeline takes it from here (its task saves `Post(_)`,
        // `Review` or `Failed` and publishes them); `Stopping` is what we leave on disk.
        self.save(&manifest).await;
        if let Err(e) = self.start_post(manifest) {
            tracing::warn!("meeting: post: {e}");
        }
        Ok(())
    }
}

impl Message<MeetingStatus> for Meeting {
    type Reply = Status;

    async fn handle(
        &mut self,
        _: MeetingStatus,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let m = self
            .current
            .as_ref()
            .map(|r| (&r.manifest, r.clock.now_ms()))
            .or_else(|| self.starting.as_ref().map(|m| (m, 0)));
        let live = self.current.as_ref().map(|r| &r.live);
        let supported = tokio::task::spawn_blocking(capture::system::supported)
            .await
            .unwrap_or_else(|e| Support::None(format!("probe: {e}")));
        Status {
            state: self.state(),
            meeting: m.map(|(m, _)| m.id),
            session: m.map(|(m, _)| m.session),
            elapsed_ms: m.map_or(0, |(_, ms)| ms),
            degraded: m.map(|(m, _)| m.degraded.clone()).unwrap_or_default(),
            cursor: live.and_then(|l| l.cursor),
            topics: live.map(|l| l.topics.live()).unwrap_or_default(),
            approval_waiting: live.is_some_and(|l| !l.approvals.is_empty()),
            supported,
            accelerator: models::onnx::ep().to_owned(),
            detection: self.detect.engine.prompt().cloned(),
            capture: live.map(|l| l.capture.clone()).unwrap_or_default(),
        }
    }
}

impl Message<ListMeetings> for Meeting {
    type Reply = Result<Vec<MeetingSummary>, String>;

    async fn handle(&mut self, m: ListMeetings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .list_in(m.limit.unwrap_or(100), m.project.as_deref())
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<GetMeeting> for Meeting {
    type Reply = Result<Manifest, String>;

    async fn handle(&mut self, m: GetMeeting, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.manifest(m.meeting).map_err(|e| format!("{e:#}"))
    }
}

impl Message<DeleteMeeting> for Meeting {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(
        &mut self,
        m: DeleteMeeting,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let live = self
            .current
            .as_ref()
            .map(|r| r.manifest.id)
            .or_else(|| self.starting.as_ref().map(|m| m.id));
        if live == Some(m.meeting) {
            tx.send(Err("that meeting is recording; stop it first".into()));
            return reply;
        }
        let (session, note) = match self.store.manifest(m.meeting) {
            Ok(manifest) => (Some(manifest.session), manifest.note),
            Err(_) => (None, None),
        };
        // A pipeline still working on it would only fail on the missing directory.
        self.post.retain(|j| {
            if j.meeting == m.meeting {
                j.handle.abort();
            }
            j.meeting != m.meeting
        });
        if let Err(e) = self.store.delete(m.meeting).await {
            tx.send(Err(format!("{e:#}")));
            return reply;
        }
        let rt = self.rt.clone();
        tokio::spawn(async move {
            if let Some(session) = session
                && let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                && let Err(e) = agents.ask(agent::DeleteSession { session }).await
            {
                tracing::warn!(%session, "delete meeting session: {e}");
            }
            // 59 §7.4: the note is the meeting's; without this it would linger in `/notes` as an orphan.
            if let Some(doc) = note
                && let Err(e) = note::delete(&doc).await
            {
                tracing::warn!(%doc, "delete meeting note: {e}");
            }
            rt.publish(MeetingEvent::Deleted { meeting: m.meeting })
                .await;
            tx.send(Ok(()));
        });
        reply
    }
}

impl Message<CheckPermissions> for Meeting {
    type Reply = Permissions;

    async fn handle(
        &mut self,
        _: CheckPermissions,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Permissions {
            microphone: capture::mic::permission(),
            audio_capture: self.audio_capture,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2 handlers (02 §2, §5, §6)

impl Message<Piece> for Meeting {
    type Reply = ();

    async fn handle(&mut self, p: Piece, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let meeting = rec.manifest.id;
        let (t0_ms, t1_ms) = times(&rec.clock, &p);
        rec.live.topics.touch(&p.text, Instant::now());
        if p.is_final && !said_something(&p.text) {
            return;
        }
        self.rt
            .publish(MeetingEvent::Utterance {
                meeting,
                track: p.track,
                t0_ms,
                t1_ms,
                text: p.text.clone(),
                request: p.request,
                r#final: p.is_final,
                speaker: None,
            })
            .await;
        if p.is_final {
            self.closed(
                Utterance {
                    track: p.track,
                    t0_ms,
                    t1_ms,
                    text: p.text,
                    request: p.request,
                    speaker: None,
                },
                p.at,
                ctx.actor_ref(),
            )
            .await;
        }
    }
}

impl Message<diar::Labels> for Meeting {
    type Reply = ();

    /// 07 §3: the diariser's view replaces the last one; whatever it now covers is released.
    async fn handle(
        &mut self,
        m: diar::Labels,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let at_ms = rec.clock.elapsed_at(m.at).as_millis() as u64;
        rec.live.labels = m.view(at_ms);
        let me = ctx.actor_ref().clone();
        self.release_held(&me, false).await;
    }
}

impl Message<Tick> for Meeting {
    type Reply = ();

    async fn handle(&mut self, _: Tick, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let now = Instant::now();
        // 173: the capture watchdogs' reports → manifest + `Capture` event.
        let (mut saved, mut changed) = (false, false);
        while let Some(r) = rec.capture.try_report() {
            let (s, c) = apply_report(&mut rec.manifest, &rec.clock, &mut rec.live.capture, r);
            saved |= s;
            changed |= c;
        }
        if saved {
            let manifest = rec.manifest.clone();
            self.save(&manifest).await;
        }
        if changed && let Some(rec) = self.current.as_ref() {
            self.rt
                .publish(MeetingEvent::Capture {
                    meeting: rec.manifest.id,
                    capture: rec.live.capture.clone(),
                })
                .await;
        }
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let batch = rec.live.batcher.tick(now);
        let gone = rec.live.topics.expire(now);
        if let Some(batch) = batch {
            self.send_batch(batch, ctx.actor_ref()).await;
        }
        self.topics_gone(gone).await;
    }
}

impl Message<oracle::Verdict> for Meeting {
    type Reply = ();

    /// 57 §3.2: act on the oracle's verdict — skip (carry), publish the cached brief as an
    /// `Oracle` topic (carry), or deliver today's batch line with the evidence.
    async fn handle(
        &mut self,
        v: oracle::Verdict,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        let pending = match (v.event, v.expected) {
            (Some(e), Some(x)) => Some((e, x, v.example)),
            _ => None,
        };
        match v.action {
            oracle::Action::Skip => self.carry(v.batch),
            oracle::Action::Publish { row, label } => {
                let at_ms = rec.clock.now_ms();
                match rec.live.topics.insert_cached(
                    &label,
                    &row,
                    BriefSource::Oracle,
                    Instant::now(),
                    at_ms,
                ) {
                    Ok((topic, brief, expired)) => {
                        rec.live.log_record(&Record::Topic(topic.clone()));
                        rec.live.log_record(&Record::Brief(brief));
                        // 57 §11.4: a tap or dismiss on this topic amends the example.
                        if let Some(id) = v.example {
                            rec.live.captured.push((topic.id, id));
                        }
                        self.topics_gone(expired).await;
                        self.topic_added(topic).await;
                        self.touch_use(row.id);
                    }
                    // A live/dismissed label or a row shown meanwhile: no turn spent either way.
                    Err(e) => tracing::debug!(label, "meeting: oracle topic skipped: {e}"),
                }
                self.carry(v.batch);
            }
            oracle::Action::Model { evidence } => {
                self.deliver_batch(v.batch, evidence, pending).await
            }
        }
    }
}

impl Message<AgentEvent> for Meeting {
    type Reply = ();

    /// 02 §0.4: turn state and model health of the meeting session. 04 §6: the
    /// form's `InputInjected` binds a summary to the turn it opened; that turn's
    /// `TurnFinished` hands the finished meeting to `summary::spawn`; a withdrawn
    /// form releases the slot.
    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.summarizing.is_empty() {
            match &ev {
                AgentEvent::InputInjected { id, turn, .. } => {
                    if let Some(s) = self.summarizing.values_mut().find(|s| s.input == *id) {
                        s.turn = Some(*turn);
                        return;
                    }
                }
                AgentEvent::InputWithdrawn { id, .. } => {
                    if let Some(s) = self.take_summarizing(|s| s.input == *id) {
                        self.summary_failed(s.manifest, "the form was withdrawn before its turn")
                            .await;
                        return;
                    }
                }
                AgentEvent::TurnFinished {
                    turn, stop_reason, ..
                } => {
                    if let Some(s) = self.take_summarizing(|s| s.turn == Some(*turn)) {
                        let error = (stop_reason == "error" || stop_reason == "cancelled")
                            .then(|| format!("the summary turn ended with {stop_reason}"));
                        summary::spawn(self.rt.clone(), self.store.clone(), s.manifest, error);
                        return;
                    }
                }
                _ => {}
            }
        }
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        match ev {
            AgentEvent::TurnStarted { session, .. } if session == rec.manifest.session => {
                rec.live.turn_running = true;
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } if session == rec.manifest.session => {
                rec.live.turn_running = false;
                let down = rec.manifest.degraded.contains(&Degradation::ModelDown);
                let errored = stop_reason == "error";
                // 03 §9: a request or deeper the turn did not answer with a brief gets one reminder.
                let nudge = (!errored).then(|| rec.live.unanswered()).flatten();
                // 57 §3.4: what the turn did versus what the oracle said (an errored turn is
                // no label: keep the verdicts pending for the retry).
                if !errored {
                    self.settle_pending().await;
                }
                let Some(rec) = self.current.as_mut() else {
                    return;
                };
                if errored && !down {
                    rec.manifest.degraded.push(Degradation::ModelDown);
                } else if !errored && down {
                    rec.manifest
                        .degraded
                        .retain(|d| *d != Degradation::ModelDown);
                } else {
                    if let Some((p, text)) = nudge {
                        self.deliver(session::nudge_line(p, &text)).await;
                    }
                    return;
                }
                let manifest = rec.manifest.clone();
                self.save(&manifest).await;
                if let Some((p, text)) = nudge {
                    self.deliver(session::nudge_line(p, &text)).await;
                }
            }
            _ => {}
        }
    }
}

impl Message<ApprovalRequested> for Meeting {
    type Reply = ();

    async fn handle(&mut self, a: ApprovalRequested, _: &mut Context<Self, Self::Reply>) {
        let Some(rec) = self
            .current
            .as_mut()
            .filter(|r| r.manifest.session == a.session)
        else {
            return;
        };
        let before = !rec.live.approvals.is_empty();
        rec.live.approvals.insert(a.id);
        let meeting = rec.manifest.id;
        self.approval_waiting(meeting, before, true).await;
    }
}

impl Message<ApprovalResolved> for Meeting {
    type Reply = ();

    async fn handle(&mut self, a: ApprovalResolved, _: &mut Context<Self, Self::Reply>) {
        let Some(rec) = self.current.as_mut() else {
            return;
        };
        if !rec.live.approvals.remove(&a.id) {
            return;
        }
        let after = !rec.live.approvals.is_empty();
        let meeting = rec.manifest.id;
        self.approval_waiting(meeting, true, after).await;
    }
}

impl Message<TapTopic> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: TapTopic, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.current.as_mut().ok_or("no meeting is recording")?;
        let topic = rec.live.topics.view(m.topic).ok_or("no such live topic")?;
        // Decision 11: the precomputed brief is shown on the tap itself; a
        // `meeting_brief` from the model, if any, is a second one.
        let brief = topic.brief.and_then(|b| rec.live.topics.brief(b).cloned());
        rec.live
            .prompted(Prompted::Tap(topic.id), topic.label.clone());
        rec.live.react(topic.id, Reaction::Tapped);
        self.topic_gone(topic.id, TopicGone::Viewed).await;
        if let Some(mut brief) = brief {
            brief.source = BriefSource::Tap;
            let Some(rec) = self.current.as_mut() else {
                return Ok(());
            };
            let meeting = rec.manifest.id;
            self.rt
                .publish(MeetingEvent::Brief { meeting, brief })
                .await;
        }
        self.deliver(session::tapped_line(&topic.label)).await;
        Ok(())
    }
}

impl Message<DismissTopic> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: DismissTopic, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.current.as_mut().ok_or("no meeting is recording")?;
        let topic = rec
            .live
            .topics
            .dismiss(m.topic)
            .ok_or("no such live topic")?;
        rec.live.react(topic.id, Reaction::Dismissed);
        self.topic_gone(topic.id, TopicGone::Dismissed).await;
        self.deliver(session::dismissed_line(&topic.label)).await;
        Ok(())
    }
}

impl Message<Deeper> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Deeper, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.current.as_mut().ok_or("no meeting is recording")?;
        let brief = rec
            .live
            .topics
            .brief(m.brief)
            .cloned()
            .ok_or("no such brief")?;
        rec.live
            .prompted(Prompted::Deeper(brief.id), brief.title.clone());
        self.deliver(session::deeper_line(brief.id, &brief.title, &brief.bullets))
            .await;
        Ok(())
    }
}

impl Message<TypedRequest> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: TypedRequest, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.current.as_ref().ok_or("no meeting is recording")?;
        if m.text.trim().is_empty() {
            return Err("empty request".into());
        }
        let hits = self.scan(&m.text);
        let cached = self.cached_request(&hits).await;
        let Some(rec) = self.current.as_mut() else {
            return Ok(());
        };
        rec.live
            .prompted(Prompted::Request { spoken: false }, m.text.clone());
        self.deliver(session::request_line(false, &m.text, cached.as_deref()))
            .await;
        Ok(())
    }
}

impl Message<PushToTalk> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PushToTalk, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.current.as_mut().ok_or("no meeting is recording")?;
        if rec.live.held == m.held {
            return Ok(());
        }
        rec.live.held = m.held;
        // The mic STT task holds its finals while held and, on release, closes the
        // open utterance and reports the whole span as one request (53 decision 10).
        rec.live.held_flag.store(m.held, Ordering::SeqCst);
        let meeting = rec.manifest.id;
        self.rt
            .publish(MeetingEvent::Listening {
                meeting,
                held: m.held,
            })
            .await;
        Ok(())
    }
}

impl Message<SetCursor> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetCursor, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.current.as_mut().ok_or("no meeting is recording")?;
        rec.live.cursor = m.brief;
        Ok(())
    }
}

impl Message<ListBriefs> for Meeting {
    type Reply = Result<Vec<Brief>, String>;

    async fn handle(&mut self, m: ListBriefs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(rec) = self.current.as_ref().filter(|r| r.manifest.id == m.meeting) {
            return Ok(rec.live.topics.briefs().to_vec());
        }
        let dir = self.store.meeting_dir(m.meeting);
        if !dir.is_dir() {
            return Err(format!("no meeting {}", m.meeting));
        }
        let records: Vec<Record> = read_jsonl(&dir.join(BRIEFS_LOG), usize::MAX)?;
        Ok(records
            .into_iter()
            .filter_map(|r| match r {
                Record::Brief(b) => Some(b),
                _ => None,
            })
            .collect())
    }
}

impl Message<ListTopics> for Meeting {
    type Reply = Vec<Topic>;

    async fn handle(&mut self, _: ListTopics, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.current
            .as_ref()
            .map(|r| r.live.topics.live())
            .unwrap_or_default()
    }
}

impl Message<LiveText> for Meeting {
    type Reply = Result<Vec<Utterance>, String>;

    async fn handle(&mut self, m: LiveText, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let dir = self.store.meeting_dir(m.meeting);
        if !dir.is_dir() {
            return Err(format!("no meeting {}", m.meeting));
        }
        read_jsonl(&dir.join(LIVE_LOG), LIVE_TEXT_CAP).map(|mut v: Vec<Utterance>| {
            // 07 §3: held system lines are appended after later mic lines.
            v.sort_by_key(|u| u.t0_ms);
            v
        })
    }
}

impl Message<AddTopic> for Meeting {
    type Reply = Result<String, String>;

    /// 05 §4: by reference (`brief`) → the row's brief, the label learnt as an
    /// alias; else the model's brief, written to the cache first so the row id
    /// rides on the topic and the alias scan will not re-add it this meeting.
    async fn handle(&mut self, m: AddTopic, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.recording_for(m.session)?;
        let meeting = rec.manifest.id;
        let at_ms = rec.clock.now_ms();
        rec.live.topics.admit(&m.label)?;
        let (topic, brief, expired, learnt) = if let Some(id) = m.brief {
            let row = self.cache()?.get(id).await?;
            let rec = self.recording_for(m.session)?;
            let (t, b, e) = rec.live.topics.insert_cached(
                &m.label,
                &row,
                BriefSource::Cache,
                Instant::now(),
                at_ms,
            )?;
            (t, b, e, Some(id))
        } else {
            let (Some(title), Some(bullets)) = (m.title, m.bullets) else {
                return Err("give `title` and `bullets`, or `brief` (a cached brief id)".into());
            };
            topics::check_bullets(&bullets)?;
            // 05 §0.3: cached the moment it is shown. A cache failure is not the model's problem.
            let cached = match self
                .cache_put(
                    CachedBriefPut {
                        id: None,
                        title: title.clone(),
                        bullets: bullets.clone(),
                        aliases: vec![m.label.clone()],
                        tags: Vec::new(),
                    },
                    &format!("meeting:{meeting}"),
                )
                .await
            {
                Ok((row, _)) => Some(row.id),
                Err(e) => {
                    tracing::debug!("meeting: brief not cached: {e}");
                    None
                }
            };
            let rec = self.recording_for(m.session)?;
            let (t, b, e) =
                rec.live
                    .topics
                    .insert(&m.label, &title, bullets, cached, Instant::now(), at_ms)?;
            (t, b, e, None)
        };
        let rec = self.recording_for(m.session)?;
        rec.live.log_record(&Record::Topic(topic.clone()));
        // 57 §3.4: the shadow outcome compares this with what the oracle said. `learnt` is the
        // row the model named; a new brief's own cache row is not a cached pick.
        rec.live.published.push((learnt, brief.title.clone()));
        rec.live.published_topics.push(topic.id);
        rec.live.log_record(&Record::Brief(brief));
        self.topics_gone(expired).await;
        self.topic_added(topic).await;
        if let Some(id) = learnt {
            self.touch_use(id);
            if let Some(c) = self.cache.clone() {
                let label = m.label.clone();
                tokio::spawn(async move {
                    if let Err(e) = c.add_alias(id, &label).await {
                        tracing::debug!("brief cache: alias: {e}");
                    }
                });
            }
            return Ok("topic registered with the cached brief".into());
        }
        Ok("topic registered".into())
    }
}

impl Message<AddBrief> for Meeting {
    type Reply = Result<String, String>;

    /// 02 §5.2: `source` from what prompted the turn — the outstanding tap for
    /// `topic`, deeper for `parent`, else a request line, else `Auto`.
    /// 05 §4: `aliases: Some` → also cached.
    async fn handle(&mut self, m: AddBrief, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rec = self.recording_for(m.session)?;
        let meeting = rec.manifest.id;
        topics::check_bullets(&m.bullets)?;
        let live = &mut rec.live;
        let source = if let Some(t) = m.topic
            && live.answer(|p| p == Prompted::Tap(t)).is_some()
        {
            BriefSource::Tap
        } else if let Some(p) = m.parent
            && live.answer(|p2| p2 == Prompted::Deeper(p)).is_some()
        {
            BriefSource::Deeper
        } else {
            match live.answer(|p| matches!(p, Prompted::Request { .. })) {
                Some(Prompted::Request { spoken: true }) => BriefSource::Request,
                Some(Prompted::Request { spoken: false }) => BriefSource::Typed,
                _ => BriefSource::Auto,
            }
        };
        let mut cached = None;
        if let Some(aliases) = m.aliases.filter(|a| !a.is_empty()) {
            cached = self
                .cache_put(
                    CachedBriefPut {
                        id: None,
                        title: m.title.clone(),
                        bullets: m.bullets.clone(),
                        aliases,
                        tags: Vec::new(),
                    },
                    &format!("meeting:{meeting}"),
                )
                .await
                .map_err(|e| format!("brief shown but not cached: {e}"))?
                .0
                .id
                .into();
        }
        let rec = self.recording_for(m.session)?;
        let at_ms = rec.clock.now_ms();
        let brief = rec.live.topics.add_brief(
            &m.title, m.bullets, m.topic, m.parent, source, cached, at_ms,
        )?;
        // The answered topic's button goes away and its precomputed brief is superseded.
        let viewed = m.topic.and_then(|t| rec.live.topics.view(t));
        if let Some(t) = viewed {
            self.topic_gone(t.id, TopicGone::Viewed).await;
        }
        self.show_brief(brief).await;
        Ok(if cached.is_some() {
            "brief shown and cached".into()
        } else {
            "brief shown".into()
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 5 handlers (05 §8): the brief cache

impl Message<BriefSearch> for Meeting {
    type Reply = Result<Vec<CachedBrief>, String>;

    async fn handle(&mut self, m: BriefSearch, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cache()?
            .search(&m.query, m.limit.unwrap_or(8))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<BriefList> for Meeting {
    type Reply = Result<Vec<CachedBrief>, String>;

    async fn handle(&mut self, m: BriefList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cache()?
            .list(m.tag.as_deref(), m.limit.unwrap_or(cache::LIST_CAP))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<BriefTags> for Meeting {
    type Reply = Result<Vec<String>, String>;

    async fn handle(&mut self, _: BriefTags, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cache()?.tags().await.map_err(|e| format!("{e:#}"))
    }
}

impl Message<BriefGet> for Meeting {
    type Reply = Result<CachedBrief, String>;

    async fn handle(&mut self, m: BriefGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cache()?.get(m.id).await
    }
}

impl Message<BriefPut> for Meeting {
    type Reply = Result<(CachedBrief, bool), String>;

    /// 05 §7 check 8: with a meeting live, an edit of a row that is on the island
    /// re-publishes it as `Brief{source: Cache}` with the new content.
    async fn handle(&mut self, m: BriefPut, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (row, created) = self.cache_put(m.put, &m.source).await?;
        if let Some(rec) = self.current.as_mut()
            && let Some(topic) = rec.live.topics.refresh_cached(&row)
        {
            let at_ms = rec.clock.now_ms();
            if let Ok(brief) = rec.live.topics.add_brief(
                &row.title,
                row.bullets.clone(),
                topic,
                None,
                BriefSource::Cache,
                Some(row.id),
                at_ms,
            ) {
                self.show_brief(brief).await;
            }
        }
        Ok((row, created))
    }
}

impl Message<BriefDelete> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: BriefDelete, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cache()?.delete(m.id).await?;
        self.rt
            .publish(BriefEvent::Changed {
                id: m.id,
                removed: true,
            })
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 4 handlers (04 §5)

impl Message<GetTranscript> for Meeting {
    type Reply = Result<Transcript, String>;

    async fn handle(
        &mut self,
        m: GetTranscript,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .transcript(m.meeting)
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<EditTranscript> for Meeting {
    type Reply = Result<Transcript, String>;

    /// All ops or none: applied to a copy, saved (and re-indexed) only on `Ok`.
    async fn handle(
        &mut self,
        m: EditTranscript,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let manifest = self.stored(m.meeting)?;
        if matches!(
            manifest.state,
            MeetingState::Post(_) | MeetingState::Summarizing
        ) {
            return Err(format!("{:?}: wait for it to finish", manifest.state));
        }
        let mut t = self
            .store
            .transcript(m.meeting)
            .map_err(|e| format!("{e:#}"))?;
        let dir = self.store.meeting_dir(m.meeting);
        let embeddings: std::collections::HashMap<String, Vec<f32>> =
            std::fs::read(dir.join(transcript::EMBEDDINGS))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        let mut profiles = Profiles::load(&self.paths().speakers).map_err(|e| format!("{e:#}"))?;
        transcript::apply(&mut t, &m.ops, &embeddings, &mut profiles)?;
        self.store
            .save_transcript(&t)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(t)
    }
}

impl Message<SegmentAudio> for Meeting {
    type Reply = DelegatedReply<Result<String, String>>;

    /// Off the mailbox: decoding up to `t1` (04 §0.5, 156 D10) is seconds for a late segment.
    async fn handle(
        &mut self,
        m: SegmentAudio,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let path = match self.store.manifest(m.meeting) {
            Ok(manifest) => self.track_path(&manifest, m.track),
            Err(e) => {
                tx.send(Err(format!("{e:#}")));
                return reply;
            }
        };
        tokio::spawn(async move {
            let r = tokio::task::spawn_blocking(move || segment_wav(&path, m.t0_ms, m.t1_ms))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| format!("{e:#}")));
            tx.send(r);
        });
        reply
    }
}

/// `[t0, t1)` of the Opus file at `path` as a base64 16-bit mono WAV at the decoder's rate.
/// 156 D10: decoded only as far as `t1`, keeping only the range.
fn segment_wav(path: &Path, t0_ms: u64, t1_ms: u64) -> anyhow::Result<String> {
    use base64::Engine as _;
    if t1_ms <= t0_ms {
        anyhow::bail!("empty span");
    }
    let slice = post::transcribe::opus_ranges(path, &[(t0_ms, t1_ms)])?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: post::transcribe::OPUS_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::with_capacity(44 + slice.len() * 2));
    {
        let mut w = hound::WavWriter::new(&mut out, spec)?;
        for s in slice {
            w.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
        }
        w.finalize()?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

impl Message<EnrollSegments> for Meeting {
    type Reply = DelegatedReply<Result<Enrolled, String>>;

    /// Checks and the cheap reads here; the embedding off the mailbox (seconds); then
    /// [`EnrollApply`] back on it, so a `meeting_edit` in between is not overwritten.
    async fn handle(
        &mut self,
        m: EnrollSegments,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let prep = (|| -> Result<_, String> {
            let manifest = self.enrollable(m.meeting)?;
            let t = self
                .store
                .transcript(m.meeting)
                .map_err(|e| format!("{e:#}"))?;
            let (track, spans) = transcript::speech_spans(&t, &m.segments)?;
            let vp = transcript::voiceprint_spans(&spans)?;
            let paths = self.paths();
            let profiles = Profiles::load(&paths.speakers).map_err(|e| format!("{e:#}"))?;
            // Refuse a bad target before the seconds of embedding; `EnrollApply` re-checks.
            match &m.target {
                EnrollTarget::New { name, .. } => {
                    if let Some(p) = profiles
                        .list()
                        .into_iter()
                        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
                    {
                        return Err(format!("a speaker named {} is already enrolled", p.name));
                    }
                }
                EnrollTarget::Existing { profile } => {
                    if !profiles.list().iter().any(|p| p.id == *profile) {
                        return Err(format!("no speaker profile {profile}"));
                    }
                }
            }
            let models =
                post::diarize::models(&paths.models, &paths.data_dir).ok_or_else(|| {
                    format!(
                        "speaker models not installed under {}",
                        paths.models.display()
                    )
                })?;
            let gated = matches!(m.target, EnrollTarget::New { gated: true, .. });
            Ok((self.track_path(&manifest, track), vp, models, gated))
        })();
        let (path, vp, models, gated) = match prep {
            Ok(p) => p,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let transcript::Voiceprint {
                spans,
                speech_ms,
                available_ms,
            } = vp;
            let emb = tokio::task::spawn_blocking(move || {
                if gated {
                    post::suggest::embed_spans_gated(&path, &spans, &models)
                } else {
                    post::embed_spans(&path, &spans, &models)
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let r = match emb {
                Ok(emb) => me
                    .ask(EnrollApply {
                        m,
                        emb,
                        speech_ms,
                        available_ms,
                    })
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            tx.send(r);
        });
        reply
    }
}

impl Message<EnrollApply> for Meeting {
    type Reply = Result<Enrolled, String>;

    async fn handle(&mut self, a: EnrollApply, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.enrollable(a.m.meeting)?;
        let mut profiles = Profiles::load(&self.paths().speakers).map_err(|e| format!("{e:#}"))?;
        let profile = match a.m.target {
            EnrollTarget::New { name, entity, .. } => profiles.create(&name, entity, &a.emb)?,
            EnrollTarget::Existing { profile } => profiles.add_sample(profile, &a.emb)?,
        };
        let mut t = self
            .store
            .transcript(a.m.meeting)
            .map_err(|e| format!("{e:#}"))?;
        let embeddings: HashMap<String, Vec<f32>> = std::fs::read(
            self.store
                .meeting_dir(a.m.meeting)
                .join(transcript::EMBEDDINGS),
        )
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
        if transcript::link_enrolled(&mut t, &a.m.segments, &profile, &embeddings, &profiles) {
            self.store
                .save_transcript(&t)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(Enrolled {
            profile,
            speech_ms: a.speech_ms,
            available_ms: a.available_ms,
            transcript: t,
        })
    }
}

impl Message<ListSpeakers> for Meeting {
    type Reply = Result<Vec<Profile>, String>;

    async fn handle(&mut self, _: ListSpeakers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Profiles::load(&self.paths().speakers)
            .map(|p| p.list())
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<DeleteSpeaker> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: DeleteSpeaker,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Profiles::load(&self.paths().speakers)
            .map_err(|e| format!("{e:#}"))?
            .delete(m.profile)
    }
}

impl Message<LiveDiarization> for Meeting {
    type Reply = bool;

    async fn handle(
        &mut self,
        _: LiveDiarization,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.cfg.live_diarization
    }
}

impl Message<SetLiveDiarization> for Meeting {
    type Reply = Result<bool, String>;

    async fn handle(
        &mut self,
        m: SetLiveDiarization,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        runtime::write_config_value(
            &self.rt.data_dir.join("config.yaml"),
            "meeting",
            "live_diarization",
            if m.on { "true" } else { "false" },
        )
        .map_err(|e| format!("config.yaml: {e}"))?;
        self.cfg.live_diarization = m.on;
        tracing::info!("meeting: live_diarization → {}", m.on);
        Ok(m.on)
    }
}

impl Meeting {
    /// Queue the summary turn for `manifest` (04 §6): the form goes out as its own
    /// turn — now when the session is idle, after the running turn otherwise — the
    /// manifest becomes `Summarizing`, and a watchdog bounds the wait by
    /// [`SUMMARY_TIMEOUT`]. `Err` leaves the manifest untouched.
    async fn begin_summary(
        &mut self,
        mut manifest: Manifest,
        me: ActorRef<Self>,
    ) -> Result<(), String> {
        let input = InputId::new();
        let restore = self.summary_model_switch(manifest.session).await;
        session::queue_turn(manifest.session, input, summary::form_for(&manifest)).await?;
        manifest.state = MeetingState::Summarizing;
        manifest.failure = None;
        self.save(&manifest).await;
        let meeting = manifest.id;
        self.summarizing.insert(
            meeting,
            Summarizing {
                manifest,
                input,
                turn: None,
                restore,
            },
        );
        tokio::spawn(async move {
            tokio::time::sleep(SUMMARY_TIMEOUT).await;
            let _ = me.tell(SummaryTimeout { meeting, input }).await;
        });
        Ok(())
    }

    /// 158 §6.1: the summary turn runs on `meeting.summary_model` when set — the session is
    /// switched here and the model to put back is returned (its own, else the default new
    /// sessions get: a session that followed the default stays pinned to that value after).
    async fn summary_model_switch(&self, session: SessionId) -> Option<String> {
        let model = self.cfg.summary_model.clone()?;
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()?;
        let prior = match agents.ask(agent::SessionModel(session)).await {
            Ok(Some(m)) => m,
            Ok(None) => agents.ask(agent::GetDefaultModel).await.ok()?,
            Err(e) => {
                tracing::warn!(%session, "meeting: summary model: {e}");
                return None;
            }
        };
        if prior == model {
            return None;
        }
        match agents
            .ask(agent::SetModel {
                session: Some(session),
                model,
                manual: false,
            })
            .await
        {
            Ok(_) => Some(prior),
            Err(e) => {
                tracing::warn!(%session, "meeting: summary model: {e}");
                None
            }
        }
    }

    /// Release the slot matching `pred`; the session goes back to its own model (§6.1).
    fn take_summarizing(&mut self, pred: impl Fn(&Summarizing) -> bool) -> Option<Summarizing> {
        let id = self
            .summarizing
            .iter()
            .find(|(_, s)| pred(s))
            .map(|(id, _)| *id)?;
        let s = self.summarizing.remove(&id)?;
        if let Some(model) = s.restore.clone() {
            let session = s.manifest.session;
            tokio::spawn(async move {
                if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    && let Err(e) = agents
                        .ask(agent::SetModel {
                            session: Some(session),
                            model,
                            manual: false,
                        })
                        .await
                {
                    tracing::warn!(%session, "meeting: restore model: {e}");
                }
            });
        }
        Some(s)
    }

    /// A summary that will not settle through its turn: back to `Review` with
    /// `failure` set so the user can retry; `State` is published by `save`.
    async fn summary_failed(&self, mut m: Manifest, reason: &str) {
        tracing::warn!(meeting = %m.id, "summary did not complete: {reason}");
        m.state = MeetingState::Review;
        m.failure = Some(format!("summary did not complete: {reason}"));
        self.save(&m).await;
    }
}

impl Message<Summarize> for Meeting {
    type Reply = Result<(), String>;

    /// 04 §6: `Review` → `Summarizing`; the form is its own turn; that turn's
    /// `AgentEvent::TurnFinished` finishes it. A `Review` with a `failure` is a retry.
    /// 59 §7.4: a meeting with a note is snapshotted `meeting-before-summary` first and gets the note contract.
    async fn handle(&mut self, m: Summarize, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let manifest = self.stored(m.meeting)?;
        if manifest.state != MeetingState::Review {
            return Err(format!("{:?}: review it first", manifest.state));
        }
        if let Some(doc) = &manifest.note {
            note::snapshot(doc, note::BEFORE_SUMMARY).await?;
        }
        self.begin_summary(manifest, ctx.actor_ref().clone()).await
    }
}

impl Message<SummaryTimeout> for Meeting {
    type Reply = ();

    /// The watchdog fired: if this attempt still holds the slot, release it.
    async fn handle(
        &mut self,
        t: SummaryTimeout,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let held = self
            .summarizing
            .get(&t.meeting)
            .is_some_and(|s| s.input == t.input);
        if held && let Some(s) = self.summarizing.remove(&t.meeting) {
            let why = if s.turn.is_some() {
                "its turn"
            } else {
                "the session"
            };
            let mins = SUMMARY_TIMEOUT.as_secs() / 60;
            self.summary_failed(
                s.manifest,
                &format!("{why} did not finish within {mins} minutes"),
            )
            .await;
        }
    }
}

impl Message<Resummarize> for Meeting {
    type Reply = ();

    /// 04 §2: a manifest found `Summarizing` at start. The session's history already
    /// holding the finished summary turn → settle on it; otherwise the form goes out
    /// again (no new `meeting-before-summary` snapshot: the one from the first attempt
    /// stands). A re-send that fails leaves `Review` + `failure`.
    async fn handle(
        &mut self,
        Resummarize(m): Resummarize,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match summary::finished_turn(m.session).await {
            Ok(true) => {
                tracing::info!(meeting = %m.id, "meeting: settling the summary turn found in the history");
                summary::spawn(self.rt.clone(), self.store.clone(), m, None);
            }
            Ok(false) | Err(_) => {
                tracing::info!(meeting = %m.id, "meeting: re-sending the summary form after a restart");
                if let Err(e) = self.begin_summary(m.clone(), ctx.actor_ref().clone()).await {
                    self.summary_failed(
                        m,
                        &format!("the daemon restarted and re-sending the form failed: {e}"),
                    )
                    .await;
                }
            }
        }
    }
}

impl Message<PostRetry> for Meeting {
    type Reply = Result<(), String>;

    /// 59 §7.4: the note goes back to `meeting-before-summary` (a diff import) before the pipeline re-runs.
    async fn handle(&mut self, m: PostRetry, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let manifest = self.stored(m.meeting)?;
        if manifest.ended_at.is_none() || manifest.state == MeetingState::Summarizing {
            return Err(format!("{:?}: not a finished meeting", manifest.state));
        }
        if let Some(doc) = &manifest.note
            && let Err(e) = note::restore_before_summary(doc).await
        {
            tracing::warn!(meeting = %manifest.id, "meeting note not restored: {e}");
        }
        self.start_post(manifest)
    }
}

impl Message<GetSummary> for Meeting {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: GetSummary, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.summary(m.meeting).map_err(|e| format!("{e:#}"))
    }
}

impl Message<agent::EndlessConfigChanged> for Meeting {
    type Reply = ();

    /// 137 §11.3: Settings › Memory changed `agent.endless`; meetings started
    /// from now on follow the new default and budget.
    async fn handle(&mut self, m: agent::EndlessConfigChanged, _: &mut Context<Self, Self::Reply>) {
        self.endless = endless::Config::from(&m.0);
    }
}

// ---------------------------------------------------------------------------
// Plugin

/// 158 D13 (W-n): a meeting the last run left `Recording`/`Paused` — crash, power loss — with
/// its tracks on disk is stopped here and enters post like any other ([`resumable`] then takes
/// it); one that never wrote audio is `Failed`. No UI.
async fn finalise_interrupted(store: &Store, manifests: &mut [Manifest]) {
    for m in manifests
        .iter_mut()
        .filter(|m| matches!(m.state, MeetingState::Recording | MeetingState::Paused))
    {
        let dir = store.meeting_dir(m.id);
        if m.tracks.iter().any(|t| dir.join(&t.file).is_file()) {
            tracing::warn!(meeting = %m.id, "meeting: left {:?} by the last run; finalising", m.state);
            m.state = MeetingState::Stopping;
            m.ended_at
                .get_or_insert_with(|| chrono::Local::now().to_rfc3339());
        } else {
            m.state = MeetingState::Failed;
            m.failure = Some("interrupted before any audio was written".into());
        }
        if let Err(e) = store.save(m).await {
            tracing::warn!(meeting = %m.id, "manifest: {e:#}");
        }
    }
}

/// 04 §2: a manifest whose post pipeline (or summary) a restart resumes.
fn resumable(m: &Manifest) -> bool {
    matches!(m.state, MeetingState::Post(_) | MeetingState::Summarizing)
        || (m.state == MeetingState::Stopping && m.ended_at.is_some())
}

/// The plugin: opens the store, spawns and registers [`Meeting`].
pub struct MeetingPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for MeetingPlugin {
    fn name(&self) -> &'static str {
        "meeting"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: MeetingConfig = rt.section("meeting").unwrap_or_else(|e| {
            tracing::debug!("no [meeting] config section, using defaults: {e}");
            MeetingConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("meeting disabled by config");
            return Ok(());
        }
        let store = match Store::open(&rt.data_dir).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!("meeting unavailable: {e:#}");
                return Ok(());
            }
        };
        let mut manifests = store.manifests();
        finalise_interrupted(&store, &mut manifests).await;
        // 04 §2: a restart mid-pipeline resumes it; a `Summarizing` manifest is
        // re-queued once the actor hears agent events (`Resummarize`, below).
        let (resume, settle): (Vec<Manifest>, Vec<Manifest>) = manifests
            .into_iter()
            .filter(resumable)
            .partition(|m| m.state != MeetingState::Summarizing);
        // 05 §0.1: the brief cache is its own file; a broken one costs the hot path, not the meeting.
        let cache = match Cache::open(&rt.data_dir).await {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                tracing::warn!("brief cache unavailable: {e:#}");
                None
            }
        };
        // 137 R1-D6: the meeting's memory port is the agent actor (kameo messages).
        endless_agent::install();
        let detect_cfg = cfg.detect.clone();
        let mut meeting = Meeting {
            rt: rt.clone(),
            detect: detect::Detector::new(&cfg.detect),
            cfg,
            endless: endless::Config::load(rt),
            store: store.clone(),
            cache: cache.clone(),
            current: None,
            starting: None,
            audio_capture: PermissionState::NotDetermined,
            post: Vec::new(),
            summarizing: HashMap::new(),
        };
        for m in resume {
            tracing::info!(meeting = %m.id, "meeting: resuming the post pipeline");
            let _ = meeting.start_post(m);
        }
        let actor = Meeting::spawn_with_mailbox(meeting, runtime::subscriber_mailbox());
        actor.register(MEETING_ACTOR)?;
        // 158 §7.3 (W2): ICS subscriptions poll from boot, not from the first calendar RPC.
        calendar::ensure_poll(rt);
        // 158 §9.4 (W3): the one-minute `Upcoming` tick.
        calendar::prebrief::ensure_upcoming(rt);
        // 02 §0.4: turn state and approvals of the meeting session.
        rt.subscribe::<Meeting, AgentEvent>(&actor).await;
        rt.subscribe::<Meeting, ApprovalRequested>(&actor).await;
        rt.subscribe::<Meeting, ApprovalResolved>(&actor).await;
        // 137 §11.3: `default_meeting` / `meeting_view_bytes` follow Settings › Memory live.
        rt.subscribe::<Meeting, agent::EndlessConfigChanged>(&actor)
            .await;
        // 158 §4.6: theuth's own dictation and talk quiet the detector; §4.2: the sensor loop.
        rt.subscribe::<Meeting, voice::VoiceEvent>(&actor).await;
        rt.subscribe::<Meeting, talk::TalkEvent>(&actor).await;
        let _ = actor.tell(detect::Configure(detect_cfg)).await;
        for m in settle {
            let _ = actor.tell(Resummarize(m)).await;
        }
        // 57 §3.4: the oracle switch position, resolved the way the decision plugin will
        // (it starts after us, so this reads the config, not the actor).
        let dcfg: decision::DecisionConfig = rt.section("decision").unwrap_or_default();
        let mode = decision::mode_for(&dcfg, &dcfg.model_dir_in(&rt.data_dir), oracle::PREFIX);
        match &mode.reason {
            Some(r) => tracing::info!(
                "meeting: decision oracle {} (configured {}: {r})",
                mode.effective,
                mode.mode
            ),
            None => tracing::info!("meeting: decision oracle {}", mode.effective),
        }
        // 02 §5.3: what `[meeting …]` lines mean; static, registered once. 04 §5:
        // the four post tools read the store and live for the whole process. 05 §4:
        // the four brief tools go through the actor, same block.
        let rt = rt.clone();
        let me = actor.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(::tools::RegisterPrompt {
                        plugin: "meeting",
                        text: tools::PROMPT.to_owned(),
                    })
                    .await;
                tools::register_post(&reg, store).await;
                if cache.is_some() {
                    tools::register_cache(&reg, me).await;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    /// Collects every `MeetingEvent` the bus publishes.
    #[derive(kameo::Actor)]
    struct Sink(Arc<Mutex<Vec<MeetingEvent>>>);

    impl Message<MeetingEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: MeetingEvent, _: &mut Context<Self, ()>) {
            self.0.lock().push(e);
        }
    }

    struct Bench {
        _dir: tempfile::TempDir,
        store: Arc<Store>,
        actor: ActorRef<Meeting>,
        events: Arc<Mutex<Vec<MeetingEvent>>>,
    }

    /// A `Meeting` actor on a temp store with no agent registered: `queue_turn`
    /// fails with "agent is not running", `summary::spawn` settles only through
    /// its `error` path.
    async fn bench() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_owned());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(events.clone()), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, MeetingEvent>(&sink).await;
        let store = Arc::new(Store::open(dir.path()).await.unwrap());
        let meeting = Meeting {
            rt,
            cfg: MeetingConfig::default(),
            endless: endless::Config::default(),
            store: store.clone(),
            cache: None,
            current: None,
            starting: None,
            audio_capture: PermissionState::NotDetermined,
            post: Vec::new(),
            summarizing: HashMap::new(),
            detect: detect::Detector::new(&DetectConfig::default()),
        };
        let actor = Meeting::spawn_with_mailbox(meeting, runtime::subscriber_mailbox());
        Bench {
            _dir: dir,
            store,
            actor,
            events,
        }
    }

    /// A finished meeting in `state`, on disk.
    async fn finished(store: &Store, state: MeetingState) -> Manifest {
        let mut m = Manifest::new(
            MeetingId::new(),
            ProcessInfo {
                pid: 1,
                name: "Zoom".into(),
                bundle_id: None,
                active: true,
            },
            "2026-09-24T10:00:00+02:00".into(),
        );
        m.session = SessionId::new();
        m.ended_at = Some("2026-09-24T10:30:00+02:00".into());
        m.state = state;
        store.create(&m).await.unwrap();
        m
    }

    /// Hold the slot for `m` as `begin_summary` does after a successful `queue_turn`.
    async fn hold(b: &Bench, m: &Manifest, input: InputId) {
        let mut m = m.clone();
        m.state = MeetingState::Summarizing;
        b.store.save(&m).await.unwrap();
        b.actor
            .ask(Hold(Summarizing {
                manifest: m,
                input,
                turn: None,
                restore: None,
            }))
            .await
            .unwrap();
    }

    struct Hold(Summarizing);
    impl Message<Hold> for Meeting {
        type Reply = ();
        async fn handle(&mut self, h: Hold, _: &mut Context<Self, ()>) {
            self.summarizing.insert(h.0.manifest.id, h.0);
        }
    }

    /// 158 §9.2 (W3): `meeting_set_project` rewrites the manifest and the index row (so the
    /// `project` filter finds it), remembers the series by title, and clearing forgets it.
    #[tokio::test]
    async fn set_project_updates_manifest_index_and_memory() {
        let b = bench().await;
        let mut m = finished(&b.store, MeetingState::Done).await;
        m.title = Some("Weekly sync".into());
        b.store.save(&m).await.unwrap();
        let got = b
            .actor
            .ask(calendar::prebrief::SetProject {
                meeting: m.id,
                project: Some("theuth".into()),
            })
            .await
            .unwrap();
        assert_eq!(got.project.as_deref(), Some("theuth"));
        assert_eq!(
            b.store.manifest(m.id).unwrap().project.as_deref(),
            Some("theuth")
        );
        let rows = b.store.list_in(10, Some("theuth")).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [m.id]);
        assert_eq!(rows[0].project.as_deref(), Some("theuth"));
        assert!(b.store.list_in(10, Some("other")).await.unwrap().is_empty());
        assert_eq!(
            b.actor
                .ask(ListMeetings {
                    limit: None,
                    project: Some("theuth".into())
                })
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            calendar::prebrief::remembered(&b.store, None, Some("weekly sync — Oct 14"))
                .await
                .unwrap()
                .as_deref(),
            Some("theuth"),
            "the title key is remembered"
        );
        assert_eq!(
            b.store.done_in_project("theuth", 3).await.unwrap()[0].id,
            m.id
        );
        let cleared = b
            .actor
            .ask(calendar::prebrief::SetProject {
                meeting: m.id,
                project: None,
            })
            .await
            .unwrap();
        assert_eq!(cleared.project, None);
        assert!(
            b.store
                .list_in(10, Some("theuth"))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            calendar::prebrief::remembered(&b.store, None, Some("Weekly sync"))
                .await
                .unwrap(),
            None,
            "a cleared project is not sticky"
        );
    }

    /// 06 §1.5 / 81: the toolset sent at start carries the configured hint
    /// limit (default 2); `0` reaches the agent as `Some(0)` = no hints.
    #[test]
    fn live_toolset_carries_the_hint_limit() {
        let cfg = MeetingConfig::default();
        let live = cfg.live_toolset();
        assert_eq!(live.hint_limit, Some(2));
        assert_eq!(
            (live.tools, live.prompts, live.runtime),
            (cfg.live_tools, cfg.live_prompts, false)
        );
        let none = MeetingConfig {
            live_hint_limit: 0,
            ..MeetingConfig::default()
        };
        assert_eq!(none.live_toolset().hint_limit, Some(0));
        let parsed: MeetingConfig = serde_json::from_str(r#"{"live_hint_limit": 4}"#).unwrap();
        assert_eq!(parsed.live_toolset().hint_limit, Some(4));
    }

    struct Held;
    impl Message<Held> for Meeting {
        type Reply = Vec<MeetingId>;
        async fn handle(&mut self, _: Held, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
            self.summarizing.keys().copied().collect()
        }
    }

    fn finished_turn(session: SessionId, turn: TurnId, stop_reason: &str) -> AgentEvent {
        AgentEvent::TurnFinished {
            session,
            turn,
            stop_reason: stop_reason.into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    /// `summary::spawn` settles off the mailbox: wait for the manifest to leave `Summarizing`.
    async fn settled(store: &Store, id: MeetingId) -> Manifest {
        for _ in 0..100 {
            let m = store.manifest(id).unwrap();
            if m.state != MeetingState::Summarizing {
                return m;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("still Summarizing");
    }

    /// `State` events for `id` once the bus delivered at least one (a `tell` hop after the save).
    async fn states(events: &Mutex<Vec<MeetingEvent>>, id: MeetingId) -> Vec<MeetingState> {
        let pick = || {
            events
                .lock()
                .iter()
                .filter_map(|e| match e {
                    MeetingEvent::State { meeting, state, .. } if *meeting == id => Some(*state),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for _ in 0..100 {
            let s = pick();
            if !s.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
                return pick();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        pick()
    }

    /// 04 §6: the turn in flight at `Summarize` ends first and is not the summary;
    /// the form's `InputInjected` binds the next turn, whose end settles — once.
    #[tokio::test]
    async fn summary_settles_on_its_own_turn_after_the_one_in_flight() {
        let b = bench().await;
        let m = finished(&b.store, MeetingState::Review).await;
        let input = InputId::new();
        hold(&b, &m, input).await;
        let (batch, summary) = (TurnId::new(), TurnId::new());

        // The stop-flush batch turn ends: nothing settles.
        b.actor
            .ask(finished_turn(m.session, batch, "end_turn"))
            .await
            .unwrap();
        assert_eq!(b.actor.ask(Held).await.unwrap(), [m.id]);
        assert_eq!(
            b.store.manifest(m.id).unwrap().state,
            MeetingState::Summarizing
        );

        // The deferred form opens the next turn.
        b.actor
            .ask(AgentEvent::InputInjected {
                session: m.session,
                id: input,
                turn: summary,
            })
            .await
            .unwrap();
        b.actor
            .ask(AgentEvent::TurnStarted {
                session: m.session,
                turn: summary,
                text: summary::SUMMARY_FORM.into(),
                model: "m".into(),
                turn_seq: 0,
                channel: None,
            })
            .await
            .unwrap();
        assert_eq!(b.actor.ask(Held).await.unwrap(), [m.id]);

        // Its end hands the meeting to `summary::spawn` (the cancelled path needs no agent).
        b.actor
            .ask(finished_turn(m.session, summary, "cancelled"))
            .await
            .unwrap();
        assert!(b.actor.ask(Held).await.unwrap().is_empty(), "slot released");
        let after = settled(&b.store, m.id).await;
        assert_eq!(after.state, MeetingState::Review);
        assert_eq!(
            after.failure.as_deref(),
            Some("summary: the summary turn ended with cancelled")
        );

        // A second end of the same turn is nothing.
        b.actor
            .ask(finished_turn(m.session, summary, "cancelled"))
            .await
            .unwrap();
        assert_eq!(states(&b.events, m.id).await, [MeetingState::Review]);
    }

    /// Idle session: `InputInjected` comes first; same settle. Two meetings hold slots at once.
    #[tokio::test]
    async fn two_meetings_summarize_independently() {
        let b = bench().await;
        let (a, c) = (
            finished(&b.store, MeetingState::Review).await,
            finished(&b.store, MeetingState::Review).await,
        );
        let (ia, ic) = (InputId::new(), InputId::new());
        hold(&b, &a, ia).await;
        hold(&b, &c, ic).await;
        let (ta, tc) = (TurnId::new(), TurnId::new());
        for (m, i, t) in [(&a, ia, ta), (&c, ic, tc)] {
            b.actor
                .ask(AgentEvent::InputInjected {
                    session: m.session,
                    id: i,
                    turn: t,
                })
                .await
                .unwrap();
        }
        b.actor
            .ask(finished_turn(c.session, tc, "error"))
            .await
            .unwrap();
        assert_eq!(b.actor.ask(Held).await.unwrap(), [a.id], "a still held");
        assert_eq!(settled(&b.store, c.id).await.state, MeetingState::Review);
        b.actor
            .ask(finished_turn(a.session, ta, "cancelled"))
            .await
            .unwrap();
        assert!(b.actor.ask(Held).await.unwrap().is_empty());
        assert_eq!(settled(&b.store, a.id).await.state, MeetingState::Review);
    }

    /// The watchdog: a stale attempt's timer is nothing; the live one releases the
    /// slot, `Review` + `failure`, `State` published. A withdrawn form does the same.
    #[tokio::test]
    async fn watchdog_and_withdrawal_release_the_slot() {
        let b = bench().await;
        let m = finished(&b.store, MeetingState::Review).await;
        let input = InputId::new();
        hold(&b, &m, input).await;
        b.actor
            .ask(SummaryTimeout {
                meeting: m.id,
                input: InputId::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            b.actor.ask(Held).await.unwrap(),
            [m.id],
            "another attempt's timer"
        );
        b.actor
            .ask(SummaryTimeout {
                meeting: m.id,
                input,
            })
            .await
            .unwrap();
        assert!(b.actor.ask(Held).await.unwrap().is_empty());
        let after = b.store.manifest(m.id).unwrap();
        assert_eq!(after.state, MeetingState::Review);
        assert_eq!(
            after.failure.as_deref(),
            Some("summary did not complete: the session did not finish within 10 minutes")
        );
        assert_eq!(states(&b.events, m.id).await, [MeetingState::Review]);

        let w = finished(&b.store, MeetingState::Review).await;
        let input = InputId::new();
        hold(&b, &w, input).await;
        b.actor
            .ask(AgentEvent::InputWithdrawn {
                session: w.session,
                id: input,
            })
            .await
            .unwrap();
        assert!(b.actor.ask(Held).await.unwrap().is_empty());
        let after = b.store.manifest(w.id).unwrap();
        assert_eq!(after.state, MeetingState::Review);
        assert!(after.failure.as_deref().unwrap().contains("withdrawn"));
    }

    /// 04 §2: a `Summarizing` manifest at start is re-queued; with no agent the
    /// re-send fails and the meeting lands in `Review` with the failure — never stuck.
    #[tokio::test]
    async fn restart_requeues_and_fails_clean_without_an_agent() {
        let b = bench().await;
        let m = finished(&b.store, MeetingState::Summarizing).await;
        b.actor.ask(Resummarize(m.clone())).await.unwrap();
        assert!(b.actor.ask(Held).await.unwrap().is_empty());
        let after = b.store.manifest(m.id).unwrap();
        assert_eq!(after.state, MeetingState::Review);
        assert_eq!(
            after.failure.as_deref(),
            Some(
                "summary did not complete: the daemon restarted and re-sending the form failed: agent is not running"
            )
        );
        assert_eq!(states(&b.events, m.id).await, [MeetingState::Review]);
    }

    /// A `Review` with a failure accepts `Summarize` again; a held or finished
    /// meeting does not. Without an agent the retry stops at delivery, manifest untouched.
    #[tokio::test]
    async fn summarize_again_after_a_failure() {
        let b = bench().await;
        let mut m = finished(&b.store, MeetingState::Review).await;
        m.failure =
            Some("summary did not complete: the session did not finish within 10 minutes".into());
        b.store.save(&m).await.unwrap();
        let r = b.actor.ask(Summarize { meeting: m.id }).await;
        assert!(
            matches!(&r, Err(kameo::error::SendError::HandlerError(e)) if e == "agent is not running"),
            "past the state gate, stopped at delivery: {r:?}"
        );
        assert_eq!(b.store.manifest(m.id).unwrap().state, MeetingState::Review);
        assert!(b.actor.ask(Held).await.unwrap().is_empty());

        let d = finished(&b.store, MeetingState::Done).await;
        let r = b.actor.ask(Summarize { meeting: d.id }).await;
        assert!(
            matches!(&r, Err(kameo::error::SendError::HandlerError(e)) if e == "Done: review it first"),
            "{r:?}"
        );
    }

    /// The handler's `Err`, or a panic naming what came back instead.
    fn refused<T: std::fmt::Debug, M: std::fmt::Debug>(
        r: Result<T, kameo::error::SendError<M, String>>,
    ) -> String {
        match r {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// A one-segment transcript of 1.5 s on the mic track.
    fn mic_transcript(meeting: MeetingId) -> Transcript {
        let words = vec![Word {
            text: "Hello".into(),
            t0_ms: 0,
            t1_ms: 1_500,
        }];
        Transcript {
            meeting,
            speakers: vec![SpeakerLabel {
                id: transcript::ME.into(),
                name: "Me".into(),
                profile: None,
                proposed: None,
            }],
            segments: vec![Segment {
                id: 0,
                track: Track::Mic,
                speaker: transcript::ME.into(),
                t0_ms: 0,
                t1_ms: 1_500,
                text: "Hello".into(),
                words,
            }],
            reviewed_at: None,
            summarized_at: None,
            diarized: true,
            note: None,
        }
    }

    /// 156 D14: `EnrollSegments` is refused while recording, in `Post(_)` and `Summarizing`,
    /// and in Review and Done passes the gate as far as the models (none on the bench).
    #[tokio::test]
    async fn enroll_segments_is_gated_by_state() {
        let b = bench().await;
        let ask = |id| {
            b.actor.ask(EnrollSegments {
                meeting: id,
                segments: vec![0],
                target: EnrollTarget::New {
                    name: "Dana".into(),
                    entity: None,
                    gated: false,
                },
            })
        };
        for (state, msg) in [
            (
                MeetingState::Post(PostStage::Aligning),
                "still processing: wait for the post pipeline",
            ),
            (
                MeetingState::Summarizing,
                "Summarizing: wait for it to finish",
            ),
            (
                MeetingState::Recording,
                "still recording: wait for the meeting to end",
            ),
        ] {
            let m = finished(&b.store, state).await;
            assert_eq!(refused(ask(m.id).await), msg);
        }
        for state in [MeetingState::Review, MeetingState::Done] {
            let m = finished(&b.store, state).await;
            b.store
                .save_transcript(&mic_transcript(m.id))
                .await
                .unwrap();
            let e = refused(ask(m.id).await);
            assert!(
                e.starts_with("speaker models not installed under "),
                "{state:?}: {e}"
            );
            let e = refused(
                b.actor
                    .ask(EnrollSegments {
                        meeting: m.id,
                        segments: vec![7],
                        target: EnrollTarget::Existing {
                            profile: ProfileId::new(),
                        },
                    })
                    .await,
            );
            assert_eq!(e, "no segment 7 in this meeting");
        }
    }

    /// 156 D14, the real embedder on a COPY of a real meeting. Needs
    /// `THEUTH_TEST_MEETING_DIR` (the copied `meetings/<id>/` directory) and the models under
    /// `$THEUTH_DATA_DIR` (else `~/.theuth`) read-only; the profiles go to a temp dir.
    ///
    /// ```text
    /// THEUTH_TEST_MEETING_DIR=/tmp/156/w3-data/meetings/<id> \
    ///   cargo test -p meeting --lib enroll_segments_on_a_real_meeting_copy -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs the real speaker models and THEUTH_TEST_MEETING_DIR (a copied meeting)"]
    fn enroll_segments_on_a_real_meeting_copy() {
        let dir = PathBuf::from(
            std::env::var_os("THEUTH_TEST_MEETING_DIR").expect("THEUTH_TEST_MEETING_DIR"),
        );
        let data_dir = std::env::var_os("THEUTH_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".theuth"));
        let models =
            post::diarize::models(&data_dir.join("models"), &data_dir).expect("speaker models");
        let t: Transcript =
            serde_json::from_slice(&std::fs::read(dir.join(transcript::FILE)).unwrap()).unwrap();
        let embeddings: HashMap<String, Vec<f32>> =
            serde_json::from_slice(&std::fs::read(dir.join(transcript::EMBEDDINGS)).unwrap())
                .unwrap();
        // The first mic segments reaching 5 s of words: the `me` voice.
        let mut chosen = Vec::new();
        let mut ms = 0;
        for s in t.segments.iter().filter(|s| s.track == Track::Mic) {
            chosen.push(s.id);
            ms += s.words.iter().map(|w| w.t1_ms - w.t0_ms).sum::<u64>();
            if ms >= 5_000 {
                break;
            }
        }
        let (track, spans) = transcript::speech_spans(&t, &chosen).unwrap();
        let transcript::Voiceprint {
            spans,
            speech_ms,
            available_ms,
        } = transcript::voiceprint_spans(&spans).unwrap();
        let path = dir.join(Stem::Mic.file());
        let started = Instant::now();
        let emb = post::embed_spans(&path, &spans, &models).unwrap();
        let took = started.elapsed();

        let tmp = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::load(tmp.path()).unwrap();
        let created = profiles.create("Test Me", None, &emb).unwrap();
        let folded = profiles.add_sample(created.id, &emb).unwrap();
        let mut linked = t.clone();
        let did_link =
            transcript::link_enrolled(&mut linked, &chosen, &folded, &embeddings, &profiles);
        let me_cos = embeddings
            .get(transcript::ME)
            .map(|me| speakers::cosine(me, &emb));
        eprintln!(
            "segments {chosen:?} on {track:?}: {} spans, speech_ms {speech_ms}, available_ms {available_ms}; \
             embedding len {}, {took:?}; profile {} samples {}; cosine to the pipeline's `me` {me_cos:?}; linked {did_link}",
            spans.len(),
            emb.len(),
            folded.id,
            folded.samples
        );
        assert_eq!(track, Track::Mic);
        assert!((1_000..=30_000).contains(&speech_ms));
        assert_eq!(folded.samples, 2);
        assert!(emb.len() >= 128, "{}", emb.len());
        assert!(did_link);
        let me = linked
            .speakers
            .iter()
            .find(|l| l.id == transcript::ME)
            .unwrap();
        assert_eq!(
            (me.name.as_str(), me.profile),
            ("Test Me", Some(created.id))
        );
        if let Some(c) = me_cos {
            assert!(
                c >= speakers::MATCH,
                "the segment voiceprint should match the pipeline's `me`: {c}"
            );
        }
        assert_eq!(Profiles::load(tmp.path()).unwrap().list().len(), 1);
    }

    fn zoom() -> ProcessInfo {
        ProcessInfo {
            pid: 4242,
            name: "zoom.us".into(),
            bundle_id: Some("us.zoom.xos".into()),
            active: true,
        }
    }

    /// 158 §4.7 integration: fake mic holders ticked into the actor → `Detected` on the bus
    /// after 15 s, `Status.detection` set, "Not now" → `DetectionCleared{NotNow}` and clear.
    #[tokio::test]
    async fn fake_holders_reach_the_bus_as_detected() {
        let b = bench().await;
        tokio::time::pause(); // after the store opened: sqlx's pool timeout is wall-clock
        b.actor.ask(detect::Holders(vec![zoom()])).await.unwrap();
        tokio::time::advance(Duration::from_secs(14)).await;
        b.actor.ask(detect::Holders(vec![zoom()])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await; // the bus hop
        assert!(
            !b.events
                .lock()
                .iter()
                .any(|e| matches!(e, MeetingEvent::Detected(_))),
            "14 s is not a hold"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        b.actor.ask(detect::Holders(vec![zoom()])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await; // the bus hop
        let det = b
            .events
            .lock()
            .iter()
            .find_map(|e| match e {
                MeetingEvent::Detected(d) => Some(d.clone()),
                _ => None,
            })
            .expect("Detected on the bus");
        assert_eq!(det.app_id, "us.zoom.xos");
        assert_eq!(det.app, zoom());
        assert!(det.event.is_none(), "W2 fills the calendar match");
        let status = b.actor.ask(MeetingStatus).await.unwrap();
        assert_eq!(status.detection.as_ref(), Some(&det));
        assert_eq!(
            b.actor
                .ask(detect::CurrentDetection)
                .await
                .unwrap()
                .as_ref(),
            Some(&det)
        );
        b.actor
            .ask(detect::Dismiss {
                app_id: "us.zoom.xos".into(),
                never: false,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await; // the bus hop
        assert!(b.events.lock().iter().any(|e| matches!(
            e,
            MeetingEvent::DetectionCleared {
                reason: Cleared::NotNow,
                ..
            }
        )));
        assert!(
            b.actor
                .ask(detect::CurrentDetection)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// §4.3: "Never for" lands in `config.yaml` under `meeting.detect.ignore` and the live
    /// settings reflect it.
    #[tokio::test]
    async fn never_persists_to_the_ignore_list() {
        let b = bench().await;
        let path = b._dir.path().join("config.yaml");
        std::fs::write(&path, "meeting:\n  max_minutes: 90\n").unwrap();
        b.actor
            .ask(detect::Dismiss {
                app_id: "us.zoom.xos".into(),
                never: true,
            })
            .await
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("max_minutes: 90"), "{text}");
        use figment::providers::Format as _;
        let cfg: MeetingConfig = figment::Figment::new()
            .merge(figment::providers::Yaml::file(&path))
            .extract_inner("meeting")
            .unwrap();
        assert!(cfg.detect.ignore.iter().any(|i| i == "us.zoom.xos"));
        assert!(
            cfg.detect.ignore.iter().any(|i| i == "com.apple.Siri"),
            "defaults kept"
        );
        let live = b.actor.ask(GetSettings).await.unwrap();
        assert!(live.detect.ignore.iter().any(|i| i == "us.zoom.xos"));
    }

    /// §6.2: `meeting_settings_set` persists every key, keeps the rest of the file, and the
    /// live config follows; the next start resolves `meeting.model`.
    #[tokio::test]
    async fn settings_set_persists_and_applies() {
        let b = bench().await;
        let path = b._dir.path().join("config.yaml");
        std::fs::write(&path, "agent:\n  model: x/y\nmeeting:\n  max_minutes: 90\n").unwrap();
        let mut s = MeetingSettings {
            model: Some("openai/gpt-5".into()),
            summary_model: Some("anthropic/claude".into()),
            live_diarization: false,
            ..MeetingSettings::default()
        };
        s.detect.enabled = false;
        s.detect.poll_secs = 7;
        s.calendar.eventkit = true;
        let got = b.actor.ask(SetSettings(s.clone())).await.unwrap();
        assert_eq!(got, s);
        assert_eq!(b.actor.ask(GetSettings).await.unwrap(), s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("agent:\n  model: x/y"), "{text}");
        assert!(text.contains("max_minutes: 90"), "{text}");
        use figment::providers::Format as _;
        let cfg: MeetingConfig = figment::Figment::new()
            .merge(figment::providers::Yaml::file(&path))
            .extract_inner("meeting")
            .unwrap();
        assert_eq!(MeetingSettings::from(&cfg), s);
        assert_eq!(cfg.max_minutes, 90);
        assert!(!b.actor.ask(LiveDiarization).await.unwrap());
    }

    /// 194 rows 16/17: `meeting.effort` is written beside `meeting.model` and applied live; an
    /// old block without it loads as Default; no model clears it (D4d); a word no model takes
    /// is refused (D4b) unless the model changed with it, which drops it (D4c).
    #[tokio::test]
    async fn meeting_effort_persists_with_its_model() {
        let b = bench().await;
        let path = b._dir.path().join("config.yaml");
        std::fs::write(&path, "meeting:\n  model: \"openai/gpt-5\"\n").unwrap();
        use figment::providers::Format as _;
        let read = || -> MeetingConfig {
            figment::Figment::new()
                .merge(figment::providers::Yaml::file(&path))
                .extract_inner("meeting")
                .unwrap()
        };
        assert_eq!(read().effort, None, "an old block is Default");
        let s = MeetingSettings {
            model: Some("openai/gpt-5".into()),
            effort: Some("high".into()),
            ..MeetingSettings::default()
        };
        let got = b.actor.ask(SetSettings(s.clone())).await.unwrap();
        assert_eq!(got.effort.as_deref(), Some("high"));
        assert_eq!(read().effort.as_deref(), Some("high"));
        let (model, effort) = session_pick(None, None, &read());
        assert_eq!(
            (model.as_deref(), effort.as_deref()),
            (Some("openai/gpt-5"), Some("high"))
        );

        let bad = MeetingSettings {
            effort: Some("loud".into()),
            ..s.clone()
        };
        let err = b.actor.ask(SetSettings(bad.clone())).await.unwrap_err();
        assert!(
            format!("{err:?}").contains("unknown effort loud"),
            "{err:?}"
        );
        assert_eq!(
            read().effort.as_deref(),
            Some("high"),
            "a refusal writes nothing"
        );
        let moved = MeetingSettings {
            model: Some("anthropic/claude".into()),
            ..bad
        };
        assert_eq!(b.actor.ask(SetSettings(moved)).await.unwrap().effort, None);

        let none = MeetingSettings {
            model: None,
            effort: Some("high".into()),
            ..s
        };
        assert_eq!(b.actor.ask(SetSettings(none)).await.unwrap().effort, None);
        assert_eq!(read().effort, None);
    }

    /// 194 D4a/d/e: the start's model brings the start's effort into `create_session`;
    /// without a start model, `meeting.model` brings `meeting.effort`; neither is `agent.*`.
    #[test]
    fn the_session_starts_on_the_picked_effort() {
        let cfg = MeetingConfig {
            model: Some("cfg/m".into()),
            effort: Some("low".into()),
            ..MeetingConfig::default()
        };
        let pick = |m: Option<&str>, e: Option<&str>| {
            let (m, e) = session_pick(m.map(Into::into), e.map(Into::into), &cfg);
            (m.unwrap_or_default(), e.unwrap_or_default())
        };
        assert_eq!(
            pick(Some("a/b"), Some("high")),
            ("a/b".into(), "high".into())
        );
        assert_eq!(pick(Some("a/b"), None), ("a/b".into(), String::new()));
        assert_eq!(pick(None, Some("high")), ("cfg/m".into(), "low".into()));
        let bare = session_pick(None, None, &MeetingConfig::default());
        assert_eq!(bare, (None, None), "today: agent.model at agent.effort");
    }

    /// 158 D13: a manifest the last run left `Recording` with a track on disk is `Stopping`
    /// with an end time (so the post pipeline resumes it); one without audio is `Failed`.
    #[tokio::test]
    async fn interrupted_recordings_finalise_on_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let mut with_audio =
            Manifest::new(MeetingId::new(), zoom(), "2026-10-10T10:00:00+02:00".into());
        with_audio.session = SessionId::new();
        with_audio.state = MeetingState::Recording;
        with_audio.tracks = vec![TrackFile {
            track: Stem::Mic,
            file: "mic.opus".into(),
            sample_rate: 24_000,
        }];
        let mdir = store.create(&with_audio).await.unwrap();
        std::fs::write(mdir.join("mic.opus"), b"OggS").unwrap();
        let mut no_audio =
            Manifest::new(MeetingId::new(), zoom(), "2026-10-10T11:00:00+02:00".into());
        no_audio.session = SessionId::new();
        no_audio.state = MeetingState::Paused;
        store.create(&no_audio).await.unwrap();
        let done = finished(&store, MeetingState::Done).await;

        let mut manifests = store.manifests();
        assert!(
            !manifests.iter().any(resumable),
            "nothing resumable before the arm"
        );
        finalise_interrupted(&store, &mut manifests).await;

        let a = store.manifest(with_audio.id).unwrap();
        assert_eq!(a.state, MeetingState::Stopping);
        assert!(a.ended_at.is_some());
        assert!(resumable(&a), "the post pipeline picks it up");
        let n = store.manifest(no_audio.id).unwrap();
        assert_eq!(n.state, MeetingState::Failed);
        assert!(n.failure.as_deref().unwrap_or("").contains("interrupted"));
        assert!(!resumable(&n));
        assert_eq!(store.manifest(done.id).unwrap().state, MeetingState::Done);
        assert_eq!(manifests.iter().filter(|m| resumable(m)).count(), 1);
    }

    /// 173 R3 #1: a capture gap reported after a user pause lands on the recording clock.
    #[test]
    fn apply_report_maps_gap_instants_across_a_pause() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut clock = RecordingClock::start_at(t0);
        clock.pause_at(s(120));
        clock.resume_at(s(180));
        let mut m = Manifest::new(MeetingId::new(), zoom(), "2026-11-02T21:02:13Z".into());
        let mut status = CaptureStatus::default();
        let changed = apply_report(
            &mut m,
            &clock,
            &mut status,
            capture::Report::Gap {
                track: Track::System,
                start: s(100),
                end: s(200),
                reason: CaptureFault::Silent,
                retarget: None,
            },
        );
        assert_eq!(changed, (true, false));
        assert_eq!((m.gaps[0].at_ms, m.gaps[0].dur_ms), (100_000, 40_000));
    }
}
