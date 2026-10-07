# 12 as-built — `glob` / `grep` parallel walk (design 12 §10)

Branch `feat/glob-parallel-walk`. Files: `crates/tools/src/glob.rs`,
`crates/tools/src/fs.rs` (`fn grep` only). No new dependencies, no index, no cache.

## What changed

| Decision | Where |
|---|---|
| D1 parallel walker (`build_parallel`, `threads(walk_threads())`, `Mutex<Vec>` + atomics) | glob.rs:91 `walk_threads`, glob.rs:164; fs.rs:642 (one `Searcher` per thread, fs.rs:643; `RegexMatcher` shared by `&`) |
| D2 cap / early stop | glob.rs:188 keeps at most `limit + 1` hits, `Quit` after; fs.rs:650–683 `AtomicUsize` line count, sink returns `false` and worker returns `Quit` at `limit`; fs.rs:710 sort by (path components, line); fs.rs:717 `[… limit N reached]` |
| D3 ancestor-ignore retry | glob.rs:100 `hidden_by_ancestor` (shared), glob.rs:209 retry with `parents(false)`, header + `structured.ancestor_ignore_bypassed` in `render` glob.rs:231–252; fs.rs:694–701 |
| D4 anchored pruning (glob only) | glob.rs:107 `anchored_prefix`, glob.rs:153 `filter_entry`; `WalkStats.visited` (glob.rs:80) feeds the test |
| D5 descriptions | glob.rs:47, fs.rs:589 (one sentence appended each) |
| D7 bench harness | glob.rs `tests::bench_walk` (`#[ignore]`) |
| D8 tests | glob.rs `cap_holds_under_parallel_walk`, `anchored_pattern_prunes_but_matches_the_same`, `ancestor_gitignore_hiding_the_root_is_bypassed_once`; fs.rs `grep_sorts_by_path_then_line_and_marks_the_cap`, `grep_bypasses_an_ancestor_gitignore_that_hides_the_root` |

## Before / after (2026-10-07, load average 30–50, three runs each, ms)

Threads: `available_parallelism().min(8)` = 8 on this machine. Not tuned further
(D1 allowed 4/16 if clearly better; not measured — the brief's own figure of 0.12 s
parallel vs 1.7 s serial cold was already met with 8).

| Case | Before (serial) | After (parallel) | Cache |
|---|---|---|---|
| (a) theuth, `*.zzz_none` (full walk, 2 678 entries) | 162 / 44 / 44 | 18 / 11 / 12 (re-run: 20 / 12 / 12) | run 1 of BEFORE partly cold; everything else warm (other sessions walk this tree constantly) |
| (b) theuth, `*.rs` (capped at 500) | 46 / 34 / 32 | 10 / 8 / 9 | warm |
| (c) ~/Develop, `*.zzz_none` | 32 / 4 / 3 — **0 entries walked**: `~/.gitignore` is `*`, so the ancestor rule pruned the root (the bug) | 4 640 / 2 501 / 2 479 (re-run: 2 750 / 2 424 / 2 650); 777 339 entries visited, `ancestor_bypass=true`; each run is two walks (the empty first one + the retry) | run 1 cooler; no page-cache eviction was possible on the shared machine, so no true cold number |

Measured 02:08–02:16 WEST, BEFORE and AFTER seven minutes apart.

## Semantics that changed

- **Ancestor-ignore bypass.** When the first walk yields nothing below a non-empty
  directory root, both tools walk once more with `parents(false)`. If that yields
  entries, output starts with `[glob: an ancestor .gitignore hides {root}; walked
  without ancestor ignore rules]` / `[grep: …]`, and glob sets
  `structured.ancestor_ignore_bypassed = true` (key absent otherwise). The root's
  own `.gitignore` is not an ancestor rule: a root that hides itself still returns
  the plain no-match result. Caveat: with `parents(false)` the walker also no longer
  sees a `.git` in an ancestor, so a `.gitignore` *inside* the root is only honoured
  when the root or a subdirectory has its own `.git` (sub-repos under `~/Develop` do).
- **Which matches survive a cap is nondeterministic** between runs (threads race);
  the truncation header / `[… limit N reached]` line already tells the model the
  result is partial. The set is deterministic when under the cap.
- **grep sorts after the walk** by path components then line number (same order the
  serial `sort_by_file_path` produced); `[… limit N reached]` is appended after sorting.
- glob with an anchored pattern never enters directories outside the literal prefix;
  results are identical to the unpruned walk.

## Follow-ups

- `crates/codeindex/src/walk.rs` still uses the serial walker (out of scope here).
- Optional index-free dir-mtime snapshot to skip unchanged subtrees between calls.
- `std::sync::Mutex` is used (brief D1); `parking_lot` is not a `tools` dependency.

## Re-running the benchmark

```sh
export PATH="$(dirname "$(rustup which cargo)"):$PATH" CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 BUILD_LABEL=glob-fast
GLOB_BENCH_ROOT=/Users/Sky.Moore/Develop/theuth [GLOB_BENCH_PATTERN='*.rs'] \
  build-lock cargo test -p tools --lib -- glob::tests::bench_walk --ignored --nocapture
```

Prints per run: wall ms, files found, `truncated`, `visited` entries, `ancestor_bypass`.
