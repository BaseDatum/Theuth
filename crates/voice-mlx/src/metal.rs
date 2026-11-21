//! Where `mlx.metallib` comes from at run time (README "Packaging facts"). MLX throws from its Metal device
//! constructor when no metallib loads, which is not an error this crate can catch reliably, so [`prepare`]
//! finds the file first and pins MLX to it with `set_metallib_path`; nothing touches the GPU without it.
//!
//! Lookup, first hit wins: beside the executable, `Resources/` beside it, `../Resources/` (the executable in
//! `theuth.app/Contents/MacOS/`, the file in `Contents/Resources/`), then the build machine's path
//! (`VOICE_MLX_BUILD_METALLIB`, from build.rs; dev builds and tests).

use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};

const FILE: &str = "mlx.metallib";

/// The candidates in lookup order.
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        v.push(dir.join(FILE));
        v.push(dir.join("Resources").join(FILE));
        v.push(dir.join("..").join("Resources").join(FILE));
    }
    let built = env!("VOICE_MLX_BUILD_METALLIB");
    if !built.is_empty() {
        v.push(PathBuf::from(built));
    }
    v
}

/// The metallib MLX will use, or why there is none. No MLX call.
pub(crate) fn find() -> Result<PathBuf> {
    let c = candidates();
    c.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
        anyhow!(
            "{FILE} not found (looked in {})",
            c.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// [`find`], then point MLX at it; once per process (the result is cached, MLX's device is a singleton).
pub(crate) fn prepare() -> Result<PathBuf> {
    static DONE: LazyLock<std::result::Result<PathBuf, String>> = LazyLock::new(|| {
        let p = find().map_err(|e| e.to_string())?;
        let s = p
            .to_str()
            .ok_or_else(|| format!("{} is not UTF-8", p.display()))?;
        mlx_rs::metal::set_metallib_path(s).map_err(|e| format!("set_metallib_path: {e}"))?;
        Ok(p)
    });
    DONE.clone().map_err(|e| anyhow!(e))
}
