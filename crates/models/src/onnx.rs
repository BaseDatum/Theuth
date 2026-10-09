//! The process-wide ONNX Runtime initialiser (57 §6 steps 1 and 2c), shared by every
//! `ort` user in the tree: voice STT (Parakeet TDT, Nemotron streaming, TEN-VAD),
//! meeting diarisation (Nemotron 3 Diarization, wespeaker) and the decision oracle (laya).
//!
//! ONNX Runtime is loaded at run time, in this order: the app bundle (`Contents/Frameworks/`
//! beside the `.app`'s `MacOS/theuth`, or the binary's own directory — `package-macos.sh`
//! ships it there, codesigned, and a hardened-runtime app must dlopen *that* copy, which is
//! why it comes first), then [`DIR`] under `<data_dir>/models/` (dev and headless builds;
//! the desktop downloader and `release-models.sh` fill it), then `$ORT_DYLIB_PATH`, then
//! Homebrew's (`brew install onnxruntime`). Linked statically it brings protobuf 3.21, which
//! cannot share a process with the protobuf-lite 3.14 inside `sentencepiece` (ptts): the TTS
//! load aborts. As a dylib its symbols stay private to it.

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use ort::AsPointer;
use ort::memory::DeviceType;
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

/// The runtime `ort::init_from(..).commit()` loaded, or why the one attempt to load it failed.
/// A failed *search* is not cached (the downloader may land the file while the daemon runs);
/// a failed *load* is final: `ort`'s loader cell completes even when its dlopen fails, so a
/// second `init_from` reports success over a null library handle and the first session then
/// panics in `dlsym(0x0, OrtGetApiBase)` and poisons `ort`'s environment mutex for good.
static ORT: Mutex<Option<Result<PathBuf, String>>> = Mutex::new(None);

/// [`webgpu_plugin_path`] of the `data_dir` [`ort_lib`] loaded from, when the file exists.
static WEBGPU_PLUGIN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `models.webgpu_adapter`: a substring of the adapter's vendor or description that wins the
/// device choice ([`pick_adapter`]); `None` = discrete over integrated over first.
static WEBGPU_ADAPTER: Mutex<Option<String>> = Mutex::new(None);

/// Serialises every `Session::run` on, and every drop of, a session [`configure`] put on
/// WebGPU. The plugin EP we ship (0.4.0) shares ONE `WebGpuContext` with ONE unlocked command
/// encoder between all sessions: a run ending on one thread `Finish()`es the encoder another
/// thread is still recording into ("WebGPU validation failed. [CommandEncoder] is already
/// finished"), then a native fault in `onnxruntime_providers_webgpu.dll` takes the engine
/// down. Upstream fixed it in microsoft/onnxruntime #32587 ("Support concurrent use with
/// explicit Session streams") and #32874 ("Each Session owns its command recording"), neither
/// in a released plugin. Remove when the WebGPU plugin ships #32587; it may also need
/// explicit per-session streams (`OrtEp CreateSyncStreamForDevice`). CPU sessions never take it.
static WEBGPU_RUN: Mutex<()> = Mutex::new(());

/// Time spent waiting for [`WEBGPU_RUN`]: (acquisitions, total ns, max ns). The queueing
/// is the lock's whole latency cost; the GPU concurrency test reports it.
static WEBGPU_WAIT: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// [`WEBGPU_RUN`] waits so far: `(acquisitions, total, max)`.
pub fn gpu_wait_stats() -> (u64, std::time::Duration, std::time::Duration) {
    let [n, total, max] = &WEBGPU_WAIT;
    (
        n.load(Ordering::Relaxed),
        std::time::Duration::from_nanos(total.load(Ordering::Relaxed)),
        std::time::Duration::from_nanos(max.load(Ordering::Relaxed)),
    )
}

/// The [`WEBGPU_RUN`] guard when `on_gpu`, `None` otherwise. A poisoned lock (a panic inside a
/// run) is recovered: the lock protects no data of ours.
pub fn gpu_guard(on_gpu: bool) -> Option<MutexGuard<'static, ()>> {
    on_gpu.then(|| {
        let t = std::time::Instant::now();
        let guard = WEBGPU_RUN.lock().unwrap_or_else(PoisonError::into_inner);
        let waited = t.elapsed().as_nanos() as u64;
        let [n, total, max] = &WEBGPU_WAIT;
        n.fetch_add(1, Ordering::Relaxed);
        total.fetch_add(waited, Ordering::Relaxed);
        max.fetch_max(waited, Ordering::Relaxed);
        guard
    })
}

/// Run `f` under [`WEBGPU_RUN`] when `on_gpu` (the flag [`configure`] returned for the
/// session `f` drives); plain call otherwise.
pub fn gpu_serial<T>(on_gpu: bool, f: impl FnOnce() -> T) -> T {
    let _guard = gpu_guard(on_gpu);
    f()
}

/// A model (an `ort::Session` or a parakeet-rs model owning several) with the flag
/// [`configure`] returned for it. Derefs to the model; callers hold [`Guarded::guard`] (or use
/// [`gpu_serial`] with [`Guarded::on_gpu`]) around each run, and the drop — which releases GPU
/// resources on the shared context — happens under the lock here.
pub struct Guarded<T> {
    inner: ManuallyDrop<T>,
    on_gpu: bool,
}

impl<T> Guarded<T> {
    /// Wrap `inner`, built on WebGPU when `on_gpu`.
    pub fn new(inner: T, on_gpu: bool) -> Self {
        Self {
            inner: ManuallyDrop::new(inner),
            on_gpu,
        }
    }

    /// Whether this model's sessions run on WebGPU.
    pub fn on_gpu(&self) -> bool {
        self.on_gpu
    }

    /// [`gpu_guard`] for this model: hold it across one run, drop it before reading outputs.
    pub fn guard(&self) -> Option<MutexGuard<'static, ()>> {
        gpu_guard(self.on_gpu)
    }
}

impl<T> Deref for Guarded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for Guarded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T> Drop for Guarded<T> {
    fn drop(&mut self) {
        // SAFETY: `inner` is dropped exactly once, here, and never touched again.
        gpu_serial(self.on_gpu, || unsafe {
            ManuallyDrop::drop(&mut self.inner)
        });
    }
}

/// The WebGPU device [`configure`] appends once [`WEBGPU_STATE`] is 1: its position in
/// `Environment::devices()` (stable for the process; the list only grows at registration) and
/// its label for the bench.
static WEBGPU_CHOICE: Mutex<Option<(usize, String)>> = Mutex::new(None);

/// The one `RegisterExecutionProviderLibrary` this process makes for `webgpu_ep`, with the
/// plugin path it named: ORT rejects a second under the same name ("library is already
/// registered under webgpu_ep"), which sent the second caller in a process — meeting STT after
/// voice Status — to the CPU. See [`register_once`].
static WEBGPU_REGISTERED: OnceLock<(PathBuf, Result<(), String>)> = OnceLock::new();

/// Record `models.webgpu_adapter` (`theuth` main, before any model loads).
pub fn prefer_webgpu_adapter(substring: Option<String>) {
    *WEBGPU_ADAPTER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        substring.filter(|s| !s.trim().is_empty());
}

/// The WebGPU device sessions run on (`<type> <vendor> <metadata>`), once [`ep`] or
/// [`configure`] has chosen one; `None` on the CPU or DirectML.
pub fn webgpu_device() -> Option<String> {
    (WEBGPU_STATE.load(Ordering::Relaxed) == 1)
        .then(|| {
            WEBGPU_CHOICE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(|(_, label)| label.clone())
        })
        .flatten()
}

/// `ort::init_from` runs once per process, on the first call that finds the runtime; every
/// later call returns the path it used — or the error the one load attempt made, since after
/// it nothing in this process can load a runtime ([`ORT`]). The error text names
/// `libonnxruntime` so callers can tell "no runtime" from "no model", and where to put it.
pub fn ort_lib(data_dir: &Path) -> Result<PathBuf, String> {
    let mut slot = ORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(r) = slot.as_ref() {
        return r.clone();
    }
    let lib = find_ort_lib(data_dir)?;
    let loaded = ort::init_from(&lib).map_err(|e| {
        format!(
            "load {}: {} (ONNX Runtime stays unavailable until restart)",
            lib.display(),
            load_error_text(&e)
        )
    });
    let r = match loaded {
        Ok(builder) => {
            builder.commit();
            tracing::debug!("onnxruntime: loaded {}", lib.display());
            let plugin = webgpu_plugin_path(data_dir);
            *WEBGPU_PLUGIN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                plugin.is_file().then_some(plugin);
            Ok(lib)
        }
        Err(e) => {
            tracing::error!("onnxruntime: {e}");
            Err(e)
        }
    };
    *slot = Some(r.clone());
    r
}

/// `ort`'s load error with the loader's reason appended: `libloading` 0.9 prints a bare
/// "dlopen failed" and keeps dyld's/ld.so's text (`slice is not valid mach-o file`, `code
/// signature invalid`, …) in `source()`, which is the one line that says why a file that exists
/// did not load.
fn load_error_text(e: &ort::LoadDynamicError) -> String {
    let mut text = e.to_string();
    let mut source = match e {
        ort::LoadDynamicError::Dlopen { error, .. } => std::error::Error::source(error),
        _ => None,
    };
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

/// Whether [`ort_lib`] has already succeeded in this process. Every entry point that reaches
/// `ort::api()` ([`ep`], [`devices`], [`configure`], plugin registration) checks this first:
/// before it, `ort` would dlopen its default `libonnxruntime` name, fail, and leave the process
/// in the state [`ORT`] describes.
pub fn ort_ready() -> bool {
    matches!(
        ORT.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
        Some(Ok(_))
    )
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

/// WebGPU plugin registration: 0 = not tried, 1 = registered and a hardware GPU (or, for
/// explicit `webgpu`, a software adapter) chosen in [`WEBGPU_CHOICE`], 2 = failed (one WARN;
/// every session on the CPU from then on).
static WEBGPU_STATE: AtomicU8 = AtomicU8::new(0);

/// Whether sessions built by [`configure`] ask for DirectML: only when the configured
/// accelerator says so. `auto` never picks it (58 step 13): Microsoft's DirectML build of ORT
/// ended at 1.24.4, below the runtime `ort` loads, so the EP is dead unless someone points
/// `$ORT_DYLIB_PATH` at an old DirectML build on purpose.
fn directml_wanted() -> bool {
    cfg!(windows)
        && !DML_FAILED.load(Ordering::Relaxed)
        && accelerator::get() == Accelerator::Directml
}

/// Whether the WebGPU plugin is registered and a device chosen, registering it on the first
/// call that wants it: the configured accelerator is `webgpu`, or `auto` and the plugin
/// library sits under `<data_dir>/models/onnxruntime/plugins/` **and** a discrete GPU answers
/// ([`choose_adapter`]). Needs [`ort_lib`] first (before it there is no environment to register
/// with, and asking `ort` for one would load the wrong library); a failure is final for the process.
fn webgpu_ready() -> bool {
    match WEBGPU_STATE.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    if !ort_ready() {
        return false;
    }
    let plugin = WEBGPU_PLUGIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if !webgpu_wanted(accelerator::get(), plugin.is_some()) {
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

/// What a WebGPU adapter is, from ORT's device description ([`classify_adapter`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    /// NVIDIA / AMD: preferred.
    Discrete,
    /// Intel, Apple, Qualcomm, … — any other hardware GPU.
    Integrated,
    /// A software renderer (SwiftShader, llvmpipe/lavapipe, WARP, Dawn's null adapter with
    /// no vendor): 15× slower than the CPU EP on the Ubuntu LXC; `auto` never takes it.
    Software,
}

/// Names that mark a software renderer, matched case-insensitively against the hardware
/// vendor and every metadata value.
const SOFTWARE_NAMES: [&str; 5] = [
    "swiftshader",
    "llvmpipe",
    "lavapipe",
    "warp",
    "microsoft basic render",
];
/// PCI vendor IDs of software renderers: Microsoft (WARP, Basic Render Driver), Google
/// (SwiftShader), Mesa (`VK_VENDOR_ID_MESA`).
const SOFTWARE_VENDOR_IDS: [u32; 3] = [0x1414, 0x1AE0, 0x10005];
/// PCI vendor IDs of the discrete-GPU makers: NVIDIA, AMD.
const DISCRETE_VENDOR_IDS: [u32; 2] = [0x10DE, 0x1002];

/// Classify one EP device's hardware: `None` when it is not a GPU at all. A GPU with neither a
/// vendor name nor a vendor ID (Dawn's fallback adapter on a driverless Linux box), a software
/// renderer's vendor ID, or a software renderer's name anywhere in `description` (the metadata
/// values joined) is [`Adapter::Software`]; NVIDIA/AMD by ID or name is [`Adapter::Discrete`];
/// everything else [`Adapter::Integrated`].
pub fn classify_adapter(
    ty: DeviceType,
    vendor: &str,
    vendor_id: u32,
    description: &str,
) -> Option<Adapter> {
    if ty != DeviceType::GPU {
        return None;
    }
    let text = format!("{vendor} {description}").to_ascii_lowercase();
    Some(
        if (vendor.trim().is_empty() && vendor_id == 0)
            || SOFTWARE_VENDOR_IDS.contains(&vendor_id)
            || SOFTWARE_NAMES.iter().any(|n| text.contains(n))
        {
            Adapter::Software
        } else if DISCRETE_VENDOR_IDS.contains(&vendor_id)
            || ["nvidia", "amd", "radeon"].iter().any(|n| text.contains(n))
        {
            Adapter::Discrete
        } else {
            Adapter::Integrated
        },
    )
}

/// Which of `adapters` (kind, label) a session should use: the first whose label contains
/// `preferred` (`models.webgpu_adapter`, case-insensitive — an explicit name may pick anything),
/// else the first discrete GPU, else the first integrated one, else `None`: a software renderer
/// is never chosen by kind.
pub fn pick_adapter(adapters: &[(Adapter, String)], preferred: Option<&str>) -> Option<usize> {
    if let Some(p) = preferred {
        let p = p.to_ascii_lowercase();
        if let Some(i) = adapters
            .iter()
            .position(|(_, label)| label.to_ascii_lowercase().contains(&p))
        {
            return Some(i);
        }
        tracing::warn!("webgpu: no adapter matches models.webgpu_adapter {p:?}; choosing by kind");
    }
    [Adapter::Discrete, Adapter::Integrated]
        .iter()
        .find_map(|k| adapters.iter().position(|(a, _)| a == k))
}

/// The adapter sessions get, or why none does (the WARN text). Sky's ruling (58 step 13):
/// `auto` ships WebGPU for a **discrete** GPU only — integrated GPUs measured slower than the
/// CPU EP (M3 Max), a software adapter 15× slower (Ubuntu LXC). Explicit `webgpu` takes any
/// hardware adapter, or the software one with a WARN.
pub fn choose_adapter(
    accelerator: Accelerator,
    adapters: &[(Adapter, String)],
    preferred: Option<&str>,
) -> Result<usize, String> {
    let explicit = accelerator == Accelerator::Webgpu;
    let Some(i) = pick_adapter(adapters, preferred) else {
        if explicit {
            tracing::warn!(
                "webgpu: only a software adapter ({}); using it because models.accelerator is webgpu",
                adapters[0].1
            );
            return Ok(0);
        }
        return Err(format!(
            "only a software adapter ({}); using CPU",
            adapters[0].1
        ));
    };
    let (kind, label) = &adapters[i];
    if !explicit && *kind != Adapter::Discrete {
        return Err(format!(
            "{label} is not a discrete GPU; using CPU (models.accelerator: webgpu overrides)"
        ));
    }
    Ok(i)
}

/// `key=value` pairs of an `OrtKeyValuePairs` (raw `ort-sys`: `ort` rc.13 wraps neither the
/// device metadata nor `HardwareDevice_VendorId`).
fn key_values(kv: *const ort::sys::OrtKeyValuePairs) -> Vec<(String, String)> {
    if kv.is_null() {
        return Vec::new();
    }
    let api = ort::api();
    let (mut keys, mut values, mut n) = (std::ptr::null(), std::ptr::null(), 0usize);
    // SAFETY: `kv` is a live pointer ORT handed out for a device the environment still owns;
    // the arrays it fills are ORT-owned, read for `n` entries only.
    unsafe {
        (api.GetKeyValuePairs)(kv, &mut keys, &mut values, &mut n);
        let text =
            |p: *const std::ffi::c_char| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
        (0..n)
            .map(|i| (text(*keys.add(i)), text(*values.add(i))))
            .collect()
    }
}

/// One EP device as text: `<EP> — <type> <vendor>[:<vendor id>] [<k>=<v>, …] (<EP vendor>)`.
fn describe(d: &ort::device::Device<'_>) -> String {
    let hw = d.hardware_device();
    // SAFETY: `hw.ptr()` is non-null and ORT-owned for the environment's lifetime.
    let vendor_id = unsafe { (ort::api().HardwareDevice_VendorId)(hw.ptr()) };
    let meta: Vec<String> = key_values(unsafe { (ort::api().HardwareDevice_Metadata)(hw.ptr()) })
        .into_iter()
        .chain(key_values(unsafe {
            (ort::api().EpDevice_EpMetadata)(d.ptr())
        }))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    format!(
        "{} — {:?} {}:{vendor_id:#x} [{}] ({})",
        d.ep().unwrap_or("?"),
        hw.ty(),
        hw.vendor().unwrap_or_default(),
        meta.join(", "),
        d.ep_vendor().unwrap_or("?")
    )
}

/// The WebGPU devices in `env`: (position in `devices()`, kind, label), GPUs only.
fn webgpu_adapters(env: &ort::environment::Environment) -> Vec<(usize, Adapter, String)> {
    env.devices()
        .enumerate()
        .filter(|(_, d)| d.ep().is_ok_and(|n| n == WEBGPU_EP))
        .filter_map(|(i, d)| {
            let hw = d.hardware_device();
            // SAFETY: as in `describe`.
            let vendor_id = unsafe { (ort::api().HardwareDevice_VendorId)(hw.ptr()) };
            let meta = key_values(unsafe { (ort::api().HardwareDevice_Metadata)(hw.ptr()) })
                .into_iter()
                .chain(key_values(unsafe {
                    (ort::api().EpDevice_EpMetadata)(d.ptr())
                }))
                .map(|(_, v)| v)
                .collect::<Vec<_>>()
                .join(" ");
            let vendor = hw.vendor().unwrap_or_default();
            let kind = classify_adapter(hw.ty(), vendor, vendor_id, &meta)?;
            Some((
                i,
                kind,
                format!("{:?} {vendor} {meta}", hw.ty())
                    .trim_end()
                    .to_owned(),
            ))
        })
        .collect()
}

/// Whether the environment already offers a WebGPU device.
fn has_webgpu_device(env: &ort::environment::Environment) -> bool {
    env.devices().any(|d| d.ep().is_ok_and(|n| n == WEBGPU_EP))
}

/// Make a WebGPU device available in the process-wide environment and choose the one sessions
/// use. A runtime with the EP built in (Microsoft's osx-arm64 1.27.0 has it; the win-x64 and
/// linux-x64 CPU builds do not) already has one, and loading the plugin beside it would add a
/// second `WebGpuExecutionProvider` from another factory that `with_devices` refuses to mix —
/// so the plugin is only registered (`RegisterExecutionProviderLibrary` + `GetEpDevices`
/// through `ort`'s `api-22`) when there is none, and must then produce a device. Then
/// [`pick_adapter`]: a hardware GPU, discrete first; only a software renderer (Dawn on a
/// GPU-less box makes one) is a failure for `auto` and a WARN for explicit `webgpu`.
fn register_webgpu(plugin: Option<&Path>) -> Result<(), String> {
    let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
    if has_webgpu_device(&env) {
        tracing::debug!("onnxruntime: {WEBGPU_EP} is built into the runtime; plugin not loaded");
    } else {
        let plugin = plugin.ok_or_else(|| {
            format!("the runtime has no {WEBGPU_EP} and there is no {WEBGPU_LIB} under <data_dir>/models/{DIR}/{PLUGIN_DIR}/ (Microsoft.ML.OnnxRuntime.EP.WebGpu, runtimes/<rid>/native/)")
        })?;
        if let Some(v) = runtime_version()
            && !webgpu_accepts(&v)
        {
            return Err(format!(
                "ONNX Runtime {v} is older than {}.{}.{}, the plugin's minimum",
                WEBGPU_MIN_ORT[0], WEBGPU_MIN_ORT[1], WEBGPU_MIN_ORT[2]
            ));
        }
        register_once(&WEBGPU_REGISTERED, plugin, |p| {
            env.register_ep_library("webgpu_ep", p)
                .map(drop)
                .map_err(|e| format!("register {}: {e}", p.display()))
        })?;
    }
    let adapters = webgpu_adapters(&env);
    if adapters.is_empty() {
        return Err(format!(
            "{WEBGPU_EP} registered but no WebGPU device was found (no GPU adapter, no Vulkan loader, or an old driver)"
        ));
    }
    let preferred = WEBGPU_ADAPTER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let kinds: Vec<(Adapter, String)> = adapters.iter().map(|(_, k, l)| (*k, l.clone())).collect();
    let (index, kind, label) =
        &adapters[choose_adapter(accelerator::get(), &kinds, preferred.as_deref())?];
    tracing::debug!("onnxruntime: {WEBGPU_EP} device {label} ({kind:?})");
    *WEBGPU_CHOICE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((*index, label.clone()));
    Ok(())
}

/// Run `register(plugin)` at most once per `cell` (concurrent callers wait for the first) and
/// hand every caller the first result; a later call naming another path is logged and gets it
/// too. An error containing "already registered" counts as success — something else in the
/// process (a second environment, an earlier run of us) loaded the library under that name.
fn register_once(
    cell: &OnceLock<(PathBuf, Result<(), String>)>,
    plugin: &Path,
    register: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let (first, result) = cell.get_or_init(|| {
        let r = match register(plugin) {
            Err(e) if e.contains("already registered") => Ok(()),
            r => r,
        };
        (plugin.to_path_buf(), r)
    });
    if first != plugin {
        tracing::debug!(
            "onnxruntime: {WEBGPU_EP} plugin already registered from {}; {} ignored",
            first.display(),
            plugin.display()
        );
    }
    result.clone()
}

/// The loaded runtime's version from the `VERSION` file beside it (the models-dir layout);
/// `None` for a bundled, `$ORT_DYLIB_PATH` or Homebrew runtime, whose version `ort` already
/// checked against its API level at load.
fn runtime_version() -> Option<String> {
    let lib = ORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?
        .ok()?;
    std::fs::read_to_string(lib.parent()?.join("VERSION"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whether an ORT `version` ("1.27.0") is at least [`WEBGPU_MIN_ORT`]. Unparseable → `false`.
pub fn webgpu_accepts(version: &str) -> bool {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u32>().ok());
    let v = [
        parts.next().flatten(),
        parts.next().flatten(),
        parts.next().flatten(),
    ];
    let [Some(a), Some(b), Some(c)] = v else {
        return false;
    };
    [a, b, c] >= WEBGPU_MIN_ORT
}

/// The execution provider sessions built by [`configure`] run on, for status replies:
/// [`WEBGPU_EP`], [`DML_EP`] or [`CPU_EP`]. Before [`ort_lib`] it is [`CPU_EP`] (or [`DML_EP`]
/// when configured) without touching `ort`: `auto` cannot see the plugin and nothing can be
/// registered yet.
pub fn ep() -> &'static str {
    if webgpu_ready() {
        WEBGPU_EP
    } else if directml_wanted() {
        DML_EP
    } else {
        CPU_EP
    }
}

/// Every execution-provider device the environment knows, one line each ([`describe`]), for
/// `t --bench-models`. Empty before [`ort_lib`] (without touching `ort`). WebGPU shows up only
/// after registration ([`ep`] or [`configure`] triggers it).
pub fn devices() -> Vec<String> {
    if !ort_ready() {
        return Vec::new();
    }
    let Ok(env) = ort::environment::Environment::current() else {
        return Vec::new();
    };
    env.devices().map(|d| describe(&d)).collect()
}

/// Register the execution provider [`accelerator::get`] resolves to on `builder`. CPU is
/// ORT's default and needs nothing; WebGPU or DirectML that fails to register logs one WARN
/// and leaves this and every later session on the CPU. Never fails: the builder comes back
/// untouched in the worst case. The flag is `true` when WebGPU was appended: the session's
/// runs and drop must then go through [`WEBGPU_RUN`] ([`Guarded`]).
pub fn configure(builder: SessionBuilder) -> (SessionBuilder, bool) {
    if !ort_ready() {
        // A builder exists, so `ort` found a runtime by itself (`$ORT_DYLIB_PATH`, a library on
        // the loader path) — never the one `ort_lib` would have chosen. Name that.
        tracing::warn!(
            "onnxruntime: configure() before ort_lib(): the runtime was not loaded by ort_lib"
        );
    }
    if webgpu_ready() {
        let index = WEBGPU_CHOICE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, |(i, _)| *i);
        return match ort::environment::Environment::current() {
            // One device, the chosen adapter: several would be several factories' devices, which ORT refuses to mix.
            Ok(env) => match builder.with_devices(env.devices().skip(index).take(1), None) {
                Ok(b) => (b, true),
                Err(e) => {
                    WEBGPU_STATE.store(2, Ordering::Relaxed);
                    tracing::warn!(
                        "onnxruntime: appending {WEBGPU_EP} failed ({e}); ONNX models run on the CPU"
                    );
                    (e.recover(), false)
                }
            },
            Err(e) => {
                WEBGPU_STATE.store(2, Ordering::Relaxed);
                tracing::warn!(
                    "onnxruntime: no environment for {WEBGPU_EP} ({e}); ONNX models run on the CPU"
                );
                (builder, false)
            }
        };
    }
    if !directml_wanted() {
        return (builder, false);
    }
    #[cfg(windows)]
    {
        match builder
            .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
        {
            Ok(b) => {
                tracing::debug!("onnxruntime: {DML_EP} registered");
                (b, false)
            }
            Err(e) => {
                DML_FAILED.store(true, Ordering::Relaxed);
                tracing::warn!(
                    "onnxruntime: DirectML unavailable ({e}); ONNX models run on the CPU"
                );
                (e.recover(), false)
            }
        }
    }
    #[cfg(not(windows))]
    (builder, false)
}

/// `<data_dir>/models/onnxruntime/plugins/<WEBGPU_LIB>`, where `auto` looks for the plugin.
pub fn webgpu_plugin_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("models")
        .join(DIR)
        .join(PLUGIN_DIR)
        .join(WEBGPU_LIB)
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

    /// Status paths run before any model loads ([`ep`] for `voice_status`/`meeting_status`,
    /// [`devices`] for the bench). Before [`ort_lib`] they must answer without reaching
    /// `ort::api()`, which would dlopen `ort`'s default library name, fail (a panic here), and
    /// leave the process unable to load a runtime. `ort`'s state is process-wide: no test in this
    /// binary calls `ort_lib`, so `ort_ready()` stays false throughout.
    #[test]
    fn status_before_ort_lib_answers_without_loading_ort() {
        assert!(!ort_ready());
        assert!(matches!(ep(), CPU_EP | DML_EP));
        assert!(devices().is_empty());
        assert!(webgpu_device().is_none());
        assert!(!ort_ready());
    }

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

    #[test]
    fn plugin_is_registered_once_and_already_registered_is_success() {
        let cell = OnceLock::new();
        let a = Path::new("/plugins/a.dll");
        let calls = std::cell::Cell::new(0);
        let reg = |_: &Path| {
            calls.set(calls.get() + 1);
            Ok(())
        };
        assert_eq!(register_once(&cell, a, reg), Ok(()));
        assert_eq!(register_once(&cell, a, reg), Ok(()));
        // Another path, another caller: the first registration is what counts.
        assert_eq!(
            register_once(&cell, Path::new("/plugins/b.dll"), |_| Err("x".into())),
            Ok(())
        );
        assert_eq!(calls.get(), 1);

        // ORT's refusal is a success; any other first error is final for the process.
        let cell = OnceLock::new();
        let ort = |p: &Path| {
            Err(format!(
                "register {}: library is already registered under webgpu_ep",
                p.display()
            ))
        };
        assert_eq!(register_once(&cell, a, ort), Ok(()));
        let cell = OnceLock::new();
        assert_eq!(
            register_once(&cell, a, |_| Err("no such file".into())),
            Err("no such file".into())
        );
        assert_eq!(
            register_once(&cell, a, |_| Ok(())),
            Err("no such file".into())
        );
    }

    #[test]
    fn adapters_are_classified_from_the_device_description() {
        use Adapter::*;
        let gpu = DeviceType::GPU;
        // The Windows box: Quadro M2000M + HD P530, by vendor id and by name alone.
        assert_eq!(
            classify_adapter(gpu, "NVIDIA", 0x10DE, "Description=NVIDIA Quadro M2000M"),
            Some(Discrete)
        );
        assert_eq!(classify_adapter(gpu, "NVIDIA", 0, ""), Some(Discrete));
        assert_eq!(
            classify_adapter(
                gpu,
                "Intel Corporation",
                0x8086,
                "Intel(R) HD Graphics P530"
            ),
            Some(Integrated)
        );
        assert_eq!(
            classify_adapter(gpu, "Advanced Micro Devices", 0x1002, "Radeon RX 6600"),
            Some(Discrete)
        );
        // The Mac's built-in EP names Apple with no metadata.
        assert_eq!(classify_adapter(gpu, "Apple", 0, ""), Some(Integrated));
        // The Ubuntu LXC: Dawn's adapter with no vendor at all.
        assert_eq!(classify_adapter(gpu, "", 0, ""), Some(Software));
        assert_eq!(
            classify_adapter(gpu, "Microsoft", 0x1414, "Microsoft Basic Render Driver"),
            Some(Software)
        );
        assert_eq!(
            classify_adapter(gpu, "Microsoft", 0, "WARP"),
            Some(Software)
        );
        assert_eq!(
            classify_adapter(gpu, "Google", 0x1AE0, "SwiftShader Device (LLVM 10.0.0)"),
            Some(Software)
        );
        assert_eq!(
            classify_adapter(gpu, "Mesa", 0, "llvmpipe (LLVM 15.0.7, 256 bits)"),
            Some(Software)
        );
        assert_eq!(
            classify_adapter(gpu, "Mesa", 0x10005, "lavapipe"),
            Some(Software)
        );
        assert_eq!(classify_adapter(DeviceType::CPU, "Intel", 0x8086, ""), None);
    }

    #[test]
    fn discrete_beats_integrated_and_software_is_never_picked_by_kind() {
        let win = [
            (
                Adapter::Integrated,
                "GPU Intel Corporation HD P530".to_owned(),
            ),
            (Adapter::Discrete, "GPU NVIDIA Quadro M2000M".to_owned()),
        ];
        assert_eq!(pick_adapter(&win, None), Some(1));
        assert_eq!(pick_adapter(&win, Some("intel")), Some(0));
        assert_eq!(pick_adapter(&win, Some("nomatch")), Some(1));
        let lxc = [(Adapter::Software, "GPU".to_owned())];
        assert_eq!(pick_adapter(&lxc, None), None);
        assert_eq!(pick_adapter(&lxc, Some("gpu")), Some(0));
        let mixed = [
            (Adapter::Software, "GPU WARP".to_owned()),
            (Adapter::Integrated, "GPU Apple".to_owned()),
        ];
        assert_eq!(pick_adapter(&mixed, None), Some(1));
    }

    #[test]
    fn auto_takes_only_a_discrete_gpu_and_webgpu_takes_anything() {
        let win = [
            (
                Adapter::Integrated,
                "GPU Intel Corporation HD P530".to_owned(),
            ),
            (Adapter::Discrete, "GPU NVIDIA Quadro M2000M".to_owned()),
        ];
        let mac = [(Adapter::Integrated, "GPU Apple".to_owned())];
        let lxc = [(Adapter::Software, "GPU".to_owned())];
        assert_eq!(choose_adapter(Accelerator::Auto, &win, None), Ok(1));
        assert!(choose_adapter(Accelerator::Auto, &win, Some("intel")).is_err());
        assert!(
            choose_adapter(Accelerator::Auto, &mac, None)
                .unwrap_err()
                .contains("not a discrete GPU")
        );
        assert!(
            choose_adapter(Accelerator::Auto, &lxc, None)
                .unwrap_err()
                .contains("software adapter")
        );
        assert_eq!(
            choose_adapter(Accelerator::Webgpu, &win, Some("intel")),
            Ok(0)
        );
        assert_eq!(choose_adapter(Accelerator::Webgpu, &mac, None), Ok(0));
        assert_eq!(choose_adapter(Accelerator::Webgpu, &lxc, None), Ok(0));
    }

    #[test]
    fn cpu_sessions_never_take_the_run_lock() {
        // Hold the lock on this thread: a CPU run must still go straight through.
        let _held = gpu_guard(true).expect("guard");
        assert!(gpu_guard(false).is_none());
        assert_eq!(gpu_serial(false, || 7), 7);
    }

    #[test]
    fn gpu_runs_never_overlap_and_a_drop_waits_its_turn() {
        use std::sync::atomic::AtomicUsize;
        static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
        static OVERLAPS: AtomicUsize = AtomicUsize::new(0);
        struct Spins;
        impl Drop for Spins {
            fn drop(&mut self) {
                if IN_FLIGHT.fetch_add(1, Ordering::SeqCst) != 0 {
                    OVERLAPS.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_micros(200));
                IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let threads: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        if i == 0 {
                            drop(Guarded::new(Spins, true));
                        } else {
                            gpu_serial(true, || drop(Spins));
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(OVERLAPS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_panic_under_the_lock_does_not_poison_later_runs() {
        let r = std::thread::spawn(|| gpu_serial(true, || panic!("inside a run"))).join();
        assert!(r.is_err());
        assert_eq!(gpu_serial(true, || 1), 1);
        assert!(gpu_guard(true).is_some());
    }
}
