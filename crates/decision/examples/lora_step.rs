//! 126 W0 spike, now a thin client of `decision::candle::lora` (W3): train a rank-`r` LoRA
//! through the vendored laya encoder on one fixed synthetic batch and print per-step timings.
//!
//! Needs `--features train` (the encoder's rope / softmax / biased layer-norm become their
//! differentiable siblings). Loads the real checkpoint from `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`, as `tests/model.rs` does). **Do not run it on a machine someone is
//! using**: a Metal step saturates the GPU (126 as-built W0).
//!
//! ```text
//! cargo run -p decision --features train --example lora_step -- [--device metal|cpu]
//!     [--batch 2] [--seq 256] [--rank 8] [--steps 50] [--lr 1e-4] [--probe] [--save DIR]
//! ```
//!
//! `--probe` only runs one `backward()` through each fused candle-nn op the forward uses and
//! through its primitive-composed sibling, and prints what each returns.

use std::path::PathBuf;
use std::time::Instant;

use candle_core::{DType, Device, Result, Tensor, Var};
use decision::candle::lora::{AdamW, Base, Lora, TRAIN_BUILD, weighted_nll_sum};
use decision::engine::model_files;

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
        batch: 2,
        seq: 256,
        rank: 8,
        steps: 50,
        lr: 1e-4,
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
    type Case<'a> = (&'a str, Box<dyn Fn(&Tensor) -> Result<Tensor> + 'a>);
    let cases: Vec<Case> = vec![
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
    anyhow::ensure!(TRAIN_BUILD, "built without the `train` feature: the graph is cut");
    // Same dtype rule as `candle::Gpu::dtype`: f16 on Metal, f32 on CPU.
    let dtype = if device.is_metal() {
        DType::F16
    } else {
        DType::F32
    };

    let t = Instant::now();
    let base = Base::open(&model_dir(), device.clone(), dtype)?;
    let base_params: usize = base.tensors.values().map(Tensor::elem_count).sum();
    println!(
        "loaded {} tensors ({base_params} params, {dtype:?}) on {:?} in {:.1}s",
        base.tensors.len(),
        device.location(),
        t.elapsed().as_secs_f64()
    );
    let lora = Lora::new(&base, args.rank, 0x126)?;
    println!(
        "lora rank {}: {} trainable params ({:.1} MB f32)",
        args.rank,
        lora.param_count(),
        lora.param_count() as f64 * 4. / 1e6
    );

    // ---- one fixed synthetic batch: random ids, no padding, 4 marker slots, Choice qtype 0
    let (b, s, k) = (args.batch, args.seq, 4usize);
    let mut rng = Rng(0x126_0007);
    let ids: Vec<u32> = (0..b * s).map(|_| rng.below(base.enc.vocab_size) as u32).collect();
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
    let weights = Tensor::full(1. / b as f32, b, &device)?;

    let mut opt = AdamW::new(lora.varmap(), args.lr)?;

    println!("batch {b} x seq {s}, {} steps, lr {}", args.steps, args.lr);
    println!("step  load_ms  fwd_ms  bwd_ms  opt_ms  total_ms  loss");
    let mut losses = Vec::with_capacity(args.steps);
    let mut steady = Vec::new();
    for step in 1..=args.steps {
        let t0 = Instant::now();
        let model = base.model(Some(&lora), true)?;
        device.synchronize()?;
        let t_load = t0.elapsed();
        let out = model.forward(&input_ids, &attention_mask, &marker_pos, &marker_mask, &qtype)?;
        let loss = weighted_nll_sum(&out.logits, &labels, &weights)?;
        let loss_v = loss.to_scalar::<f32>()?;
        let t_fwd = t0.elapsed() - t_load;
        let grads = loss.backward()?;
        device.synchronize()?;
        let t_bwd = t0.elapsed() - t_load - t_fwd;
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
    println!(
        "5,000-row epoch at batch {b}: {:.1} min",
        5000f64 / b as f64 * mean / 60e3
    );

    if let Some(dir) = &args.save {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("adapter.safetensors");
        lora.save(&path)?;
        println!(
            "saved {} ({} bytes)",
            path.display(),
            std::fs::metadata(&path)?.len()
        );
    }

    let (first, last) = (losses[0], losses[losses.len() - 1]);
    anyhow::ensure!(last < first, "loss did not fall: {first} -> {last}");
    Ok(())
}
