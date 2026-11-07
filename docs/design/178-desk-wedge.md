# 178 DESK-WEDGE — subagent caps survive restarts, resumed turns don't starve, desks say what they wait on

Status: built on `fix/desk-wedge` (base `042255a1`). Working notes and the live proof: `178-notes.md`.

## Incident

On Sky's second Mac the desk `hans` did nothing for ~2 h: messages and tasks queued, the Agents view said
`idle`, the log repeated `wakes held: desk still serving an earlier wake agent=hans`; a restart did not help.
Cause, traced on `042255a1`: a child of the desk's current wake kept running (its `max_runtime_secs` was an
in-memory timer armed only for a spawn turn, so a resumed or follow-up native child turn ran uncapped); the
desk kept `serving` that wake while the child was live (WAKE-LIVE-CHILD D1, correct), the ledger skips every
agent with a claimed wake (correct), and nothing bounded "while the child runs". Resumed turns 5..N waited on
`resume_parallel` slots held until the first four turns *ended*. The roster's `idle` ignored `serving` and
`TurnResumed`.

## Decisions

| # | Decision | As built |
|---|---|---|
| W1 | Every child turn (a session with a parent; native and ACP; spawn, follow-up, human-opened, resumed) gets the supervisor's runtime timer; one arming point; a turn that has a timer keeps it. Budget = `max_runtime_secs − ran`, `ran` = pre-restart run time from the journal (first step → `interrupted_at`; ACP: the open journal turn's first → newest step, i.e. the drain's `suspended` row). Downtime never counts. Floor 60 s, never above the cap. `ChildTurn.started_at = now − ran`. Expiry: the existing `ChildTimedOut → tell_cancel → TurnFinished(cancelled) → hand_off` path. | `supervisor.rs` `turn_began` / `ran_before_restart` / `arm_child_turn` / free `runtime_timer(child, me, after)`; `durability::remaining_run` + `RESUME_GRACE`; `Store::turn_span`. `drive()`'s `PeerWork` wall clock runs at depth 0 only, so a child never has two clocks. |
| W2 | A depth-0 turn that ran bounded (50 §8: a peer input, or a scoped input at depth 0) keeps its bound after a resume, with the remaining run time. | The journal's opener `source` says so (`peer`, new `scoped`); `ResumePlan.{started_at, bounded}`; `Session::bind_turn` (shared with `start_turn`) sets `peer_work` **and** the `PermitSlot` (the trap: `reacquire_permit` refuses a turn not in the slot). |
| W3 | The `resume_parallel` slot bounds the replay burst, not the turn: released after `resume_prelude` + the first `round`, or after 60 s. A stop while waiting lands `cancelled`. | `Turn::resume_slot` (a oneshot sender; dropping it frees the permit held by a 60 s timeout task); `RESUME_SLOT_HOLD` (2 s under `cfg(test)`). |
| W4 | The Agents view tells the truth. | Roster: `TurnResumed` sets `Desk.working`; `working()` = `working \|\| serving.is_some()`; `AgentSummary.waiting_on: Vec<String>` (serde default, ts optional) = `nudges::waiting_on(serving, children_of(desk))` with the judge's `is_live`. UI: `ActivityChip waiting=` shows `waiting on <name>` / `waiting on <name> +N`; TUI `agents_table` the same text (`AgentSummary::waiting_label`). No `AgentState` variant. |
| W5 | Roster Tick backstop, liveness-based: a serving desk whose session tree is not active now and was not active for 2 × TICK is judged as for a yielded turn end; a live child keeps deferring; unknown liveness skips. | `desk::nudges::verdict()` (pure; `Release/Handled/Defer/Nudge/Block`), `judge_turn` acts on it; Tick uses `Roster.probe: Arc<dyn LivenessProbe>` (default `tasks::AgentProbe`), `SERVING_GRACE_MS`. A `Resume` wake handed to a taken session finishes the wake instead of setting `serving` on the desk. |
| + | Found live: at boot the roster reopens desks before the tasks plugin starts, so the reopen's `ReleaseClaims` was skipped and the dead turn's claim held the agent's queue for good ("a restart did not help"). | `open_desk` queues the session in `Roster.unreleased` when there is no tasks actor; `TasksUp` releases those claims before enabling dispatch. |
| W6 | A failed child says so. | `hand_off` prefixes `[child turn ended: error — <first line of the turn's last Error, ≤200 chars>]`, `[child turn cancelled]` (the cap keeps `[timed out after Ns, stop=…]`), `[child turn ended with no reply]`. `Agents.child_errors` records a child turn's `AgentEvent::Error`. No retries. |
| W7 | Desk back-off doubles. | `Ledger::delivered` keeps `retry[wake]`; `finish_wake` clears it; `dispatch` drops counters of wakes no longer in the table. |
| W8 | `child_result` rows never dangle. | `child_waits::child_gone(children, why, quiet)`: clear the row, tell the parent once (`<message from="session:<child>">… no result will come from it.`). Called by `drop_queued` (dropped/archived/stopped queued child) and by `DeleteSession` before the rows go (parents inside the deleted tree are quiet). |

Unchanged on purpose: `judge_turn`'s deferral table; the hold rule while a child of the current wake runs ("free the desk" is Sky's open question; now bounded by W1); `child_result.deadline` stays bookkeeping; no age/lease release of ledger claims; no wall clock on every `drive()` turn; no new `AgentState`; no automatic retries; the genai fork.

## Behaviour notes

- The cap is per turn and one knob (`agent.subagents.max_runtime_secs`): each new child turn starts a fresh cap; a resumed turn continues its own. A turn a human opened on a child session is capped too.
- Run time, not wall clock (fix round F1): every running turn has a run clock (`durability::run_clock`, `std::time::Instant` = `CLOCK_UPTIME_RAW` on Darwin / `CLOCK_MONOTONIC` on Linux, neither advances in sleep; Windows' QPC may). Native turns start it in `drive` from the journal's run time, ACP prompts at `TurnStarted` in the supervisor. Every `suspended` writer (boot drain, the loop's drain round, the release drain) puts the cumulative `ran_ms` on the row; `store::durability::run_ms` reads the newest `ran_ms` plus the span of the rows after it (a run no drain measured, i.e. a crash), falling back to the whole span for rows from before 178. A turn resumed twice counts each run once; a crashed ACP child with only `input_accepted` gets a full cap.
- Rows the turn writes inside the drain window after the drain's `suspended` row count as a short tail span (≤ `drain_secs`).
- A bounded depth-0 turn's remaining bound starts when its resume slot is granted (F3), as its `TurnResumed`.
- `ChildTimedOut` carries the arm (`ChildTurn.arm`, F2): a fire that lost the race with its turn's end never cancels the next turn.
- The roster backstop judges a serving desk only once a turn start, resume or finish of it was seen since the delivery (`Serving.turn_seen`, F3). `AgentSummary.waiting_on` is a per-desk cache (F4), refreshed on Tick, at the desk's turn end and on turn events of its wake's children; the dreaming gate counts desks with a running turn (`roster::DesksRunning`), not desks only serving a wake. `unload_session` drops the child's `ChildTurn` and run clock (F5).

## Tests

`agent`: `supervisor::tests::{native_spawn_turn_is_capped_and_timeout_is_prefixed, native_follow_up_turn_gets_runtime_timer, resumed_native_child_gets_a_timer_with_remaining_time, timer_does_not_count_downtime, resumed_acp_child_cap_counts_pre_restart_time, hand_off_prefixes_error_cancelled_and_empty_results, drop_queued_clears_the_child_result_wait, deleted_child_clears_the_parent_wait}`, `loop_::durability_tests::resume_slots_and_bounds_contract` (scenarios `resume_slot_is_released_after_the_first_round`, `queued_resume_turn_cancel_lands_cancelled`, `bounded_turn_keeps_its_bound_after_resume`). `roster`: `tests::{turn_resumed_marks_desk_working_until_finished, summary_is_working_while_a_wake_is_served, summary_lists_children_the_desk_waits_on, tick_rejudges_a_serving_desk_when_nothing_is_alive, tick_keeps_deferring_while_a_child_runs, tick_waits_out_the_delivery_grace, resume_wake_to_a_taken_session_does_not_set_serving}`, `desk::tests::{non_yield_verdicts_release_with_floor, live_child_defers_only_rows_that_ignore_children}`. `tasks`: `ledger::tests::{release_wake_backoff_doubles_across_deliveries, current_desks_claim_is_never_aged_by_sweep}` (+ `release_wake_unclaims_with_a_floor` updated: second release = 10 s). UI: `components/agents/identity.test.ts`. TUI: `agents_table` renders `waiting on …`.

Fix round: `supervisor::tests::{twice_resumed_turn_counts_run_time_once, twice_resumed_acp_child_counts_run_time_once (replaces resumed_acp_child_cap_counts_pre_restart_time), stale_runtime_fire_does_not_cancel_the_next_turn, next_turn_after_unloaded_child_is_capped}`, `roster::tests::tick_leaves_a_desk_whose_resumed_turn_waits_for_a_slot`.
