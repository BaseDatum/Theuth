# 132 — as built, ENG (engine half of ATTENTION-ROWS-AND-ORDER)

Branch `feat/132-eng`, cut from `7b518912`. Contract: `docs/design/132-attention-rows-and-order.md` §2.1–2.3. TS shapes of §2.4 are unchanged (`Priority`, `Task.priority`, `TaskCreate/TaskPatch.priority`, `Packet.if_wrong`, `AttentionItem.priority` + `rank`, `Waiting`, `task_move(id, state, note, next)`).

## Shape changes against §2 (read first)

- `ALREADY_WAITING` is a function, not a const: `tasks::ledger::already_waiting(decision: &str) -> String` (the id is interpolated; a `&str` const cannot carry it). Text as the design.
- `Ledger::review_packet(&self, m: tasks::ReviewPacket)` takes the message (which gains `options`, `recommended`, `if_wrong`) instead of six positional args. `Ledger::move_to` gains `next: Option<u32>` after `done_because`; `tasks::MoveTask` gains `next`.
- `WakeReason::Answer` now applies to a `done` task (`types.rs` `applies_to`): the D3 `**next**` wake would otherwise be dropped as stale by `wake()`, `dispatch()` and `drop_stale_wakes`. A late `decide` on a done task therefore also wakes the owner (it used to be dropped).
- `Escalate` message / `escalate` tool args gain `if_wrong`; `task_update` / `task_move` tool args gain `options`, `recommended`, `if_wrong` (+ `next` on move, `priority` on create/update).
- New message `tasks::WaitingOnUser(actor)` → `Vec<ledger::Waiting>`; new message `agent::LateAnswer { id, answers }` (D6 hook); new store read `agent Store::question_get(id)`.

## File map (path:line)

- `crates/tasks/migrations/0008_priority.sql` — `ALTER TABLE tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'medium'`.
- `crates/tasks/src/types.rs:82-132` `Priority` (weights 9/7/5/2, `as_str`, `parse`); `Task.priority` :260; `TaskCreate.priority` :534; `TaskPatch.priority` :578; `WakeReason::applies_to` :340 (Answer on Done).
- `crates/tasks/src/rank.rs` — `RANK_STEP`, `project_score`, `rank` + 3 unit tests.
- `crates/tasks/src/config.rs:26` `TasksConfig.project_order`.
- `crates/tasks/src/store.rs:85,148,153-156,187,212` — `priority` column read/write (`COLS`, `put`).
- `crates/tasks/src/packet.rs:31-36` `IF_WRONG_MAX`, `ROW_BUDGET`, `REVIEW_OPTIONS_MAX`; :58 `NEEDS_IF_WRONG`; `Packet.if_wrong` :243; `Ask.if_wrong` :269; `validate` :296-309 (if_wrong cap, review options ≤ 4); `require_for_user` :479-494; `thread_markdown` "If wrong:" :580; `head_sentence` :698-728; tests `user_audience_rule`, `if_wrong_cap_and_review_options_cap`, `head_sentence_clips_on_a_sentence_then_a_word`.
- `crates/tasks/src/ledger.rs:34-52` `Waiting`; :229 `PRIORITY_IS_THE_USERS`; :233 `already_waiting`; `create` gate :576 + field :619; `update` gate :936 + change log :1053; `move_to` `next` :1186-1222, :1278-1282; `escalate` dedupe + inheritance :1566-1608, `if_wrong` on the packet :1633; `review_packet` :1697; `waiting_on_user` :1806-1853; `dispatch` key :1875-1946. Tests: `priority_is_the_users_to_set`, `escalate_refuses_the_same_actors_second_ask_to_the_user`, `escalate_refuses_an_over_cap_if_wrong_before_writing`, `review_options_and_task_move_next`, `dispatch_picks_by_rank_then_answer_class_then_age`.
- `crates/tasks/src/lib.rs:117-121` `WaitingOnUser`; `MoveTask.next` :164; `Escalate.if_wrong` :275; `ReviewPacket.options/recommended/if_wrong` :316-324; handlers :606, :638, :696.
- `crates/tasks/src/tools.rs:61-74` registration (eleven tools); `TaskWaitingOnUserTool` :378-451 (`age`, one line per row, empty text); `priority` on create :175 / update :475 args; `TaskUpdateArgs.options/recommended/if_wrong` :497-507; `TaskMoveArgs.next/options/recommended/if_wrong` :687-699; `review_gate` :792; `EscalateArgs.if_wrong` :1333; descriptions of `task_update` :591, `task_move` :792, `escalate` :1418 (D9).
- `crates/rpc/src/api.rs:1816-1827, 5839-5857` `task_move` gains `next`. `crates/rpc/src/export.rs:333-334` exports `Priority`, `ledger::Waiting` (the ts export list lives here, not in `api.rs`).
- `crates/roster/src/attention.rs:122-130` `AttentionItem.priority/rank`; `Inputs.project_order` :289; `derive` :310-805 (`ranked`, `by_session`, decision/review rows :495-579, hire :604, budget crossing :642-654, order :792-803); `gather` reads `tasks.project_order` :1084. Tests `order_is_rank_then_waiting_then_newest`, `budget_item_carries_the_crossing_time`; seven existing tests moved to the new order/strings (below).
- `crates/roster/src/desk.rs:75-91, 101-113` manual 2(iv) and 4 (D9).
- `crates/tools/src/ask.rs:561-585` `late_answer_text` + test `late_answer_text_one_line_per_question`; re-exported from `tools`.
- `crates/agent/src/lib.rs:310-360` `answer_ask` late path + `LateAnswer`; `crates/agent/src/supervisor.rs:2737-2765` handler (journal row → `AskRequested` payload → `human_input` on the row's session); `crates/agent/src/store/durability.rs:1025-1034` `question_get`.
- `crates/document/board.md:99-111` D9 paragraph in "Mini-boards for decisions and reviews".
- Mechanical one-liners for the new required fields outside my ownership (struct literals would not compile otherwise): `crates/roster/src/lib.rs:1122`, `crates/roster/src/proposals.rs:39` (`priority: None` on `TaskCreate`), `crates/signal/src/channel.rs:1382`, `crates/slack/src/channel.rs:1440` (same), `crates/tui/src/model.rs:14309` (`Task` test literal), `crates/double/src/actor.rs:809-810` (`AttentionItem.priority/rank`).

## What was verified and how

- `cargo clippy -p tasks -p roster -p tools -- -D warnings`: clean. `cargo test -p rpc --features ts --lib --quiet export_bindings`: 8 passed; `bindings/Priority.ts` (`"urgent" | "high" | "medium" | "low"`) and `bindings/Waiting.ts` exist, `AttentionItem.ts` has `priority?: Priority | null` and `rank: number`, `Packet.ts` has `if_wrong: string | null`, `Task.ts` has `priority: Priority`, `TaskCreate.ts` `priority: Priority | null`, `TaskPatch.ts` `priority?: Priority | null`. Nothing under `bindings/` committed. Note for UI: ts-rs emits `TaskCreate.priority` and `Packet.if_wrong` as required-nullable (`: X | null`), like every other `Option` on those two types; the §2.4 `?:` stubs are a superset and type-check against them.

- `cargo test -p tasks`: 100 passed (was 91 + 9 new/updated). Covers §5.1: `head_sentence` (verbatim · sentence clip past 40 · word clip with `…` · `#` stripped · hard cut), `rank` (unranked 0, ranked beats unranked, step dominates priority, weights), `ALREADY_WAITING` same actor refused with nothing written / another actor's forward allowed / answered → allowed again, `if_wrong` required for Decision-to-user after board+summary, optional on Review, inherited on forward and mirrored as `If wrong:`, over-cap refused before writing, review `options` ≤ 4 accepted and `task_move done next=1` writes `**next** (2) Revise Y first` + wakes the owner `answer` (out of range → "offers 3 moves (0..2)", no options → refusal), `priority` from `agent:ops` refused on create and patch / from `user@desktop` accepted and logged as a `priority` event, `dispatch` picks the project-ordered task first, then Answer-class before older Assigned at equal rank, then Urgent before Medium, `waiting_on_user` lists only the caller's line (lead and ops see it, `other` does not, the user sees all).
- `cargo test -p roster`: 66 passed. Decision row = clipped question + `agent:ops · deploy`; review row = summary sentence (112 chars) + `“migrate” · by agent:ops`; legacy rows unchanged; order rank → waiting_since → at_ms with a checkout-borrowed priority on a session item and `finished` (None) last; budget `at_ms` = the filed budget task's `created_ms`, `now` only without one.
- `cargo test -p tools`: 175 passed, 1 ignored (pre-existing). `late_answer_text` one line per question (choice label, typed text, `(skipped)`), empty for no answers. The agent-side delivery (`LateAnswer` → `human_input`) is exercised only by compilation; no integration test (the existing ask harness has no journal-backed agent store).

## Deviations from the design

- `ALREADY_WAITING` fn, `review_packet(ReviewPacket)`, `Answer` applies to `done` — see "Shape changes".
- Dedupe condition: refused when any pending `decision:` packet on the task has `by == actor` and `to == "user"`, regardless of who the actor's manager is now (the design's literal condition; the manager is the user in every real case).
- `priority` gate: any `Some(priority)` from a non-user actor is refused, even when equal to the current value (the smaller reading of "priority from a non-user actor → Err"). `system` counts as non-user.
- Session items borrow their checkout task's priority **and rank** (the design text gives session items `priority` and leaves `rank` at 5; one rank for the inbox is the D4 intent, and a UI grouping by `priority` without the matching `rank` would split the order).
- Budget crossing time: the roster keeps no timestamp; the open `Budget: <agent> …` / `Budget override: <agent> …` task the roster files at the crossing is the record (its `created_ms`), `now_ms` otherwise. Hire time was already the hire task's `created_ms`.
- Existing tests moved from newest-first to the design order (rank → waiting_since asc → at_ms desc) and from the old row strings to D1: `attention.rs` `derives_every_kind_and_hides_dismissed_or_snoozed`, `decision_and_review_items_carry_their_packet`, `decision_item_is_the_newest_one_addressed_to_the_user`, `handoffs_and_secret_requests_become_items`, `resume_items_lifecycle`, `asks_and_shows_become_items_with_their_session_and_desk`, `task_items_carry_the_working_session`; `ledger.rs` `escalate_forwards_the_parent_packet_with_a_recommendation` (mirror now carries `If wrong:`), `escalate_user_path` / `escalate_to_user_needs_a_summary_and_a_board` / `escalate_with_a_packet_stores_and_mirrors_it` / `desk_ask()` (user-addressed asks carry `if_wrong`); `packet.rs` `user_audience_rule`.
- The ts export list is in `crates/rpc/src/export.rs`, not `api.rs`; edited there.
- D6 split: the text lives in `tools::ask::late_answer_text` (unit-tested); the lookup and delivery live in the agent crate (`LateAnswer`), because `tools` cannot read the journal or reach a session — `answer_ask` in `agent/src/lib.rs` is where every frontend's answer already passes. The RPC still returns `false`.

## Open items

- Files outside ENG ownership touched with one-line field additions (see File map): roster `lib.rs`/`proposals.rs`, signal, slack, tui, double. Orchestrator to fold; no behaviour change.
- `task_waiting_on_user {project}` filters by one `GetTask` per row (ponytail: fine for a handful; a `project` on `Waiting` would avoid it).
- `Priority::Urgent` pre-emption of running children: out of scope (design §6).
- The `agent` crate's own tests were not run as a gate (not in the brief; the crate compiles under the `rpc` export gate).
