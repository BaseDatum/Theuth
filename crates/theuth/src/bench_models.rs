//! `t --bench-models [--accelerator X]` (58 step 12): what the local models actually run on.
//! Loads ONNX Runtime and prints its path and the execution provider sessions get, runs the
//! STT encoder once on 5 s of silence, opens the decision checkpoint and runs one question —
//! every step with its wall time — and says what is missing. Exit 0 when everything ran,
//! 2 when a runtime or a model is absent (the lines above name what and where).
//! `$ORT_DYLIB_PATH` is honoured like everywhere else (`models::onnx::ort_lib`).

use std::path::Path;
use std::time::Instant;

use decision::candle::Loaded;
use decision::{Candidate, DecisionConfig, Question};
use models::accelerator::Accelerator;

/// Exit code for "a model or the runtime is missing".
pub const MISSING: i32 = 2;

/// The utterance the third STT run must bring back (about 5 s spoken).
const SPEECH: &str =
    "The quick brown fox jumps over the lazy dog while the meeting notes are being transcribed.";

pub fn run(data_dir: &Path, decision_cfg: &DecisionConfig, accelerator: Accelerator) -> i32 {
    models::accelerator::set(accelerator);
    println!(
        "platform: {}  accelerator: {}",
        platform::target::target_triple(),
        accelerator.name()
    );
    let mut missing = false;

    // ONNX Runtime and the STT encoder.
    match models::onnx::ort_lib(data_dir) {
        Err(e) => {
            missing = true;
            println!("onnxruntime: missing — {e}");
        }
        Ok(lib) => {
            println!("onnxruntime: {}", lib.display());
            println!("onnxruntime ep: {}", models::onnx::ep());
            let plugin = models::onnx::webgpu_plugin_path(data_dir);
            println!(
                "onnxruntime webgpu plugin: {} ({})",
                plugin.display(),
                if plugin.is_file() {
                    "present"
                } else {
                    "absent"
                }
            );
            for d in models::onnx::devices() {
                println!("onnxruntime device: {d}");
            }
            if let Some(d) = models::onnx::webgpu_device() {
                println!("onnxruntime webgpu device: {d}");
            }
            let dir = data_dir.join("models").join(voice::TDT_MODEL_SUBDIR);
            if !dir.is_dir() {
                missing = true;
                println!("stt: missing — no checkpoint at {}", dir.display());
            } else {
                let t = Instant::now();
                match voice::Engine::load(&dir, data_dir) {
                    Err(e) => println!("stt: failed to load {}: {e:#}", dir.display()),
                    Ok(engine) => {
                        println!(
                            "stt load: {} ms ({})",
                            t.elapsed().as_millis(),
                            dir.display()
                        );
                        let silence = vec![0.0f32; 16_000 * 5];
                        let mut first = 0u128;
                        for pass in ["first", "second"] {
                            let t = Instant::now();
                            match engine.transcribe(&silence, false) {
                                Ok(p) => {
                                    let ms = t.elapsed().as_millis();
                                    if pass == "first" {
                                        first = ms;
                                    } else {
                                        println!(
                                            "stt first-run overhead (shader/graph compile): {} ms",
                                            first.saturating_sub(ms)
                                        );
                                    }
                                    println!(
                                        "stt {pass} run, 5 s silence: {ms} ms (text {:?})",
                                        p.text
                                    );
                                }
                                Err(e) => println!("stt {pass} run: failed: {e:#}"),
                            }
                        }
                        // A real utterance: silence transcribing to "" proves nothing about the EP.
                        let tts = data_dir.join("models/pocket-tts");
                        let t = Instant::now();
                        match voice::speech_sample(&tts, "peter_yearsley", SPEECH) {
                            Err(e) => println!(
                                "stt speech clip: none — {e:#} (pocket-tts at {})",
                                tts.display()
                            ),
                            Ok(pcm) => {
                                println!(
                                    "stt speech clip: {:.1} s synthesized in {} ms (expect {SPEECH:?})",
                                    pcm.len() as f32 / 16_000.0,
                                    t.elapsed().as_millis()
                                );
                                let t = Instant::now();
                                match engine.transcribe(&pcm, false) {
                                    Ok(p) => println!(
                                        "stt third run, speech: {} ms (text {:?})",
                                        t.elapsed().as_millis(),
                                        p.text
                                    ),
                                    Err(e) => println!("stt third run: failed: {e:#}"),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // The decision checkpoint on candle.
    let dir = decision_cfg.model_dir_in(data_dir);
    if !dir.is_dir() {
        missing = true;
        println!("decision: missing — no checkpoint at {}", dir.display());
    } else {
        let gpu = decision_cfg.device.gpu();
        println!("decision asks for: {gpu:?} {:?}", gpu.dtype());
        let t = Instant::now();
        match Loaded::open(&dir, gpu, gpu.dtype()) {
            Err(e) => println!("decision: failed to load {}: {e:#}", dir.display()),
            Ok(m) => {
                println!(
                    "decision load: {} ms on {:?} {:?} ({})",
                    t.elapsed().as_millis(),
                    m.device.location(),
                    m.dtype,
                    dir.display()
                );
                let q = Question::Choice {
                    instructions: "which cached brief is the subject being discussed".into(),
                    candidates: (0..3)
                        .map(|i| Candidate {
                            id: format!("BRIEF-{i}"),
                            text: format!("brief {i}: an incident in service {i}"),
                        })
                        .collect(),
                    state: "The database in service 1 has been paging since 03:00.".into(),
                };
                match m.build(&q) {
                    Err(e) => println!("decision build: {e}"),
                    Ok(seq) => {
                        for pass in ["first", "second"] {
                            let t = Instant::now();
                            match m.run(std::slice::from_ref(&seq)) {
                                Ok(out) => println!(
                                    "decision {pass} ask, {} tokens: {} ms (probs {:?})",
                                    seq.input_ids.len(),
                                    t.elapsed().as_millis(),
                                    out.first().map(|r| r.probs.clone()).unwrap_or_default()
                                ),
                                Err(e) => println!("decision {pass} ask: failed: {e:#}"),
                            }
                        }
                    }
                }
            }
        }
    }

    if missing { MISSING } else { 0 }
}
