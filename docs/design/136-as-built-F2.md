# 136 — As-built F2 (DREAMING-crate fixes from review R + live proof P)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-f2`, branch `feat/136-f2`, cut from `6f853263` (P's `f9edd39c` + F1 + F3), `feat/136-dreaming` merged at `ee01d6c8` (P's `7f4160e2` fire-dated night and as-built §6.1/§6.2). Owns `crates/dreaming/**`, `crates/rpc/**` (`dreaming.*`, TS exports); `crates/theuth/src/main.rs` untouched (the plugin API did not move). Commits: `83c6eaed` (every fix), `f5deec15` (night tests, clippy), `ee01d6c8` (merge), `789df666` (rpc disabled-path settings, this file). Line numbers are this tree's at `ee01d6c8`.

## 0. One screen

- **The gate is the server's** (D2, R-2): every `replay_case` persists `{night, case_id, candidate_sha256, score, judge_model, at}` to `dreaming/replays.jsonl`; `skill_learned_edit{pass: "record"}` recomputes `replay::gate` from tonight's rows of exactly that candidate text (paired by case, latest score wins) and refuses with `no replays for this candidate` when none exist; `unverified`/`rejected` log, never write; the caller's `gate` is advisory (a mismatch is `tracing::info!`-logged). `pass` is a new optional arg (`candidate` default).
- **The proposer's JSON parses** (D3, R-3): `EditRecord` is `#[serde(default)]` on everything but `op`; a `{"op","content","anchor","rationale","cases"}` object is enough.
- **Bounded blast radius** (D4, R-4): `routine.md` carries `cap: mutating`; the skill never names `shell`/`git`/`delete_file`/`knowledge_delete`/MCP; `skill_learned_edit{op: "retire"}` moves a dreaming-made skill (`origin: dreaming` only) to `skills/retired/<name>.md` with `retired_because:`/`retired_on:`, keeps the original under `backups/<night>/retired/` and undoes with `skill:<name>`; verification is read-only tools with an explicit `unverifiable` out.
- **The night closes server-side** (D7, R-8/R-11/R-31/R-46/R-49): `NightSummary.status` = `budget` (the guard's own predicate `spent + estimate > cap` over the session's persisted `Budget`) or `incomplete` on `TurnFinished` before the journal; `last_dream_at` moves at `BeginNight` (`Signals::DreamStarted`, counters kept); a second `journal` is `night … is already closed` (one receipt, `nights` once); `run_request` is written only after `FireWith` answers Ok; `RunNow` refuses while paused, with `dreaming.enabled: false` and with the routine row off.
- **D6 wired**: `ScheduleEvent::Fired{name: "dreaming"}` and the `RunNow` reply register the session with `Knowledge::register_consolidating_session` (BeginNight re-registers as a belt); `finish` clears it. One `fired: Option<(at, session)>` field also dates the night from the fire (P's `7f4160e2`).
- **Preferences** (D8, R-9/R-16): `preferences_set` backs `preferences.md` up once per night before its first write; undo restores it once (`sidecars_undone: ["preferences"]`), an empty backup removes the file; with `auto_from_explicit` off (read live from the actor's settings) `add` files a `remember` proposal and answers `auto-promote is off: filed as a remember proposal`.
- **Counters are the server's** (D10, R-12): `cases::transition` → `edits::count_case`: `settled` → `helpful += 1`, `reopened` → `harmful += 1` on every bullet citing the case; `harmful ≥ 3` moves the bullet to `skills/retired/learned-<skill>.md`, journals a `Settle` line with the undo handle, backs the sidecar up under tonight first. The skill's "bump via replace" prose is gone. `transition` also enforces the state machine (R-38).
- **Models** (D12, R-14): `DreamConfig::resolve_models(&AgentDefaults)` — `tidy` = pick ?? `agent.model`, `propose` = tidy, `judge` = pick ?? `agent.escalate_model` (≠ propose) ?? propose with `judge_same_model` on the night, `target` = `agent.model`; `model:` is always written at install and patched on `SetSettings`; `replay` uses the resolved target/judge; `DreamSettings.resolved: {tidy, propose, judge, target}` (`DreamResolvedModels` in TS).
- **Money** (D13, R-15): `budget_line` renders from nanos (`0.1234567891` → `0.123456789 USD`, `1e20` parses); `SetSettings` bounds `0 < budget ≤ 1000` (`MAX_BUDGET_USD`).
- **Signals** (D14, R-17/R-18/R-19/R-33): `TurnStarted` stamps; only attended roots stamp or count toward `sessions_since`; `written_at` older than 3 min (or unparseable) → `signals stale` on both tables (sensor v3, heartbeat every 60 s); the engine arg is always rendered (home engine or self).
- **Smaller**: `tidy::stem` char-safe (R-32); anchors normalised (`- `, line number, metadata tail, whitespace — R-29); learned names `[a-z0-9-]{1,64}` on the writer side (R-24, see hand-off); `dreaming.enabled: false` removes the three bundled files at start and on `SetSettings` (D11, R-13); `install_files` patches an existing routine in place (R-47); `dreaming_ledger` is `Mutating` and gated (D9, R-10) and a second call keeps the first pass's window and rewinds (R-43).

## 1. Findings → file:line → test

| Finding | Where | Test |
|---|---|---|
| R-2 (D2) | `replay.rs` `ReplayRow`/`log_row`/`rows`/`gate_from_rows` (`:43-131`), `replay()` logs a row (`:215-285`); `edits.rs` `apply` record pass (`:428-569`: refuse without rows, gate from rows, mismatch logged, `unverified`/`rejected` not written, one `RejectedEdit` per proposal); `tools.rs` `SkillLearnedEditArgs.pass` | `replay::tests::gate_is_recomputed_from_tonights_replay_rows` (no rows → `None`; other candidate/night ignored; latest score wins; 2 pairs → unverified; candidate without baseline → 0 pairs); `edits::tests::record_pass_gate_comes_from_the_replay_rows_not_the_caller` (asserted `accepted` + no rows → refused, nothing written; 2 pairs → logged unverified, not applied, case open; 3 pairs 0.4→0.7 → applied, backed up, case closed; a regressing candidate asserted `accepted` → rejected, buffered) |
| R-3 (D3) | `types.rs` `EditRecord` (`:176-210`) | `types::tests::the_proposer_prompts_edit_shape_parses` |
| R-4 (D4) | `routine.md:11` `cap: mutating`; `types.rs` `EditOp::Retire`; `edits.rs` `skill_path`/`retired_path`/`retire_backup_path`/`is_dreaming_made`/`retire`/`retired_that_night` (`:326-423`); `undo.rs` retired skills move back (`:106-128`); `skill.md` §intro (tool surface), §2 Verify (read-only, `unverifiable`), §5 Retire by disuse, tool table | `lib::tests::routine_parses_with_the_schedule_crate_and_carries_the_budget` (`spec.cap == Some(Mutating)`); `edits::tests::retire_moves_a_dreaming_made_skill_and_refuses_a_human_one`; `undo::tests::a_retired_skill_moves_back`; the skill text carries no `shell`/`git log`/`delete_file` as an instruction (grep in §6 below) |
| R-8 / R-49 (D7) | `night.rs` `budget_hit` (`:1668-1690`), `CloseEnded` (`:1692-1724`: `status` budget\|incomplete, `Ended` line), `finish(r, status)` (`:613-671`: clears the registration), `BeginNight` sets `last_dream_at` + `DreamStarted` (`:825-919`); `signals.rs` `DreamStarted` (`:289-302`); `types.rs` `NightSummary.status` | `night::tests::turn_end_without_a_journal_closes_the_open_night` (status `incomplete`, receipt `· ended: incomplete`, a later `journal` refused); `begin_moves_last_dream_at_and_the_journal_closes_once`; `signals::tests::only_attended_roots_stamp_and_count` (`DreamStarted` keeps the counters) |
| R-11 (D7) | `night.rs` `Commit` (`:921-958`: `finished_at.is_some()` → `Err("already closed")`) | `night::tests::begin_moves_last_dream_at_and_the_journal_closes_once` (`nights == 1`, one `### Receipt`) |
| R-31 (D7) | `night.rs` `RunNow` (`:1198-1259`: `write_state` after the `FireWith` Ok) | `night::tests::run_now_respects_the_switches_and_persists_nothing_without_a_fire` |
| R-46 (D7) | `night.rs` `routine_enabled` (`:1191`), `RunNow` refusals (paused / `dreaming.enabled` / routine `enabled: false`) | same test |
| D6 (R-6/R-20 hand-off from F1) | `night.rs` `register_session` (`:558`), `ScheduleEvent` handler (`:1594-1607`), `RunNow` (`:1255`), `BeginNight` belt (`:888-893`), `finish` clear (`:666-668`), `start` subscribes `ScheduleEvent` | `night::tests::the_fired_session_consolidates_until_the_close` (`Fired{dreaming}` → `is_consolidating_session`; another routine's fire ignored; `BeginNight{session: None}` takes the fired session; the close clears it, `may_consolidate` false after) |
| R-9 (D8) | `preferences.rs` `backup_path`/`backup_once` (`:286-301`), `set` backs up before the write (`:278`); `undo.rs` preferences once + empty-backup removal (`:191-214`) | `undo::tests::preferences_restore_once_from_the_nights_backup` |
| R-16 (D8) | `tools.rs` `PreferencesSetTool::call` (`:518-566`: the live switch from `Night::Settings`, a `remember` proposal) | `tools::tests::preferences_add_becomes_a_proposal_when_auto_promote_is_off` |
| R-10 (D9) | `tools.rs` `LedgerTool` (`RISK = Mutating`, `self.0.mutating(..)` first) | `tools::tests::ledger_refuses_an_ungated_session_and_leaves_the_pending_rewinds`; `tool_names_and_risks_match_the_contract` re-pinned |
| R-43 (D9) | `ledger.rs` `build` (`:211-230`, `:268-272`: the earlier pass's `since` and `pending_rewinds` are kept) | `ledger::tests::build_without_actors_folds_pending_copies_journals_and_writes_the_night_file` (second call: same `since`, 2 rewinds on disk) |
| R-12 (D10) | `edits.rs` `HARMFUL_RETIRE`/`retired_bullets_path`/`count_case` (`:570-650`); `cases.rs` `transition` calls it (`:167`); `journal.rs` `append_sync` (`:141`); `night.rs` `append_text`; `skill.md` §5 | `edits::tests::case_transitions_bump_the_counters_and_retire_a_harmful_bullet` |
| R-38 | `cases.rs` `transition` state machine (`:149-159`); `night.rs` `choice_is_no` (`:1375`: `"no"`/`"no "`/`"not "`/`" not "`, so "Now" is not a no) | `cases::tests::append_fills_scrubs_clips_and_transitions_fold` (open→reopened, closed→closed, settled→closed refused); `night_records_list_newest_first…` keeps the `choice_is_no` cases |
| R-13 (D11) | `lib.rs` `BUNDLED`/`remove_files` (`:135-195`), plugin `start` removes before the early return (`:219-226`); `night.rs` `SetSettings` removes/reinstalls (`:1308-1318`) | `lib::tests::disabling_removes_the_bundled_files`; `night::tests::set_settings_patches_the_routine_model_and_toggles_the_files` |
| R-14 (D12) | `config.rs` `AgentDefaults`/`resolve_models` (`:79-124`); `lib.rs` `routine_text` always emits `model:` (`:105-133`), plugin reads `AgentDefaults::from_runtime`; `night.rs` `update_routine_file` patches/inserts `model`/`cap` (`:227-292`), `settings()` → `resolved`, `Night.agent`, `BeginNight` stamps `judge_same_model`; `replay.rs` resolved target/judge; `types.rs` `ResolvedModels`, `DreamSettings.resolved`, `NightSummary.judge_same_model`; `rpc/src/export.rs:225` | `config::tests::model_tiers_resolve_from_the_agent_section`; `lib::tests::routine_parses…` (`model: agent.model` by default); `night::tests::set_settings_patches_the_routine_model_and_toggles_the_files` (pick → file, `None` → default again, `resolved` answered) |
| R-15 (D13) | `lib.rs` `budget_line` (`:91`); `night.rs` `MAX_BUDGET_USD` + the `SetSettings` bound (`:1297-1303`) | `lib::tests::budget_line_always_parses` (`0.1234567891`, `1e12`, `1e20`, `0`, `1000`, `3.0000000004` all parse); the settings test (`0`, `-1`, `1000.01`, `NaN` refused; `1000` → `budget: 1000 USD`) |
| R-17 (D14) | `signals.rs` `TurnStarted` arm (`:216-222`) | `signals::tests::only_attended_roots_stamp_and_count` |
| R-18 (D14) | `signals.rs` `TurnFinished` counts only attended roots (`:223-236`) | same test (routine + child turns: `sessions_since` stays; the dreaming session's turn end after `Dreamt` → 0) |
| R-19 (D14) | `sensor_rules.rs` `STALE_SECS` + the rule (`:14`, `:52-57`), three table rows; `sensor.py` v3 `STALE_SECS` + rule; `signals.rs` `HEARTBEAT`/`Heartbeat` + the task in `start` | `sensor_rules::tests::rule_table` + `python_sensor_answers_the_same_table` (20 rows incl. `signals stale` ×2, fresh ×1, both sides); the signals test asserts the heartbeat re-stamps |
| R-33 (D14) | `lib.rs` plugin `start` (`:236-243`: `home_engine` else self) | `lib::tests::routine_parses…` (`engine=eng1` rendered; `None`/empty → no arg) — the plugin path is not unit-testable without a mesh identity; the install path is |
| R-20 (D14) | `night.rs` `NightRecord.head_at_start` (`BeginNight` captures `history(None, 1)`), `commit_before(k, e, head)` falls back to it for an event without a commit (`:1032-1068`) | `night::tests::the_fired_session_consolidates_until_the_close` (`head_at_start` = HEAD; a commit-less event restores to it); P's `commit_before_is_the_parent_of_the_events_commit_not_the_latest_history` still covers the two-commits-one-id case |
| R-24 (D15, writer side) | `edits.rs` `valid_skill_name` `[a-z0-9-]{1,64}` (`:262`) | `record_pass_gate…` (`../etc`, `Harness_1` refused) — the reader (`agent/src/skills.rs` `learned_path`) is F1's file: hand-off below |
| R-29 (D15) | `edits.rs` `normalise_anchor`/`same_text` (`:152-176`), `apply_one` matches on them; the proposer prompt states the grammar; the skill reads sidecars with `numbers=False` | `edits::tests::anchors_are_normalised_to_the_bullet_content` |
| R-30 (D15) | `skill.md` §4 step 3 (candidate = block + line, no `skill_learned_edit`) and step 6 (explicit → `preferences_set(add)`, inferred → `remember` proposal, rejected → journal) | prose; `replay::tests::the_judge_never_sees_a_rationale_and_the_frame_names_the_rung` renders the preferences target |
| R-32 (D15) | `tidy.rs` `stem` (`:41-44`) | `tidy::tests::stems_strip_dates_and_versions_only` (`café-2026-10-05`, `日本語`) |
| R-35 | `night.rs` `KnowledgeChanged` (`:1090-1106`: one `merged` per merge commit via `Night.merge_commits`) | covered by the handler path in `the_fired_session_consolidates_until_the_close` (a commit-less event); no dedicated multi-stub test — see §4 |
| R-36 | `edits.rs` `log` scrubs the record (`:287`); `night.rs` `Commit` scrubs the phase summary (`:948`); `ledger.rs` `copy_desk_journals` scrubs (`:104-105`) | the ledger test asserts `[redacted]` in the copied journal |
| R-37 | `undo.rs` unknown group → `Err` naming the night's groups (`:46-77`); created ids counted as undone after their line (`:83-91`); nothing restored → no journal line, no event (`:231-238`) | `undo::tests::sidecar_backups_restore_edits_are_marked_and_cases_reopen` (re-pinned: `skill:other` is an error; the summary is `undone`; the second run says nothing) |
| R-39 | `sensor.py` `_int`/`_hhmm` try/except; `dry_run.rs` `open_ro`/`read_uses` warn (`:98-131`) | the python lockstep tests run the new sensor; dry-run warn is a log line |
| R-40 | `ledger.rs` `DayLedger.min_sessions` (`:46-48`); `skill.md` drops hint-miss from Mine and the 45/30-day rules become a meta-note tally | prose + the field |
| R-41 | `skill.md` §3 (`questions=`, 2,000-row batches, `instructions` on every choice row) | prose |
| R-42 | `skill.md` ("an error containing `budget exhausted:`", `case_id=c["id"]`) | prose |
| R-47 | `lib.rs` `install_files` patches an existing routine via `update_routine_file` (`:168-175`) | `lib::tests::install_writes_owned_files_always_and_patches_the_routine_in_place` (`enabled: false` kept, `budget: 4 USD` and `engine=eng9` carried) |
| P §5.4 / §6.2 #3 (close gate) | P's `left_undone` kept; the skill now says the close is once and the harness closes a cut-off night | `left_undone_names_the_unworked_lists_and_is_none_when_worked` (P's) |

## 2. Skill diff summary (`crates/dreaming/src/skill.md`, 24,559 bytes; cap 24 KB)

- Intro: a tool-surface paragraph — the bounded set plus read-only tools; capped at `mutating`; no `shell`/`git`/`delete_file`/`knowledge_delete`/MCP; retire = `skill_learned_edit(op="retire")`.
- Budget order: the `<budget>` note is gone (a one-turn routine never sees it); the harness closes a cut-off night with status `budget`; the only in-turn signal is a `replay_case` error containing `budget exhausted:`.
- Backlog mode: repeat while the ledger reports backlog and `budget_left_tonight_usd` affords another; a second `dreaming_ledger()` keeps the window and rewinds.
- Recall: `L.min_sessions`.
- Tidy › Verify: read-only tools only (`read_file`/`glob`/`grep`/`code_find`/`knowledge_history`/`read_url`), no `git log`, `unverifiable` → skip and count.
- Mine: hint-miss dropped; `decide_many` batching and `instructions`; reopen → the server bumps `harmful`.
- Rehearse: records are `{op, content, anchor, rationale, cases}`; step 3 `pass` default candidate (preferences clusters skip it); step 4 both replays persisted, target/judge resolved server-side; step 5 `pass="record"` with the server gate ("read the verdict from the report — never assert one"); step 6 standing-preferences routing.
- Proposer prompt: the anchor grammar (the words between `- ` and ` [helpful: …]`).
- Settle: counters server-side; retire by disuse through `op: retire`; the 45/30-day rules as a meta-note tally; preferences with the auto switch and the per-night backup.
- Journal: `journal` once (a second call is a no-op); examples without the model names.
- Tool table: `dreaming_ledger` mutating; `skill_learned_edit{pass?}` + retire; `replay_case` persisted; `preferences_set` with the switch.

`routine.md`: `cap: mutating`; the prompt no longer promises a `<budget>` note, names the bounded tool surface and the `budget_left_tonight_usd` backlog rule.

## 3. Hand-offs

- **F1 (`crates/agent/src/skills.rs`)**: R-24 — `learned_path(dir, name)` must sanitise `name` to `[a-z0-9-]{1,64}` (or refuse in `index()`); dreaming's writer already refuses other names, so a sidecar can only exist under a conforming name, but a repo skill with `name: ../x` still splices `<dir>/x.md` on read. Test: skill `name: ../x`, `<dir>/x.md` beside `learned/` → `body()` does not contain `x.md`.
- **F1 (`crates/knowledge/src/tools.rs`)**: P §6 #2 / §6.2 #7 — `knowledge_verify` has no read-tool gate; the skill forbids it (twice) and now names `unverifiable`, but only a code gate (refuse unless the session's `inner_calls` since `knowledge_get(id)` include a read tool) makes `verified:` evidence. Not reachable from dreaming (the write happens inside the knowledge tool).
- **F1 (`crates/agent/src/loop_.rs`)**: P §6 #1 / §6.2 #8 — `<knowledge>` hints in the dreaming session record impressions; skip the note (or `record_shown`) when `knowledge.is_consolidating_session(session)` — the registration now exists from fire time, so the predicate is available in the loop.
- **F3 (`crates/desktop/ui`)**: `DreamSettings.resolved` is now `DreamResolvedModels | null` (`{tidy, propose, judge, target}` strings — the hand-written binding line is replaced by `just ts-desktop`); `NightSummary.status?: "budget" | "incomplete"` and `judge_same_model?: boolean` are new optional fields (a running row is `finished_at: null`; an ended-by-budget row says so); `DayLedger.min_sessions` is tool-only. `judge === propose` still means "same model" — the server's `judge_same_model` flag agrees with it.
- **Re-proof (P/integration)**: `sensors/dreaming.py` is v3 — the installed copy is rewritten at start; `signals.json` must carry a `written_at` younger than 3 min or the gate answers `signals stale` (the heartbeat is every 60 s). The routine file is patched at every start (`model:`, `cap:` appear on an old install).

## 4. Deviations and judgement calls

- **D14 `commit_before`**: P's per-event `KnowledgeChanged.commit` → parent (live-proven 315/315, 390/390 byte-identical) is kept as the primary rule; the knowledge HEAD captured at `BeginNight` (`head_at_start`) is the restore point only for an event without a commit (the old history walk is the last fallback). Strictly more precise than HEAD-at-start for an id a harvest also wrote during the night.
- **D7 `status: budget` detection**: no agent event says "refused"; `budget_hit` applies the guard's predicate (`Store::budget(session)` + `budget_spent(session, currency, session.model, since)`) at close time. Without an agent actor (tests) every cut-off night is `incomplete`.
- **D7 `last_dream_at` at start vs the ledger window**: `ledger::build` reads the window *before* `BeginNight` and a second pass reuses the first pass's `since`, so moving `last_dream_at` at start does not empty the night's own ledger. `finish` still sets it to the close time.
- **D2 rejected buffer**: one `RejectedEdit` per rejected proposal (its edits joined with ` | `) instead of one per edit — the gate is per candidate text, not per edit.
- **D4 retire**: on `skills/<skill>.md` with `origin: dreaming` in the front matter (not a filename prefix), so a human skill is refused by content; the sidecar `skills/learned/<skill>.md` is left (retiring the skill ends its loads; the sidecar is inert).
- **D8 veto**: `preferences_set` reads `auto_from_explicit` from the live `Night::Settings` (a `SetSettings` flip applies without a restart); the `Seams` copy is the fallback when the actor is absent.
- **D10 journal from a sync path**: `journal::append_sync` writes the file only; the document mirror catches up at the next `append` (at the latest the night's `Receipt`).
- **R-24**: applied to the writer (`[a-z0-9-]{1,64}`); skills named with `_` or `.` can no longer get a learned sidecar — the ruling's grammar.
- **R-35**: counted per merge commit (an event without a commit counts as its own merge); P's §6.2 "21 for 10 groups" would read 10.
- **R-37**: a created-by-dreaming id (`commit_before == ""`) is said once and then counts as undone, so a whole-night undo with only that error marks the summary `undone` (the old test asserted the opposite; re-pinned).
- **R-40**: hint-miss dropped from the skill's Mine list and the 45/30-day rules rephrased as a meta-note tally; `min_sessions` added to `DayLedger`. No `Night`-side 45/30-day computation (no per-id load history exists to compute it from).
- **R-33 plugin path**: `install_files(.., Some(&home), ..)` where `home = home_engine ?? engine`; not unit-tested (needs a mesh identity) — the rendering is.
- **P §6.2 #3 (Mine floor)**: P's close gate (`left_undone`) is kept; no stricter floor added (model-behaviour, not a code defect; a `Skipped` reason is still accepted).

## 5. Skipped (with the reason)

- **R-21, R-22, R-23, R-25, R-26, R-27, R-28, R-44, R-45** — F1's crates, done by F1.
- **R-34, R-48** — UI, done by F3 (`MAX_BUDGET_USD = 1000` mirrors D13 here).
- **R-5, R-6, R-7, R-1** — F1.
- **R-35 dedicated test** — the merge path needs a knowledge store merge publishing N events with one commit; covered structurally (`merge_commits`), not with a 4-into-1 fixture (the knowledge merge in a test is F1's surface; the `KnowledgeChanged` handler is exercised with a commit-less event).
- **D15 R-21/R-22/R-23/R-26/R-28** — F1.
- **P §6 #4 (links after undo), #6 (restore speed), #7 (126-char retitle)** — knowledge crate.
- **R-30 server gate on `preferences_set(add)`** — the review's "should require a passed gate for inferred lines": D8/D15 route inferred lines to a proposal (the user's press is the gate); explicit lines write when the switch is on. No replay-row requirement added to `preferences_set`.

## 6. Gates (all through `build-lock`, label `136-F2`, `CARGO_TARGET_DIR=…/W3`)

| Gate | Result |
|---|---|
| `cargo test -p dreaming` | 59 passed (42 before; 17 new/re-pinned), incl. `python_sensor_answers_the_same_table` (20 rows) and `python_sensor_reads_signals_under_the_read_file_header` with `/opt/homebrew/bin/python3` |
| `cargo test -p rpc --lib` | 52 passed |
| `cargo clippy -p dreaming --tests -- -D warnings` | clean |
| `cargo clippy -p rpc --tests -- -D warnings` | clean |
| `cargo fmt -p dreaming -p rpc -- --check` | clean |
| `grep -nE "shell|git log|delete_file" crates/dreaming/src/skill.md` | only the "no `shell`, `git`, `delete_file`…" prohibition and "there is no `git log` here" |

`t` was not built (the re-proof worker's job); `just ts-desktop` was not run (UI worker's gate) — `DreamResolvedModels` is exported from `rpc/src/export.rs`.
