//! Locate and load a Pocket TTS checkpoint directory: `config.json` (ours, from
//! `data/pocket-tts-en-2026-10/`), Kyutai's `model.safetensors` and `tokenizer.json`, and
//! `voices/<name>.safetensors` (`emb` voice files made by `build-voices/from_state.py`).

use std::path::Path;

use anyhow::Context;
use ptts::checkpoint::Checkpoint;
use ptts::preprocess::Lang;
use ptts::synth::{DeviceKind, Synth};

/// Read `dir`'s config and find its weights, tokenizer and voices (`ptts::checkpoint`'s layout:
/// `model.safetensors`, `tokenizer.json`, `voices/` and `embeddings/`).
pub fn locate(dir: &Path) -> anyhow::Result<Checkpoint> {
    Checkpoint::open(dir).with_context(|| format!("Pocket TTS checkpoint {}", dir.display()))
}

/// Load the model with English text normalization and register every voice; a voice that
/// fails to load is logged and skipped so one bad embedding does not take the model down.
/// `DeviceKind::Auto` is Metal on macOS (the voice crate enables ptts's `metal` there; CPU if
/// Metal fails to start) and the CPU elsewhere. Temperature stays at ptts's default 0.3,
/// Kyutai's `default_temperature` for this checkpoint.
pub fn load(cp: &Checkpoint) -> anyhow::Result<Synth> {
    // CPU path (other platforms, Metal fallback): xn's worker pool is sized once, at the first
    // tensor op, to every core by default. Batch-1 decode is many tiny kernels, so past ~4
    // threads dispatch outgrows the work: measured on an M3 Max, 4 threads gave RTF 0.08 at
    // 7.5 s CPU against RTF 0.14 at 22 s CPU for 14.
    xn::set_num_threads(4);
    let mut synth = cp
        .builder(Lang::En)
        .device(DeviceKind::Auto)
        .build()
        .with_context(|| format!("loading {}", cp.weights.display()))?;
    cp.register_voices(&mut synth);
    Ok(synth)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Every voice the checkpoint ships says a sentence Parakeet transcribes word for word: a wrong
    /// config value, a voice file built for another checkpoint or a missing BOS row loads without
    /// error and then babbles. Skips without the checkpoint (`$THEUTH_TTS_DIR`, else
    /// `~/.theuth/models/pocket-tts-en-2026-10`), the Parakeet checkpoint or its dylib.
    #[test]
    fn every_voice_speaks_intelligibly() {
        let Some(data_dir) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth"))
        else {
            return;
        };
        let dir = std::env::var_os("THEUTH_TTS_DIR").map_or_else(
            || data_dir.join("models").join(crate::TTS_MODEL_SUBDIR),
            PathBuf::from,
        );
        let stt_dir = data_dir.join("models").join(crate::TDT_MODEL_SUBDIR);
        if !dir.join("config.json").is_file() || !stt_dir.is_dir() {
            eprintln!(
                "skipped: no checkpoint at {} or {}",
                dir.display(),
                stt_dir.display()
            );
            return;
        }
        let engine = match crate::stt::Engine::load(&stt_dir, &data_dir) {
            Ok(e) => e,
            Err(e) if format!("{e:#}").contains("libonnxruntime") => {
                eprintln!("skipped: {e:#}");
                return;
            }
            Err(e) => panic!("{e:#}"),
        };
        let voices: Vec<String> = locate(&dir)
            .unwrap()
            .voices
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(voices, ["alba", "peter_yearsley"]);
        for voice in voices {
            let pcm =
                crate::stt::speech_sample(&dir, &voice, "The deploy is blocked on the image tags.")
                    .unwrap();
            let text = engine.transcribe(&pcm, false).unwrap().text;
            let said: String = text
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect();
            assert_eq!(
                said, "the deploy is blocked on the image tags",
                "{voice}: {text:?}"
            );
        }
    }
}
