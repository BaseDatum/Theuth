# 159 C — as built (contract wave)

Branch `feat/159-variants`, worktree `theuth-159-int`, cut from main 484ae3e6. Commits: `682e2f3d` docs §18 → `2fd17d43` golden ledger (recorded on the base) → `1a70e5fe` SKELETON → Phase 2 (this note's commit). Design: `docs/design/159-autoresearch-variants.md` §5 (contract) and §18 (Revision 1, the parallel carve). Paths under `crates/autoresearch/src/` are written bare.

## 1. What was built, by design section

| § | what | where |
|---|---|---|
| 5.1 | `StudySpec` gains `axes`, `objectives`, `cases`, `judge`, `population`, `literature`, `hooks`, `world`, `suite`, `calls`, every one `#[serde(default)]` and skipped at its default; `Axes` (`ObjectivesMode`, `Scoring`, `Search`, `Artefact`); the seven empty block specs (`block_specs!`); `ModelsSpec.{target, judge}` | types.rs `StudySpec` #L36-L156, axes #L158-L240, `block_specs!` #L330-L365 |
| 5.2 | `Objective`, `ObjectiveRole`, `ObjectiveSource`; `StudySpec::{objectives_in_force, headline, normalise, rule, kind}`; the kind words `KIND_*`; `validate_spec` per combination (`validate_axes`); `normalise()` before every save (`tools::write_draft`, `actor::create`) | types.rs #L252-L330, #L390-L500; tools.rs `validate_spec` #L246, `validate_axes` #L324 |
| 5.3 | CASE v2 (names kept, ≤ 16 pairs, ≤ 2 000 ids, a one-`=` line takes the v1 path verbatim) and `SPEND` (sum, ≥ 0, finite, label ≤ 40) in `scanner::scan_line`; `Scanned.{case_values, spend_nanos, spend_labels}`; `Sample.{case_values, spend_nanos}` filled by `runner::sample`; a reported spend past `per_experiment_usd` crashes the sample (W-m) | scanner.rs #L105-L162; runner.rs #L103-L127 |
| 5.4 | `migrations/0003_variants.sql` exactly as §5.4 | — |
| 5.5 | `StudyRecord.{scores, spec_v, mechanism}`, `ExperimentRecord.{gate, bench_spend_nanos, parent_ref, wait}`, sample columns; accessors `archive_{list,put,remove}`, `judgments_{insert,list}`, `judgment_set_human`, `ratings_{put,list}`, `citations_{put,get}`, `launches_{put,get}`, `observations_{insert,list}`, `bench_spend`, `waiting_experiments`; `dangling_experiment` unchanged (three states) | store.rs `// ── 159 variants ──` |
| 5.6 | `GateStep::Decide.detail: Option<GateDecision>`, `GateStep::Wait`, `GateDecision`, `ObjectiveDelta`, `Call`, `GateRule`; `Verdict::{Traded, Pending}`; `Phase::{Cases, Judging, Window}`; every scalar decision carries `detail: None`; the decision's detail lands in `experiments.gate` | types.rs #L1054-L1150; gate.rs #L78-L84, `line` #L263-L288; actor.rs measure loop #L3018-L3132 |
| 5.7 | `StudyInfo.kind`, `StudyDetail.view: KindView` with the row types (`FrontPoint`, `CaseRow`, `SplitCounts`, `RatingRow`, `Agreement`, `CellRow`, `ArmWindow`, `LaunchRow`, `SuiteRow`); `actor::kind_view` dispatch; UI `KindPanel` switch in `StudyView.tsx`, `Scalar` = today's chart | types.rs #L1855-L2060; actor.rs #L5182; StudyView.tsx |
| 5.8 | `SpendChannel`, `BudgetUsed.by_channel` (`session`, `bench`); `study_spend` = Session + Bench; `spend_channels` | types.rs #L1183; actor.rs #L832-L865, `info_full` |
| 5.9 | `ExperimentState::Waiting`, `is_open()`, `is_dangling()`, `is_closed()`; the six "open, measuring or measured" sites routed through them (actor `is_closed`, brief `is_closed`, finalize `assemble`, stop `view`, wave `wave_closed` and `promote_env`); `PauseReason::LaunchApproval { n }`; `WaitInfo`, `WaitWhy` | types.rs #L827-L868, #L1154-L1180 |
| 5.10 | `Backend::{invoke_tool, side_call, plugin_command}` with production bodies; `review::side_call` shared with the reviewer's `call`; `runner::{CommandRun, command_run}` with `extra_env` | actor.rs #L493-L592; review.rs #L165-L201; runner.rs #L170-L212 |
| 5.11 | `METHODS` 15 → 21; six stub handlers; six trait fns + impls in `crates/rpc/src/api.rs`; 41 new types in `export.rs`; UI `api.ts` interface + sorted `METHODS` | rpc_impl.rs #L13-L36, #L181-L231; api.rs trait #L2020-L2062, impl #L7239-L7300 |
| 5.12 | golden ledger test + fixture; the P tests of §3 below | actor/golden.rs, fixtures/golden/ledger-140.json |
| 18.2 | the stub modules and files: `pareto.rs`, `refs.rs` (implemented), `cases.rs`, `judge.rs`, `population.rs`, `literature.rs`, `hooks.rs`, `world.rs`, `mechanism.rs`, `suite.rs`, `crates/app/src/launch_card.rs`, `crates/evalcore/**`, `skill/*.md` (empty), eight UI stub components | — |
| 18.3 | the registration lines, every one `// 159:<TAG>`: `kind_sample`, `kind_rule`, `parent_of`, `kind_view`, `recover` (`world::rearm`), hooks before/after, literature consult (actor.rs); `kind_section`, `Context.{hooks, protocol}` (brief.rs); `KIND_SETUP` (setup.rs); `kind_headline` (finalize.rs); `kind_line` (study_draft.rs); `explains_disabled` (binding.rs); `fire_prompt` (schedule_file.rs); `advisor_prompt` (review.rs); the six rpc stubs; `KindPanel` and `LiteratureIdeas` (UI); `pub mod launch_card` (app) | grep `159:W` |
| 18.4 | the frozen cross-wave APIs: see the SKELETON message's path#L list; `refs::{name, update, delete, list}` are implemented, `evalcore::{split, candidate_hash}` are implemented, everything else is a neutral stub | — |

## 2. Deviations (also §18.5 of the design)

- **C-a…C-e** as in §18.5: `GateRule` on the decision; `StudySpec::kind()` precedence; the golden dumps the 0002 column list; `by_channel` filled for every study; the plug-in points replace §15's "edits to shared files".
- **C-f** `refs.rs` wraps `git::plumbing` (a new pub fn in git.rs); git refuses `:` in a ref name, so a cell key `i<island>:<bin>.<bin>` is stored as `refs/autoresearch/<slug>/cell/i<island>/<bin>.<bin>` and `refs::list` maps it back (the `archive` table keeps `:`). Found by the round-trip test.
- **C-g** Per-kind `sample(env, study, req, cancel)` and `step(env, study, cfg, input, candidate, reference)`; `kind_sample` reads the ledger only for non-bench scoring; `GateStep::Wait` becomes `Verdict::Pending { until }` in the measure loop (the `Waiting` state change is `world::wait`'s).
- **C-h** `Backend::plugin_command` derives `<root>/<slug>` from the worktree path and logs under `runs/plugin/<what>-0.log`.
- **C-i** `launch_card.rs` is a card module (`lines`, `source`, `post`), not a `Tool`; app/lib.rs gets only the `pub mod` line.
- **C-j** `setup::SetupRow` + `KIND_SETUP` registry with fall-through arms in `label`, `present`, `value`, `set_leaf`, `view_of`; literature rows show when `literature` or `hooks` is set; `fire_prompt`'s root is derived from the worktree path; the protocol and hook output travel in `brief::Context`.
- **C-k** `review::side_call` is the one SideCall body; the advisor system prompt is `mechanism::advisor_prompt(..)` with the compiled fallback.
- **C-l** `evalcore` depends on `serde`, `serde_json`, `sha2`, `regex`, `async-trait` (not `runtime`): the model call is behind `evalcore::run::Model`, so the crate stays free of the actor and the runtime; the caller owns cost and `CallKind`.
- **C-m** `Cargo.lock` gained the `evalcore` entry (a new workspace member; the only lock change).
- **C-n** The W-m spend cap is in `runner::sample`, not the actor: the sample is a crash with `failure = "bench reported SPEND $x over per_experiment_usd $y"`.

## 3. Gates and results

Phase 1 (skeleton `1a70e5fe`):
- `build-lock mbx check -p autoresearch -p evalcore -p app -p rpc --locked` → rc 0, no warnings.
- `build-lock mbx test -p autoresearch -p evalcore --locked --no-run` then `mbx test -p autoresearch -p evalcore --locked` → 188 passed (autoresearch), 2 passed (evalcore), 0 failed; `golden_ledger_140` passes unchanged with 0003 applied.
- `build-lock mbx clippy -p autoresearch -p evalcore -p app -p rpc --locked` → rc 0 after two fixes (`useless_conversion`, `manual_contains`).
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; 890 binding files, `methods.json` with 21 `autoresearch.*` methods; copied to `crates/desktop/ui/src/rpc/bindings/`.
- `build-lock bun install` in `crates/desktop/ui` and `packages/apps` (`.ui-ready` was absent); `build-lock bun run check` → rc 0; `build-lock bun run test -- src/components/autoresearch src/rpc/api.test.ts` → 5 files, 37 tests passed.

Phase 2: see §6 below (filled after the runs).

## 4. New P tests (§5.12 item 4)

| test | proves |
|---|---|
| `actor::tests::golden::golden_ledger_140` | the 140 ledger dump (studies, segments, experiments, samples, ideas, steers, events; 12 experiments with a crash, a keep, a review revert, a segment, holdout runs) is byte-identical to the one recorded on 484ae3e6 |
| `types::tests::a_140_spec_serialises_without_any_159_key` | a 140 spec and its flattened `DraftArgs` write no 159 key; round trip; `kind()`/`rule()` for 140 and for a pareto spec |
| `setup::tests::fingerprints_are_unchanged_by_159_and_drafts_carry_no_159_key` | the probe fingerprint equals sha256 of the pre-C JSON; every 159 field is ignored by it; a draft `spec.json` has no 159 key |
| `types::tests::objectives_in_force_and_normalise` | `objectives_in_force()` from `metric` + guards; `normalise()` identity without objectives, mirrors headline and guards with them, idempotent |
| `types::tests::is_open_covers_waiting_and_dangling_does_not` | W-q |
| `types::tests::rule_table_and_kind_words` | the §5.6 table, the unbuilt combinations, the kind precedence, the tagged JSON forms |
| `scanner::tests::case_v2_keeps_names_and_v1_lines_unchanged` | names kept, several pairs, v1 lines unchanged, bad number, the 16-pair cap |
| `scanner::tests::spend_sums_caps_labels_and_crashes_on_bad_numbers` | SPEND sum, label cap, negative and non-numeric spend are bad numbers |
| `runner::tests::spend_sums_and_the_cap_crashes` | SPEND through a real bench run; the per-experiment cap crashes the sample (W-m) |
| `gate::tests::scalar_decisions_carry_no_detail` | `detail: None` on every scalar verdict path |
| `store::tests::migrates_and_reopens` (3 migrations), `migration_0003_applies_to_a_140_ledger_and_reads_back`, `gate_decision_round_trips` | 0003 on a 140 ledger, every default read back, every new accessor round-trips |
| `refs::tests::{names_follow_w_f, update_list_delete_round_trip}` | the ref namespace and the cell-key encoding against a real git repo |
| `evalcore::case::tests::{split_is_stable_and_near_the_shares, hash_is_sha256_hex}` | the shared split's stability, shares and seed sensitivity |
| UI `components.test.ts` "KindPanel front" | the switch renders the kind's component |

## 5. Hand-offs

- **Z** `RunTable.tsx` per-objective Δ % columns (W1), the attention row (W3), the Settings › Autoresearch pane (W7): not in the skeleton, per §18.3.
- **W6** `launch_card.rs` has no `Tool`; if the card needs a tool registration beyond `autoresearch.launch`, hand off the `RegisterTool` line for app/lib.rs.
- **W7** `mechanism::fire_prompt(root, None)` is called with the root derived from the worktree path and no pinned sha (the study record is not in `schedule_file::text`); if the fire prompt must follow `studies.mechanism`, hand off a `text()` signature change.
- **Orchestrator** `.ui-ready` never appeared in this worktree; C ran `bun install` itself in `crates/desktop/ui` and `packages/apps`.

## 6. Phase 2 gates

- `build-lock mbx test -p autoresearch -p evalcore -p app -p rpc --locked --no-run` → rc 0.
- `mbx test -p autoresearch -p evalcore -p app -p rpc --locked --no-fail-fast` → autoresearch 190 passed / 0 failed (golden ledger included); evalcore 2 / 0; rpc 56 / 0 (+ alerts 1, prompts 1; `autoresearch_methods_are_registered` covers the 21 methods); app 45 passed / 2 failed — `helper::tests::{wrong_hash_or_missing_binary_never_serves, crash_fails_that_request_and_the_next_one_respawns}`, which spawn a helper binary under a timeout. Both pass alone (`… helper::tests` → 8 passed; `wrong_hash_or_missing_binary_never_serves` 3/3 reruns ok) and touch nothing C changed; the machine's load average was 90 with seven workers building. Reported as load-flaky, not a regression. app `tests/publish.rs` 1 / 0.
- `build-lock mbx clippy -p autoresearch -p evalcore -p app -p rpc --locked` → rc 0.
- `build-lock bun run check` → rc 0 (Phase 1 bindings); `build-lock bun run test -- src/components/autoresearch src/rpc/api.test.ts` → 5 files, 37 tests passed.
- `rustfmt --edition 2024` was run only on the files C owns in Phase 2 (`runner.rs`, `setup.rs`); no wave-owned stub file was touched after the skeleton.

## 7. Open items per wave

- **W1** `pareto::*` bodies; the k-objective simulations in `gate.rs mod tests`; `FrontChart.tsx`; `skill/pareto.md`; `fixtures/toy-pareto`. `refs.rs` is implemented — W1 may extend it. Hand-off to Z: `RunTable.tsx` columns.
- **W2** `crates/evalcore` bodies (`judge`, `gate::{margin, paired, decide}`, `rows`, `run::run_case`; `split` and `candidate_hash` are done), `cases.rs`, `CaseStrip.tsx`, `skill/prompts.md`, `fixtures/toy-prompt`, the dreaming split fix (§13.2).
- **W3** `judge.rs`, `evalcore/src/pair.rs`, `RatingLadder.tsx`, `PairViewer.tsx`, `skill/judged.md`, `fixtures/toy-judged`; the attention row → Z.
- **W4** `population.rs`, `ArchiveGrid.tsx`, `skill/population.md`, `fixtures/toy-route`; consumes `refs::{update, delete, list}` (cell keys with `:` are encoded, C-f).
- **W5** `literature.rs`, `hooks.rs` (both registration lines are inert today: `hooks::{before, after}` return `Ok(None)` without `spec.hooks`; `literature::consult` runs only with `spec.literature`), `LiteratureIdeas.tsx`, `skill/literature.md`, `fixtures/literature`.
- **W6** `world.rs` (`wait`, `resume`, `rearm` are the API W7 consumes), `launch_card.rs`, `WorldChart.tsx`, `skill/world.md`, `fixtures/toy-world`; `explains_disabled` returns `false` until W6; the `rpc_impl::launch` stub.
- **W7** `mechanism.rs` (`protocol`, `fire_prompt`, `advisor_prompt` return `None` until W7), `suite.rs`, `SuiteChart.tsx`, `skill/meta.md`, the four suite fixtures; the `rpc_impl::mechanism` stub; the Settings pane → Z.
- **Every wave** the block specs (`CaseSetSpec`, `JudgeSpec`, `PopulationSpec`, `LiteratureSpec`, `HooksSpec`, `WorldSpec`, `SuiteSpec`) are empty `wire!` structs in the frozen `types.rs`: each wave writes its fields as an exact diff in its as-built "Hand-offs" (D6's addendum) for the orchestrator or Z to apply.
