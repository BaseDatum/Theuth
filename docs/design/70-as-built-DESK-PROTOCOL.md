# 70 DESK-PROTOCOL — desks act like employees (as built)

Branch `fix/desk-protocol-employee`. Brief: `/tmp/69/omp-DESK-PROTOCOL.md` plus two interjections (item 5 identity first; paperclip alignment (a)–(f) and Sky's `done_when` ruling). Read alongside `67-agents.md` §6–§9, `69-as-built-TASKS.md`.

Sky's three observations and where each landed:

| Observation | Built |
|---|---|
| theuth's desk spawned a generic child instead of handing a k8s task to the k8s specialist | fit check: the `assigned`/`reassigned` wake carries a `<roster>` block (active agents, title, capabilities); manual step 1; `task_reassign` tool → `Ledger::reassign` (owner change, checkout released, `reassigned to ‹agent›: ‹why›` in the thread, new owner woken `reassigned`); "still `todo` at turn end" nudge |
| the desk relayed the child's result and stopped | follow-through: manual step 2 (read → judge against `done_when` → spawn more / file child tasks, ≤ `roster.max_rounds` = 3 → `task_note` report block → disposition); the `disposition` nudge fires when a turn ends with the task `doing`, no live child and no continuation; a second unmoved end → `blocked: no continuation` + manager woken |
| a review wake produced an opinion but no transition | `review` nudge row: the turn must end with the task out of `review`; same one-nudge-then-block rule |
| `karol` said "I'm theuth" | identity-first system prompt for every desk and its work sessions (`IdentitySource`) |

## Commit 1 — `feat(desk): identity-first system prompt for roster desks and their work sessions` (`dc36eeb`)

- `crates/agent/src/frame.rs` — `baseline(data_dir, identity: Option<&str>, peer_line, plugin_blocks, claude_code, subagent_model, attended)`. With `identity` the block replaces theuth's header (`THEUTH_HEADER`) **and** `prompt.md`; the cwd/date line, skills, peer line, plugin blocks and guidance stay. Test `identity_replaces_theuth_persona`.
- `crates/agent/src/supervisor.rs` — `IdentitySource` (async trait, `identity(&[SessionId] lineage) -> Option<String>`; lineage = session, parent, … root, so a child is placed without waiting for a spawn event), `InstallIdentitySource`, `Identity { session }` ask, `Agents.identity`, `Agents::identity_of`. Used by `Turn::rebase` (`loop_.rs`) and `ShowPrompt`. Re-exported from `agent`.
- `crates/roster/src/desk.rs` — `identity(f, journal, max_rounds)` replaces `opening`: `You are ‹Title› (`‹name›`), an agent on this theuth roster; you report to ‹reports_to›. Your charter: …` + `Capabilities …` + `<journal>` + the manual. `child_identity(f, task)`: `You are working for ‹Title› (`‹name›`), an agent on this theuth roster, on task ‹id› — ‹title›. …`. `Shared::clear_desk` no longer forgets the session → the closing (journal) turn still speaks and writes knowledge as the agent; `put_away` forgets it after archiving.
- `crates/roster/src/lib.rs` — `Identities { shared, data_dir, max_rounds }: agent::IdentitySource`, installed on the agent actor with `lookup_when`. A desk session gets `identity`; a descendant gets `child_identity` with the task whose `checkout.session` is the desk (via `tasks::ListTasks`). `theuth`'s desk gets the same shape with `prompt.md` (root-only spans cut) as its charter — see Deviations. `DeliverWake` sends the wake text only (the opening block moved to the system prompt).

## Commit 2 — `feat(desk): fit check, follow-through, nudge table, done_when (70 DESK-PROTOCOL 1–4, paperclip (a)–(f))`

### tasks crate

- `migrations/0003_done_when.sql` — `tasks.done_when TEXT` (replicated with the row; `Store::put/get/list` carry it).
- `types.rs` — `Task.done_when`, `TaskCreate.done_when`, `TaskPatch.done_when`, `TaskPatch.done_because`; `DoneBecause { CriteriaMet | IntentMet | FollowUpsFiled { tasks } }` (Display: `criteria met` / `intent met` / `follow-ups filed (ids)`); `WakeReason::{Reassigned, BlockersResolved, ChildrenCompleted}` (+ `as_str`, `store::reason_of`).
- `ledger.rs`
  - `guard (Review, Done)`: a self-reviewer (reviewer == owner, owner an agent) is refused unless `done_when` is set.
  - `update`: `done_because` required from an `agent:` actor moving to `done` (`user`/`system` default to `IntentMet`); `CriteriaMet` needs a `done_when`; a self-reviewer may only claim `CriteriaMet`; `FollowUpsFiled` must name existing tasks; the verdict is written to the thread as `**actor** (stamp): done because: …`. A move to `review` by an agent with no `done_when` and no reviewer other than itself sets `reviewer = manager` (the review goes to the manager, never to itself).
  - `move_to(id, to, note, done_because, actor, session)`; `MoveTask.done_because` (`#[serde(default)]`, RPC passes `None` = user intent).
  - `reassign(id, to, why, actor, session)` — owner or user only; `todo | doing | blocked`; checkout released, `doing → todo`; a reviewer equal to the new owner becomes the new owner's manager; events `owner` (+`state`, `reviewer`); thread `reassigned to ‹to›: ‹why›`; the old owner's delivered wake is completed; `to` woken `reassigned` (not while `blocked`). `ReassignTask` actor message.
  - `on_closed`: a task whose blockers are all resolved is unblocked with `blockers_resolved` (was `resume`); when every child of the closed task's parent is `done | cancelled`, the parent's thread gets `every child is closed (last: …)` and its owner is woken `children_completed`. `Store::children(parent)`.
  - `no_continuation(task, agent, why)` — the 67 §7.3 rule on one task in any open state (blocked, `no continuation` blocker, checkout cleared, thread line, manager woken `review`); `episode_closing` now uses it; `NoContinuation { id, agent, why }` actor message.
- `tools.rs` — `task_create.done_when`, `task_update.done_when/done_because`, `task_move.done_because`, new `task_reassign { task, to, why }`; `PROMPT` names `done_when`, `done_because`, `task_reassign`. Ten tools registered.
- `crates/app/src/draft.rs` — `Draft.done_when` (rendered as **Done when:** under the brief) so theuth drafts criteria the user can correct before approval.
- One-line literal updates: `crates/rpc/src/api.rs` (`MoveTask.done_because: None`), `crates/tui/src/model.rs`, `crates/signal/src/channel.rs`, `crates/slack/src/channel.rs`, `crates/roster/src/proposals.rs`, `crates/roster/src/lib.rs` (`done_when: None` in `TaskCreate`/`Task` literals).

### roster crate

- `config.rs` — `roster.max_rounds` (default 3).
- `desk.rs`
  - `manual(f, max_rounds)` rewritten as the employee protocol: 1 fit check (`task_reassign`, theuth routes by default), 2 follow through (read → `done_when` → more children ≤ N or child tasks → `task_note` report block → disposition), 3 delegation = child tasks + blockers, woken by `blockers_resolved` / `children_completed`, never poll, sideways talk = a self-contained task, prose is not a waiting path, 4 disposition checklist with the three done reasons in order, the rule "Assigning yourself plus a please-review comment is not a review path", review/comment wake duties, the one-nudge-then-block rule.
  - `roster_block(me, agents)` — the `<roster>` block an `assigned`/`reassigned` wake carries (other active agents: name, title, capabilities one-liner).
  - `wake_text` shows `Done when: …` (or that none is stated).
  - `Desk.serving: Option<Serving { wake, state, owner, events, nudged, rounds }>`.
  - `desk::nudges` — the table Sky rules on: `Nudge::TurnEnd { name, reasons, unhandled, text }` rows `disposition` (assigned, reassigned, manual, schedule, resume, blockers_resolved, children_completed: unhandled when `todo` and unmoved, or `doing` with no live child and no `continuing`/`delegated` disposition since delivery), `review` (unhandled while still `review`), `reply` (comment, mention, answer: unhandled with no event by the agent since delivery); `Nudge::Time` rows `no-progress back-off` (ledger `complete_wake`, 2 min doubling to 30 min) and `stale checkout` (ledger `sweep`). `THEN` is the shared tail: one attempt, then `blocked: "no continuation"` and the manager woken. The `disposition` text is paperclip's four-option shape: done · review {reviewer} · blocked {on} · continuing/delegated.
- `lib.rs`
  - `DeliverWake` — (f) gates on every wake: paused agent, or a `blocked` task with blockers (except comment/mention/answer/manual wakes) → one `wake … held: …` diagnostic in the thread per wake key (`Roster.diagnosed`), and `Err("held: …")` so the ledger leaves the wake pending and retries (5 s → 5 min) until the gate opens. (e) the roster block on assignments. Snapshots `Serving` (state, owner, `task_events` count).
  - `judge_turn(name)` on every desk `TurnFinished` (not the closing turn): looks up the row for the wake reason; if unhandled and a child is live (running, or spawned and not yet finished) → wait (the child's result lands in this session); else first time → thread `nudged: ‹row› — …` + `SendInput` of the nudge (with a "rounds spent" sentence once `rounds ≥ max_rounds`); second time → `tasks::NoContinuation`. Handled → `serving` cleared.
  - `SubagentSpawned` from a desk → `serving.rounds += 1`.
  - `close_desk` now sends `tasks::EpisodeClosing` (the dormant rule had no sender). 179: not for a pause — a pause closes the episode without the journal turn or this rule, and the dispatcher no longer offers a paused agent's wakes (the `DeliverWake` gate is a backstop; `179-pause-suspend.md`).
  - `thread_note` = system progress note (wakes nobody).

### Item 4 — child result continues the same episode

Verified in `crates/agent`: `Agents::child_finished → hand_off → enqueue_peer → Session<PeerInput>` on the **parent desk session itself** (`loop_.rs` `peer_wrapper`, `<message from="session:…">`), starting a new turn there or steering the running one. Same session id = same desk episode; the roster sees it as `TurnStarted`/`TurnFinished` on `d.session`, and `serving` is kept across it (a live child suppresses the nudge). No fix was needed. Constraint (pre-existing, 50 §2.1): with communication off `spawn_subagent` is hidden **and** `hand_off` drops results — a desk cannot delegate at all; Sky's run had it on.

## Deviations / decisions (nobody to ask)

- theuth's own desk also gets the identity-first prompt (`You are theuth (`theuth`) … Your charter: ‹prompt.md›`), not the user-session header. One code path; user chat sessions are unchanged. If theuth's desk should keep the meta-harness header, return `None` for `theuth` in `Identities::identity` and restore the opening block in `DeliverWake`.
- Fit check is manual + roster block + `task_reassign`, not a roster-side automatic reassignment (paperclip "never automatically reassign"; the desk decides and says why).
- Held wakes return `Err` from `deliver` instead of consuming the wake, reusing the ledger's retry; the diagnostic is written once per wake key.
- "Comment wake without a reply" is judged from `task_events` by the agent since delivery (a `task_note` is a `comment`/`note` event), not by reading the document.
- Paperclip's 5-attempt "deliberate wait with nothing to wait on" repair is not built: `guard` already refuses `blocked` without a blockers entry and `continuing` requires `by`; prose-only waits cannot be recorded. Listed in the manual, not in the table.
- `done_because` from a `user` (board approval) defaults to `intent met`; RPC `task_move` passes `None`.
- Commits 2's tasks and roster halves are one commit: the roster literal updates (`done_when: None`) and the manual/nudges live in the same files.
- TS bindings are generated (`just ts`, not checked in); `Task`, `TaskCreate`, `TaskPatch`, `WakeReason`, `DoneBecause` export via `ts_rs` as before. Not regenerated here (`just` is off-limits for workers); `crates/desktop/ui` untouched.
- The brief's `/tmp/67/…` paths and `67-as-built-…` name were taken as `/tmp/69/…` and `70-as-built-DESK-PROTOCOL.md` (the later "Tests:" line).

## Not built

- Follow-through rounds are counted and surfaced (manual, nudge sentence); the roster cannot refuse a fourth `spawn_subagent` — enforcement is the nudge/block path.
- No roster-side test of `judge_turn` end to end (needs live `agent::Agents` and `tasks::Tasks` actors); the decision logic is the pure table, tested in `desk.rs`.

## Tests written

- `crates/agent/src/frame.rs` `identity_replaces_theuth_persona` — `karol` prompt starts with the identity line, has no `You are theuth` / `Theuth —` / prompt.md text, keeps plugin blocks and guidance.
- `crates/roster/src/desk.rs` — `identity_leads_with_the_agent_and_has_the_disposition_rule` (identity + manual content incl. the review-path rule, rounds, wake reasons), `roster_block_lists_active_others_with_capabilities`, `nudge_table_judges_turn_ends` (assigned still todo → four options; reassigned / checked-out-with-live-child handled; child returned with no disposition → nudge; `delegated`/`continuing` handled; review needs the transition, an opinion is not one; comment needs a reply; time rows present), `sessions_follow_the_desk` (a closed desk keeps its agent until forgotten), `wake_text_snapshot` (`Done when:` line).
- `crates/tasks/src/ledger.rs` — `self_review_only_against_explicit_criteria`, `done_because_three_reasons`, `children_completed_and_blockers_resolved_wakes`, `reassign_wakes_the_specialist`; `state_machine_*` and `closing_a_blocker_unblocks_to_previous_state` updated.

## Verification (exact results)

- `cargo check -p roster --tests` — `Finished dev profile` (commit 1 and 2; the one `missing-docs` warning on `nudges::TABLE` fixed before commit 2).
- `cargo check -p tasks -p app -p rpc -p roster -p tui -p signal -p slack --tests` — passed for every crate but `roster` mid-edit (fixed, re-checked above).
- `cargo test -p tasks -p roster --lib --no-fail-fast` — roster `34 passed; 0 failed`, tasks `42 passed; 0 failed` (after updating `wake_text_snapshot` for the `Done when:` line and `closing_a_blocker_unblocks_to_previous_state` for the `blockers_resolved` reason).
- `cargo check --workspace --tests` — fails on two pre-existing, unrelated targets: `nodes` (lib test) `include_str!("../../../bindings/methods.json")` — `bindings/` is gitignored and produced by `just ts`, absent in this worktree; `tui` (lib test) `missing fields agent and project in initializer of ScheduleInfo` at `crates/tui/src/model.rs:12884`, introduced by `0f15704` (routines branch) before this work. `cargo check --workspace --tests --exclude nodes --exclude tui` — `Finished` (exit 0; only the workspace-wide `block v0.1.6` future-incompat note).
- `cargo test -p agent --lib` not run (needs `monty` on PATH per the brief; `cargo build` is off-limits for workers). The agent-side change is covered by `frame::tests::identity_replaces_theuth_persona`, `prompt_parts_in_order_with_blank_lines` and `baseline_by_audience` (signature updated).
