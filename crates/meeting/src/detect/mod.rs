//! Meeting detection (158 §4): which app holds the microphone (`sensor.rs`), the rules engine
//! that turns a sustained hold into one `MeetingEvent::Detected` prompt (`rules.rs`), and the
//! glue that runs both inside the meeting actor: the poll task, the actor messages, theuth's own
//! voice activity (quiet, §4.6), and the `meeting_detection*` RPC delegates.

pub mod rules;
pub mod sensor;

use std::collections::HashSet;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::calendar::{self, EventId, EventMatch};
use crate::{MEETING_ACTOR, Meeting, MeetingEvent, MeetingId, ProcessInfo};

/// `meeting.detect.*` (158 §6.1, §4.3). Nested in [`crate::MeetingConfig`]; every key defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DetectConfig {
    /// Run the mic sensor and the rules engine at all.
    pub enabled: bool,
    /// App ids (§4.3: bundle id on macOS, image name on Windows, `application.name` on Linux)
    /// that never prompt. "Never for <App>" appends here.
    pub ignore: Vec<String>,
    /// Sensor tick in seconds (§4.2: 2, 5 on Linux).
    pub poll_secs: u32,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ignore: DEFAULT_IGNORE.iter().map(|s| (*s).to_owned()).collect(),
            poll_secs: if cfg!(target_os = "linux") { 5 } else { 2 },
        }
    }
}

/// §4.3's deny-list: theuth's own processes, dictation apps, screen recorders, voice assistants.
/// Matched case-insensitively against [`rules::app_id`]: macOS bundle ids
/// (`bundle-companion.sh`, `tauri.conf.json`, vendor MDM docs / app catalogs), Windows `<stem>.exe`
/// (`AssemblyName`, `mainBinaryName`, vendor docs), Linux `application.name`. The engine's own pid
/// is skipped by the sensor.
pub const DEFAULT_IGNORE: &[&str] = &[
    // theuth itself (§18 contradiction 2: all three processes)
    "io.theuth.mac.companion",
    "io.theuth.mac",
    "Theuth.Companion.exe",
    "theuth-desktop.exe",
    "theuth-desktop",
    // voice assistants
    "com.apple.CoreSpeech",
    "com.apple.Siri",
    // dictation apps
    "com.apple.VoiceMemos",
    "com.electron.wispr-flow",
    "Wispr Flow.exe",
    "Wispr Flow Helper.exe",
    "com.superwhisper.macOS",
    "com.prakashjoshipax.VoiceInk",
    "com.goodsnooze.MacWhisper",
    // screen recorders
    "com.obsproject.obs-studio",
    "obs64.exe",
    "OBS Studio",
    "com.loom.desktop",
    "Loom.exe",
    "so.cap.desktop",
    "Cap.exe",
    "com.timpler.screenstudio",
    "pl.maketheweb.cleanshotx",
    "com.wulkano.kap",
];

/// Why a `Detected` prompt went away (§4.4 rules 5 and 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Cleared {
    /// The user said "Not now": a 10-minute cooldown for that app.
    NotNow,
    /// The user said "Never for <App>": the app id joined `meeting.detect.ignore`.
    Never,
    /// The app released the mic for 30 s with nobody answering.
    Released,
    /// A meeting started (from the prompt or by hand).
    Started,
}

/// The current prompt (§4.5 `Detected`, §15.1): what `meeting_detection` returns and what
/// `Status.detection` carries so a companion connecting mid-prompt can render it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Detection {
    /// The longest-held eligible app.
    pub app: ProcessInfo,
    /// Its §4.3 app id.
    pub app_id: String,
    /// RFC 3339: when the hold began.
    pub since: String,
    /// Other eligible holders at the same time (§4.4 rule 4).
    #[serde(default)]
    pub also: Vec<ProcessInfo>,
    /// The calendar match, once W2 fills it.
    #[serde(default)]
    pub event: Option<EventMatch>,
    /// The remembered or default project, once W3 fills it.
    #[serde(default)]
    pub project: Option<String>,
    /// The cached pre-meeting brief bullets, once W3 fills it.
    #[serde(default)]
    pub brief: Option<Vec<String>>,
}

/// How a meeting was started (`StartMeeting.source`, §15.1). Exported to TS as `StartSource`:
/// `voice::Source` already owns `Source.ts` in the flat bindings directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "StartSource"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// The start form or an RPC with no source.
    Manual,
    /// The detection prompt's "Start recording".
    Detected {
        /// The prompted app's id.
        app_id: String,
    },
    /// An `Upcoming` prompt or an event picked by hand.
    Calendar {
        /// The event.
        event: EventId,
    },
}

/// The actor's detection state: the engine, theuth's own voice activity (§4.4 rule 3) and the
/// sensor task (§4.2: runs only while `meeting.detect.enabled`).
pub(crate) struct Detector {
    pub(crate) engine: rules::Engine,
    dictating: HashSet<voice::DictationId>,
    talking: bool,
    poll: Option<tokio::task::JoinHandle<()>>,
}

impl Detector {
    pub(crate) fn new(cfg: &DetectConfig) -> Self {
        Self {
            engine: rules::Engine::new(cfg.ignore.clone()),
            dictating: HashSet::new(),
            talking: false,
            poll: None,
        }
    }

    /// Apply `cfg`: the ignore list, and the sensor loop started, restarted or stopped.
    pub(crate) fn apply(&mut self, cfg: &DetectConfig, me: &ActorRef<Meeting>) {
        if let Some(h) = self.poll.take() {
            h.abort();
        }
        self.engine.ignore = cfg.ignore.clone();
        if cfg.enabled {
            self.poll = Some(spawn_poll(me.clone(), cfg.poll_secs));
        }
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        if let Some(h) = self.poll.take() {
            h.abort();
        }
    }
}

/// §4.2: every `poll_secs` call [`sensor::mic_holders`] off the runtime and hand the list to the
/// actor; a tick that finds the previous call still running is skipped (`Skip`, and the
/// `await` serialises them). Ends when the actor is gone.
fn spawn_poll(me: ActorRef<Meeting>, poll_secs: u32) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(u64::from(poll_secs.max(1))));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match tokio::task::spawn_blocking(sensor::mic_holders).await {
                Ok(Ok(holders)) => {
                    if me.tell(Holders(holders)).await.is_err() {
                        return;
                    }
                }
                Ok(Err(e)) => tracing::debug!("meeting: mic sensor: {e:#}"),
                Err(e) => tracing::warn!("meeting: mic sensor task: {e}"),
            }
        }
    })
}

/// `ask` → `bool`: `meeting_detection_set_event` (§8.2) re-matched the open prompt of `app_id`;
/// the engine's copy follows so `meeting_detection` / `Status.detection` carry the pick (and,
/// 158 §9 W3, the project and brief that follow it).
#[derive(Clone, Debug)]
pub struct SetPromptEvent {
    /// The prompted app.
    pub app_id: String,
    /// The re-match.
    pub event: Option<EventMatch>,
    /// The remembered project for the pick.
    pub project: Option<String>,
    /// The cached brief for the pick.
    pub brief: Option<Vec<String>>,
}

impl Message<SetPromptEvent> for Meeting {
    type Reply = bool;

    async fn handle(&mut self, m: SetPromptEvent, _: &mut Context<Self, Self::Reply>) -> bool {
        let hit = self.detect.engine.set_event(&m.app_id, m.event);
        if let Some(d) = self.detect.engine.prompt_mut().filter(|d| d.app_id == m.app_id) {
            d.project = m.project;
            d.brief = m.brief;
        }
        hit
    }
}

/// One sensor tick (`tell`): the processes holding an input stream right now.
#[derive(Clone, Debug)]
pub struct Holders(pub Vec<ProcessInfo>);

/// `ask` → `Option<Detection>`: the open prompt.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentDetection;

/// `ask` → `Result<(), String>`: "Not now" or, with `never`, "Never for <App>" (persisted to
/// `meeting.detect.ignore`; a failed write changes nothing).
#[derive(Clone, Debug)]
pub struct Dismiss {
    /// The app id (§4.3).
    pub app_id: String,
    /// Append to the ignore list instead of a cooldown.
    pub never: bool,
}

/// `tell` at plugin start: run the detector per `cfg` (it needs the actor's own ref).
#[derive(Clone, Debug)]
pub(crate) struct Configure(pub(crate) DetectConfig);

impl Message<Configure> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: Configure, ctx: &mut Context<Self, Self::Reply>) {
        self.detect.apply(&m.0, ctx.actor_ref());
    }
}

impl Meeting {
    /// §4.4 rule 3: a meeting is live, or theuth's own dictation or talk holds a mic.
    fn quiet(&self) -> bool {
        self.current.is_some()
            || self.starting.is_some()
            || !self.detect.dictating.is_empty()
            || self.detect.talking
    }

    /// Publish what the engine produced (§4.5). The ONE calendar seam (§8): a new prompt is
    /// matched against the event store here, a cleared prompt forgets the user's pick.
    pub(crate) async fn publish_detect(&mut self, out: Vec<rules::Output>) {
        for o in out {
            let ev = match o {
                rules::Output::Detected(mut d) => {
                    d.event = calendar::match_for(&self.store, chrono::Utc::now(), &d.app_id).await;
                    // 158 §9.3 / §9.4 (W3): the project chip and a cached brief, from memory.
                    calendar::prebrief::decorate(&self.store, self.cfg.project.as_deref(), &mut d)
                        .await;
                    self.detect.engine.set_event(&d.app_id, d.event.clone());
                    if let Some(p) = self.detect.engine.prompt_mut().filter(|p| p.app_id == d.app_id) {
                        p.project = d.project.clone();
                        p.brief = d.brief.clone();
                    }
                    MeetingEvent::Detected(*d)
                }
                rules::Output::Cleared { app_id, reason } => {
                    calendar::matcher::clear_choice();
                    MeetingEvent::DetectionCleared { app_id, reason }
                }
                rules::Output::AppReleased {
                    meeting,
                    app,
                    released_for_s,
                } => MeetingEvent::AppReleased {
                    meeting,
                    app,
                    released_for_s,
                },
            };
            self.rt.publish(ev).await;
        }
    }

    /// A meeting was accepted for start: the prompt clears (`Started`) and a one-tap start's
    /// app id is remembered for [`Meeting::detect_recording`].
    pub(crate) async fn detect_starting(&mut self, source: Option<&Source>) {
        let app = match source {
            Some(Source::Detected { app_id }) => Some(app_id.clone()),
            _ => None,
        };
        let out = self.detect.engine.starting(app);
        self.publish_detect(out).await;
    }

    /// The meeting records `process` (rule 7).
    pub(crate) fn detect_recording(&mut self, meeting: MeetingId, process: &ProcessInfo) {
        self.detect
            .engine
            .recording(Instant::now(), meeting, process);
    }

    /// The meeting ended or never began.
    pub(crate) fn detect_ended(&mut self) {
        self.detect.engine.ended(Instant::now());
    }

    /// `Never for <App>`: the list on disk, then the engine's.
    fn persist_ignore(&self) -> Result<(), String> {
        let list = serde_json::to_string(&self.detect.engine.ignore).map_err(|e| e.to_string())?;
        runtime::write_nested_config_value(
            &self.rt.data_dir.join("config.yaml"),
            "meeting",
            "detect",
            "ignore",
            &list,
        )
        .map_err(|e| format!("config.yaml: {e}"))
    }
}

impl Message<Holders> for Meeting {
    type Reply = ();

    async fn handle(&mut self, h: Holders, _: &mut Context<Self, Self::Reply>) {
        let quiet = self.quiet();
        let out = self.detect.engine.tick(Instant::now(), &h.0, quiet);
        self.publish_detect(out).await;
    }
}

impl Message<CurrentDetection> for Meeting {
    type Reply = Option<Detection>;

    async fn handle(
        &mut self,
        _: CurrentDetection,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.detect.engine.prompt().cloned()
    }
}

impl Message<Dismiss> for Meeting {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Dismiss, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.never {
            // The write first: a failed one leaves the engine (and the prompt) as they were.
            let mut list = self.detect.engine.ignore.clone();
            if !list.iter().any(|i| i.eq_ignore_ascii_case(&m.app_id)) {
                list.push(m.app_id.clone());
            }
            std::mem::swap(&mut self.detect.engine.ignore, &mut list);
            if let Err(e) = self.persist_ignore() {
                self.detect.engine.ignore = list;
                return Err(e);
            }
            self.cfg.detect.ignore = self.detect.engine.ignore.clone();
        }
        let out = self
            .detect
            .engine
            .dismiss(Instant::now(), &m.app_id, m.never);
        self.publish_detect(out).await;
        Ok(())
    }
}

/// §4.6: theuth's own dictation holds a mic (the engine's or the companion's).
impl Message<voice::VoiceEvent> for Meeting {
    type Reply = ();

    async fn handle(&mut self, ev: voice::VoiceEvent, _: &mut Context<Self, Self::Reply>) {
        match ev {
            voice::VoiceEvent::DictationStarted { id } => {
                self.detect.dictating.insert(id);
            }
            voice::VoiceEvent::DictationFinished { id, .. } => {
                self.detect.dictating.remove(&id);
            }
            _ => {}
        }
    }
}

/// §4.6: a talk holds the engine's mic.
impl Message<talk::TalkEvent> for Meeting {
    type Reply = ();

    async fn handle(&mut self, ev: talk::TalkEvent, _: &mut Context<Self, Self::Reply>) {
        match ev {
            talk::TalkEvent::TalkStarted { .. } => self.detect.talking = true,
            talk::TalkEvent::TalkFinished { .. } => self.detect.talking = false,
            _ => {}
        }
    }
}

fn actor() -> anyhow::Result<ActorRef<Meeting>> {
    ActorRef::<Meeting>::lookup(MEETING_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("meetings are disabled"))
}

/// `meeting_detection() -> Option<Detection>` (§15.2, W1).
pub async fn rpc_detection(_rt: &Runtime) -> anyhow::Result<Option<Detection>> {
    actor()?
        .ask(CurrentDetection)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// `meeting_detection_dismiss(app_id, never)` (§15.2, W1): "Not now" (cooldown) or "Never for"
/// (append to `meeting.detect.ignore`).
pub async fn rpc_detection_dismiss(
    _rt: &Runtime,
    app_id: String,
    never: bool,
) -> anyhow::Result<()> {
    actor()?
        .ask(Dismiss { app_id, never })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}
