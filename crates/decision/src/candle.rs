//! The candle backend (57 §12.8): laya-rs's `DecisionModel` — ModernBERT + decision head — over
//! the `safetensors` checkpoint, on Metal (f16) or CPU (f32). Only the forward pass is theirs:
//! rendering ([`build_sequence`]), padding ([`collate`]), calibration and confidence
//! ([`postprocess`]), the gate and the log are the ORT path's, so the two engines are
//! interchangeable behind [`Oracle`].

use std::path::Path;
use std::time::{Duration, Instant};

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use laya::config::{AgentConfig, EncoderConfig};
use laya::model::DecisionModel;
use laya::weights::SanitizedBackend;
use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;

use crate::engine::{CANDLE_FILES, Oracle};
use crate::laya::{Batch, LayaConfig, RowOut, Sequence, build_sequence, collate, postprocess};
use crate::question::Question;

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
/// sha256.
pub struct CandleLoaded {
    model: DecisionModel,
    /// Where the weights live.
    pub device: Device,
    /// f16 on Metal, f32 on CPU by default.
    pub dtype: DType,
    /// laya's fast BPE tokenizer.
    pub tokenizer: Tokenizer,
    /// Budgets and temperatures from `rl_agent_config.json`.
    pub laya: LayaConfig,
    /// sha256 hex of `model.safetensors`, part of every gate version.
    pub model_sha: String,
    /// Wall-clock of `open`, hashing included.
    pub load_time: Duration,
    /// The sha256 pass alone (843 MB; the part of `load_time` that is not candle).
    pub hash_time: Duration,
}

impl CandleLoaded {
    /// Open a laya checkpoint dir ([`CANDLE_FILES`], Hugging Face layout) on Metal (`metal`) or
    /// CPU at `dtype`. A Metal device that cannot be created falls back to CPU f32 with a warning.
    pub fn open(dir: &Path, metal: bool, dtype: DType) -> anyhow::Result<CandleLoaded> {
        debug_assert!(CANDLE_FILES.iter().all(|f| dir.join(f).is_file()));
        let t = Instant::now();
        let (device, dtype) = match metal {
            false => (Device::Cpu, dtype),
            true => match Device::new_metal(0) {
                Ok(d) => (d, dtype),
                Err(e) => {
                    tracing::warn!("decision: Metal unavailable ({e}); candle runs on the CPU in f32");
                    (Device::Cpu, DType::F32)
                }
            },
        };
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer/tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer/tokenizer.json: {e}"))?;
        let agent_raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("rl_agent_config.json"))?)
                .map_err(|e| anyhow::anyhow!("rl_agent_config.json: {e}"))?;
        let laya: LayaConfig = serde_json::from_value(agent_raw.clone())
            .map_err(|e| anyhow::anyhow!("rl_agent_config.json: {e}"))?;
        let encoder_raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("encoder/config.json"))?)
                .map_err(|e| anyhow::anyhow!("encoder/config.json: {e}"))?;
        let encoder_cfg = EncoderConfig::from_value(&encoder_raw)?;
        let agent_cfg = AgentConfig::from_value(&agent_raw, encoder_cfg.max_position_embeddings)?;

        let weights = dir.join("model.safetensors");
        let th = Instant::now();
        let mut hasher = Sha256::new();
        std::io::copy(&mut std::fs::File::open(&weights)?, &mut hasher)?;
        let model_sha = format!("{:x}", hasher.finalize());
        let hash_time = th.elapsed();

        // SAFETY: the file is read-only for the process lifetime; the mmap is what laya-rs's
        // own loader does (`Agent::load`).
        let mmaped = unsafe { candle_core::safetensors::MmapedSafetensors::new(&weights)? };
        let names: Vec<String> = mmaped.tensors().into_iter().map(|(n, _)| n).collect();
        let backend = SanitizedBackend::new(Box::new(mmaped), names);
        let vb = VarBuilder::from_backend(Box::new(backend), dtype, device.clone());
        let model = DecisionModel::load(vb, &encoder_cfg, &agent_cfg, dtype, &device)
            .map_err(|e| anyhow::anyhow!("load {}: {e}", weights.display()))?;
        Ok(CandleLoaded {
            model,
            device,
            dtype,
            tokenizer,
            laya,
            model_sha,
            load_time: t.elapsed(),
            hash_time,
        })
    }

    /// [`build_sequence`] with this checkpoint's tokenizer and budgets.
    pub fn build(&self, q: &Question) -> Result<Sequence, String> {
        build_sequence(&self.tokenizer, q, &self.laya)
    }

    /// One forward pass over `rows` (dynamic S and K, at least two marker slots — the act
    /// head's top-2 features need them), one [`RowOut`] per row. The head's logits come back
    /// raw (unused slots at −1e4) and go through our [`postprocess`]; nothing is calibrated twice.
    pub fn run(&self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let batch = collate(rows, Some((0, 2)));
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
        let (k, na) = (batch.k, act.len() / batch.b);
        Ok(rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                postprocess(
                    row,
                    &logits[i * k..(i + 1) * k],
                    &act[i * na..(i + 1) * na],
                    &self.laya,
                )
            })
            .collect())
    }
}

impl Oracle for CandleLoaded {
    fn model_sha(&self) -> &str {
        &self.model_sha
    }
    fn build(&self, q: &Question) -> Result<Sequence, String> {
        CandleLoaded::build(self, q)
    }
    fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
        CandleLoaded::run(self, rows)
    }
    fn load_time(&self) -> Duration {
        self.load_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laya::{CLS, MASK, SEP};

    fn row(len: usize, markers: &[i64], qtype: i64) -> Sequence {
        let mut input_ids = vec![CLS];
        input_ids.extend((1..len as i64 - 1).map(|i| if markers.contains(&i) { MASK } else { 1000 + i }));
        input_ids.push(SEP);
        Sequence {
            attention_mask: vec![1; input_ids.len()],
            input_ids,
            marker_pos: markers.to_vec(),
            marker_mask: vec![true; markers.len()],
            qtype,
            truncated: false,
        }
    }

    /// The tensors `DecisionModel::forward` receives: shapes `[B,S] [B,S] [B,K] [B,K] [B]`,
    /// laya-rs's dtypes, K floored at 2 even when every row has one marker.
    #[test]
    fn inputs_match_the_forward_signature() {
        let rows = [row(12, &[3], 1), row(20, &[3, 7, 11], 0)];
        let batch = collate(&rows, Some((0, 2)));
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &Device::Cpu).unwrap();
        assert_eq!(ids.dims(), &[2, 20]);
        assert_eq!(attn.dims(), &[2, 20]);
        assert_eq!(pos.dims(), &[2, 3]);
        assert_eq!(mask.dims(), &[2, 3]);
        assert_eq!(qtype.dims(), &[2]);
        assert_eq!(
            [ids.dtype(), attn.dtype(), pos.dtype(), mask.dtype(), qtype.dtype()],
            [DType::U32, DType::U8, DType::I64, DType::U8, DType::U32]
        );
        assert_eq!(attn.to_vec2::<u8>().unwrap()[0][11..], [1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(mask.to_vec2::<u8>().unwrap(), [[1, 0, 0], [1, 1, 1]]);
        assert_eq!(qtype.to_vec1::<u32>().unwrap(), [1, 0]);

        let one = collate(&rows[..1], Some((0, 2)));
        let [_, _, pos, mask, _] = inputs(&one, &Device::Cpu).unwrap();
        assert_eq!(pos.dims(), &[1, 2]);
        assert_eq!(mask.to_vec2::<u8>().unwrap(), [[1, 0]]);
    }
}
