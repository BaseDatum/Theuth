# 139 W1 — OWNER-SCOPE (D1, D2, D13 gate, D17; Revision 1 Q1, Q3, Q6, Q9), as built

Branch `feat/139-w1`, cut from C's contract commit `1afe827c`. Coded against C's frozen signatures; no C or sibling-wave file edited except the Revision 1 Q3 guard in `supervisor.rs` (assigned to W1 by the brief).

## What changed where

- `crates/agent/src/supervisor/resume.rs:109-169` `resume_turn`: after `resume_plan` and the `Finished` check, `store.session(session)` → a row that is not `here` returns with a `warn!` (139 I1 second line of defence) before `self.session`; for `cause ∈ {Transfer, Takeover}` `rebuild_waits_for(session)` runs first (§7.3), then `transfer::on_resume_here` (W2/W3), then the actor + `Resume`. `resume_scan` → `open_turns(Scope::Here)` and `OpenTurn` → `open_turn` were already C's. Tests `:172-262`: `pub(crate) b_holding_a_turn(communication)` (engine `a` writes a session + open turn, closes; `agents_on(dir, Some("b"))` reopens the file) and `own_turn` — the two-store shape every W1 test uses; `scan_and_resume_turn_skip_a_foreign_owners_turn`.
- `crates/agent/src/supervisor/drain.rs:241-317` `resume_state`: reads the `sessions` row once; `owner = Some(row.owner)` when non-empty, `here = row.here`, in both the open-turn and the latest-report branch (no row / error → `None, true`). `Drain`'s two loops and `OpenTurns(Scope)` were C's. Test `drain_suspends_own_open_turns_once` (`:384-449`) replaces the single-engine drain test: `b` drains its own turn only, `a`'s turn stays `Interrupted`, exactly one `TurnSuspended`, `OpenTurns(Here) == [b's]`, `OpenTurns(All).len() == 2`, `resume_state(a's) == (Some("a"), false)` and `(Some("b"), true)` for its own.
- `crates/agent/src/supervisor/waits.rs:99-134, 136-228`: `rebuild_waits` (boot, `Scope::Here`) and new `rebuild_waits_for(session)` (`open_turn(session)`) share `rebuild_waits_of(session, turn, approval)` returning `Counts`; `approval_actor()` replaces the inline lookup; `rebuild_waits_with` is gone (its test calls `rebuild_waits_of`). `deliver_spared` (`:279-329`): `mirror(child)` before the route (as `resolve` does), then a parent with `remote_owner` is forwarded through the existing `forward_peer` (a `Deliver` from the child, `await_reply: false`), never enqueued locally. Tests: `rebuild_waits_for_one_session_leaves_others`, `spared_result_for_a_foreign_parent_is_forwarded_not_injected` (no mesh → receipt `failed` with the `OwnerUnreachable{"a"}` text, no actor, no ticket); `open_batch` now takes `&Store`.
- `crates/agent/src/supervisor/acp_resume.rs:27-49` `respawn_acp_children`: iterates `store.child_waits(Scope::Here)` (parent-owner scoped, no open-turn JOIN), dedup by child. `respawn_acp_child` (`:57-120`) gates in order: `row.here` → local harness entry → `row.cwd` readable (`read_dir`) when recorded; each refusal is one `warn!`. `respawn_decision_table` rewritten two-store: `+ taken` (child owned by `a`) and `+ bad_cwd` rows, `a`'s own parent wait is not `b`'s trigger; the positive row has `cwd = dir`.
- `crates/roster/src/attention.rs:198-211` `ResumeNeed.taken_from: Option<String>`; `:744-748` title *"<name> was taken from <A>"* (detail `"resume?"`); `:1434-1471` `resume_transition(reports, ev, now)`: `ResumeReport` with gaps / `ResumePending` insert, `TurnResumed` / `TurnFinished` remove (Q9); the `AgentEvent` handler (`:1476-1482`) calls it and still runs the 130 E3 arms for `TurnFinished`. Test `takeover_pending_item_raised_and_cleared`.
- `crates/agent/src/store.rs:554-566`: the D17 backfill logs `info!("68: claimed {n} unowned session(s) as {me}")` when `n > 0` (Q6: log only).
- `crates/agent/src/supervisor.rs:1312-1323` `inject`: `remote_owner(to)` → `warn!` and return before any ticket / `route` (Q3). `:2315-2324` `rearm_reminders`: a row whose session is not `here` is skipped (not deleted — it is the owner's); no row / error deletes as before.
- `crates/schedule/src/lib.rs`: nothing beyond C's `OpenTurns(Scope::Here)`.

## Deviations from §11 / Revision 1 and why

1. **Q3 `inject` drops instead of forwarding.** "Forward as `Agents::deliver` does" needs a sender session: `PeerEnvelope.from: SessionId`, the owner's `peer_deliver` runs `deliver` → `resolve` → `Policy::route(from, to)`, and `route` refuses `from == to` (`SelfRoute`). A harness-made `<job_result>`/`<reminder>` has no sender session, and the owner would re-wrap it as a `<message>`. After D1 every `inject` source is owner-scoped (reaper engine filter W3, spared results above, reminders gated here), so the guard is a backstop: it logs and drops, never spawns. A harness envelope on `theuth/rpc/1` would be a 69 change — closer/follow-on.
2. `respawn_acp_child`: a child with **no recorded `cwd`** passes the cwd gate (the lazy `acp_session` path uses the harness default); only a recorded `cwd` must be a readable directory. Strict "must be a directory" would refuse every pre-cwd row.
3. Attention: `TurnFinished` clears the session's `resume:` report for *any* turn (the map is keyed by session, as before); `TurnResumed` clears it too. A `ResumeReport` with gaps published after the `TurnResumed` of the same turn re-inserts it, so the gaps item still shows for the resumed turn's duration.
4. `deliver_spared` gained `mirror(child)` before `route` — the existing boot path routed without the lineage (`UnknownSession` on a freshly opened store); the forwarding test needed it, production gets it too.
5. Commit prefix `139 W1:` and `BUILD_LABEL=139` (the brief's working-rules text says 138; the task is 139).
6. Build: the warm-target clone (`cp -Rc` of a 69 GB source) was still copying when `.ready` appeared (the flag was copied from the source); after 15 min+ the partial target failed with missing rlibs, so the clone into `./target` was stopped and `target/debug` built cold (brief's fallback).

## Gates (numbers)

- `cargo test -p agent --lib -- supervisor::resume supervisor::drain supervisor::waits supervisor::acp_resume`: 11 passed, 0 failed (+4 new tests; `drain_suspends_open_turns_once` → `drain_suspends_own_open_turns_once`).
- `cargo test -p roster -p schedule --lib`: roster 67 passed (+1 new), schedule 72 passed, 0 failed.
- `cargo clippy -p agent -p roster -p schedule --tests -- -D warnings`: clean. `rustfmt --check` clean on the four `supervisor/*.rs` files; `attention.rs` has pre-existing drift outside W1 lines (not reformatted, as C noted for main).

## Open items for the closer

- Q3 forwarding form (deviation 1) if a harness result for a transferred session must reach the owner rather than be dropped with a log.
- `supervisor.rs` is W2's file: W1 touched only `inject` (`:1312-1323`) and `rearm_reminders` (`:2315-2324`); W2's regions (`UnloadSession`, `draining_sessions`, the input handler, `journal_prune` loop) are disjoint — expect a clean merge, verify.
- `resume_turn`'s `rebuild_waits_for` runs before W2's `on_resume_here`; if W2 wants the jobs/child-waits pass first, swap the two calls in `resume.rs:137-146`.
