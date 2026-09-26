# 70 STALE-WAKE-DESKS — a done task re-reviewed after a restart, desks duplicated (as built)

Branch `fix/stale-wakes-desk-reuse` (worktree `theuth-wake`, off `71a1a96`). Brief: `/tmp/69/omp-STALE-WAKE-DESKS.md`. Read with `70-as-built-DESK-PROTOCOL.md`, `70-as-built-UI-FIXES-2.md` (re-queue on conflict), `70-as-built-UI-FIXES-3.md` (`sessions.desk`), `67-agents.md` §4.4/§4.6.

## Incident, traced to code (all UTC, 2026-09-26; rows from a read-only copy of `~/.theuth`)

| when | what | code |
|---|---|---|
| 10:46:30 | stas `doing → review`; wake `01a0dd52` (theuth, review) raised | `ledger::update` → `wake(reviewer, Review)` |
| 10:47:45–10:48:03 | theuth's desk `01a0dd2e` reviews, moves `review → done`; **wake row stays, `claimed_by = 01a0dd2e`** | `update()` completed a wake only when `actor == before.owner` (`ledger.rs:663`); the reviewer is not the owner, so `complete_wake`/`finish_wakes` never ran and no transition ever deleted wakes |
| 11:42:22 | shutdown: `iroh_gossip … ActorDropped`, then `roster: hub lease elsewhere: desks close, wakes held` | `mesh::Ctx::run_lease` emits `Lease { holder: None, reason: "release" }` on its way out (`mesh/src/lib.rs:435-443`); `Roster`'s `MeshEvent` handler treated it as a loss → `close_desk("lease")` on both desks |
| 11:43:00/05 | new sessions `01a0dd86-95c4` (stas) and `01a0dd86-a95b` (theuth); two `desk='stas'`, two `desk='theuth'` rows | `Roster::open_desk` always `CreateSession`; nothing looked for the row already marked `desk = <agent>` |
| 11:43:05 | `01a0dd52` re-claimed by `01a0dd86-a95b`; theuth re-reviews the done task, wakes stas twice, files `01a0dd8a` | `ledger::sweep`: `desk_for(agent) != claimed_by` → un-claim → `dispatch()` → `DeliverWake` with no check of the task's state against the wake's reason |

Independent of 3: even without the spurious lease event, the restart alone would have created fresh desks (2) and re-armed the stale claim (1).

Two latent bugs found on the same path and closed by the same fix: (a) a wake finished by a turn that never wrote the task (a `comment` wake answered with a comment) stayed claimed → the agent was "busy" (`dispatch()`'s one-in-flight rule) until its episode closed; (b) any reviewer wake likewise, so a reviewer was blocked from further wakes after every review.

## Fix 1 — wakes die with their purpose (`crates/tasks`)

- `types.rs` — `WakeReason::applies_to(state)`, the one predicate: `done | cancelled` → nothing applies; `review` → `review | blocked` (the `no continuation` manager wake lands on a blocked task); `assigned | reassigned | resume | resumed | blockers_resolved | children_completed` → `todo | doing`; `comment | mention | answer | manual | schedule` → any open state. `WakeReason::ALL` for the store's filter.
- `store.rs` — `drop_stale_wakes(task, state)` (`DELETE … WHERE task = ? AND reason IN (<not applicable>)`), `finish_wake(id)`, `release_claimed_by(session, now)` (`claimed_by = NULL`). `finish_claimed_by` (delete-by-session) removed.
- `ledger.rs`
  - `save()` drops the non-applicable wakes of the saved row: every state transition — `update`, `reassign`, `unblock`, `no_continuation`, the sweep — routes through it, so `review → done` deletes the `review` wake whoever moved it; a terminal state deletes all.
  - `wake()` refuses to raise a wake that does not apply (a comment on a done task wakes nobody).
  - `dispatch()` validates each candidate against its task before claiming: not applicable, or the task is gone → the row is deleted, `tracing::info!("wake dropped: stale …")` and a thread line `wake `<reason>` for <agent> dropped: stale (task is <state>)`. This is also the startup sweep over rows left by earlier builds (the hub dispatches every second).
  - `delivered` is keyed `(agent, task)` (was `task`: a reviewer's and the owner's wakes on one task collided); completion fires on the **acting agent's** write (`actor = agent:<name>`), not only the owner's; only the owner's unmoved write counts as no progress.
  - `finish_wake(&Wake)` / `release_claims(session)` behind two new actor messages `FinishWake(Wake)` (tell) and `ReleaseClaims(String)` (tell).
  - `episode_closing` **releases** the closing sessions' claims instead of deleting them (67 §4.6: a wake the closed desk did not finish is delivered again, validated then) and forgets the agent's `delivered` entries.
  - `sweep()` releases a claim unless the claiming session *is* the agent's desk here (was: only when a different desk is mapped) — a fresh hub before theuth's desk opens would otherwise never deliver theuth's first wake.
  - `forget_delivery` (no callers) deleted.
- `lib.rs` — `Tasks::start` releases `dispatching` placeholder claims (they only live between one process's claim and its delivery).

## Fix 2 — one desk per agent, reopened not re-created (`crates/agent`, `crates/roster`)

- `crates/agent/migrations/0025_one_desk_per_agent.sql` — dedupe, then `CREATE UNIQUE INDEX sessions_desk_owner ON sessions (desk, owner) WHERE desk IS NOT NULL`. Dedupe rule (the orchestrator's): per `(desk, owner)` the most recently `updated_at` row stays the desk; every other row gets `desk = NULL, pinned = 0, archived = 1, title = '<agent> · desk (superseded <today>)'`. `owner = ''` counts as this engine's (the backfill in `Store::open` runs right after).
- `crates/agent/src/store.rs` — `Store::desk_session(agent) -> Option<SessionId>`: `desk = ? AND (owner = '' OR owner = me) ORDER BY updated_at DESC LIMIT 1`, archived or not. `crates/agent/src/lib.rs` `DeskSession(String)` message; `supervisor.rs` handler.
- `crates/roster/src/lib.rs`
  - `open_desk`: `DeskSession(name)` first; found → reuse (un-archive — a desk put away by an earlier build —, pin, cap, episode row, `ReleaseClaims(session)`); none → `CreateSession` + rename + `MarkDesk` as before. `desk_or_open` (`desk_open` RPC) and `DeliverWake`'s lazy theuth open go through it, so the RPC succeeds and returns the same row the roster maps even before the index existed (newest by `updated_at`).
  - `close_desk` no longer calls `Shared::clear_desk`: the desk is removed from `Roster.desks` (closed in memory, stops taking wakes) but the agent → session mapping (`Shared.desks`, `AgentSummary.desk`, `DeskRouter::desk_for`) survives idle closes, lease moves and restarts.
  - `put_away`: only a `context` close retires the session (`MarkDesk None`, rename `<name> · desk (superseded <date>)`, unpin, archive, `clear_desk`, `forget`); every other reason leaves the row as the desk for the next episode. The next `open_desk` then reuses it.
- Which row the UI shows: `AgentSummary.desk` = `Shared.desk(name)`, set by `open_desk` from `DeskSession` = the one row with `desk = name` on this engine — the same row the wake claims and the sidebar use.

## Fix 3 — our shutdown is not a lease loss (`crates/roster/src/lib.rs`)

- `Roster::lease_event_applies()`: `false` (one info line) once `rt.shutdown` is cancelled; the `MeshEvent` handler skips `Lease` events then. The mesh's release runs after `rt.shutdown` fired (`run_lease(stop.child_token())` loops until cancelled, then emits), so ordering is by the token, not by actor teardown order.
- `SetDispatching(false)` ("wakes held") never touched `claimed_by` — confirmed; `close_desk("lease")` → `EpisodeClosing` used to **delete** the desk's claimed wakes and now **releases** them; a consumed wake (turn handled) is already gone via `FinishWake`, so nothing consumed can be re-armed.

## Roster → ledger: when is a wake finished?

`DeliverWake` records `Desk.serving` for every wake (task or not). `Roster::clear_serving(name)` — handled turn, `no continuation`, task-less wake — now sends `tasks::FinishWake(wake)`: the row is deleted whether or not the agent wrote the task. Plus the acting agent's write (`complete_wake`) and the transition purge. A turn killed by shutdown or lease loss leaves the row claimed; the reopen (`ReleaseClaims`) or the sweep releases it and dispatch re-validates it.

## Repair the migration performs on the user's data (dry-run on `/tmp/69/dbcopy`, same SQL)

`sessions.sqlite`, migration 0025:

| row | agent | updated_at | result |
|---|---|---|---|
| `01a0dd86-95c4-7002-8372-37336b370455` | stas | 11:46:50 | **stays the desk** (`stas · desk`, pinned, not archived) |
| `01a0dd41-b48c-72c1-aeaa-de761cfe6736` | stas | 11:42:22 | superseded: `desk = NULL`, archived, unpinned, title `stas · desk (superseded 2026-09-26)` |
| `01a0dd86-a95b-7162-8ed3-431bbc4d2040` | theuth | 11:47:05 | **stays the desk** (title stays `DR pre-drill verification checklist` — the engine's first-line titling had overwritten `theuth · desk`; not touched here) |
| `01a0dd2e-8503-7ee0-875d-50b771d057c5` | theuth | 11:42:22 | superseded: `desk = NULL`, archived, unpinned, title `theuth · desk (superseded 2026-09-26)` |

`tasks.sqlite` — no SQL migration; the startup path does it on the first dispatch/sweep (≤ 30 s after the hub is up):

| wake | agent · reason | task state | claimed_by | result |
|---|---|---|---|---|
| `01a0dd52-dcee-7ef3-a096-2a6f627ef4bd` | theuth · review | done | `01a0dd86-a95b` | sweep releases (theuth's desk not yet mapped) → dispatch **drops it**: thread line `wake `review` for theuth dropped: stale (task is done)` |
| `01a0dd8a-2a40-7452-8a45-40a3b68aa73c` | stas · comment (×2) | done | — | dispatch **drops it** (same line, `comment`, `stas`) |
| `01a0dd8a-061e-76a1-a47b-b1f5d60f21a7` | stas · assigned | todo | `01a0dd86-95c4` | `open_desk(stas)` reuses `01a0dd86-95c4` → `ReleaseClaims` → **redelivered** to stas's desk (the task is still `todo`; the turn that took it died with the old process) |

Task `01a0dd46` stays `done`; task `01a0dd8a` (filed by the second review) is left as is — it is a real todo for stas, whether or not the review that filed it should have run.

## Tests

- `tasks::ledger::tests::terminal_transition_drops_wakes_and_frees_the_reviewer` — the incident: reviewer's `review → done` leaves no wake, a comment on the done task wakes nobody, the reviewer's next wake dispatches.
- `tasks::ledger::tests::stale_wake_is_dropped_at_dispatch_not_delivered` — pre-fix rows (review/comment on a cancelled task, a wake on a deleted task) are dropped with the thread line; the live wake goes out.
- `tasks::ledger::tests::finish_wake_and_release_claims` — release → redispatch; `finish_wake` frees the agent.
- `tasks::ledger::tests::episode_closing_blocks_leftover_tasks` — updated: the desk's unfinished wake is released, not deleted.
- `agent::store::tests::one_desk_per_agent_is_found_and_enforced` — `desk_session` finds the marked row (archived too), refuses a second mark (schema), ignores another engine's desk, follows a retire.
- `agent::store::tests::migration_dedupes_desks_by_last_update` — replays `0025` over a fixture with two theuth and two stas rows: newest kept, older superseded/archived/unpinned/renamed, a lone desk untouched.
- `roster::tests::lease_events_are_ignored_during_shutdown` — `Lease { holder: None }` is a loss before shutdown (`OpenDesk` refuses with the lease gate) and a no-op after `rt.shutdown.cancel()`.

## Deviations (nobody to ask)

- **Consumed = turn over, not turn start.** The brief asked that a wake be deleted "when the desk turn that took it starts". Built: the row stays claimed for the turn and is deleted when the turn is judged handled (`FinishWake`), the acting agent writes the task (`complete_wake`), or the state changes past it (`save()`); a claim orphaned by a restart/lease move is released and the wake re-validated at dispatch. Reason: deleting at turn start loses the wake when the process dies mid-turn — a `review` wake would leave the task parked in `review` with nobody woken; 67 §4.6 says such wakes are "un-claimed by the orphan sweep and redelivered". The incident's failure (redelivery of a wake for a *done* task) is closed by the transition purge and the dispatch validation regardless. If turn-start deletion is still wanted: call `store.finish_wake` in `Ledger::delivered` and keep an in-memory busy set.
- **UNIQUE on `(desk, owner)`, not `(desk)`.** Session rows replicate between engines (68) and each hub opens its own desk when the lease moves; a global unique index would reject the peer's row. Same effect on a single engine. Ceiling: a `TakeSession` of another engine's desk into an engine that has its own desk for that agent will fail the index (`ponytail:` a take never targets a desk today — RPC forwards desks instead of taking them; the orphan sweep takes work sessions).
- **Tasks side is a startup sweep, not a SQL migration.** The reason ↔ state rule lives once in `WakeReason::applies_to`; `dispatch()` applies it to every row it considers, so the first dispatch pass on the hub is the sweep. A SQL migration would duplicate the rule.
- **"activity line"** for a dropped wake = a task-thread line plus `tracing::info!`, the pattern the held-wake diagnostic uses; no new `ActivityKind` (would need `just ts`).
- **`Shared.desks` keeps the mapping across closes**: `AgentSummary.desk` and `DeskRouter::desk_for` now name the agent's desk session while its episode is closed too. `Roster.desks` (open episodes) is unchanged. `RoutableAgents.desk` follows.
- **Retire only on `context`.** Idle, lease, paused and RPC closes keep the session; only the context-pressure close (which exists to reset the transcript) supersedes it. Archived past desks therefore no longer carry the `desk` mark (UI-FIXES-3 had kept it for the Runs tab); superseded desks show in the archived chat list under their `(superseded <date>)` title.
- Not reproduced: why the user's Stas desk row "would not open". `[INFERENCE]` the sidebar row linked the roster's in-memory desk while the ledger/UI held the other row; after this change all three read the same `sessions.desk` row. The desktop was not run (no `cargo build` of the shell in this worktree).
- No ts-exported type changed shape (`WakeReason` gained a method and a const only; `DeskSession`, `FinishWake`, `ReleaseClaims` are internal actor messages); `just ts` was run only so the `nodes` test target compiles (`bindings/` is gitignored).

## Verification (run)

- `cargo test -p tasks --lib` → 51 passed (48 before + 3 new; `episode_closing_blocks_leftover_tasks` updated).
- `cargo test -p roster --lib` → 42 passed (41 + 1).
- `cargo test -p agent --lib store::tests::` → 44 passed (2 new).
- `just ts` → `export_bindings` 5 passed, 379 binding files (needed for `nodes`' `include_str!` of `bindings/methods.json`; no exported type changed shape).
- `cargo check --workspace --tests` → exit 0, no errors, no new warnings (cold build, 8m43s).
- Migration `0025` applied with `sqlite3` to a copy of the user's `sessions.sqlite` → the four rows above, index created. The same SQL replayed in `migration_dedupes_desks_by_last_update`.
- Not run: the desktop shell / a live engine against a data dir (`cargo build` of the binary is outside this worktree's rules; nothing was run against `~/.theuth`).

Commits: `5cc9242` fix(tasks): wakes die with their purpose · `f731872` fix(roster,agent): one desk per agent, reused across restarts · docs.

DONE STALE-WAKE-DESKS
