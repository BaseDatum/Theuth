# 136 — As-built F1 (ENGINE fixes from review R)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-f1`, branch `feat/136-f1`, cut from `aa9696fe` · owns `crates/agent/**`, `crates/knowledge/**`, `crates/schedule/**`, `crates/tools/**`, `crates/runtime/**` (runtime untouched: nothing assigned needed it). Fixes R-1, R-4 (engine side), R-5, R-6, R-7, R-20 (agent/knowledge side), R-21, R-22, R-23, R-25, R-26, R-27, R-28, R-44, R-45 per DECISIONS D1, D4, D5, D6, D15. Commits: `be847671` (review R as docs), `5cb4333f` (every fix), plus the fmt/as-built commit named at the end.

## 0. One screen

- **Budget is per night** (D1, R-1): `agent::Budget` gains `since` (RFC 3339, `#[serde(default)]`, `#[ts(skip)]` — wire type unchanged); `Agents::handle(SetBudget)` stamps it `now` and persists the cap on `sessions.budget` (R-22); `Store::budget_spent(owner, currency, model, since)` sums only rows `created_at >= since` and adds one `estimate` per `pending`/`dispatched` row (R-27). A lazily rebuilt session actor loads the persisted cap (`Session::with_budget`), so a durability-resumed turn is metered before its first call.
- **Tool cap in the fire path** (D4, R-4): schedule front-matter key `cap: read_only|mutating|destructive` → `Spec.cap: Option<tools::Risk>`; `fire_with` sends `tools::SetCap{session, Some(cap)}` beside `SetApprovals`, before `SendInput`. The registry already applies a cap before the approvals bypass (`registry.rs` `action()`): a bypassed, `Mutating`-capped session runs `knowledge_merge` and is refused `shell`/`knowledge_delete` with `denied: subagent is capped at mutating`.
- **Knowledge identity** (D5, R-5/R-7/R-23/R-25/R-44): `stamp` (source + timestamp) is now the *creation* stamp only; every dreaming write to an existing file uses `touch` → `consolidated_at` (new optional `Meta` key, serialised only when set, indexed as a column). `put_for` sets `owner` only on create, `visibility` only on create or when the put names it, keeps `scope` when the writer is dreaming, unions `evidence`, and never demotes a consolidated entry to `fresh`. `source: user` is refused by merge (either side), supersede, archive and a dreaming put — `"<id> is human-authored (source: user); dreaming never rewrites it"`; retitle and verify stay allowed. Retitle inserts under an existing `## Summary`; merge revives the canonical (`hint: true`, no `superseded_by`/`valid_until`/`merged_into`). `split_title` cuts at 119 chars when there is no space (R-45).
- **Who consolidates** (D6, R-6/R-20): `Knowledge` carries the open night's session (`register_consolidating_session` / `clear_consolidating_session` / `is_consolidating_session`); `may_consolidate(s)` = registered session, else the `Consolidator`'s answer, which `Store::may_consolidate` now narrows to `scope = 'owner' AND parent IS NULL AND desk IS NULL AND NOT unattended`. An unattended session with no registered night is refused by every consolidation op and by `knowledge_put` with a consolidation key.
- **Hints** (R-26): the hint path filters `hint = 1 AND status IN ('fresh','tidy')` in the FTS query and drops rank-0 records before the bm25 cut; `search` is unchanged.
- **Opens** (R-28): `knowledge_get` skips `record_opened` for the registered dreaming session only (not for the owner's attended root — those reads are real use).
- **Ledger off the mailbox** (R-21): `Agents::handle(LedgerSince)` is a `DelegatedReply` + `tokio::spawn` (the `ListAcp`/`SideCall` pattern); migration `0039_budget.sql` adds `messages_at ON messages(at)` and `model_calls_created ON model_calls(owner_id, created_at)`.

## 1. Findings → file:line → test

| Finding | Where | Test |
|---|---|---|
| R-1 (D1) | `crates/agent/src/lib.rs` `Budget::since` (`:447-466`); `supervisor.rs` `SetBudget` handler stamps `since` (`:3129-3153`); `store/accounting.rs` `budget_spent(.., since)` `AND created_at >= ?3` (`:443-498`); `loop_.rs` `Reserved::open` passes `&b.since` (`:2573`) | `accounting::tests::budget_spent_windows_from_since` (a $0.90 row before `since` counts 0; the next call goes out; after a $0.20 settle `(spent, est) == (0.20, 0.20)`; the epoch window sees $1.10) |
| R-27 | `store/accounting.rs:460-473` `COALESCE(SUM(status IN (pending, dispatched)), 0)` × estimate | `accounting::tests::budget_spent_counts_in_flight_calls_at_the_estimate` (one settled $0.30 + one dispatched + one pending → spent $0.90; settling the dispatched row at $0.10 → $0.50, estimate $0.10) |
| R-22 | `migrations/0039_budget.sql` (`sessions.budget TEXT NULL`); `store.rs` `set_budget`/`budget` (`:1101-1125`); `supervisor.rs` `SetBudget` persists before the `tell`, `session()` loads `store.budget(id)` and `.with_budget(budget)` (`:756-757`, `:866-868`); `loop_.rs` `Session::with_budget` (`:880-884`), test-only `BudgetOf` | `supervisor::tests::budget_survives_a_rebuild_from_the_store` (`SetBudget` → persisted with a parseable `since`; `UnloadSession`; `PinTitle` rebuilds the actor lazily; `BudgetOf == saved`) |
| R-4 (D4, engine side) | `crates/schedule/src/file.rs` `Spec.cap` (`:427-429`), `FrontMatter.cap`, parse (`:687-696`, error `cap: expected read_only\|mutating\|destructive`), `ScheduleFile.cap` (`#[serde(default)]`, `#[ts(optional)]`), `FileArgs.cap`, `render`; `tools.rs:168-170` (`cap: None`); `lib.rs` `fire_with` (`:1093-1094`, `:1286-1318`) | `file::tests::cap_key` (parse, absent, refusal, editor round trip); `tools::registry::tests::cap_holds_for_a_session_with_approvals_off` (bypassed + `SetCap{Mutating}`: `touch` runs, `wipe` denied with the cap text, not listed) |
| R-5 (D5) | `crates/knowledge/src/store.rs` `put_for` (`:833-921`) | `store::tests::dreaming_put_preserves_identity` (desk's `owner: agent:ops`, `private`, `scope`, `source`, `timestamp` kept by a scope-less dreaming tidy put; `consolidated_at` set; an explicit `visibility` still sets) |
| R-7 (D5) | `store.rs` `stamp`/`now_rfc3339`/`touch`/`ensure_not_user` (`:303-333`); `touch` in merge stubs + canonical, supersede, archive, verify, retitle | `store::tests::ops_do_not_restamp_timestamp` (20-day record: not hinted before, `retitle`, `timestamp`/`source` unchanged, still not hinted); `supersede_archive_verify_retitle_effects_author_and_restore` re-pinned (`source: model` kept, `consolidated_at` set) |
| R-23 | `store.rs:887-906` (`fresh` only when the file has no status or `fresh`; evidence union) | `store::tests::harvest_reput_unions_evidence_and_never_demotes_status`; `put_for_round_trips_the_136_keys_and_unknown_keys` re-pinned to the union |
| R-25 (D5) | `store.rs` `ensure_not_user` in merge (folded and canonical), supersede, archive, dreaming `put_for` | `store::tests::user_authored_entries_are_refused_by_the_destructive_ops` (five refusals, file byte-identical, the other entry not folded, retitle allowed) |
| R-44 | `store.rs` merge canonical (`:1769-1778`), retitle body (`:1923-1928`) | `store::tests::merge_revives_an_archived_canonical`; the retitle test asserts one `## Summary` after two retitles |
| R-45 | `crates/knowledge/src/harvest.rs:66-76` | `harvest::tests::no_space_at_all_stays_within_the_cap` (300 × `x` and 300 × `日` → exactly 120 chars with `…`) |
| R-6 / R-20 (D6) | `knowledge/src/store.rs` `consolidating` field, `register_consolidating_session`/`clear_consolidating_session`/`is_consolidating_session`/`may_consolidate` (`:773-807`); `knowledge/src/lib.rs` `Consolidator` docs; `agent/src/store.rs` `may_consolidate` (`:1278-1292`) | `knowledge store::tests::consolidating_session_is_the_registered_night_or_the_owner_root`; `agent store::tests::may_consolidate_admits_only_the_attended_owner_root` (isolated root, owner root, unattended owner root, child, desk, unknown) |
| R-26 | `store.rs` `ranked(.., live_only)` (`:1460-1551`), `hint_for` passes `true`, `search_for` `false` | `store::tests::hint_window_is_not_spent_on_dead_rows` (4 archived + 1 stub outrank 2 live; `hint(limit 5)` returns both live; `search` sees 7) |
| R-28 | `knowledge/src/tools.rs` `GetTool::call` (`:181-200`) | `tools::tests::consolidation_tools_need_owner_root_or_unattended` extended (registered session → `opened` 0; plain → 1; after `clear` → 2) |
| R-21 | `agent/src/supervisor.rs` `LedgerSince` handler (`:3157-3209`); `migrations/0039_budget.sql` indexes | No new test: the handler body is unchanged, only its executor; the existing `LedgerSince` tests (`dreaming_ledger`, `store/dreaming.rs`) pass through the delegated reply. Timing on the APFS copy was not re-run (no live store in this tree). |

## 2. Shapes agreed for F2 / F3

- **D6 seam** (`knowledge::Knowledge`): `register_consolidating_session(SessionId)`, `clear_consolidating_session()`, `is_consolidating_session(SessionId) -> bool`. `Night` must register the dreaming session **before its first gated tool call**. `BeginNight` is sent from `dreaming_ledger`, which D9 gates on `may_consolidate` → registering only inside `BeginNight` dead-locks the first call. Register at fire time instead: subscribe `schedule::ScheduleEvent::Fired { name: "dreaming", session, .. }` (published by `fire_with` after `SendInput`, before the model's first round trip) and the `RunNow` → `FireWith` reply `Fired::Session(session)`; clear in the night's close (`TurnFinished`, D7). Until F2 wires this, every unattended consolidation op is refused (D6/R-20 as ruled).
- **`Budget::since`**: stamped only by `Agents::handle(SetBudget)`; `FireWith` → `SetBudget` already runs on every fire, so night start = the stamp. Tests that build a `Budget` by hand set `since` explicitly (`"2000-01-01T00:00:00+00:00"` for a lifetime window).
- **`cap:`**: F2 adds `cap: mutating` to `crates/dreaming/src/routine.md` and may patch it in `update_routine_file`; `ScheduleFile.cap` is `#[ts(optional)]` so the desktop `ScheduleFile` fixtures (`NEW`, `draftToFile`) need no change, and the editor's read → render round trip keeps the key.
- **`Meta.consolidated_at`** (`#[ts(optional)]`, serialised only when set): F3's `changedByDreaming` (`desktop/ui/src/components/knowledge/status.ts:18`, `source === 'dreaming' || status !== 'tidy'`) now misses retitled/verified entries — D5 keeps `source`. Suggested: `|| m.consolidated_at != null`. `night.rs` `KnowledgeChanged` handling is unaffected (the event's `source: Dreaming` comes from `consolidate()`, not the file).
- **Harvest/`knowledge_put` semantics F2's skill may rely on**: a dreaming put on an existing entry no longer rewrites `owner`/`visibility`/`scope`/`source`/`timestamp`; `source: user` entries are refused by every destructive op and by any dreaming put (`knowledge_retitle` and `knowledge_verify` remain the only writes allowed on them). The skill's tidy recipe should skip `source: user` rows (belt) — the server refuses them anyway.

## 3. Deviations and judgement calls

- **R-28 scope**: the review's fix was "skip when `may_consolidate(cx.session)`"; under D6 that includes the owner's attended root, whose reads are genuine use. Skipped only for the registered dreaming session (`is_consolidating_session`).
- **R-25 width**: D5 names merge/supersede/archive. `verify` (ok → `verified: <date>`; stale → `status: stale` + note) and `retitle` stay allowed on `source: user`; a dreaming `put` on a user file is refused outright (a body rewrite is more than "retitled/linked").
- **Merging *into* a `source: user` canonical** is refused too (its body would be replaced).
- **D6 literalness**: a root created by `CreateSession` on behalf of a remote parent (67 §8) is `scope = 'owner'` and therefore admitted by `Store::may_consolidate`; D6 names `scope = 'owner'` as the attended rule, so left as ruled.
- **`budget_spent` estimate** (the newest settled row of the model) is deliberately *not* windowed by `since`: last night's cost is the best estimate for tonight's first call.
- **R-21**: indexes added; the scan still runs on the read pool, now off the supervisor mailbox. No `spawn_blocking`: sqlx is async.
- **tools fmt**: `cargo fmt -p tools -- --check` failed on the base tree (`approval.rs:326`, `ask.rs:402`, pre-existing); formatted so the gate passes.
- **Pre-existing flake**: `schedule::watch_tests::queued_payload_is_regated_when_the_run_closes` failed once under the parallel suite (`Local::now()` timing), passes in isolation ×3 and in the full re-run. One agent lib test run died with SIGSEGV before any test output; the re-run passed 472/472. Neither touches changed code.

## 4. Gates (all through `build-lock`, `CARGO_TARGET_DIR=…/W2`)

| Gate | Result |
|---|---|
| `cargo test -p knowledge` | 52 passed |
| `cargo test -p schedule` | 75 passed (after the flake above) |
| `cargo test -p tools` | 176 passed, 1 ignored |
| `cargo test -p agent --lib` | 472 passed, 2 ignored |
| `cargo clippy -p agent --tests -- -D warnings` | clean |
| `cargo clippy -p knowledge -p schedule -p tools --tests -- -D warnings` | clean |
| `cargo fmt -p {agent,knowledge,schedule,tools} -- --check` | clean after formatting |

## 5. POLISH (R-35…R-49) in my crates

None of R-35…R-49 lands in agent/knowledge/schedule/tools except R-44 and R-45 (fixed above); the rest are dreaming/desktop (F2/F3).
