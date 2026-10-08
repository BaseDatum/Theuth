# 140 as-built — W1 MEASURE (runner, scanner, gate, custody, bench slot, toy-sort fixture)

Worker 140-W1, branch `feat/140-w1` from C's skeleton 4aa41bdc. Owns
`crates/autoresearch/src/{runner,scanner,gate,custody,slot}.rs` and
`crates/autoresearch/fixtures/toy-sort/**`. Every public signature C wrote is unchanged;
additions are listed under hand-offs.

## What was built, file by file

- **`scanner.rs`** (§5.1 grammar). `Scanner::feed` splits chunks on `\n`, keeps only the
  pending partial line (a partial line past 1 MiB is dropped and poisoned so its tail can
  never parse as a contract line). `scan_line`: `METRIC name=number` (last wins; `nan`,
  `inf`, or any non-`f64` text → `bad_number`), `CASE id name=number` keyed by id, `ASI
  key=text` with the 2 KiB total cap enforced per sample (over-long text truncated at a char
  boundary, further keys dropped), CRLF stripped. Tests: grammar/last-wins/CRLF, NaN/inf,
  chunk-split lines, ASI cap, 200 MB stream with bounded pending, non-contract lines.
- **`gate.rs`** (§5.2). `pooled_sigma`: classical pooled within-tree sd; once ≥ 12 repeated
  samples (over trees with n ≥ 2) the robust 1.4826 × pooled MAD (deviations from each tree's
  median, one median over all). `step`: σ = 0 path (`Δ > min_effect` keeps, `Δ < 0` worse,
  else no gain, no confirm); one-sample path (`z₁ < −z` worse, `< z_discard` no gain,
  `≥ z_fast ∧ Δ > min_effect` keep, else `Confirm{confirm_runs, incumbent_sample: age ≥
  remeasure_after}`); means path (`Δ > T = max(z·σ·√(1/n_c+1/n_i), min_effect)` keeps);
  suspicious (`z ≥ z_suspicious` or `Δ > 3×` largest accepted) always confirms first and
  decides `Suspicious`; guards (on means) turn a keep into `GuardFailed`; `keep_simpler`
  asked in the run forces the confirm so means exist. `guard_failures` renders
  `alloc_mb +7.20 % > 5.00 %`. `keep_simpler`: `removed > added`, guards hold, `z > −1`
  on means (σ = 0: `Δ ≥ 0`). `drifted`: `|x − x̄_i| > 3σ`. `line`: the §5.2 item 5 texts.
  Tests: seeded LCG + Box-Muller simulation (100 studies × 100 null experiments, σ pooled as
  the study grows, incumbent re-measures honoured) — false-keep ≤ 1 %; 40 × 50 experiments
  each a true 3σ improvement — keep ≥ 85 %; unit tests for every path.
- **`custody.rs`** (§5.3 custody half). `take`: for each of bench/checks/holdout command,
  every whitespace token that is a relative path and exists in the repo (worktree at base) or
  in `<root>/drafts/<slug>/` is copied into `evaluator/<segment>/` under the same relative
  name; every `evaluator_paths` entry (file or directory, recursive) likewise; the three command
  texts are frozen in `commands.json`. Map keys: repo-relative path for repo files,
  `custody/<name>` for custody-only files (draft scripts, `commands.json`). `verify` hashes the
  worktree's copy of every repo key (missing or different → listed) and skips `custody/` keys.
  Tests: copy + verify + edit/remove
  detection, non-path command frozen only, missing/`..` evaluator path refused, new segment
  re-copies.
  **R1 (orchestrator 12:01): `scope_diff` is W1's.** `git status --porcelain=v2 --untracked-files=all -z`
  (ordinary, renamed pairs, unmerged, untracked) + `git diff --numstat -w --ignore-blank-lines HEAD`
  (HEAD = incumbent; whitespace-only hunks not counted; untracked files add their non-blank lines);
  `deviations` = outside `scope` globs (empty scope → none), `off_limits` = `off_limits` globs **or**
  under an `evaluator_paths` entry (ScopeDiff has no evaluator field; both make a keep impossible).
  Globs via `globset` (literal separator; `dir/` and plain paths also match their subtree). No patch
  hash / file listing (W2's `git::patch_hash`, `git::files_of`). Test: temp git repo with in-scope,
  untracked, off-limits and evaluator edits, line counts, empty scope, staged rename.
- **`slot.rs`** (§5.5). Builtin lock: `mkdir /tmp/theuth-bench-slots/0`, owner file `pid label
  epoch bench`, dead-pid reclaim (build-lock's rule: only when an owner pid exists and is
  dead), 5–10 s polling, cancel → `Refused`, released on `Drop`. Wrapper lock: `acquire`
  returns a no-op slot; the runner prefixes the command. `load` = `platform::power::load1() /
  available_parallelism`. `wait_for_load` polls every 15 s up to the budget. The per-experiment
  total of `bench.wait` lives in a process-wide `(slug, n, waited)` ledger
  (`wait_budget`/`charge_wait`) because the runner has no experiment object. `build_slot_holders`
  reads `/tmp/theuth-build-slots/*/owner`.
- **`runner.rs`** (§5.1, §5.5). `sample`: slot (when `exclusive`) → load wait (charged against
  the experiment's budget; miss → `noisy`) → `/bin/bash -lc <cmd>` with cwd = worktree, own
  process group (`process_group(0)`), stdin null, `AUTORESEARCH*` env (+ `GIT_TERMINAL_PROMPT=0`),
  any command token naming a file in custody replaced by the custody copy's absolute path;
  wrapper mode runs `<wrapper> /bin/bash -lc '<cmd>'` with `BUILD_SLOTS=1 BUILD_LABEL=bench:<slug>`.
  stdout+stderr pumped in 64 KiB chunks into `runs/<n>/sample-<k>.log` and the scanner (nothing
  held whole). Timeout or cancel: `SIGTERM` the group, `SIGKILL` 2 s later, exit `None`. A pipe
  held open after the leader exits is drained 2 s then the group is killed. Exit 0 + primary
  present + no bad number = `value: Some`; else a crash sample. `command`: same spawn for
  checks/smoke/warm (no slot), log `runs/<n>/command-<k>.log`, returns `(exit, tail, wall_ms)`.
  `tail`: last 4 KiB, cut fragment dropped, last 30 lines. `parse_duration`: humantime.
  Tests spawn real processes: metrics + env + cwd, crash variants, timeout kills a background
  grandchild, cancel, custody copy wins over the worktree's script, checks tail, tail bounds.
- **`fixtures/toy-sort/`**: `sorter.py` (insertion sort), `main.py`, `bench.sh` (n = 4000,
  8000, 12000 seeded; `CASE`, `METRIC time_ms`, `METRIC peak_kb`, one `ASI`), `test.sh`
  (`main.py < fixtures/input.txt` vs `fixtures/expected.txt` — the bait), `holdout.sh` (other
  sizes/seeds, verifies output itself), `fixtures/{input,expected}.txt`, `README.md` with the
  study spec the smoke uses.

## Gates

All through `BUILD_LABEL=140-W1 build-lock`, stable rustc 1.96.1, `CARGO_BUILD_JOBS=4`.

- `cargo test -p autoresearch`: **44 passed, 0 failed** (W1: 6 scanner, 11 gate, 5 custody,
  5 slot, 8 runner; C's 9 config/types/store/stub tests still green).
- Simulation table (seeded LCG + Box-Muller, defaults z 2.0 / z_fast 3.5 / z_discard 1.0 /
  confirm 2 / remeasure_after 6, σ = 1 % of the metric, `min_effect` 0.5 %):

  | variant | null false-keep (10,000 exp.) | true 3σ keep (2,000 exp.) |
  |---|---|---|
  | §5.2 verbatim (incumbent re-measured only by age) | 0.92 % (92) | 82.4 % (1648) — fails ≥ 85 % |
  | as built (+ re-measure a single-sample incumbent at confirm) | **0.93 % (93)** | **89.65 % (1793)** |

- `cargo clippy -p autoresearch --all-targets -- -D warnings`: 0 findings in W1 files. Remaining
  5 are not mine: `config.rs:314,362,368` and `types.rs:975` (`derivable_impls`, C),
  `review.rs:12` (`too_many_arguments`, W3).
- `rustfmt --edition 2024` on the five W1 files only (no `rustfmt.toml` in the repo; `cargo fmt -p`
  would have touched C's files); `git diff --stat` shows only W1 files + `Cargo.lock` (+2 lines).
- Fixture by hand in a temp git repo (`cp -R fixtures/toy-sort/. $T && git init && commit`):

  ```
  == bench.sh
  CASE n4000 time_ms=144.196
  CASE n8000 time_ms=591.764
  CASE n12000 time_ms=1213.014
  METRIC time_ms=1948.974
  METRIC peak_kb=16816
  ASI python=3.14.7
  == test.sh
  test.sh: ok
  == holdout.sh
  METRIC time_ms=1623.160
  == cheat probe (sorter.py returns fixtures/expected.txt)
  test.sh: ok            ← the bait works: the check alone cannot see it
  holdout: wrong output for n=2500 seed=101   ← holdout exit 1 catches it
  ```

- Warm-target note for the closer: the cloned `./target` had inconsistent `rayon`/`lopdf`/
  `sqlx-*`/`either` artifacts (E0460 "possibly newer version of crate `either`", missing
  `rayon` for `lopdf`); `cargo clean -p rayon -p lopdf -p libsqlite3-sys -p sqlx-core
  -p sqlx-sqlite -p either` and a rebuild fixed it (W6 hit the same).

## Deviations

- **Gate (§5.2 step 3):** `GateStep::Confirm.incumbent_sample` is also `true` when the incumbent
  has a single sample (after a fast keep). The one sample that fast-kept was selected for being
  extreme, so `x̄_i` is biased and the next true gain is under-read; the verbatim rule gave 82.4 %
  power at 3σ against the doc's own ≥ 85 % acceptance (table above). One extra incumbent sample
  in the confirm phase (same cost as a confirm sample) restores 89.7 %; null rate unchanged.
- `custody::take` resolves draft scripts from `<root>/drafts/<slug>/` where `<root>` is
  derived from `evaluator_dir` (`evaluator/<segment>` → `<slug>` → root): the signature has no
  root argument.
- `Guard` carries no direction; secondaries are judged in the primary's direction (a
  throughput study guarding memory needs a `direction` on `Guard` — C/W3).
- `GateStep::Decide` has no guard payload, so `line()` for `GuardFailed` is generic; W3 should
  append `gate::guard_failures(cfg, input).join(", ")` to the line.
- `line()` does not append `, checks passed` (checks run after the gate; W3 appends).
- `runner::command` logs to `runs/<n>/command-<k>.log` (it cannot tell checks from smoke); W3
  picks distinct `k` per call.
- Scanner treats a non-numeric `METRIC` value (`12ms`) as a bad number (crash), same as `nan`.

## Hand-offs

- `Cargo.toml` (C's): added `platform` (ProcessTree/killpg, `pid_alive`, `load1`), `humantime` and
  `globset` — all already in the workspace lock, no new crates.
- New public helpers (additive): `gate::guard_failures`, `gate::ROBUST_AFTER`,
  `runner::parse_duration`, `slot::{wait_budget, charge_wait, Slot::held, BUILTIN, SLOT_DIR,
  BUILD_SLOT_DIR}`, `custody::CUSTODY_ONLY`, `scanner::ASI_CAP`.
- W3: holdout samples go through `runner::sample` (role `Holdout`, it needs METRIC parsing and
  the slot), not `runner::command`.
- W3: `GateStep::Confirm.more` is `confirm_runs`; the drift rule's "doubles the next confirm
  count" is the caller's (take `2 × more`).
