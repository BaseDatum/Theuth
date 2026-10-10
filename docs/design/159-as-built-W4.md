# 159 W4 — as built (search, not a ratchet: population, archives, islands)

Branch `feat/159-w4-population`, worktree `theuth-159-w4`, cut from C's `8c8410aa`. Design: `docs/design/159-autoresearch-variants.md` §9, built against §18 (file ownership, the `// 159:W4` lines, C's frozen APIs). Paths under `crates/autoresearch/src/` are written bare.

## 1. What was built, by §9 part

| §9 part | what | where |
|---|---|---|
| Mechanism · Cells | `PopulationSpec {islands, descriptors, migrate_every, remeasure_after}`; `Descriptor {source: secondary \| asi \| case_best, name, bins}`; `cfg()` resolves defaults (islands = `parallel` clamped 2–4, migrate every 4 waves, `remeasure_after` = the gate's); `bin`, `cell_key` (`i<island>:<bin>[.<bin>]`), `descriptor_values` (secondary = a `METRIC` mean, the primary allowed; asi = a numeric `ASI` mean; case_best = the case, by sorted id, improved most over the base's per-case means), `bins_of` | types.rs `PopulationSpec`; population.rs "spec", "cells" |
| Mechanism · Islands, Parent sampling | the pure `Archive {islands, cells: Vec<Elite>}`: `ranked`, `champion`, `weights` = `s(rank) / (1 + children)` with `rank_weight` a logistic (top ≈ 0.9+, median 0.5), `pick_parent` (10 % uniform, counts the child), `accept` (empty cell takes; occupied by the caller's `beats`), `offers` (island i's best re-keyed to island i + 1); a splitmix64 `Rng` seeded by `<slug>/<n>` so the pick is reproducible per experiment | population.rs "the model" |
| Mechanism · Children | `parent_of(env, study, slot)`: island = (slot − 1) mod islands; seeds the archive with the base on every island when empty (rows + refs); picks, counts the child, saves; re-measures the picked elite with one fresh `Incumbent` sample in a scratch worktree when its newest sample is older than `remeasure_after` experiments. wave.rs already cuts each slot from this parent (`open_env` → `actor::parent_of`) | population.rs `parent_of`, `seed`, `sample_tree` |
| Mechanism · Acceptance, Champion, Re-measures, Novelty | `settle(env, study, archive, exp) -> Outcome {Entered, Replaced{n}, Rejected(why)}`: the child's cell from its own candidate samples; a kept slot (commit) or a discarded `no_gain`/`worse` slot (saved patch) qualifies; an empty cell takes it when the guards hold against its parent and — for a discarded slot — its checks pass on the patch re-applied over the incumbent (`materialise`, then `Backend::keep` → the stepping stone becomes a commit and its samples are re-tagged; the row keeps the gate's `discarded`/`worse`, R2); an occupied cell by `gate_step` against the elite's samples, following `Confirm` with a fresh sample of the holder when it asks for the reference and up to `confirm_runs` rounds of extra candidate samples taken in the materialised tree, `Suspicious` rejected unreviewed; a kept slot the archive rejects is `discarded` with its patch saved (wave.rs's `lose` shape). `promote(env, study, wave) -> wave::Promotion` is the replacement of wave promotion: abandons open slots, settles every closed slot in `n` order, migrates every `migrate_every` waves (offers decided by the gate on existing samples; an undecided gate is no migration), moves the incumbent and resets the study branch to the champion (best headline mean over every island), saves rows and refs, removes `cand-<k>`, refreshes σ, writes `wave_promoted` (what wave.rs's `settle` keys on) and `archive` events. Novelty's patch-hash refusal is the measure loop's, unchanged; a child's parent is counted at the pick, credit or not | population.rs `settle`, `settle_inner`, `materialise`, `gate_once`, `promote` |
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
| RPC | `population::archive(store, slug)` = `archive_list(slug, Some("cell"))`; the rpc line is C's stub — W1's `ArchiveQuery` message dispatches to it (R2, R-W4-7) | population.rs `archive` |
| Budget | elite re-measures and the extra confirm samples of an archive decision are ordinary `scored_sample` rows (bench channel, `samples.n` = the slot's experiment); checks of a stepping stone run through `Backend::command`; no new spend channel | population.rs `sample_in`, `materialise` |

Not dispatched for this kind and left as C's neutral stubs: `population::sample` (scoring is `bench`) and `population::step` (the rule is `z_gate`).

## 2. Deviations

- **W4-a** `PopulationSpec` derives `Default` but not `Eq`: bin edges are `f64`. `StudySpec` is `PartialEq` only, so nothing upstream needs `Eq`.
- **W4-b** `Descriptor` and `DescriptorSource` live in population.rs (the wire derive set spelled out, `ts(export)`), referenced from types.rs as `crate::population::Descriptor`: the rule allows only the block spec's fields in types.rs, and a nested type is W4's.
- **W4-c** (revised in R2, R-W4-1) The null-champion bound. The only fresh re-measure a population champion gets is finalize's `confirm` (`kind_headline` → `None` so it runs): `repeats` samples per side and a ±1.96 σ CI with no decision. Under the null that CI reads a "gain" in ≈ 6 % of studies (sim: 61 / 1000) — the archive's champion is the best of ~200 lucky draws and the CI alone is no guard. The guard is the one-sided t rule at the realised dof (`t ≥ t_quantile(nb + nf − 2, 0.99)`, 3.75 at 4 dof): 7 / 1000 null studies confirmed, the ≤ 1 % §9 asks. The rule lives in finalize.rs (frozen) → Hand-off H4, exact diff; the sim models that fixed-n comparison. (The first W4-c measured the *sequential* gate on fresh samples at ≈ 1.6–1.9 %; that was the wrong mechanism — the production path never runs the sequential gate at finalize.)
- **W4-d** Wave promotion is not dispatched to the archive in the skeleton: wave.rs is frozen and has no `// 159:W4` line, while §9 "Seams" says promotion is replaced by archive acceptance when `search: population`. `population::promote` is built as the drop-in body; the one-line dispatch in `wave::promote_env` is Hand-off H1. Until it lands, a population study's last slot log runs the 140 cherry-pick promotion. The wave test stands the hand-off in by recording the `wave_promoted` event for the wave before the last slot logs (the frozen path then does nothing) and calls `population::promote` itself; the test says so in a comment.
- **W4-e** (revised in R2, R-W4-7) The `autoresearch.archive` rpc line is back to C's stub: W1's `ArchiveQuery` actor message dispatches `Search::Population → population::archive` with the actor's store, so no second `Store` is opened per UI poll.
- **W4-f** (revised in R2, R-W4-5) No reviewer runs at archive acceptance (wave.rs's `review` is private). A `Suspicious` gate verdict against a cell's elite is therefore not an acceptance: `Rejected("suspicious; not reviewed")`, the cell keeps its elite. The reviewer at acceptance stays Hand-off H5.
- **W4-g** `regime_segment` (a regime change restarting the segment at a keep) is not applied by `promote`: the champion is a sibling, not a step on one lineage.
- **W4-h** The measure loop (frozen) measures every slot against the incumbent (the champion), so a slot's `verdict`/`delta`/`z` on its row are against the champion, its patch hash and saved patch are against the incumbent; the archive decision re-gates the child against its cell's elite on the same samples (plus extra ones when the gate asks). A slot's `Worse` against the champion is therefore not a rejection — it still enters an empty cell. Measuring against `exp.parent_sha` would be a one-line change in `measure_env` plus the discard base in `log_env` (Hand-off H3, optional).
- **W4-i** `migrate_every` counts waves (`wave % migrate_every == 0`), decided on the samples both trees already have; an undecided gate is no migration (no extra samples for migration).
- **W4-j** (revised in R2, R-W4-10) Every sample and check of a settle runs on the study's live `cancel_token`; `promote` holds that token and stops settling the remaining slots after a Stop-now (they keep the gate's word, are not archived, and the wave still closes with its `wave_promoted` event). A `pop-*` scratch worktree is removed on return either way.
- **W4-k** (R2, R-W4-4) A stepping stone keeps the gate's word: its row stays `discarded` / `worse` (or `no_gain`) with `commit_sha` set to the materialised commit; only the `archive` event and the row's line say it entered. No 140 reader (stop.rs plateau and explore, the brief's keeps, `largest_accepted_delta`) counts it as a keep. Kept slots that enter or replace stay `kept`; kept slots the archive rejects become `discarded`.
- **W4-l** (R2, R-W4-9) A descriptor no sample reports is `Rejected("descriptor `<name>` not reported")`, not bin 0. `case_best` bins the 0-based index of the case (ids sorted) improved most; the skill says so.
- **W4-m** (R2, R-W4-11) `incumbent_mean` after a champion change is the champion's mean over every sample of its tree (slot samples, re-measures, confirm extras), not its archive score.

## 3. Gates and results

R1 (first DONE, `9a8e337e`): check rc 0; test 202 / 0; clippy rc 0; bindings 8 passed; `bun run check` rc 0; vitest 41 passed.

R2 (`feat(159-w4): R2 …`):
- `build-lock mbx test -p autoresearch --locked --no-run` → rc 0; `mbx test -p autoresearch --locked --no-fail-fast` → **204 passed, 0 failed** (14 W4; `golden_ledger_140` ok). With `--nocapture`: `deceptive: archive 50 / 50, ratchet 0 / 50`; `null champion: CI gain 61 / 1000, confirmed (t rule) 7 / 1000`.
- `build-lock mbx clippy -p autoresearch --locked` → rc 0, 0 warnings.
- No wire type changed in R2 (types.rs untouched since R1): bindings not re-exported; no UI file touched: `bun` gates not re-run.
- Before/after proof for R-W4-2: with the old whole-row write put back (`patch.apply_to(&mut study); update_study(&study)`), `a_wave_settles_into_cells_…` fails at "the promotion patches its own columns and keeps the notes another session wrote" (`left: ""`); with `patch_study` it passes.
- `cargo fmt`: not run over the crate (it would touch files W4 does not own); population.rs and population/tests.rs formatted with `rustfmt --edition 2024` on those two files only (`--check` clean afterwards).
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; `Descriptor.ts`, `DescriptorSource.ts`, `PopulationSpec.ts` generated; copied to `crates/desktop/ui/src/rpc/bindings/` (not committed).
- `.ui-ready` appeared at 17:45; `build-lock bun run check` → rc 0; `build-lock bun run test -- src/components/autoresearch src/rpc/api.test.ts` → 6 files, **41 passed** (37 + the 4 of `ArchiveGrid.test.ts`). Visual limit: the grid is proved by static paint (`renderToStaticMarkup`) — one section per island, 4 cells, one `data-champion`, `★ #1`, hover text, 5 empty cells on a 2 × 4 grid; no running desktop UI in this worktree.

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
| `t_quantile_matches_the_table` | the sim's stand-in for `evalcore::gate::t_quantile` against the table (3.747 / 2.764 / 2.457 at 4 / 10 / 30 dof, 0.99; 2.776 at 4 dof, 0.975) |
| `null_population_champion_confirmed_at_most_one_percent_under_the_t_rule` | 1 000 null studies × 200 experiments, 2 islands × 4 cells; the champion's recorded mean is always a lucky draw; finalize's fixed-n re-measure (3 + 3 fresh samples, σ pooled from them) reads a gain past the ±1.96 σ CI in 61 / 1000 and is confirmed under the H4 t rule in 7 / 1000 (R-W4-1) |
| `deceptive_landscape_archive_reaches_the_optimum_the_ratchet_does_not` | step one 2σ worse, step two 10σ better, the run time as the descriptor: 30 experiments × 50 seeds — the archive's champion is the optimum in 50 / 50 seeds, the ratchet's incumbent in 0 / 50 |
| `a_wave_settles_into_cells_moves_the_champion_and_cuts_the_next_wave_from_elites` | real git on toy-route, 2 slots: the base seeds `i0:0`/`i1:0` with refs and one child each; slot 1 (`improved`, 50 ms) enters `i0:1`; slot 2 (`worse`, 200 ms, discarded with a patch) runs the checks once, is materialised as a commit on the base containing its edit, its sample re-tagged, enters `i1:1` and **stays `discarded` / `worse`** (R-W4-4); notes another session writes during the settle's checks survive the promotion (R-W4-2); `parent_ref` per slot; the incumbent, `incumbent_mean` and the study branch move to slot 1's commit; rows, refs, `wave_promoted` and `archive` events; `cand-<k>` and the scratch worktree removed; `view` stars the champion; the brief names it unconfirmed; `headline` is `None`; wave 2 opens through wave.rs's settle (no second promotion), each slot cut from an elite of its island with `children` + 1, no bench sample spent at open |
| `a_challenged_one_sample_elite_is_re_measured_and_suspicious_jumps_stay_out` | σ = 10; wave 1 files two one-sample stepping stones; wave 2 challenges both: the gate asks for the holder's sample (R-25) and the settle takes it on the holder's tree (`Incumbent`, k = 80 — R-W4-3); slot 1's challenger reads 12 σ better → `suspicious; not reviewed`, rejected, the holder keeps its cell (R-W4-5); slot 2's reads 6.9 σ better → `improved`, replaces its holder (row, ref) and stays `discarded` / `worse`; the bench queue is drained in the scripted order; zero `kept` rows (R-W4-4) |
| `a_stale_elite_picked_as_parent_is_re_measured_first` | `remeasure_after` 1 and seven experiments later: the pick takes one fresh `Incumbent` sample (n = the slot's experiment, k = 90) in a scratch worktree that is removed; picked again at once, no second sample |

## 5. Fixture evidence

`bash bench.sh` in `fixtures/toy-route`: `CASE s11 length=7247.9 · CASE s23 length=7431.2 · CASE s37 length=7670.0 · METRIC length=7449.71 · METRIC time_ms=1.283 · ASI python=3.14.8`, rc 0 in < 0.2 s. `bash test.sh` → `test.sh: ok`. `bash holdout.sh` → `METRIC length=7759.63`. Offline reference (2-opt + or-opt + 2-opt, 40 restarts per seed, 78 s): 6189.5 / 5844.3 / 6464.9, mean 6166.2 (−17.2 % against nearest neighbour), in `fixtures/optimum.txt` outside the scope. T (three population seeds against three ratchet seeds at the same experiment budget) is the closer's live run; the spec and the control are in the fixture README.

## 6. UI test (ArchiveGrid.test.ts)

| test | proves |
|---|---|
| `bins of a key` | `i0:2.1` → [2, 1], `i3:4` → [4], `i1:` → [] |
| `distance from the champion` | 0 at the champion, 1 at the farthest, proportional between, whatever the direction |
| `grid: one section per island …` | two `data-island` sections, four `data-cell`s, exactly one `data-champion`, the star, descriptor and children in the hover text, no empty cell when the single-descriptor grid is full; with two descriptors 5 of 8 cells empty, both axis labels |
| `grid: no elite yet says so` | the empty-archive text under `data-kind="archive"` |

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

**H2 — `autoresearch.archive` (Z, R-W4-7 / R-SEAMS-27).** W4 no longer touches the rpc line: it is back to C's stub so the merge takes W1's `ArchiveQuery` actor message, which dispatches `Search::Population → population::archive(store, slug)` with the actor's store. Nothing to apply beyond keeping W1's line.

**H3 — measure slots against their parent elite (optional).** actor.rs `measure_env`: `let incumbent = if study.spec.axes.search == Search::Population { exp.parent_sha.clone() } else { incumbent_of(study)? };` and the same in `log_env`'s discard (`discard(&worktree, &incumbent, &patch)`), so a slot's row verdict reads against the tree it was cut from and `materialise` applies the patch over `exp.parent_sha` instead of the incumbent (one line in `population::materialise`, W4's).

**H4 — champion confirmation at ≤ 1 % (R-W4-1, replaces the first H4).** finalize.rs `confirm` is the only fresh re-measure any kind gets; it prints a CI and decides nothing. The rule, proved by `null_population_champion_confirmed_at_most_one_percent_under_the_t_rule` (7 / 1000 null studies confirmed; the ±1.96 σ CI alone reads a gain in 61 / 1000): a one-sided t at the realised dof. `t_quantile` is W2's `evalcore::gate::t_quantile(dof, p)`; the test carries a stand-in checked against the table (3.747 at 4 dof).

```diff
--- a/crates/autoresearch/src/finalize.rs
+++ b/crates/autoresearch/src/finalize.rs
@@ async fn confirm(&self) -> Result<Headline> {
         let (nb, nf) = (b.len() as u32, f.len() as u32);
         let ci = if nb > 0 && nf > 0 && base_mean != 0.0 {
             1.96 * sigma * (1.0 / f64::from(nb) + 1.0 / f64::from(nf)).sqrt() / base_mean.abs()
                 * 100.0
         } else {
             f64::NAN
         };
+        // 159 R-W4-1 / R-SEAMS-26: the re-measure decides. One-sided t at the realised dof
+        // (3.75 at 4 dof, 2.33 only past ~30), so a population champion — the best of many
+        // lucky draws — is claimed in ≤ 1 % of null studies; a deterministic metric confirms
+        // on the sign alone.
+        let gain = crate::gate::normalise(self.spec.metric.direction, final_mean, base_mean);
+        let confirmed = if sigma > 0.0 && nb + nf > 2 {
+            let se = sigma * (1.0 / f64::from(nb) + 1.0 / f64::from(nf)).sqrt();
+            gain / se >= evalcore::gate::t_quantile((nb + nf - 2) as usize, 0.99)
+        } else {
+            sigma == 0.0 && gain > 0.0
+        };
+        let word = if confirmed { " · confirmed" } else { " · unconfirmed" };
         let unit = &self.spec.metric.unit;
         let text = if ci.is_finite() {
             format!(
-                "{} {:.4} → {:.4} {unit} ({:+.1} %, CI ±{:.1} %)",
+                "{} {:.4} → {:.4} {unit} ({:+.1} %, CI ±{:.1} %){word}",
                 self.spec.metric.name, base_mean, final_mean, delta_pct, ci
             )
         } else {
             format!(
-                "{} {:.4} → {:.4} {unit} ({:+.1} %)",
+                "{} {:.4} → {:.4} {unit} ({:+.1} %){word}",
                 self.spec.metric.name, base_mean, final_mean, delta_pct
             )
         };
```

The word reaches every kind's headline (R-SEAMS-26 wants it for all); `Finished.tsx`'s headline test matches on the Δ, so Z checks it still passes. `t_quantile`'s argument type follows W2's signature (cast as needed).

**H4b — wave.rs `promote_env`: column patch instead of the whole-row write (R-W4-2 / R-SEAMS-25).** The same shape as W4's fix; both writes of the stale `study` copy:

```diff
--- a/crates/autoresearch/src/wave.rs
+++ b/crates/autoresearch/src/wave.rs
@@ pub(crate) async fn promote_env(env: &Env, slug: &str, wave: u32) -> Result<Promotion> {
                 exp.commit_sha = Some(new_sha.clone());
                 exp.metric = Some(mean_c);
                 exp.line = format!("{} · promoted from wave {wave}", exp.line);
                 study.incumbent_sha = Some(new_sha.clone());
                 study.incumbent_mean = Some(mean_c);
                 env.store.update_experiment(&exp).await?;
-                env.store.update_study(&study).await?;
+                env.store
+                    .patch_study(slug, &StudyPatch::incumbent(&new_sha, Some(mean_c), study.sigma))
+                    .await?;
@@
     let sigma_before = study.sigma;
     study.sigma = Some(actor::segment_sigma(env, &study).await?);
-    env.store.update_study(&study).await?;
+    env.store
+        .patch_study(
+            slug,
+            &StudyPatch {
+                sigma: Some(study.sigma),
+                ..StudyPatch::default()
+            },
+        )
+        .await?;
```

(`use crate::store::StudyPatch` joins wave.rs's imports; the `review(..)` call between the two writes patches through `actor::patch` already.)

**H4c — refuse `search: population` until H1 lands (R-W4-6, tools.rs `validate_axes`).** One line after the `parallel < 2` refusal: `if a.search == Search::Population { return Err("population search: wave promotion is not dispatched to the archive yet (159 W4 H1)".into()); }` — removed in the same commit as H1.

**H5 — reviewer at archive acceptance (W4-f, Z).** In `population::promote` after `settle` returns `Entered | Replaced` for a slot in state `Kept`: call wave.rs's `review(..)` (make it `pub(crate)`) or `review::review` directly; a `Flagged` verdict under `on_suspicious: revert` removes the cell (`archive_remove` + `refs::delete`) and marks the experiment `rejected_by_review`.

**H6 — lineage on hover (optional, Z).** `CellRow` has no parent; the grid's hover shows the cell, not the chain. Adding `parent_ref: Option<String>` to `ExperimentRow` (types.rs) lets `ArchiveGrid` take `rows` and walk `parent_ref` → cell → its experiment.

## 8. R2 — review fixes (R-W4.md)

- **R-W4-1 [BUG]** handed off + proved: the champion's confirmation is finalize's `confirm` (frozen) → H4 is now the exact diff with `evalcore::gate::t_quantile(nb + nf − 2, 0.99)`; the sim models that fixed-n re-measure: CI alone 61 / 1000, t rule 7 / 1000 (≤ 1 %). Tests `null_population_champion_confirmed_at_most_one_percent_under_the_t_rule`, `t_quantile_matches_the_table`. W4-c rewritten.
- **R-W4-2 [BUG]** fixed: `promote` ends with `patch_study(StudyPatch { incumbent_sha, incumbent_mean, sigma })`, never the whole row. Test: `a_wave_settles_into_cells_…` writes the playbook from the Bench during the stepping stone's checks and asserts it survives (fails on the old write, §3). wave.rs's two `update_study` → H4b diff.
- **R-W4-3 [RISK]** fixed: when the gate asks `incumbent_sample` against a cell's elite, `settle` takes one `Incumbent` sample on the holder's tree (`sample_tree`, k from 80) before re-asking. Test: `a_challenged_one_sample_elite_…` (both holders gain exactly one `Incumbent` row with the challenger's n).
- **R-W4-4 [RISK]** fixed: a stepping stone keeps `state`/`verdict` as the gate said; `commit_sha` carries the materialised commit; the `archive` event and the line say it entered. Tests: both e2e tests (`discarded` / `worse` after entering; zero `kept` rows after a replacement). W4-k.
- **R-W4-5 [RISK]** fixed: `Suspicious` against an elite → `Rejected("suspicious; not reviewed")`. Test: `a_challenged_one_sample_elite_…` (the 12 σ jump stays out). H5 stays.
- **R-W4-6 [RISK]** handed off: H4c (one line in tools.rs `validate_axes`, removed with H1). The e2e tests still stand H1 in by recording `wave_promoted` before the last log (`stand_in_promoted`).
- **R-W4-7 [RISK]** fixed by removal: rpc_impl.rs restored to C's stub (no second `Store`); W1's `ArchiveQuery` dispatches to `population::archive` (H2, R-SEAMS-27).
- **R-W4-8 [RISK]** not changed: migration on existing samples is a cost choice (W4-i); one extra sample per side when the gate asks is a small follow-up if islands turn out to exchange nothing in the live T run.
- **R-W4-9 [RISK]** fixed: a descriptor no sample reports → `Rejected("descriptor `<name>` not reported")`; the skill names the `case_best` index and the "print every descriptor on every run" rule. Covered by the existing `descriptor_values_…` (an unreported descriptor is absent from the map, which `settle_inner` now refuses); W4-l.
- **R-W4-10 [RISK]** fixed: the study's live `cancel_token` runs every settle sample and check, and `promote` stops settling after a Stop-now (W4-j).
- **R-W4-11 [RISK]** fixed: `incumbent_mean` = the champion's mean over every sample of its tree (W4-m).
- **R-W4-12 [POLISH]** fixed: `parent_ref` resolves against the archive as it was when the wave was cut (`before`), so a parent replaced earlier in the same settle still names the cell.
- **R-W4-13 [POLISH]** not changed: a per-commit `kept/<n>` ref is W1's R-W1-10 shape too; one convention for both archives is Z's (refs.rs is W1's file).

Commits: R1 `afd14b34`, `9a8e337e`; R2 see the final message.
