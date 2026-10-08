# 140 as-built — F3 FIX (model-facing side, config, UI, docs)

Worker 140-F3, 2026-10-08, branch `feat/140-f3` from 78411fb3 (worktree `~/Develop/theuth-140-f3`). Work list: `~/.theuth/briefs/2026-10-08/140/review-R.md`, every finding whose first cited path is in `crates/autoresearch/src/{tools,brief,novelty,review,config}.rs`, `crates/autoresearch/skill.md`, `crates/app/src/study_draft.rs`, the TUI/desktop `/autoresearch` lines, `crates/desktop/ui/src/**` and `docs/design/**`. Tiny changes in files no fixer owns: `crates/rpc/src/api.rs` (R-94), `crates/theuth/src/main.rs` (one template comment, R-72), `crates/tui/Cargo.toml` (+ `autoresearch` dep, R-92), `README.md` (R-118), `docs/design/00-architecture.md` (R-119).

## Findings → outcome

| id | sev | outcome |
|---|---|---|
| R-3 (brief side) | BUG | **fixed** — `brief.rs`: `is_bench()` excludes `SampleRole::Holdout` from n_i and from the "before" side of *Cases that moved most*. Test `holdout_samples_never_reach_the_brief` (fails before: n_i = 4 → threshold 1.64; cases polluted by the holdout's 1000). Actor-side reads (x̄_i, verdict line, `cases_moved`) are F1's (R-3 proper). |
| R-10 | BUG | **fixed** — the press path is a **press token**: `tools::write_draft` writes `{spec, origin, token}` and returns the token; the card's Start buttons run `Mutation("autoresearch_start", {draft_id, token[, window]})`; new `StartTool` (`autoresearch_start`) refuses a missing/mismatched token before sending `StartDraft`; `autoresearch_control` no longer has `start`/`start_window` (an explicit refusal names the card). The model never sees the token (`autoresearch_draft`'s result no longer carries `spec_path`). Tests: `draft_spec_round_trips_on_disk_with_a_press_token` (tools), `source_wires_presses_through_the_token` (app). Ceiling (ponytail comment in `write_draft`): the token is on disk and in the card source, so a model with `read_file`/`shell` can still find it — §8.3's acknowledged limit; it closes the ordinary tool path the finding names. `ToolCx` carries no activation and adding one touches ~40 constructors across crates (hand-off below). |
| R-13 | RISK | **fixed** — `ControlTool::call` refuses when `cx.session` is a bound study session (`Status` → `StudyInfo.session`) or a wave child with a slot (`wave::slot_of`). Gap: a registered child that has not yet called `experiment_begin{slot}` is not detected — `wave::parent_of` is private (hand-off F2: `pub fn is_child(session) -> bool`). |
| R-26 | RISK | **fixed** — `validate_spec`: `bench.repeats >= 2`. Test `one_repeat_is_refused`. |
| R-68 | BUG | **fixed** — `metric_line` uses `gate::normalise` (+ = better) like every row; fixture kept rows carry `delta_pct = +2.4`; the fixture brief now reads `Incumbent 41.2 ms = +2.6 % vs base` / `(+2.4 %, 3.1 sigma)` (test `fixture_brief_fits_and_has_every_section` fails before). |
| R-70 | BUG | **fixed** — `refused` dropped from `novelty::DEAD_ENDS` and `brief::is_dead_end`. Tests `novelty::refused_runs_are_not_dead_ends`, `brief::refused_runs_are_not_dead_ends`. |
| R-71 | RISK | **fixed** — `review::keep_diff` fetches the whole diff (`usize::MAX`); `summarise_diff` (already called in `prompt`) builds the per-file summary over every file and picks the largest hunks from all of them within `DIFF_MAX`. No `--numstat` round trip needed: the summary counts `+`/`-` lines of the full diff. |
| R-72 | RISK | **my side + hand-off** — `autoresearch.effort` documented as *reserved, not applied* (config.rs doc, §9.3, §12.2, the config template comment). Hand-off (runtime + agent, no fixer owns them): `SideCall { effort: Option<String> }` applied at the provider request; then `review::call` passes `cfg.effort.review` / `.advisor`; the executor's effort needs a schedule-file key or `SetEffort` on the bound session. |
| R-77 | RISK | **fixed** — dead-end groups are capped newest-first (`truncate(cap)`) before the category sort. Test `dead_end_cap_drops_the_oldest_groups`. |
| R-78 | POLISH | **fixed** — no `·` for blank playbook lines; `trim` ends a body at the blank line before the next known header (`HEADERS`), so blank lines inside the playbook are body. Test `playbook_blank_lines_survive_and_trim_from_its_bottom`. |
| R-79 | POLISH | **fixed** — `Category balance: …` and `Cases that moved most last time: …` match §7.2. |
| R-80 | POLISH | **fixed** — `Incumbent X ms (vs base unknown)` when the baseline is unknown. |
| R-81 | POLISH | **fixed** — `experiment_notes` description formats `brief.playbook_bytes` (KiB). |
| R-82 | POLISH | **fixed** — `study_draft::approval` deleted (with its test). |
| R-83 | POLISH | **fixed** — §12.4 amended as built (`autoresearch_start`, the control op list, no `start` op, R-13 refusal). |
| R-84 | BUG | **fixed** — `AgentDefaults::from_runtime` chain is `[subagents.model, title_model, agent.model]` (R1-D5). Test `reviewer_falls_back_to_agent_model_when_executor_is_overridden` (fails before: reviewer = executor). |
| R-85 | BUG | **fixed (UI side)** — `preparing` shows "preparing: worktree, custody, baseline samples…" and no Stop now (the actor refuses `StopNow` outside Running/Paused). Component test updated. Hand-off F1 (optional): allow `StopNow` in Preparing by cancelling the baseline task; then the button comes back. |
| R-86 | BUG | **fixed (UI side)** — `RoutinesView`: a study row's Run now → `autoresearch.control {run_once}` (then opens the Study view); its switch → `pause` / `resume`. `binding.rs` reconciliation of a direct `SetEnabled` (R-54) stays F2's. |
| R-87 | RISK | **fixed (UI side)** — `useStudy` polls the detail every 2 s while `info.current != null` (`LIVE_MS`), so `StudyDetail.tail` is live without an actor change; the `ExperimentProgress` handler no longer invalidates the detail. Hand-off F1 (better): push the tail in `ExperimentProgress` or an `autoresearch.tail` stream; then drop the poll. |
| R-88 | RISK | **fixed** — `routes/routines.$name.$section.tsx` `beforeLoad` redirects `autoresearch-*` to `/autoresearch/<slug>` (covers the HUD ring link and deep links). |
| R-89 | RISK | **fixed (UI side)** — `holdoutByN` means the samples per `n`; `holdoutSummary` and the chart diamonds use it; the fixture now has three samples per checkpoint; `model.test.ts` asserts the means. The actor may keep emitting one point per sample. |
| R-90 | RISK | **fixed** — `SidePanel` Ideas: a p3…p0 select per open idea → `IdeaChange::Priority`. |
| R-91 | RISK | **fixed** — `useExperiments` is an infinite query paging by `before` until a short page; the table, tally and Restore lookup see the whole study. |
| R-92 | RISK | **fixed** — TUI: `Command::AutoresearchStatus/Control/Steer` → `autoresearch_list/experiments/control/steer` RPC, rendered with `tools::render_info` / `render_row` (new pub helper); `/autoresearch` lists, `<slug>` prints the study + last 10, `pause|resume|stop|stop_now <slug>`, `steer <slug> <text>`; anything else hands the skill to the model. Desktop `slash.ts`: the same forms; `list`/`status` open the Library tab, a bare known slug opens the study. `crates/tui/Cargo.toml` gains `autoresearch`. |
| R-93 | RISK | **fixed** — `NewSegment` (AlertDialog with a required reason) in the running and paused controls. |
| R-94 | RISK | **fixed** — `rpc/api.rs`: `autoresearch.list` → `[]` and `settings.set` persists via `config::persist` when the actor is not running; `api.ts` comment says so. |
| R-95 | POLISH | **fixed** — `useAutoresearchEvents` is mounted once (Explorer); the Study view no longer mounts it; progress events no longer refetch the detail (the poll does, R-87). |
| R-96 | POLISH | **fixed (UI side)** — the needs-eyes line's trailing `· Restore · Keep reverted` is stripped before rendering (the buttons say it). F1 may also drop the suffix in `actor.rs:3375`. |
| R-97 | POLISH | **fixed (UI side)** — `markLabel`: the chart tooltip is `#n outcome +Δ % zσ · value · idea` from the run row (`StudyChart` takes `rows`); falls back to `#n outcome · value`. `StudyChartPoint.line` (actor) not needed. |
| R-98 | POLISH | **fixed (UI side)** — `Finished.tsx` / `SidePanel.tsx` link `/notes/$id` only when `report_doc` has no `/`; a file-path fallback is shown as text. F1's `report_doc: None` on the fallback remains the clean fix. |
| R-99 | POLISH | **fixed** — Settings: reviewer On/Off toggle (`review.model: off`); timeouts edited in seconds (`secondsOf`, written as `<n>s`); Discard hidden for an already-discarded study. |
| R-112 | BUG | **fixed** — `autoresearch_draft` asks the actor for the live section (`tools::live_cfg`) before writing: with `autoresearch.enabled: false` it refuses with "autoresearch is not running" and writes nothing. (Registration stays in `app`: only `app` can commit the card under the tool's call id, W6 dev. 1.) |
| R-113 | RISK | **fixed** — same `live_cfg`: the draft is written under the actor's current `root`. |
| R-114 | RISK | **fixed** — `NotesTool` checks `playbook_bytes` from `live_cfg()`. |
| R-116 | RISK | **fixed** — `persist` finds the block by `starts_with("autoresearch:")` (inline map / trailing comment). Test `persist_replaces_an_inline_or_commented_block`. |
| R-118 | POLISH | **fixed** — README: crate entry, `autoresearch:` config bullet + block, `/autoresearch` TUI line. |
| R-119 | POLISH | **fixed** — 00 §: "(140; built 2026-10-08)". |
| R-121 | POLISH | **declined** — per-key `write_config_value` cannot express the nested section (`defaults.budget.usd` is three levels; `write_nested_config_value` stops at two) and a diff-based writer is well past 15 lines; the whole-section write is documented on `persist`. Comments inside the `autoresearch:` block are lost on `settings.set` (unchanged). |
| R-122 | POLISH | **fixed** — `CONSUMES_SECRETS = false` override removed from `StudyDraftTool`. |
| R-123 | POLISH | **fixed** — dead `DraftTool` deleted. |
| R-124 | POLISH | **fixed** — `AgentDefaults::from_runtime` documents why it reads the `agent` section from the startup snapshot. |
| R-8 (skill side) | BUG (F2) | **unchanged, flagged** — `skill.md` keeps `bench.command: <data_dir>/autoresearch/drafts/<slug>/bench.sh` because the review's fix for R-8 (F2, `custody.rs`) accepts absolute tokens under `drafts/<slug>/` as custody-only files. If F2 lands a different rule, this example must follow. Also fixed in the skill: `window:` is a bare sensor file name (W3 dev. 10) and the "Never" section says no tool starts a drafted study. |
| IDLE_SENSOR | — | **fixed (found while fixing R-10)** — `tools::IDLE_SENSOR` was `sensors/idle.py`; 40's `when:` grammar refuses path separators (W3 dev. 10), so "Start in idle window" with the default sensor would have failed at `schedule_file::write`. Now `idle.py`. |

## Hand-offs

- **F1 (`actor.rs`)**: R-3 actor reads (`bench_values()` excluding `Holdout` at every incumbent read, `cases_moved`); R-85 allow `StopNow` in Preparing (cancel the baseline) — the UI will show the button again once the actor accepts it; R-87 push the tail in `ExperimentProgress` (then drop `LIVE_MS` polling in `queries.ts`); R-96 drop "· Restore · Keep reverted" from the needs-eyes line (the UI strips it meanwhile); R-98 `report_doc: None` on the file fallback.
- **F2 (`wave.rs`)**: `pub fn is_child(session: SessionId) -> bool` so `tools::refuse_study_sessions` can refuse a registered child before it binds a slot; R-8 custody rule for absolute `drafts/<slug>/` tokens (the skill assumes it).
- **runtime + agent (no owner)**: R-72 `SideCall.effort`; R-10's stronger form would be `UserActivation` on `ToolCx` (app-press provenance) — then `autoresearch_start` can check the activation instead of a token.

## Gates (`BUILD_LABEL=140-F3 build-lock`, stable rustc 1.96.1, `CARGO_BUILD_JOBS=4`, warm target)

GATES_TABLE

## Deviations

1. **R-10 is a token, not provenance.** `ToolCx` has no app-activation field and adding one is a cross-crate change of every `ToolCx` constructor; the token closes the model's ordinary path (the finding), not a model that reads the draft file. Recorded as a ponytail ceiling in `write_draft`.
2. **R-85 drops the button rather than allowing the op**: the actor is F1's; a Preparing study has no experiment to discard and the baseline task has no cancel path today.
3. **R-87/R-89/R-96/R-97/R-98 are fixed on the UI side only**; the actor-side improvements the review names are listed as hand-offs, none of them blocking.
4. **R-121 declined** (reason in the table).
