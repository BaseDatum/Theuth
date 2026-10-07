# 126 as-built — W0 spike: can candle 0.11 train a LoRA adapter through the vendored laya encoder on this Mac?

Branch `spike/126-w0-lora` off main `e5832a87`; worktree `/Users/Sky.Moore/Develop/theuth-126-w0`; target `/Users/Sky.Moore/Develop/theuth-126-w0-target`. Machine: Mac15,8 (M3 Max class), 64 GB unified memory, macOS darwin 25.6.0, cargo 1.98.1, candle 0.11.0 (`Cargo.lock` pin). CUDA: not available on this machine and no GPU node in the mesh today — not measured.

## Verdict

**SUCCESS on mechanics, with a cost finding that changes the sizing.** Gradients flow end to end (forward → Choice cross-entropy on the decision head's logits → `backward()` → `AdamW::step`) through all 28 encoder layers on Metal (f16) with a rank-8 LoRA on `Wqkv`/`Wo`/`Wi`/`Wo`, the loss falls monotonically on a fixed batch (1.466 → 0.0001 in 50 steps at lr 1e-4, batch 2 × 256), the adapter saves as `safetensors` via `VarMap::save` (14.4 MB). **But** the design's sizing batch, 8 × 512, never completed a single step: the first attempt ran out of GPU memory in the debug build; the release build ran for 9.5 minutes without finishing step 1 and was stopped; a 2 × 256 run in the same session made the machine unusable for the owner, who stopped the spike. The CPU path was not measured (owner's stop). Section "Findings" has the numbers; "What W3 must know" has the mechanics; "Open items" lists what Sky must weigh before W3 is briefed.

## What was built, where

| file | what |
|---|---|
| `crates/decision/Cargo.toml:16-18` | feature `train = []`, off by default. `:56-58` `[[example]] lora_step` with `required-features = ["train"]`, so the example never builds into the default gate. |
| `crates/decision/src/candle/model.rs:96-103` | the rotary call: `rope` (default) vs `candle_nn::rotary_emb::rope_slow` (`train`), selected by `#[cfg]` `use` aliases inside `Rotary::apply`. |
| `crates/decision/src/candle/model.rs:27-28, 36-44` | `train` only: a local `softmax_last_dim` that is `candle_nn::ops::softmax(xs, D::Minus1)` (primitive-composed); default build imports the fused `candle_nn::ops::softmax_last_dim` unchanged. Used at `:179` (attention) and `:632` (probability features). |
| `crates/decision/src/candle/model.rs:46-53, 504, 506, 621` | `head_norm`: the three **biased** head norms (`norm1`, `norm2`, `scorer_norm`) go through `candle_nn::ops::layer_norm_slow` under `train`; default build calls `LayerNorm::forward` exactly as before. Encoder norms are bias-free and already take candle's primitive path (candle-nn-0.11.0/src/layer_norm.rs:118-122). |
| `crates/decision/examples/lora_step.rs` | the spike: loader (`:126` `model_dir`, same env rules as `tests/model.rs:19-35`), `LoraBackend` (`:42-79`), probe (`:157-192`), 50-step loop (`:194-399`). |

Deviation from the brief's "nothing else in model.rs": two more fused ops had to be swapped (below). Both swaps are `#[cfg(feature = "train")]`; the default forward is byte-identical (same functions, same call order) and `cargo test -p decision` with default features is the proof (gates below). No `lora.rs`; the wrapper lives in the example.

## Method and findings

### 1. Baseline

`BUILD_LABEL=126-W0 build-lock cargo test -p decision` at `e5832a87`: lib **88 passed**, `tests/decide.rs` 2, `tests/gliner.rs` 3, `tests/model.rs` 3, doc-tests 0 — 96 passed, 0 failed, 0 ignored; 660 s for the lib (the GLiNER CPU parity test alone runs > 60 s). No warnings.

### 2. The blocker, empirically — and it is not what the design said

The design (§7.3, §7.6) predicted `Error::BackwardNotSupported { op: "rotary-emb" }` from the first attention layer. **That is wrong for candle 0.11.** `rope` is applied with `Tensor::apply_op3_no_bwd` (candle-nn-0.11.0/src/rotary_emb.rs:580), which builds the result with `BackpropOp::none()` (candle-core-0.11.0/src/custom_op.rs:170-180). The op is therefore **not in the graph at all**: `backward()` returns `Ok`, and no gradient reaches anything upstream of it. The `CustomOp3::bwd` default that returns `BackwardNotSupported` (candle-core-0.11.0/src/custom_op.rs:142-151) is only reached through `apply_op3`/`apply_op3_arc` (`:216-243`), which `rope` does not use. Same mechanism for two more ops the forward uses:

| op in the forward | call site (model.rs, default build) | candle-nn 0.11 | what `backward()` does |
|---|---|---|---|
| `rotary_emb::rope` | `Rotary::apply` :102-103 | `apply_op3_no_bwd` (rotary_emb.rs:580) | `Ok`, gradient silently dropped |
| `ops::softmax_last_dim` | `attention` :179, probabilities :632 | `apply_op1_no_bwd` (ops.rs:437-438) | `Ok`, gradient silently dropped |
| biased `LayerNorm::forward` → `ops::layer_norm` | head `norm1`/`norm2`/`scorer_norm` | `apply_op3_no_bwd` (ops.rs:932-944; layer_norm.rs:118-122 takes this path only when a bias exists and the input is contiguous) | `Ok`, gradient silently dropped |

Probe output (`lora_step --probe`, `(1, 16, 8, 64)` f32 tensor, same on Metal and CPU):

```
rope on Metal: backward Ok but NO gradient reaches the input (graph cut)
rope_slow on Metal: backward ok, input grad [1, 16, 8, 64]
softmax_last_dim on Metal: backward Ok but NO gradient reaches the input (graph cut)
softmax(D::Minus1) on Metal: backward ok, input grad [1, 16, 8, 64]
LayerNorm(bias).forward on Metal: backward Ok but NO gradient reaches the input (graph cut)
layer_norm_slow on Metal: backward ok, input grad [1, 16, 8, 64]
```

(`rope … on Cpu` lines identical.) Consequence seen on the full model before the second and third swaps: a 5-step Metal run with only `rope_slow` printed `loss 1.3845` five times, `bwd_ms 1`, `opt_ms 0`, and every LoRA `Var` had `grad none` — training would have looked like "it runs" and done nothing. **W3 must grep the forward for every `*_no_bwd` entry point, not for `BackwardNotSupported`.** The remaining ops in the loss path (`gather`, `where_cond`, `matmul`, `gelu_erf`, `relu`, `cat`, `narrow`, `broadcast_*`, `to_dtype`, `sqr/sum/sqrt`) are core ops with backward (candle-core-0.11.0/src/backprop.rs). `sort_last_dim` (model.rs `:646`) is cut too but only feeds the act head, which the loss does not use — as the design said.

After all three swaps, step 1 of the full model on Metal:

```
layer 0  Wqkv.lora_b grad norm q 2.37e-1 | k 2.83e-1 | v 5.65e-1
layer 27 Wqkv.lora_b grad norm q 1.10e-2 | k 9.87e-3 | v 8.59e-3
```

q and k (through rope) and v (bypassing it) all receive gradient at the first and last layer — the gradient crosses the whole encoder.

### 3. The 50-step runs

All runs: rank 8 on 112 matrices, 3,598,336 trainable f32 params (14.4 MB), base 421,293,830 params frozen in f16, fixed synthetic batch (random ids, no padding, 4 Choice slots, random labels), `AdamW` default betas, Metal f16, release build. Per-step `total_ms` includes rebuilding the model from the `VarBuilder` (`load_ms`, ~40–90 ms: 112 rank-8 matmuls + the merged-weight adds), forward, backward, optimiser; `device.synchronize()` after each phase. Steady state = steps 4–50.

| device | batch × seq | rank | lr | ms/step (steady mean / min / max) | fwd / bwd / opt ms | peak RSS (`/usr/bin/time -l` max RSS) | peak phys footprint | loss step 1 / 25 / 50 | exit |
|---|---|---|---|---|---|---|---|---|---|
| Metal f16 | 2 × 256 | 8 | 1e-3 | 1,421 / 1,300 / 1,820 | ~210 / ~1,000 / ~90 | 1,858 MB | 12,183 MB | 1.4660 / 0.0361 / 1.4868 | 1 (loss oscillated: fell to 0.0026 at step 30, diverged to 1.39–1.65 by 42–50; lr too high for a 2-row batch) |
| Metal f16 | 2 × 256 | 8 | 1e-4 | **1,089 / 1,043 / 1,158** | ~170 / ~750 / ~85 | 1,874 MB | 12,180 MB | **1.4660 / 0.0012 / 0.0001** (monotone) | **0**; adapter saved, 14,417,560 bytes |
| Metal f16 | 2 × 256 | 8 | 1e-3, earlier run with f32 delta* | 894–1,187 (steps 4–33, run interrupted by an engine restart) | ~200 / ~700 / ~30 | — | — | 1.4660 / 0.0054 / — | — |
| Metal f16, **debug** build, f32 delta* | 2 × 256 | 8 | 1e-3 | 5,427 (steps 4–5 only) | 180 / 4,500 / 730 | 1,759 MB | 23,218 MB | 1.4660 / — / — (0.1151 at step 5) | — |
| Metal f16, debug, f32 delta*, rope_slow only | 8 × 512 | 8 | 1e-3 | 2,361 **forward-only** (no gradient flowed, see §2) | 2,200 / 1 / 0 | 1,735 MB | 26,548 MB | 1.3845 flat | 1 |
| Metal f16, debug, all swaps | 8 × 512 | 8 | 1e-3 | — | — | — | — | — | `Metal error Command buffer had following error: Insufficient Memory (kIOGPUCommandBufferCallbackErrorOutOfMemory)` before step 1 |
| Metal f16, release, all swaps | 8 × 512 | 8 | 1e-4 | **run stopped by the owner — machine unusable while it ran.** Did not finish step 1 within 9.5 min (SIGTERM at 580 s). | — | — | — | — | 143 |
| CPU f32 | any | — | — | **not run** (owner's stop; the Metal run alone saturated the machine) | | | | | |

\* "f32 delta": `LoraBackend` first computed `B·A` in f32 and cast the weight-sized result to f16; commit `151172a4` casts the factors first so delta and its gradient are f16 — that halved `bwd_ms` (≈1,000 → ≈750) and the footprint (23 GB → 12 GB) at 2 × 256.

Three facts, not estimates:

- **Memory.** The 2 × 256 step costs a 12.2 GB physical footprint (Metal buffers are counted in phys_footprint, not RSS; RSS stays at the 1.9 GB mmapped checkpoint). Candle's Metal allocator keeps every buffer it ever allocated in size buckets and only reuses, never frees, until `drop_unused_buffers` runs on the next allocation miss (candle-core-0.11.0/src/metal_backend/device.rs:131-154, 294-310), so the footprint is the high-water mark of the backward graph. 8 × 512 is 16× the tokens of 2 × 256 with attention scores 4× wider per token: that is the OOM in the debug build and, in release, a step that did not finish in 9.5 min — almost certainly swapping unified memory. Attention scores at 8 × 512 are `8 × 16 × 512 × 512` f16 = 64 MB per layer per tensor, kept for 28 layers × (scores, softmax, mask-add) in the graph; the MLP intermediate `8 × 512 × 5248` f16 = 43 MB × several per layer. Without activation checkpointing (candle has none) the graph is tens of GB at this batch.
- **Interactivity.** Even the 2 × 256 run — ~1 s/step, 50 steps, ~1 min — made the Mac unusable for the owner while it ran. This is the finding that matters for §8.2's "local preconditions": a training step that saturates the GPU is not something that can run while the owner works, regardless of the step time. Any local training must run only when the machine is idle (and the mesh venue becomes the primary design, not the alternative).
- **Throughput at the only measured size.** 1.09 s/step at batch 2 → 0.54 s/row → a **5,000-row epoch ≈ 45 min** at 2 × 256 (design estimate was 25 min at 8 × 512; that batch is not reachable in memory here). Longer sequences cost more than linearly (attention), so real rows (median card text is well under 256 tokens for approvals, longer for asks) will land either side of this.

Loss trajectory, the exit-0 run (lr 1e-4, 2 × 256, every 5th step): 1.4660, 1.2403 (5), 0.9845 (10), 0.5 (15, approx.), 0.08 (20, approx.), 0.0012 (25), … 0.0001 (50) — monotone, no NaN. (Exact per-step lines for steps 1–3/10/25/50 in the table; the full log files under `/tmp/126-w0/` were lost to the engine restarts — these numbers are transcribed from the run output as it printed in the session.)

### 4. Other ops lacking a backward

Two beyond rope, both found by the probe and fixed with the candle-provided `*_slow`/primitive siblings (table in §2). No rewrite of model math was needed; nothing was vendored. `sort_last_dim` remains cut and is harmless for this loss.

### 5. Gates after the change

| gate | result |
|---|---|
| `build-lock cargo test -p decision` (default features) | see "Gate log" below |
| `build-lock cargo clippy -p decision --features train --all-targets -- -D warnings` | see "Gate log" below |
| `build-lock cargo build [--release] -p decision --features train --example lora_step` | clean, no warnings |

Gate log (both ran after the owner's stop, both through build-lock, at `3df3955d`):

- `cargo clippy -p decision --features train --all-targets -- -D warnings`: **clean**, exit 0 (first run flagged `clippy::type_complexity` on the probe's case vector; fixed with a local `type Case`).
- `cargo test -p decision` (default features): lib **88 passed** (859.8 s), `tests/decide.rs` 2, `tests/gliner.rs` 3, `tests/model.rs` 3 (readme parity against the real checkpoint on Metal included), doc-tests 0 — **96 passed, 0 failed, 0 ignored**, no warnings; identical counts to the baseline. The default forward is unchanged.

## What W3 must know

- **Loader.** `~/.theuth/models/decision/` (`$DECISION_MODEL_DIR` overrides, `$THEUTH_DATA_DIR` moves the data dir — `tests/model.rs:19-35`), files per `engine::model_files(Kind::Laya)`. The checkpoint is 205 f16 tensors + 1 f32, 842,609,210 bytes, mmapped by `MmapedSafetensors` behind `weights::SanitizedBackend` (`candle/mod.rs:214-228`). The example loads every tensor to the device once (`get_unchecked`, 1.4 s on Metal) and then never touches the file.
- **Frozen vs trained.** Every checkpoint tensor is a plain `Tensor` (no `Var`), so it is frozen by construction; `backward()` only populates grads for `Var`s and their ancestors. Trainable: `encoder.layers.{0..27}.{attn.Wqkv,attn.Wo,mlp.Wi,mlp.Wo}.weight.lora_{a,b}` — `A (r, in)` init `Randn(0, 1/√in)`, `B (out, r)` init zero, scale α/r = 2 (α = 2r), f32 `Var`s in a `candle_nn::VarMap`. The decision head, scorer, act head, embeddings and norms are untouched.
- **How the adapter is attached in the spike, and how it should be in lora.rs.** The vendored `Linear`s are private fields, so the example applies LoRA *through the `VarBuilder`*: `LoraBackend::get` returns `W + (α/r · B)·A` as a graph node for the four adapted names and rebuilds `DecisionModel` from the builder every step (`lora_step.rs:42-79, 317`). That is mathematically the same adapter but materialises 112 merged f16 weights per step (`load_ms` ~40–90 ms, and the merged weights plus their gradients sit in the graph — part of the memory cost). W3's `lora.rs` should wrap the `Linear` (`y = xW^T + (α/r)(xA^T)B^T`) so the forward never materialises `B·A`; that needs `EncoderAttention`/`EncoderMlp` to take a `LoraLinear` under `train` — a model.rs change W0 did not make (brief scope). Inference with a trained adapter can use the merge-through-`VarBuilder` trick once at load (no graph) — it is the cheaper option for the shipped predictor.
- **Saving.** `VarMap::save(path)` writes the 224 `lora_a`/`lora_b` tensors as safetensors — 14,417,560 bytes at rank 8 (`lora_step.rs:391`). Load with `VarMap::load` into the same names.
- **`train` feature mechanics.** `--features train` swaps three functions (`rope → rope_slow`, fused `softmax_last_dim → softmax(D::Minus1)`, biased fused `LayerNorm → layer_norm_slow`). `rope_slow` builds `cat([cos,cos])`, `rotate_half`, two broadcast muls per call — slower than the fused kernel; not measured against the default forward (forward-only comparison not run). The trained adapter is used by the *default* build at inference (the default kernels compute the same function), so the feature only gates the trainer binary.
- **Loss and dtypes.** `candle_nn::loss::cross_entropy` over `ModelOutput.logits` (f32, masked to −1e4 on unused slots). The adapter's `Var`s are f32; the delta is cast to f16 before the add so the merged weight matches the model dtype. AdamW state lives in f32 on the device.
- **lr.** 1e-3 oscillated/diverged on a 2-row batch; 1e-4 was monotone. With real batches and a schedule, start at 1e-4.

## Open items / hand-offs (for Sky, before W3 is briefed)

1. **The design's cost model is off by the batch dimension.** 8 × 512 does not fit (OOM at f16, debug) or does not finish a step (release, 9.5 min) in 64 GB with candle 0.11's graph (no activation checkpointing, no buffer freeing within a step). What works: 2 × 256 at 1.09 s/step → 45 min per 5,000-row epoch *if* the machine is otherwise idle. Options, costed, no decision made here: (a) gradient accumulation at micro-batch 2 (free, keeps rows/epoch, ~same wall time); (b) cap the corpus harder via the rehearsal sampler (§7.3 already does; 2,000 rows → 18 min); (c) a CUDA/mesh venue as the *primary* path for Tier B (the design listed it as "preferred when one exists"; there is none today).
2. **Local training is incompatible with the owner using the machine.** Even the 1-minute 2 × 256 run was stopped because the Mac became unusable. §8.2's preconditions (idle, power, thermal) are necessary, not nice-to-have, and candle offers no way to throttle a Metal command buffer. If local Tier B stays in the design it needs an idle detector that *aborts* the run on activity (not just one that gates its start) — W3 scope, but Sky should rule whether local Tier B is in v1 at all.
3. **CPU/Accelerate path unmeasured.** The brief asked for it; the owner's stop pre-empted it. [INFERENCE] From the forward's known ratio (Metal f16 74–111 ms/question vs CPU f32 several ×), CPU would be slower per step but would not touch the GPU — still a full-core load. Not a fallback decision (Revision 1 ruling 9); listed so Sky knows it is unmeasured.
4. **Full logs lost.** `/tmp/126-w0/*.txt` and the saved `/tmp/126-w0-adapter/adapter.safetensors` were wiped by the two engine restarts (10:19, 10:56–11:07); numbers above were transcribed from the run output in the session. Re-running to regenerate them is explicitly not allowed on this machine (owner's stop). The example reproduces them: `cargo run --release -p decision --features train --example lora_step -- --device metal --batch 2 --seq 256 --lr 1e-4 --save DIR` (do not run it while the owner is using the machine).
5. **Hand-off to the C-contract sibling (`crates/decision/src/candle/model.rs`).** W0's model.rs edits are confined to: the import block `:23-28`, the two helpers `:36-53`, `Rotary::apply` `:96-103`, and the three `head_norm` call sites `:504, :506, :621`. The sibling's `ModelOutput`/pooled-state change should merge without conflict; if it adds a `softmax_last_dim` call it should use the local one (unqualified name) so the `train` build stays differentiable.
6. **Design corrections for the next revision of 126-double.md §7.3/§7.6.** "`rope` … returns `BackwardNotSupported`" → "`rope`, `softmax_last_dim` and the biased `LayerNorm` are applied with `apply_op*_no_bwd` and cut the graph silently; the `*_slow`/primitive siblings are differentiable." "a 5,000-row epoch ≈ 25 min at 8 × 512" → "8 × 512 does not fit; 45 min at 2 × 256, machine unusable while running."

**Decision required from Sky on items 1–2 before W3 is briefed.** The mechanics are proven; the venue and the batch are not what the design assumed.
