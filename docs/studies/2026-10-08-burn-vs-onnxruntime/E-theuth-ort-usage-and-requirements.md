# Appendix E — theuth ORT usage inventory and requirements (child `theuth-ort-inventory`, 2026-10-08)

Read-only inventory of `/Users/Sky.Moore/Develop/theuth` at main cd4c8b5c. Short forms: `PRS/`, `WKV/`, `ORT/` = `~/.cargo/registry/src/*/{parakeet-rs-0.3.7, wavekat-vad-0.1.17, ort-2.0.0-rc.13}/`; `onnx.rs`, `accelerator.rs` = `crates/models/src/`; `stt.rs`, `live.rs`, `vad.rs`, `dictation.rs` = `crates/voice/src/`; `diarize.rs` = `crates/meeting/src/post/`; `models.rs` = `crates/desktop/src/`; `step-12/13` = `docs/design/cross-platform/step-1N-*-as-built.md`; `02`, `04`, `07` = `docs/design/meetings/{02-live,04-post-nemotron-as-built,07-live-diarization}.md`; `45`, `54`, `57`, `58` = `docs/design/<n>-*.md`.

## Summary
- One process-wide ORT dylib (`ort =2.0.0-rc.13`, features `load-dynamic, half, coreml, api-22`; Cargo.toml:116), loaded lazily by `models::onnx::ort_lib` at the first model load, never at daemon start (onnx.rs:96-133). Dependents: models, voice, meeting, decision (optional spike), parakeet-rs 0.3.7, wavekat-vad 0.1.17.
- Five production session kinds — two built by theuth, two by parakeet-rs, one by wavekat-vad. The decision oracle is candle, not ORT.
- Every model is a runtime file under `<data_dir>/models/<name>/` listed in `models.json` (size + sha256), fetched by the desktop at start and swapped atomically; the ORT dylib and WebGPU plugin ship through the same manifest; a model update is a file + manifest row, no rebuild (scripts/fetch-models.sh:44-68; scripts/release-models.sh:14-27; models.rs:1-16).
- GPU: CPU EP by default everywhere; WebGPU plugin EP only for `auto` on a discrete NVIDIA/AMD GPU (onnx.rs:315-345); DirectML explicit-only and dead; CUDA EP never registered; CoreML EP never registered in production.
- Inherited constraints: no second protobuf in the process (ptts/sentencepiece protobuf-lite 3.14 vs static ORT's 3.21 aborted the TTS load — Cargo.toml:112-114; 45:128-140); caller-owned streaming state every 560 ms / 16 ms / 6.4 s; dynamic time dims; ≈0.9 GB int8 sessions resident (dictation.rs:9), 1.75 GB TDT (45:85-87), 2.54–2.59 GB RSS while recording (02:514); a failed runtime load degrades, never crashes (onnx.rs:49-54).
- Effective runtime floor is ORT **1.27**: theuth asks `api-22` but wavekat-vad pulls `ort` with default features (`api-27`), which is the mismatch measured on build-B in September.

## Session table (f32 unless noted; varying dims in bold; threads intra/inter; L3 = `GraphOptimizationLevel::Level3`)

| # | where · owner | files | inputs as fed | state across runs | threads/opt | `run` cadence | numbers |
|---|---|---|---|---|---|---|---|
| 1 | `voice::Engine` stt.rs:170-183 · parakeet-rs `ParakeetTDTModel` | `parakeet-tdt/encoder-model.onnx` 902 MB int8, `decoder_joint-model.onnx` 17 MB, `vocab.txt` | enc `audio_signal` [1,128,**T**], `length` i64[1]; dec per frame `encoder_outputs` [1,1024,1], `targets` i32[1,1], `target_length`, `input_states_1/2` [2,1,640] | LSTM h/c within one piece; fresh per call | 4/1, L3 + `configure` | 1 enc run per ≤ 180 s piece; ≤ 10 dec runs per frame | load 0.7–0.9 s Mac / 4.6 s Win; 5 s clip 190–199 ms Mac, 610–627 ms Win; RSS 1.75 GB; unload after 600 s idle |
| 2 | `voice::stt::Stream` stt.rs:237-266 (dictation/talk; 2 per meeting) · parakeet-rs `NemotronModel` | `nemotron-streaming-en/encoder.onnx` 880 MB int8, `decoder_joint.onnx`, `tokenizer.model` | `processed_signal` [1,128,65] (9 pre-encode + 56 mel), `processed_signal_length`, `cache_last_channel` [24,1,70,1024], `cache_last_time` [24,1,1024,8], `cache_last_channel_len` | 3 encoder caches + 2 LSTM states persist; `reset()` at a turn | 4/1, L3 | 1 enc step per 560 ms window (8,960 samples) | first partial 0.6–1.1 s; 2 sessions load 1.99 s; ≈0.9 GB each; never idle-unloaded |
| 3 | `voice::vad::Vad` vad.rs:21-61 · wavekat-vad `TenVad` | `ten-vad.onnx` `include_bytes!` at build (downloaded by wavekat's build.rs unless `TEN_VAD_MODEL_PATH`) | `input_1` [1,3,41] (40 mel + pitch × 3 frames), `input_2/3/6/7` [1,64] | 4 hidden states + preprocessor buffers; `reset()` at turns | 1 intra, default opt, bypasses `configure` | 1 run per 16 ms frame | n/d |
| 4 | `diarize::Streamer` diarize.rs:483-575 (offline + live) · theuth | `nemotron-diarization/model_quantized.onnx` 356 KB + `.onnx_data` 115 MB, 8-bit MatMulNBits, opset 21 | `input_features` [1,**M**,128], `cached_embeds` [1,**C**,512] (C=0 first), `attention_mask` i64[1,**C+⌈M/8⌉**] | speaker cache + FIFO kept by caller | offline default intra; live 4; L3 | offline 1 run per 27.2 s + 3.2 s look-ahead; live per 6.4 s + 1.6 s | live RTF 0.035, step p50/p95 225/273 ms @ 8 thr, ~300 MB; offline 11.75 min in 8.6 s |
| 5 | `diarize::Embedder` diarize.rs:656-703 · theuth | `pyannote/wespeaker_en_voxceleb_CAM++.onnx` | `feats` [1,**T**,bins] knf-rs fbank, T tiled to a multiple of 200 frames | none | default intra, L3 | once per speaker per post pass, ≤ 30 s audio | "under a second per speaker" |

Concurrency: a recording meeting holds 2×(#2+#3) + live #4; its post pass loads #1, #4, #5; dictation/talk holds another #2+#3. All inference runs in `spawn_blocking` behind `parking_lot::Mutex` slots.

## Third-party ORT dependents
- **parakeet-rs 0.3.7** (MIT OR Apache-2.0): theuth calls `ParakeetTDT::from_pretrained`, `transcribe_samples`, `Nemotron::from_pretrained`, `transcribe_chunk`, `reset`, `ExecutionConfig::with_custom_configure`. Without ORT theuth would re-implement or vendor: feature extraction (`audio.rs`, 332 lines), TDT greedy decode with duration skips, token→text + timestamps (~875 lines), Nemotron chunking/RNNT loop/SentencePiece parser (`nemotron.rs`, 1,009 lines).
- **wavekat-vad 0.1.17** (Apache-2.0; `ort = "2.0.0-rc.12"` with defaults ON): TEN-VAD preprocessing (961 lines), the 4-state recurrence, `FrameAdapter` (926 lines), the build-time download.

## Distribution and platforms
- Manifest `https://s3.rwx.dev/theuth/releases/models/models.json` (`THEUTH_MODELS_URL` overrides): `{files:[{path,size,sha256,revision?,origin?,platform?,key?}]}`; 20 REQUIRED files; `fetch-models.sh --check`; `release-models.sh` uploads models, ORT 1.27.0 per platform, WebGPU plugin files, models.json last. macOS bundles the dylib codesigned in `Contents/Frameworks`; Windows bundles `onnxruntime.dll`; Linux bundles nothing.
- With compiled-in graphs: `.bpk`/`.onnx` weights still flow through the manifest; a graph change becomes an app release; `onnxruntime/*` rows, `VERSION`, `ort-version.env`, `find_ort_lib`, `platform::ort` and the bundle/codesign steps go away.
- Hosts: macOS 15+ aarch64/x86_64, Windows 10 22H2+ x86_64/aarch64, Linux glibc ≥ 2.35 x86_64/aarch64; releases build natively on two ssh build hosts; Android recon names the dlopen'd ORT as the first embedded-core blocker.
- Measured numbers: Parakeet 5 s silence Mac CPU 190–199 ms vs built-in WebGPU/Metal 206–275 ms (+738 ms compile); Windows CPU 610–627 ms vs WebGPU 523–530 ms (+1,650 ms compile) on a 2015 Maxwell.

## Requirements R1..R22

| id | level | requirement | why / cite |
|---|---|---|---|
| R1 | MUST | Weights load at run time from `<data_dir>/models/<name>/`, verified by size+sha256, swappable while the daemon runs; weights-only updates need no rebuild | models.rs:1-16; fetch-models.sh:44-68 |
| R2 | MUST | Compiled-in graphs carry a graph↔weight-file compatibility check at load with a path-bearing error | stt.rs:179-180,245-250 |
| R3 | MUST | Dynamic time dimension per call: TDT T up to 180 s; diariser M and C vary (C=0 first); wespeaker T in multiples of 200 | model_tdt.rs:121-139; diarize.rs:520-538 |
| R4 | MUST | Caller-owned streaming state (Nemotron 3 caches + 2 LSTM states per 560 ms, TEN-VAD 4×[1,64] per 16 ms, TDT LSTM per frame, diariser cache/FIFO); explicit `reset()` | model_nemotron.rs:171-201; ten_vad.rs:44-45 |
| R5 | MUST | Op families: 8-bit MatMulNBits + external `.onnx_data`, int8 SmoothQuant Parakeet encoder, int8 Nemotron export, LSTM-state decoders, Sortformer attention at opset 21 | 04:46,66-67; 45:141-149; 54:167 |
| R6 | MUST | Latency ≤ today's CPU EP: Parakeet 5 s ≤ ~200 ms Mac / ~620 ms Win; Nemotron step ≪ 560 ms; live diariser p95 ≤ ~280 ms; VAD 62.5 runs/s per stream | step-13:218,247; 02:516-518; 07:77 |
| R7 | MUST | Memory ≤ today: ≈0.9 GB per streaming session, 1.75 GB TDT; int8/8-bit weights (fp32 pairs rejected at ~5 GB) | dictation.rs:9; 45:86-87; 02:47-49 |
| R8 | MUST | ~7 sessions alive at once, inference on blocking threads behind `Mutex` slots, `Send` sessions, per-session intra-thread caps | meeting/src/lib.rs:1960-1967; stt.rs:168-170 |
| R9 | MUST | No protobuf (or other C++ runtime) symbol conflict with ptts/sentencepiece-sys | Cargo.toml:112-114; 45:128-140 |
| R10 | MUST | Graceful degradation: missing/failed engine → `Err`, never a panic or poisoned global; status answers before any load | onnx.rs:49-54,153-164 |
| R11 | MUST | Builds on aarch64-apple-darwin, x86_64/aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc in the native-host release flow | target.rs:44-54; .cargo/config.toml:10-15 |
| R12 | MUST | Licence shippable (today ort, parakeet-rs MIT/Apache-2.0; wavekat-vad Apache-2.0; Parakeet CC-BY-4.0) | — |
| R13 | SHOULD | GPU offload per OS ≥ today's opt-in, one `models.accelerator` knob + adapter override, device name in status/bench | accelerator.rs:12-33; onnx.rs:315-345 |
| R14 | SHOULD | fp16 optional (no production session uses f16 I/O) | — |
| R15 | SHOULD | Lazy load on first use (≈0.7–2 s), TDT unload after `stt_idle_secs`, first-run compile off the request path | stt.rs:290-306; step-13:219,248 |
| R16 | SHOULD | parakeet-rs front-end/decode/tokeniser/timestamps (≈2,900 lines) and TEN-VAD preprocessing (≈1,900 lines) reused or re-implemented with parity tests | — |
| R17 | SHOULD | Replace the build-time network fetch of `ten-vad.onnx` with a manifest-distributed weight | WKV/build.rs:125-171 |
| R18 | SHOULD | Keep `t --bench-models` lines/exit codes, `fetch-models.sh --check`, the `models.json` shape | bench_models.rs:1-6 |
| R19 | SHOULD | Binary growth bounded and measured (ORT adds 0 B today; 15–32 MB dylib shipped separately) | 45:139-140 |
| R20 | NICE | Android/iOS embedded core without a dlopen'd runtime beside the models | recon-android.md:15,61,258 |
| R21 | NICE | Future CoreML/NNAPI-class offload without ORT-CoreML-style partitioning failures | — |
| R22 | NICE | Cross-compiling Linux/Windows artifacts from macOS | 58:218-221 |

## Tests and gates a swap must keep green
`crates/models/tests/ort_load_failure.rs`; onnx.rs:664-842 unit tests (status-before-init, version gate, adapter classification fixtures); stt.rs:422-480 and live.rs:305-352 model tests (skip without checkpoint/dylib); pure tests in vad.rs, diarize.rs, diar.rs, models.rs; `t --bench-models` output contract; `just fetch-models --check`, `just release-models`.
