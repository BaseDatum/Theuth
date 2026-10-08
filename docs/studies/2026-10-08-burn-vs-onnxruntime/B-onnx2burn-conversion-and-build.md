# CONVERT.md — `onnx2burn` 0.22.0 results

Converter: `bin/bin/onnx2burn` (crates.io burn-onnx 0.22.0, default features = import+mmap; development mode on by default → `.onnx.txt` graph dump when parsing succeeds). Each run: `RUST_BACKTRACE=1 /usr/bin/time -l perl -e 'alarm 1800; exec @ARGV' onnx2burn <model> gen/<tag>` (tools/convert.sh). Logs were logs/02-convert-<tag>.log (see BUILD.md: the whole build tree was deleted externally before delivery; numbers below were recorded as they were measured).

| # | model | outcome | unsupported ops / error (verbatim) | wall | max RSS | .rs bytes / lines | submodules | .bpk bytes | fp32? |
|---|-------|---------|-------------------------|------|---------|-------------------|------------|-----------|-------|
| M1 | parakeet-tdt/nemo128.onnx (140 KB) | **panic** (type inference) | `panicked at onnx-ir-0.22.0/src/ir/argument.rs:322:34: ArgType::Shape has no DType` — backtrace: `onnx_ir::node::range::RangeProcessor::infer_types` (a `Range` node fed by a Shape-typed input). Identical with `--no-simplify`. | 0.07 s | 5 MB | — | — | — | — |
| M9 | ten-vad.onnx (315 KB) | **ok** | — (4× `WARN onnx_ir::graph_state: Input … not found, should only happen when peeking`) | 0.09 s | 11 MB | 11 900 B / 318 lines | 0 (53 nodes < 200-node partition threshold) | 307 840 | yes — ONNX fp32 initializers 301 992 B (+128 B int64/int32) ≈ .bpk 307 840 B |
| M7 | pyannote/segmentation-3.0.onnx (6 MB) | **error** (type inference) | `Failed to parse ONNX file '…/segmentation-3.0.onnx': Type inference failed: Node 'conv1d1' (Conv1d): Conv1d expects input tensor of rank 3 (N x C x L), got rank 4` — the SincNet first conv sits inside an `If`; the `else_branch` is `Slice→Squeeze→Unsqueeze→Concat→Reshape→Conv` (Conv_135) whose Reshape makes the input rank 4 while the weight `/sincnet/conv1d.0/Concat_2_output_0` is [80,1,251]; the importer type-checks both branches (then_branch converted fine). | 0.05 s | 15 MB | — | — | — | — |
| M5 | nemotron-streaming-en/decoder_joint.onnx (11 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)`<br>`Register hooks via ModelGen::register_custom_op.` (fails fast in phase 2d; the model also has 3× `DynamicQuantizeLinear`, ❌ in SUPPORTED-ONNX-OPS.md) | 0.04 s | 12 MB | — | — | — | — |
| M3 | parakeet-tdt/decoder_joint-model.onnx (18 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (also 3× `DynamicQuantizeLinear` ❌) | 0.03 s | 12 MB | — | — | — | — |
| M6 | nemotron-diarization/model_quantized.onnx (364 KB + 120 MB external) | **unsupported ops** | `model contains 3 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::MatMulNBits used by 127 node(s)`<br>`  - com.microsoft::RotaryEmbedding used by 62 node(s)`<br>`  - com.microsoft::SkipLayerNormalization used by 62 node(s)` | 0.08 s | 17 MB | — | — | — | — |
| M8 | pyannote/wespeaker_en_voxceleb_CAM++.onnx (29 MB) | **error** (type inference) | `Type inference failed: Node 'averagepool1d1' (AveragePool1d): AveragePool: ceil_mode requires opset 19+, got opset 14` — all 52 `AveragePool` nodes have `ceil_mode=1, kernel_shape [100], strides [100]`; onnx-ir `node/avg_pool1d.rs:86` gates `ceil_mode != 0` on opset ≥ 19 (ONNX has `ceil_mode` on AveragePool since opset 10). | 0.36 s | 290 MB | — | — | — | — |
| M4 | nemotron-streaming-en/encoder.onnx (880 MB) | **unsupported ops** | `Type inference failed: Unsupported ONNX operation(s): DynamicQuantizeLinear (node 'dynamicquantizelinear1'), DynamicQuantizeLinear (node 'dynamicquantizelinear2'), …` — the 9.7 KB message lists **170 × `DynamicQuantizeLinear`** (standard domain) and nothing else; `MatMulInteger` (217 nodes) passed. | 1.47 s | 1.57 GB | — | — | — | — |
| M2 | parakeet-tdt/encoder-model.onnx (945 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::MatMulNBits used by 217 node(s)` | 0.28 s | 149 MB | — | — | — | — |

Notes
- M6 external data: **read**. Log showed `DEBUG onnx_ir::proto_conversion: Creating external data ref for '…': file=…/model_quantized.onnx_data, offset=…, length=…` for the 606 external initializers (lazy refs, mmap feature) before the custom-op check aborted.
- M2/M4 never reached codegen; no memory/time pressure (M4 max 1.57 GB / 1.47 s; M2 149 MB / 0.28 s — the mmap loader did not materialise the 945 MB of weights before the custom-op check).
- The converter stops at the first failing phase: phase 2d "Custom-op coverage check" runs before phase 3 "Type Inference", so the M3/M5/M2/M6 lists only contain `com.microsoft` ops; standard-domain gaps (`DynamicQuantizeLinear`) only surfaced for M4, which has no custom-domain ops.
- Exit code 101 for every failure: the CLI reports via `panic!` in `ModelGen::run` (burn-onnx-0.22.0/src/import/model_gen.rs:416).
- Parsed-graph dump (`.onnx.txt`) existed only for M9 (985 583 B).
- Op inventory per model (onnx python, logs/00-onnx-stats.txt): M1 opset 17, 34 nodes incl. STFT/Range/Where; M9 opset 9, 53 nodes (Conv 6, LSTM 2, MaxPool 1); M7 opset 17, 50 nodes (LSTM 4, If 1, InstanceNormalization 4); M5 opset 17+com.microsoft, 54 nodes; M3 63 nodes (+TopK, ReduceLogSumExp); M8 opset 14, 3207 nodes (Conv 225, BatchNormalization 56, AveragePool 52); M6 opset 21+com.microsoft, 749 nodes (MatMulNBits 127, RotaryEmbedding 62, SkipLayerNormalization 62, Gelu 31); M4 opset 17, 6691 nodes (MatMulInteger 217, DynamicQuantizeLinear 170, LayerNormalization 144, Conv 77); M2 opset 17+com.microsoft, 2041 nodes (MatMulNBits 217, LayerNormalization 120, Conv 77).
- Initializer bytes by dtype (fp32 question for a hypothetical .bpk): M3 = 78 904 FLOAT + 11.6 MB UINT8 + 6.6 MB INT8; M5 = 2.67 MB FLOAT + 8.27 MB INT8; M6 = 19.8 MB FLOAT + 100.7 MB UINT8; M4 = 304.8 MB FLOAT + 532.9 MB INT8; M2 = 412.3 MB FLOAT + 532.7 MB UINT8; M7 5.97 MB FLOAT; M8 28.9 MB FLOAT; M1 131.6 KB FLOAT + 4 KB DOUBLE.


# spike-burn — burn-onnx 0.22.0 import spike (BUILD.md)

Machine: MacBook, Apple M3 Max (16 CPU cores, 40-core GPU, 64 GB), shared; load avg 15–35 throughout. Rust 1.98.1. `CARGO_BUILD_JOBS=4`, `CARGO_TARGET_DIR=…/spike-burn/build/target`, every cargo call under `BUILD_LABEL=spike-burn build-lock nice -n 10`.
Inputs: burn-onnx f20ba03 (0.22.0) and burn 20426b9 (0.22.0) clones for reading; **crates.io releases 0.22.0 of burn-onnx / burn / burn-store used for everything** (no path dependency was needed).

> **Data-loss notice.** At ≈14:55 WEST, while the last release rebuild (adapter-name print) was queued on build-lock, the entire `/Users/Sky.Moore/Develop/spike-burn/` tree (build/, inventory/, burn/, burn-onnx/, tract/) was deleted by another process — not by this session (this session never ran `rm` on anything but `gen/`-free paths). `logs/`, `gen/`, `spike/`, `bin/`, `target/` and the sibling's `inventory/ref` are gone. This BUILD.md and CONVERT.md were rewritten into a recreated `build/` directory from the numbers recorded in the session transcript as they were measured; "raw log paths" in (f) therefore name files that no longer exist. Every number below was observed before the deletion.

## (a) Converter build — `cargo install burn-onnx@0.22.0 --locked`
- Bin name `onnx2burn`; default features `import, mmap` include the CLI (no `cli` feature exists). Flags: `--no-simplify --no-partition --no-development --embed-states`; development mode (the `.onnx.txt` graph dump) is on by default.
- Timeline: started 12:20:10; build-lock queue **462 s** (slots 140-W6/140-W2); cargo `Finished release in 3m 17s`; installed 12:31:xx → ≈11 min wall incl. queue. Binary 10 575 464 B.
- Dependencies: **188 "Compiling" lines**; `cargo tree -p burn-onnx --prefix none | sort -u` in a scratch manifest = **178 crates** (191 incl. build-deps). The lockfile also pulled llvm-sys, cubecl-hip-sys, torch-sys, cubecl-metal as downloads (not compiled).
- Memory: `/usr/bin/time -l` summary for this one command was lost (not written to the log); no RSS figure.

## (b) Conversion — see CONVERT.md (full table, verbatim errors)
| model | outcome | unsupported ops | time | RSS | .rs lines | .bpk MB |
|---|---|---|---|---|---|---|
| M1 nemo128 | panic (`ArgType::Shape has no DType`, Range infer_types) | — (importer panic) | 0.07 s | 5 MB | — | — |
| M9 ten-vad | **ok** | — | 0.09 s | 11 MB | 318 (11 900 B, 0 submodules) | 0.31 (fp32) |
| M7 segmentation-3.0 | error: Conv1d got rank-4 input inside `If` else_branch | — (importer type check) | 0.05 s | 15 MB | — | — |
| M5 nemotron decoder_joint | unsupported | `com.microsoft::DynamicQuantizeLSTM` ×2 (+ `DynamicQuantizeLinear` ×3 ❌) | 0.04 s | 12 MB | — | — |
| M3 parakeet decoder_joint | unsupported | `com.microsoft::DynamicQuantizeLSTM` ×2 (+ `DynamicQuantizeLinear` ×3 ❌) | 0.03 s | 12 MB | — | — |
| M8 wespeaker CAM++ | error: `AveragePool: ceil_mode requires opset 19+, got opset 14` (52 nodes, ceil_mode=1) | — (importer opset gate) | 0.36 s | 290 MB | — | — |
| M6 nemotron diarization (quantized, external data) | unsupported | `com.microsoft::MatMulNBits` ×127, `RotaryEmbedding` ×62, `SkipLayerNormalization` ×62 | 0.08 s | 17 MB | — | — |
| M4 nemotron encoder (880 MB) | unsupported | `DynamicQuantizeLinear` ×170 (standard domain) | 1.47 s | 1.57 GB | — | — |
| M2 parakeet encoder (945 MB) | unsupported | `com.microsoft::MatMulNBits` ×217 | 0.28 s | 149 MB | — | — |

**1 of 9 models converts.** M6's external `.onnx_data` was read (606 external-data refs logged). M2/M4 never stressed the converter (< 1.6 GB, < 1.5 s). No conversion hit the 30-min cap.

## (c) Compile — scratch crate `spike/` (edition 2021; `burn = { version = "0.22.0", features = ["flex","wgpu","metal"] }`, `burn-store = "0.22.0"`, `serde_json`; no build.rs; `gen/m9_tenvad/ten-vad.rs` copied verbatim to `spike/src/models/m9_tenvad.rs`; one bin `m9_tenvad` + a throwaway `probe` bin)
burn 0.22 has a runtime-dispatched `Device` (`Device::flex()`, `Device::wgpu(DeviceKind::DefaultDevice)`, `Device::metal(DeviceKind::DefaultDevice)`), and generated models are not generic over a backend type, so **one binary with flex+wgpu+metal** was built and the backend chosen per run (no separate `metal` feature build was needed; `metal` = wgpu pinned to the Metal API with native MSL).

| build | build-lock wait | compile | crates compiled | max RSS (build tree, `time -l`) | result |
|---|---|---|---|---|---|
| `cargo build --bin m9_tenvad` (debug, cold) | 1736 s | **4 min 19 s** | 430 | 2.54 GB | ok. Generated file compiled **unmodified**. Warnings: linker `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets`, one deprecation in my harness. Debug bin 420 841 992 B. |
| `cargo build --release --bin m9_tenvad` (cold release) | 1620 s | **9 min 08 s** | 389 | 2.18 GB | ok. Release bin 129 400 680 B (130 482 712 B after harness edits). |
| `cargo build --bin probe` (debug, incremental, bin only) | 447 s | 6.75 s | 1 | — | ok |
| `cargo build --release --bin m9_tenvad` (incremental, harness edit) ×3 | 65 s / 333 s / 850 s | 7.6 s / 17.0 s / (deleted before it ran) | 1 | — | ok / compile error in my harness (`WgpuDevice::DefaultDevice` is not a variant in cubecl 0.11; fixed) / — |
| `cargo test --lib` (harness self-check, debug) | 0 s | 5.5 s | — | — | 1 passed |

Per-model incremental cost (lines of generated code vs seconds): **not measurable — only one model (318 lines) converted.** The incremental rebuild of the single bin after a harness edit was 7–17 s.

## (d)+(e) Results — M9 ten-vad, release bin `m9_tenvad <backend> ten-vad.bpk inventory/ref/M9 20`
Reference (sibling's `inventory/ref/M9/spec.json`): ORT 1.30.0, CPUExecutionProvider, intra_op 4, ORT_ENABLE_ALL; `session_load_ms` 6.1; `timing_ms` median **0.054**, p90 0.057, min 0.053, max 0.059 (3 warm-ups, 20 runs). Inputs: `input_1` [1,3,41] f32 (RandomState(2) normal), four states [1,64] zeros. Outputs: `output_1` [1,1,1] = 0.8288508653640747, four states [1,64].
Burn measurement: load = `Model::from_file(.bpk)` + `device.sync()`; each timed run = create 5 input tensors on device, `forward`, read back all 5 outputs (`into_data`); first call timed separately as "cold", then 3 warm-ups, 20 timed runs. One process at a time. flex runs repeated 3× because the shared machine made them vary ~2×.

| model | backend | load ms | cold (1st call) ms | median ms | p90 ms | min ms | **ORT median ms** | parity max abs diff | argmax agree | new-length first-call ms (warm at same length) |
|---|---|---|---|---|---|---|---|---|---|---|
| M9 | flex, default threads (rayon pool = 16) | 0.40 / 0.45 / 2.23 | 3.05 / 3.35 / 2.28 | **0.157 / 0.304 / 0.341** | 0.52 / 0.47 / 0.42 | 0.10 / 0.17 / 0.14 | 0.054 | 5.36e-7 | n/a (scalar sigmoid output; prob 0.828851 = ref 0.828851 to 6 d.p.) | B=2: 0.55 (0.52); B=3: 0.72 (0.62) |
| M9 | flex, `RAYON_NUM_THREADS=4` (honoured — flex uses the rayon global pool) | 0.37 / 0.43 / 0.40 | 1.97 / 2.21 / 2.05 | **0.127 / 0.196 / 0.178** | 0.27 / 0.34 / 0.38 | 0.10 / 0.12 / 0.13 | 0.054 | 5.36e-7 | n/a (same) | B=2: 0.21 (0.19); B=3: 0.25 (0.27) |
| M9 | flex, `RAYON_NUM_THREADS=1` | 0.37 | 1.84 | **0.122** | 0.18 | 0.094 | 0.054 | 5.36e-7 | n/a (same) | B=2: 0.24 (0.21); B=3: 0.28 (0.93) |
| M9 | wgpu — `Device::wgpu(DefaultDevice)`, backend Auto (resolves to Metal API on this Mac, auto compiler) | 42 / 125 | **1558 / 2427** | **4.31 / 7.40** | 4.83 / 14.9 | 3.31 / 5.21 | 0.054 | 4.17e-7 | n/a (same) | B=2: **453 / 3440** (6.4 / 7.8); B=3: **116 / 2351** (5.4 / 9.1) |
| M9 | metal — `Device::metal(DefaultDevice)`, native MSL | 53 / 142 | **1425 / 2185** | **4.04 / 7.09** | 4.97 / 10.8 | 3.04 / 5.41 | 0.054 | 4.17e-7 | n/a (same) | B=2: **307 / 3426** (5.2 / 7.3); B=3: **92 / 2109** (6.5 / 10.7) |

Values "a / b" are separate process runs (first run of each GPU backend happened at load ≈31, second at ≈15).

Observations (measured, not interpreted)
- Parity: all 257 output values match ORT to max abs 5.4e-7 (flex) / 4.2e-7 (wgpu, metal); max rel diff 4.19e-2 is a single state element of magnitude ≈1e-5 (rel = abs/(|ref|+1e-6)); probability 0.828851 identical to 6 d.p. Argmax is not applicable (one sigmoid scalar per frame).
- Adapter: the Burn `Device` debug string reports `WgpuDevice { kind: DefaultDevice, backend: Auto }` / `backend: Metal`; it does not expose the adapter name, and a second `init_setup` on the same device panics with `ServiceCreationError "Service already initialized."`. A direct `wgpu::Instance::enumerate_adapters` print was written but its build was deleted before it ran. OS-level identity of the only GPU: `system_profiler` → **Apple M3 Max, 40 cores, Metal 4** — this is the adapter wgpu's `DefaultDevice` resolves to on a single-GPU Mac.
- Shape specialisation: on wgpu/metal **every new input shape costs 0.09–3.4 s on first call** (kernel compilation + autotune), then 5–10 ms warm; on flex the first call at a new shape costs the same as a warm call (0.2–0.7 ms). For ten-vad the only dynamic axis of `input_1` ([B,3,41]) is folded by `reshape([-1,1,80])` into the LSTM *sequence* axis (states stay [1,64]); 0.5× of B=1 is impossible, so B=2 and B=3 were used; ORT and Burn agree on output shape [1,B,1].
- Warm GPU time (4–7 ms) is 30–60× slower than flex CPU (0.12–0.34 ms) and 75–135× slower than ORT CPU (0.054 ms) for this 53-op / 300 KB model. flex median is 2.3–6.3× ORT's.
- RSS: flex 11.3 MB after load, 14 MB at exit; wgpu/metal 30 MB after load, 84 MB at exit, 86–108 MB max RSS by `time -l`.
- Load: flex 0.4 ms (2.2 ms on the first exec after the build) vs ORT session 6.1 ms; wgpu/metal 42–142 ms (includes runtime/device init).
- Binary size (release, flex+wgpu+metal, default profile with `debug=false`): **130 482 712 B unstripped; 79 524 248 B stripped.**
- Threads: flex honours `RAYON_NUM_THREADS`; 1 or 4 threads were faster than the 16-thread default on this loaded machine at this model size.
- One harness bug was hit and fixed during the run (my new-length check initially also scaled the LSTM state batch, which panics in `Lstm::forward_iter` slice_assign — the same input would be rejected by ORT); not a model/importer issue.

## Big encoders (step 5)
M2 and M4 did not convert (MatMulNBits / DynamicQuantizeLinear), so nothing was compiled or run. Converter peak for M4 was 1.57 GB / 1.47 s, for M2 149 MB / 0.28 s.

## (f) Raw logs (paths as they existed under build/ before the external deletion; content not recoverable)
- logs/00-onnx-stats.txt — per-model op histogram / initializer bytes (python onnx)
- logs/01-install.log — cargo install burn-onnx
- logs/02-convert-<tag>.log for m1_nemo128 (+ .bt / .nosimplify), m9_tenvad, m7_segmentation, m5_nemotron_decoder, m3_parakeet_decoder, m8_wespeaker, m6_nemotron_diar, m4_nemotron_encoder, m2_parakeet_encoder; logs/02-convert-big.log
- logs/03-build-debug-m9.log, 03-build-release-m9.log, 03-build-debug-probe.log, 03-build-release-m9-fix{,2,3}.log, 03-test-harness.log
- logs/04-run-m9.log — all RESULT / NEWSHAPE lines above
- gen/m9_tenvad/{ten-vad.rs, ten-vad.bpk, ten-vad.onnx.txt}; spike/ (Cargo.toml, src/lib.rs, src/harness.rs, src/models/m9_tenvad.rs, src/bin/{m9_tenvad,probe}.rs); tools/{convert.sh,cargo.sh,run_m9.sh,onnx_stats.py,deptree/}

## build-lock queue time
install 462 s; debug build 1736 s; release build 1620 s; probe 447 s; release rebuilds 65 s + 333 s + 850 s → **≈ 92 min queued vs ≈ 18 min compiling**. Longest single stretch 29 min (debug build); the lock was never bypassed and no foreign process was touched.
