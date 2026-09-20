//! Meeting recorder (design doc 53; phase 1 is `docs/design/meetings/01-capture.md`):
//! the meeting app's audio through a Core Audio process tap and the microphone
//! through Apple's VoiceProcessingIO unit (hardware-clocked AEC), encoded live
//! as three Opus tracks under `<data_dir>/meetings/<id>/`, with a manifest, an
//! index and the meeting's own agent session. macOS 15 only; elsewhere the
//! plugin starts nothing.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`MEETING_ACTOR`] actor ([`Meeting`]): [`ListProcesses`],
//!   [`StartMeeting`], [`PauseMeeting`], [`ResumeMeeting`], [`StopMeeting`],
//!   [`MeetingStatus`], [`ListMeetings`], [`GetMeeting`], [`DeleteMeeting`],
//!   [`CheckPermissions`]
//! - events: [`MeetingEvent`] (one stream; every variant carries `meeting`)
//!
//! The types below are the shared contract frozen in `meetings/README.md` §3;
//! phases 2–4 add variants and fields, never rename or retype.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

pub mod capture;
pub mod ogg;
pub mod state;
pub mod store;

pub use state::{Recording, RecordingClock};
pub use store::{Gap, Manifest, MeetingSummary, Stem, Store, TrackFile};

/// Registry name of the [`Meeting`] actor.
pub const MEETING_ACTOR: &str = "meeting";
/// Directory under `data_dir`: `<data_dir>/meetings/`.
pub const DIR: &str = "meetings";
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
    /// The VoiceProcessingIO unit could not be opened; the plain mic is used, no echo cancellation.
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
}

/// `meeting:` config section (README §3). Phase 1 reads `enabled` and
/// `max_minutes`; the rest is declared with its defaults so the struct is
/// written once.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MeetingConfig {
    /// `false`: no actor; `meeting_*` answers "meetings are disabled".
    pub enabled: bool,
    /// Hard stop; `NearMaxLength` at `max_minutes - 10`.
    pub max_minutes: u32,
    /// Phase 2: per-session compaction trigger.
    pub context_tokens: u32,
    /// Phase 2: batcher cadence.
    pub batch_secs: u32,
    /// Phase 2: batcher size.
    pub batch_utterances: u32,
    /// Phase 2: continuous speech is cut into utterances at this length.
    pub stt_max_utterance_ms: u64,
    /// Phase 2: Nemotron streaming directory; `None` → `voice.dictation_model_dir`.
    pub live_model_dir: Option<std::path::PathBuf>,
    /// Phase 2: how long a topic button lives.
    pub topic_ttl_secs: u32,
    /// Phase 2: most topic buttons at once.
    pub max_topics: u32,
    /// Phase 2: ReadOnly tools run without asking in the meeting session.
    pub allow_read_only: bool,
    /// Phase 4: pyannote models; `None` → `<data_dir>/models/pyannote`.
    pub models_dir: Option<std::path::PathBuf>,
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_minutes: 180,
            context_tokens: 24_000,
            batch_secs: 25,
            batch_utterances: 6,
            stt_max_utterance_ms: 15_000,
            live_model_dir: None,
            topic_ttl_secs: 600,
            max_topics: 6,
            allow_read_only: false,
            models_dir: None,
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
/// None` → `Err` (a meeting is a meeting app); a meeting already recording →
/// `Err`. Replies once the devices are open; `State{Recording}` and `Started`
/// are published just before.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StartMeeting {
    /// Pid of the meeting app (`meeting_processes`).
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

/// `ask` → `Result<(), String>`: `Recording | Paused` → `Stopping` → `Done`.
/// Replies once the last Ogg pages are on disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct StopMeeting;

/// `ask` → [`Status`]: for a UI on reconnect.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeetingStatus;

/// Reply to [`MeetingStatus`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
}

/// `ask` → `Result<Vec<MeetingSummary>, String>`: from the index, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListMeetings {
    /// Most rows; `None` = 100.
    pub limit: Option<u32>,
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

/// `tell` from the start task: the devices are open (or not).
struct Begun {
    manifest: Manifest,
    result: Result<capture::Capture, String>,
}

/// `tell` from the timers and the capture threads: something degraded.
struct Degrade(Degradation);

// ---------------------------------------------------------------------------
// Actor

/// The [`MEETING_ACTOR`] actor: one recording at a time (one mic).
#[derive(kameo::Actor)]
pub struct Meeting {
    rt: Runtime,
    cfg: MeetingConfig,
    store: Arc<Store>,
    /// The live meeting, from `Begun(Ok)` to `StopMeeting`.
    current: Option<Recording>,
    /// Set between `StartMeeting` and `Begun`; the manifest of the meeting being set up.
    starting: Option<Manifest>,
    audio_capture: PermissionState,
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
        rec.tasks.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if disk_low(&dir) {
                    let _ = me.tell(Degrade(Degradation::DiskLow)).await;
                }
            }
        }));
    }
}

/// `statvfs` on `dir`: under [`DISK_LOW_BYTES`] free. Unknown → `false`.
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
    #[cfg(not(unix))]
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
        let Some(pid) = m.process else {
            tx.send(Err("a meeting needs a process to record".into()));
            return reply;
        };
        if self.current.is_some() || self.starting.is_some() {
            tx.send(Err("a meeting is already recording".into()));
            return reply;
        }
        let process = match tokio::task::spawn_blocking(capture::system::processes).await {
            Ok(Ok(list)) => match list.into_iter().find(|p| p.pid == pid) {
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
        };
        let now = chrono::Local::now();
        let mut manifest = Manifest::new(MeetingId::new(), process, now.to_rfc3339());
        manifest.title = m.title;
        manifest.context = m.context;
        manifest.session_model = m.session_model;
        if disk_low(&self.rt.data_dir) {
            manifest.degraded.push(Degradation::DiskLow);
        }
        self.starting = Some(manifest.clone());
        self.publish_state(&manifest).await;

        let store = self.store.clone();
        let me = ctx.actor_ref().clone();
        let title = format!("Meeting {}", now.format("%Y-%m-%d %H:%M"));
        tokio::spawn(async move {
            let begun = start_task(&store, manifest, &title).await;
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

/// Session → directory + manifest → devices (01 §3, off the mailbox). The
/// session comes first because `Manifest::session` is not optional.
async fn start_task(store: &Store, mut manifest: Manifest, title: &str) -> Begun {
    let agents = match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(a)) => a,
        _ => return Begun::failed(manifest, "agent is not running".into()),
    };
    let open = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions {
                project: Some("meetings".into()),
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
    if let Err(e) = agents
        .ask(agent::RenameSession {
            session,
            title: title.to_owned(),
        })
        .await
    {
        tracing::warn!(%session, "rename meeting session: {e}");
    }
    let dir = match store.create(&manifest).await {
        Ok(dir) => dir,
        Err(e) => return Begun::failed(manifest, format!("{e:#}")),
    };
    let pid = manifest.process.pid;
    let file_title = title.to_owned();
    let opened =
        tokio::task::spawn_blocking(move || capture::Capture::start(pid, &dir, &file_title))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| format!("{e:#}")));
    match opened {
        Ok((capture, degraded)) => {
            for d in degraded {
                if !manifest.degraded.contains(&d) {
                    manifest.degraded.push(d);
                }
            }
            Begun {
                manifest,
                result: Ok(capture),
            }
        }
        Err(e) => Begun::failed(manifest, e),
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
                // The directory exists only if the session did; `save` creates
                // the row either way so the failure is listed.
                if self.store.meeting_dir(manifest.id).exists() {
                    self.save(&manifest).await;
                } else {
                    self.publish_state(&manifest).await;
                }
            }
            Ok(capture) => {
                self.audio_capture = PermissionState::Granted;
                manifest.state = MeetingState::Recording;
                let id = manifest.id;
                let mut rec = Recording::new(manifest, capture);
                self.spawn_tasks(id, &mut rec, ctx.actor_ref().clone());
                self.rt
                    .publish(MeetingEvent::Started {
                        meeting: id,
                        session: rec.manifest.session,
                        process: rec.manifest.process.clone(),
                        at: rec.manifest.started_at.clone(),
                    })
                    .await;
                manifest = rec.manifest.clone();
                self.current = Some(rec);
                self.save(&manifest).await;
            }
        }
    }
}

impl Message<Degrade> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: Degrade, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
        } = rec;
        for t in tasks {
            t.abort();
        }
        // Joins the three threads; the encoder writes the EOS page.
        if let Err(e) = tokio::task::spawn_blocking(move || capture.stop()).await {
            tracing::warn!(meeting = %manifest.id, "capture stop: {e}");
        }
        manifest.ended_at = Some(chrono::Local::now().to_rfc3339());
        manifest.duration_ms = Some(clock.now_ms());
        manifest.state = MeetingState::Done;
        self.save(&manifest).await;
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
        Status {
            state: self.state(),
            meeting: m.map(|(m, _)| m.id),
            session: m.map(|(m, _)| m.session),
            elapsed_ms: m.map_or(0, |(_, ms)| ms),
            degraded: m.map(|(m, _)| m.degraded.clone()).unwrap_or_default(),
        }
    }
}

impl Message<ListMeetings> for Meeting {
    type Reply = Result<Vec<MeetingSummary>, String>;

    async fn handle(&mut self, m: ListMeetings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .list(m.limit.unwrap_or(100))
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
        let session = self.store.manifest(m.meeting).ok().map(|m| m.session);
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
// Plugin

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
        if !cfg!(target_os = "macos") {
            tracing::info!("meetings need macOS 15 (Core Audio taps); not starting");
            return Ok(());
        }
        let store = match Store::open(&rt.data_dir).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!("meeting unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = Meeting::spawn_with_mailbox(
            Meeting {
                rt: rt.clone(),
                cfg,
                store,
                current: None,
                starting: None,
                audio_capture: PermissionState::NotDetermined,
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(MEETING_ACTOR)?;
        Ok(())
    }
}
