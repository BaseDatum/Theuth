# CONVERT.md — `onnx2burn` 0.22.0 results

Converter: `bin/bin/onnx2burn` (crates.io burn-onnx 0.22.0, default features = import+mmap, development mode on → `.onnx.txt` dump when parsing succeeds). Each run: `/usr/bin/time -l perl -e 'alarm 1800; exec' onnx2burn <model> gen/<tag>` (tools/convert.sh). Logs: logs/02-convert-<tag>.log (RUST_BACKTRACE=1).

| # | model | outcome | unsupported ops / error | wall | max RSS | .rs bytes / lines | submodules | .bpk bytes | fp32? |
|---|-------|---------|-------------------------|------|---------|-------------------|------------|-----------|-------|
| M1 | parakeet-tdt/nemo128.onnx (140 KB) | **panic** (type inference) | `ArgType::Shape has no DType` in `onnx_ir::node::range::RangeProcessor::infer_types` (Range fed by a Shape-typed input). Same with `--no-simplify`. | 0.07 s | 5 MB | — | — | — | — |
| M9 | ten-vad.onnx (315 KB) | **ok** | — (4 WARN "Input … not found, should only happen when peeking") | 0.09 s | 11 MB | 11 900 B / 318 lines | 0 (not partitioned, 53 nodes < 200) | 307 840 | yes: ONNX fp32 initializer bytes 301 992 (+128 B int) ≈ .bpk 307 840 |
| M7 | pyannote/segmentation-3.0.onnx (6 MB) | **error** (type inference) | `Node 'conv1d1' (Conv1d): Conv1d expects input tensor of rank 3 (N x C x L), got rank 4` — the SincNet first conv lives inside an `If`; its `else_branch` is `Reshape→Conv` with a rank-3 weight on a rank-4 input, and the importer type-checks both branches. | 0.05 s | 15 MB | — | — | — | — |
| M5 | nemotron-streaming-en/decoder_joint.onnx (11 MB) | **unsupported ops** | `model contains 1 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (fails fast; the model also has 3× `DynamicQuantizeLinear`, which is ❌ in SUPPORTED-ONNX-OPS.md and would fail next) | 0.04 s | 12 MB | — | — | — | — |
| M3 | parakeet-tdt/decoder_joint-model.onnx (18 MB) | **unsupported ops** | `  - com.microsoft::DynamicQuantizeLSTM used by 2 node(s)` (also 3× `DynamicQuantizeLinear` ❌) | 0.03 s | 12 MB | — | — | — | — |
| M6 | nemotron-diarization/model_quantized.onnx (364 KB + 120 MB external) | **unsupported ops** | `model contains 3 custom op(s) with no covering inference hook:`<br>`  - com.microsoft::MatMulNBits used by 127 node(s)`<br>`  - com.microsoft::RotaryEmbedding used by 62 node(s)`<br>`  - com.microsoft::SkipLayerNormalization used by 62 node(s)` | 0.08 s | 17 MB | — | — | — | — |
| M8 | pyannote/wespeaker_en_voxceleb_CAM++.onnx (29 MB) | **error** (type inference) | `Node 'averagepool1d1' (AveragePool1d): AveragePool: ceil_mode requires opset 19+, got opset 14` — all 52 `AveragePool` nodes have `ceil_mode=1, kernel 100, stride 100`; onnx-ir `avg_pool1d.rs:86` gates `ceil_mode != 0` on opset ≥ 19 (ONNX itself has `ceil_mode` since opset 10) | 0.36 s | 290 MB | — | — | — | — |
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
| `cargo build --release --bin m9_tenvad` (cold release, after debug) | 1620 s | **9m 08s** (389 crates) | 389 | 2.18 GB | ok; release bin 129.4 MB unstripped |
| `cargo build --bin probe` (debug, incremental, bin only) | 447 s | 6.75 s | 1 | — | ok |
Generated ten-vad.rs compiled unmodified (no fix needed).
Per-model incremental cost: not measurable — only one model converted.

## Step 4 — run & measure (release bin `target/release/m9_tenvad <backend> <bpk> <ref_dir> 20`; one process at a time; log logs/04-run-m9.log)
Only M9 (ten-vad) reached this step. Reference: inventory/ref/M9 (ORT 1.30.0 CPU EP, intra_op 4, `timing_ms.median` = **0.054 ms**, p90 0.057, session_load 6.1 ms). Load = `Model::from_file(.bpk)` + device sync. Each timed run = upload 5 inputs, forward, read back all 5 outputs. Machine load avg 15–31 during runs; flex numbers vary ~2× run-to-run, so three repetitions are listed.

| model | backend | load ms | RSS after load | cold (1st call) ms | median ms | p90 ms | min ms | ORT median ms | parity max abs | max rel | prob got / ref | new-shape first-call ms (warm) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| M9 | flex, default threads (rayon = 16) | 0.40–2.2 | 11.3 MB | 2.3–3.4 | **0.157 / 0.304 / 0.341** | 0.42–0.52 | 0.10–0.17 | 0.054 | 5.4e-7 | 4.2e-2 (on a value ≈1e-5) | 0.828851 / 0.828851 | B=2: 0.55 (0.52); B=3: 0.72 (0.62) |
| M9 | flex, `RAYON_NUM_THREADS=4` (honoured: rayon global pool) | 0.37–0.43 | 11.4 MB | 2.0–2.2 | **0.127 / 0.196 / 0.178** | 0.27–0.38 | 0.10–0.13 | 0.054 | 5.4e-7 | 4.2e-2 | same | B=2: 0.21 (0.19); B=3: 0.25 (0.27) |
| M9 | flex, `RAYON_NUM_THREADS=1` | 0.37 | 11.4 MB | 1.8 | **0.122** | 0.18 | 0.094 | 0.054 | 5.4e-7 | 4.2e-2 | same | B=2: 0.24 (0.21); B=3: 0.28 (0.93) |
| M9 | wgpu (`Device::wgpu(DefaultDevice)`, backend Auto → Metal API, WGSL/auto compiler) | 42–125 | 30.3 MB | **1558 / 2427** | **4.31 / 7.40** | 4.8 / 14.9 | 3.3 / 5.2 | 0.054 | 4.2e-7 | 4.2e-2 | same | B=2: **453 / 3440** (6.4); B=3: **116 / 2351** (5.4) |
| M9 | metal (`Device::metal(DefaultDevice)`, native MSL) | 53–142 | 30.2 MB | **1425 / 2185** | **4.04 / 7.09** | 5.0 / 10.8 | 3.0 / 5.4 | 0.054 | 4.2e-7 | 4.2e-2 | same | B=2: **307 / 3426** (5.2); B=3: **92 / 2109** (6.5) |

Notes
- Argmax: n/a — ten-vad outputs one sigmoid probability ([1,B,1]) plus four LSTM states; the probability matches ORT to 6 decimals (0.828851) and max abs diff over all 257 output values is 5.4e-7 (flex) / 4.2e-7 (wgpu/metal). The 4.2e-2 max *rel* diff is one state element of magnitude ~1e-5 (abs diff 5e-7 / (|ref|+1e-6)); it is noise-level, not a mismatch.
- RSS end-of-process: flex 14 MB; wgpu/metal 84 MB (+ ~100 MB max RSS by `time -l` incl. shader compilation).
- GPU first-call cost is per *shape*: every new B costs 0.1–3.4 s of kernel compilation/autotune (the first process paid 1.5–2.4 s for B=1, then 0.1–0.5 s or 2–3.4 s for B=2/B=3 depending on the run). Warm GPU time (4–7 ms) is ≈30–60× slower than flex CPU (0.12–0.34 ms) and ≈75–130× slower than ORT CPU (0.054 ms) for this tiny model (≈300 KB of weights, 53 ops): per-op dispatch/readback dominates.
- `flex` warm median is 2.3–6× ORT's; thread count barely matters at this size (16 threads under load was the worst).
- Cold (first call) on flex 1.8–3.4 ms vs. ORT's first run not reported in spec.json.
- New-shape check on ten-vad: the only dynamic axis of `input_1` ([B,3,41]) is folded by `reshape([-1,1,80])` into the LSTM *sequence* axis (states stay [1,64]); 0.5× of B=1 is impossible, so B=2 and B=3 were used (ORT and Burn agree on output shape [1,B,1]).
- Release binary: **130 482 712 B unstripped, 79 524 248 B stripped** (`strip`), with flex+wgpu+metal backends, no LTO/opt tweaks beyond `debug=false`.
- Harness self-check: `cargo test --lib` → 1 passed (logs/03-test-harness.log).

## Step 5 — big encoders
M2 and M4 did not convert (CONVERT.md), so nothing to compile or run. No converter memory/time pressure was observed (M4: 1.47 s, 1.57 GB; M2: 0.28 s, 149 MB).

## build-lock queue time (spike total)
install 462 s; debug build 1736 s; release build 1620 s; probe 447 s; release rebuilds 65 s + 333 s + 850 s (+ pending). ≈ 92 min queued vs ≈ 18 min compiling. Longest single stretch 29 min (debug build) — never bypassed.

## Models
- M9 ten-vad.onnx copied from /Users/Sky.Moore/Develop/theuth/target/release/build/wavekat-vad-2aca1730478ece5f/out/ten-vad.onnx (315449 B, sha1 1241f823…) → models/ten-vad.onnx.
