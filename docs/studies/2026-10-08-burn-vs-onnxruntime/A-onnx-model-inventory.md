# ONNX model inventory, Burn gap table and ORT reference data

Scratch dir: `/Users/Sky.Moore/Develop/spike-burn/inventory/` · written 2026-10-08 12:30 (Europe/London) by the `onnx-inventory` worker. Facts only; every number below comes from the JSON files next to this report.

## 0. Setup and versions

- Host: MacBook (Apple Silicon `arm64`, 16 cores, macOS), shared with other sessions — 1-minute load average was **132** when this job started and **21–26** during the ORT timing runs. No Rust/cargo was used.
- Python **3.14.7** (`/opt/homebrew/bin/python3`) in `inventory/venv`; **onnx 1.23.2**, **onnxruntime 1.30.0** (CPU-only wheel, `CPUExecutionProvider`), **numpy 2.5.3**.
  Reproducibility note: the user's pip config points at a private CodeArtifact index that answers 401; install with `PIP_CONFIG_FILE=/dev/null pip install --isolated --index-url https://pypi.org/simple onnx onnxruntime numpy`.
- Scripts (all in the scratch dir, run as `python -P <script>` — `-P` keeps `inspect.py` from shadowing the stdlib `inspect` module):
  - `inspect.py TAG PATH OUT.json --burn-table …` → `M1.json … M9.json` (everything in §1/§2; loads with `load_external_data=False`).
  - `attrs.py TAG PATH OUT.json` → `M*.attrs.json` (distinct attribute/input signatures of the interesting ops; quoted in the per-model notes).
  - `ref.py TAG [--threads 4] [--sweep]` → `ref/<TAG>/spec.json`, `in_<i>.bin`, `out_<i>.bin` (§3).
- Models were opened read-only in place. **M9 (TEN-VAD)** was not under `~/.theuth/models`; it was found in the `wavekat-vad` 0.1.17 build-script output — three identical copies (sha256 `e10b98a0cab1c98e847fbdda14cb3d45a38336d47535a3f63a0fb6c4e0f4cdf4`, 315,449 B) under `~/Develop/theuth/target/{release,debug}/build/wavekat-vad-*/out/ten-vad.onnx`; the release copy `target/release/build/wavekat-vad-2aca1730478ece5f/out/ten-vad.onnx` was copied to `inventory/models/ten-vad.onnx`. The crate's `build.rs` downloads it from the TEN-framework GitHub repo at build time unless `TEN_VAD_MODEL_PATH` is set; no download was needed.
- Burn table: `/Users/Sky.Moore/Develop/spike-burn/burn-onnx/SUPPORTED-ONNX-OPS.md` parsed into 208 rows — 172 ✅/✅, 36 ❌/❌, none mixed. Mapping used: `Conv`/`ConvTranspose` → `ConvNd` by weight rank (all weights here are initializers, so the rank is known); `MaxPool`/`AveragePool`/`LpPool` → `…Nd` by `kernel_shape` length; `ReduceL1`/`ReduceL2` → the table's `ReduceL` row; everything else by exact name. Per the brief, an op is a gap when it is absent from the table, listed ❌, or lives in a non-default domain (`com.microsoft`). The table's own text says such ops "can still be imported by registering a custom op hook".

## 1. Summary

| model | size | opset | nodes | distinct ops | unsupported-by-Burn ops (count) | quantized? | external data? | control flow? | dynamic dims |
|---|---|---|---:|---:|---|---|---|---|---|
| M1 | 139.8 KB | onnx 17 | 34 | 20 | STFT (1) | no | no | no | N, T, batch_size |
| M2 | 945.5 MB | onnx 17 + com.microsoft 1 | 2041 | 32 | ms::MatMulNBits (217) | yes (8-bit MatMulNBits, block 32) | no | no | batch + time (audio_signal_dynamic_axes_1/2, length_dynamic_axes_1) |
| M3 | 18.2 MB | onnx 17 + com.microsoft 1 | 63 | 18 | DynamicQuantizeLinear (3), ms::DynamicQuantizeLSTM (2) | yes (dynamic u8 act × u8/i8 weights) | no | no | batch, T_enc, U (targets), state batch (7 named axes) |
| M4 | 880.6 MB | onnx 17 | 6691 | 37 | DynamicQuantizeLinear (170) | yes (dynamic u8 act × i8 weights) | no | no | batch, time (caches fixed [24,1,70,1024] / [24,1,1024,8]) |
| M5 | 11.0 MB | onnx 17 + com.microsoft 1 | 54 | 15 | DynamicQuantizeLinear (3), ms::DynamicQuantizeLSTM (2) | yes (dynamic u8 act × i8 weights) | no | no | batch, T_enc, U (targets), state batch (7 named axes) |
| M6 | 364.4 KB + 120.5 MB ext | onnx 21 + com.microsoft 1 | 749 | 24 | ms::MatMulNBits (127), ms::RotaryEmbedding (62), ms::SkipLayerNormalization (62) | yes (8-bit MatMulNBits, block 32, zero points) | yes | no | batch_size, num_cached_frames, num_chunk_frames, num_frames, num_logit_frames, num_mel_frames |
| M7 | 6.0 MB | onnx 17 (+7 declared, unused: com.microsoft, ai.onnx.ml 4, training, nchwc, aten…) | 50+12 | 19 | — | no | no | If ×1 (then 1 / else 11 nodes) | B, C, T |
| M8 | 29.3 MB | onnx 14 | 3207 | 29 | — | no | no | no | B, T |
| M9 | 315.4 KB | onnx 9 + onnx.ml 2 | 53 | 15 | — | no | no | no | batch only (unk__*) |

Nodes for M7 are `main+subgraph`. "Distinct ops" counts the main graph (M7's `If` branches add no new op types). Sizes are the `.onnx` file; M6's weights live in the external file.

**Union gap across M1–M9: 6 op types, 649 nodes.**

| op | nodes (all models) | where | why it is a gap |
|---|---:|---|---|
| `com.microsoft::MatMulNBits` | 344 | M2 ×217, M6 ×127 | non-default domain com.microsoft |
| `DynamicQuantizeLinear` | 176 | M3 ×3, M4 ×170, M5 ×3 | listed ❌ in Burn table |
| `com.microsoft::RotaryEmbedding` | 62 | M6 ×62 | non-default domain com.microsoft |
| `com.microsoft::SkipLayerNormalization` | 62 | M6 ×62 | non-default domain com.microsoft |
| `com.microsoft::DynamicQuantizeLSTM` | 4 | M3 ×2, M5 ×2 | non-default domain com.microsoft |
| `STFT` | 1 | M1 ×1 | absent from Burn table |

Everything else in all nine graphs maps to a ✅ row — including `LSTM` (M7 4× bidirectional, M9 2× forward), `If` (M7), `InstanceNormalization` (M7), `MatMulInteger` (M3/M5 3 each, M4 217), `DequantizeLinear` (M3 1), `Gelu` (M6, opset-21 op), `Mod` (M6, int64), `STFT`-adjacent ops `HannWindow`/`DFT` are *not* used (M1 uses the fused `STFT` op), `Pad` in `reflect` mode (M1), `AveragePool` with `ceil_mode=1` and kernel 100 (M8 ×52), depthwise `Conv` (`group`>1: M2/M4 26 each, M9 2).

**Runtime (non-initializer) weights**: none in any model. Every `Conv`, `LayerNormalization`, `Gemm`-less `MatMul`-as-Linear and `MatMulInteger`/`MatMulNBits` weight is a graph initializer. The only `MatMul`s whose both operands are activations are the attention products (QKᵀ, softmax·V, and the relative-position product): M2 72, M4 72, M6 62. No `Gemm` anywhere.

## 2. Per-model inventory

### M1 — NeMo mel-spectrogram preprocessor (parakeet-tdt/nemo128.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/parakeet-tdt/nemo128.onnx`
- File size: 139,764 B (139.8 KB)
- ir_version 8; opset_import: ai.onnx=17; producer: OnnxScript 0.3.2
- metadata_props: model_author=Ilya Stupakov; model_license=MIT License
- Graph: 34 nodes, 20 distinct op types; value_info entries: 48
- Op histogram: Div 4, Add 3, Cast 3, Slice 3, Sub 3, Where 3, ReduceSumSquare 2, Concat 1, Less 1, Log 1, MatMul 1, Mul 1, Pad 1, Range 1, ReduceSum 1, STFT 1, Shape 1, Sqrt 1, Transpose 1, Unsqueeze 1
- Control flow (Loop/If/Scan): none
- Inputs: `waveforms` float32 [batch_size,N]; `waveforms_lens` int64 [batch_size]
- Outputs: `features` float32 [batch_size,128,T]; `features_lens` int64 [batch_size]
- Dynamic dim names: N, T, batch_size
- Initializers: 16 tensors, 135,804 B logical (135.8 KB); by dtype: float32 6× 131.6 KB, float64 1× 4.1 KB, int64 9× 104 B
- Largest initializers: `tmp` float32 [257,128] 131.6 KB; `hann_window` float64 [512] 4.1 KB; `tmp_11` int64 [4] 32 B
- External data: no (all weights embedded)
- Quantization-related ops: none
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. MatMul 1: operands {'A=node:Cast | B=initializer': 1}
- **Burn gap: 1 op type(s), 1 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `STFT` | 1 | absent from Burn table | `n13_2` |


### M2 — Parakeet TDT 0.6B v3 FastConformer encoder, SmoothQuant INT8 build (parakeet-tdt/encoder-model.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/parakeet-tdt/encoder-model.onnx`
- File size: 945,502,874 B (945.5 MB)
- ir_version 8; opset_import: ai.onnx=17, com.microsoft=1; producer: pytorch 2.8.0
- Graph: 2041 nodes, 32 distinct op types; value_info entries: 2063
- Op histogram: com.microsoft::MatMulNBits 217, Transpose 196, Add 176, Reshape 169, Unsqueeze 156, Mul 144, Gather 125, LayerNormalization 120, Concat 102, Shape 100, Sigmoid 96, Conv 77, Slice 74, Where 73, MatMul 72, Div 27, Pad 24, Softmax 24, Split 24, Squeeze 24, Cast 3, Floor 3, Relu 3, And 2, Not 2, Sub 2, ConstantOfShape 1, Equal 1, Expand 1, Less 1, Range 1, Tile 1
- Control flow (Loop/If/Scan): none
- Inputs: `audio_signal` float32 [audio_signal_dynamic_axes_1,128,audio_signal_dynamic_axes_2]; `length` int64 [length_dynamic_axes_1]
- Outputs: `outputs` float32 [Transposeoutputs_dim_0,1024,Transposeoutputs_dim_2]; `encoded_lengths` int64 [length_dynamic_axes_1]
- Dynamic dim names: Transposeoutputs_dim_0, Transposeoutputs_dim_2, audio_signal_dynamic_axes_1, audio_signal_dynamic_axes_2, length_dynamic_axes_1
- Initializers: 856 tensors, 944,931,084 B logical (944.9 MB); by dtype: uint8 217× 532.7 MB, float32 620× 412.3 MB, int64 19× 240 B
- Largest initializers: `onnx::Slice_780` float32 [1,9999,1024] 41.0 MB; `layers.9.conv.pointwise_conv1.weight` float32 [2048,1024,1] 8.4 MB; `layers.8.conv.pointwise_conv1.weight` float32 [2048,1024,1] 8.4 MB
- External data: no (all weights embedded)
- Quantization-related ops: com.microsoft::MatMulNBits 217
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 77: weight source {'initializer': 77}, bias {'initializer': 29}, spatial rank {'2': 5, '1': 72}, {'group=1': 51, 'group>1': 26}; LayerNormalization 120: scale {'initializer': 120}, bias {'initializer': 120}; MatMul 72: operands {'A=node:Transpose | B=node:Transpose': 48, 'A=node:Where | B=node:Transpose': 24}
- **Burn gap: 1 op type(s), 217 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `com.microsoft::MatMulNBits` | 217 | non-default domain com.microsoft | `/pre_encode/out/MatMul_Q8`, `/layers.0/feed_forward1/linear1/MatMul_Q8`, `/layers.0/feed_forward1/linear2/MatMul_Q8` |


### M3 — Parakeet TDT prediction net + joint, INT8 (parakeet-tdt/decoder_joint-model.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/parakeet-tdt/decoder_joint-model.onnx`
- File size: 18,203,493 B (18.2 MB)
- ir_version 8; opset_import: ai.onnx=17, com.microsoft=1; producer: onnx.quantize 0.1.0
- metadata_props: onnx.infer=onnxruntime.quant
- Graph: 63 nodes, 18 distinct op types; value_info entries: 42
- Op histogram: Constant 16, Slice 8, Mul 6, Cast 5, Add 4, DynamicQuantizeLinear 3, MatMulInteger 3, Transpose 3, Concat 2, ReduceLogSumExp 2, Squeeze 2, Unsqueeze 2, com.microsoft::DynamicQuantizeLSTM 2, DequantizeLinear 1, Gather 1, Identity 1, Relu 1, TopK 1
- Control flow (Loop/If/Scan): none
- Inputs: `encoder_outputs` float32 [encoder_outputs_dynamic_axes_1,1024,encoder_outputs_dynamic_axes_2]; `targets` int32 [targets_dynamic_axes_1,targets_dynamic_axes_2]; `target_length` int32 [target_length_dynamic_axes_1]; `input_states_1` float32 [2,input_states_1_dynamic_axes_1,640]; `input_states_2` float32 [2,input_states_2_dynamic_axes_1,640]
- Outputs: `outputs` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2,8198]; `prednet_lengths` int32 [target_length_dynamic_axes_1]; `output_states_1` float32 [2,Concatoutput_states_1_dim_1,640]; `output_states_2` float32 [2,Concatoutput_states_2_dim_1,640]; `lse_token` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2]; `lse_duration` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2]; `topk_logits` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2,16]; `topk_ids` int32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2,16]; `duration_logits` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2,5]
- Dynamic dim names: Addoutputs_dim_0, Addoutputs_dim_1, Addoutputs_dim_2, Concatoutput_states_1_dim_1, Concatoutput_states_2_dim_1, encoder_outputs_dynamic_axes_1, encoder_outputs_dynamic_axes_2, input_states_1_dynamic_axes_1, input_states_2_dynamic_axes_1, target_length_dynamic_axes_1, targets_dynamic_axes_1, targets_dynamic_axes_2
- Initializers: 40 tensors, 18,187,800 B logical (18.2 MB); by dtype: uint8 8× 11.6 MB, int8 8× 6.6 MB, float32 13× 78.9 KB, int64 11× 88 B
- Largest initializers: `onnx::MatMul_230_quantized` uint8 [640,8198] 5.2 MB; `decoder.prediction.embed.weight_quantized` uint8 [8193,640] 5.2 MB; `onnx::LSTM_226_quantized` int8 [1,640,2560] 1.6 MB
- Constant nodes: 16 carrying 128 B of tensors (int64 16× 128 B)
- External data: no (all weights embedded)
- Quantization-related ops: DynamicQuantizeLinear 3, MatMulInteger 3, com.microsoft::DynamicQuantizeLSTM 2, DequantizeLinear 1
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. MatMulInteger 3: B {'initializer': 3}
- **Burn gap: 2 op type(s), 5 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `DynamicQuantizeLinear` | 3 | listed ❌ in Burn table | `/joint/Transpose_output_0_QuantizeLinear`, `/joint/Transpose_1_output_0_QuantizeLinear`, `/joint/joint_net/joint_net.0/Relu_output_0_QuantizeLinear` |
| `com.microsoft::DynamicQuantizeLSTM` | 2 | non-default domain com.microsoft | `/decoder/dec_rnn/lstm/LSTM_quant`, `/decoder/dec_rnn/lstm/LSTM_1_quant` |


### M4 — Nemotron streaming FastConformer encoder with cache-state inputs, INT8 (nemotron-streaming-en/encoder.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/nemotron-streaming-en/encoder.onnx`
- File size: 880,555,453 B (880.6 MB)
- ir_version 8; opset_import: ai.onnx=17; producer: onnx.quantize 0.1.0
- metadata_props: onnx.infer=onnxruntime.quant
- Graph: 6691 nodes, 37 distinct op types; value_info entries: 5889
- Op histogram: Constant 2257, Mul 605, Unsqueeze 496, Cast 378, Concat 315, Transpose 295, Reshape 289, Gather 257, Shape 252, MatMulInteger 217, Slice 203, Add 183, DynamicQuantizeLinear 170, LayerNormalization 144, Sigmoid 96, Where 90, Conv 77, MatMul 72, ConstantOfShape 71, Pad 51, Div 28, Softmax 24, Split 24, Squeeze 24, Expand 19, Equal 18, Sub 7, Range 6, And 5, Less 5, Clip 3, Relu 3, GreaterOrEqual 2, Not 2, LessOrEqual 1, Neg 1, Tile 1
- Control flow (Loop/If/Scan): none
- Inputs: `processed_signal` float32 [batch,128,time]; `processed_signal_length` int64 [batch]; `cache_last_channel` float32 [24,1,70,1024]; `cache_last_time` float32 [24,1,1024,8]; `cache_last_channel_len` int64 [1]
- Outputs: `encoded` float32 [batch,1024,Sliceencoded_dim_2]; `encoded_len` int64 [batch]; `cache_last_channel_next` float32 [24,1,Slicecache_last_channel_next_dim_2,1024]; `cache_last_time_next` float32 [24,1,1024,Concatcache_last_time_next_dim_3]; `cache_last_channel_len_next` int64 [1]
- Dynamic dim names: Concatcache_last_time_next_dim_3, Slicecache_last_channel_next_dim_2, Sliceencoded_dim_2, batch, time
- Initializers: 1070 tensors, 837,751,869 B logical (837.8 MB); by dtype: int8 434× 532.9 MB, float32 636× 304.8 MB
- Largest initializers: `encoder.layers.9.conv.pointwise_conv1.weight` float32 [2048,1024,1] 8.4 MB; `encoder.layers.8.conv.pointwise_conv1.weight` float32 [2048,1024,1] 8.4 MB; `encoder.layers.7.conv.pointwise_conv1.weight` float32 [2048,1024,1] 8.4 MB
- Constant nodes: 2257 carrying 40,974,168 B of tensors (int64 2094× 17.6 KB, float32 160× 41.0 MB, int32 3× 12 B)
- External data: no (all weights embedded)
- Quantization-related ops: MatMulInteger 217, DynamicQuantizeLinear 170
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 77: weight source {'initializer': 77}, bias {'initializer': 5}, spatial rank {'2': 5, '1': 72}, {'group=1': 51, 'group>1': 26}; LayerNormalization 144: scale {'initializer': 144}, bias {'initializer': 144}; MatMul 72: operands {'A=node:Transpose | B=node:Transpose': 48, 'A=node:Where | B=node:Transpose': 24}; MatMulInteger 217: B {'initializer': 217}
- **Burn gap: 1 op type(s), 170 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `DynamicQuantizeLinear` | 170 | listed ❌ in Burn table | `/encoder/pre_encode/Reshape_output_0_QuantizeLinear`, `/encoder/layers.0/norm_feed_forward1/LayerNormalization_output_0_QuantizeLinear`, `/encoder/pos_enc/Slice_output_0_QuantizeLinear` |


### M5 — Nemotron streaming prediction net + joint, INT8 (nemotron-streaming-en/decoder_joint.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/nemotron-streaming-en/decoder_joint.onnx`
- File size: 10,962,697 B (11.0 MB)
- ir_version 8; opset_import: ai.onnx=17, com.microsoft=1; producer: onnx.quantize 0.1.0
- metadata_props: onnx.infer=onnxruntime.quant
- Graph: 54 nodes, 15 distinct op types; value_info entries: 42
- Op histogram: Constant 16, Mul 6, Add 4, Cast 4, Slice 4, DynamicQuantizeLinear 3, MatMulInteger 3, Transpose 3, Concat 2, Squeeze 2, Unsqueeze 2, com.microsoft::DynamicQuantizeLSTM 2, Gather 1, Identity 1, Relu 1
- Control flow (Loop/If/Scan): none
- Inputs: `encoder_outputs` float32 [encoder_outputs_dynamic_axes_1,1024,encoder_outputs_dynamic_axes_2]; `targets` int32 [targets_dynamic_axes_1,targets_dynamic_axes_2]; `target_length` int32 [target_length_dynamic_axes_1]; `input_states_1` float32 [2,input_states_1_dynamic_axes_1,640]; `input_states_2` float32 [2,input_states_2_dynamic_axes_1,640]
- Outputs: `outputs` float32 [Addoutputs_dim_0,Addoutputs_dim_1,Addoutputs_dim_2,1025]; `prednet_lengths` int32 [target_length_dynamic_axes_1]; `output_states_1` float32 [2,Concatoutput_states_1_dim_1,640]; `output_states_2` float32 [2,Concatoutput_states_2_dim_1,640]
- Dynamic dim names: Addoutputs_dim_0, Addoutputs_dim_1, Addoutputs_dim_2, Concatoutput_states_1_dim_1, Concatoutput_states_2_dim_1, encoder_outputs_dynamic_axes_1, encoder_outputs_dynamic_axes_2, input_states_1_dynamic_axes_1, input_states_2_dynamic_axes_1, target_length_dynamic_axes_1, targets_dynamic_axes_1, targets_dynamic_axes_2
- Initializers: 27 tensors, 10,948,775 B logical (10.9 MB); by dtype: int8 14× 8.3 MB, float32 13× 2.7 MB
- Largest initializers: `decoder.prediction.embed.weight` float32 [1025,640] 2.6 MB; `onnx::LSTM_226_quantized` int8 [1,640,2560] 1.6 MB; `onnx::LSTM_225_quantized` int8 [1,640,2560] 1.6 MB
- Constant nodes: 16 carrying 128 B of tensors (int64 16× 128 B)
- External data: no (all weights embedded)
- Quantization-related ops: DynamicQuantizeLinear 3, MatMulInteger 3, com.microsoft::DynamicQuantizeLSTM 2
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. MatMulInteger 3: B {'initializer': 3}
- **Burn gap: 2 op type(s), 5 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `DynamicQuantizeLinear` | 3 | listed ❌ in Burn table | `/joint/Transpose_output_0_QuantizeLinear`, `/joint/Transpose_1_output_0_QuantizeLinear`, `/joint/joint_net/joint_net.0/Relu_output_0_QuantizeLinear` |
| `com.microsoft::DynamicQuantizeLSTM` | 2 | non-default domain com.microsoft | `/decoder/dec_rnn/lstm/LSTM_quant`, `/decoder/dec_rnn/lstm/LSTM_1_quant` |


### M6 — Nemotron 3 Diarization (streaming Sortformer), 8-bit MatMulNBits, external data (nemotron-diarization/model_quantized.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/nemotron-diarization/model_quantized.onnx`
- File size: 364,375 B (364.4 KB) + external data `model_quantized.onnx_data` referenced by 606 tensors, 120,479,872 B by TensorProto.external_data `length` (file on disk: 120,479,872 B — identical)
- ir_version 10; opset_import: ai.onnx=21, com.microsoft=1; producer: huggingface
- Graph: 749 nodes, 24 distinct op types; value_info entries: 1241
- Op histogram: com.microsoft::MatMulNBits 127, Reshape 126, Transpose 126, Add 65, MatMul 63, com.microsoft::RotaryEmbedding 62, com.microsoft::SkipLayerNormalization 62, Gelu 31, Softmax 31, Split 31, Concat 4, Pad 3, Gather 2, LayerNormalization 2, Mod 2, Neg 2, Relu 2, Shape 2, Cast 1, Conv 1, Mul 1, Slice 1, Sub 1, Unsqueeze 1
- Control flow (Loop/If/Scan): none
- Inputs: `input_features` float32 [batch_size,num_mel_frames,128]; `cached_embeds` float32 [batch_size,num_cached_frames,512]; `attention_mask` int64 [batch_size,num_frames]
- Outputs: `logits` float32 [batch_size,num_logit_frames,8]; `chunk_embeds` float32 [batch_size,num_chunk_frames,512]; `silence_embeds` float32 [512]
- Dynamic dim names: batch_size, num_cached_frames, num_chunk_frames, num_frames, num_logit_frames, num_mel_frames
- Initializers: 625 tensors, 120,481,920 B logical (120.5 MB); by dtype: uint8 254× 100.7 MB, float32 359× 19.8 MB, int64 12× 232 B
- Largest initializers: `model.upsampler.conv.weight` float32 [1536,192,3] 3.5 MB; `model.audio_tower.embedder.projection.MatMul.weight` float32 [1024,512] 2.1 MB; `model_layers_9_mlp_fc2_MatMul_weight_quant` uint8 [512,64,32] 1.0 MB
- External data: yes — 606 tensors in model_quantized.onnx_data
- Quantization-related ops: com.microsoft::MatMulNBits 127
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 1: weight source {'initializer': 1}, bias {'initializer': 1}, spatial rank {'1': 1}, {'group=1': 1}; LayerNormalization 2: scale {'initializer': 2}, bias {'initializer': 2}; MatMul 63: operands {'A=node:Reshape | B=initializer': 1, 'A=node:Transpose | B=node:Transpose': 31, 'A=node:Softmax | B=node:Transpose': 31}
- **Burn gap: 3 op type(s), 251 nodes**

| op | count | why | example nodes |
|---|---:|---|---|
| `com.microsoft::MatMulNBits` | 127 | non-default domain com.microsoft | `/model/layers.0/attn/qkv_proj/MatMul_Quant`, `/model/layers.0/attn/o_proj/MatMul_Quant`, `/model/layers.0/mlp/fc1/MatMul_Quant` |
| `com.microsoft::RotaryEmbedding` | 62 | non-default domain com.microsoft | `/model/layers.0/attn/q_rotary/RotaryEmbedding`, `/model/layers.0/attn/k_rotary/RotaryEmbedding`, `/model/layers.1/attn/q_rotary/RotaryEmbedding` |
| `com.microsoft::SkipLayerNormalization` | 62 | non-default domain com.microsoft | `/model/layers.0/post_attention_layernorm/SkipLayerNorm`, `/model/layers.1/input_layernorm/SkipLayerNorm`, `/model/layers.1/post_attention_layernorm/SkipLayerNorm` |


### M7 — pyannote segmentation-3.0: SincNet + BiLSTM (pyannote/segmentation-3.0.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/pyannote/segmentation-3.0.onnx`
- File size: 5,983,836 B (6.0 MB)
- ir_version 8; opset_import: ai.onnx=17, com.microsoft.experimental=1, ai.onnx.ml=4, ai.onnx.training=1, com.microsoft=1, ai.onnx.preview.training=1, com.microsoft.nchwc=1, org.pytorch.aten=1; producer: pytorch 2.2.1
- Graph: 50 nodes (+12 in subgraphs), 19 distinct op types; value_info entries: 0
- Op histogram: Transpose 6, LeakyRelu 5, InstanceNormalization 4, LSTM 4, Reshape 4, Slice 4, Add 3, Gather 3, MatMul 3, MaxPool 3, Conv 2, Shape 2, Abs 1, Concat 1, ConstantOfShape 1, Equal 1, If 1, LogSoftmax 1, Unsqueeze 1
- Control flow: `If` node `If_117` attr `then_branch` → 1 nodes: Conv 1
- Control flow: `If` node `If_117` attr `else_branch` → 11 nodes: Slice 3, Concat 2, Reshape 2, Conv 1, Shape 1, Squeeze 1, Unsqueeze 1
- Inputs: `input` float32 [B,C,T]
- Outputs: `output` float32 [LogSoftmaxoutput_dim_0,LogSoftmaxoutput_dim_1,7]
- Dynamic dim names: B, C, LogSoftmaxoutput_dim_0, LogSoftmaxoutput_dim_1, T
- Initializers: 41 tensors, 5,973,172 B logical (6.0 MB); by dtype: float32 31× 6.0 MB, int64 10× 112 B
- Largest initializers: `onnx::LSTM_913` float32 [2,512,256] 1.0 MB; `onnx::LSTM_870` float32 [2,512,256] 1.0 MB; `onnx::LSTM_827` float32 [2,512,256] 1.0 MB
- External data: no (all weights embedded)
- Quantization-related ops: none
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 2: weight source {'initializer': 2}, bias {'initializer': 2}, spatial rank {'1': 2}, {'group=1': 2}; MatMul 3: operands {'A=node:Transpose | B=initializer': 1, 'A=node:LeakyRelu | B=initializer': 2}
- **Burn gap: none** (every op maps to a ✅ row)

### M8 — wespeaker CAM++ speaker embedding (pyannote/wespeaker_en_voxceleb_CAM++.onnx)

- Path: `/Users/Sky.Moore/.theuth/models/pyannote/wespeaker_en_voxceleb_CAM++.onnx`
- File size: 29,292,684 B (29.3 MB)
- ir_version 7; opset_import: ai.onnx=14; producer: pytorch 1.12.1
- metadata_props: framework=wespeaker; language=English; url=https://wespeaker-1256283475.cos.ap-shanghai.myqcloud.com/models/voxceleb/voxcel; comment=no comment; sample_rate=16000; output_dim=512; normalize_samples=0
- Graph: 3207 nodes, 29 distinct op types; value_info entries: 0
- Op histogram: Constant 795, Unsqueeze 369, Shape 265, Conv 225, Gather 213, Identity 204, Relu 171, Concat 158, Mul 107, Reshape 105, Add 57, BatchNormalization 56, ReduceMean 55, AveragePool 52, ConstantOfShape 52, Equal 52, Expand 52, Pad 52, Sigmoid 52, Slice 52, Where 52, Sub 3, Flatten 2, Cast 1, Div 1, ReduceProd 1, Sqrt 1, Squeeze 1, Transpose 1
- Control flow (Loop/If/Scan): none
- Inputs: `feats` float32 [B,T,80]
- Outputs: `embs` float32 [B,512]
- Dynamic dim names: B, T
- Initializers: 622 tensors, 28,946,080 B logical (28.9 MB); by dtype: float32 618× 28.9 MB, int64 4× 32 B
- Largest initializers: `onnx::Conv_4828` float32 [512,1024,1] 2.1 MB; `model.xvector.transit2.linear.weight` float32 [512,1024,1] 2.1 MB; `model.xvector.dense.linear.weight` float32 [512,1024,1] 2.1 MB
- Constant nodes: 795 carrying 12,512 B of tensors (int64 791× 8.4 KB, float32 4× 4.1 KB)
- External data: no (all weights embedded)
- Quantization-related ops: none
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 225: weight source {'initializer': 225}, bias {'initializer': 170}, spatial rank {'2': 12, '1': 213}, {'group=1': 225}
- **Burn gap: none** (every op maps to a ✅ row)

### M9 — TEN-VAD (copied from the wavekat-vad 0.1.17 build output; see §1)

- Path: `/Users/Sky.Moore/Develop/spike-burn/inventory/models/ten-vad.onnx`
- File size: 315,449 B (315.4 KB)
- ir_version 4; opset_import: ai.onnx=9, ai.onnx.ml=2; producer: tf2onnx 1.10.0
- Graph: 53 nodes, 15 distinct op types; value_info entries: 0
- Op histogram: Squeeze 8, Reshape 7, Conv 6, Unsqueeze 6, Cast 4, Relu 4, Concat 3, Transpose 3, Add 2, Gather 2, LSTM 2, MatMul 2, Shape 2, MaxPool 1, Sigmoid 1
- Control flow (Loop/If/Scan): none
- Inputs: `input_1` float32 [unk__187,3,41]; `input_2` float32 [unk__188,64]; `input_3` float32 [unk__189,64]; `input_6` float32 [unk__192,64]; `input_7` float32 [unk__193,64]
- Outputs: `output_1` float32 [unk__194,unk__195,1]; `output_2` float32 [unk__196,64]; `output_3` float32 [unk__197,64]; `output_6` float32 [unk__200,64]; `output_7` float32 [unk__201,64]
- Dynamic dim names: unk__187, unk__188, unk__189, unk__192, unk__193, unk__194, unk__195, unk__196, unk__197, unk__200, unk__201
- Initializers: 27 tensors, 302,120 B logical (302.1 KB); by dtype: float32 19× 302.0 KB, int64 5× 112 B, int32 3× 16 B
- Largest initializers: `W0__70` float32 [1,256,80] 81.9 KB; `W0__99` float32 [1,256,64] 65.5 KB; `R0__71` float32 [1,256,64] 65.5 KB
- External data: no (all weights embedded)
- Quantization-related ops: none
- Runtime (non-initializer) weights for Conv/LayerNormalization: **0**. Conv 6: weight source {'initializer': 6}, bias {'initializer': 3}, spatial rank {'2': 6}, {'group=1': 4, 'group>1': 2}; MatMul 2: operands {'A=node:Reshape | B=initializer': 2}
- **Burn gap: none** (every op maps to a ✅ row)

## 3. ORT reference data (`inventory/ref/<tag>/`)

Marker `inventory/ref/READY` lists the nine tags written (M1–M9, none skipped). Each dir holds `spec.json` plus raw little-endian C-order binaries written with numpy `.tofile()`: `in_<i>.bin` in session input order, `out_<i>.bin` in session output order; `spec.json` has `model`, `onnxruntime` (1.30.0), `provider` (`CPUExecutionProvider`), `intra_op_threads` (4; `inter_op_threads` 1, sequential execution, `ORT_ENABLE_ALL`), `inputs`/`outputs` as `{name, dtype, shape, file, stats}` (`stats` = min/max/mean/std/nan/inf for floats, min/max and the values for small int tensors), `timing_ms` `{warmup 3, runs 20, median, p90, min, max, mean}`, `session_load_ms`, `host` (load averages before/after), and `notes` (exactly what was fed). M2 additionally has `timing_ms_sweep` (`intra_op_1`, `intra_op_default`; 3 warm-ups + 10 runs each). All outputs are finite (0 NaN / 0 Inf everywhere).

Reference waveform: 5.0 s at 16 kHz = 80,000 samples of `0.3·sin(2π·220t) + 0.2·sin(2π·1250t) + 0.05·RandomState(0).randn(n)`, `t = arange(n)/16000`, float32 (min −0.706, max 0.651, std 0.260). The 10 s variant for M6 uses the same formula over 160,000 samples; its first 80,000 samples are bit-identical to the 5 s clip.

What each model was fed (also in each `spec.json` `notes`):

| tag | inputs | outputs |
|---|---|---|
| M1 | `waveforms` [1,80000], `waveforms_lens` [80000] | `features` [1,128,501] (mean 0.000, std 0.999 → the graph includes per-feature normalisation), `features_lens` [501] |
| M2 | `audio_signal` = M1 `features` [1,128,501]; `length` = [501] | `outputs` [1,1024,63], `encoded_lengths` [63] |
| M3 | `encoder_outputs` = M2 frame 31 of 63 → [1,1024,1]; `targets` [[42]] int32 (embedding `decoder.prediction.embed.weight_quantized` [8193,640] → valid ids 0..8192; 8192 = blank); `target_length` [1]; `input_states_1/2` zeros [2,1,640] | `outputs` [1,1,1,8198] (8193 tokens + 5 TDT durations), `prednet_lengths`, `output_states_1/2`, `lse_token`, `lse_duration`, `topk_logits`/`topk_ids` [1,1,1,16] (top id 8192 = blank, as expected for a sine), `duration_logits` [1,1,1,5] |
| M4 | first streaming chunk exactly as `parakeet-rs` 0.3.7 `src/nemotron.rs` builds it: `processed_signal` [1,128,65] = 9 zero pre-encode-cache frames + M1 `features[:, :, :56]` (56 mel frames = 560 ms = 7 encoder frames × 8); `processed_signal_length` [65]; `cache_last_channel` zeros [24,1,70,1024]; `cache_last_time` zeros [24,1,1024,8]; `cache_last_channel_len` [0]. The cache dims are fixed in the graph (24 layers, left context 70, conv context 8); `metadata_props` carries nothing, so the 56/9 geometry is the crate's documented default ("56 for the default 560ms export", "default 9"). The app feeds un-normalised log-mel here while M1 normalises; magnitudes are comparable (std 0.96). | `encoded` [1,1024,7], `encoded_len` [7], `cache_last_channel_next` [24,1,70,1024], `cache_last_time_next` [24,1,1024,8], `cache_last_channel_len_next` [7] |
| M5 | `encoder_outputs` = M4 `encoded` frame 3 of 7 → [1,1024,1]; `targets` [[42]] (embedding `decoder.prediction.embed.weight` [1025,640] → ids 0..1024); `target_length` [1]; states zeros [2,1,640] | `outputs` [1,1,1,1025], `prednet_lengths`, `output_states_1/2` [2,1,640] |
| M6 | `input_features` [1,1000,128] = first 1000 frames (10.0 s at 10 ms hop) of M1's log-mel for the 10 s waveform, transposed to [1,T,128]; empty streaming state: `cached_embeds` zeros [1,0,512] (C=0 — ORT accepts the zero-length dim), `attention_mask` ones int64 [1, C+ceil(1000/8)] = [1,125] (layout from `crates/meeting/src/post/diarize.rs`: cache then FIFO embeddings, mask over cached+chunk encoder frames) | `logits` [1,1000,8] (10 ms rate), `chunk_embeds` [1,125,512], `silence_embeds` [512] |
| M7 | `input` [1,1,80000] = the waveform | `output` [1,293,7] (log-softmax powerset) |
| M8 | the graph takes fbank features, not audio: `feats` [1,498,80] = RandomState(1).normal(0,1) float32 (498 = kaldi 25 ms/10 ms frames for 5 s; metadata says sample_rate 16000, normalize_samples 0) | `embs` [1,512] |
| M9 | the graph takes 3 context frames × 41 normalised features (40 mel + pitch, per `wavekat-vad` `src/backends/ten_vad.rs`): `input_1` [1,3,41] = RandomState(2).normal(0,1); recurrent states `input_2/3/6/7` zeros [1,64] | `output_1` [1,1,1] (= 0.829), `output_2/3/6/7` [1,64] |

### Timings (CPU EP, intra-op 4 threads, 3 warm-ups + 20 runs; ms)

| model | inputs fed | outputs (shape) | median ms | p90 ms | min ms | session load ms | load avg (1 m) at run; M2 thread sweep |
|---|---|---|---:|---:|---:|---:|---|
| M1 | waveforms [1,80000] + lens [80000] | features [1,128,501]; features_lens [1] | 7.054 | 7.247 | 6.210 | 40 | 26 |
| M2 | features [1,128,501] (= M1 out) + length [501] | outputs [1,1024,63]; encoded_lengths [1] | 211.138 | 238.582 | 198.077 | 1397 | 24 1 thread: median 574.4 (p90 710.3); ORT default threads: median 181.6 (p90 197.0) — 10 runs each |
| M3 | enc frame [1,1024,1] (M2 frame 31/63), targets [[42]], len [1], states 0 | outputs [1,1,1,8198]; prednet_lengths [1]; output_states_1 [2,1,640] … | 0.264 | 0.272 | 0.256 | 22 | 22 |
| M4 | chunk [1,128,65] (9 zero + 56 mel), len [65], caches 0, cache_len [0] | encoded [1,1024,7]; encoded_len [1]; cache_last_channel_next [24,1,70,1024] … | 81.871 | 85.638 | 70.972 | 1292 | 24 |
| M5 | enc frame [1,1024,1] (M4 frame 3/7), targets [[42]], len [1], states 0 | outputs [1,1,1,1025]; prednet_lengths [1]; output_states_1 [2,1,640] … | 0.140 | 0.141 | 0.137 | 14 | 24 |
| M6 | log-mel [1,1000,128] (10 s via M1), cached_embeds [1,0,512], mask ones [1,125] | logits [1,1000,8]; chunk_embeds [1,125,512]; silence_embeds [512] | 235.799 | 332.932 | 217.356 | 107 | 24 |
| M7 | waveform [1,1,80000] | output [1,293,7] | 22.984 | 24.339 | 18.111 | 15 | 26 |
| M8 | fbank [1,498,80] RandomState(1) N(0,1) | embs [1,512] | 54.799 | 57.740 | 48.254 | 329 | 26 |
| M9 | features [1,3,41] RandomState(2) N(0,1), 4 states [1,64] zeros | output_1 [1,1,1]; output_2 [1,64]; output_3 [1,64] … | 0.054 | 0.057 | 0.053 | 6 | 26 |

- **M2 vs the known in-app number (128–136 ms per 5 s clip, ORT 1.27 CPU EP, M3 Max, quiet machine)**: here 211 ms median at 4 threads, **182 ms with ORT's default thread pool**, 574 ms single-threaded — measured with ORT 1.30 while the 1-minute load average was 21–26 on 16 cores (other sessions compiling), so the default-thread figure is ~1.35–1.4× the in-app number and not a clean comparison; the single-thread 574 ms is the more stable Burn baseline. Session creation for M2 took 1.4 s (M4 1.3 s).
- M6 at 236 ms for 10 s of audio (p90 333 ms — the noisiest run) ≈ 1.4 s per minute under this load; the branch note's bench was 0.75 s/min on a quiet M3 Max.
- Per-chunk M4 = 82 ms per 560 ms chunk at 4 threads; M3/M5 (one decode step) 0.26 / 0.14 ms; M9 (one 16 ms VAD frame) 0.054 ms.

## 4. Things worth knowing before converting

1. **Three different quantisation schemes across the five quantised graphs.** M2 and M6 use `com.microsoft::MatMulNBits` with `bits=8, block_size=32` (M2: `accuracy_level=4`, inputs A/B[uint8 N×K/32×32]/scales, **no zero points**, 3 shapes: 1024×1024 ×120, 4096→1024 ×49, 1024→4096 ×48; M6: 7 shapes, **with** uint8 zero points `[N, K/32]`, no `accuracy_level`). M4 is ORT static-free "dynamic" quantisation: `DynamicQuantizeLinear` (activation → uint8 + scale + zero point) feeding `MatMulInteger` with **int8** weight initializers (`[1024,1024]` ×120, `[1024,4096]` ×48, `[4096,1024]` ×48, `[4352,1024]` ×1) and scalar int8 weight zero points, then `Cast`/`Mul` by scales (the 605 `Mul`, 378 `Cast`). M3/M5 use the same dynamic path for the three joint-net matmuls (M3 weights are **uint8**, M5 **int8**) plus `com.microsoft::DynamicQuantizeLSTM` for the 2-layer prediction net (hidden 640, int8 W/R `[1,640,2560]`). Burn has `MatMulInteger` ✅ but `DynamicQuantizeLinear` ❌ — i.e. M4 is one small op away (a per-tensor dynamic uint8 quantiser) from a fully supported graph, whereas M2/M6 need a MatMulNBits (block-wise 8-bit dequant + matmul) hook.
2. **The "INT8" encoders are mostly float32 by bytes.** M2: 532.7 MB of uint8 MatMulNBits blobs + 412.3 MB float32 (all 72 pointwise `Conv` 1×1 weights are kept float32 — 24×(8.4+4.2) MB ≈ 302 MB — plus a 41.0 MB `[1,9999,1024]` relative-position table `onnx::Slice_780`). M4: 532.9 MB int8 + 304.8 MB float32 initializers + **41.0 MB of float32 inside 160 `Constant` nodes** (M4 has 2,257 `Constant` nodes in total, 2,094 of them int64 shape scalars).
3. **M6 is a transformer with HF-style fused ops**: 31 layers × (`MatMulNBits` qkv/o/fc1/fc2 + 2 `RotaryEmbedding` with cos/sin caches `[5000,32]`, `interleaved=0` + 2 `SkipLayerNormalization` ε=1e-5 with a 3-input fused bias). Only 2 plain `LayerNormalization`, 1 `Conv1d` (the 192→1536 upsampler), 2 `Mod` on int64 shape math. Its 606 external tensors cover the whole `.onnx_data` file byte-for-byte (sum of `external_data.length` = 120,479,872 = file size).
4. **M1 is a single `STFT` op (opset 17) + mel matmul** — `STFT(signal, frame_step=int64 scalar, window=float64 Hann[512])`, `Pad` mode `reflect`, then `MatMul` with a `[257,128]` float32 filterbank, log, and per-feature mean/std normalisation. Burn has `DFT`, `HannWindow`, `MelWeightMatrix` but no `STFT` row.
5. **M7 has one `If`** (`If_117`: then-branch 1 `Conv`, else-branch 11 nodes — the SincNet first conv with/without padding) and declares **eight opset domains** (com.microsoft, ai.onnx.ml 4, training, nchwc, org.pytorch.aten…) of which only `ai.onnx` 17 is used. The LSTMs are 4 bidirectional layers, hidden 128, `layout=0`, with initial h/c passed as activations.
6. **M8 is 3,207 nodes of which ~2,600 are shape plumbing**: 795 `Constant`, 265 `Shape`, 213 `Gather`, 158 `Concat`, 369 `Unsqueeze`, 204 `Identity`, and 52 repeats of `ConstantOfShape/Expand/Equal/Where/Pad/Slice/AveragePool(kernel 100, ceil_mode=1)` — the CAM++ context-aware masking pooling. Real compute is 225 `Conv` (213 `Conv1d`, 12 `Conv2d`, all `group=1`) + 56 `BatchNormalization`. Opset 14, ir 7, exported by PyTorch 1.12.1.
7. **M9 is opset 9 / ir 4 from tf2onnx 1.10** (old-style `Squeeze`/`Unsqueeze` with `axes` attributes; `LSTM` with explicit `activations`), 6 `Conv2d` of which 2 depthwise (`group=16`, strides 2×2, asymmetric pads `[0,1,0,1]` / `[0,0,0,1]`), 1 `MaxPool2d` kernel `[1,3]` stride `[1,2]`. Burn's table claims opsets 1–24.
8. **Dynamic shapes**: M2/M4 carry the usual `Shape→Gather→Concat→Reshape` chains (M2 100 `Shape`, M4 252) and attention masking via `Where` (M2: 72 with a scalar float initializer as the fill value; M4: 90 with all three operands computed at runtime); M4's `Range` ×6 / `Tile` / `Expand` ×19 build the streaming masks. M4's cache tensors are static `[24,1,70,1024]` and `[24,1,1024,8]` while `processed_signal` time is symbolic.
9. **No `Gemm`, no `Resize`, no `Loop`/`Scan`, no `Attention`-op, no fp16/bf16 tensors anywhere** — all float weights are float32, all quantised weights uint8/int8; M1's Hann window is float64.

## 5. Files

`M1.json … M9.json` (inventory), `M*.attrs.json` (attribute signatures), `inspect.py`, `attrs.py`, `ref.py`, `ref/<tag>/{spec.json,in_*.bin,out_*.bin}`, `ref/READY`, `models/ten-vad.onnx` (copy of M9), `venv/`.
