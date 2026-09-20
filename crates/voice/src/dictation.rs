//! Live dictation (54 §4.3–4.5): the [`Dictation`] actor owns the streaming
//! Nemotron session ([`Stream`]) and the VAD, loads both on [`Prewarm`], and per
//! dictation runs one blocking task that reads the audio source — the daemon's
//! microphone, or PCM a remote client streams in with [`PushAudio`] — gates
//! it with the VAD and feeds the model only between onset and close (02-live
//! §2). Text reaches the frontends as [`VoiceEvent::DictationText`]; where it
//! lands is theirs. Nothing is written to disk.
//!
//! ponytail: the `Stream` (≈0.9 GB int8) is never idle-unloaded — `reset()`
//! between dictations, dropped only after a model failure so the next
//! `Prewarm` reloads. Add `voice.dictation_idle_secs` if RSS complaints arrive.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use rubato::Resampler as _;
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::mic::{Capture, Chunk, RATE};
use crate::stt::{Stream, WINDOW};
use crate::vad::{Edge, FRAME, Gate, Vad};
use crate::{DictationId, FinishReason, ModelState, VoiceEvent};

/// The model's input rate.
const MODEL_RATE: usize = 16_000;
/// Chunks per [`VoiceEvent::DictationLevel`] (160 ms).
const LEVELS: usize = 8;
/// How long the task waits for an audio chunk before rechecking cancel and deadline.
const POLL: Duration = Duration::from_millis(50);
/// 16 kHz samples kept behind the newest VAD frame while no utterance is open:
/// the pre-roll, the onset run, one frame of slack.
const HISTORY: usize = (Gate::PREROLL + Gate::ONSET + 1) * FRAME;

/// Load the streaming model and the VAD if not loaded; never opens the mic.
/// Replies with the current state (`Loading` while in flight).
#[derive(Clone, Copy, Debug)]
pub struct Prewarm;

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
/// flight (or starts one) rather than failing.
#[derive(Clone, Copy, Debug)]
pub struct StartDictation {
    /// Where the audio comes from.
    pub source: AudioSource,
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
}

/// Loaded model and VAD; moves into the dictation task and back.
type Engine = (Stream, Vad);

/// The dictation in progress.
struct Running {
    id: DictationId,
    cancel: CancellationToken,
    /// The feed for a remote source; `None` when the task reads the mic.
    remote: Option<Remote>,
}

/// The actor's end of a remote dictation's audio: pushed PCM is rechunked into
/// 20 ms chunks for the task, a partial tail waiting for the next push.
struct Remote {
    tx: Sender<Chunk>,
    pending: Vec<f32>,
    /// Samples per chunk: `sample_rate / 50`.
    chunk: usize,
}

/// `tell` from the load task.
struct Loaded(Result<Engine, String>);

/// What the task tells the actor; each becomes a [`VoiceEvent`].
enum Progress {
    /// The audio source is open.
    Started,
    /// RMS per 20 ms chunk, oldest first.
    Level(Vec<f32>),
    /// Utterance text so far.
    Text {
        utterance: u32,
        text: String,
        is_final: bool,
    },
    /// The task ended. `None` engine: lost to a model failure; the next `Prewarm` reloads.
    /// Boxed: the engine is ~700 bytes and every other variant is small.
    Finished {
        reason: FinishReason,
        engine: Box<Option<Engine>>,
    },
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
    /// `None` while loading or while a dictation task owns it.
    engine: Option<Engine>,
    /// `StartDictation` askers parked until the load finishes, each with its source.
    waiters: Vec<(AudioSource, ReplySender<Result<DictationId, String>>)>,
    active: Option<Running>,
}

impl Dictation {
    pub(crate) fn new(rt: Runtime, dir: PathBuf, max: Duration) -> Self {
        Self {
            rt,
            dir,
            max,
            model: ModelState::Missing,
            engine: None,
            waiters: Vec::new(),
            active: None,
        }
    }

    async fn set_model(&mut self, state: ModelState) {
        self.model = state.clone();
        self.rt.publish(VoiceEvent::DictationModel { state }).await;
    }

    /// Load off the runtime unless loaded or loading; the result comes back as [`Loaded`].
    async fn load(&mut self, me: ActorRef<Self>) {
        if !matches!(self.model, ModelState::Missing | ModelState::Failed { .. }) {
            return;
        }
        self.set_model(ModelState::Loading).await;
        let dir = self.dir.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<Engine> {
                // `Stream::open` runs the once-per-process `ort` init the VAD session needs (02-live §0.1c).
                let stream = Stream::open(&dir)?;
                let vad = Vad::new()?;
                Ok((stream, vad))
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("dictation model load task panicked: {e}")));
            if r.is_ok() {
                tracing::info!("voice: dictation model loaded in {:?}", t.elapsed());
            }
            let _ = me.tell(Loaded(r.map_err(|e| format!("{e:#}")))).await;
        });
    }

    /// Open the audio source and start the task on the loaded engine.
    /// `source` was validated by the `StartDictation` handler.
    fn begin(&mut self, me: ActorRef<Self>, source: AudioSource) -> Result<DictationId, String> {
        if self.active.is_some() {
            return Err("already dictating".into());
        }
        let Some(engine) = self.engine.take() else {
            return Err("dictation model not loaded".into());
        };
        let rate = match source {
            AudioSource::Mic => RATE,
            AudioSource::Remote { sample_rate } => sample_rate,
        };
        let chunk = rate as usize / 50;
        let resampler = if rate as usize == MODEL_RATE {
            Ok(None)
        } else {
            rubato::FftFixedIn::<f32>::new(rate as usize, MODEL_RATE, chunk, 2, 1)
                .map(Some)
                .map_err(|e| format!("resampler: {e}"))
        };
        let started = resampler.and_then(|r| match source {
            AudioSource::Mic => Capture::open()
                .map(|(cap, rx)| (r, Some(cap), rx, None))
                .map_err(|e| format!("microphone: {e:#}")),
            AudioSource::Remote { .. } => {
                let (tx, rx) = std::sync::mpsc::channel();
                Ok((
                    r,
                    None,
                    rx,
                    Some(Remote {
                        tx,
                        pending: Vec::new(),
                        chunk,
                    }),
                ))
            }
        });
        let (resampler, cap, rx, remote) = match started {
            Ok(s) => s,
            Err(e) => {
                self.engine = Some(engine);
                return Err(e);
            }
        };
        let id = DictationId::new();
        let cancel = CancellationToken::new();
        self.active = Some(Running {
            id,
            cancel: cancel.clone(),
            remote,
        });
        let max = self.max;
        let task = Task {
            me: me.clone(),
            id,
            stream: engine.0,
            vad: engine.1,
            gate: Gate::new(),
            resampler,
            buf: Vec::new(),
            base: 0,
            total: 0,
            open: None,
            utterance: 0,
            text: String::new(),
            levels: Vec::with_capacity(LEVELS),
        };
        tokio::spawn(async move {
            let (reason, engine) =
                tokio::task::spawn_blocking(move || task.run(cap, rx, cancel, max))
                    .await
                    .unwrap_or_else(|e| {
                        (
                            FinishReason::Error {
                                message: format!("dictation task panicked: {e}"),
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
        Ok(id)
    }
}

impl Message<Prewarm> for Dictation {
    type Reply = ModelState;
    async fn handle(&mut self, _: Prewarm, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
            tx.send(Err("already dictating".into()));
        } else if self.model == ModelState::Ready {
            tx.send(self.begin(me, m.source));
        } else {
            self.load(me).await;
            self.waiters.push((m.source, tx));
        }
        reply
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
                // The first waiter gets the slot; the rest hear "already dictating".
                for (source, tx) in std::mem::take(&mut self.waiters) {
                    tx.send(self.begin(me.clone(), source));
                }
            }
            Err(message) => {
                tracing::warn!("voice: dictation model: {message}");
                for (_, tx) in self.waiters.drain(..) {
                    tx.send(Err(message.clone()));
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
    async fn handle(&mut self, m: Report, _: &mut Context<Self, Self::Reply>) {
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
                is_final,
            } => VoiceEvent::DictationText {
                id,
                utterance,
                text,
                is_final,
            },
            Progress::Finished { reason, engine } => {
                self.active = None;
                match *engine {
                    Some(e) => self.engine = Some(e),
                    None => self.set_model(ModelState::Missing).await,
                }
                VoiceEvent::DictationFinished { id, reason }
            }
        };
        self.rt.publish(event).await;
    }
}

/// One dictation, on its blocking thread (54 §4.5).
struct Task {
    me: ActorRef<Dictation>,
    id: DictationId,
    stream: Stream,
    vad: Vad,
    gate: Gate,
    /// Source rate → 16 kHz; `None` when the source is already 16 kHz. FIR
    /// state must survive chunk boundaries.
    resampler: Option<rubato::FftFixedIn<f32>>,
    /// 16 kHz samples from absolute sample `base` on.
    buf: Vec<f32>,
    base: usize,
    /// 16 kHz samples seen so far.
    total: usize,
    /// `Some(fed)` while an utterance is open: samples before `fed` went to the model.
    open: Option<usize>,
    /// Index of the open (or next) utterance.
    utterance: u32,
    /// Text of the open utterance.
    text: String,
    /// RMS per chunk, flushed every [`LEVELS`].
    levels: Vec<f32>,
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

    /// Read the source until cancel, deadline or failure; flush; hand the engine back.
    /// `cap` is `Some` for the mic; a remote source's chunks arrive via `rx` alone.
    fn run(
        mut self,
        cap: Option<Capture>,
        rx: Receiver<Chunk>,
        cancel: CancellationToken,
        max: Duration,
    ) -> (FinishReason, Option<Engine>) {
        self.report(Progress::Started);
        let deadline = Instant::now() + max;
        // `Err`: the model or resampler failed — the engine is not handed back.
        let reason = loop {
            if cancel.is_cancelled() || !self.me.is_alive() {
                break Ok(FinishReason::Done);
            }
            if Instant::now() >= deadline {
                break Ok(FinishReason::Stopped);
            }
            // `Disconnected`: the mic thread died. A remote feed cannot disconnect
            // while the actor lives — it holds the `Sender` in `Running` until this
            // task's `Finished` lands — and an actor gone is caught by `is_alive` above.
            let chunk = match rx.recv_timeout(POLL) {
                Ok(Ok(c)) => c,
                Ok(Err(m)) => {
                    break Ok(FinishReason::Error {
                        message: format!("microphone: {m}"),
                    });
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    break Ok(FinishReason::Error {
                        message: "audio source closed".into(),
                    });
                }
            };
            if let Err(e) = self.chunk(&chunk) {
                break Err(e);
            }
        };
        // Device closed before the flush, which may take a second.
        drop(cap);
        match reason.and_then(|r| self.close().map(|()| r)) {
            Ok(reason) => {
                self.stream.reset();
                (reason, Some((self.stream, self.vad)))
            }
            Err(e) => {
                let message = format!("{e:#}");
                tracing::warn!("voice: dictation: {message}");
                (FinishReason::Error { message }, None)
            }
        }
    }

    /// One 20 ms source chunk: level, resample, VAD, feed.
    fn chunk(&mut self, pcm: &[f32]) -> anyhow::Result<()> {
        let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
        self.levels.push(rms.min(1.0));
        if self.levels.len() == LEVELS {
            let levels = std::mem::take(&mut self.levels);
            self.report(Progress::Level(levels));
        }
        let out;
        let new: &[f32] = match self.resampler.as_mut() {
            Some(r) => {
                out = r.process(&[pcm], None)?;
                &out[0]
            }
            None => pcm,
        };
        self.buf.extend_from_slice(new);
        self.total += new.len();
        let probs = self.vad.process(new);
        for (k, &p) in probs.iter().enumerate() {
            // The last frame ends at (within a frame of) `total`; earlier ones FRAME apart.
            let end = self.total - (probs.len() - 1 - k) * FRAME;
            match self.gate.push(p) {
                Some(Edge::Onset) if self.open.is_none() => {
                    let start = end.saturating_sub((Gate::ONSET + Gate::PREROLL) * FRAME);
                    self.open = Some(start.max(self.base));
                }
                Some(Edge::Close) => self.close()?,
                Some(Edge::Turn) => {
                    self.stream.reset();
                    self.vad.reset();
                }
                _ => {}
            }
        }
        self.feed_full()?;
        let keep = self
            .open
            .unwrap_or_else(|| self.total.saturating_sub(HISTORY))
            .max(self.base);
        self.buf.drain(..keep - self.base);
        self.base = keep;
        Ok(())
    }

    /// Feed every full window past `fed`; report new text.
    fn feed_full(&mut self) -> anyhow::Result<()> {
        while let Some(fed) = self.open
            && self.total - fed >= WINDOW
        {
            let at = fed - self.base;
            let window: &[f32; WINDOW] = self.buf[at..at + WINDOW].try_into()?;
            let t = self.stream.feed(window)?;
            self.open = Some(fed + WINDOW);
            if self.append(&t) {
                self.report(Progress::Text {
                    utterance: self.utterance,
                    text: self.text.trim().to_owned(),
                    is_final: false,
                });
            }
        }
        Ok(())
    }

    /// Close the open utterance: the partial window zero-padded, then one zero
    /// window to pull the tail out of the decoder's lookahead; publish `final`.
    fn close(&mut self) -> anyhow::Result<()> {
        self.feed_full()?;
        let Some(fed) = self.open.take() else {
            return Ok(());
        };
        let mut window = [0f32; WINDOW];
        let rest = &self.buf[fed - self.base..];
        window[..rest.len()].copy_from_slice(rest);
        let t = self.stream.feed(&window)?;
        self.append(&t);
        window.fill(0.0);
        let t = self.stream.feed(&window)?;
        self.append(&t);
        self.report(Progress::Text {
            utterance: self.utterance,
            text: self.text.trim().to_owned(),
            is_final: true,
        });
        self.utterance += 1;
        self.text.clear();
        Ok(())
    }

    /// Append new model text, one space between pieces. `false` when it was blank.
    fn append(&mut self, t: &str) -> bool {
        if t.trim().is_empty() {
            return false;
        }
        if !self.text.is_empty()
            && !self.text.ends_with(char::is_whitespace)
            && !t.starts_with(char::is_whitespace)
        {
            self.text.push(' ');
        }
        self.text.push_str(t);
        true
    }
}

#[cfg(test)]
mod tests {
    use kameo::actor::Spawn as _;

    use super::*;

    /// No model on disk: `Prewarm` reports `Loading`, a `StartDictation` parked
    /// behind the load fails with the path-bearing error, the state settles on
    /// `Missing`, and stopping an unknown id is a no-op `Ok`.
    #[tokio::test]
    async fn missing_model_fails_parked_start() {
        let dir = std::env::temp_dir().join("wth-no-such-dictation-model");
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = Dictation::spawn(Dictation::new(rt, dir.clone(), Duration::from_secs(1)));

        assert_eq!(actor.ask(Prewarm).await.expect("ask"), ModelState::Loading);
        let err = match actor
            .ask(StartDictation {
                source: AudioSource::Mic,
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
            std::env::temp_dir().join("wth-no-such-dictation-model"),
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
}
