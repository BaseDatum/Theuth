//! GLINER-3 bench: GLiNER2.5-Decide cold load, per-row latency and peak RSS for 1 / 8 / 48
//! labels at states of ~50 / ~400 subwords, one backend per process (peak RSS is per process).
//!
//! `cargo run --release -p decision --example gliner_bench -- --backend candle --device metal|cpu`
//! `cargo run --release -p decision --features gliner-onnx --example gliner_bench -- --backend ort --model model_fp32.onnx [--threads 8] [--coreml ...]`
//!
//! `--iters N` (default 10, median and p95 reported after 2 warm-ups). The checkpoint dir is
//! `$THEUTH_MODELS_DIR/decision-gliner` (`~/.theuth/models`); ORT exports come from
//! `$GLINER_ONNX_DIR` (`/tmp/69/gliner-onnx`). Prints one markdown table row per cell.

use std::path::PathBuf;
use std::time::Instant;

use decision::engine::Scores;
use decision::gliner::prompt::GlinerRow;
use decision::{Candidate, Question};
use tokenizers::Tokenizer;

/// Peak resident set (`ru_maxrss`, bytes on macOS, KiB on Linux) in MiB.
fn peak_rss_mib() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let bytes = if cfg!(target_os = "macos") {
        ru.ru_maxrss as f64
    } else {
        ru.ru_maxrss as f64 * 1024.0
    };
    bytes / (1024.0 * 1024.0)
}

fn footprint_mib() -> f64 {
    decision::process_memory().footprint as f64 / (1024.0 * 1024.0)
}

/// A state of about `subwords` subwords of operational prose.
fn state_of(tok: &Tokenizer, subwords: usize) -> String {
    let unit = "Row 7: order #1007 shipped from depot 2, carrier UPS, status delayed. ";
    let mut s = String::new();
    while tok.encode(s.as_str(), false).map(|e| e.len()).unwrap_or(0) < subwords {
        s.push_str(unit);
    }
    s
}

fn question(labels: usize, state: String) -> Question {
    if labels == 1 {
        return Question::Score {
            instructions: "Is this shipment late?".into(),
            levels: vec!["late".into()],
            state,
        };
    }
    Question::Choice {
        instructions: "Which page should open next?".into(),
        candidates: (0..labels)
            .map(|i| Candidate {
                id: format!("p{i}"),
                text: format!("page {i} settings"),
            })
            .collect(),
        state,
    }
}

trait Backend {
    fn build(&self, q: &Question) -> GlinerRow;
    fn run(&mut self, row: &GlinerRow) -> Scores;
}

struct Candle(decision::gliner::Loaded);
impl Backend for Candle {
    fn build(&self, q: &Question) -> GlinerRow {
        self.0.build(q).unwrap()
    }
    fn run(&mut self, row: &GlinerRow) -> Scores {
        self.0.run(&[row]).unwrap().remove(0)
    }
}

#[cfg(feature = "gliner-onnx")]
struct Ort(decision::gliner::onnx::Loaded);
#[cfg(feature = "gliner-onnx")]
impl Backend for Ort {
    fn build(&self, q: &Question) -> GlinerRow {
        self.0.build(q).unwrap()
    }
    fn run(&mut self, row: &GlinerRow) -> Scores {
        self.0.run(&[row]).unwrap().remove(0)
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let models = std::env::var_os("THEUTH_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth/models")
        });
    let dir = models.join("decision-gliner");
    // ORT-only flags, read only under `gliner-onnx`.
    #[cfg_attr(not(feature = "gliner-onnx"), allow(dead_code))]
    struct OrtFlags {
        model: String,
        threads: usize,
        coreml: Option<String>,
    }
    let (mut backend, mut metal, mut iters) =
        ("candle".to_owned(), decision::Device::Auto.metal(), 10usize);
    let mut ort = OrtFlags {
        model: "model_fp32.onnx".to_owned(),
        threads: 8,
        coreml: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--backend" => backend = args.next().unwrap(),
            "--device" => metal = args.next().as_deref() == Some("metal"),
            "--iters" => iters = args.next().unwrap().parse()?,
            "--model" => ort.model = args.next().unwrap(),
            "--threads" => ort.threads = args.next().unwrap().parse()?,
            "--coreml" => ort.coreml = args.next(),
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let tokenizer =
        Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow::anyhow!("{e}"))?;
    let before = footprint_mib();
    let t = Instant::now();
    let (label, mut b): (String, Box<dyn Backend>) = match backend.as_str() {
        "candle" => {
            let gpu = if metal {
                decision::candle::Gpu::Metal
            } else {
                decision::candle::Gpu::None
            };
            let m = decision::gliner::Loaded::open(&dir, gpu)?;
            (
                format!("candle {:?} {:?}", m.device.location(), m.dtype),
                Box::new(Candle(m)),
            )
        }
        #[cfg(feature = "gliner-onnx")]
        "ort" => {
            use decision::gliner::onnx::{Ep, Loaded, data_dir, export_dir};
            let ep = match ort.coreml.as_deref() {
                None => Ep::Cpu {
                    threads: ort.threads,
                },
                // `mlprogram|nn`,`all|ane`,`static|dynamic`
                Some(spec) => {
                    let p: Vec<&str> = spec.split(',').collect();
                    Ep::CoreML {
                        ml_program: p.first() != Some(&"nn"),
                        all_units: p.get(1) != Some(&"ane"),
                        static_shapes: p.get(2) == Some(&"static"),
                        cache_dir: data_dir().join("cache/coreml/gliner-spike"),
                    }
                }
            };
            let m = Loaded::open(&export_dir().join(&ort.model), &dir, &data_dir(), &ep)?;
            if ort.coreml.is_some() {
                println!("coreml {ep:?}: {:?}", m.partitions);
            }
            (format!("ort {} {ep:?}", ort.model), Box::new(Ort(m)))
        }
        other => anyhow::bail!("backend {other} not built (ort needs --features gliner-onnx)"),
    };
    let cold = t.elapsed();
    println!(
        "{label}: cold load {} ms, footprint before {before:.0} MiB → after load {:.0} MiB, peak {:.0} MiB",
        cold.as_millis(),
        footprint_mib(),
        peak_rss_mib()
    );
    println!("| backend | labels | S | p50 ms | p95 ms | peak RSS MiB | footprint MiB | top p |");
    println!("|---|---|---|---|---|---|---|---|");
    for labels in [1usize, 8, 48] {
        for subwords in [50usize, 400] {
            let row = b.build(&question(labels, state_of(&tokenizer, subwords)));
            let s = row.input_ids.len();
            for _ in 0..2 {
                b.run(&row);
            }
            let mut times = Vec::with_capacity(iters);
            let mut top = 0.0f32;
            for _ in 0..iters {
                let t = Instant::now();
                let out = b.run(&row);
                times.push(t.elapsed());
                top = out.probs.iter().copied().fold(0.0, f32::max);
            }
            times.sort();
            let p = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
            println!(
                "| {label} | {labels} | {s} | {:.1} | {:.1} | {:.0} | {:.0} | {top:.3} |",
                p(0.5).as_secs_f64() * 1e3,
                p(0.95).as_secs_f64() * 1e3,
                peak_rss_mib(),
                footprint_mib()
            );
        }
    }
    Ok(())
}
