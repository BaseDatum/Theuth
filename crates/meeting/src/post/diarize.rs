//! Speaker diarization on the system track (04 §2 stage 2, 53 decision 6):
//! pyannote `segmentation-3.0` for speech spans, wespeaker CAM++ for one
//! embedding per span, greedy cosine clustering into `s1..sn`. Runs on `ort`
//! directly with the two ONNX files `pyannote-rs` 0.3.4 loads (`BUILDING.md`)
//! and the same windowing as its `segment.rs`; that crate itself does not
//! compile against the workspace `ort` (04 §10). [`session`] runs the
//! process-wide `models::onnx::ort_lib` first (a no-op once voice has).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::post::align::Span;
use crate::speakers::cosine;

/// The segmentation model file (`pyannote-rs` `BUILDING.md`).
pub const SEGMENTATION: &str = "segmentation-3.0.onnx";
/// The embedding model file.
pub const EMBEDDING: &str = "wespeaker_en_voxceleb_CAM++.onnx";
/// Input rate of both models.
pub const RATE: u32 = 16_000;
/// Clusters merge while their centroid cosine is at least this. 04 §2 said 0.75:
/// on a clean two-voice WAV same-voice spans score 0.82–0.96, but on the same
/// clip played through speakers and tapped (the phase-2 recording) the shorter
/// spans of one voice score 0.57–0.85 and 0.75 split it into four; different
/// voices score 0.06–0.24 on both. 0.5 (`pyannote-rs`'s example) sits in the gap (04 §10).
pub const CLUSTER: f32 = 0.5;
/// Spans shorter than this reuse the nearest longer span's label instead of
/// getting their own (unreliable) embedding.
const MIN_EMBED_MS: u64 = 1_000;
/// The embedding model's frame period (2 s at 10 ms): inputs are tiled to a multiple.
pub const FRAMES: usize = 200;
/// Segmentation window (10 s) and its frame geometry, as `pyannote-rs` uses them.
const WINDOW: usize = 10 * RATE as usize;
const FRAME: usize = 270;
const FRAME_START: usize = 721;

/// The two model files, both present.
#[derive(Clone, Debug)]
pub struct Models {
    /// `segmentation-3.0.onnx`.
    pub segmentation: PathBuf,
    /// `wespeaker_en_voxceleb_CAM++.onnx`.
    pub embedding: PathBuf,
}

/// Both files under `dir`, or `None` (04 §0.1: the pipeline then runs without diarization).
pub fn models(dir: &Path) -> Option<Models> {
    let m = Models {
        segmentation: dir.join(SEGMENTATION),
        embedding: dir.join(EMBEDDING),
    };
    (m.segmentation.is_file() && m.embedding.is_file()).then_some(m)
}

/// What the stage produces: labelled spans and one mean embedding per label.
#[derive(Clone, Debug, Default)]
pub struct Diarized {
    /// Speech spans on the system track with their cluster labels.
    pub spans: Vec<Span>,
    /// Label → mean embedding of its spans.
    pub embeddings: HashMap<String, Vec<f32>>,
}

/// `ort::Error<R>` is not `Send + Sync` (it carries the failed builder); stringify.
fn ort_err<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn session(path: &Path) -> anyhow::Result<Session> {
    models::onnx::ort_lib(&[path.parent().unwrap_or(path)]).map_err(anyhow::Error::msg)?;
    Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("load {}: {e}", path.display()))
}

/// `f32` PCM in `[-1, 1]` → `i16`, the models' input.
pub fn to_i16(pcm: &[f32]) -> Vec<i16> {
    pcm.iter()
        .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

/// Speech spans `(start, end)` in samples, from the segmentation model over
/// 10 s windows: a frame is speech when its arg-max powerset class is not `0`;
/// a change of class (the window's local speaker, or an overlap class) also
/// closes the span, so a span carries one voice for the embedding.
pub fn speech(
    pcm: &[i16],
    model: &Path,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<Vec<(usize, usize)>> {
    let mut sess = session(model)?;
    let windows = pcm.len().div_ceil(WINDOW).max(1);
    let mut spans = Vec::new();
    let mut offset = FRAME_START;
    let mut open: Option<(usize, usize)> = None;
    for w in 0..windows {
        let start = w * WINDOW;
        let mut window: Vec<f32> = pcm[start..(start + WINDOW).min(pcm.len())]
            .iter()
            .map(|&x| f32::from(x))
            .collect();
        window.resize(WINDOW, 0.0);
        let input = Tensor::from_array((vec![1i64, 1, WINDOW as i64], window))?;
        let outputs = sess.run(ort::inputs![input])?;
        let out = outputs
            .iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("segmentation: no output"))?
            .1;
        let (shape, data) = out.try_extract_tensor::<f32>()?;
        let dims = shape.as_ref();
        let classes = *dims.last().unwrap_or(&1) as usize;
        for frame in data.chunks(classes.max(1)) {
            let class = frame
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |(i, _)| i);
            if let Some((s, c)) = open
                && c != class
            {
                spans.push((s.min(pcm.len()), offset.min(pcm.len())));
                open = None;
            }
            if class != 0 && open.is_none() {
                open = Some((offset, class));
            }
            offset += FRAME;
        }
        progress(w + 1, windows);
    }
    if let Some((s, _)) = open {
        spans.push((s.min(pcm.len()), pcm.len()));
    }
    spans.retain(|(a, b)| b > a);
    Ok(spans)
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
    pub fn open(model: &Path) -> anyhow::Result<Self> {
        Ok(Self(session(model)?))
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
    pub fn embed_frames(&mut self, t: usize, bins: usize, mut data: Vec<f32>) -> anyhow::Result<Vec<f32>> {
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

/// Agglomerative clustering on centroid cosine: start with one cluster per
/// embedding, merge the closest pair while its cosine is ≥ [`CLUSTER`].
/// Returns each embedding's cluster index (by first appearance) and the means.
pub fn cluster(embs: &[Vec<f32>]) -> (Vec<usize>, Vec<Vec<f32>>) {
    // (sum, members)
    let mut cl: Vec<(Vec<f32>, Vec<usize>)> =
        embs.iter().enumerate().map(|(i, e)| (e.clone(), vec![i])).collect();
    let mean = |c: &(Vec<f32>, Vec<usize>)| -> Vec<f32> {
        let n = c.1.len() as f32;
        c.0.iter().map(|x| x / n).collect()
    };
    loop {
        let means: Vec<Vec<f32>> = cl.iter().map(mean).collect();
        let mut best = (CLUSTER, None);
        for i in 0..cl.len() {
            for j in i + 1..cl.len() {
                let c = cosine(&means[i], &means[j]);
                if c >= best.0 {
                    best = (c, Some((i, j)));
                }
            }
        }
        let Some((i, j)) = best.1 else { break };
        let (sum, members) = cl.remove(j);
        for (x, y) in cl[i].0.iter_mut().zip(&sum) {
            *x += y;
        }
        cl[i].1.extend(members);
    }
    // Number clusters by their earliest member, so `s1` is whoever spoke first.
    cl.sort_by_key(|c| c.1.iter().copied().min().unwrap_or(usize::MAX));
    let mut labels = vec![0; embs.len()];
    for (k, c) in cl.iter().enumerate() {
        for &m in &c.1 {
            labels[m] = k;
        }
    }
    (labels, cl.iter().map(mean).collect())
}

/// The whole stage. `progress(pct)` 0–100 within the stage.
pub fn run(pcm: &[i16], models: &Models, mut progress: impl FnMut(u8)) -> anyhow::Result<Diarized> {
    let spans = speech(pcm, &models.segmentation, |done, total| {
        progress((done * 50 / total.max(1)) as u8);
    })?;
    if spans.is_empty() {
        return Ok(Diarized::default());
    }
    let ms = |s: usize| s as u64 * 1000 / u64::from(RATE);
    let mut embedder = Embedder::open(&models.embedding)?;
    // Embeddings for the spans long enough to carry one.
    let long: Vec<usize> = spans
        .iter()
        .enumerate()
        .filter(|(_, (a, b))| ms(b - a) >= MIN_EMBED_MS)
        .map(|(i, _)| i)
        .collect();
    let mut embs = Vec::with_capacity(long.len());
    for (k, &i) in long.iter().enumerate() {
        let (a, b) = spans[i];
        embs.push(embedder.compute(&pcm[a..b])?);
        progress((50 + (k + 1) * 50 / long.len().max(1)) as u8);
    }
    let (clusters, means) = cluster(&embs);
    let label_of = |c: usize| format!("s{}", c + 1);
    let mut labels: Vec<Option<String>> = vec![None; spans.len()];
    for (k, &i) in long.iter().enumerate() {
        labels[i] = Some(label_of(clusters[k]));
    }
    // Short spans: the nearest long span's label; no long span at all: one speaker.
    let out = spans
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            let speaker = labels[i].clone().unwrap_or_else(|| {
                let mid = a.midpoint(b);
                long.iter()
                    .min_by_key(|&&j| spans[j].0.midpoint(spans[j].1).abs_diff(mid))
                    .and_then(|&j| labels[j].clone())
                    .unwrap_or_else(|| "s1".to_owned())
            });
            Span {
                t0_ms: ms(a),
                t1_ms: ms(b),
                speaker,
            }
        })
        .collect();
    let embeddings = means
        .into_iter()
        .enumerate()
        .map(|(c, m)| (label_of(c), m))
        .collect();
    Ok(Diarized {
        spans: out,
        embeddings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agglomerative_merges_closest_pairs_first_and_numbers_by_first_appearance() {
        // Two voices at 90°, a third one 60° from the first. Greedy-by-order would
        // have glued index 3 to whichever came first; the closest pair (0, 4) merges
        // first, then (1, 2), and 3 (cos 0.5 to voice A, 0.87 to voice B) joins B.
        let embs = vec![
            vec![1.0, 0.0],   // A
            vec![0.0, 1.0],   // B
            vec![0.1, 0.99],  // B
            vec![0.5, 0.866], // 60° → B (0.87) over A (0.5)
            vec![0.99, 0.1],  // A
            vec![-1.0, 0.0],  // nobody: cos ≤ 0 to everyone → own cluster
        ];
        let (labels, means) = cluster(&embs);
        assert_eq!(labels, [0, 1, 1, 1, 0, 2]);
        assert_eq!(means.len(), 3);
        assert!((means[0][0] - 0.995).abs() < 1e-6, "{means:?}");
    }
}
