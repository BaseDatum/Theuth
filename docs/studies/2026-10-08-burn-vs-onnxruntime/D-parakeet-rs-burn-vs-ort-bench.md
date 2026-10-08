# parakeet-rs: Burn (Metal / CPU) vs ONNX Runtime — fp32 Parakeet TDT 0.6B v3 on Apple M3 Max

Measurements only. Workdir: `/Users/Sky.Moore/Develop/spike-burn/parakeet/`. Raw logs: `logs/`.

## 1. Environment

| item | value |
|---|---|
| machine | MacBook Pro, Apple M3 Max, 16 cores, 64 GB, darwin 25.6.0 (shared: load avg 20–38 throughout) |
| toolchain | cargo 1.98.1 (797e8a9bc 2026-08-05), rustc 1.98.1 (48a229cea 2026-09-01), aarch64-apple-darwin |
| parakeet-rs | `git clone --depth 50 https://github.com/altunenes/parakeet-rs` → HEAD `322493105793658eb93c2ab15d5a5fa6b7bbbef5`, 2026-10-08T13:11:07+03:00, "fix ceil_mode opset in generate_burn docstring" (package version 0.3.8, unreleased main after PR #136) |
| Cargo.toml pins | `burn = "=0.22.0"` (features `flex`), `burn-store = "=0.22.0"`, `ort = "2.0.0-rc.13"` (default-features off; feature `ort-defaults` → `ort/default`) |
| Cargo.lock resolved | burn 0.22.0, burn-store 0.22.0, burn-wgpu 0.22.0, burn-cpu 0.22.0, burn-ndarray 0.22.0, cubecl 0.11.0, cubecl-wgpu 0.11.0, cubecl-runtime 0.11.0, wgpu 30.0.1, naga 30.0.1, ort 2.0.0-rc.13, ort-sys 2.0.0-rc.13 |
| features | `default = ["cpu","ort-defaults","api-28"]`; `burn` = CPU; `wgpu`; `metal = ["wgpu","burn/metal"]` (MSL kernels, "much faster on Apple silicon"); `vulkan`; `burn-cuda`; `burn-rocm`; `sortformer`; `multitalker` |
| src/burn_backend/generated/ | 28 files, 100,956 lines, 4,774,104 bytes (4.55 MiB; `du` 4720 KiB). Largest: sortformer.rs 584,243 B, nemotron_multi_encoder.rs 543,871 B, multitalker_encoder.rs 538,851 B, nemotron_encoder.rs 537,013 B, unified_encoder.rs 475,226 B, tdt_encoder.rs 433,689 B, ctc.rs 403,978 B, eou_encoder.rs 396,516 B; the `*_weights.rs` files 3–126 KB each |

Notes from source (verbatim where quoted):
- `src/burn_backend/mod.rs` header: "The model code in `generated/` comes from burn-onnx and is checked in, so building parakeet-rs never converts models. Weights are read from the same `.onnx` files ONNX Runtime uses (see [`onnx`]). Regenerate with scripts/generate_burn.py when upgrading burn, then check with scripts/check_burn_parity.sh." Device probing: `Device::flex()` for BurnCpu, `Device::wgpu(DefaultDevice)` for BurnWgpu; a 1-element zeros tensor is materialised at load so a missing GPU errors at load time; burn panics are caught with `catch_unwind` and turned into `Error::Model`.
- `src/execution.rs` `ExecutionProvider::BurnCpu`: "Uses all cores (`RAYON_NUM_THREADS` limits them); `intra_threads` applies to ONNX Runtime only." `BurnWgpu`: "The first run on a machine compiles and tunes kernels (seconds); they are cached on disk." No cache path is documented in the crate; see §6 for where it actually landed.
- `examples/burn.rs` header: "The first GPU run on a machine compiles and tunes kernels for each new range of audio lengths, which takes seconds. Results are cached on disk, so later runs and later launches are fast: an app can transcribe a short clip at startup to warm up." The example loads the model once (prints `Loaded <path> on <provider> in X.XXs`), then runs transcription twice in-process (`Run 1: X.XXs`, `Run 2: X.XXs`) and prints the Run 2 transcript with sentence timestamps. It prints no per-stage (encoder/decoder) split — only load, Run 1, Run 2. TDT uses `TimestampMode::Sentences`.
- README "Pure-Rust backend (experimental)": "All models except Cohere can also run on burn, without ONNX Runtime, on the CPU or GPU. Same model files (fp32), same output." … "On Apple silicon with `metal`, every model is faster than ONNX Runtime on the CPU."

## 2. Downloads

Load average at download time: 20.06 / 37.66 / 37.96 (`uptime` 12:42 local).

| file | bytes | source | time | check |
|---|---|---|---|---|
| models/tdt/vocab.txt | 93,939 | huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx | 0.21 s | size matches HF content-length |
| models/tdt/decoder_joint-model.onnx | 72,520,893 | same | 1.74 s | sha256 `e978ddf6…8706c1` = HF LFS etag ✔ |
| models/tdt/encoder-model.onnx | 41,770,866 | same | 1.09 s | sha256 `98a74b21…96bb1` = HF LFS etag ✔ |
| models/tdt/encoder-model.onnx.data | 2,435,420,160 | same | 25.44 s (~96 MB/s) | sha256 `9a22d372…24f36` = HF LFS etag ✔ |
| audio/2086-149220-0033.wav | 237,964 | dldata-public.s3.us-east-2.amazonaws.com | 0.81 s | 16 kHz mono 16-bit, 118,960 frames = **7.435 s** |

HF was reachable first try; no retries, no venv fallback needed. Full sha256 lines: `logs/05-tdt-sha256.txt`. Download log: `logs/02-dl-tdt.log`.

Synthetic clips (`gen_audio.py`: `0.3·sin(2π·220t)+0.2·sin(2π·1250t)+0.01·uniform(-1,1)`, `random.seed(0)`, 16 kHz mono 16-bit):

| file | duration | sha256 |
|---|---|---|
| audio/synth5s.wav | 5.000 s (80,000 frames) | `9e2ec4a6…18ad6` |
| audio/synth7s.wav | 7.000 s (112,000 frames) | `5e91a956…7554` |
| audio/synth30s.wav | 30.000 s (480,000 frames) | `01243ca2…194c` |
| audio/say16k.wav | 19.018 s (304,282 frames) | macOS `say` → `afconvert -f WAVE -d LEI16@16000 -c 1`, 60-word English text |

Second/third-priority models (huggingface.co/altunenes/parakeet-rs, `logs/04-dl-nemo.log`, `logs/04b-dl-nemo.log`; the first script was killed by the shell session after encoder.onnx.data finished — the resumed run found that file complete and fetched the rest):

| file | bytes | time | sha256 vs HF LFS etag |
|---|---|---|---|
| models/nemotron/encoder.onnx | 42,159,995 | 1.80 s | `5c5110ca…` ✔ |
| models/nemotron/encoder.onnx.data | 2,436,567,040 | 75.43 s (~32 MB/s) | `44f65771…` ✔ |
| models/nemotron/decoder_joint.onnx | 35,779,240 | <5 s | `8bcfde85…` ✔ |
| models/nemotron/tokenizer.model | 251,056 | <1 s | `07d4e5a6…` ✔ |
| models/diar/nemotron3_diar_v3.onnx | 400,506,656 | ~10 s | `915e4fa2…` ✔ |

## 3. Builds

All builds: `CARGO_BUILD_JOBS=4`, `CARGO_TARGET_DIR=…/parakeet/target`, `BUILD_LABEL=spike-parakeet build-lock nice -n 10 cargo …`, `/usr/bin/time -l` around the build-lock command, `--timings`.

### B1 — `cargo build --release --example burn --no-default-features --features metal` → `bin/burn-metal`

| metric | value |
|---|---|
| start / load | 11:43:14Z, load 24.42 / 36.55 / 37.56 |
| build-lock queue | 65 s ("slot 0 acquired after 65s") |
| cargo wall (cargo's own figure) | **15 m 40 s** (cold target; 641 timing units) |
| `/usr/bin/time -l` on build-lock+cargo | 1006.64 s real, 3207.60 s user, 97.32 s sys; max RSS 2,184,462,336 B (2.03 GiB — the largest single rustc) |
| binary | `target/release/examples/burn` 141,356,360 B (134.8 MiB) |
| target/ after B1 | 2.60 GB |
| parakeet-rs lib compile (generated code; the row in cargo-timing html) | **92.9 s** (starts at t=841 s; `burn` example 6.6 s more) |
| longest units (jobs=4, nice 10, load ~25–37) | burn-flex 325.8 s, burn-cubecl 243.4 s, cubek-matmul 237.7 s, turso_core 230.7 s, burn-dispatch 220.4 s, burn-cubecl-fusion 116.1 s, cubecl-ir 97.4 s, parakeet-rs 92.9 s, pliron 75.3 s, cubek-quant 57.1 s, naga 55.9 s, burn-nn 54.4 s |
| warning | `stripping debug info with rust-objcopy failed: signal: 6 (SIGABRT) … Library not loaded: @rpath/libLLVM.dylib` — local toolchain issue (rust-objcopy missing libLLVM), binary still produced |

Log: `logs/03-build-b1.log`; timings: `target/cargo-timings/cargo-timing-20261008T114420382Z-5bdf13ffd86f494c.html`.

### B2 — `cargo build --release --example burn --features metal,sortformer` (default features on → ONNX Runtime) → `bin/burn-metal-ort`

| metric | value |
|---|---|
| start / load | 12:03:58Z, load ~75 / 59 / 51 at end (13:15 local: "load averages: 75.51 58.73 51.10") |
| build-lock queue | 87 s |
| cargo wall | **9 m 45 s** (same target dir as B1, but burn-flex/burn-cubecl/burn-dispatch/cubek-matmul/parakeet-rs were all recompiled — the default-feature set changes the resolved features of shared deps; 681 timing units) |
| `/usr/bin/time -l` | 673.59 s real, 2148.97 s user, 34.99 s sys; max RSS 2,243,248,128 B (2.09 GiB) |
| binary | `target/release/examples/burn` 171,445,032 B (163.5 MiB; +30.1 MiB over B1 = static libonnxruntime) |
| target/ after B2 | 3,951,556 KiB = 3.77 GB |
| parakeet-rs lib compile | **103.2 s** (with `sortformer` generated code added; starts t=473 s) |
| longest units | burn-flex 294.3 s, burn-cubecl 220.0 s, burn-dispatch 213.1 s, cubek-matmul 212.5 s, burn-cubecl-fusion 103.3 s, parakeet-rs 103.2 s, cubecl-ir 56.0 s, cubek-quant 54.8 s |
| ort crate | ort 2.0.0-rc.13 / ort-sys 2.0.0-rc.13 |
| ORT binary fetched by ort-sys | `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml.tar.lzma2` (sha256 `6934874e…74c7`, from ort-sys `build/download/dist.tsv`) → **ONNX Runtime 1.28.0**, static `libonnxruntime.a` 80,425,592 B, already present in `~/Library/Caches/ort.pyke.io/dfbin/aarch64-apple-darwin/6934874e…/` (dated Sep 17, so no network download during this build); links frameworks Foundation + CoreML; CPU EP used by the example (`ExecutionProvider::Cpu`) |
| errors | none (same rust-objcopy strip warning as B1) |

Log: `logs/06-build-b2.log`; timings: `target/cargo-timings/cargo-timing-20261008T120527072Z-5bdf13ffd86f494c.html`.

## 4. TDT fp32 results (`bin/burn-metal-ort`, example `burn tdt <clip> models/tdt <device>`)

How to read: each **launch** (L1–L4) is a fresh process; the example loads the model once (`Loaded … in`), transcribes the same clip twice in-process (`Run 1`, `Run 2`) and prints the Run 2 transcript. So "Run 1 of L1" = machine-cold (first-ever Metal compile + autotune for that shape range), "Run 1 of L2–L4" = process-cold but disk-cache-warm, "Run 2" = fully warm. Wall and max RSS are for the whole process (`/usr/bin/time -l`). Per-stage (encoder/decoder) timings are not printed by the example. Logs: `logs/run/burn-metal-ort-tdt-<device>-<clip>-L<n>.log`.

### 4.1 `gpu` = `ExecutionProvider::BurnWgpu` (Metal, MSL kernels)

Load avg at start of block: 65.15 / 57.45 / 50.88 (13:15); at end 34.87 / 49.13 / 48.52 (13:17). GPU runs are mostly GPU-bound, CPU load affects them less than the CPU rows below.

| clip | launch | load s | Run 1 s | Run 2 s | wall s | max RSS MB |
|---|---|---|---|---|---|---|
| synth 5 s | L1 (machine-cold) | 1.66 | **31.02** | 0.15 | 35.51 | 618 |
| synth 5 s | L2 | 1.32 | 3.03 | 0.12 | 4.51 | 610 |
| synth 5 s | L3 | 1.08 | 2.68 | 0.10 | 3.94 | 629 |
| synth 5 s | L4 | 1.22 | 2.99 | 0.61 | 4.88 | 695 |
| LibriSpeech 7.435 s | L1 (new length) | 1.18 | **18.98** | 0.22 | 20.44 | 632 |
| LibriSpeech 7.435 s | L2 | 1.09 | 3.33 | 0.21 | 4.69 | 618 |
| LibriSpeech 7.435 s | L3 | 1.09 | 2.50 | 0.19 | 3.84 | 618 |
| LibriSpeech 7.435 s | L4 | 1.10 | 2.86 | 0.23 | 4.24 | 631 |
| synth 30 s | L1 (new length) | 1.21 | **19.32** | 0.47 | 21.07 | 627 |
| synth 30 s | L2 | 1.04 | 3.71 | 0.45 | 5.26 | 712 |
| synth 30 s | L3 | 1.14 | 3.70 | 0.61 | 5.51 | 619 |
| synth 30 s | L4 | 1.27 | 4.21 | 0.45 | 5.99 | 626 |
| synth 7 s (after 5/7.4/30 s) | L1 (new length) | 1.11 | **6.28** | 0.33 | 7.78 | 632 |

Summary (gpu): warm in-process (Run 2) medians: 5 s → **0.135 s** (L1–L3; L4 outlier 0.61), 7.4 s → **0.215 s**, 30 s → **0.46 s**. Process-cold-but-cached Run 1 (L2–L4) median: 5 s 2.99 s, 7.4 s 2.86 s, 30 s 3.71 s — i.e. every new process pays ~2.5–4 s on its first transcription even with the disk cache populated (Metal pipeline creation / autotune lookup), independent of clip length. Machine-cold first run: 31.0 s (5 s clip); each new audio length then cost 19.0 s (7.4 s), 19.3 s (30 s), and 6.3 s (7 s, after those three) on top of the ~3 s process-cold cost — consistent with the example's "compiles and tunes kernels for each new range of audio lengths". Model load ≈ 1.1–1.7 s (2.4 GB weights). Max RSS ≈ 610–712 MB (weights live in Metal buffers, not counted as resident pages of the process).

LibriSpeech transcript on gpu (Run 2, L2–L4 identical):
```
[  0.32s -   4.88s] Well, I don't wish to see it any more, observed Phoebe, turning away her eyes.
[  4.88s -   7.36s] It is certainly very like the old portrait.
```
Synthetic tone clips (5 s, 7 s, 30 s) produce an empty transcript on every device (no tokens emitted) — expected for pure tones; they still run the full encoder.

### 4.2 `cpu` = `ExecutionProvider::BurnCpu` (burn 0.22 `flex` CPU backend, rayon, all 16 logical cores by default)

Load avg during block: 33.68 / 48.64 / 48.36 (13:18) → 61.68 / 54.23 / 50.58 (13:19). These numbers are heavily affected by the other sessions' load (16 rayon threads competing on a 16-core box with load 35–60): run-to-run spread is 2×.

| clip | launch | load s | Run 1 s | Run 2 s | wall s | max RSS MB | user/sys s |
|---|---|---|---|---|---|---|---|
| synth 5 s | L1 | 0.12 | 2.83 | 1.78 | 4.84 | 2618 | |
| synth 5 s | L2 | 0.12 | 2.82 | 2.25 | 5.30 | 2618 | |
| synth 5 s | L3 | 0.13 | 3.03 | 3.28 | 6.55 | 2618 | |
| synth 5 s | L4 | 0.12 | 4.33 | 1.88 | 6.44 | 2618 | |
| LibriSpeech 7.435 s | L1 | 0.12 | 4.77 | 1.42 | 6.42 | 2631 | |
| LibriSpeech 7.435 s | L2 | 0.11 | 2.25 | 2.91 | 5.39 | 2631 | |
| LibriSpeech 7.435 s | L3 | 0.12 | 2.83 | 4.18 | 7.27 | 2634 | |
| LibriSpeech 7.435 s | L4 | 0.12 | 5.32 | 4.19 | 9.76 | 2631 | |
| synth 30 s | L1 | 0.12 | 9.07 | 9.67 | 19.00 | 2867 | 24.55 / 2.34 |
| synth 30 s | L2 | 0.12 | 10.06 | 10.24 | 20.55 | 2864 | |
| synth 30 s | L3 | 0.12 | 8.65 | 9.63 | 18.55 | 2867 | |
| synth 30 s | L4 | 0.12 | 6.80 | 4.79 | 11.81 | 2867 | |

Summary (burn cpu): no cold/warm distinction (Run 1 ≈ Run 2; no JIT). Medians over all 8 runs: 5 s → **2.8 s** (range 1.78–4.33), 7.4 s → **3.5 s** (1.42–5.32), 30 s → **9.4 s** (4.79–10.24). Load is 0.12 s (weights are read lazily/mmapped; the 2.4 GB show up as RSS 2.6–2.9 GB). user/sys for the 30 s L1 process: 24.55 s user over 19.00 s real ≈ 1.3 cores effective — the rayon pool got little of the machine at load ≈ 50–60.

### 4.3 `ort` = `ExecutionProvider::Cpu` (ONNX Runtime 1.28.0 CPU EP; crate default `intra_threads: 4`, `inter_threads: 1`)

Load avg during block: 59.93 / 54.11 / 50.58 (13:20) → 44.96 / 51.25 / 49.75.

| clip | launch | load s | Run 1 s | Run 2 s | wall s | max RSS MB | user/sys s |
|---|---|---|---|---|---|---|---|
| synth 5 s | L1 | 2.40 | 0.62 | 0.69 | 3.79 | 2569 | |
| synth 5 s | L2 | 2.68 | 0.66 | 0.66 | 4.12 | 2571 | |
| synth 5 s | L3 | 2.48 | 0.60 | 0.57 | 3.74 | 2570 | |
| synth 5 s | L4 | 2.18 | 0.57 | 0.59 | 3.45 | 2570 | |
| LibriSpeech 7.435 s | L1 | 2.38 | 0.81 | 0.80 | 4.08 | 2577 | |
| LibriSpeech 7.435 s | L2 | 2.42 | 0.88 | 0.86 | 4.24 | 2576 | |
| LibriSpeech 7.435 s | L3 | 2.42 | 0.95 | 0.84 | 4.29 | 2576 | |
| LibriSpeech 7.435 s | L4 | 2.00 | 0.85 | 0.86 | 3.81 | 2575 | |
| synth 30 s | L1 | 2.01 | 3.00 | 3.05 | 8.13 | 2743 | 23.41 / 0.89 |
| synth 30 s | L2 | 2.41 | 2.84 | 2.66 | 7.99 | 2743 | |
| synth 30 s | L3 | 1.85 | 2.86 | 2.70 | 7.52 | 2743 | |
| synth 30 s | L4 | 1.88 | 2.68 | 2.63 | 7.29 | 2746 | |

Summary (ort cpu, fp32): Run 1 ≈ Run 2. Medians: 5 s → **0.61 s**, 7.4 s → **0.855 s**, 30 s → **2.77 s**. Session load 1.85–2.68 s (ORT parses/optimises the 2.4 GB graph eagerly; burn's 0.12 s / 1.1 s loads are the mirror image). RSS 2.57–2.75 GB.

### 4.4 Side-by-side (TDT fp32, warm medians, this machine under load 35–65)

| device | 5 s clip | 7.4 s LibriSpeech | 30 s clip | model load | RSS |
|---|---|---|---|---|---|
| burn Metal (`metal`), in-process warm (Run 2) | **0.135 s** | **0.215 s** | **0.46 s** | 1.1–1.7 s | ~620 MB |
| burn Metal, first transcription of a new process (cache on disk) | 2.99 s | 2.86 s | 3.71 s | | |
| burn Metal, machine-cold / first time this audio length | 31.0 s | 19.0 s | 19.3 s | | |
| burn CPU (`burn`, 16 rayon threads) | 2.8 s | 3.5 s | 9.4 s | 0.12 s | ~2.6–2.9 GB |
| ONNX Runtime 1.28 CPU EP, 4 intra threads, fp32 | 0.61 s | 0.855 s | 2.77 s | 1.9–2.7 s | ~2.6–2.7 GB |

Ratios (warm): Metal is 4.5× (5 s) / 4.0× (7.4 s) / 6.0× (30 s) faster than ORT-CPU fp32; burn-CPU is 4.6× / 4.1× / 3.4× slower than ORT-CPU fp32. Real-time factors (warm): Metal 0.015–0.03; ORT CPU 0.09–0.12; burn CPU 0.31–0.56 (under this load).

### 4.5 Transcript parity (LibriSpeech 2086-149220-0033, Run 2 of each launch)

All three devices, all four launches each, printed byte-identical output:
```
[  0.32s -   4.88s] Well, I don't wish to see it any more, observed Phoebe, turning away her eyes.
[  4.88s -   7.36s] It is certainly very like the old portrait.
```
gpu: `logs/run/burn-metal-ort-tdt-gpu-2086-149220-0033-L{1..4}.log`; cpu: `…-tdt-cpu-…`; ort: `…-tdt-ort-…`.

### 4.6 Threads

- burn CPU: `ExecutionProvider::BurnCpu` doc: "Uses all cores (`RAYON_NUM_THREADS` limits them)". Not set here → rayon default = 16 (hw.ncpu 16: 12 P + 4 E). No other knob is exposed.
- ORT: `ModelConfig::default()` in `src/execution.rs` sets `intra_threads: 4, inter_threads: 1`; the example does not override, so every `ort` row above is 4 intra-op threads (ORT's own default would be the physical core count; the crate does not use it).
- burn Metal: no thread knob; the process used ~2.2 s user CPU for a 5.3 s wall 30-s-clip launch (feature extraction + decoder host side).

### 4.7 Cold vs warm, new-length recompilation, and where the cache lives

- Marker `logs/run/00-first-gpu-marker` was touched before the first GPU run. `find ~/Library/Caches ~/.cache -newer logs/run/00-first-gpu-marker -maxdepth 4` → the only new non-browser entry is **`~/.cache/cubecl/`** (created 13:15, the first GPU run).
- Contents: one SQLite database `default.db` (1,028,096 B) + `default.db-wal` (1,487,352 B) + `default.db-tshm` (86,016 B) = **3,172 KiB total** after all TDT GPU runs (5 s, 7.4 s, 30 s, 7 s). Schema: `meta` (schema_version 3) and `entries(namespace, key BLOB, value BLOB, origin)`, 693 rows:

| namespace | rows | value bytes |
|---|---|---|
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl-kernel-matmul-tune-base` | 29 | 83,516 |
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl-kernel-conv-forward-tune` | 23 | 44,011 |
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl_fusion-optim-matmul-tune` | 15 | 27,847 |
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl_fusion-optim-reduce-tune` | 15 | 11,073 |
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl_fusion-optim-reduce_broadcasted-tune` | 12 | 4,903 |
| `autotune/0.11.0/device-100-0-wgpu<msl>/burn_cubecl-kernel-reduce-tune-reduce-dim` | 5 | 5,219 |
| `records/v1/autotune` | 99 | 263,889 |
| `records/v1/compilation` | 478 | 366,769 |
| `records/v1/sessions` | 4 | 402 |
| `throughput/0.11.0/probe-v2/wgpu-msl-_spirv-0-0_Apple-M3-Max_mem4294967292_par0` | 13 | 529 |

  So the on-disk cache is the cubecl 0.11 autotune result store (99 tuned kernel choices, 478 compilation records, keyed per device "wgpu<msl>" and per shape), not compiled Metal binaries; the ~3 s "first transcription per process" cost with a warm cache is Metal pipeline/shader creation that happens in-process (the OS Metal shader cache may also contribute; not measured separately).
- Timeline of GPU "Run 1" costs: machine-cold 5 s clip 31.0 s → LibriSpeech 7.4 s (new length) 19.0 s → 30 s (new length) 19.3 s → repeat launches of each ~2.5–4.2 s → 7 s clip (new length, after 5/7.4/30 s) 6.3 s. The autotune rows grew with each new length (the example's "each new range of audio lengths"); the 7 s clip, sitting between already-tuned 5 s and 7.4 s, still paid ~3 s beyond the per-process baseline.

## 5. Sibling ORT context (read-only, `inventory/INVENTORY.md` §3)

The sibling benchmark measured the **INT8** (SmoothQuant / MatMulNBits) Parakeet TDT encoder graph alone (M2: features [1,128,501] → [1,1024,63]) with onnxruntime **1.30.0** Python, CPU EP, 5 s clip, load avg 21–26: median **211.1 ms at 4 intra threads** (p90 238.6), **181.6 ms with ORT's default thread pool**, **574.4 ms single-thread**; session load 1,397 ms; known in-app number 128–136 ms on a quiet M3 Max (ORT 1.27). Its 5 s signal was `0.3·sin(2π·220t)+0.2·sin(2π·1250t)+0.05·randn` (seed 0, float32); mine uses the same tones with `0.01·uniform(-1,1)` noise written as 16-bit PCM — same tones, different noise term.

Beside those: this run's **fp32** figures are end-to-end (mel features + encoder + TDT decoder loop + tokenisation) on the same clip length: ORT 1.28 CPU 4 threads fp32 **610 ms**, burn Metal warm **135 ms**, burn CPU **2.8 s** (load 35–60). The int8 encoder-only 211 ms vs fp32 end-to-end 610 ms are not the same graph nor the same scope.

## 6. Nemotron streaming fp32 (`burn nemotron <clip> models/nemotron <device>`, `bin/burn-metal-ort`)

The example calls `Nemotron::transcribe_audio(&mono)` once per run (all 560 ms chunks in sequence, `reset()` between runs) and prints only the total; **it does not print per-chunk timings**. Chunk geometry from `src/nemotron.rs`: 56 mel frames (560 ms) per chunk + 9 pre-encode-cache frames → LibriSpeech 7.435 s ≈ 14 chunks, say16k 19.018 s ≈ 34 chunks. Per-chunk figures below are total ÷ chunks. Load avg: 37.76 / 49.21 (13:21 start) → 28.67 / 39.73 (13:27 end). Logs: `logs/run/burn-metal-ort-nemotron-<device>-<clip>-L<n>.log`.

| device | clip | launch | load s | Run 1 s | Run 2 s | wall s | RSS MB |
|---|---|---|---|---|---|---|---|
| gpu (Metal) | LibriSpeech 7.4 s | L1 (machine-cold for this model) | 1.26 | **19.60** | 1.70 | 22.62 | 654 |
| gpu | LibriSpeech | L2 | 1.28 | 5.60 | 1.23 | 8.17 | 588 |
| gpu | LibriSpeech | L3 | 0.95 | 5.02 | 1.13 | 7.17 | 660 |
| gpu | LibriSpeech | L4 | 0.96 | 6.81 | 1.75 | 9.57 | 663 |
| gpu | say16k 19.0 s | L1 | 1.17 | 8.81 | 3.72 | 13.76 | 589 |
| gpu | say16k | L2 | 1.15 | 7.88 | 4.89 | 13.99 | 613 |
| gpu | say16k | L3 | 1.29 | 9.20 | 4.40 | 14.92 | 667 |
| gpu | say16k | L4 | 1.14 | 8.76 | 6.06 | 16.00 | 664 |
| cpu (burn) | LibriSpeech | L1 | 0.11 | 11.56 | 12.32 | 24.07 | 2521 |
| cpu | LibriSpeech | L2 | 0.11 | 11.34 | 7.62 | 19.19 | 2524 |
| cpu | LibriSpeech | L3 | 0.12 | 9.04 | 9.52 | 18.81 | 2523 |
| cpu | LibriSpeech | L4 | 0.10 | 6.99 | 4.80 | 11.96 | 2521 |
| cpu | say16k | L1 | 0.08 | 12.65 | 12.67 | 25.49 | 2525 |
| cpu | say16k | L2 | 0.08 | 13.72 | 12.10 | 25.99 | 2525 |
| cpu | say16k | L3 | 0.10 | 14.63 | 19.76 | 34.58 | 2526 |
| cpu | say16k | L4 | 0.11 | 23.77 | 16.74 | 40.71 | 2525 |
| ort (ORT 1.28 CPU, 4 thr) | LibriSpeech | L1 | 2.31 | 2.11 | 2.21 | 6.73 | 2555 |
| ort | LibriSpeech | L2 | 1.87 | 2.16 | 2.23 | 6.35 | 2555 |
| ort | LibriSpeech | L3 | 2.21 | 2.20 | 1.88 | 6.38 | 2556 |
| ort | LibriSpeech | L4 | 1.87 | 2.38 | 2.32 | 6.76 | 2557 |
| ort | say16k | L1 | 2.12 | 5.64 | 5.56 | 13.42 | 2562 |
| ort | say16k | L2 | 1.92 | 5.35 | 5.47 | 12.86 | 2563 |
| ort | say16k | L3 | 2.40 | 6.16 | 5.41 | 14.04 | 2564 |
| ort | say16k | L4 | 1.95 | 5.70 | 6.24 | 14.00 | 2564 |

Nemotron summary (warm medians, ≈ per 560 ms chunk): Metal Run 2 — LibriSpeech **1.47 s** (≈105 ms/chunk), say16k **4.65 s** (≈137 ms/chunk); Metal process-cold Run 1 with cache: 5.0–9.2 s; machine-cold first run 19.6 s. ORT CPU fp32 — **2.2 s** (≈157 ms/chunk) and **5.5 s** (≈162 ms/chunk). burn CPU — **9.3 s** (≈660 ms/chunk, slower than real time) and **14.7 s** (≈430 ms/chunk). Metal vs ORT here: 1.5× (7.4 s) / 1.2× (19 s) — the streaming model's small per-chunk launches amortise the GPU dispatch overhead far less than the offline TDT encoder. The cubecl cache grew to 3,616 KiB after the Nemotron GPU runs. Note L1 of say16k (first time at that length) cost 8.8 s vs 7.9–9.2 s for L2–L4 — i.e. no visible new-length penalty for the streaming model: chunk shapes are fixed, so the kernels tuned by the LibriSpeech run were reused.

Transcript parity (identical across all launches and all three devices — one distinct string per device/clip):
```
LibriSpeech: [transcript] Well, I don't wish to see it any more, observed Phoebe, turning away her eyes. It is certainly very like the old portrait.
say16k:      [transcript] The quick brown fox jumps over the lazy dog while the old lighthouse keeper watches the harbor from his window every morning. He climbs the narrow spiral staircase, counts the fishing boats leaving the bay, and writes the weather in a small leather notebook that is grandfather once carried across the ocean many years ago. Today the sky is clear and the wind is calm.
```
(`say` source text had "…from his window. Every morning he climbs…" and "that his grandfather"; all three backends make the same two errors.)

## 7. Nemotron-3 Diarization / Sortformer v3 (`burn diar <clip> models/diar/nemotron3_diar_v3.onnx <device> offline`, `bin/burn-metal-ort`, built with `sortformer`)

Single fp32 .onnx (400.5 MB). Offline preset. Load avg 28.28 / 38.92 (13:27) → 25.81 / 35.96 (13:28). Logs: `logs/run/burn-metal-ort-diar-<device>-<clip>-L<n>.log`.

| device | clip | launch | load s | Run 1 s | Run 2 s | wall s | RSS MB |
|---|---|---|---|---|---|---|---|
| gpu (Metal) | LibriSpeech 7.4 s | L1 (machine-cold for this model) | 0.39 | **13.35** | 0.06 | 13.86 | 535 |
| gpu | LibriSpeech | L2 | 0.37 | 2.19 | 0.03 | 2.64 | 534 |
| gpu | LibriSpeech | L3 | 0.31 | 2.05 | 0.04 | 2.45 | 540 |
| gpu | LibriSpeech | L4 | 0.32 | 2.10 | 0.04 | 2.51 | 540 |
| gpu | say16k 19.0 s | L1 (new length) | 0.34 | **13.49** | 0.07 | 13.94 | 541 |
| gpu | say16k | L2 | 0.34 | 3.32 | 0.08 | 3.81 | 548 |
| gpu | say16k | L3 | 0.44 | 3.37 | 0.04 | 3.88 | 549 |
| gpu | say16k | L4 | 0.36 | 3.01 | 0.19 | 3.62 | 542 |
| ort (ORT 1.28 CPU, 4 thr) | LibriSpeech | L1 | 1.39 | 0.15 | 0.15 | 1.74 | 502 |
| ort | LibriSpeech | L2 | 1.38 | 0.15 | 0.15 | 1.73 | 498 |
| ort | LibriSpeech | L3 | 1.38 | 0.14 | 0.16 | 1.75 | 499 |
| ort | LibriSpeech | L4 | 1.37 | 0.16 | 0.16 | 1.72 | 498 |
| ort | say16k | L1 | 1.37 | 0.37 | 0.37 | 2.16 | 508 |
| ort | say16k | L2 | 1.42 | 0.39 | 0.38 | 2.23 | 502 |
| ort | say16k | L3 | 1.35 | 0.43 | 0.42 | 2.25 | 504 |
| ort | say16k | L4 | 1.40 | 0.39 | 0.38 | 2.21 | 513 |

Diar summary: Metal warm (Run 2) **0.04 s** / **0.07 s** vs ORT CPU **0.15 s** / **0.38 s** (3.7× / 5.4×); Metal first-run-per-process 2.05–3.37 s; machine-cold 13.4 s, and the second clip length cost another 13.5 s (new-length retune, same magnitude as the first). ORT session load 1.4 s vs burn 0.3–0.4 s. Cache after diar runs: 5,664 KiB.

Segments (identical across L1–L4 per device):
```
LibriSpeech, gpu and ort (identical):
[   0.51s -    3.24s] speaker_0
[   3.28s -    4.47s] speaker_0
[   5.05s -    7.14s] speaker_0
say16k, gpu:                        say16k, ort:
[   0.00s -    5.45s] speaker_0     [   0.00s -    5.45s] speaker_0
[   5.72s -    8.24s] speaker_0     [   5.72s -    8.24s] speaker_0
[   8.61s -   10.42s] speaker_0     [   8.61s -   10.42s] speaker_0
[  10.76s -   16.19s] speaker_0     [  10.76s -   16.19s] speaker_0
[  16.48s -   18.99s] speaker_0     [  16.48s -   19.02s] speaker_0   <- only difference: last segment end 18.99 vs 19.02 s
```
