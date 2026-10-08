# 139 W2 — RELEASE-DRAIN + RESUME-ON-TAKE, as built

Branch `feat/139-w2`, cut from C's contract commit `1afe827c`. D3, D4, D5, D15, D16, D19; Revision 1 Q2, Q4, Q5, Q10.

## What changed where

- `crates/agent/src/supervisor/transfer.rs` (all bodies):
  - `impl Message<DrainSession>` (`:77-118`) → `DelegatedReply<SessionDrainReport>`, off the mailbox like `Drain` (a turn ending inside the window asks `Agents`); marks `draining_sessions[id] = reason` first, spawns `drain_session` (`:120-179`): (1) `suspend_open_turn` writes `suspended{reason}` durably; (2) `Draining` told; (3) polls `is_running` to the deadline; (4) `Jobs::KillSession` (W3 body); (5) `LiveActor::stop`; then `TurnSuspended{reason}` when the turn is still open. `SessionDrained` (`:214-241`) brings `drop_queued`/`sessions.remove`/`running.remove` back onto the mailbox; a release holds the refusal for `2 × (window + 1 s)` or until `demoted()` (`:259`, told by `owner::demote`) via `ClearDraining` (`:243`).
  - `suspend_open_turn` (`:181`, pub): store-only step 1, reused by `owner::release` when no supervisor runs.
  - `stop_actor` (`:202`, generic over `Session`/`AcpChild`): `tell(CancelTurn)` + `stop_gracefully` + `wait_for_shutdown` (D19).
  - `draining_error` / `DRAINING_PREFIX` (`:64-75`): the `Err` string `route` returns for a draining session; W4 maps the prefix to `-32011`.
  - `on_resume_here` (`:276`): `Transfer | Takeover` → `child_waits::settle_on_transfer` (W3) + `settle_transferred_jobs` (`:295`).
- `crates/agent/src/supervisor.rs`: `draining_sessions: HashMap<SessionId, &'static str>` (`:172`, both constructors); `LiveActor` is `pub(crate)` with `stop()` (`:341`); `unload_session` (`:5918`) uses `stop()` — `kill()` stays only for `DeleteSession`/private-tree close; `route` (`:1198-1205`) refuses a draining session before any row is written (ticket released, no `AgentEvent::Error`, no transcript row); D15 hourly `journal_prune(Scope::Here, journal_keep_days)` loop in `start` (`:8275-8297`).
- `crates/agent/src/loop_.rs`: `Turn.resume_from` (`:2381`) set by the `Resume` handler (`:1317`); `TurnResumed.from` (`:4472`); `suspending()` (`:2961`) = shutdown token ∨ `shared.draining`, used at the stop-reason site (`:4568`, the §6.1 `:4530` change) **and** in `run_batch` (`:5455`) so a drain-deadline cancel leaves the batch as journaled (`started` rows stay, no "cancelled by user" message); `restart_note(plan, from, ..)` (`:7032`) spells `kind` from `plan.cause` and adds `from="<engine>"` for transfer/takeover. `report()` already wrote `cause` (C).
- `crates/sessionsync/src/owner.rs`: `release` → `drain_session` + `flush` (`:306`, `:419`); `ReleaseWaitSecs` ask on `SessionSync` (`:444-460`) gives the owner its own `release_wait_secs`; `release_window(secs − 1)`; `release_from` first wait = `release_wait_secs + durability.drain_secs` (`:236`, Q4); `Take::run` → `Take::take` (`:93-163`): taker-side `unload` deleted, step 9 `resume_on_take` (`:165`) by `resume_policy` (`:74`) → `ResumeTurn{cause: Some(..), from}` / `ResumePending` + local note / nothing; `demote` (`:330`) calls `transfer::demoted`.
- Tests: `owner.rs` — `take_from_a_reachable_owner…` seeds an open turn, asserts `suspended{release}` on B (`resume_plan` kind `Suspended`, cause `Transfer`) and `Resume(Transfer)` decided; `stale_owner_demotes…` asserts `Resume(Takeover)` with `kind = Interrupted`, 2 orphans; new `resume_policy_follows_the_table`, `system_takeover_leaves_the_open_turn_pending` (`ResumePending` on the bus via a `Sink` actor, the note, `Orphan`, no-open-turn → nothing). `durability_tests.rs` (in `drain_suspends_and_the_next_start_resumes`) — `release_drain_writes_suspended_or_finished` (a real `Session` actor through `drain_session`: suspended past the deadline / finished inside the window / nothing open), `cancel_while_draining_is_a_suspension` (token cancel while `draining` → no `turn_finished`, `started` stays; resumed with `kind="transfer" from="a"`, `TurnResumed{Transfer, from: a}`, transcript length equals the reference), `unload_always_lands_the_cancel` (100 live turns, `stop_actor`, each `TurnFinished{cancelled}`). `transfer.rs` — `release_window`, `draining_error` units.

## Deviations from §11 / the brief, and why

1. `DrainSession` reply is `DelegatedReply<SessionDrainReport>`, not `SessionDrainReport` (C's stub): the body waits up to `release_wait_secs − 1` for the turn, and the turn's own end asks `Agents` — on the mailbox it would deadlock, exactly `Drain`'s reason. `ask` callers see the same value. `drain_session(agents, m)` became `drain_session(store, rt, live, m)` + `SessionDrained` for the mailbox half.
2. `reaper::settle_transferred` (W3) does not exist in C's tree; `on_resume_here` carries `settle_transferred_jobs` over C's `Store::jobs_settle_transferred` + `Reaped{LostJob{reason: OwnerChanged}}` with `deliver: None`, `tail: ""`. **Closer:** replace the call with W3's fn when both land.
3. `release(store, id)` keeps its signature (`lib.rs` calls it) and learns `release_wait_secs` by asking `SessionSync` (`ReleaseWaitSecs`). Safe from `answer_release` (a spawned stream task). The dead `ReleaseSession` handler (no callers) would self-ask → **closer:** delete it or make it delegated.
4. No supervisor (sessionsync's tests, a pairing run): `release` still writes `suspended{release}` through `suspend_open_turn` (I3 holds without an actor). The owner.rs tests have no `Agents` (C deviation 6), so "B received `ResumeTurn{Transfer}`" is asserted as `Take::take`'s returned decision `ResumeOnTake::Resume(Transfer)`; the `tell` itself is wiring. "No actor existed on B" is not asserted (none can exist there).
5. The §6.1 loop change needed a second site: `run_batch` (`:5455`) also returned to `finish_batch` on a plain cancel, which wrote a "cancelled by user" batch message and closed every `started` row — the taker would then see `ToolBatch{all_finished: true}` and the mutating call reported as cancelled instead of interrupted. Same predicate, same file.
6. D16 `draining_sessions` is cleared on `demote` or after `2 × (window + 1 s)` (the §6.2 "fence interplay" rule), not at step 5 (§6.1's literal text) — for `reason == release` only; the refusal text names no taker (`Msg::Release` carries none, `next_owner = None`).
7. Q10 lands in the policy table as: `TakeReason::User` resumes; W4 passes `User` for a `Write` take from a real client peer. No fourth reason.
8. `restart_note` spells `kind` from `plan.cause` for every resume (`drain`/`parent_gone` → `shutdown`, Q2), so 109's wording is unchanged for single-engine resumes.
9. Working rules said `BUILD_LABEL=138` / commit prefix `138`; the design is 139 — used `139`. `./target/.ready` was a stale marker copied from the source target while the orchestrator's `cp -Rc` was still running (58 min in); the half-copied target failed with missing rlibs / colliding `StableCrateId`, so the copy was stopped, `target/debug` wiped, and the gates built cold.

## Gates

- `cargo test -p agent --lib -- loop_::durability_tests supervisor::`: **81 passed, 0 failed, 1 ignored** (`crash_child`, by design). The three W2 scenarios run inside `drain_suspends_and_the_next_start_resumes`.
- `cargo test -p sessionsync --lib`: **14 passed, 0 failed** (12 before W2; +2 new).
- `cargo clippy -p agent -p sessionsync --tests -- -D warnings`: clean. `cargo fmt -p agent -p sessionsync`: only W2's files changed.

## Open items for the closer

- Swap `settle_transferred_jobs` for W3's `reaper::settle_transferred` (deviation 2); `KillSession` is a no-op until W3 lands.
- W4: map `transfer::DRAINING_PREFIX` errors from `send_input`/`steer`/… to `SESSION_DRAINING = -32011` (`FrontDoorError::SessionDraining{next_owner: None}`); composer retry.
- `ReleaseSession` (sessionsync `lib.rs`, dead) self-asks after deviation 3.
- Q4/Q12 operational rule: one `sessions.*` config per mesh; upgrade every engine before the first post-0038 write.
- Z: T2/T3 end-to-end with two real engines (the in-process matrix has no `Agents` on the sessionsync side).
