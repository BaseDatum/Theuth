# 159 Z — as built (closer: merge, hand-offs, seams, gates)

Worktree `~/Develop/theuth-159-int`, branch `feat/159-variants`. Brief: `~/.theuth/briefs/2026-10-10/159/159-Z.md`. Phase I ends at the sha in the final message; Phase II (live proof) is appended below it once the orchestrator says so.

## Phase I

### 1. Merges

| merge | commit | head merged | conflicts |
|---|---|---|---|
| W1 feat/159-w1-pareto | c762e6f0 | 92661f17 (as expected) | none |
| W2 feat/159-w2-prompts | 8dfc6104 | 5ffefb37 | none |
| W3 feat/159-w3-judged | afbc11b4 | a3d73be3 | types.rs `block_specs!` (each wave deletes its own line) → union: both lines gone |
| W4 feat/159-w4-population | 1e9c43ac | 759245c1 | same, types.rs |
| W5 feat/159-w5-literature | 00ec08fa | 20bbe65e | same, types.rs |
| W6 feat/159-w6-world | 533be32f | e4fd1e59 | same, types.rs |
| main (fold) | a84f8917 | 0ac16fa3 (local main, 158 W4 landed) | none; `cargo fmt` on meeting, mcp-server, rpc, theuth touched only `crates/rpc/src/api.rs` (1b15e286) |
| W7 feat/159-w7-meta | 4caacc70 | 3c99ba39 — the sha in `W7-FINAL.txt` (the branch head was f6216180 when I started; the file's sha is the R2-fixed head with the test-timeout commit) | types.rs: the last `block_specs!` line vs W7's `SuiteSpec` → the empty `block_specs!` macro and invocation are deleted (every wave defined its own spec); the R1 comment says so |

No Cargo.toml duplicates (evalcore gained `tokio` + dev `tempfile`, dreaming gained `evalcore`; one line each). Cargo.lock merged without conflict and `--locked` builds pass, so no regeneration was needed. R-SEAMS-1 (W2's val-first `split`): only `cases.rs` and dreaming consume `evalcore::split`; W3 and W7 never call it — no id-level expectation moved.

### 2. Hand-off status

Applied = in this tree with a test where the as-built asked for one. Declined = reason given. Follow-up = listed in §6.

**C**
| hand-off | status |
|---|---|
| RunTable per-objective Δ % columns (W1 H2) | applied — `RunTable.tsx` + `model.ts objectiveColumns`: one `<name> Δ % <lower/higher>` column per objective when a shown row has `gate.rule == dominance`, cell = Δ + call word; scalar Δ stays the headline. Needed `ExperimentRow.gate: Option<GateDecision>` and `.line` on the wire (types.rs + `ExperimentRecord::row`, both `skip_serializing_if` so a 140 row is unchanged). Direction derived from sign(delta) vs sign(delta_pct) (ObjectiveDelta carries no direction) — ceiling: a 0.0 delta shows no direction word |
| Attention rows W3 (human pair) / W6 (launch) | applied in `actor::AttentionSource::items`: `autoresearch:pair:<slug>:<id>` Blocked, "Study <slug>: which blurb is better? (pair <id>)", detail `case <id>` (texts in the PairViewer: `pair_texts` needs an Env); `autoresearch:launch:<slug>:<n>` Blocked, "Study <slug>: launch #<n> needs your press · exposure <x %>", the token never in the row. Test `attention_lists_pending_pairs_and_requested_launches` |
| Settings › Autoresearch pane incl. W7 Mechanism (W7 H9) | applied — `settings/Mechanism.tsx` mounted in `Autoresearch.tsx`: active version, list (version · sha · at · summary), Show (per-file text), Adopt / Revert behind a confirm; `queries.ts useMechanism / useMechanismFiles`. Test `Mechanism.test.ts` (2) |

**W1**
| H1 front needs `parallel: 1` (tools.rs) | applied, `tools::tests::axis_seams_are_refused_with_the_reason` |
|---|---|
| H2 RunTable columns | applied (C above) |
| H3 setup `confirm` + `row_json` for kind rows | applied, `setup::tests::kind_rows_confirm_stick_and_literature_toggles_on_a_plain_draft` |
| H4 `begin_env` writes `parent_ref` | applied |
| H5 rpc_impl `archive` keeps W1's `ArchiveQuery` | kept; `pareto.rs` dispatches `Search::Population → population::archive` (R-SEAMS-27) |
| H6 `axes.*` setup leaf (optional) | applied minimal: a 140 draft reaches the `objectives` leaf through `kind_setup_for_leaf`; two lines switch it to pareto·front; no separate `axes` leaf |
| H7 per-slot front promotion in wave.rs | declined — H1 refuses the combination; follow-up |
| H8 git base = `exp.parent_sha` (R-SEAMS-7) | applied in `measure_env`/`log_env` (patch_hash, reference samples, verify_tree_for_keep, keep_pending, review revert diff/reject, discard); the Fake records `(op, base)`. Fail-before `run_and_log_diff_against_the_parent_not_the_incumbent`: `[("patch_hash","base0000"),("discard","base0000")]` |
| H9 `incumbent_sample_in_place` on the parent (optional) | declined — W1 says `decide` asks for it only when parent == incumbent; follow-up with R-W1-3 |
| H10 `t_of` → `evalcore::gate::t_quantile` (R-SEAMS-9) | applied; `t_tail`/`beta_inc`/`betacf`/`ln_gamma` deleted; `t_of_is_the_exact_student_quantile` green |
| H11 `refs/autoresearch/<slug>/kept/<n>` after keep | applied (`git::plumbing update-ref`, guarded by `is_git_repo`, failure = warn) |
| H12 `KindView::Front.directions` | declined (POLISH R-W1-12); follow-up |

**W2**
| H1 `create` requires `bench.command` only for `Scoring::Bench` (R-SEAMS-11, R-W6-3) | applied; `create` now normalises before validating (toy-pareto's objectives-only spec was refused `metric.name is empty`). One create test per kind: scalar, pareto, prompt, judged, population, world, scalar+literature+hooks, meta (`create_*` in actor/tests.rs; `create_accepts_every_kind_meta_and_pins_the_active_mechanism`). Fail-before: prompt/world `bench.command is empty`, pareto `metric.name is empty` |
|---|---|
| H2 setup `present("checks")` for Cases | applied |
| H3 `ProbeKind::Cases` | declined — a new probe kind across types.rs/setup.rs with a model call in the panel; follow-up |
| H4 `validate_spec` cases block | applied, tools test |
| H5 §13.3 landing via `dreaming::edits::land_text` | applied in `crates/rpc/src/landing.rs` (`before_merge` on `ControlOp::Merge` for `cases.source == Dreaming` + `skills/learned/<skill>.md`; writes the `NightRecord` so `dreaming_undo` finds the backup); `api.rs autoresearch_control` calls it before `rpc_impl::control`. Tests `landing::tests::{sidecar_skill_reads_only_learned_sidecars, land_is_undoable_like_a_night}` |
| H6 scratch repo for a repo-less File artefact | applied: `StudyPatch.{repo, spec}`, `create`/`validate_spec` accept an empty repo when `axes.artefact == File`, `prepare_steps` seeds `<root>/<slug>/artefact` (git init, copy `data_dir/<cases.artefact>`, commit `seed`) and patches repo+spec. Test `a_file_artefact_without_a_repo_gets_a_seeded_scratch_repo` |
| H7 skill.md judge-only bullet | applied, merged with W3's wording (prompt study or judged study) |
| H8 bindings | regenerated at the gate step |
| H9 finalize skips the bench for engine-side scoring (R-SEAMS-3) | applied: `changesets()` notes `measured by the kind's gate only`; `confirm()` verified to return at `kind_headline` before any sample. Fail-before `changesets_skip_the_bench_for_engine_side_scoring` hung in `runner::sample` (60 s+) → 1.3 s. Holdout-at-finalize Env plumbing declined (needs `Env` through `Backend::finalize`); follow-up |

**W3**
| attention row | applied (C) |
|---|---|
| `JudgmentRow.a_text/b_text` | applied (types.rs; `judge::judgments` fills them for pending human rows via `pair_texts`; PairViewer shows them, still blind — `judged.test.ts`) |
| `actor::pause` shared (R-SEAMS-24) | applied: `pub(crate) pause`; `judge::human_pair` and world.rs (`pause_for_launch`/`pause_other` deleted) use it |
| `ProbeKind::Judge` | declined (optional; needs a bound session in setup); follow-up |
| skill.md bullet | applied |
| alias `cases.path` → `judge.prompts` (R-SEAMS-23) | declined — two snapshot conventions to reconcile; follow-up |
| R-W3-8 `judge.prompts` in `evaluator_paths` | applied (tools test) |
| R-W3-9 pair from the keep path | not changed (RISK); follow-up |

**W4**
| H1 wave.rs promote dispatch (required) | applied: `Box::pin(population::promote(..))` after the study loads; the stand-ins in population/tests.rs removed, both e2e tests read the real `wave_promoted` |
|---|---|
| H2 archive line | kept W1's |
| H3 slots measured against the parent | covered by W1 H8 (`exp.parent_sha` everywhere) |
| H4 champion confirmation with the t rule (required, R-W4-1) | applied: `finalize::confirmed(direction, base, final, σ, nb, nf)` = gain/se ≥ `t_quantile(nb+nf−2, 0.99)`, σ = 0 → sign; headline text ends ` · confirmed` / ` · unconfirmed`. Fail-before `a_null_re_measure_is_unconfirmed_and_a_clear_gain_confirmed` (no word before). **Null-champion simulation re-run with `finalize::confirmed`: CI-alone 61/1000, confirmed 7/1000 = 0.7 % (≤ 1 %)**; the stand-in quantile in population/tests.rs deleted, `t_quantile_matches_the_table` checks evalcore's. A `Headline.confirmed` flag was not added (the word in the text serves every kind; R-SEAMS-26) |
| H4b wave.rs column patches (required, R-W4-2 / R-SEAMS-25) | applied (`patch_study` for incumbent and σ; `review`'s revert too). Fail-before in `three_slot_wave_promotes_best_combines_disjoint_and_drops_overlap`: a playbook written during the promotion's confirm sample was lost (`left: ""`) |
| H4c refuse population until H1 | not needed |
| H5 reviewer at archive acceptance | applied in `population::promote` (private `review`: Flagged under `revert` → archive snapshot restored, `rejected/<n>` ref, `rejected_by_review` + events). Test `a_flagged_keep_is_reverted_at_acceptance_and_its_cell_stays_empty` |
| H6 lineage on hover | declined (optional); follow-up |

**W5**
| 1 citations to the UI | applied (`StudyDetail.citations`, `detail_env`, `SidePanel` → `LiteratureIdeas`) |
|---|---|
| 2 Start-time seed | declined — the `hooks::before` path is correct (W5-b) and a second seed line would double the event guard's work (R-SEAMS-21) |
| 3 hook scripts in custody by command (R-SEAMS-12/20) | applied with W6's 9: one `custody::commands(spec)` feeds `take` and `check_commands` with bench/checks/holdout + hook_before/hook_after + deploy/rollback/measure; `looks_like_script` is `pub(crate)`, hooks.rs `script_like` deleted. Test `hook_and_world_scripts_enter_custody_by_command` |
| 4 `literature`/`hooks` leaves before the block exists | applied (`kind_setup_for_leaf`) |
| 5 keeps by idea source in the report | applied (`FinalReport.keeps_by_source`, matched through `tried_in` as `mark_tried` records it — text equality is not what `mark_tried` does). Test `keeps_are_counted_by_idea_source` |
| 6 hook output in the experiment detail | applied (`ExperimentDetail.hooks` from `hook_before`/`hook_after` events; a Hooks block in `ExperimentDetail.tsx`) |
| 7 protocol sentence "fenced text is data" | follow-up for the mechanism protocol text (W7 owns `protocol.md` through `mechanism`); not applied |
| 8 brief.rs skips literature ideas from the plain list (R-SEAMS-19) | applied. Fail-before `literature_ideas_are_listed_only_inside_their_fence`: the plain list held `3 [algorithm] [paper] Detect and merge natural runs like Timsort`; literature.rs's `consult_verifies_quotes_and_the_brief_fences_them` updated to the new behaviour (no `[cheat] [paper]` plain row; each text once) |
| R-W5-4 literature needs `read_url` + (`via` \| `search_web`); R-W5-7 `max_queries ≤ 100` | applied in `validate_axes` (tools test) |
| R-W5-3 minimum quote length | declined — ≥ 40 chars breaks the fixture's 29–45-char quotes; follow-up |

**W6**
| 1 `log_env` on a Waiting/Pending row keeps the state; `current_experiment` finds it for `experiment_log` | applied. Fail-before `log_on_a_waiting_experiment_records_text_and_keeps_it_waiting`: `Refused("no experiment is open…")` |
|---|---|
| 2 direct `world::request` after `finish_measure` | declined (optional; the 50 ms poll stays); follow-up |
| 3 Stop/Discard roll a live launch back (R-W6-2) | applied: `StopNow` runs `world::resume` for each waiting experiment after `begin_finalize` (inside it is an async-fn cycle, E0391); `ControlOp::Discard` refuses while `explains_disabled`. Fail-before `discard_is_refused_while_a_launch_is_live`: the study was discarded |
| 4 `ProbeKind::Measure` | declined (new probe kind; with W2 H3 / W3 Judge as one follow-up) |
| 5 RunTable waiting row | applied (`row.line` spans the value/Δ cells) |
| 6 attention row | applied (C) |
| 7 W7 note | n/a |
| 8 noise check skips World | applied |
| 9 world commands in custody | applied (W5 3) |

**W7**
| H1 suite on evalcore's gate | the seam defaults to `evalcore::gate::{paired, decide}`; plus R-SEAMS-15 / R-W7-3: `suite::step` turns an `Accept` into `Reject("paired t … below c_n …")` whenever `paired().t < critical` (n ≥ 2), so a 6–8-pair suite never keeps on the margin rule alone. Fail-before `a_margin_only_accept_is_no_gain_without_the_paired_t` with R-W7-3's six deltas: `Improved` → `NoGain`; `step_asks_for_the_incumbent_rerun_and_maps_the_decision` now uses zero-spread pairs (said in the test) |
|---|---|
| H2 pin the active mechanism at Start; `ensure` for a meta draft | applied (`create`, `setup::view_of`); the meta create test checks the pin (R-SEAMS-17) |
| H3 W6 Waiting path for the suite | declined — not clean: `world::resume` has no kind dispatch and `finish_measure` cannot be entered from `Waiting` (C-g); the inner studies run inside the outer sample (W7-b). Follow-up |
| H5 toy-pareto / toy-route in `suite::FIXTURES` (R-SEAMS-16) | applied: `Fixture.{metric, guard}`, `inner_spec` derived from the fixture; toy-pareto scalar on `time_ms` with a `size_kb` guard, toy-route on `length`; both `holdout.sh` read `AUTORESEARCH_HOLDOUT_SEED`; references embedded, never materialised; READMEs corrected. `inner_repos_carry_no_answer_key…` extended to every fixture |
| H6 `mechanism::overlay` readers | applied: `actor::mechanism_cfg` at brief render (caps, crash_retries, code_map_tokens), run_env, measure_env novelty threshold, `stop::evaluate`; wave.rs `assign_categories` seeds from `params.categories`. Test `an_adopted_mechanism_changes_what_the_brief_and_novelty_readers_see`; `readers_are_silent_until_a_version_is_adopted` green |
| H7 hide `meta:` studies | applied: `StudyInfo.created_by` on the wire; `StudyList.tsx` hides `meta:` rows behind a "Show N inner studies" toggle (`StudyList.test.ts`) |
| H8 `review::advise` root | applied (reads `autoresearch.root` from the config as finalize.rs does; passes the study's pin) |
| H9 Settings Mechanism pane | applied (C) |
| H10 toy-sort `holdout.sh` = `TOY_SORT_HOLDOUT` | applied; identical CASE labels at seed 0 |
| H11 suite cost in `validate_axes` | applied (`validate_spec_refuses_a_suite_run_over_the_per_experiment_cap`) |

### 3. R-SEAMS [BUG-class] and the waves' [BUG] findings

| finding | state |
|---|---|
| R-SEAMS-3 finalize runs the bench for engine-side kinds | fixed (W2 H9 above), fail-before test |
| R-SEAMS-7 git base is the incumbent, not the parent | fixed (W1 H8), fail-before test |
| R-SEAMS-11 `create` refuses every kind without `bench.command` | fixed (W2 H1), fail-before create tests |
| R-SEAMS-19 literature ideas unfenced in the plain list | fixed (W5 8), fail-before test |
| R-SEAMS-22 W3 used `run_case` as a generator | refuted by W3's R2 (R-W3-4: `Model::call` directly; `judge.rs` has no `run_case` call) |
| R-SEAMS-25 stale-copy `update_study` in promotions | fixed (W4 H4b), fail-before test |
| R-W1-1 | = R-SEAMS-7 |
| R-W2-1, -2, -3 | fixed by W2 R2 with tests (`leak_refuses_pasted_answers…`, `spend_charge_stops_at_the_cap`, CheckCtx); R-W2-4 = R-SEAMS-3 |
| R-W3-1…4 | fixed by W3 R2 with tests (blind PairList, `outputs_dir_for`, `pair::task` carries the input, direct generation) |
| R-W4-1, -2 | fixed here (H4, H4b); the sim rate is 0.7 % |
| R-W5-1, -2 | W5 R2 (`[paper]` prefix, `fence_safe` passages) + R-SEAMS-19 here |
| R-W6-1, -2, -3 | W6 R2 (CAS press, `rollback_failed`) + W6 3 and W2 H1 here |
| R-W7-1, -2 | fixed by W7 R2 with tests (no answer key in inner repos; cap and spend on every path) |

R-W6-3's "every kind creatable": eight `create_*` tests in actor/tests.rs (scalar, pareto, prompt, judged, population, world, literature+hooks, meta).

### 4. Seams wired
- W7's suite → `evalcore::gate::{paired, decide}` + the t requirement (above).
- `autoresearch.archive` → W1's `ArchiveQuery`, dispatching `Search::Population → population::archive`.
- Waiting / `is_open()`: `experiment_log` is the one new Waiting-aware site (W6 1); promotion loops abandon `is_dangling()` slots only, so a Waiting slot stays waiting; every other `Open|Measuring|Measured` match in actor.rs is a specific transition and was left (ActorHandoffs audit, item 13).
- Headlines and card lines: every `// 159:W<n>` arm (finalize `kind_headline`, study_draft `kind_line`, brief `kind_section`, setup `KIND_SETUP`, rpc_impl) calls a real body; the only `NotBuilt` left are the per-kind `sample`/`step` stubs of pareto.rs and population.rs, which are not scoring/rule kinds.

### 5. Gates (BUILD_LABEL=159-Z, -p autoresearch -p evalcore -p dreaming -p app -p rpc)

| gate | result |
|---|---|
| `build-lock mbx test … --locked --no-run` | ok, 0 warnings |
| `mbx test … --locked --no-fail-fast` | autoresearch **303 / 0** (golden_ledger_140 and the gate simulations included), evalcore 15 / 0, dreaming 64 / 0, app 49 / 0 + publish 1 / 0 (no helper timeout this run), rpc 58 / 0 + alerts 1 + prompts 1. One flake on the second full run: `rpc engine::tests::second_acquire_in_one_process_is_running` ("engine.lock is held but engine.json is missing") — a lock-file race in a 141 file 159 does not touch; green on the first full run and alone (4 / 0) |
| `build-lock mbx clippy … --locked --tests` | 0 warnings after six fixes in the merged tree (`measure_env` 8 args → `mechanism_cfg` computed inside; `rfind`; two test-only `MutexGuard` scopes, a `needless_update`, a `manual_range_contains`) |
| `cargo fmt --check` autoresearch, evalcore, dreaming, app, rpc, theuth | clean |
| UI `build-lock bun run check` | clean (after `build-lock bun install`: the main fold's `smol-toml` was missing from node_modules) |
| UI `bun run test -- src/components/autoresearch` + `settings/Mechanism.test.ts` | 13 files, **62 / 0** |
| UI full vitest (`BUILD_SLOTS=1`) | 265 files, **1677 / 0** |
| `BUILD_SLOTS=1 build-lock mbx check --workspace --exclude desktop --locked` | ok, 0 warnings |
| TS bindings | regenerated (`export_bindings` 8 / 0) and copied; the new fields come out optional (`gate?`, `line?`, `citations?`, `hooks?`, `a_text?`, `created_by?`) and the UI type-checks against them |

**Golden ledger stack.** After the W7 merge `golden_ledger_140` overflowed libtest's 2 MiB thread (passes at 3 MiB; MetaHandoffs measured the futures — begin_env 34 KB, log_env 34 KB — so it is the debug poll-frame depth of begin → brief → kind sections → log → gate → finalize once all seven kinds dispatch from one place, not one grown future; `Box::pin` on the arms did not move it). Tokio's workers also default to 2 MiB, so the engine was one deep handler away from the same abort: `crates/theuth/src/main.rs` now builds the runtime with `thread_stack_size(8 << 20)` (reserved, not committed), and the golden test runs its scripted study on an 8 MiB thread with a current-thread runtime — the same stack the engine gives it. Fail-before: `thread 'golden_ledger_140' has overflowed its stack` at 2 MiB on 4caacc70.

§15 must-not-regress: golden ledger green; gate simulations (`gate::tests`) green inside the 303; custody refusal, closed toolset, holdout-never-in-brief and money-stop tests are in the autoresearch suite and green; the Windows refusal is untouched (lib.rs).

### 6. Follow-ups (RISK and POLISH, not fixed now)
R-SEAMS: RISK 1 2 4 6 8 9(done) 12(done) 13 16(done) 17(done) 20(done) 23 26(word only) 27(kept) 28 · POLISH 5 10 14 18 21 24(done).
R-W1: RISK 2(W1 fixed) 3 4 5 6(done) 7(done) 8(W1 fixed) 9 10 · POLISH 11 12 13 14.
R-W2: RISK 5–15 · POLISH 16–22.
R-W3: RISK 5(W3 fixed) 6 7(W3 fixed) 8(done) 9 · POLISH 10 11.
R-W4: RISK 3–11 (W4 fixed 3 4 5 7 9 10 11; 6 done here; 8 open) · POLISH 12 13.
R-W5: RISK 3 4(done) 5(W5 fixed) 6 7(done) 8(W5 fixed) · POLISH 9 10 11.
R-W6: RISK 4(W6 fixed) 5–13 · POLISH 14 15 16.
R-W7: RISK 3(done) 4–10 · POLISH 11 12 13.
Open hand-offs: W1 H7/H9/H12, W2 H3, W3 Probe/alias/R-W3-9, W4 H6, W5 2/7 + R-W5-3, W6 2/4, W7 H3; the finalize holdout-at-finalize Env plumbing (W2 H9 second half); W7's `protocol.md` sentence (W5 7); R-SEAMS-13 (W6's off-mailbox ledger writes are CAS-only today — documented, not re-routed).
