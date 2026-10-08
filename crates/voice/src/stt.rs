//! Speech-to-text (45 §4.2): any container symphonia demuxes → mono 16 kHz →
//! Parakeet TDT via `parakeet-rs`. [`Engine`] is the TDT handle (`meetings/02-live.md`
//! §0.1): the [`Transcriber`] actor loads one on first use and drops it after
//! `stt_idle_secs` unused; the meeting plugin loads its own for the transcript of
//! record, with word times. [`Stream`] is the live half (54 §0.3): the cache-aware
//! Nemotron transducer fed fixed windows by the dictation task.
//! No `parakeet_rs`/`symphonia` type leaves here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::message::{Context, Message};
use parakeet_rs::{ExecutionConfig, Nemotron, ParakeetTDT, TimestampMode, Transcriber as _};
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
/// TDT's sequence limit is ~4–5 min; longer audio goes in pieces of this many
/// 16 kHz samples (180 s, 45 §0.8).
pub const PIECE: usize = 180 * MODEL_RATE as usize;
/// How often the idle watchdog looks.
const WATCH: Duration = Duration::from_secs(60);
/// Samples per streaming window: 560 ms at 16 kHz, the Nemotron encoder's chunk
/// (56 mel frames × 160 hop). 35 VAD frames of 256, so window edges are frame edges.
pub const WINDOW: usize = 8_960;

/// Any container symphonia demuxes → mono f32 at the source rate. Opus packets
/// (webm/ogg from Chromium, the meeting tracks) go to libopus, which symphonia
/// does not decode; that branch always returns 48 kHz.
pub fn decode(bytes: Vec<u8>, ext: &str) -> anyhow::Result<(Vec<f32>, u32)> {
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
        // `copy_interleaved_ref` asserts the buffer fits: size it to this frame's capacity and spec, not the first's.
        let sb = match sbuf.as_mut() {
            Some(sb) if sb.capacity() >= a.capacity() * ch => sb,
            _ => sbuf.insert(SampleBuffer::new(a.capacity() as u64, spec)),
        };
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
pub fn resample(pcm: Vec<f32>, from: u32, to: u32) -> anyhow::Result<Vec<f32>> {
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

/// `text` spoken by the Pocket TTS checkpoint at `tts_dir` (`<data_dir>/models/pocket-tts`) with
/// `voice`, as 16 kHz mono PCM for [`Engine::transcribe`]: a real utterance for `t --bench-models`,
/// where silence transcribing to `""` proves nothing about the execution provider (58 step 13).
/// Blocking; loads the TTS model (seconds) — not for a request path.
pub fn speech_sample(tts_dir: &Path, voice: &str, text: &str) -> anyhow::Result<Vec<f32>> {
    use ptts::synth::SpeechOptions;
    let synth = crate::model::load(&crate::model::locate(tts_dir)?)?;
    let mut pcm: Vec<f32> = Vec::new();
    for chunk in synth
        .stream_with(text, &SpeechOptions::default().voice(voice))
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        pcm.extend_from_slice(&chunk.map_err(|e| anyhow::anyhow!("{e}"))?);
    }
    resample(pcm, crate::speak::SAMPLE_RATE, MODEL_RATE)
}

/// One word with its times, from [`Engine::transcribe`] with `words: true`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    /// The word (standalone punctuation is attached to the word before it).
    pub text: String,
    /// Start, ms from the start of the piece.
    pub t0_ms: u64,
    /// End, ms from the start of the piece.
    pub t1_ms: u64,
}

/// What [`Engine::transcribe`] returns for one piece of audio.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    /// Punctuated, cased text.
    pub text: String,
    /// Word times; empty unless asked for.
    pub words: Vec<Word>,
}

/// parakeet-rs's defaults (4 intra threads, CPU) plus the execution provider
/// `models.accelerator` resolves to (`models::onnx::configure`: DirectML on Windows).
fn exec_config() -> ExecutionConfig {
    ExecutionConfig::default().with_custom_configure(|b| Ok(models::onnx::configure(b)))
}

/// The offline TDT model (`meetings/02-live.md` §0.1): loaded once from a
/// checkpoint directory, transcribes 16 kHz mono pieces. Inference takes the
/// inner lock, so one `Engine` serialises its callers; `Send + Sync`.
pub struct Engine(Mutex<ParakeetTDT>);

impl Engine {
    /// Load the TDT checkpoint in `dir` (`encoder-model.onnx`, `decoder_joint-model.onnx`,
    /// `vocab.txt`) after the process-wide ONNX Runtime init (`models::onnx::ort_lib`,
    /// which finds the runtime under `<data_dir>/models/onnxruntime/` among other places).
    pub fn load(dir: &Path, data_dir: &Path) -> anyhow::Result<Engine> {
        let t = Instant::now();
        models::onnx::ort_lib(data_dir).map_err(anyhow::Error::msg)?;
        let m = ParakeetTDT::from_pretrained(dir, Some(exec_config()))
            .map_err(|e| anyhow::anyhow!("no STT model in {}: {e}", dir.display()))?;
        tracing::info!("voice: STT model loaded in {:?}", t.elapsed());
        Ok(Engine(Mutex::new(m)))
    }

    /// Transcribe one piece (16 kHz mono, at most [`PIECE`] samples — TDT's
    /// sequence limit is a few minutes). `words: true` returns word times.
    pub fn transcribe(&self, pcm16k: &[f32], words: bool) -> anyhow::Result<Piece> {
        let mode = if words {
            TimestampMode::Words
        } else {
            TimestampMode::Sentences
        };
        let r = self
            .0
            .lock()
            .transcribe_samples(pcm16k.to_vec(), MODEL_RATE, 1, Some(mode))
            .map_err(|e| anyhow::anyhow!("transcribe: {e}"))?;
        let mut out: Vec<Word> = Vec::new();
        if words {
            out.reserve(r.tokens.len());
            for t in r.tokens {
                let text = t.text.trim();
                if text.is_empty() {
                    continue;
                }
                let ms = |s: f32| (f64::from(s.max(0.0)) * 1000.0).round() as u64;
                // `Words` mode leaves `.`/`,` as their own tokens; the text glues them on.
                let punct = text.len() == 1
                    && text
                        .chars()
                        .all(|c| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | ')'));
                match out.last_mut() {
                    Some(prev) if punct => {
                        prev.text.push_str(text);
                        prev.t1_ms = prev.t1_ms.max(ms(t.end));
                    }
                    _ => out.push(Word {
                        text: text.to_owned(),
                        t0_ms: ms(t.start),
                        t1_ms: ms(t.end).max(ms(t.start)),
                    }),
                }
            }
        }
        Ok(Piece {
            text: r.text.trim().to_owned(),
            words: out,
        })
    }
}

/// One live streaming-STT session (54 §0.3, `meetings/02-live.md` §0.1b): the
/// cache-aware Nemotron transducer. Feed 16 kHz mono in fixed [`WINDOW`]s; each
/// call returns only the *new* punctuated text — encoder cache and decoder state
/// persist across windows, so the output is append-only. `reset` at a real turn
/// boundary, never per window. `Send` but not `Sync`: one owner task per stream.
pub struct Stream(Nemotron);

impl Stream {
    /// Load the session from `dir` (`encoder.onnx`, `decoder_joint.onnx`,
    /// `tokenizer.model`) after the process-wide ONNX Runtime init. A missing
    /// directory is a distinct, path-bearing error so the caller can report `Missing`.
    pub fn open(dir: &Path, data_dir: &Path) -> anyhow::Result<Stream> {
        if !dir.is_dir() {
            anyhow::bail!("dictation model not found at {}", dir.display());
        }
        models::onnx::ort_lib(data_dir).map_err(anyhow::Error::msg)?;
        let t = Instant::now();
        let m = Nemotron::from_pretrained(dir, Some(exec_config()))
            .map_err(|e| anyhow::anyhow!("no streaming STT model in {}: {e}", dir.display()))?;
        tracing::info!("voice: streaming STT model loaded in {:?}", t.elapsed());
        Ok(Stream(m))
    }

    /// One encoder step over `window`; the text emitted by it (often empty).
    pub fn feed(&mut self, window: &[f32; WINDOW]) -> anyhow::Result<String> {
        self.0
            .transcribe_chunk(window)
            .map_err(|e| anyhow::anyhow!("transcribe_chunk: {e}"))
    }

    /// Drop encoder cache, decoder state and the audio buffer: next `feed` starts a new turn.
    pub fn reset(&mut self) {
        self.0.reset();
    }
}

/// The [`crate::TRANSCRIBER_ACTOR`] actor (45 §4.2). Inference runs in
/// `spawn_blocking`; the actor itself never waits on it.
#[derive(kameo::Actor)]
pub struct Transcriber {
    /// Held only inside the blocking task; `None` until first use or after idle.
    model: Arc<Mutex<Option<Engine>>>,
    last_used: Arc<Mutex<Instant>>,
    dir: PathBuf,
    data_dir: PathBuf,
    /// Zero: never unload.
    idle: Duration,
}

impl Transcriber {
    pub(crate) fn new(dir: PathBuf, data_dir: PathBuf, idle: Duration) -> Self {
        let this = Self {
            model: Arc::new(Mutex::new(None)),
            last_used: Arc::new(Mutex::new(Instant::now())),
            dir,
            data_dir,
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
        let model = match guard.as_ref() {
            Some(m) => m,
            None => guard.insert(Engine::load(&self.dir, &self.data_dir)?),
        };
        let mut parts = Vec::new();
        for piece in pcm.chunks(PIECE) {
            parts.push(model.transcribe(piece, false)?.text);
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
            data_dir: self.data_dir.clone(),
            idle: self.idle,
        };
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            // A panic here (a decoder on an odd container) must not drop `tx`: the asker would read "actor stopped".
            let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                this.transcribe(&m.path)
            })) {
                Ok(r) => r.map_err(|e| format!("{e:#}")),
                Err(p) => Err(format!(
                    "transcribe panicked: {}",
                    p.downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default()
                )),
            };
            match &r {
                Ok(t2) => tracing::debug!(
                    "voice: transcribed {} ms of audio in {:?}",
                    t2.duration_ms,
                    t.elapsed()
                ),
                Err(e) => tracing::warn!("voice: {e}"),
            }
            tx.send(r);
        });
        reply
    }
}

/// Timed words of a whole file (144 §4.1 audio/video rows): [`Transcribe`] with `words: true` per
/// [`PIECE`], times shifted to the file. `progress`, when given, hears 0–100 as pieces finish.
/// `ponytail:` pieces are cut at 180 s, not at pauses — a word on a boundary may split; the meeting
/// crate's VAD-span path is the upgrade when a notebook corpus shows it.
#[derive(Debug)]
pub struct TranscribeWords {
    /// Audio or video file on this machine; the extension picks the demuxer.
    pub path: PathBuf,
    /// Percent sink.
    pub progress: Option<tokio::sync::mpsc::UnboundedSender<u8>>,
}

/// Reply to [`TranscribeWords`].
#[derive(Clone, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct WordTranscript {
    /// Every word with file times, in order.
    pub words: Vec<Word>,
    /// Length of the audio.
    pub duration_ms: u64,
}

impl Transcriber {
    /// Whole pipeline for one file with word times; blocking.
    fn transcribe_words(
        &self,
        path: &Path,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<u8>>,
    ) -> anyhow::Result<WordTranscript> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let bytes = std::fs::read(path)?;
        let (pcm, rate) = decode(bytes, ext)?;
        let pcm = resample(pcm, rate, MODEL_RATE)?;
        let duration_ms = pcm.len() as u64 * 1000 / MODEL_RATE as u64;
        let mut guard = self.model.lock();
        let model = match guard.as_ref() {
            Some(m) => m,
            None => guard.insert(Engine::load(&self.dir, &self.data_dir)?),
        };
        let mut words = Vec::new();
        let pieces = pcm.chunks(PIECE).count().max(1);
        for (i, piece) in pcm.chunks(PIECE).enumerate() {
            let offset = (i * PIECE) as u64 * 1000 / MODEL_RATE as u64;
            words.extend(
                model
                    .transcribe(piece, true)?
                    .words
                    .into_iter()
                    .map(|w| Word {
                        text: w.text,
                        t0_ms: w.t0_ms + offset,
                        t1_ms: w.t1_ms + offset,
                    }),
            );
            if let Some(p) = progress {
                let _ = p.send(((i + 1) * 100 / pieces) as u8);
            }
        }
        *self.last_used.lock() = Instant::now();
        Ok(WordTranscript { words, duration_ms })
    }
}

impl Message<TranscribeWords> for Transcriber {
    type Reply = kameo::reply::DelegatedReply<Result<WordTranscript, String>>;
    async fn handle(
        &mut self,
        m: TranscribeWords,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let this = Self {
            model: self.model.clone(),
            last_used: self.last_used.clone(),
            dir: self.dir.clone(),
            data_dir: self.data_dir.clone(),
            idle: self.idle,
        };
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                this.transcribe_words(&m.path, m.progress.as_ref())
            })) {
                Ok(r) => r.map_err(|e| format!("{e:#}")),
                Err(p) => Err(format!(
                    "transcribe panicked: {}",
                    p.downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default()
                )),
            };
            match &r {
                Ok(w) => tracing::debug!(
                    "voice: transcribed {} ms of audio with word times in {:?}",
                    w.duration_ms,
                    t.elapsed()
                ),
                Err(e) => tracing::warn!("voice: {e}"),
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

    /// The smoke sentence spoken by `say`, through the TDT `Engine` with word
    /// times: the words come back in order with monotonic, non-overlapping
    /// times inside the clip. Skips without the checkpoint, `say` or the dylib.
    #[test]
    fn engine_returns_words_with_monotonic_times() {
        let data_dir = match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(".theuth"),
            None => return,
        };
        let dir = data_dir.join("models").join(crate::TDT_MODEL_SUBDIR);
        if !dir.is_dir() {
            eprintln!("skipped: no TDT model at {}", dir.display());
            return;
        }
        let path = std::env::temp_dir().join(format!("theuth-stt-{}.wav", uuid::Uuid::now_v7()));
        let ok = std::process::Command::new("say")
            .args(["-o"])
            .arg(&path)
            .args([
                "--data-format=LEI16@48000",
                "The deploy is blocked on the image tags.",
            ])
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipped: no `say`");
            return;
        }
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let (pcm, rate) = decode(bytes, "wav").unwrap();
        let pcm = resample(pcm, rate, MODEL_RATE).unwrap();
        let engine = match Engine::load(&dir, &data_dir) {
            Ok(e) => e,
            Err(e) if format!("{e:#}").contains("libonnxruntime") => {
                eprintln!("skipped: {e:#}");
                return;
            }
            Err(e) => panic!("{e:#}"),
        };
        let p = engine.transcribe(&pcm, true).unwrap();
        let said: String = p
            .words
            .iter()
            .map(|w| w.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();
        assert_eq!(said, "the deploy is blocked on the image tags", "{p:?}");
        assert_eq!(p.words.len(), 8, "{p:?}");
        let clip_ms = pcm.len() as u64 * 1000 / u64::from(MODEL_RATE);
        for (a, b) in p.words.iter().zip(p.words.iter().skip(1)) {
            assert!(a.t0_ms <= a.t1_ms && a.t1_ms <= b.t0_ms, "{p:?}");
        }
        assert!(p.words.last().unwrap().t1_ms <= clip_ms + 200, "{p:?}");
        // Without word times the text is the same sentence and `words` is empty.
        let s = engine.transcribe(&pcm, false).unwrap();
        assert!(s.words.is_empty());
        assert!(s.text.to_lowercase().contains("image tags"), "{s:?}");
    }
}
