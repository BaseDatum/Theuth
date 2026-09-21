//! The process-wide ONNX Runtime initialiser (57 §6 step 1), shared by every
//! `ort` user in the tree: voice STT (Parakeet TDT, Nemotron streaming, TEN-VAD),
//! meeting diarisation (pyannote, wespeaker) and the decision oracle (laya).
//!
//! ONNX Runtime is loaded at run time, from the first of: the bundle (`Contents/Frameworks/`
//! beside the `.app`'s `MacOS/theuth`, or the binary's own directory — `package-macos.sh` ships
//! it there, signed), any `libonnxruntime*` in the hint directories (a model checkpoint's),
//! `$ORT_DYLIB_PATH`, Homebrew's (`brew install onnxruntime`). Linked statically it brings
//! protobuf 3.21, which cannot share a process with the protobuf-lite 3.14 inside
//! `sentencepiece` (ptts): the TTS load aborts. As a dylib its symbols stay private to it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ORT: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// `ort::init_from` runs once per process, on the first call; every later call returns the
/// first result. `hint_dirs` are searched for `libonnxruntime*` between the bundle and
/// `$ORT_DYLIB_PATH`, so the first caller's checkpoint directory decides; the other
/// locations are dir-independent. The error text names `libonnxruntime` so callers can
/// tell "no runtime" from "no model".
pub fn ort_lib(hint_dirs: &[&Path]) -> Result<PathBuf, String> {
    ORT.get_or_init(|| {
        let lib = find_ort_lib(hint_dirs)?;
        ort::init_from(&lib)
            .map_err(|e| format!("load {}: {e}", lib.display()))?
            .commit();
        Ok(lib)
    })
    .clone()
}

/// Whether [`ort_lib`] has already succeeded in this process.
pub fn ort_ready() -> bool {
    ORT.get().is_some_and(Result::is_ok)
}

fn find_ort_lib(hint_dirs: &[&Path]) -> Result<PathBuf, String> {
    let ext = if cfg!(target_os = "macos") {
        ".dylib"
    } else if cfg!(target_os = "windows") {
        ".dll"
    } else {
        ".so"
    };
    let stem = if cfg!(target_os = "windows") {
        "onnxruntime"
    } else {
        "libonnxruntime"
    };
    let name = format!("{stem}{ext}");
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let bundled = exe_dir
        .iter()
        .flat_map(|d| [d.join("../Frameworks").join(&name), d.join(&name)]);
    let in_dirs = hint_dirs.iter().flat_map(|dir| {
        std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(stem) && n.contains(ext))
            })
    });
    let env = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from);
    let brew = ["/opt/homebrew/lib", "/usr/local/lib"]
        .into_iter()
        .map(|d| Path::new(d).join(&name));
    bundled
        .chain(in_dirs)
        .chain(env)
        .chain(brew)
        .find(|p| p.exists())
        .ok_or_else(|| {
            let dirs: Vec<String> = hint_dirs.iter().map(|d| d.display().to_string()).collect();
            format!(
                "no {name} bundled, in {} (nor $ORT_DYLIB_PATH, nor Homebrew): copy lib/libonnxruntime* from an ONNX Runtime release (github.com/microsoft/onnxruntime/releases, 1.17 or newer) there",
                dirs.join(", ")
            )
        })
}
