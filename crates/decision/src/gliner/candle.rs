//! GLiNER2.5-Decide natively in candle: candle-transformers' `debertav2::DebertaV2Model` over
//! the `encoder.*` tensors of `model.safetensors` plus the `classifier.{0,2}` head
//! (Linear 1024→2048, ReLU, Linear 2048→1) at every `[L]` position. The checkpoint's other
//! heads (`span_rep.*`, `count_*.*`: extraction/counting) are never read.
//!
//! Dtype: **f32 everywhere, Metal included.** candle-transformers 0.11's DeBERTa is written for
//! f32 only: `DebertaV2DisentangledSelfAttention::disentangled_attention_bias` starts its
//! relative-position `score` as an f32 scalar (`Tensor::new(&[0f32])`) and `broadcast_add`s the
//! c2p term onto it, so an f16 model fails in the first layer with `dtype mismatch in add, lhs:
//! F32, rhs: F16` (measured on this Mac, `tests::metal_f16_is_documented_unsupported`);
//! `XSoftmax::apply`'s `f32::MIN` mask would be next. Metal f32 is still the fast path; CPU f32
//! (Accelerate) the fallback.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::debertav2::{Config, DebertaV2Model};
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use super::prompt::{GlinerConfig, GlinerRow, GlinerSpecial, build_row};
use super::{collate, downcast, scores};
use crate::candle::{Gpu, HeaderEntry};
use crate::engine::{ModelSha, Oracle, Row, Scores};
use crate::question::Question;

/// Head width (`create_mlp(hidden, [2·hidden], 1)`).
const HEAD_HIDDEN: usize = 2048;

/// The checkpoint on one device. Owns its `Device` — a fresh one per load, gone with the session.
pub struct Loaded {
    encoder: DebertaV2Model,
    head_in: candle_nn::Linear,
    head_out: candle_nn::Linear,
    /// Where the weights live.
    pub device: Device,
    /// Always f32 (module docs).
    pub dtype: DType,
    /// DeBERTa-v3's Unigram `tokenizer.json` with the ten gliner2 markers.
    pub tokenizer: Tokenizer,
    /// Row budget (512, state cut allowed; the family gate decides what a cut means).
    pub cfg: GlinerConfig,
    /// `size:<bytes>` of `model.safetensors` until the background sha256 lands, then its hex.
    pub model_sha: ModelSha,
    /// Wall-clock of `open`: header check + mmap + upload, no hashing.
    pub load_time: Duration,
}

/// What `model.safetensors` must contain for `open` to proceed: the encoder's embedding table at
/// `vocab × hidden`, its last layer, the relative-position table at `2·buckets × hidden`, and
/// the head's two layers at the sizes gliner2 builds; every tensor a float dtype. Named and
/// refused otherwise, before any weight is read.
pub fn verify_header(header: &[HeaderEntry], cfg: &Config) -> Result<(), String> {
    let find = |name: &str| {
        header
            .iter()
            .find(|(n, _, _)| n == name)
            .ok_or_else(|| format!("model.safetensors lacks `{name}`"))
    };
    let shaped = |name: &str, want: &[usize]| -> Result<(), String> {
        let (_, _, shape) = find(name)?;
        if shape != want {
            return Err(format!(
                "model.safetensors: `{name}` is {shape:?}, expected {want:?}"
            ));
        }
        Ok(())
    };
    let h = cfg.hidden_size;
    shaped(
        "encoder.embeddings.word_embeddings.weight",
        &[cfg.vocab_size, h],
    )?;
    let buckets = cfg
        .position_buckets
        .filter(|&b| b > 0)
        .unwrap_or(cfg.max_position_embeddings as isize) as usize;
    shaped("encoder.encoder.rel_embeddings.weight", &[2 * buckets, h])?;
    find(&format!(
        "encoder.encoder.layer.{}.output.dense.weight",
        cfg.num_hidden_layers - 1
    ))?;
    shaped("classifier.0.weight", &[HEAD_HIDDEN, h])?;
    shaped("classifier.2.weight", &[1, HEAD_HIDDEN])?;
    if let Some((name, dtype, _)) = header.iter().find(|(_, d, _)| !d.is_float()) {
        return Err(format!(
            "model.safetensors: `{name}` is {dtype:?}, not a float"
        ));
    }
    Ok(())
}

impl Loaded {
    /// Open a GLiNER2.5-Decide checkpoint dir (`crate::engine::model_files(Kind::Gliner)`) on
    /// `gpu` or the CPU. The weights are read in f32 whatever `gpu` would pick for laya (module
    /// docs); a GPU that cannot be created falls back to the CPU with a warning.
    pub fn open(dir: &Path, gpu: Gpu) -> anyhow::Result<Loaded> {
        Self::open_as(dir, gpu, DType::F32)
    }

    /// [`Self::open`] at an explicit `dtype` — f16 is only reachable from the parity test that
    /// documents why it does not run.
    pub fn open_as(dir: &Path, gpu: Gpu, dtype: DType) -> anyhow::Result<Loaded> {
        let t = Instant::now();
        let picked = match gpu {
            Gpu::None => Ok(Device::Cpu),
            Gpu::Metal => Device::new_metal(0).map_err(|e| format!("Metal unavailable ({e})")),
            #[cfg(feature = "cuda")]
            Gpu::Cuda => Device::new_cuda(0).map_err(|e| format!("CUDA unavailable ({e})")),
            #[cfg(not(feature = "cuda"))]
            Gpu::Cuda => {
                Err("this build has no CUDA (needs `--features decision/cuda` and nvcc)".into())
            }
        };
        let (device, dtype) = match picked {
            Ok(d) => (d, dtype),
            Err(e) => {
                tracing::warn!("decision: {e}; gliner runs on the CPU in f32");
                (Device::Cpu, DType::F32)
            }
        };
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))?;
        GlinerSpecial::from_tokenizer(&tokenizer)
            .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))?;
        let config: Config =
            serde_json::from_slice(&std::fs::read(dir.join("encoder_config/config.json"))?)
                .map_err(|e| anyhow::anyhow!("encoder_config/config.json: {e}"))?;
        if !config.relative_attention || config.type_vocab_size != 0 {
            anyhow::bail!(
                "encoder_config/config.json: not a GLiNER2.5-Decide encoder (relative_attention {}, type_vocab_size {})",
                config.relative_attention,
                config.type_vocab_size
            );
        }

        let weights = dir.join("model.safetensors");
        let model_sha: ModelSha = Arc::new(Mutex::new(format!(
            "size:{}",
            std::fs::metadata(&weights)?.len()
        )));
        // SAFETY: the file is read-only for the process lifetime (the laya loader's contract).
        let mmaped = unsafe { candle_core::safetensors::MmapedSafetensors::new(&weights)? };
        let header: Vec<HeaderEntry> = mmaped
            .tensors()
            .into_iter()
            .map(|(name, view)| {
                let dtype = DType::try_from(view.dtype())
                    .map_err(|e| anyhow::anyhow!("model.safetensors: `{name}`: {e}"))?;
                Ok((name, dtype, view.shape().to_vec()))
            })
            .collect::<anyhow::Result<_>>()?;
        verify_header(&header, &config).map_err(anyhow::Error::msg)?;
        drop(mmaped);
        // SAFETY: as above; candle re-mmaps and casts each tensor to `dtype` on `device`.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[&weights], dtype, &device)? };
        let encoder = DebertaV2Model::load(vb.pp("encoder"), &config)
            .map_err(|e| anyhow::anyhow!("load {}: {e}", weights.display()))?;
        let head_in = candle_nn::linear(config.hidden_size, HEAD_HIDDEN, vb.pp("classifier.0"))?;
        let head_out = candle_nn::linear(HEAD_HIDDEN, 1, vb.pp("classifier.2"))?;
        std::thread::spawn({
            let (weights, model_sha) = (weights, model_sha.clone());
            move || match platform::sha256::cached(&weights) {
                Ok(hex) => *model_sha.lock() = hex,
                Err(e) => tracing::warn!(
                    "decision: sha256 of {} failed ({e}); log rows keep the size tag",
                    weights.display()
                ),
            }
        });
        Ok(Loaded {
            encoder,
            head_in,
            head_out,
            device,
            dtype,
            tokenizer,
            cfg: GlinerConfig::default(),
            model_sha,
            load_time: t.elapsed(),
        })
    }

    /// [`build_row`] with this checkpoint's tokenizer and budget.
    pub fn build(&self, q: &Question) -> Result<GlinerRow, String> {
        build_row(&self.tokenizer, q, &self.cfg)
    }

    /// One forward over `rows` (padded to the longest, labels to the widest): encoder → hidden
    /// states gathered at each `[L]` → head → softmax per row.
    pub fn run(&self, rows: &[&GlinerRow]) -> anyhow::Result<Vec<Scores>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let batch = collate(rows);
        let (b, s, n) = (batch.b, batch.s, batch.n);
        let ids = Tensor::from_vec(batch.input_ids, (b, s), &self.device)?;
        let mask = Tensor::from_vec(batch.attention_mask, (b, s), &self.device)?;
        let pos = Tensor::from_vec(batch.label_pos, (b, n), &self.device)?;
        let hidden = self.encoder.forward(&ids, None, Some(mask))?;
        let h = hidden.dim(2)?;
        let gathered = hidden.gather(&pos.unsqueeze(2)?.expand((b, n, h))?.contiguous()?, 1)?;
        let logits = self
            .head_out
            .forward(&self.head_in.forward(&gathered)?.relu()?)?
            .squeeze(2)?
            .to_dtype(DType::F32)?
            .flatten_all()?
            .to_vec1::<f32>()?;
        scores(rows, &logits, n).map_err(|e| {
            anyhow::anyhow!(
                "{e} on {:?} {:?} (S {s} N {n})",
                self.device.location(),
                self.dtype
            )
        })
    }
}

impl Oracle for Loaded {
    fn model_sha(&self) -> ModelSha {
        self.model_sha.clone()
    }
    fn build(&self, q: &Question) -> Result<Box<dyn Row>, String> {
        Loaded::build(self, q).map(|r| Box::new(r) as Box<dyn Row>)
    }
    fn run(&mut self, rows: &[Box<dyn Row>]) -> anyhow::Result<Vec<Scores>> {
        Loaded::run(self, &downcast(rows)?)
    }
    fn load_time(&self) -> Duration {
        self.load_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gliner::reference::{self, compare};

    fn parity(gpu: Gpu, dtype: DType, tol: f32) -> Option<(f32, usize)> {
        let dir = reference::checkpoint()?;
        let t = Instant::now();
        let m = match Loaded::open_as(&dir, gpu, dtype) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{gpu:?} {dtype:?}: open failed: {e:#}");
                return None;
            }
        };
        eprintln!("{gpu:?} {dtype:?}: loaded in {:?}", t.elapsed());
        if m.device.is_cpu() != (gpu == Gpu::None) {
            eprintln!("SKIP: {gpu:?} unavailable, fell back to CPU");
            return None;
        }
        let r = reference::load();
        let mut got = Vec::new();
        for c in &r.cases {
            let row = m.build(&c.question).unwrap();
            assert_eq!(row.input_ids, c.input_ids, "{}: tokens", c.name);
            assert_eq!(row.label_pos, c.label_pos, "{}: label_pos", c.name);
            assert_eq!(row.state_cut, c.state_cut, "{}: state_cut", c.name);
            match m.run(&[&row]) {
                Ok(s) => got.push(s.into_iter().next().unwrap().probs),
                Err(e) => {
                    eprintln!("{gpu:?} {dtype:?}: run failed on {}: {e:#}", c.name);
                    return None;
                }
            }
        }
        let (worst, flips, lines) = compare(r.cases.iter().zip(got.iter().map(Vec::as_slice)));
        for l in &lines {
            eprintln!("{l}");
        }
        eprintln!(
            "{gpu:?} {dtype:?}: {} cases, max |Δp| {worst:.2e}, {flips} argmax flips",
            r.cases.len()
        );
        assert!(worst <= tol, "max |Δp| {worst} > {tol}");
        assert_eq!(flips, 0);
        Some((worst, flips))
    }

    /// The 51 reference questions on the CPU in f32: |Δp| ≤ 1e-4, same argmax, batched too.
    #[test]
    fn parity_cpu_f32() {
        let Some(_) = parity(Gpu::None, DType::F32, 1e-4) else {
            return;
        };
        // A padded batch of unequal rows scores each row as alone.
        let dir = reference::checkpoint().unwrap();
        let m = Loaded::open(&dir, Gpu::None).unwrap();
        let r = reference::load();
        let rows: Vec<GlinerRow> = r.cases[..4]
            .iter()
            .map(|c| m.build(&c.question).unwrap())
            .collect();
        let refs: Vec<&GlinerRow> = rows.iter().collect();
        let batched: Vec<Vec<f32>> = m.run(&refs).unwrap().into_iter().map(|s| s.probs).collect();
        let (worst, flips, _) = compare(r.cases[..4].iter().zip(batched.iter().map(Vec::as_slice)));
        assert!(
            worst <= 1e-4 && flips == 0,
            "batched: max |Δp| {worst}, {flips} flips"
        );
    }

    /// Metal f32: the production device on a Mac.
    #[test]
    fn parity_metal_f32() {
        parity(Gpu::Metal, DType::F32, 1e-3);
    }

    /// Metal f16 does not run (module docs): the failure is printed with the op name; a pass
    /// here would mean candle-transformers grew f16 support and `open` should pick it up.
    #[test]
    fn metal_f16_is_documented_unsupported() {
        let Some(dir) = reference::checkpoint() else {
            return;
        };
        let m = match Loaded::open_as(&dir, Gpu::Metal, DType::F16) {
            Ok(m) if !m.device.is_cpu() => m,
            _ => return,
        };
        let r = reference::load();
        let row = m.build(&r.cases[0].question).unwrap();
        match m.run(&[&row]) {
            Err(e) => eprintln!("Metal f16: {e:#}"),
            Ok(s) => panic!("Metal f16 now runs ({:?}); make `open` use it", s[0].probs),
        }
    }
}
