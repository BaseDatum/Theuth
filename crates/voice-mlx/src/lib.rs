//! Parakeet TDT 0.6B v3 on MLX, in-process: a Rust port of the inference path of the Python package
//! `parakeet-mlx` 0.7.0 (`from_pretrained` + `transcribe` with greedy TDT decoding). See README.md for the
//! route decision and the packaging facts (metallib, linked frameworks).
//!
//! Real only on `cfg(all(target_os = "macos", target_arch = "aarch64"))`; elsewhere every entry point is a stub
//! and no MLX dependency is compiled.

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
use std::path::Path;

/// One decoded word: consecutive tokens from a `▁`-prefixed token up to the next one; times from the TDT
/// frame index and predicted duration.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// The word's text, without the leading `▁` marker.
    pub text: String,
    /// Start time in seconds from the beginning of the audio.
    pub start_s: f32,
    /// End time in seconds from the beginning of the audio.
    pub end_s: f32,
}

/// The result of one transcription: the full text and its words with timings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    /// The whole decoded text.
    pub text: String,
    /// The decoded words in order, with their times.
    pub words: Vec<Word>,
}

/// The files of `mlx-community/parakeet-tdt-0.6b-v3` that [`ParakeetMlx::load`] reads. The vocabulary is the
/// `joint.vocabulary` list inside config.json, exactly as parakeet-mlx uses it; the sentencepiece files are not
/// needed.
pub const FILES: &[&str] = &["config.json", "model.safetensors"];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mel;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod metal;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod model;
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    allow(dead_code)
)]
mod text;

/// True only where the MLX engine is compiled in (macOS on Apple silicon).
pub fn available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// Why the engine cannot run in this process, if it cannot: not compiled in, or no `mlx.metallib` where
/// MLX would look (see `src/metal.rs`). Touches no MLX state; [`ParakeetMlx::load`] checks it again.
pub fn unavailable_reason() -> Option<String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        metal::find().err().map(|e| e.to_string())
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Some("Parakeet on MLX needs macOS on Apple silicon".into())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use model::ParakeetMlx;

/// Stub for targets without MLX: loading always fails.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub struct ParakeetMlx {
    _private: (),
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
impl ParakeetMlx {
    /// Load `config.json` and `model.safetensors` from a model directory (see [`FILES`]); always
    /// fails on targets without MLX.
    pub fn load(_dir: &Path) -> anyhow::Result<Self> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }

    /// Warm up the engine before the first transcription; always fails on targets without MLX.
    pub fn warm(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }

    /// Transcribe 16 kHz mono float PCM; always fails on targets without MLX.
    pub fn transcribe(&mut self, _pcm16k: &[f32]) -> anyhow::Result<Transcript> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }
}
