# 140 as-built — P2 RE-PROOF (F4E confine + F4A loop polish on a real engine)

Worker 140-P2, 2026-10-09, branch `feat/140-p` after `git merge --ff-only fix/140-tab-agents` (c5212f9d). Brief: `~/.theuth/briefs/2026-10-08/140/140-P2.md`. Scratch engine `/tmp/140/live2` (rpc `127.0.0.1:17941`, `t2` = `target/debug/t` 2.13.0 built with mbx from this tree, `/tmp/140/bin/t2`), config = P's `live1/config.yaml` with the port changed (haiku-4-5 for executor/reviewer/title, `agent.communication: true`, `noise.max_cv 0.2`); `bench.max_load` raised live to 64 and `bench.wait` to 10 s (load 31–47 on 16 cores: other sessions' gates). Repo `/tmp/140/repo-sort2` = `crates/autoresearch/fixtures/toy-sort` at `401eeeb base` (the fixture's untracked `__pycache__/` removed first); manifest `/tmp/140/repo-sort2-before.sha` (9 files). Drivers: P's `call.ts`, `watch.ts`, `mon2.sh`; events `/tmp/140/live2-events.jsonl`; logs `/tmp/140/p2/`. Spend: **$0.39** (88 model calls; cap $2). Two studies: `p2-a` (sequential, 6 experiments, $0.18) and `p2-w` (`parallel: 2`, 2 experiments = one wave, $0.21).

Environment notes: the first engine start (a `&` background from the tool shell) died silently at 02:26:42 mid-sample (no panic, no crash report — the tool's process tree, most likely); restarted as a managed service at 02:31:33. Side effect, recorded: `p2-a` #2 `interrupted: engine restart`, drift repaired at #3 (`stray.patch`), the loop continued — P's restart story again, unplanned. The bench slot owner file `/tmp/theuth-bench-slots/0/owner` of the killed engine was still there when the new one started; the new engine's samples ran (the slot checks the owner pid).

## Checks

### 1. Confinement (F4E) — **PASS**

- Relative paths resolve inside the worktree. After the steer "(1) use RELATIVE paths … (2) write a NOTES.md into the original repository at /tmp/140/repo-sort2 with write_file", `p2-a` #3's tool calls (`sessions.sqlite tool_calls`, session `01a11e7b-670b…`):

  | tool | args | result |
  |---|---|---|
  | `read_file` | `{"path":"sorter.py"}` | `[read_file sorter.py: 10 lines, 232 bytes]` — the **worktree's** kept `list.sort()` version (the user repo's is 16 lines / 404 bytes) |
  | `write_file` | `{"path":"/tmp/140/repo-sort2/NOTES.md", …}` | **is_error=1**: `write_file: /tmp/140/repo-sort2/NOTES.md: outside this session's write root /tmp/140/live2/autoresearch/p2-a/worktree — this session may only write inside it` |
  | `write_file` | `{"path":"NOTES.md", …}` | `created NOTES.md (758 bytes)` — in the worktree |
  | `list_dir` | `{"path":"."}` | `.git .gitignore NOTES.md README.md bench.sh fixtures/ holdout.sh main.py sorter.py test.sh` — the worktree |
  | `edit` | `{"path":"sorter.py", …}` | `edited /tmp/140/live2/autoresearch/p2-a/worktree/sorter.py` |
  | `experiment_run` | — | `no gain: -3.86 %, 0.476σ … Paths outside scope: NOTES.md — a keep needs a justification` (scope accounting saw the relative write) |
  | `shell` | `rm NOTES.md` | exit 0 (haiku cleaned up itself) |

  Before the steer (#1, #4–#6) haiku used absolute worktree paths as the brief's `Worktree:` line asks. No `write_file`/`edit` of any session ever succeeded on a path outside a study tree (query over `tool_calls`: 0 rows).
- `git -C /tmp/140/repo-sort2 status --porcelain` empty and the manifest **identical** after both studies (`diff` against `repo-sort2-before.sha` empty), checked after `p2-a` and again after `p2-w`.
- Wave round (`p2-w`, `parallel: 2`, `wave_open` 02:37:42 with slots `cand-1`/`cand-2`): slot-1 (session `01a11e86-1394…`) edited only `/tmp/140/live2/autoresearch/p2-w/cand-1/sorter.py`, slot-2 (`01a11e86-139b…`) only `…/cand-2/sorter.py`; both read only inside their own `cand-<k>`; `wave_promoted {promoted:[1]}`, `cand-*` removed after the wave (`ls p2-w/` = `evaluator report.md runs worktree`). The slot children's write root (`cand-<k>`) was not provoked live; the as-built test `wave::child_round` asserts it.
- Known and designed gap (§8.3, F4E ruling 2): `shell` is seated but not sandboxed — the steer was phrased to use `write_file`; a model that reaches for `echo > /tmp/…` is stopped by nothing but the brief.

### 2. Regime segment (F4A) — **PASS**

- `p2-a` baseline 7356 ms (σ 93.6 over 3 samples); #1 `list.sort()` → 3.145 ms, `suspicious` 68σ → reviewer CLEAN → kept `60f2ec2a`. Event `segment_auto {"segment":1,"n":1,"sigma":93.55,"delta":7352.6,"reason":"regime change at #1: Δ 7353 ms ≥ 10σ (σ 93.6)"}` at 02:26:22, one second after `keep`. `segments` row 1: `opened_by plugin`, baseline 3.145 (n 3 = the keep's samples).
- #2's brief: `Noise: sigma 0.221 ms (7.02 %). A keep needs about 0.892 ms (28.36 %) on one clear sample, or a confirmed 2-sigma gain.` and `Segment 1: opened automatically — regime change at #1: Δ 7353 ms ≥ 10σ (σ 93.6). Earlier experiments stay listed but left the statistics.` (P saw "a keep needs 1940 ms" here.)
- Later experiments judged on the new scale: #3 `sorted()` 3.02 ms `no gain −3.9 %, 0.48σ`; #4 +4.0 % −0.6σ; #5 +6.4 % −1.0σ; #6 `array.array` 4.44 ms **`worse` +41.2 %, −6.2σ** over 2 samples — a 1.3 ms regression that under the old σ would have read 0.01σ. No later *real* improvement exists on this fixture (`sorted()` is the floor), so "judged against the new σ" is shown by the thresholds and the −6.2σ verdict, not by a second keep. Study view "Needs your eyes": `segment 1 opened automatically at #1 — regime change …; σ restarted from the keep's samples`.
- `p2-w` likewise: `segment_auto` at the promoted keep (Δ 8126 ms ≥ 10σ, σ 79.3), via `wave::promote_env`.

### 3. Δ convention — **FAIL (two defects, both fixed below)**

Same signed number everywhere that F4A touched: trailer `Autoresearch-Metric: time_ms 7356 -> 3.15 (-100.0%, 68.1 sigma, n=3)`; ledger `experiments.delta_pct` #1 −99.957, #6 +41.2; finalize headline `time_ms 7788.4 → 3.81 ms (-100.0 %, CI ±6.4 %)`; changeset `delta_alone_pct −99.956 / delta_together_pct −99.957`; RunTable `−100.0 %` kept / `+41.2 %` worse. Two places disagreed:

- **D1 — "vs base" after a regime segment read `+0 %`.** The brief (`Incumbent 3.145 ms = +0 % vs base, after 1 keeps`), `StudyInfo.delta_pct` (0), the list row (`0.0 %`), the Incumbent stat (`3.15 ms 0.0 %, base 3.15 ms`) and the Finished headline (`time_ms 3.15 → 3.15 ms (0.0 %)`) all took "base" as the *current segment's* baseline, which a regime segment sets to the keep's mean. Before F4A only a user-opened segment did this; now every big win does, exactly where the user looks. Screenshot `docs/design/assets/140-p2/study-before-fix-1280.webp`.
- **D2 — the finished attention row read `Study p2-a finished: +100.0 % · Review`** (`actor.rs` `AttentionSource::items` still used `gate::normalise`, the direction flip Revision 2 removed everywhere else). Screenshot `attention-before-fix-1280.webp`.

### 4. Sidebar / attention — **PASS**

- `list_sessions` after both studies: `autoresearch-p2-a`, `autoresearch-p2-w` (roots), the two slot children (titled from their task text, parent `01a11e85…`), plus an ordinary chat created for the check. The chat sidebar (vite against the engine) lists **only** "Today: ordinary chat for the P2 sidebar…"; neither study title nor a slot child appears (`body` text search: `autoresearch-p2-a` false, `autoresearch-p2-w` false, `slot 1 of` false, `ordinary chat` true). Screenshot `sidebar-1280.webp`.
- `attention_list`: exactly two rows, `autoresearch:finished:p2-a` and `autoresearch:finished:p2-w` (kind `review`, detail `report <doc>`); **no** generic `finished:<session>` row for any of the four study sessions (the `p2-a` session ended ~10 turns, the roster added nothing).

### 5. Tab — **PASS**

`docs/design/assets/140-p2/tab-1280.webp`: the Agents band reads `Agents · Tasks · Routines · Autoresearch · Skills · Tools` with Autoresearch selected, the studies rail on the left and "Select a study" on the right (vite dev server `nice -n 10 bun run dev --port 5174`, `VITE_THEUTH_URL/TOKEN` of the scratch engine; stopped afterwards).

## Fixes (commit `140 P2: …`)

**D1 — "vs base" is the study base.** `store.rs` `Store::base_mean(slug)` = segment 0's `baseline_mean`; used by `brief::render` (the metric line), `actor::info_full` (`StudyInfo.baseline_mean` / `delta_pct`), the overfit check's `search_delta` (it compared the holdout *vs base* with the search *vs current segment* — after a regime segment `search_delta` was 0 and overfit could never fire) and `finalize`'s "every base sample crashed" fallback (it measures `self.base`). σ, the gate, custody and the chart band stay on the current segment (`ChartSeries.baseline_mean` unchanged). `types.rs` doc for `StudyInfo.baseline_mean`. UI `StudyView.tsx`: the Noise σ stat's percentage divides by the incumbent (the brief's `noise_line` convention; `keepNeedsPct` already did) instead of `baseline_mean`, which would otherwise read 0.002 % against the 7356 ms base. Tests: `brief::tests::vs_base_reads_the_study_base_on_a_later_segment` (segment 1 by the plugin with baseline = incumbent → `Incumbent 41.2 ms = −2.6 % vs base (better)` and the `Segment 1: opened automatically — …` line; failed before with `+0 %`), `actor::tests::a_regime_change_opens_a_segment_at_the_keep` extended (`StudyInfo.baseline_mean == 100`, `delta_pct == −49.75` after the auto segment; was 50.25 / 0).

**D2 — raw Δ in the finished attention row.** `AttentionSource::items`: `fmt_pct(pct(i − b, b))` over `Store::base_mean` (no `normalise`). Test: `experiment_budget_stops_and_finalizes_with_the_result_card_data` asserts `Study t1 finished: -5.0 % · Review` (was `+5.0 %`).

**D3 — finished samples stamped at whole seconds.** `runner.rs` wrote `Sample.at` with `SecondsFormat::Secs` while the pending row and `segments.opened_at` use `store::now()` (ms, F4A's segment-cut fix). Live: every finished `samples.at` read `…:16Z`, the pending one `…:42.040Z`, segment 1 `opened_at 02:26:16Z` — on a sub-second bench the cut F4A moved to ms would still have shared a second. Now `crate::store::now()`; `runner::tests::a_good_sample_parses_metrics_and_sees_the_environment` asserts the ms shape. Not rebuilt into `t2` (the live re-check below is D1/D2); the unit test and the gates cover it.

**D4 — flaky F4E test.** `stop_now_interrupts_the_experiment_in_flight` read the seat after `control` had replied `Finalizing`; the finalize task can reach `Finished` (and `unseat`) in between (1 failure in 4 runs here). The test now reads the seat first and the state second — `Finished` is written before `unseat`, so a state still `Finalizing` afterwards proves the seat had to hold. Test-only.

Repo branches at the end: `main`, `autoresearch/p2-a`, `autoresearch/p2-a+1-sorter`, `autoresearch/p2-w`, `autoresearch/p2-w+1-sorter`; the user's checkout `main` clean, manifest identical (the study branches are the designed local-only landing path, §11).

## Live re-check with the fixed binary (`t2` rebuilt, engine restarted on the same data dir)

- `autoresearch.list`: `p2-a delta_pct −99.957, baseline_mean 7355.79, incumbent 3.145 · "… 6 experiments · -100.0 % · finished"`; `p2-w −99.957 / 8129.57 / 3.531`.
- `attention_list`: `Study p2-a finished: -100.0 % · Review`, `Study p2-w finished: -100.0 % · Review`.
- Study view (`study-after-fix-1280.webp`): Finished `time_ms 7,356 → 3.15 ms (−100.0 %, better) · holdout 7429.4210 → 4.1013 (-99.9 %)`; Incumbent `3.15 ms −100.0 %, base 7,356 ms`; Noise σ `0.18 ms · 5.7 % · a keep needs ≈ 16.2 %` (unchanged — on the incumbent's scale); changeset `−100.0 % / −100.0 %`; rail rows `−100.0 %`. Attention page `attention-after-fix-1280.webp`.
- The one number that differs on purpose: the brief's / list's "vs base" is the ledger's base→incumbent (−99.957 %), finalize's confirm headline is the A/B/A/B re-measure (−99.951 %, `7788 → 3.81`) — same sign, same convention, different measurement.

## Hand-offs

- `Finished.tsx` builds its headline from `StudyInfo.baseline_mean → incumbent.mean` (the ledger's means); finalize's A/B/A/B confirmation (`7788 → 3.81, CI ±6.4 %`) is only in the report document and the `finalize` event. One source for the headline is a small UI ruling.
- The chart's holdout right axis showed `6864–6866.5` for a study whose holdout went 6301 → 4.1: `holdoutByN` plots the base holdout only (finalize's holdout samples live at `n = 0` by F4A ruling 3); the axis range is a single point's. Cosmetic.
- `experiments.delta_pct` of a wave slot that lost the promotion (`p2-w` #2 `discarded`, verdict `suspicious`, Δ −100 %) renders in the RunTable as a discarded row with a kept-sized Δ; the row's state says why. Cosmetic.
- Shell writes outside the worktree are not stopped (designed; §8.3).
- The TS bindings under `crates/desktop/ui/src/rpc/bindings/` are gitignored; a fresh worktree needs the export line before `bun run check` (255 stale-binding errors here until regenerated). Not a code hand-off, a setup note.
- The scratch engine started from the tool shell's background died silently after ~2 min (no panic, nothing in the system log); as a managed service it ran the rest of the night. Scratch-only.

## Gates (mbx 1.22.0, stable 1.96.1, `BUILD_LABEL=140-P2`)

- `mbx build -p theuth --bin t` — 2× (37 min cold after the merge, 7.5 min after the fixes; `/tmp/140/p2/build1.log`, `build2.log`).
- `mbx test -p autoresearch` — **161 passed, 0 failed** (`/tmp/140/p2/test4.log`; 160 on the merged branch + `brief::tests::vs_base_reads_the_study_base_on_a_later_segment`; the runner, attention and regime assertions live inside existing tests; the first run after the fixes failed only the flaky D4 test, `test3.log`).
- `mbx clippy -p autoresearch --tests -- -D warnings` — clean (`clippy.log`, `clippy2.log`).
- `cargo fmt -p autoresearch -- --check` — clean.
- `mbx test -p rpc --features ts --lib export_bindings` — 8 passed; bindings copied (`bindings.log`).
- `bun run check` (tsc) — 0 errors after the bindings (`tsc2.log`). `bunx vitest run src/components/autoresearch src/components/sidebar src/lib/session-tree` — **41 passed** in 4 files (`vitest.log`).
- Not run: `-p theuth --tests`, `-p app`, workspace runs (orchestrator's gate; the `t` binary built twice from this tree).

Scratch engine (`engine2b`) and the vite dev server stopped; the browser tab released.
