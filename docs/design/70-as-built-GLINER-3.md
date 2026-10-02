# 70 as built — GLINER-3: the GLiNER2.5-Decide backend — ONNX spike as measured baseline, candle port as the engine

Branch `feat/gliner-backend` off `origin/main` `840012b6` (GLINER-1 + GLINER-2 merged). Brief:
`/tmp/69/brief-GLINER-3.md`; report with the full tables and the A-vs-B recommendation:
`/tmp/69/report-GLINER-3.md`. Model `fastino/GLiNER2.5-Decide` @ `5a7adf72` (57 §2.2), weights now under
`~/.theuth/models/decision-gliner/` (`model.safetensors` 1,945,828,140 B, sha256
`40a5a23f…6dc997`, 419 f32 tensors, 486,444,053 params — header verified: `encoder.*` 390,
`classifier.{0,2}.{weight,bias}`, plus unused `span_rep.*` 12 / `count_embed.*` 9 / `count_pred.*` 4).

**Decision: route B (candle) is the engine; route A (ORT) stays a feature-gated spike.** Metal f32 candle
answers a short question in 55 ms and a 512-token one in ~330 ms with the same probabilities as Python to
1e-6; ORT fp32 on the CPU is 75–634 ms, int8 is faster (38–346 ms) but breaks parity (7/51 argmax flips),
and the CoreML EP either throws at session init or runs 2–196 partitions slower than the CPU.

## What changed

- **`Cargo.toml`** `candle-transformers = { version = "0.11", default-features = false }` (workspace, `:120`);
  **`crates/decision/Cargo.toml`** adds it (+ `metal`/`accelerate` on macOS, `cuda` forwarded by the
  `cuda` feature) and the new feature **`gliner-onnx = ["dep:ort"]`** (`:15`, off by default — `ort` is the
  workspace pin voice uses; `decision`'s default build stays ORT-free).
- **`crates/decision/src/gliner/mod.rs`** (rewritten; the GLINER-1 `Rendered` row and `NOT_BUILT` stub are
  gone): `impl Row for prompt::GlinerRow` (`:24`); `Batch` + `collate(rows)` (`:46`, `:62`: right-padded
  ids/mask with `[PAD]` 0, `label_pos` padded with 0); `scores(rows, logits, n)` (`:88`: first
  `label_count` logits per row, softmax, temperature 1, `act_p` 0; a **Bool row's `[yes, no]` is reversed
  to the engine's `[false, true]`** — `laya::labels` order, which `Scores::confidence` and `decide` assume);
  `downcast` (`:114`); `pub use candle::Loaded`; test-only `reference` module (`:127`: loads
  `fixtures/reference.json`, finds the checkpoint, `compare` = max |Δp| + argmax flips, Bool reversed).
- **`crates/decision/src/gliner/prompt.rs`** `GlinerRow.is_bool` (`:160-162`, set by `build_row` `:290`).
- **`crates/decision/src/gliner/candle.rs`** (new, production): `Loaded { encoder: DebertaV2Model, head_in,
  head_out, device, dtype, tokenizer, cfg, model_sha, load_time }` (`:34`); `verify_header` (`:56`:
  embedding `vocab×hidden`, `rel_embeddings 2·buckets×hidden`, last layer, `classifier.0 [2048,1024]`,
  `classifier.2 [1,2048]`, all floats); `open(dir, gpu)` (`:100`) = `open_as(dir, gpu, F32)` (`:106`):
  device pick + CPU fallback as laya's, `tokenizer.json` + `GlinerSpecial::from_tokenizer`,
  `encoder_config/config.json` → `debertav2::Config` (checked `relative_attention`, `type_vocab_size 0`),
  header check, `VarBuilder::from_mmaped_safetensors` → `DebertaV2Model::load(vb.pp("encoder"))` +
  `candle_nn::linear` ×2 from `classifier.0`/`classifier.2`, background `sha256_cached` (shared with laya,
  now `pub(crate)`); `run(rows)` (`:194`): collate → `[B,S]` i64 ids/mask, encoder forward with the mask,
  `gather` at `[B,N,H]`, `head_out(relu(head_in(x)))`, `scores`; `impl Oracle` (`:223`).
  **Dtype is f32 on every device**: candle-transformers 0.11's DeBERTa is f32-only —
  `disentangled_attention_bias` seeds `score` with `Tensor::new(&[0f32])` and `broadcast_add`s the f16 c2p
  term, so Metal f16 fails in layer 0 with **`dtype mismatch in add, lhs: F32, rhs: F16`** (measured;
  `XSoftmax`'s `f32::MIN` `where_cond` would be next). `tests::metal_f16_is_documented_unsupported`
  (`:317`) prints the error and fails loudly the day it starts working.
- **`crates/decision/src/gliner/onnx.rs`** (new, `#[cfg(feature = "gliner-onnx")]`): `Ep::{Cpu{threads},
  CoreML{ml_program, all_units, static_shapes, cache_dir}}` (`:24`), `Partitions` parsed from ORT's INFO
  logger (`:45`), `Loaded::open(model, dir, data_dir, ep)` (`:74`: `models::onnx::ort_lib(data_dir)` for
  the dylib, `Session::builder` Level3 + session logger, CoreML option builder from `ort::ep::coreml`),
  `run` (`:167`: `input_ids`/`attention_mask`/`marker_positions` i64 → `logits [B,N]`), `impl Oracle`.
- **`crates/decision/src/engine.rs`** `model_files(Kind::Gliner)` = `["model.safetensors", "tokenizer.json",
  "encoder_config/config.json"]` (`:32-46`; the top-level `config.json` holds nothing the loader reads, the
  DeBERTa hyper-parameters are only in the nested file — nested paths are already the laya convention);
  `open` (`:627`) passes `cfg.device.gpu()` to `gliner::Loaded::open`. **`lib.rs`** test
  `gliner_kind_parses_and_mode_for_checks_its_files` follows the list (`:784-806`).
- **`crates/decision/src/gliner/fixtures/reference.json`** (new, 158 KB, 51 cases) from
  **`scripts/tests/gliner-reference.py`** (new): the 13 golden questions + 38 more (Choice 2/3/4/5/12/48,
  Bool, Score 3/5, ~400-subword states, two 900-subword states cut at 512, CJK/Spanish/German/French/emoji,
  empty-text ids, literal markers in the state); per case question, labels, `input_ids`, `label_pos`,
  `state_cut`, fp32 CPU `logits` and `probs` from `model.encoder(...)` → gather → `model.classifier` →
  softmax, each cross-checked against `GLiNER2.classify_text(include_confidence=True)` (same winner, same
  confidence to 1e-6). Cut states are the Rust row (subword-granular) fed to the Python model — gliner2
  itself never truncates.
- **`crates/decision/examples/gliner_bench.rs`** (new): `--backend candle|ort --device metal|cpu --model
  <onnx> --threads N --coreml <fmt,units,shapes> --iters N`; cold load, footprint, `ru_maxrss` peak, p50/p95
  for 1/8/48 labels × ~50/~400-subword states; one process per cell.
- **`crates/decision/tests/gliner.rs`** + **`tests/fixtures/gliner.yaml`** (new): `checkpoints.gliner:
  { kind: gliner }` and three `guard.*` shadow families on it; `fixture_resolves_the_gliner_checkpoint`
  (no model), `ask_many_through_the_actor_in_shadow` (model-gated: Bool/Choice/Score through `Engine::ask
  (AskMany)` → `Decision` shape, label orders `[false,true]` / ids+`none` / level indices, log rows with the
  gliner sha, k, logits, `load_ms`), `release_reclaims_the_checkpoint` (mirror of `tests/model.rs`: cycle 2
  before 16 MB → loaded 2004 MB → released 59 MB in 0.25 s).
- **`scripts/fetch-models.sh`** three `decision-gliner/*` REQUIRED lines (`:65-67`) and the `origin_of` arm
  `hf:fastino/GLiNER2.5-Decide/${1#*/}` (`:81`); **`scripts/tests/fetch-models-test.sh`** counts 20 → 23
  files / 13 → 16 not checkable, and its browser 404 expectation `v10` → `v17s` (it had not followed the
  v17s bump — the test failed on main). `release-models.sh` sources REQUIRED; no edit.
- **`docs/design/57-decision.md`** §2.2 "GLiNER2.5-Decide checkpoint" (hub id, revision, file table with
  sizes/sha256, what the loader reads); **`crates/theuth/src/main.rs:1049-1050`** generated-config comment.
- `decision_loaded` / Labs: no change needed — `LoadedInfo` is per engine (`checkpoint` name, footprint),
  so a resident `gliner` checkpoint already shows as `gliner · <MB>`.

## Measured (M3 Max, 10 iterations after 2 warm-ups, one process per row; `peak` = `ru_maxrss`, `fp` = `phys_footprint`)

| backend | cold load | fp after load | peak | 1 label S72 | 1 label S471 | 8 labels S102 | 8 labels S501 | 48 labels S262 | 48 labels S512 | parity vs Python (51 cases) |
|---|---|---|---|---|---|---|---|---|---|---|
| **candle Metal f32** | **294 ms** | 1880 MiB | 3590 MiB | **55 / 64** | 303 / 318 | 60 / 63 | 335 / 354 | 130 / 138 | 328 / 343 | **max \|Δp\| 8.9e-7, 0 flips** |
| candle CPU f32 (Accelerate) | 262 ms | 1866 MiB | 3570 MiB | 209 / 220 | 1774 / 1873 | 273 / 274 | 1970 / 1977 | 764 / 767 | 1978 / 2043 | max \|Δp\| 1.2e-6, 0 flips |
| ORT fp32 CPU, 8 threads | 1195 ms | 1603 MiB | 2022 MiB | 75 / 82 | 562 / 618 | 111 / 111 | 602 / 666 | 312 / 318 | 634 / 699 | max \|Δp\| 2.5e-6, 0 flips |
| ORT fp16 CPU (f32 I/O) | 329 ms | 385 → 2431 MiB | 4122 MiB | 178 / 194 | 689 / 691 | 225 / 227 | 717 / 722 | 392 / 402 | 742 / 748 | max \|Δp\| 9.0e-4, 0 flips |
| ORT int8 (`quantize_dynamic`) CPU | 904 ms | 878 MiB | 1298 MiB | 38 / 39 | 317 / 332 | 58 / 64 | 346 / 348 | 153 / 155 | 344 / 357 | **max \|Δp\| 0.66, 7/51 flips** |

Cells are p50 / p95 ms. candle's 3.5 GiB `ru_maxrss` is the load (mmap pages + the f32 copy both resident
for a moment); the steady footprint is 1.9–2.2 GB on both devices. ORT's CoreML EP (fp32 export, 1363 nodes):
MLProgram + dynamic shapes (All or CPU+ANE) → **`HandleNegativeAxis … axis 2 is not in valid range [-2,1]`
at session init** (the same ORT bug as laya's); MLProgram + `RequireStaticInputShapes` → 2 partitions, 59
nodes on CoreML, 79–598 ms (CPU speed) and an Espresso "data-dependent shapes" exception for the 24
`repeat` ops; NeuralNetwork → 196 partitions / 539 nodes, 11.4 s load, 5.3–14 GB footprint, 162–1087 ms.
ONNX export re-run from the local 5a7adf72 weights with onnx-community's script (torch 2.14.1 dynamo,
opset 17): `model_fp32.onnx.data` 1,744,089,088 B sha256 `f5aef45f…ac8d58` (the community file's size
exactly), fp16 872,044,544 B, int8 589,566,976 B — `/tmp/69/gliner-onnx/manifest.json`.

## Verified

- `cargo test -p decision --release`: lib **76 passed, 0 failed** (69 → 76: `gliner::candle::tests::{parity_cpu_f32,
  parity_metal_f32, metal_f16_is_documented_unsupported}`, `gliner::tests::collate_…`, +3 from GLINER-2's
  tokenizer-gated tests now finding the files); `tests/gliner.rs` **3 passed**; `tests/model.rs` 3 passed.
  `--features gliner-onnx`: + `gliner::onnx::tests::{parity_fp32, parity_fp16_reported, parity_int8_reported}`
  (3 passed; int8 is report-only). Exact counts of the final default-profile run: report §Verification.
- `cargo check -p decision --tests` (default and `--features gliner-onnx`), `cargo check --workspace --tests`,
  `cargo clippy -p decision --no-deps --all-targets -- -D warnings` (both feature sets): clean.
- `scripts/tests/fetch-models-test.sh`: 4 ok; `./scripts/fetch-models.sh --check`: the three
  `decision-gliner/*` files `up to date (rev 5a7adf72…)`; `shellcheck -x -P scripts` clean.

## Deviations

- Metal **f32**, not f16: the candle-transformers DeBERTa port has no f16 path (op above). f16 would halve
  the footprint (~1 GB) — a follow-up patch to candle-transformers (`score` and the `XSoftmax` mask in the
  input dtype), not something this crate can work around.
- `model_files(Kind::Gliner)` drops `config.json` (GLINER-1's provisional list had it) for
  `encoder_config/config.json`: the loader reads only the latter.
- `fetch-models.sh` has no per-origin revision pin (laya/browser track `main` the same way); the pin is 57
  §2.2 + the `.origin` record; a new upstream commit shows as `STALE`.
- `cargo fmt --all` was not run: the toolchain here (rustfmt 1.9.0-stable) reformats 286 files on `main`;
  only the new/edited gliner files were formatted with `rustfmt --edition 2024`.
- Calibration is untouched (temperature 1.0, BCE-trained head): the per-family temperature fit is the later
  task the brief names.
