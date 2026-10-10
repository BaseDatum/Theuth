# 159 W4 — as built (search, not a ratchet: population, archives, islands)

Branch `feat/159-w4-population`, worktree `theuth-159-w4`, cut from C's `8c8410aa`. Design: `docs/design/159-autoresearch-variants.md` §9, built against §18 (file ownership, the `// 159:W4` lines, C's frozen APIs). Paths under `crates/autoresearch/src/` are written bare.

## 1. What was built, by §9 part

| §9 part | what | where |
|---|---|---|
| Mechanism · Cells | `PopulationSpec {islands, descriptors, migrate_every, remeasure_after}`; `Descriptor {source: secondary \| asi \| case_best, name, bins}`; `cfg()` resolves defaults (islands = `parallel` clamped 2–4, migrate every 4 waves, `remeasure_after` = the gate's); `bin`, `cell_key` (`i<island>:<bin>[.<bin>]`), `descriptor_values` (secondary = a `METRIC` mean, the primary allowed; asi = a numeric `ASI` mean; case_best = the case, by sorted id, improved most over the base's per-case means), `bins_of` | types.rs `PopulationSpec`; population.rs "spec", "cells" |
| Mechanism · Islands, Parent sampling | the pure `Archive {islands, cells: Vec<Elite>}`: `ranked`, `champion`, `weights` = `s(rank) / (1 + children)` with `rank_weight` a logistic (top ≈ 0.9+, median 0.5), `pick_parent` (10 % uniform, counts the child), `accept` (empty cell takes; occupied by the caller's `beats`), `offers` (island i's best re-keyed to island i + 1); a splitmix64 `Rng` seeded by `<slug>/<n>` so the pick is reproducible per experiment | population.rs "the model" |
| Mechanism · Children | `parent_of(env, study, slot)`: island = (slot − 1) mod islands; seeds the archive with the base on every island when empty (rows + refs); picks, counts the child, saves; re-measures the picked elite with one fresh `Incumbent` sample in a scratch worktree when its newest sample is older than `remeasure_after` experiments. wave.rs already cuts each slot from this parent (`open_env` → `actor::parent_of`) | population.rs `parent_of`, `seed`, `sample_tree` |
| Mechanism · Acceptance, Champion, Re-measures, Novelty | `settle(env, study, archive, exp) -> Outcome {Entered, Replaced{n}, Rejected(why)}`: the child's cell from its own candidate samples; a kept slot (commit) or a discarded `no_gain`/`worse` slot (saved patch) qualifies; an empty cell takes it when the guards hold against its parent and — for a discarded slot — its checks pass on the patch re-applied over the incumbent (`materialise`, then `Backend::keep` → the stepping stone becomes a commit, its samples re-tagged, the experiment `kept`); an occupied cell by `gate_step` against the elite's samples, following `Confirm` with up to `confirm_runs` extra candidate samples taken in the materialised tree; a kept slot the archive rejects is `discarded` with its patch saved (wave.rs's `lose` shape). `promote(env, study, wave) -> wave::Promotion` is the replacement of wave promotion: abandons open slots, settles every closed slot in `n` order, migrates every `migrate_every` waves (offers decided by the gate on existing samples; an undecided gate is no migration), moves the incumbent and resets the study branch to the champion (best headline mean over every island), saves rows and refs, removes `cand-<k>`, refreshes σ, writes `wave_promoted` (what wave.rs's `settle` keys on) and `archive` events. Novelty's patch-hash refusal is the measure loop's, unchanged; a child's parent is counted at the pick, credit or not | population.rs `settle`, `settle_inner`, `materialise`, `gate_once`, `promote` |
| Seams | refs through `refs::{update, delete, list}` (C-f encoding); `parent_sha` is the elite the slot was cut from; `experiments.parent_ref` = the parent cell's ref (the slot's island first when the base holds several cells) | population.rs `save`, `settle` |
| Data and UI · Ledger | `archive` rows of kind `cell` (`descriptor`, `island`, `children`, `scores[metric]`), `experiments.parent_ref`, `events` of kind `archive` `{wave, n, key, outcome, why}` | population.rs `row_of`, `save` |
| Data and UI · Study view | `view()` → `KindView::Archive {axes, cells, islands}` with the champion = the incumbent; `ArchiveGrid.tsx`: one grid per island, columns = descriptor 1 bins, rows = descriptor 2 bins (row 0 at the bottom), cells shaded by distance from the champion's score, the champion starred, hover/aria = key, experiment, score, descriptor values, children, sha | population.rs `view`; `crates/desktop/ui/src/components/autoresearch/ArchiveGrid.tsx` |
| Data and UI · Run table | each settled experiment's `line` starts with its outcome: `entered an empty cell (parent i0:0)`, `replaced #7 in its cell (parent …)`, `lost wave 3: archive rejected it (cell i0:2 held by #7 (no_gain z 0.41))`; RunTable.tsx is Z's, the existing line column shows it | population.rs `archive_line` |
| Data and UI · Setup chat | `SETUP_ROWS` = one `population` row with leaves `population.{islands, descriptors, migrate_every, remeasure_after}`; `setup_value` → `3 islands · cells over `time_ms` [1, 10, 100] · migrate every 4 waves`; `setup_set` with the grammar `name: e1, e2; asi:name: e1; case: e1` (≤ 2 descriptors, ascending edges) | population.rs "setup" |
| Data and UI · Draft card | `card_line` → "Population search: 3 islands, cells over `peak_kb` × `time_ms`; the champion is re-measured before it counts." | population.rs `card_line`; study_draft.rs `// 159:W4` (C's line, unchanged) |
| Data and UI · Brief | `brief_section` → an `<archive>` block: islands and grid, the acceptance rule in two sentences, the elites per cell with score, descriptor values, children, and the champion marked "unconfirmed until finalize re-measures it" | population.rs `brief_section`; brief.rs `// 159:W4` (C's line) |
| Data and UI · Skill | `skill/population.md`: when to use it, when not, the spec block, descriptor advice (behaviour not quality; edges around the base's value; never guard the descriptor) | `crates/autoresearch/skill/population.md` |
| Guard | the champion re-measure: `headline()` returns `None`, so finalize's scalar confirmation re-measures the study branch (the champion) against the base with `repeats` fresh interleaved samples — exactly the mechanism §9 names (finalize.rs `confirm`); elites are re-measured when picked (`parent_of`); acceptance into an occupied cell is `Backend::gate_step` with the study's `GateCfg` (the 140 gate) | population.rs `headline`, `parent_of`, `gate_once` |
| Fixture | `fixtures/toy-route/`: `router.py` (nearest neighbour, the scope), `bench.sh` (seeds 11, 23, 37, n = 60; `CASE s<seed> length`, `METRIC length`, `METRIC time_ms`, `ASI python`; exit 1 on a non-tour), `test.sh` (permutation check on `fixtures/input.json`), `holdout.sh` (other seeds and sizes), `fixtures/optimum.txt` (offline 2-opt + or-opt, 40 restarts: mean 6166.2 against nearest neighbour's 7449.7), README with the population spec and the ratchet control | `crates/autoresearch/fixtures/toy-route/**` |
| RPC | `autoresearch.archive {slug}` → `population::archive` = `archive_list(slug, Some("cell"))` | rpc_impl.rs `// 159:W1 159:W4` line |
| Budget | elite re-measures and the extra confirm samples of an archive decision are ordinary `scored_sample` rows (bench channel, `samples.n` = the slot's experiment); checks of a stepping stone run through `Backend::command`; no new spend channel | population.rs `sample_in`, `materialise` |

Not dispatched for this kind and left as C's neutral stubs: `population::sample` (scoring is `bench`) and `population::step` (the rule is `z_gate`).

## 2. Deviations

- **W4-a** `PopulationSpec` derives `Default` but not `Eq`: bin edges are `f64`. `StudySpec` is `PartialEq` only, so nothing upstream needs `Eq`.
- **W4-b** `Descriptor` and `DescriptorSource` live in population.rs (the wire derive set spelled out, `ts(export)`), referenced from types.rs as `crate::population::Descriptor`: the rule allows only the block spec's fields in types.rs, and a nested type is W4's.
- **W4-c** The null-champion bound. §9 asks ≤ 1 %. One clean sequential comparison with the production gate (z 2.0, z_fast 3.5, z_discard 1.0, confirm_runs 2) has a null keep rate of ≈ 1.6–1.9 % (Monte Carlo, 200 000 trials, σ known; the W4 sim measured 7 / 400 = 1.75 %). The 140 null simulation's ≤ 1 % (gate.rs `null_false_keep_rate_at_most_one_percent`) is a property of the dependent ratchet sequence — after a lucky keep the incumbent's own samples read the next candidates low — not of one comparison. The re-measure does what §9 wants (the archive's champion is the best of ~200 null draws and would be claimed almost always; after fresh samples the claim rate is the gate's own), so the test asserts the gate's rate (≤ 2 %) and says so. To reach ≤ 1 % the champion's confirmation needs a one-sided z ≈ 2.33 — finalize's (frozen) or the gate's default; see Hand-offs.
- **W4-d** Wave promotion is not dispatched to the archive in the skeleton: wave.rs is frozen and has no `// 159:W4` line, while §9 "Seams" says promotion is replaced by archive acceptance when `search: population`. `population::promote` is built as the drop-in body; the one-line dispatch in `wave::promote_env` is Hand-off H1. Until it lands, a population study's last slot log runs the 140 cherry-pick promotion. The wave test stands the hand-off in by recording the `wave_promoted` event for the wave before the last slot logs (the frozen path then does nothing) and calls `population::promote` itself; the test says so in a comment.
- **W4-e** `rpc_impl::archive` opens a `Store` on the drafts root (`ask(DraftRoot)`) because rpc_impl has no store handle and lib.rs (the actor messages) is frozen. A read on an already-migrated file; an `ArchiveRows` message would be cleaner (Hand-off H2). The line is shared with W1 and returns cells; W1 chains `front`.
- **W4-f** The reviewer is not consulted at archive acceptance (wave.rs's `review` is private; the 140 reviewer runs at promotion). A kept slot was reviewed by nothing in the 140 wave path either until promotion; the stepping-stone path adds checks but no review. Recorded as an integrity gap for Z: `review::review` on `Entered`/`Replaced` kept slots.
- **W4-g** `regime_segment` (a regime change restarting the segment at a keep) is not applied by `promote`: the champion is a sibling, not a step on one lineage.
- **W4-h** The measure loop (frozen) measures every slot against the incumbent (the champion), so a slot's `verdict`/`delta`/`z` on its row are against the champion, its patch hash and saved patch are against the incumbent; the archive decision re-gates the child against its cell's elite on the same samples (plus extra ones when the gate asks). A slot's `Worse` against the champion is therefore not a rejection — it still enters an empty cell. Measuring against `exp.parent_sha` would be a one-line change in `measure_env` plus the discard base in `log_env` (Hand-off H3, optional).
- **W4-i** `migrate_every` counts waves (`wave % migrate_every == 0`), decided on the samples both trees already have; an undecided gate is no migration (no extra samples for migration).
- **W4-j** The scratch worktrees (`<root>/<slug>/pop-<sha8>-<n>` for re-measures, `pop-child-<n>` for a settled slot) use a fresh `CancellationToken`: a Stop-now during a settle finishes the sample in flight. Settles run inside wave.rs's settle lock once H1 lands.

## 3. Gates and results

- `BUILD_LABEL=159-W4 build-lock mbx check -p autoresearch --locked` → rc 0.
- `build-lock mbx test -p autoresearch --locked --no-run` → rc 0; `mbx test -p autoresearch --locked --no-fail-fast` → **202 passed, 0 failed** (190 before W4 + 12 W4; `golden_ledger_140` ok).
- `build-lock mbx clippy -p autoresearch --locked` → see §6 (filled after the run).
- `cargo fmt`: not run over the crate (it would touch files W4 does not own); population.rs and population/tests.rs were formatted with `rustfmt --edition 2024` on those two files only.
- UI: `.ui-ready` — see §6.

## 4. P tests (population/tests.rs)

| test | proves |
|---|---|
| `bins_keys_and_islands` | `bin` (NaN → 0, past the last edge → k), `cell_key`, `island_of`, `rekey`, the C-f ref form |
| `cfg_resolves_defaults_and_caps_descriptors` | islands from `parallel` clamped 2–4, ≤ 2 descriptors, sorted deduped edges, defaults 4 / the gate's `remeasure_after`, explicit values win |
| `descriptor_values_come_from_secondaries_asi_and_cases` | secondary means over clean samples (noisy left out), the primary as a descriptor, an unreported descriptor is bin 0, `ASI` parsed, `case_best` = the case improved most over the base |
| `rank_weights_are_logistic_and_children_halve_them` | `s(rank)` monotone, top > 0.9, median 0.5; best-first ranking; one child halves the weight; the champion per direction |
| `pick_parent_is_seeded_counts_children_and_favours_the_top` | the same seed picks the same parent; an empty island → `None`; over 3 000 picks the order best > second > worst with the worst still picked > 5 % (the uniform share) |
| `accept_enters_replaces_and_rejects` | empty cell enters, `beats` replaces (the holder is the one asked), otherwise rejected naming the holder |
| `migration_offers_rekey_each_islands_best_to_the_next` | island i's best (not its most-used) elite offered to i + 1 with `children` 0; a single island offers nothing |
| `setup_grammar_round_trips_and_refuses_bad_input` | the descriptor grammar and its refusals; `setup_set`/`setup_value`/`card_line`; the spec round-trips with the block |
| `null_population_champion_clears_the_gate_at_the_gates_own_rate` | 400 null studies × 200 experiments, 2 islands × 4 cells; the champion's recorded mean is always a lucky draw; the fresh re-measure clears the sequential gate in 7 / 400 (W4-c) |
| `deceptive_landscape_archive_reaches_the_optimum_the_ratchet_does_not` | step one 2σ worse, step two 10σ better, the run time as the descriptor: 30 experiments × 50 seeds — the archive's champion is the optimum in ≥ 80 % of seeds, the ratchet's incumbent in ≤ 20 % (measured: see §6) |
| `a_wave_settles_into_cells_moves_the_champion_and_cuts_the_next_wave_from_elites` | real git on toy-route, 2 slots: the base seeds `i0:0`/`i1:0` with refs and one child each; slot 1 (`improved`, 50 ms) enters `i0:1`; slot 2 (`worse`, 200 ms, discarded with a patch) runs the checks once, is materialised as a commit on the base containing its edit, its sample re-tagged, and enters `i1:1` as `kept`; `parent_ref` per slot; the incumbent, `incumbent_mean` and the study branch move to slot 1's commit; rows, refs, `wave_promoted` and `archive` events; `cand-<k>` and the scratch worktree removed; `view` stars the champion; the brief names it unconfirmed; `headline` is `None`; wave 2 opens through wave.rs's settle (no second promotion), each slot is cut from an elite of its island at that elite's sha with `children` + 1, no bench sample spent at open |
| `a_stale_elite_picked_as_parent_is_re_measured_first` | `remeasure_after` 1 and seven experiments later: the pick takes one fresh `Incumbent` sample (n = the slot's experiment, k = 90) in a scratch worktree that is removed; picked again at once, no second sample |

## 5. Fixture evidence

`bash bench.sh` in `fixtures/toy-route`: `CASE s11 length=7247.9 · CASE s23 length=7431.2 · CASE s37 length=7670.0 · METRIC length=7449.71 · METRIC time_ms=1.283 · ASI python=3.14.8`, rc 0 in < 0.2 s. `bash test.sh` → `test.sh: ok`. `bash holdout.sh` → `METRIC length=7759.63`. Offline reference (2-opt + or-opt + 2-opt, 40 restarts per seed, 78 s): 6189.5 / 5844.3 / 6464.9, mean 6166.2 (−17.2 % against nearest neighbour), in `fixtures/optimum.txt` outside the scope. T (three population seeds against three ratchet seeds at the same experiment budget) is the closer's live run; the spec and the control are in the fixture README.

## 6. Late results

Filled after the clippy / UI runs (see the final message).

## 7. Hand-offs

**H1 — wave.rs `promote_env`: dispatch population waves to the archive (required for D7).** The one line §9 "Seams" needs; it goes right after the study is loaded, before the slots are classified, so population waves never reach the cherry-pick:

```diff
--- a/crates/autoresearch/src/wave.rs
+++ b/crates/autoresearch/src/wave.rs
@@ pub(crate) async fn promote_env(env: &Env, slug: &str, wave: u32) -> Result<Promotion> {
     if promoted_already(env, slug, wave).await? {
         return Ok(Promotion::default());
     }
     let mut study = study_by_slug(env, slug).await?;
+    if study.spec.axes.search == Search::Population {
+        return crate::population::promote(env, &study, wave).await; // 159:W4
+    }
     let exps: Vec<ExperimentRecord> = env
```

`population::promote` writes the `wave_promoted` event, removes the candidate worktrees, refreshes σ and clears the phase, so `settle`'s loop, `log_env`'s post-promotion read and `begin_env`'s pre-wave settle work unchanged. After H1, delete the stand-in `append_event(.., EVENT_PROMOTED, ..)` block in `population/tests.rs` (`a_wave_settles_into_cells_…`, marked "stands in for it") — the test then exercises the real path through `log_env`.

**H2 — `autoresearch.archive` without a second `Store` (optional, Z).** lib.rs: `delegate! { ArchiveRows => Vec<ArchiveRow>, archive_rows; }` with `pub struct ArchiveRows { pub slug: String }` in types.rs and `actor::archive_rows(env, m) = env.store.archive_list(&m.slug, None)`; rpc_impl's line becomes `ask(ArchiveRows { slug }).await` and both W1's front rows and W4's cells come back in one list.

**H3 — measure slots against their parent elite (optional).** actor.rs `measure_env`: `let incumbent = if study.spec.axes.search == Search::Population { exp.parent_sha.clone() } else { incumbent_of(study)? };` and the same in `log_env`'s discard (`discard(&worktree, &incumbent, &patch)`), so a slot's row verdict reads against the tree it was cut from and `materialise` applies the patch over `exp.parent_sha` instead of the incumbent (one line in `population::materialise`, W4's).

**H4 — champion confirmation at ≤ 1 % (W4-c).** Either `GateConfig.z` 2.0 → 2.33 (every study; the 140 sims would need re-running) or, in finalize.rs `confirm`, a one-sided z = 2.33 "confirmed" flag on the population headline (`kind_headline` stays `None`; the flag text joins `Headline.text`). Z's call; W4 recommends the second.

**H5 — reviewer at archive acceptance (W4-f, Z).** In `population::promote` after `settle` returns `Entered | Replaced` for a slot in state `Kept`: call wave.rs's `review(..)` (make it `pub(crate)`) or `review::review` directly; a `Flagged` verdict under `on_suspicious: revert` removes the cell (`archive_remove` + `refs::delete`) and marks the experiment `rejected_by_review`.

**H6 — lineage on hover (optional, Z).** `CellRow` has no parent; the grid's hover shows the cell, not the chain. Adding `parent_ref: Option<String>` to `ExperimentRow` (types.rs) lets `ArchiveGrid` take `rows` and walk `parent_ref` → cell → its experiment.

**Orchestrator** `.ui-ready` had not appeared in this worktree by the time the Rust gates were green; see §6 for what W4 could run.
