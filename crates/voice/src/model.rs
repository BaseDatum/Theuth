//! Locate and load a Pocket TTS checkpoint directory.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ptts::synth::Synth;
use ptts::tts_model::TTSConfig;

/// Weight file names tried, in order.
const WEIGHTS: &[&str] = &[
    "tts_b6369a24.safetensors",
    "model.safetensors",
    "model.q8.gguf",
];
/// Tokenizer file names tried, in order (`ptts::tok::Tok` picks the reader by extension).
const TOKENIZERS: &[&str] = &["tokenizer.model", "tokenizer.json"];
/// Directory holding `<voice>.safetensors` speaker embeddings (`[T, dim]`, what `ptts`'s
/// `create_voice` writes). The Hub's `embeddings/` and `embeddings_v3/` are Kyutai KV-cache
/// *states* (rank-5 tensors) that `ptts` loads without complaint and then fails on at the first
/// `stream_with` — so they are deliberately not scanned.
const VOICE_DIRS: &[&str] = &["voices"];

/// A checkpoint whose files have been located.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    /// Safetensors or GGUF weights.
    pub weights: PathBuf,
    /// SentencePiece `tokenizer.model` or HF `tokenizer.json`.
    pub tokenizer: PathBuf,
    /// Voice name (file stem) → embedding file, sorted by name.
    pub voices: Vec<(String, PathBuf)>,
}

/// Find weights, tokenizer and voices under `dir`, falling back to
/// `dir/languages/english` when the root holds no weights (the Hub repo's
/// newer layout).
pub fn locate(dir: &Path) -> anyhow::Result<Checkpoint> {
    let root = if find(dir, WEIGHTS).is_some() {
        dir.to_path_buf()
    } else {
        dir.join("languages/english")
    };
    let weights = find(&root, WEIGHTS).with_context(|| {
        format!(
            "no weights in {} (expected one of {})",
            dir.display(),
            WEIGHTS.join(", ")
        )
    })?;
    let tokenizer = find(&root, TOKENIZERS).with_context(|| {
        format!(
            "no tokenizer in {} (expected one of {})",
            root.display(),
            TOKENIZERS.join(", ")
        )
    })?;
    let mut voices = Vec::new();
    for sub in VOICE_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(sub)) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("safetensors") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                voices.push((name.to_string(), path));
            }
        }
    }
    voices.sort();
    voices.dedup_by(|a, b| a.0 == b.0);
    Ok(Checkpoint {
        weights,
        tokenizer,
        voices,
    })
}

fn find(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Load the model on the CPU and register every voice; a voice that fails to
/// load is logged and skipped so one bad embedding does not take the model down.
pub fn load(cp: &Checkpoint) -> anyhow::Result<Synth> {
    // xn's worker pool is sized once, at the first tensor op, to every core by default. Batch-1
    // decode is many tiny kernels, so past ~4 threads dispatch outgrows the work: measured on an
    // M3 Max, 4 threads gave RTF 0.08 at 7.5 s CPU against RTF 0.14 at 22 s CPU for 14.
    xn::set_num_threads(4);
    let mut synth = Synth::builder(TTSConfig::v202601(0.7), &cp.weights)
        .tokenizer_file(&cp.tokenizer)
        .build()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("loading {}", cp.weights.display()))?;
    for (name, path) in &cp.voices {
        if let Err(e) = synth.add_voice_file(name, path) {
            tracing::warn!(voice = %name, path = %path.display(), error = %e, "skipping voice embedding");
        }
    }
    Ok(synth)
}
