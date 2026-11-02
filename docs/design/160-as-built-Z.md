# 160 — as built: Z (integration, gates, live proof)

Branch `integrate/160` (worktree `~/Develop/theuth-160`), from `9026c641`. This note is the last commit.

## Merges

| commit | what |
|---|---|
| `2b54be34` | `--no-ff feat/160-eng` @ `fa8e654e`, clean |
| `573b8474` | `--no-ff feat/160-lim` @ `58189469`. One conflict, `crates/agent/src/limits/mod.rs` (add/add): LIM's file taken whole. `lib.rs` auto-merged into two `pub mod limits;` lines; the duplicate was removed. `supervisor.rs` auto-merged: ENG's cost service wiring plus LIM's `// 160:LIM pollers` line. |
| `9c595754` | `--no-ff origin/main` @ `5e135d81` (`f6ba0cc8` Slack Stop/forward + monty 1.4.0), clean. `3f4a1754` is not in the history (`git merge-base --is-ancestor` → 1). `Cargo.lock` has main's slack-morphism `d207bb5` plus LIM's genai line and the `reqwest` line in `agent`. |
| `3f1ad7b4` | `--no-ff feat/160-ui` @ `ae5d0a00` (the sha in `UI-DONE`), clean |

**genai pin.** Before R1, `origin/main` still pinned `skymoore/rust-genai` `branch = "main"` @ `617cafe`, and the fork's `main` had neither the sync nor `ee51f82`. So the override stayed as LIM left it (`theuth-160-response-headers` @ `ee51f822`). That override is now dropped; see "Fold of 33daa811".

**Bindings.** I seeded `bindings/` from the ENG tree, then regenerated on the merged tree with `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` (8 passed) and ran `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`. `methods.json` lists `cost_calls`, `cost_cube`, `usage_limits` and `usage_limits_refresh`. With doc comments stripped, all 20 of the UI's hand-written stubs match the generated types exactly: the 13 cost types in design §3.1 and §4, plus the 7 limits types in §5.2. Neither side broke the contract. The UI's inline `CostCubeChanged` now comes from its binding (`bc0a4284`, UI deviation 1). `api.test.ts` is green.

## Fold of 33daa811 (R1)

- `5fdbfd8d`: `git merge --no-ff origin/main` @ `33daa811`. This brings in the lock refresh (`dedf2089`), the genai fork synced to upstream `0a32194` plus `on_response` (`af110827`), iocraft 0.9.3, unjar 0.1.1 and vendored a2a-client 0.2.9. `3f4a1754` is not in HEAD.
- The only conflict was `Cargo.lock`. I took main's `Cargo.toml` and `Cargo.lock` whole, which drops our `theuth-160-response-headers` `[patch]` override; genai is now `branch = "main"` @ `d2fd71e`. `cargo metadata` re-resolved the lock. Against `origin/main` the lock diff is exactly one line, `+ "reqwest 0.13.5"` in `agent`'s dependencies. LIM's `reqwest` line in `crates/agent/Cargo.toml` stays.
- Synced genai vs `ee51f82`: `d2fd71e` is the cherry-pick. It has the same `ChatFrameSink::on_response` signature, the `FrameTap` forwarder, and `WebStream.response_tap` fed by `EventSourceStream::with_frame_tap` (LIM deviation 4). `with_raw_frame_sink_arc`, `exec_chat_stream` and `resolve_service_target` are unchanged. `Backend::stream` and `crates/agent/src/limits` compiled with no edits. The upstream adapter changes (temperature/top_p, stop/seed, omitted None fields, Bedrock cache tokens, stream timeouts) touch nothing 160 reads.
- Gates, all re-run on `5fdbfd8d`:
  - `cargo fmt -p agent -p rpc --check`: clean.
  - `build-lock mbx clippy -p agent -p rpc --all-targets -- -A clippy::chunks_exact_to_as_chunks`: exit 0, with 0 warnings in 160 files. The new warnings are 5 `rmcp::model::ServerInfo` deprecations in `crates/mcp-server/src/catalog.rs`, from main's lock refresh.
  - `build-lock mbx test -p agent -p rpc --no-run`: exit 0.
  - `mbx test -p agent -p rpc`: agent lib 652 passed, with the same 4 monty failures (the old `t` on PATH) and the run stopping there.
  - `THEUTH_WORKER=/tmp/160-z/bin/t mbx test -p agent -p rpc --no-fail-fast`: exit 0. Agent lib 656 passed, 0 failed, 4 ignored; notebooks_binding 2; notebooks_block 7; rpc lib 59; alerts 1; prompts 1. (The earlier gate block only reports agent lib because the default fail-fast run stopped after it.)
  - `BUILD_SLOTS=1 build-lock mbx check --workspace`: Finished in 12m 03s, exit 0.
  - Bindings regenerated with `export_bindings` (8 passed) and recopied into the UI tree.
  - UI: `bun run check` gave 0 error TS; `BUILD_SLOTS=1 … CI=1 bun run test` gave Test Files 271 passed (271), Tests 1707 passed (1707); `vite build` built in 21.77 s, costs chunk 528.20 kB (148.07 kB gzip).
  - `build-lock mbx build -p theuth` succeeded, and the result was copied to `/tmp/160-z/bin/t`.
- Live re-proof on a fresh `.backup` copy (19:12 UTC), production bundle via `vite preview`, headless Chrome 1280×900. Screenshots are the `r1-*.png` files in `smoke/`.

| step | result |
|---|---|
| RPC | All equal to truth. Native: $18,366.284251 + $111.197519 repriced = cube estimated $18,477.481770. Reported: $6,131.466234 over 534 meters. Calls: 144,831. Endless: $370.353626 over 67,085 rows. H5: `spend_report(7)` month $360.542485 = cube MTD. `cost_cube(null)`: 3,928 cells, 320,430 B, 23–26 ms. Empty delta: 1.28 ms. `cost_calls`: 3 pages at 9.2 / 9.1 / 9.2 ms, 600 unique rows, order ok. |
| 1 | **PASS.** The first `/costs` DOM sample, 337 ms after the click, already had all 5 KPIs. 20 samples at 50 ms: 0 spinners or skeletons, no "Loading". (`r1-01a`, `r1-01b`) |
| 2 | **PASS.** All: $24,608.95 = $18,477.48 + $6,131.47; 147,335 calls. 30D: $19,464.35. (`r1-02`) |
| 7 | **PASS.** `delete_session` on root `01a111d4…`: the total is $24,608.948004 before and after; the tombstone was written and its 4 calls kept. Sorting by Calls reorders the rows. The row reads "Deleted session · 6 Oct · you · $0.49". A row opens `/c/01a10df6…`. (`r1-07a`–`c`) |
| 9 | **PASS.** One `send_input` on Haiku 4.5 made 2 calls: $0.029681 + $0.000436. Calls ticked at 1.6 s and money at 3.1 s ($360.54 → $360.57) with no reload; the HUD shows "TODAY $360.57" and "$360.57 THIS MONTH". **Header capture through the synced genai works:** `anthropic:api_key` went from `stale` ("no calls observed yet") to `ok` with 4 buckets scoped `claude-haiku` (20k requests, 12M tokens, 10M input, 2M output). (`r1-09a`–`c`) |
| 11 | **PARTIAL.** 24 samples. Click handler plus commit: p50 0.5 ms, p95 0.6 ms. Click → next frame: p50 27.8 ms, p95 73.9 ms, with one 249 ms outlier on `All`. Load average was 27–32 during the run (parallel builds on this machine). The < 16 ms target is still not met; see open item 2. |

Scratch engine and preview stopped; `/tmp/160-z/data`, `/dist` and `/chrome2` removed; `/tmp/160-z/bin/t` (now the folded build) kept.

## R2: ledger pages for every filter, UI R2 merge

### 1. `cost_calls` for rare filters (`e47e50c8`)

- **Migration `0048_cost_calls_filters.sql`** adds `model_calls(provider, created_at)`, `(requested_model, created_at)` and `(kind, created_at)`, plus two partial indexes on `(created_at)`: `WHERE returned_nanos IS NULL AND estimated_nanos IS NULL` (unpriced, plus the repriced rows) and `WHERE returned_nanos IS NOT NULL` (known). I left `call_id` out of the keys: the page orders on `created_at` alone and completes the oldest millisecond separately. Building the same five indexes on a copy of Sky's ledger in the prototype (python `sqlite3`) took 3.3 + 0.5 + 0.4 + 0.04 + 0.03 s; the engine pays that once, at the first start after the upgrade. The cost afterwards is five more indexes to maintain on `model_calls` inserts and settles.
- **`Store::cost_call_rows`** is now built with `sqlx::QueryBuilder`. Every value is bound; only static fragments and index names are pushed. The WHERE holds only the filters present: each list becomes one `IN (…)` of bound values, and `basis IN (…)` appears only when `bases` is set.
- **Which index drives the scan.** SQLite has no statistics on this table (no `ANALYZE`), so its guesses went wrong. Examples from the prototype on the copy, before `INDEXED BY`: amazon-bedrock + round chose the kind index (91 k rows) and took 60 ms; a two-id model list sorted every opus row and took 90 ms. So the scan now runs on a `CallDriver` (`Time · Roots · Provider · Model · Kind · Unpriced · Known`) forced with `INDEXED BY`. `Cube::call_driver` picks it from the cube's own native counts in a single pass over the cells, which takes microseconds. It estimates the rows each candidate reads to fill the page and takes the cheapest: an ordered index reads (its matches × want / the filter's matches); `Roots` reads every row of its roots and sorts them. A driving list of several values becomes one ordered, `LIMIT`ed branch per value merged by `UNION ALL`, so no branch sorts its whole match set. The one-millisecond edge query always runs on `Time`. Before the first cube build every page uses `Time`.
- **Regression test** `costs::tests::call_drivers_return_the_reference_rows`. It builds 120 calls over 6 owners, 5 models (including an alias pair), 4 kinds and all three native bases, with distinct stamps. It runs 12 filter shapes: none, each single filter, value lists, pairs, `roots` plus a model, `bases` unpriced / known / estimated+known with a kind, and an empty list. Each runs × every applicable driver × limits 1 / 7 / 1000 × two time windows. Every result must equal an unindexed `NOT INDEXED` reference query filtered in Rust. The test also checks that the cube picks `Provider` for a rare provider, `Known` for `[known]` and `Time` for no filter. The paging, gap and Σ-money test (`cost_calls_pages_cover_the_cube`) now runs through the cube's drivers too.
- **Timings.** RPC round trips on the debug `t` with a fresh `.backup` copy (20:22 UTC, 0048 applied at boot, the cube built). Each figure is the median of 3, page size 200, first page and then the next page from its cursor:

| filter | rows | first page | next page |
|---|---|---|---|
| none | 200 | 15.9 ms | 21.4 ms |
| providers=[openai] | 200 | **16.5 ms** (was 1,575 ms) | 19.2 ms |
| providers=[amazon-bedrock] | 81 | 11.9 ms | — |
| providers=[anthropic] | 200 | 16.2 ms | 18.0 ms |
| providers=[anthropic, openai] | 200 | 19.5 ms | 25.8 ms |
| models=[rare: bedrock fable-5-1] | 3 | 4.5 ms | — |
| models=opus (2 alias ids) | 200 | 18.6 ms | 23.2 ms |
| kinds=[title] | 200 | 18.9 ms | 38.7 ms |
| kinds=[codemap] | 58 | 6.3 ms | — |
| kinds=[round] | 200 | 14.7 ms | 20.7 ms |
| vias=[omp] (reported pieces only) | 200 | 6.3 ms | 6.8 ms |
| bases=[unpriced] | 200 | 12.4 ms | 14.0 ms |
| bases=[known] (empty) | 0 | 2.2 ms | — |
| bases=[estimated] | 200 | 12.0 ms | 13.8 ms |
| roots=[one small root] | 1 | 2.8 ms (was 2,298 ms before R1) | — |
| roots=owner you (176 roots) | 200 | 15.0 ms | 19.0 ms |
| from/to 1 h | 200 | 13.9 ms | 13.2 ms |
| from/to 1 d | 200 | 12.0 ms | 13.6 ms |
| openai + title | 0 | 11.8 ms | — |
| anthropic + codemap | 58 | 7.2 ms | — |
| amazon-bedrock + round | 78 | 8.1 ms | — |
| anthropic + round | 200 | 14.4 ms | 17.7 ms |
| openai + 1 d window | 0 | 1.3 ms | — |

About 10–15 ms of each row is fixed cost per call: the WebSocket round trip, serialising 200 rows to JSON on a debug build, and merging in the reported leg. `EXPLAIN QUERY PLAN` and run time of the SQL the builder emits for each driver: I reconstructed it in python `sqlite3` on the same migrated copy, and it took 0.1–7.9 ms. The driver shown for each shape is what `call_driver`'s cost rule gives for that filter on this ledger; the rule's picks were asserted only in the unit test, not logged on the live engine.

```
providers=[openai]  → SEARCH m USING INDEX model_calls_provider_created (provider=? AND created_at>? AND created_at<?)
models=opus ×2      → MERGE (UNION ALL): LEFT/RIGHT each SEARCH m USING INDEX model_calls_model_created (requested_model=? AND created_at>? AND created_at<?), LIMIT 201 per branch
bases=[unpriced]    → SEARCH m USING INDEX model_calls_unpriced_created (created_at>? AND created_at<?)
bedrock + round     → SEARCH m USING INDEX model_calls_provider_created (provider=? …)   (was: kind index, 60 ms)
roots=[small]       → SEARCH c USING INDEX cost_owners_root (root_id=?) → SEARCH m USING INDEX model_calls_created (owner_id=? AND created_at>? AND created_at<?)
none / owner=you    → SEARCH m USING INDEX model_calls_created_at (created_at>? AND created_at<?)
every shape         → SEARCH c USING INDEX sqlite_autoindex_cost_owners_1 (id=?)
```

### 2. UI R2 merge (`ae6ac440`, `feat/160-ui` @ `40a905ab` from `UI-R2-DONE`)

The merge was clean; the UI branch had already merged `integrate/160` @ `5fdbfd8d`. It brings the resident `/costs` page (pre-rendered at idle, kept mounted), stock Recharts `<Bar>`s instead of the custom path bars, Hour allowed up to 7 days, and the cap back at 400.

**Gates after the merge**

- `bun install`; `bun run check`: 0 error TS.
- `BUILD_SLOTS=1 … CI=1 bun run test`: 1706 passed, 1 failed. The failure is `src/link/pairing.test.ts` ("large records split/reassemble"), a timing flake under load in the link layer, outside 160. It passes alone: `bunx vitest run src/link/pairing.test.ts` gave 7 passed.
- `vite build`: ✓ built in 22.38 s.
- Rust: `cargo fmt --check` clean. `build-lock mbx clippy -p agent -p rpc --all-targets -- -A clippy::chunks_exact_to_as_chunks`: exit 0, 0 warnings in 160 files.
- `build-lock mbx test -p agent -p rpc --no-run`, then `THEUTH_WORKER=/tmp/160-z/bin/t mbx test -p agent -p rpc --no-fail-fast`: agent lib 657 passed, 0 failed; notebooks 2 + 7; rpc 59; alerts 1; prompts 1.
- `build-lock mbx build -p theuth`: ok.

**Re-measured** on a production build (`vite build` with the scratch engine's URL, served by `vite preview`) against `/tmp/160-z/bin/t` on the fresh copy, headless Chrome 1280×900, real mouse events through CDP:

| measure | p50 | p95 | notes |
|---|---|---|---|
| `/costs` navigation → first full paint | **22.7 ms** | **30.6 ms** | 10 warm runs: real click on the HUD SPEND button, `history.back()` between. Timed from `pointerdown` to the first animation frame in which the Costs band is outside any `inert` subtree, ≥ 5 KPIs are live and the main chart has Recharts bar rectangles. The first navigation after a reload took 33.5 ms. |
| pointerdown → next frame (round 1) | **8.9 ms** | **17.4 ms** | 20 clicks: 7D, 30D, 90D, MTD, All, 30D, Provider, Owner, Kind, Basis, Model, Calls, Tokens, $, vs previous ×2, Week, Day, 24H, 30D. Timed from `pointerdown` to the first rAF after the `click` handler ran. With 20 samples the p95 is the slowest click (MTD). |
| pointerdown → next frame (round 2) | **12.1 ms** | **17.0 ms** | same 20 clicks, load average 154 → 155 (other sessions' builds) |

Screenshots in `smoke/` (12 PNGs):
- `r2-costs-top-{desk,phone}-{soot,bone}`
- `r2-7d-hourly-{desk,phone}-{soot,bone}` (7D by Hour, Hour checked, 25 stock bar rectangles)
- `r2-chart-filtered-{desk,phone}-{soot,bone}` (`provider=["anthropic"]`, grouped by kind)
- `r2-01-costs-after-click-desk-soot`

Scratch engine, preview and Chrome stopped; `/tmp/160-z/data`, `/dist` and `/chrome3` removed; `/tmp/160-z/bin/t` kept (it includes 0048).

## R3: privacy pass (no real names or session titles in committed files)

- **Budgets test.** `crates/agent/src/limits/budgets.rs`, in the LIM test `monthly_caps_and_latest_routine_run` (not `limits/tests.rs`), used two of Sky's roster desk names. They are renamed to the invented `atlas` (the capped agent: `agents/atlas.md`, `set_desk(…, "atlas")`, `agent:atlas`) and `birch` (the agent without a cap). `cargo fmt -p agent --check` is clean. `mbx test -p agent -p rpc --lib -- limits::` gave 31 passed, 0 failed, 1 ignored, including `monthly_caps_and_latest_routine_run`.
- **This note.** The live-step rows 4–6 listed Sky's roster agents by name and quoted a real routine-run title. They now say "five roster agents", `agent:<roster agent>` and "two dreaming-routine night caps".
- **Scans on the committed tree.**
  - The brief's name scan (`git diff origin/main...HEAD -U0 | grep '^+' | grep -n -E '\b(<the six roster names>)\b'`) prints nothing. The same names case-insensitive, and the full desk list read from the ledger (those six plus `theuth`), also print nothing apart from `theuth`, the product name.
  - Every session title (≥ 12 chars) and project name (≥ 4 chars) in `~/.theuth/sessions.sqlite` (1,045 strings, read with `sqlite3 -readonly`) was checked against every added line, whole and as 4-word fragments. Matches: `THEUTH`, which is only code constants and env vars (`THEUTH_160_DB`, `THEUTH_WORKER`, `VITE_THEUTH_URL`); `dreaming · dreaming`, the built-in dreaming routine's generated title used in the budgets test; and "Reply with exactly one", a coincidental fragment of my own live-test prompt. No real chat title remains.
- Kept on purpose, because the brief's evidence asks for them: aggregate ledger numbers (dollar totals, call counts), model ids and two truncated session ids (`01a10df6…`, `01a111d4…`). The smoke screenshots under `~/.theuth/briefs/…/smoke/` show real titles, but they are not in the repository.

## Reviews

### review-160-lim.md (2 BUG, 5 RISK, 9 NIT)

The failing tests were committed first in `1af35798` and run before the fix (log `/tmp/160-z/fail-before.log`):
- `absurd_numbers_saturate_without_panicking` panicked at `headers.rs:93`.
- `codex_window_absurd_seconds_saturate` panicked at `poll.rs:415`.
- `code_flow_token_errors_split_transient_from_hard` failed: a 503 was mapped to Hard.

The fixes are in `c3e1ea04`.

| # | disposition |
|---|---|
| 1 BUG overflow | **fixed.** Saturating arithmetic at all six sites (`headers.rs` retry-after-ms, window-minutes ×60, reset seconds, Go duration, retry-after; `poll.rs` Codex `reset_at` / `reset_after_seconds`). Tests are the two above. |
| 2 BUG token errors | **fixed.** `token_error` treats `: token endpoint: …` (transport) and `returned 429/5xx` as transient. A 4xx, `not logged in` or `login expired` stays a hard error. The test uses Code-flow strings. |
| 3 RISK per-scope 429 | **fixed.** `Row.limited` is now a map keyed by model scope; `""` means the whole row, set by a poll. A 2xx clears only its own scope. `status_transitions…` now asserts that a Haiku 200 (with or without headers) keeps an Opus 429. |
| 4 RISK dead `HeaderTap::error` | **fixed.** I deleted it and its two calls; `Backend::stream` is back to its pre-160 mapping. Failures are observed through the fork's `on_response`. New wiremock test `backend::tests::gateway_429_through_stream_marks_the_row_limited` sends a gateway 429 through `Backend::stream`; the row ends up `limited` with `remaining 0`. |
| 5 RISK refresh cancelled by timeout | **fixed.** `poll::access` runs `oauth::Store::access` in a detached task, so the 15 s timeout only stops waiting. Test `a_timed_out_poll_never_cancels_a_token_refresh`: the poll gives up after 50 ms, the token endpoint answers at 300 ms, and the rotated grant (`r2`) is saved. |
| 6 RISK budgets miss meters / deleted desk roots | **fixed.** Month spend reads the desk from the live row, else from the tombstone (H1). It also adds the cube's placed meter pieces for the month (`CostService::pieces`). The budgets test adds a reported piece, a piece from last month (excluded) and a root delete, and expects $2.50. |
| 7 RISK restart poll storms | **fixed.** `Schedule::new(source, first_due, last_run)` is seeded from the restored row's `observed_at_ms`, so both the background cadence and the kick spacing survive a restart. The test is in `schedule_cadence…`. |
| 15 NIT `create_dir_all` | **fixed.** Uses `platform::files::private_dir`. The `set_sensitive` half is not done. |
| 8–14, 16 NIT | **deferred** (low impact): window/balance date ordering, kill switch vs `presence`, Codex `limit_reached` deadline, sink composition, window id normalisation, read-only pool timeouts and per-routine SQL, dirty-flag comparison, clamping. |

### review-160-eng.md (1 BUG, 7 RISK, 7 NIT)

The failing tests were committed first in `a307c8d9` and run before the fix (log `/tmp/160-z/fail-before-eng.log`):
- `root_delete_keeps_a_meters_fallback_model`: after the delete, the cell read `"unknown", "unknown"` instead of `anthropic/claude-fable-5-1`.
- `late_settles_reach_the_cube`: estimated was `0`; expected `750000000`.

The fixes are in `b7773ee4`.

| # | disposition |
|---|---|
| 1 BUG meter model lost on delete | **fixed.** New migration `0047_cost_owner_model.sql` adds `cost_owners.harness_model`. Both tombstone paths (child and root) copy the session's `harness_model` before the session rows go. `cost_meters` reads `COALESCE(s.harness_model, c.harness_model)`. |
| 2 RISK `?N IS NULL OR` defeats indexes | **fixed for roots and bounds.** A root filter now uses its own static variant (`DISTINCT json_each → cost_owners_root → model_calls_created`, forced with `CROSS JOIN`). Lower bounds are `>= COALESCE(?2, '')`, for both `cost_calls` and the refresh. Measured on Sky's ledger, small root: 2,298 ms → 2.3 ms; largest root: 19 ms. **Open:** a rare provider or model filter still walks the time index (`providers:["openai"]` takes 1.57 s for the first page; common filters take 7–20 ms). |
| 3 RISK unbounded H2 rows | **fixed.** A report whose amount and currency equal the meter's last kept row is not inserted again (tested). Caching pieces per meter is deferred. |
| 4 RISK long in-flight calls | **fixed.** The refresh also re-reads from the oldest hour that still held a `pending` / `dispatched` cell (in memory, no SQL). Tested with a call reserved 3 h before it settles. |
| 5 RISK settle after root delete | **fixed.** `bump_in` also bumps a tombstoned owner itself; it is not returned, so no event fires for a deleted session. Tested. |
| 6 RISK bench leg | **deviation, recorded.** Not implemented. There is no `autoresearch.sqlite` on Sky's machine and `samples.spend_nanos` has 0 rows by design. Add it when a study records spend. |
| 7 RISK idle refresh drops state | **fixed.** `step` stores the advanced cube on `Some(false)` without bumping `rev`. |
| 8 RISK empty full answer before the build | **covered by UI.** UI deviation 10 never lets an empty `full` answer replace a non-empty cube. Live step 10 shows it. |
| 9–15 NIT | **deferred** (no rows affected on Sky's ledger): repriced `status='settled'`, non-USD meters, `:` cut in model ids, piece-id renumbering, unparsable `created_at`, service restart guard, `SpendOwnerKind::Other` doc (changing the doc would mean another bindings export). |

## Gates (merged tree; exact commands and result lines)

```
cargo fmt -p agent -p rpc --check                                          → clean
BUILD_LABEL=160-z build-lock mbx clippy -p agent -p rpc --all-targets -- -A clippy::chunks_exact_to_as_chunks
  → exit 0; 2 pre-existing warnings only (crates/link/src/types.rs:323, crates/agent/src/loop_/thinking_tests.rs:52 deprecated fetch_update); 0 in 160 files
BUILD_LABEL=160-z build-lock mbx test -p agent -p rpc --no-run              → exit 0
mbx test -p agent -p rpc                                                    → agent lib 651 passed / 5 failed / 4 ignored
    · 4 monty tests (sensor::fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks,
      runtime_bridge::guessed_direct_calls…) fail with "unsupported protocol version 6 (server supports 5)":
      the worker they find on PATH is /opt/homebrew/bin/t (Sky's installed theuth, older than main's monty 1.4.0 bump).
      With THEUTH_WORKER=target/debug/t they pass (sensor::tests + runtime_bridge_tests: 13/13 at the next line).
    · supervisor::tests::talk_ending_releases_parked_items / human_input_releases_parked_items_as_steers: 60 s timing
      flake under load (LIM saw the same); `mbx test -p agent -p rpc --lib -- supervisor::tests` → ok. 87 passed.
THEUTH_WORKER=… mbx test -p agent -p rpc --lib -- sensor::tests loop_::runtime_bridge_tests costs:: limits:: …
  → 55 passed, 1 failed (the human_input timing flake above, run beside the monty workers)
  · repl::tests::capping_binds_out_name fails only when THEUTH_MONTY_WORKER is set (it skips otherwise); repl.rs is
    main's monty change, untouched by 160 → pre-existing, outside 160.
BUILD_SLOTS=1 BUILD_LABEL=160-z build-lock mbx check --workspace            → Finished `dev` profile in 6m 03s, exit 0
BUILD_LABEL=160-z build-lock mbx build -p theuth                            → exit 0 (rebuilt after the review fixes)
UI (crates/desktop/ui): build-lock bun install (+ packages/apps for @openuidev/*)
  build-lock bun run check                                                  → exit 0, 0 error TS (again after 3d07ab31)
  BUILD_SLOTS=1 build-lock env CI=1 bun run test                            → Test Files 270 passed (270) · Tests 1706 passed (1706)
  build-lock bunx vite build                                                → ✓ built in 6.21s; costs chunk 528.16 kB (148.06 kB gzip)
  CI=1 build-lock bunx vitest run src/components/costs (after the live fix) → 2 files, 4 tests passed
```

## Live proof: RPC numbers against SQL truth

Setup: a `.backup` copy of `~/.theuth/sessions.sqlite` (3.88 GB, 16:42 UTC) and `roster.sqlite` in `/tmp/160-z/data`. There is no `autoresearch.sqlite`; no secrets were copied. The config follows the brief. The engine was `/tmp/160-z/bin/t` (debug) at `127.0.0.1:7862`. Truth was read with `sqlite3` SELECTs on the same copy; `-readonly` cannot open a WAL copy that has no `-shm`.

| check | cube / RPC | truth | |
|---|---|---|---|
| Σ native (estimated incl. repriced) | $18,405.850010 | `sum(coalesce(returned,estimated,0))` = $18,294.652491, plus the 764 repriced rows = $111.197519 | equal |
| Σ reported | $6,009.102971 | `sum(remote_meters.attributed_nanos)` over 530 meters = $6,009.102971 | equal |
| known | $0 | `returned_nanos` NULL everywhere | equal |
| Σ native calls | 144,000 | non-`not_dispatched` rows = 144,000 | equal |
| estimated, kind `endless` | $369.355254 | `endless_calls.usd_estimate` = $369.355254 (66,681 rows) | equal |
| H5 `spend_report(7)` | month $166.547462 · today $166.547462 · 7 days $178.073965 | the cube's local month, today and 7 days (Europe/Lisbon) = the same three numbers | equal |
| pre-ledger sessions | 86 | owners with `history_coverage='unknown'` = 86 (17 roots) | — |
| unpriced reasons | no usage returned 11,071 · failed 265 · cancelled 230 · dropped 14 · unresolved 9 · dispatched 1 | — | — |

| performance (debug build, WS round trip) | measured |
|---|---|
| `cost_cube(null)` | 3,888 cells · 186 roots · 16 models · 317,062 B JSON. Five calls: 16.5–21.8 ms |
| `cost_cube({epoch, rev})` | empty delta (`full:false`, 0 hours, 0 cells), 2,307 B, 1.1–1.3 ms |
| `cost_calls` | Before the fix, 3 × 200 rows: 33 / 41 / 37 ms. After: 8.4 / 7.5 / 6.8 ms. 600 unique rows, newest-first order ok, no duplicates or gaps. Small root 2.3 ms (was 2,298 ms); largest root 19.1 ms; provider / kind / basis / model / range / via filters 7–20 ms; `providers:["openai"]` 1,575 ms (open). |
| `usage_limits()` | 0.62 ms. Before the call: `anthropic:api_key` stale (snapshot restored from a first boot); `amazon-bedrock` unavailable; `openai:api_key` not_configured (Codex grant not copied, as expected). After the one real call: `ok`, 4 buckets scoped `claude-haiku` (requests 20,000, tokens 12M, input 10M, output 2M). |
| H1 through RPC | `delete_session(01a111d4…)` (a $0.49 root) took 525 ms. Totals before and after: $24,414.952981 both. The tombstone row was written and the root's 4 calls kept. |

## Live proof: the 12 UI steps

Vite dev at `:5862` against the scratch engine; production numbers come from `vite build` + `vite preview` at `:5863`. Headless Chrome 1280×900 (DPR 1.25) and 390×844 (DPR 2). Screenshots are in `~/.theuth/briefs/2026-11-02/160/smoke/`.

| # | step | result | evidence |
|---|---|---|---|
| 1 | HUD SPEND button opens `/costs`, no spinner or skeleton | **PASS** | `01a-hud-desk-soot`, `01b-hud-spend-button`, `01c-costs-after-click-desk-soot`, `01d-hud-desk-bone`, frames `01e-frame-00..05-prod`. DOM checks every 50 ms for 1.2 s: 0 spinner / skeleton / busy nodes, no "Loading". On the production build the first `/costs` sample (315 ms after the click) already had all 5 KPIs. That 313 ms is the main thread busy with the page's first mount (dev mode: 963 ms). Noted below. |
| 2 | totals equal truth (30D, All) | **PASS** | `01c` / `02-totals-all-desk-soot`. 30D: $19,270.35 = cube from local 4 Oct 00:00 (`2026-10-03T23:00Z`) $19,270.354956; estimated $14.3k = SQL $14,293.52; reported $4.98k; calls 128,777 = 126,390 native + 2,387 turns. All: $24,414.95 = truth; 146,474 calls; 764 priced at read time; 11,536 failed. |
| 3 | presets, custom range, drag-zoom, granularity guard | **PASS** | `03a`–`03e`. All 7 presets write the URL. Calendar 5–9 Oct → `range=custom&from=…&to=…` (Hour; Week and Month disabled). Drag across the chart → `ReferenceArea`, then a custom 12–22 Oct. The guard uses UI R1's cap (≤ 120 buckets), not §6.6's 400, so 7D Hour is disabled. |
| 4 | every group-by and metric; legend toggle; previous period | **PASS** | `04-metric-calls`, `04-metric-tokens`, `04a-by-owner-metric-tokens`, `04b-legend-toggled-fable-off`. Seven group-bys give their own legends (model 7 + other; provider 2; owner: you, the `theuth` desk, five roster agents, other; kind; via theuth / omp; basis; project). The legend click hides the series and rescales. "vs previous": 1 dashed line → 0 (`compare=false`). |
| 5 | each chip; cross-filter from a bar segment, a breakdown row, an owner | **PASS** | `05a-chip-model-open`, `05b-crossfilter-bar`, `05c-crossfilter-owner`. All 7 chips list options with spend for the range (e.g. `anthropic $19.3k`, `openai $3.38`, `amazon-bedrock 7 calls`) and write `?provider=["anthropic"]` and so on. Bar segment → `model=["claude-fable-5-1"]`; provider row → `provider=["openai"]`; owner bar → `owner=["agent:<roster agent>"]`. |
| 6 | limits panel | **PASS** | `06-limits-panel-after-call-desk-soot`, `12f-phone-limits-*`. Anthropic API `ok` with 4 rate meters after step 9's call. Amazon Bedrock is one muted line: "no api · Bedrock exposes no usage meter…". OpenAI API not configured. Budgets card: two dreaming-routine night caps, $7.73 / $25 and $8.56 / $25. **Fixed here:** the panel read "generated in 45 s" (see Fixes). |
| 7 | sessions sort, a row opens the session, deleted root | **PASS** | `07a-sessions-sorted-calls`, `07b-deleted-session-row`, `07c-session-opened`. Sorting by Calls reorders the rows (34,013 first). A row opens `/c/01a10df6…`. The root deleted through the RPC shows as "Deleted session · 6 Oct · you · claude-haiku-4-5+1 · 4 · 118.5k · $0.49", and All stays $24,414.95. |
| 8 | ledger: three pages, previous rows stay, no spinner | **PASS** | `08-ledger-after-3-pages-desk-soot`. Four scrolls to the end: the scroll height goes 6,628 → 13,228 → 19,828 → 26,428 → 33,028 px (200 rows a page); `scrollTop` keeps its place; 0 spinners, no "Loading", no hairline needed. |
| 9 | one live chat on Haiku 4.5 reaches `/costs` and the HUD within 15 s | **PASS** | `09a-costs-after-live-call`, `09b-hud-after-live-call`. `send_input(null, "Reply with exactly one word: ok")` made 2 calls (turn $0.029706, title $0.000436), about $0.03. On `/costs` Calls moved 1,295 → 1,296 at 2.2 s (reserve) and spend moved $166.55 → $166.58 within about 6 s, with no reload. HUD: "TODAY $166.58" / "$166.58 THIS MONTH". The Anthropic row went `stale` → `ok` with 4 buckets. |
| 10 | cold start from IndexedDB, then reconcile | **PASS** (with a note) | `10a`–`10d`. With the engine *stopped*, the app shell's global "connect failed" gate takes over before any route renders (`10a`); that gate is outside 160. So the socket was blocked for `cost_cube` only: an init script drops the request (1 dropped). The page painted $24,414.98 / $166.58 from IDB (`theuth-costs` / `cube` / `v1:ws://127.0.0.1:7862`) with 0 spinners (`10c`). After unblocking and reloading, the IDB epoch `01a19d62…` and rev 1 equal the live engine's (`10d`). |
| 11 | interaction timing on the real cube | **PARTIAL** | 12 controls (6 ranges, 4 group-bys, 2 metrics), run twice, 24 samples on the production build. Click handler plus commit: p50 0.5 ms, p95 0.8 ms (R1 defers rendering). Click → next animation frame, which includes the wait for vsync: **p50 20.4 ms, p95 42.8 ms**. The < 16 ms target is not met at p95 here (headless, shared machine); the slowest were 7D, MTD, Owner and Model. Dev mode: p50 57 ms, p95 91 ms. |
| 12 | phone 390×844 | **PASS** | `12e-phone-top-{soot,bone}`, `12f-phone-limits-*`, `12g-phone-sessions-*`, `12h-phone-ledger-*`, `12i-phone-chart-*`. Page width 390 = viewport (no sideways page scroll). The 8 chips wrap onto 2 rows. KPIs sit in 2 columns. The sessions and ledger tables scroll sideways inside their panels. The rate meters and budgets are legible. Bone desktop: `12a`–`12d`. |

## Fixes made by Z

- `1af35798` / `c3e1ea04`: LIM review (above).
- `a307c8d9` / `b7773ee4`: ENG review (above), including migration `0047`.
- `bc0a4284`: `CostCubeChanged` imported from its binding.
- `3d07ab31`: the usage-limits panel showed "generated in 45 s". The page clock ticks once a minute while the snapshot refreshes every 15 s. The panel's clock is now `max(page now, generated_at_ms)`. Test `LimitsPanel.test.ts` failed first (`expected … to contain 'generated just now'`) and passes after the fix.

## Open items

1. ~~Rare-filter ledger pages~~ **resolved in R2** (`e47e50c8`): every case in the R2 table is < 40 ms.
2. **§6.5 < 16 ms at p95**: R2 brings click → next frame to p50 8.9 / 12.1 ms and p95 17.4 / 17.0 ms over two rounds of 20 clicks, at a load average of 150. The first `/costs` paint after a navigation is 22.7 / 30.6 ms (p50 / p95). p95 is about 1 ms over the target on a fully loaded machine; re-measure when idle.
3. **"Do not hand roll the charts"**: the UI's R2 reverted the main chart to stock Recharts `<Bar>`s, and 7D by Hour draws 25 `.recharts-bar-rectangle` nodes. One small custom `<path>` is left: the empty-cell background of the `When` heat grid (`When.tsx:36`), drawn inside the Recharts `ScatterChart`. The cells themselves are Recharts Scatter points.
4. ~~Granularity cap 120~~ **resolved in UI R2**: Hour is allowed up to 7 days (7D by Hour is on), and every other choice is capped at 400 buckets.
5. **Engine-down cold start.** The app shell's connect gate hides every route, `/costs` included, when the engine is unreachable at boot. IDB painting works whenever the shell is up and the cube RPC is slow or blocked.
6. **Pre-existing, outside 160:** the monty worker tests need `THEUTH_WORKER` pointing at a binary built after main's monty 1.4.0 bump (the PATH `t` is older); `repl::tests::capping_binds_out_name` fails with a current worker; the supervisor talk tests flake on 60 s timing under load.
7. Deferred review NITs as listed; the bench leg as noted (ENG #6).
8. The limits footer says "resets N ago" for a bucket whose reset has passed; that bucket already reads as full. Wording only.
9. Not live-verified (no grant on the scratch engine): the Codex and Claude OAuth pollers, OpenRouter / OpenCode Go / DeepSeek.

## Cleanup

I stopped my engine, vite dev, vite preview and Chrome (my processes only), and removed `/tmp/160-z/data`, `/tmp/160-z/dist` (it held the scratch token) and `/tmp/160-z/chrome`. `/tmp/160-z/bin/t` and its helpers are kept.
