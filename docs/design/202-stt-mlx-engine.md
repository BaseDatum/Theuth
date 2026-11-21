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

Explicit only: RPC `voice_mlx_download` (`mlx::install`), never on a dictation path. For each file in `voice_mlx::FILES` (`config.json`, `model.safetensors`): `HEAD https://huggingface.co/<repo>/resolve/main/<file>` without following the redirect (LFS files answer `x-linked-size` and `x-linked-etag` = sha256; others `content-length`), then `GET` into `<file>.part` — resumed with `Range` when a `.part` exists and the server answers 206 — size and (LFS) sha256 checked, renamed into place; `.complete` written last. Progress: `voice_mlx_status` → `downloading{done_bytes,total_bytes}`. States: `unavailable{reason}` (not in this build) | `absent` | `downloading` | `ready` | `failed{reason}`.

## 5. Config and fallback

`voice.stt_engine: onnx | mlx`, default `onnx` (`#[serde(default)]` section: old configs unchanged).

- `onnx`: no engine object; `Utterer` buffering and finals exactly as before.
- `mlx` in a build without MLX: warning at start, ONNX.
- `mlx` not downloaded: warning at start, ONNX until `.complete` appears (checked per call; no restart needed).
- Live finals: MLX error, `Failed` engine, still loading, or no text within 2.5 s + 0.5 × utterance seconds → warning, the streaming final. An empty MLX text never replaces a non-empty streaming final.

## 6. Build

`voice`'s cargo feature `mlx` pulls `voice-mlx`; off by default until that crate builds. `TODO(202)`: turn it on by default for `cfg(all(target_os = "macos", target_arch = "aarch64"))` once `crates/voice-mlx` is green. Without it, `mlx::compiled()` is false: the state is `unavailable`, `select` falls back to ONNX.

## 7. Not done

- Settings UI ("Speech model" choice, Download button, progress) and regenerated TS bindings; the openrpc fixture does not list `voice_mlx_status` / `voice_mlx_download` yet.
- `voice.stt_engine` has no live setter: config edit + restart.
- The meeting transcript of record and word-timed transcription stay on ONNX (§2).
