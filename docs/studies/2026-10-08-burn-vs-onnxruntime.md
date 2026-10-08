# Study: Burn as theuth's ONNX inference provider (vs ONNX Runtime)

- **Date:** 2026-10-08 (spike 12:11–13:40, Sky's M3 Max under load 20–150)
- **Question (Sky):** "find out if we can and should use [tracel-ai/burn] as our onnx inference provider in rust instead of the microsoft bundled binary. seems cleaner to me, native rust."
- **Status:** answered — **keep ONNX Runtime**; revisit under the conditions in §9. This file is the living record; append to §10 as the ecosystem moves.
- **Evidence:** appendices under `docs/studies/2026-10-08-burn-vs-onnxruntime/` (raw child reports); scratch tree `~/Develop/spike-burn/` (clones, models, logs, generated code — not in git). Knowledge: `decisions/theuth-keep-ort-over-burn-onnx-2026-10-08`, `facts/parakeet-rs-burn-metal-vs-ort-fp32-parakeet-tdt-benchmark-2026-10-08`, `facts/burn-onnx-0-22-recon-for-ort-replacement-2026-10-08`, `facts/theuth-onnx-model-op-inventory-vs-burn-2026-10-08`, `facts/theuth-ort-usage-requirements-for-engine-swap-2026-10-08`.

## 1. Verdict

**No — not as the ONNX provider.** Burn is not an ONNX runtime; `burn-onnx` compiles a `.onnx` into Rust source at build time. Run on theuth's nine production graphs its converter accepts **1 of 9** (TEN-VAD). The pure-Rust *runtime* alternative, sonos/tract, runs **2 of 9**. The root cause is the files we ship: ONNX **Runtime** artifacts (ORT's quantiser output and `com.microsoft` fused ops), not neutral ONNX.

The one real opening is narrower: `parakeet-rs` — the crate theuth already uses for Parakeet and Nemotron — merged a Burn backend on 2026-10-08 01:15 UTC (PR #136). Its Metal path transcribes fp32 Parakeet-TDT in **0.135 s per 5 s clip** on this Mac vs **~0.19 s** for the int8 ORT CPU path we ship and **0.57–0.69 s** for ORT CPU on the same fp32 files. Costs: 2.44 GB weights instead of 945 MB, 3–31 s JIT warm-ups, a 15-minute cold compile, +135 MB binary, and code that was 12 hours old when measured.

## 2. Method

Five children on fixed axes plus one follow-up, orchestrated from one session (procedure recorded as `procedures/theuth-inference-engine-swap-spike-five-children-2026-10-08`):

| child | kind | axis | output |
|---|---|---|---|
| burn-recon | read-only | source recon of `tracel-ai/burn` 20426b9 and `tracel-ai/burn-onnx` f20ba03 (both 0.22.0, released 2026-10-06) | §3 |
| theuth-ort-inventory | read-only | every ORT session in theuth, distribution, platforms, constraints → requirements R1–R22 | §4, Appendix E |
| onnx-inventory | shell + python | op histogram of the 9 shipped graphs vs `SUPPORTED-ONNX-OPS.md`, ORT 1.30 reference tensors + timings | §5, Appendix A |
| burn-build | omp | `onnx2burn` 0.22.0 on all 9 files; compile + bench what converts | §6, Appendix B |
| tract-probe | shell | tract 0.23.8 CLI + Rust API on the same files, CPU 1/4 threads and Metal | §7, Appendix C |
| parakeet-burn-bench | omp | parakeet-rs main `examples/burn.rs`: fp32 TDT/Nemotron on Burn Metal vs Burn CPU vs ORT CPU, same binary, same clips | §8, Appendix D |

All builds went through `~/.theuth/bin/build-lock` at `CARGO_BUILD_JOBS=4` under `nice`; benchmark processes ran one at a time at ≤ 4 threads (GPU runs excepted). Load average was 20–150 throughout, so every timing is indicative; single-thread and GPU numbers are the least affected.

## 3. What Burn is (recon of 0.22.0)

- **Build-time code generator, no runtime loader.** `ModelGen` in `build.rs` or the `onnx2burn` CLI: protobuf → `onnx-ir` → simplify → partition (>200 nodes) → Rust `Model` (a burn `Module` with a typed `forward`) + a `.bpk` weights file (burn-pack, lazily read). `scoreboard/README.md`: "a code generator, not a runtime". Nothing loads a `.onnx` graph at run time; downstreams that want weights from the `.onnx` write their own initializer reader (parakeet-rs did, 37 KB, pure Rust).
- **Ranks fixed at codegen, dims dynamic**, but tensor-typed shapes (`Reshape`/`Expand` fed by `Shape` chains) are read back to the host with `.to_data()` on every call — burn-onnx #603 (2026-10-08) measures ~70 syncs ≈ 25 % of a Nemotron chunk on wgpu.
- **Op coverage:** 172 ops ✅ on paper incl. LSTM/GRU/Conv/LayerNorm/Attention/STFT/If/Loop/Scan/TopK/ScatterND; no `com.microsoft` ops (hook via `register_custom_op`, ~45 lines + a kernel per op, weighted ops need `field` + `collect_tensors`); `DynamicQuantizeLinear`, `QLinearConv`, `ConvInteger`, `RMSNormalization`, `RotaryEmbedding` ❌; `QuantizeLinear`/`DequantizeLinear` reject rank-0 (per-tensor) scales. Official ONNX conformance at HEAD: 1,190 pass / 451 skip-codegen / 78 skip-compile / 40 fail-compare of 1,765. External `.onnx_data` is read (mmap). fp16/int8 weights keep their dtype.
- **Backends:** `flex` (pure-Rust CPU: `gemm` + SIMD + rayon, native f16, no BLAS), `cpu` (CubeCL JIT through bundled LLVM), `wgpu` (Metal via native MSL with the `metal` feature, Vulkan, DX12), `cuda`, `rocm`; `ndarray`/`tch` deprecated, candle removed in 0.22. JIT backends compile and autotune kernels per anchored shape bucket, results cached on disk.
- **Maturity:** MIT OR Apache-2.0; Tracel AI; burn minor every 3–5 months with large breaking changes (0.22 removed the `Tensor` backend generic, moved records to burnpack). burn-onnx: 18 open / 168 closed issues, one author on 67 % of human commits. Model tracker #18: Silero VAD ✓, kokoro ✓ (1.3× amplitude drift, 7-minute compile), piper/VITS ✗ (generated code does not compile), "NVIDIA FastConformer — awaiting a re-test".

## 4. What theuth needs from an engine (inventory, condensed — full R1–R22 in Appendix E)

Five production session kinds: Parakeet-TDT (parakeet-rs; encoder + decoder/joint, one run per ≤ 180 s piece), Nemotron streaming (parakeet-rs; 65-frame chunk every 560 ms with three caller-owned cache tensors + two LSTM states; two sessions per meeting, one per dictation/talk), TEN-VAD (wavekat-vad; one 16 ms frame at a time, four [1,64] states), Nemotron-3 Sortformer diariser (theuth; 8-bit MatMulNBits + external data, cache/FIFO outside the graph), wespeaker CAM++ (theuth; once per speaker). All weights arrive at run time from `models.json` (size + sha256, atomic swap, no rebuild); the ORT dylib and the WebGPU plugin travel the same way; `load-dynamic` exists because pyke's static ORT exported protobuf 3.21 symbols that aborted ptts/sentencepiece's protobuf-lite 3.14. Memory ceilings are explicit: ≈0.9 GB per int8 Nemotron session, 1.75 GB TDT, "two fp32 sessions would be ~5 GB" (design 02). GPU today: CPU EP everywhere; WebGPU plugin only for `auto` on a discrete GPU; nothing usable on Apple GPUs.

## 5. Our nine graphs, op by op (ORT artifacts, not neutral ONNX)

| model | file | nodes | opset | quantisation | ops Burn lacks (count) |
|---|---|---:|---|---|---|
| Parakeet-TDT encoder | parakeet-tdt/encoder-model.onnx, 945 MB | 2,041 | 17 + com.microsoft | 8-bit `MatMulNBits` block 32 (SmoothQuant); 412 MB still fp32 | `com.microsoft::MatMulNBits` ×217 |
| Parakeet decoder+joint | decoder_joint-model.onnx, 18 MB | 63 | 17 + ms | ORT dynamic u8; `DynamicQuantizeLSTM` ×2 | `DynamicQuantizeLinear` ×3 ❌, `ms::DynamicQuantizeLSTM` ×2 |
| Nemotron streaming encoder | nemotron-streaming-en/encoder.onnx, 880 MB | 6,691 | 17 | ORT dynamic u8 act × i8 weights (`MatMulInteger` ×217) | `DynamicQuantizeLinear` ×170 ❌ |
| Nemotron decoder+joint | decoder_joint.onnx, 11 MB | 54 | 17 + ms | as Parakeet decoder | `DynamicQuantizeLinear` ×3, `ms::DynamicQuantizeLSTM` ×2 |
| Nemotron-3 diariser | nemotron-diarization/model_quantized.onnx + 120 MB `.onnx_data` | 749 | 21 + ms | 8-bit `MatMulNBits` with zero points | `MatMulNBits` ×127, `ms::RotaryEmbedding` ×62, `ms::SkipLayerNormalization` ×62 |
| nemo128 mel preprocessor | parakeet-tdt/nemo128.onnx, 140 KB | 34 | 17 | — | `STFT` ×1 (absent) |
| pyannote segmentation-3.0 | pyannote/segmentation-3.0.onnx, 6 MB | 50 + 12 (If) | 17 | — | none on paper |
| wespeaker CAM++ | pyannote/wespeaker_en_voxceleb_CAM++.onnx, 29 MB | 3,207 | 14 | — | none on paper |
| TEN-VAD | ten-vad.onnx (wavekat-vad build output), 315 KB | 53 | 9 | — | none on paper |

Union gap: 6 op types / 649 nodes. No runtime (non-initializer) weights anywhere; no fp16; no Loop/Scan; the only `If` is pyannote's SincNet first conv. ORT 1.30 CPU baselines measured the same day (4 intra threads, load 21–26): encoder 211 ms per 5 s clip (182 ms default threads, 574 ms single-thread), Nemotron chunk 82 ms, diariser 236 ms per 10 s, segmentation 23 ms, wespeaker 55 ms, VAD 0.054 ms per frame, decoders 0.14–0.26 ms per step.

## 6. onnx2burn 0.22.0 on those files

Converter: `cargo install burn-onnx@0.22.0` = 178 crates, 3 m 17 s at `-j4` (plus 7.7 min of build-lock queue). Every conversion finished in < 1.5 s and < 1.6 GB RSS because each stopped at its first failing phase; the 945/880 MB encoders never stressed it.

| model | outcome | detail |
|---|---|---|
| TEN-VAD | **ok** | 318 lines of Rust + 308 KB `.bpk` (fp32), compiles unmodified in a spike crate (debug 4 m 19 s cold, 430 crates) |
| nemo128 | **panic** | `ArgType::Shape has no DType` in `RangeProcessor::infer_types` — Range fed by a Shape-typed input; same with `--no-simplify` |
| pyannote segmentation | **type error** | `Conv1d expects input tensor of rank 3, got rank 4` — the SincNet first conv sits in an `If`; both branches are type-checked |
| wespeaker CAM++ | **type error** | `AveragePool: ceil_mode requires opset 19+, got opset 14` — importer gate is wrong (ONNX has `ceil_mode` since opset 10) |
| Parakeet encoder | unsupported | `com.microsoft::MatMulNBits used by 217 node(s)` |
| Parakeet decoder, Nemotron decoder | unsupported | `com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (then `DynamicQuantizeLinear` ❌) |
| Nemotron encoder | unsupported | `Unsupported ONNX operation(s): DynamicQuantizeLinear` ×170 (`MatMulInteger` ×217 passed) |
| Nemotron diariser | unsupported | MatMulNBits ×127, RotaryEmbedding ×62, SkipLayerNormalization ×62 (external data was read) |

Three of the four graphs that are "fully covered" by the op table fail on importer bugs, not on missing ops.

## 7. tract 0.23.8 on the same files (the runtime alternative)

`cargo install tract-cli --features multithread-mm`: 261 crates, 5 m 57 s, 65 MB binary. Every model needed `--onnx-ignore-value-info --onnx-ignore-output-shapes` (symbolic `dim_param`s otherwise fail to unify with concrete inputs).

| model | result | numbers |
|---|---|---|
| wespeaker CAM++ | **runs** | CPU 1 thread 94 ms (API median 95), Metal 85–127 ms (55 `DeviceSyncToHost` round-trips: pooling stays on CPU), parity 3e-5 vs ORT; ORT 55 ms |
| Nemotron encoder | runs, unusable | 2.1–2.8 s per 560 ms chunk CPU, 1.7–2.8 s Metal — 241 `Reduce<Sum>` recompute the MatMulInteger zero-point correction over every int8 weight matrix each run (92 % of time); ORT 82 ms; numerics 5 % of range off ORT |
| TEN-VAD | typed OK, declutter bug | `Downsample over AddAxis: Invalid axis`; unoptimised 1.07 ms vs ORT 0.054 ms, parity 3.6e-7 |
| Parakeet encoder, diariser | ✗ load | `MatMulNBits: only bits=4 and bits=2 are supported (got 8)` |
| both decoders | ✗ analyse | `Unimplemented(DynamicQuantizeLSTM)` |
| nemo128 | ✗ analyse | STFT rank-2 signal rejected (spec says rank 3; ORT accepts 2); patched, then Range rank-1 limit rejected |
| pyannote segmentation | ✗ type | `If` branches of different rank (1,80,7975 vs 1,1,80,7975) |

tract's own NeMo examples (`nemo-parakeet-asr`, `nemo-nemotron-streaming-asr`) do **not** load ONNX: they export fp32 NNEF from the NeMo checkpoint with `torch_to_nnef` (Python + NeMo + torch at export time) and stream by pulsification. Metal on the two runnable graphs was slower than one CPU core.

## 8. The fp32 route measured: parakeet-rs Burn backend (main 3224931, 12 h after merge)

Same crate, same fp32 files (`istupakov/parakeet-tdt-0.6b-v3-onnx`, sha-verified; HF at ~96 MB/s), same clips, `examples/burn.rs` (loads once, transcribes twice; prints load / Run 1 / Run 2). ORT = pyke static ORT 1.28.0, CPU EP, 4 intra threads (parakeet-rs default). 101k lines of burn-onnx-generated code are checked into the crate; weights come from the `.onnx` via a pure-Rust reader.

| clip | Burn **Metal** warm (Run 2) | Burn Metal Run 1 (process-cold, disk cache warm) | Burn **CPU** flex, 16 threads | **ORT CPU fp32** | theuth today, ORT int8 CPU |
|---|---:|---:|---:|---:|---:|
| 5 s synthetic | **0.135 s** | 2.7–3.0 s (31.0 s machine-cold) | 1.8–3.3 s | 0.57–0.69 s | 0.19–0.20 s quiet / 0.18 s loaded |
| 7.4 s LibriSpeech | **0.215 s** | 2.5–3.3 s (19.0 s first time at this length) | 1.4–4.2 s | 0.80–0.95 s | — |
| 30 s synthetic | **0.46 s** | 3.7–4.2 s (19.3 s first time) | — | 2.6–3.0 s | — |
| 7 s after the three above | 0.33 s | 6.3 s (new length bucket) | — | — | — |
| Nemotron streaming, 7.4 s | 1.1–1.75 s total ≈ 90–130 ms per chunk | 5–7 s (19.6 s machine-cold) | 4.8–12 s (slower than real time) | pending | 82 ms per chunk |

Load 1.1–1.7 s Metal / 2.0–2.7 s ORT; max RSS 620 MB Metal (weights live in Metal buffers) vs 2.6 GB Burn CPU vs 2.7–2.9 GB ORT fp32; transcripts identical on all devices. Build: `--no-default-features --features metal` cold **15 m 40 s** at `-j4` (burn-flex 326 s, burn-cubecl 243 s, cubek-matmul 238 s, burn-dispatch 220 s, the generated model code 93–103 s), binary **135 MB** (164 MB with static ORT), target 2.6–3.8 GB. Every new audio-length bucket costs 6–19 s of kernel compile/autotune the first time (disk-cached afterwards); every process pays ~3 s on its first transcription even with a warm cache.

Reading: on Apple silicon the Metal path is 4–6× faster than ORT CPU on fp32 and ~1.4× faster than the int8 CPU path we actually ship, with a third of the RSS. Burn's CPU backend is 2–5× slower than ORT CPU fp32 and 10–20× slower than our int8 path.

## 9. Recommendation and revisit conditions

1. **Keep ORT as the ONNX provider.** Nothing in the pure-Rust ecosystem loads our files; the latency and memory requirements (R6, R7) are met by int8 graphs those engines cannot run.
2. **Track parakeet-rs's Burn backend as a macOS GPU tier**, not a replacement. Revisit when **all** of these hold:
   - [ ] parakeet-rs publishes a release (> 0.3.8) with the `burn`/`metal` features;
   - [ ] burn-onnx #601 (simplifier breaks Conformer rel-pos shift), #602 (Sortformer ScatterElements), #603 (host syncs) are closed;
   - [ ] the per-length-bucket JIT cost (6–19 s) has a warm-up or bucketing story acceptable for voice notes of arbitrary length;
   - [ ] a bounded experiment plan exists: optional fp32 TDT download (2.44 GB) + `models.accelerator: metal` on Apple silicon + startup warm-up, measured on real voice-note length spread, with the 15-minute cold build and +135 MB binary accepted (Sky: "builds already take a while, if it's clearly better maybe we should consider").
3. **If the goal is fewer Microsoft binaries, the lever is export format, not engine:** fp32 or standard-op (QDQ) exports open both Burn and tract — at 2.5× download and memory, which design 02 rejected.
4. **tract is not a route for our files** today: 8-bit MatMulNBits and DynamicQuantizeLSTM unsupported, int8 MatMulInteger path 32× slower than ORT; its ONNX Metal runtime was slower than CPU on both graphs it ran.

## Appendices (same directory, raw child reports as delivered)

- `2026-10-08-burn-vs-onnxruntime/A-onnx-model-inventory.md` — op histograms, gap tables, ORT 1.30 reference data and timings for the nine graphs (`onnx-inventory`).
- `…/B-onnx2burn-conversion-and-build.md` — `onnx2burn` 0.22.0 results per model with verbatim errors, converter build cost, TEN-VAD spike build (`burn-build`).
- `…/C-tract-probe.md` — tract 0.23.8 CLI + Rust API results, Metal profile, pulsification notes (`tract-probe`).
- `…/D-parakeet-rs-burn-vs-ort-bench.md` — parakeet-rs Burn Metal / Burn CPU / ORT CPU on fp32 Parakeet-TDT and Nemotron, build costs, cold/warm/new-length behaviour (`parakeet-burn-bench`).
- `…/E-theuth-ort-usage-and-requirements.md` — every ORT session in theuth, distribution, platforms, constraints, R1–R22 (`theuth-ort-inventory`).
- `…/F-burn-onnx-recon.md` — burn/burn-onnx 0.22.0 mechanism, operator table, backends, maturity (`burn-recon`).

## 10. Changelog

- 2026-10-08 — study written from the spike; pending refinements: TEN-VAD flex/wgpu micro-bench vs ORT 0.054 ms/frame (`burn-build`), Nemotron ORT rows + Sortformer on Metal + `wgpu`-vs-`metal` comparison (`parakeet-burn-bench`). Neither can change §1.
