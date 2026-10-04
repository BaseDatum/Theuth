# 70 as built — DECIDE: `decide` / `decide_many` / `decide_label` as runtime functions (92 steps 1 + 2 + prompt/skill)

Branch `feat/92-decide-tool` off local `main` `61ca4e24`. Brief: `/tmp/69/brief-DECIDE.md`; report:
`/tmp/69/report-DECIDE.md`. Design: `92-decide-tool.md` (§8 rulings: default checkpoint gliner, no taint
logic, caps 2,000 rows · 4 KB state, per-call `capture="tool.<task>"` + `decide_label()`, guidance = a
prompt paragraph + a bundled `decide` skill). Commits: `fc1d925b` (family + config), `d383b7a9` (the tool),
`bee919fb` (bench + batching defaults + model-gated tests), `7b001ac5` (skill + prompt), plus this doc.

## What changed

### Family and config (`fc1d925b`)

- **`crates/decision/src/gate.rs`** — `FamilyGate.temperature: f32` (`:98`, serde default 1.0; part of
  `version()` only when ≠ 1.0, `:227`, so laya rows keep their gate versions);
  `FamilyGate::tool()` (`:154`): `active`, τ 0/0, `none: true`, `cut_ok: true`, budget 2 s, checkpoint
  `gliner`, train false.
- **`crates/decision/src/lib.rs`** — `GLINER_CHECKPOINT` (`:69`; `kind_for` answers `Kind::Gliner` for it
  unless configured, `:366`, so `decision-gliner/` resolves without a `checkpoints:` entry),
  `TOOL_FAMILY` (`:72`), `ToolConfig` = `decision.tool { max_rows 2000, max_state_bytes 4096, batch 8 }`
  (`:302`), `tool` in `default_families` (`:433`), `FamilyPatch.temperature` (`:465`), and in `families()`
  a new rule (`:532`): an unknown `parent.child` key whose parent is a family is a sub-family derived
  from the (already patched) parent with any subset of fields — `tool.mail: { train: true }` works; a
  key like `meeting.topic` still falls through to the prefix path. `DecisionPlugin::start` registers the
  tools (`:692`) when the plugin and the `tool` family are on.
- **`crates/decision/src/engine.rs`** — `decide()` (`:682`) re-softmaxes the logits at
  `model × family` temperature when the family's ≠ 1.0 (no allocation on the 1.0 path), logs the
  effective temperature, sets `calibrated = effective ≠ 1.0` (`:725`) and `truncated = row.truncated()`
  (`:726`); `AskMany.checkpoint: Option<String>` (`:132`, `AskMany::new` keeps the three callers) pins a
  batch to one engine, skipping the "family is on another checkpoint" check (`run_batch(…, pinned)`,
  `:769`); a row the oracle cannot `build` is that row's `HardError`, the rest of the batch runs
  (`Scored`, `:116`); a gliner engine always runs an `AskMany` as one padded forward (`:829`, from the
  bench below).
- **`crates/decision/src/question.rs`** — `Decision.truncated: bool` (`:209`, serde default): the state
  was cut or a marker lost; the ten `Decision { … }` literals gained `truncated: false`
  (engine/escalate/gate/log/subject/training tests, `meeting/src/oracle.rs` test).
- **`crates/decision/src/escalate.rs`** — `EscalateError::Disabled` (`:36`); `escalate()` returns it for
  `tool*` families before any side call (`:198`). `escalate` is the only tier-2 entry (meeting calls it;
  the browser loop has no tier 2), so this is the "escalation call site".
- **`crates/decision/src/training.rs`** — `Amendment::Label { choice }` (`:277`) and `Folded.label`
  (`:462`, folded at `:481`); `label()` (the meeting derivation) is untouched.

### The tool (`d383b7a9`) — `crates/decision/src/tool.rs` (new)

- `is_tool_family` (`:35`). `Q` (`:47`): `{type: choice|bool|score}` dicts, serde-tagged, schema shown to
  the model; `prepare()` renders a `Question` whose candidate ids are positional (`0..n`) — the
  caller's `ids` (or the labels) are only the keys mapped back in `RowOut::from_decision` (`:249`), so
  **no checkpoint ever reads an id**, laya included (laya renders `id: text`). Per-row errors: ids/labels
  length mismatch, reserved `none`, duplicate id, no labels/levels, state > `max_state_bytes`
  (`"state is 5,000 bytes; cap 4,096 — pass a slice (sender, subject, first lines)"`), lost markers
  (`truncated: …`), oracle build failure, budget (`budget: the batch did not finish in N ms — …`),
  model missing, family off. The call fails only for `> max_rows` (`"decide_many: 2,431 rows >
  max_rows 2,000 — loop over slices"`), a non-tool `family`, a checkpoint with no engine, or a bad
  `capture` name.
- `Decide` (`:357`) holds the startup `DecisionConfig`, a `training::Store` over the same
  `<data_dir>/decisions/training` the engine writes, and `captured: Mutex<BTreeMap<LogId, Arc<str>>>`
  (bounded to `CAPTURED_MAX` 50,000, oldest first — `LogId` gained `Ord`). `target()` (`:386`): `family`
  default `tool`, unknown `tool.<x>` → `tool` with a note in the summary line, `checkpoint` default the
  gate's, engine looked up as `actor_name(checkpoint)`; `run()` (`:467`): caps → rows sorted by rendered
  byte length → `AskMany { checkpoint: Some(…) }` per `decision.tool.batch` rows → `yield_now` between
  batches → input order restored → `Release` at the end unless `held_by_others` (`:439`: a live
  non-tool family names the checkpoint). `capture()` (`:577`): one 57 §11 `Example` per answered row,
  `id = ExampleId(log_id)`, `family = tool.<task>`, `text` = state, `questions = [Asked]` with the
  caller's keys and remapped probabilities, `verdict: Model { reason: "decide_many: …" }`; `label()`
  (`:607`): `Amend { Label { choice } }` keyed by the same id, or the exact
  `row was not captured; pass capture="tool.<task>" to decide_many`.
- Tools: `DecideManyTool` (`:653`, `RETURNS list[dict]`), `DecideTool` (`:696`, `dict`),
  `DecideLabelTool` (`:737`, `dict`), all `Risk::ReadOnly`, `CONSUMES_SECRETS false` (arguments are
  data); `register()` (`:766`). The description shares `SHAPES` (`:643`) — the dict grammar, the
  ids-vs-labels rule, the 4 KB / 512-subword cut, the result row, "confidence is a raw margin",
  the honest numbers, "load the `decide` skill".
- Monty helpers `choice`/`bool_q`/`score` were **not** added: `repl.rs::prelude()` (`:593`) only
  replays stored `import` lines, so the shapes are the dict convention above (brief's fallback).

### Batching, memory, bench (`bee919fb`)

- **`crates/decision/examples/decide_bench.rs`** (new): 500 synthetic ~150-token rows × 5 labels sent
  as `AskMany` batches of 8/16/32 through the engine actor exactly as the tool does, both checkpoints,
  padded and per-row forwards.
- **`crates/decision/tests/decide.rs`** (new, model-gated): `forty_emails_over_five_folders`,
  `decide_many_releases_the_checkpoint_it_loaded`.

### Prompt and skill (`7b001ac5`)

- **`crates/decision/decide.md`** (new; landed in `fc1d925b` by an over-broad `git add`, wired here):
  front matter `name: decide` + one-line description; when to reach for it / not; the loop; the three
  shapes with exact fields; rules (labels are read, ids are keys, state after labels and cut at ~512
  subwords, `none`); call arguments and caps; which checkpoint and the numbers; recipes (mail folders,
  files by content with `path + first 1 KB`, Jira components, relevance `bool` pre-filter); capture and
  labels; pitfalls.
- **`tool.rs` `PROMPT`** (`:786`, 7 lines, `## Bulk classification`) registered as
  `RegisterPrompt { plugin: "decision" }` in `register()` — the existing per-plugin mechanism
  (`frame.rs` `plugin_blocks`), so the paragraph is present exactly when `decide_many` is registered.
  `DECIDE_SKILL` (`:797`) = `include_str!("../decide.md")`.
- **`crates/theuth/src/main.rs`** `first_run` (`:1160`): `("decide.md", decision::tool::DECIDE_SKILL)`
  beside `apps.md`/`board.md`, rewritten on every start like the others.

## Measured

### Batch bench (`cargo run --release -p decision --example decide_bench`, M3 Max, **load average 27** from sibling builds, Metal, 500 rows, 5 labels)

| checkpoint | forward | batch | tokens/row | p50 ms/row | p95 ms/row | rows/s | total s | footprint MiB |
|---|---|---|---|---|---|---|---|---|
| gliner | padded | **8** | 165 | **99.0** | 101.2 | 10.1 | 49.5 | 2268 |
| gliner | padded | 16 | 165 | 104.6 | 107.4 | 9.6 | 52.3 | 2208 |
| gliner | padded | 32 | 165 | 112.8 | 121.6 | 8.7 | 57.5 | 2039 |
| gliner | per-row | 8 | 165 | 130.8 | 141.2 | 7.6 | 65.4 | 2083 |
| gliner | per-row | 16 | 165 | 155.6 | 159.9 | 6.5 | 76.8 | 2099 |
| gliner | per-row | 32 | 165 | 156.8 | 162.6 | 6.4 | 78.1 | 2064 |
| laya (`default`) | padded | 8 | 153 | 81.0 | 128.9 | 10.9 | 45.9 | 1240 |
| laya | padded | 16 | 153 | 84.3 | 92.9 | 11.7 | 42.9 | 1241 |
| laya | padded | 32 | 153 | 93.1 | 96.2 | 11.0 | 45.5 | 1447 |
| laya | per-row | 8 | 153 | 68.5 | 73.0 | 14.6 | 34.3 | 1202 |
| laya | per-row | 16 | 153 | 68.5 | 69.6 | 14.6 | 34.3 | 1194 |
| laya | per-row | 32 | 153 | 69.7 | 74.5 | 14.3 | 35.0 | 1206 |

ms/row = batch wall ÷ rows in it, so p50/p95 are over rows. Decisions from the table: **gliner pads its
forwards** (25 % faster at batch 8, the gain shrinks as the padded batch grows), **laya stays per-row**
(`decision.batch_forward` unchanged), **`decision.tool.batch` defaults to 8**, not 16. Absolute numbers
are ~2× GLINER-3's (55–100 ms expected at these lengths): the box was at load 27 the whole run; the
ordering was stable across cells. Re-run on a quiet machine before quoting ms/row to a user.

### Model-gated tests (`cargo test -p decision --release --test decide -- --nocapture`)

- `forty_emails_over_five_folders`: 40 emails in 1.83 s = **46 ms/row**, **40/40 right**, confidence
  min 0.36 · p50 0.57 · max 0.91 (`calibrated: false` everywhere). Smoke, not an eval: the emails
  are obvious by construction.
- `decide_many_releases_the_checkpoint_it_loaded`: cycle 2 footprint before 21 MB → peak 2,023 MB
  during the call → 61 MB after the tool's own `Release` (≤ before + 50 MB), `LoadedNow` empty
  right after the call returns.

## Verified

| check | result |
|---|---|
| `cargo check -p decision --tests` | clean |
| `cargo clippy -p decision --no-deps --all-targets -- -D warnings` | clean (default features; `decision` stays ORT-free) |
| `cargo test -p decision` (debug, both checkpoints present) | lib **88 passed, 0 failed** (81 on `main` → 88: `tool::tests` ×8, `engine::tests::family_temperature_identity_and_scaling`, `escalate::tests::tool_families_have_no_tier_2`, `tests::tool_sub_family_derives_from_its_parent`; 681 s, the gliner CPU parity test dominates); `tests/decide.rs` **2 passed**; `tests/gliner.rs` **3 passed**; `tests/model.rs` **3 passed** |
| `cargo test -p browser` | lib 20 · `tests/live.rs` 1 · `tests/systemone.rs` 2 passed, 0 failed |
| `cargo check --workspace --tests` | clean, after `cargo test -p rpc --features ts --lib export_bindings` regenerated the gitignored `bindings/methods.json` the fresh worktree lacked (`nodes` includes it) |
| `cargo test -p agent` | not run: `crates/agent` is untouched (prompt via `RegisterPrompt`) |
| skill front matter | `tool::tests::skill_front_matter_and_prompt_shape` (YAML `name: decide`, one-line `description`; `skills.rs` parses with gray_matter's YAML engine; no skills.rs test indexes bundled skills) |
| formatting | `rustfmt --edition 2024` on touched files only (rustfmt drift on `main`, GLINER-3) |

## Deviations

- **`calibrated` semantics changed for tier-1 rows**: it was a constant `true`; it is now "a temperature
  other than 1.0 was applied" (model's own × family's). laya families with `rl_agent_config`
  temperatures stay `true`; gliner rows are `false` until a family temperature is fitted (92 §5). The
  gliner integration test asserts the new value. Nothing else read the field.
- **`Decision.truncated`** is new API (serde default), needed because `cut_ok: true` otherwise hides a
  soft cut from the tool; ten literals updated, one in `meeting` (a test).
- **`AskMany.checkpoint`** is new API (the per-call override has to reach the engine); the three
  callers use `AskMany::new`. No `Ask` caller changed.
- **Per-row build failures** no longer fail the whole batch (engine change for every family): a
  rendered-length pathological row used to take its two browser siblings down with it.
- **Release is immediate**, at the end of every `decide_many`, when no other live family names the
  checkpoint — not "when the Monty cell ends" (the decision crate cannot see the agent's turn
  boundary; `browser` uses `agent::AgentEvent`, which `decision` must not depend on). A slice loop
  therefore reloads between calls (~300 ms cold load on Metal, GLINER-3). A short grace timer would
  remove that; not added until it shows up.
- **Capture bypasses `train`**: the per-call `capture` argument is the consent (92 §8 ruling), so the
  tool writes to the `Store` directly instead of through `Capture` (which drops rows unless
  `families.<prefix>.train`). `Example.model_sha` is left empty (the tool does not hold the engine's
  sha); `gate_version` in `Asked` encodes the model.
- **`decision.tool.batch` 8**, not 16 (bench). **laya is slower than advertised here** (68 vs ~22 ms/row):
  load 27 and 5 labels × 150 tokens; the skill quotes GLINER-3's quiet-box numbers and says "≈".
- **No Monty prelude helpers** (`choice`/`bool_q`/`score`): the prelude is imports-only; dict shapes
  documented instead.
- `decide.md` was committed one commit early (in `fc1d925b`) by `git add -A`; its wiring is in
  `7b001ac5`.

## Open

- Calibration fit (92 step 4: `t --decision-calibrate tool.<task>`) and the email lab (step 3).
- `SetMode { prefix: "tool" }` / `check_prefix` match `tool.*` only, not the `tool` family itself
  (`any_under` is `starts_with("tool.")`); the Labs switch is out of scope here (no RPC/UI changes).
- Padded forwards for gliner are a kind rule, not a per-checkpoint knob.
