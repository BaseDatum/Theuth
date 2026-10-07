//! Rank-r LoRA over the frozen laya checkpoint (126 §7.3, W3): the adapter the double's Tier B
//! trains and the default build applies at inference.
//!
//! The vendored `Linear`s in [`model`](super::model) are private fields, so the adapter is not a
//! wrapped layer: it is applied **through the `VarBuilder`**. [`LoraBackend`] serves every
//! checkpoint tensor from device memory and, for the four adapted matrices of every encoder layer
//! (`attn.Wqkv`, `attn.Wo`, `mlp.Wi`, `mlp.Wo`), returns `W + (α/r)·B·A` with `A (r, in)` and
//! `B (out, r)` f32 `Var`s. While training the sum is a graph node, so `backward()` reaches `A`
//! and `B`; the model is rebuilt from the builder every step so the merged weights follow the
//! optimiser (the W0 spike's shape, 40–90 ms of the measured 1,089 ms step). At inference the
//! same merge runs once, detached, at load: the 93 unadapted tensors are shared `Tensor` clones
//! of the base, only the 112 adapted matrices are materialised, and swapping adapters is a
//! model rebuild (`Base::model`), never a copy of the checkpoint. Unmerged inference (an adapter
//! swapped per call under one model) needs `EncoderAttention`/`EncoderMlp` to hold a
//! `LoraLinear` — a `model.rs` change that is not W3's to make; see the as-built's hand-offs.
//!
//! **The silent graph cut.** Without the `train` cargo feature the forward uses candle-nn's
//! fused `rope` / `softmax_last_dim` / biased `LayerNorm`, which are applied with
//! `apply_op*_no_bwd`: `backward()` returns `Ok` and no gradient reaches the adapter — the loss
//! stays flat with no error. [`TRAIN_BUILD`] is that fact as a constant; every trainer must refuse
//! to run when it is `false`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use candle_core::backprop::GradStore;
use candle_core::{D, DType, Device, Error, Result, Shape, Tensor, Var};
use candle_nn::var_builder::SimpleBackend;
use candle_nn::{Init, VarBuilder, VarMap};
use tokenizers::Tokenizer;

use super::config::{AgentConfig, EncoderConfig, FULL_ATTENTION, SLIDING_ATTENTION};
use super::model::DecisionModel;
use super::weights::{SanitizedBackend, sanitize_name};
use super::{HeaderEntry, verify_header};
use crate::laya::{LayaConfig, Sequence, Special, build_sequence};
use crate::question::Question;

/// `true` when this binary was built with `--features decision/train`, i.e. the encoder's
/// forward is differentiable end to end. A trainer running with `false` would silently train
/// nothing (module docs).
pub const TRAIN_BUILD: bool = cfg!(feature = "train");

/// The matrices the adapter attaches to, per encoder layer.
pub const ADAPTED: [&str; 4] = ["attn.Wqkv", "attn.Wo", "mlp.Wi", "mlp.Wo"];

/// `encoder.layers.<i>.<matrix>.weight` for every layer and every [`ADAPTED`] matrix.
pub fn adapted_names(enc: &EncoderConfig) -> Vec<String> {
    (0..enc.num_hidden_layers)
        .flat_map(|layer| {
            ADAPTED
                .iter()
                .map(move |m| format!("encoder.layers.{layer}.{m}.weight"))
        })
        .collect()
}

/// Deterministic xorshift64\*: the adapter init, the epoch order and the rehearsal sample must
/// not depend on the device RNG (the CPU backend cannot be seeded), so every random draw in the
/// trainer goes through this.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// Seeded; a zero seed is nudged (xorshift's fixed point).
    pub fn new(seed: u64) -> Self {
        Rng((seed ^ 0x9E37_79B9_7F4A_7C15).max(1))
    }

    /// FNV-1a of `s`, to fold a name into a seed.
    pub fn hash(s: &str) -> u64 {
        s.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn uniform(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f32 {
        let u1 = self.uniform().max(1e-7);
        let u2 = self.uniform();
        (-2. * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }

    /// Fisher–Yates.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// The frozen checkpoint on one device: configs, tokenizer and every tensor already uploaded
/// in the model dtype (plain `Tensor`s, so frozen by construction — `backward()` only populates
/// `Var`s). One `Base` serves any number of adapters and model rebuilds.
pub struct Base {
    /// `encoder/config.json`.
    pub enc: EncoderConfig,
    /// `rl_agent_config.json`.
    pub agent: AgentConfig,
    /// Budgets, temperatures and the tokenizer's special ids, as `Loaded` reads them.
    pub laya: LayaConfig,
    /// `tokenizer/tokenizer.json`.
    pub tokenizer: Tokenizer,
    /// Sanitized name → tensor on `device` in `dtype`.
    pub tensors: Arc<HashMap<String, Tensor>>,
    /// The model dtype every tensor was uploaded in (f16 on Metal, f32 elsewhere).
    pub dtype: DType,
    /// Where the tensors live; every rebuild and every adapter uses it.
    pub device: Device,
    /// sha256 hex of `model.safetensors` (`synthetic` for the test fixture): the manifest's
    /// `base_hash`.
    pub sha: String,
}

impl Base {
    /// Open a laya checkpoint dir (the `Loaded::open` layout) and upload every tensor to
    /// `device` at `dtype`. Hashes the weights synchronously (~2 s for 842 MB) — a trainer needs
    /// the base hash in its manifest before it writes anything.
    pub fn open(dir: &Path, device: Device, dtype: DType) -> anyhow::Result<Base> {
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
        let enc = EncoderConfig::from_value(&encoder_raw)?;
        let agent = AgentConfig::from_value(&agent_raw, enc.max_position_embeddings)?;

        let weights = dir.join("model.safetensors");
        // SAFETY: read-only for the process lifetime, as in `Loaded::open`.
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
        verify_header(&header, &enc, &agent).map_err(anyhow::Error::msg)?;
        let stored: Vec<String> = header.into_iter().map(|(n, _, _)| n).collect();
        let sanitized = SanitizedBackend::new(Box::new(mmaped), stored.iter().cloned());
        let mut tensors = HashMap::with_capacity(stored.len());
        for name in &stored {
            let name = sanitize_name(name);
            let tensor = sanitized.get_unchecked(&name, dtype, &device)?;
            tensors.insert(name, tensor);
        }
        let sha = platform::sha256::cached(&weights)?;
        Ok(Base {
            enc,
            agent,
            laya,
            tokenizer,
            tensors: Arc::new(tensors),
            dtype,
            device,
            sha,
        })
    }

    /// The test fixture: a 2-layer, hidden-64, 4-head, vocab-128 model with random weights and
    /// a word-level tokenizer over [`SYNTHETIC_WORDS`], built in memory on `device` in f32 — no
    /// checkpoint is read. The weights come from candle's own inits, which the CPU backend
    /// cannot seed (`Device::set_seed` bails there): two calls give two different bases, one
    /// base is consistent for its lifetime.
    pub fn synthetic(device: &Device) -> anyhow::Result<Base> {
        let enc = EncoderConfig {
            vocab_size: 128,
            hidden_size: 64,
            intermediate_size: 64,
            num_hidden_layers: 2,
            num_attention_heads: 4,
            model_type: "modernbert".into(),
            norm_eps: 1e-5,
            norm_bias: false,
            attention_bias: false,
            mlp_bias: false,
            hidden_activation: "gelu".into(),
            local_attention: 16,
            global_attn_every_n_layers: 2,
            global_rope_theta: 160_000.0,
            local_rope_theta: 10_000.0,
            max_position_embeddings: 128,
            layer_types: Some(vec![FULL_ATTENTION.into(), SLIDING_ATTENTION.into()]),
            rope_parameters: None,
        };
        let agent = AgentConfig {
            head_layers: 1,
            max_len: 64,
            head_max_len: 24,
            act_classes: 2,
            temperature: [1.0; 3],
            temperature_by_options: Default::default(),
        };
        let mut vocab = serde_json::Map::new();
        for w in ["[UNK]", "[CLS]", "[SEP]", "[PAD]", "[MASK]"]
            .into_iter()
            .chain(SYNTHETIC_WORDS.split_whitespace())
        {
            let n = vocab.len();
            vocab.entry(w).or_insert(n.into());
        }
        assert!(vocab.len() <= enc.vocab_size);
        let spec = serde_json::json!({
            "version": "1.0",
            "pre_tokenizer": { "type": "Whitespace" },
            "model": { "type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]" },
        });
        let tokenizer = Tokenizer::from_bytes(serde_json::to_vec(&spec)?)
            .map_err(|e| anyhow::anyhow!("synthetic tokenizer: {e}"))?;
        let laya = LayaConfig {
            max_len: agent.max_len,
            head_max_len: agent.head_max_len,
            special: Special::from_tokenizer(&tokenizer).map_err(anyhow::Error::msg)?,
            ..LayaConfig::default()
        };
        // Random init through a VarMap-backed builder (candle's own `linear`/`embedding`
        // inits), then snapshot every tensor detached: a checkpoint that never touched a file.
        let varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, device);
        DecisionModel::load(vb, &enc, &agent, DType::F32, device)?;
        let tensors: HashMap<String, Tensor> = varmap
            .data()
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_tensor().detach()))
            .collect();
        Ok(Base {
            enc,
            agent,
            laya,
            tokenizer,
            tensors: Arc::new(tensors),
            dtype: DType::F32,
            device: device.clone(),
            sha: "synthetic".into(),
        })
    }

    /// Build the model: the plain checkpoint when `lora` is `None`, else the base merged with
    /// the adapter. `train` keeps the merge in the autograd graph (one rebuild per step);
    /// inference passes `false` so the merged weights are detached constants.
    pub fn model(&self, lora: Option<&Lora>, train: bool) -> Result<DecisionModel> {
        let backend = LoraBackend {
            base: Arc::clone(&self.tensors),
            adapters: lora.map(|l| l.adapters.clone()).unwrap_or_default(),
            scale: lora.map_or(0., |l| l.scale),
            train,
        };
        let vb = VarBuilder::from_backend(Box::new(backend), self.dtype, self.device.clone());
        DecisionModel::load(vb, &self.enc, &self.agent, self.dtype, &self.device)
    }

    /// [`build_sequence`] with this checkpoint's tokenizer and budgets, the state cut to
    /// `max_len` tokens (the trainer's sequence cap; `None` = the checkpoint's own).
    pub fn sequence(
        &self,
        q: &Question,
        max_len: Option<usize>,
    ) -> std::result::Result<Sequence, String> {
        let mut cfg = self.laya.clone();
        if let Some(m) = max_len {
            cfg.max_len = m.min(cfg.max_len);
        }
        build_sequence(&self.tokenizer, q, &cfg)
    }
}

/// The synthetic tokenizer's vocabulary (besides the specials); test card texts draw from it.
pub const SYNTHETIC_WORDS: &str = "choice question : shell git push pull test cargo build rm \
    allow deny none escalate add to backlog discard start now approve file read write path \
    home develop theuth docs src main rs toml yes no the a of on in and for with at";

/// Serves the frozen checkpoint from device memory and merges the LoRA delta on lookup.
struct LoraBackend {
    base: Arc<HashMap<String, Tensor>>,
    /// Sanitized name → `(A (r, in), B (out, r))`.
    adapters: HashMap<String, (Tensor, Tensor)>,
    scale: f64,
    train: bool,
}

impl LoraBackend {
    fn merged(&self, name: &str, dtype: DType) -> Result<Tensor> {
        let base = self
            .base
            .get(name)
            .ok_or_else(|| Error::CannotFindTensor { path: name.into() })?;
        match self.adapters.get(name) {
            // Cast the (tiny) factors first so the weight-sized delta and its gradient are in
            // the model dtype (f16 on Metal: the 151172a4 fix halved the step); scale folds
            // into B.
            Some((a, b)) => {
                let (a, b) = if self.train {
                    (a.clone(), b.clone())
                } else {
                    (a.detach(), b.detach())
                };
                base + (b * self.scale)?
                    .to_dtype(dtype)?
                    .matmul(&a.to_dtype(dtype)?)?
            }
            None => Ok(base.clone()),
        }
    }
}

impl SimpleBackend for LoraBackend {
    fn get(&self, s: Shape, name: &str, _: Init, dtype: DType, _: &Device) -> Result<Tensor> {
        let tensor = self.merged(name, dtype)?;
        if tensor.shape() != &s {
            candle_core::bail!("shape mismatch for {name}: {s:?} <> {:?}", tensor.shape());
        }
        Ok(tensor)
    }

    fn get_unchecked(&self, name: &str, dtype: DType, _: &Device) -> Result<Tensor> {
        self.merged(name, dtype)
    }

    fn contains_tensor(&self, name: &str) -> bool {
        self.base.contains_key(name)
    }
}

/// The adapter: f32 `Var`s `A (r, in)` (init `N(0, 1/√in)`) and `B (out, r)` (init 0) for every
/// [`adapted_names`] entry, named `<weight>.lora_a` / `<weight>.lora_b` in a `VarMap`, scale
/// α/r = 2 (α = 2r). Saved and loaded as safetensors under those names.
pub struct Lora {
    /// `r`.
    pub rank: usize,
    /// α/r, folded into `B` at merge time.
    pub scale: f64,
    varmap: VarMap,
    adapters: HashMap<String, (Tensor, Tensor)>,
}

impl Lora {
    /// Fresh adapter at `rank`: `A ~ N(0, 1/√in)` from [`Rng`] seeded by `seed` and the weight
    /// name (so the init is identical on every device and run), `B = 0`.
    pub fn new(base: &Base, rank: usize, seed: u64) -> Result<Lora> {
        let varmap = VarMap::new();
        let mut adapters = HashMap::new();
        for name in adapted_names(&base.enc) {
            let w = base
                .tensors
                .get(&name)
                .ok_or_else(|| Error::CannotFindTensor { path: name.clone() })?;
            let (out, inp) = w.dims2()?;
            let mut rng = Rng::new(seed ^ Rng::hash(&name));
            let stdev = 1. / (inp as f32).sqrt();
            let init: Vec<f32> = (0..rank * inp).map(|_| rng.normal() * stdev).collect();
            let a = Var::from_tensor(&Tensor::from_vec(init, (rank, inp), &base.device)?)?;
            let a = {
                let ta = a.as_tensor().clone();
                varmap
                    .data()
                    .lock()
                    .unwrap()
                    .insert(format!("{name}.lora_a"), a);
                ta
            };
            let b = varmap.get(
                (out, rank),
                &format!("{name}.lora_b"),
                Init::Const(0.),
                DType::F32,
                &base.device,
            )?;
            adapters.insert(name, (a, b));
        }
        Ok(Lora {
            rank,
            scale: 2.0,
            varmap,
            adapters,
        })
    }

    /// Load `adapter.safetensors` written by [`Lora::save`]; the rank comes from the file.
    /// Every adapted matrix of `base` must be present.
    pub fn load(base: &Base, path: &Path) -> Result<Lora> {
        let tensors = candle_core::safetensors::load(path, &base.device)?;
        let varmap = VarMap::new();
        let mut adapters = HashMap::new();
        let mut rank = 0;
        for name in adapted_names(&base.enc) {
            let (ka, kb) = (format!("{name}.lora_a"), format!("{name}.lora_b"));
            let a = tensors
                .get(&ka)
                .ok_or_else(|| Error::CannotFindTensor { path: ka.clone() })?
                .to_dtype(DType::F32)?;
            let b = tensors
                .get(&kb)
                .ok_or_else(|| Error::CannotFindTensor { path: kb.clone() })?
                .to_dtype(DType::F32)?;
            rank = a.dim(0)?;
            let (va, vb) = (Var::from_tensor(&a)?, Var::from_tensor(&b)?);
            let (ta, tb) = (va.as_tensor().clone(), vb.as_tensor().clone());
            let mut data = varmap.data().lock().unwrap();
            data.insert(ka, va);
            data.insert(kb, vb);
            adapters.insert(name, (ta, tb));
        }
        Ok(Lora {
            rank,
            scale: 2.0,
            varmap,
            adapters,
        })
    }

    /// `VarMap::save`: the `lora_a` / `lora_b` tensors as safetensors.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.varmap.save(path)
    }

    /// The adapter's `Var`s by name (`AdamW::new` reads it).
    pub fn varmap(&self) -> &VarMap {
        &self.varmap
    }

    /// Every adapter `Var`.
    pub fn vars(&self) -> Vec<Var> {
        self.varmap.all_vars()
    }

    /// Trainable scalars.
    pub fn param_count(&self) -> usize {
        self.vars().iter().map(|v| v.as_tensor().elem_count()).sum()
    }

    /// The `(A, B)` pair of one adapted weight (tests read gradients off it).
    pub fn pair(&self, weight_name: &str) -> Option<&(Tensor, Tensor)> {
        self.adapters.get(weight_name)
    }
}

/// Choice cross-entropy summed over the rows with per-row weights:
/// `Σ_i w_i · −log softmax(logits_i)[label_i]`. `logits (B, K)` f32 (unused slots already at
/// −1e4), `labels (B,)` u32, `weights (B,)` f32. The caller divides by the effective batch's
/// total weight, so a batch split into micro-batches sums to the same loss and gradient.
pub fn weighted_nll_sum(logits: &Tensor, labels: &Tensor, weights: &Tensor) -> Result<Tensor> {
    let lp = candle_nn::ops::log_softmax(logits, D::Minus1)?;
    let picked = lp.gather(&labels.unsqueeze(1)?, 1)?.squeeze(1)?;
    (picked.neg()? * weights)?.sum_all()
}

/// AdamW with decoupled weight decay (candle-nn's formula) whose moments live in a `VarMap` so
/// the optimiser state checkpoints with `save`/`load` — candle's own `AdamW` keeps them private.
pub struct AdamW {
    /// `(param, m, v)` in name order.
    slots: Vec<(Var, Var, Var)>,
    state: VarMap,
    t: usize,
    /// Learning rate.
    pub lr: f64,
    /// First-moment decay.
    pub beta1: f64,
    /// Second-moment decay.
    pub beta2: f64,
    /// Denominator floor.
    pub eps: f64,
    /// Decoupled weight decay (`θ ← θ(1 − lr·wd)` before the Adam step).
    pub weight_decay: f64,
}

impl AdamW {
    /// Moments for every var of `params`, zeroed; betas 0.9 / 0.999, eps 1e-8, decay 0.01.
    pub fn new(params: &VarMap, lr: f64) -> Result<AdamW> {
        let state = VarMap::new();
        let mut names: Vec<(String, Var)> = params
            .data()
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let mut slots = Vec::with_capacity(names.len());
        for (name, p) in names {
            let (shape, dtype, dev) = (p.shape().clone(), p.dtype(), p.device().clone());
            state.get(
                shape.clone(),
                &format!("{name}.m"),
                Init::Const(0.),
                dtype,
                &dev,
            )?;
            state.get(shape, &format!("{name}.v"), Init::Const(0.), dtype, &dev)?;
            let data = state.data().lock().unwrap();
            slots.push((
                p,
                data[&format!("{name}.m")].clone(),
                data[&format!("{name}.v")].clone(),
            ));
        }
        Ok(AdamW {
            slots,
            state,
            t: 0,
            lr,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            weight_decay: 0.01,
        })
    }

    /// Steps taken so far (the bias-correction counter).
    pub fn steps(&self) -> usize {
        self.t
    }

    /// One update from `grads`; params without a gradient are skipped.
    pub fn step(&mut self, grads: &GradStore) -> Result<()> {
        self.t += 1;
        let t = self.t as f64;
        let lr_t = self.lr * (1. - self.beta2.powf(t)).sqrt() / (1. - self.beta1.powf(t));
        for (p, m, v) in &self.slots {
            let Some(g) = grads.get(p.as_tensor()) else {
                continue;
            };
            let m_new = ((m.as_tensor() * self.beta1)? + (g * (1. - self.beta1))?)?;
            let v_new = ((v.as_tensor() * self.beta2)? + (g.sqr()? * (1. - self.beta2))?)?;
            let update = (m_new.clone() * lr_t)?.div(&(v_new.sqrt()? + self.eps)?)?;
            let decayed = (p.as_tensor() * (1. - self.lr * self.weight_decay))?;
            p.set(&(decayed - update)?)?;
            m.set(&m_new)?;
            v.set(&v_new)?;
        }
        Ok(())
    }

    /// Moments plus the step counter (`adamw.t`, one f32) as safetensors.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dev = self
            .slots
            .first()
            .map_or(Device::Cpu, |(p, _, _)| p.device().clone());
        let t = Tensor::new(&[self.t as f32], &dev)?;
        let data = self.state.data().lock().unwrap();
        let all = data
            .iter()
            .map(|(k, v)| (k.clone(), v.as_tensor().clone()))
            .chain(std::iter::once(("adamw.t".to_string(), t)));
        candle_core::safetensors::save(&all.collect::<HashMap<_, _>>(), path)
    }

    /// Restore what [`AdamW::save`] wrote into an optimiser built over the same params.
    pub fn load(&mut self, path: &Path) -> Result<()> {
        let dev = self
            .slots
            .first()
            .map_or(Device::Cpu, |(p, _, _)| p.device().clone());
        let mut tensors = candle_core::safetensors::load(path, &dev)?;
        let t = tensors
            .remove("adamw.t")
            .ok_or_else(|| Error::CannotFindTensor {
                path: "adamw.t".into(),
            })?;
        self.t = t.to_vec1::<f32>()?[0] as usize;
        self.state.set(tensors.iter())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candle::inputs;
    use crate::laya::collate;
    use crate::question::Candidate;

    fn card(state: &str) -> Question {
        Question::Choice {
            instructions: "shell git push".into(),
            candidates: ["allow", "deny", "none"]
                .iter()
                .map(|id| Candidate {
                    id: (*id).into(),
                    text: String::new(),
                })
                .collect(),
            state: state.into(),
        }
    }

    /// One backward through the tiny model with a fresh adapter; the gradient of
    /// `encoder.layers.<l>.attn.Wqkv.weight.lora_b`'s q / k / v thirds at a layer.
    fn qkv_grad_norms(layer: usize) -> [f32; 3] {
        let base = Base::synthetic(&Device::Cpu).unwrap();
        let lora = Lora::new(&base, 4, 7).unwrap();
        let model = base.model(Some(&lora), true).unwrap();
        let rows = [
            base.sequence(&card("cargo test in develop theuth"), None)
                .unwrap(),
            base.sequence(&card("rm the home path"), None).unwrap(),
        ];
        let batch = collate(&rows, Some((0, 2)), base.laya.special.pad);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &Device::Cpu).unwrap();
        let out = model.forward(&ids, &attn, &pos, &mask, &qtype).unwrap();
        let labels = Tensor::new(&[0u32, 1], &Device::Cpu).unwrap();
        let weights = Tensor::new(&[1f32, 1.], &Device::Cpu).unwrap();
        let loss = weighted_nll_sum(&out.logits, &labels, &weights).unwrap();
        assert!(loss.to_scalar::<f32>().unwrap().is_finite());
        let grads = loss.backward().unwrap();
        let (_, b) = lora
            .pair(&format!("encoder.layers.{layer}.attn.Wqkv.weight"))
            .unwrap();
        let h = base.enc.hidden_size;
        std::array::from_fn(|i| match grads.get(b) {
            Some(g) => g
                .narrow(0, i * h, h)
                .unwrap()
                .sqr()
                .unwrap()
                .sum_all()
                .unwrap()
                .sqrt()
                .unwrap()
                .to_scalar::<f32>()
                .unwrap(),
            None => 0.,
        })
    }

    /// With `train`: the gradient crosses rope into q and k and reaches v at the first and
    /// the last layer (the W0 finding, on the tiny model).
    #[cfg(feature = "train")]
    #[test]
    fn gradient_reaches_every_third_of_wqkv_at_both_ends() {
        for layer in [0, 1] {
            let [q, k, v] = qkv_grad_norms(layer);
            assert!(
                q > 0. && k > 0. && v > 0.,
                "layer {layer}: q {q} k {k} v {v}"
            );
        }
    }

    /// Without `train`: candle-nn 0.11's fused ops cut the graph silently — `backward()` is
    /// `Ok` and the adapter gets no gradient at all. The reason `TRAIN_BUILD` exists; if a
    /// candle upgrade makes this fail, the guard can go.
    #[cfg(not(feature = "train"))]
    #[test]
    fn default_build_cuts_the_graph_silently() {
        assert!(!TRAIN_BUILD);
        let [q, k, v] = qkv_grad_norms(0);
        assert_eq!([q, k, v], [0., 0., 0.]);
    }

    /// Save → load round-trips the factors and the rank; the merged forward is identical.
    #[test]
    fn adapter_round_trips_through_safetensors() {
        let dir = tempfile::tempdir().unwrap();
        let base = Base::synthetic(&Device::Cpu).unwrap();
        let lora = Lora::new(&base, 4, 3).unwrap();
        // Make B non-zero so the merge is visible.
        for v in lora.vars() {
            v.set(&v.as_tensor().ones_like().unwrap()).unwrap();
        }
        let path = dir.path().join("adapter.safetensors");
        lora.save(&path).unwrap();
        let back = Lora::load(&base, &path).unwrap();
        assert_eq!(back.rank, 4);
        assert_eq!(back.param_count(), lora.param_count());
        let row = base.sequence(&card("git push"), None).unwrap();
        let batch = collate(&[row], Some((0, 2)), base.laya.special.pad);
        let [ids, attn, pos, mask, qtype] = inputs(&batch, &Device::Cpu).unwrap();
        let a = base.model(Some(&lora), false).unwrap();
        let b = base.model(Some(&back), false).unwrap();
        let plain = base.model(None, false).unwrap();
        let la: Vec<f32> = a
            .forward(&ids, &attn, &pos, &mask, &qtype)
            .unwrap()
            .logits
            .flatten_all()
            .unwrap()
            .to_vec1()
            .unwrap();
        let lb: Vec<f32> = b
            .forward(&ids, &attn, &pos, &mask, &qtype)
            .unwrap()
            .logits
            .flatten_all()
            .unwrap()
            .to_vec1()
            .unwrap();
        let lp: Vec<f32> = plain
            .forward(&ids, &attn, &pos, &mask, &qtype)
            .unwrap()
            .logits
            .flatten_all()
            .unwrap()
            .to_vec1()
            .unwrap();
        assert_eq!(la, lb);
        assert_ne!(la, lp, "the adapter must change the logits");
    }

    /// The optimiser state round-trips: a step after `load` equals the step the uninterrupted
    /// optimiser takes.
    #[test]
    fn adamw_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let dev = Device::Cpu;
        let make = || {
            let vm = VarMap::new();
            vm.get(3, "w", Init::Const(1.), DType::F32, &dev).unwrap();
            vm
        };
        let grad = |vm: &VarMap| {
            let mut g = GradStore::default();
            let w = vm.data().lock().unwrap()["w"].clone();
            g.insert(
                w.as_tensor(),
                Tensor::new(&[0.5f32, -1., 2.], &dev).unwrap(),
            );
            g
        };
        let (vm_a, vm_b) = (make(), make());
        let (mut a, mut b) = (
            AdamW::new(&vm_a, 1e-2).unwrap(),
            AdamW::new(&vm_b, 1e-2).unwrap(),
        );
        a.step(&grad(&vm_a)).unwrap();
        a.step(&grad(&vm_a)).unwrap();
        let p = dir.path().join("optim.safetensors");
        a.save(&p).unwrap();
        b.step(&grad(&vm_b)).unwrap();
        b.step(&grad(&vm_b)).unwrap();
        b.load(&p).unwrap();
        assert_eq!(b.steps(), 2);
        a.step(&grad(&vm_a)).unwrap();
        b.step(&grad(&vm_b)).unwrap();
        let wa: Vec<f32> = vm_a.data().lock().unwrap()["w"]
            .as_tensor()
            .to_vec1()
            .unwrap();
        let wb: Vec<f32> = vm_b.data().lock().unwrap()["w"]
            .as_tensor()
            .to_vec1()
            .unwrap();
        assert_eq!(wa, wb);
        assert_ne!(wa, vec![1., 1., 1.]);
    }
}
