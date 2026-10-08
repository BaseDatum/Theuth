# 139 C — contract, as built

Branch `feat/139-mesh-durability`, cut from `7d6832ba`. Signatures below are frozen for W1–W4.

## What changed where

- `crates/agent/migrations/0038_mesh_durability.sql` (new): `jobs.engine TEXT NOT NULL DEFAULT ''` + backfill from `sessions.owner`; `resume_reports.cause TEXT NOT NULL DEFAULT ''`. 0038 verified free on main (latest `0036`) and in `theuth-137*` (uses `0037`). Applied by hand to a `.backup` copy of `~/.theuth/sessions.sqlite` (version 36, 2376 jobs → all `engine = <owner>`, 10 reports → `cause = ''`, 0 rows where `engine != owner`).
- `crates/agent/src/store/capture.rs:12-36`: `ATTACHED` → 17 entries, `+ ("pty_screens", "session_id")`; doc comment rewritten.
- `crates/agent/src/store/durability.rs`: `Scope { Here, All }` (no `Default`) with `Scope::sql()` (`?1` = `Store::engine()`, Q1 `here` form); `open_turns(Scope)`, `open_turn(session)`, `jobs_open(Scope)` (+ `engine IN ('', me)` for `Here`), `jobs_settle_transferred(session, me, now_ms) -> Vec<JobRow>` (`UPDATE … RETURNING`), `child_waits(Scope)`, `journal_prune(Scope, keep_days)` (the `fin` CTE **and** the questions delete scoped — an unscoped prune on a replica would hit the fence and fail whole); `JobRow.engine` in `JOB_COLS`/`job_put`; `ResumeReportRow.cause` in `REPORT_COLS`/`resume_report_put` (`''` → boot rule by `kind`); `ResumePlan.cause` derived in `resume_plan` from the newest `suspended{reason}` (`ResumeCause::from_journal`); `cut_journal_in` skips `kind = 'child_result'` (Q8); `pty_screen_put` doc line. Tests: `open_turns_here_excludes_foreign_owners` (also `open_turn` any-owner, `child_waits`, scoped prune leaves `a`'s finished turn, `All` prune is fenced), `jobs_open_here_filters_by_engine` (also `jobs_settle_transferred`), `pty_screen_rows_replicate` (inverts `journal_rows_replicate_but_screens_do_not`, deleted). `store.rs:63` re-exports `Scope`.
- `crates/agent/src/durability/mod.rs`: `ResumeCause { Crash, Shutdown, Update, Transfer, Takeover }` + `as_str/parse/from_journal`, `ts(export)`; `ResumePlan.cause`; `ResumeReportRow.cause`.
- `crates/agent/src/durability/journal.rs:61-65`: `StoreJournal::job` stamps `engine = store.engine()`.
- `crates/agent/src/durability/jobs.rs`: reaper reads `jobs_open(Scope::Here)` / `open_turns(Scope::Here)`; `LostJob.reason = Restart` (mechanical only; W3 owns the logic).
- `crates/agent/src/lib.rs`: `ResumeState.{owner: Option<String>, here: bool (default true), cause: Option<ResumeCause>}`; `TurnResumed.{cause: ResumeCause (wire default crash), from: Option<String>}`; `+ AgentEvent::ResumePending { session, turn, from: String }` in `session()`.
- `crates/agent/src/loop_.rs`: `Resume { plan, slots, cause: ResumeCause, from: Option<String> }` — the handler writes `cause` over `plan.cause`; `TurnResumed{cause: plan.cause, from: None}` (W2 fills `from`); `report()` writes `cause`; `settle_open_turns` → `open_turn(session)`.
- `crates/agent/src/supervisor/resume.rs`: `ResumeTurn { session, turn, cause: Option<ResumeCause>, from: Option<String> }` (`None` = boot rule from the plan); `resume_turn(session, turn, slots, cause, from)`; `resume_scan` → `Scope::Here`; `OpenTurn` → `open_turn`; hook `transfer::on_resume_here(self, session, cause, from)` after `resume_plan`.
- `crates/agent/src/supervisor/drain.rs`: `OpenTurns(pub Scope)`; both drain loops `Scope::Here`; `resume_state` → `open_turn`, fills `cause`, leaves `owner: None, here: true` for W1.
- `crates/agent/src/supervisor/waits.rs`, `acp_resume.rs`: `open_turns(Scope::Here)` (W1 rewrites bodies).
- `crates/agent/src/supervisor.rs`: `pub mod child_waits`, `pub mod transfer`, re-export `DrainSession`, `SessionDrainReport`; `UnloadSession` body factored into `Agents::unload_session` (the stub drain calls it); `SpawnChild.call_id: String` (set from `cx.call_id` in `peers.rs:260`); hooks `child_waits::child_spawned` (SpawnChild success, before `SubagentSpawned`) and `child_waits::child_result_delivered` (top of `hand_off`).
- New stubs: `crates/agent/src/supervisor/transfer.rs` (`DrainSession{session, reason, deadline}` ask → `SessionDrainReport{open, finished, suspended}`; `drain_session` today = `unload_session`; `release_window(release_wait_secs: u64)`; `on_resume_here` no-op) — W2. `crates/agent/src/supervisor/child_waits.rs` (`child_spawned`, `child_result_delivered`, `settle_on_transfer` no-ops) — W3.
- `crates/tools/src/journal.rs`: `JobRow.engine: String` (`#[serde(default)]`). `crates/tools/src/jobs.rs`: `KillSession { session }` + no-op handler (W3 body), `LostReason { Restart (default), OwnerChanged { engine } }`, `LostJob.reason`.
- `crates/rpc/src/forward.rs`: `SESSION_DRAINING = -32011`, `FrontDoorError::SessionDraining { session, reason, next_owner }`. `api.rs` `resume_turn` sends `cause: None, from: None`.
- `crates/roster/src/attention.rs`, `crates/schedule/src/lib.rs`: `OpenTurns(Scope::Here)`. `crates/slack/src/channel.rs`, `crates/tui/src/model.rs` (test), `crates/rpc/src/api.rs` (wire test): mechanical.
- `crates/desktop/ui/src/rpc/bindings/`: regenerated (`ResumeCause.ts` new; `ResumeState`, `AgentEvent`, `FrontDoorError` grown).

## Deviations from §11 C and why

1. `0033:94-95` "Engine-local" comment **not edited**: `sqlx::migrate!` checksums applied migrations; the supersession is noted in 0038's comment instead.
2. `ResumeTurn.cause` is `Option<ResumeCause>` and `ResumePlan` gained `cause`: the boot rule needs the `suspended` row's reason, which only `resume_plan` reads — callers pass `None` instead of re-deriving it. `ResumeTurn`/`Resume` also carry `from: Option<String>` (the previous owner is not recoverable from the store after the take; `TurnResumed.from` needs it).
3. `ResumeReportRow.cause` is non-optional: a pre-139 `''` row reads as `crash`/`shutdown` by `kind`.
4. `jobs_settle_transferred` uses `engine NOT IN ('', me)`, not `!= me`: a `''` row is this engine's (Q14, same rule as `jobs_open(Here)`).
5. `Scope::All` SQL still references `?1` (`(?1 IS NOT NULL)`) so both scopes bind the same arguments.
6. `agent` feature `test-support` / `pub mod testing` **not added** (Q13 decision point): `supervisor::tests` (~3 000 lines, `agents_on`) and `loop_::durability_tests` (`Harness`, `BatchFake`) depend on dev-only crates (`wiremock`, `figment`) and on dozens of `#[cfg(test)]`-gated helpers (`Auth::stub`, `Agents::registry → Ok(None)`, `render`, `PermitsOf`, …); exporting them under a feature means gating all of those and carrying the `#[tokio::test]` bodies as dead code under `-D warnings` — a refactor. Z keeps the unit tests and runs T1–T7 as the scripted two-engine smoke.
7. `cargo clippy --workspace --tests` / `cargo build --workspace`: see gate numbers below for what actually ran.

## Gates (numbers)

Filled in at the end of the run — see the final section.

## Open items for the closer

- Q12 operational rule: upgrade every engine of a mesh before the first post-0038 write ships (a `jobs`/`resume_reports` changeset with the new column fails to apply on an unmigrated replica).
- `resume_state` still returns `owner: None, here: true` (W1); `TurnResumed.from` is `None` (W2); `on_resume_here`, `child_waits::*`, `KillSession` are no-ops (W2/W3); `drain_session` unloads as before (W2).
- Z: in-process harness not available from `agent` (deviation 6) — scripted smoke.
