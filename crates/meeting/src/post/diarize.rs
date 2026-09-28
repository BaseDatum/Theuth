//! Speaker diarization on the system track (04 §2 stage 2, 53 decision 6):
//! NVIDIA's Nemotron 3 Diarization (a Streaming Sortformer, the
//! `onnx-community/Nemotron-3-Diarization-ONNX` export) scores every 10 ms frame
//! of the recording for up to eight speakers, numbered by first arrival; the
//! frames become `s1..sn` turns, and wespeaker CAM++ embeds each speaker's speech
//! once for the profile match. Both run on `ort` directly; [`session`] runs the
//! process-wide `models::onnx::ort_lib(data_dir)` first (a no-op once voice has).
//! [`Streamer`] is the streaming loop for one [`Geometry`]: the offline pass drives
//! it with [`OFFLINE`], the live diariser (`meeting::diar`, 07) with [`LIVE`]. The
//! graph's I/O, the loop and the thresholds are written up in
//! `docs/design/meetings/04-post-nemotron-as-built.md`.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use realfft::RealToComplex;
use realfft::num_complex::Complex;

use crate::post::align::Span;

/// The diarization model's directory under the models root.
pub const DIAR_DIR: &str = "nemotron-diarization";
/// Its graph; the weights sit beside it in `model_quantized.onnx_data` (ONNX external data).
pub const DIAR_MODEL: &str = "model_quantized.onnx";
/// The embedding model's directory (the `pyannote-rs` export).
pub const EMBED_DIR: &str = "pyannote";
/// The embedding model file.
pub const EMBEDDING: &str = "wespeaker_en_voxceleb_CAM++.onnx";
/// Input rate of both models.
pub const RATE: u32 = 16_000;
/// The embedding model's frame period (2 s at 10 ms): inputs are tiled to a multiple.
pub const FRAMES: usize = 200;
/// Most of one speaker's speech folded into their embedding (as `ME_EMBED_SAMPLES`),
/// and the least that gets one at all.
const EMBED_SAMPLES: usize = 30 * RATE as usize;
const MIN_EMBED_SAMPLES: usize = RATE as usize;

// Front end (`preprocessor_config.json`).
const N_FFT: usize = 512;
const WIN: usize = 400;
const HOP: usize = 160;
const MELS: usize = 128;
const PREEMPHASIS: f32 = 0.97;
/// NeMo's additive log guard, 2⁻²⁴.
const LOG_GUARD: f32 = 5.960_464_5e-8;

// Encoder geometry (`config.json`).
const SUBSAMPLING: usize = 8;
const HIDDEN: usize = 512;
const SPEAKERS: usize = 8;
/// One logit frame.
const STEP_MS: u64 = 10;

// The cache policy from `config.json` `streaming_config`, shared by every geometry.
const CACHE: usize = 264;
const SILENCE_SLOTS: usize = 1;
const SCORE_THRESHOLD: f32 = 0.25;
const LATEST_BOOST: f32 = 0.05;
const MIN_POSITIVE_RATE: f32 = 0.5;
const STRONG_BOOST_RATE: f32 = 0.75;
const WEAK_BOOST_RATE: f32 = 1.5;
/// Per-speaker share of the cache once compressed, minus its silence slot.
const BUDGET: usize = CACHE / SPEAKERS - SILENCE_SLOTS;

/// One streaming geometry of the model card, in encoder frames (80 ms = 8 mel frames):
/// how much audio a step scores, how far it looks ahead, how many recent frames the
/// FIFO keeps, and how many of its oldest move to the speaker cache when it overflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Frames scored per step.
    pub chunk: usize,
    /// Look-ahead frames attended to but scored again as the head of the next step.
    pub right: usize,
    /// FIFO length.
    pub fifo: usize,
    /// Frames moved from the FIFO to the cache per update.
    pub update: usize,
}

impl Geometry {
    /// The audio a step waits for: chunk plus look-ahead.
    pub const fn buffer_ms(self) -> u64 {
        ((self.chunk + self.right) * SUBSAMPLING) as u64 * STEP_MS
    }

    /// 16 kHz samples per scored chunk.
    pub const fn chunk_samples(self) -> usize {
        self.chunk * SUBSAMPLING * HOP
    }

    /// 16 kHz samples a step needs from the chunk start: chunk plus look-ahead.
    pub const fn step_samples(self) -> usize {
        (self.chunk + self.right) * SUBSAMPLING * HOP
    }
}

/// The offline setup of `config.json` (`chunk_length`, `chunk_right_context`,
/// `fifo_length`, `speaker_cache_update_period`): 27.2 s chunks with 3.2 s look-ahead,
/// the card's best-scoring "30.4 s input buffer" (04 §4).
pub const OFFLINE: Geometry = Geometry {
    chunk: 340,
    right: 40,
    fifo: 40,
    update: 300,
};
/// The live setup (07 §0.2, `acc8` in the bench): the card's low-latency cache policy
/// (FIFO 264, update 222) with a 6.4 s chunk and 1.6 s look-ahead — an 8 s input
/// buffer, RTF 0.035 on the CPU, 97–99 % frame agreement with [`OFFLINE`].
pub const LIVE: Geometry = Geometry {
    chunk: 80,
    right: 20,
    fifo: 264,
    update: 222,
};

// Frames → turns.
/// A speaker is active in a frame at or above this probability (the reference's default).
pub const SPEECH: f32 = 0.5;
/// A pause shorter than this inside one speaker's activity does not close their turn.
pub const MERGE_GAP_MS: u64 = 500;
/// Turns shorter than this (after merging) are dropped: a false alarm carries no word.
pub const MIN_TURN_MS: u64 = 200;

/// The two model files, both present, and the data dir that locates the runtime.
#[derive(Clone, Debug)]
pub struct Models {
    /// `nemotron-diarization/model_quantized.onnx`.
    pub diarization: PathBuf,
    /// `pyannote/wespeaker_en_voxceleb_CAM++.onnx`.
    pub embedding: PathBuf,
    /// `<data_dir>`, for `models::onnx::ort_lib`.
    pub data_dir: PathBuf,
}

/// Both files under the models root `dir`, or `None` (04 §0.1: the pipeline then
/// runs without diarization).
pub fn models(dir: &Path, data_dir: &Path) -> Option<Models> {
    let m = Models {
        diarization: dir.join(DIAR_DIR).join(DIAR_MODEL),
        embedding: dir.join(EMBED_DIR).join(EMBEDDING),
        data_dir: data_dir.to_path_buf(),
    };
    (m.diarization.is_file() && m.embedding.is_file()).then_some(m)
}

/// What the stage produces: labelled turns and one embedding per label (a label
/// with under a second of speech has none).
#[derive(Clone, Debug, Default)]
pub struct Diarized {
    /// Speech turns on the system track, by start; overlapping speech overlaps.
    pub spans: Vec<Span>,
    /// Label → wespeaker embedding of (up to 30 s of) its speech.
    pub embeddings: HashMap<String, Vec<f32>>,
}

/// `ort::Error<R>` is not `Send + Sync` (it carries the failed builder); stringify.
fn ort_err<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// `intra_threads`: ORT's default (the physical cores) when `None`; the live diariser
/// caps it so the STT tasks and capture threads keep theirs (07 §2).
fn session(path: &Path, data_dir: &Path, intra_threads: Option<usize>) -> anyhow::Result<Session> {
    models::onnx::ort_lib(data_dir).map_err(anyhow::Error::msg)?;
    let mut builder = Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?;
    if let Some(n) = intra_threads {
        builder = builder.with_intra_threads(n).map_err(ort_err)?;
    }
    models::onnx::configure(builder)
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("load {}: {e}", path.display()))
}

/// `f32` PCM in `[-1, 1]` → `i16`, the embedding model's input.
pub fn to_i16(pcm: &[f32]) -> Vec<i16> {
    pcm.iter()
        .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

/// The diarization model's log-mel front end (`NemotronAsrStreamingFeatureExtractor`):
/// pre-emphasis 0.97, a symmetric 400-sample Hann window centred in a 512-point FFT,
/// hop 160, 128 Slaney mel bins over 0–8 kHz, `ln(power + 2⁻²⁴)`, no normalisation.
/// Frame `t` is centred on sample `t·160` (zero padding past the ends, as
/// `torch.stft(center=True, pad_mode="constant")`); a recording has `⌊len/160⌋` frames.
pub struct Mel {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// `MELS × (N_FFT/2 + 1)`, row-major.
    filters: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Default for Mel {
    fn default() -> Self {
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT);
        Self {
            frame: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window: (0..WIN)
                .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (WIN - 1) as f32).cos())
                .collect(),
            filters: mel_filters(),
        }
    }
}

/// librosa's `filters.mel(sr, n_fft, n_mels, fmin=0, fmax=sr/2, norm="slaney")`:
/// triangles between consecutive Slaney-mel-spaced edges, each scaled to unit area.
fn mel_filters() -> Vec<f32> {
    let bins = N_FFT / 2 + 1;
    let hz_to_mel = |hz: f64| {
        if hz < 1000.0 {
            hz * 3.0 / 200.0
        } else {
            15.0 + (hz / 1000.0).ln() / (6.4f64.ln() / 27.0)
        }
    };
    let mel_to_hz = |mel: f64| {
        if mel < 15.0 {
            mel * 200.0 / 3.0
        } else {
            1000.0 * ((mel - 15.0) * (6.4f64.ln() / 27.0)).exp()
        }
    };
    let top = hz_to_mel(f64::from(RATE) / 2.0);
    let edges: Vec<f64> = (0..=MELS + 1)
        .map(|i| mel_to_hz(top * i as f64 / (MELS + 1) as f64))
        .collect();
    let mut filters = vec![0f32; MELS * bins];
    for m in 0..MELS {
        let (lo, mid, hi) = (edges[m], edges[m + 1], edges[m + 2]);
        let norm = 2.0 / (hi - lo);
        for k in 0..bins {
            let hz = k as f64 * f64::from(RATE) / N_FFT as f64;
            let w = ((hz - lo) / (mid - lo))
                .min((hi - hz) / (hi - mid))
                .max(0.0);
            filters[m * bins + k] = (w * norm) as f32;
        }
    }
    filters
}

impl Mel {
    /// Frames `t0..t1` of `pcm` appended to `out`, row-major `(t1 - t0) × 128`.
    pub fn frames(&mut self, pcm: &[f32], t0: usize, t1: usize, out: &mut Vec<f32>) {
        let bins = N_FFT / 2 + 1;
        let pad = (N_FFT - WIN) / 2;
        out.reserve((t1 - t0) * MELS);
        for t in t0..t1 {
            self.frame.fill(0.0);
            // The window's first sample sits `N_FFT/2 - pad` before the frame centre.
            let start = t as isize * HOP as isize - (N_FFT / 2 - pad) as isize;
            for (i, w) in self.window.iter().enumerate() {
                let n = start + i as isize;
                if n < 0 || n as usize >= pcm.len() {
                    continue;
                }
                let n = n as usize;
                let x = pcm[n] - if n > 0 { PREEMPHASIS * pcm[n - 1] } else { 0.0 };
                self.frame[pad + i] = x * w;
            }
            self.fft
                .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
                .expect("realfft: buffers sized by the plan");
            for row in self.filters.chunks_exact(bins) {
                let mel: f32 = row
                    .iter()
                    .zip(&self.spectrum)
                    .map(|(f, c)| f * c.norm_sqr())
                    .sum();
                out.push((mel + LOG_GUARD).ln());
            }
        }
    }
}

/// The streaming state (a port of HF's `Nemotron3DiarizationSpeakerCache`): the
/// arrival-order speaker cache — up to [`CACHE`] encoder frames with the speaker
/// probabilities they were kept for — and the FIFO of the newest frames, sized by the
/// [`Geometry`]. Every chunk attends to both, so a speaker heard in minute 2 keeps
/// their channel in minute 50.
struct SpeakerCache {
    fifo_len: usize,
    update_period: usize,
    /// `n × HIDDEN`.
    embeds: Vec<f32>,
    /// `n × SPEAKERS`.
    probs: Vec<f32>,
    /// `m × HIDDEN`.
    fifo: Vec<f32>,
    compressed: bool,
}

impl SpeakerCache {
    fn new(g: Geometry) -> Self {
        Self {
            fifo_len: g.fifo,
            update_period: g.update,
            embeds: Vec::new(),
            probs: Vec::new(),
            fifo: Vec::new(),
            compressed: false,
        }
    }

    /// The `cached_embeds` input: cache then FIFO.
    fn cached(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(self.embeds.len() + self.fifo.len());
        v.extend_from_slice(&self.embeds);
        v.extend_from_slice(&self.fifo);
        v
    }

    /// Push a scored chunk. `probs`: the step's per-encoder-frame speaker probabilities
    /// over its whole input (cache, FIFO, chunk, look-ahead); `chunk_embeds`: the
    /// step's fresh frames (chunk and look-ahead), of which the first `n_chunk` join the
    /// FIFO — the look-ahead frames open the next chunk. When the FIFO overflows, at
    /// least `update_period` of its oldest frames move to the cache, which is
    /// compressed once it exceeds [`CACHE`].
    fn update(&mut self, probs: &[f32], chunk_embeds: &[f32], n_chunk: usize, silence: &[f32]) {
        let n_cache = self.embeds.len() / HIDDEN;
        let n_fifo = self.fifo.len() / HIDDEN;
        let mut fifo = std::mem::take(&mut self.fifo);
        fifo.extend_from_slice(&chunk_embeds[..n_chunk * HIDDEN]);
        let len = n_fifo + n_chunk;
        let popped = if len <= self.fifo_len {
            0
        } else {
            self.update_period.max(len - self.fifo_len).min(len)
        };
        if popped > 0 {
            // An uncompressed cache holds plain frames whose probabilities this step
            // re-estimated; a compressed one is out of order, so its stored ones stay.
            let mut cache_probs = if self.compressed {
                std::mem::take(&mut self.probs)
            } else {
                probs[..n_cache * SPEAKERS].to_vec()
            };
            let fifo_probs = &probs[n_cache * SPEAKERS..(n_cache + len) * SPEAKERS];
            cache_probs.extend_from_slice(&fifo_probs[..popped * SPEAKERS]);
            let mut cache = std::mem::take(&mut self.embeds);
            cache.extend_from_slice(&fifo[..popped * HIDDEN]);
            fifo.drain(..popped * HIDDEN);
            if cache.len() / HIDDEN > CACHE {
                (cache, cache_probs) = compress(&cache, &cache_probs, silence);
                self.compressed = true;
            }
            self.embeds = cache;
            self.probs = cache_probs;
        }
        self.fifo = fifo;
    }
}

/// Keep the [`CACHE`] most useful of `n` frames: scored per speaker by how clearly
/// that speaker alone is active ([`frame_scores`]), the frames just popped from the
/// FIFO nudged up, each speaker's best frames boosted so every speaker keeps some;
/// one [`SILENCE_SLOTS`] slot per speaker takes the learned silence embedding.
/// The result is grouped by speaker, in time order within a speaker.
fn compress(embeds: &[f32], probs: &[f32], silence: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = probs.len() / SPEAKERS;
    let mut scores = frame_scores(probs);
    for s in &mut scores[CACHE * SPEAKERS..] {
        *s += LATEST_BOOST;
    }
    let ln2 = 2f32.ln();
    boost(
        &mut scores,
        (BUDGET as f32 * STRONG_BOOST_RATE) as usize,
        2.0 * ln2,
    );
    boost(&mut scores, (BUDGET as f32 * WEAK_BOOST_RATE) as usize, ln2);
    // Flat (speaker, frame) ranking; frame `n` is the silence slot, +∞ so it always stays.
    let rows = n + SILENCE_SLOTS;
    let sentinel = rows * SPEAKERS;
    let mut all: Vec<(f32, usize)> = Vec::with_capacity(sentinel);
    for s in 0..SPEAKERS {
        all.extend((0..n).map(|f| (scores[f * SPEAKERS + s], s * rows + f)));
        all.extend((n..rows).map(|f| (f32::INFINITY, s * rows + f)));
    }
    all.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut keep: Vec<usize> = all[..CACHE]
        .iter()
        .map(|&(score, i)| {
            if score == f32::NEG_INFINITY {
                sentinel
            } else {
                i
            }
        })
        .collect();
    keep.sort_unstable();
    let mut out_embeds = Vec::with_capacity(CACHE * HIDDEN);
    let mut out_probs = Vec::with_capacity(CACHE * SPEAKERS);
    for i in keep {
        let f = if i == sentinel { n } else { (i % rows).min(n) };
        if f == n {
            out_embeds.extend_from_slice(silence);
            out_probs.extend(std::iter::repeat_n(0.0, SPEAKERS));
        } else {
            out_embeds.extend_from_slice(&embeds[f * HIDDEN..(f + 1) * HIDDEN]);
            out_probs.extend_from_slice(&probs[f * SPEAKERS..(f + 1) * SPEAKERS]);
        }
    }
    (out_embeds, out_probs)
}

/// Per (frame, speaker) evidence that this speaker alone is active: the log odds of
/// the speaker plus the log probability that every other one is silent (clamped at
/// [`SCORE_THRESHOLD`]). Frames where the speaker is not active score −∞; so do the
/// non-positive ones once a speaker has at least `BUDGET/2` positive frames.
fn frame_scores(probs: &[f32]) -> Vec<f32> {
    let n = probs.len() / SPEAKERS;
    let min_positive = (BUDGET as f32 * MIN_POSITIVE_RATE) as usize;
    let mut scores = vec![f32::NEG_INFINITY; n * SPEAKERS];
    let mut positive = [0usize; SPEAKERS];
    for (f, p) in probs.as_chunks::<SPEAKERS>().0.iter().enumerate() {
        let complements: Vec<f32> = p
            .iter()
            .map(|&x| (1.0 - x).max(SCORE_THRESHOLD).ln())
            .collect();
        let sum: f32 = complements.iter().sum();
        for s in 0..SPEAKERS {
            if p[s] > 0.5 {
                let score = p[s].max(SCORE_THRESHOLD).ln() - complements[s] + sum + 2f32.ln();
                scores[f * SPEAKERS + s] = score;
                positive[s] += usize::from(score > 0.0);
            }
        }
    }
    for s in 0..SPEAKERS {
        if positive[s] >= min_positive {
            for f in 0..n {
                let x = &mut scores[f * SPEAKERS + s];
                if *x <= 0.0 {
                    *x = f32::NEG_INFINITY;
                }
            }
        }
    }
    scores
}

/// Add `by` to each speaker's `k` highest-scoring frames.
fn boost(scores: &mut [f32], k: usize, by: f32) {
    let n = scores.len() / SPEAKERS;
    let mut order: Vec<usize> = (0..n).collect();
    for s in 0..SPEAKERS {
        order.sort_unstable_by(|&a, &b| {
            scores[b * SPEAKERS + s].total_cmp(&scores[a * SPEAKERS + s])
        });
        for &f in &order[..k.min(n)] {
            scores[f * SPEAKERS + s] += by;
        }
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Thread cap of the live diariser's session (07 §2): CPU seconds are the same at any
/// count, this keeps the STT tasks and capture threads on their cores.
pub const LIVE_INTRA_THREADS: usize = 4;

/// The streaming loop's state: session, front end and speaker cache for one
/// [`Geometry`]. [`Streamer::step`] scores one chunk; the offline pass
/// ([`probabilities`]) and the live diariser (`meeting::diar`) both drive it.
pub struct Streamer {
    sess: Session,
    mel: Mel,
    cache: SpeakerCache,
    geometry: Geometry,
    feats: Vec<f32>,
}

impl Streamer {
    /// Load `model` (`ort_lib` first) with `intra_threads` (`None`: ORT's default).
    pub fn open(
        model: &Path,
        data_dir: &Path,
        geometry: Geometry,
        intra_threads: Option<usize>,
    ) -> anyhow::Result<Streamer> {
        Ok(Streamer {
            sess: session(model, data_dir, intra_threads)?,
            mel: Mel::default(),
            cache: SpeakerCache::new(geometry),
            geometry,
            feats: Vec::new(),
        })
    }

    /// The geometry this streamer was opened with.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Score the chunk whose first mel frame is `m0` of `pcm` (16 kHz; frame `t` is
    /// centred on sample `160·t`, so `pcm` must reach a little before `160·m0` for the
    /// window's left half, or start at the recording's start). The chunk is
    /// `[m0, min(m0 + chunk·8, ⌊len/160⌋))`; the look-ahead after it is whatever `pcm`
    /// holds up to `right·8` frames — clamp at the end of `pcm` is the last-chunk
    /// behaviour (every frame scored, no look-ahead). Returns one row of `SPEAKERS`
    /// probabilities per 10 ms frame of the chunk (empty when `m0` is past the end).
    pub fn step(&mut self, pcm: &[f32], m0: usize) -> anyhow::Result<Vec<f32>> {
        let frames = pcm.len() / HOP;
        let m_end = (m0 + self.geometry.chunk * SUBSAMPLING).min(frames);
        if m_end <= m0 {
            return Ok(Vec::new());
        }
        let m_stop = (m_end + self.geometry.right * SUBSAMPLING).min(frames);
        let n_chunk = (m_end - m0).div_ceil(SUBSAMPLING);
        let n_fresh = (m_stop - m0).div_ceil(SUBSAMPLING);
        self.feats.clear();
        self.mel.frames(pcm, m0, m_stop, &mut self.feats);
        let cached = self.cache.cached();
        let n_cached = cached.len() / HIDDEN;
        let n_in = n_cached + n_fresh;
        let outputs = self.sess.run(ort::inputs![
            "input_features" => Tensor::from_array((vec![1i64, (m_stop - m0) as i64, MELS as i64], std::mem::take(&mut self.feats)))?,
            "cached_embeds" => Tensor::from_array((vec![1i64, n_cached as i64, HIDDEN as i64], cached))?,
            "attention_mask" => Tensor::from_array((vec![1i64, n_in as i64], vec![1i64; n_in]))?,
        ])?;
        let get = |name: &str| -> anyhow::Result<&[f32]> {
            let (_, data) = outputs
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("diarization: no `{name}` output"))?
                .try_extract_tensor::<f32>()?;
            Ok(data)
        };
        let logits = get("logits")?;
        anyhow::ensure!(
            logits.len() == n_in * SUBSAMPLING * SPEAKERS,
            "diarization: {} logits for {n_in} encoder frames",
            logits.len()
        );
        // The chunk's own frames, at 10 ms (a padded last group is cut to the real
        // frames); the look-ahead is scored again next step.
        let own =
            n_cached * SUBSAMPLING * SPEAKERS..(n_cached * SUBSAMPLING + (m_end - m0)) * SPEAKERS;
        let probs: Vec<f32> = logits[own].iter().map(|&x| sigmoid(x)).collect();
        // Pooled to encoder frames for the cache policy.
        let pooled: Vec<f32> = (0..n_in * SPEAKERS)
            .map(|i| {
                let (f, s) = (i / SPEAKERS, i % SPEAKERS);
                (0..SUBSAMPLING)
                    .map(|j| sigmoid(logits[((f * SUBSAMPLING + j) * SPEAKERS) + s]))
                    .sum::<f32>()
                    / SUBSAMPLING as f32
            })
            .collect();
        self.cache.update(
            &pooled,
            get("chunk_embeds")?,
            n_chunk,
            get("silence_embeds")?,
        );
        Ok(probs)
    }
}

/// Per-frame speaker probabilities of the whole recording (`⌊len/160⌋ × SPEAKERS`, one row
/// per 10 ms), from the streaming loop in the [`OFFLINE`] geometry, every step attending
/// to the speaker cache and FIFO. `progress(done, total)` in chunks.
pub fn probabilities(
    pcm: &[f32],
    model: &Path,
    data_dir: &Path,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<Vec<f32>> {
    let frames = pcm.len() / HOP;
    let mut out = Vec::with_capacity(frames * SPEAKERS);
    if frames == 0 {
        return Ok(out);
    }
    let mut streamer = Streamer::open(model, data_dir, OFFLINE, None)?;
    let step = OFFLINE.chunk * SUBSAMPLING;
    let chunks = frames.div_ceil(step);
    for (k, m0) in (0..frames).step_by(step).enumerate() {
        out.extend(streamer.step(pcm, m0)?);
        progress(k + 1, chunks);
    }
    Ok(out)
}

/// Turns from 10 ms probabilities (`frames × SPEAKERS`): a channel is active at or
/// above [`SPEECH`]; pauses under [`MERGE_GAP_MS`] close, turns under [`MIN_TURN_MS`]
/// drop. Labels `s1..` go to the channels in order of their first turn, so they
/// are dense even when the model skipped a channel.
pub fn turns(probs: &[f32]) -> Vec<Span> {
    let frames = probs.len() / SPEAKERS;
    let mut channels: Vec<Vec<(u64, u64)>> = Vec::with_capacity(SPEAKERS);
    for s in 0..SPEAKERS {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut open: Option<u64> = None;
        for t in 0..=frames {
            let active = t < frames && probs[t * SPEAKERS + s] >= SPEECH;
            match (open, active) {
                (None, true) => open = Some(t as u64 * STEP_MS),
                (Some(t0), false) => {
                    let t1 = t as u64 * STEP_MS;
                    match runs.last_mut() {
                        Some(last) if t0 - last.1 < MERGE_GAP_MS => last.1 = t1,
                        _ => runs.push((t0, t1)),
                    }
                    open = None;
                }
                _ => {}
            }
        }
        runs.retain(|(a, b)| b - a >= MIN_TURN_MS);
        if !runs.is_empty() {
            channels.push(runs);
        }
    }
    channels.sort_by_key(|runs| runs[0].0);
    let mut spans: Vec<Span> = channels
        .iter()
        .enumerate()
        .flat_map(|(i, runs)| {
            runs.iter().map(move |&(t0_ms, t1_ms)| Span {
                t0_ms,
                t1_ms,
                speaker: format!("s{}", i + 1),
            })
        })
        .collect();
    spans.sort_by_key(|s| (s.t0_ms, s.t1_ms));
    spans
}

/// Kaldi fbank of `pcm` (16 kHz): `(frames, bins, row-major data)`, mean-normalised per bin.
pub fn fbank(pcm: &[i16]) -> anyhow::Result<(usize, usize, Vec<f32>)> {
    let mut f = vec![0f32; pcm.len()];
    knf_rs::convert_integer_to_float_audio(pcm, &mut f);
    let feats = knf_rs::compute_fbank(&f).map_err(|e| anyhow::anyhow!("fbank: {e}"))?;
    let (t, bins) = feats.dim();
    Ok((t, bins, feats.iter().copied().collect()))
}

/// The wespeaker session: kaldi fbank (`knf-rs`) → `embs`.
pub struct Embedder(Session);

impl Embedder {
    /// Open the embedding model.
    pub fn open(models: &Models) -> anyhow::Result<Self> {
        Ok(Self(session(&models.embedding, &models.data_dir, None)?))
    }

    /// One embedding for `pcm` (16 kHz); at least a few hundred ms of audio.
    ///
    /// The frames are tiled (repeated from the start) up to a multiple of
    /// [`FRAMES`]: measured on this export, other counts return a large-norm
    /// vector unrelated to the voice — a 3 s cut of a speaker scored 0.00 against
    /// her 4 s cut while 2/4/6/8/10 s cuts scored 0.93–0.98, and the same 3 s
    /// tiled to 4 s scores 0.97. Repeat-padding is the usual trick for short
    /// utterances anyway.
    pub fn compute(&mut self, pcm: &[i16]) -> anyhow::Result<Vec<f32>> {
        let (frames, bins, data) = fbank(pcm)?;
        if frames == 0 {
            anyhow::bail!("embedding: too little audio");
        }
        let target = frames.div_ceil(FRAMES) * FRAMES;
        let mut tiled = Vec::with_capacity(target * bins);
        while tiled.len() < target * bins {
            let need = target * bins - tiled.len();
            tiled.extend_from_slice(&data[..need.min(data.len())]);
        }
        self.embed_frames(target, bins, tiled)
    }

    /// The model over the first `t` frames of `data` (`t × bins`, row-major).
    pub fn embed_frames(
        &mut self,
        t: usize,
        bins: usize,
        mut data: Vec<f32>,
    ) -> anyhow::Result<Vec<f32>> {
        data.truncate(t * bins);
        let input = Tensor::from_array((vec![1i64, t as i64, bins as i64], data))?;
        let outputs = self.0.run(ort::inputs!["feats" => input])?;
        let out = outputs
            .get("embs")
            .ok_or_else(|| anyhow::anyhow!("embedding: no `embs` output"))?;
        let (_, data) = out.try_extract_tensor::<f32>()?;
        Ok(data.to_vec())
    }
}

/// The whole stage on `pcm` (16 kHz, `[-1, 1]`). `progress(pct)` 0–100 within the stage.
pub fn run(pcm: &[f32], models: &Models, mut progress: impl FnMut(u8)) -> anyhow::Result<Diarized> {
    let probs = probabilities(pcm, &models.diarization, &models.data_dir, |done, total| {
        progress((done * 80 / total.max(1)) as u8);
    })?;
    let spans = turns(&probs);
    if spans.is_empty() {
        return Ok(Diarized::default());
    }
    // One embedding per label from its first ≤ 30 s of speech.
    let mut embedder = Embedder::open(models)?;
    let mut labels: Vec<&str> = spans.iter().map(|s| s.speaker.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    let sample = |ms: u64| (ms as usize * RATE as usize / 1000).min(pcm.len());
    let mut embeddings = HashMap::new();
    for (k, label) in labels.iter().enumerate() {
        let mut speech: Vec<f32> = Vec::new();
        for s in spans.iter().filter(|s| s.speaker == *label) {
            speech.extend_from_slice(&pcm[sample(s.t0_ms)..sample(s.t1_ms)]);
            if speech.len() >= EMBED_SAMPLES {
                break;
            }
        }
        if speech.len() >= MIN_EMBED_SAMPLES {
            embeddings.insert((*label).to_owned(), embedder.compute(&to_i16(&speech))?);
        }
        progress((80 + (k + 1) * 20 / labels.len()) as u8);
    }
    Ok(Diarized { spans, embeddings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_merge_short_pauses_drop_blips_and_number_by_first_appearance() {
        // 10 ms frames: channel 2 speaks 0–1 s; channel 0 speaks 1.2–2 s and, after a
        // 300 ms pause, 2.3–3 s (merged); channel 5 has one 100 ms blip (dropped).
        let frames = 300;
        let mut probs = vec![0f32; frames * SPEAKERS];
        for t in 0..100 {
            probs[t * SPEAKERS + 2] = 0.9;
        }
        for t in (120..200).chain(230..300) {
            probs[t * SPEAKERS] = 0.7;
        }
        for t in 150..160 {
            probs[t * SPEAKERS + 5] = 1.0;
        }
        let spans = turns(&probs);
        assert_eq!(
            spans,
            [
                Span {
                    t0_ms: 0,
                    t1_ms: 1000,
                    speaker: "s1".into()
                },
                Span {
                    t0_ms: 1200,
                    t1_ms: 3000,
                    speaker: "s2".into()
                },
            ]
        );
    }

    #[test]
    fn compress_keeps_cache_size_groups_by_speaker_and_seats_silence() {
        // 300 frames: speaker 0 alone in the first half, speaker 1 in the second,
        // frame 299 both (an overlap, which scores below a clean frame).
        let n = 300;
        let mut probs = vec![0.01f32; n * SPEAKERS];
        for f in 0..n {
            probs[f * SPEAKERS + usize::from(f >= 150)] = 0.95;
        }
        probs[299 * SPEAKERS] = 0.9;
        let embeds: Vec<f32> = (0..n)
            .flat_map(|f| std::iter::repeat_n(f as f32, HIDDEN))
            .collect();
        let silence = vec![-1f32; HIDDEN];
        let (e, p) = compress(&embeds, &probs, &silence);
        assert_eq!((e.len(), p.len()), (CACHE * HIDDEN, CACHE * SPEAKERS));
        let ids: Vec<f32> = e.as_chunks::<HIDDEN>().0.iter().map(|r| r[0]).collect();
        // Speaker 0's frames first (in time order), then their silence slot, then speaker
        // 1's, then six more silence slots for the speakers never heard.
        let silences = ids.iter().filter(|&&x| x == -1.0).count();
        assert_eq!(silences, SPEAKERS * SILENCE_SLOTS, "{ids:?}");
        let first_silence = ids.iter().position(|&x| x == -1.0).unwrap();
        assert!(
            ids[..first_silence]
                .windows(2)
                .all(|w| w[0] < w[1] && w[1] < 150.0),
            "{ids:?}"
        );
        let spk1: Vec<f32> = ids[first_silence + 1..]
            .iter()
            .copied()
            .filter(|&x| x >= 0.0)
            .collect();
        assert!(
            spk1.windows(2).all(|w| w[0] < w[1]) && spk1.iter().all(|&x| x >= 150.0),
            "{spk1:?}"
        );
        // The overlap frame lost to the clean ones.
        assert!(!ids.contains(&299.0), "{ids:?}");
        // The silence slots carry zero probability.
        let silent_probs: Vec<&[f32]> = p
            .as_chunks::<SPEAKERS>()
            .0
            .iter()
            .zip(&ids)
            .filter(|(_, id)| **id == -1.0)
            .map(|(row, _)| row.as_slice())
            .collect();
        assert!(silent_probs.iter().all(|r| r.iter().all(|&x| x == 0.0)));
    }

    #[test]
    fn fifo_overflow_moves_frames_to_the_cache_and_compresses_it() {
        let (chunk_len, fifo) = (OFFLINE.chunk, OFFLINE.fifo);
        let mut cache = SpeakerCache::new(OFFLINE);
        let silence = vec![0f32; HIDDEN];
        // First chunk: 340 frames + 40 look-ahead, nothing cached yet.
        let step = |n_in: usize| vec![0.01f32; n_in * SPEAKERS];
        let chunk = |n: usize| vec![1f32; n * HIDDEN];
        cache.update(&step(380), &chunk(380), chunk_len, &silence);
        // 340 > FIFO 40 → 300 popped to the cache (> 264 → compressed), 40 stay.
        assert_eq!(cache.embeds.len() / HIDDEN, CACHE);
        assert_eq!(cache.fifo.len() / HIDDEN, fifo);
        assert!(cache.compressed);
        // Second chunk attends to 304 cached frames.
        assert_eq!(cache.cached().len() / HIDDEN, CACHE + fifo);
        cache.update(&step(CACHE + fifo + 380), &chunk(380), chunk_len, &silence);
        assert_eq!(cache.embeds.len() / HIDDEN, CACHE);
        assert_eq!(cache.fifo.len() / HIDDEN, fifo);
        // A 10-frame last chunk still overflows the 40-frame FIFO: all 50 move to the cache.
        cache.update(&step(CACHE + fifo + 10), &chunk(10), 10, &silence);
        assert_eq!(cache.fifo.len() / HIDDEN, 0);
        assert_eq!(cache.embeds.len() / HIDDEN, CACHE);
    }

    #[test]
    fn live_fifo_fills_before_the_cache_takes_an_update_period() {
        // LIVE: 80-frame chunks, FIFO 264, update 222. Three chunks (240) fit; the fourth
        // overflows: 222 move to the cache (uncompressed, ≤ 264), 98 stay.
        let mut cache = SpeakerCache::new(LIVE);
        let silence = vec![0f32; HIDDEN];
        let step = |n_in: usize| vec![0.01f32; n_in * SPEAKERS];
        let chunk = |n: usize| vec![1f32; n * HIDDEN];
        for k in 0..3 {
            cache.update(&step(k * 80 + 100), &chunk(100), 80, &silence);
        }
        assert_eq!((cache.embeds.len(), cache.fifo.len() / HIDDEN), (0, 240));
        cache.update(&step(240 + 100), &chunk(100), 80, &silence);
        assert_eq!(cache.embeds.len() / HIDDEN, LIVE.update);
        assert_eq!(cache.fifo.len() / HIDDEN, 320 - LIVE.update);
        assert!(!cache.compressed);
        assert_eq!(LIVE.buffer_ms(), 8_000);
        assert_eq!(OFFLINE.buffer_ms(), 30_400);
    }

    #[test]
    fn mel_matches_the_reference_extractor() {
        // `NemotronAsrStreamingFeatureExtractor` (transformers 5.18 dev, `nvidia/Nemotron-3-
        // Diarization` preprocessor config) over 0.5 s of two tones, a chirp and a DC step:
        // 50 valid frames; the values below are its output at scattered (frame, bin) points.
        let pcm: Vec<f32> = (0..8000)
            .map(|n| {
                let n = n as f64;
                let x = 0.3 * (2.0 * std::f64::consts::PI * 440.0 * n / 16000.0).sin()
                    + 0.2 * (2.0 * std::f64::consts::PI * 3100.0 * n / 16000.0 + 1.0).sin()
                    + 0.1 * (2.0 * std::f64::consts::PI * (200.0 + 0.5 * n) * n / 16000.0).sin();
                (x + if n >= 4000.0 { 0.05 } else { 0.0 }) as f32
            })
            .collect();
        let mut mel = Mel::default();
        let mut feats = Vec::new();
        mel.frames(&pcm, 0, pcm.len() / HOP, &mut feats);
        assert_eq!(feats.len(), 50 * MELS);
        let expected = [
            (0, 0, -8.0663),
            (0, 5, -6.1975),
            (0, 64, -8.4057),
            (0, 127, -8.9841),
            (1, 13, -3.93),
            (10, 3, -15.334),
            (10, 27, -12.365),
            (10, 100, -16.576),
            (25, 0, -7.0942),
            (25, 50, -9.4205),
            (25, 99, -3.897),
            (37, 10, -12.345),
            (49, 64, -12.984),
            (49, 127, 0.37891),
        ];
        for (t, b, want) in expected {
            let got = feats[t * MELS + b];
            assert!(
                (got - want).abs() < 2e-3,
                "frame {t} bin {b}: got {got}, reference {want}"
            );
        }
        let sum: f32 = feats.iter().sum();
        assert!((sum - -81538.39).abs() < 1.0, "sum {sum}");
    }
}
