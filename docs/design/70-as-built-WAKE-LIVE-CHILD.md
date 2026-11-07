# 70 WAKE-LIVE-CHILD — a desk's wake is never finished after a restart; a desk waiting on its children is swept as lost (as built)

Branch `fix/desk-wake-live-child` (worktree `theuth-wt-wake`, off `7e514107` = v2.7.0). Brief: `/tmp/mc-2026-10-05/omp-WAKE-LIVE-CHILD.md`. Read with `70-as-built-STALE-WAKE-DESKS.md` (release/finish of wakes, desk reuse), `70-as-built-DESK-PROTOCOL.md` (`Serving`, `judge_turn`, the nudge table), `67-agents.md` §7.3/§7.5. Corrects `brief-routine-wake-delivery.md` (2026-10-05): its root cause was wrong — the schedule fire and the wake were fine; the desk never finished the wake it already held.

## Incident, traced to code (UTC, 2026-10-05; live data read only)

| when | what | code |
|---|---|---|
| 15:19:18 | restart; `desk opened agent=nikolai` (reuse → `ReleaseClaims`) | `roster/lib.rs` `open_desk` |
| 15:19:21 | `desk opened agent=theuth`: a wake was dispatched 3 s after start — the sweep released the pre-restart claim (`desk_for` is `None` in a fresh process) and `dispatch()` re-picked `mention` wake `01a10c9c-6181` on task `01a0e8ba-e7f5` | `ledger.rs` `sweep` / `dispatch`, `DeliverWake` |
| turn #40 | theuth's desk answers "Duplicate wake … Nothing to do": one `doc_read`, no `task_note`, no task write; turn ends `end_turn` | — |
| never | the end-of-turn finish. `judge_turn` asked `ListAgents { session }` and called every descendant with `!Idle \|\| finished_at.is_none()` a live child. `finished_at` is the supervisor's in-memory `last_stop`; after a restart every pre-restart child (8 under theuth's desk, 2 under nikolai's) is `Idle` with `finished_at: None` → `live_child == true` for ever → return before `clear_serving`/the nudge → `Desk.serving` keeps the wake → row stays `claimed_by = <desk>` → `dispatch()` skips every other wake of the agent as busy (4 theuth + 1 nikolai waiting at 15:5x). Every later `TurnFinished` of the desk re-judged the same stale `serving`. | `roster/lib.rs` `judge_turn` (old L754-776), `supervisor.rs` `info_row`/`state_of` |
| 15:35–15:36, 15:52 | all five SRE desks orphaned 16 min after the restart; sergei's twice while two omp workers ran `pulumi preview` | `ledger.rs` `sweep`: `now - progress_at_ms >= stale_secs` with nothing asking whether the session or its children run |

dmitri was not stuck: his `resume` wake was finished by `complete_wake` when he wrote the task (`update()`), which bypasses the roster. A desk that answers with `task_note()` only (what the nudge text tells it to do) depends entirely on `judge_turn`.

Two more holes closed on the same path: a non-yielded turn (`cancelled | timed_out | error`) left `serving` + the claim in place ("the next wake resumes it" — but no next wake can be dispatched while the claim stands); nothing logged while an agent was skipped as busy.

## D1 — roster: a cold child is not a live child (the fix)

- `crates/roster/src/desk.rs:336-338` — `Serving.children: HashSet<SessionId>` (direct children spawned under the desk while this wake is served) and `Serving.logged_live_child: bool`. `DeliverWake` creates both empty (`lib.rs:1346-1347`); the `SubagentSpawned { parent, child }` arm inserts the child next to `rounds += 1` (`lib.rs:1414`).
- `desk.rs:483` — `nudges::live_child(s, kids)`: any kid `Running | Awaiting{..} | Approval{..}`, or `s.children.contains(&kid.id) && kid.finished_at.is_none()`. A cold descendant not spawned for this wake never counts, whatever its `finished_at`.
- `lib.rs:706` `children_of(session)` (the `ListAgents` ask, empty without the actor); `judge_turn` (`lib.rs:736`) uses `live_child(&s, &kids)`. The deferral is logged once per serving (`logged_live_child`, `lib.rs:811`): `turn not judged: a child of the desk is still running` with `agent, task, wake, children`.
- `AgentInfo`, `SessionActivity`, `AgentState` unchanged; the supervisor invents nothing for cold sessions.

## D2 — roster: a non-yielded turn releases the wake

- `lib.rs:737-757` — `!yielded(stop_reason)` with a `serving`: take it, `info!(agent, task, wake, stop_reason, "wake released: turn did not yield")`, `tell tasks::ReleaseWake { wake, min_backoff_ms }` — `300_000` for `cancelled`, `0` (the 5 s-doubling delivery back-off) for `error | timed_out`. The task's checkout is untouched.
- `crates/tasks/src/lib.rs:335` `ReleaseWake` (tell, next to `FinishWake`/`ReleaseClaims`), handler at `lib.rs:787`.
- `ledger.rs:1600` `release_wake(&wake, min_backoff_ms)`: `retry[id]` attempts + 1, `not_before = now + max(5 s << attempts, cap 5 min, min_backoff_ms)`, `store.claim(id, None, now)`. `delivery_failed` (`ledger.rs:1592`) is `release_wake(wake, 0)` — one implementation.

## D3 — ledger: the busy skip and the schedule path are visible

- `ledger.rs:138` `Ledger.reported_busy: HashMap<agent, blocking wake id>`. `dispatch()` (`ledger.rs:1474-1497`): entries of agents no longer busy are dropped; for each claimed wake with ≥ 1 unclaimed wake of the same agent and `reported_busy[agent] != id`: `info!(agent, behind, reason, task, claimed_by, waiting, "wakes held: desk still serving an earlier wake")`, recorded. One line per (agent, blocking wake). `ponytail:` O(claimed × wakes) scan over a table of a handful of rows.
- `ledger.rs:524,546` — `schedule fire: wake re-raised on the existing task` / `schedule fire: task created, wake raised` (`schedule, agent, task`).
- `lib.rs:865` — Dispatch handler: `info!(agent, wake, task, reason, "wake delivered")` on `Ok` (the `warn` on failure existed).
- `task_inbox` line: **skipped**. No actor message lists wakes (`Ledger::wakes()` is a method; the tool talks to the actor only), so the line needs a new `ListWakes` ask + handler + the tool text — over the brief's 15-line budget. The D3 log lines carry the same information for whoever reads the engine log.

## D4 — ledger sweep: a checkout with live work is not stale (ask 2)

- `ledger.rs:87-100` — `Liveness { active_now, last_active_ms }`, `trait LivenessProbe { async fn liveness(&self, session) -> Option<Liveness> }`, next to `SessionTaker`. `Ledger.probe: Arc<dyn LivenessProbe>` (`ledger.rs:127`), default `crate::AgentProbe` in `Ledger::new`; the test rig overrides it with `MemProbe` (`ledger.rs:1977`) the same way it sets `taker`.
- `crates/tasks/src/lib.rs:946` `AgentProbe`: own `agent::ActivityOf([id])` + `ListAgents { session }`. `active_now` = `session_active(own) || kids.any(session_active(k.activity))` where `session_active` (`lib.rs:931`) = `turn_started_at.is_some() || state ∈ {Running, Awaiting, Approval} || jobs + ptys > 0`. `last_active_ms` = max of own and every descendant's `activity.last_active`. `SessionActivity.children` is not used. `None` when there is no agent actor or the id does not parse.
- `crates/agent/src/supervisor.rs:521,4858` — `ActivityOf(pub Vec<SessionId>)` → `HashMap<SessionId, SessionActivity>` over the existing `activity_for`; re-exported from `agent`; internal, not ts-exported.
- `ledger.rs:1691-1701` — staleness arm: the probe is asked only once `now - progress_at_ms >= stale_secs` (the max can only push staleness later); then `stale = !active_now && now - max(progress_at_ms, last_active_ms.unwrap_or(0)) >= stale_secs`. The unreachable-engine arm is unchanged. `None` → today's rule.
- `ledger.rs:1738-1750` — the orphan note reads `session … is presumed lost; last activity N min ago; …` when the probe answered.

## D5 — roster: a desk with running children is not idle

- `desk.rs:508` — `Desk::idle_due(now_ms, idle_secs, live_work: bool)`, still pure.
- `lib.rs:1185-1198` — Tick: for each desk the clock alone would close (`idle_due(now, secs, false)`), ask `tasks::AgentProbe` (the D4 probe — one implementation of "this session or its subtree is at work") and keep the desk open while `active_now`. Probed only for the clock's candidates, so an idle 6 h desk costs one `ListAgents` per tick, a busy one nothing.

## D6 — docs

This file; `judge_turn`'s doc comment (`lib.rs:719-735`), `sweep`'s (`ledger.rs:1653-1660`), `nudges::yielded`'s (`desk.rs:493-497`) and the `stale checkout` row of `nudges::TABLE` (`desk.rs:462-468`) state the new rules.

## D7 — `schedule()` and `agent:` — verified only

`git grep -n 'agent' crates/schedule/src/tools.rs crates/schedule/src/lib.rs` + the routines UI (`crates/desktop/ui/src/components/routines/*`): no stamping path exists; `tools.rs:182` writes `agent: None`, `ScheduleArgs` has no `agent` field, and the UI's Agent select writes only what the user picks. The key on the live routines was written by hand (or chosen in the UI). Nothing changed in `crates/schedule`.

## Tests

- T1 `roster::desk::tests::cold_children_are_not_live` — the five cases of the brief plus `Awaiting` and an empty list.
- T2 — **not an end-to-end roster test**: `judge_turn` needs the tasks actor (`GetTask`, `TaskEvents`) and the agent actor (`SendInput`), neither of which the roster harness hosts (`Not built` in DESK-PROTOCOL still holds). Built the brief's fallback: `children_of(session)` is the one `ListAgents` seam and the judgement is `unhandled(&s, &task, &mine, live_child(&s, &kids))` — the pure composition of two tested functions (`nudge_table_judges_turn_ends` for the table, T1 for `live_child`).
- T3 `tasks::ledger::tests::release_wake_unclaims_with_a_floor` — floor 300 s honoured (skipped at 299 s, delivered at 301 s); with `0` the 5 s back-off applies.
- T4 `tasks::ledger::tests::busy_skip_is_reported_once_per_blocking_wake` — set once across two passes, gone after `finish_wake` → the comment wake is delivered.
- T5 `tasks::ledger::tests::checkout_with_live_work_is_not_stale` — `active_now` with an hour-old note → kept; last active 2 min ago → kept; 20 min ago → orphaned with `resume` and `last activity 20 min ago` in the thread; unknown session → today's rule at 901 s.
- T6 `schedule_fire_creates_coalesces_and_closes` extended (the brief's `agent_fire_wakes_desk`): the first fire's `Schedule` wake dispatches for `ops` with its task; the second fire's waits (`reported_busy["ops"]` = the first id) and dispatches after `finish_wake`.
- T7 `roster::desk::tests::desk_with_running_child_is_not_idle`.
- T8 `agent::supervisor::tests::activity_of_returns_rows_for_the_asked_ids` — exactly the asked ids, a cold child with the store's `updated_at`, empty for `[]`.
- Existing: `terminal_transition_drops_wakes_and_frees_the_reviewer`, `stale_wake_is_dropped_at_dispatch_not_delivered`, `finish_wake_and_release_claims`, `episode_closing_blocks_leftover_tasks`, `orphan_by_staleness`, `orphan_by_unreachable_engine_then_superseded`, `lease_events_are_ignored_during_shutdown`, `nudge_table_judges_turn_ends`, `idle_rotation_uses_the_given_clock` (signature updated) — all green.

## Deviations (nobody to ask)

- **`task_inbox` line not built** (D3, see above): no seam under 15 lines.
- **T2 is the fallback form** (see Tests).
- **One probe for sweep and Tick.** D5 says "the same running-now input"; rather than a second `ListAgents`/`ActivityOf` reading in the roster, the Tick calls `tasks::AgentProbe` directly (roster already depends on tasks). `Desk::idle_due` stays pure with the extra `bool`.
- **The probe is asked lazily** in the sweep (only once `progress_at_ms` alone is stale) and in the Tick (only for the clock's candidates). `last_active` can only make a checkout *less* stale, so the result is identical and a sweep over a busy board stays one query.
- **`release_wake` counts an attempt.** "max(current back-off, floor)" read as: the attempt counter advances as for a failed delivery, the floor is applied on top. A successful delivery (`delivered()`) still clears the counter, so error → 5 s, two errors → 10 s, cancel → 5 min.
- **Schedule re-raise log** added next to the brief's "task created" line (`schedule fire: wake re-raised on the existing task`, with the count) — the same path, one line.
- **`MemProbe` uses `tokio::sync::Mutex`** (the probe is async; the repo rule prefers no `lock().unwrap()`; `parking_lot` is not a `tasks` dependency and a test stub does not justify one). The older stubs in the module keep their `std::sync::Mutex`.
- **`bindings/` generated for the workspace gate.** `cargo check --workspace --tests` fails cold on `nodes` (`include_str!("../../../bindings/methods.json")`, gitignored, produced by `just ts`); `cargo test -p rpc --features ts --lib --quiet export_bindings` (what `just ts` runs) was executed only so that target compiles. No ts-exported type changed shape (`ReleaseWake`, `ActivityOf`, `Liveness` are internal).
- No desktop/TUI run; nothing executed against `~/.theuth`.

## What happens to Sky's live data after deploy + restart

Nothing by hand. On start the desks reopen (`ReleaseClaims`) and the sweep releases whatever is still claimed by a session that is not the agent's desk; `dispatch()` re-delivers theuth's `01a10c9c-6181` and nikolai's `01a10ca1-ff9b`. The desks answer (or get the `reply` nudge — the cold pre-restart children no longer count as live), `FinishWake` deletes the rows, and the 4 theuth + 1 nikolai wakes behind them flow one per turn; each held pass logs `wakes held: …` once. The SRE desks waiting on omp workers are no longer orphaned while a worker runs (`active_now`) or within 15 min of its last activity.

If the two rows should go before deploying (engine stopped; `wakes` has replication triggers): `DELETE FROM wakes WHERE id IN ('01a10c9c-6181…', '01a10ca1-ff9b…')` — **not** `UPDATE … SET claimed_by = NULL`, which re-enters the same deadlock on the unfixed binary.

## Verification (run from the worktree root)

| gate | result |
|---|---|
| `cargo test -p roster --lib --no-fail-fast` | `50 passed; 0 failed` (2 new: T1, T7) |
| `cargo test -p tasks --lib --no-fail-fast` | `57 passed; 0 failed` (3 new: T3, T4, T5; T6 extended) |
| `cargo test -p agent --lib --no-fail-fast` | `380 passed; 0 failed` (1 new: T8) |
| `cargo test -p schedule --lib --no-fail-fast` | not run — `crates/schedule` unchanged (D7 verify-only) |
| `cargo check --workspace --tests` | exit 0, no errors, no new warnings (after `bindings/` generated, see Deviations; cold run ≈ 29 min on this host) |

Formatting: `rustfmt --edition 2024` on the touched files; pre-existing drift in `pin.rs`, `place.rs` (pulled in through `mod`) reverted, the four pre-existing hunks in `supervisor.rs` left alone (my hunk formatted by hand).

Commits: `07a4bf69` fix(tasks,agent): release non-yielded wakes, report busy skips, sweep asks liveness · `8b074a8d` fix(roster): a cold child is not a live child; a non-yielded turn releases its wake · docs(design) this file.

DONE WAKE-LIVE-CHILD

## Amendment (178 DESK-WEDGE, 2026-11-07)

- **Back-off doubles across deliveries.** `delivered()` no longer clears the wake's retry counter; `finish_wake` does (and `dispatch` drops counters of wakes that left the table). error → 5 s, again → 10 s, 20 s … 5 min; cancel → 5 min floor as before. Supersedes the "successful delivery still clears the counter" deviation above.
- **Judge = pure `nudges::verdict()`** (`Release{min_backoff_ms} | Handled | Defer | Nudge | Block`); `judge_turn` fetches the task/events/children and acts. The deferral table is unchanged.
- **Tick backstop.** A desk that serves a wake while the D4 probe says its session tree is not active now, and nothing in it (desk or descendant) was active for 2 × TICK, is judged as for a yielded turn end (`judge_turn(name, "end_turn")`): the existing protocol (finish, one nudge, `NoContinuation`). Liveness, never age: a running child keeps deferring (D1); an unknown liveness judges nothing. Covers a lost `TurnFinished` and a refused/lost child result. The Tick's probe is `Roster.probe` (`tasks::AgentProbe`; a fake in tests).
- **A `Resume` wake handed to a taken session is not served on the desk.** No desk turn ends to judge it, so `DeliverWake` finishes the wake once the taken session has its input instead of setting `Desk.serving` (which held the agent's queue for ever).
- **Status.** `TurnResumed` marks the desk working like `TurnStarted`; `working()` is `working || serving.is_some()` (the `AgentStatus::Working` doc). `AgentSummary.waiting_on` names the children holding the current wake (`nudges::waiting_on`, the judge's `is_live`); the views show `waiting on <name> (+N)`.
- **Reopen release waits for tasks.** At boot the roster opens desks before the tasks plugin starts; `open_desk`'s `ReleaseClaims` needs the tasks actor, so a reopened desk without one is queued (`Roster.unreleased`) and released at `TasksUp`, before dispatch is enabled. Before this, the dead turn's claim survived every restart.
