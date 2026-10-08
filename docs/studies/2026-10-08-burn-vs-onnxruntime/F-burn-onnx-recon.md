# Appendix F — Burn / burn-onnx 0.22.0 source recon, condensed (child `burn-recon`, 2026-10-08)

Clones read: `tracel-ai/burn` 20426b9 (2026-10-07, workspace 0.22.0) and `tracel-ai/burn-onnx` f20ba03 (2026-10-07, 0.22.0). Citations are `path:line` in those clones. The full report's facts are also in knowledge `facts/burn-onnx-0-22-recon-for-ort-replacement-2026-10-08`.

## Mechanism
- Pipeline: protobuf → `OnnxGraphBuilder::parse_file` (mmap) → init, node conversion, early constant folding, custom-op coverage check, type inference, post-processing, simplification, finalisation (onnx-ir/src/pipeline.rs:316-334, 496-599) → `into_burn().codegen()` + `.bpk` (burn-onnx/src/import/model_gen.rs:412-416). Entry points `ModelGen::run_from_script` / `run_from_cli`; CLI flags `--no-simplify --no-partition --no-development --embed-states`; `partition` splits graphs > 200 nodes (partition.rs:6); `LoadStrategy` File | Embedded | Bytes | None. No runtime interpreter (lib.rs:8-9).
- Generated `Model`: rank and kind baked (argument_helpers.rs:16-22), dims not; `dim_param` → `None` (proto_conversion.rs:888-896); `Shape` → `dims()`; `Reshape`/`Expand` with tensor inputs → `.to_data()` host read (reshape.rs:220-256; expand.rs:44). Bail points: undeterminable Reshape rank, non-constant STFT/DFT params, GatherND without static indices shape. Symbolic `value_info` trips the simplifier on Conformer graphs (#601).
- `.bpk` = burnpack (CBOR metadata, 256-byte-aligned data, lazy `Bytes::from_file`); tensors keyed by generated module paths, Gemm-Linear weights pre-transposed — "an old `.bpk` will not match new code". External `.onnx_data` read lazily with path/offset/length checks (proto_conversion.rs:324-407). fp16/bf16/int8 initializers keep their dtype; no auto-cast at mixed-precision boundaries (#247).

## Operator table (S = SUPPORTED-ONNX-OPS.md, I = crates/onnx-ir/src/node/, E = onnx-official-tests/expectations.toml)

| op | status | limitation |
|---|---|---|
| LSTM | ✅ | no peepholes, no `sequence_lens`, same activations both directions (I lstm.rs:322-332) |
| GRU / RNN | ✅ | no `sequence_lens` |
| LayerNormalization | ✅ | module when scale is a 1-D constant on the last axis, else functional with runtime scale/bias |
| Conv 1d/2d/3d | ✅ | groups/dilation ok; `auto_pad` SAME on dynamic spatial dims rejected |
| ConvTranspose | ✅ | asymmetric pads rejected (1d) |
| MatMul / Gemm | ✅ | 1-D·1-D fails; Gemm→Linear pre-transposed |
| Attention (opset 23) | ✅ | past/present KV all-or-none; 122/128 official pass |
| Where, Range, Expand, ConstantOfShape, NonZero, ScatterND/Elements, GatherND, TopK, Einsum, Pad, Loop/If/Scan, Trilu, Mod, CumSum, Erf, Gelu, Cast | ✅ | Range: 6 rows skip-compile; ConstantOfShape needs a constant value or static dim; Pad: no `wrap`/negative; If/Loop: no Sequence/Optional types in bodies |
| STFT / DFT / MelWeightMatrix | ✅ | frame_step/frame_length/axis compile-time constants; pow-2 n_fft native, else f64 matrix DFT; stft 0/2 and dft 0/10 official rows pass |
| Resize | partial | 9/39 official; no `axes`, antialias, `half_pixel_symmetric` |
| QuantizeLinear / DequantizeLinear | partial | rank-0 (per-tensor) scale rejected; no blocked quant, FP8, INT4 |
| MatMulInteger / QLinearMatMul | ✅ | I8/U8 only; 3-D cases fail |
| DynamicQuantizeLinear, QLinearConv, ConvInteger, RMSNormalization, RotaryEmbedding | ❌ | — |
| any `com.microsoft` op (MatMulNBits, SkipLayerNormalization, DynamicQuantizeLSTM, MultiHeadAttention…) | ❌ | `register_custom_op` only: type inference + `quote!` codegen + a Rust kernel (~45 lines + kernel per op; weighted ops also need `field` + `collect_tensors`) |

Official conformance at HEAD: 1,765 rows — 1,190 pass, 451 skip-codegen, 78 skip-compile, 40 fail-compare, 6 flaky (TODO.md's 932/1,615 is the 2026-08-18 sweep). Non-pass families: sce 68, castlike 62, cast 48, nllloss 30, resize 30, linear_attention 28, rms_normalization 19, causal_conv_with_state 14, sequence 14, dequantizelinear 13, quantizelinear 12, rotary 11, dft 10.

## Backends
- `burn` features (crates/burn/Cargo.toml:134-143): `flex` pure-Rust CPU (`gemm` matmul with native f16, macerator SIMD, rayon, no BLAS; benchmarked only vs burn-ndarray and candle-core); `cpu` = CubeCL CPU JIT through bundled LLVM (no wasm); `ndarray` deprecated (sole consumer of `accelerate`/`openblas`); `wgpu` = Vulkan/Metal/DX12/OpenGL/WebGPU, `metal` feature = native MSL via `MslCompiler`; `cuda` (CUDA 12.x); `rocm`; `tch` deprecated; candle removed; `fusion`/`autotune` default for CubeCL runtimes. Device choice `Device::wgpu(DeviceKind::DiscreteGpu(n) | IntegratedGpu(n) | …)`, default "high power" adapter, override `CUBECL_WGPU_DEFAULT_DEVICE`.
- Shape specialisation: autotune keys anchored on spatial/sequence dims (burn-cubecl conv/forward/tune.rs:174-177; attention/base.rs:78-100), fusion caches on relative shape ids, autotune results cached on disk (burn-book kernel-selection.md:8-14; bundling the cache is advised for deployment); kernel cache paths live in cubecl (not determined).
- `Tensor`/`Model` are Send + Sync; `forward(&self)`; concurrent calls on one wgpu queue: not determined. Known perf issues: burn#4539 CUDA RF-DETR 10–17× slower than TensorRT; burn#5992 WebGPU SqueezeNet slower than flex in the browser; burn#4530 naive attention over Metal's 32 KB shared memory on ViT-S.

## Maturity
- MIT OR Apache-2.0 (both workspaces); cubecl is a git dependency (rev 9865237) whose licence file was not in the clone.
- burn releases: 0.16.0 2025-01-14 … 0.19.0 2025-10-28, 0.20.0 2026-01-15, 0.21.0 2026-05-07, 0.22.0 2026-10-06 (≈ 3–5 months per minor, 617 changelog bullets and a 611-line migration guide for 0.22). burn-onnx: 0.21.0 2026-05-12 (split from burn-import), 0.22.0 2026-10-06. Last 200 commits: burn 30 authors (top 19 %); burn-onnx 22 authors incl. two bots, antimora 72 of 108 human commits (67 %).
- burn-onnx issues: 18 open / 168 closed. Speech-relevant: #601 (simplifier breaks the Conformer rel-pos shift unless `--no-simplify`), #602 (Sortformer ScatterElements-on-Shape), #603 (host syncs ≈ 25 % of a Nemotron chunk) — all 2026-10-08 from the parakeet-rs author; #371 Kokoro 1.3× drift; #160 quantisation ops; #247 F16 `.bpk`. Tracker #18: Silero VAD ✓, kokoro ✓, piper/VITS ✗ (#533/#534), "NVIDIA FastConformer — awaiting a re-test on `main`".
- In-repo model-checks (compared with ORT): ALBERT, all-MiniLM-L6-v2, ModernBERT-base, CLIP, Qwen 1.5/2.5/3 0.5–0.6B and SmolLM (no KV cache, fixed seq 32), SDXL UNet, Depth Pro, Silero VAD (12/12 at 1 %), Kokoro 310 MB (runs; Pearson 0.69; ~7 min compile on an M3), RF-DETR (build failure at README date), YOLOv10n TopK failure.
- `burn-import` is now a deprecated shim; `burn-store` loads safetensors/PyTorch directly (the weight route for a hand-written burn model that never touches ONNX).
