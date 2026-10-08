# CONVERT.md — `onnx2burn` 0.22.0 results

Converter: `bin/bin/onnx2burn` (crates.io burn-onnx 0.22.0, default features = import+mmap, development mode on → `.onnx.txt` dump when parsing succeeds). Each run: `/usr/bin/time -l perl -e 'alarm 1800; exec' onnx2burn <model> gen/<tag>` (tools/convert.sh). Logs: logs/02-convert-<tag>.log (RUST_BACKTRACE=1).

| # | model | outcome | unsupported ops / error | wall | max RSS | .rs bytes / lines | submodules | .bpk bytes | fp32? |
|---|-------|---------|-------------------------|------|---------|-------------------|------------|-----------|-------|
| M1 | parakeet-tdt/nemo128.onnx (140 KB) | **panic** (type inference) | `ArgType::Shape has no DType` in `onnx_ir::node::range::RangeProcessor::infer_types` (Range fed by a Shape-typed input). Same with `--no-simplify`. | 0.07 s | 5 MB | — | — | — | — |
| M9 | ten-vad.onnx (315 KB) | **ok** | — (4 WARN "Input … not found, should only happen when peeking") | 0.09 s | 11 MB | 11 900 B / 318 lines | 0 (not partitioned, 53 nodes < 200) | 307 840 | yes: ONNX fp32 initializer bytes 301 992 (+128 B int) ≈ .bpk 307 840 |
| M7 | pyannote/segmentation-3.0.onnx (6 MB) | **error** (type inference) | `Node 'conv1d1' (Conv1d): Conv1d expects input tensor of rank 3 (N x C x L), got rank 4` — the SincNet first conv lives inside an `If`; its `else_branch` is `Reshape→Conv` with a rank-3 weight on a rank-4 input, and the importer type-checks both branches. | 0.05 s | 15 MB | — | — | — | — |
| M5 | nemotron-streaming-en/decoder_joint.onnx (11 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (fails fast; the model also has 3× `DynamicQuantizeLinear`, which is ❌ in SUPPORTED-ONNX-OPS.md and would fail next) | 0.04 s | 12 MB | — | — | — | — |
| M3 | parakeet-tdt/decoder_joint-model.onnx (18 MB) | **unsupported ops** | `  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (also 3× `DynamicQuantizeLinear` ❌) | 0.03 s | 12 MB | — | — | — | — |
| M8 | pyannote/wespeaker_en_voxceleb_CAM++.onnx (29 MB) | **error** (type inference) | `Node 'averagepool1d1' (AveragePool1d): AveragePool: ceil_mode requires opset 19+, got opset 14` (importer version gate; ONNX itself has `ceil_mode` since opset 10) | 0.36 s | 290 MB | — | — | — | — |
| M6 | nemotron-diarization/model_quantized.onnx (364 KB + 120 MB external) | **unsupported ops** | `model contains 3 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::MatMulNBits used by 127 node(s)`<br>`  - com.microsoft::RotaryEmbedding used by 62 node(s)`<br>`  - com.microsoft::SkipLayerNormalization used by 62 node(s)` | 0.08 s | 17 MB | — | — | — | — |
| M4 | nemotron-streaming-en/encoder.onnx (880 MB) | **unsupported ops** | `Type inference failed: Unsupported ONNX operation(s): DynamicQuantizeLinear (node 'dynamicquantizelinear1'), … ` — 170 × `DynamicQuantizeLinear` (standard domain), no other op listed; `MatMulInteger` (217) passed. | 1.47 s | 1.57 GB | — | — | — | — |
| M2 | parakeet-tdt/encoder-model.onnx (945 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::MatMulNBits used by 217 node(s)` | 0.28 s | 149 MB | — | — | — | — |

Notes
- M6 external data: yes, read. Log shows `Creating external data ref for '…': file=…/model_quantized.onnx_data, offset=…, length=…` for the 606 external initializers (lazy refs via mmap feature) before the custom-op check aborted.
- M2/M4 never reached codegen; no memory/time pressure (max 1.57 GB, 1.5 s). The mmap loader did not materialise the 945 MB of M2 weights before the custom-op check (149 MB RSS).
- The converter stops at the first failing phase: custom-op coverage check (phase 2d) runs before type inference (phase 3), so M3/M5/M2/M6 lists above only contain `com.microsoft` ops; standard-domain gaps (`DynamicQuantizeLinear`) only surface for M4.
- Parsed-graph dump (`.onnx.txt`, development mode) only exists for M9: gen/m9_tenvad/ten-vad.onnx.txt (985 KB).
- Exit code 101 (panic) for every failure; the CLI reports errors via `panic!` in `ModelGen::run` (model_gen.rs:416).


# spike-burn — burn-onnx 0.22.0 import spike

Machine: MacBook M3 Max (16 cores), shared, load ~35 at start. Rust 1.98.1. `CARGO_BUILD_JOBS=4`, all cargo under `build-lock nice -n 10`.
Inputs: burn-onnx f20ba03 (0.22.0), burn 20426b9 (0.22.0); crates.io releases used for everything unless noted.

## Step 1 — converter build (`cargo install burn-onnx@0.22.0`)
- Bin: `onnx2burn` (default features `import, mmap` include the CLI; no `cli` feature needed).
- CLI flags: `--no-simplify --no-partition --no-development --embed-states`; development mode (the `.onnx.txt` graph dump) is ON by default.
- Started 12:20:10 WEST; queued behind build-lock slots 140-W6 140-W2 for **462 s**; then `Finished release in 3m 17s`; installed 12:31 (binary bin/bin/onnx2burn, 10.6 MB). Total wall ≈ 11 min incl. queue. Log: logs/01-install.log (the `/usr/bin/time -l` summary was lost for this one run; cargo's own timer is authoritative).
- Dependency count: 188 "Compiling" lines in the install log; `cargo tree -p burn-onnx --prefix none | sort -u` (scratch manifest tools/deptree) = **178 crates** (191 incl. build-deps). Downloads included llvm-sys, cubecl-hip-sys, torch-sys, cubecl-metal (lockfile-only, not compiled).

## Step 2 — conversion
See CONVERT.md. Summary: **1 of 9 models converts (M9 ten-vad)**. M1/M7/M8 fail in the importer (Range-on-Shape panic, Conv rank check inside an `If` branch, AveragePool `ceil_mode` opset gate); M3/M5/M2/M6 are blocked by `com.microsoft` ops (DynamicQuantizeLSTM, MatMulNBits, RotaryEmbedding, SkipLayerNormalization); M4 by 170× standard-domain `DynamicQuantizeLinear`. All failures < 1.5 s and < 1.6 GB RSS, so the big encoders never stressed the converter.

## Backend selection note
burn 0.22 uses a runtime-dispatched `Device` (`Device::flex()`, `Device::wgpu(DeviceKind::DefaultDevice)`, `Device::metal(DeviceKind::DefaultDevice)` behind `flex`/`wgpu`/`metal` features). Generated models are not generic over a backend type, so one spike binary with all three features selects the backend per run.

## Step 3 — compile (spike/ crate: burn 0.22.0 `flex+wgpu+metal`, burn-store 0.22.0, serde_json; no build.rs; gen/m9_tenvad/ten-vad.rs copied to spike/src/models/m9_tenvad.rs)
| build | lock wait | compile | crates | max RSS (build tree) | result |
|---|---|---|---|---|---|
| `cargo build --bin m9_tenvad` (debug, cold) | 1736 s | **4m 19s** (430 crates) | 430 | 2.54 GB | ok; 1 deprecation warning in my harness (`TensorData::to_vec`), linker warning `__eh_frame section too large (max 16MB)`; debug bin 421 MB |
Generated ten-vad.rs compiled unmodified (no fix needed).
Per-model incremental cost: not measurable — only one model converted.

## Models
- M9 ten-vad.onnx copied from /Users/Sky.Moore/Develop/theuth/target/release/build/wavekat-vad-2aca1730478ece5f/out/ten-vad.onnx (315449 B, sha1 1241f823…) → models/ten-vad.onnx.
