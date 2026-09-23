//! `models.accelerator` (58 step 12): one process-wide knob for where local models run.
//! `theuth` sets it from the config once at start ([`set`]); the ONNX users ask
//! [`crate::onnx::configure`] for their session options and the candle engine (`decision`)
//! asks [`get`]. It is a static rather than a parameter because the session-opening
//! functions (`voice::stt::Engine::load`, `meeting::post::diarize::session`, …) are reached
//! through actors that do not carry config.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

/// `models.accelerator`. `Auto` = Metal on an Apple-silicon Mac (candle), DirectML on Windows
/// when `DirectML.dll` sits beside `onnxruntime.dll` (ORT), CUDA when the binary was built
/// with `decision/cuda` and a device answers (candle), else CPU.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Accelerator {
    /// Resolved per engine at load time, see above.
    #[default]
    Auto = 0,
    /// No GPU anywhere: candle CPU, ORT CPU EP.
    Cpu = 1,
    /// ONNX Runtime's DirectML execution provider (any DX12 GPU, Windows only).
    Directml = 2,
    /// candle on CUDA (needs the `decision/cuda` build); ONNX sessions stay on the CPU —
    /// the CUDA EP also needs cuDNN, which we do not ship.
    Cuda = 3,
}

impl Accelerator {
    /// The config spelling.
    pub fn name(self) -> &'static str {
        match self {
            Accelerator::Auto => "auto",
            Accelerator::Cpu => "cpu",
            Accelerator::Directml => "directml",
            Accelerator::Cuda => "cuda",
        }
    }
}

impl std::str::FromStr for Accelerator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Accelerator::Auto),
            "cpu" => Ok(Accelerator::Cpu),
            "directml" => Ok(Accelerator::Directml),
            "cuda" => Ok(Accelerator::Cuda),
            _ => Err(format!("accelerator: {s:?} is not auto | cpu | directml | cuda")),
        }
    }
}

/// `models:` config section. Every field has a default; the section may be absent.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    /// `auto` | `cpu` | `directml` | `cuda`.
    pub accelerator: Accelerator,
}

static CURRENT: AtomicU8 = AtomicU8::new(Accelerator::Auto as u8);

/// Record the configured accelerator for the process (`theuth` main, before any model loads).
pub fn set(a: Accelerator) {
    CURRENT.store(a as u8, Ordering::Relaxed);
}

/// The configured accelerator (`Auto` until [`set`] runs).
pub fn get() -> Accelerator {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Accelerator::Cpu,
        2 => Accelerator::Directml,
        3 => Accelerator::Cuda,
        _ => Accelerator::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_spelling() {
        for a in [Accelerator::Auto, Accelerator::Cpu, Accelerator::Directml, Accelerator::Cuda] {
            assert_eq!(a.name().parse::<Accelerator>(), Ok(a));
            set(a);
            assert_eq!(get(), a);
        }
        assert!("metal".parse::<Accelerator>().is_err());
        set(Accelerator::Auto);
    }
}
