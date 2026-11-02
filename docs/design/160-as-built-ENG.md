# 160 — as built: ENG (cost cube, calls ledger, RPC plumbing)

Branch `feat/160-eng` (worktree `~/Develop/theuth-160-eng`), cut from `9026c641`. Code commit `82bfe018`; this note is the last commit.

## What was built (file level)

| file | what |
|---|---|
| `crates/agent/src/costs/types.rs` | §3.1 / §4 wire types exactly as frozen (`CostBasis`, `CostCubeCursor`, `CostRoot`, `CostRootEntry`, `CostModel`, `CostCells`, `CostReason`, `CostCubeMeta`, `CostCube`, `CostCallsCursor`, `CostCallsQuery`, `CostCall`, `CostCallsPage`) + the bus/wire event `CostCubeChanged { epoch, rev }`. Crate derive pattern (`schemars`, `ts(export)`, `ts(type = …)` on every u64). `CostCallsQuery` is `#[serde(default)]` so the UI may omit filters. |
| `crates/agent/src/costs/mod.rs` | `Prices` (catalog rates by exact `provider/model`, sha256 `price_table`, priced-id JSON for SQL), `estimate` (= `loop_::estimate` in nanos), `cache_saved`, `model_name` (canonical display name: catalog `canonical` else raw id, provider stripped, `-YYYYMMDD` suffix stripped), time helpers (`ms_text` = a text bound that compares like chrono's `AutoSi` RFC 3339). |
| `crates/agent/src/costs/place.rs` | recon §2.4 placement of meter money, money-conserving per meter: exact report increments (running max), pre-window remainder over the owner's turns weighted by catalog-priced tokens (uniform when none price), no reports → turns else `updated_at`, residual at the last report; cap at `attributed_nanos`. Reports = `remote_meter_reports` rows ∪ `source_metadata.reports`, deduped by `(at, amount)`. Pieces sorted by `(at_ms, id)`. |
| `crates/agent/src/costs/cube.rs` | the cube: native cells per root `BTreeMap<(hour, model, provider, kind, basis, reason)>`, reported cells, dictionaries (append-only per epoch), root dimension with per-root change rev, change log (1024 revs), `build` (new epoch, rev 1), `refresh` (§3.3 owner-revision diff → dirty roots → re-aggregate hours ≥ `hour(last_refresh − 15 min)`; reported leg + roots rebuilt in full; `None` → full rebuild when > 50 roots dirty), `answer(since)` (§3.2 full/delta), meta. |
| `crates/agent/src/costs/calls.rs` | `cost_calls`: native rows keyset-paged newest first on `created_at` text with the oldest millisecond completed so a page edge never splits it, re-ordered by `(at_ms, id)`; reported pieces interleaved; money per row identical to what the cube sums. |
| `crates/agent/src/costs/service.rs` | `CostService`: background task (full build at start, refresh on `ModelUsageChanged` / `SessionDeleted` / `TitleChanged` / `SessionMetadataChanged` / `SubagentSpawned` with a 1 s trailing debounce capped at 3 s, and a 15 s tick); publishes `CostCubeChanged` after a change (≥ 1 s apart by construction); `cube()`, `calls()`, `spend_report()` (H5), `set_catalog()` (price-table change → full rebuild); `install()` / `engine()` process-wide handle. |
| `crates/agent/src/costs/tests.rs` | tests (below) + the `#[ignore]`d measurement on `THEUTH_160_DB`. |
| `crates/agent/src/limits/mod.rs` | STUB: exactly the §5.2 types and a neutral `LimitsHub` (`global()`, `snapshot()` → empty providers/budgets + now, no-op `observe_headers` / `kick`). LIM replaces the file whole. |
| `crates/agent/migrations/0046_cost_history.sql` | `cost_root_tombstones` (H1), `remote_meter_reports` + index (H2), `model_calls_created_at` index (§4, see deviations). |
| `crates/agent/src/store/accounting.rs` | H1 `tombstone_root_in` replaces `delete_root_ledger_in` (owners tombstoned, one tombstone row, calls/meters/turns kept); shared `root_owner` rules (live row, else tombstone, else `other`); H2 `record_report_in` at every applied report (same `at` as the metadata ring); H5 `spend_report(now_ms, days, reported)`; cube/calls reads (`cost_roots`, `cost_revisions`, `cost_pre_ledger_sessions`, `cost_native`, `cost_meters`, `cost_meter_reports`, `cost_turns`, `cost_call_rows`). |
| `crates/agent/src/store.rs` | `delete_session` root branch calls `tombstone_root_in` (doc comment updated). |
| `crates/agent/src/store/turn_usage.rs` | test updated: turns survive a root delete. |
| `crates/agent/src/lib.rs` | `pub mod costs;`, `pub mod limits;`, exports of every cost type and `CostService`. |
| `crates/agent/src/supervisor.rs` | `Agents.costs`; service created, spawned and installed in `start` (after `Auth`, before the actor); every `AgentEvent` goes through `costs.observe`; catalog reloads (`AddProvider` / `RemoveProvider`) call `set_catalog`; `SpendReportQuery` → `CostService::spend_report`. |
| `crates/rpc/src/api.rs` | `cost_cube`, `cost_calls`, `usage_limits`, `usage_limits_refresh` beside `spend_report` + one contract test. |
| `crates/rpc/src/bridge.rs` | `CostCubeChanged` forwarded onto `subscribe_events`. |
| `crates/rpc/src/export.rs` | binding exports for all cost and limits types. |

## Deviations from the design, and why

1. **No new actor messages.** `cost_cube` / `cost_calls` read `agent::CostService::engine()` (a `OnceLock` set by `supervisor::start`) and `usage_limits*` call `LimitsHub::global()` straight from the RPC handler. Reason: the supervisor's mailbox can be busy for seconds (session loads, boot); D8 says the page never waits. `CostCubeQuery` / `CostCallsQuery` / `UsageLimitsQuery` messages do not exist; `CostCallsQuery` is only the wire type. Before the agent plugin runs the RPCs answer `NOT_RUNNING` (-32001). `spend_report` still goes through `SpendReportQuery` on `Agents`.
2. **`LimitsHub::global()`** uses a `LazyLock` static (project rule: LazyLock when the initializer is fixed). The API is the frozen one.
3. **`Store::spend_report(now_ms, days, reported: &[Piece])`** gained the reported pieces and is now `pub(crate)` (no caller outside the crate). H5 adds placed reported money to `days[].usd`, `month.usd` and the owner's `usd`, **not** to `calls` (`SpendDay.calls` is documented as attempts). Owners now use the shared root rules including tombstones: a deleted root's spend stays under its agent / routine / `you` instead of `other`. The existing test is unchanged except for the new argument.
4. **`CostCubeChanged`** is a standalone bus struct with `#[serde(tag = "type")]` (the `EndlessProgress` pattern), not an `AgentEvent` variant; TS: `{ "type": "CostCubeChanged", epoch: string, rev: number }`. It carries no `session`, so plain and session subscriptions pass it; a `descendants: true` subscription drops it (the UI's global subscription gets it).
5. **Index added** `model_calls_created_at ON model_calls(created_at)`: on the copy, the unfiltered newest-first page planned `SCAN m … USE TEMP B-TREE FOR ORDER BY` (1.93 s); with the index `SEARCH m USING INDEX model_calls_created_at (created_at<?)` (37 ms).
6. **Money in another currency** (none exists): native rows count as calls with $0 in the cube and `nanos: 0` in `cost_calls`; non-USD meters are not placed.
7. **`first_call_ms`** has hour precision (earliest cell hour), or the earliest placed piece when earlier.
8. **Reported model / provider**: piece model = the turn's model (turn-weighted pieces) else the owner's latest turn at or before the piece, else its first turn, else `sessions.harness_model`, else `unknown`; `:effort` stripped; provider = the model's prefix, `unknown` without one.
9. **In-flight rows**: `reserve_call` / `mark_dispatched` / `mark_not_dispatched` bump no revision, so a pending row appears when its root next refreshes (the settle bumps it). A reserve followed by `not_dispatched` with no other bump of that root inside 15 min could leave a stale pending count until the next full build (engine restart); there are 0 `not_dispatched` rows on Sky's ledger.
10. **Vanished owners** (only possible from a pre-160 erase) re-aggregate their root's whole history; > 50 dirty roots or a price-table change → full rebuild with a new epoch.
11. **Root dimension**: every `cost_owners` root (186 on the copy, incl. roots without calls). A deleted root: `title: ""`, `deleted: true`, owner from the tombstone, `project` / `created_at_ms` from the tombstone. A root whose session row is gone without a tombstone (pre-160) → `other`, `created_at_ms: 0`.
12. **`meta.engine`** = `platform::process::hostname()`.
13. **Bindings** regenerated (`bindings/`, gitignored) but **not copied** into `crates/desktop/ui/src/rpc/bindings/` — `crates/desktop/**` is outside ENG's ownership; the closer regenerates on the merged tree.
14. **Toolchain**: `stable` is rustc/clippy 1.99.0 on this machine (the brief says 1.96.1). The bare clippy gate fails on a pre-existing lint outside ENG's files (`crates/voice/src/history/mod.rs:465`, `clippy::chunks_exact_to_as_chunks`, denied via `clippy::all`); the gate was run with that one lint allowed (below).

## Tests (agent crate unless noted)

New (7 + 1 ignored in `costs::tests`, 1 in rpc):
- `legs_reconcile_with_the_ledger` — known / estimated / repriced / unpriced / reported sums equal SQL truth, call count, repriced count and formula, failed count, turn tokens on the reported leg with the effort suffix stripped, `cache_saved` formula, unpriced reasons, owners (D6), model display name.
- `root_delete_keeps_the_books_under_its_owner` — `delete_session` on a desk root writes the tombstone, cells unchanged, root shown deleted under `builder`.
- `incremental_equals_full_rebuild` — after each of settle / cancel / meter report / turn / retitle / new root / root delete: incremental cube == fresh full build (cells and roots), and a client fed only deltas holds the same cells and roots; an idle refresh does not bump rev.
- `delta_answers` — empty delta when current; full on null, other epoch, rev ahead; delta after a change carries the touched hour only; full when the rev is older than the log.
- `placement_conserves_attributed` — full history with a lower total, window-limited meter completed by H2 rows, turn-weighted, bare fallback, imported baseline; Σ pieces = `attributed_nanos` per meter; sort order.
- `cost_calls_pages_cover_the_cube` — 7 rows in one millisecond, a whole-second stamp and a µs stamp; page sizes 1–1000; no duplicates, no gaps, newest first, Σ page nanos = cube total for no filter / estimated / reported+unpriced; model filter.
- `spend_report_includes_reported_money` — +$0.35 on today, the month and `you`; calls unchanged.
- `measure_on_a_real_ledger` (`#[ignore]`, `THEUTH_160_DB`) — numbers below, with the §3.4 equalities asserted.
- rpc `cost_methods_are_registered_with_their_wire_shapes` — four methods registered, partial `CostCallsQuery` parses, `usage_limits` answers, `CostCubeChanged` JSON shape.

Updated: `store::accounting::tests::worked_example_lineage_replay_tombstone_delete` (root delete keeps 3 calls and the $0.60, 0 live owners), `spend_report_buckets_days_month_and_owners` (new argument), `store::turn_usage::tests::remote_turns_sum_per_model_and_cascade` (turns stay).

## Measured on a copy of Sky's ledger

`sqlite3 -readonly ~/.theuth/sessions.sqlite ".backup /tmp/160-eng/sessions.sqlite"` (2026-11-02 14:54, 3.9 GB), then `THEUTH_160_DB=/tmp/160-eng/sessions.sqlite <agent test binary> costs::tests::measure_on_a_real_ledger --ignored --nocapture` (debug build, shared machine under load).

| §3.4 check | cube | SQL truth on the copy | |
|---|---|---|---|
| Σ native (known + estimated − repriced) | $18,241.6515 | `sum(coalesce(returned,estimated,0))` non-`not_dispatched` = $18,241.6515 | equal (asserted, nanos) |
| repriced leg | 764 calls, $111.1975 | 764 rows with tokens and no money, priced per row = $111.1975 (recon: $111.197) | equal |
| Σ reported | $5,952.8450 | `sum(remote_meters.attributed_nanos)` = $5,952.8450 (522 meters) | equal (asserted) |
| Σ calls | 143,463 | non-`not_dispatched` rows = 143,463 | equal (asserted) |
| estimated, kind `endless` | $369.294379 | `model_calls` endless $369.294379; `endless_calls.usd_estimate` $369.294379 (66,644 attempts) | equal, within 1 nano/attempt (asserted) |
| month to date (local) | $57.288421 | `spend_report(…).month.usd` after H5 = $57.288421 (2026-11) | equal (asserted) |
| known | $0.00 | `returned_nanos` NULL everywhere | — |
| placement | 2,450 pieces; $968.77 approximate (16.3 %), rest exact | recon: 83.7 % exact | conserves per meter |
| cache saved | $223,513.19 all legs; $132,653.57 native | recon §11: $131,882.74 read savings (8 priced models, native) | native incl. sonnet-5 + repriced rows |
| unpriced reasons | no usage returned 11,071 · failed 265 · cancelled 229 · dropped before settlement 14 · unresolved 9 | recon §3.1 | — |
| pre-ledger sessions | 86 | `history_coverage = 'unknown'` = 86 | equal |

| performance | measured |
|---|---|
| full build (background) | 0.95 s and 2.16 s (two runs, debug, load) |
| cells / roots / models | 3,867 / 186 / 16 |
| serialized full `cost_cube` JSON | 315,442 bytes |
| full answer from memory | 2.6–4.7 ms |
| delta answer (current) | 0.29 ms |
| idle refresh (no change) | 115 ms and 44 ms (two runs) |
| refresh with one dirty root (newest call's) | 91 ms |
| `cost_calls` first page (200 rows, unfiltered) | 26 ms and 106 ms (two runs); second page 39 ms and 29 ms; reported-only page 0.15 ms |

## Gates (exact commands, result lines)

```
cargo fmt -p agent -p rpc                                                     # clean
BUILD_LABEL=160-eng ~/.theuth/bin/build-lock mbx clippy -p agent -p rpc --all-targets -- -A clippy::chunks_exact_to_as_chunks
  → exit 0; remaining warnings pre-existing: crates/link/src/types.rs:323, crates/agent/src/loop_/thinking_tests.rs:52 (deprecated fetch_update)
  (bare: exit 101, only `crates/voice/src/history/mod.rs:465 chunks_exact_to_as_chunks`, pre-existing)
BUILD_LABEL=160-eng ~/.theuth/bin/build-lock mbx test -p agent -p rpc --no-run   → exit 0
mbx test -p agent -p rpc                                                       → exit 0
  agent lib:        test result: ok. 622 passed; 0 failed; 3 ignored
  agent notebooks_binding: ok. 2 passed · notebooks_block: ok. 7 passed
  rpc lib:          test result: ok. 59 passed; 0 failed; 0 ignored
  rpc alerts: ok. 1 passed · prompts: ok. 1 passed · doc-tests: 0
BUILD_SLOTS=1 ~/.theuth/bin/build-lock mbx test -p rpc --features ts --lib --quiet export_bindings
  → test result: ok. 8 passed; methods.json lists cost_calls, cost_cube, usage_limits, usage_limits_refresh
```

## Open items for the closer

- Copy `bindings/.` into `crates/desktop/ui/src/rpc/bindings/` on the merged tree (ENG did not touch `crates/desktop/**`); the UI's `api.ts` METHODS must list the four methods (UI owns).
- LIM: `crates/agent/src/limits/mod.rs` is replaced whole; `lib.rs` already has `pub mod limits;` (keep one). ENG's `supervisor::start` addition (cost service, after `Endless::new`) sits after `Auth` is built, where LIM adds its `// 160:LIM pollers` line — adjacent hunks, no shared lines expected.
- Live proof (§10): the spawn loop, `CostCubeChanged` on `subscribe_events` and the RPC → `CostService::engine()` path are covered by unit tests only up to the service; first exercise end to end on the scratch engine. Release-build timings there.
- Pre-existing clippy 1.99 failure in `crates/voice` blocks a bare `mbx clippy` gate on any crate set that builds `voice`.
