//! The process-wide ONNX Runtime initialiser (57 §6 steps 1 and 2c), shared by every
//! `ort` user in the tree: voice STT (Parakeet TDT, Nemotron streaming, TEN-VAD),
//! meeting diarisation (pyannote, wespeaker) and the decision oracle (laya).
//!
//! ONNX Runtime is loaded at run time, in this order: the app bundle (`Contents/Frameworks/`
//! beside the `.app`'s `MacOS/theuth`, or the binary's own directory — `package-macos.sh`
//! ships it there, codesigned, and a hardened-runtime app must dlopen *that* copy, which is
//! why it comes first), then [`DIR`] under `<data_dir>/models/` (dev and headless builds;
//! the desktop downloader and `release-models.sh` fill it), then `$ORT_DYLIB_PATH`, then
//! Homebrew's (`brew install onnxruntime`). Linked statically it brings protobuf 3.21, which
//! cannot share a process with the protobuf-lite 3.14 inside `sentencepiece` (ptts): the TTS
//! load aborts. As a dylib its symbols stay private to it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ort::session::builder::SessionBuilder;

use crate::accelerator::{self, Accelerator};

/// The runtime's home under `<data_dir>/models/`: `libonnxruntime.{dylib,so}` /
/// `onnxruntime.dll`, unversioned, plus a `VERSION` text file. Nothing else under
/// `models/` is a runtime.
pub const DIR: &str = "onnxruntime";

/// The dylib name Microsoft's release ships as, without its version ([`platform::ort::ort_library_name`]).
pub const LIB: &str = platform::ort::ort_library_name();

/// The loaded runtime, once `ort::init_from(..).commit()` has run. A failed search is not
/// cached: the downloader may land the file while the daemon runs.
static ORT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `ort::init_from` runs once per process, on the first call that finds the runtime; every
/// later call returns the path it used. The error text names `libonnxruntime` so callers
/// can tell "no runtime" from "no model", and names where to put it.
pub fn ort_lib(data_dir: &Path) -> Result<PathBuf, String> {
    let mut slot = ORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(p) = slot.as_ref() {
        return Ok(p.clone());
    }
    let lib = find_ort_lib(data_dir)?;
    ort::init_from(&lib)
        .map_err(|e| format!("load {}: {e}", lib.display()))?
        .commit();
    tracing::debug!("onnxruntime: loaded {}", lib.display());
    *slot = Some(lib.clone());
    Ok(lib)
}

/// Whether [`ort_lib`] has already succeeded in this process.
pub fn ort_ready() -> bool {
    ORT.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

/// ORT's name for the CPU provider, what [`ep`] reports when nothing else is registered.
pub const CPU_EP: &str = "CPUExecutionProvider";
/// ORT's name for the DirectML provider.
pub const DML_EP: &str = "DmlExecutionProvider";

/// Set once DirectML registration has failed in this process: every later session goes
/// straight to the CPU and the WARN is not repeated.
static DML_FAILED: AtomicBool = AtomicBool::new(false);

/// Whether sessions built by [`configure`] ask for DirectML: the configured accelerator
/// says so, or it is `auto` on Windows and `DirectML.dll` sits beside the loaded runtime
/// (Microsoft's DirectML build of ORT ships the two together; the CPU build has no DML EP).
fn directml_wanted() -> bool {
    if !cfg!(windows) || DML_FAILED.load(Ordering::Relaxed) {
        return false;
    }
    match accelerator::get() {
        Accelerator::Directml => true,
        Accelerator::Auto => ORT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            .and_then(Path::parent)
            .is_some_and(|d| d.join("DirectML.dll").is_file()),
        Accelerator::Cpu | Accelerator::Cuda => false,
    }
}

/// The execution provider sessions built by [`configure`] run on, for status replies:
/// [`DML_EP`] or [`CPU_EP`]. Call after [`ort_lib`] (before it, `auto` cannot see the runtime).
pub fn ep() -> &'static str {
    if directml_wanted() { DML_EP } else { CPU_EP }
}

/// Register the execution provider [`accelerator::get`] resolves to on `builder`. CPU is
/// ORT's default and needs nothing; DirectML that fails to register (runtime without the
/// EP, no DX12 device, stale `DirectML.dll`) logs one WARN and leaves this and every later
/// session on the CPU. Never fails: the builder comes back untouched in the worst case.
pub fn configure(builder: SessionBuilder) -> SessionBuilder {
    if !directml_wanted() {
        return builder;
    }
    #[cfg(windows)]
    {
        match builder.with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()]) {
            Ok(b) => {
                tracing::debug!("onnxruntime: {DML_EP} registered");
                b
            }
            Err(e) => {
                DML_FAILED.store(true, Ordering::Relaxed);
                tracing::warn!("onnxruntime: DirectML unavailable ({e}); ONNX models run on the CPU");
                e.recover()
            }
        }
    }
    #[cfg(not(windows))]
    builder
}

/// `<data_dir>/models/onnxruntime/<LIB>`, the canonical on-disk location.
pub fn runtime_path(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(DIR).join(LIB)
}

fn find_ort_lib(data_dir: &Path) -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let bundled = exe_dir
        .iter()
        .flat_map(|d| [d.join("../Frameworks").join(LIB), d.join(LIB)]);
    let installed = runtime_path(data_dir);
    let env = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from);
    let brew = ["/opt/homebrew/lib", "/usr/local/lib"]
        .into_iter()
        .map(|d| Path::new(d).join(LIB));
    bundled
        .map(|p| ("bundle", p))
        .chain(std::iter::once(("models dir", installed.clone())))
        .chain(env.map(|p| ("$ORT_DYLIB_PATH", p)))
        .chain(brew.map(|p| ("Homebrew", p)))
        .find(|(_, p)| p.is_file())
        .map(|(source, p)| {
            tracing::debug!("onnxruntime: using {} ({source})", p.display());
            p
        })
        .ok_or_else(|| {
            format!(
                "no {LIB} bundled, at {} (nor $ORT_DYLIB_PATH, nor Homebrew): the desktop app downloads it there at start; by hand, copy lib/{LIB} from an ONNX Runtime release (github.com/microsoft/onnxruntime/releases, 1.17 or newer) and write its version to {}/VERSION",
                installed.display(),
                installed.parent().map_or(String::new(), |d| d.display().to_string())
            )
        })
}
