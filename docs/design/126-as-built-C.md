# 126 as-built — wave C (the contract)

Branch `feat/126-double`, worktree `/Users/Sky.Moore/Develop/theuth-126`, base main `e5832a87`. Last code commit before this note: `85f1ebdc 126 C: re-export Explain/Neighbour/Parts for the ts export list`. Design: `docs/design/126-double.md` including Revision 1 (grace 30 s, no pre-0033 bootstrap, L0–L2 in v1).

Scope: types, schema, config, stubs and seams — no behaviour. Every place behaviour belongs to another wave carries a `// 126 W<n>` marker (listed in §Insertion markers).

## What was built, where

### §3 keyer → `crates/double/src/class.rs`
- `CardKind` (approval | ask | attention | packet | draft; `as_str` is the key prefix and `classes.kind`), `DraftKind`, `Requester` (`root | subagent | desk:<agent> | routine:<name>`, `Display` is the suffix), `Card<'a>` (the payload slice the shape rule reads), `class_key(card, requester)` → `kind:shape:requester`.
- Normalisers: `shell_shape` (strips leading `cd … &&` segments, `VAR=value`, `nohup`/`nice [-n N]`/`env`, basenames the first token, keeps two tokens); `path_shape` (first component under `$HOME`, or first two when the first is not dotted — `Develop/theuth`, `.theuth`); MCP = `mcp` flag or a `__` in the name → tool + sorted top-level arg keys; `ask_hash` (sha256 over lower-cased, whitespace-collapsed, sorted labels, first 12 hex); `ASK_FREEFORM = "ask:freeform"`.
- Tests: `ten_example_keys` pins the ten §3 keys; `ask_hash_normalises`, `shell_prefixes_are_stripped`, `other_kinds_and_requesters`.
- `Card::Approval.mcp`: the payload (`ApprovalRequested`) does not say whether a tool is an MCP tool; the registry knows. W1's subscriber passes it; a `__` in the name is the fallback.
- `ponytail:` marker at class.rs:236 — the repo root is not in the payload; the home-relative two-component rule is the heuristic.

### §5 store → `crates/double/src/store.rs`, `crates/double/migrations/0001_double.sql`
- Path: `store::path(data_dir)` = `<data_dir>/double/double.sqlite`; the data dir is `Runtime::data_dir` (crates/runtime/src/lib.rs:30), resolved in crates/theuth/src/main.rs:1243-1246 (`--data-dir` / `~/.theuth`) — the same source `tasks` (crates/tasks/src/lib.rs:1332 `rt.data_dir.join(DB)`) and `knowledge` (crates/knowledge/src/store.rs `Knowledge::open(&rt.data_dir, …)`) use.
- Open pattern copied from crates/tasks/src/store.rs:158-174: `SqliteConnectOptions` WAL + busy_timeout 5 s + `create_if_missing`, pool of 4, `sqlx::migrate!("./migrations")`; plus `foreign_keys(true)` and `create_dir_all` of the parent.
- Tables exactly §5: `classes`, `examples`, `labels`, `predictions`, `versions`, `runs`, `golden`; added `UNIQUE INDEX examples_question (question_id)` (one example per card), `examples_class_asked`, `predictions_version_at`; `ON DELETE CASCADE` from examples to labels/predictions/golden; the `baseline` version row is inserted by the migration (`active = 0`; `active_version()` is `None` until Tier A activates one).
- Rows: `ClassRow`, `ExampleRow`, `LabelRow`, `PredictionRow`, `VersionRow`, `RunRow`; insert/upsert + select per table; `activate_version` is one transaction and refuses an unknown id without clearing the old active.
- Scrubbing at write: `scrub()` = `tools::secrets::redact_value` over `card_text`, option labels, `labels.choice`, `predictions.choice` and `probs` labels (store.rs). `card_text` cut to 4 KB on a char boundary. Embedding f16 LE (`encode_f16` / `decode_f16`).
- Tests on a temp file: `every_table_round_trips`, `card_text_is_capped`, `f16_round_trip`, `scrub_tests::card_strings_are_scrubbed_at_write` (installs a fake `tools::secrets::Secrets`).

### §12 config → `crates/double/src/config.rs`
- `DoubleConfig` with every §12 key: `enabled` (false), `capture`, `ladder { grace_secs: 30, caps { read_only: 2, mutating: 1, destructive: 1 }, mutating_opt_in: false }`, `training { venue: auto, window: 02:00-06:00, idle_min: 15, run_cap_min: 90, triggers { labels: 200, days: 7 }, share_data: true, tiers { a, b { rank 8, batch 8, epochs 3, lr 2e-4 }, c } }`, `retention_days: 365`, `model { checkpoint: default, adapter_rank: 8 }`. All `#[serde(default)]` so a partial YAML patches.
- `Caps::for_risk(Option<Risk>)` clamps destructive to 1 whatever the file says.
- Read once at startup: `DoublePlugin::start` → `rt.section("double")` (lib.rs:62), the knowledge/schedule precedent (crates/knowledge/src/lib.rs:354, crates/schedule/src/lib.rs:2575). Module doc says so. `config.yaml` template section added at crates/theuth/src/main.rs:1107-1130 (after `decision:`), plus the `embed:` line under `decision.families` at main.rs:1093-1095.
- Test: `defaults_and_partial_yaml`.

### §13 types, events, RPC → `crates/double/src/{types,events}.rs`, `crates/rpc/src/{api,bridge,export}.rs`
- `types.rs`: `Tier`, `RunState`, `LabelSource`, `Outcome` (each with `as_str`/`parse`), `RunId`, `Kpis`, `DoubleStatus`, `DoubleRun`, `DoubleClass`, `CurvePoint`, `DoubleExample`, `DoubleClassDetail`, `ForgetTarget`, `Venue` (`as_config`/`parse`: `auto | local | node:<name> | off`), `DoubleExport`. All derive `schemars::JsonSchema` + `ts_rs::TS` under `ts`, like neighbours.
- `events.rs`: `DoublePredicted`, `DoubleAnswered`, `DoubleTrainingState`, `DoubleVersionActivated` + `Explain`, `Neighbour`, `Parts`; `#[serde(tag = "type")]` so the bridge's `type` field is the Rust name; an optional `session: Option<SessionId>` on `DoublePredicted`/`DoubleAnswered` so `subscribe_events`'s per-session filter (crates/rpc/src/api.rs `wanted`) works. `index` is `Option<u32>` (ts-rs has no `usize`).
- Bridge: `forward!` list + `rt.subscribe::<Bridge, T>` for the four events (crates/rpc/src/bridge.rs:16, 134-137, 193-196).
- RPC: `Api` trait methods at crates/rpc/src/api.rs:1419-1468 (`double_status`, `double_classes`, `double_class`, `double_train`, `double_pause`, `double_forget`, `double_rollback`, `double_set_cap`, `double_export`, `double_delete_all`, `double_take_over`, `double_settings_get`, `double_settings_set`); stubs in `impl ApiServer for Server` at api.rs:5054-5124. Read stubs answer typed values (`double_status` and `double_settings_get` read the `double:` section via `Server::double_config`, api.rs:1999; `double_classes` → `[]`; `double_take_over` → `false`); mutating stubs return a JSON-RPC error `NOT_RUNNING` "double: … is not available yet (126 W<n>)" (`double_not_yet`, api.rs:2004). Never a panic.
- Export list: crates/rpc/src/export.rs:171-206 (every `Double*` type, the enums, the config sub-structs, `tools::Answerer`, `mesh::Accelerator`). `rpc/ts` feature fans out to `double/ts` (crates/rpc/Cargo.toml:78).

### §7.2 embedding exposure → `crates/decision`
- `ModelOutput.pooled: Tensor` `(batch, hidden)` f32 (candle/model.rs:516), set at model.rs:632-648: `pooled = xs.i((.., 0)).to_dtype(F32)` — the `[CLS]` slice **without** the four confidence features; the act head's input is now `cat(pooled, features)` as before (no numeric change to logits/action). **W0 note:** my model.rs change is confined to the `ModelOutput` struct (505-517) and the forward's tail (632-648); the rope call at model.rs:75-76 is untouched.
- `RowOut.embedding: Option<Vec<f32>>` (laya.rs:360), `Loaded::run_with(rows, embed)` (candle/mod.rs:266) does one `to_vec2` of `pooled` when asked; `run` = `run_with(_, false)`.
- `Oracle::run_embed(rows, embed)` default method (engine.rs:112) → `run`; `candle::Loaded` overrides it (mod.rs:323). `Scores.embedding: Option<Vec<f32>>` (engine.rs:87).
- `FamilyGate.embed: bool` (gate.rs:103, default false; `FamilyPatch.embed` in lib.rs so YAML can set it); `run_batch` asks the oracle when any family in the batch has `embed` (engine.rs:849) and `decide` copies it onto `Decision.embedding` only for a gate with `embed` (engine.rs:738-742). `Decision.embedding: Option<Vec<f32>>` (question.rs:214, `skip_serializing_if` none).
- `FamilyGate::double()` (gate.rs:189): active, `none: true`, `cut_ok: true`, `embed: true`, `train: false`; the six default families `double.approval/ask/attention/packet/draft/correction` (lib.rs:448-453).
- Tests: `engine::tests::embedding_follows_the_family_gate` (fake oracle, no checkpoint: embed family gets 8 floats, sibling in the same batch gets `None`, wire shape drops the field); `tests/model.rs::pooled_embedding_is_hidden_sized_and_opt_in` loads the real checkpoint and is **off unless `DECISION_EMBED_TEST=1`** (orchestrator rule for this machine — see Gates). Existing decision config tests updated for the family count (8 → 14; patched tables 15/16) and the active/embed invariants.

### §4.3 `answered_by`
- `tools::Answerer { Human, Double }` (crates/tools/src/journal.rs:152-181; `as_str`/`parse`; ts-exported), `QuestionRow.answered_by: Option<Answerer>` (journal.rs:208, `#[serde(default)]`).
- `tools::ResolveBy { id, decision, by }` (approval.rs:81) beside the unchanged `Resolve`; both route through `Approval::resolve(id, decision, by)` (approval.rs:176); `settle` takes `by: Option<Answerer>` (approval.rs:159) and writes `row.answered_by`; `Expire` writes `None`. `answer` JSON unchanged (`HonourAnswer` replay untouched).
- `tools::AnswerAskBy { id, answers: Vec<Answer>, by }` (ask.rs:124 — `answers` required: the double never dismisses) beside `AnswerAsk`; both route through `AskUser::answer` (ask.rs:208); `settle` takes `by` (ask.rs:189).
- `agent::answer_ask_by(id, answers, by)` (crates/agent/src/lib.rs:333) beside `answer_ask`.
- Migration `crates/agent/migrations/0034_double.sql`: `ALTER TABLE questions ADD COLUMN answered_by TEXT NULL`. Store: `QUESTION_COLS`, `question_of`, `question_put` bind it (durability.rs:280-300, 921-938); `question_resolve` leaves it NULL (a path only 109's supervisor uses).
- Every existing caller unchanged (`Resolve`/`AnswerAsk` = `Human`). Other `QuestionRow` literals got `answered_by: None` (roster/handoff.rs:248, secrets/tools.rs:248, agent waits.rs test helper).
- Tests: approval.rs `question_row_pending_then_answered` pins `Human`; new `resolve_by_double_names_the_answerer`; `deadline_elapsed_is_timeout` pins `None`; durability.rs `questions_waits_jobs_screens_round_trip` round-trips `Double` through upsert, mapper and raw column.

### `ALPN_DOUBLE`
- `replica::ALPN_DOUBLE = "theuth/double/1"` (crates/replica/src/lib.rs:57, beside the other ALPNs); registered in `STREAM_ALPNS` (crates/mesh/src/net.rs:344-353, length 7 → 8) so `Transport::accept(ALPN_DOUBLE)` works. **Deviation from the brief:** the brief named `crates/artifact/src/mesh.rs` as the model and `double/mesh.rs` as the home of the constant; the design (§8.4) and the tree put every ALPN in `replica`, so the constant lives there and `double::mesh` re-exports it (crates/double/src/mesh.rs:15) with the documented `// 126 W2` handler registration point (mesh.rs:17-21). No transport logic.

### `EngineStats.accelerator`
- `mesh::Accelerator { Metal, Cuda, Cpu }` (crates/mesh/src/types.rs:149-174; snake_case; `as_str`/`parse`) and `EngineStats.accelerator: Option<Accelerator>` (types.rs:137, `#[serde(default, skip_serializing_if)]` — older engines deserialise rows without it and newer engines accept old rows). **Deviation:** the brief said `Metal | Cuda | None`; `None` as a variant would be `Option<Option<…>>`-shaped on the wire, so the enum is `Metal | Cuda | Cpu` and "unknown / older engine" is the `Option`'s `None`. The design (§8.4) spelled it as `Option<String>` with the `models::accelerator` names; `cpu`/`cuda` match that spelling, `metal` too.
- Probe: `stats::accelerator()` (crates/mesh/src/stats.rs:112-122): Apple silicon → Metal, `/dev/nvidia0` → Cuda, else Cpu; `probe()` sets `Some(…)` (stats.rs:30); the spawn_blocking fallback literal is `None` (lib.rs:725).
- Persistence: `crates/mesh/migrations/0005_accelerator.sql` adds `engine_stats.accelerator TEXT`; `put_stats` / `stats()` bind and read it (store.rs:145-163, 201-224). Replica compat: `replica::apply` skips an unknown column with a warning, so a row from a newer engine lands minus that column on an older one.
- `mesh_peers` line (crates/mesh/src/tools.rs:83-125): `… gpu=<bool>[ accel=metal|cuda|cpu][ hub]` — printed only when advertised, so the old pinned test string is unchanged; test extended for `accel=metal`.
- Other `EngineStats` literals got `accelerator: None` (rpc/src/acp.rs:942, tasks/src/place.rs:432 test helpers; mesh store test uses `Some(Cuda)` and round-trips it).

### `AttentionKind::Double`
- crates/roster/src/attention.rs:66-69, snake_case `double`, same derives as the siblings; `// 126 W1 produces items; W4 renders.` No `match` on `AttentionKind` exists anywhere, so nothing else changed. ts: `AttentionKind.ts` now ends `| "double"`.

### The crate → `crates/double/Cargo.toml`, `src/lib.rs`, `src/mesh.rs`
- Workspace member via the `crates/*` glob; `double = { path = "crates/double" }` in `[workspace.dependencies]` (Cargo.toml:66). Deps: runtime, tools, replica, sqlx, sha2, half, uuid, chrono, serde*, schemars, tracing, async-trait, anyhow; `ts` feature → `runtime/ts`, `tools/ts`. Not depending on `decision` or `agent` yet (nothing in C needs them; W1 adds them with the actor).
- `DoublePlugin` (lib.rs:53-85) implements `runtime::Plugin` exactly like `KnowledgePlugin` (crates/knowledge/src/lib.rs:348-387): reads `double:`, returns early when `enabled: false` (the default), opens the store, then hits the W1/W2/W5 markers. Registered in `engine_plugins()` after `DecisionPlugin` (crates/theuth/src/main.rs:1648-1649); not in the `--acp` list (that list carries no decision plugin either). `theuth` depends on `double` (crates/theuth/Cargo.toml:45).
- `DOUBLE_ACTOR = "double"` (lib.rs:38).

## Insertion markers (file:line)

| marker | where | for |
|---|---|---|
| `// 126 W1: pub mod actor; …` | crates/double/src/lib.rs:40-43 | module declarations |
| `// 126 W2: pub mod train_a; sched; precond; venue; versions;` | crates/double/src/lib.rs:44 | module declarations |
| `// 126 W3: pub mod train_b;` | crates/double/src/lib.rs:46 | |
| `// 126 W5: pub mod tool; …` | crates/double/src/lib.rs:47 | |
| `// 126 W1: spawn actor::Double …` | crates/double/src/lib.rs:78-80 | inside `DoublePlugin::start` after the store opens |
| `// 126 W2: sched::start(…)` | crates/double/src/lib.rs:81 | same |
| `// 126 W5: register tool::* …` | crates/double/src/lib.rs:82 | same |
| `// 126 W2: serve / train_on` | crates/double/src/mesh.rs:17-21 | the ALPN handler registration point |
| `// 126 W1 / W2: every double_* method …` | crates/rpc/src/api.rs:5054-5056 | the stub block to replace with `ask::<Double, _>(DOUBLE_ACTOR, …)` |
| `// 126 W1 produces items; W4 renders.` | crates/roster/src/attention.rs:68 | the new kind |
| `126 W<n>` in `double_not_yet(..)` | crates/rpc/src/api.rs:5071-5120 | each mutating stub names its wave |

## Deviations from the design / brief

1. `ALPN_DOUBLE` lives in `replica`, not `double::mesh` (design §8.4 says replica; brief item 7 pointed at `double/mesh.rs`); `double::mesh` re-exports it.
2. `Accelerator` is `Metal | Cuda | Cpu` inside an `Option` (brief said `Metal | Cuda | None`); see above.
3. Caps default `read_only: 2` (Revision 1 ruling 4), not the v0 body's 3; grace 30 s (ruling 3).
4. `Answerer` lives in `tools::journal` (next to `QuestionRow`), spelled `human | double` as §13.1 says; the design's §4.3 text calls it `Answerer`, the brief `AnsweredBy` — the design's name won. `Resolve.by` / `AnswerAsk.by` are new message types (`ResolveBy`, `AnswerAskBy`) rather than an added field on `Resolve` (which is `Copy` and constructed at 9 call sites across tui/slack/signal/telegram/rpc/agent/tools that I do not own); behaviour of every existing caller is unchanged.
5. §3 key 9's hex: the design's `3f9a1c2b7d0e` was written without a shell; the real sha256-12 of the normalised trio "add to backlog / discard / start now" is `b9474647c6a4` (`printf 'add to backlog\ndiscard\nstart now' | shasum -a 256`), and the test pins that.
6. `DoublePredicted.index` is `u32` not `usize` (ts-rs).
7. `Decision.embedding` was added so the engine can hand the embedding to the double's `Ask` reply (the design names `Scores`; `Scores` is engine-internal and never leaves the actor — the reply type had to carry it too). Not logged.
8. The six `double.*` families are in `default_families()` now (design §12 says "the decision side gains" them); shipping them active with `embed` costs nothing until something asks under those families.
9. The `tests/model.rs` embedding test is gated behind `DECISION_EMBED_TEST=1` (orchestrator steer: no model loads in gates on this machine).

## Gates (all through `BUILD_LABEL=126-C /Users/Sky.Moore/.theuth/bin/build-lock`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, target `/Users/Sky.Moore/Develop/theuth-126-target`)

| # | command | result |
|---|---|---|
| 1 | `cargo test -p double` | 11 passed, 0 failed (lib); 0 doc-tests; 0 warnings after the docs pass |
| 1' | `cargo clippy -p double --all-targets -- -D warnings` | clean |
| 2 | `cargo test -p decision` (full, first run 10:0x) | 86 passed, 3 failed — the three config tests pinning `families.len()`; model-loading tests (`gliner::candle::tests::parity_cpu_f32`, `parity_metal_f32`, `candle::model::tests::sliding_mask_builds_on_metal_and_matches_cpu`, `laya::tests::real_tokenizer_when_present`) all passed in that run |
| 2' | `cargo test -p decision --lib -- --exact tests::config_defaults_and_yaml tests::prefix_entry_sets_every_family_under_it_and_wins tests::tool_sub_family_derives_from_its_parent engine::tests::embedding_follows_the_family_gate` | 4 passed, 0 failed, 85 filtered (after the count fixes). Per the orchestrator's rule the model-loading decision tests were not rerun; `tests/model.rs` integration tests (incl. the new `pooled_embedding_is_hidden_sized_and_opt_in`) were not executed on this machine — the new one is env-gated off by default and skips with a printed reason |
| 3 | `cargo test -p tools` | 171 passed, 0 failed, 1 ignored |
| 3 | `cargo test -p agent` | 428 passed, 0 failed, 1 ignored (no flake; one run) |
| 4 | `cargo test -p mesh -p replica -p roster` | mesh 28, replica 13 + 4 + 5, roster 60; all passed |
| 5 | `cargo test -p rpc --features ts --lib --quiet export_bindings` (= `just ts`) | 8 passed; `bindings/` regenerated with 36 new files (listed below) and 13 `double_*` methods in `methods.json` |
| 5 | `cargo check -p rpc --features ts` | Finished, no warnings |
| 6 | `cargo check -p theuth` | Finished, no warnings |

Warnings: the `mesh` crate briefly emitted 3 `missing_docs` on the new enum variants; fixed before gate 4. No clippy debris touched in crates I do not own.

Not run: `cargo fmt` in crates other than `double`; `cargo test -p decision` as a whole after the fixes (model loads forbidden on this machine); `cargo test -p rpc` beyond the export test.

## Open items / hand-offs

- **W1 (actor):** replace every stub in crates/rpc/src/api.rs:5054-5124 with `ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, …)`; add `decision` and `agent` to `crates/double/Cargo.toml`; pass `Card::Approval { mcp }` from the registry's knowledge of MCP tools (a `__` in the name is the fallback); `double_take_over`; re-publish `DoublePredicted` for pending cards on subscribe (§4.4). Use `tools::ResolveBy { by: Answerer::Double }` and `agent::answer_ask_by(id, answers, Answerer::Double)` at L2/L3.
- **W2:** `double::mesh::serve` / `train_on` on `ALPN_DOUBLE` (accept via `Transport::accept(replica::ALPN_DOUBLE)`); `Venue::parse(cfg.training.venue)`; prefer a reachable `EngineStats { gpu: true, accelerator: Some(Cuda), class ∉ {Laptop, Vm} }`; `double_export` / `double_delete_all` / `double_rollback`; `double_settings_set` (persist via `runtime::write_config_value`).
- **W4:** hand-write the TS below until the integrator runs `just ts-desktop`. Event `type` tags are the Rust names. `AttentionKind` gained `"double"`.
- **W5:** `double_settings_set`'s "live" semantics for `capture`/`ladder`/`training` need the actor; the prompt paragraph + skill.
- **Integrator:** `bindings/` is gitignored — run `just ts-desktop` after merging C. Sky's main checkout stash `stash@{0} "pre-51-merge"` exists in this worktree's shared stash list; I accidentally popped it once and reverted with `git reset --hard` (the entry is still kept; nothing of it was committed).
- Decision §12 also mentions `decision.families.double.*` — present as defaults; if the engine's `set_decision_mode` prefix `double` is wanted in Settings › Labs, that is W4/W5.

## TS contract (verbatim)

Generated by `just ts` at the last commit; `bindings/` is gitignored, so these are copied here for W4.

RPC methods (from `bindings/methods.json`): ``double_class`, `double_classes`, `double_delete_all`, `double_export`, `double_forget`, `double_pause`, `double_rollback`, `double_set_cap`, `double_settings_get`, `double_settings_set`, `double_status`, `double_take_over`, `double_train``.

Method signatures (Rust → JSON-RPC positional params; from `crates/rpc/src/api.rs:1419-1468`):

```ts
double_status(): DoubleStatus
double_classes(): DoubleClass[]
double_class(key: string): DoubleClassDetail
double_train(tier: Tier, venue: Venue): RunId
double_pause(on: boolean): DoubleStatus
double_forget(target: ForgetTarget): number          // count removed (u32)
double_rollback(version: string): DoubleStatus
double_set_cap(class: string, rung: number /* u8 */, mutating_opt_in: boolean): DoubleClass
double_export(): DoubleExport
double_delete_all(): null
double_take_over(question_id: string): boolean
double_settings_get(): DoubleConfig
double_settings_set(settings: DoubleConfig): DoubleConfig
```

Events on `subscribe_events` (the `type` field): `DoublePredicted`, `DoubleAnswered`, `DoubleTrainingState`, `DoubleVersionActivated`.

#### `bindings/DoublePredicted.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { CardKind } from "./CardKind";
import type { Explain } from "./Explain";
import type { SessionId } from "./SessionId";

/**
 * A prediction was written for a card, before the owner saw it (§7.1 step 6).
 * Re-published for every pending card when a client (re)subscribes (§4.4).
 */
export type DoublePredicted = { "type": "DoublePredicted", 
/**
 * `ApprovalId` / `AskId` / packet key / draft id / attention item id, as text.
 */
question_id: string, 
/**
 * The card's session, when it has one (packets and attention items may not);
 * the bridge's per-session filter reads it.
 */
session?: SessionId, 
/**
 * Which card kind.
 */
kind: CardKind, 
/**
 * The §3 key.
 */
class: string, 
/**
 * Option label as rendered on the card (`Allow`, `Start now`, …); `none` = escalate.
 */
choice: string, 
/**
 * Ask: the option index `choice` maps to.
 */
index?: number, 
/**
 * Calibrated top-1.
 */
p: number, 
/**
 * Every option incl. `none`.
 */
probs: Array<[string, number]>, 
/**
 * The class's rung when predicted, 0..=3.
 */
rung: number, 
/**
 * RFC 3339 grace end at L2; `None` at L0/L1; `Some(now)` at L3.
 */
will_act_at?: string, 
/**
 * `versions.id` the prediction names.
 */
version: string, 
/**
 * Why.
 */
explain: Explain, };
```

#### `bindings/DoubleAnswered.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { SessionId } from "./SessionId";

/**
 * The double answered a card at L2 (grace expired) or L3 (§4.2).
 */
export type DoubleAnswered = { "type": "DoubleAnswered", 
/**
 * The card answered.
 */
question_id: string, 
/**
 * The card's session, when it has one.
 */
session?: SessionId, 
/**
 * The §3 key.
 */
class: string, 
/**
 * The option label sent.
 */
choice: string, 
/**
 * The version that predicted it.
 */
version: string, 
/**
 * RFC 3339 end of the undo window, when there is one.
 */
undo_until?: string, };
```

#### `bindings/DoubleTrainingState.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { RunState } from "./RunState";
import type { Tier } from "./Tier";

/**
 * A training run changed state (§8.1); Settings shows it live.
 */
export type DoubleTrainingState = { "type": "DoubleTrainingState", 
/**
 * `runs.id`.
 */
run_id: string, 
/**
 * Which tier runs.
 */
tier: Tier, 
/**
 * `local` | `node:<name>`.
 */
venue: string, 
/**
 * Where in the state machine.
 */
state: RunState, 
/**
 * 0..=1 while running.
 */
progress?: number, 
/**
 * One human-readable line (`epoch 2/3`, `rejected: golden`).
 */
message: string, };
```

#### `bindings/DoubleVersionActivated.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { JsonValue } from "./serde_json/JsonValue";

/**
 * A candidate passed the gate (or `double_rollback` ran) and is now active (§8.5).
 */
export type DoubleVersionActivated = { "type": "DoubleVersionActivated", 
/**
 * The now-active `versions.id`.
 */
version: string, 
/**
 * The version it replaced.
 */
previous?: string, 
/**
 * The version's `metrics` JSON.
 */
metrics: JsonValue, };
```

#### `bindings/Explain.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Neighbour } from "./Neighbour";
import type { Parts } from "./Parts";

/**
 * Why the double predicted what it did (the chip popover, §10).
 */
export type Explain = { 
/**
 * The ≤ 3 nearest past decisions.
 */
neighbours: Array<Neighbour>, 
/**
 * What each part of the head contributed to the top choice.
 */
parts: Parts, 
/**
 * What would promote the class ("12 more correct in a row").
 */
promote_hint: string, };
```

#### `bindings/Neighbour.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * One nearest past decision.
 */
export type Neighbour = { 
/**
 * RFC 3339.
 */
asked_at: string, 
/**
 * ≤ 160 chars of the scrubbed card text.
 */
card_excerpt: string, 
/**
 * What the owner chose.
 */
chose: string, };
```

#### `bindings/Parts.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * The head's contributions to the top choice (§7.1 step 5).
 */
export type Parts = { 
/**
 * Zero-shot oracle.
 */
oracle: number, 
/**
 * k-NN memory.
 */
memory: number, 
/**
 * Class prior.
 */
prior: number, };
```

#### `bindings/CardKind.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * What kind of card a class keys (126 §2 "decision card").
 */
export type CardKind = "approval" | "ask" | "attention" | "packet" | "draft";
```

#### `bindings/Tier.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * A training tier (§0 item 6); also `versions.tier` / `runs.tier`.
 */
export type Tier = "a" | "b" | "c" | "baseline";
```

#### `bindings/RunState.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * A training run's state (§8.1); also `runs.state`.
 */
export type RunState = "idle" | "eligible" | "waiting" | "acquiring" | "running" | "paused" | "evaluating" | "promoted" | "rejected" | "cleaned";
```

#### `bindings/LabelSource.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Where a label came from (§6); also `labels.source`.
 */
export type LabelSource = "direct" | "hindsight_rewind" | "hindsight_correction" | "hindsight_interrupt" | "override" | "timeout" | "excluded";
```

#### `bindings/Outcome.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * How a prediction scored once the label arrived; also `predictions.outcome`.
 */
export type Outcome = "correct" | "wrong" | "unlabelled";
```

#### `bindings/RunId.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * One training run's id (`runs.id`), the reply of `double_train`.
 */
export type RunId = string;
```

#### `bindings/Kpis.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * The §7.4 numbers for one version over one window, overall or per class.
 */
export type Kpis = { 
/**
 * Labelled prediction rows the numbers are over.
 */
n: number, 
/**
 * Prequential accuracy, 0..=1.
 */
accuracy: number, 
/**
 * Mean squared error of `probs` against the one-hot label.
 */
brier: number, 
/**
 * Expected calibration error over 10 bins of `p_top`.
 */
ece: number, 
/**
 * Share of decisions answerable at ≤ 2 % error with the current gate.
 */
coverage_2pct: number, 
/**
 * L2/L3 answers (`acted = 1`).
 */
asks_avoided: number, 
/**
 * Σ median_answer_ms over acted rows + 30 s interruption cost each.
 */
minutes_saved: number, 
/**
 * Overrides per 100 acted rows.
 */
overrides_per_100: number, 
/**
 * Undos during grace per 100 acted rows.
 */
undo_rate_per_100: number, };
```

#### `bindings/DoubleStatus.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { DoubleRun } from "./DoubleRun";
import type { Kpis } from "./Kpis";

/**
 * `double_status` (§13).
 */
export type DoubleStatus = { 
/**
 * `double.enabled`; `false` = the plugin registered nothing.
 */
enabled: boolean, 
/**
 * Learning paused: predictions and rungs continue, labels stop accruing.
 */
paused: boolean, 
/**
 * `versions.id` of the active version; `None` before the first Tier A run (= baseline only).
 */
active_version?: string, 
/**
 * The most recent `runs` row.
 */
last_run?: DoubleRun, 
/**
 * RFC 3339 start of the next Tier B window.
 */
next_window?: string, 
/**
 * The resolved venue: `local` | `node:<name>` | `off`.
 */
venue: string, 
/**
 * Sessions with approvals off (§6.6): double learns only from asks there.
 */
approvals_off_sessions: number, 
/**
 * Overall, active version, last 30 days.
 */
kpis: Kpis, 
/**
 * The same rows scored by the frozen zero-shot baseline.
 */
baseline: Kpis, };
```

#### `bindings/DoubleRun.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { RunId } from "./RunId";
import type { RunState } from "./RunState";
import type { Tier } from "./Tier";

/**
 * One `runs` row (§8.1).
 */
export type DoubleRun = { 
/**
 * `runs.id`.
 */
id: RunId, 
/**
 * Which tier.
 */
tier: Tier, 
/**
 * `local` | `node:<name>`.
 */
venue: string, 
/**
 * Where in the state machine.
 */
state: RunState, 
/**
 * RFC 3339.
 */
started_at: string, 
/**
 * RFC 3339, when finished.
 */
ended_at?: string, 
/**
 * `promoted` / `rejected: <gate>` / an error.
 */
result?: string, };
```

#### `bindings/DoubleClass.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { CardKind } from "./CardKind";
import type { Kpis } from "./Kpis";

/**
 * One row of `double_classes` (§13): the `classes` table plus its last-30-days numbers.
 */
export type DoubleClass = { 
/**
 * The §3 key.
 */
key: string, 
/**
 * The key's kind prefix.
 */
kind: CardKind, 
/**
 * Examples seen.
 */
n: number, 
/**
 * Current rung, 0..=3.
 */
rung: number, 
/**
 * Max rung for this class (§9 caps).
 */
cap: number, 
/**
 * Why the cap holds, when it is below the ladder's next rung (`mutating — opt in to allow L2`).
 */
cap_reason?: string, 
/**
 * Per-class opt-in for L2 on a mutating class.
 */
mutating_opt_in: boolean, 
/**
 * Owner's median time-to-answer for this class.
 */
median_answer_ms: number, 
/**
 * Last 30 days, active version.
 */
kpis: Kpis, 
/**
 * Last 30 days, baseline.
 */
baseline: Kpis, 
/**
 * RFC 3339 of the newest example.
 */
last_decision_at?: string, };
```

#### `bindings/CurvePoint.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Kpis } from "./Kpis";

/**
 * One point of the learning curve (§7.4): a week, current vs baseline.
 */
export type CurvePoint = { 
/**
 * RFC 3339 date of the week's Monday.
 */
week: string, 
/**
 * The active version.
 */
current: Kpis, 
/**
 * The frozen zero-shot oracle on the same rows.
 */
baseline: Kpis, };
```

#### `bindings/DoubleExample.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Outcome } from "./Outcome";

/**
 * One scrubbed example as the UI shows it (`double_class`, the popover).
 */
export type DoubleExample = { 
/**
 * `examples.id`.
 */
id: string, 
/**
 * The card id as text.
 */
question_id: string, 
/**
 * RFC 3339.
 */
asked_at: string, 
/**
 * ≤ 160 chars of the scrubbed card text.
 */
card_excerpt: string, 
/**
 * Option labels in card order.
 */
options: Array<string>, 
/**
 * What the owner chose (the direct / override label), when labelled.
 */
chose?: string, 
/**
 * What the active version predicted.
 */
predicted?: string, 
/**
 * How the active version's prediction scored.
 */
outcome: Outcome, };
```

#### `bindings/DoubleClassDetail.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { CurvePoint } from "./CurvePoint";
import type { DoubleClass } from "./DoubleClass";
import type { DoubleExample } from "./DoubleExample";

/**
 * `double_class(key)` (§13).
 */
export type DoubleClassDetail = { 
/**
 * The class row.
 */
class: DoubleClass, 
/**
 * Weekly points, oldest first.
 */
curve: Array<CurvePoint>, 
/**
 * The last 50, newest first, scrubbed.
 */
examples: Array<DoubleExample>, 
/**
 * What one more correct answer would do ("12 more correct in a row").
 */
promote_hint: string, };
```

#### `bindings/ForgetTarget.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * `double_forget(target)` (§13).
 */
export type ForgetTarget = { "target": "class", 
/**
 * The §3 key.
 */
key: string, } | { "target": "examples", 
/**
 * `examples.id`s.
 */
ids: Array<string>, };
```

#### `bindings/Venue.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * `double_train(tier, venue)`: where to run (§8.4).
 */
export type Venue = { "venue": "auto" } | { "venue": "local" } | { "venue": "node", "name": string } | { "venue": "off" };
```

#### `bindings/DoubleExport.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * `double_export()` (§13): the archive written under `<data_dir>/double/export/`.
 */
export type DoubleExport = { 
/**
 * Absolute path of the scrubbed JSONL + embeddings archive.
 */
path: string, 
/**
 * Rows written.
 */
examples: number, 
/**
 * Archive size.
 */
bytes: bigint, };
```

#### `bindings/DoubleConfig.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Capture } from "./Capture";
import type { Ladder } from "./Ladder";
import type { Model } from "./Model";
import type { Training } from "./Training";

/**
 * `double:`.
 */
export type DoubleConfig = { 
/**
 * `false` (default): no actor, no tools, no prompt paragraph. Restart to change.
 */
enabled: boolean, 
/**
 * Which card kinds are predicted and learned from.
 */
capture: Capture, 
/**
 * Grace, caps, opt-in default.
 */
ladder: Ladder, 
/**
 * Venue, window, triggers, tiers.
 */
training: Training, 
/**
 * `examples` / `labels` / `predictions` older than this are deleted,
 * except golden rows and the last 1,000 per class (§5).
 */
retention_days: number, 
/**
 * Restart to change.
 */
model: Model, };
```

#### `bindings/Capture.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Which card kinds double predicts and learns from (§6.2).
 */
export type Capture = { 
/**
 * Tool-call approvals.
 */
approvals: boolean, 
/**
 * `ask_user` sets.
 */
asks: boolean, 
/**
 * Attention-inbox items.
 */
attention: boolean, 
/**
 * Decision / review packets.
 */
packets: boolean, 
/**
 * Task / agent / routine drafts.
 */
drafts: boolean, };
```

#### `bindings/Ladder.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Caps } from "./Caps";

/**
 * The autonomy ladder's knobs (§9).
 */
export type Ladder = { 
/**
 * L2 countdown; 30 s (Revision 1 ruling 3). Clamped to `deadline − 2 s`.
 */
grace_secs: number, 
/**
 * Max rung by risk.
 */
caps: Caps, 
/**
 * Default `classes.mutating_opt_in` for new classes.
 */
mutating_opt_in: boolean, };
```

#### `bindings/Caps.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Max rung by risk (§9 caps). `mutating` may reach 3 only with the per-class opt-in.
 */
export type Caps = { 
/**
 * 2 in the first release: L3 stays behind this cap (Revision 1 ruling 4).
 */
read_only: number, 
/**
 * Mutating approvals; 3 only with the per-class opt-in.
 */
mutating: number, 
/**
 * Always 1: destructive never climbs past L1 (§0 item 4).
 */
destructive: number, };
```

#### `bindings/Training.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Tiers } from "./Tiers";
import type { Triggers } from "./Triggers";

/**
 * The training cycle (§8).
 */
export type Training = { 
/**
 * `auto` | `local` | `node:<name>` | `off` (§8.4; `crate::types::Venue::parse`).
 */
venue: string, 
/**
 * Local hours Tier B may run, `HH:MM-HH:MM`.
 */
window: string, 
/**
 * User idle required before a run starts or resumes.
 */
idle_min: number, 
/**
 * Cumulative `running` minutes per night per run.
 */
run_cap_min: number, 
/**
 * When Tier B becomes eligible.
 */
triggers: Triggers, 
/**
 * Ship scrubbed bundles to the owner's own GPU nodes (never a `Vm`).
 */
share_data: boolean, 
/**
 * Per-tier switches.
 */
tiers: Tiers, };
```

#### `bindings/Triggers.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * When Tier B becomes eligible (§7.3): whichever comes first. Tier A runs
 * every 25 labels or nightly regardless.
 */
export type Triggers = { 
/**
 * New labels since the active version's `data_range` end.
 */
labels: number, 
/**
 * Days since the active version was trained.
 */
days: number, };
```

#### `bindings/Tiers.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { TierA } from "./TierA";
import type { TierB } from "./TierB";
import type { TierC } from "./TierC";

/**
 * Per-tier switches (§7.3).
 */
export type Tiers = { 
/**
 * Temperatures, k-NN, heads.
 */
a: TierA, 
/**
 * LoRA adapter.
 */
b: TierB, 
/**
 * Full fine-tune.
 */
c: TierC, };
```

#### `bindings/TierA.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Tier A: in-process, deterministic, every 25 labels or nightly.
 */
export type TierA = { 
/**
 * Off: no Tier A runs.
 */
enabled: boolean, };
```

#### `bindings/TierB.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Tier B: the LoRA adapter, data-triggered, gate-promoted.
 */
export type TierB = { 
/**
 * Off: no Tier B runs.
 */
enabled: boolean, 
/**
 * LoRA rank.
 */
rank: number, 
/**
 * Rows per step.
 */
batch: number, 
/**
 * Passes over the corpus per run.
 */
epochs: number, 
/**
 * Learning rate.
 */
lr: number, };
```

#### `bindings/TierC.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Owner action only; never scheduled.
 */
export type TierC = { 
/**
 * Off (default): `double_train(c, …)` is refused.
 */
enabled: boolean, };
```

#### `bindings/Model.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Restart to change (§12).
 */
export type Model = { 
/**
 * A `decision.checkpoints` name.
 */
checkpoint: string, 
/**
 * LoRA rank of Tier B adapters.
 */
adapter_rank: number, };
```

#### `bindings/Answerer.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * Who settled a question (126 §4.3): the owner, or the owner's double. Written to
 * `questions.answered_by` as `human` | `double`; `None` on a row still pending or
 * settled by a timeout. The `answer` JSON keeps its bare shape (109's replay reads it).
 */
export type Answerer = "human" | "double";
```

#### `bindings/Accelerator.ts`

```ts
// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

/**
 * The training-grade accelerator an engine advertises (126 §8.4), detected
 * the cheap way: Apple silicon → `metal`; `/dev/nvidia0` → `cuda`; else
 * `cpu` (a probed machine with no training-grade GPU — an Intel iGPU still
 * says `gpu = true`). Spelled like `models.accelerator`.
 */
export type Accelerator = "metal" | "cuda" | "cpu";
```


Also changed bindings: `AttentionKind.ts` (adds `"double"`), `EngineStats.ts` (adds `accelerator?: Accelerator`), `Answerer.ts`, `Accelerator.ts` (above).
