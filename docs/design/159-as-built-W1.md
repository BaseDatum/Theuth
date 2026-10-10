# 159 W1 PARETO — as built

Worktree `theuth-159-w1`, branch `feat/159-w1-pareto`, cut from C's skeleton `1a70e5fe`. Implements §6 "Several objectives — Pareto keeps" end to end on C's contract (§5, §18): engine rule, parent choice, ledger mirror, Study view, setup row, draft card, skill section, guard tests, fixture.

## What was built

| file | what |
|---|---|
| `crates/autoresearch/src/pareto.rs` (new body) | thresholds (`sidak_z`, `t_of`, `dof_of`); objectives as `Axis`; `compare` (one candidate against one reference, per-objective `Call`); `Dom`/`Trees`/`decide` — the pure sequential dominance rule; `replay`/`crowding`/`front` — the front replayed from the ledger; `sync` — the mirror into `archive`, the front refs and `studies.scores`; the frozen plug-ins `step`, `parent_of`, `settle`, `view`, `brief_section`, `headline`, `card_line`, `archive`, `SETUP_ROWS`/`setup_value`/`setup_set`; `ArchiveQuery` (the `autoresearch.archive` actor message, dispatching to `population::archive` for `search: population`); `parent_ref` (helper for hand-off H4); tests |
| `crates/autoresearch/src/refs.rs` | unchanged (C's four plumbing calls are what `sync` uses) |
| `crates/autoresearch/src/gate.rs` (`mod tests` only) | `simulate_front`, `front_null_entry_rate_at_most_one_percent_for_two_to_four_objectives`, `front_keeps_a_true_three_sigma_gain_at_three_objectives` |
| `crates/autoresearch/src/rpc_impl.rs` (`// 159:W1` line) | `ask(crate::pareto::ArchiveQuery { slug })` |
| `crates/autoresearch/skill/pareto.md` | "## Several objectives (Pareto keeps)": when to use it against a guard, the one question (`max_regression`), the spec shape with `axes`, what changes in the loop, `parallel: 1` |
| `crates/autoresearch/fixtures/toy-pareto/**` | `codec.py` (scope), `corpus.py` (evaluator), `bench.sh`, `test.sh`, `holdout.sh`, `README.md` with the spec and the known front |
| `crates/desktop/ui/src/components/autoresearch/FrontChart.tsx`, `FrontChart.test.ts` | the scatter on the first two objectives, members outlined and joined by a staircase, hover/keyboard labels, empty card |
| `docs/design/159-as-built-W1.md` | this note |

Registration lines used: `actor.rs` `parent_of` (`Search::Front`), `kind_rule` (`GateRule::Dominance`), `kind_view`; `brief.rs` `kind_section`; `setup.rs` `KIND_SETUP`; `finalize.rs` `kind_headline`; `study_draft.rs` `kind_line`; `rpc_impl.rs` `archive`; `StudyView.tsx` `KindPanel`; `lib.rs` `include_str!("../skill/pareto.md")`. None was moved or added.

## How it maps to §6

**Mechanism.**
- *Objectives*: `axes_of(spec)` = `objectives_in_force()` with role `objective` (competing) then `guard`; `report` never enters. `Effect` strings parsed once.
- *Noise per objective*: `sigmas_of` = `gate::pooled_sigma` per objective over every clean tree of the study (holdout, noisy and lost rows excluded), the headline from `Sample.value`, the others from `Sample.secondaries`.
- *Comparing two trees*: `compare` — Δ_j direction-normalised, `z_j = Δ_j / (σ_j √(1/n_c + 1/n_r))`; better iff `z_j ≥ z_k ∧ Δ_j > min_effect_j`; worse iff `z_j ≤ −z_k` (a tolerated loss is still a loss, R-W1-2); `max_regression_j` is the loss a trade may carry — past it the candidate is not kept (`Compare::beyond_tolerance`); empty = any loss; σ_j = 0 is deterministic (effects alone); a guard is judged in its own direction and never called better.
- *Dominance under noise*: `Compare::dominates` = better on ≥ 1 competing objective and worse on none.
- *Entry rule*: in `decide` — (1) no member dominates c, (2) c is better than every member on some objective (each on the member's own samples), (3) every guard holds (`GuardFailed`). Members c dominates leave in `replay` (`front.retain(!dominates)`); their rows stay, `sync` deletes their refs.
- *Verdicts*: `Improved` when c dominates its parent (no significant loss: it replaces the parent), `Traded` when it enters beside it with a loss within `max_regression`; a loss past `max_regression` is `Worse`; else `NoGain` / `Worse` / `GuardFailed`.
- *Sequential sampling*: one sample → `Worse` on a significant loss with no gain; `NoGain` below `z_discard` (confirm when the parent has < 3 samples and some Δ > 0, as 140); a fast entry needs `z_fast,k` on some objective and no worse objective; a possible trade always confirms; else `Confirm { confirm_runs }`. On means every call is at `z_k`.
- *Parents*: `parent_of` sorts members by children ascending, ties by the headline objective; slot k takes the k-th. The study worktree is reset to the parent through `Backend::discard` when `Backend::head` differs (the Fake proves the reset: `resets == ["keep0001"]` at #4).
- *Staleness*: `Trees::parent_age` from the parent's newest sample; a re-measure is requested only when the parent is the incumbent (the engine's in-place sample measures `study.incumbent_sha`, §Deviations W1-j).
- *Cap*: `FRONT_CAP = 8`; `crowding` is NSGA-II's (extremes infinite); the least crowded leaves.
- *Waves*: `parent_of(Some(_))` is refused (W1-l, R-W1-8); promotion stays the ratchet's (hand-off H1).

**Seams.** `pareto::step` is `kind_rule`'s `Dominance` arm and returns `Decide { detail: Some(GateDecision { rule: Dominance, objectives: one ObjectiveDelta per axis, stat: max competing z, threshold: z_k or z_fast,k, n: candidate samples }) }`; the scalar fields carry the headline (axis 0). `gate_cfgs` of the design is `Dom::new(cfg, axes, sigmas, dof)`. One input per objective is built from the samples (`tree_values`), not from `GateInput.secondaries`. `incumbent_of` stays; `parent_of` is the dispatch C made.

**Data and UI.** `archive` rows of kind `front` (`f<n>`, `f0` = the base), `experiments.gate` with rule `dominance`, `studies.scores` = the headline member's means — all written by `sync` from `parent_of` (also `settle`). `KindView::Front { axes: first two competing objectives, points: base + every experiment with a measured candidate, members }`; `FrontChart` draws it. Setup row `objectives` (label "Objectives", one leaf). Card line 1: ``Find the trade-offs between `time_ms` (lower) and `size_kb` (lower) of `bash bench.sh` in <repo> (base …); every non-dominated version is kept on the front``, plus the warning past three objectives. Skill section as above.

**Guard.** Keep thresholds Šidák-corrected for the competing count (`sidak_z`: 2.00 → 2.28 / 2.43 / 2.53 / 2.61, tested against §4's table), then widened to the Student-t quantile at the σ pool's degrees of freedom (`t_of`, W1-c). Condition 2 and the cap as above. Every dominance decision stores its per-objective z and the threshold it had to clear.

## Deviations

- **W1-a** The setup row `objectives` has one leaf, `objectives`, edited as lines `name lower|higher min_effect [max_regression] [unit]` (the `secondary_guards` grammar), not the indexed leaves `objectives[i].{…}` of §6: `SetupRow.leaves` is a static list and `kind_setup_for` matches leaf names exactly. Two or more lines set `axes.objectives: pareto` and `axes.search: front` and `normalise()` the spec.
- **W1-b** The engine's incumbent (and so the study branch, holdouts, the drift repair, the trailer's x̄_i) is the **latest kept member**, not the headline member: `record_keep` moves `incumbent_sha` on every keep candidate including `Traded`, and no W1 hook runs between a keep and the next fire. The headline member is what `studies.scores`, the headline, the brief and the view name; the parent choice does not depend on the incumbent. Landing is per changeset anyway (§17 Q8).
- **W1-c** On top of Šidák, `z_k` and `z_fast,k` are widened to the exact Student-t quantile at the σ pool's dof (`t_of`; R2 replaced the Cornish–Fisher expansion, R-W1-6). Without it the simulated family-wise null entry rate with the staleness re-measure was 0.76 / 0.92 / 1.05 % at k = 2 / 3 / 4 (over §6's 1 % bound at k = 4): with k per-objective σ estimates on a few repeated samples, normal thresholds under-correct. With it: 0.61 / 0.80 / 0.83 %, power 76.8 % at k = 3. `z_discard` stays uncorrected (W-p); correcting it cost 20 points of power.
- **W1-d** The "more than three objectives" warning is appended to the card's line 1 by `card_line` (`study_draft::warnings` has no per-kind hook).
- **W1-e** `headline` reads each member's means from the ledger's samples; no fresh per-member measurement against the base at finalize (§6 Cost line 3). Finalize's changesets still re-measure every kept commit alone against the base.
- **W1-f** The dominance rule has no `Suspicious` verdict; the reviewer still runs on every keep.
- **W1-g** Segments: the front pools samples across segments (a member from an earlier segment keeps its samples) and σ_j is pooled over every tree of the study, not the current segment. A 52 → 6 ms keep on the fixture does open a regime segment (140 §5.3 Rev. 2) and spends the R-25 re-measure; the end-to-end test covers it.
- **W1-h** `experiments.parent_ref` is not written: the row is inserted by `begin_env` after `parent_of` returns a bare sha and rewritten whole by the measure loop. Hand-off H4 is the one-line fix.
- **W1-i** Store-only surfaces (`view`, `brief_section`, `headline`, `archive`) replay the front at the default gate thresholds (`DEFAULT_Z` = 2.0 / 3.5); `step` and `parent_of` use the study's config. They differ only when `gate.z` is configured away from the default.
- **W1-j** A confirm re-measures the parent in place when it is the incumbent and has fewer than 3 samples (140: fewer than 2): a fast-kept member's mean is selected for being extreme, and under the corrected threshold a true gain over it read low (power 64 → 77 %). A parent that is not the incumbent is never re-measured (the engine samples `study.incumbent_sha`).
- **W1-k** The front is not state: `front` replays every `Kept` experiment in order through the entry rule over the ledger's samples; `archive`, the refs and `studies.scores` are an idempotent mirror written at `parent_of` (and `settle`). A crash between a keep and the next fire loses nothing.
- **W1-l** Front + `parallel > 1`: `parent_of(Some(k))` gives each slot a different member, but `wave::promote_env` cherry-picks onto the incumbent and confirms against it (wave.rs is frozen). Hand-off H1 refuses the combination; the skill says `parallel: 1`.
- **W1-m** A possible trade (better on one objective, worse on another) is never entered on one sample; it takes the confirm samples first.
- **W1-n** `adopt_pending_keep` (R-50) requires `parent_sha == incumbent`; a Front keep whose ledger write crashed on a non-incumbent lineage is saved as a stray patch, not adopted.
- **W1-o** `ArchiveQuery` implements `Message<_> for Studies` inside `pareto.rs` (lib.rs is frozen); it dispatches on `axes.search` so W4 needs no second line.

## Gates

| gate | result |
|---|---|
| `build-lock mbx test -p autoresearch --locked --no-run` | ok |
| `mbx test -p autoresearch --locked` | **196 passed, 0 failed** (golden ledger included) |
| `build-lock mbx clippy -p autoresearch --locked --tests` | ok, 0 warnings |
| `cargo fmt -p autoresearch` | touched only `pareto.rs` / `gate.rs` |
| `build-lock bun run check` (crates/desktop/ui) | ok |
| `build-lock bun run test -- FrontChart.test.ts components.test.ts` | 12 passed (2 files) |

## Proof (§15 P / A / T)

**P — dominance.** `pareto::tests::compare_calls_better_same_worse` (better / same / worse / deterministic / guard), `decide_sequential_and_entry_rule` (fast `Improved` with stored detail, one-sample trade confirms, trade enters as `Traded`, one-sample `Worse`, `NoGain`, confirm band, conditions 1–3, staleness re-measure only for the incumbent parent, deterministic), `replay_evicts_dominated_and_caps_by_crowding`, `sidak_matches_the_design_table`, `setup_and_card`.

**P — null ≤ 1 % at k = 2–4, power ≥ 70 %** (`gate.rs`, 100 studies × 100 experiments; 200 × 50 for power; the sim follows every `Confirm`, rotates parents over the front, re-measures stale parents, evicts dominated members):

| k | null front-entry rate | 3σ gain on one objective |
|---|---|---|
| 1 (control, same machinery) | 0.32 % | — |
| 2 | 0.61 % (61 / 10 000) | — |
| 3 | 0.80 % (80 / 10 000) | **76.8 %** (7 678 / 10 000) |
| 4 | 0.83 % (83 / 10 000) | — |

140's own simulations are unchanged: null 0.39 %, 3σ keep 83.6 %.

**P — the fixture through the test backend.** `pareto::tests::toy_pareto_front_end_to_end` drives `create → on_fired → begin/run/log × 5 → begin` with a `Backend` double that scripts `(time_ms, size_kb)` samples from `toy-pareto/README.md`'s measured numbers and no git or models:

| # | idea | parent | samples | verdict | state |
|---|---|---|---|---|---|
| base | RLE + zlib 6 | — | 52.0 / 52.4 / 51.6 ms · 79.0 KB | — | f0 |
| 1 | drop the RLE pass | f0 | 6.0 ms · 78.6 KB (1; fast) | `Improved` (z_fast,2 at dof 2) | Kept → f1; f0 dominated, leaves; regime segment |
| 2 | bz2 | f1 (incumbent, re-measured 6.1) | 20.0 / 20.4 / 19.8 · 48.0 | `Traded` (worse, better) | Kept → f2 |
| 3 | bz2 again | f2 (fewest children) | 20.1 · 48.0 (1) | `NoGain` | Discarded |
| 4 | store | f1 (tie → headline; worktree reset to keep0001) | 0.30 / 0.35 / 0.30 · 266 | `Traded` | Kept → f4 |
| 5 | slow big codec | f4 | 30 · 300 (1) | `Worse` | Discarded |

After #6 opens: `archive` = f1 (`keep0001`, 2 children) · f2 (`keep0002`, 1) · f4 (`keep0004`, 1), refs `refs/autoresearch/toy-pareto/front/f<n>`, parent of #6 = f4; `studies.scores.time_ms` = 0.317; the engine's incumbent = `keep0004` (W1-b); `KindView::Front { axes: [time_ms, size_kb], members: [1, 2, 4], points: #0…#5 }`; `autoresearch.archive` = 3 rows; headline `front of 3 · time_ms −99.4 % · size_kb −39.2 %`; every decided row carries `gate.rule = dominance` and its threshold. Realised entry rate on the script: 3 of 5 (every entry a true one by construction).

**P — the fixture itself.** `bash bench.sh` in a copy: `METRIC time_ms=51.7`, `METRIC size_kb=79.0`, 1.4 s wall; `test.sh: ok`; `holdout.sh` 36 ms · 55 KB. The stdlib survey (README): `store` 0 ms · 266 KB, `zlib 1` 2.2 · 80, `zlib 6` 6.0 · 69, `bz2` 20.5 · 48 — a front spanning 5.5× in size and > 9× in time; `lzma` dominated.

**A.** The tables above are the front and the realised rates. **T** (the live 30-experiment study on a scratch engine) is the closer's; the spec is in the README.

**UI.** `FrontChart` rendered from the end-to-end numbers in headless Chromium: six dots, three outlined members, the staircase `M90.7,56.2 H155.4V224.9 H312.2V252.4` (store → zlib-1-like → bz2), axes `time_ms →` / `size_kb →`, legend.

## Hand-offs (for Z or the orchestrator; exact diffs)

**H1 — `crates/autoresearch/src/tools.rs`, `validate_axes`** (W1-l): refuse a front study with waves until wave.rs promotes per slot.
```diff
     if a.search == Search::Front && a.objectives != ObjectivesMode::Pareto {
         return Err("`search: front` needs `objectives: pareto`".into());
     }
+    if a.search == Search::Front && spec.parallel > 1 {
+        return Err("a front study runs one experiment at a time (`parallel: 1`); wave promotion onto the front is not built (159 W1-l)".into());
+    }
```

**H2 — `crates/desktop/ui/src/components/autoresearch/RunTable.tsx`** (C-e): one Δ % column per objective with its direction word, read from `row.gate.objectives[*].{name, delta_pct, call}` when `row.gate?.rule === 'dominance'`; the scalar Δ % column stays the headline. (Z owns the file; no diff proposed beyond the data path.)

**H3 — `crates/autoresearch/src/setup.rs`, `confirm`** (F7 of the skeleton): the panel cannot OK a kind row.
```diff
 pub fn confirm(root: &Path, draft_id: &str, row: &str) -> Result<()> {
-    if !MAIN_ROWS.contains(&row) && !ADVANCED_ROWS.contains(&row) {
+    let mut d = read_draft(root, draft_id)?;
+    if !MAIN_ROWS.contains(&row)
+        && !ADVANCED_ROWS.contains(&row)
+        && !kind_rows(&d.spec).iter().any(|r| r.row == row)
+    {
         return Err(Error::refused(format!("unknown row `{row}`")));
     }
-    let mut d = read_draft(root, draft_id)?;
```
Likewise `row_json` returns `Null` for kind rows, so the sticky check ignores them; `_ => kind_setup_for(spec, row).map_or(Value::Null, |(k, r)| serde_json::json!((k.value)(spec, r.row)))` would make them sticky.

**H4 — `crates/autoresearch/src/actor.rs`, `begin_env`** (W1-h): write `parent_ref`.
```diff
             let parent = parent_of(env, &study, None).await?;
-            let exp = ExperimentRecord::open(&slug, n, study.segment, &parent);
+            let mut exp = ExperimentRecord::open(&slug, n, study.segment, &parent);
+            exp.parent_ref = crate::pareto::parent_ref(&env.store, &slug, &parent).await?; // 159:W1
             env.store.insert_experiment(&exp).await?;
```

**H5 — `crates/autoresearch/src/rpc_impl.rs` line 188** is shared with W4; keep W1's version (`ask(crate::pareto::ArchiveQuery { slug })`): the handler already dispatches `axes.search == Population` to `population::archive`.

**H6 — `crates/autoresearch/src/setup.rs`** (optional): an `axes.objectives` / `axes.search` leaf, so a 140 draft can be switched to a front study from the panel; today the switch happens through `autoresearch_draft` (the skill says how) or by setting two `objectives` lines.

**H7 — `crates/autoresearch/src/wave.rs`** (later wave or Z): per-slot entry-rule promotion for `Search::Front` (§6 "Waves"); until then H1.

## R2 — review fixes (R-W1.md)

Gates re-run after the fixes: `build-lock mbx test -p autoresearch --locked --no-run` ok · `mbx test -p autoresearch --locked` **197 passed / 0 failed** (golden ledger green) · `build-lock mbx clippy -p autoresearch --locked --tests` 0 warnings · `cargo fmt -p autoresearch` touched only `pareto.rs`/`gate.rs` · `build-lock bun run check` ok · `bun run test` FrontChart + components **13 passed** (no wire change, no bindings export). Simulations after the exact t quantile: null front-entry 0.61 / 0.80 / 0.82 % at k = 2 / 3 / 4, power 76.7 % at k = 3.

- **R-W1-1 [BUG]** — handed off (actor.rs is frozen); exact diff H8 below. The P-test cannot see it (the double writes the literal `"diff"`), so no test is possible inside W1's files; H8 is the fix and the ratchet is unchanged by it (`parent_sha == incumbent` there).
- **R-W1-2 [RISK]** — fixed. A significant loss is `Worse` whether or not it is tolerated, so a tolerated loss never dominates and never evicts; `max_regression` is the loss a trade may carry — past it the candidate is `Worse` and not kept (`Axis.max_regression: Option<Effect>`, `None` = any loss; `Compare::beyond_tolerance`; `decide` refuses past it). Tests: `compare_calls_better_same_worse` (4 % within a 10 % tolerance → `Worse`, not dominating, not beyond; 20 % → beyond) and `decide_sequential_and_entry_rule` (8 % → `Traded`, 30 % → `Worse`) — both fail on the old code (`Same`/`Improved`/`Traded`). Skill text and the brief line say so now.
- **R-W1-3 [RISK]** — not changed. A non-incumbent parent is not re-measured; `parent_of` could sample it in place, but every sample taken outside the measure loop bypasses the engine's drift/noisy accounting (`absorb_incumbent_sample` is private to actor.rs) and the budget's per-experiment bench cap. Recorded as W1-j; hand-off H9 names the one-line engine change that makes the measure loop sample the parent instead.
- **R-W1-4 [RISK]** — not changed; recorded next to the simulation table (below). Pooling σ per segment would leave members from earlier segments without samples; pooling within ±50 % of the reference mean is a reasonable next step and lives entirely in `sigmas_of`.
- **R-W1-5 [RISK]** — partly refuted, partly open. Membership is recomputed from all samples, but eviction depends only on the dominated member's and the evictor's own samples plus σ_j/dof; members' samples never change after their keep, so only σ_j (every new sample) can flip a call near the threshold. The flicker is bounded to borderline pairs; the archive mirror is idempotent and `children` is recounted from `parent_sha`, never lost. Persisting eviction (an `evicted_by` event written at `parent_of`) is the follow-up; not done in R2 (it changes the replay's semantics and the sim would need it too).
- **R-W1-6 [RISK]** — fixed. `t_of` is now the exact Student-t quantile (regularised incomplete beta, bisection) — `evalcore::gate::t_quantile` does not exist on C's skeleton in this worktree (evalcore has `margin`/`paired`/`decide` only); Z may swap the body for W2's function at merge (H10). Test `t_of_is_the_exact_student_quantile`: `t_of(3.68, 2)` within 0.05 of the closed form 65.4 (the expansion gave 34.6 → fails before), `t_of(2.0, 2)` exact, `t_of(2.0, 10) ∈ (2.22, 2.3)`, large dof → z.
- **R-W1-7 [RISK]** — handed off (actor.rs); H8 covers the report's `inc_samples` too.
- **R-W1-8 [RISK]** — fixed. `parent_of(Some(_))` returns `Error::refused` (fail closed); test in `toy_pareto_front_end_to_end` (`parent_of(&st.env(), &study, Some(0))` is an error — returned `keep0002` before the fix). H1 still refuses at draft time.
- **R-W1-9 [RISK]** — recorded: the ≤ 1 % figures hold for the simulated model (homoscedastic σ = 1 on every tree, permanent eviction, no sample cap, parent re-measure only for the incumbent, `z_discard` uncorrected). Not covered: heteroscedastic σ (R-W1-4), membership flicker (R-W1-5), the actor's `max_samples` fallback to the scalar `gate_step` against the incumbent when `decide` keeps confirming (unreachable: `decide` decides once the candidate has `1 + confirm_runs` samples and `max_samples = 2·confirm_runs + 2`).
- **R-W1-10 [RISK]** — not changed; W1-b stands. Keeping the worktree detached is refused by `git::repair` on every begin (it re-attaches the branch, with an incident); moving the branch to the headline member needs the same frozen `repair`. A `refs/autoresearch/<slug>/kept/<n>` ref per keep is a good idea for Z (H11: one `refs::update`-style call in `log_env` after `backend.keep`).
- **R-W1-11 [POLISH]** — fixed: the brief line no longer quotes thresholds and describes the tolerance correctly.
- **R-W1-12 [POLISH]** — not changed: `KindView::Front.axes` carries names only (types.rs frozen); the chart's "lower-left" assumption holds for the fixture and every `lower` objective; a `higher` objective needs the directions in the view (H12: add `directions: [MetricDirection; 2]` to `KindView::Front`). Duplicate tick keys when all values are equal: real but harmless (React warns); fixed by deduping ticks in FrontChart — done.
- **R-W1-13 [POLISH]** — fixed: `holdout.sh` prints the seed's own `size_kb`; `test.sh` round-trips a document no corpus seed produces instead of the vacuous identity check; README updated.
- **R-W1-14 [POLISH]** — `settle` and `sample` are the frozen per-kind surface (§18.4) and stay; `let _ = slug;` in rpc_impl.rs is C's line, not the registration line (Z may drop it).

### Hand-offs added in R2

**H8 — `crates/autoresearch/src/actor.rs`, `measure_env` / `log_env` (R-W1-1, R-W1-7).** A front experiment's worktree sits at `exp.parent_sha`, not at the incumbent; every git base and the report's reference must be the parent. The ratchet is unchanged (`parent_sha == incumbent`).
```diff
@@ measure_env
-    let patch_hash = env.backend.patch_hash(worktree, &incumbent)?;
+    let patch_hash = env.backend.patch_hash(worktree, &exp.parent_sha)?;
@@ measure_env (the report's reference samples; R-W1-7)
-    let inc_samples = incumbent_samples(env, study, &incumbent).await?;
+    let inc_samples = incumbent_samples(env, study, &exp.parent_sha).await?;
     let inc_vals = bench_values(&inc_samples);
@@ log_env
     let incumbent = incumbent_of(&study)?;
+    // 159 W1: the tree the worktree rests on (a front member); the ratchet's parent is the incumbent.
+    let base = exp.parent_sha.clone();
@@ log_env — verify_tree_for_keep
-        match verify_tree_for_keep(env, &study, &exp, &worktree, &incumbent)? {
+        match verify_tree_for_keep(env, &study, &exp, &worktree, &base)? {
@@ log_env — keep_pending
-                &serde_json::json!({ "n": n, "parent": incumbent }),
+                &serde_json::json!({ "n": n, "parent": base }),
@@ log_env — review revert
-                        .diff_text(&worktree, &incumbent, &sha, 4 << 20)?;
+                        .diff_text(&worktree, &base, &sha, 4 << 20)?;
-                    env.backend.reject(&worktree, &slug, n, &incumbent)?;
+                    env.backend.reject(&worktree, &slug, n, &base)?;
                     let back = StudyPatch::incumbent(&incumbent, previous_mean, study.sigma);   // the study's incumbent goes back to the previous keep; unchanged
@@ log_env — discard
-        let hash = env.backend.discard(&worktree, &incumbent, &patch).map_err(|e| {
+        let hash = env.backend.discard(&worktree, &base, &patch).map_err(|e| {
```
`incumbent_sample_in_place` (confirm re-measure) keeps `incumbent`: `decide` only asks for it when the parent is the incumbent.

**H9 — `crates/autoresearch/src/actor.rs`, `incumbent_sample_in_place` (R-W1-3, optional).** Sample `exp.parent_sha` instead of `incumbent` (`discard(worktree, &exp.parent_sha, stash)`, `tree_sha = &exp.parent_sha`, `absorb_incumbent_sample` only when the two coincide). Then `pareto::decide`'s `parent_is_incumbent` gate on `incumbent_sample` can be dropped (one line) and stale non-incumbent parents are re-measured by the design's rule.

**H10 — `crates/autoresearch/src/pareto.rs`, `t_of` (R-W1-6).** Once W2's `evalcore::gate::t_quantile(df, p)` is on the branch, Z may replace the body with `t_quantile(dof.max(2), 1.0 − norm_tail(z))` and delete `t_tail`/`beta_inc`/`betacf`/`ln_gamma`; `t_of_is_the_exact_student_quantile` is the regression check.

**H11 — `crates/autoresearch/src/actor.rs`, `log_env` after `backend.keep` (R-W1-10).** `git::plumbing(&worktree, &["update-ref", &format!("refs/autoresearch/{slug}/kept/{n}"), &sha])?;` so every kept commit stays reachable after its front ref is deleted.

**H12 — `crates/autoresearch/src/types.rs`, `KindView::Front` (R-W1-12).** `directions: [Direction; 2]` beside `axes`, filled in `pareto::view` from `Front.axes`; FrontChart then draws the staircase corner per direction.
