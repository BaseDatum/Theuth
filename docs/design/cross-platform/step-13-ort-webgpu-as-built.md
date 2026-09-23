# 58 step 13 — ONNX Runtime WebGPU plugin EP, as built, 2026-09-23

Branch `feature/ort-webgpu` off `644bc61`. Scope: `models.accelerator: webgpu`, the WebGPU
plugin execution provider registered at run time through `ort` (Dawn: D3D12 on Windows, Vulkan
on Linux, Metal on macOS — no CUDA, any vendor), `auto` preferring it when the plugin library is
installed, `t --bench-models` proving the graph ran on a real utterance. macOS with the default
config and no plugin installed is unchanged (CPU EP, no WARN).

## Crate API finding (deliverable 1)

`ort 2.0.0-rc.13` **has** the plugin-EP API; it is gated behind the cargo feature `api-22`:

| ORT C API | `ort` rc.13 | feature |
|---|---|---|
| `RegisterExecutionProviderLibrary(env, name, path)` | `Environment::register_ep_library(name, path) -> ExecutionProviderLibrary` (no `Drop`; dropping the handle keeps the library registered; `.unregister()` exists) | `api-22` + `std` |
| `GetEpDevices(env)` | `Environment::devices() -> impl Iterator<Item = device::Device>`; `Device::ep()`, `ep_vendor()`, `hardware_device().{ty(), vendor(), id()}` | `api-22` |
| `SessionOptionsAppendExecutionProvider_V2(opts, env, devices, n, keys, values, n)` | `SessionBuilder::with_devices(devices, Option<&[(String, String)]>)` — options keyed `"<EpName>.<key>"` | `api-22` |
| `SessionOptionsSetEpSelectionPolicy` | `SessionBuilder::with_auto_device(AutoDevicePolicy)` | `api-22` |

`ort-sys/api-22` raises `ORT_API_VERSION` from 17 to 22 (`version.rs`: `17 + V18 + … + V28`,
each `api-N` implies `api-(N-1)`), so `ort::init_from` now refuses a runtime older than 1.22.x
(we load 1.27.0 everywhere; step 12's Windows 1.24.4 DirectML build would also pass). No pin bump,
no raw `ort-sys` calls: the workspace `ort` line gained `"api-22"`, nothing else.

## The knob

`models.accelerator: auto | cpu | webgpu | directml | cuda`. For ONNX sessions:

| accelerator | ONNX sessions (STT, dictation, diarisation) |
|---|---|
| `auto` | WebGPU iff `<data_dir>/models/onnxruntime/plugins/<WEBGPU_LIB>` exists **and** registration yields a `WebGpuExecutionProvider` device; else CPU. DirectML is no longer reachable from `auto`. |
| `webgpu` | WebGPU asked for regardless of the file check; one WARN and CPU when the plugin is missing, refuses the runtime, or finds no adapter |
| `directml` | as step 12 (explicit only) |
| `cpu`, `cuda` | CPU |

candle (decision) treats `webgpu` like `auto` (`crates/decision/src/lib.rs`, one match arm —
outside the allow-list, mechanical: the enum grew).

Why DirectML left `auto`: Microsoft's DirectML build of ORT ended at **1.24.4**; the runtime we
ship and `ort` accepts is 1.27.0 (CPU build, no DML EP), so `auto` gating on `DirectML.dll`
could only ever pick a provider that is not in the loaded library. The code stays for someone
pointing `$ORT_DYLIB_PATH` at the old build with `accelerator: directml`.

## Code

- `crates/models/src/accelerator.rs`: `Accelerator::Webgpu = 4`, spelling `webgpu`.
- `crates/models/src/onnx.rs`:
  - consts `PLUGIN_DIR = "plugins"`, `WEBGPU_LIB` (`onnxruntime_providers_webgpu.dll` /
    `libonnxruntime_providers_webgpu.so` / `libonnxruntime_providers_webgpu.dylib`),
    `WEBGPU_EP = "WebGpuExecutionProvider"`, `WEBGPU_MIN_ORT = [1, 24, 4]`;
    `webgpu_plugin_path(data_dir)`.
  - `ort_lib` records whether the plugin file exists for the `data_dir` it loaded from.
  - `webgpu_ready()` registers once per process on the first `ep()`/`configure()` that wants it
    (`WEBGPU_STATE`: untried / ready / failed). `register_webgpu`: version gate on the `VERSION`
    file beside the runtime when present (`webgpu_accepts`), `Environment::current()`,
    `register_ep_library("webgpu_ep", path)`, then `devices().any(ep == WebGpuExecutionProvider)`
    — a plugin that loads but finds no adapter (no GPU, no `libvulkan.so.1`, old driver) is a
    failure with the reason in the WARN.
  - `configure` appends the WebGPU device(s) with `with_devices(.., None)`; on error one WARN,
    `e.recover()`, CPU from then on. `ep()` reports `WebGpuExecutionProvider` | `DmlExecutionProvider`
    | `CPUExecutionProvider`. `devices()` lists every EP device as text for the bench.
  - `webgpu_wanted(accelerator, plugin_present)` is the pure selection rule; tests cover it and
    the version gate.
- `crates/voice/src/stt.rs`: `speech_sample(tts_dir, voice, text) -> Vec<f32>` (16 kHz) renders a
  sentence with the Pocket TTS checkpoint through `ptts::synth::Synth::stream_with` and
  `resample`s 24 → 16 kHz; re-exported from `voice` (`lib.rs`, one word in the `pub use` line —
  outside the allow-list, disclosed).
- `crates/theuth/src/bench_models.rs`: prints the plugin path and whether it is present, every EP
  device, first-run overhead (first − second silence run), and a **third run on the synthesized
  utterance** with the expected text beside the transcript. `main.rs`: config sample and
  `--accelerator` help.

## `t --bench-models --accelerator webgpu`

```
platform: <triple>  accelerator: webgpu
onnxruntime: <path>
onnxruntime ep: WebGpuExecutionProvider | CPUExecutionProvider
onnxruntime webgpu plugin: <data_dir>/models/onnxruntime/plugins/<WEBGPU_LIB> (present | absent)
onnxruntime device: CPUExecutionProvider — CPU <vendor> (Microsoft)
onnxruntime device: WebGpuExecutionProvider — GPU <vendor> (Microsoft)      | (absent when registration failed)
stt load: N ms (<dir>)
stt first run, 5 s silence: N ms (text "")
stt first-run overhead (shader/graph compile): N ms
stt second run, 5 s silence: N ms (text "")
stt speech clip: 5.x s synthesized in N ms (expect "The quick brown fox …")
stt third run, speech: N ms (text "The quick brown fox …")
decision …
```

`RUST_LOG=warn` shows the one WARN (`onnxruntime: WebGPU unavailable (<reason>); ONNX models run
on the CPU`) when the plugin is absent or finds no device.

## Plugin library for the manifest (deliverable 3)

The GitHub releases `plugin-ep-webgpu/v0.3.0` and `v0.4.0` carry **no assets** (checked
2026-09-23 via the releases API; only `plugin-ep-cuda/v0.1.0` has zips). The binaries are on
NuGet as `Microsoft.ML.OnnxRuntime.EP.WebGpu` **0.4.0** (published 2026-09-22; also 0.2.1, 0.3.0):

| Package | URL | size | sha256 |
|---|---|---|---|
| `Microsoft.ML.OnnxRuntime.EP.WebGpu` 0.4.0 (nupkg = zip) | `https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.ep.webgpu/0.4.0/microsoft.ml.onnxruntime.ep.webgpu.0.4.0.nupkg` | 46707778 | `cc2e093a2eaa760a630d6c3299f015f1dd86885b9eb5877257135308f7c5e923` |

Its README: requires ONNX Runtime **≥ 1.24.4** ("the plugin EP will report an error when its
library is registered" otherwise); Linux needs the system Vulkan loader `libvulkan.so.1`. No
dependency on `onnxruntime.dll` itself (plugin EPs talk through the `OrtApi` they are handed).
Members (all `runtimes/<rid>/native/`), sha256 of the extracted files:

| rid | file | size | sha256 |
|---|---|---|---|
| win-x64 | `onnxruntime_providers_webgpu.dll` | 10954080 | `85d7b2065b1968b846cd00d5d87d5286d3c44e8f6c557cd382d96300e524b6cb` |
| win-x64 | `dxcompiler.dll` | 17986360 | `d7e740883f8048233d15752bc5d4ba207fc03d835c78fffab26387d400892193` |
| win-x64 | `dxil.dll` | 1508704 | `0a0d059a51dd5e1916a4a9924817d70b5dbb8e0c103a102fa2c6c92cb748c27f` |
| win-arm64 | `onnxruntime_providers_webgpu.dll` | 11279672 | `e2a72894617c0519cae3080534ca1fc957c4ab607a993c05f326d83b322a581d` |
| win-arm64 | `dxcompiler.dll` | 22285624 | `2c4895ad9707963390fba270a613cd37a038064fce5384bae025c695dc681601` |
| win-arm64 | `dxil.dll` | 1769784 | `502a7c6b7660fa5520efbe5b9516a1aac06b9a8c6a7e26011856bead7f5770ed` |
| linux-x64 | `libonnxruntime_providers_webgpu.so` | 16450888 | `9f76e3dc4459421958ad086a9da03bf7e5de9289e9a3a56e28bbb459773dec81` |
| linux-arm64 | `libonnxruntime_providers_webgpu.so` | 13862448 | `720e1288e7e465f924db44b88a98a9d1c543eba3fbb2ff801cf2545f5e7f0279` |
| osx-arm64 | `libonnxruntime_providers_webgpu.dylib` | 14966224 | `894aa43c308c97afe58f6485b8411962181a39b27170f5f210cc7503adb52850` |

`models.json` rows in step 12's format (`path` under `models/`; the desktop downloader needs no
code change — `set_executable` is a no-op for a dylib that is `dlopen`ed):

```json
{ "path": "onnxruntime/plugins/onnxruntime_providers_webgpu.dll", "size": 10954080,
  "sha256": "85d7b2065b1968b846cd00d5d87d5286d3c44e8f6c557cd382d96300e524b6cb",
  "key": "onnxruntime/x86_64-pc-windows-msvc/plugins/onnxruntime_providers_webgpu.dll", "platform": "x86_64-pc-windows-msvc" },
{ "path": "onnxruntime/plugins/dxcompiler.dll", "size": 17986360,
  "sha256": "d7e740883f8048233d15752bc5d4ba207fc03d835c78fffab26387d400892193",
  "key": "onnxruntime/x86_64-pc-windows-msvc/plugins/dxcompiler.dll", "platform": "x86_64-pc-windows-msvc" },
{ "path": "onnxruntime/plugins/dxil.dll", "size": 1508704,
  "sha256": "0a0d059a51dd5e1916a4a9924817d70b5dbb8e0c103a102fa2c6c92cb748c27f",
  "key": "onnxruntime/x86_64-pc-windows-msvc/plugins/dxil.dll", "platform": "x86_64-pc-windows-msvc" },
{ "path": "onnxruntime/plugins/libonnxruntime_providers_webgpu.so", "size": 16450888,
  "sha256": "9f76e3dc4459421958ad086a9da03bf7e5de9289e9a3a56e28bbb459773dec81",
  "key": "onnxruntime/x86_64-unknown-linux-gnu/plugins/libonnxruntime_providers_webgpu.so", "platform": "x86_64-unknown-linux-gnu" },
{ "path": "onnxruntime/plugins/libonnxruntime_providers_webgpu.dylib", "size": 14966224,
  "sha256": "894aa43c308c97afe58f6485b8411962181a39b27170f5f210cc7503adb52850",
  "key": "onnxruntime/aarch64-apple-darwin/plugins/libonnxruntime_providers_webgpu.dylib", "platform": "aarch64-apple-darwin" }
```

By hand on the Windows box: `unzip -j microsoft.ml.onnxruntime.ep.webgpu.0.4.0.nupkg
'runtimes/win-x64/native/*' -d %USERPROFILE%\.theuth\models\onnxruntime\plugins\` (three DLLs),
with `onnxruntime.dll` 1.27.0 (the CPU build from the GitHub release) and `VERSION` = `1.27.0`
one level up. Ubuntu: the `.so` into `~/.theuth/models/onnxruntime/plugins/` plus
`apt install libvulkan1`; without a GPU expect the WARN and the CPU EP. The Mac's copy went to
`~/.theuth/models/onnxruntime/plugins/libonnxruntime_providers_webgpu.dylib` (the models dir
was otherwise not written).

`otool -L` of the osx-arm64 dylib: Metal, IOSurface, QuartzCore, Cocoa, CFNetwork and the C++
runtime only — self-contained. Its exports include `onnx::` schema symbols with default visibility
(and MLAS, sqlite3, zlib, KleidiAI); registering it in the bench process next to `ptts`
(`sentencepiece`) did not abort — the first bench run below loaded both.

## Finding: the macOS runtime already has WebGPU built in

Microsoft's `onnxruntime-osx-arm64-1.27.0.tgz` (32 MB) contains the WebGPU EP (`strings`: 703
`webgpu` hits, Dawn proc-table options); so do 1.24.4 (603) and 1.23.2 (513). The win-x64 zip
(`onnxruntime.dll` 15 MB) and linux-x64 tgz have 7–8 generic hits and **no** EP: on those two
platforms the plugin is the only way. Consequences, all in `register_webgpu`:

- `Environment::devices()` on the Mac lists `WebGpuExecutionProvider — GPU Apple` before anything
  is registered. Registering the plugin as well produced a **second** `WebGpuExecutionProvider`
  device from another factory, and `with_devices` over both failed with
  `All OrtEpDevice values in ep_devices must have the same execution provider.` (first bench run,
  kept in the report). At exit the plugin's `ReleaseEpFactory` logged `Unknown exception` — expect
  that one ERROR line at process exit on Windows/Linux too; it is teardown, after every session
  is gone.
- So: a WebGPU device already present → use it, plugin not loaded (DEBUG line says so); none →
  register the plugin. `configure` appends exactly one WebGPU device (`.take(1)`).
- `auto` still gates on the plugin *file* being present (Sky's opt-in by installing it), so a Mac
  without the file behaves exactly as before: `CPUExecutionProvider`, no WARN, no Dawn — verified
  by moving `plugins/` aside and running `--accelerator auto`.

## Numbers — Mac (M3 Max, macOS 25.6, ORT 1.27.0 built-in WebGPU over Metal, debug build)

Parakeet-TDT (`voice::Engine`, 4 intra-op threads on the CPU EP as before):

| run | `--accelerator cpu` | `--accelerator webgpu` / `auto` with the plugin file |
|---|---|---|
| stt load | 715 ms | 606 ms |
| first run, 5 s silence | 193 ms | 944 ms (shader compile: **738 ms** overhead over the second run) |
| second run, 5 s silence | 190 ms | 206 ms |
| third run, 5.1 s synthesized speech | 199 ms | 420 ms, 275 ms (two runs; new shapes → more compile) |
| transcript of the third run | exact expected sentence | exact expected sentence |

Both EPs returned `"The quick brown fox jumps over the lazy dog while the meeting notes are being
transcribed."` verbatim. ORT warned twice `Some nodes were not assigned to the preferred
execution providers` (shape ops on the CPU; normal). Pocket TTS synthesis of the clip: ~28 s
(CPU, `ptts`) — unrelated to the EP, it just makes the bench slow.

**Verdict (Mac):** WebGPU is slower than CPU by 8 % warm on silence and 1.4–2.1× on speech, plus
0.7 s of shader compile on the first run, on an M3 Max — the CPU EP stays the Mac default (and
`auto` without the plugin file keeps it). The Windows/Linux numbers decide the other two; the
M2000M has no AMX-class CPU beside it, so the ratio may flip.

## Numbers — Windows (Quadro M2000M, DX12) and Ubuntu LXC

_(orchestrator's runs; to be pasted from the steer messages)_

## Judgement calls

- Registration is lazy (first `ep()`/`configure()` that wants it), not inside `ort_lib`: with
  `accelerator: cpu` Dawn is never loaded, and a `webgpu` failure is reported next to the session
  that needed it. It is still once per process and lives in `models::onnx` beside `ort_lib`.
- "Plugin registered, no device" is treated as a failure (WARN + CPU) rather than letting ORT
  append an EP with no adapter: that is exactly the Ubuntu LXC case.
- The version gate reads the `VERSION` file the models-dir layout writes; a bundled/`$ORT_DYLIB_PATH`/
  Homebrew runtime has none and is trusted (`ort` itself already refused anything < 1.22, and the
  plugin refuses < 1.24.4 at registration with its own message, which the WARN carries).
- Windows DLL search: Dawn loads `dxcompiler.dll`/`dxil.dll` by name; whether the plugin's own
  directory is searched is unverified here. If the Windows bench shows a DXC warning, Dawn falls
  back to FXC (`d3dcompiler_47.dll`, in the OS) — slower shader compile, no f16. Copying the two
  DLLs beside `theuth.exe` is the fallback layout.
- No EP options are passed (`with_devices(.., None)`): defaults (`preferredLayout`, buffer cache)
  are what onnxruntime-web ships; tune after the numbers.
