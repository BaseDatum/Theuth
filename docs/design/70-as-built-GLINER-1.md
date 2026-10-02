# 70 as built — GLINER-1: model-neutral `Oracle` + checkpoint `kind`

Brief: `/tmp/69/brief-GLINER-1.md`. Branch `feat/gliner-oracle` off `origin/main` 95f6496a. Goal: a second classifier family (GLiNER2.5-Decide, GLINER-3) answers the same `Question`s through the same `Engine` without callers changing and without the Engine knowing laya's row format.

## Before

- `engine::Oracle` (engine.rs:47-56) took and returned laya types: `build → laya::Sequence`, `run(&[Sequence]) → Vec<laya::RowOut>`; the Engine read `Sequence.{markers_lost,state_cut,input_ids}` and `RowOut.{probs,logits,temperature,act_p,confidence}` directly.
- `MODEL_FILES` (engine.rs:31) was one fixed four-file list used by `Engine::missing_files`, `mode_for`, and three model-gated tests/examples.
- `Checkpoint { dir, idle_unload }`; `Loader = Fn(&Path, &DecisionConfig)`; one production loader (`open` → `candle::Loaded`).

## What changed

### `crates/decision/src/engine.rs`
- `pub fn model_files(kind: Kind) -> &'static [&'static str]` (engine.rs:31) replaces `MODEL_FILES`: laya keeps its four, gliner = `["model.safetensors", "tokenizer.json", "config.json"]` (provisional, GLINER-3 may amend).
- `pub trait Row: Send` (engine.rs:54): `truncated`, `state_cut`, `markers_lost`, `ids(&self) -> &[i64]`, `as_any`. **Deviation from the brief**: `ids` added — the log's `question_hash` is sha256 of the rendered token ids (log.rs:128), so the Engine needs them through the trait.
- `pub struct Scores { probs, logits, temperature, act_p }` (engine.rs:70) + `Scores::confidence(&self, q)` (engine.rs:83). **Deviation**: brief had `{ probs, logits }`; `temperature` and `act_p` are kept because `AskRow`/`Decision` log them (`1.0` / `0.0` for a model without them). `confidence` is no longer a model output — the Engine derives it from `probs` + the Question (Bool → `max(p, 1−p)`, else `laya::entropy_confidence`), the same rule laya's `postprocess` applied, so laya's numbers are unchanged.
- `pub trait Oracle` (engine.rs:94): `build → Result<Box<dyn Row>, String>`, `run(&mut self, &[Box<dyn Row>]) → anyhow::Result<Vec<Scores>>`, `model_sha`, `load_time`.
- `Loader = Arc<dyn Fn(&Path, Kind, &DecisionConfig) -> …>` (engine.rs:106); `open(dir, kind, cfg)` (engine.rs:622) dispatches `Kind::Laya → candle::Loaded::open`, `Kind::Gliner → gliner::Loaded::open`.
- `Engine.kind: Kind` (engine.rs:283), set from `cfg.kind_for(name)` in `with_loader` / `with_checkpoint`; `missing_files` uses `model_files(self.kind)`.
- `decide(…, row: &dyn Row, out: &Scores, …)` (engine.rs:662) builds the `Decision` from `Scores` + `Question` only; `run_batch` carries `Vec<(Box<dyn Row>, Scores)>` and hashes `row.ids()`.
- `log::Row` is imported as `LogRow` inside engine.rs to make room for the trait name.

### `crates/decision/src/lib.rs`
- `pub enum Kind { Laya (default), Gliner }` (lib.rs:70), serde lowercase; `Checkpoint { dir, idle_unload, kind }` (lib.rs:82); `CheckpointEntry::Full` gains `#[serde(default)] kind` — bare path, `~`, `{dir}`, `{idle_unload}` forms unchanged, all default to laya. An unknown kind is a deserialisation error.
- `DecisionConfig::kind_for(checkpoint)` (lib.rs:328); `mode_for` checks `model_files(cfg.kind_for(checkpoint))` (lib.rs:560).
- `pub mod gliner`; re-exports `Row`, `Scores`.

### `crates/decision/src/laya.rs`
- `impl Row for Sequence` (laya.rs:188); `impl From<RowOut> for Scores` (laya.rs:361); `collate<R: Borrow<Sequence>>(rows: &[R], …)` (laya.rs:398) so the Oracle impl can pass `&[&Sequence]` after the downcast without cloning rows. `RowOut`, `postprocess`, `LayaConfig`, temperatures and `OPTION_CAP` stay in laya.rs — the Engine no longer touches any of them.

### `crates/decision/src/candle/mod.rs`
- `Loaded::run<R: Borrow<Sequence>>` (candle/mod.rs:260) — inherent API unchanged for `&[Sequence]` callers (examples, `tests/model.rs`); `impl Oracle for Loaded` (candle/mod.rs:294) downcasts each `Row` to `Sequence` (a foreign row is an error, not a panic) and maps `RowOut → Scores`.

### `crates/decision/src/gliner/mod.rs` (new)
- Stub for GLINER-3: `Rendered` row (`input_ids`, `state_cut`, `labels_lost`) implementing `Row`; `Loaded::open` → `Err("gliner backend not built yet (GLINER-3): <dir>")`; `impl Oracle for Loaded` with `build`/`run` returning the same error. Any ask on a gliner checkpoint today escalates `HardError("load: gliner backend not built yet (GLINER-3) …")` once its three files are present, `ModelMissing` before.

### Elsewhere
- `MODEL_FILES` → `model_files(Kind::Laya)` in `decision/tests/model.rs`, `decision/examples/phrasing.rs`, `browser/tests/systemone.rs`, `browser/tests/live.rs`, `browser/examples/systemone_suite.rs`, `meeting/tests/oracle.rs`.
- `browser/tests/live.rs` `Scripted` fake: now owns a `Pick` row (no `decision::laya` import left in `browser`). `crates/browser/src/systemone.rs` (production caller) untouched — it only uses `decision::mode_for`.
- `crates/theuth/src/main.rs:1048` generated-config comment documents `kind: laya | gliner`. No RPC shape, UI, or caller code changed.

## Tests added
- `engine::tests::non_laya_rows_answer_choice_bool_and_score_through_ask` (engine.rs:1605): a `Flat` row with none of laya's fields and a `PerLabel` oracle (first label 0.7) through `Engine::ask` for Choice (labels `[a, b, none]`, choice `a`, Accept, entropy confidence, `act_probability 0`), Bool (`false` 0.7, confidence 0.7), Score (`["0","1","2"]`, choice `0`), and a lost-label row → `Escalate(Truncated)` read through the trait.
- `tests::gliner_kind_parses_and_mode_for_checks_its_files` (lib.rs:773): `checkpoints.guard: { kind: gliner }` parses; `mode_for` names the gliner three-file list as missing under `decision-guard/`, reports installed once exactly those three exist; `kind: bert` is an error.
- `tests::checkpoint_kind_defaults_to_laya_in_every_entry_form` (lib.rs:759): bare path, `{dir}`, `{idle_unload}`, `~`, and unlisted names all resolve to `Kind::Laya`.

## Verified

Toolchain `~/.rustup/toolchains/stable-aarch64-apple-darwin`; machine under load from sibling builds (full `cargo check --workspace --tests` 2 m 56 s compile after a 20-min lock wait).

- `cargo check -p decision --tests`: green (`Finished dev`, 11.6 s incremental).
- `cargo test -p decision`: **3 suites — unit 67 passed / 0 failed** (64 on main + 3 new), **`tests/model.rs` 3 passed / 0 failed** (model-gated, ran against `~/.theuth/models/decision`: `readme_questions_match_the_reference`, `actor_end_to_end_with_report`, `release_reclaims_the_checkpoint` — the real laya checkpoint through the new `Oracle` impl's downcast path), doc-tests 0.
- `cargo check --workspace --tests`: green (`Finished dev`). Needed `bindings/methods.json` first — `cargo test -p rpc --features ts export_bindings` (8 passed) writes it; it is gitignored and absent in a fresh worktree, so `nodes`' lib test cannot compile without that step.
- `cargo test -p browser`: **3 suites — unit 20 passed, `tests/live.rs` 1 passed, `tests/systemone.rs` 2 passed, 0 failed** (the loop drives the fixture page through `AskMany` on the adapted `Scripted` oracle).
- `cargo clippy -p decision --no-deps --all-targets`: clean (one `type_complexity` on the batch closure fixed with the `Scored` alias). `cargo fmt -p decision -p browser -p meeting`: applied.

## Not done / open
- `laya::labels(q)` (the label order contract) and `laya::entropy_confidence` are still in `laya.rs` though they are model-neutral; the Engine and `escalate.rs` call them. Moving them to `question.rs` is a rename, not behaviour — left for when GLINER-3 touches the same lines.
- `scripts/fetch-models.sh` / `release-models.sh` not edited (brief item 5): see the report for what GLINER-3 must add under `decision-gliner/`.
