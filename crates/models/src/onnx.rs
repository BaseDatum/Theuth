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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use ort::session::builder::SessionBuilder;

use crate::accelerator::{self, Accelerator};

/// The runtime's home under `<data_dir>/models/`: `libonnxruntime.{dylib,so}` /
/// `onnxruntime.dll`, unversioned, plus a `VERSION` text file. Nothing else under
/// `models/` is a runtime.
pub const DIR: &str = "onnxruntime";

/// The dylib name Microsoft's release ships as, without its version ([`platform::ort::ort_library_name`]).
pub const LIB: &str = platform::ort::ort_library_name();

/// Plugin execution providers' home under `<data_dir>/models/onnxruntime/` (58 step 13).
pub const PLUGIN_DIR: &str = "plugins";

/// The WebGPU plugin EP library as the `Microsoft.ML.OnnxRuntime.EP.WebGpu` package ships it
/// (`runtimes/<rid>/native/`). On Windows `dxcompiler.dll` and `dxil.dll` from the same folder
/// go beside it.
pub const WEBGPU_LIB: &str = if cfg!(windows) {
    "onnxruntime_providers_webgpu.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime_providers_webgpu.dylib"
} else {
    "libonnxruntime_providers_webgpu.so"
};

/// The oldest runtime the WebGPU plugin (0.4.0) accepts; older ones fail at registration.
pub const WEBGPU_MIN_ORT: [u32; 3] = [1, 24, 4];

/// The loaded runtime, once `ort::init_from(..).commit()` has run. A failed search is not
/// cached: the downloader may land the file while the daemon runs.
static ORT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// [`webgpu_plugin_path`] of the `data_dir` [`ort_lib`] loaded from, when the file exists.
static WEBGPU_PLUGIN: Mutex<Option<PathBuf>> = Mutex::new(None);

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
    let plugin = webgpu_plugin_path(data_dir);
    *WEBGPU_PLUGIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = plugin.is_file().then_some(plugin);
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
/// ORT's name for the WebGPU provider (built in or plugin).
pub const WEBGPU_EP: &str = "WebGpuExecutionProvider";

/// Set once DirectML registration has failed in this process: every later session goes
/// straight to the CPU and the WARN is not repeated.
static DML_FAILED: AtomicBool = AtomicBool::new(false);

/// WebGPU plugin registration: 0 = not tried, 1 = registered and a device answers, 2 = failed
/// (one WARN; every session on the CPU from then on).
static WEBGPU_STATE: AtomicU8 = AtomicU8::new(0);

/// Whether sessions built by [`configure`] ask for DirectML: only when the configured
/// accelerator says so. `auto` never picks it (58 step 13): Microsoft's DirectML build of ORT
/// ended at 1.24.4, below the runtime `ort` loads, so the EP is dead unless someone points
/// `$ORT_DYLIB_PATH` at an old DirectML build on purpose.
fn directml_wanted() -> bool {
    cfg!(windows) && !DML_FAILED.load(Ordering::Relaxed) && accelerator::get() == Accelerator::Directml
}

/// Whether the WebGPU plugin is registered and has a device, registering it on the first
/// call that wants it: the configured accelerator is `webgpu`, or `auto` and the plugin
/// library sits under `<data_dir>/models/onnxruntime/plugins/`. Needs [`ort_lib`] first
/// (before it there is no environment to register with); a failure is final for the process.
fn webgpu_ready() -> bool {
    match WEBGPU_STATE.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let plugin = WEBGPU_PLUGIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if !webgpu_wanted(accelerator::get(), plugin.is_some()) || !ort_ready() {
        return false;
    }
    match register_webgpu(plugin.as_deref()) {
        Ok(()) => {
            WEBGPU_STATE.store(1, Ordering::Relaxed);
            tracing::debug!("onnxruntime: {WEBGPU_EP} registered");
            true
        }
        Err(e) => {
            WEBGPU_STATE.store(2, Ordering::Relaxed);
            tracing::warn!("onnxruntime: WebGPU unavailable ({e}); ONNX models run on the CPU");
            false
        }
    }
}

/// The selection rule: `webgpu` always asks (and warns when the plugin is absent); `auto` asks
/// only when the plugin file is present; every other setting leaves ONNX sessions alone.
pub fn webgpu_wanted(accelerator: Accelerator, plugin_present: bool) -> bool {
    match accelerator {
        Accelerator::Webgpu => true,
        Accelerator::Auto => plugin_present,
        Accelerator::Cpu | Accelerator::Directml | Accelerator::Cuda => false,
    }
}

/// Whether the environment already offers a WebGPU device.
fn has_webgpu_device(env: &ort::environment::Environment) -> bool {
    env.devices().any(|d| d.ep().is_ok_and(|n| n == WEBGPU_EP))
}

/// Make a WebGPU device available in the process-wide environment. A runtime with the EP built
/// in (Microsoft's osx-arm64 1.27.0 has it; the win-x64 and linux-x64 CPU builds do not) already
/// has one, and loading the plugin beside it would add a second `WebGpuExecutionProvider` from
/// another factory that `with_devices` refuses to mix — so the plugin is only registered
/// (`RegisterExecutionProviderLibrary` + `GetEpDevices` through `ort`'s `api-22`) when there is
/// none, and must then produce a device (no adapter — no GPU, no Vulkan loader on Linux, an old
/// driver — registers fine but yields none).
fn register_webgpu(plugin: Option<&Path>) -> Result<(), String> {
    let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
    if has_webgpu_device(&env) {
        tracing::debug!("onnxruntime: {WEBGPU_EP} is built into the runtime; plugin not loaded");
        return Ok(());
    }
    let plugin = plugin.ok_or_else(|| {
        format!("the runtime has no {WEBGPU_EP} and there is no {WEBGPU_LIB} under <data_dir>/models/{DIR}/{PLUGIN_DIR}/ (Microsoft.ML.OnnxRuntime.EP.WebGpu, runtimes/<rid>/native/)")
    })?;
    if let Some(v) = runtime_version() {
        if !webgpu_accepts(&v) {
            return Err(format!(
                "ONNX Runtime {v} is older than {}.{}.{}, the plugin's minimum",
                WEBGPU_MIN_ORT[0], WEBGPU_MIN_ORT[1], WEBGPU_MIN_ORT[2]
            ));
        }
    }
    env.register_ep_library("webgpu_ep", plugin)
        .map_err(|e| format!("register {}: {e}", plugin.display()))?;
    if has_webgpu_device(&env) {
        Ok(())
    } else {
        Err(format!("{} registered but no WebGPU device was found (no GPU adapter, no Vulkan loader, or an old driver)", plugin.display()))
    }
}

/// The loaded runtime's version from the `VERSION` file beside it (the models-dir layout);
/// `None` for a bundled, `$ORT_DYLIB_PATH` or Homebrew runtime, whose version `ort` already
/// checked against its API level at load.
fn runtime_version() -> Option<String> {
    let lib = ORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    std::fs::read_to_string(lib.parent()?.join("VERSION"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whether an ORT `version` ("1.27.0") is at least [`WEBGPU_MIN_ORT`]. Unparseable → `false`.
pub fn webgpu_accepts(version: &str) -> bool {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u32>().ok());
    let v = [parts.next().flatten(), parts.next().flatten(), parts.next().flatten()];
    let [Some(a), Some(b), Some(c)] = v else { return false };
    [a, b, c] >= WEBGPU_MIN_ORT
}

/// The execution provider sessions built by [`configure`] run on, for status replies:
/// [`WEBGPU_EP`], [`DML_EP`] or [`CPU_EP`]. Call after [`ort_lib`] (before it, `auto` cannot
/// see the plugin and nothing can be registered).
pub fn ep() -> &'static str {
    if webgpu_ready() {
        WEBGPU_EP
    } else if directml_wanted() {
        DML_EP
    } else {
        CPU_EP
    }
}

/// Every execution-provider device the environment knows, one line each
/// (`<EP> — <type> <hardware vendor> (<EP vendor>)`), for `t --bench-models`. Empty before
/// [`ort_lib`]. WebGPU shows up only after registration ([`ep`] or [`configure`] triggers it).
pub fn devices() -> Vec<String> {
    let Ok(env) = ort::environment::Environment::current() else { return Vec::new() };
    env.devices()
        .map(|d| {
            let hw = d.hardware_device();
            format!(
                "{} — {:?} {} ({})",
                d.ep().unwrap_or("?"),
                hw.ty(),
                hw.vendor().unwrap_or_default(),
                d.ep_vendor().unwrap_or("?")
            )
        })
        .collect()
}

/// Register the execution provider [`accelerator::get`] resolves to on `builder`. CPU is
/// ORT's default and needs nothing; WebGPU or DirectML that fails to register logs one WARN
/// and leaves this and every later session on the CPU. Never fails: the builder comes back
/// untouched in the worst case.
pub fn configure(builder: SessionBuilder) -> SessionBuilder {
    if webgpu_ready() {
        return match ort::environment::Environment::current() {
            // One device: several adapters would be several factories' devices, which ORT refuses to mix.
            Ok(env) => match builder.with_devices(env.devices().filter(|d| d.ep().is_ok_and(|n| n == WEBGPU_EP)).take(1), None) {
                Ok(b) => b,
                Err(e) => {
                    WEBGPU_STATE.store(2, Ordering::Relaxed);
                    tracing::warn!("onnxruntime: appending {WEBGPU_EP} failed ({e}); ONNX models run on the CPU");
                    e.recover()
                }
            },
            Err(e) => {
                WEBGPU_STATE.store(2, Ordering::Relaxed);
                tracing::warn!("onnxruntime: no environment for {WEBGPU_EP} ({e}); ONNX models run on the CPU");
                builder
            }
        };
    }
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

/// `<data_dir>/models/onnxruntime/plugins/<WEBGPU_LIB>`, where `auto` looks for the plugin.
pub fn webgpu_plugin_path(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(DIR).join(PLUGIN_DIR).join(WEBGPU_LIB)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gate_is_the_plugins_minimum() {
        assert!(webgpu_accepts("1.27.0"));
        assert!(webgpu_accepts("1.24.4\n"));
        assert!(webgpu_accepts("2.0.0"));
        assert!(!webgpu_accepts("1.24.3"));
        assert!(!webgpu_accepts("1.22.0"));
        assert!(!webgpu_accepts("1.24"));
        assert!(!webgpu_accepts("onnxruntime"));
    }

    #[test]
    fn auto_needs_the_plugin_file_and_webgpu_does_not() {
        assert!(webgpu_wanted(Accelerator::Webgpu, false));
        assert!(webgpu_wanted(Accelerator::Auto, true));
        assert!(!webgpu_wanted(Accelerator::Auto, false));
        for a in [Accelerator::Cpu, Accelerator::Directml, Accelerator::Cuda] {
            assert!(!webgpu_wanted(a, true), "{a:?}");
        }
    }
}
