//! Tier B: the LoRA adapter trainer (126 §7.3 as revised by Revision 2, W3).
//!
//! Given a training [`Bundle`] (W2's shape: scrubbed rows + the base checkpoint hash) and the
//! frozen [`Base`], train a rank-r adapter over the encoder with micro-batch 2 × 256 and gradient
//! accumulation to an effective batch (default 16), AdamW at lr 1e-4 (the measured stable lr),
//! the corpus capped by the rehearsal sampler (default 2,000 rows), for `epochs` or the run cap,
//! whichever first. The adapter and the optimiser state checkpoint every N steps and at any
//! pause; a run resumes from its checkpoint with the step count and the moments intact. The
//! result is a candidate version directory — `adapter.safetensors` + `manifest.json` with the
//! metrics W2's gate reads — that W2's promotion gate re-scores and activates.
//!
//! Venue-agnostic: the same function runs on Metal, CPU or CUDA — the `Base` carries the
//! `Device`. On a node [`Never`] is the pause signal; locally (`double.training.tiers.b.local`,
//! default off) [`OwnerActivity`] aborts within one micro-batch of keyboard / mouse activity.
//!
//! The trainer refuses to run in a binary without `decision/train`
//! ([`decision::candle::lora::TRAIN_BUILD`]): there the forward's fused kernels cut the autograd
//! graph silently and a run would "succeed" having trained nothing.

use decision::candle::lora::Rng;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use candle_core::Tensor;
use candle_core::backprop::GradStore;
use chrono::{DateTime, Utc};
use decision::candle::inputs;
use decision::candle::lora::{AdamW, Base, Lora, TRAIN_BUILD, weighted_nll_sum};
use decision::laya::{Sequence, collate, labels as option_labels};
use decision::question::Question;
use serde::{Deserialize, Serialize};

use crate::config::DoubleConfig;
use crate::types::LabelSource;

// ------------------------------------------------------------------ bundle

/// `rows.jsonl` in a bundle dir.
pub const ROWS_FILE: &str = "rows.jsonl";
/// `manifest.json` in a bundle dir.
pub const BUNDLE_MANIFEST: &str = "manifest.json";

/// One labelled example as the bundle carries it: an `examples` row joined to its label,
/// already scrubbed by the store. `golden` rows are never trained on (§7.4).
// 126 I: W2's bundle writer (`mesh::train_on` / the local scheduler) emits this shape; the
// embedding is not needed by Tier B (it trains the encoder) and is not carried.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleRow {
    /// The card id as text (`examples.question_id`).
    pub question_id: String,
    /// The §3 class key.
    pub class: String,
    /// Scrubbed card text (`examples.card_text`).
    pub card_text: String,
    /// Option labels in card order (without `none`; the trainer appends it).
    pub options: Vec<String>,
    /// The label: an option label or `none`.
    pub choice: String,
    /// Where the label came from (`labels.source`).
    pub source: LabelSource,
    /// §6 weight.
    pub weight: f32,
    /// When the card was raised.
    pub asked_at: DateTime<Utc>,
    /// In the `golden` table: never trained on, re-scored by every candidate (§7.4).
    #[serde(default)]
    pub golden: bool,
}

/// `manifest.json` of a bundle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// sha256 hex of the `model.safetensors` the adapter is trained against; the node fetches
    /// that checkpoint itself (§8.4).
    pub base_hash: String,
    /// The run this bundle belongs to.
    #[serde(default)]
    pub run_id: String,
}

/// A training bundle on disk: `manifest.json` + `rows.jsonl`.
#[derive(Clone, Debug)]
pub struct Bundle {
    /// `manifest.json`.
    pub manifest: BundleManifest,
    /// `rows.jsonl`, in file order.
    pub rows: Vec<BundleRow>,
}

impl Bundle {
    /// Read `dir/manifest.json` + `dir/rows.jsonl`; a bad line is named by number.
    pub fn read(dir: &Path) -> anyhow::Result<Bundle> {
        let manifest = serde_json::from_slice(&std::fs::read(dir.join(BUNDLE_MANIFEST))?)?;
        let text = std::fs::read_to_string(dir.join(ROWS_FILE))?;
        let rows = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, l)| {
                serde_json::from_str(l).map_err(|e| anyhow::anyhow!("{ROWS_FILE}:{}: {e}", i + 1))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Bundle { manifest, rows })
    }

    /// Write both files (W2's bundle writer and the tests).
    pub fn write(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join(BUNDLE_MANIFEST),
            serde_json::to_vec_pretty(&self.manifest)?,
        )?;
        let mut out = String::new();
        for r in &self.rows {
            out.push_str(&serde_json::to_string(r)?);
            out.push('\n');
        }
        std::fs::write(dir.join(ROWS_FILE), out)?;
        Ok(())
    }
}

// ------------------------------------------------------------------ config

/// The knobs, with Revision 2's defaults (the measured 2 × 256 step, effective 16, lr 1e-4,
/// 2,000-row corpus, 90-minute cap).
#[derive(Clone, Debug, PartialEq)]
pub struct TrainConfig {
    /// LoRA rank.
    pub rank: usize,
    /// Rows per forward/backward.
    pub micro_batch: usize,
    /// Token cap per row (the state is right-truncated to it).
    pub seq: usize,
    /// Rows per optimiser step, accumulated over `micro_batch`-row pieces.
    pub effective_batch: usize,
    /// AdamW learning rate.
    pub lr: f64,
    /// Passes over the (rehearsal-capped) corpus.
    pub epochs: usize,
    /// Rehearsal sampler: the corpus cap after the per-class rule.
    pub max_rows: usize,
    /// Rehearsal sampler: newest rows kept per class, and the fixed replay sample from the rest.
    pub rehearsal_per_class: usize,
    /// `running` time per call to [`TierB::train`] (the scheduler's per-night cap): the call
    /// pauses at it and the next call, resuming from the checkpoint, gets a fresh cap.
    pub run_cap: Duration,
    /// Checkpoint every N optimiser steps (and at any pause).
    pub checkpoint_every: usize,
    /// Rows newer than `newest − holdout_days` are held out (capped at 20 % of the corpus) for
    /// the gate's last-7-days log-loss; 0 = no hold-out.
    pub holdout_days: i64,
    /// Seeds the adapter init, the epoch order and the rehearsal sample; recorded in the manifest.
    pub seed: u64,
    /// `local` | `node:<name>`, for the manifest.
    pub venue: String,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            rank: 8,
            micro_batch: 2,
            seq: 256,
            effective_batch: 16,
            lr: 1e-4,
            epochs: 3,
            max_rows: 2000,
            rehearsal_per_class: 200,
            run_cap: Duration::from_secs(90 * 60),
            checkpoint_every: 25,
            holdout_days: 7,
            seed: 126,
            venue: "local".into(),
        }
    }
}

impl TrainConfig {
    /// The `double:` section's Tier B keys over the defaults (`tiers.b.batch` is the effective
    /// batch; the micro-batch is fixed by the measured memory ceiling).
    pub fn from_double(cfg: &DoubleConfig) -> Self {
        let b = &cfg.training.tiers.b;
        Self {
            rank: b.rank.max(1) as usize,
            effective_batch: b.batch.max(1) as usize,
            lr: f64::from(b.lr),
            epochs: b.epochs as usize,
            run_cap: Duration::from_secs(u64::from(cfg.training.run_cap_min) * 60),
            ..Self::default()
        }
    }
}

// ------------------------------------------------------------------ pause

/// Polled before every micro-batch: `true` = checkpoint at the last completed step and stop.
pub trait PauseSignal {
    /// Stop now?
    fn should_pause(&self) -> bool;
}

/// A node: nobody is at the keyboard.
pub struct Never;

impl PauseSignal for Never {
    fn should_pause(&self) -> bool {
        false
    }
}

/// Seconds since the owner's last keyboard / mouse event.
pub trait IdleSeconds: Send + Sync {
    /// Seconds of idleness so far.
    fn idle_seconds(&self) -> f64;
}

/// The engine's idle clock ([`platform::idle::idle_secs`]); a platform that cannot say
/// reads as 0 s = owner active, so the local run pauses (W2's conservative rule).
pub struct PlatformIdle;

impl IdleSeconds for PlatformIdle {
    fn idle_seconds(&self) -> f64 {
        platform::idle::idle_secs().unwrap_or(0.0)
    }
}

/// The local path's abort-on-activity detector (Revision 2 ruling 1): the owner touched the
/// machine when the idle clock drops below `threshold_secs` (default 2, i.e. within the last
/// micro-batch).
pub struct OwnerActivity {
    /// The idle clock.
    pub idle: Box<dyn IdleSeconds>,
    /// Idle below this = activity.
    pub threshold_secs: f64,
}

impl OwnerActivity {
    /// Threshold 2 s.
    pub fn new(idle: Box<dyn IdleSeconds>) -> Self {
        Self {
            idle,
            threshold_secs: 2.0,
        }
    }
}

impl PauseSignal for OwnerActivity {
    fn should_pause(&self) -> bool {
        self.idle.idle_seconds() < self.threshold_secs
    }
}

// ------------------------------------------------------------------ progress / outputs

/// One optimiser step done.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Optimiser steps done, across resumes.
    pub step: usize,
    /// Steps the run will take (`epochs × ⌈rows / effective_batch⌉`).
    pub total_steps: usize,
    /// 1-based.
    pub epoch: usize,
    /// Epochs the run will take.
    pub epochs: usize,
    /// Weighted mean Choice cross-entropy over the step's effective batch.
    pub loss: f32,
    /// Cumulative running time, across resumes.
    pub elapsed: Duration,
}

/// Per-step hooks.
pub struct Hooks<'a> {
    /// Polled before every micro-batch.
    pub pause: &'a dyn PauseSignal,
    /// Called after every optimiser step.
    pub on_progress: &'a mut dyn FnMut(&Progress),
}

/// What W2's gate reads (§7.3 gate (a) and (c); `accuracy` for the record).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Weighted mean loss over the last epoch's steps.
    pub train_loss: f32,
    /// Mean −log p(label) on the held-out rows at `temperature`.
    pub holdout_log_loss: f32,
    /// Top-1 accuracy on the held-out rows.
    pub accuracy: f32,
    /// Expected calibration error, 10 equal-width bins of p_top, at `temperature`.
    pub ece: f32,
    /// Held-out rows scored.
    pub n_holdout: usize,
    /// The 1-D NLL-minimising temperature fitted on the hold-out.
    pub temperature: f32,
}

/// `manifest.json` of a candidate version (§8.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// `b`.
    pub tier: String,
    /// sha256 of the base checkpoint.
    pub base_hash: String,
    /// `[oldest, newest]` `asked_at` of the rows trained on.
    pub data_range: [DateTime<Utc>; 2],
    /// `local` | `node:<name>`.
    pub venue: String,
    /// `TrainConfig::seed`.
    pub seed: u64,
    /// LoRA rank.
    pub rank: usize,
    /// Learning rate.
    pub lr: f64,
    /// Rows per forward/backward.
    pub micro_batch: usize,
    /// Rows per optimiser step.
    pub effective_batch: usize,
    /// Token cap per row.
    pub seq: usize,
    /// Rows trained on after the rehearsal cap.
    pub rows_train: usize,
    /// Rows held out.
    pub rows_holdout: usize,
    /// Rows the bundle had whose label matched no option (skipped).
    pub rows_unlabelled: usize,
    /// Optimiser steps taken.
    pub steps: usize,
    /// Epochs run.
    pub epochs: usize,
    /// Cumulative running time, across resumes.
    pub duration_s: u64,
    /// When the run finished.
    pub trained_at: DateTime<Utc>,
    /// What the gate reads.
    pub metrics: Metrics,
}

/// A trained, not yet promoted, version: `dir/adapter.safetensors` + `dir/manifest.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateVersion {
    /// `b-<trained_at>`; W2's `versions.rs` may rename on activation.
    pub id: String,
    /// Holds `adapter.safetensors` + `manifest.json`.
    pub dir: PathBuf,
    /// The parsed `manifest.json`.
    pub manifest: Manifest,
}

/// Adapter file name inside a version dir.
pub const ADAPTER_FILE: &str = "adapter.safetensors";
/// Manifest file name inside a version dir.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Checkpoint sub-directory of the output dir while a run is in flight.
pub const CHECKPOINT_DIR: &str = "checkpoint";

/// How a call to [`TierB::train`] ended.
#[derive(Debug)]
pub enum TrainOutcome {
    /// The candidate dir is populated.
    Finished(Box<CandidateVersion>),
    /// The pause signal fired or the run cap was hit: the adapter and optimiser state are in
    /// `checkpoint`; the next call with the same `out` dir resumes at `step`.
    Paused {
        /// Optimiser steps completed.
        step: usize,
        /// `<out>/checkpoint`.
        checkpoint: PathBuf,
    },
}

/// Golden-set re-score (§7.4's forgetting detector).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldenMetrics {
    /// Golden rows scored (rows whose label matched no option are skipped).
    pub n: usize,
    /// Top-1 accuracy.
    pub accuracy: f32,
    /// Mean −log p(label) at the candidate's temperature.
    pub log_loss: f32,
}

// ------------------------------------------------------------------ dataset

/// One rendered example.
struct Example {
    seq: Sequence,
    label: u32,
    weight: f32,
}

/// The card as the model reads it: the options as candidate ids, `none` appended, the card
/// text as state.
// parity: one rendering for training and inference (predict::question)
pub fn question(row: &BundleRow) -> Question {
    crate::predict::question(&row.options, row.card_text.clone())
}

/// Render rows; a row whose label is no option, or whose markers fall past `seq`, is dropped
/// (counted by the caller via the length difference).
fn examples(base: &Base, rows: &[&BundleRow], seq: usize) -> anyhow::Result<Vec<Example>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let q = question(row);
        let Some(label) = option_labels(&q).iter().position(|l| *l == row.choice) else {
            continue;
        };
        let s = base.sequence(&q, Some(seq)).map_err(anyhow::Error::msg)?;
        if s.markers_lost {
            continue;
        }
        out.push(Example {
            seq: s,
            label: label as u32,
            weight: row.weight,
        });
    }
    Ok(out)
}

/// The rehearsal sampler (§7.3 "On what"): per class the newest `per_class` rows plus a fixed
/// (class-key-seeded) sample of `per_class` from the rest; then the whole capped at `max_rows`
/// by a seeded subsample. Output is newest-first within the survivors' original order.
pub fn rehearsal<'a>(
    rows: &[&'a BundleRow],
    per_class: usize,
    max_rows: usize,
    seed: u64,
) -> Vec<&'a BundleRow> {
    let mut by_class: HashMap<&str, Vec<&BundleRow>> = HashMap::new();
    let mut sorted: Vec<&BundleRow> = rows.to_vec();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.asked_at));
    for r in sorted {
        by_class.entry(r.class.as_str()).or_default().push(r);
    }
    let mut classes: Vec<_> = by_class.into_iter().collect();
    classes.sort_by(|a, b| a.0.cmp(b.0));
    let mut kept: Vec<&BundleRow> = Vec::new();
    for (class, rows) in classes {
        let (newest, rest) = rows.split_at(per_class.min(rows.len()));
        kept.extend_from_slice(newest);
        let mut rest = rest.to_vec();
        Rng::new(seed ^ Rng::hash(class)).shuffle(&mut rest);
        rest.truncate(per_class);
        kept.extend(rest);
    }
    if kept.len() > max_rows {
        // ponytail: a uniform seeded subsample over the per-class survivors; proportional
        // per-class quotas if one class ever drowns the rest at this stage.
        Rng::new(seed).shuffle(&mut kept);
        kept.truncate(max_rows);
    }
    kept.sort_by_key(|r| std::cmp::Reverse(r.asked_at));
    kept
}

/// Train / hold-out split: rows newer than `newest − holdout_days`, capped at 20 % of the
/// trainable rows (newest first), are held out.
fn split<'a>(
    rows: &[&'a BundleRow],
    holdout_days: i64,
) -> (Vec<&'a BundleRow>, Vec<&'a BundleRow>) {
    if holdout_days <= 0 || rows.is_empty() {
        return (rows.to_vec(), Vec::new());
    }
    let mut sorted = rows.to_vec();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.asked_at));
    let cutoff = sorted[0].asked_at - chrono::Duration::days(holdout_days);
    let cap = sorted.len() / 5;
    let n = sorted
        .iter()
        .take_while(|r| r.asked_at > cutoff)
        .count()
        .min(cap);
    let holdout = sorted[..n].to_vec();
    let train = sorted[n..].to_vec();
    (train, holdout)
}

// ------------------------------------------------------------------ the step

/// Forward + backward over `rows` in `micro`-row pieces on one merged model, gradients summed
/// into one store; returns the store and the weighted mean loss. Each piece's loss is
/// `Σ w·nll / W_total`, so the sum over pieces equals the one-batch loss and gradient.
fn accumulate(
    base: &Base,
    lora: &Lora,
    rows: &[&Example],
    micro: usize,
    pause: Option<&dyn PauseSignal>,
) -> anyhow::Result<Option<(GradStore, f32)>> {
    let total_w: f32 = rows.iter().map(|e| e.weight).sum();
    anyhow::ensure!(total_w > 0., "a batch with zero total weight");
    let model = base.model(Some(lora), true)?;
    let mut acc = GradStore::default();
    let mut loss_sum = 0f32;
    for piece in rows.chunks(micro.max(1)) {
        if pause.is_some_and(PauseSignal::should_pause) {
            return Ok(None);
        }
        let seqs: Vec<&Sequence> = piece.iter().map(|e| &e.seq).collect();
        let batch = collate(&seqs, Some((0, 2)), base.laya.special.pad);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &base.device)?;
        let labels = Tensor::from_vec(
            piece.iter().map(|e| e.label).collect::<Vec<u32>>(),
            piece.len(),
            &base.device,
        )?;
        let weights = Tensor::from_vec(
            piece
                .iter()
                .map(|e| e.weight / total_w)
                .collect::<Vec<f32>>(),
            piece.len(),
            &base.device,
        )?;
        let out = model.forward(&ids, &attn, &pos, &mask, &qtype)?;
        let loss = weighted_nll_sum(&out.logits, &labels, &weights)?;
        let v = loss.to_scalar::<f32>()?;
        anyhow::ensure!(v.is_finite(), "non-finite loss");
        loss_sum += v;
        acc.extend(loss.backward()?)?;
    }
    Ok(Some((acc, loss_sum)))
}

/// Per-row log-probabilities over the row's own K options (natural log, temperature 1), plus
/// the label — no graph.
fn score(
    base: &Base,
    lora: Option<&Lora>,
    rows: &[Example],
    micro: usize,
) -> anyhow::Result<Vec<(Vec<f32>, u32)>> {
    let model = base.model(lora, false)?;
    let mut out = Vec::with_capacity(rows.len());
    for piece in rows.chunks(micro.max(1)) {
        let seqs: Vec<&Sequence> = piece.iter().map(|e| &e.seq).collect();
        let batch = collate(&seqs, Some((0, 2)), base.laya.special.pad);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &base.device)?;
        let logits: Vec<f32> = model
            .forward(&ids, &attn, &pos, &mask, &qtype)?
            .logits
            .flatten_all()?
            .to_vec1()?;
        for (i, e) in piece.iter().enumerate() {
            let k = e.seq.marker_pos.len();
            out.push((logits[i * batch.k..i * batch.k + k].to_vec(), e.label));
        }
    }
    Ok(out)
}

fn log_softmax(logits: &[f32], t: f32) -> Vec<f32> {
    let scaled: Vec<f32> = logits.iter().map(|l| l / t).collect();
    let m = scaled.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let lse = scaled.iter().map(|l| (l - m).exp()).sum::<f32>().ln() + m;
    scaled.iter().map(|l| l - lse).collect()
}

fn mean_nll(scored: &[(Vec<f32>, u32)], t: f32) -> f32 {
    if scored.is_empty() {
        return f32::NAN;
    }
    scored
        .iter()
        .map(|(l, y)| -log_softmax(l, t)[*y as usize])
        .sum::<f32>()
        / scored.len() as f32
}

/// The hold-out metrics: temperature by a 1-D NLL grid (0.25 … 4, geometric), then log-loss,
/// accuracy and 10-bin ECE at it.
fn holdout_metrics(scored: &[(Vec<f32>, u32)]) -> (f32, f32, f32, f32) {
    if scored.is_empty() {
        return (1., f32::NAN, f32::NAN, f32::NAN);
    }
    let grid = (0..=40).map(|i| 0.25f32 * 16f32.powf(i as f32 / 40.));
    let t = grid
        .map(|t| (t, mean_nll(scored, t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(1., |(t, _)| t);
    let mut correct = 0usize;
    let mut bins = [(0usize, 0f32, 0f32); 10];
    for (l, y) in scored {
        let p: Vec<f32> = log_softmax(l, t).iter().map(|v| v.exp()).collect();
        let (top, p_top) =
            p.iter().enumerate().fold(
                (0, f32::NEG_INFINITY),
                |m, (i, &v)| if v > m.1 { (i, v) } else { m },
            );
        let hit = top == *y as usize;
        correct += usize::from(hit);
        let b = ((p_top * 10.) as usize).min(9);
        bins[b].0 += 1;
        bins[b].1 += p_top;
        bins[b].2 += f32::from(u8::from(hit));
    }
    let n = scored.len() as f32;
    let ece: f32 = bins
        .iter()
        .filter(|b| b.0 > 0)
        .map(|b| (b.0 as f32 / n) * (b.1 / b.0 as f32 - b.2 / b.0 as f32).abs())
        .sum();
    (t, mean_nll(scored, t), correct as f32 / n, ece)
}

// ------------------------------------------------------------------ checkpoint

#[derive(Serialize, Deserialize)]
struct State {
    step: usize,
    epoch: usize,
    cursor: usize,
    elapsed_secs: f64,
    seed: u64,
    /// Sum and count of the current epoch's step losses, for `train_loss`.
    epoch_loss: (f32, usize),
}

fn checkpoint(dir: &Path, lora: &Lora, opt: &AdamW, state: &State) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    lora.save(&dir.join(ADAPTER_FILE))?;
    opt.save(&dir.join("optim.safetensors"))?;
    std::fs::write(dir.join("state.json"), serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

// ------------------------------------------------------------------ TierB

/// The Tier B trainer (functions only; the state is on disk under `out`).
pub struct TierB;

impl TierB {
    /// Local Tier B is opt-in (`double.training.tiers.b.local`, default `false`): without it
    /// `venue: auto` resolves Tier B to a mesh GPU node or leaves it inactive (Revision 2).
    pub fn local_allowed(cfg: &DoubleConfig) -> bool {
        cfg.training.tiers.b.enabled && cfg.training.tiers.b.local
    }

    /// Train (or resume) the adapter for `bundle` on `base`, writing into `out`
    /// (`out/checkpoint/` while in flight; `out/adapter.safetensors` + `out/manifest.json` when
    /// finished). `hooks.pause` is polled before every micro-batch; `hooks.on_progress` after
    /// every optimiser step.
    pub fn train(
        bundle: &Bundle,
        cfg: &TrainConfig,
        base: &Base,
        out: &Path,
        hooks: Hooks<'_>,
    ) -> anyhow::Result<TrainOutcome> {
        anyhow::ensure!(
            TRAIN_BUILD,
            "this binary was built without `decision/train`: the encoder's fused kernels cut the \
             gradient silently; rebuild with `--features decision/train`"
        );
        anyhow::ensure!(
            bundle.manifest.base_hash == base.sha,
            "bundle base_hash {} ≠ checkpoint sha {}",
            bundle.manifest.base_hash,
            base.sha
        );
        let started = Instant::now();
        let trainable: Vec<&BundleRow> = bundle
            .rows
            .iter()
            .filter(|r| !r.golden && r.weight > 0. && r.source != LabelSource::Excluded)
            .collect();
        let (train_rows, holdout_rows) = split(&trainable, cfg.holdout_days);
        let train_rows = rehearsal(&train_rows, cfg.rehearsal_per_class, cfg.max_rows, cfg.seed);
        let train = examples(base, &train_rows, cfg.seq)?;
        let holdout = examples(base, &holdout_rows, cfg.seq)?;
        let unlabelled = train_rows.len() - train.len() + holdout_rows.len() - holdout.len();
        anyhow::ensure!(!train.is_empty(), "no trainable rows in the bundle");
        let data_range = [
            train_rows.iter().map(|r| r.asked_at).min().unwrap(),
            train_rows.iter().map(|r| r.asked_at).max().unwrap(),
        ];

        let steps_per_epoch = train.len().div_ceil(cfg.effective_batch.max(1));
        let total_steps = steps_per_epoch * cfg.epochs;
        let ckpt = out.join(CHECKPOINT_DIR);
        let lora;
        let mut opt;
        let mut state;
        if ckpt.join("state.json").is_file() {
            state = serde_json::from_slice::<State>(&std::fs::read(ckpt.join("state.json"))?)?;
            lora = Lora::load(base, &ckpt.join(ADAPTER_FILE))?;
            opt = AdamW::new(lora.varmap(), cfg.lr)?;
            opt.load(&ckpt.join("optim.safetensors"))?;
            anyhow::ensure!(opt.steps() == state.step, "checkpoint step mismatch");
        } else {
            state = State {
                step: 0,
                epoch: 0,
                cursor: 0,
                elapsed_secs: 0.,
                seed: cfg.seed,
                epoch_loss: (0., 0),
            };
            lora = Lora::new(base, cfg.rank, cfg.seed)?;
            opt = AdamW::new(lora.varmap(), cfg.lr)?;
        }
        let elapsed =
            |state: &State| Duration::from_secs_f64(state.elapsed_secs) + started.elapsed();
        let pause_now =
            |state: &mut State, lora: &Lora, opt: &AdamW| -> anyhow::Result<TrainOutcome> {
                state.elapsed_secs = elapsed(state).as_secs_f64();
                checkpoint(&ckpt, lora, opt, state)?;
                Ok(TrainOutcome::Paused {
                    step: state.step,
                    checkpoint: ckpt.clone(),
                })
            };

        let mut last_epoch_loss = state.epoch_loss;
        while state.epoch < cfg.epochs {
            let mut order: Vec<usize> = (0..train.len()).collect();
            Rng::new(state.seed ^ (state.epoch as u64 + 1)).shuffle(&mut order);
            while state.cursor < train.len() {
                if started.elapsed() >= cfg.run_cap || hooks.pause.should_pause() {
                    return pause_now(&mut state, &lora, &opt);
                }
                let end = (state.cursor + cfg.effective_batch).min(train.len());
                let rows: Vec<&Example> = order[state.cursor..end]
                    .iter()
                    .map(|&i| &train[i])
                    .collect();
                let Some((grads, loss)) =
                    accumulate(base, &lora, &rows, cfg.micro_batch, Some(hooks.pause))?
                else {
                    return pause_now(&mut state, &lora, &opt);
                };
                opt.step(&grads)?;
                state.step += 1;
                state.cursor = end;
                state.epoch_loss.0 += loss;
                state.epoch_loss.1 += 1;
                (hooks.on_progress)(&Progress {
                    step: state.step,
                    total_steps,
                    epoch: state.epoch + 1,
                    epochs: cfg.epochs,
                    loss,
                    elapsed: elapsed(&state),
                });
                if cfg.checkpoint_every > 0 && state.step % cfg.checkpoint_every == 0 {
                    state.elapsed_secs = elapsed(&state).as_secs_f64();
                    checkpoint(&ckpt, &lora, &opt, &state)?;
                }
            }
            last_epoch_loss = state.epoch_loss;
            state.epoch += 1;
            state.cursor = 0;
            state.epoch_loss = (0., 0);
        }

        // ---- finish: metrics, files, manifest
        let scored = score(base, Some(&lora), &holdout, cfg.micro_batch * 4)?;
        let (temperature, holdout_log_loss, accuracy, ece) = holdout_metrics(&scored);
        let trained_at = Utc::now();
        let manifest = Manifest {
            tier: "b".into(),
            base_hash: base.sha.clone(),
            data_range,
            venue: cfg.venue.clone(),
            seed: cfg.seed,
            rank: lora.rank,
            lr: cfg.lr,
            micro_batch: cfg.micro_batch,
            effective_batch: cfg.effective_batch,
            seq: cfg.seq,
            rows_train: train.len(),
            rows_holdout: holdout.len(),
            rows_unlabelled: unlabelled,
            steps: state.step,
            epochs: cfg.epochs,
            duration_s: elapsed(&state).as_secs(),
            trained_at,
            metrics: Metrics {
                train_loss: last_epoch_loss.0 / last_epoch_loss.1.max(1) as f32,
                holdout_log_loss,
                accuracy,
                ece,
                n_holdout: holdout.len(),
                temperature,
            },
        };
        std::fs::create_dir_all(out)?;
        lora.save(&out.join(ADAPTER_FILE))?;
        std::fs::write(
            out.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        if ckpt.is_dir() {
            std::fs::remove_dir_all(&ckpt)?;
        }
        Ok(TrainOutcome::Finished(Box::new(CandidateVersion {
            id: format!("b-{}", trained_at.format("%Y%m%dT%H%M%SZ")),
            dir: out.to_path_buf(),
            manifest,
        })))
    }
}

/// Re-score a candidate on the golden rows (inference only, at the manifest's temperature).
// 126 I: W2's gate consumes this — §7.3 gate (b), "no more than 1 point of accuracy lost".
pub fn rescore_golden(
    base: &Base,
    candidate: &CandidateVersion,
    golden: &[BundleRow],
) -> anyhow::Result<GoldenMetrics> {
    let lora = Lora::load(base, &candidate.dir.join(ADAPTER_FILE))?;
    let rows: Vec<&BundleRow> = golden.iter().collect();
    let ex = examples(base, &rows, candidate.manifest.seq)?;
    let scored = score(base, Some(&lora), &ex, 8)?;
    let t = candidate.manifest.metrics.temperature;
    let correct = scored
        .iter()
        .filter(|(l, y)| {
            l.iter()
                .enumerate()
                .fold(
                    (0, f32::NEG_INFINITY),
                    |m, (i, &v)| if v > m.1 { (i, v) } else { m },
                )
                .0
                == *y as usize
        })
        .count();
    Ok(GoldenMetrics {
        n: scored.len(),
        accuracy: if scored.is_empty() {
            f32::NAN
        } else {
            correct as f32 / scored.len() as f32
        },
        log_loss: mean_nll(&scored, t),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;
    use decision::candle::lora::SYNTHETIC_WORDS;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `n` rows over two classes whose label follows the class (learnable), card texts from the
    /// synthetic vocabulary, one hour apart ending now; the last `golden` are golden.
    fn rows(n: usize, golden: usize) -> Vec<BundleRow> {
        let words: Vec<&str> = SYNTHETIC_WORDS.split_whitespace().collect();
        let now = Utc::now();
        (0..n)
            .map(|i| {
                let push = i % 2 == 0;
                let text: Vec<&str> = (0..3 + i % 5)
                    .map(|j| words[(i * 7 + j * 3) % words.len()])
                    .collect();
                BundleRow {
                    question_id: format!("q{i}"),
                    class: if push {
                        "approval:shell:mutating:git push:root"
                    } else {
                        "approval:shell:read_only:cargo test:root"
                    }
                    .into(),
                    card_text: format!(
                        "{} {}",
                        if push { "git push" } else { "cargo test" },
                        text.join(" ")
                    ),
                    options: vec!["allow".into(), "deny".into()],
                    choice: if push { "allow" } else { "deny" }.into(),
                    source: LabelSource::Direct,
                    weight: if i % 5 == 0 { 0.5 } else { 1.0 },
                    asked_at: now - chrono::Duration::hours((n - i) as i64),
                    golden: i >= n - golden,
                }
            })
            .collect()
    }

    fn bundle(n: usize, golden: usize) -> Bundle {
        Bundle {
            manifest: BundleManifest {
                base_hash: "synthetic".into(),
                run_id: "test".into(),
            },
            rows: rows(n, golden),
        }
    }

    fn tiny_cfg() -> TrainConfig {
        TrainConfig {
            rank: 4,
            micro_batch: 2,
            seq: 64,
            effective_batch: 4,
            lr: 1e-2,
            epochs: 2,
            holdout_days: 0,
            checkpoint_every: 1000,
            ..TrainConfig::default()
        }
    }

    fn adapter_values(path: &Path) -> Vec<(String, Vec<f32>)> {
        let mut v: Vec<(String, Vec<f32>)> = candle_core::safetensors::load(path, &Device::Cpu)
            .unwrap()
            .into_iter()
            .map(|(k, t)| (k, t.flatten_all().unwrap().to_vec1().unwrap()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Eight micro-batches of 2 give the same gradient as one batch of 16 (to float rounding),
    /// with rows of different lengths so padding differs between the two.
    #[test]
    fn accumulation_matches_one_full_batch() {
        let base = Base::synthetic(&Device::Cpu).unwrap();
        let bundle = bundle(16, 0);
        let refs: Vec<&BundleRow> = bundle.rows.iter().collect();
        let ex = examples(&base, &refs, 64).unwrap();
        assert_eq!(ex.len(), 16);
        let lens: std::collections::HashSet<usize> =
            ex.iter().map(|e| e.seq.input_ids.len()).collect();
        assert!(
            lens.len() > 1,
            "rows must differ in length for the test to mean anything"
        );
        let rows: Vec<&Example> = ex.iter().collect();
        let lora = Lora::new(&base, 4, 1).unwrap();
        // Random non-zero B so every factor gets a real gradient (a constant B makes ∂/∂A of
        // the `Wo` adapters vanish: LayerNorm downstream kills the mean of the residual grad).
        let mut rng = Rng::new(9);
        for v in lora.vars() {
            let n = v.elem_count();
            let noise: Vec<f32> = (0..n).map(|_| 0.05 * rng.normal()).collect();
            let noise = Tensor::from_vec(noise, v.shape(), &Device::Cpu).unwrap();
            v.set(&(v.as_tensor() + noise).unwrap()).unwrap();
        }
        let (micro, l_micro) = accumulate(&base, &lora, &rows, 2, None).unwrap().unwrap();
        let (full, l_full) = accumulate(&base, &lora, &rows, 16, None).unwrap().unwrap();
        assert!(
            (l_micro - l_full).abs() <= 1e-4 * l_full.abs().max(1.),
            "{l_micro} vs {l_full}"
        );
        let mut worst = 0f32;
        for v in lora.vars() {
            let a: Vec<f32> = micro
                .get(v.as_tensor())
                .unwrap()
                .flatten_all()
                .unwrap()
                .to_vec1()
                .unwrap();
            let b: Vec<f32> = full
                .get(v.as_tensor())
                .unwrap()
                .flatten_all()
                .unwrap()
                .to_vec1()
                .unwrap();
            let scale = b.iter().map(|x| x.abs()).fold(0f32, f32::max);
            assert!(
                scale > 1e-4,
                "a vanishing gradient would make the comparison vacuous"
            );
            for (x, y) in a.iter().zip(&b) {
                worst = worst.max((x - y).abs() / scale);
            }
        }
        eprintln!(
            "accumulation: loss {l_micro} vs {l_full}, max relative gradient diff {worst:.3e}"
        );
        assert!(worst < 1e-4, "max relative gradient difference {worst}");
    }

    /// A fake idle clock: idle for the first `active_after` polls, then "the owner touched the
    /// mouse" for ever. Counts the polls after that moment.
    struct FakeIdle {
        polls: AtomicUsize,
        active_after: usize,
    }

    impl IdleSeconds for FakeIdle {
        fn idle_seconds(&self) -> f64 {
            let n = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
            if n > self.active_after { 0.0 } else { 900.0 }
        }
    }

    /// Activity pauses the run within one micro-batch, the checkpoint lands, and a resume
    /// reproduces the uninterrupted run step for step — adapter and optimiser state included.
    #[test]
    fn pauses_within_one_micro_batch_and_resumes_exactly() {
        let base = Base::synthetic(&Device::Cpu).unwrap();
        let bundle = bundle(16, 0);
        let cfg = tiny_cfg();
        let dir = tempfile::tempdir().unwrap();

        // Reference: uninterrupted.
        let mut ref_steps = Vec::new();
        let TrainOutcome::Finished(reference) = TierB::train(
            &bundle,
            &cfg,
            &base,
            &dir.path().join("ref"),
            Hooks {
                pause: &Never,
                on_progress: &mut |p| ref_steps.push(p.step),
            },
        )
        .unwrap() else {
            panic!("reference run paused")
        };
        assert_eq!(ref_steps, (1..=8).collect::<Vec<_>>());
        assert_eq!(reference.manifest.steps, 8);

        // Interrupted: each step polls once before it and once per micro-batch (two), so step k
        // polls 3k−2 … 3k. The owner moves the mouse at poll 6 = step 2's last micro-batch: that
        // step is abandoned and the checkpoint holds step 1.
        let idle = FakeIdle {
            polls: AtomicUsize::new(0),
            active_after: 5,
        };
        let pause = OwnerActivity::new(Box::new(idle));
        let out = dir.path().join("run");
        let mut steps = Vec::new();
        let paused = TierB::train(
            &bundle,
            &cfg,
            &base,
            &out,
            Hooks {
                pause: &pause,
                on_progress: &mut |p| steps.push(p.step),
            },
        )
        .unwrap();
        let TrainOutcome::Paused { step, checkpoint } = paused else {
            panic!("expected a pause")
        };
        assert_eq!(step, 1, "the partial step is abandoned");
        assert_eq!(steps, vec![1]);
        assert!(checkpoint.join(ADAPTER_FILE).is_file());
        assert!(checkpoint.join("optim.safetensors").is_file());
        assert!(checkpoint.join("state.json").is_file());
        // Resume with nobody at the keyboard: continues at step 2, finishes at 8.
        let mut steps = Vec::new();
        let TrainOutcome::Finished(resumed) = TierB::train(
            &bundle,
            &cfg,
            &base,
            &out,
            Hooks {
                pause: &Never,
                on_progress: &mut |p| steps.push(p.step),
            },
        )
        .unwrap() else {
            panic!("resume paused")
        };
        assert_eq!(steps, (2..=8).collect::<Vec<_>>());
        assert_eq!(resumed.manifest.steps, 8);
        assert!(
            !out.join(CHECKPOINT_DIR).exists(),
            "checkpoint removed after finish"
        );
        let a = adapter_values(&reference.dir.join(ADAPTER_FILE));
        let b = adapter_values(&resumed.dir.join(ADAPTER_FILE));
        assert_eq!(a.len(), b.len());
        let mut worst = 0f32;
        for ((ka, va), (kb, vb)) in a.iter().zip(&b) {
            assert_eq!(ka, kb);
            for (x, y) in va.iter().zip(vb) {
                worst = worst.max((x - y).abs());
            }
        }
        eprintln!(
            "resume: paused at step {step}, finished at {}, max |Δ adapter| {worst:.3e}",
            resumed.manifest.steps
        );
        assert!(
            worst < 1e-5,
            "resumed adapter differs from the uninterrupted one by {worst}"
        );
    }

    /// The pause signal fires on the first poll after activity (latency = one micro-batch).
    #[test]
    fn owner_activity_fires_on_the_first_poll_after_activity() {
        let pause = OwnerActivity::new(Box::new(FakeIdle {
            polls: AtomicUsize::new(0),
            active_after: 2,
        }));
        assert!(!pause.should_pause());
        assert!(!pause.should_pause());
        assert!(pause.should_pause());
    }

    /// A full run on the tiny model through the on-disk bundle: the loss falls, the manifest
    /// carries finite metrics over a real hold-out, and the golden re-score runs on the
    /// candidate.
    #[test]
    fn loss_falls_and_manifest_carries_the_gate_metrics() {
        let base = Base::synthetic(&Device::Cpu).unwrap();
        let dir = tempfile::tempdir().unwrap();
        // 40 rows an hour apart: the newest 7 days = all of them, so the 20 % cap (8 rows) is
        // the hold-out; 4 golden rows are excluded from both.
        bundle(40, 4).write(&dir.path().join("bundle")).unwrap();
        let bundle = Bundle::read(&dir.path().join("bundle")).unwrap();
        assert_eq!(bundle.rows.len(), 40);
        let cfg = TrainConfig {
            epochs: 3,
            holdout_days: 7,
            ..tiny_cfg()
        };
        let mut losses = Vec::new();
        let TrainOutcome::Finished(c) = TierB::train(
            &bundle,
            &cfg,
            &base,
            &dir.path().join("out"),
            Hooks {
                pause: &Never,
                on_progress: &mut |p| losses.push(p.loss),
            },
        )
        .unwrap() else {
            panic!("paused")
        };
        let m = &c.manifest;
        assert_eq!(
            (m.rows_train, m.rows_holdout, m.rows_unlabelled),
            (29, 7, 0)
        );
        assert_eq!(m.steps, 3 * 29usize.div_ceil(4));
        assert_eq!(losses.len(), m.steps);
        let first: f32 = losses[..3].iter().sum::<f32>() / 3.;
        let last: f32 = losses[losses.len() - 3..].iter().sum::<f32>() / 3.;
        eprintln!(
            "loss first-3 {first:.4} -> last-3 {last:.4}; metrics {:?}",
            m.metrics
        );
        assert!(last < first, "loss did not fall: {first} -> {last}");
        assert!(m.metrics.holdout_log_loss.is_finite());
        assert!((0. ..=1.).contains(&m.metrics.accuracy));
        assert!((0. ..=1.).contains(&m.metrics.ece));
        assert!(m.metrics.temperature > 0.);
        assert_eq!(m.metrics.n_holdout, 7);
        assert!(c.dir.join(ADAPTER_FILE).is_file() && c.dir.join(MANIFEST_FILE).is_file());
        let back: Manifest =
            serde_json::from_slice(&std::fs::read(c.dir.join(MANIFEST_FILE)).unwrap()).unwrap();
        assert_eq!(&back, m);

        let golden: Vec<BundleRow> = bundle.rows.iter().filter(|r| r.golden).cloned().collect();
        let g = rescore_golden(&base, &c, &golden).unwrap();
        assert_eq!(g.n, 4);
        assert!(g.log_loss.is_finite() && (0. ..=1.).contains(&g.accuracy));
    }

    /// Per class: the newest `per_class` plus a fixed sample of `per_class` from the rest; the
    /// total capped; the same seed gives the same pick.
    #[test]
    fn rehearsal_caps_per_class_and_total() {
        let all = rows(100, 0);
        let refs: Vec<&BundleRow> = all.iter().collect();
        let kept = rehearsal(&refs, 10, 1000, 1);
        assert_eq!(kept.len(), 40);
        for class in [
            "approval:shell:mutating:git push:root",
            "approval:shell:read_only:cargo test:root",
        ] {
            let of_class: Vec<&&BundleRow> = kept.iter().filter(|r| r.class == class).collect();
            assert_eq!(of_class.len(), 20);
            // The ten newest of the class are all there.
            let newest: Vec<&BundleRow> = {
                let mut v: Vec<&BundleRow> = all.iter().filter(|r| r.class == class).collect();
                v.sort_by_key(|r| std::cmp::Reverse(r.asked_at));
                v.truncate(10);
                v
            };
            assert!(
                newest
                    .iter()
                    .all(|n| of_class.iter().any(|k| k.question_id == n.question_id))
            );
        }
        let again = rehearsal(&refs, 10, 1000, 1);
        assert_eq!(kept, again);
        let other = rehearsal(&refs, 10, 1000, 2);
        assert_ne!(kept, other);
        assert_eq!(rehearsal(&refs, 10, 25, 1).len(), 25);
    }

    #[test]
    fn local_tier_b_is_off_by_default() {
        let cfg = DoubleConfig::default();
        assert!(!TierB::local_allowed(&cfg));
        let on: DoubleConfig =
            serde_yaml::from_str("training: { tiers: { b: { local: true } } }").unwrap();
        assert!(TierB::local_allowed(&on));
    }
}
