//! Live diarisation feasibility bench (`docs/design/meetings/07-live-diarization.md` §1):
//! runs the Nemotron 3 Diarization graph (`model_quantized`, CPU EP) in the offline and
//! the low-latency streaming geometries of the model card (plus a few variants) over
//! one recorded system track, and reports per-step latency, real-time factor, CPU
//! seconds per audio second, speakers found, and agreement with the offline pass on
//! the same audio. `--stt` also runs the live STT `Utterer` over the same audio, alone
//! and then concurrently with the `low` diariser, for the shared-core question.
//!
//! ```text
//! cargo run --release -p meeting --example live_diar_bench -- <track.opus|wav> \
//!     [--words transcript.json] [--stt] [--secs N] [--threads N] [--configs a,b,…] [--data-dir ~/.theuth]
//! ```
//!
//! `--threads` sets the diariser session's intra-op threads (ORT's default: physical
//! cores). `--configs` picks from the names in [`GEOMETRIES`]; `offline` is always run
//! first as the reference.
//!
//! Throwaway: the streaming loop and `SpeakerCache` below are a copy of
//! `post::diarize`'s with FIFO length and cache update period as parameters (the
//! production ones are consts with the offline sizes). Everything else — `Mel`,
//! `turns`, the model path, `ort` setup — is the production code.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use meeting::post::diarize::{DIAR_DIR, DIAR_MODEL, Mel, RATE, SPEECH, turns};
use meeting::post::transcribe::pcm16k;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

// Geometry shared with `post::diarize`.
const HOP: usize = 160;
const MELS: usize = 128;
const SUBSAMPLING: usize = 8;
const HIDDEN: usize = 512;
const SPEAKERS: usize = 8;
const CACHE: usize = 264;
const SILENCE_SLOTS: usize = 1;
const SCORE_THRESHOLD: f32 = 0.25;
const LATEST_BOOST: f32 = 0.05;
const MIN_POSITIVE_RATE: f32 = 0.5;
const STRONG_BOOST_RATE: f32 = 0.75;
const WEAK_BOOST_RATE: f32 = 1.5;
const BUDGET: usize = CACHE / SPEAKERS - SILENCE_SLOTS;
const FRAME_MS: f32 = 10.0;

/// One streaming setup of the model card (all in 80 ms encoder frames).
#[derive(Clone, Copy)]
struct Geometry {
    name: &'static str,
    chunk: usize,
    right: usize,
    fifo: usize,
    update: usize,
}

impl Geometry {
    /// Input buffer latency: the audio that must arrive before a chunk is scored.
    fn buffer_ms(&self) -> usize {
        (self.chunk + self.right) * SUBSAMPLING * 10
    }
}

/// The model card's four rows, then the variants this bench adds: `mid` doubles the
/// chunk (1.76 s buffer, half the steps); `low_fifo124` / `low_fifo40` shrink the FIFO
/// the step re-attends (the offline policy's 40 with its update period of 300);
/// `mid_fifo40` does both.
const GEOMETRIES: [Geometry; 8] = [
    Geometry { name: "offline", chunk: 340, right: 40, fifo: 40, update: 300 },
    Geometry { name: "low", chunk: 9, right: 4, fifo: 264, update: 222 },
    Geometry { name: "very_low", chunk: 6, right: 2, fifo: 264, update: 222 },
    Geometry { name: "ultra_low", chunk: 3, right: 1, fifo: 264, update: 222 },
    Geometry { name: "mid", chunk: 18, right: 4, fifo: 264, update: 222 },
    Geometry { name: "low_fifo124", chunk: 9, right: 4, fifo: 124, update: 100 },
    Geometry { name: "low_fifo40", chunk: 9, right: 4, fifo: 40, update: 300 },
    Geometry { name: "mid_fifo40", chunk: 18, right: 4, fifo: 40, update: 300 },
];
/// Without `--configs`: the model card's rows.
const DEFAULT_CONFIGS: &str = "low,very_low,ultra_low";

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut audio: Option<PathBuf> = None;
    let mut words: Option<PathBuf> = None;
    let mut stt = false;
    let mut secs: Option<usize> = None;
    let mut threads: Option<usize> = None;
    let mut configs = DEFAULT_CONFIGS.to_owned();
    let mut data_dir = dirs_home().join(".theuth");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--words" => words = args.next().map(PathBuf::from),
            "--stt" => stt = true,
            "--secs" => secs = args.next().and_then(|s| s.parse().ok()),
            "--threads" => threads = args.next().and_then(|s| s.parse().ok()),
            "--configs" => configs = args.next().unwrap_or_default(),
            "--data-dir" => data_dir = args.next().map(PathBuf::from).unwrap_or(data_dir),
            _ => audio = Some(PathBuf::from(a)),
        }
    }
    let audio = audio.ok_or_else(|| {
        anyhow::anyhow!("usage: live_diar_bench <track> [--words t.json] [--stt] [--secs N] [--threads N] [--configs a,b]")
    })?;
    let model = data_dir.join("models").join(DIAR_DIR).join(DIAR_MODEL);
    anyhow::ensure!(model.is_file(), "no model at {}", model.display());
    let chosen: Vec<&Geometry> = std::iter::once(&GEOMETRIES[0])
        .chain(configs.split(',').filter_map(|n| GEOMETRIES.iter().find(|g| g.name == n.trim())))
        .collect();

    let mut pcm = pcm16k(&audio)?;
    if let Some(s) = secs {
        pcm.truncate(s * RATE as usize);
    }
    let audio_s = pcm.len() as f64 / f64::from(RATE);
    println!(
        "audio: {} · {:.1} s · model {} · {} cores · intra-op threads {}",
        audio.display(),
        audio_s,
        model.display(),
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        threads.map_or("default".to_owned(), |t| t.to_string())
    );

    let mut sess = session(&model, &data_dir, threads)?;
    // Warm-up: the first `run` pays for kernel selection and arena growth.
    let _ = stream(&pcm[..pcm.len().min(RATE as usize * 40)], &mut sess, &GEOMETRIES[1]);

    let mut runs = Vec::new();
    for g in chosen {
        let r = stream(&pcm, &mut sess, g);
        report(&r, audio_s);
        runs.push(r);
    }

    let system_words = words.as_deref().map(load_words).transpose()?.unwrap_or_default();
    println!();
    println!(
        "agreement vs offline (channels matched by overlap; DER = miss+false+confusion over offline speech){}",
        if system_words.is_empty() { "" } else { "; words = stored system-track words by midpoint" }
    );
    println!(
        "{:<12} {:>5} {:>10} {:>8} {:>9} {:>10} {:>14} {}",
        "config", "spk", "frames=", "DER", "words=", "unassigned", "unassigned-off", "channel map"
    );
    let offline = &runs[0];
    for r in &runs {
        let a = agreement(&offline.probs, &r.probs, &system_words);
        println!(
            "{:<12} {:>5} {:>9.1}% {:>7.1}% {:>8.1}% {:>9.1}% {:>13.1}% {}",
            r.geometry.name,
            r.speakers,
            a.frame_agree * 100.0,
            a.der * 100.0,
            a.word_agree * 100.0,
            a.hyp_unassigned * 100.0,
            a.ref_unassigned * 100.0,
            a.map
        );
    }

    if stt {
        stt_bench(&pcm, &data_dir, &model, audio_s, threads)?;
    }
    println!();
    println!("peak RSS: {} MB", max_rss_mb());
    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `ort::Error<R>` is not `Send + Sync` (it carries the failed builder); stringify.
fn ort_err<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// As `post::diarize::session`, plus the optional intra-op thread count.
fn session(path: &Path, data_dir: &Path, threads: Option<usize>) -> anyhow::Result<Session> {
    models::onnx::ort_lib(data_dir).map_err(anyhow::Error::msg)?;
    let mut builder = Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?;
    if let Some(t) = threads {
        builder = builder.with_intra_threads(t).map_err(ort_err)?;
    }
    models::onnx::configure(builder)
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("load {}: {e}", path.display()))
}

/// One pass over the recording in one geometry.
struct Run {
    geometry: Geometry,
    /// `frames × SPEAKERS` at 10 ms.
    probs: Vec<f32>,
    /// Per step: features + `run` + cache update.
    step_ms: Vec<f32>,
    wall: Duration,
    cpu: Duration,
    speakers: usize,
    turns: usize,
}

/// `post::diarize::probabilities` with the geometry as a parameter and per-step timing.
fn stream(pcm: &[f32], sess: &mut Session, g: &Geometry) -> Run {
    let frames = pcm.len() / HOP;
    let encoder_frames = frames.div_ceil(SUBSAMPLING);
    let mut out = Vec::with_capacity(frames * SPEAKERS);
    let mut mel = Mel::default();
    let mut cache = SpeakerCache::new(g.fifo, g.update);
    let mut feats = Vec::new();
    let mut step_ms = Vec::with_capacity(encoder_frames / g.chunk + 1);
    let cpu0 = cpu_time();
    let t0 = Instant::now();
    for start in (0..encoder_frames).step_by(g.chunk) {
        let t = Instant::now();
        let end = (start + g.chunk).min(encoder_frames);
        let stop = (end + g.right).min(encoder_frames);
        let n_chunk = end - start;
        let (m0, m1) = (start * SUBSAMPLING, (stop * SUBSAMPLING).min(frames));
        feats.clear();
        mel.frames(pcm, m0, m1, &mut feats);
        let cached = cache.cached();
        let n_cached = cached.len() / HIDDEN;
        let n_in = n_cached + stop - start;
        let outputs = sess
            .run(ort::inputs![
                "input_features" => Tensor::from_array((vec![1i64, (m1 - m0) as i64, MELS as i64], std::mem::take(&mut feats))).expect("shape"),
                "cached_embeds" => Tensor::from_array((vec![1i64, n_cached as i64, HIDDEN as i64], cached)).expect("shape"),
                "attention_mask" => Tensor::from_array((vec![1i64, n_in as i64], vec![1i64; n_in])).expect("shape"),
            ])
            .expect("run");
        let get = |name: &str| -> &[f32] { outputs[name].try_extract_tensor::<f32>().expect("f32").1 };
        let logits = get("logits");
        assert_eq!(logits.len(), n_in * SUBSAMPLING * SPEAKERS);
        let own = n_cached * SUBSAMPLING * SPEAKERS..(n_cached + n_chunk) * SUBSAMPLING * SPEAKERS;
        out.extend(logits[own].iter().map(|&x| sigmoid(x)));
        let pooled: Vec<f32> = (0..n_in * SPEAKERS)
            .map(|i| {
                let (f, s) = (i / SPEAKERS, i % SPEAKERS);
                (0..SUBSAMPLING)
                    .map(|j| sigmoid(logits[((f * SUBSAMPLING + j) * SPEAKERS) + s]))
                    .sum::<f32>()
                    / SUBSAMPLING as f32
            })
            .collect();
        cache.update(&pooled, get("chunk_embeds"), n_chunk, get("silence_embeds"));
        step_ms.push(t.elapsed().as_secs_f32() * 1000.0);
    }
    let wall = t0.elapsed();
    let cpu = cpu_time() - cpu0;
    out.truncate(frames * SPEAKERS);
    let spans = turns(&out);
    let mut labels: Vec<&str> = spans.iter().map(|s| s.speaker.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    Run { geometry: *g, probs: out, step_ms, wall, cpu, speakers: labels.len(), turns: spans.len() }
}

fn report(r: &Run, audio_s: f64) {
    let mut sorted = r.step_ms.clone();
    sorted.sort_by(f32::total_cmp);
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    let g = &r.geometry;
    println!();
    println!(
        "[{}] chunk {} · right {} · fifo {} · update {} · buffer {} ms",
        g.name, g.chunk, g.right, g.fifo, g.update, g.buffer_ms()
    );
    println!(
        "  steps {} · step ms p50 {:.1} · p95 {:.1} · max {:.1} · wall {:.1} s · RTF {:.3} · cpu {:.1} s = {:.0}% of one core live · threads×{:.1}",
        sorted.len(),
        pct(0.5),
        pct(0.95),
        sorted[sorted.len() - 1],
        r.wall.as_secs_f64(),
        r.wall.as_secs_f64() / audio_s,
        r.cpu.as_secs_f64(),
        r.cpu.as_secs_f64() / audio_s * 100.0,
        r.cpu.as_secs_f64() / r.wall.as_secs_f64()
    );
    println!(
        "  label latency ≈ {} ms buffer + {:.0} ms p95 compute = {:.2} s · speakers {} · turns {} · per-channel speech s: {}",
        g.buffer_ms(),
        pct(0.95),
        (g.buffer_ms() as f32 + pct(0.95)) / 1000.0,
        r.speakers,
        r.turns,
        channel_seconds(&r.probs)
    );
}

fn channel_seconds(probs: &[f32]) -> String {
    let mut s = [0usize; SPEAKERS];
    for row in probs.chunks_exact(SPEAKERS) {
        for (k, &p) in row.iter().enumerate() {
            s[k] += usize::from(p >= SPEECH);
        }
    }
    s.iter().map(|&n| format!("{:.0}", n as f32 * FRAME_MS / 1000.0)).collect::<Vec<_>>().join("/")
}

struct Agreement {
    frame_agree: f64,
    der: f64,
    word_agree: f64,
    hyp_unassigned: f64,
    ref_unassigned: f64,
    /// `hyp channel → offline channel` for channels with speech, e.g. `0→0 1→1 2→3`.
    map: String,
}

/// `hyp` against `reference` (both `frames × SPEAKERS`): channels matched by the
/// permutation maximising co-active frames (8! is cheap), then frame-set agreement,
/// a collar-less DER, and — per stored system word — the same speaker at its midpoint.
fn agreement(reference: &[f32], hyp: &[f32], words: &[(u64, u64)]) -> Agreement {
    let frames = (reference.len() / SPEAKERS).min(hyp.len() / SPEAKERS);
    let active = |p: &[f32], t: usize, s: usize| p[t * SPEAKERS + s] >= SPEECH;
    let mut overlap = [[0usize; SPEAKERS]; SPEAKERS];
    let mut hyp_frames = [0usize; SPEAKERS];
    for t in 0..frames {
        for h in 0..SPEAKERS {
            hyp_frames[h] += usize::from(active(hyp, t, h));
        }
        for r in 0..SPEAKERS {
            if active(reference, t, r) {
                for h in 0..SPEAKERS {
                    overlap[r][h] += usize::from(active(hyp, t, h));
                }
            }
        }
    }
    let perm = best_permutation(&overlap); // perm[h] = r
    let (mut same, mut err, mut ref_speech) = (0usize, 0usize, 0usize);
    for t in 0..frames {
        let mut ref_set = [false; SPEAKERS];
        let mut hyp_set = [false; SPEAKERS];
        for s in 0..SPEAKERS {
            ref_set[s] = active(reference, t, s);
            if active(hyp, t, s) {
                hyp_set[perm[s]] = true;
            }
        }
        same += usize::from(ref_set == hyp_set);
        let nr = ref_set.iter().filter(|&&b| b).count();
        let nh = hyp_set.iter().filter(|&&b| b).count();
        let nc = (0..SPEAKERS).filter(|&s| ref_set[s] && hyp_set[s]).count();
        err += nr.max(nh) - nc;
        ref_speech += nr;
    }
    // Speaker at a word's midpoint: the most probable active channel, else none.
    let speaker_at = |p: &[f32], ms: u64| -> Option<usize> {
        let t = (ms / FRAME_MS as u64) as usize;
        if t >= frames {
            return None;
        }
        (0..SPEAKERS)
            .filter(|&s| active(p, t, s))
            .max_by(|&a, &b| p[t * SPEAKERS + a].total_cmp(&p[t * SPEAKERS + b]))
    };
    let (mut w_same, mut w_hyp_none, mut w_ref_none) = (0usize, 0usize, 0usize);
    for &(t0, t1) in words {
        let mid = (t0 + t1) / 2;
        let r = speaker_at(reference, mid);
        let h = speaker_at(hyp, mid).map(|h| perm[h]);
        w_same += usize::from(r == h);
        w_hyp_none += usize::from(h.is_none());
        w_ref_none += usize::from(r.is_none());
    }
    let n_words = words.len().max(1) as f64;
    let map = (0..SPEAKERS)
        .filter(|&h| hyp_frames[h] >= 100)
        .map(|h| format!("{h}→{}", perm[h]))
        .collect::<Vec<_>>()
        .join(" ");
    Agreement {
        frame_agree: same as f64 / frames.max(1) as f64,
        der: err as f64 / ref_speech.max(1) as f64,
        word_agree: w_same as f64 / n_words,
        hyp_unassigned: w_hyp_none as f64 / n_words,
        ref_unassigned: w_ref_none as f64 / n_words,
        map,
    }
}

/// `perm[h] = r` maximising `Σ overlap[perm[h]][h]`, by exhaustive search; ties keep
/// the identity where possible.
fn best_permutation(overlap: &[[usize; SPEAKERS]; SPEAKERS]) -> [usize; SPEAKERS] {
    fn go(
        h: usize,
        used: u8,
        cur: &mut [usize; SPEAKERS],
        best: &mut (usize, [usize; SPEAKERS]),
        acc: usize,
        overlap: &[[usize; SPEAKERS]; SPEAKERS],
    ) {
        if h == SPEAKERS {
            if acc > best.0 {
                *best = (acc, *cur);
            }
            return;
        }
        for r in 0..SPEAKERS {
            if used & (1 << r) == 0 {
                cur[h] = r;
                go(h + 1, used | (1 << r), cur, best, acc + overlap[r][h], overlap);
            }
        }
    }
    let identity: [usize; SPEAKERS] = std::array::from_fn(|i| i);
    let mut best = ((0..SPEAKERS).map(|i| overlap[i][i]).sum(), identity);
    go(0, 0, &mut [0; SPEAKERS], &mut best, 0, overlap);
    best.1
}

/// `(t0_ms, t1_ms)` of the stored transcript's system-track words.
fn load_words(path: &Path) -> anyhow::Result<Vec<(u64, u64)>> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut out = Vec::new();
    for seg in v["segments"].as_array().into_iter().flatten() {
        if seg["track"] != "System" {
            continue;
        }
        for w in seg["words"].as_array().into_iter().flatten() {
            if let (Some(a), Some(b)) = (w["t0_ms"].as_u64(), w["t1_ms"].as_u64()) {
                out.push((a, b));
            }
        }
    }
    Ok(out)
}

/// The live STT path (`voice::live::Utterer`: VAD + gate + Nemotron streaming, 20 ms
/// pushes as the capture ring delivers) alone, then concurrently with the `low`
/// diariser on a second thread.
fn stt_bench(pcm: &[f32], data_dir: &Path, model: &Path, audio_s: f64, threads: Option<usize>) -> anyhow::Result<()> {
    let dir = data_dir.join("models").join(voice::STREAMING_MODEL_SUBDIR);
    println!();
    println!("live STT ({}) over the same audio, 20 ms pushes", dir.display());
    let run_stt = |pcm: &[f32]| -> anyhow::Result<(Duration, Duration, usize)> {
        let mut live = voice::live::Utterer::open(&dir, data_dir)?;
        let cpu0 = cpu_time();
        let t0 = Instant::now();
        let mut pieces = 0;
        for chunk in pcm.chunks(RATE as usize / 50) {
            pieces += live.chunk(chunk)?.into_iter().filter(|t| t.is_final).count();
        }
        pieces += live.close()?.len();
        Ok((t0.elapsed(), cpu_time() - cpu0, pieces))
    };
    let (wall, cpu, utterances) = run_stt(pcm)?;
    println!(
        "  alone: wall {:.1} s · RTF {:.3} · cpu {:.1} s = {:.0}% of one core · {} utterances",
        wall.as_secs_f64(),
        wall.as_secs_f64() / audio_s,
        cpu.as_secs_f64(),
        cpu.as_secs_f64() / audio_s * 100.0,
        utterances
    );
    let low = GEOMETRIES[1];
    // CPU time is process-wide: the concurrent figures below are the pair's total.
    let cpu0 = cpu_time();
    let diar = std::thread::scope(|s| -> anyhow::Result<(Run, (Duration, Duration, usize))> {
        let stt = s.spawn(|| run_stt(pcm));
        let mut sess = session(model, data_dir, threads)?;
        let r = stream(pcm, &mut sess, &low);
        Ok((r, stt.join().expect("stt thread")?))
    })?;
    let both_cpu = cpu_time() - cpu0;
    let (r, (wall, _, _)) = diar;
    let mut sorted = r.step_ms.clone();
    sorted.sort_by(f32::total_cmp);
    println!(
        "  concurrent with `low` diariser: STT wall {:.1} s RTF {:.3} · diariser wall {:.1} s RTF {:.3} step p50 {:.1} p95 {:.1} ms · pair cpu {:.1} s = {:.0}% of one core",
        wall.as_secs_f64(),
        wall.as_secs_f64() / audio_s,
        r.wall.as_secs_f64(),
        r.wall.as_secs_f64() / audio_s,
        sorted[sorted.len() / 2],
        sorted[(sorted.len() - 1) * 95 / 100],
        both_cpu.as_secs_f64(),
        both_cpu.as_secs_f64() / audio_s * 100.0
    );
    Ok(())
}

/// Process CPU time (user + system).
fn cpu_time() -> Duration {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, (t.tv_usec as u32) * 1000);
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

fn max_rss_mb() -> u64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    // macOS reports bytes, Linux kilobytes.
    if cfg!(target_os = "macos") { ru.ru_maxrss as u64 >> 20 } else { ru.ru_maxrss as u64 >> 10 }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// `post::diarize::SpeakerCache` with `fifo` and `update` as fields.
struct SpeakerCache {
    fifo_len: usize,
    update_period: usize,
    embeds: Vec<f32>,
    probs: Vec<f32>,
    fifo: Vec<f32>,
    compressed: bool,
}

impl SpeakerCache {
    fn new(fifo_len: usize, update_period: usize) -> Self {
        Self { fifo_len, update_period, embeds: Vec::new(), probs: Vec::new(), fifo: Vec::new(), compressed: false }
    }

    fn cached(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(self.embeds.len() + self.fifo.len());
        v.extend_from_slice(&self.embeds);
        v.extend_from_slice(&self.fifo);
        v
    }

    fn update(&mut self, probs: &[f32], chunk_embeds: &[f32], n_chunk: usize, silence: &[f32]) {
        let n_cache = self.embeds.len() / HIDDEN;
        let n_fifo = self.fifo.len() / HIDDEN;
        let mut fifo = std::mem::take(&mut self.fifo);
        fifo.extend_from_slice(&chunk_embeds[..n_chunk * HIDDEN]);
        let len = n_fifo + n_chunk;
        let popped = if len <= self.fifo_len { 0 } else { self.update_period.max(len - self.fifo_len).min(len) };
        if popped > 0 {
            let mut cache_probs =
                if self.compressed { std::mem::take(&mut self.probs) } else { probs[..n_cache * SPEAKERS].to_vec() };
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

fn compress(embeds: &[f32], probs: &[f32], silence: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = probs.len() / SPEAKERS;
    let mut scores = frame_scores(probs);
    for s in &mut scores[CACHE * SPEAKERS..] {
        *s += LATEST_BOOST;
    }
    let ln2 = 2f32.ln();
    boost(&mut scores, (BUDGET as f32 * STRONG_BOOST_RATE) as usize, 2.0 * ln2);
    boost(&mut scores, (BUDGET as f32 * WEAK_BOOST_RATE) as usize, ln2);
    let rows = n + SILENCE_SLOTS;
    let sentinel = rows * SPEAKERS;
    let mut all: Vec<(f32, usize)> = Vec::with_capacity(sentinel);
    for s in 0..SPEAKERS {
        all.extend((0..n).map(|f| (scores[f * SPEAKERS + s], s * rows + f)));
        all.extend((n..rows).map(|f| (f32::INFINITY, s * rows + f)));
    }
    all.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut keep: Vec<usize> =
        all[..CACHE].iter().map(|&(score, i)| if score == f32::NEG_INFINITY { sentinel } else { i }).collect();
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

fn frame_scores(probs: &[f32]) -> Vec<f32> {
    let n = probs.len() / SPEAKERS;
    let min_positive = (BUDGET as f32 * MIN_POSITIVE_RATE) as usize;
    let mut scores = vec![f32::NEG_INFINITY; n * SPEAKERS];
    let mut positive = [0usize; SPEAKERS];
    for (f, p) in probs.chunks_exact(SPEAKERS).enumerate() {
        let complements: Vec<f32> = p.iter().map(|&x| (1.0 - x).max(SCORE_THRESHOLD).ln()).collect();
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

fn boost(scores: &mut [f32], k: usize, by: f32) {
    let n = scores.len() / SPEAKERS;
    let mut order: Vec<usize> = (0..n).collect();
    for s in 0..SPEAKERS {
        order.sort_unstable_by(|&a, &b| scores[b * SPEAKERS + s].total_cmp(&scores[a * SPEAKERS + s]));
        for &f in &order[..k.min(n)] {
            scores[f * SPEAKERS + s] += by;
        }
    }
}
