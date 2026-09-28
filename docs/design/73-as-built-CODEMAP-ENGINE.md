# 73 — as built: CODEMAP-ENGINE (E1–E2 + wiring, `feature/73-codemap-engine`)

Built 2026-09-28 on `feature/73-codemap-engine` (base `02b17d8a` = `feature/codeindex-wave2` + `feature/73-codemap-ui`), against design 73 §5/§7/§11 and the UI's fixtures under `crates/desktop/ui/src/components/codemap/__fixtures__/` (the contract; they won where §5 and they differed — see Deviations). Brief: `/tmp/enh-2026-09-28/omp-codemap-engine.md`.

## Commits

| # | Subject | What |
|---|---|---|
| `0feba1f7` | `feat(codeindex): CodeToolRan/CodeReconciled events (73 E1)` | `event.rs`, runtime handle + per-root `seq`, 250 ms gate, `Codeindex::touch`, `ran!` on every `code_*` call, bridge/client wiring |
| `1478d14e` | `feat(rpc): codemap lens RPCs (73 E2)` | `lens.rs` (types + here/fit/change/touch), store queries, actor messages, four RPCs, `export.rs`, `tests/codemap.rs` (E1 tests included here: one test file) |
| `d8de13ee` | `feat(ui): codemap on the engine adapter` | bindings replace the hand-typed wire types, `Api`/`METHODS`, engine adapter by default, `ts(type)` narrowing on `tool`/`args` |
| `b366be7b` | `perf(codeindex): worktree diff skips stat-clean files; lens roots need a completed index` | git-index stat shortcut (WORKTREE 3 470 → 206 ms), `git_head` on the first `CodeReconciled`, `open_indexed` requires `meta.last_reconcile_at`, `scripts/codemap-smoke.ts` |
| (this) | `docs(73): as-built CODEMAP-ENGINE` | this note |

## What changed (file:line)

Engine — events (E1)
- `crates/codeindex/src/event.rs` — `CodeindexEvent { CodeToolRan, CodeReconciled }` (`:23`, `#[serde(tag = "type")]`, `ts_rs` under `ts`; `tool` narrowed to the six names and `args` to `Record<string, unknown>` by `ts(type)`), `CodeToolSummary` (`:85`) + `SummaryDefinition`/`SummaryImpact`/`SummaryRange`; `CodeToolSummary::of(tool, args, reconciled, output)` (`:175`) reads rows/dropped/definition/impact counts/path off the tool's structured envelope and freshness off `Reconciled`; `event_args` (`:232`) drops `budget`, `root` and absent optionals. `RECONCILED_PATHS = 200` caps `reparsed`/`removed`, `more` counts the rest.
- `crates/codeindex/src/index.rs` — `Codeindex.rt: Option<Runtime>` (`:43`) set by `with_runtime` (`:144`; the plugin calls it, `lib.rs:247`; tests and `outline_hook` keep `Codeindex::new`); `publish` (`:155`); `indexed(root)` (`:150`). `RootIndex` gains `suspect`/`truncated` (the last walk's verdicts, `:65-67`), `seq: AtomicU64` (`:69`; `next_seq` `:83` bumps on every event, `seq` `:88` is what a lens reply carries) and `gate` (`:70`, `Gate { last, deferred }` `:75`). `snapshot()` (`:93`) = freshness without a walk. `reconcile` records the verdicts (`:210-211`), publishes the sync write after the co-change walk (`:231`, `:291`) and the background write after `pending`/`background` are cleared (`:275`). `touch(ri, paths)` (`:371`): `under_root` per path (an escape fails the call), a known path gone from disk is a removal, `(mtime_ns, size)` unchanged is a no-op, else `write` → `CodeReconciled`. `reconciled` (`:430`) is the ≥ 250 ms gate per root: inside the gap the paths merge into one deferred trailing event; `reconciled_event` (`:618`) builds the payload (`git_head` from `meta`). `Diff.gone` is now the paths; `write` returns the paths written.
- `crates/codeindex/src/walk.rs:76-101` — `walk::entry(root, rel, max_bytes)` (one file as the walk would list it) and `entry_of`, shared with the walk loop.
- `crates/codeindex/src/tools.rs` — `ran!` (`:188`): the shared shape of every `Tool::call` = `open` + the tool's `run(a, ri, r)` + one `CodeToolRan` via `tool_ran` (`:201`), errors included; the six former `call` bodies are `run` methods (`CodeImpactTool::run` is `pub(crate)` for the Change lens, `:1646`). A root that cannot be opened publishes nothing (no root to key by). `head_of` (`:157`) and `cache_state` (`:168`) extracted from `code_map` for the lenses; `pick`/`Picked` `pub(crate)` (`:493-501`); `symbols_changed` rows gain `moniker` (`:1794`, additive).
- `crates/rpc/src/bridge.rs:13,121,168` (`forward!` + `subscribe`), `crates/rpc/src/client.rs:282` (`republish!`), `crates/rpc/src/export.rs:119-130` (event + lens bindings).

Engine — lenses (E2)
- `crates/codeindex/src/lens.rs` — messages `Here`/`Fit`/`Change`/`Touch` (`:36-73`); reply types `LensMeta`, `MapRow`, `DirRow`, `HereLens`, `FitNode`, `FitEdge`, `FitLens`, `ChangeSym`, `ChangeFile`, `ChangeLens` (`:85-307`, field names exactly as the fixtures; 64-bit fields `ts(type = "number")`); `open_indexed` (`:314`): `root::discover`, the db file must exist **and** `meta.last_reconcile_at` must be set (`codeindex_stats` creates an empty db on open) else `"… is not indexed; run a code_* tool first"` → `ASK_FAILED`; `meta()` (`:327`) = `seq`, `files` count, snapshot freshness, `git_head`, `ms`.
  - `here` (`:393`): `focus`/`edited` empty → `store::ranked_symbol_rows(limit)` (stored ranks, a lookup); else `Graph::load` + `g.focus(focus, edited)` + `g.rank` (RANK's personalisation) and `symbol_rows(ids)`; badges from `store::map_facts` (`importers`, `callers`, `recent`), `entry` from `meta.entry_points`, `is_test`/`exported`/`line_end` from the row; files by best rank, symbols by line; `dirs` = depth-2 components (`rank::component`) with parsed files and no row, `ranked_rows` = dropped tags under them; body cached under `map_cache:{state}:here;n=…;f=…;e=…` (same `state` and eviction as `code_map`'s cache; `cache_hit`), never on a stale index.
  - `fit` (`:595`): `tools::pick` (moniker | name | `path:line` | id); several → `candidates`, focus = best stored rank; `store::callers_of` (refs → focus grouped per referencing definition, file node `<lang>:<path>` at module scope) and `callees_of` (refs inside the focus span grouped per target); best confidence then most refs first; `limit − 1` split evenly (callers take the odd one, a short side hands its slack over); `more_callers`/`more_callees`; `ambiguous = store::ambiguous_named(name)`; one `FitEdge` per shown neighbour with `count`, `kind`, `provenance`, best `confidence`.
  - `change` (`:695`): `CodeImpactTool::run` over the open root with `ri.snapshot()` (no reconcile), `budget: 1` (the text is unused); neither `git_range` nor `files` → `WORKTREE`; bands: changed (status, ≤ 12 `symbols` with monikers), direct (`via`, `changed: true` rows stay in band 0, test files move to the lane), transitive (depth), tests (`tests` → `test_link: "ref"`, `tests_by_subject` → `"subject"`); caps `CHANGE_CAPS = [20, 30, 30, 30]` → `more[4]`; `mode` `worktree` (range `(HEAD[..8], "WORKTREE")`) / `range` / `files`; cached under `map_cache:{state}:change;r=…;f=…;t=…`.
  - `touch` (`:871`): `Some(paths)` → `Codeindex::touch`; `None` → `reconcile(force = true)` (the **Reconcile** button).
- `crates/codeindex/src/store.rs:1337-1482` — `SymbolRow`, `ranked_symbol_rows(limit) -> (rows, total)`, `symbol_rows(ids)`, `Neighbour`, `callers_of`, `callees_of`.
- `crates/codeindex/src/lib.rs:208-224` — `lens_handler!` wires `Here → HereLens`, `Fit → FitLens`, `Change → ChangeLens`, `Touch → LensMeta` on `CodeindexActor` (`Result<_, String>` like `Stats`); re-exports.
- `crates/rpc/src/api.rs:907-940` (trait, flat names, positional params) and `:3940-4004` (impls, `send(&codeindex()?, …)`).
- `crates/codeindex/src/git.rs:245-271` — `Range::Worktree` consults the git index first (the `git status` rule): an entry holding HEAD's blob whose stat matches the file is unchanged — no read, no hash; racy entries (`Stat::is_racy` vs the index timestamp) fall through to the hash. Same result as before, 17× faster (Numbers).

UI wiring
- `crates/desktop/ui/src/lib/codemap/types.ts` — wire shapes re-exported from `@/rpc/bindings/*` (`CodeToolRan`/`CodeReconciled` = `Extract<CodeindexEvent, …>`); only `CodeTool`, `CodemapApi`, `CodemapSlice`, `TrailRow` stay hand-typed.
- `crates/desktop/ui/src/rpc/events.ts:36,92` `WireEvent |= CodeindexEvent`; `crates/desktop/ui/src/rpc/api.ts:45-48,237-244,758-761` the four methods on `Api` + `METHODS` (`api.test.ts` parity against `bindings/methods.json` passes).
- `crates/desktop/ui/src/lib/codemap/api.ts:17` `disconnectedCodemap` (rejects until connected); `rpcCodemap` deleted — `Api` is a structural `CodemapApi`. `CodeMapPanel.tsx:48-51` `useCodemapApi` = mock only under `VITE_CODEMAP_MOCK=1`, else the connection's `Api`.
- `crates/desktop/ui/src/dev/codemap-fixture.ts` kept behind the same flag (the mock adapter without its event replay shows nothing); header comment updated.
- `scripts/codemap-smoke.ts` — the headless smoke client (below).

## Numbers (§7; debug build, this repository — 1 760 indexed files, 2 184 ranked definitions; headless `t`, bun client over WebSocket on loopback)

| Path | Budget | Measured | Verdict |
|---|---|---|---|
| `codemap_here` warm (2 000 rows, cache hit) | < 30 ms | engine `meta.ms` **13 ms** (min 13 · max 14, 19 calls); wall **44 ms** median (43.3–45.7) | engine meets; wall misses by ~14 ms: the reply is ~0.5 MB of JSON for 2 000 rows (serialize + WS + client `JSON.parse`); a smaller `limit` is the panel's knob |
| `codemap_here` miss (unpersonalised) | < 80 ms | **176 ms** (wall 209) | misses by ~100 ms: `map_facts` (two `GROUP BY`s over `refs`) + `ranked_symbol_rows` twice (cut + the tail for `dirs.ranked_rows`) + a 0.5 MB cache `INSERT`; once per index state, then hits |
| `codemap_here` personalised miss (`focus=[crates/codeindex]`, `edited=[lens.rs]`) | — (§9 gives personalised rank up) | **462 ms** (wall 492); warm hit 43 ms wall | `Graph::load` + PageRank in a debug build (60 §22.1 baseline 101 + 43 ms release-ish); cached per `(state, focus, edited)` |
| `codemap_fit` (`SessionId::new`, 283 callers → 19 shown + `more_callers 264`) | < 30 ms | engine **7 ms** median (6–8), wall **8.6 ms** (7.0–10.1), 20 calls | meets |
| `codemap_change` WORKTREE cold | < 300 ms | **206 ms** (wall 206); before the git-index shortcut **3 470 ms** | meets (was 11.6× over: hashing every tracked blob in debug SHA-1) |
| `codemap_change` WORKTREE cached | — | **2–4 ms** | lookup |
| `codemap_touch` unchanged file | < 50 ms/file | **25 ms** (stat + `known_files`) | meets |
| `codemap_touch` new/changed file | < 50 ms/file | **709 ms** for one `.ts` file (`parse_ms=148` incl. `Workspace::load`, `resolve_ms=218`, `subjects_ms=15`, `rank_ms=324`); removal 508 ms | **misses by ~660 ms**: the write path re-persists the whole rank and reloads the workspace on every write (60 §7 keeps `code_find`'s order a lookup). The `ponytail:` in `Codeindex::touch` names the upgrade (defer `rank::persist` to the next tool call in a burst) |
| `CodeReconciled` gate | ≥ 250 ms apart per root | `reconciled_is_gated_250ms`: two touches 1 ms apart → one event at once, one trailing ≥ 240 ms later carrying the second path | meets |
| `CodeToolRan` | one per call | smoke: `seq 2` right after the model's `code_map` (`seq 1` was the first index's `CodeReconciled`) | meets |

Rank persist on this repo in the debug build: 323–329 ms (matches 60 §22.1 / WAVE2's 306–323 ms).

## Tests written (`crates/codeindex/tests/codemap.rs`, scratch copy of the rust fixture, a real `Runtime` bus + a `Sink` actor subscribed to `CodeindexEvent`)

- `events_are_published_once_per_call` — a `code_find` and an erroring `code_refs` → exactly two `CodeToolRan` (tool, call_id, `rows 1`/`rows 0`, `is_error`, monotonic `seq`, `args` minus `root`/`budget`/`kind: null`), no stray `CodeReconciled`.
- `reconciled_is_gated_250ms` — two touches inside one window → one event immediately, the trailing one ≥ 240 ms later with the second path and a higher `seq`; an unchanged touch and an escaping path (`Err`) publish nothing.
- `here_is_a_cached_store_lookup_with_badges` — rows > 3, files-by-rank/symbols-by-line order, `sub::check` carries `importers`/`callers`, moniker grammar, `line_end`; second call `cache_hit`; `limit 2` → `dropped` and a coverage dir with `ranked_rows > 0`; personalised has its own key (miss then hit).
- `fit_caps_at_limit_and_counts_more` — `Registry::new` by `path:line`: ≥ 2 callers, edges = nodes, all touch the focus; `limit 2` → one node and `more_callers` accounts for the rest, best-first order preserved; the moniker re-finds the same focus; `"new"` → `candidates ≥ 2` containing the focus; unknown symbol → `Err`.
- `touch_reparses_only_named_paths` — two files edited, one touched: only its `mtime` moves in `files`, only its new definition appears, one `CodeReconciled` with exactly that path; `paths: None` (forced reconcile) picks up the other.
- `lens_rpcs_never_walk` — a new file + an edit on disk, then here/fit/change(files): `known_files` unchanged, the new file unknown, no event; an unindexed root is refused with `not indexed`.
- `change_worktree_is_the_default_and_cached` — `git init` + commit, one file touched: `mode worktree`, `range (HEAD[..8], "WORKTREE")`, one changed file with its symbol, `more = [0,0,0,0]`, second call `cache_hit`, `include_tests` changes the key.

Gate: `cargo test -p codeindex` 40 unit + 7 codemap + 23 `tests/index.rs` + 1 locate; `cargo test -p rpc` 34 + 1; `cargo clippy -p codeindex -p rpc --all-targets -- -D warnings` clean; `cargo check --workspace` clean (pre-existing `block v0.1.6` future-incompat note). UI: `bun run check` clean, `bun run test` alone 110 files / 627 tests, `bun run build` ok; `just ts-desktop` regenerated `bindings/` (gitignored) and `methods.json` with the four methods.

## End-to-end smoke (trimmed)

`rm -rf /tmp/codemap-smoke; config.yaml = {secrets.key_store: file, rpc.listen: 127.0.0.1:7863}`; `THEUTH_RPC__LISTEN=127.0.0.1:7863 target/debug/t --serve --data-dir /tmp/codemap-smoke` (cwd = this worktree, `ANTHROPIC_API_KEY` from the environment); then `bun scripts/codemap-smoke.ts --url ws://127.0.0.1:7863 --data-dir /tmp/codemap-smoke --root $PWD`:

```
session 01a0e86a-…; waiting for CodeToolRan…
CodeToolRan after 14.3 s:
{"type":"CodeToolRan","session":"01a0e86a-…","root":"/Users/Sky.Moore/Develop/theuth-codemap-engine","call_id":"toolu_01Y1A6RDgdpsYTw9ym67Skqt.1","tool":"code_map","args":{},"summary":{"rows":22,"dropped":2162,"is_error":false,"stale":false,"pending":0,"truncated_walk":false,"suspect_root":false,"hint":null},"seq":2}
CodeReconciled from the first index: {"type":"CodeReconciled","root":"…","reparsed":"[200]","removed":[],"more":1560,"stale":false,"pending":0,"suspect_root":false,"hint":null,"git_head":"d8de13ee5b17…","seq":1}
codemap_here ×20: rows 2000 dropped 184 dirs 3 cache_hit(last) true
  first call wall 209.0 ms (engine 176 ms); warm median 44.1 ms · min 43.3 · max 45.7; engine ms warm median 13.0 ms · min 13.0 · max 14.0
codemap_here personalised ×5: rows 2000 personalised true; cold wall 491.8 ms (engine 462); warm median 43.0 ms
codemap_fit(rust:crates/runtime/src/lib.rs#SessionId.new(method)) ×20: focus new callers 19 (+264) callees 0 (+0) edges 19 ambiguous 982
  wall median 8.6 ms · min 7.0 · max 10.1; engine ms median 7.0 ms · min 6.0 · max 8.0
codemap_fit("new") by name: focus new, candidates 151
codemap_change(WORKTREE): mode worktree range ["d8de13ee","WORKTREE"] bands 4/6/8/7 more [0,0,0,0] unindexed 0
  cold wall 205.9 ms (engine 205 ms); cached wall 2.8 ms (engine 2 ms, cache_hit true)
codemap_touch([crates/codeindex/src/lens.rs], unchanged) wall 26.1 ms engine 25 ms seq 2
codemap_touch([scripts/codemap-smoke-probe.ts], new file) wall 735.7 ms engine 735 ms seq 3
CodeReconciled after touch: {"type":"CodeReconciled","root":"…","reparsed":["scripts/codemap-smoke-probe.ts"],"removed":[],…,"git_head":"d8de13ee5b17…","seq":3}
codemap_fit(codemapProbe) sees the new definition: typescript:scripts/codemap-smoke-probe.ts#codemapProbe(function)
CodeReconciled after delete: {"type":"CodeReconciled","root":"…","reparsed":[],"removed":["scripts/codemap-smoke-probe.ts"],…,"seq":4}
touch outside root rejected: …/../outside.rs does not exist: No such file or directory (os error 2) (code -32000)
here on an unindexed root rejected: /private/tmp is not indexed; run a code_* tool first (code -32000)
```

Engine log for the first index and the touch: `codeindex reconciled … reparsed=1760 ms=8710`; `codeindex write phases parse_ms=148 apply_ms=0 resolve_ms=218 subjects_ms=15 rank_ms=324 resolved_files=1`; `codeindex touched … reparsed=1 ms=709`. The `args: {}` on the smoke's `CodeToolRan` is correct: the model passed only `root` and `budget`, both stripped by §5.1.

## Deviations

1. **`CodeToolRan` is not published when the root cannot be opened** (`root::discover` fails, e.g. a nonexistent `root=`): there is no root to key the event by and the UI slice keys everything by root. Every other error (empty name, ambiguous symbol, `code_impact` with both args, no such definition) publishes with `is_error: true, rows: 0`.
2. **`summary.rows` per tool** is "the main list the model got": `rows` (map/find), `symbols` (outline, top level), `refs + candidates` (refs), `callers + callees` (symbol), `changed + direct + transitive + tests` (impact). `summary.focus` is `args.focus` as given (the fixture's `args.focus_files` is fixture data; the real `code_map` arg is `focus`).
3. **`codemap_here(focus, edited)` computes RANK's personalised rank on a miss** (`Graph::load` + `rank`, ~460 ms debug) and caches the body under the `map_cache:` namespace with `edited` in the key — the brief asked to reuse the rendered-map cache and RANK's personalisation; the agent's *rendered* cache entries are keyed on its `budget` and hold ≤ ~120 rows, useless for a 2 000-row lens, so the lens has its own entries in the same namespace (evicted together on any index state change). Unpersonalised = stored ranks, a lookup; `personalised` says which.
4. **`codemap_change` band caps are the popped-out sizes** (`20/30/30/30`, `more[]` counts beyond them); §5.2 wanted 60 in the panel / 110 popped but the method has no `limit` parameter (fixtures/`CodemapApi` have four positional params, and they win). The UI already cuts to `12/16/16/16` with its own `+N in <dir>` bundles.
5. **Test files leave the direct/transitive bands for the tests lane** (`ref` = `code_impact.tests`, `subject` = `tests_by_subject` not already by ref); a changed file that references another changed file (`direct[].changed = true`) stays in band 0 only. Bands are disjoint layers, as §6.4 draws them.
6. **WORKTREE `range` = `(HEAD[..8], "WORKTREE")`** (the fixture's `["22b39455", "WORKTREE"]`); `code_impact` itself still reports `{"old": "HEAD", "new": "WORKTREE"}`.
7. **Lens replies carry the current `seq` without bumping it** — a reply reflects the index *as of* that seq; events bump. Two replies may share a seq; the UI drops only *older* ones.
8. **`open_indexed` requires `meta.last_reconcile_at`**, not just the database file: `codeindex_stats(root)` creates an empty db on `open_root`, which would have let a panel index a stranger's tree through `codemap_touch(root, null)` (§8 threat). Engine `ASK_FAILED` text: `"<root> is not indexed; run a code_* tool first"`.
9. **`codemap_touch` misses its 50 ms/file budget (709 ms)**: every write re-persists the rank (324 ms) and reloads the resolver workspace (inside `parse_ms`); the deferral of `rank::persist` in a burst (§7) is not built — `ponytail:` comment in `Codeindex::touch` names it. The unchanged-file path (the common double-touch from two windows) is 25 ms.
10. **`codemap_here` miss is 176 ms (budget 80)** and the warm *wall* is 44 ms for 2 000 rows (engine 13 ms). Not optimised: the miss is once per index state; the wall is JSON volume, which `limit` controls.
11. **Worktree diff now uses the git index's stat cache** (`git.rs`, the `git status` rule) — a change to `code_impact(git_range="WORKTREE")` and `code_map(mode="changed")` too, same results (`git::tests::ranges_hunks_worktree_and_cochange`, `impact_on_a_fixture_repository` pass); racy entries still hash. Without it the Change lens was 3.5 s cold on every index-state change.
12. **`CodeReconciled` is not published for the `suspect_root` refusal** (nothing was written); the verdict reaches the UI through `LensMeta.suspect_root/hint` (stored on `RootIndex.suspect`, read by `snapshot()`) and the next tool call's `CodeToolRan.summary`.
13. **`CodeToolRan.tool` / `.args` types are narrowed with `ts(type)`** (`"code_map" | …`, `Record<string, unknown>`) instead of widening the UI to `string`/`JsonValue`: the engine really produces one of six names and an object.
14. **`symbols_changed[]` gained `moniker`** in `code_impact`'s JSON (additive) so `ChangeSym.moniker` is the row's own, not a positional guess against `monikers[]`.
15. **`dev/codemap-fixture.ts` kept** behind `VITE_CODEMAP_MOCK=1` (the mock adapter and its event replay are one dev mode); `rpcCodemap` deleted, `disconnectedCodemap` added for the no-connection window.
16. **E1's tests live in the E2 commit** (`tests/codemap.rs` covers both; E1's commit builds and runs on its own, without them).
17. `RootIndex.last_reconciled_at` (§4) is the `Gate.last: Instant` inside a `tokio::sync::Mutex`, not an atomic ms stamp: the deferred paths need the same lock.

## Not built

- `rank::persist` deferral for touch bursts (§7); the workspace reload per write.
- `codemap_here` miss below 80 ms (badge columns written at `apply` time, §7's fallback).
- A `limit` parameter on `codemap_change` (110 popped) — the UI bundles client-side.
- Browser-level smoke of the panel against this engine (the UI as-built's mock smoke stands; the engine side is proven over the WebSocket above). Not run: the UI's "Reconcile" button path (`codemap_touch(root, null)`) end-to-end — `lens::touch` with `None` is covered by `touch_reparses_only_named_paths`.
