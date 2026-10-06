# 109 as-built — W3 (questions and waits made durable)

Worktree `theuth-109-w3`, branch `feat/109-w3` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: `questions` and `waits` rows from the four human-wait kinds and `send_message{await_reply}`;
re-publish / honour / supersede on resume; `Agents.waits` rebuilt at boot; the §7.2 boot pass over
spared child results; `resume_question_call` / `resume_wait_call`.

## 1. Questions (§6)

Every kind writes one `questions` row on raise (`pending`, kind, card as payload, `deadline =
asked_at + timeout`) plus a `waits(question)` row for the asking call, and the terminal row
(`answered` + answer / `timed_out`) when it ends, clearing the wait. All through the `Journal`
seam; the actors keep their oneshots for the live path.

| kind | raise / resolve | file:line |
|---|---|---|
| `approval` | `Approval` actor: `RequestApproval` (now carries `call_id: Option<String>`) writes the row and the wait; `Resolve` → `answered` with `"allow"`/`"deny"`; `Expire` (timeout) → `timed_out` | `crates/tools/src/approval.rs:171-293`; registry passes `call_id` at `registry.rs:1582`, `None` for `ExternalApproval` (`:1198`) |
| `ask` | `AskUser`: `Request` (now `call_id`) → `arm` writes the row/wait and publishes; `AnswerAsk` → `answered` with the answers JSON (`null` = dismissed); `Expire` → `timed_out` | `crates/tools/src/ask.rs:186-253` (`arm`), `:272-296` (`Request`), `:343-363` |
| `handoff` | the tool body itself (no actor): row id = the `handoffs` row id, payload = that row; polls the row to `q.deadline`; `answered` `{outcome}` / `timed_out` | `crates/roster/src/handoff.rs:186-292` |
| `secret` | `secrets::tools::request`: the tool mints the `RequestId`, writes the row (payload `{id, title, fields}` — field names/labels/`current` versions, never values), asks the actor with `Request.id = Some(id)`, settles `answered {outcome: stored|declined}` / `timed_out` | `crates/secrets/src/tools.rs:201-300`; `secrets/src/lib.rs:162-176, 392-425` (`Request.id`, re-arm) |

Tools outside `crates/tools` (handoff, secrets, `send_message`) reach the installed journal through
`tools::registry::journal()` — a process-wide handle set by the `InstallJournal` handler
(`registry.rs:1369-1379`); `NoopJournal` before the install.

### 1.1 Resume path A — `Republish` (`started` human-wait call)

`resume_question_call` (`crates/agent/src/loop_/resume_calls.rs:43-91`) asks the supervisor
`ResumeQuestion { turn, call_id }` → `(session, newest questions row of the call)`
(`supervisor/waits.rs:319-333`), hands the row to `tools::registry::republish(row)`
(`registry.rs:1381-1398`, keyed `(session, call_id)`), and re-invokes the call through the
registry's ordinary `Invoke`. The tool body takes the row with `take_republish(cx.session,
&cx.call_id)` and, instead of raising a new question:

- `ask_user` (`ask.rs:413-470`): a `pending` row → `AskUser::Republish(row)` (`ask.rs:298-341`)
  re-inserts the waiter **under the row's id** with `deadline - now` left and publishes the same
  `AskRequested` (same id → the desktop card and 104's `ask:<id>` item come back); `PendingAsks`
  lists it, so the attention item re-derives. Deadline passed → `timed_out` row and the tool's own
  `no answer from the user in time`. An `answered` row → rendered from `row.answer` without
  asking. Idempotent on the id (§6.4): a second `Republish` arms nothing.
- `desktop_handoff` (`handoff.rs:190-210`): a `pending` row → polls the existing `handoffs` row
  until `q.deadline`; `answered`/`timed_out` rows → the stored outcome.
- `secret_request*` (`secrets/tools.rs:209-237`): a `pending` row → `Request { id: Some(row id) }`;
  the actor finds the existing `secret_requests` row and re-arms its `wait()` on it with
  `created_ms + request_timeout_secs − now` (`lib.rs:396-402`), publishing `SecretsEvent::Requested`
  with the same id; an already-answered row resolves at once from the row.

Re-invoking through `Invoke` keeps the journal consistent: `call_started` on a `started` row is a
no-op (contract), `call_finished` closes it with what the model sees.

### 1.2 Resume path B — approvals (`planned` call)

A `planned` call replays through the registry and the gate asks again. At boot
`Agents::rebuild_waits` (`supervisor/waits.rs:102-191`; the body is `rebuild_waits_with(approval)` so a test can hand in the actor) walks every open turn's `planned` calls:
a `pending` approval row → `question_resolve(id, Superseded)` (its registry invocation died with
the process; §6.2); an `answered` approval row → `Approval::HonourAnswer { session, call_id,
decision }` (`approval.rs:99-106, 163-169`). The next `RequestApproval` for that `(session,
call_id)` resolves with the stored decision at once — no card, no new row (`approval.rs:179-195`);
`deny` surfaces as the registry's `denied by user`. Approval-before-`Start` is untouched: the
gate still runs before `call_started` and the body.

## 2. Waits (§4.4)

- `peer_reply`: `SendMessageTool::call` writes one `waits` row per call before awaiting
  (`crates/agent/src/peers.rs:483-508`): `target` = the first awaited exchange id, `payload =
  {request_id, request_ids: [all]}`, `deadline = now + reply_timeout_secs`; cleared after the
  await loop (`:522-524`). (The PK is `(session, call)`, so a fan-out's ids ride in the payload.)
- `question`: written by the four kinds above (`target` = question id).
- `Agents::rebuild_waits` (boot, after `reap_jobs`, before `resume_scan`): for every open turn's
  `peer_reply` row and each of its ids, `rearm_peer_wait` (`waits.rs:196-240`) re-creates the
  `Wait` in `Agents.waits` via the existing `Agents::wait` with the route's current generation
  (`route(session, to)` / `remote(session)`) **only while the exchange is still `accepted` /
  `consumed`**; the receiver is parked in the module-level `REARMED` map until the resumed call
  takes it. A reply arriving in between settles through `settle_wait` as before and is buffered in
  the oneshot. Terminal exchanges are not re-armed: the receipt is the result.
- `resume_wait_call` (`resume_calls.rs:96-149`) asks `ResumeWait { turn, call_id }`
  (`waits.rs:336-399`): per awaited id the current receipt and, for the open ones, the live
  receiver; `replied` receipts are delivered as they stand; open ones are awaited until the row's
  remaining deadline (`remaining(deadline)`), a passed deadline being the tool's own `timed
  out…; message not recalled` receipt with `DropWait`; output via `peers::receipts_output` (made
  `pub(crate)`). No `waits` row → the interrupted note (nothing was accepted before the kill).
- `child_result`: `ResumeWait` consumes the spared `result:` exchanges from `session:<child>`
  (`consume_exchange_direct`) and returns their texts as the call's result (`waits.rs:371-396`).
  No code writes a `child_result` row today — `spawn_subagent` returns immediately and
  `send_message`'s child answer is a `peer_reply` on its exchange — so this arm is reachable only
  by a future writer; the brief asked for it, it is small, and the boot pass below respects it.

## 3. Peer side (§7.1, §7.2)

- `PeerMessagePage`'s pending sets still derive from live waiters (`Agents::pending_requests`,
  unchanged); because `rebuild_waits` refills `Agents.waits` from the rows, the sets are
  "rows + live waiters" after boot without a second source. Doc comment corrected
  (`communication.rs:520-523`).
- Boot pass over spared results: `rebuild_waits` ends by `try_send`ing `DeliverSparedResults`
  to the supervisor itself (`waits.rs:184-190`) — queued behind the running `ActivityRegistered`
  handler, i.e. after `resume_scan`, as §7.2 orders. The handler (`deliver_spared_results`, `waits.rs:282-317`) reads
  `Store::spared_child_results()` and enqueues each into its parent through `enqueue_peer`
  (`deliver_spared`, `waits.rs:251-278`: `enqueued` dedup, route generation, `fail_exchange` on
  refusal — `hand_off`'s tail without the live-child bookkeeping); a result whose parent has an
  open turn with a `child_result` wait on that child is skipped (the resumed call consumes it).

## 4. Tests

`crates/tools/src/approval.rs::tests` (3): `question_row_pending_then_answered` (row + wait on
raise, `answered`/`"allow"` + wait cleared, stale `Resolve` refused), `deadline_elapsed_is_timeout`,
`answered_row_honoured_on_replay` (no card, no row, consumed once). A `RecJournal` recorder lives
there (`pub(crate)`) for `ask.rs::tests` (2): `republish_same_id` (raise on actor 1, kill it, a
second actor over the stored row via the tool's `take_republish` path re-publishes the **same id**,
`PendingAsks` lists it, duplicate `Republish` is a no-op, the answer resolves the resumed call,
the row ends `answered` with the answers) and `republish_past_deadline_and_answered_row`.

`crates/agent/src/supervisor/waits.rs::tests` (4, over a real store via
`supervisor::tests::agents_with`): `superseded_on_reask_and_answered_row_honoured` (pending →
`superseded`, answered → honoured by the registered `Approval` actor, other kinds/started calls
untouched, `resume_question` session lookup), `wait_rearmed_with_remaining_deadline` (waiter back
in `Agents.waits` and in both sessions' pending sets, remaining deadline, reply after the restart
resolves the taken receiver, passed deadline, no row), `replied_exchange_delivered_on_resume`,
`spared_child_results_delivered_once` (admitted once, dedup on repeat, awaited child kept for the
resumed call and consumed there).

## 5. Deviations from the design / brief

| what | design says | built | why |
|---|---|---|---|
| supersession | "a new row is raised with a fresh id and the old pending row is set `superseded`" when the replayed call re-asks | the old `pending` approval row of a `planned` call is set `superseded` at boot in `rebuild_waits`; the replay then raises its fresh row | the dead invocation can never be answered whether or not the call replays (`resume = off/ask`), and the registry needs no row reader; `approval::tests::superseded_on_reask` therefore lives in `supervisor/waits.rs::tests` |
| honouring an `answered` row | "the registry honours the stored answer" | `rebuild_waits` reads the row and tells the `Approval` actor `HonourAnswer`; the actor answers the replay's `RequestApproval` from memory | keeps the `Journal` trait write-only (contract); the map is consumed by the one replay |
| `Republish(question)` | the actor is told to re-insert and publish | the resumed call is re-invoked through the registry; the tool body takes the row from `tools::registry::republish` and re-arms it (`AskUser::Republish` exists for `ask`; handoff/secrets re-arm on their own durable rows) | one path for all four kinds without a per-kind renderer in the loop; the registry's `call_finished` closes the journal row the same way a live call does |
| `resume_*_call` signature | `(call, plan)` (contract) | kept; the session is resolved from `plan.turn` via `open_turns()` in the supervisor | the stubs carry no session; `TurnId` is a uuid, so the lookup is exact. Open item for W1/integrator: pass the session and drop `session_of_turn` |
| `peer_reply` row | `{request_id, deadline}` | `{request_id, request_ids}` + `deadline` column; `target` = first id | one row per call (PK) but a fan-out awaits several exchanges |
| `child_result` writer | `send_message` to an owned child / `spawn_subagent`'s implicit wait | not written: neither tool blocks on a child result today | nothing to journal; resume arm and boot-pass exclusion kept so a future writer works |
| secret row id | actor-minted | tool-minted and passed in `Request.id`; a `Some(id)` naming an existing row re-arms it | the tool must know the id before the ask to write the row; the same field carries the resume |
| `wait{question}` for approvals | §4.4: "a `started` call … writes a `waits` row" | written for the `planned` call too (the gate precedes `started`) | §4.4's table lists the approval gate under `question`; the row has no FK to `tool_calls` |

## 6. Cross-seam touches

- `crates/agent/src/store/durability.rs:622-641` — `Store::questions_of_call(session, call_id)`
  (newest first): the contract has only `questions_pending`; the honour/republish paths need the
  `answered` row.
- `crates/agent/src/store/communication.rs:1418-1433` — `Store::spared_child_results()`: the
  boot pass needs the spared rows without knowing their recipients.
- `crates/agent/src/peers.rs:365, 483-524` — `receipts_output` `pub(crate)`; the `peer_reply`
  wait row around the await loop (the only place that knows the call id and the exchange ids).
- `crates/agent/src/supervisor.rs:113-114` — `// 109 W3` re-export of `ResumeQuestion`,
  `ResumeWait`, `remaining`.
- `crates/agent/src/communication.rs:520-523` — doc comment of `PeerMessagePage` (its
  "after a restart both are empty" was made false by this wave).
- `crates/secrets/src/lib.rs:162-176, 392-425` — `Request.id: Option<RequestId>` and the re-arm
  branch; `:899` test constructor. `crates/mesh/src/lib.rs:870` — `id: None`.
- `crates/tasks/src/lib.rs:823`, `crates/rpc/tests/prompts.rs:92, 107` — `call_id: None` for the
  new `ask::Request` / `RequestApproval` field.

Not touched: `loop_.rs`, `jobs.rs`, `pty.rs`, `shell.rs`, `acp_child.rs`, `supervisor/resume.rs`,
`supervisor/drain.rs`, `crates/desktop/**`, `crates/tui/**`, the migration.

## 7. Not done / open items

- `resume_question_call` / `resume_wait_call` are exercised only through their supervisor halves
  (`resume_question`, `resume_wait`) and the tools' `take_republish` paths; the glue needs the
  `AGENT_ACTOR` registered and W1's wiring — W6's harness covers it end to end.
- A human-wait call whose question row was `answered` microseconds before the kill but whose body
  never returned: `ask_user`/handoff/secret render the stored answer (path A); an approval's
  `answered` row for a `started` call cannot occur (the gate precedes `started`).
- `questions.session_id REFERENCES sessions(id)`: an `ExternalApproval` for a session outside the
  durable store (a private tree) fails the insert; `StoreJournal` logs it and nothing is surfaced
  — private trees have no journal (§4.6), consistent.
- `tools::registry::journal()` / `republish` are process-global (one engine per process);
  `REARMED` in `supervisor/waits.rs` likewise, keyed by exchange uuid.
- Shared `CARGO_TARGET_DIR` note for the integrator: cargo's artifact hashes exclude the workspace
  path, so sibling worktrees overwrite each other's `libtools-*`; a `touch` of the changed sources
  before a gate forces the rebuild (done before every gate below).

## 8. Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-target`, cargo 1.98.1:

1. `cargo test -p tools` — `159 passed; 0 failed` (lib) + `0 passed` (doc). The 5 new tests:
   `cargo test -p tools --lib -- approval:: ask::` → `7 passed`.
2. `cargo test -p agent` — `393 passed; 0 failed` (lib) + `0 passed` (doc). The 4 new tests alone:
   `cargo test -p agent --lib -- supervisor::waits` → `4 passed`. (One earlier full run failed
   `superseded_on_reask_and_answered_row_honoured` with `ActorStopped`: the test had looked up
   the process-wide `APPROVAL_ACTOR`, which another test's dying runtime owned; fixed by handing
   the actor to `rebuild_waits_with` directly. No shared-state test depends on the registry name.)
3. `cargo test -p secrets -p roster` — `23 passed` (secrets lib), `57 passed` (roster lib), docs
   `0 passed`.
4. `cargo clippy -p roster` and `cargo clippy -p tools -p agent -p secrets -p tasks -p mesh
   --tests` — 0 warnings in any touched file (0 warnings overall). `cargo clippy -p roster
   --tests` stops on a pre-existing `clone_on_ref_ptr`-class lint in `roster/src/attention.rs:1672`
   (untouched; left alone per the brief).
5. Not run: the Tauri CLI, `cargo fmt --all`, `--workspace` gates (brief). `cargo check -p tasks`
   and `-p mesh` passed as part of step 4.

Every gate was preceded by `touch` of the touched sources because the shared target dir's
`libtools-*` is overwritten by sibling worktrees (see §7); two runs that skipped the touch
compiled `agent` against a sibling's `tools` and failed with `HonourAnswer`/`republish` not found
— not a code defect, re-run green after the touch.

Files touched: `crates/tools/src/{approval.rs, ask.rs, registry.rs}`;
`crates/secrets/src/{tools.rs, lib.rs}`; `crates/roster/src/handoff.rs`;
`crates/agent/src/supervisor/waits.rs`, `crates/agent/src/loop_/resume_calls.rs`,
`crates/agent/src/{supervisor.rs, peers.rs, communication.rs}`,
`crates/agent/src/store/{durability.rs, communication.rs}`; `crates/mesh/src/lib.rs`,
`crates/tasks/src/lib.rs`, `crates/rpc/tests/prompts.rs`; `docs/design/109-as-built-W3.md` (this note).
