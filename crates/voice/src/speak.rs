//! One blocking task per utterance (43 §4.5): markdown → sentences → `ptts`
//! PCM → one Opus packet per Mimi frame, reported to the [`Voice`] actor —
//! or, for [`Sink::Device`] (76 §0.3), the same PCM frames written straight
//! into the [`Player`] ring, no codec.

use std::sync::Arc;

use base64::Engine as _;
use kameo::actor::ActorRef;
use ptts::synth::{SpeechOptions, Synth};
use tokio_util::sync::CancellationToken;

use crate::out::Player;
use crate::{FinishReason, SpeechId, Voice};

/// PCM rate `ptts` produces and the packets decode to.
pub(crate) const SAMPLE_RATE: u32 = 24_000;
/// 80 ms at [`SAMPLE_RATE`]: one Mimi frame, one Opus packet.
const FRAME: usize = 1920;
/// Opus bitrate; ≈ 320 B per packet.
const BITRATE: i32 = 32_000;
/// Upper bound `encode_vec_float` may write for one packet.
const MAX_PACKET: usize = 400;

/// PCM chunks of one utterance; each item holds whole Mimi frames, several at a time.
pub(crate) type Pcm = Box<dyn Iterator<Item = anyhow::Result<Vec<f32>>>>;

/// What the speak and clip tasks need from the model: text in, 24 kHz PCM
/// out. `ptts::Synth` in production; a fake in the actor tests, so the queue
/// is testable without weights.
pub(crate) trait Synthesizer: Send + Sync {
    /// Start synthesizing `spoken` (already sentences) in `voice`.
    fn stream(&self, spoken: &str, voice: &str) -> anyhow::Result<Pcm>;
}

impl Synthesizer for Synth {
    fn stream(&self, spoken: &str, voice: &str) -> anyhow::Result<Pcm> {
        let stream = self
            .stream_with(spoken, &SpeechOptions::default().voice(voice))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Box::new(
            stream.map(|c| c.map_err(|e| anyhow::anyhow!("{e}"))),
        ))
    }
}

/// What the task tells the actor; the actor publishes each as a [`crate::VoiceEvent`].
pub(crate) enum Progress {
    /// The stream opened; audio follows.
    Started {
        /// Length of the spoken text.
        chars: u32,
    },
    /// One base64 Opus packet ([`Sink::Client`] only).
    Audio {
        /// Starts at 0, no gaps.
        seq: u32,
        /// Base64 of the packet.
        opus: Arc<str>,
    },
    /// Synthesis ended. Never sent after a cancel — the actor already published `Stopped`.
    Finished(FinishReason),
}

/// `tell` to the actor from the task.
pub(crate) struct Report {
    /// Utterance this belongs to; stale ids (after `Stop`) are dropped.
    pub id: SpeechId,
    /// What happened.
    pub progress: Progress,
}

/// Where the frames go.
pub(crate) enum Out {
    /// Opus packets to the actor, one `Progress::Audio` each.
    Client,
    /// Straight into the speaker ring; nothing but `Started`/`Finished` reaches the actor.
    Device(Arc<Player>),
}

/// Synthesize `text` and report to `me`. Runs in `spawn_blocking`; returns
/// when the stream ends, `cancel` fires, or the actor is gone.
pub(crate) fn run(
    synth: Arc<dyn Synthesizer>,
    voice: String,
    id: SpeechId,
    text: String,
    out: Out,
    cancel: CancellationToken,
    me: ActorRef<Voice>,
) {
    let report = |progress: Progress| me.tell(Report { id, progress }).blocking_send().is_ok();
    let spoken = markdown::spoken(&text);
    if spoken.is_empty() {
        report(Progress::Finished(FinishReason::Done));
        return;
    }
    let reason = match synthesize(&*synth, &voice, &spoken, id, &out, &cancel, &report) {
        Ok(true) => FinishReason::Done,
        Ok(false) => return,
        Err(e) => FinishReason::Error {
            message: format!("{e:#}"),
        },
    };
    report(Progress::Finished(reason));
}

/// `Ok(true)`: every sample was reported. `Ok(false)`: cancelled, flushed or the
/// actor is gone — nothing more to say. `Err`: terminal synthesis or codec failure.
fn synthesize(
    synth: &dyn Synthesizer,
    voice: &str,
    spoken: &str,
    id: SpeechId,
    out: &Out,
    cancel: &CancellationToken,
    report: &impl Fn(Progress) -> bool,
) -> anyhow::Result<bool> {
    // Dropping `stream` at return stops both ptts threads.
    let stream = synth.stream(spoken, voice)?;
    let mut emit = match out {
        Out::Client => {
            let mut enc =
                opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Audio)?;
            enc.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
            Emit::Opus(enc)
        }
        Out::Device(player) => Emit::Ring(player),
    };
    if !report(Progress::Started {
        chars: spoken.chars().count() as u32,
    }) {
        return Ok(false);
    }
    let mut seq = 0u32;
    let mut send = |frame: &[f32]| -> anyhow::Result<bool> {
        let ok = match &mut emit {
            Emit::Opus(enc) => {
                let packet = enc.encode_vec_float(frame, MAX_PACKET)?;
                let opus = base64::engine::general_purpose::STANDARD.encode(packet);
                report(Progress::Audio {
                    seq,
                    opus: opus.into(),
                })
            }
            // `false`: a flush retired this utterance; `Stop` already published `Stopped`.
            Emit::Ring(player) => player.write(id, frame),
        };
        seq += 1;
        Ok(ok)
    };
    // ptts hands back whole Mimi frames but several at a time; re-frame to exactly one per packet.
    let mut buf: Vec<f32> = Vec::with_capacity(FRAME * 4);
    for chunk in stream {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        buf.extend_from_slice(&chunk?);
        for frame in buf.as_chunks::<FRAME>().0 {
            if !send(frame)? {
                return Ok(false);
            }
        }
        let rem = buf.len() % FRAME;
        buf.drain(..buf.len() - rem);
    }
    if !buf.is_empty() && !cancel.is_cancelled() {
        // Tail shorter than a frame: pad with silence.
        buf.resize(FRAME, 0.0);
        if !send(&buf)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One frame's destination.
enum Emit<'a> {
    Opus(opus::Encoder),
    Ring(&'a Player),
}
