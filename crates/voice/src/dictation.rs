//! Live dictation (54 §4.3–4.5, 153 §4.6/§7/§8): the [`Dictation`] actor owns
//! the streaming Nemotron session ([`Stream`]) and the VAD, loads both on
//! [`Prewarm`], and per dictation runs one blocking task that reads the audio
//! source — the daemon's microphone, or PCM a remote client streams in with
//! [`PushAudio`] — gates it with the VAD and feeds the model only between onset
//! and close (02-live §2). Every text piece goes through the shaper (153 §4);
//! spoken end commands stop the dictation or drop an utterance (§7); a lost
//! mic re-arms on the preference list (§8.2); a silent or dead device is
//! rejected within half a second (§8.3); the capture stays warm after a
//! dictation (§8.4); timings per dictation (§8.7) and the history store (§6).
//!
//! ponytail: the `Stream` (≈0.9 GB int8) is never idle-unloaded — `reset()`
//! between dictations, dropped only after a model failure so the next
//! `Prewarm` reloads. Add `voice.dictation_idle_secs` if RSS complaints arrive.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::dictionary::{Dictionary, Entry, EntryId, Kind, Negative, Scope, Source, Variant};
use crate::history::{HistoryRow, Store, SuggestionAnswer as Answer, SuggestionId};
use crate::live::{MODEL_RATE, Text, Utterer};
use crate::mic::{Capture, Chunk, RATE};
use crate::shape::{EndCommand, EndPhrases, Preset, ShapeConfig, ShapePresets, Shaped, Shaper};
use crate::{
    AppProfile, Destination, DeviceState, DictationId, DictationOpts, DictationTimings,
    FinishReason, HistoryConfig, ModelState, Surface, VoiceConfig, VoiceEvent,
};

/// Chunks per [`VoiceEvent::DictationLevel`] (160 ms) — also §8.3's level window.
const LEVELS: usize = 8;
/// How long the task waits for an audio chunk before rechecking cancel and deadline.
const POLL: Duration = Duration::from_millis(50);
/// §7.3: after `Send` arms, no new speech for this long …
pub const SETTLE: Duration = Duration::from_millis(1500);
/// §7.3: … then this much closed-gate silence stops the dictation.
pub const SILENCE: Duration = Duration::from_millis(350);
/// §8.3: a capture that yields no PCM for this long is not ready.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// §8.3: consecutive 160 ms windows at or under the thresholds that mark a dead device.
const SILENT_WINDOWS: u32 = 3;
/// §8.3 (A:L27).
const SILENT_RMS: f32 = 1e-5;
/// §8.3 (A:L27).
const SILENT_PEAK: f32 = 5e-5;
/// §8.2: how often the preference list is retried after "device lost" …
const REARM_RETRY: Duration = Duration::from_millis(500);
/// §8.2: … and for how long before the dictation ends.
const REARM_MAX: Duration = Duration::from_secs(5);
/// Source chunks kept while no utterance is open, prepended to the history
/// audio so the gate's pre-roll is on the recording (§6.2 `PCM_RETENTION`).
const TAIL_CHUNKS: usize = 25;
/// Dictation → surface pairs remembered for `Correction` (§6.3's card names the surface).
const RECENT: usize = 32;

/// Load the streaming model and the VAD if not loaded; never opens the mic.
/// Replies with the current state (`Loading` while in flight).
#[derive(Clone, Copy, Debug)]
pub struct Prewarm;

/// Take the resident [`Utterer`] (76 §0.10): a talk drives it instead of a
/// dictation. Loads it first if it is not loaded (the [`Prewarm`] path).
/// Errors: "already dictating" while a dictation runs, "talking" while
/// already leased, a model load failure. Give it back with [`Return`].
#[derive(Clone, Copy, Debug)]
pub struct Lease;

/// Hand the leased [`Utterer`] back; dictation works again. `None`: the
/// model died under the lease — the slot is freed and the model reloads on
/// the next `Lease`/`StartDictation`, as after a dictation's own failure.
pub struct Return(pub Option<Box<Utterer>>);

/// An asker parked until the load finishes.
enum Waiter {
    Start(
        AudioSource,
        DictationOpts,
        ReplySender<Result<DictationId, String>>,
    ),
    Lease(ReplySender<Result<Box<Utterer>, String>>),
}

/// Where a dictation's audio comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AudioSource {
    /// The daemon's own microphone (54 §0.2).
    Mic,
    /// The client streams PCM with [`PushAudio`]; mono i16 at `sample_rate` Hz.
    Remote {
        /// Sample rate of the pushed PCM; a multiple of 50 (20 ms chunks), 8–192 kHz.
        sample_rate: u32,
    },
}

/// Start dictating from `source`. Errors: "already dictating", a bad remote
/// `sample_rate`, a model load failure, a mic open failure. Waits for a load in
/// flight (or starts one) rather than failing. `opts` default = today's
/// behaviour: `Surface::Composer`, `Destination::AgentPrompt`, the surface's preset.
#[derive(Clone, Debug)]
pub struct StartDictation {
    /// Where the audio comes from.
    pub source: AudioSource,
    /// Surface, destination, app row, press anchor (153 §14.1).
    pub opts: DictationOpts,
}

/// Mono i16 PCM for a [`AudioSource::Remote`] dictation, at the `sample_rate`
/// given at start. Any length; the actor rechunks. Errors: "not dictating"
/// (unknown or finished id), "not a remote dictation".
#[derive(Clone, Debug)]
pub struct PushAudio {
    /// The dictation the audio belongs to.
    pub id: DictationId,
    /// The samples.
    pub pcm: Vec<i16>,
}

/// Stop `id` (idempotent; unknown id → `Ok`). The open utterance is flushed
/// first; [`VoiceEvent::DictationFinished`] follows the reply.
#[derive(Clone, Copy, Debug)]
pub struct StopDictation {
    /// Dictation to stop.
    pub id: DictationId,
}

/// What is loaded and what is running.
#[derive(Clone, Copy, Debug)]
pub struct DictationStatus;

/// Reply to [`DictationStatus`].
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DictationState {
    /// Streaming model lifecycle.
    pub model: ModelState,
    /// The dictation running, if any (one per daemon: one mic).
    pub active: Option<DictationId>,
    /// The input device the running dictation opened (153 §8.1).
    #[serde(default)]
    pub input_device: Option<String>,
    /// Timings of the last finished dictation (153 §8.7).
    #[serde(default)]
    pub last: Option<crate::DictationTimings>,
}

/// `dictation_correction` (153 §6.1): the surface sent `sent_text` for
/// dictation `id`; the store diffs, and every due suggestion is published as
/// [`VoiceEvent::DictionarySuggestion`]. Errors: the store's.
#[derive(Clone, Debug)]
pub struct Correction {
    /// The dictation whose text was edited.
    pub id: DictationId,
    /// What actually went out.
    pub sent_text: String,
}

/// `dictionary_suggestion_answer` (153 §6.3 step 4). `Add` writes a learned
/// entry with the heard variant, `Never` a negative, `NotNow` only the store's
/// cooldown. Errors: "unknown suggestion".
#[derive(Clone, Copy, Debug)]
pub struct AnswerSuggestion {
    /// The suggestion the card showed.
    pub id: SuggestionId,
    /// The user's answer.
    pub answer: Answer,
}

/// The dictionary RPCs (153 §5.6); the actor owns the mutable dictionary and
/// re-snapshots it for the next dictation after every change.
#[derive(Clone, Debug)]
pub enum DictionaryOp {
    /// User + learned entries, optionally one scope.
    List(Option<Scope>),
    /// Upsert by canonical + scope.
    Put(Entry),
    /// Delete by id.
    Delete(EntryId),
    /// Record a veto.
    Negative(Negative),
}

/// Reply to [`DictionaryOp`], one variant per op.
#[derive(Clone, Debug, kameo::Reply)]
pub enum DictionaryReply {
    /// `List`.
    Entries(Vec<Entry>),
    /// `Put`: the stored entry.
    Entry(Entry),
    /// `Delete`: whether an entry went.
    Deleted(bool),
    /// `Negative`.
    Done,
}

/// A `voice.*` key changed through its RPC; the next dictation uses it
/// (the config file is written by the RPC layer).
#[derive(Clone, Debug)]
pub enum Setting {
    /// `voice.shape.<surface>`.
    Shape(Surface, Preset),
    /// `voice.end_commands`.
    EndPhrases(EndPhrases),
    /// `voice.input_devices`; a standby capture is dropped so the next open walks the new list.
    InputDevices(Vec<String>),
}

/// The live `voice.input_devices` list (the companions read it from `voice_status`).
#[derive(Clone, Copy, Debug)]
pub struct InputDevicePrefs;

/// The history store (153 §6.5) for `dictation_history*` / `dictation_audio_get`;
/// `None` when it failed to open or history is disabled.
#[derive(Clone, Copy, Debug)]
pub struct HistoryStore;

/// What the task drives: the live utterer, or a scripted stand-in under test.
// The size gap only exists in test builds (the fake is small); one engine per actor.
#[cfg_attr(test, allow(clippy::large_enum_variant))]
enum Engine {
    Live(Utterer),
    #[cfg(test)]
    Fake(tests::Fake),
}

impl Engine {
    fn set_rate(&mut self, rate: u32) -> anyhow::Result<()> {
        match self {
            Engine::Live(u) => u.set_rate(rate),
            #[cfg(test)]
            Engine::Fake(_) => Ok(()),
        }
    }

    fn chunk(&mut self, pcm: &[f32]) -> anyhow::Result<Vec<Text>> {
        match self {
            Engine::Live(u) => u.chunk(pcm),
            #[cfg(test)]
            Engine::Fake(f) => Ok(f.chunk(pcm)),
        }
    }

    fn close(&mut self) -> anyhow::Result<Vec<Text>> {
        match self {
            Engine::Live(u) => u.close(),
            #[cfg(test)]
            Engine::Fake(f) => Ok(f.close()),
        }
    }

    fn reset(&mut self) {
        match self {
            Engine::Live(u) => u.reset(),
            #[cfg(test)]
            Engine::Fake(_) => {}
        }
    }

    fn is_open(&self) -> bool {
        match self {
            Engine::Live(u) => u.is_open(),
            #[cfg(test)]
            Engine::Fake(f) => f.open,
        }
    }
}

/// What `Capture::open_on` returns: the capture, its chunks, the device name.
type Opened = (Capture, Receiver<Chunk>, String);
/// Opens the mic on a preference list; the task re-arms through it too. Tests inject a feed.
type Opener = Arc<dyn Fn(&[String]) -> anyhow::Result<Opened> + Send + Sync>;

/// The audio a task reads: `cap` keeps a mic alive (`None` for a remote source).
struct Feed {
    cap: Option<Capture>,
    rx: Receiver<Chunk>,
}

/// The dictation in progress.
struct Running {
    id: DictationId,
    cancel: CancellationToken,
    /// The feed for a remote source; `None` when the task reads the mic.
    remote: Option<Remote>,
    /// The mic that opened (updated on re-arm); `None` for a remote source.
    device: Option<String>,
    /// The `dictations` row so far (§6.5); text and timings land at `Finished`.
    row: HistoryRow,
}

/// The actor's end of a remote dictation's audio: pushed PCM is rechunked into
/// 20 ms chunks for the task, a partial tail waiting for the next push.
struct Remote {
    tx: Sender<Chunk>,
    pending: Vec<f32>,
    /// Samples per chunk: `sample_rate / 50`.
    chunk: usize,
}

/// §8.4: the mic kept open after a dictation so a re-press starts at once.
struct Standby {
    cap: Capture,
    rx: Receiver<Chunk>,
    device: String,
    generation: u64,
}

/// A suggestion card out there, waiting for its answer (§6.3).
struct Pending {
    heard: String,
    typed: String,
}

/// The `voice.*` keys this actor reads, live (the RPC setters update them).
struct Settings {
    shape: ShapePresets,
    phrases: EndPhrases,
    input_devices: Vec<String>,
    standby: Duration,
    history: HistoryConfig,
}

/// `tell` from the load task.
struct Loaded(Result<Engine, String>);

/// `tell` from the standby timer; dropped if a newer standby took over.
struct StandbyExpired(u64);

/// What the task tells the actor; each becomes a [`VoiceEvent`].
enum Progress {
    /// The audio source is open and (for the mic) delivered its first chunk.
    Started,
    /// RMS per 20 ms chunk, oldest first.
    Level(Vec<f32>),
    /// Utterance text so far, shaped.
    Text {
        utterance: u32,
        text: String,
        shaped: String,
        command: Option<EndCommand>,
        actions: Vec<crate::Action>,
        is_final: bool,
    },
    /// The mic was lost, or re-armed on `device` (§8.2).
    Device {
        device: Option<String>,
        state: DeviceState,
    },
    /// One closed utterance's audio, 16 kHz i16, for the history store.
    Audio { utterance: u32, pcm: Vec<i16> },
    /// The task ended. `None` engine: lost to a model failure; the next `Prewarm` reloads.
    /// Boxed: the engine is ~700 bytes and every other variant is small.
    Finished(Box<Done>),
}

/// Everything a finished task hands back.
struct Done {
    reason: FinishReason,
    engine: Option<Engine>,
    /// The mic, still open, for standby; `None` for a remote source or a mic error.
    standby: Option<Opened>,
    timings: DictationTimings,
    /// When the stop was observed (cancel, deadline, or the settle), for `stop_to_final_ms`.
    stop_at: Option<Instant>,
    /// Finals joined, raw and shaped.
    raw: String,
    shaped: String,
}

/// `tell` to the actor from the task.
struct Report {
    id: DictationId,
    progress: Progress,
}

/// The [`crate::DICTATION_ACTOR`] actor (54 §4.3).
#[derive(kameo::Actor)]
pub struct Dictation {
    rt: Runtime,
    dir: PathBuf,
    /// A dictation stops itself after this long.
    max: Duration,
    model: ModelState,
    /// `None` while loading, while a dictation task owns it, or while leased.
    engine: Option<Engine>,
    /// A talk holds the engine ([`Lease`]); `StartDictation` answers "talking".
    leased: bool,
    /// Askers parked until the load finishes.
    waiters: Vec<Waiter>,
    active: Option<Running>,
    cfg: Settings,
    opener: Opener,
    /// The history store and the dictionary were opened (once, on first use).
    stores_opened: bool,
    store: Option<Store>,
    dictionary: Dictionary,
    /// Read-only copy handed to each dictation; rebuilt after every change.
    snapshot: Arc<Dictionary>,
    standby: Option<Standby>,
    standby_generation: u64,
    /// §8.7: the last finished dictation's numbers.
    last: Option<DictationTimings>,
    pending: HashMap<SuggestionId, Pending>,
    /// Recent dictations' surfaces, for the suggestion card.
    recent: VecDeque<(DictationId, Surface)>,
}

impl Dictation {
    pub(crate) fn new(rt: Runtime, dir: PathBuf, max: Duration) -> Self {
        let cfg: VoiceConfig = rt.section("voice").unwrap_or_default();
        Self {
            rt,
            dir,
            max,
            model: ModelState::Missing,
            engine: None,
            leased: false,
            waiters: Vec::new(),
            active: None,
            cfg: Settings {
                shape: cfg.shape,
                phrases: cfg.end_commands,
                input_devices: cfg.input_devices,
                standby: Duration::from_secs(cfg.standby_secs),
                history: cfg.history,
            },
            opener: Arc::new(Capture::open_on),
            stores_opened: false,
            store: None,
            dictionary: Dictionary::default(),
            snapshot: Arc::new(Dictionary::default()),
            standby: None,
            standby_generation: 0,
            last: None,
            pending: HashMap::new(),
            recent: VecDeque::new(),
        }
    }

    async fn set_model(&mut self, state: ModelState) {
        self.model = state.clone();
        self.rt.publish(VoiceEvent::DictationModel { state }).await;
    }

    /// Open the history store and load the dictionary, once (153 §5/§6); a
    /// failure logs and leaves that half off — dictation itself never waits on it.
    async fn open_stores(&mut self) {
        if self.stores_opened {
            return;
        }
        self.stores_opened = true;
        let data_dir = self.rt.data_dir.clone();
        if self.cfg.history.enabled {
            match Store::open(&data_dir).await {
                Ok(s) => self.store = Some(s),
                Err(e) => tracing::warn!("voice: history store: {e:#}"),
            }
        }
        let dir = data_dir.clone();
        match tokio::task::spawn_blocking(move || Dictionary::load(&dir)).await {
            Ok(Ok(d)) => {
                self.dictionary = d;
                self.snapshot = self.dictionary.snapshot();
            }
            Ok(Err(e)) => tracing::warn!("voice: dictionary: {e:#}"),
            Err(e) => tracing::warn!("voice: dictionary load panicked: {e}"),
        }
    }

    /// Load off the runtime unless loaded or loading; the result comes back as [`Loaded`].
    async fn load(&mut self, me: ActorRef<Self>) {
        if !matches!(self.model, ModelState::Missing | ModelState::Failed { .. }) {
            return;
        }
        self.set_model(ModelState::Loading).await;
        let (dir, data_dir) = (self.dir.clone(), self.rt.data_dir.clone());
        tokio::spawn(async move {
            let t = Instant::now();
            let r = tokio::task::spawn_blocking(move || Utterer::open(&dir, &data_dir))
                .await
                .unwrap_or_else(|e| {
                    Err(anyhow::anyhow!("dictation model load task panicked: {e}"))
                });
            if r.is_ok() {
                tracing::info!("voice: dictation model loaded in {:?}", t.elapsed());
            }
            let _ = me
                .tell(Loaded(r.map(Engine::Live).map_err(|e| format!("{e:#}"))))
                .await;
        });
    }

    /// [`Self::begin`] traced: every start, refusal and failure at INFO with the source — the companion's 2026-10-09 voice-key
    /// report found nothing of the dictation path in the engine log (149f). The peer is the RPC layer's; it is not known here.
    fn start(
        &mut self,
        me: ActorRef<Self>,
        source: AudioSource,
        opts: DictationOpts,
    ) -> Result<DictationId, String> {
        let surface = opts.surface.unwrap_or_default();
        let r = self.begin(me, source, opts);
        match &r {
            Ok(id) => {
                tracing::info!(
                    "voice: dictation started id={id} source={source:?} surface={surface:?}"
                )
            }
            Err(e) => tracing::info!("voice: dictation refused source={source:?} reason={e}"),
        }
        r
    }

    /// §8.4: the warm mic, with the chunks it buffered meanwhile dropped;
    /// `None` when there is none or it failed while waiting.
    fn take_standby(&mut self) -> Option<Opened> {
        let s = self.standby.take()?;
        loop {
            match s.rx.try_recv() {
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => break,
            }
        }
        Some((s.cap, s.rx, s.device))
    }

    /// Open the audio source and start the task on the loaded engine.
    /// `source` was validated by the `StartDictation` handler.
    fn begin(
        &mut self,
        me: ActorRef<Self>,
        source: AudioSource,
        opts: DictationOpts,
    ) -> Result<DictationId, String> {
        if self.active.is_some() {
            return Err("already dictating".into());
        }
        if self.leased {
            return Err("talking".into());
        }
        let Some(mut engine) = self.engine.take() else {
            return Err("dictation model not loaded".into());
        };
        let recv = Instant::now();
        let rate = match source {
            AudioSource::Mic => RATE,
            AudioSource::Remote { sample_rate } => sample_rate,
        };
        let started = engine
            .set_rate(rate)
            .map_err(|e| format!("resampler: {e}"))
            .and_then(|()| match source {
                AudioSource::Mic => self
                    .take_standby()
                    .map(Ok)
                    .unwrap_or_else(|| (self.opener)(&self.cfg.input_devices))
                    .map(|(cap, rx, device)| (Feed { cap: Some(cap), rx }, None, Some(device)))
                    .map_err(|e| format!("microphone: {e:#}")),
                AudioSource::Remote { .. } => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    Ok((
                        Feed { cap: None, rx },
                        Some(Remote {
                            tx,
                            pending: Vec::new(),
                            chunk: rate as usize / 50,
                        }),
                        None,
                    ))
                }
            });
        let (feed, remote, device) = match started {
            Ok(s) => s,
            Err(e) => {
                self.engine = Some(engine);
                return Err(e);
            }
        };
        let id = DictationId::new();
        let cancel = CancellationToken::new();
        let surface = opts.surface.unwrap_or_default();
        let destination = opts.destination.unwrap_or(Destination::AgentPrompt);
        let app = match (&destination, &opts.app) {
            (Destination::AnyApp { app }, _) => Some(app.clone()),
            (_, Some(p)) => Some(p.id.clone()),
            _ => None,
        };
        self.active = Some(Running {
            id,
            cancel: cancel.clone(),
            remote,
            device: device.clone(),
            row: HistoryRow {
                id,
                started_ms: unix_ms(),
                finished_ms: None,
                surface,
                destination: match &destination {
                    Destination::AgentPrompt => "agent_prompt".into(),
                    Destination::Document => "document".into(),
                    Destination::MessageReply => "message_reply".into(),
                    Destination::AnyApp { app } => format!("any_app:{app}"),
                },
                app,
                raw: String::new(),
                shaped: String::new(),
                polished: None,
                delivered: None,
                press_ms: None,
                first_partial_ms: None,
                first_final_ms: None,
                final_ms: None,
                reason: String::new(),
                audio_bytes: 0,
                has_audio: false,
            },
        });
        self.recent.push_back((id, surface));
        if self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        let press = opts
            .press_ago_ms
            .and_then(|ms| recv.checked_sub(Duration::from_millis(u64::from(ms))))
            .unwrap_or(recv);
        let task = Task {
            me: me.clone(),
            id,
            live: engine,
            rate,
            levels: Vec::with_capacity(LEVELS),
            shaper: ShaperOwned {
                cfg: self.cfg.shape.get(surface).into(),
                phrases: self.cfg.phrases.clone(),
                dict: self.snapshot.clone(),
                app: opts.app,
            },
            prev: String::new(),
            finals: Vec::new(),
            utterance: None,
            armed: None,
            send_at: None,
            quiet_since: None,
            press,
            first_pcm: None,
            first_partial: None,
            first_final: None,
            surface,
            opener: self.opener.clone(),
            prefs: self.cfg.input_devices.clone(),
            device,
            tried_next: false,
            heard: false,
            peak: 0.0,
            silent_windows: 0,
            record_audio: self.store.is_some(),
            tail: VecDeque::new(),
            utt_audio: Vec::new(),
            was_open: false,
        };
        let max = self.max;
        tokio::spawn(async move {
            let done = tokio::task::spawn_blocking(move || task.run(feed, cancel, max))
                .await
                .unwrap_or_else(|e| Done {
                    reason: FinishReason::Error {
                        message: format!("dictation task panicked: {e}"),
                    },
                    engine: None,
                    standby: None,
                    timings: DictationTimings {
                        surface,
                        press_to_first_pcm_ms: None,
                        press_to_first_partial_ms: None,
                        press_to_first_final_ms: None,
                        stop_to_final_ms: None,
                        polish_ms: None,
                        device: None,
                    },
                    stop_at: None,
                    raw: String::new(),
                    shaped: String::new(),
                });
            let _ = me
                .tell(Report {
                    id,
                    progress: Progress::Finished(Box::new(done)),
                })
                .await;
        });
        Ok(id)
    }

    /// Hand the engine to a talk, if nothing else holds it.
    fn lease(&mut self) -> Result<Box<Utterer>, String> {
        if self.active.is_some() {
            return Err("already dictating".into());
        }
        if self.leased {
            return Err("talking".into());
        }
        match self.engine.take() {
            Some(Engine::Live(u)) => {
                self.leased = true;
                // A talk opens its own mic (VPIO); the warm one goes.
                self.standby = None;
                Ok(Box::new(u))
            }
            #[cfg(test)]
            Some(fake) => {
                self.engine = Some(fake);
                Err("test engine".into())
            }
            None => Err("dictation model not loaded".into()),
        }
    }

    /// §8.4: keep `opened` warm for `cfg.standby`, then drop it.
    fn keep_warm(&mut self, me: ActorRef<Self>, opened: Opened) {
        if self.cfg.standby.is_zero() {
            return;
        }
        self.standby_generation += 1;
        let generation = self.standby_generation;
        let (cap, rx, device) = opened;
        self.standby = Some(Standby {
            cap,
            rx,
            device,
            generation,
        });
        let standby = self.cfg.standby;
        tokio::spawn(async move {
            tokio::time::sleep(standby).await;
            let _ = me.tell(StandbyExpired(generation)).await;
        });
    }

    /// §6.3 step 3: one card per due pair; the id lives here until answered.
    async fn suggest(&mut self, id: DictationId, due: Vec<crate::Correction>) {
        let surface = self
            .recent
            .iter()
            .find(|(d, _)| *d == id)
            .map_or(Surface::Composer, |(_, s)| *s);
        for c in due {
            let sid = SuggestionId::new();
            self.pending.insert(
                sid,
                Pending {
                    heard: c.heard.clone(),
                    typed: c.typed.clone(),
                },
            );
            self.rt
                .publish(VoiceEvent::DictionarySuggestion {
                    id: sid,
                    heard: c.heard,
                    typed: c.typed,
                    // The store's due rule is 2 in 7 days (§6.2); it reports no count.
                    occurrences: 2,
                    surface,
                    has_audio: false,
                })
                .await;
        }
    }
}

/// Now, Unix ms.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Message<Prewarm> for Dictation {
    type Reply = ModelState;
    async fn handle(&mut self, _: Prewarm, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.open_stores().await;
        self.load(ctx.actor_ref().clone()).await;
        self.model.clone()
    }
}

impl Message<StartDictation> for Dictation {
    type Reply = DelegatedReply<Result<DictationId, String>>;
    async fn handle(
        &mut self,
        m: StartDictation,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let me = ctx.actor_ref().clone();
        if let AudioSource::Remote { sample_rate } = m.source
            && let Err(e) = check_rate(sample_rate)
        {
            tx.send(Err(e));
        } else if self.active.is_some() {
            tracing::info!(
                "voice: dictation refused source={:?} reason=already dictating active={}",
                m.source,
                self.active
                    .as_ref()
                    .map(|r| r.id.to_string())
                    .unwrap_or_default()
            );
            tx.send(Err("already dictating".into()));
        } else if self.leased {
            tracing::info!(
                "voice: dictation refused source={:?} reason=talking",
                m.source
            );
            tx.send(Err("talking".into()));
        } else if self.model == ModelState::Ready {
            self.open_stores().await;
            tx.send(self.start(me, m.source, m.opts));
        } else {
            self.open_stores().await;
            self.load(me).await;
            self.waiters.push(Waiter::Start(m.source, m.opts, tx));
        }
        reply
    }
}

impl Message<Lease> for Dictation {
    type Reply = DelegatedReply<Result<Box<Utterer>, String>>;
    async fn handle(&mut self, _: Lease, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if self.active.is_some() || self.leased || self.model == ModelState::Ready {
            tx.send(self.lease());
        } else {
            self.load(ctx.actor_ref().clone()).await;
            self.waiters.push(Waiter::Lease(tx));
        }
        reply
    }
}

impl Message<Return> for Dictation {
    type Reply = ();
    async fn handle(&mut self, m: Return, _: &mut Context<Self, Self::Reply>) {
        self.leased = false;
        match m.0 {
            Some(e) => self.engine = Some(Engine::Live(*e)),
            None => self.set_model(ModelState::Missing).await,
        }
    }
}

/// A remote `sample_rate` the task can chunk (20 ms = `rate / 50` whole samples) and resample.
fn check_rate(rate: u32) -> Result<(), String> {
    if !rate.is_multiple_of(50) {
        Err("sample_rate must be a multiple of 50".into())
    } else if !(8_000..=192_000).contains(&rate) {
        Err("sample_rate must be between 8000 and 192000".into())
    } else {
        Ok(())
    }
}

impl Message<PushAudio> for Dictation {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: PushAudio, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(r) = self.active.as_mut().filter(|r| r.id == m.id) else {
            return Err("not dictating".into());
        };
        let Some(remote) = r.remote.as_mut() else {
            return Err("not a remote dictation".into());
        };
        remote
            .pending
            .extend(m.pcm.iter().map(|&s| f32::from(s) / 32768.0));
        while remote.pending.len() >= remote.chunk {
            let chunk: Vec<f32> = remote.pending.drain(..remote.chunk).collect();
            // Receiver gone: the task ended and its `Finished` is in flight.
            remote
                .tx
                .send(Ok(chunk))
                .map_err(|_| String::from("not dictating"))?;
        }
        Ok(())
    }
}

impl Message<StopDictation> for Dictation {
    type Reply = Result<(), String>;
    async fn handle(
        &mut self,
        m: StopDictation,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let known = self.active.as_ref().is_some_and(|r| r.id == m.id);
        tracing::info!("voice: dictation stop id={} known={known}", m.id);
        if let Some(r) = self.active.as_ref().filter(|r| r.id == m.id) {
            r.cancel.cancel();
        }
        Ok(())
    }
}

impl Message<DictationStatus> for Dictation {
    type Reply = DictationState;
    async fn handle(
        &mut self,
        _: DictationStatus,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        DictationState {
            model: self.model.clone(),
            active: self.active.as_ref().map(|r| r.id),
            input_device: self.active.as_ref().and_then(|r| r.device.clone()),
            last: self.last.clone(),
        }
    }
}

impl Message<Correction> for Dictation {
    type Reply = Result<(), String>;
    async fn handle(&mut self, m: Correction, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.open_stores().await;
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        let due = store
            .correction(m.id, &m.sent_text)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.suggest(m.id, due).await;
        Ok(())
    }
}

impl Message<AnswerSuggestion> for Dictation {
    type Reply = Result<(), String>;
    async fn handle(
        &mut self,
        m: AnswerSuggestion,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Some(p) = self.pending.remove(&m.id) else {
            return Err("unknown suggestion".into());
        };
        match m.answer {
            Answer::Add => {
                self.dictionary.put(Entry {
                    id: EntryId(uuid::Uuid::now_v7().to_string()),
                    canonical: p.typed.clone(),
                    kind: Kind::Term,
                    spoken: vec![p.heard.clone()],
                    variants: vec![Variant {
                        heard: p.heard,
                        metaphone: (String::new(), String::new()),
                        seen: 1,
                        last_ms: unix_ms(),
                    }],
                    scope: Scope::Global,
                    source: Source::Learned,
                    tags: Vec::new(),
                });
                self.snapshot = self.dictionary.snapshot();
            }
            Answer::Never => {
                self.dictionary.negative(Negative {
                    heard_lc: p.heard.to_lowercase(),
                    canonical: p.typed,
                });
                self.snapshot = self.dictionary.snapshot();
            }
            Answer::NotNow => {}
        }
        if let Some(store) = &self.store {
            store
                .answer(m.id, m.answer)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(())
    }
}

impl Message<DictionaryOp> for Dictation {
    type Reply = DictionaryReply;
    async fn handle(&mut self, m: DictionaryOp, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.open_stores().await;
        let reply = match m {
            DictionaryOp::List(scope) => {
                return DictionaryReply::Entries(self.dictionary.list(scope.as_ref()));
            }
            DictionaryOp::Put(entry) => DictionaryReply::Entry(self.dictionary.put(entry)),
            DictionaryOp::Delete(id) => DictionaryReply::Deleted(self.dictionary.delete(&id)),
            DictionaryOp::Negative(n) => {
                self.dictionary.negative(n);
                DictionaryReply::Done
            }
        };
        self.snapshot = self.dictionary.snapshot();
        reply
    }
}

impl Message<Setting> for Dictation {
    type Reply = ();
    async fn handle(&mut self, m: Setting, _: &mut Context<Self, Self::Reply>) {
        match m {
            Setting::Shape(surface, preset) => match surface {
                Surface::Composer => self.cfg.shape.composer = preset,
                Surface::Tui => self.cfg.shape.tui = preset,
                Surface::Companion => self.cfg.shape.companion = preset,
                Surface::Phone => self.cfg.shape.phone = preset,
                Surface::AnyApp => self.cfg.shape.any_app = preset,
            },
            Setting::EndPhrases(p) => self.cfg.phrases = p,
            Setting::InputDevices(names) => {
                self.cfg.input_devices = names;
                self.standby = None;
            }
        }
    }
}

impl Message<InputDevicePrefs> for Dictation {
    type Reply = Vec<String>;
    async fn handle(
        &mut self,
        _: InputDevicePrefs,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.cfg.input_devices.clone()
    }
}

impl Message<HistoryStore> for Dictation {
    type Reply = Option<Store>;
    async fn handle(&mut self, _: HistoryStore, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.open_stores().await;
        self.store.clone()
    }
}

impl Message<StandbyExpired> for Dictation {
    type Reply = ();
    async fn handle(&mut self, m: StandbyExpired, _: &mut Context<Self, Self::Reply>) {
        if self.standby.as_ref().is_some_and(|s| s.generation == m.0) {
            self.standby = None;
        }
    }
}

impl Message<Loaded> for Dictation {
    type Reply = ();
    async fn handle(&mut self, m: Loaded, ctx: &mut Context<Self, Self::Reply>) {
        match m.0 {
            Ok(engine) => {
                self.engine = Some(engine);
                self.set_model(ModelState::Ready).await;
                let me = ctx.actor_ref().clone();
                // The first waiter gets the slot; the rest hear "already dictating" / "talking".
                for w in std::mem::take(&mut self.waiters) {
                    match w {
                        Waiter::Start(source, opts, tx) => {
                            tx.send(self.start(me.clone(), source, opts))
                        }
                        Waiter::Lease(tx) => tx.send(self.lease()),
                    }
                }
            }
            Err(message) => {
                tracing::warn!("voice: dictation model: {message}");
                for w in self.waiters.drain(..) {
                    match w {
                        Waiter::Start(_, _, tx) => tx.send(Err(message.clone())),
                        Waiter::Lease(tx) => tx.send(Err(message.clone())),
                    }
                }
                let state = if self.dir.is_dir() {
                    ModelState::Failed { message }
                } else {
                    ModelState::Missing
                };
                self.set_model(state).await;
            }
        }
    }
}

impl Message<Report> for Dictation {
    type Reply = ();
    async fn handle(&mut self, m: Report, ctx: &mut Context<Self, Self::Reply>) {
        if !self.active.as_ref().is_some_and(|r| r.id == m.id) {
            return;
        }
        let id = m.id;
        let event = match m.progress {
            Progress::Started => VoiceEvent::DictationStarted { id },
            Progress::Level(levels) => VoiceEvent::DictationLevel { id, levels },
            Progress::Text {
                utterance,
                text,
                shaped,
                command,
                actions,
                is_final,
            } => VoiceEvent::DictationText {
                id,
                utterance,
                text,
                is_final,
                shaped,
                command,
                actions,
            },
            Progress::Device { device, state } => {
                if let Some(r) = self.active.as_mut() {
                    r.device = device.clone();
                }
                VoiceEvent::DictationDevice { id, device, state }
            }
            Progress::Audio { utterance, pcm } => {
                if let Some(store) = self.store.clone() {
                    tokio::spawn(async move {
                        if let Err(e) = store.attach_audio(id, utterance, &pcm).await {
                            tracing::debug!("voice: history audio: {e:#}");
                        }
                    });
                }
                return;
            }
            Progress::Finished(done) => {
                let done = *done;
                tracing::info!("voice: dictation finished id={id} reason={:?}", done.reason);
                let Some(run) = self.active.take() else {
                    return;
                };
                match done.engine {
                    Some(e) => self.engine = Some(e),
                    None => self.set_model(ModelState::Missing).await,
                }
                let mut timings = done.timings;
                timings.stop_to_final_ms = done.stop_at.map(|t| ms(t.elapsed()));
                if let Some(opened) = done.standby {
                    self.keep_warm(ctx.actor_ref().clone(), opened);
                }
                if let Some(store) = self.store.clone() {
                    let mut row = run.row;
                    row.finished_ms = Some(unix_ms());
                    row.raw = done.raw;
                    row.shaped = done.shaped;
                    row.press_ms = timings.press_to_first_pcm_ms;
                    row.first_partial_ms = timings.press_to_first_partial_ms;
                    row.first_final_ms = timings.press_to_first_final_ms;
                    row.final_ms = row
                        .finished_ms
                        .map(|f| f.saturating_sub(row.started_ms) as u32);
                    row.reason = match &done.reason {
                        FinishReason::Done => "done".into(),
                        FinishReason::Stopped => "stopped".into(),
                        FinishReason::Error { message } => format!("error:{message}"),
                    };
                    tokio::spawn(async move {
                        if let Err(e) = store.record(row).await {
                            tracing::debug!("voice: history row: {e:#}");
                        }
                    });
                }
                self.last = Some(timings);
                VoiceEvent::DictationFinished {
                    id,
                    reason: done.reason,
                }
            }
        };
        self.rt.publish(event).await;
    }
}

/// Whole milliseconds, saturating.
fn ms(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// §4.6's `ShaperOwned`: what one dictation shapes with, owned so the task can move.
struct ShaperOwned {
    cfg: ShapeConfig,
    phrases: EndPhrases,
    dict: Arc<Dictionary>,
    app: Option<AppProfile>,
}

impl ShaperOwned {
    fn shape(&self, prev: &str, raw: &str, is_final: bool) -> Shaped {
        #[allow(unused_mut)]
        let mut s = Shaper::new(self.cfg, &self.phrases, &self.dict, self.app.as_ref())
            .shape(prev, raw, is_final);
        // Until A1's parser arms (`shape/end.rs` is a stub at the PIN), the actor
        // tests arm on an exact phrase at the tail; harmless once the parser lands.
        #[cfg(test)]
        if s.command.is_none() && self.cfg.end_commands {
            s.command = tests::end_phrase(&self.phrases, raw);
        }
        s
    }
}

/// One dictation, on its blocking thread (54 §4.5).
struct Task {
    me: ActorRef<Dictation>,
    id: DictationId,
    live: Engine,
    /// Source rate.
    rate: u32,
    /// RMS per chunk, flushed every [`LEVELS`].
    levels: Vec<f32>,
    shaper: ShaperOwned,
    /// Finals so far, shaped and joined: the shaper's context (§4.6).
    prev: String,
    /// Closed utterances kept: `(utterance, raw, shaped)`; "scratch that" pops one.
    finals: Vec<(u32, String, String)>,
    /// The utterance the last piece belonged to; a new one clears `armed`.
    utterance: Option<u32>,
    /// §7.1: once a command arms in an utterance it stays until the final.
    armed: Option<EndCommand>,
    /// §7.3: when the last piece carrying `Send` arrived; `None` once new speech followed.
    send_at: Option<Instant>,
    /// §7.3: since when the gate has been closed after the settle.
    quiet_since: Option<Instant>,
    /// §8.7 anchors.
    press: Instant,
    first_pcm: Option<Instant>,
    first_partial: Option<Instant>,
    first_final: Option<Instant>,
    surface: Surface,
    /// §8.2/§8.3: re-arm and next-device through the same opener the actor used.
    opener: Opener,
    prefs: Vec<String>,
    device: Option<String>,
    /// §8.3: the one next-device step was taken.
    tried_next: bool,
    /// §8.3: a non-silent window arrived; the detector is off.
    heard: bool,
    /// Peak |sample| in the current level window (while `!heard`).
    peak: f32,
    silent_windows: u32,
    /// §6: stream the utterances' audio to the history store.
    record_audio: bool,
    /// Last [`TAIL_CHUNKS`] source chunks while no utterance is open (the pre-roll).
    tail: VecDeque<Vec<f32>>,
    /// Source samples of the open utterance.
    utt_audio: Vec<f32>,
    was_open: bool,
}

impl Task {
    fn report(&self, progress: Progress) -> bool {
        self.me
            .tell(Report {
                id: self.id,
                progress,
            })
            .blocking_send()
            .is_ok()
    }

    fn since_press(&self, t: Option<Instant>) -> Option<u32> {
        t.map(|t| ms(t.saturating_duration_since(self.press)))
    }

    /// Read the source until cancel, deadline, the settle, or failure; flush;
    /// hand the engine (and a healthy mic) back.
    fn run(mut self, mut feed: Feed, cancel: CancellationToken, max: Duration) -> Done {
        let mic = feed.cap.is_some();
        if !mic {
            self.report(Progress::Started);
        }
        let deadline = Instant::now() + max;
        // §8.3: the current device owes its first chunk by then; `None` once it delivered.
        let mut ready_by = mic.then(|| Instant::now() + READY_TIMEOUT);
        let mut stop_at = None;
        // `Err`: the model or resampler failed — the engine is not handed back.
        let reason = loop {
            if cancel.is_cancelled() || !self.me.is_alive() {
                stop_at = Some(Instant::now());
                break Ok(FinishReason::Done);
            }
            if Instant::now() >= deadline {
                stop_at = Some(Instant::now());
                break Ok(FinishReason::Stopped);
            }
            if self.settled() {
                stop_at = Some(Instant::now());
                break Ok(FinishReason::Done);
            }
            // `Disconnected`: the mic thread died. A remote feed cannot disconnect
            // while the actor lives — it holds the `Sender` in `Running` until this
            // task's `Finished` lands — and an actor gone is caught by `is_alive` above.
            match feed.rx.recv_timeout(POLL) {
                Ok(Ok(c)) => {
                    if ready_by.take().is_some() {
                        self.report(Progress::Started);
                    }
                    self.first_pcm.get_or_insert_with(Instant::now);
                    match self.chunk(&c, mic) {
                        Ok(false) => {}
                        Ok(true) => match self.next_device(&mut feed, "no sound from") {
                            Ok(()) => ready_by = Some(Instant::now() + READY_TIMEOUT),
                            Err(message) => break Ok(FinishReason::Error { message }),
                        },
                        Err(e) => break Err(e),
                    }
                }
                Ok(Err(m)) if mic && m == "device lost" => match self.rearm(&mut feed, &cancel) {
                    Ok(true) => ready_by = Some(Instant::now() + READY_TIMEOUT),
                    Ok(false) => {
                        break Ok(FinishReason::Error {
                            message: "microphone: device lost".into(),
                        });
                    }
                    Err(e) => break Err(e),
                },
                Ok(Err(m)) => {
                    break Ok(FinishReason::Error {
                        message: format!("microphone: {m}"),
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    if ready_by.is_some_and(|t| Instant::now() >= t) {
                        match self.next_device(&mut feed, "no audio from") {
                            Ok(()) => ready_by = Some(Instant::now() + READY_TIMEOUT),
                            Err(message) => break Ok(FinishReason::Error { message }),
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break Ok(FinishReason::Error {
                        message: "audio source closed".into(),
                    });
                }
            }
        };
        // The mic stays open through the flush (which may take a second) only
        // for standby; a mic error drops it now.
        let healthy = !matches!(reason, Ok(FinishReason::Error { .. }));
        let standby = match (feed.cap.take(), healthy, &self.device) {
            (Some(cap), true, Some(device)) => Some((cap, feed.rx, device.clone())),
            _ => None,
        };
        // `Err` here is the model's: the engine is not handed back.
        let mut engine_lost = false;
        let (reason, texts) = match reason.and_then(|r| self.live.close().map(|texts| (r, texts))) {
            Ok(ok) => ok,
            Err(e) => {
                let message = format!("{e:#}");
                tracing::warn!("voice: dictation: {message}");
                engine_lost = true;
                (FinishReason::Error { message }, Vec::new())
            }
        };
        self.texts(texts);
        let join = |k: usize| {
            self.finals
                .iter()
                .map(|f| if k == 1 { f.1.as_str() } else { f.2.as_str() })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut done = Done {
            timings: DictationTimings {
                surface: self.surface,
                press_to_first_pcm_ms: self.since_press(self.first_pcm),
                press_to_first_partial_ms: self.since_press(self.first_partial),
                press_to_first_final_ms: self.since_press(self.first_final),
                stop_to_final_ms: None,
                polish_ms: None,
                device: self.device.clone(),
            },
            raw: join(1),
            shaped: join(2),
            reason,
            engine: None,
            standby,
            stop_at,
        };
        if !engine_lost {
            self.live.reset();
            done.engine = Some(self.live);
        }
        done
    }

    /// §7.3: `Send` armed, `SETTLE` without new speech, then `SILENCE` of closed gate.
    fn settled(&mut self) -> bool {
        let now = Instant::now();
        let Some(at) = self.send_at else {
            self.quiet_since = None;
            return false;
        };
        if now.duration_since(at) < SETTLE || self.live.is_open() {
            self.quiet_since = None;
            return false;
        }
        let quiet = *self.quiet_since.get_or_insert(now);
        now.duration_since(quiet) >= SILENCE
    }

    /// §8.3: the device that opened delivers nothing usable; try the rest of
    /// the list once. `Err` = the message the dictation ends with.
    fn next_device(&mut self, feed: &mut Feed, why: &str) -> Result<(), String> {
        let failed = self.device.clone().unwrap_or_default();
        let message = format!("microphone: {why} {failed}");
        if self.tried_next || self.prefs.is_empty() {
            self.report(Progress::Device {
                device: None,
                state: DeviceState::Lost,
            });
            return Err(message);
        }
        self.tried_next = true;
        self.prefs.retain(|n| *n != failed);
        drop(feed.cap.take());
        match (self.opener)(&self.prefs) {
            Ok((cap, rx, name)) => {
                tracing::info!("voice: {message}; next device {name:?}");
                feed.cap = Some(cap);
                feed.rx = rx;
                self.device = Some(name.clone());
                self.silent_windows = 0;
                self.peak = 0.0;
                self.levels.clear();
                self.report(Progress::Device {
                    device: Some(name),
                    state: DeviceState::Changed,
                });
                Ok(())
            }
            Err(e) => {
                self.report(Progress::Device {
                    device: None,
                    state: DeviceState::Lost,
                });
                Err(format!("{message}; next: {e:#}"))
            }
        }
    }

    /// §8.2: the mic went away mid-dictation. Close the open utterance, then
    /// retry the preference list every [`REARM_RETRY`] for [`REARM_MAX`].
    /// `Ok(true)` re-armed (or cancelled meanwhile — the loop sees it),
    /// `Ok(false)` gave up, `Err` the model failed on the close.
    fn rearm(&mut self, feed: &mut Feed, cancel: &CancellationToken) -> anyhow::Result<bool> {
        tracing::info!("voice: dictation {} device lost; re-arming", self.id);
        self.report(Progress::Device {
            device: None,
            state: DeviceState::Lost,
        });
        let texts = self.live.close()?;
        self.texts(texts);
        drop(feed.cap.take());
        let since = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Ok(true);
            }
            match (self.opener)(&self.prefs) {
                Ok((cap, rx, name)) => {
                    tracing::info!("voice: dictation {} re-armed on {name:?}", self.id);
                    feed.cap = Some(cap);
                    feed.rx = rx;
                    self.device = Some(name.clone());
                    self.report(Progress::Device {
                        device: Some(name),
                        state: DeviceState::Changed,
                    });
                    return Ok(true);
                }
                Err(e) => tracing::debug!("voice: re-arm: {e:#}"),
            }
            let left = REARM_MAX.saturating_sub(since.elapsed());
            if left.is_zero() {
                return Ok(false);
            }
            std::thread::sleep(REARM_RETRY.min(left));
        }
    }

    /// One 20 ms source chunk: level (and §8.3's silent-device check while
    /// `detect`), the utterer, the history audio. `Ok(true)`: the device is silent.
    fn chunk(&mut self, pcm: &[f32], detect: bool) -> anyhow::Result<bool> {
        let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
        self.levels.push(rms.min(1.0));
        if detect && !self.heard {
            self.peak = pcm.iter().fold(self.peak, |m, s| m.max(s.abs()));
        }
        if self.levels.len() == LEVELS {
            let levels = std::mem::take(&mut self.levels);
            if detect && !self.heard {
                let window = (levels.iter().map(|l| l * l).sum::<f32>() / LEVELS as f32).sqrt();
                if window <= SILENT_RMS && self.peak <= SILENT_PEAK {
                    self.silent_windows += 1;
                    if self.silent_windows >= SILENT_WINDOWS {
                        return Ok(true);
                    }
                } else {
                    self.heard = true;
                }
                self.peak = 0.0;
            }
            self.report(Progress::Level(levels));
        }
        let texts = self.live.chunk(pcm)?;
        if self.record_audio {
            let open = self.live.is_open();
            if open {
                if !self.was_open {
                    self.utt_audio.extend(self.tail.drain(..).flatten());
                }
                self.utt_audio.extend_from_slice(pcm);
            } else {
                self.tail.push_back(pcm.to_vec());
                if self.tail.len() > TAIL_CHUNKS {
                    self.tail.pop_front();
                }
            }
            self.was_open = open;
        }
        self.texts(texts);
        Ok(false)
    }

    /// The open utterance's audio → 16 kHz i16 → the store (§6.2).
    fn flush_audio(&mut self, utterance: u32) {
        if !self.record_audio || self.utt_audio.is_empty() {
            return;
        }
        let pcm = std::mem::take(&mut self.utt_audio);
        match crate::stt::resample(pcm, self.rate, MODEL_RATE) {
            Ok(p) => {
                let pcm = p
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                self.report(Progress::Audio { utterance, pcm });
            }
            Err(e) => tracing::debug!("voice: history audio resample: {e:#}"),
        }
    }

    /// §4.6: every piece through the shaper; end commands sticky per utterance
    /// (§7.1); "scratch that" re-sends the previous closed utterance empty (§7.4).
    fn texts(&mut self, texts: Vec<Text>) {
        for t in texts {
            let now = Instant::now();
            if self.utterance != Some(t.utterance) {
                self.utterance = Some(t.utterance);
                self.armed = None;
            }
            let mut s = self.shaper.shape(&self.prev, &t.text, t.is_final);
            match s.command {
                Some(c) => self.armed = Some(c),
                None => s.command = self.armed,
            }
            self.send_at = matches!(s.command, Some(EndCommand::Send { .. })).then_some(now);
            self.first_partial.get_or_insert(now);
            if t.is_final {
                self.first_final.get_or_insert(now);
                self.flush_audio(t.utterance);
                if matches!(s.command, Some(EndCommand::ScratchThat)) {
                    if let Some((utterance, _, _)) = self.finals.pop() {
                        self.report(Progress::Text {
                            utterance,
                            text: String::new(),
                            shaped: String::new(),
                            command: None,
                            actions: Vec::new(),
                            is_final: true,
                        });
                    }
                } else {
                    self.finals
                        .push((t.utterance, t.text.clone(), s.text.clone()));
                }
                self.prev = self
                    .finals
                    .iter()
                    .map(|f| f.2.as_str())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            self.report(Progress::Text {
                utterance: t.utterance,
                text: t.text,
                shaped: s.text,
                command: s.command,
                actions: s.actions,
                is_final: t.is_final,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::mpsc::channel;

    use kameo::actor::Spawn as _;

    use super::*;

    /// The test stand-in for the utterer: `script` says which chunk index
    /// (1-based; `usize::MAX` = on `close`) yields which piece; the gate is
    /// "open" while chunks carry signal.
    pub(super) struct Fake {
        pub script: Vec<(usize, Text)>,
        pub fed: usize,
        pub open: bool,
    }

    impl Fake {
        pub fn chunk(&mut self, pcm: &[f32]) -> Vec<Text> {
            self.fed += 1;
            self.open = pcm.iter().any(|s| s.abs() > 1e-3);
            let fed = self.fed;
            self.take(fed)
        }

        pub fn close(&mut self) -> Vec<Text> {
            self.open = false;
            self.take(usize::MAX)
        }

        fn take(&mut self, at: usize) -> Vec<Text> {
            let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.script)
                .into_iter()
                .partition(|(i, _)| *i == at);
            self.script = later;
            now.into_iter().map(|(_, t)| t).collect()
        }
    }

    /// The test shim for A1's parser: an exact phrase at the tail arms.
    pub(super) fn end_phrase(p: &EndPhrases, raw: &str) -> Option<EndCommand> {
        let raw = raw
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if p.send.iter().any(|s| raw.ends_with(&s.to_lowercase())) {
            Some(EndCommand::Send { near_miss: false })
        } else if p.scratch.iter().any(|s| raw.ends_with(&s.to_lowercase())) {
            Some(EndCommand::ScratchThat)
        } else {
            None
        }
    }

    fn text(utterance: u32, s: &str, is_final: bool) -> Text {
        Text {
            utterance,
            start: 0,
            text: s.into(),
            is_final,
        }
    }

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * ((i as f32) * 0.3).sin()).collect()
    }

    /// Every published [`VoiceEvent`], in order.
    #[derive(kameo::Actor)]
    struct Events(tokio::sync::mpsc::UnboundedSender<VoiceEvent>);

    impl Message<VoiceEvent> for Events {
        type Reply = ();
        async fn handle(&mut self, e: VoiceEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    /// A ready actor on a fake engine with the event sink; mic opens hand out
    /// `mics` (name, receiver) in order and fail when the list is empty.
    async fn ready(
        script: Vec<(usize, Text)>,
        mics: Vec<(&'static str, Receiver<Chunk>)>,
    ) -> (
        ActorRef<Dictation>,
        tokio::sync::mpsc::UnboundedReceiver<VoiceEvent>,
    ) {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = Events::spawn_with_mailbox(Events(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Events, VoiceEvent>(&sink).await;
        let mut d = Dictation::new(
            rt,
            std::env::temp_dir().join("theuth-no-such-dictation-model"),
            Duration::from_secs(20),
        );
        d.model = ModelState::Ready;
        d.engine = Some(Engine::Fake(Fake {
            script,
            fed: 0,
            open: false,
        }));
        d.cfg.standby = Duration::ZERO;
        let mics = Mutex::new(mics);
        d.opener = Arc::new(move |_prefs: &[String]| {
            let mut m = mics.lock();
            if m.is_empty() {
                anyhow::bail!("no device");
            }
            let (name, rx) = m.remove(0);
            Ok((Capture::fake(), rx, name.into()))
        });
        (Dictation::spawn(d), rx)
    }

    /// Pull events until `pred` matches or `within` passes; the matched event.
    async fn until(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<VoiceEvent>,
        within: Duration,
        mut pred: impl FnMut(&VoiceEvent) -> bool,
    ) -> Option<VoiceEvent> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let e = tokio::time::timeout_at(deadline, rx.recv()).await.ok()??;
            if pred(&e) {
                return Some(e);
            }
        }
    }

    fn remote_16k() -> StartDictation {
        StartDictation {
            source: AudioSource::Remote {
                sample_rate: 16_000,
            },
            opts: DictationOpts::default(),
        }
    }

    /// Push `n` chunks of 20 ms at 16 kHz, `signal` or zeros.
    async fn push(actor: &ActorRef<Dictation>, id: DictationId, n: usize, signal: bool) {
        for _ in 0..n {
            let pcm: Vec<i16> = if signal {
                tone(320).iter().map(|s| (s * 32767.0) as i16).collect()
            } else {
                vec![0; 320]
            };
            actor.ask(PushAudio { id, pcm }).await.expect("push");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// §16.2: `opts: None` is today — the shaped text equals the raw text for
    /// plain prose, no command, no actions, `Finished { Done }` on stop.
    #[tokio::test]
    async fn none_opts_is_today() {
        let (actor, mut rx) = ready(
            vec![
                (3, text(0, "the deploy is", false)),
                (6, text(0, "the deploy is done", true)),
            ],
            vec![],
        )
        .await;
        let id = actor.ask(remote_16k()).await.expect("start");
        push(&actor, id, 8, true).await;
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { is_final: true, .. })
        })
        .await
        .expect("a final");
        let VoiceEvent::DictationText {
            text,
            shaped,
            command,
            actions,
            ..
        } = e
        else {
            unreachable!()
        };
        assert_eq!(text, "the deploy is done");
        assert_eq!(shaped, text);
        assert_eq!(command, None);
        assert!(actions.is_empty());
        actor.ask(StopDictation { id }).await.expect("stop");
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationFinished { .. })
        })
        .await
        .expect("finished");
        assert!(matches!(
            e,
            VoiceEvent::DictationFinished {
                reason: FinishReason::Done,
                ..
            }
        ));
        let s = actor.ask(DictationStatus).await.expect("status");
        assert_eq!(s.active, None);
        assert_eq!(s.last.expect("timings").surface, Surface::Composer);
    }

    /// §7.3/§7.5: "send it" in a partial arms; after the final, 1.5 s settle
    /// plus 0.35 s of closed gate stop the dictation by itself with `Done`;
    /// the last text event carries `Send`.
    #[tokio::test]
    async fn send_it_settle_then_silence_stops() {
        let (actor, mut rx) = ready(
            vec![
                (3, text(0, "ship it send it", false)),
                (5, text(0, "ship it send it", true)),
            ],
            vec![],
        )
        .await;
        let id = actor.ask(remote_16k()).await.expect("start");
        let t0 = Instant::now();
        push(&actor, id, 5, true).await;
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { is_final: true, .. })
        })
        .await
        .expect("final");
        let VoiceEvent::DictationText { command, .. } = e else {
            unreachable!()
        };
        assert_eq!(command, Some(EndCommand::Send { near_miss: false }));
        // Silence: zeros for 2.5 s (the actor polls the settle every 50 ms).
        let silence = tokio::spawn({
            let actor = actor.clone();
            async move { push(&actor, id, 125, false).await }
        });
        let e = until(&mut rx, Duration::from_secs(5), |e| {
            matches!(e, VoiceEvent::DictationFinished { .. })
        })
        .await
        .expect("finished by itself");
        silence.abort();
        assert!(matches!(
            e,
            VoiceEvent::DictationFinished {
                reason: FinishReason::Done,
                ..
            }
        ));
        let took = t0.elapsed();
        assert!(
            took >= SETTLE + SILENCE && took < Duration::from_secs(4),
            "{took:?}"
        );
        assert_eq!(
            actor.ask(DictationStatus).await.expect("status").active,
            None
        );
    }

    /// §7.4: "scratch that" at a final re-sends the previous closed utterance empty.
    #[tokio::test]
    async fn scratch_that_empties_previous_utterance() {
        let (actor, mut rx) = ready(
            vec![
                (2, text(0, "foo", true)),
                (4, text(1, "scratch that", true)),
                (6, text(2, "bar", true)),
            ],
            vec![],
        )
        .await;
        let id = actor.ask(remote_16k()).await.expect("start");
        push(&actor, id, 7, true).await;
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { utterance: 0, is_final: true, text, .. } if text.is_empty())
        })
        .await;
        assert!(e.is_some(), "utterance 0 re-sent empty");
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { utterance: 2, .. })
        })
        .await
        .expect("bar");
        let VoiceEvent::DictationText { shaped, .. } = e else {
            unreachable!()
        };
        assert_eq!(shaped, "bar");
        actor.ask(StopDictation { id }).await.expect("stop");
    }

    /// §8.2 / §16 row 6: the mic feed sends `Err("device lost")`; a second
    /// feed opens, `DictationDevice { Lost }` then `{ Changed }` are published,
    /// no `Finished`, and the same dictation keeps producing text.
    #[tokio::test]
    async fn device_lost_rearms_within_5s() {
        let (tx1, rx1) = channel::<Chunk>();
        let (tx2, rx2) = channel::<Chunk>();
        let (actor, mut rx) = ready(
            vec![(2, text(0, "before", true)), (6, text(1, "after", true))],
            vec![("mic-1", rx1), ("mic-2", rx2)],
        )
        .await;
        let id = actor
            .ask(StartDictation {
                source: AudioSource::Mic,
                opts: DictationOpts::default(),
            })
            .await
            .expect("start");
        for _ in 0..3 {
            tx1.send(Ok(tone(960))).unwrap();
        }
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(
                e,
                VoiceEvent::DictationText {
                    utterance: 0,
                    is_final: true,
                    ..
                }
            )
        })
        .await
        .expect("before");
        assert_eq!(
            actor
                .ask(DictationStatus)
                .await
                .unwrap()
                .input_device
                .as_deref(),
            Some("mic-1")
        );
        let lost_at = Instant::now();
        tx1.send(Err("device lost".into())).unwrap();
        drop(tx1);
        let e = until(&mut rx, Duration::from_secs(5), |e| {
            matches!(
                e,
                VoiceEvent::DictationDevice {
                    state: DeviceState::Changed,
                    ..
                }
            )
        })
        .await
        .expect("re-armed");
        assert!(lost_at.elapsed() < Duration::from_secs(5));
        let VoiceEvent::DictationDevice { device, .. } = e else {
            unreachable!()
        };
        assert_eq!(device.as_deref(), Some("mic-2"));
        for _ in 0..4 {
            tx2.send(Ok(tone(960))).unwrap();
        }
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(
                e,
                VoiceEvent::DictationText {
                    utterance: 1,
                    is_final: true,
                    ..
                }
            )
        })
        .await
        .expect("after, same dictation");
        let s = actor.ask(DictationStatus).await.unwrap();
        assert_eq!(s.active, Some(id), "no Finished");
        assert_eq!(s.input_device.as_deref(), Some("mic-2"));
        actor.ask(StopDictation { id }).await.unwrap();
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationFinished { .. })
        })
        .await
        .expect("finished on stop");
    }

    /// §8.3 / §16 row 7: three silent 160 ms windows reject the device within
    /// ~480 ms; with no next device in the list the dictation ends with the
    /// named error after `DictationDevice { Lost }`.
    #[tokio::test]
    async fn silent_device_rejected_in_480ms() {
        let (tx, frx) = channel::<Chunk>();
        let (actor, mut rx) = ready(vec![], vec![("dead-mic", frx)]).await;
        let id = actor
            .ask(StartDictation {
                source: AudioSource::Mic,
                opts: DictationOpts::default(),
            })
            .await
            .expect("start");
        let t0 = Instant::now();
        // 30 chunks = 600 ms of exact zeros, paced like a device.
        let feeder = std::thread::spawn(move || {
            for _ in 0..30 {
                if tx.send(Ok(vec![0.0; 960])).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(
                e,
                VoiceEvent::DictationDevice {
                    state: DeviceState::Lost,
                    ..
                }
            )
        })
        .await
        .expect("lost");
        let e = until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationFinished { .. })
        })
        .await
        .expect("finished");
        assert!(
            t0.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t0.elapsed()
        );
        let VoiceEvent::DictationFinished { reason, id: fid } = e else {
            unreachable!()
        };
        assert_eq!(fid, id);
        assert!(
            matches!(&reason, FinishReason::Error { message } if message.contains("no sound from dead-mic")),
            "{reason:?}"
        );
        feeder.join().unwrap();
    }

    /// §8.3 / §16 row 7: a capture that delivers nothing for 2 s is not
    /// ready — the next device in the list is tried once, and `Started` is
    /// reported only when PCM arrives.
    #[tokio::test]
    async fn readiness_gate_2s() {
        let (_quiet_tx, quiet_rx) = channel::<Chunk>();
        let (live_tx, live_rx) = channel::<Chunk>();
        let (actor, mut rx) = ready(
            vec![(2, text(0, "hi", true))],
            vec![("quiet", quiet_rx), ("live", live_rx)],
        )
        .await;
        actor
            .ask(Setting::InputDevices(vec!["quiet".into(), "live".into()]))
            .await
            .unwrap();
        let id = actor
            .ask(StartDictation {
                source: AudioSource::Mic,
                opts: DictationOpts::default(),
            })
            .await
            .expect("start");
        let t0 = Instant::now();
        let e = until(&mut rx, Duration::from_secs(4), |e| {
            matches!(
                e,
                VoiceEvent::DictationDevice {
                    state: DeviceState::Changed,
                    ..
                }
            )
        })
        .await
        .expect("next device");
        let waited = t0.elapsed();
        assert!(
            waited >= READY_TIMEOUT && waited < Duration::from_secs(3),
            "{waited:?}"
        );
        let VoiceEvent::DictationDevice { device, .. } = e else {
            unreachable!()
        };
        assert_eq!(device.as_deref(), Some("live"));
        for _ in 0..3 {
            live_tx.send(Ok(tone(960))).unwrap();
        }
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationStarted { .. })
        })
        .await
        .expect("started after the first chunk");
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { is_final: true, .. })
        })
        .await
        .expect("text from the live device");
        actor.ask(StopDictation { id }).await.unwrap();
    }

    /// §8.7 / §16 row 8: with a remote tone and a press 100 ms before the
    /// start, the four numbers are present and monotonic, first partial ≥
    /// first PCM, and `dictation_status.last` carries them.
    #[tokio::test]
    async fn timings_recorded_with_synthetic_feed() {
        let (actor, mut rx) = ready(
            vec![
                (3, text(0, "one two", false)),
                (6, text(0, "one two three", true)),
            ],
            vec![],
        )
        .await;
        let id = actor
            .ask(StartDictation {
                source: AudioSource::Remote {
                    sample_rate: 16_000,
                },
                opts: DictationOpts {
                    surface: Some(Surface::Tui),
                    press_ago_ms: Some(100),
                    ..Default::default()
                },
            })
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(30)).await;
        push(&actor, id, 8, true).await;
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationText { is_final: true, .. })
        })
        .await
        .expect("final");
        actor.ask(StopDictation { id }).await.unwrap();
        until(&mut rx, Duration::from_secs(3), |e| {
            matches!(e, VoiceEvent::DictationFinished { .. })
        })
        .await
        .expect("finished");
        let t = actor
            .ask(DictationStatus)
            .await
            .unwrap()
            .last
            .expect("timings");
        assert_eq!(t.surface, Surface::Tui);
        let pcm = t.press_to_first_pcm_ms.expect("first pcm");
        let partial = t.press_to_first_partial_ms.expect("first partial");
        let fin = t.press_to_first_final_ms.expect("first final");
        let stop = t.stop_to_final_ms.expect("stop → final");
        assert!(pcm >= 100 && pcm <= partial && partial <= fin, "{t:?}");
        assert!(stop < 1000, "{t:?}");
        assert_eq!(t.device, None);
    }

    /// No model on disk: `Prewarm` reports `Loading`, a `StartDictation` parked
    /// behind the load fails with the path-bearing error, the state settles on
    /// `Missing`, and stopping an unknown id is a no-op `Ok`.
    #[tokio::test]
    async fn missing_model_fails_parked_start() {
        let dir = std::env::temp_dir().join("theuth-no-such-dictation-model");
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = Dictation::spawn(Dictation::new(rt, dir.clone(), Duration::from_secs(1)));

        assert_eq!(actor.ask(Prewarm).await.expect("ask"), ModelState::Loading);
        let err = match actor
            .ask(StartDictation {
                source: AudioSource::Mic,
                opts: DictationOpts::default(),
            })
            .await
        {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            other => panic!("expected a handler error, got {other:?}"),
        };
        assert!(err.contains(&dir.display().to_string()), "{err}");
        let status = actor.ask(DictationStatus).await.expect("ask");
        assert_eq!(status.model, ModelState::Missing);
        assert_eq!(status.active, None);
        actor
            .ask(StopDictation {
                id: DictationId::new(),
            })
            .await
            .expect("unknown id is Ok");
    }

    /// Without a dictation, pushed audio is refused; a remote rate that does not
    /// split into 20 ms chunks is refused before any load starts.
    #[tokio::test]
    async fn remote_guards() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = Dictation::spawn(Dictation::new(
            rt,
            std::env::temp_dir().join("theuth-no-such-dictation-model"),
            Duration::from_secs(1),
        ));
        let err = match actor
            .ask(PushAudio {
                id: DictationId::new(),
                pcm: vec![0; 320],
            })
            .await
        {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            other => panic!("expected a handler error, got {other:?}"),
        };
        assert_eq!(err, "not dictating");
        let err = match actor
            .ask(StartDictation {
                source: AudioSource::Remote {
                    sample_rate: 22_051,
                },
                opts: DictationOpts::default(),
            })
            .await
        {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            other => panic!("expected a handler error, got {other:?}"),
        };
        assert!(err.contains("multiple of 50"), "{err}");
        assert_eq!(
            actor.ask(DictationStatus).await.expect("ask").model,
            ModelState::Missing
        );
    }

    fn handler_err<T, M>(r: Result<T, kameo::error::SendError<M, String>>) -> String {
        match r {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            Err(_) => panic!("expected a handler error, got a send error"),
            Ok(_) => panic!("expected a handler error, got Ok"),
        }
    }

    /// 76 §10 step 4: while the engine is leased, `StartDictation` and a
    /// second `Lease` answer "talking", `Prewarm` is a no-op `Ready`, and a
    /// `Return` makes dictation reachable again (the mic is what fails then).
    #[tokio::test]
    async fn leased_engine_refuses_dictation() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut d = Dictation::new(
            rt,
            std::env::temp_dir().join("theuth-no-such-dictation-model"),
            Duration::from_secs(1),
        );
        d.model = ModelState::Ready;
        d.leased = true;
        let actor = Dictation::spawn(d);
        let start = StartDictation {
            source: AudioSource::Mic,
            opts: DictationOpts::default(),
        };
        assert_eq!(handler_err(actor.ask(start).await), "talking");
        assert_eq!(handler_err(actor.ask(Lease).await), "talking");
        assert_eq!(actor.ask(Prewarm).await.expect("ask"), ModelState::Ready);
    }

    /// No model on disk: `Lease` parks behind the load and fails with its error.
    #[tokio::test]
    async fn lease_needs_the_model() {
        let dir = std::env::temp_dir().join("theuth-no-such-dictation-model");
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = Dictation::spawn(Dictation::new(rt, dir.clone(), Duration::from_secs(1)));
        let err = handler_err(actor.ask(Lease).await);
        assert!(err.contains(&dir.display().to_string()), "{err}");
        assert_eq!(
            actor.ask(DictationStatus).await.expect("ask").model,
            ModelState::Missing
        );
    }
}
