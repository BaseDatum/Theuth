# 139 W3 — TRANSFER-RESOURCES, as built

Branch `feat/139-w3`, cut from the contract commit `1afe827c`. Owns only the §11 W3 files; C's
signatures unchanged. `pty.rs`/`tjobs.rs` = `crates/tools/src/{pty,jobs}.rs`; `reaper.rs` =
`crates/agent/src/durability/jobs.rs`; `child_waits.rs` = `crates/agent/src/supervisor/child_waits.rs`;
`resume_calls.rs` = `crates/agent/src/loop_/resume_calls.rs`.

## What changed where

- `tjobs.rs:433-442` `KillSession` doc; `:1228-1301` `KillAll` and `KillSession` share one body,
  `Jobs::kill_lost(only: Option<SessionId>)` — rows `exit = lost` through the journal first, PTY
  `snapshot_to_store`, then `kill(pgids)` + `task.abort()`; charges released per job
  (`saturating_sub`, `KillAll` used to zero the counter). `KillSession` additionally marks the
  in-memory job `exit = Lost` so the old owner's `jobs()` lists it `lost` and a later `KillAll`
  does not rewrite its row under a fence it no longer holds. `:592-599` `Lost.reason: LostReason`;
  `:1303-1384` the `Reaped` handler stores it. `Reaped`/`LostJob`/`LostReason` derive `Clone`
  (`Debug`), needed so `settle_transferred` can tell the actor and return the rows.
- `pty.rs:1183-1201` `lost_page` banner by `LostReason`: *"[process gone — the session moved from A
  at hh:mm:ss; this is the last screen captured on A at … (revision N)]"*; `:1493-1510` `SendInput`'s
  refusal: *"the terminal is gone (session moved from A at hh:mm:ss); last screen follows"*.
- `reaper.rs:45-129` `reap_jobs` → `tell_jobs(Reaped)` (shared); `settle_transferred(store,
  session, from) -> Reaped`: `jobs_settle_transferred(session, store.engine(), now)` (no adopt, no
  killpg), replicated screen per PTY, `in_batch` from `open_turn(session)`'s `started` calls,
  `LostJob{reason: OwnerChanged{engine: row.engine}}`, tells the Jobs actor, returns the rows.
  `:223-269` `lost_body(.., reason)`: *"owner changed (ran on A): the session moved here at hh:mm:ss;
  the job stayed behind and its exit code is unknown."*; the two 109 bodies unchanged.
- `child_waits.rs` (all bodies): `child_spawned` → `wait_put(ChildResult, target = child,
  payload {"agent"})`, skipped for an empty `call_id` (no tool call spawned it); `child_result_delivered`
  → `child_waits(Scope::Here)` filtered to `parent` and `target == child` → `wait_clear`;
  `settle_on_transfer(agents, parent, from)`: `from` reachable (`transport().peers()` contains it,
  the same check `mesh::forward` makes) → nothing; unreachable (or no mesh) → for each wait whose
  child row has `!here && owner == from`: `wait_clear` + `Agents::inject(parent, "session:<child>",
  body, <message from="session:<child>" at=…>)`, body = *"<name> (session:<id>) was running on A, which
  is unreachable; its result is lost. "* + `compose_handoff(store, child, handoff_tokens)` when
  `row.cwd` is a directory here.
- `resume_calls.rs:23-44` `interrupted_note` spells the cause via `how(plan)`: `transfer` → *"the
  session moved to this engine at …"*, `takeover` → *"this engine took the session over at …"*,
  else 109's *"the engine restarted at …"*; `:100-128` job note when `row.engine ∉ {'', me}`: *"the
  session moved here from A at … while this command ran (job j1, started …); the job stayed on A
  and its exit code is unknown here"*; `:150-181` lost `wait` banner says *moved from A* / *captured
  on A*; `:311-319, 351-366` Q7: no row / no receipts after `transfer|takeover` → *"waiting for a
  reply was interrupted — …; the reply, if any, arrives as a <message> and is not lost. Do not resend
  the message."*

Tests: `tjobs.rs` `kill_session_writes_lost_before_kill_and_spares_others`; `pty.rs`
`screen_after_transfer_names_the_previous_owner`; `reaper.rs` `reaper_ignores_foreign_engine_rows`,
`settle_transferred_never_signals` (a real `sleep 600` group stands in for A's; alive after both);
`child_waits.rs` `child_result_wait_written_and_cleared`, `settle_on_transfer_injects_the_lost_note`
(reachable / `from = None` / unreachable; with and without `cwd`; a `c`-owned child untouched);
`resume_calls.rs` `notes_spell_the_cause`, `job_note_names_the_engine_that_ran_it`.

## Gates

- `cargo test -p agent --lib -- durability:: supervisor::child_waits loop_::resume_calls`: 24 passed,
  0 failed (6 new).
- `cargo test -p tools --lib -- --test-threads=1 jobs:: pty::`: 52 passed, 0 failed (2 new).
- `cargo clippy -p agent -p tools --tests -- -D warnings`: clean (first run caught
  `zombie_processes` in the new test helper; the child is now `wait`ed).
- `rustfmt --check` on the five owned files: clean (ran `rustfmt` on those files only; `cargo fmt -p`
  would reflow unrelated spots C recorded as not fmt-clean).

## Deviations from §11 / the brief and why

1. `settle_transferred` both **tells the Jobs actor and returns** the `Reaped` (hence the `Clone`
   derives): §7.1 says "the rows reach B's Jobs actor as `Reaped`" and W2's dispatcher only gets a
   return value; W2 needs nothing but `let r = settle_transferred(..).await; r.jobs.len()`.
2. `interrupted_note` (generic §4.3 note) also spells `transfer`/`takeover`; 109's crash/shutdown
   wording is byte-identical. The design listed only the peer-wait and job notes for W3, but a
   mutating `started` call after a transfer saying "the engine restarted" would be false.
3. The Q7 note cannot name the previous owner: `ResumePlan` carries `cause` but not `from` (C froze
   it; `Resume.from` stops in `loop_.rs`, W2's). It says "the session moved to this engine at …".
4. `child_spawned` writes nothing for an empty `call_id` (C's supervisor tests build `SpawnChild`
   with `call_id: String::new()`; `waits`' key is `(session, call_id)`, so they would collide).
5. `compose_handoff`'s header is addressed to the child ("Your previous session with this harness…");
   §7.4 asked for it verbatim after the lost note, so it is appended as is — a parent-facing render
   is a follow-on if the wording matters.
6. `KillSession` keeps the session's jobs in the old owner's actor as `lost` instead of removing
   them (`CloseSessions` shape): removing would restart `next` numbering and collide with the
   replicated ids if the session comes back.
7. Build: the warm-target `cp -Rc` into `./target` was still running past the 15-min wait and
   clobbered a cold build (`can't find crate for uucore`); built cold under `CARGO_TARGET_DIR=./target/w3`
   instead (inside my worktree, outside the copy). `BUILD_LABEL=139` per the prompt (brief text says 138).

## Open items for the closer / W2

- W2's `on_resume_here` must call `settle_transferred(&agents.store, session, from)` and
  `child_waits::settle_on_transfer(agents, session, from)` for `cause ∈ {Transfer, Takeover}`; the
  `Reaped.jobs.len()` is the `lost_jobs` count for the resume report.
- `jobs_settle_transferred` (C) overwrites A's `settled_ms` with B's `now`; the owner-changed body
  therefore shows the resume time, not A's kill time. Acceptable; noted.
- `rebuild_waits`/`ResumeWait` (W1) still answer `child_result` waits from spared results; a wait
  this wave settled lost is already deleted, so no double report.
