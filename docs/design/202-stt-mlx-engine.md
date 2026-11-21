# 202 — MLX speech-to-text engine

An optional whole-utterance STT engine: Parakeet TDT 0.6B v3 (`mlx-community/parakeet-tdt-0.6b-v3`, the published safetensors) on the Apple GPU through MLX, in-process, beside the built-in ONNX int8 engine (45 §4.2). One binary: no Python, no helper process. Apple-silicon macOS only.

## 1. Route

No maintained Rust crate runs Parakeet TDT on MLX (`parakeet-rs` is ONNX Runtime only; MLX Parakeet exists only as the Python `parakeet-mlx` / `mlx-audio`). The model is a Rust port of `parakeet-mlx` 0.7's inference path onto `mlx-rs`, in its own crate `crates/voice-mlx` (`ParakeetMlx::{load, warm, transcribe}`, `FILES`, `available()`; stubbed with no MLX dependency off macOS aarch64; its README covers the metallib and linked frameworks). This note covers the voice-crate side: engine thread, selection, fallback, download, where it is used. (A first cut ran `parakeet-mlx` in a Python sidecar — `97400c901`; replaced by this design. Its one measurement stands as the reference: a 20.5 s dictation clip in 705 ms on the GPU, model load 0.4 s warm.)

## 2. Where it is used

- `stt::Transcriber` (voice notes, `transcribe` RPC): each ≤ 180 s piece on MLX when selected and ready; any MLX error → warning, the same clip on ONNX.
- `live::Utterer` (dictation, live talk, live meeting text): rolling pieces stay on the streaming Nemotron model; when an utterance closes, its buffered 16 kHz audio from the utterance start (pre-roll included) is transcribed once on MLX and that text is the `is_final` text.
- Not switched: the meeting transcript of record (`meeting/src/post`, `meeting/src/stt.rs`) and `TranscribeWords` — they need word times; `voice_mlx::Transcript::words` has them, the callers are not moved yet.

## 3. Engine thread (`mlx::Engine`)

MLX state is thread-bound. One engine owns one OS thread (`voice-mlx`) that builds the model through a loader closure (real: `voice_mlx::ParakeetMlx::load(<data_dir>/models/parakeet-tdt-mlx)`), warms it, then serves jobs from a channel; each job carries its own reply channel.

- Lazy: started by the first call, by `prewarm` (plugin start when installed, `Utterer::open`, each utterance onset).
- Whole clips wait for a load (120 s + 2 × the live deadline); a live final never does — a loading engine fails that final at once.
- Deadline: the caller stops waiting; the thread finishes the job and its result is dropped. The engine stays.
- Failure: a load error, a load or transcribe panic (caught on the thread), or `MAX_FAILURES` (3) transcribe errors in a row mark the engine `Failed` (sticky until restart); every caller then falls back.
- Idle: after `voice.stt_idle_secs` with no job the thread exits and drops the model; the next call starts a new one (`0` = never).

## 4. Download

Explicit only: RPC `voice_mlx_download` (`mlx::install`), never on a dictation path. For each file in `voice_mlx::FILES` (`config.json`, `model.safetensors`): `HEAD https://huggingface.co/<repo>/resolve/main/<file>` without following the redirect (LFS files answer `x-linked-size` and `x-linked-etag` = sha256; others `content-length`), then `GET` into `<file>.part` — resumed with `Range` when a `.part` exists and the server answers 206 — size and (LFS) sha256 checked, renamed into place; `.complete` written last. Progress: `voice_mlx_status` → `{ engine, available, state }` with `state` = `downloading{done_bytes,total_bytes}`. States: `unavailable{reason}` (not in this build, or no `mlx.metallib` — §6) | `absent` | `downloading` | `ready` | `failed{reason}`; `available` is `state != unavailable`, `engine` the `voice.stt_engine` in effect.

## 5. Config and fallback

`voice.stt_engine: onnx | mlx`, default `onnx` (`#[serde(default)]` section: old configs unchanged).

- `onnx`: no engine object; `Utterer` buffering and finals exactly as before.
- `mlx` in a build without MLX (or without `mlx.metallib`): warning at start, ONNX; the engine's load fails before MLX is touched and the engine is `Failed`.
- `mlx` not downloaded: warning at start, ONNX until `.complete` appears (checked per call; no restart needed).
- Run-time switch: RPC `set_stt_engine(onnx|mlx)` writes `voice.stt_engine` and calls `mlx::switch`, which re-runs `select` with the `stt_idle_secs` the plugin started with: the live-final slot (`mlx::live`) and the voice-note slot (`mlx::current`, read per clip by `Transcriber`) both change at once. An utterance or clip already running keeps the `Arc` it took at its start. Same engine → no-op (the loaded model survives). Voice disabled (plugin not started) → only the choice is recorded.
- Live finals: MLX error, `Failed` engine, still loading, or no text within 2.5 s + 0.5 × utterance seconds → warning, the streaming final. An empty MLX text never replaces a non-empty streaming final.

## 6. Build and the Metal library

No feature flag. `voice` depends on `voice-mlx` on every target; `voice-mlx` pulls `mlx-rs` only under `cfg(all(target_os = "macos", target_arch = "aarch64"))` and stubs itself elsewhere (`available() == false`, every load fails), so `talk`, `meeting`, `desktop`, `theuth`, `tui` need no change and Linux/Windows/Intel builds compile no MLX.

MLX needs `mlx.metallib` at run time. When none loads, MLX throws from its Metal device constructor; through mlx-c and mlx-rs 0.34 that surfaces as a Rust panic on the first array (`Failed to create array: … Can not load metallib from specified location …`, observed 2026-11-21 with `set_metallib_path("/nonexistent/…")`), not a process abort — still, nothing may touch the GPU without the file. `voice_mlx::metal` (crates/voice-mlx/src/metal.rs) therefore looks first, in this order: beside the executable, `Resources/` beside it, `../Resources/` (the sidecar in `theuth.app/Contents/MacOS/t`, the file in `Contents/Resources/`), then the build machine's path (`VOICE_MLX_BUILD_METALLIB`, from voice-mlx's build.rs: `$MLX_RS_METAL_PATH`, else the newest non-JIT `~/.mlx/lib/<key>/`; dev builds and tests). The first hit is pinned with `mlx_rs::metal::set_metallib_path` once per process, inside `ParakeetMlx::load`, i.e. on the engine thread before any MLX call. No hit: `voice_mlx::unavailable_reason()` names the file and every path looked at; `voice_mlx_status` reports `unavailable{reason}`, and a load fails with the same message (engine `Failed`, ONNX).

Packaging (`scripts/package-macos.sh`): the release build runs with `MLX_RS_METAL_PATH=target/release/mlx-metal`, and the script copies `mlx.metallib` into `theuth.app/Contents/Resources/` before codesigning (a data file, sealed with the bundle). A bare `t` (CLI tarball, `just install`) has no metallib beside it and reports the engine unavailable unless the build machine's path exists. Not tested: the packaging run itself, the signed bundle loading the file, and `MACOSX_DEPLOYMENT_TARGET` reaching mlx-sys's cmake (voice-mlx README).

Full metallib, not `metal-jit`: the full kernel set is 136 000 544 bytes (`~/.mlx/lib/<key>/mlx.metallib`, MLX v0.32.2), measured; with it the real-clip test runs 179–269 ms per clip after a 412 ms load. `metal-jit` (~1.5 MB per the mlx-rs docs) compiles kernels from source on first use in every process; its first-transcription cost was **not measured** — it needs a second cmake build of MLX with the feature, which the shared build machine could not spare that night. Decision: ship the full metallib (+136 MB in the DMG before compression) until someone measures JIT's first-use latency; if it is under ~2 s, switch (`mlx-rs = { features = ["metal-jit"] }` in voice-mlx and the same copy step).

## 7. Settings UI

Settings › Voice › Dictation, section "Speech model" (`crates/desktop/ui/src/components/settings/Voice.tsx`): a select "Parakeet TDT v3 — ONNX int8 (built in)" / "Parakeet TDT v3 — MLX full precision (Apple GPU, 2.5 GB download)" (`set_stt_engine`; the MLX item disabled when `available` is false), and an "MLX model" row with the status (`mlxStatusText`: not downloaded / downloading N% / ready / failed: reason / unavailable: reason; "ONNX is used until the MLX model is ready" while MLX is chosen and not ready) and a Download button (`voice_mlx_download`, enabled when absent or failed). `voice_mlx_status` is polled every second while downloading. Choosing MLX before the download finishes is allowed. Help line: "MLX re-transcribes each finished utterance for the final text; the live preview still comes from the streaming model." Same tab: 203's "Shape dictated text" switch.

## 8. Not done

- The meeting transcript of record and word-timed transcription stay on ONNX (§2).
- The TUI has neither switch (`voice.stt_engine`, `voice.shape.enabled`): config edit there.
- `metal-jit` first-use latency unmeasured (§6).
