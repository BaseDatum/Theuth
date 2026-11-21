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
    pub text: String,
    pub start_s: f32,
    pub end_s: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    pub words: Vec<Word>,
}

/// The files of `mlx-community/parakeet-tdt-0.6b-v3` that [`ParakeetMlx::load`] reads. The vocabulary is the
/// `joint.vocabulary` list inside config.json, exactly as parakeet-mlx uses it; the sentencepiece files are not
/// needed.
pub const FILES: &[&str] = &["config.json", "model.safetensors"];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mel;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod model;
mod text;

/// True only where the MLX engine is compiled in (macOS on Apple silicon).
pub fn available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
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
    pub fn load(_dir: &Path) -> anyhow::Result<Self> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }
    pub fn warm(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }
    pub fn transcribe(&mut self, _pcm16k: &[f32]) -> anyhow::Result<Transcript> {
        anyhow::bail!("Parakeet on MLX needs macOS on Apple silicon")
    }
}
