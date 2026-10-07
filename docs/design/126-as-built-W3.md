# 126 as-built — W3: the Tier B LoRA trainer, the node job entry, the local path behind a flag

Branch `feat/126-w3` (C's contract `5cdcbfde` + the four W0 spike commits, base `74977c67`); worktree `/Users/Sky.Moore/Develop/theuth-126-w3`; target `/Users/Sky.Moore/Develop/theuth-126-w3-target`. Rulings implemented: Revision 2 (2026-10-07 12:40) — node-first venue, `double.training.tiers.b.local` default off with abort-on-activity, micro-batch 2 × 256 with gradient accumulation to an effective batch of 16, rehearsal cap 2,000 rows, lr 1e-4, run cap 90 min. **No training run, no checkpoint load and no example run happened on this machine**: every test builds a 2-layer / hidden-64 / 4-head / vocab-128 model in memory on the CPU.

## What was built, where

| file | what |
|---|---|
| `crates/decision/src/candle/lora.rs` (new) | `TRAIN_BUILD` (the `train` feature as a constant); `ADAPTED` / `adapted_names`; `Rng` (deterministic xorshift64\*, Box–Muller, Fisher–Yates — the CPU backend cannot be seeded, `candle-core-0.11.0/src/cpu_backend/mod.rs:3096`); `Base` (the frozen checkpoint on one device: `open(dir, device, dtype)` = `Loaded::open`'s loader + every tensor uploaded + the sha256; `synthetic(device)` = the tiny fixture; `model(lora, train)`; `sequence(q, max_len)`); `LoraBackend` (`SimpleBackend` that merges `W + (α/r)·B·A` on lookup, in the model dtype, detached when `train = false`); `Lora` (`new(base, rank, seed)`, `load(base, path)`, `save`, `varmap`, `vars`, `param_count`, `pair`); `weighted_nll_sum` (the Choice loss with per-row weights, summed); `AdamW` (moments in a `VarMap`, `save`/`load` with the step counter). Tests: `gradient_reaches_every_third_of_wqkv_at_both_ends` (`train`), `default_build_cuts_the_graph_silently` (default), `adapter_round_trips_through_safetensors`, `adamw_state_round_trips`. |
| `crates/decision/src/candle/mod.rs` | `pub mod lora;`; `Gpu::open()` extracted from `Loaded::open` (the Metal / CUDA / CPU pick with the "no CUDA in this build" message), used by `Loaded::open` and `t double train-b`. |
| `crates/decision/examples/lora_step.rs` | now a thin client: `Base::open`, `Lora::new`, `base.model(Some(&lora), true)`, `weighted_nll_sum`, `lora::AdamW`; defaults changed to the measured 2 × 256 / lr 1e-4; refuses to run without `TRAIN_BUILD`. The probe is unchanged. **Not run.** |
| `crates/decision/Cargo.toml` | the `train` feature comment names `lora.rs` and the trainer binary. |
| `crates/double/src/train_b.rs` (new) | `Bundle` / `BundleRow` / `BundleManifest` (+ `read` / `write`); `TrainConfig` (Revision 2 defaults; `from_double(&DoubleConfig)`); `PauseSignal` with `Never` and `OwnerActivity { idle: Box<dyn IdleSeconds>, threshold_secs: 2 }`; `IdleSeconds` (the `// 126 I: bind to platform::idle` seam); `Hooks { pause, on_progress }`, `Progress`; `rehearsal` (the §7.3 sampler); `split` (last-7-days hold-out, capped at 20 %); `accumulate` (one merged model per step, `micro_batch` pieces, `GradStore::extend`, pause polled per piece); `score` / `holdout_metrics` (temperature grid, log-loss, accuracy, 10-bin ECE); checkpoint (`checkpoint/{adapter,optim}.safetensors` + `state.json`); `TierB::train(bundle, cfg, base, out, hooks) -> TrainOutcome::{Finished(CandidateVersion), Paused}`; `TierB::local_allowed(cfg)`; `rescore_golden(base, candidate, rows) -> GoldenMetrics` (`// 126 I: W2 gate consumes this`); `Manifest` / `Metrics` / `CandidateVersion`. |
| `crates/double/src/lib.rs` | `pub mod train_b;` at C's `// 126 W3` marker. |
| `crates/double/Cargo.toml` | deps `decision`, `candle-core`; dev-dep `decision` with `features = ["train"]` so `cargo test -p double` trains on the differentiable encoder while `t` stays on the fused kernels. |
| `crates/double/src/config.rs` | `TierB.local: bool` (default `false`, Revision 2 ruling 1); `TierB` defaults `batch: 16` (the effective batch, ruling 2) and `lr: 1e-4` (W0's stable lr) — C's were 8 / 2e-4 from the v0 body. C's `defaults_and_partial_yaml` test does not pin either. |
| `crates/theuth/src/main.rs` | `Cmd::Double { cmd: DoubleCmd }`, `DoubleCmd::TrainB { --bundle, --out, --device }` (clap derive, the `Cmd` enum at `:149`; dispatched in `run()` before `cmd_67` like `Cmd::Mcp`, so no engine is needed); `double_train_b(...) -> i32` (exit 0 candidate written · 2 bad input / no checkpoint / no `train` build · 3 run cap paused it · 1 other); the config.yaml template line for `tiers.b`. |

## Design decisions

- **Merged-through-the-`VarBuilder`, not a wrapped `Linear`.** The brief asked for unmerged inference. `EncoderAttention` / `EncoderMlp` hold private `Linear`s (`model.rs:234-240, 283-286`) and `model.rs` is out of W3's scope beyond the spike's edits, so the adapter is applied as the spike did: `LoraBackend::get` returns `W + (α/r)·B·A` as a graph node (training: one model rebuild per optimiser step, shared by the step's micro-batches) or as a detached constant (inference: one rebuild per adapter load). What the brief wanted from "unmerged" holds anyway: the base tensors are `Arc`-shared `Tensor` clones across every rebuild, only the 112 adapted matrices are materialised per adapter, and "swap the adapter per call" is `Base::model(Some(&lora), false)` — a 40–90 ms rebuild on the real model (W0's `load_ms`), no checkpoint copy. A true per-call swap under one model needs a `LoraLinear` in `model.rs`; hand-off below.
- **`lora.rs` is not behind the `train` feature** (brief: "behind the `train` feature"). The adapter must be *applied* by the default build at inference (W0: "the trained adapter is used by the default build"), and `Lora::load` + `Base::model(.., false)` need nothing training-specific. Only the differentiable forward is behind the feature; `TRAIN_BUILD` tells a trainer which build it is in, and `TierB::train` refuses to run without it (the W0 finding: the fused ops cut the graph *silently*, a run in the default build would report falling… nothing — the loss stays flat and every `Var` has `grad none`). The test `default_build_cuts_the_graph_silently` pins that behaviour so a candle upgrade that fixes it shows up.
- **Own `AdamW`.** candle-nn's keeps its moments private, so a checkpoint could not carry the optimiser state. `lora::AdamW` is the same decoupled-weight-decay update (betas 0.9 / 0.999, eps 1e-8, decay 0.01) over `Var`s with `m` / `v` in a `VarMap` — `save` / `load` are `safetensors` plus the step counter. ~60 lines.
- **Pause granularity = one micro-batch, not one optimiser step.** An optimiser step at effective 16 is eight 2 × 256 forwards/backwards (~9 s on the real model); the owner's GPU must come back within ~1 s. `hooks.pause` is polled before every micro-batch; a partial step's accumulated gradients are dropped and the checkpoint holds the last *completed* step (adapter + optimiser + `state.json` are always at a step boundary, so resume is exact).
- **Hold-out.** The brief's "held-out log-loss on the bundle's last-7-days rows": rows newer than `newest − 7 d` are held out of training, capped at 20 % of the trainable rows (a wave week puts the whole corpus inside 7 days). W2's gate (a) compares this log-loss to the active version's live log-loss on the same rows; holding them out makes the comparison honest.
- **Rehearsal.** Per class: the newest 200 plus a class-key-seeded sample of 200 from the rest; then a seeded subsample to 2,000. `ponytail:` marker on the second stage (uniform, not per-class-proportional).
- **Determinism.** Every random draw (adapter init, epoch order, rehearsal sample) goes through `lora::Rng` seeded from `TrainConfig.seed` (recorded in the manifest), because `Device::set_seed` bails on the CPU backend and candle's `Init::Randn` uses `rand::rng()` there. Two runs with the same bundle, seed and device agree to float rounding (f16 on Metal, §7.5's honest limit). The synthetic fixture's *base* weights still come from candle's inits (fine: one base per test).
- **The card as the model reads it** (`train_b::question`): `Choice { instructions: <class key>, candidates: <options as ids>, state: <card_text> }` with `engine::with_none`. `// 126 I` marker: W1's `predict.rs` must render the same prompt or the adapter trains on a different sequence than it predicts on.

## The tiny-model proofs (`cargo test -p double`, CPU f32, 2 layers / hidden 64 / 4 heads / vocab 128, rank 4)

| test | what it proves | numbers |
|---|---|---|
| `train_b::tests::accumulation_matches_one_full_batch` | 8 micro-batches of 2 (rows of different lengths, so padding differs) give the same loss and the same gradient on every adapter `Var` as one batch of 16 | GATE_ACC |
| `train_b::tests::pauses_within_one_micro_batch_and_resumes_exactly` | a fake idle clock flips to "active" at the 6th poll (= step 2's second micro-batch): the run returns `Paused { step: 1 }` having called `on_progress` once, the checkpoint holds `adapter.safetensors` + `optim.safetensors` + `state.json`; the resume with `Never` emits steps 2..=8 and finishes at 8 steps; the final adapter equals the uninterrupted run's | GATE_PAUSE |
| `train_b::tests::owner_activity_fires_on_the_first_poll_after_activity` | `OwnerActivity` returns pause on the first poll after the idle clock drops below 2 s | latency = 1 poll = 1 micro-batch |
| `train_b::tests::loss_falls_and_manifest_carries_the_gate_metrics` | 40 rows (4 golden) through an on-disk bundle: 29 train / 7 hold-out / 0 unlabelled, 24 steps over 3 epochs, the mean loss of the last 3 steps is below the first 3; manifest round-trips; `rescore_golden` scores the 4 golden rows | GATE_LOSS |
| `train_b::tests::rehearsal_caps_per_class_and_total` | 100 rows / 2 classes, per-class 10 → 40 kept with every class's 10 newest; same seed same pick, other seed other pick; total cap 25 → 25 | — |
| `train_b::tests::local_tier_b_is_off_by_default` | `TierB::local_allowed(DoubleConfig::default()) == false`; `tiers.b.local: true` turns it on | — |
| `candle::lora::tests::gradient_reaches_every_third_of_wqkv_at_both_ends` (`--features train`) | q, k (through rope) and v of `Wqkv.lora_b` receive gradient at layers 0 and 1 | GATE_GRAD |
| `candle::lora::tests::default_build_cuts_the_graph_silently` (default) | the same forward without `train`: `backward()` is `Ok`, no `Var` has a gradient | — |
| `candle::lora::tests::adapter_round_trips_through_safetensors`, `adamw_state_round_trips` | save/load of adapter and optimiser state | — |

## Node run recipe (Linux, CUDA)

1. **Build `t` with the trainer and CUDA.** The decision crate's `cuda` feature forwards to candle (`crates/decision/Cargo.toml:10`), `train` swaps the three fused ops for their differentiable siblings (`:19`); `theuth` has no `[features]` block of its own, so both are named per crate (the 58 convention, `docs/design/cross-platform/step-12-models-gpu-as-built.md:135`):
   ```sh
   cargo build --release -p theuth --features decision/cuda,decision/train
   ```
   (`nvcc` on `PATH`; the resulting `t` also runs the engine, with the slower `rope_slow` forward — a node is a trainer, not the owner's predictor.)
2. **Fetch the checkpoint** the bundle names: `t --data-dir <dir>` with `decision.checkpoints.default` (the models downloader writes `<data_dir>/models/decision/{model.safetensors, encoder/config.json, rl_agent_config.json, tokenizer/tokenizer.json}` and the `.sha256` sidecar that `Base::open` reads through `platform::sha256::cached`). `bundle/manifest.json`'s `base_hash` must equal that sha256, else `train-b` refuses (`bundle base_hash … ≠ checkpoint sha …`).
3. **Ship the bundle** — `manifest.json` `{ "base_hash", "run_id" }` + `rows.jsonl` (one `BundleRow` per line: `question_id, class, card_text, options[], choice, source, weight, asked_at, golden`) — to the node (W2's `ALPN_DOUBLE` stream, or `scp` for a manual run).
4. **Run:**
   ```sh
   t --data-dir <dir> double train-b --bundle <bundle-dir> --out <candidate-dir> --device cuda
   ```
   Progress lines `step i/N epoch e/E loss L elapsed S s`; exit 0 with `<candidate-dir>/adapter.safetensors` + `manifest.json`; exit 3 when the 90-minute cap paused it (re-run the same command to resume from `<candidate-dir>/checkpoint/`). `double.training.tiers.b.{rank, batch, epochs, lr}` and `double.training.run_cap_min` from the node's config.yaml set the knobs (`TrainConfig::from_double`); the venue in the manifest is `node:<hostname>`.
5. **Ship the candidate back** (`adapter.safetensors` + `manifest.json`, ~14.4 MB at rank 8) — W2's `TrainResult`; activation stays on the laptop: W2's gate calls `rescore_golden` and compares `manifest.metrics.holdout_log_loss` / `ece` (§7.3 gate (a)–(c)), then `versions.rs` activates. Delete the bundle, the candidate and the checkpoint on the node afterwards (§8.4 "a node keeps nothing").

In-process on the node (W2's `mesh::train_on`): the same `TierB::train(&bundle, &TrainConfig::from_double(&cfg), &Base::open(&dir, gpu.open()?, gpu.dtype())?, &out, Hooks { pause: &Never, on_progress })`.

## Local path recipe (behind the flag; not for this machine while the owner uses it)

`double.training.tiers.b.local: true` is the switch W2's scheduler reads through `TierB::local_allowed(&cfg)`; it adds nothing to §8.2's preconditions, it only makes them reachable. When W2 runs local Tier B it must: hold a `BUILD_SLOTS=1` build slot (`~/.theuth/bin/build-lock`, label `double-train`); start only inside the window, on AC, idle ≥ 15 min; and pass `Hooks { pause: &OwnerActivity::new(Box::new(<platform::idle>)), .. }` so the first keyboard / mouse event checkpoints and stops within one micro-batch (~1 s at 2 × 256 on the M3 Max). The `t` binary must be built with `decision/train` for this (the default `t` refuses with exit 2 and the reason) — which means the owner's engine would run the slower `rope_slow` forward; the honest local shape is a *second* binary (`cargo build --release -p theuth --features decision/train` into another path) that the scheduler spawns as a job, not the engine's own process. Caveat from W0: even a 1-minute 2 × 256 run made the Mac unusable; the detector bounds the damage to one micro-batch, it does not make training and the owner compatible.

## Deviations from the brief / design

1. `lora.rs` is not feature-gated (above). The `train` feature gates only the differentiable forward; `TRAIN_BUILD` + the trainer's refusal make the default build safe.
2. Merged-at-load inference instead of unmerged (above; `model.rs` is not W3's).
3. `TierB::train` returns `TrainOutcome` (`Finished(CandidateVersion) | Paused { step, checkpoint }`), not `CandidateVersion`: a pause is a normal outcome that must carry the checkpoint path.
4. `TierB.batch` is the *effective* batch (default 16, was 8) and `lr` defaults to 1e-4 (was 2e-4); `TierB.local` added. C's config struct gained one field — a `#[serde(default)]` addition, TS-exported, so W4's hand-written `DoubleConfig.ts` needs `local: boolean` on `TierB` (integrator: `just ts-desktop`).
5. Design §8.1's checkpoint path `~/.theuth/double/runs/<id>/epoch-<n>.safetensors` → `<out>/checkpoint/{adapter.safetensors, optim.safetensors, state.json}` (one checkpoint, overwritten every `checkpoint_every` = 25 steps and at any pause; the run id is the caller's `out` dir). W2's scheduler picks `out`.
6. The hold-out comes out of the training set (brief said "held-out", design §7.3 scored the candidate on rows it had trained on).
7. Pause polled per micro-batch, not per optimiser step (above).
8. The decision crate's `Gpu::open` is a small refactor of `Loaded::open` (not a contract change; the `candle/mod.rs` device pick now has one home).

## Gates

All through `BUILD_LABEL=126-W3 /Users/Sky.Moore/.theuth/bin/build-lock`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, target `/Users/Sky.Moore/Develop/theuth-126-w3-target`, foreground, no `--release`, no example run, `DECISION_EMBED_TEST` unset.

GATE_TABLE

## Open items / hand-offs

- **W2 (`sched.rs` / `venue.rs` / `mesh.rs` / `versions.rs`):** gate local Tier B on `TierB::local_allowed(&cfg)`; build the bundle in `train_b::Bundle` shape (`Bundle::write`), `base_hash` = the checkpoint's sha256 as `platform::sha256::cached` reports it; call `TierB::train` with `Never` on a node and `OwnerActivity` locally; on `Paused` keep `out` and re-call in the next window; consume `CandidateVersion.manifest.metrics` + `rescore_golden` in the gate; `impl train_b::IdleSeconds for platform::idle::<type>` at the `// 126 I` marker.
- **W1 (`predict.rs`):** render the card as `train_b::question` does (class key as instructions, options as candidate ids, `with_none`, card text as state) — or change `question` to match; one of the two must move.
- **Integrator:** `DoubleConfig.ts` gains `local: boolean` under `TierB`; the `double` crate now depends on `decision` (W1 adds the same line — keep one); `cargo test -p double` builds `decision` with `train` (dev-dep) — ~1 extra minute of compile, no effect on `t`.
- **Future `model.rs` change for unmerged inference:** `EncoderAttention { wqkv: LoraLinear, .. }` where `LoraLinear { base: Linear, adapter: Option<(Tensor, Tensor, f64)> }` computes `xWᵀ + (α/r)(xAᵀ)Bᵀ`; `DecisionModel::load` takes an `Option<&Lora>`; then `Loaded` can hold one model and swap adapters per call. Not needed for v1: one double adapter, rebuilt once per activation.
- **Not measured here (by rule):** the real-model step time of the shipped loop (W0's 1,089 ms was with a rebuild per step; this loop rebuilds once per *optimiser* step, so eight micro-batches share one `load_ms`), the CUDA path, `Base::open`'s upload time on a node. The node recipe's first run should record them in `runs`.
