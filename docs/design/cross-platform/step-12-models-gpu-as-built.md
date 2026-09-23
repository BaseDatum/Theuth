# 58 step 12 — models on the Windows GPU, as built, 2026-09-23

Branch `feature/models-gpu` off `067bfa8`. Scope: one `models.accelerator` knob,
ONNX Runtime's DirectML execution provider on Windows, candle's CUDA device for the
decision model, `t --bench-models`, and the Windows runtime entries for the model
manifest. macOS behaviour with the default config is unchanged.

## The knob

`models.accelerator: auto | cpu | directml | cuda` (default `auto`), read once by
`theuth` main right after the Figment is built and stored process-wide in
`models::accelerator::{set, get}` (an `AtomicU8`). A static rather than a parameter
because the session-opening functions (`voice::stt::Engine::load`, `Stream::open`,
`meeting::post::diarize::session`) are reached through actors that carry no config and
the brief limited those files to "session options only".

Resolution is per engine, because the two GPU paths do not overlap:

| accelerator | ONNX sessions (STT, dictation, diarisation) | candle (decision) |
|---|---|---|
| `auto` | Windows: DirectML iff `DirectML.dll` sits beside the `onnxruntime.dll` that `ort_lib` loaded; else CPU | Apple silicon: Metal f16 (today); else CUDA f32 iff built with `decision/cuda`; else CPU f32 |
| `cpu` | CPU | CPU f32 (also on a Mac — an explicit `cpu` wins over Metal) |
| `directml` | DirectML asked for on every platform; one WARN and CPU when registration fails (non-Windows, CPU-only runtime, no DX12 device, stale `DirectML.dll`) | as `auto` |
| `cuda` | CPU (the ORT CUDA EP needs cuDNN 9, not shipped) | `Device::new_cuda(0)`; a build without the feature or a device that does not answer → one WARN and CPU f32 |

`decision.device` keeps its override role: `auto` follows the table above, `cpu` /
`metal` / new `cuda` force. TEN-VAD (`wavekat-vad`'s own session) has no session-options
hook and stays on the CPU; it is a 256-sample model.

## Code

- `crates/models/src/accelerator.rs` (new): `Accelerator` (serde lowercase, `FromStr` for the
  CLI, `name()`), `ModelsConfig { accelerator }`, `set`/`get`.
- `crates/models/src/onnx.rs`: `configure(SessionBuilder) -> SessionBuilder` registers
  `ort::ep::DirectML::default().build().error_on_failure()` when wanted; on `Err` it sets a
  process-wide `DML_FAILED` flag, logs one WARN, and returns `e.recover()` — the builder
  untouched. `ep() -> &'static str` names the provider sessions get
  (`DmlExecutionProvider` | `CPUExecutionProvider`) for status replies; call it after
  `ort_lib` (before, `auto` cannot see the runtime's directory). `CPU_EP`/`DML_EP` consts.
  `find_ort_lib` needed no change: `LIB` is already `onnxruntime.dll` on Windows via
  `platform::ort::ort_library_name()`, so `<data_dir>/models/onnxruntime/onnxruntime.dll`
  and `$ORT_DYLIB_PATH` were already accepted.
- `crates/models/Cargo.toml`: `[target.'cfg(windows)'.dependencies] ort = { workspace = true,
  features = ["directml"] }` — the rc.13 feature is spelled `directml`; `ort-sys/directml` is
  an empty feature under `load-dynamic`, so nothing links. `cargo check -p models --target
  x86_64-pc-windows-msvc` passes.
- `crates/voice/src/stt.rs`: `ParakeetTDT::from_pretrained(dir, Some(exec_config()))` and the
  same for `Nemotron`; `exec_config()` = parakeet-rs's `ExecutionConfig::default()` (4 intra
  threads, CPU — what `None` gave) plus `with_custom_configure(|b| Ok(models::onnx::configure(b)))`.
- `crates/meeting/src/post/diarize.rs::session`: the builder goes through
  `models::onnx::configure` before `commit_from_file`.
- `crates/decision`: `candle::Gpu { None, Metal, Cuda }` with `dtype()` (f16 on Metal, f32
  otherwise); `Loaded::open(dir, gpu: Gpu, dtype)` replaces the `metal: bool` — `Gpu::Cuda`
  is `Device::new_cuda(0)` under `#[cfg(feature = "cuda")]` and a WARN + CPU f32 otherwise.
  `decision::Device` gains `Cuda`; `Device::gpu()` does the resolution above and
  `Device::metal()` is now `gpu() == Gpu::Metal`. `engine::open` uses `gpu.dtype()`. New dep
  `models` in `crates/decision/Cargo.toml`. The four `Loaded::open` callers in
  `examples/{bench,lifecycle,phrasing}.rs` and `tests/model.rs` pass
  `if metal { Gpu::Metal } else { Gpu::None }` (outside the allow-list; mechanical).
- Status: `voice::VoiceStatus.accelerator: String` and `meeting::Status.accelerator: String`
  (`#[serde(default)]`, from `models::onnx::ep()`). Those two `lib.rs` files were outside the
  allow-list; the fields are additive and the deliverable asked for them.
- `crates/theuth`: `--bench-models [--accelerator X]` (`src/bench_models.rs`, new file beside
  `main.rs`); `models` dep; `main.rs` sets the knob for every code path and documents
  `models:` + the `decision.device` line in `DEFAULT_CONFIG`.

## `t --bench-models`

```
platform: <triple>  accelerator: <name>
onnxruntime: <path>            | onnxruntime: missing — <ort_lib's error>
onnxruntime ep: DmlExecutionProvider | CPUExecutionProvider
stt load: N ms (<dir>)         | stt: missing — no checkpoint at <dir>
stt first run, 5 s silence: N ms (text "")
stt second run, 5 s silence: N ms (text "")
decision asks for: <Gpu> <DType>
decision load: N ms on <DeviceLocation> <DType> (<dir>)   | decision: missing — …
decision first ask, T tokens: N ms (probs [...])
decision second ask, T tokens: N ms (probs [...])
```

Exit 0 when everything ran, 2 when the runtime or a checkpoint is missing (the lines say
which and where), 1 on a clap error. `RUST_LOG=debug` on stderr shows `ort_lib`'s source
line and `configure`'s "DmlExecutionProvider registered" / the WARN. `$ORT_DYLIB_PATH`
points at any `onnxruntime.dll` (DirectML build or not) for the orchestrator's manual test;
`--accelerator directml` forces the EP regardless of `DirectML.dll`'s location.

## Windows runtime for the manifest (Sky publishes; `scripts/` was not in scope)

Microsoft stopped shipping the DirectML build of ONNX Runtime after **1.24.4**
(`Microsoft.ML.OnnxRuntime.DirectML` on NuGet ends there; 1.25+ point at Windows ML). The
GitHub `onnxruntime-win-x64-1.27.0.zip` is the CPU build and has no DML EP. `ort`
rc.13 with `load-dynamic` requests API 17 (no `api-N` feature is on), so a 1.24.4 runtime
is accepted. Windows therefore pins 1.24.4 while the other platforms stay on 1.27.0; the
two never share a process.

Sources (sha256 of the nupkg = zip):

| Package | URL | sha256 |
|---|---|---|
| `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4 | `https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg` | `57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681` |
| `Microsoft.AI.DirectML` 1.15.4 (its declared dependency) | `https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg` | `4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9` |

Members and the `models.json` rows (`platform`/`key` as the existing runtime rows;
`crates/desktop/src/models.rs` needs no change — `set_executable` is a no-op on Windows):

```json
{ "path": "onnxruntime/onnxruntime.dll", "size": 17328152,
  "sha256": "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
  "key": "onnxruntime/x86_64-pc-windows-msvc/onnxruntime.dll", "platform": "x86_64-pc-windows-msvc" },
{ "path": "onnxruntime/DirectML.dll", "size": 18527776,
  "sha256": "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1",
  "key": "onnxruntime/x86_64-pc-windows-msvc/DirectML.dll", "platform": "x86_64-pc-windows-msvc" },
{ "path": "onnxruntime/VERSION", "size": 7,
  "sha256": "c7c427fdbf86b1ad24159a0915c1ac8cafdcbfa81bffbcb92a7bccb2a95b2202",
  "key": "onnxruntime/x86_64-pc-windows-msvc/VERSION", "platform": "x86_64-pc-windows-msvc" }
```

`onnxruntime.dll` = `runtimes/win-x64/native/onnxruntime.dll` in the first nupkg;
`DirectML.dll` = `bin/x64-win/DirectML.dll` in the second; `VERSION` = `1.24.4\n`.
(`onnxruntime_providers_shared.dll` in the same folder is not needed by the DML EP.)

For `scripts/release-models.sh`: add `win-x64` to `PLATFORMS`, but its row fetches the two
nupkgs above with `unzip -p` instead of the GitHub `.tgz`, uploads three files, and uses
`ORT_VERSION_WIN_X64="1.24.4"`; `ort-version.env` gets the two nupkg sha256s. By hand
on the Windows box: unpack both members into `%USERPROFILE%\.theuth\models\onnxruntime\`
and write `VERSION`.

## Judgement calls

- ORT loads `DirectML.dll` from its own directory first (the EP's `LoadLibrary` path), so
  "beside `onnxruntime.dll`" is the layout; `auto` gates on exactly that file so a CPU-only
  runtime never tries the EP. Unverified here: a Windows box tells.
- CUDA runs f32: the target is a Maxwell sm_50 card (no native f16; candle's f16 kernels
  need sm_53+). If `cargo build --features decision/cuda` fails on the box, the likely cause
  is `candle-kernels` compiling f16 code for sm_50 — not this branch.
- `models.accelerator: cpu` also forces candle to the CPU on a Mac (an explicit `cpu` should
  mean CPU). The default config path is unchanged.
