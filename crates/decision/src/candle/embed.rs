//! Retrieval embeddings (144 §7.2): the vendored [`ModernBert`] encoder alone — no decision head —
//! over a Hugging Face checkpoint directory (`config.json`, `model.safetensors`, `tokenizer.json`),
//! `[CLS]` pooled and L2-normalised; f16 on Metal, f32 on the CPU. `Scores.embedding` (the
//! oracle's post-head state) is a different thing and untouched.

use std::path::Path;

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use tokenizers::Tokenizer;

use super::Gpu;
use super::config::EncoderConfig;
use super::model::ModernBert;

/// Texts per forward pass (§7.2).
pub const BATCH: usize = 32;
/// Tokens kept per text, specials included: a 400-token chunk plus its heading fits; a query is
/// cut here (§7.3 "truncated at 400 tokens").
pub const MAX_TOKENS: usize = 512;

/// The three files a checkpoint directory must hold.
pub const FILES: [&str; 3] = ["config.json", "model.safetensors", "tokenizer.json"];

/// A loaded encoder on one device.
pub struct Embedder {
    model: ModernBert,
    tokenizer: Tokenizer,
    device: Device,
    dtype: DType,
    /// `hidden_size`: the vector length.
    pub dims: usize,
    pad: u32,
    max_len: usize,
}

/// `config.json` of `dir`, validated as a ModernBERT encoder.
pub fn config(dir: &Path) -> anyhow::Result<EncoderConfig> {
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)
        .map_err(|e| anyhow::anyhow!("config.json: {e}"))?;
    Ok(EncoderConfig::from_value(&raw)?)
}

/// `tokenizer.json` of `dir`.
pub fn tokenizer(dir: &Path) -> anyhow::Result<Tokenizer> {
    Tokenizer::from_file(dir.join("tokenizer.json"))
        .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))
}

/// Tokens of `text` as the model sees it (specials included), capped at [`MAX_TOKENS`].
pub fn count_tokens(tok: &Tokenizer, text: &str) -> anyhow::Result<usize> {
    let enc = tok
        .encode(text, true)
        .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
    Ok(enc.len())
}

impl Embedder {
    /// Open `dir` on `gpu` (CPU f32 with a warning when the GPU cannot be created). Weights
    /// stored under a `model.` prefix (a `ForMaskedLM`-style save) load like a bare
    /// `ModernBertModel` save.
    pub fn open(dir: &Path, gpu: Gpu) -> anyhow::Result<Embedder> {
        let (device, dtype) = match gpu.open() {
            Ok(d) => (d, gpu.dtype()),
            Err(e) => {
                tracing::warn!("embed: {e}; candle runs on the CPU in f32");
                (Device::Cpu, DType::F32)
            }
        };
        let cfg = config(dir)?;
        let tokenizer = tokenizer(dir)?;
        let weights = dir.join("model.safetensors");
        // SAFETY: the file is read-only for the process lifetime (the loader's own contract,
        // `Loaded::open`).
        let mmaped = unsafe { candle_core::safetensors::MmapedSafetensors::new(&weights)? };
        let prefixed = mmaped
            .tensors()
            .iter()
            .all(|(name, _)| name.starts_with("model."));
        drop(mmaped);
        // SAFETY: as above.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[&weights], dtype, &device)? };
        let vb = if prefixed { vb.pp("model") } else { vb };
        let model = ModernBert::load(vb, &cfg, dtype, &device)
            .map_err(|e| anyhow::anyhow!("load {}: {e}", weights.display()))?;
        let pad = tokenizer
            .get_padding()
            .map(|p| p.pad_id)
            .or_else(|| tokenizer.token_to_id("[PAD]"))
            .unwrap_or(0);
        Ok(Embedder {
            model,
            tokenizer,
            device,
            dtype,
            dims: cfg.hidden_size,
            pad,
            max_len: MAX_TOKENS.min(cfg.max_position_embeddings),
        })
    }

    /// The checkpoint's tokenizer.
    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// One unit vector per text, in order; [`BATCH`] texts per forward.
    pub fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            out.extend(self.batch(batch)?);
        }
        Ok(out)
    }

    fn batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(texts.len());
        for t in texts {
            let enc = self
                .tokenizer
                .encode(t.as_str(), true)
                .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
            let mut ids = enc.get_ids().to_vec();
            ids.truncate(self.max_len);
            rows.push(ids);
        }
        let s = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let b = rows.len();
        let mut ids = Vec::with_capacity(b * s);
        let mut mask = Vec::with_capacity(b * s);
        for r in &rows {
            ids.extend_from_slice(r);
            ids.extend(std::iter::repeat_n(self.pad, s - r.len()));
            mask.extend(std::iter::repeat_n(1u8, r.len()));
            mask.extend(std::iter::repeat_n(0u8, s - r.len()));
        }
        let ids = Tensor::from_vec(ids, (b, s), &self.device)?;
        let mask = Tensor::from_vec(mask, (b, s), &self.device)?;
        let hidden = self.model.forward(&ids, &mask)?;
        let cls = hidden.i((.., 0))?.to_dtype(DType::F32)?;
        let norm = cls
            .sqr()?
            .sum_keepdim(1)?
            .sqrt()?
            .clamp(1e-12f32, f32::MAX)?;
        let unit: Vec<Vec<f32>> = cls.broadcast_div(&norm)?.to_vec2()?;
        if unit.iter().flatten().any(|v| !v.is_finite()) {
            anyhow::bail!(
                "non-finite embedding on {:?} {:?} (S {s})",
                self.device.location(),
                self.dtype
            );
        }
        Ok(unit)
    }
}

/// Words the tiny checkpoint's tokenizer knows ([`write_tiny_checkpoint`]); anything else is `[UNK]`.
pub const TINY_WORDS: &str = "the a an of and to in is for on with at by from as it this that \
    failover database cluster replica primary standby node leader election quorum backup restore \
    restart heartbeat timeout retry latency throughput cache queue index search vector block note \
    source notebook meeting board insight heading text chunk model token embed query hit score \
    alpha beta gamma delta one two three four five six seven eight nine ten red green blue";

/// Write a random-weight ModernBERT checkpoint (2 layers, 64 dims, whitespace `WordLevel`
/// tokenizer over [`TINY_WORDS`] with `[CLS]`/`[SEP]` post-processing) into `dir`: the test
/// double for the real `embed-gte-modernbert-base` — same files, same code path, no download.
#[doc(hidden)]
pub fn write_tiny_checkpoint(dir: &Path) -> anyhow::Result<()> {
    use candle_nn::VarMap;
    std::fs::create_dir_all(dir)?;
    let specials = ["[PAD]", "[CLS]", "[SEP]", "[UNK]"];
    let mut vocab = serde_json::Map::new();
    for w in specials.into_iter().chain(TINY_WORDS.split_whitespace()) {
        let n = vocab.len();
        vocab.entry(w).or_insert(n.into());
    }
    let vocab_size = vocab.len().next_multiple_of(8);
    let cfg = serde_json::json!({
        "model_type": "modernbert",
        "architectures": ["ModernBertModel"],
        "vocab_size": vocab_size,
        "hidden_size": 64,
        "intermediate_size": 96,
        "num_hidden_layers": 2,
        "num_attention_heads": 2,
        "hidden_activation": "gelu",
        "norm_eps": 1e-5,
        "norm_bias": false,
        "attention_bias": false,
        "mlp_bias": false,
        "local_attention": 128,
        "global_attn_every_n_layers": 3,
        "global_rope_theta": 160000.0,
        "local_rope_theta": 10000.0,
        "max_position_embeddings": 1024,
        "pad_token_id": 0,
        "cls_token_id": 1,
        "sep_token_id": 2,
    });
    std::fs::write(dir.join("config.json"), serde_json::to_vec_pretty(&cfg)?)?;
    let added: Vec<serde_json::Value> = specials
        .iter()
        .enumerate()
        .map(|(id, content)| {
            serde_json::json!({ "id": id, "content": content, "single_word": false, "lstrip": false,
                "rstrip": false, "normalized": false, "special": true })
        })
        .collect();
    let tok = serde_json::json!({
        "version": "1.0",
        "truncation": null,
        "padding": null,
        "added_tokens": added,
        "normalizer": { "type": "Lowercase" },
        "pre_tokenizer": { "type": "Whitespace" },
        "post_processor": {
            "type": "TemplateProcessing",
            "single": [
                { "SpecialToken": { "id": "[CLS]", "type_id": 0 } },
                { "Sequence": { "id": "A", "type_id": 0 } },
                { "SpecialToken": { "id": "[SEP]", "type_id": 0 } }
            ],
            "pair": [
                { "SpecialToken": { "id": "[CLS]", "type_id": 0 } },
                { "Sequence": { "id": "A", "type_id": 0 } },
                { "SpecialToken": { "id": "[SEP]", "type_id": 0 } },
                { "Sequence": { "id": "B", "type_id": 1 } },
                { "SpecialToken": { "id": "[SEP]", "type_id": 1 } }
            ],
            "special_tokens": {
                "[CLS]": { "id": "[CLS]", "ids": [1], "tokens": ["[CLS]"] },
                "[SEP]": { "id": "[SEP]", "ids": [2], "tokens": ["[SEP]"] }
            }
        },
        "decoder": null,
        "model": { "type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]" }
    });
    std::fs::write(dir.join("tokenizer.json"), serde_json::to_vec(&tok)?)?;
    let enc = EncoderConfig::from_value(&cfg)?;
    let varmap = VarMap::new();
    let vb = VarBuilder::from_varmap(&varmap, DType::F32, &Device::Cpu);
    ModernBert::load(vb, &enc, DType::F32, &Device::Cpu)?;
    varmap.save(dir.join("model.safetensors"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipeline end to end on the tiny checkpoint: 64-dim unit vectors, one per text, in
    /// order, deterministic, and a text equal to itself beats a different one.
    #[test]
    fn tiny_checkpoint_embeds_unit_vectors_in_order() {
        let dir = tempfile::tempdir().unwrap();
        write_tiny_checkpoint(dir.path()).unwrap();
        let e = Embedder::open(dir.path(), Gpu::None).unwrap();
        assert_eq!(e.dims, 64);
        let texts: Vec<String> = (0..BATCH + 3)
            .map(|i| format!("failover cluster {}", ["alpha", "beta", "gamma"][i % 3]))
            .collect();
        let v = e.embed(&texts).unwrap();
        assert_eq!(v.len(), BATCH + 3);
        for row in &v {
            assert_eq!(row.len(), 64);
            let n: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((n - 1.0).abs() < 1e-3, "norm {n}");
        }
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert!(dot(&v[0], &v[3]) > 0.999, "same text, same vector");
        assert!(
            dot(&v[0], &v[1]) < dot(&v[0], &v[3]),
            "different text is further"
        );
        assert_eq!(
            count_tokens(e.tokenizer(), "failover cluster alpha").unwrap(),
            5
        );
    }
}
