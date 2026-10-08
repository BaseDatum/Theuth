# 140 — as-built, F1 FIX (the actor core: actor.rs, actor/tests.rs, lib.rs, store.rs, types.rs)

Branch `feat/140-f1`, cut from the integration head 78411fb3. Work list: `~/.theuth/briefs/2026-10-08/140/review-R.md` (independent review 140-R). Owned files: `crates/autoresearch/src/{actor.rs, actor/tests.rs, lib.rs, store.rs, types.rs}` (+ migrations; none added). No other file was touched.

## What changed, file by file

### `src/store.rs`
- **`StudyPatch`** (column-scoped study update, R-4): `Option` per mutable column, `patch_study(slug, &p)` / `patch_study_if(slug, from_state_tags, &p) -> bool` built with `QueryBuilder`; `StudyPatch::state(..)`, `::incumbent(sha, mean, sigma)`, `apply_to(&mut StudyRecord)` keep a handler's copy in step. `update_study` (whole row) stays for the row-owning paths (`binding::on_fired`, `wave.rs`); nothing in actor.rs calls it any more.
- **`update_experiment_if(e, from_states) -> bool`** and **`claim_experiment(slug, n, from, to) -> bool`** (R-4/R-60): conditional close-outs and the `measuring` claim. `update_experiment` delegates to the shared `experiment_update` builder.
- **`record_keep(exp, sha, study_patch)`** (R-50/R-109): retag samples + experiment row + study incumbent + `keep {n, sha}` event in one transaction.
- **`update_sample(id, &Sample)`** (R-34): fills a pending row; `insert_sample` doc says what a pending row is.

### `src/types.rs`
- `StudySpec.include_dirty: bool` (R-11; serde default false).
- `Guard.direction: Direction` (R-19; default `lower`). **Not yet read by `gate::guard_failures`** — see hand-offs (the `GateCfg.guards` tuple is destructured in F2's gate.rs; changing it in parallel would break F2's build).

### `src/actor.rs`
- **Backend**: `resolve_base(.., include_dirty)`; new `apply_patch`, `discard_study`, `commit_parent` (one read-only `git rev-parse` — R-18 hand-off to F2 for a git.rs read helper), `report_prose` (a `runtime::SideCall` charged to the study session), `replace_report` (`document::ImportMarkdown`), `session_cost(..) -> Result<f64>` (returned + `estimated_for_unavailable`, `Err` when the agent actor / session is unknown); `spend_usd` kept as `session_cost().unwrap_or(0.0)` only because `wave.rs:516` calls it; `record` removed (R-62: `finalize::run` writes the record).
- **Helpers**: `bench_values` (no holdout rows), `incumbent_samples(env, study, sha)` (no holdout, `at ≥ segment.opened_at`), `segment_sigma` leaves out `checks_failed` candidates, `last_closed_n`, `study_spend` / `study_spend_or_closed`, `set_state_if` (CAS on the state tag), `patch(env, study, StudyPatch)`, `scored_sample` (pending row → sample → fill; progress re-published every 3 s while sampling), `incumbent_sample_in_place` (candidate saved as `runs/<n>/stash-<k>.patch` → reset to the incumbent → sample → re-apply), `absorb_incumbent_sample` (drift check + x̄_i/σ patch for *every* incumbent sample), `adopt_pending_keep`, `run_once_pending`, `tree_changed_pending`, `verify_tree_for_keep`, `brief_context`, `append_block`, `apply_rules_from_turn`, `begin_finalize_with` / `spawn_finalize` / `finalize_deferred`, `report_turn`.
- **Create/prepare**: `is_git_repo` through `spawn_blocking` (R-100); every write a `StudyPatch` (R-4); custody taken from the **worktree** (R-7); `baseline` skips `noisy` samples and refuses with fewer than two clean ones (R-32); holdout through `scored_sample` (R-42).
- **Begin**: paused studies accept only a fire with a `run_once` event newer than the last `begin` (R-51); spend is `study_spend()?` — unreadable → the fire is refused (R-14); `adopt_pending_keep` before the drift repair (R-50); a dangling `measuring` experiment is cancelled and its samples marked lost (R-49), its change saved as `runs/<n>/candidate.patch` (R-63), the close-out conditional (R-4); scheduled re-measure counted by `remeasure` events, drift check on every incumbent sample (R-20); `begin` event stores the live tree's spend; `brief_context` shared with wave slots (R-73) with `code_map(root = worktree, focus = scope paths without glob tails)` (R-76); wave blocks appended inside `brief.max_bytes` (R-74).
- **Run**: `runs_so_far` via `u8::try_from` (R-107); `Measured` + a `tree_changed` event newer than the last `run` re-opens the run (R-1); the `measuring` claim is conditional (R-60); the body is `measure_env`, and on `Err` a cancel closes the row `interrupted`, anything else hands it back `open` with the pending sample lost (R-52); `patch_hash` errors propagate (R-102); `mark_tried` after the refusals (R-69); candidate tree `exp:<n>` or `exp:<n>r<k>` on a re-run; samples through `scored_sample` with live-tail ticks (R-34/R-87); the confirm-phase incumbent sample measures the incumbent tree under `k = 101, 102, …` (R-2/R-23/R-25) and is absorbed with the drift check (R-20); incumbent reads through `incumbent_samples`/`bench_values` (R-3/R-28); `guard failed: <name> +x % > y %` in the line (R-35); crash line says "exit 0 but no usable `METRIC` line" (R-41); checks under `k = 1` (R-36); σ patched, not the row (R-4); `finish_measure`/`interrupted` conditional (R-4).
- **Log**: before any keep (incl. keep-simpler) `verify_tree_for_keep`: custody or off-limits violated → the experiment closes `refused` (custody counted), patch hash differs → `tree_changed` event + the error "run experiment_run again" (R-1); `keep_simpler_granted` brings the candidate to `1 + confirm_runs` samples before judging on means, checks under `k = 2` (R-21/R-36); keep = `keep_pending` event → `git keep` → `record_keep` transaction (R-50/R-109); revert writes with `?`, `patch_path` only after the write (R-103); discard failure returns `Err` and leaves the experiment `measured` (R-104); cost = live − `begin` mark, a `closed {n, spend_usd}` mark hands the rest to `study_spend` (R-14); overfit payload carries `after_n` (R-44); a stop from Log defers §11 to the turn's end (R-56).
- **Rules / pause / finalize**: `explore`/`resumed` `after_n` = highest closed `n` (R-64); `advisor_failed` event (R-75; brief.rs clause → F3); `pause` is a no-op when already paused and a CAS from `running` (R-46/R-4); `resume` patches state + `paused_ms` with a CAS from `paused`; `begin_finalize` CAS from running/paused, `finalize_deferred` event + a 10-minute fallback timer, `spawn_finalize` single-flight (`finalize_started` event + an in-process set); `finalize_task` retries a store error 3× (R-105), resets the worktree to the incumbent first (R-56), runs `report_turn` (R-45: prose from the executor via a side call, then `# title / prose / --- / tables` into `report.md` and the report document), patches `finished_at`/`report_doc`/state (R-4); the second knowledge-record write is gone (R-62).
- **Control** (`control_env`, off-mailbox — R-115): StopNow waits ≤ 15 s for a measuring run to close itself before the reset (R-58), `abandoned` unless measuring (R-66), git through `spawn_blocking` (R-100); RunOnce records `run_once` (R-51); Merge through `spawn_blocking`; Discard calls `git::discard_study` (worktrees, `autoresearch/<slug>`, `+k` branches, rejected refs) and records `discarded {removed}` (R-9).
- **Segment**: custody from the worktree (R-7), column patches (R-4).
- **Reads** (`status_env`, `detail_env`, `experiments_env`, `experiment_env`, off-mailbox — R-115): holdout chart series one point per `n` (R-89); needs-eyes line without " · Restore" (R-96); experiment detail's `cases_moved` from `incumbent_samples` (R-3); `info(root, study)` (R-110).
- **Cards**: real links `[/autoresearch/<slug>](…)`, `[/notes/<doc>](…)`; a file-path `report_doc` is only named (R-98).
- **recover**: `interrupted` only from `measuring` (else `abandoned`), publishes `ExperimentFinished` and runs the stop rules (R-65); finalizing studies resume through `spawn_finalize`.

### `src/lib.rs`
- `Control`, `Status`, `Detail`, `Experiments`, `Experiment` reply through `DelegatedReply` off the mailbox (R-115).
- `SetSettings` refuses a `root` change at runtime (R-101).
- `enabled: false` → `disable_study_files(data_dir)` flips `enabled: false` on every `schedules/autoresearch-*.md` (R-6, through `schedule::file::set_enabled` + `write_atomic`).
- A failed `start()` registers nothing (R-117). `install_skill`'s dead empty-bundle guard removed (R-120; `first_run` still does not write this skill — documented in the doc comment).

## Findings in scope → disposition

| id | sev | disposition |
|---|---|---|
| R-1 | BUG | fixed — `verify_tree_for_keep`, `tree_changed` re-run; test `keep_refuses_a_tree_changed_after_the_measurement` |
| R-2 | BUG | fixed — `incumbent_sample_in_place` (candidate stashed as a patch, same worktree so build caches stay warm; a detached worktree would measure a cold build); test `confirm_incumbent_sample_measures_the_incumbent_tree` |
| R-3 | BUG | fixed — `bench_values` / `incumbent_samples` at every incumbent read, `cases_moved` filters role; test `holdout_values_never_enter_incumbent_statistics`. brief.rs:75-78, 595 → hand-off F3 |
| R-4 | BUG | fixed — `StudyPatch`, `patch_study_if`, `update_experiment_if`, `claim_experiment`; no whole-row study write left in actor.rs; test `pause_during_a_run_survives_the_runs_write_back`. `binding.rs:79` (`update_study` for the session) → hand-off F2: `store.patch_study(slug, &StudyPatch { session: Some(Some(session)), ..Default::default() })` |
| R-6 | BUG | fixed — `disable_study_files`; test `disabled_plugin_pauses_study_schedule_files` |
| R-7 | BUG | fixed — custody from the worktree in `prepare_steps` and `segment_task`; test `create_and_discard_use_the_worktree_and_the_repo_cleanup` |
| R-9 | BUG | fixed — `Backend::discard_study` in the Discard arm; same test |
| R-10 | BUG | handed off to F3 (first path tools.rs; study_draft.rs). The actor has no provenance to act on: `StartDraft` is the message both the model tool and `autoresearch.create` send. F3: drop `start`/`start_window` from `autoresearch_control`, route the card press through `autoresearch.create` |
| R-11 | BUG | fixed — `StudySpec.include_dirty` threaded to `resolve_base`; same test. Draft-card choice → hand-off F3 |
| R-14 | RISK | fixed — `session_cost -> Result`, `study_spend` = Σ closed `cost_nanos` + live tree since its `begin`/`closed` mark; begin refuses on `Err`; tests `begin_refuses_when_the_spend_is_unknown`, `study_spend_sums_closed_rows_plus_the_live_tree`. `wave.rs:516` → hand-off F2: `actor::study_spend(env, &study).await?` (then `spend_usd` can go) |
| R-19 | BUG (gate.rs first) | my side: `Guard.direction` on the type. Hand-off closer/F2: `GateCfg.guards: Vec<(String, Effect, Direction)>` (types.rs, mine — I did not change it to keep F2's gate.rs compiling in parallel), `gate_cfg` fills it from `Guard.direction`, `gate::guard_failures` normalises with the guard's direction |
| R-20 | BUG | fixed — `remeasure` events, `absorb_incumbent_sample`; test `scheduled_remeasure_is_counted_by_its_own_event` |
| R-21 | BUG | fixed — confirm samples inside `keep_simpler_granted`; test `keep_simpler_takes_confirm_samples_before_judging` |
| R-23 | BUG | fixed — `k = 101…` for incumbent samples (asserted in the R-2 test) |
| R-28 | RISK | fixed — `incumbent_samples` filters `at ≥ opened_at` |
| R-32 | RISK | fixed — `baseline` skips noisy, refuses < 2 clean |
| R-33 | RISK (slot.rs first) | handed off to F2: fill `Sample.asi["slot_holders"]` in `runner::sample` from `slot::build_slot_holders` (no type/schema change; the ASI map already reaches the run report). A `Sample` field would break `runner::sample`'s exhaustive literal in F2's file |
| R-34 | RISK (runner.rs first) | fixed on my side — pending row before the spawn (`scored_sample`), `update_sample` after; the restart test now drives the production path (a hanging benchmark) instead of a fabricated row |
| R-35 | POLISH | fixed — guard names in the line |
| R-36 | POLISH | fixed — smoke `k=0`, checks `k=1`, keep-simpler checks `k=2` |
| R-40 | POLISH | fixed — `segment_sigma` excludes `checks_failed` candidates |
| R-41 | POLISH | fixed — exit-0-no-value crash line names the cause (`Scanned.bad_number` itself stays in runner.rs, F2) |
| R-42 | POLISH | fixed — Begin re-measure and holdout through `scored_sample` → `bench_sample` |
| R-43 | POLISH | fixed — σ_holdout = 0 falls back to the search σ, else no verdict |
| R-44 | BUG | fixed — `after_n` in the overfit payload; view-level test `overfit_event_feeds_the_stop_view` |
| R-45 | BUG | fixed — `report_turn`: one executor side call (brief + tables → prose), document = prose then tables; test `finalize_writes_the_report_prose_before_the_tables`. finalize.rs:7's claim → F2/doc |
| R-46 | BUG (stop.rs first) | my side fixed: `pause` is a no-op when paused. `stop::hours_running` subtracting `now − paused.at` while paused → F2 |
| R-48 | BUG (binding.rs first) | not changed here: `on_turn_finished` is F2's; the advisor consult stays inside `apply_rules`. F2 can `tokio::spawn(actor::after_experiment(env, slug))` from `on_turn_finished` |
| R-49 | BUG | fixed — cancel + `mark_lost` in Begin; test `begin_cancels_a_dangling_measurement` |
| R-50 | BUG | fixed — `keep_pending` → commit → `record_keep` transaction; `adopt_pending_keep` at Begin; test `keep_commit_without_ledger_is_adopted_at_begin` |
| R-51 | BUG | fixed — `run_once` event gate; test `paused_study_accepts_only_a_run_once_fire` |
| R-52 | BUG | fixed — `measure_env` wrapper; test `failed_measurement_reopens_the_experiment` |
| R-56 | RISK | fixed — finalize deferred to the turn's end (`finalize_deferred` + `on_turn_finished` → `stop_after_pending` → `begin_finalize`; 10-minute fallback) and `repair` before confirm; asserted in `experiment_budget_stops_and_finalizes_with_the_result_card_data` |
| R-58 | RISK | fixed — StopNow waits for the run's own close-out before the reset |
| R-60 | RISK | fixed — `claim_experiment` (asserted in the R-52 test) |
| R-62 | POLISH | fixed — actor's second record write removed |
| R-63 | POLISH | fixed — dangling change → `runs/<n>/candidate.patch` (asserted in the R-49 test) |
| R-64 | POLISH | fixed — `after_n` = max closed `n` |
| R-65 | POLISH | fixed — `recover` publishes and runs the rules |
| R-66 | POLISH | fixed — `abandoned` unless measuring; test assertion corrected |
| R-69 | BUG | fixed — `mark_tried` after the refusals; test `refused_runs_do_not_mark_ideas_tried` |
| R-73 | RISK | fixed — `brief_context` for slots |
| R-74 | RISK | fixed — `append_block` trims into the cap |
| R-75 | RISK | fixed — `advisor_failed` event; brief.rs clause → hand-off F3 (drop "the ideas below include the advisor's suggestions" when the newest `advisor_failed` is newer than the `explore`) |
| R-76 | RISK | fixed — worktree root, glob tails stripped |
| R-87 | RISK | fixed on my side — progress re-published every 3 s while a sample runs (the UI already refetches the detail, whose `tail` is the newest log). UI polling/`jobTail` → F3 |
| R-89 | RISK | fixed — one holdout point per `n` (mean). `model.ts` `holdoutSummary` → F3 |
| R-96 | POLISH | fixed |
| R-98 | POLISH | fixed — real links; file-path report only named. `finalize.rs:1001-1003` returning a path as `doc` → F2 (`None`) |
| R-100 | RISK | fixed — `spawn_blocking` for merge / discard / stop-now reset / `is_git_repo`; reads and control off the mailbox |
| R-101 | RISK | fixed — root change refused; test `settings_refuse_a_root_change_at_runtime` |
| R-102 | RISK | fixed |
| R-103 | RISK | fixed (wave.rs:695-699, 892-898 same pattern → F2) |
| R-104 | RISK | fixed |
| R-105 | RISK | fixed |
| R-107 | POLISH | fixed |
| R-109 | POLISH | fixed — `record_keep` transaction |
| R-110 | POLISH | fixed |
| R-111 | POLISH | fixed — warn text matches R-51's rule |
| R-115 | RISK (binding.rs first) | my side fixed: Control/Status/Detail/Experiments/Experiment off the mailbox. `binding::on_fired`'s `SetToolset` ask → F2 |
| R-117 | RISK | fixed |
| R-120 | POLISH | fixed — dead guard dropped; `first_run` → closer (main.rs) if the skill should be written while disabled |
| R-38 | POLISH | declined — `BenchSpec`'s serde default cannot see the metric unit; the draft (`study_draft.rs:160`) already derives `exclusive` from `is_time_unit`, so the only path that gets the unconditional default is a hand-written spec |
| R-85 | BUG (StudyView first) | actor side fixed: `StopNow` on a `preparing` study cancels the baseline (the study ends `failed` with the cancellation as reason) instead of being refused; the button can stay |

## Gates (through build-lock, stable 1.96.1)

- `cargo test -p autoresearch`: **127 passed, 0 failed** (the 110 inherited tests incl. W5's wave end-to-ends + 17 new: `keep_refuses_a_tree_changed_after_the_measurement`, `confirm_incumbent_sample_measures_the_incumbent_tree`, `holdout_values_never_enter_incumbent_statistics`, `pause_during_a_run_survives_the_runs_write_back`, `begin_refuses_when_the_spend_is_unknown`, `study_spend_sums_closed_rows_plus_the_live_tree`, `scheduled_remeasure_is_counted_by_its_own_event`, `keep_simpler_takes_confirm_samples_before_judging`, `overfit_event_feeds_the_stop_view`, `finalize_writes_the_report_prose_before_the_tables`, `begin_cancels_a_dangling_measurement`, `keep_commit_without_ledger_is_adopted_at_begin`, `paused_study_accepts_only_a_run_once_fire`, `failed_measurement_reopens_the_experiment`, `refused_runs_do_not_mark_ideas_tried`, `create_and_discard_use_the_worktree_and_the_repo_cleanup`, lib `settings_refuse_a_root_change_at_runtime`, `disabled_plugin_pauses_study_schedule_files`; `restart_recovery_marks_interrupted_and_lost` rewritten to drive the production path). Each new test drives the exact path the review cited and pins the behaviour 78411fb3 lacked (they do not compile against the old `Backend`, so "fails before" is by construction, not by a run).
- `cargo clippy -p autoresearch --all-targets -- -D warnings`: clean.
- `cargo fmt -p autoresearch` on my five files: clean (`rustfmt --check`).

## Deviations

1. **R-2 measures the incumbent in the study worktree, not a detached one.** The candidate (tracked + untracked) is saved as `runs/<n>/stash-<k>.patch` with `git::discard`, the tree reset to the incumbent, the sample taken, the patch re-applied. A fresh detached worktree has no `target/`/`node_modules/` and would measure a cold build. If re-applying fails the run errors naming the stash path (the experiment is handed back `open`).
2. **R-56 defers finalize to the turn's end** instead of cancelling the study turn: `experiment_log` returns `stop: Some(..)`, the model's turn ends, `binding::on_turn_finished` → `stop_after_pending` (true for a deferred finalize) → `begin_finalize` → `spawn_finalize`. A missed `TurnFinished` is covered by a 10-minute fallback. `finalize_task` also runs `git::repair` first so a post-log edit is saved as `finalize/stray.patch`, never measured.
3. **R-14's study total** is Σ closed `cost_nanos` plus the live tree's spend since its last `begin`/`closed` mark (both marks store the tree's total). Pre-begin tokens of a fire are attributed to the next close; a re-created session reads below its mark and adds nothing (small undercount in that minute, never a double count).
4. **R-45 is a side call, not a schedule-session turn.** `begin_finalize` already ended the loop (`schedule::Done`/`Delete`), so there is no live study turn to send the protocol to; the executor model answers one `SideCall` charged to the study session (as the reviewer does) and the plugin inserts the tables after the prose. Without a bound session or agent actor the tables stand alone (warned).
5. **R-50 adoption** needs the commit's parent; `Backend::commit_parent` is one read-only `git rev-parse <sha>^` in actor.rs because git.rs's read helpers are private (R-18 is F2's).
6. **`session_cost` without an agent actor is `0`, not an error**: no agent actor means no model call can be made, so nothing is spent; a failed `SessionCostQuery` or an unknown session is still `Err` and refuses the fire. (W5's wave tests run without the agent actor and would otherwise never begin.)

## Hand-offs

- **F2 (`binding.rs`)**: R-4 `on_fired` → `store.patch_study(slug, &StudyPatch { session: Some(Some(session)), ..Default::default() })`; R-48 spawn `actor::after_experiment`; R-115 `SetToolset` through `reply_sender`/spawn.
- **F2 (`wave.rs`)**: R-14 `actor::study_spend(env, &study).await?` at line 516 (then delete `Backend::spend_usd`); R-3 line 727 `actor::incumbent_samples(env, &study, incumbent)` instead of `samples_of_tree`; R-103 the two `let _ = std::fs::write` revert sites.
- **F2 (`runner.rs`/`slot.rs`)**: R-33 `Sample.asi["slot_holders"]` from `slot::build_slot_holders`.
- **F2 (`stop.rs`)**: R-46 `hours_running` subtracts `now − paused.at` while `Paused`.
- **F2 (`finalize.rs`)**: R-98 return `None`/`Err` instead of a file path as the document id; line 7's comment (the actor does the report turn now).
- **F2 / closer (`gate.rs` + `types.rs`)**: R-19 as above.
- **F3 (`tools.rs`, `study_draft.rs`)**: R-10; R-11 the "include my uncommitted changes" choice sets `StudySpec.include_dirty`.
- **F3 (`brief.rs`)**: R-3 `n_i` and line 595's incumbent read must skip `SampleRole::Holdout` (use `actor::bench_values`); R-75 the advisor clause.
- **F3 (UI)**: R-87 live tail (progress events now tick every 3 s while sampling); R-89 `holdoutSummary` over the per-`n` points.
