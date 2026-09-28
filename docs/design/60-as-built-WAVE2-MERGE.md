# 60 — as-built WAVE2-MERGE: `feature/codeindex-rank` into `feature/codeindex-wave2` (2026-09-28)

Worktree `~/Develop/theuth-ci-wave2`, branch `feature/codeindex-wave2` (SEARCH + IMPACT + RENDER at
`514050e6`) ← `origin/feature/codeindex-rank` (`2e2af9dc`). Brief `/tmp/enh-2026-09-28/omp-wave2-merge.md`.
Merge commit `853b4cb6`, subject
`merge: feature/codeindex-rank into codeindex-wave2 (code_map keeps mode/cache/badges + edited/entry seeds)`.
Companion notes: `60-as-built-CI-{SEARCH,IMPACT,RENDER,RANK}.md`.

## Conflicts and resolutions

| File | Hunk | Resolution |
|---|---|---|
| `src/rank.rs` | test helper `kind_sym` (`rank.rs:~948`): HEAD `signature: "fn {name}()"` + `moniker`; RANK `signature: "{kind} {name}"` | RANK's signature (it renders the `kind` the helper is parameterised on) + HEAD's `moniker`. No test asserts the signature text. Everything else — RANK's `ImportRow`/re-export passthrough/`Shape` damping/`Focus.seeds`/`focus(entries, edited)`/`entry_paths` and RENDER's `MapFacts`/`Rendered`/`component`/`render(tags, budget, facts)`/coverage — merged clean. |
| `src/walk.rs` | HEAD `is_test_dir`/`is_test_data`/`subject_candidates` vs RANK `entry_points`/`json_strings` | Both, in that order (`walk.rs:114–361`). RANK's `is_test_path` had the dir match inline; HEAD's `is_test_dir` form is the common base now. |
| `src/walk.rs` | test bodies `subject_candidates_by_convention` vs `entry_points_from_manifests` shared one `#[test]` header | Both tests, header re-inserted between them. |
| `src/tools.rs` | `code_map` description | One string naming both: passthrough/damping, `edited`, `entry?`, badges, `cached`, `coverage_added`, `mode`. |
| `src/tools.rs` | `call` body: HEAD `mode`/HEAD-lookup/cache key/`changed` dispatch/`full(...)` head vs RANK `personalised`/`focus(&focus,&edited)`/`entries` from `meta.entry_points` | HEAD's `call` (`tools.rs:700`) kept whole; cache key gains `;e={edited sorted, 0x1f-joined}` after `f=` (`tools.rs:778`). `full` (`tools.rs:851`) takes `edited`, uses RANK's `personalised` gate and `g.focus(focus, edited)`, reads `entries`. RANK's second `let mut extra` dropped (`extra` is the caller's map). `full` lost its unused `&self` (clippy `too_many_arguments` at 8) → `Self::full(...)`. |
| `src/tools.rs` | row construction: HEAD `badge_fields` + `rank::render(.., facts)`/`Rendered.kept` vs RANK `entry: true` + `rank::render(tags, budget) -> (text, kept)` | RENDER's render path; `entry` flag added after `badge_fields` on both `paths_only` rows (`tools.rs:~895`) and tag rows (`tools.rs:~918`). |
| `tests/index.rs` | `map` closure literal: `mode: None` vs `edited: None` | Both. Two non-conflicting literals (`index.rs:1212`, `2574`) lacked `edited`; added. |

Also done (brief rule 2, IMPACT's follow-up): `code_map` now emits `suspect_root`/`hint` in the envelope
and ends `content` with `stale_note(&r)` like the other tools (`tools.rs:~812–819`).

Formatting: HEAD carried 12 pre-existing `rustfmt` diffs in `rank.rs`/`tools.rs` from the earlier wave
merges; `cargo fmt -p codeindex` applied in the merge commit so the fmt gate is green.

## Combined `code_map` shape (what design 73's engine worker reads)

| Part | Field | Type | Source | Notes |
|---|---|---|---|---|
| arg | `focus` | `[string]?` | SEARCH | identifiers and/or root-relative paths; personalises |
| arg | `edited` | `[path]?` | RANK | root-relative or absolute under root; weighted above a focus path, co-change partners lightly; in cache key sorted |
| arg | `budget` | `u32?` | base | estimated tokens (chars/3), default 4000 |
| arg | `paths_only` | `bool?` | base | ranked files instead of tags; error with `mode="changed"` |
| arg | `mode` | `"full"\|"changed"?` | RENDER | `changed` ignores `focus`/`edited` for content, keeps them in the cache key |
| arg | `root` | `path?` | base | default git top-level of cwd |
| row (tags) | `path, line, kind, name, signature, moniker, rank` | | base/SEARCH | `Tag` serialised (`rank.rs:158`) |
| row (tags) | `importers` | `i64` | RENDER | distinct importing/referencing files |
| row (tags) | `recent` | `bool` | RENDER | `mtime_ns ≥ now − 48 h` |
| row (tags) | `callers` | `i64` | RENDER | cross-file resolved refs to the definition |
| row (tags) | `entry` | `true`, absent otherwise | RANK | path ∈ `meta.entry_points` |
| row (paths_only) | `path, rank, lines, language, importers, recent, callers: 0, entry?` | | RENDER+RANK | text line `path (→N) [NEW]  rank  lines  language` |
| row (changed) | outline rows + `importers/recent/callers` | | RENDER | unchanged by this merge; no `entry` flag (see Deviations) |
| envelope | `root, indexed_files, stale, pending, dropped, truncated_walk` | | base | `Envelope::new` |
| envelope | `suspect_root: bool`, `hint: string?` | | IMPACT | new in `code_map` here |
| envelope | `estimated_tokens` | `u64` | base | |
| envelope | `mode` | `string` | RENDER | echoed |
| envelope | `cached` | `bool` | RENDER | `true` when served from the rendered-map cache |
| envelope | `coverage_added` | `usize` | RENDER | `mode="full"`, tags path only |
| envelope | `focus_files, focus_idents, pagerank_iterations?, entry_points` | | SEARCH/RANK | only on a personalised or `paths_only` run (graph loaded); `entry_points` = `Graph::entry_paths()` |
| envelope | `rows`, `content` | | base | `content` trailer `stale_note(&r)` |
| cache key | `{SCHEMA_VERSION}/{files}/{max mtime}/{HEAD\|-}/{hour}:b={budget};p={paths_only};m={mode};f={focus↑};e={edited↑}` | | RENDER+merge | `↑` = sorted, `0x1f`-joined |

## `code_map()` on this repository (budget 1500, fresh worktree so every file is `[NEW]`)

1472 estimated tokens, 43 rows, 2095 dropped. Top-10 files with badges (from `map_and_symbol`):

```
crates/desktop/ui/src/store/engine-scope.tsx (→15) [NEW]        95: scoped<T>  ↑6
crates/desktop/ui/src/rpc/api.ts (→27) [NEW]                   190: interface Api  ↑26
crates/runtime/src/lib.rs (→158) [NEW]                          37: new  ↑65 · 93: write_config_value  ↑28 · 205: subscriber_mailbox  ↑39 · 228: new  ↑374 · 260: new  ↑71 · 286: new  ↑19
crates/desktop/ui/src/components/sidebar/SessionList.test.ts (→27) [NEW]   29: keys  ↑44
crates/mesh/src/types.rs (→25) [NEW]                            26: struct EngineId  ↑54
crates/tools/src/resources.rs (→28) [NEW]                       55: durable_arc  ↑27
crates/desktop/ui/src/store/reduce.ts (→41) [NEW]               66: reconcileTodos  ↑2 · 161: recordCall  ↑1 · 967: now  ↑75
crates/desktop/ui/src/rpc/client.ts (→24) [NEW]                 51: class RpcClient  ↑20
crates/tools/src/registry.rs (→46) [NEW]                       249: struct RegisterTool  ↑93
crates/desktop/ui/src/rpc/transport.ts (→3) [NEW]               23: class LocalTransport  ↑1
```

`paths_only` top 5: `runtime/src/lib.rs 0.01831`, `store/reduce.ts 0.01288`, `store/engine-scope.tsx 0.01186`,
`tools/src/registry.rs 0.01099`, `rpc/api.ts 0.00961`. `entry_points` lists 24 crate roots
(`crates/runtime/src/lib.rs`, `crates/codeindex/src/lib.rs`, …). `focus=[ScrapeUrlTool]` top 5:
`scrape/src/tools.rs`, `tools/src/registry.rs`, `scrape/src/crawl.rs`, `firecrawl-native/src/crawler.rs`,
`firecrawl-native/src/lib.rs`; 19 iterations.

## Gate

| Check | Result |
|---|---|
| `cargo test -p codeindex` | unit 40 passed (1.23 s); integration `tests/index.rs` 23 passed (40.07 s); 1 + 0 doctests |
| `cargo clippy -p codeindex --all-targets -- -D warnings` | clean |
| `cargo fmt -p codeindex --check` | clean (after `cargo fmt -p codeindex`) |
| `cargo check --workspace` | clean, 9 m 14 s cold (`block v0.1.6` future-incompat warning, pre-existing) |
| `rank::persist` (1070 nodes) | cold 50 iterations 323 ms; warm 8 iterations 306 ms |
| `Graph::load` / personalised rank | 211 ms / 74 ms, 20 iterations |

## Deviations

- `mode="changed"` rows carry no `entry` flag: RANK never rendered that mode and RENDER had no
  entry concept. Adding it is one `entries.contains` per block; do it when the engine worker wants it.
- `entry_points` in the envelope only appears on a personalised or `paths_only` run (RANK's
  behaviour, kept); the cached default map has `entry: true` rows but no `entry_points` list.
- `full` is an associated fn (`Self::full`) rather than a method: clippy's 7-argument cap after `edited`.
