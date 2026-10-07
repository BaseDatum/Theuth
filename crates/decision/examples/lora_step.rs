//! 126 W0 spike: can candle 0.11 train a LoRA adapter through the vendored laya encoder?
//!
//! Needs `--features train` (the encoder's rotary call becomes `rope_slow`, which has a
//! backward). Loads the real checkpoint from `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`, as `tests/model.rs` does), keeps every base tensor frozen, attaches a
//! rank-`r` LoRA to `Wqkv` / `Wo` / `Wi` / `Wo` of every encoder layer, and runs `--steps`
//! AdamW steps on one fixed synthetic batch with a Choice-style cross-entropy on the decision
//! head's logits. Prints per-step wall time and loss; exits 0 when the loss at the last step is
//! below the loss at the first.
//!
//! ```text
//! cargo run -p decision --features train --example lora_step -- [--device metal|cpu]
//!     [--batch 8] [--seq 512] [--rank 8] [--steps 50] [--lr 1e-3] [--probe] [--save DIR]
//! ```
//!
//! `--probe` only runs one `backward()` through the stock `candle_nn::rotary_emb::rope` and
//! through `rope_slow` on an encoder-shaped tensor and prints what each returns.
//!
//! How the adapter reaches the frozen model without touching `model.rs`: the model's `Linear`s
//! are private, so instead of wrapping them the adapter is applied through the `VarBuilder`.
//! [`LoraBackend`] serves every checkpoint tensor from a device-resident cache and, for the four
//! adapted matrices per layer, returns `W + scale · (B · A)` with `A`, `B` `Var`s from a
//! `VarMap`. The sum is a graph node, so `backward()` reaches `A` and `B`; the model is rebuilt
//! from the `VarBuilder` every step (112 rank-8 matmuls — the base tensors are only cloned)
//! so the merged weights follow the optimiser. Mathematically the same LoRA as a wrapped
//! `Linear`; the shipped `lora.rs` (W3) should wrap the `Linear`s instead so the forward does
//! not materialise merged weights.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use candle_core::{DType, Device, Error, Result, Shape, Tensor, Var};
use candle_nn::var_builder::SimpleBackend;
use candle_nn::{AdamW, Init, Optimizer, ParamsAdamW, VarBuilder, VarMap};
use decision::candle::config::{AgentConfig, EncoderConfig};
use decision::candle::model::DecisionModel;
use decision::candle::weights::{SanitizedBackend, sanitize_name};
use decision::engine::model_files;

/// Serves the frozen checkpoint from device memory and merges the LoRA delta on lookup.
struct LoraBackend {
    /// Sanitized name -> tensor on the device in the model dtype (frozen, no graph).
    base: HashMap<String, Tensor>,
    /// Sanitized name -> `(A (r, in), B (out, r))`, f32 `Var`-backed tensors.
    adapters: HashMap<String, (Tensor, Tensor)>,
    scale: f64,
}

impl LoraBackend {
    fn merged(&self, name: &str, dtype: DType) -> Result<Tensor> {
        let base = self
            .base
            .get(name)
            .ok_or_else(|| Error::CannotFindTensor { path: name.into() })?;
        match self.adapters.get(name) {
            // Cast the (tiny) factors first so the weight-sized delta and its gradient are
            // in the model dtype (f16 on Metal); scale folds into B.
            Some((a, b)) => {
                base + (b * self.scale)?.to_dtype(dtype)?.matmul(&a.to_dtype(dtype)?)?
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

struct Args {
    device: String,
    batch: usize,
    seq: usize,
    rank: usize,
    steps: usize,
    lr: f64,
    probe: bool,
    save: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args {
        device: "metal".into(),
        batch: 8,
        seq: 512,
        rank: 8,
        steps: 50,
        lr: 1e-3,
        probe: false,
        save: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--device" => a.device = value(),
            "--batch" => a.batch = value().parse().unwrap(),
            "--seq" => a.seq = value().parse().unwrap(),
            "--rank" => a.rank = value().parse().unwrap(),
            "--steps" => a.steps = value().parse().unwrap(),
            "--lr" => a.lr = value().parse().unwrap(),
            "--probe" => a.probe = true,
            "--save" => a.save = Some(PathBuf::from(value())),
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

/// `~/.theuth/models/decision` or `$DECISION_MODEL_DIR`, checked for `model_files(Kind::Laya)`.
fn model_dir() -> PathBuf {
    let data = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth")))
        .expect("HOME");
    let dir = std::env::var_os("DECISION_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models/decision"));
    for f in model_files(decision::Kind::Laya) {
        assert!(dir.join(f).is_file(), "missing {} in {}", f, dir.display());
    }
    dir
}

/// Deterministic xorshift so the batch is the same on every run and device.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One `backward()` through each fused candle-nn op the forward uses and through its
/// primitive-composed sibling, on encoder-shaped tensors. Reports whether a gradient reaches
/// the input: `apply_op*_no_bwd` ops return `Ok` and silently drop it.
fn probe(device: &Device) -> Result<()> {
    use candle_nn::{Module, ops, rotary_emb};
    let (heads, seq, head_dim) = (16usize, 8usize, 64usize);
    let x = Var::randn(0f32, 1f32, (1, heads, seq, head_dim), device)?;
    let cos = Tensor::randn(0f32, 1f32, (seq, head_dim / 2), device)?;
    let sin = Tensor::randn(0f32, 1f32, (seq, head_dim / 2), device)?;
    let w = Tensor::ones(head_dim, DType::F32, device)?;
    let b = Tensor::zeros(head_dim, DType::F32, device)?;
    let biased = candle_nn::LayerNorm::new(w.clone(), b.clone(), 1e-5);
    let cases: Vec<(&str, Box<dyn Fn(&Tensor) -> Result<Tensor>>)> = vec![
        ("rope", Box::new(|t| rotary_emb::rope(t, &cos, &sin))),
        ("rope_slow", Box::new(|t| rotary_emb::rope_slow(t, &cos, &sin))),
        ("softmax_last_dim", Box::new(ops::softmax_last_dim)),
        ("softmax(D::Minus1)", Box::new(|t| ops::softmax(t, candle_core::D::Minus1))),
        ("LayerNorm(bias).forward", Box::new(|t| biased.forward(t))),
        ("layer_norm_slow", Box::new(|t| ops::layer_norm_slow(t, &w, &b, 1e-5))),
    ];
    for (name, f) in cases {
        let y = f(x.as_tensor())?;
        match y.sum_all()?.backward() {
            Ok(grads) => match grads.get(&x) {
                Some(g) => println!(
                    "{name} on {:?}: backward ok, input grad {:?}",
                    device.location(),
                    g.dims()
                ),
                None => println!(
                    "{name} on {:?}: backward Ok but NO gradient reaches the input (graph cut)",
                    device.location()
                ),
            },
            Err(e) => println!("{name} on {:?}: backward FAILED: {e}", device.location()),
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = parse_args();
    let device = match args.device.as_str() {
        "metal" => Device::new_metal(0)?,
        "cpu" => Device::Cpu,
        other => anyhow::bail!("--device {other}: metal or cpu"),
    };
    if args.probe {
        probe(&device)?;
        return Ok(());
    }
    // Same dtype rule as `candle::Gpu::dtype`: f16 on Metal, f32 on CPU.
    let dtype = if device.is_metal() {
        DType::F16
    } else {
        DType::F32
    };

    // ---- load the frozen checkpoint the way `Loaded::open` does (crates/decision/src/candle/mod.rs)
    let dir = model_dir();
    let t = Instant::now();
    let agent_raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("rl_agent_config.json"))?)?;
    let encoder_raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("encoder/config.json"))?)?;
    let enc = EncoderConfig::from_value(&encoder_raw)?;
    let agent = AgentConfig::from_value(&agent_raw, enc.max_position_embeddings)?;
    // SAFETY: read-only for the process lifetime, as in `Loaded::open`.
    let mmaped =
        unsafe { candle_core::safetensors::MmapedSafetensors::new(dir.join("model.safetensors"))? };
    let stored: Vec<String> = mmaped.tensors().into_iter().map(|(n, _)| n).collect();
    let sanitized = SanitizedBackend::new(Box::new(mmaped), stored.iter().cloned());
    let mut base = HashMap::with_capacity(stored.len());
    for name in &stored {
        let name = sanitize_name(name);
        let tensor = sanitized.get_unchecked(&name, dtype, &device)?;
        base.insert(name, tensor);
    }
    let base_params: usize = base.values().map(Tensor::elem_count).sum();
    println!(
        "loaded {} tensors ({base_params} params, {dtype:?}) on {:?} in {:.1}s",
        base.len(),
        device.location(),
        t.elapsed().as_secs_f64()
    );

    // ---- rank-r LoRA on Wqkv / Wo / Wi / Wo of every encoder layer, f32 Vars in a VarMap
    let varmap = VarMap::new();
    let mut adapters = HashMap::new();
    for layer in 0..enc.num_hidden_layers {
        for matrix in ["attn.Wqkv", "attn.Wo", "mlp.Wi", "mlp.Wo"] {
            let name = format!("encoder.layers.{layer}.{matrix}.weight");
            let (out, inp) = base[&name].dims2()?;
            let a = varmap.get(
                (args.rank, inp),
                &format!("{name}.lora_a"),
                Init::Randn {
                    mean: 0.,
                    stdev: 1. / (inp as f64).sqrt(),
                },
                DType::F32,
                &device,
            )?;
            let b = varmap.get(
                (out, args.rank),
                &format!("{name}.lora_b"),
                Init::Const(0.),
                DType::F32,
                &device,
            )?;
            adapters.insert(name, (a, b));
        }
    }
    let lora_params: usize = varmap
        .all_vars()
        .iter()
        .map(|v| v.as_tensor().elem_count())
        .sum();
    println!(
        "lora rank {}: {} matrices, {lora_params} trainable params ({:.1} MB f32)",
        args.rank,
        adapters.len(),
        lora_params as f64 * 4. / 1e6
    );
    let backend = LoraBackend {
        base,
        adapters,
        scale: 2.0, // alpha = 2r
    };
    let vb = VarBuilder::from_backend(Box::new(backend), dtype, device.clone());

    // ---- one fixed synthetic batch: random ids, no padding, 4 marker slots, Choice qtype 0
    let (b, s, k) = (args.batch, args.seq, 4usize);
    let mut rng = Rng(0x126_0007);
    let ids: Vec<u32> = (0..b * s).map(|_| rng.below(enc.vocab_size) as u32).collect();
    let mut pos = Vec::with_capacity(b * k);
    for _ in 0..b {
        let mut row: Vec<i64> = (0..k).map(|_| rng.below(s - 1) as i64 + 1).collect();
        row.sort_unstable();
        pos.extend(row);
    }
    let labels: Vec<u32> = (0..b).map(|_| rng.below(k) as u32).collect();
    let input_ids = Tensor::from_vec(ids, (b, s), &device)?;
    let attention_mask = Tensor::ones((b, s), DType::U8, &device)?;
    let marker_pos = Tensor::from_vec(pos, (b, k), &device)?;
    let marker_mask = Tensor::ones((b, k), DType::U8, &device)?;
    let qtype = Tensor::zeros(b, DType::U32, &device)?;
    let labels = Tensor::from_vec(labels, b, &device)?;

    let mut opt = AdamW::new(
        varmap.all_vars(),
        ParamsAdamW {
            lr: args.lr,
            ..Default::default()
        },
    )?;

    println!("batch {b} x seq {s}, {} steps, lr {}", args.steps, args.lr);
    println!("step  load_ms  fwd_ms  bwd_ms  opt_ms  total_ms  loss");
    let mut losses = Vec::with_capacity(args.steps);
    let mut steady = Vec::new();
    for step in 1..=args.steps {
        let t0 = Instant::now();
        let model = DecisionModel::load(vb.clone(), &enc, &agent, dtype, &device)?;
        device.synchronize()?;
        let t_load = t0.elapsed();
        let out = model.forward(&input_ids, &attention_mask, &marker_pos, &marker_mask, &qtype)?;
        let loss = candle_nn::loss::cross_entropy(&out.logits, &labels)?;
        let loss_v = loss.to_scalar::<f32>()?;
        let t_fwd = t0.elapsed() - t_load;
        let grads = loss.backward()?;
        device.synchronize()?;
        let t_bwd = t0.elapsed() - t_load - t_fwd;
        if step == 1 {
            // Does the gradient reach the q and k thirds of Wqkv (they pass through rope) or
            // only the v third (which bypasses it)? Stock `rope` cuts the graph silently.
            let vars = varmap.data().lock().unwrap();
            for layer in [0, enc.num_hidden_layers - 1] {
                let name = format!("encoder.layers.{layer}.attn.Wqkv.weight.lora_b");
                let g = grads.get(vars[&name].as_tensor());
                let norms: Vec<String> = (0..3)
                    .map(|i| match g {
                        Some(g) => g
                            .narrow(0, i * enc.hidden_size, enc.hidden_size)
                            .and_then(|t| t.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>())
                            .map_or_else(|e| e.to_string(), |n| format!("{n:.3e}")),
                        None => "none".into(),
                    })
                    .collect();
                println!("  layer {layer} Wqkv.lora_b grad norm q {} | k {} | v {}", norms[0], norms[1], norms[2]);
            }
        }
        opt.step(&grads)?;
        device.synchronize()?;
        let total = t0.elapsed();
        let t_opt = total - t_load - t_fwd - t_bwd;
        println!(
            "{step:>4}  {:>7.0}  {:>6.0}  {:>6.0}  {:>6.0}  {:>8.0}  {loss_v:.4}",
            t_load.as_secs_f64() * 1e3,
            t_fwd.as_secs_f64() * 1e3,
            t_bwd.as_secs_f64() * 1e3,
            t_opt.as_secs_f64() * 1e3,
            total.as_secs_f64() * 1e3,
        );
        if !loss_v.is_finite() {
            anyhow::bail!("non-finite loss at step {step}");
        }
        losses.push(loss_v);
        if step > 3 {
            steady.push(total.as_secs_f64() * 1e3);
        }
    }

    let mem = decision::process_memory();
    let mean = steady.iter().sum::<f64>() / steady.len().max(1) as f64;
    println!(
        "steady-state (steps 4..): {mean:.0} ms/step mean, {:.0} ms min, {:.0} ms max",
        steady.iter().cloned().fold(f64::INFINITY, f64::min),
        steady.iter().cloned().fold(0., f64::max)
    );
    println!(
        "loss step 1 {:.4} -> step {} {:.4}; phys_footprint {:.0} MB, rss {:.0} MB",
        losses[0],
        losses.len(),
        losses[losses.len() - 1],
        mem.footprint as f64 / 1e6,
        mem.resident as f64 / 1e6
    );
    // A 5,000-row epoch at this batch size, from the steady-state mean.
    println!(
        "5,000-row epoch at batch {b}: {:.1} min",
        5000f64 / b as f64 * mean / 60e3
    );

    if let Some(dir) = &args.save {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("adapter.safetensors");
        varmap.save(&path)?;
        println!(
            "saved {} ({} bytes)",
            path.display(),
            std::fs::metadata(&path)?.len()
        );
    }

    // The gradient-flow check: on a fixed batch the loss must fall.
    let (first, last) = (losses[0], losses[losses.len() - 1]);
    anyhow::ensure!(last < first, "loss did not fall: {first} -> {last}");
    Ok(())
}
