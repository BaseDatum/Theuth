# 136 — As-built C (contract worker)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136`, branch `feat/136-dreaming` · binds W1–W4 and Z (contract §0: read this before touching code). Everything here compiles and is unit-tested at the seams; behaviour is where the `// 136 W<n>` markers say.

## 0. One screen

- **knowledge** (`crates/knowledge`): `Status` (+ `is_live`), `VerifyOutcome`, `Uses`, `Receipt`; `Source::Dreaming`; `KnowledgeOp` gains `merge | supersede | archive | verify | retitle`; the seven keys on `Meta` (serde defaults: `status: tidy`, `hint: true`, rest empty) and on `Put` (`Option`/`Vec`, `None`/empty = keep the file's value); `AgentScope` is serde; `Default for Meta`. Store: `uses` table (durable, never touched by `sync`), seven UNINDEXED FTS columns (schema mismatch → `concepts` dropped and rebuilt, `uses` kept), `record_shown` / `record_opened` / `stats` **real**, `merge` / `supersede` / `archive` / `verify` / `retitle` **stubs** (`bail!("not built: W1 (…)")`), `put_for` round-trips every existing frontmatter key, `hint_for` filters `hint: false` and non-live statuses (rank is W1's), `knowledge_get` records an open, `set_consolidator` / `may_consolidate` gate. Actor: `Merge` / `Supersede` / `Archive` / `Verify` / `Retitle` / `Stats` / `RegisterConsolidator` + the `Consolidator` trait. Tools: `knowledge_stats` real; `knowledge_merge` / `_supersede` / `_archive` / `_verify` / `_retitle` registered (Mutating) with `writable()` + the gate, bodies reach the W1 stubs.
- **agent** (`crates/agent`): `CallKind::Dream` (runtime); `Budget` (ts `SessionBudget`) + `SetBudget` → `SessionState.budget` (via `SetBudgetNow` on the session actor; a budget set mid-turn is applied at `TurnDone`); `// 136 W2: guard` marker at `Reserved::open`; `dreaming_ledger.rs` with `LedgerSince` + every `Ledger*` struct and a handler answering `Ledger::empty(since, until)` (`truncated: false`); `knowledge.record_shown(&ids, now)` right after the `<knowledge>` note in `Turn::round`; the harvest `Put` takes `..Put::default()` (W2 sets `evidence`/`status`); `Store::may_consolidate` (`unattended OR (parent IS NULL AND desk IS NULL)`) installed into knowledge as the `Consolidator` from `supervisor::start`.
- **schedule** (`crates/schedule`): `budget: <number> <CUR>` (`file::Budget::parse`, `Spec.budget`, `ScheduleFile.budget`, `FileArgs.budget`, rendered); every fire sends `agent::SetBudget{session, nanos: override ?? file ?? None, currency}`; `FireWith{name, overrides: FireOverrides{budget_nanos, run_timeout_secs, payload}}` → `fire_with`, payload inside the envelope as `<request>…</request>` after the prompt; `Fire` and `FireWith` share `fire_now`.
- **dreaming** (NEW `crates/dreaming`): `DreamConfig` (§9.2 defaults, `budget_usd: 1.0`), every type of §2.5–2.7/§6/§7.2, `DreamingPlugin` (installs bundled files when enabled, spawns `Night` + `Signals`, registers nine tools + the `## Standing preferences (learned)` block), `sensor.py` + `sensor_rules::gate` (same table), `routine.md` template, `skill.md` headings + tool contract, `rpc.rs` fns, `dry_run(data_dir) -> Report` (ts `DryRunReport`), W3 stub modules.
- **main.rs**: `Box::new(dreaming::DreamingPlugin)` after `agent::AgentPlugin` in both lists; `dreaming:` block in `DEFAULT_CONFIG`.
- **rpc**: the nine `dreaming.*` methods (dotted names, a first in this API); ts exports for every 136 type; `dreaming.status` / `dreaming.settings` answer from `state.json` + the section when the plugin is disabled.

## 1. File by file

| File | What |
|---|---|
| `Cargo.toml` | workspace dep `dreaming = { path = "crates/dreaming" }` |
| `crates/runtime/src/side_call.rs` | `CallKind::Dream` (`"dream"`), round-trip test row |
| `crates/knowledge/src/types.rs` | `Source::Dreaming`; `KnowledgeOp::{Merge, Supersede, Archive, Verify, Retitle}`; `Status` (lowercase serde, default `Tidy`, `as_str`/`parse`/`is_live`; ts `KnowledgeStatus`); `VerifyOutcome`; `Uses`; `Receipt{op, ids, commit}`; `Meta` + 7 fields; `Put` + 7 fields; `AgentScope: Serialize, Deserialize`; `impl Default for Meta` |
| `crates/knowledge/src/store.rs` | `CREATE_CONCEPTS` + 7 UNINDEXED columns, `META_COLS`, `CREATE_USES`; `MetaRow` + columns; `Indexed::new` reads the 7 keys (`fm_string`/`fm_bool`/`fm_strings`); `open_db` probes all 9 new columns, drops `concepts` on mismatch, creates `uses`; `consolidator` field, `set_consolidator`, `may_consolidate`; `put_for` starts from the existing file's frontmatter (unknown keys verbatim; `tags`/`scope` removed when the put has none; 7 keys set only when the put sets them); `upsert` binds the 7 columns; `hint_for` filter; `stub()` clears `evidence`; `record_shown`/`record_opened`/`stats` (upsert SQL); five W1 stubs; tests `put_for_round_trips_the_136_keys_and_unknown_keys`, `uses_table_records_shown_and_opened_and_survives_sync`, `hint_for_skips_unhinted_and_non_live_but_search_keeps_them` |
| `crates/knowledge/src/lib.rs` | `Merge`/`Supersede`/`Archive`/`Verify`/`Retitle`/`Stats` asks + handlers; `Consolidator` trait; `RegisterConsolidator` |
| `crates/knowledge/src/tools.rs` | `consolidating(k, name, cx)` gate; `knowledge_get` → `record_opened`; `StatsTool` (real), `MergeTool`, `SupersedeTool`, `ArchiveTool`, `VerifyTool`, `RetitleTool`; tests `tool_names_match_doc` (+ risks), `consolidation_tools_need_owner_root_or_unattended` |
| `crates/knowledge/src/{harvest,search,sync}.rs` | test fixtures take the new fields |
| `crates/agent/src/dreaming_ledger.rs` | NEW: caps consts, `LedgerSince`, `Ledger`, `LedgerSession`, `LedgerUserMessage`, `LedgerCount`, `LedgerToolError`, `LedgerQuestion`, `LedgerStop`, `LedgerHintUse`, `LedgerRewind`, `Ledger::empty`; tests |
| `crates/agent/src/lib.rs` | `pub mod dreaming_ledger` + re-exports; `Budget` (ts `SessionBudget`), `SetBudget` |
| `crates/agent/src/loop_.rs` | `SetBudgetNow`; `SessionState.budget`; `Session.pending_budget` (applied at `TurnDone`); W2 guard marker in `Reserved::open`; `record_shown` after the note; harvest `Put` `..Put::default()` |
| `crates/agent/src/store.rs` | `Store::may_consolidate` |
| `crates/agent/src/supervisor.rs` | `impl Message<SetBudget>` / `impl Message<LedgerSince>` (empty ledger) on `Agents`; `StoreConsolidator` registered into knowledge from `start` (waits on the knowledge actor) |
| `crates/agent/src/store/durability.rs` | rustfmt only |
| `crates/schedule/src/file.rs` | `NANOS_PER_UNIT`, `Budget::parse`; `Spec.budget`; `FrontMatter.budget`; `FileArgs.budget`; `ScheduleFile.budget` (`#[serde(default)]`); render; test `budget_key` |
| `crates/schedule/src/lib.rs` | `FireOverrides`, `FireWith`; `fire` → `fire_with(.., overrides)`; `SetBudget` on every session fire; `<request>` block; `fire_now` shared by `Fire`/`FireWith`; test `fire_with_payload_is_a_request_block_inside_the_envelope` |
| `crates/schedule/src/tools.rs` | `render_args` passes `budget: None` |
| `crates/schedule/src/validate.rs` | rustfmt only (was unformatted on main) |
| `crates/dreaming/Cargo.toml` | deps: runtime, tools, knowledge, agent, schedule, document, mesh, kameo, tokio, parking_lot, async-trait, anyhow, serde, serde_json, serde_yaml, schemars, chrono, tracing, ts-rs (feature `ts`) |
| `crates/dreaming/src/lib.rs` | `DreamingPlugin`; `SKILL`/`SENSOR` consts; `routine_text(cfg, data_dir, engine)`; `install_files(data_dir, cfg, engine) -> Vec<String>`; `dry_run(data_dir) -> Report` (empty); events `DreamNightStarted{night}`, `DreamNightFinished{night, counts, spend_nanos, proposals}`, `DreamUndone{night, group}`; tests (routine parses with `schedule::file::parse`, install semantics, dry_run writes nothing) |
| `crates/dreaming/src/config.rs` | `DreamConfig` + `HeavyDay`, `Models`, `Caps`, `Lifetimes`, `Gate`, `Preferences` (ts `Dream*`); defaults test |
| `crates/dreaming/src/types.rs` | `NightId`, `CaseKind`, `Split`, `CaseOrigin`, `CaseState`, `Case`, `EditOp` (ts `LearnedEditOp`), `Verdict` (ts `GateVerdict`), `GateResult`, `EditRecord`, `ApplyStatus`, `ApplyReport`, `NightCounts`, `NightSummary`, `NightReceipt`, `ProposalKind`, `Proposal`, `SignalsSnapshot`, `RunMode`, `RunRequest`, `RejectedEdit`, `DreamState`, `Backlog`, `DreamStatus`, `UndoReport`, `DreamSettings`, `DreamModels`, `Metrics`, `Report` (ts `DryRunReport`); binding-JSON test for `Case`/`EditRecord` |
| `crates/dreaming/src/signals.rs` | `Signals` actor (REAL): `signals.json` atomic write with `engine`, `written_at`, `tz_offset_minutes`, `last_root_turn_at` (attended roots via the agent store; every session when the store is unavailable), `sessions_since`, `new_concepts_since` (harvest/model puts), `last_dream_at` from state; asks `Snapshot`; tell `Dreamt(at)` resets; subscribes `AgentEvent` + `KnowledgeChanged`; test |
| `crates/dreaming/src/night.rs` | `Night` actor: `DREAMING_ACTOR`, `dir`/`state_path`/`read_state`/`write_state`/`write_atomic`, `next_window`, `status()` (REAL except `backlog`), `settings()` (REAL); asks `BeginNight`, `Commit`, `Undo`, `Export`, `RunNow`, `Status`, `Nights`, `Settings`, `SetSettings`, `Proposals`, `ProposalAct`, `MetricsOver` (W3 stubs answer `Err("not built: W3 (…)")`; `Status`/`Settings`/`Nights`/`Proposals` real from files); `start()` stamps `engine` into state.json; tests |
| `crates/dreaming/src/sensor_rules.rs` | `gate(cfg, snapshot, now, tz_offset_minutes, self_engine) -> Result<(), &'static str>`, `parse_hhmm`, `local_minutes`, `minutes_into_window`, `is_heavy`, `BATTERY_MIN_PCT`; the rule table test (17 rows) + midnight-crossing test |
| `crates/dreaming/src/sensor.py` | `# theuth-sensor: dreaming 1`; `gate(args, signals, now)` mirrors the Rust table (same reason strings); `check` reads `dreaming/signals.json` via `read_file`, payload `subject=dreaming events=go|hold` |
| `crates/dreaming/src/routine.md` | template: `every: 30m`, `when: dreaming.py window=… idle=… heavy_concepts=… heavy_sessions=… [engine=…]`, `fire: while_true`, `cooldown: 3h`, `mode: skip`, `memory: clear`, `runtime: clear`, `quiet: true`, `approvals: false`, `run_timeout: {{run_timeout}}`, `budget: {{budget}}`, `max_runs: 100000`, `cwd: <data_dir>`, optional `model:` |
| `crates/dreaming/src/skill.md` | front matter + phase headings (0–7), budget order, proposer/judge prompt placeholders (W3), the tool contract table |
| `crates/dreaming/src/tools.rs` | `Seams{data_dir, cfg, knowledge}`; `register` (prompt block when `preferences.md` non-empty + nine tools); `dreaming_ledger`, `dreaming_state` (REAL), `case_put`, `case_list`, `skill_learned_edit` (cap check real), `replay_case`, `preferences_set`, `dreaming_journal`, `dreaming_undo`; tests |
| `crates/dreaming/src/rpc.rs` | `METHODS` (nine names); `status`, `nights`, `run_now`, `undo`, `settings`, `set_settings`, `proposals`, `proposal_act`, `metrics` → `Night` asks |
| `crates/dreaming/src/preferences.rs` | `HEADING`, `path`, `block(data_dir) -> Option<String>` (REAL), `set` (W3); test |
| `crates/dreaming/src/{ledger,cases,edits,replay,journal,undo}.rs` | W3 signatures (`ledger::build`, `DayLedger`; `cases::append/list`; `edits::apply`; `replay::replay`, `Replay{answer, score, reason}`; `journal::append`; `undo::undo`), each `bail!("not built: W3 (…)")` |
| `crates/theuth/Cargo.toml` / `src/main.rs` | dep; plugin in both lists after agent; `dreaming:` in `DEFAULT_CONFIG`; `first_run` doc note |
| `crates/rpc/Cargo.toml` | dep `dreaming`, feature `ts` → `dreaming/ts` |
| `crates/rpc/src/api.rs` | trait + impls for the nine `dreaming.*` methods, `dreaming_config()`, `dreaming_err`; test `dreaming_methods_are_registered` |
| `crates/rpc/src/export.rs` | exports: every type in §3 below |
| `crates/rpc/bindings/*` | legacy tracked dir regenerated by the export (`Meta.ts`, `Source.ts`, `KnowledgeStatus.ts`) |

## 2. Deviations from the contract (and why)

1. **`Status` ts name** — Rust `knowledge::Status` as contracted; exported as **`KnowledgeStatus`** (`#[ts(rename)]`) because `bindings/Status.ts` is the todo `Status`. Same for `agent::Budget` → ts **`SessionBudget`** (roster `Budget`), dreaming `config::{Caps, Gate, Models, Lifetimes, Preferences, HeavyDay}` → **`DreamCaps`, `DreamGate`, `DreamConfigModels`, `DreamLifetimes`, `DreamPreferences`, `DreamHeavyDay`** (double `Caps`, mcp `Gate`), `Verdict` → **`GateVerdict`** (schedule `Verdict`), `EditOp` → **`LearnedEditOp`** (meeting `EditOp`), `Report` → **`DryRunReport`**. Rust names unchanged.
2. **`Receipt`** (not in the contract): the reply of the five knowledge ops — `{op: KnowledgeOp, ids, commit}`. W1 fills it; the tools return it as `structured`.
3. **The owner-root-or-unattended predicate** lives in knowledge as `Consolidator` (trait) + `Knowledge::may_consolidate`, installed by the agent plugin (`StoreConsolidator` over `sessions`), not as fields on `ToolCx` — `ToolCx` is in `crates/tools`, which C must not touch. Until the agent installs it, nobody may consolidate (fail closed). Dreaming's mutating tools use the same gate through `Seams.knowledge`.
4. **`merge`'s `scope`**: the store signature is the contract's (`&AgentScope`); the `knowledge_merge` tool passes the user's scope (`owner: user`, empty line) because the tool has no scoper access — W1 may resolve the session's scope inside `Knowledge::merge` instead (the store has `scope_for(session)`; consider changing the tool to pass `cx.session` — a signature change W1 owns and must record).
5. **`SetBudget` is a `tell` to the session actor** (`SetBudgetNow`), stored on `SessionState.budget`; while a turn is running the value waits in `Session.pending_budget` and is applied at `TurnDone`. W2's guard reads `SessionState.budget` from inside the turn — the running `Turn` owns the state, so a mid-turn `SetBudget` takes effect next turn (the schedule sends it before `SendInput`, so a fire is always covered).
6. **Routine file**: `fire: while_true` + `cooldown: 3h` instead of the design's `fire: once` (40 §1.6 `once` disables the watcher after its first fire — a nightly routine must keep firing; the heavy-day second window is ≥ 3 h later); `when: dreaming.py …` (the sensor key takes a file name in `<data_dir>/sensors/`, no `sensors/` prefix — `Sensor::parse` refuses path separators); `max_runs: 100000` because a `when:` file needs a stop (`until` / `max_runs` / `expires`) and `expires` deletes the file; `cwd: <data_dir>` so the sensor's `read_file("dreaming/signals.json")` resolves; the sensor args carry the window / idle / heavy-day numbers because the Monty sandbox has no config access.
7. **Where the bundled files are written**: `dreaming::install_files` from `DreamingPlugin::start` (needs `dreaming.enabled`), not `main.rs::first_run` (runs before the config is read). `main.rs` registers the plugin only.
8. **`SignalsSnapshot`** gains `written_at` and `tz_offset_minutes` (the sensor has a UTC clock only; the plugin writes its local offset). `last_dream_window` of design §4 phase 0 is derived (`last_dream_at` vs the window start), not stored.
9. **`DreamState`** gains `journal` (document id) and `paused_until` (the contract puts `paused_until` on `DreamStatus`/`DreamSettings`; it has to live somewhere).
10. **`RunRequest.mode`** is `#[serde(default)]` (`pass`) and `run_timeout_secs` `Option<u64>` default `None` (4 h in backlog mode is W3's choice at fire time).
11. **Dotted RPC names** (`dreaming.status` …) are registered as contracted; every other method in `api.rs` is snake_case. `rpc/src/api.ts` on the UI side is W4's.
12. **`rustfmt` on files I did not otherwise edit**: `crates/schedule/src/validate.rs`, `crates/agent/src/store/durability.rs` (both were unformatted on main; `cargo fmt -p <crate>` on owned crates is allowed).


## 3. TS contract (verbatim)

Generated by `cargo test -p rpc --features ts --lib export_bindings` into `bindings/` (gitignored); doc comments stripped here, shapes byte-identical otherwise. `bigint` = Rust `u64`/`i64`; `number` = `u32`/`u8`/`f64`. W4 hand-writes these under the gitignored bindings dir.

### RPC methods (`bindings/methods.json` carries all nine)

| Method | Params (positional) | Result |
|---|---|---|
| `dreaming.status` | `[]` | `DreamStatus` |
| `dreaming.nights` | `[limit: number]` | `NightSummary[]` |
| `dreaming.run_now` | `[request: RunRequest]` | `SessionId` |
| `dreaming.undo` | `[night: string, group: string \| null]` | `UndoReport` |
| `dreaming.settings` | `[]` | `DreamSettings` |
| `dreaming.set_settings` | `[settings: DreamSettings]` | `DreamSettings` |
| `dreaming.proposals` | `[]` | `Proposal[]` |
| `dreaming.proposal_act` | `[id: string, choice: string]` | `null` |
| `dreaming.metrics` | `[days: number]` | `Metrics` |

Errors: the actor absent (`dreaming.enabled: false`) → `NOT_RUNNING` for every method except `status`/`settings`, which answer from `state.json` + the config section; a W3 stub → `ASK_FAILED` with `"not built: W3 (…)"`. `enable` is the existing `enable_schedule("dreaming", bool)`; `fire_schedule("dreaming")` still works (no overrides).

### Types

```ts
// DreamStatus.ts
import type { Backlog } from "./Backlog";
import type { NightSummary } from "./NightSummary";
export type DreamStatus = { 
enabled: boolean, 
last_night: NightSummary | null, 
next_window: string | null, 
budget_usd: number, 
budget_left_tonight_usd: number, 
backlog: Backlog, 
proposals_pending: bigint, 
paused_until: string | null, };
```

```ts
// NightSummary.ts
import type { NightCounts } from "./NightCounts";
export type NightSummary = { 
night: string, 
started_at: string, 
finished_at: string | null, 
counts: NightCounts, 
spend_usd: number, 
mode: string, 
undone: boolean, };
```

```ts
// NightCounts.ts
export type NightCounts = { 
triaged: bigint, 
retitled: bigint, 
merged: bigint, 
superseded: bigint, 
archived: bigint, 
verified: bigint, 
cases: bigint, 
edits_accepted: bigint, 
edits_rejected: bigint, 
proposals: bigint, };
```

```ts
// Backlog.ts
export type Backlog = { 
fresh: bigint, 
untidied: bigint, 
open_cases: bigint, };
```

```ts
// RunRequest.ts
import type { RunMode } from "./RunMode";
export type RunRequest = { 
budget_usd: number, 
mode: RunMode, 
run_timeout_secs: bigint | null, };
```

```ts
// RunMode.ts
export type RunMode = "pass" | "backlog";
```

```ts
// UndoReport.ts
export type UndoReport = { 
night: string, 
group: string | null, 
knowledge_restored: Array<string>, 
skills_restored: Array<string>, 
edits_undone: Array<string>, 
errors: Array<string>, };
```

```ts
// DreamSettings.ts
import type { DreamModels } from "./DreamModels";
export type DreamSettings = { 
enabled: boolean, 
window: [string, string], 
budget_usd: number, 
models: DreamModels, 
preferences_auto: boolean, 
digest: string, 
paused_until: string | null, };
```

```ts
// DreamModels.ts
export type DreamModels = { 
triage: string | null, 
tidy: string | null, 
propose: string | null, 
judge: string | null, };
```

```ts
// Proposal.ts
import type { ProposalKind } from "./ProposalKind";
export type Proposal = { 
id: string, 
kind: ProposalKind, 
title: string, 
body: string, 
options: Array<string>, 
created_at: string, };
```

```ts
// ProposalKind.ts
export type ProposalKind = "routine" | "double_class" | "remember";
```

```ts
// Metrics.ts
export type Metrics = { 
days: number, 
hint_open_rate: number, 
impressions_never_opened_share: number, 
repeat_correction_rate: number, 
active_concepts: bigint, 
median_title_chars: bigint, 
skill_loads_per_100_sessions: number, 
preference_lines: bigint, 
spend_usd: number, 
undos: bigint, };
```

```ts
// Case.ts
import type { CaseKind } from "./CaseKind";
import type { CaseOrigin } from "./CaseOrigin";
import type { CaseState } from "./CaseState";
import type { SessionId } from "./SessionId";
import type { Split } from "./Split";
export type Case = { 
id: string, 
night: string, 
kind: CaseKind, 
intent: string, 
context: string, 
happened: string, 
user_words: string, 
ideal: string, 
tags: Array<string>, 
skill_hint: string, 
source_session: SessionId, 
at: string, 
split: Split, 
origin: CaseOrigin, 
state: CaseState, 
closed_by: string | null, 
reopened_at: string | null, };
```

```ts
// CaseKind.ts
export type CaseKind = "correction" | "re-ask" | "disapproval" | "rewind" | "stop" | "denial" | "tool-chain" | "hint-miss" | "task-returned";
```

```ts
// CaseState.ts
export type CaseState = "open" | "closed" | "reopened" | "settled";
```

```ts
// CaseOrigin.ts
export type CaseOrigin = "real" | "synthetic";
```

```ts
// Split.ts
export type Split = "train" | "val";
```

```ts
// EditRecord.ts
import type { GateResult } from "./GateResult";
import type { LearnedEditOp } from "./LearnedEditOp";
export type EditRecord = { 
id: string, 
night: string, 
target: string, 
op: LearnedEditOp, 
content: string, 
anchor: string | null, 
rationale: string, 
cases: Array<string>, 
gate: GateResult | null, 
applied_at: string | null, 
undone_at: string | null, };
```

```ts
// LearnedEditOp.ts
export type LearnedEditOp = "add" | "replace" | "delete";
```

```ts
// GateResult.ts
import type { GateVerdict } from "./GateVerdict";
export type GateResult = { 
baseline: number, 
candidate: number, 
min_delta: number, 
val_cases: number, 
verdict: GateVerdict, };
```

```ts
// GateVerdict.ts
export type GateVerdict = "accepted" | "rejected" | "unverified";
```

```ts
// ApplyReport.ts
import type { ApplyStatus } from "./ApplyStatus";
export type ApplyReport = { 
target: string, 
edits: Array<[string, ApplyStatus, string]>, 
text: string, 
backup: string | null, };
```

```ts
// ApplyStatus.ts
export type ApplyStatus = "applied" | "skipped_unmatched" | "error";
```

```ts
// Uses.ts
export type Uses = { 
id: string, 
shown: bigint, 
opened: bigint, 
last_shown: string | null, 
last_opened: string | null, };
```

```ts
// KnowledgeStatus.ts
export type KnowledgeStatus = "fresh" | "tidy" | "merged" | "archived" | "stale" | "retired";
```

```ts
// VerifyOutcome.ts
export type VerifyOutcome = "ok" | "stale";
```

```ts
// Receipt.ts
import type { KnowledgeOp } from "./KnowledgeOp";
export type Receipt = { 
op: KnowledgeOp, 
ids: Array<string>, 
commit: string | null, };
```

```ts
// KnowledgeOp.ts
export type KnowledgeOp = "put" | "delete" | "merge" | "supersede" | "archive" | "verify" | "retitle";
```

```ts
// Source.ts
export type Source = "harvest" | "model" | "user" | "sync" | "dreaming";
```

```ts
// Meta.ts
import type { KnowledgeStatus } from "./KnowledgeStatus";
import type { Source } from "./Source";
import type { Visibility } from "./Visibility";
export type Meta = { 
id: string, 
type: string, 
title: string, 
description: string, 
tags: Array<string>, 
scope: string | null, 
source: Source | null, 
timestamp: string | null, 
owner: string, 
visibility: Visibility, 
status: KnowledgeStatus, 
hint: boolean, 
evidence: Array<string>, 
verified: string | null, 
valid_until: string | null, 
superseded_by: string | null, 
merged_into: string | null, };
```

```ts
// Put.ts
import type { KnowledgeStatus } from "./KnowledgeStatus";
import type { Visibility } from "./Visibility";
export type Put = { 
id: string, 
type: string, 
title: string, 
description: string | null, 
tags: Array<string>, 
scope: string | null, 
body: string, 
visibility: Visibility | null, 
status: KnowledgeStatus | null, 
hint: boolean | null, 
evidence: Array<string>, 
verified: string | null, 
valid_until: string | null, 
superseded_by: string | null, 
merged_into: string | null, };
```

```ts
// SessionBudget.ts
export type SessionBudget = { 
nanos: bigint, 
currency: string, };
```

```ts
// Ledger.ts
import type { LedgerCount } from "./LedgerCount";
import type { LedgerHintUse } from "./LedgerHintUse";
import type { LedgerQuestion } from "./LedgerQuestion";
import type { LedgerRewind } from "./LedgerRewind";
import type { LedgerSession } from "./LedgerSession";
import type { LedgerStop } from "./LedgerStop";
import type { LedgerToolError } from "./LedgerToolError";
import type { LedgerUserMessage } from "./LedgerUserMessage";
export type Ledger = { 
since: string, 
until: string, 
sessions: Array<LedgerSession>, 
user_messages: Array<LedgerUserMessage>, 
inner_calls: Array<LedgerCount>, 
tool_errors: Array<LedgerToolError>, 
questions: Array<LedgerQuestion>, 
stops: Array<LedgerStop>, 
app_presses: Array<LedgerUserMessage>, 
hints: Array<LedgerHintUse>, 
skill_loads: Array<LedgerCount>, 
rewinds: Array<LedgerRewind>, 
truncated: boolean, };
```

```ts
// LedgerSession.ts
import type { SessionId } from "./SessionId";
export type LedgerSession = { 
id: SessionId, 
title: string, 
kind: string, 
parent: SessionId | null, 
created_at: string, 
turns: bigint, 
tool_calls: bigint, 
errors: bigint, 
model_cost_nanos: bigint, };
```

```ts
// LedgerUserMessage.ts
import type { SessionId } from "./SessionId";
export type LedgerUserMessage = { 
session: SessionId, 
seq: bigint, 
at: string, 
text: string, };
```

```ts
// LedgerCount.ts
export type LedgerCount = { 
name: string, 
calls: bigint, 
errors: bigint, 
sessions: bigint, };
```

```ts
// LedgerToolError.ts
import type { SessionId } from "./SessionId";
export type LedgerToolError = { 
session: SessionId, 
turn_id: string, 
name: string, 
attempt: number, 
at: string, 
output: string, };
```

```ts
// LedgerQuestion.ts
import type { SessionId } from "./SessionId";
export type LedgerQuestion = { 
session: SessionId, 
kind: string, 
state: string, 
answer: string | null, 
answered_by: string | null, 
asked_at: string, };
```

```ts
// LedgerStop.ts
import type { SessionId } from "./SessionId";
export type LedgerStop = { 
session: SessionId, 
turn_id: string, 
stop_reason: string, 
at: string, };
```

```ts
// LedgerHintUse.ts
export type LedgerHintUse = { 
id: string, 
shown: bigint, 
opened: bigint, };
```

```ts
// LedgerRewind.ts
import type { SessionId } from "./SessionId";
export type LedgerRewind = { 
session: SessionId, 
at: string, 
dropped_assistant_text: string, };
```

```ts
// SignalsSnapshot.ts
export type SignalsSnapshot = { 
last_root_turn_at: string | null, 
desks_doing: number, 
new_concepts_since: number, 
sessions_since: number, 
last_dream_at: string | null, 
on_ac: boolean | null, 
battery_pct: number | null, 
engine: string, 
written_at: string | null, 
tz_offset_minutes: number, };
```

```ts
// DreamState.ts
import type { NightSummary } from "./NightSummary";
import type { RejectedEdit } from "./RejectedEdit";
import type { RunRequest } from "./RunRequest";
export type DreamState = { 
last_night: NightSummary | null, 
nights: bigint, 
last_dream_at: string | null, 
meta_note: string, 
rejected: Array<RejectedEdit>, 
run_request: RunRequest | null, 
engine: string, 
journal: string | null, 
paused_until: string | null, };
```

```ts
// RejectedEdit.ts
export type RejectedEdit = { 
id: string, 
target: string, 
content: string, 
reasons: Array<string>, 
night: string, };
```

```ts
// NightReceipt.ts
export type NightReceipt = { 
night: string, 
knowledge: Array<[string, string]>, 
backups: Array<string>, };
```

```ts
// DryRunReport.ts
export type DryRunReport = { 
fresh: bigint, 
retitle_candidates: bigint, 
duplicate_stems: bigint, 
archive_by_use_candidates: bigint, 
verification_sample: bigint, 
sessions: bigint, 
friction_events: bigint, };
```

```ts
// DreamConfig.ts
import type { DreamCaps } from "./DreamCaps";
import type { DreamConfigModels } from "./DreamConfigModels";
import type { DreamGate } from "./DreamGate";
import type { DreamHeavyDay } from "./DreamHeavyDay";
import type { DreamLifetimes } from "./DreamLifetimes";
import type { DreamPreferences } from "./DreamPreferences";
export type DreamConfig = { 
enabled: boolean, 
window: [string, string], 
idle_minutes: number, 
min_sessions: number, 
heavy_day: DreamHeavyDay, 
budget_usd: number, 
run_timeout: string, 
models: DreamConfigModels, 
caps: DreamCaps, 
lifetimes: DreamLifetimes, 
gate: DreamGate, 
preferences: DreamPreferences, 
digest: string, 
home_engine: string | null, };
```

```ts
// DreamHeavyDay.ts
export type DreamHeavyDay = { 
concepts: number, 
sessions: number, };
```

```ts
// DreamConfigModels.ts
export type DreamConfigModels = { 
triage: string | null, 
tidy: string | null, 
propose: string | null, 
judge: string | null, };
```

```ts
// DreamCaps.ts
export type DreamCaps = { 
triage: number, 
merges: number, 
archives: number, 
verifications: number, 
cases: number, 
edits_per_target: number, 
targets: number, 
rows_per_week: number, };
```

```ts
// DreamLifetimes.ts
export type DreamLifetimes = { 
record: number, 
fact: number, 
procedure: number, 
entity: number, };
```

```ts
// DreamGate.ts
export type DreamGate = { 
margin: number, 
max_regression: number, 
min_val_cases: number, 
val_fraction: number, };
```

```ts
// DreamPreferences.ts
export type DreamPreferences = { 
auto_from_explicit: boolean, 
cap_lines: number, };
```

```ts
// DreamNightStarted.ts
export type DreamNightStarted = { "type": "DreamNightStarted", 
night: string, };
```

```ts
// DreamNightFinished.ts
import type { NightCounts } from "./NightCounts";
import type { Proposal } from "./Proposal";
export type DreamNightFinished = { "type": "DreamNightFinished", 
night: string, 
counts: NightCounts, 
spend_nanos: bigint, 
proposals: Array<Proposal>, };
```

```ts
// DreamUndone.ts
export type DreamUndone = { "type": "DreamUndone", 
night: string, 
group: string | null, };
```
## 4. Hand-offs

- **Z (workspace)**: `cargo check --workspace --tests` fails in `crates/tui` only — two test `Meta {` literals (`crates/tui/src/model.rs:14817`, `crates/tui/src/ui.rs:2257`) miss the seven new fields; fix = append `..Default::default()` to each (`impl Default for Meta` is in). `crates/tui` is outside C's list. Nothing else in the workspace references `Meta {`.
- **Z / whoever owns `crates/rpc/tests/prompts.rs`**: `prompts_resolve_everywhere_and_replay_only_while_pending` fails on this branch's base as on main (not 136): the 132 D6 `LateAnswer` path in `agent::answer_ask` needs the agent actor, which the test does not spawn (second `answer_ask` → `"agent actor is not running"`). Pre-existing since `0bea9e65`.
- **Z / agent prompt owner**: `agent::loop_::tests::prompt_parts_in_order_with_blank_lines` fails on main too: its `!full.contains("rules")` trips on the 2a7253e4 header prose ("Write the rules you are under verbatim …"). Not 136; the assertion wants a tighter needle (`"rules.md"` / `"Always rule"`).
- **W1**: fill `Knowledge::{merge, supersede, archive, verify, retitle}` (commit author `dreaming <dreaming@localhost>` when `Source::Dreaming`, `KnowledgeChanged` with the new ops, `Receipt.ids/commit`); the §5.3 rank in `ranked()` (type weights, freshness, use from `uses`; read `knowledge.lifetimes` from your own section — add `lifetimes` to `KnowledgeConfig`); `search` marks `[archived]`; decide whether `knowledge_merge` should pass `cx.session` so `merge` resolves the agent scope (deviation 4); `knowledge_restore` of a merged stub.
- **W2**: the guard at `Reserved::open` (marker in place; `SessionState.budget` is on the turn's state — thread it into `open` or check before calling it) + the `<budget spent cap>` note; `Agents::handle(LedgerSince)` queries and caps (`dreaming_ledger.rs` docs name the tables); harvest: `Put.evidence = [session]`, `Put.status = Some(Fresh)` at the `..Put::default()` marker in `loop_.rs`, title split, `record`/`warning` types; sidecars in `skills.rs`; verify `record_shown` fires once per note (`Turn::round`, right after `self.note("knowledge", …)`).
- **W3**: everything behind `// 136 W3`: `Night` handlers (`BeginNight` must record `(id, commit_before)` — use `knowledge::HistoryOf`), `Signals`: `desks_doing` (roster task events), `on_ac`/`battery_pct` (`pmset -g batt` ≤ 1/min), rewind snapshot (`ContextChanged{reason: "rewind"}` → `ledger/pending/<ulid>.json` — no ulid crate in the workspace; `uuid::Uuid::now_v7()` is time-ordered and already a dep, or add `ulid` to `Cargo.toml` + your crate), `Dreamt` tell after the journal phase; `backlog` counts in `night::status`; `Nights` from `ledger/*.json`; `SetSettings` through the config path (`runtime::write_config_value(path, "dreaming", key, value)` as `SetRunTimeout` does in `schedule/src/lib.rs` ≈ L2529, or the whole-section write `double::sched::SetSettings` uses); `RunNow` → `state.run_request` + `schedule::FireWith { name: "dreaming", overrides: FireOverrides { budget_nanos: Some(usd × 1e9), run_timeout_secs: backlog.then_some(4·3600), payload: Some("mode: backlog …") } }`; `ledger::build` (asks `agent::LedgerSince` + `knowledge::Stats`); `cases`, `edits`, `replay` (`SideCall` with `CallKind::Dream` — check `agent::SideCall` reaches the budget guard; today `SideCall` runs under `Reserved::open` so it does once W2 lands), `preferences::set`, `journal`, `undo`; `dry_run`; the skill prose and prompts; events published; `sensor.py` ↔ `sensor_rules.rs` lockstep (`sensor_rules::tests::rule_table` is the table — add a row there first). The sensor arg list is fixed by `routine_text`; if you add a rule arg, add it to the template. Also: `schedule::file::Spec` does not allow a permanent watcher — `max_runs: 100000` stands in; a `stop: never` key would be a schedule change for Z.
- **W4**: the TS shapes above; `dreaming.status.backlog` is zeros until W3; `budget_left_tonight_usd` = `budget_usd − last_night.spend_usd` when `last_night.night` is today; `next_window` is UTC RFC 3339 of the next local `window[0]`.
- **Z**: wire `t dreaming dry-run --data-dir <copy>` to `dreaming::dry_run`; `just ts-desktop`; fix the tui literals; decide whether `dreaming.*` dotted names should become `dreaming_*` for `api.ts` symmetry (the UI's generated `api.ts` method list is snake_case today — W4 will see the dotted names in `methods.json`).

## 5. Open items the contract leaves to a wave

- Permanent watcher stop semantics (`max_runs: 100000`); `fire: once` in design §9.7 is wrong for a nightly routine — design §9.7 should say `fire: while_true` + `cooldown`.
- `knowledge.lifetimes` config section (W1) vs `dreaming.lifetimes` (mirror) — the as-built of W1 says which wins (contract: knowledge's).
- `Consolidator` answers from `sessions` (`unattended OR owner root`); desks' closing turns and children never pass. A private root passes `may_consolidate` but is refused earlier by `writable()`.
- `SetBudget` currency is the file's (`USD` default); the guard compares against the owner's `model_calls` in that currency only.
- `signals.json.sessions_since` counts distinct sessions that finished a turn since the last `Dreamt`; it survives restarts (read back from the file) but the distinct-set does not (a restart may double-count a session already seen).
- The `uses` table makes `knowledge.sqlite` partly durable: design 10's "the index is deletable" sentence is amended (W1's as-built restates it).

## 6. Verified / not verified

Verified (through build-lock, target dir `~/Develop/theuth-136-target/C`): `cargo test -p knowledge` (36), `-p schedule` (74), `-p dreaming` (15), `-p rpc --lib` (52) green; `cargo test -p agent --lib`: 462 pass, 1 pre-existing failure (above); `cargo test -p rpc` integration: `tests/prompts.rs` pre-existing failure (above); `cargo clippy -p knowledge -p agent -p schedule -p dreaming -p rpc --tests -- -D warnings` green; `cargo fmt … --check` green; `cargo test -p rpc --features ts --lib export_bindings` green (bindings regenerated, name collisions resolved); ONE `BUILD_SLOTS=1 cargo check --workspace --tests`: fails only in `crates/tui` (two test literals, hand-off above).

Not verified: the engine running with the plugin (no `t` against `~/.theuth`, no restart); `sensor.py` under Monty (the rule function is unit-tested in Rust and mirrored by hand; W3 runs it through `SensorRunner` tests); `install_files` against a live data dir; `signals.json` from a real `TurnFinished` (unit test drives the actor directly); the dotted RPC names over a real WebSocket client.
