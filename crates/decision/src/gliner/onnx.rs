//! GLINER-3 spike (feature `gliner-onnx`, off by default): the GLiNER2.5-Decide classification
//! graph — `(input_ids, attention_mask, marker_positions) → logits [B, N]`, onnx-community's
//! export re-run from revision 5a7adf72 — through ONNX Runtime, as the measured control for
//! [`super::candle`]. Not a production path: `decision` ships ORT-free (57 §12.8); the dylib
//! comes from `models::onnx::ort_lib` (`<data_dir>/models/onnxruntime/`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use super::prompt::{GlinerConfig, GlinerRow, GlinerSpecial, build_row};
use super::{collate, downcast, scores};
use crate::engine::{ModelSha, Oracle, Row, Scores};
use crate::question::Question;

/// Which execution provider the session asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ep {
    /// ORT's CPU EP, `threads` intra-op threads.
    Cpu {
        /// Intra-op threads.
        threads: usize,
    },
    /// The CoreML EP ahead of the CPU EP (the knob sweep of 57 §12.6 / `facts/theuth-ort-coreml-ep-modernbert`).
    CoreML {
        /// `MLProgram` (`true`) or `NeuralNetwork`.
        ml_program: bool,
        /// `All` when `true`, `CPUAndNeuralEngine` otherwise.
        all_units: bool,
        /// `RequireStaticInputShapes`.
        static_shapes: bool,
        /// `ModelCacheDirectory`.
        cache_dir: PathBuf,
    },
}

/// What ORT's INFO log said about CoreML partitioning, when the EP was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partitions {
    /// `number of partitions supported by CoreML: N number of nodes in the graph: M number of nodes supported by CoreML: K`.
    pub partitions: usize,
    /// Nodes in the graph.
    pub nodes: usize,
    /// Nodes CoreML took.
    pub on_coreml: usize,
    /// Every warning/error line ORT logged while building the session.
    pub problems: Vec<String>,
}

/// An ORT session over one export plus the checkpoint's tokenizer.
pub struct Loaded {
    session: Session,
    /// The checkpoint's `tokenizer.json`.
    pub tokenizer: Tokenizer,
    /// Row budget.
    pub cfg: GlinerConfig,
    /// `size:<bytes>` of the `.onnx` graph file (the spike never hashes).
    pub model_sha: ModelSha,
    /// Wall-clock of `open` (dylib init + session build).
    pub load_time: Duration,
    /// CoreML partitioning, when asked for.
    pub partitions: Partitions,
}

impl Loaded {
    /// `model` is the `.onnx` (its `.data` beside it); `dir` holds `tokenizer.json`; `data_dir`
    /// is where `ort_lib` finds the runtime.
    pub fn open(model: &Path, dir: &Path, data_dir: &Path, ep: &Ep) -> anyhow::Result<Loaded> {
        let t = Instant::now();
        models::onnx::ort_lib(data_dir).map_err(anyhow::Error::msg)?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))?;
        GlinerSpecial::from_tokenizer(&tokenizer).map_err(anyhow::Error::msg)?;
        let lines: Arc<Mutex<Vec<(ort::logging::LogLevel, String)>>> = Arc::default();
        // Builder steps hand the builder back inside their error; only the message is wanted.
        let msg = |e: ort::Error<ort::session::builder::SessionBuilder>| anyhow::anyhow!("{e}");
        let mut builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(msg)?
            .with_log_level(ort::logging::LogLevel::Info)
            .map_err(msg)?
            .with_logger(Arc::new({
                let lines = lines.clone();
                move |level, _cat, _id, _loc, msg: &str| lines.lock().push((level, msg.to_owned()))
            }))
            .map_err(msg)?;
        match ep {
            Ep::Cpu { threads } => builder = builder.with_intra_threads(*threads).map_err(msg)?,
            Ep::CoreML {
                ml_program,
                all_units,
                static_shapes,
                cache_dir,
            } => {
                use ort::ep::coreml::{ComputeUnits, CoreML, ModelFormat};
                let coreml = CoreML::default()
                    .with_model_format(if *ml_program {
                        ModelFormat::MLProgram
                    } else {
                        ModelFormat::NeuralNetwork
                    })
                    .with_compute_units(if *all_units {
                        ComputeUnits::All
                    } else {
                        ComputeUnits::CPUAndNeuralEngine
                    })
                    .with_static_input_shapes(*static_shapes)
                    .with_model_cache_dir(cache_dir.display());
                builder = builder
                    .with_execution_providers([coreml.build().error_on_failure()])
                    .map_err(msg)?;
            }
        }
        let session = builder.commit_from_file(model)?;
        let mut partitions = Partitions::default();
        for (level, msg) in lines.lock().iter() {
            if let Some(i) = msg.find("number of partitions supported by CoreML: ") {
                let rest = &msg[i..];
                let nums: Vec<usize> = rest
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if let [p, n, k, ..] = nums[..] {
                    partitions = Partitions {
                        partitions: p,
                        nodes: n,
                        on_coreml: k,
                        problems: std::mem::take(&mut partitions.problems),
                    };
                }
            } else if matches!(
                level,
                ort::logging::LogLevel::Warning
                    | ort::logging::LogLevel::Error
                    | ort::logging::LogLevel::Fatal
            ) {
                partitions.problems.push(msg.clone());
            }
        }
        let model_sha = Arc::new(Mutex::new(format!(
            "size:{}",
            std::fs::metadata(model)?.len()
        )));
        Ok(Loaded {
            session,
            tokenizer,
            cfg: GlinerConfig::default(),
            model_sha,
            load_time: t.elapsed(),
            partitions,
        })
    }

    /// [`build_row`] with this checkpoint's tokenizer.
    pub fn build(&self, q: &Question) -> Result<GlinerRow, String> {
        build_row(&self.tokenizer, q, &self.cfg)
    }

    /// One `session.run` over the collated rows → softmax per row.
    pub fn run(&mut self, rows: &[&GlinerRow]) -> anyhow::Result<Vec<Scores>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let batch = collate(rows);
        let (b, s, n) = (batch.b, batch.s, batch.n);
        let outputs = self.session.run(ort::inputs![
            "input_ids" => Tensor::from_array(([b, s], batch.input_ids))?,
            "attention_mask" => Tensor::from_array(([b, s], batch.attention_mask))?,
            "marker_positions" => Tensor::from_array(([b, n], batch.label_pos))?,
        ])?;
        let (shape, logits) = outputs["logits"].try_extract_tensor::<f32>()?;
        anyhow::ensure!(
            shape.len() == 2 && shape[0] as usize == b && shape[1] as usize == n,
            "logits shape {shape:?}, expected [{b}, {n}]"
        );
        scores(rows, logits, n)
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

/// `$GLINER_ONNX_DIR` (default `/tmp/69/gliner-onnx`), where `export.py` wrote the three files.
pub fn export_dir() -> PathBuf {
    std::env::var_os("GLINER_ONNX_DIR")
        .map_or_else(|| PathBuf::from("/tmp/69/gliner-onnx"), PathBuf::from)
}

/// `$THEUTH_DATA_DIR` (default `~/.theuth`): the ORT dylib's home.
pub fn data_dir() -> PathBuf {
    std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".theuth")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gliner::reference::{self, compare};

    /// `tol = None` only reports (int8 is measured, not held to a bound).
    fn parity(file: &str, tol: Option<f32>) -> Option<(f32, usize)> {
        let dir = reference::checkpoint()?;
        let model = export_dir().join(file);
        if !model.is_file() {
            eprintln!(
                "SKIP: {} missing — run /tmp/69/gliner-onnx/export.py",
                model.display()
            );
            return None;
        }
        let t = Instant::now();
        let mut m = Loaded::open(&model, &dir, &data_dir(), &Ep::Cpu { threads: 8 })
            .unwrap_or_else(|e| panic!("{file}: {e:#}"));
        eprintln!("{file}: loaded in {:?}", t.elapsed());
        let r = reference::load();
        let mut got = Vec::new();
        for c in &r.cases {
            let row = m.build(&c.question).unwrap();
            assert_eq!(row.input_ids, c.input_ids, "{}", c.name);
            got.push(m.run(&[&row]).unwrap().remove(0).probs);
        }
        let (worst, flips, lines) = compare(r.cases.iter().zip(got.iter().map(Vec::as_slice)));
        for l in &lines {
            eprintln!("{l}");
        }
        eprintln!(
            "{file}: {} cases, max |Δp| {worst:.2e}, {flips} argmax flips",
            r.cases.len()
        );
        if let Some(tol) = tol {
            assert!(worst <= tol, "max |Δp| {worst} > {tol}");
        }
        Some((worst, flips))
    }

    #[test]
    fn parity_fp32() {
        if let Some((_, flips)) = parity("model_fp32.onnx", Some(1e-4)) {
            assert_eq!(flips, 0);
        }
    }

    #[test]
    fn parity_fp16_reported() {
        parity("model_fp16.onnx", Some(0.05));
    }

    /// `quantize_dynamic` int8: measured 0.66 max |Δp|, 7/51 flips on this export — unusable,
    /// recorded only.
    #[test]
    fn parity_int8_reported() {
        parity("model_int8.onnx", None);
    }
}
