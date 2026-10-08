//! The decision engine (57 §12.8): laya-rs 0.1.0's `DecisionModel` — ModernBERT + decision head,
//! vendored in [`model`], [`weights`], [`config`] — over the `safetensors` checkpoint, on Metal
//! (f16) or CPU (f32). Only the forward pass is theirs: rendering ([`build_sequence`]), padding
//! ([`collate`]), calibration and confidence ([`postprocess`]), the gate and the log are ours.

pub mod config;
pub mod embed;
pub mod lora;
pub mod model;
pub mod weights;

use std::borrow::Borrow;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use self::config::{AgentConfig, EncoderConfig};
use self::model::DecisionModel;
use self::weights::{SanitizedBackend, sanitize_name};

use crate::engine::{ModelSha, Oracle, Row, Scores};
use crate::laya::{
    Batch, LayaConfig, RowOut, Sequence, Special, build_sequence, collate, postprocess,
};
use crate::question::Question;

/// One entry of the safetensors header: `(name, dtype, shape)`.
pub type HeaderEntry = (String, DType, Vec<usize>);

/// The checkpoint the configs describe, checked against the file's header before any weight is
/// read: the embedding table has the encoder's `vocab_size × hidden_size`, the last encoder
/// layer, the last head layer, the scorer, the act head and the type embedding are present
/// (names as [`sanitize_name`] maps them — both the PyTorch and the MLX layout load), and every
/// tensor is a float dtype candle can cast (f16 / bf16 / f32). Anything else is refused and named.
pub fn verify_header(
    header: &[HeaderEntry],
    enc: &EncoderConfig,
    agent: &AgentConfig,
) -> Result<(), String> {
    let mut missing = Vec::new();
    let find = |want: &str| {
        header
            .iter()
            .find(|(name, _, _)| sanitize_name(name) == want)
    };
    let last_layer = enc
        .num_hidden_layers
        .checked_sub(1)
        .ok_or("encoder has no layers")?;
    let last_head = agent
        .head_layers
        .checked_sub(1)
        .ok_or("head has no layers")?;
    for name in [
        "encoder.embeddings.tok_embeddings.weight".to_owned(),
        "encoder.final_norm.weight".to_owned(),
        format!("encoder.layers.{last_layer}.attn.Wqkv.weight"),
        format!("head.layers.{last_head}.self_attn.in_proj.weight"),
        "scorer.layers.3.weight".to_owned(),
        "act_head.layers.2.weight".to_owned(),
        "type_emb.weight".to_owned(),
    ] {
        if find(&name).is_none() {
            missing.push(name);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "model.safetensors: missing tensors {} ({} in file)",
            missing.join(", "),
            header.len()
        ));
    }
    if let Some((_, _, shape)) = find("encoder.embeddings.tok_embeddings.weight")
        && shape != &[enc.vocab_size, enc.hidden_size]
    {
        return Err(format!(
            "model.safetensors: tok_embeddings is {shape:?}, encoder/config.json says [{}, {}]",
            enc.vocab_size, enc.hidden_size
        ));
    }
    if let Some((name, dtype, _)) = header
        .iter()
        .find(|(_, d, _)| !matches!(d, DType::F16 | DType::BF16 | DType::F32))
    {
        return Err(format!(
            "model.safetensors: `{name}` is {dtype:?}, expected f16, bf16 or f32"
        ));
    }
    Ok(())
}

/// The five model inputs, in `DecisionModel::forward` order and the dtypes laya-rs's own
/// `Agent::collate` feeds it: `input_ids u32[B,S]`, `attention_mask u8[B,S]`,
/// `marker_pos i64[B,K]`, `marker_mask u8[B,K]`, `qtype u32[B]`.
pub fn inputs(batch: &Batch, device: &Device) -> candle_core::Result<[Tensor; 5]> {
    let (b, s, k) = (batch.b, batch.s, batch.k);
    let ids: Vec<u32> = batch.input_ids.iter().map(|&x| x as u32).collect();
    let attn: Vec<u8> = batch.attention_mask.iter().map(|&x| x as u8).collect();
    let marker_mask: Vec<u8> = batch.marker_mask.iter().map(|&x| x as u8).collect();
    let qtype: Vec<u32> = batch.qtype.iter().map(|&x| x as u32).collect();
    Ok([
        Tensor::from_vec(ids, (b, s), device)?,
        Tensor::from_vec(attn, (b, s), device)?,
        Tensor::from_vec(batch.marker_pos.clone(), (b, k), device)?,
        Tensor::from_vec(marker_mask, (b, k), device)?,
        Tensor::from_vec(qtype, b, device)?,
    ])
}

/// The checkpoint on one device: the model, the tokenizer, the temperatures and the weights'
/// identity for the log. Owns its `Device` — a fresh one per load, gone with the session.
pub struct Loaded {
    model: DecisionModel,
    /// Where the weights live.
    pub device: Device,
    /// f16 on Metal, f32 on CPU by default.
    pub dtype: DType,
    /// laya's fast BPE tokenizer.
    pub tokenizer: Tokenizer,
    /// Budgets and temperatures from `rl_agent_config.json`.
    pub laya: LayaConfig,
    /// `size:<bytes>` of `model.safetensors` until the background sha256 lands (~2 s after
    /// `open` on this Mac), then its hex; part of every gate version and log row.
    pub model_sha: ModelSha,
    /// Wall-clock of `open`: header check + mmap + upload, no hashing.
    pub load_time: Duration,
}

// Reclaim after drop (57 §2.1 item 3, `examples/lifecycle`, as-built step 6): Metal buffers go
// back to the OS on their own within a few seconds of the drop (steady state +0..+2 MB over a
// ~47 MB one-off Metal-stack baseline). On the macOS **CPU f32** path the freed weights sit in
// libmalloc's large cache (`MALLOC_LARGE (empty)`, 64 entries, ~790 MB, reused by the next
// load, never growing); `malloc_zone_pressure_relief` returns 0 for it and churning large
// frees does not evict it — only `MallocLargeCache=0` in the process environment (set before
// the allocator initialises) brings that path to +4 MB. Nothing in-process helps, so nothing
// is done here; Metal is the Mac engine and CPU f32 the Linux/Windows path.

/// Which GPU [`Loaded::open`] asks candle for ([`crate::Device::gpu`] resolves the config).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpu {
    /// CPU (Accelerate on a Mac).
    None,
    /// Apple GPU, device 0.
    Metal,
    /// Device 0; only reachable when this crate was built with the `cuda` feature.
    Cuda,
}

impl Gpu {
    /// f16 on Metal, f32 on CPU and CUDA (the CUDA target tonight is a Maxwell card, where
    /// f16 arithmetic is neither native nor faster).
    pub fn dtype(self) -> DType {
        match self {
            Gpu::Metal => DType::F16,
            Gpu::None | Gpu::Cuda => DType::F32,
        }
    }

    /// Create the candle device, or say why it cannot be (no GPU; CUDA in a binary built
    /// without `decision/cuda`).
    pub fn open(self) -> std::result::Result<Device, String> {
        match self {
            Gpu::None => Ok(Device::Cpu),
            Gpu::Metal => Device::new_metal(0).map_err(|e| format!("Metal unavailable ({e})")),
            #[cfg(feature = "cuda")]
            Gpu::Cuda => Device::new_cuda(0).map_err(|e| format!("CUDA unavailable ({e})")),
            #[cfg(not(feature = "cuda"))]
            Gpu::Cuda => {
                Err("this build has no CUDA (needs `--features decision/cuda` and nvcc)".into())
            }
        }
    }
}

impl Loaded {
    /// Open a laya checkpoint dir (`crate::engine::model_files(Kind::Laya)`, the Hugging Face layout) on
    /// `gpu` or the CPU at `dtype`. A GPU that cannot be created — or CUDA in a binary built
    /// without `decision/cuda` — falls back to CPU f32 with a warning.
    pub fn open(dir: &Path, gpu: Gpu, dtype: DType) -> anyhow::Result<Loaded> {
        let t = Instant::now();
        let picked = gpu.open();
        let (device, dtype) = match picked {
            Ok(d) => (d, dtype),
            Err(e) => {
                tracing::warn!("decision: {e}; candle runs on the CPU in f32");
                (Device::Cpu, DType::F32)
            }
        };
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer/tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer/tokenizer.json: {e}"))?;
        let agent_raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("rl_agent_config.json"))?)
                .map_err(|e| anyhow::anyhow!("rl_agent_config.json: {e}"))?;
        let mut laya: LayaConfig = serde_json::from_value(agent_raw.clone())
            .map_err(|e| anyhow::anyhow!("rl_agent_config.json: {e}"))?;
        laya.special = Special::from_tokenizer(&tokenizer)
            .map_err(|e| anyhow::anyhow!("tokenizer/tokenizer.json: {e}"))?;
        let encoder_raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("encoder/config.json"))?)
                .map_err(|e| anyhow::anyhow!("encoder/config.json: {e}"))?;
        let encoder_cfg = EncoderConfig::from_value(&encoder_raw)?;
        let agent_cfg = AgentConfig::from_value(&agent_raw, encoder_cfg.max_position_embeddings)?;

        let weights = dir.join("model.safetensors");
        // Integrity is the downloader's job (size + sha256 against the manifest); here the
        // header parse is the size check (safetensors refuses offsets past the file) and the
        // hash is only the log's identity, so it runs off the load path — after the upload,
        // which it would otherwise compete with for the page cache.
        let model_sha: ModelSha = Arc::new(Mutex::new(format!(
            "size:{}",
            std::fs::metadata(&weights)?.len()
        )));

        // SAFETY: the file is read-only for the process lifetime; the mmap is what laya-rs's
        // own loader does (`Agent::load`).
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
        verify_header(&header, &encoder_cfg, &agent_cfg).map_err(anyhow::Error::msg)?;
        let names = header.into_iter().map(|(n, _, _)| n).collect::<Vec<_>>();
        let backend = SanitizedBackend::new(Box::new(mmaped), names);
        let vb = VarBuilder::from_backend(Box::new(backend), dtype, device.clone());
        let model = DecisionModel::load(vb, &encoder_cfg, &agent_cfg, dtype, &device)
            .map_err(|e| anyhow::anyhow!("load {}: {e}", weights.display()))?;
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
            model,
            device,
            dtype,
            tokenizer,
            laya,
            model_sha,
            load_time: t.elapsed(),
        })
    }

    /// [`build_sequence`] with this checkpoint's tokenizer and budgets.
    pub fn build(&self, q: &Question) -> Result<Sequence, String> {
        build_sequence(&self.tokenizer, q, &self.laya)
    }

    /// One forward pass over `rows` (dynamic S and K, at least two marker slots — the act
    /// head's top-2 features need them), one [`RowOut`] per row. The head's logits come back
    /// raw (unused slots at −1e4) and go through our [`postprocess`]; nothing is calibrated twice.
    pub fn run<R: Borrow<Sequence>>(&self, rows: &[R]) -> anyhow::Result<Vec<RowOut>> {
        self.run_with(rows, false)
    }

    /// [`Self::run`] that also copies the pooled `[CLS]` state into
    /// `RowOut::embedding` when `embed` (126 §7.2): one `to_vec2` per batch,
    /// no second forward; the families that do not ask pay nothing.
    pub fn run_with<R: Borrow<Sequence>>(
        &self,
        rows: &[R],
        embed: bool,
    ) -> anyhow::Result<Vec<RowOut>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let batch = collate(rows, Some((0, 2)), self.laya.special.pad);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &self.device)?;
        let out = self.model.forward(&ids, &attn, &pos, &mask, &qtype)?;
        let logits: Vec<f32> = out.logits.flatten_all()?.to_vec1()?;
        let act: Vec<f32> = out.action.flatten_all()?.to_vec1()?;
        if logits.iter().chain(&act).any(|v| !v.is_finite()) {
            anyhow::bail!(
                "non-finite model output on {:?} {:?} (S {} K {})",
                self.device.location(),
                self.dtype,
                batch.s,
                batch.k
            );
        }
        let mut pooled: Vec<Vec<f32>> = if embed {
            out.pooled.to_vec2()?
        } else {
            Vec::new()
        };
        let (k, na) = (batch.k, act.len() / batch.b);
        Ok(rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let mut r = postprocess(
                    row.borrow(),
                    &logits[i * k..(i + 1) * k],
                    &act[i * na..(i + 1) * na],
                    &self.laya,
                );
                if embed {
                    r.embedding = Some(std::mem::take(&mut pooled[i]));
                }
                r
            })
            .collect())
    }
}

impl Oracle for Loaded {
    fn model_sha(&self) -> ModelSha {
        self.model_sha.clone()
    }
    fn build(&self, q: &Question) -> Result<Box<dyn Row>, String> {
        Loaded::build(self, q).map(|s| Box::new(s) as Box<dyn Row>)
    }
    fn run(&mut self, rows: &[Box<dyn Row>]) -> anyhow::Result<Vec<Scores>> {
        self.run_embed(rows, false)
    }
    fn run_embed(&mut self, rows: &[Box<dyn Row>], embed: bool) -> anyhow::Result<Vec<Scores>> {
        let seqs = rows
            .iter()
            .map(|r| {
                r.as_any()
                    .downcast_ref::<Sequence>()
                    .ok_or_else(|| anyhow::anyhow!("laya oracle given a row it did not build"))
            })
            .collect::<anyhow::Result<Vec<&Sequence>>>()?;
        Ok(Loaded::run_with(self, &seqs, embed)?
            .into_iter()
            .map(Scores::from)
            .collect())
    }
    fn load_time(&self) -> Duration {
        self.load_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laya::{CLS, MASK, PAD, SEP};

    fn row(len: usize, markers: &[i64], qtype: i64) -> Sequence {
        let mut input_ids = vec![CLS];
        input_ids.extend(
            (1..len as i64 - 1).map(|i| if markers.contains(&i) { MASK } else { 1000 + i }),
        );
        input_ids.push(SEP);
        Sequence {
            attention_mask: vec![1; input_ids.len()],
            input_ids,
            marker_pos: markers.to_vec(),
            marker_mask: vec![true; markers.len()],
            qtype,
            truncated: false,
            state_cut: false,
            markers_lost: false,
        }
    }

    /// The tensors `DecisionModel::forward` receives: shapes `[B,S] [B,S] [B,K] [B,K] [B]`,
    /// laya-rs's dtypes, K floored at 2 even when every row has one marker.
    #[test]
    fn inputs_match_the_forward_signature() {
        let rows = [row(12, &[3], 1), row(20, &[3, 7, 11], 0)];
        let batch = collate(&rows, Some((0, 2)), PAD);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &Device::Cpu).unwrap();
        assert_eq!(ids.dims(), &[2, 20]);
        assert_eq!(attn.dims(), &[2, 20]);
        assert_eq!(pos.dims(), &[2, 3]);
        assert_eq!(mask.dims(), &[2, 3]);
        assert_eq!(qtype.dims(), &[2]);
        assert_eq!(
            [
                ids.dtype(),
                attn.dtype(),
                pos.dtype(),
                mask.dtype(),
                qtype.dtype()
            ],
            [DType::U32, DType::U8, DType::I64, DType::U8, DType::U32]
        );
        assert_eq!(
            attn.to_vec2::<u8>().unwrap()[0][11..],
            [1, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(mask.to_vec2::<u8>().unwrap(), [[1, 0, 0], [1, 1, 1]]);
        assert_eq!(qtype.to_vec1::<u32>().unwrap(), [1, 0]);

        let one = collate(&rows[..1], Some((0, 2)), PAD);
        let [_, _, pos, mask, _] = inputs(&one, &Device::Cpu).unwrap();
        assert_eq!(pos.dims(), &[1, 2]);
        assert_eq!(mask.to_vec2::<u8>().unwrap(), [[1, 0]]);
    }

    fn configs() -> (EncoderConfig, AgentConfig) {
        let enc = EncoderConfig::from_value(&serde_json::json!({
            "model_type": "modernbert", "vocab_size": 100, "hidden_size": 16,
            "intermediate_size": 24, "num_hidden_layers": 2, "num_attention_heads": 2,
            "max_position_embeddings": 64
        }))
        .unwrap();
        let agent = AgentConfig::from_value(
            &serde_json::json!({"encoder": "x", "head_layers": 1, "max_len": 64, "head_max_len": 16}),
            64,
        )
        .unwrap();
        (enc, agent)
    }

    /// The PyTorch layout (`in_proj_weight`, `scorer.3`) and the MLX layout (`in_proj.weight`,
    /// `scorer.layers.3`) both verify.
    fn header(pytorch: bool, dtype: DType) -> Vec<HeaderEntry> {
        let (in_proj, scorer, act) = if pytorch {
            (
                "head.layers.0.self_attn.in_proj_weight",
                "scorer.3.weight",
                "act_head.2.weight",
            )
        } else {
            (
                "head.layers.0.self_attn.in_proj.weight",
                "scorer.layers.3.weight",
                "act_head.layers.2.weight",
            )
        };
        [
            ("encoder.embeddings.tok_embeddings.weight", vec![100, 16]),
            ("encoder.final_norm.weight", vec![16]),
            ("encoder.layers.1.attn.Wqkv.weight", vec![48, 16]),
            (in_proj, vec![48, 16]),
            (scorer, vec![1, 16]),
            (act, vec![2, 256]),
            ("type_emb.weight", vec![3, 16]),
        ]
        .into_iter()
        .map(|(n, s)| (n.to_owned(), dtype, s))
        .collect()
    }

    #[test]
    fn header_verification() {
        let (enc, agent) = configs();
        assert_eq!(
            verify_header(&header(true, DType::F16), &enc, &agent),
            Ok(())
        );
        assert_eq!(
            verify_header(&header(false, DType::BF16), &enc, &agent),
            Ok(())
        );
        // A layer short (the file is a smaller model than the config says).
        let mut h = header(true, DType::F16);
        h.retain(|(n, _, _)| n != "encoder.layers.1.attn.Wqkv.weight");
        let e = verify_header(&h, &enc, &agent).unwrap_err();
        assert!(
            e.contains("missing tensors encoder.layers.1.attn.Wqkv.weight"),
            "{e}"
        );
        assert!(e.contains("6 in file"));
        // Another vocabulary.
        let mut h = header(true, DType::F16);
        h[0].2 = vec![50368, 16];
        assert!(
            verify_header(&h, &enc, &agent)
                .unwrap_err()
                .contains("[100, 16]")
        );
        // A quantised export.
        let mut h = header(true, DType::F32);
        h[3].1 = DType::U8;
        let e = verify_header(&h, &enc, &agent).unwrap_err();
        assert!(e.contains("in_proj_weight` is U8"), "{e}");
    }
}
