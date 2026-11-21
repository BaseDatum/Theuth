//! Records where mlx-sys put `mlx.metallib` on this build machine (`VOICE_MLX_BUILD_METALLIB`), the last
//! entry of the run-time lookup in `src/metal.rs`. mlx-sys does not export the path it bakes into MLX, so this
//! repeats its rule: `$MLX_RS_METAL_PATH`, else the newest non-JIT `~/.mlx/lib/<key>/` (mlx-sys's build script
//! has run by now: dependencies' build scripts run before this one).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MLX_RS_METAL_PATH");
    println!("cargo:rerun-if-changed=build.rs");
    let path = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        find().map(|p| p.display().to_string()).unwrap_or_default()
    } else {
        String::new()
    };
    println!("cargo:rustc-env=VOICE_MLX_BUILD_METALLIB={path}");
}

fn find() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MLX_RS_METAL_PATH") {
        return Some(PathBuf::from(dir).join("mlx.metallib"));
    }
    let lib = PathBuf::from(std::env::var_os("HOME")?)
        .join(".mlx")
        .join("lib");
    println!("cargo:rerun-if-changed={}", lib.display());
    std::fs::read_dir(&lib)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().ends_with("-jit"))
        .map(|e| e.path().join("mlx.metallib"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
}
