# 179 PAUSE-SUSPEND — pausing an agent suspends its work, un-pausing resumes it, a paused agent gets no wakes

Status: built on `fix/pause-suspend` (base `565ac0d2`). Working notes, the Phase A reproduction and the live proof:
`179-notes.md`.

## Report and ruling

Sky, 2026-11-07: "I paused an agent in the agents view and it's still getting wakes from the task system … I saw a
couple wake messages come through in the chat." Ruling: **pause = suspend and resume** — freeze its turn and
subagents where they are, like a clean quit; un-pausing picks up from there; no lost or repeated work.

Reproduced on `565ac0d2` (notes, Phase A): after a pause the desk's chat still got (1) the pause's own `<journal>`
closing turn, (2) a running child's result (`<message from="session:…">`, a new desk turn), (3) input queued behind
the running turn; a restart resumed the paused desk's turn; a stale `agent_put` un-paused it; every un-pause
re-delivered the wake the desk was still continuing (`EpisodeClosing` had released its claim).

## Decisions

| # | Decision | As built |
|---|---|---|
| D1 | Pause = suspend the desk tree as a graceful quit does. | `agent::supervisor::pause::PauseTree{desk}` (off the mailbox): holds the tree first, then `transfer::drain_session` per live session of desk + descendants, one `durability.drain_secs` deadline, reason `paused` — `suspended{paused, ran_ms}`, `Draining`, the in-flight call finishes inside the window, session jobs killed, actor stopped; `TreeStopped` forgets actor, `running`, the child's timer and run clock. Roster: one path for the switch, the budget cap (`budget_pause`) and a file edit — `sync_desks` → `sync_pauses` → `pause_tree`, run inline by `PauseAgent` (the queued `Reconcile` raced turn ends and Ticks). `theuth` still refused. |
| D2 | Un-pause = resume, then reopen the desk. | `ResumeTree{desk}`: lifts the hold, `resume_turn` for every open turn of the tree without a live actor (desk first, one `resume_parallel` semaphore; 178's run clock and remaining cap come from the plan), replays held inputs. Roster `resume_tree`: then `open_desk_with(release = !resumed)`; with resumed work the claim and the served wake stand (`PausedDesk.serving`, or after a restart `tasks::ClaimedWake` → `Serving`), so the wake is not delivered twice; a resumed desk turn holds the agent's other wakes (`Shared::set_resuming`) until its `TurnFinished` (Tick backstop), then `tasks::Unpaused`. A desk turn that finished inside the drain window is judged at the un-pause. |
| D3 | No wake attempts while paused. | `DeskRouter::is_paused` (default `false`; roster: the file's `paused`, or `resuming`). Ledger: `dispatch` filters paused agents' wakes (no claim, no back-off, no "held" line); `wake()` writes `queued for <a> (paused) (<reason>)`; `unpaused()` drops the agent's back-off; `tasks::Unpaused` dispatches at once. `DeliverWake`'s gate stays; a `held:` refusal logs at debug. |
| D4 | Checkouts survive a pause. | `close_desk(.., "paused")`: no `EpisodeClosing`, no journal turn — `put_away` at once (episode reason `paused`). Sweep: a paused owner's `doing` checkout is skipped (no "presumed lost", no `resume`), its claims are kept. |
| D5 | Routines skip paused agents. | `TaskCreator::agent_paused` (default `false`; `ScheduleTasks` asks `tasks::IsPaused`). `fire_with` records `skipped: agent paused` via the shared `skip` (as `busy`), publishes `Skipped`, creates no task. |
| D6 | No turn starts in a paused tree. | `Agents.paused: desk → [(session, Held)]`. `route` and `enqueue_peer` hold any input for a session of a held tree (no actor is created; the sender's receipt stands); `quiet` defers to `route`; `ResumeTree` replays in arrival order. |
| D7 | `PutAgent` cannot change `paused`. | An existing file keeps its stored `paused` and `proposed_by`; a new file honours them ("Hire paused"). |
| D8 | Boot. | Engine start (`pause::boot_accounting`): every agent file that says `paused` (`pause::agent_file_paused`, the roster's own front-matter parse via `pause::front_matter`; `theuth` skipped) holds its desk tree from construction — before reminders re-arm (`rearm_reminders_for` skips held trees) and spared results go (`deliver_spared_results` skips them) — and the restart accounting spares the exchanges addressed to it (`fail_interrupted_except`). `unpaused_turns` drops held trees from the native scan and the ACP respawn; `resume_turn` refuses a held tree (Resume button, RPC); `OpenTurns` hides it; the quit drain leaves a held tree's journal alone. Roster load: every paused agent's desk session is mapped (`set_desk`) and held (`PauseTree`); `sync_pauses` reconciles the agent actor's `HeldDesks` (an un-paused or invalid file lifts, a removed agent's is `ReleaseTree`d: its open turns are closed `cancelled`, never resumed unmapped). |
| D9 | UI. | `AgentSummary.{held_wakes, suspended}` (serde default, ts optional) from `tasks::HeldWakes` / `TreeSuspended` for paused agents; `AgentSummary::paused_label`. Desktop `PausedChip` (`Paused · N wakes held · suspended mid-turn`) in the Agents list, org chart and detail header; TUI `agents_table`; TaskDetail `Comment — queues a wake for A (paused)` / `Unblock — queues a wake for A (paused)`; toasts `Paused <A> — its work is suspended` / `<A> resumed`. |
| D10 | Logs. | info: roster `179: paused: desk closed, its session tree suspending`, agent `179: paused: session tree suspended desk=… turns=[…]`, roster `179: un-paused: tree resumed, desk reopening`, agent `179: un-paused: session tree resumed`. No warn while paused. |

## Behaviour notes

- The hold, not the journal, decides: a turn that reaches a round inside the drain window also writes the loop's own
  `suspended{drain}` after the pause's `suspended{paused}`; the resume cause reads `shutdown` either way.
- A turn already open but not running when the pause lands (a crash left it) gets no marker; it is held and resumed
  at the un-pause like the rest.
- An un-pause while the pause still drains waits for the stop (`Agents.pausing`), then resumes; `TreeStopped` forgets
  only the actors it stopped. The wait ends at the drain deadline plus 5 s (`UNPAUSE_GRACE`): a warn, what is stopped
  resumes, and a late `TreeStopped` resumes the rest. An ACP child's cancel from the stop (before or after it,
  `acp_cut`) is no turn end; the marker goes with the child's next turn start, or at the un-pause when nothing
  re-prompts it, so a later real cancel ends the turn. A live idle native session's suspended turn resumes in place.
- Durable while paused: child results and local peers' messages stay `accepted` rows (spared at boot, replayed by
  `ResumeTree` from `pending_exchanges`, deduplicated against in-process holds); reminder rows stay (timers stopped
  at the pause, re-armed at the un-pause). Harness notices held in memory (`inject`) do not survive a restart.
  A person's text is refused: `<agent> is paused — un-pause it to send`.
- A child brief held by the pause keeps its permit; its cap timer arms when the turn really starts.
- While an un-paused desk finishes its resumed turn the agent still reads as paused to the router: the thread says
  `queued …` and a routine firing in that window is skipped.
- A wake rebuilt from the ledger after a restart has no delivery-time event count (0): the judge counts all of the
  agent's events on the task. A desk turn that ended inside the drain window keeps its served wake and is judged at
  the un-pause even when nothing else resumes.

## Tests

`tasks`: `ledger::tests::{dispatch_leaves_a_paused_agents_wakes_unclaimed, unpause_dispatches_held_wakes_at_once,
sweep_skips_a_paused_owners_checkout, thread_line_says_queued_for_a_paused_owner}`. `schedule`:
`fire_tests::fire_for_a_paused_agent_is_skipped_and_creates_no_task`. `roster`: `tests::{deliver_wake_to_a_paused_agent_is_held_with_an_open_desk,
pause_closes_the_desk_and_keeps_checkouts, unpause_reopens_desk_and_releases_claims, put_agent_keeps_stored_paused,
put_agent_new_file_honours_paused, budget_cap_pause_suspends, pause_parse_matches_the_roster,
turn_that_ends_inside_the_drain_window_is_judged_not_redelivered}`. `agent`: `supervisor::tests::{pause_suspends_the_desk_tree,
resume_scan_skips_a_paused_agents_sessions, unpause_resumes_the_tree_with_remaining_caps,
input_to_a_suspended_session_is_held_until_resume, human_input_to_a_paused_agent_is_refused,
peer_message_to_a_paused_tree_survives_a_restart, unpause_rearms_the_trees_reminders,
boot_holds_a_paused_desk_without_an_open_turn, held_child_cap_excludes_paused_time, paused_acp_child_resumes_on_unpause,
unpause_during_the_pause_drain_resumes_after_the_stop, acp_cancel_after_unpause_is_not_swallowed}`,
`supervisor::pause::tests::agent_file_paused_reads_the_front_matter_only`.
UI: `identity.test.ts` (the paused chip).
