# 140 as-built — F2 FIX (binding, loop file, measurement, git)

Fixer 140-F2, branch `feat/140-f2` from 78411fb3. Owns `crates/autoresearch/src/{binding,stop,schedule_file,wave,runner,scanner,gate,custody,slot,git,finalize,record}.rs`, `crates/autoresearch/fixtures/**`, and the `crates/schedule` change R-5 needs. Work list: `~/.theuth/briefs/2026-10-08/140/review-R.md`.

## Built, file by file

- **`crates/schedule/src/lib.rs`** (R-5). A process-wide confinement registry: `schedule::confine(prefix, resolver)` registers a `ToolsetResolver` (`Fn(&str) -> Future<Result<Option<agent::Toolset>, String>>`) for every schedule whose name starts with `prefix`. `fire_with` resolves it before the agent lookup: `Ok(Some(toolset))` → `agent::SetToolset` is *asked* before `SendInput` (both land in the session's mailbox in order, so the first turn's rebase already reads the allowlist); a failed `SetToolset` refuses the fire; `Ok(None)` = the owner no longer knows the schedule → the loop ends like a broken `until` (`toolset gone: no owner for this schedule`, file removed); `Err` refuses this fire (retried next tick). `ScheduleEvent::Fired` is now published *before* `SendInput` (after toolset + budget), so the subscriber's `studies.session` is on record before the session's first tool call. The schedule crate learns nothing about studies. Test `confined_schedule_refuses_or_ends_without_its_toolset`.
- **`binding.rs`**. `confinement(store)` is the resolver (registered in `lib.rs::start` — one line, F1's file): a `running`/`paused` study → `toolset(parallel)`; anything else under `autoresearch-` → orphaned. `on_fired` only records the session (no agent ask on the mailbox, R-115). `on_turn_started` records the user's typed text in a bound session as a steer (`<…` envelopes skipped) — R-47. `on_turn_finished(a, session, stop_reason)`: `cancelled` with an experiment still open → `timeout` (the plugin's own stop-now closes the experiment before cancelling, so an external cancel is the schedule's `run_timeout`) — R-55; `mark_lost` for a measuring one; the stop rules run in a spawned task (the advisor consult no longer blocks the mailbox) — R-48. `ScheduleEvent::Changed` → `reconcile` (spawned, 2 s settle): a running/paused study whose file is gone finalizes (`Experiments` when the schedule's last row says `max_runs`, else `User`) — R-53; a file/ledger `enabled` mismatch goes through `Control { Pause | Resume, by: User }` so the ledger, events and file agree — R-54. Tests `confinement_resolves_live_studies_only`, `typed_text_in_a_bound_session_is_a_steer`, `cancelled_turn_records_timeout`.
- **`schedule_file.rs`**. `disable_all(data_dir)` flips `enabled: false` on every `autoresearch-*.md`; called from `Plugin::start` when `autoresearch.enabled: false` (R-6; three lines in `lib.rs`). `fire(slug, paused)`: when the interleaved tick already fired the loop (`Fire` → `busy`) the running session is returned from `ListSchedules` instead of an error (R-59). `done_run` deletes the study file bound to the session (never `schedule::Done`, whose "done (model)" line ignores `quiet`); `done` deletes (R-61). Test `disable_all_pauses_study_files_only`.
- **`wave.rs`**. A wave child's toolset is applied off the mailbox; a child that cannot be confined is cancelled (`agent::Cancel`, tree) and forgotten instead of running with every tool (R-12/R-115). `lose` and the review-revert save the patch with `?` and set `patch_path` only after the write (R-103).
- **`gate.rs`**. `Guard` has its own `direction` (default `lower`); `GateCfg.guards` is `(name, Effect, Direction)`; `guard_failures` normalises each guard in its own direction (R-19; one-line change in `actor::gate_cfg`, one field in `types::Guard`). Robust σ (≥ 12 repeated samples) is now the median of all within-tree pairwise |differences| × 1.04827: a pairwise difference has the same distribution whatever the tree size, so confirm pairs/triples no longer read σ 30–50 % low; one outlier poisons only its own pairs (R-24). A keep whose *first* sample was suspicious (z₁ ≥ z_suspicious or Δ₁ > 3× the largest accepted) stays `suspicious` after confirm even when the means fall under the thresholds (R-30). The power-fix comment now cites the as-built table (82.4 % → 89.7 %) and says the sample must be taken on the incumbent tree (R-25). Tests `guards_use_their_own_direction`, `robust_sigma_is_consistent_for_small_trees`, `suspicion_from_the_first_sample_survives_confirm`.
- **`runner.rs`**. Load wait first, then the bench slot, one re-check of the load under the slot (R-31). `Group` RAII guard owns the process tree: TERM→KILL on timeout/cancel, killed on drop (an early `?`, a dropped future), what remains killed when the run ends (R-27). Pipe EOF no longer ends the run: the leader is awaited under the same deadline (R-22). The deadline branch is gated on `status.is_none()` (R-29). Non-candidate samples log to `sample-<k>-<role>.log` (R-23). `resolve` accepts `./x` and `<root>/drafts/<slug>/x` tokens and shell-quotes the custody path (R-8/R-17). `root_relative` slices from the component before `runs` (R-37). Tests `dot_slash_and_draft_paths_resolve_into_custody_quoted`, `redirected_output_still_times_out`, `role_logs_and_root_relative_paths`.
- **`custody.rs`**. `command_path(token, drafts)` classifies tokens (`./x` → `x`; absolute under drafts → custody-only; other absolute, `..`, flags → none). `take` copies both forms and refuses the study when a script-looking token (`x/y`, `*.sh|py|rb|js|mjs|ts|pl|lua|bash|zsh`) resolves to no file (R-8). The `git` helper is `git::read` (R-18). Test `dot_slash_and_absolute_draft_tokens_enter_custody`.
- **`slot.rs`**. Owner file is `pid label epoch argv` (`acquire` takes the command) (R-39).
- **`stop.rs`**. `after_n` falls back to `n` (the `overfit` payload) so the overfit → plateau rule is live (R-44). `hours_running_from(study, now, paused_since)` subtracts the open pause; `paused_since` = the oldest `paused` row newer than the last `resumed` (a re-stamp does not lose the start); `hours_running_live(store, study)` for callers with a store; the view uses it (R-46). Crash streak, custody refusals and review flags count only over `n > resumed.after_n` (R-57). Plateau is evaluated before overfit, as the table (R-67). Tests `view_reads_overfit_streaks_and_pauses_from_the_ledger`, `hours_exclude_paused_time`.
- **`git.rs`**. `read(cwd, args)` for other modules (R-18). `branch_at` uses plain `branch`; an existing branch is moved only when its tip carries `Autoresearch-Study:` (R-16). `merge_into_checkout` runs through `cmd_unsigned` (no author env): the merge commit is the user's (R-15). `reset_clean` is public (R-56). Tests `branch_at_refuses_to_move_a_user_branch`, `merge_helper_refuses_dirty_and_merges_no_ff` (author assertion).
- **`finalize.rs`**. `confirm` starts with `reset_clean(worktree, final_sha)` (R-56). `run_with(store, study, reason, cfg, root, cancel)` takes the live config; `run(…, rt)` keeps the seam's shape and delegates (R-106). The record is written by the `record` step through `record::write_record` (R-62).
- **`record.rs`**. `write_record(store, study, headline)`; `write(…, rt)` delegates.

## Findings in scope

| id | sev | status |
|---|---|---|
| R-5 | BUG | fixed — schedule `confine` + Fired-before-SendInput; `binding::confinement`; tests `confined_schedule_refuses_or_ends_without_its_toolset`, `confinement_resolves_live_studies_only` |
| R-6 | BUG | fixed — `schedule_file::disable_all` at `Plugin::start`; test `disable_all_pauses_study_files_only` |
| R-7 | BUG | handed off → F1: `prepare` (actor.rs:1288) and `segment_task` (:3204) call `custody_take(&spec, &worktree, &evaluator)` — custody must come from the study worktree at the base; `custody::verify` already hashes the worktree |
| R-8 | BUG | fixed — `custody::command_path`, `take` refuses missing scripts, `runner::resolve`; tests `dot_slash_and_absolute_draft_tokens_enter_custody`, `dot_slash_and_draft_paths_resolve_into_custody_quoted`. Create-time validation (tools.rs/study_draft) → F3 may call `custody::take` semantics early; not required |
| R-9 | BUG | handed off → F1: `ControlOp::Discard` arm calls `git::discard_study(repo, slug, &study_dir)` (add a `Backend::discard_study` seam); `git.rs` side is complete and tested (`discard_study_removes_worktrees_branches_and_refs_only`) |
| R-12 | RISK | fixed — wave child cancelled when the toolset cannot be applied; the study session's toolset is applied by the schedule itself (a failure refuses the fire) |
| R-15 | POLISH | fixed — `cmd_unsigned` for the merge; author asserted in `merge_helper_refuses_dirty_and_merges_no_ff` |
| R-16 | POLISH | fixed — `branch_at`; test `branch_at_refuses_to_move_a_user_branch` |
| R-17 | POLISH | fixed — quoted custody paths |
| R-18 | POLISH | fixed — `git::read` |
| R-19 | BUG | fixed — `Guard.direction`, `GateCfg.guards` triple, `guard_failures`; test `guards_use_their_own_direction`. Touched `types.rs` (one field) and `actor.rs:505` (one line). F3: the draft card may expose `direction` per guard (optional; default `lower`) |
| R-22 | BUG | fixed — runner EOF handling; test `redirected_output_still_times_out` |
| R-23 | BUG | fixed (runner side) — role in the log name; test `role_logs_and_root_relative_paths` |
| R-24 | RISK | fixed — pairwise-difference robust σ; test `robust_sigma_is_consistent_for_small_trees` |
| R-25 | RISK | fixed (gate side) — incumbent-first `Confirm { more: 0, incumbent_sample: true }` after a fast keep; no one-sample discard while `n_i < 3`; comment cites the as-built numbers. Handed off → F1: `run_env` honours `more == 0` (drop `more.max(1)`, actor.rs:2053); F3: §5.2 text |
| R-27 | RISK | fixed — `Group` guard |
| R-29 | RISK | fixed |
| R-30 | RISK | fixed — first-sample suspicion carried; test `suspicion_from_the_first_sample_survives_confirm` |
| R-31 | RISK | fixed |
| R-33 | RISK | handed off → F1: `Sample.slot_holders: Vec<String>` (types.rs, `#[serde(default)]`), a `samples.slot_holders` column + migration, `store::insert_sample`; runner fills it from `slot::build_slot_holders()` once the field exists (eight `Sample {}` literals in F1/F3 files would break on an unannounced field) |
| R-37 | POLISH | fixed |
| R-39 | POLISH | fixed |
| R-41 | POLISH | handed off → F1: a `Sample.failure: Option<String>` (or the line built in `run_env` from `exit`/`bad_number`/missing primary) |
| R-44 | BUG | fixed (stop side) — `after_n` reads `n`; test `view_reads_overfit_streaks_and_pauses_from_the_ledger` |
| R-46 | BUG | fixed (stop side) — `hours_running_from` / `hours_running_live` / `paused_since`; test as above. Handed off → F1: `pause()` no-op when already paused; `info`, `begin_env`, `log_env` brief callers use `stop::hours_running_live(&env.store, &study)` (the 2-arg `hours_running` ignores the open pause) |
| R-47 | BUG | fixed — test `typed_text_in_a_bound_session_is_a_steer` |
| R-48 | BUG | fixed — `after_experiment` spawned from the handler |
| R-51 | BUG | handed off → F1 (first path actor.rs): record a `run_once` event in the `RunOnce` arm before `schedule_file::fire`; `begin_env` accepts `Paused` only when it is newer than the last `begin`. Warn text at actor.rs:2697 (R-111) goes with it |
| R-53 | RISK | fixed — `reconcile` finalizes a running study whose file is gone |
| R-54 | RISK | fixed — `reconcile` pauses/resumes through `Control` |
| R-55 | RISK | fixed — test `cancelled_turn_records_timeout` |
| R-56 | RISK | fixed (finalize side) — `reset_clean` at confirm start. Deferring `finalize_task` to `TurnFinished` is F1's call |
| R-57 | RISK | fixed |
| R-59 | RISK | fixed — `busy` after the re-arm is the run that started (no schedule change: a force-fire flag on `Fire` would break `rpc/api.rs`) |
| R-61 | POLISH | fixed — `done_run`/`done` both Delete |
| R-62 | POLISH | my side done (`write_record` from the `record` step). Handed off → F1: delete the `env.backend.record(...)` call in `finalize_task` (and the `Backend::record` seam) |
| R-67 | POLISH | fixed |
| R-103 | RISK | fixed (wave.rs side); actor.rs:2410-2416 → F1 |
| R-106 | RISK | fixed (finalize side) — `finalize::run_with`. Handed off → F1: `Backend::finalize` passes `env.cfg.clone()`, `&env.root`, `env.rt.shutdown.clone()` to `run_with` |
| R-108 | POLISH | declined — one indexed `WHERE session = ?` per `TurnFinished`; an in-memory set would be a second source of truth for `studies.session` |
| R-115 | RISK | fixed (binding/wave side): no agent ask on the mailbox in `on_fired` (the schedule applies the toolset), child toolset and `reconcile` spawned. actor.rs `Cancel`/`SessionCostQuery` asks → F1 |

## Hand-offs (summary)

- **F1**: R-7, R-9, R-25 (`more == 0` in the gate loop), R-33, R-41, R-46 (`pause()` no-op; `hours_running_live` callers), R-51, R-62 (delete the second record write), R-106 (`run_with`), R-115 (actor asks), and `on_fired` should use a column-scoped `Store::set_study_session(slug, session)` once R-4's setters exist (today it rewrites the record). `binding::on_turn_finished` now takes `stop_reason: &str` — the two calls in `actor/tests.rs` (874, 895) pass `"end_turn"` (edited here, mechanical). `crash_streak_pauses_with_attention_and_resume_counts_paused_time` pinned the R-57 bug (its own comment said "does not re-pause until two more crashes" while asserting a re-pause after one); it now asserts Running after one crash and Paused after two.
- **F3**: §5.2 text for R-24 (pairwise robust σ) and R-25; §8.2 (`Fired` is published before the prompt; the toolset is applied by the schedule); a per-guard `direction` in the draft card (optional).
- **agent crate (nobody's this round)**: deterministic confinement of wave children needs `SpawnChild` to copy the parent's toolset onto the child before its first turn (`supervisor.rs` after `self.session(...)`, before `SubagentSpawned`); until then a child's first round can start before `SetToolset` lands. Not changed here: an `agent` edit rebuilds every dependent crate in the shared target for all three fixers.

## Deviations

- §5.2 step 1 says "1.4826 × pooled MAD"; built as the scaled median of within-tree pairwise differences (same robustness goal, no small-tree bias). Numbers: for σ = 1 normal noise in trees of 2/3/4 the estimate is within 5 % of 1 (test), where the MAD-around-median read 0.71 for pairs.
- **§15 power acceptance.** With the unbiased σ the gate's defaults (`z` 2.0 / `z_fast` 3.5 / `z_discard` 1.0 / `confirm_runs` 2) give, in W1's simulation (σ = 1 % of the metric, `min_effect` 0.5 %, 200 studies × 50 experiments): **true 3σ keep rate 83.6 % (8361 / 10 000)**, **null false-keep rate 0.39 % (39 / 10 000)**. W1's 89.7 % / 0.93 % were both products of the σ that read 30–50 % low past 12 samples (every z inflated ≈ 1.4×). The ≥ 85 % line in §15 therefore needs Sky's call: raise power through the `gate:` defaults (e.g. `z_discard` 1.0 → 0.8 cuts one-sample discards of true gains; the null budget has room: 0.39 % of 1 %) or amend §15. The test floor is 80 % with this note; the null test stays at ≤ 1 %.
- **R-25 in the gate.** After a fast keep the incumbent has one sample (the lucky one). `step` now returns `Confirm { more: 0, incumbent_sample: true }` on the next experiment's first sample — measure the incumbent once more (on its own tree, R-2) before judging — and does not one-sample-discard a positive Δ while the incumbent has fewer than 3 samples (confirm on means instead). The actor's loop reads `more.max(1)` (actor.rs:2053): until F1 drops the `max(1)` the incumbent-first step also costs one candidate sample; handed off.
- R-5's fix is a schedule-crate registry rather than publishing `Fired` first alone: `publish` is a `tell` into a bus actor, so a subscriber's `SetToolset` could never be ordered before `SendInput`; the schedule applying it on the same ask path is the only deterministic order.
- R-59 without a schedule change: the `busy` fallback reports the session of the run the tick started.
- Shared `./target` hazard for the closer: cargo's fingerprint for `autoresearch`'s test binary records the *last builder's* source paths, so a sibling's build makes `cargo test` in another worktree report "fresh" and run the sibling's binary (observed: 117-test binaries with F3's `brief::` tests and the old 89.65 % sim number). Every gate below ran as `cargo clean -p autoresearch -p schedule && cargo test …` inside one build-lock slot; the closer should do the same or use a private target.

## Gates

All through `BUILD_LABEL=140-F2 build-lock`, stable rustc 1.96.1, `CARGO_BUILD_JOBS=4`, after `cargo clean -p autoresearch -p schedule` in the same slot.

- `cargo test -p autoresearch`: **122 passed, 0 failed** (+14 new: binding 3, schedule_file 1, gate 3, runner 3, custody 1, stop 1, git 1, plus the schedule crate's 1).
- `cargo test -p schedule`: **76 passed, 0 failed**.
- `cargo clippy -p autoresearch -p schedule --all-targets -- -D warnings`: **0 findings** (`Finished`, same slot as the clean).
- `rustfmt --edition 2024` on the twelve F2 files (no `rustfmt.toml`; `cargo fmt -p` would touch F1/F3 files).
- Simulation (gate defaults): null 0.39 % (39 / 10 000); true 3σ 83.6 % (8361 / 10 000); robust σ̂ within 5 % of 1 for trees of 2/3/4 (4 000 trees each).
