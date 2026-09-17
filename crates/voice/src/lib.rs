//! Read replies aloud (design doc 43): Pocket TTS runs in the harness via
//! `ptts`; audio leaves as [`VoiceEvent::SpeechAudio`] — one Opus packet per
//! 80 ms Mimi frame, base64 in JSON — over the bus.

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use ptts::synth::Synth;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub mod model;
mod speak;

use speak::{Progress, Report};

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    /// Not on disk.
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
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            voice: "peter_yearsley".into(),
            model_dir: None,
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
}

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
                    self.start(synth, cur.id, cur.session, text, ctx.actor_ref().clone());
                }
            }
            Err(e) => {
                let message = format!("{e:#}");
                tracing::warn!("voice: {message}");
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
        let actor = Voice::spawn(Voice {
            rt: rt.clone(),
            cfg,
            model: None,
            model_state: ModelState::Missing,
            voices: Vec::new(),
            current: None,
        });
        actor.register(VOICE_ACTOR)?;
        Ok(())
    }
}
