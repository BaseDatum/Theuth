# 202 — MLX speech-to-text engine

An optional whole-utterance STT engine: Parakeet TDT 0.6B v3 (`mlx-community/parakeet-tdt-0.6b-v3`, the published weights, cast to bfloat16 by `parakeet_mlx.from_pretrained` exactly as the reference project does) on the Apple GPU through MLX, beside the built-in ONNX int8 engine (45 §4.2). Apple-silicon macOS only.

## 1. What was built

- `crates/voice/sidecar/parakeet_mlx_server.py` — the sidecar, embedded with `include_str!` and written to `<data_dir>/engines/parakeet-mlx/server.py` before each spawn.
- `crates/voice/src/mlx.rs` — config enum, state, explicit install, the sidecar client, the live-final hook.
- `stt::Transcriber` (voice notes, `transcribe` RPC) uses MLX when selected and installed.
- `live::Utterer` (dictation, live talk, live meeting text) replaces each utterance's final with MLX text.
- RPC `voice_mlx_status`, `voice_mlx_download` (`crates/talk/src/rpc_hub.rs`).

## 2. Protocol

Newline-delimited JSON over the sidecar's stdin/stdout; stderr is log only (forwarded at DEBUG).

- `server.py serve <model_dir>`: loads from the local dir (`HF_HUB_OFFLINE=1`), runs one 0.5 s warm clip, prints `{"ready": true, "model", "backend"}` once, then serves requests serially on its main thread (MLX state is thread-local):
  - `{"id", "op": "warm"}` → `{"id", "text": "", "ms"}`
  - `{"id", "op": "transcribe", "pcm16_b64", "sample_rate": 16000, "words"}` → `{"id", "text", "ms", "words"?: [{text, start, end}]}` (token times in seconds)
  - any failure → `{"id", "error"}`; EOF on stdin ends the process.
- `server.py download <model_dir> <repo>`: `snapshot_download` into `<model_dir>`, `{"done_bytes", "total_bytes"}` every second, sha256 of `model.safetensors` checked against the hub's LFS hash, `<model_dir>/.complete` written last, `{"complete": true}` or `{"error"}`.

Audio goes inline as base64 pcm16 (no temp files, no ffmpeg: `parakeet_mlx`'s own file loader shells out to ffmpeg).

## 3. Lifecycle (`mlx::Client`)

- Spawned lazily: on the first whole-clip request (blocking, load timeout 180 s), or in the background at plugin start when installed, at `Utterer::open`, and at each utterance onset.
- The live final never waits for a load: sidecar not up → start it, keep the streaming final this time.
- A request that does not answer within its timeout kills the process (next use respawns). A process that dies mid-request is restarted once and the request retried; a second death is an error.
- Dropped after `voice.stt_idle_secs` unused (the ONNX engine's setting; `0` keeps it).
- `Sidecar` kills and reaps its child on drop; the sidecar also exits on stdin EOF, so a crashed daemon leaves no orphan. The venv's python is exec'd directly (no `uv run` wrapper at serve time), so the kill reaches the process holding the model.

## 4. Download, state

Nothing is fetched implicitly. `voice_mlx_download` (→ `mlx::install`) creates `<data_dir>/engines/parakeet-mlx/` with `uv venv --python 3.12` and `uv pip install 'parakeet-mlx>=0.7.0' huggingface_hub`, then runs the `download` mode into `<data_dir>/models/parakeet-tdt-mlx/`. `voice_mlx_status` (`mlx::state`): `unavailable{reason}` (not Apple silicon; `uv` not on PATH — never installed for the user) | `absent` | `downloading{done_bytes,total_bytes}` | `ready` | `failed{reason}`. Ready = venv python + `.complete`.

## 5. Config and fallback

`voice.stt_engine: onnx | mlx`, default `onnx` (`#[serde(default)]` on the section: old configs unchanged).

- `onnx`: no engine object; `Utterer` keeps exactly today's buffering and finals.
- `mlx` on a non-Apple-silicon build: warning at start, ONNX.
- `mlx` not downloaded: warning at start, ONNX until `.complete` appears (checked per call, so a download made while running takes effect without a restart).
- Whole clips: any MLX error → warning, the same clip on ONNX.
- Live finals: the utterance's buffered 16 kHz audio from its start (pre-roll included) goes to MLX; failure or no answer within 2.5 s + 0.5 × utterance seconds → warning, the streaming final. An empty MLX text is taken as is (the whole-utterance model is the authority). Rolling pieces stay on the streaming model.

## 6. Not done

- Meeting transcript of record (`meeting/src/post`, `meeting/src/stt.rs`) and `TranscribeWords` stay on ONNX: they need word times; the sidecar returns token times and `mlx::join_words` groups them, but that caller is not switched.
- Desktop settings UI ("Speech model" choice, Download button, progress) and regenerated TS bindings: not built; the RPC methods exist.
- Setting `voice.stt_engine` needs a config edit and restart; no live setter.
- `ponytail:` a timed-out live final kills the sidecar (spec: kill on a hung request); a machine that is merely slow pays a reload on the next utterance.
- Pure-Rust route not built: candidate is porting the model to `mlx-rs` (Rust bindings to MLX) — needs the FastConformer encoder + TDT decoder reimplemented; the sidecar was the scoped choice.

## 7. Measured (M-series Mac, `/tmp` install)

See the integration test `mlx::tests::real_sidecar_transcribes_a_clip` (`#[ignore]`, `THEUTH_MLX_DATA_DIR`, `THEUTH_MLX_CLIP`). Manual sidecar run on a 20.5 s history clip: model load 0.4 s (warm page cache), transcribe 705 ms.
