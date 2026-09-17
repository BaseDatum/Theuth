//! Speech-to-text (45 §4.2): any container symphonia demuxes → mono 16 kHz →
//! Parakeet TDT via `parakeet-rs`. The model is loaded on first use and dropped
//! after `stt_idle_secs` unused. No `parakeet_rs`/`symphonia` type leaves here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::message::{Context, Message};
use parakeet_rs::{ParakeetTDT, TimestampMode, Transcriber as _};
use parking_lot::Mutex;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::{Transcribe, Transcript};

/// The model's input rate; anything else is refused by `parakeet-rs`.
const MODEL_RATE: u32 = 16_000;
/// TDT's sequence limit is ~4–5 min; longer notes go in pieces (45 §0.8).
const PIECE: usize = 180 * MODEL_RATE as usize;
/// How often the idle watchdog looks.
const WATCH: Duration = Duration::from_secs(60);

/// ONNX Runtime is loaded at run time: `libonnxruntime*.dylib` / `.so` in the checkpoint
/// directory, else `$ORT_DYLIB_PATH`, else Homebrew's (`brew install onnxruntime`). Linked
/// statically it brings protobuf 3.21, which cannot share a process with the protobuf-lite
/// 3.14 inside `sentencepiece` (ptts): the TTS load aborts. As a dylib its symbols stay
/// private to it.
fn ort_lib(dir: &Path) -> anyhow::Result<PathBuf> {
    let ext = if cfg!(target_os = "macos") {
        ".dylib"
    } else {
        ".so"
    };
    let in_dir = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("libonnxruntime") && n.contains(ext))
        });
    in_dir
        .or_else(|| std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from))
        .or_else(|| {
            ["/opt/homebrew/lib", "/usr/local/lib"]
                .into_iter()
                .map(|d| Path::new(d).join(format!("libonnxruntime{ext}")))
                .find(|p| p.exists())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no libonnxruntime{ext} in {} (nor $ORT_DYLIB_PATH, nor Homebrew): copy lib/libonnxruntime* from an ONNX Runtime release (github.com/microsoft/onnxruntime/releases, 1.17 or newer) there",
                dir.display()
            )
        })
}

/// Any container symphonia demuxes → mono f32 at the source rate. Opus packets
/// (webm/ogg from Chromium) go to libopus, which symphonia does not decode.
pub(crate) fn decode(bytes: Vec<u8>, ext: &str) -> anyhow::Result<(Vec<f32>, u32)> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(ext);
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no audio track"))?;
    let id = track.id;
    let params = track.codec_params.clone();
    let mut out = Vec::new();
    if params.codec == CODEC_TYPE_OPUS {
        let mut dec = opus::Decoder::new(48_000, opus::Channels::Mono)?;
        let mut buf = vec![0f32; 5760];
        while let Ok(p) = format.next_packet() {
            if p.track_id() != id {
                continue;
            }
            let n = dec.decode_float(&p.data, &mut buf, false)?;
            out.extend_from_slice(&buf[..n]);
        }
        return Ok((out, 48_000));
    }
    let mut dec = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
    let mut rate = 0;
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    while let Ok(p) = format.next_packet() {
        if p.track_id() != id {
            continue;
        }
        let a = match dec.decode(&p) {
            Ok(a) => a,
            // A damaged packet; skip it rather than lose the note.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let spec = *a.spec();
        rate = spec.rate;
        let ch = spec.channels.count();
        let sb = sbuf.get_or_insert_with(|| SampleBuffer::new(a.capacity() as u64, spec));
        sb.copy_interleaved_ref(a);
        for frame in sb.samples().chunks(ch) {
            out.push(frame.iter().sum::<f32>() / ch as f32);
        }
    }
    if rate == 0 {
        anyhow::bail!("no decodable audio");
    }
    Ok((out, rate))
}

/// `from` Hz → `to` Hz, mono.
pub(crate) fn resample(pcm: Vec<f32>, from: u32, to: u32) -> anyhow::Result<Vec<f32>> {
    use rubato::Resampler as _;
    if from == to {
        return Ok(pcm);
    }
    const CHUNK: usize = 1024;
    let mut r = rubato::FftFixedIn::<f32>::new(from as usize, to as usize, CHUNK, 2, 1)?;
    let mut out = Vec::with_capacity(pcm.len() * to as usize / from as usize + CHUNK);
    let mut i = 0;
    while i + CHUNK <= pcm.len() {
        let o = r.process(&[&pcm[i..i + CHUNK]], None)?;
        out.extend_from_slice(&o[0]);
        i += CHUNK;
    }
    if i < pcm.len() {
        let o = r.process_partial(Some(&[&pcm[i..]]), None)?;
        out.extend_from_slice(&o[0]);
    }
    Ok(out)
}

/// The [`crate::TRANSCRIBER_ACTOR`] actor (45 §4.2). Inference runs in
/// `spawn_blocking`; the actor itself never waits on it.
#[derive(kameo::Actor)]
pub struct Transcriber {
    /// Held only inside the blocking task; `None` until first use or after idle.
    model: Arc<Mutex<Option<ParakeetTDT>>>,
    last_used: Arc<Mutex<Instant>>,
    dir: PathBuf,
    /// Zero: never unload.
    idle: Duration,
}

impl Transcriber {
    pub(crate) fn new(dir: PathBuf, idle: Duration) -> Self {
        let this = Self {
            model: Arc::new(Mutex::new(None)),
            last_used: Arc::new(Mutex::new(Instant::now())),
            dir,
            idle,
        };
        if !idle.is_zero() {
            // ponytail: fixed 60 s interval, not a timer reset per note — the model
            // lives at most one minute past `idle`. Fine for a 1.75 GB resident.
            let (model, last_used) = (this.model.clone(), this.last_used.clone());
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(WATCH);
                loop {
                    tick.tick().await;
                    if last_used.lock().elapsed() <= idle {
                        continue;
                    }
                    let mut m = model.lock();
                    if m.take().is_some() {
                        tracing::info!("voice: STT model unloaded after {} s idle", idle.as_secs());
                    }
                }
            });
        }
        this
    }

    /// Whole pipeline for one file; blocking.
    fn transcribe(&self, path: &Path) -> anyhow::Result<Transcript> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let bytes = std::fs::read(path)?;
        let (pcm, rate) = decode(bytes, ext)?;
        let pcm = resample(pcm, rate, MODEL_RATE)?;
        let duration_ms = pcm.len() as u64 * 1000 / MODEL_RATE as u64;
        let mut guard = self.model.lock();
        let model = match guard.as_mut() {
            Some(m) => m,
            None => {
                let t = Instant::now();
                let lib = ort_lib(&self.dir)?;
                ort::init_from(&lib)
                    .map_err(|e| anyhow::anyhow!("load {}: {e}", lib.display()))?
                    .commit();
                let m = ParakeetTDT::from_pretrained(&self.dir, None)
                    .map_err(|e| anyhow::anyhow!("no STT model in {}: {e}", self.dir.display()))?;
                tracing::info!("voice: STT model loaded in {:?}", t.elapsed());
                guard.insert(m)
            }
        };
        let mut parts = Vec::new();
        for piece in pcm.chunks(PIECE) {
            let r = model
                .transcribe_samples(
                    piece.to_vec(),
                    MODEL_RATE,
                    1,
                    Some(TimestampMode::Sentences),
                )
                .map_err(|e| anyhow::anyhow!("transcribe: {e}"))?;
            parts.push(r.text);
        }
        *self.last_used.lock() = Instant::now();
        Ok(Transcript {
            text: parts.join(" ").trim().to_owned(),
            duration_ms,
        })
    }
}

impl Message<Transcribe> for Transcriber {
    type Reply = kameo::reply::DelegatedReply<Result<Transcript, String>>;
    async fn handle(&mut self, m: Transcribe, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let this = Self {
            model: self.model.clone(),
            last_used: self.last_used.clone(),
            dir: self.dir.clone(),
            idle: self.idle,
        };
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let r = this.transcribe(&m.path).map_err(|e| format!("{e:#}"));
            if let Ok(t2) = &r {
                tracing::debug!(
                    "voice: transcribed {} ms of audio in {:?}",
                    t2.duration_ms,
                    t.elapsed()
                );
            }
            tx.send(r);
        });
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16-bit mono PCM WAV, `n` samples of a 440 Hz tone at `rate`.
    fn wav(rate: u32, n: usize) -> Vec<u8> {
        let data = n as u32 * 2;
        let mut w = Vec::with_capacity(44 + data as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        for i in 0..n {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
            w.extend_from_slice(&((s * 20_000.0) as i16).to_le_bytes());
        }
        w
    }

    #[test]
    fn decodes_wav_and_resamples() {
        let (pcm, rate) = decode(wav(48_000, 48_000), "wav").unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(pcm.len(), 48_000);
        assert!(pcm.iter().any(|s| s.abs() > 0.5));
        let r = resample(pcm, 48_000, 16_000).unwrap();
        // rubato's filter delay swallows a few ms at the tail; STT does not care.
        assert!((r.len() as i64 - 16_000).abs() < 160, "{}", r.len());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(vec![0; 64], "m4a").is_err());
    }
}
