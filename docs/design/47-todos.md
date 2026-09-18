# 47 — Todos: visible plans and continuation only at verified quiescence

Status: proposed · not implemented. Extends [07 §9](07-context.md#9-todo-list),
[01](01-config-and-subagents.md), [14](14-jobs.md), [19](19-subagent-board.md),
[22](22-pty.md), [25](25-desktop.md), [29](29-rewind.md), and [31](31-link.md).
The source observations below describe the current tree, not completed work.

## 0. Decisions

1. **Plan before multi-step work.** Strengthen the existing static todo
   guidance; retain the existing loop-local `todo` tool. A greeting or direct
   answer does not need a plan. An explicit checklist must be represented in
   full before execution, not reduced to its first step.
2. **Nudge only a natural yield with actionable todos and proven quiescence.**
   No admitted/running/finishing work, active descendants, waits, incoming
   inputs, or undelivered results may remain. Unknown is not idle.
3. **A scheduling pause is not a forgotten plan.** Running jobs, PTYs and
   children already have completion paths. Let those paths wake the model;
   never append a reminder merely to keep it talking or polling.
4. **Make the decision in the existing agent supervisor.** `Agents` owns
   session/child admission and incoming messages. Add acknowledged activity
   admission and completion handoff from tools, not a polling watchdog,
   frontend timer, new plugin, or general workflow engine.
5. **An eligible nudge continues the same native turn.** Decide before public
   `TurnFinished`; append one harness note and resume the existing loop with
   its existing limits. No premature parent result, print-mode exit, or ACP
   prompt completion followed by a surprise automatic turn.
6. **A persistent, collapsed-by-default todo card in every console.** Desktop
   and mobile share one component outside the transcript. The TUI gets the
   same fixed-position disclosure. Current state arrives as typed snapshots
   and change events, never by finding or parsing an old tool card.
7. **Persist the plan in the existing session database.** The added console
   requirement earns structured persistence: reopen, reconnect, compaction,
   undo/redo and forks must show the correct list without history scrolling.
   Plans remain per-session, not a shared task database.
8. **Blocked is different from done.** Add `block`/`unblock` with a reason for
   work awaiting external input. Pending and in-progress items are actionable;
   blocked, done and dropped items do not request continuation.

These follow [00](00-architecture.md): typed actor commands, plugin-owned
contracts, no god-wiring, one RPC surface, and glue rather than a new
subsystem. The additional lifecycle accounting is necessary for the user's
safety condition; a three-line `if todos && !running` would not meet it.

## 1. What exists, and what cannot be reused as proof

### 1.1 wth

| Current behavior | Source / consequence |
|---|---|
| Static guidance already says to keep a todo list for three or more steps. | `crates/agent/src/frame.rs`, `GUIDANCE`. Strengthen this, do not add another prompt builder. |
| `todo` has `init/add/start/done/drop/view`, text identifiers, 32-item cap, and auto-promotion. | `crates/agent/src/todo.rs`. No blocked state or structured persistence today. |
| A reminder is appended before a model call after four rounds without todo interaction. | `Turn::round`, `loop_.rs:1547–1556`. This is an in-turn progress reminder, not a stop guard. It cannot wake a stopped model. |
| The running task owns `SessionState`; `Session.state` is `None` until `TurnDone`. | `loop_.rs:154–275,391–507,712–724`. `TurnFinished` precedes title generation and state handback (`1293–1393`). Public finish is not an idle certificate. |
| `TurnDone` restores state and starts a new turn if steering remains. | Same handler. Testing only `inflight == None` misses input and handoff races. Manual compaction and harvest also use `TurnDone`; they are not model yields. |
| `Agents.running` and the board are event projections. A spawned child can be waiting for a semaphore before `TurnStarted`. | `supervisor.rs:606–647,1103–1219,1312–1371`. Neither the running set nor an `Idle` board row proves no active child. |
| Every child turn finish routes its result to the parent, or resolves the parent's peer waiter. | `Agents::child_finished`, `supervisor.rs:571–603`. Native, external ACP and node-backed children must all count. |
| A job becomes terminal before its completion event is published. | `crates/tools/src/jobs.rs`, `run` (`690–808`); PTY equivalent in `pty.rs:612–710`. Zero running rows can coexist with a result not yet sent. |
| `Runtime::publish` awaits enqueue to the bus, not processing by subscribers. `inject` uses a session `tell`. | `runtime/src/lib.rs:44–60`; `supervisor.rs:125–134,464–558,674–707`. `MessageDelivered` is currently not a receipt. |
| Registry invocations run in detached tasks, including approval and nested calls. | `tools/src/registry.rs:423–500`. Dropping a caller does not stop the tool. Cancellation can emit a synthetic `ToolCallFinished` while real work continues. |
| Quiet shell calls have no visible job row/event. PTY readiness/send/wait use delegated tasks. | `tools/src/jobs.rs:175–193,408–516`; `tools/src/pty.rs:309–561`. Visibility and actual activity are different. |
| Resume, undo and redo rebuild an empty native todo vector. | `SessionState::resume`, `loop_.rs:206–232`. A last list in prose is not enough for a persistent current-state card. |
| ACP plans are rendered as transient `TextDelta`, not persisted as a structured plan. | `acp_child.rs`, `SessionUpdate::Plan` translation (`419–423`). This is important in a delegation-first harness. |
| Desktop/iOS/browser use the same React app and per-harness session store; TUI has a fixed agent board outside its transcript. | `desktop/ui/src/routes/c.$session.tsx`, `store/{sessions,reduce}.ts`; `tui/src/ui.rs`, `board`/`view_rows`. Reuse these layout/state conventions. |

Do not treat historical “What exists today” sections in 14/19 as current
implementation. Their later as-built sections and the code supersede them.

### 1.2 What to borrow from oh-my-pi

Reference sources are under `../oh-my-pi/packages/coding-agent/src/`:

- `prompts/system/eager-todo.md` and `session/todo-tracker.ts`: early planning
  guidance and a bounded stop-time reminder listing unfinished tasks.
- `tools/todo.ts`: explicit blocked/actionable distinction and canonical state.
- `session/agent-session.ts`, `#hasPendingAsyncWake`: counts both running jobs
  **and queued/in-flight result deliveries**, including the yield queue.
- `async/job-manager.ts` and `session/yield-queue.ts`: preserve ownership until
  delivery is accepted. Finished execution is not finished delivery.

Do **not** transplant its predicate or scheduler. Its todo gate filters out
watched/acknowledged jobs and does not establish absence of jobless active
children, all supervised-process completions, or all peer-message wakes.
`TodoTracker.checkCompletion` also awaits event publication between checking
eligibility and appending the reminder. wth must not inherit that stale-check
window. First-message punctuation heuristics and prose question detection are
not reliable task or pause state either.

## 2. Model-facing plan contract

### 2.1 Beginning of work

Replace the last todo sentence in `frame::GUIDANCE` with static guidance:

> Before substantive work on a request with three or more steps, or any
> explicit user checklist, initialize a todo list covering the whole request,
> including verification. Preserve every requested item. Keep task text short
> and stable. Update the list as work completes; continue working after
> bookkeeping. Incorporate new user requirements into the current plan.
>
> A job or subagent may continue after you end a turn; its result will wake
> you. Do not poll or send filler to stay active. Before a final answer,
> reconcile your todos with actual results. If remaining work requires user
> input or another external prerequisite, block those items with a reason;
> do not mark unfinished work done. Use `ask_user` when a decision is needed.

No forced provider tool-choice, new first-turn API, or model-based task
classifier. The instruction is present from the first model call and remains
in the stable baseline. The existing four-round reminder stays a separate
in-turn mechanism; use the same actionable predicate and avoid adding it
immediately after a yield reminder. It never authorizes continuation.

### 2.2 States and operations

Retain text identifiers and the 32-item cap. Extend `Status` with `Blocked`
and `Todo` with an optional `reason`. Extend `Op` with `Block` and `Unblock`:

- `block {task, reason}` requires a non-empty reason and changes an open item
  to blocked. `unblock {task}` changes blocked to pending and clears its reason.
- Auto-promotion selects only pending items. `start` is not an implicit
  unblock; done/dropped items are not silently reopened.
- After a mutation, retain at most one in-progress item and promote the
  earliest pending item if none is in progress. Validate before changing the
  live list; errors leave it unchanged. Exact identifiers must be unique.
- `done` means verified completion. `drop` records an intentional scope
  removal, not “I could not finish”. `init []` explicitly clears the list.
- `view` and no-op operations do not advance the state revision or count as
  progress. A state transition or reason change does.
- Waiting on an owned child/job may be described by a blocked item, but the
  runtime independently tracks that work. Marking items blocked/done/dropped
  must never remove a job, child, wait, or delivery blocker.

No phases, dependencies, assignees, cross-session task sharing, or todo-editing
UI in this change. The parent owns its integration plan; each child owns its
own plan. The persistent card is read-only.

### 2.3 The yield reminder

Append through the existing harness-note path, preserving the cached prefix:

```text
<todo_continue>
You yielded with actionable todo items remaining:
- <current actionable task>
- <next actionable task>
Continue the work. Mark items done only when verified; block items that
require external input and explain the blocker. Do not poll background work.
Reminder 1 of 3 for this request.
</todo_continue>
```

Render task text as data, with wrapper escaping; do not interpolate unescaped
markup into structural delimiters. Reuse the normal todo renderer where
possible. The exact prose is not an API. Typed state, not this wrapper or an
assistant's final sentence, controls eligibility.

The desktop transcript renders this wrapper as a collapsible **Nudge** card
with a bell icon and harness attribution, distinct from a user message or a
model tool call. Expanding it shows the reminder body as literal text, with
wrapper escaping decoded once. Stored nudges retain the usual context-removal
and archive controls; other hidden harness notes remain hidden.

## 3. The quiescence contract

### 3.1 Scope and predicate

“Nothing happening” means **no outstanding work or continuation belonging to
this session or its descendants**, not no activity anywhere in the daemon.
An idle MCP connection, node reconnect loop, browser page animation, or future
schedule is infrastructure. An admitted MCP call, pending browser command,
active scheduled run, or explicitly installed future-result delivery is work.
Other root sessions never block this one.

At the single serialized decision point:

```text
eligible(candidate) =
    candidate is this native turn's current natural-yield checkpoint
    AND todo snapshot is current and contains Pending or InProgress
    AND request is not cancelled, paused, shutting down, or limit-stopped
    AND continuation budget permits another reminder
    AND no ordinary input or continuation is admitted/queued/in delivery
    AND no owned operation is admitted/running/finishing
    AND no descendant has admitted/running/finishing work or pending delivery
    AND no outstanding user/peer/approval wait remains
    AND every relevant activity source is accounted for and healthy
```

The checkpoint itself is excluded from “running turn”; it is waiting for this
decision and cannot simultaneously issue another model/tool call. Every other
piece of work counts, including work from a previous cancelled turn.

The supervisor must positively certify this predicate. Missing actor, closed
recipient, unresolved ownership, failed delivery, or disconnected remote work
means **unknown: suppress the nudge**, not `unwrap_or(false)`.

### 3.2 What blocks it

| State | Required behavior |
|---|---|
| Child admitted but waiting for concurrency capacity / transport / startup | Block from admission, before `SubagentSpawned` or `TurnStarted` can arrive. |
| Native or ACP/node child executing, compacting, retrying, asking, or handing back state | Block. A quiet child is not idle. |
| Idle child with an active job, queued follow-up or active grandchild | Block recursively. An idle child with none of these does not block merely because its actor exists. |
| Shell/PTY job running, including a ready server or quiet REPL | Block. No command-name, readiness, or output-silence exemptions. |
| `start.ready`, `send`, `wait`, tool approval, `ask_user`, peer reply wait | Block until the actual operation settles. A timeout is not process exit. |
| Job terminal, result formatting/queued/in bus/in supervisor/in session mailbox | Block continuously until accepted, then session input/running-turn ownership takes over. |
| Inline job result | No second wake. Cover it through the invocation and normal tool-batch consumption. |
| Quiet/nested invocation or cancelled caller with surviving tool task | Block until actual settlement, regardless of missing UI events. |
| Cancel requested, process reaper/reader or delegated task not settled | Still block. Cancellation acknowledgement is not cessation. |
| Result resolves a peer waiter rather than creating input | Transfer to the still-active waiting invocation; do not briefly expose zero work. |
| Late user message, steering, peer input, recovery, or another reserved continuation | Ordinary input wins; do not also append a reminder. |
| Lost remote connection / unverified teardown / undeliverable completion | Unknown blocks; surface the real failure. A reminder is not a recovery mechanism. |
| Open todos but explicit user pause, error, refusal, output exhaustion, tool-round landing | No nudge. Do not restart around an intentional boundary. |

PTY services can therefore suppress nudges indefinitely. This is deliberate:
the requested policy says no running job. Do not quietly weaken it to “no job
whose result seems useful”. The card still exposes unfinished tasks, and the
user can send input at any time.

### 3.3 Natural yield is positive evidence

Carry a private stop classification alongside the existing display/provider
stop string. A successful, tool-free natural stop is eligible; a missing or
unknown provider reason is not silently treated as proof. Normalize supported
provider normal-stop forms (`end_turn`, `stop`, etc.) at the existing backend
boundary. Preserve the raw reason for public reporting.

Exclude cancellation, transport/auth/context failures, refusals, truncation,
exhausted recovery, manual compaction/harvest, and `landing == true`. In
particular, the landing call can return normal-looking text: inspecting only
its provider stop string would bypass `max_tool_rounds`.

## 4. Ownership and race-free decision

### 4.1 Extend existing owners, not UI projections

`Agents` owns a small private activity ledger alongside its existing live
sessions and parent links. Entries identify an admitted operation and its
owning session; existing turn/call/job identities should be reused where
unique. An opaque nonce is needed for invocations that reuse a caller ID.
Entries need only distinguish unfinished execution, pending delivery, and
unknown outcome. Public job/agent states remain their owners' projections.

The tools crate owns the typed cross-plugin activity protocol, next to
`Invoke` and `ToolCx`. `Agents` implements its recipient and registers that
recipient through a typed command. Use kameo recipients, as the codebase
already does for subscriptions; no `tools -> agent` dependency and no new
shared type in `runtime`. The binary does not wire private handles.

Protocol semantics (names illustrative, contract required):

- **Admit(owner, operation)**: acknowledged insertion **before** execution or
  detached scheduling. An unacknowledged operation may not run. Calls that can
  admit work must leave their actor mailbox available while awaiting the ack.
- **Complete(operation)**: actual work has settled and owes no separate wake.
  Removal can be delayed safely; it must never be early.
- **CompleteWithResult(operation, result)**: atomically change execution into
  a pending-delivery obligation; the supervisor handles the existing job
  result formatting/routing. Never remove then independently publish a wake.
- **AcceptInput(delivery, session-turn/queue identity)**: the receiving session
  has accepted responsibility for the input. Only then retire the delivery;
  its queued/running activity already exists. An enqueue-only `tell` is not
  acceptance. A waiter resolution performs the equivalent ownership transfer.
- **Unknown(operation, failure)**: retain a blocker and report the failure if
  actual termination or result ownership cannot be established.

An admission-only task may await registration without executing the operation;
that is not permission to spawn the tool body first. If admission is rejected,
return an ordinary tool error without side effects. Pair accepted operations
with producer settlement on success, error and panic. Losing the supervising
actor or an operation's completion owner invalidates the affected coverage;
never reconstruct an empty ledger and immediately authorize nudges.

Only the supervising protocol carries this authority. `JobEvent`,
`ToolCallFinished`, `MessageDelivered`, RPC subscriptions and board snapshots
remain observation. For jobs, migrate model delivery off the current
`JobEvent::Finished -> inject` subscriber into the acknowledged result path;
keep public job events for all current UIs. Never deliver both paths.

This is a narrow admission/settlement contract, not a second process manager.
`Jobs` still runs processes. The registry still gates tools. `Agents` still
routes input. No periodic roster/job scans, broker process or durable work queue.

### 4.2 Coverage and lifetime rules

1. `Agents::input`, `SpawnChild`, queued first-child turns, follow-up child
   input and native/ACP turn startup register work before launch. Replace
   event-derived authorization with these direct transitions; events continue
   to feed the board. Child completion remains owned through parent delivery.
2. `ToolRegistry::Invoke` registers the real invocation before its detached
   task, spanning approval, nested calls, tool execution and result handoff.
   Dropping the request future or synthesizing a cancelled tool response does
   not retire the entry. Quiet invocations use the same accounting without
   disclosing command text or secret output.
3. `Jobs::Start` / `StartPty` register independent job lifetimes before spawn.
   The parent invocation remains active across this admission, so returning a
   running stub cannot open a gap. PTY readiness/wait/send tasks retain their
   own lifetime when callers can disappear; RPC paths must participate too.
4. The underlying producer, not the cancelled caller, completes an entry.
   A child task that can outlive its parent task inherits or acquires ownership
   before the parent releases it. Count process termination and required
   reader/reaper teardown honestly; retain unknown rather than infer it from
   a terminal row. Track shell-owned background children through the process
   owner, not by searching command strings for `&`.
5. Browser/render operations that outlive caller timeouts retain the originating
   invocation ownership through actual command completion. Scrape render-slot
   `SessionId`s are resource keys, not the conversation owner. Propagate the
   originating ownership through this existing nested path. Idle browser JS
   is not a promised model wake; a harness-managed pending command is.
6. External ACP/node children count before connection and until their local
   work and delivery settle. On a disconnect that cannot establish remote
   termination, preserve unknown activity for the affected subtree. Ordinary
   node presence or a zero connection count cannot clear it.
7. Register the activity recipient before enabling automatic nudges. A source
   that has not acknowledged coverage cannot be interpreted as empty. No
   best-effort fallback to the old running set. New background tools must use
   this contract before they may claim session-owned asynchronous work.
8. `Unknown` clears only when its owner positively reconciles termination and
   delivery, not when a timeout elapses, the model says “continue”, a todo is
   dropped, or an error row disappears. The warning names the affected operation
   without secret payloads. Normal user-driven work remains possible while
   automatic nudges are suppressed. Existing owners should resolve known
   failures normally; unknown is for a genuinely unverified outcome, not every
   nonzero exit or tool error.

No lifecycle protocol can prove that an arbitrary remote server or a process
that deliberately escaped ownership has stopped. Such uncertainty suppresses
automatic continuation; it is not permission to report quiescence. Do not
promise OS-wide process discovery as part of this feature.

### 4.3 One decision and one ordered commit

The native loop proposes a candidate at a natural stop **before finalization
and `TurnFinished`**. It carries `(session, turn, checkpoint, control_epoch,
todo_version)` and its committed todo snapshot. The session enters a private
`Yielding` checkpoint while its task awaits the decision; it issues no model
call, tool call, or todo mutation during that checkpoint.

All input/work admission and stop-control changes pass through `Agents`.
Queued input remains accounted for until the turn acknowledges consumption;
there is no untracked `Shared::steer`/mailbox interval. User cancel/pause,
clear/rewind, replacement input, and new work invalidate old candidates.

The decisive handler reads only supervisor-owned state. It must not query
actors, await the database, publish an event, or call an extension between
its final checks and the following atomic logical transition:

```text
DecideYield(candidate):
    if candidate is stale or control stopped: reject
    if ordinary input/recovery is pending: use the normal input path
    if activity exists or its state is unknown: finish this scheduling pause
    if no actionable todos or reminder budget exhausted: finish normally
    otherwise:
        consume this checkpoint exactly once
        reserve the next same-turn continuation
        increment the request's reminder count
        enqueue the typed todo reminder in that session's ordered ingress
        return ContinueSameTurn
```

“Enqueue” here is the **authoritative logical commit**, not a suggestion later
rechecked by an unrelated UI callback. Record ownership/order before replying.
The turn drains that committed input through the existing persistence/note
path, then calls the model. No `TurnFinished` is published for this checkpoint.
A dropped reply cannot erase the reservation or allow a duplicate decision;
failed persistence or task loss becomes a visible failure, not silent success.

Work/input arriving **before** this commit invalidates or suppresses the
candidate. Admission arriving **after** it is new activity, ordered after the
committed continuation. It cannot retroactively make the earlier decision
false. Cancellation is still checked before note persistence and provider
execution; an unused reservation can be discarded without injecting a nudge.
Do not return a reusable “idle=true” permit and append a reminder after an
arbitrary await. That recreates the reference race.

This establishes a linearization point for “verified”. No implementation can
promise absence of future input; it can and must promise that all already
admitted work and input was absent at the commit point. A request not yet
admitted by its owner cannot start execution ahead of that point.

Keep message handlers responsive. A session awaiting the supervisor's decision
must do so from its existing spawned turn task, not by blocking its own actor.
Similarly, `Agents` must not await a session handler which can synchronously
ask back into `Agents`; use delegated replies/ack messages for handoffs.

### 4.4 Suppression is not deferred nudging

When a job or child blocks the check, **discard the candidate**. Its completion
uses the normal input path and wakes the model. Assess a new candidate only
when that newly running model naturally yields again.

Do not retain a timer that sends a todo reminder at the last job exit. That
would compete with precisely the result that should wake the model. Duplicate
settlement events and duplicate candidate messages are idempotent by identity.
If unknown work later becomes known-finished without a wake, this design does
not manufacture an old reminder: the next real turn supplies a new checkpoint.

### 4.5 Bounds and user control

Allow at most **three yield reminders per externally initiated request**.
Keep this count through same-turn retries and automatic job/child/peer-result
turns. A fresh human request or initial child assignment starts a new budget;
synthetic continuations do not. Use typed input origin from the existing human
versus harness ingress paths, not XML/punctuation or `InputId` presence.

After a reminder, require a committed todo state change before another
reminder. Read-only `view`, unchanged `init`, tool errors and merely writing
another final paragraph do not reset this latch. State change unlocks another
attempt but does not reset the three-attempt ceiling. At exhaustion, yield
with the unfinished plan still visible; never mark it complete automatically.

Same-turn continuation retains `rounds`, landing state, retry/salvage counters,
cancellation token and any parent runtime deadline. It must not construct a
fresh `Turn` to obtain another allowance. Existing recovery has priority.
User Stop/cancel invalidates candidates even if the model has just reached a
checkpoint. Blocked-only plans wait for real input. No question-mark heuristic
and no model nudge asking it to answer its own `ask_user`.

Loading a saved session does not run its plan. After daemon restart, resume,
clear/undo/redo or fork, old candidates/reservations are never replayed. Durable
todos are display/reasoning state, not a startup execution queue.
An unclean daemon/owner restart also loses the live proof of outstanding work.
Resumed affected sessions remain ineligible until their owners can reconcile
that uncertainty; restoring a durable plan or receiving fresh user input does
not itself prove old processes stopped. This proposal adds no durable job
recovery queue. Known clean startup/new sessions use normal fresh accounting.

## 5. Persistent canonical state and console delivery

### 5.1 One plan, one structured snapshot

Add serializable/schema/TS derives to the native todo types and expose a
snapshot owned by `agent`:

```rust
pub struct TodoSnapshot {
    pub version: u64,              // monotonic per session, including rewind
    pub source: TodoSource,         // native | acp
    pub items: Vec<Todo>,
}
// AgentEvent::TodoChanged { session, snapshot }
// Loaded.todos: TodoSnapshot
// GetTodos { session } -> TodoSnapshot; RPC get_todos(session)
```

Use the generated bindings via `crates/rpc/src/export.rs` / `just ts`; do not
hand-copy new types into desktop `rpc/extra.ts`. Full snapshots are appropriate
for a capped native list and replace rather than patch previous UI state.
`TodoChanged` has no fabricated `TurnId`: clear/rewind can change a plan between
turns. It is part of the existing agent-owned event stream, automatically
forwarded by the RPC/link transport.

A changed list commits with the tool-result batch before a success event is
published. Multiple local todo mutations in one batch can publish its final
snapshot once. Apply operations against working state; a storage failure must
not advertise a successful new canonical list or permit a yield decision
using it. `get_todos` must work during a running turn without waiting for the
model: read the latest committed snapshot, not `Session.state` while it is
moved out. `Loaded.todos` is independent of transcript paging.

### 5.2 Persistence, cuts and compaction

Use the existing `sessions.sqlite`, not a file scraped from tool output.
Add a small `todo_snapshots` table with session, history anchor `at_seq`, an
ordered snapshot key, source and structured items; index by session and anchor.
Store the monotonic current projection version on `sessions`. This is one
history-aligned state table, not a second task store or event-sourcing framework.
Load plan, projection version and visible cut from one read transaction. The
version and selected list must describe the same committed state; two separate
reads could label an old list with a newer version and defeat reconciliation.

- **Mutation:** append the tool response and its resulting snapshot in one
  store transaction. Anchor at that committed response's seq. Update version
  in the same transaction, then publish `TodoChanged`.
- **Read:** latest snapshot under the current visible cut, ordered by
  `(at_seq, snapshot key)`. Empty if no snapshot exists. `SessionState::resume`
  loads this structured state, not a parsed summary.
- **Compaction:** normal summary/cut/snap retain the plan and still include its
  rendering in the harness head. No new snapshot for unchanged state.
- **Clear:** save an empty snapshot with the compaction record atomically,
  using its boundary anchor. The ordered key distinguishes clear from a todo
  update at the same seq. Publish empty state only after commit.
- **Undo/redo:** select the snapshot below `sessions.cut_at`, matching message
  visibility. Bump the projection version even when selecting an older list;
  do not resurrect an old transport version. Publish the resulting snapshot.
- **Commit cut:** delete hidden snapshots in the same transaction as hidden
  messages/epochs. No orphaned future plan after new work.
- **Fork:** copy snapshots only through the copied history boundary, including
  the mid-turn reply cut. A stripped, unexecuted tool call does not contribute
  a snapshot. The new session has its own version and no active work/candidate.
- **Delete:** delete snapshots with the session/subtree. An explicit new `init`
  replaces the visible plan; completing a plan does not auto-clear its card.

The ordered key is history order, not the UI version; undo/redo can move the
selected history backward while the projection version only moves forward.
Legacy sessions begin with no structured plan. Do not backfill by parsing
old text glyphs or replaying guessed tool operations; the model can initialize
one on a subsequent request. Document this migration limit explicitly.

### 5.3 External harness plans

Map ACP `SessionUpdate::Plan` to the same read-only snapshot and persistent
card, with `source: acp`. Map pending/in-progress/completed statuses directly;
retain upstream order and content. ACP owns its plan and may publish more than
32 rows, so the UI must scroll rather than silently truncate that source.
Persist updates at the current ACP history boundary and publish after commit.
Remove the duplicate transient plan-as-`TextDelta` path and replace its
implementation-pinning test with structured plan behavior coverage.

Do not send wth todo-tool instructions to an external harness or enforce wth
reminders inside its loop: it owns those semantics. The external child's
activity and result delivery still block the native parent's nudge. Parent and
child cards never merge lists or mark parent items done on child finish.

### 5.4 Snapshot/event consistency

Use the existing subscribe-then-snapshot pattern:

1. Establish the event subscription, then load the session/snapshot.
2. Replace todo state only with a higher version; equal versions are idempotent.
   A slow `load_session` response must not overwrite a newer `TodoChanged`.
3. On reconnect or RPC `Lagged`, mark current state stale, fetch an authoritative
   snapshot, and apply it with the same version rule. Never interpret a network
   error as an empty/completed plan. Hidden sessions refresh before display.
4. Scope all state, versions and expansion preferences by harness and session.
   Paging older transcript rows must not roll back the live plan.

The frontends are projections only. Neither expansion state nor a stale/missing
snapshot participates in continuation eligibility.

## 6. Persistent UI: desktop, mobile and TUI

### 6.1 Common behavior

The card is part of the session chrome, **not a transcript entry**. It remains
visible while the user reads any history position, the model runs or yields,
and results arrive. Tool cards remain the historical record.

Collapsed example (one compact row; labels, not color alone):

```text
> Todos · 2/5 done · 1 blocked      Now: Verify result delivery
```

Expanded example:

```text
v Todos · 2/5 done · 1 blocked
  [done]        Trace session lifecycle
  [done]        Compare reminder behavior
  [in progress] Verify result delivery
  [pending]     Check the three console surfaces
  [blocked]     Confirm external access
                Waiting for the account owner
```

- Collapsed by default on first view. User expansion choice survives incoming
  snapshots and session switching within that client, keyed by session/harness;
  no server preference or cross-device expansion sync.
- Updates never auto-expand, steal focus, or scroll the transcript to the end.
  The compact row updates counts and the in-progress task; if none, first
  pending task; if blocked-only, say `Waiting: N blocked` rather than spinning.
- Done and dropped are distinct. For a settled mixed list show `N done,
  M dropped`, not a misleading `100% done`. Keep a non-empty settled plan
  visible until explicitly cleared/replaced. Hide the card only for an
  authoritative empty snapshot; stale/unavailable state is labelled as such.
- Expanded rows show full task text, status and blocker reason; wrap long text.
  Give the list its own bounded scroll area. All rows are reachable without
  scrolling conversation history. Do not silently omit overflow rows.
- The card represents the selected session. Opening a child displays its own
  plan, labelled as an external plan when ACP-sourced. Parent plan stays intact
  on return. No new task-edit controls or clickable completion checkboxes.

### 6.2 Desktop and mobile app

Add one `components/chat/TodoCard.tsx` using the installed shadcn/Radix
`Collapsible` primitive. Mount it between `Transcript` and `Composer` in
`routes/c.$session.tsx`, in the non-scrolling flex column, with the same chat
column width/padding. Subscribe only to that session's todo snapshot, not to
streaming transcript text. Add `todos` and local disclosure state to the
existing harness-scoped session store and event reducer.

Desktop, iOS and relay browser use this **same component and RPC**. No mobile
fork, native bridge, new dependency or extra event subscription. Extend the
current session reload path and typed bindings.

- The disclosure trigger is a real button, keyboard-operable with Enter/Space,
  `aria-expanded` and an associated content region; visible focus ring.
- On mobile use a minimum 44 px tap target. Keep the collapsed header in view
  above the composer when the software keyboard opens; honor current
  `100dvh`/visual-viewport and safe-area behavior. No hover-only controls.
- Expanded content is bounded by available viewport height (about one third
  on a normal screen), shrinking before the composer or approval UI. Scroll
  inside the card, not behind it. Small landscape/keyboard layouts can reduce
  the body to a few rows while keeping its trigger and composer usable.
- Use a polite short status announcement for changed progress, not an assertive
  re-reading of the whole list. Preserve focus when item text/state changes.

### 6.3 TUI

Add a fixed todo disclosure beside the existing fixed agent board, above the
editor/status region in `tui/src/ui.rs`. It is not an `Entry::Card` and is not
controlled by transcript position. `Model` stores the latest typed snapshot,
disclosure state and independent list scroll position. `view_rows` subtracts
its actual rendered height so transcript anchoring remains correct.

Use **F2** to expand/collapse and focus the todo card; it is unassigned in the
current key dispatcher. Show `F2 todos` in its collapsed header/status help.
While focused, Up/Down and PageUp/PageDown scroll the todo body; Esc collapses
it and restores editor focus without cancelling the model. Tab returns focus
to the editor while leaving the card expanded. F2 works without clearing or
submitting the user's draft. Approvals, secret entry and `ask_user` keep their
existing higher-priority key handling; never consume their answers.

Bound the body to available terminal height, normally at most six rows, with
an overflow/position indicator and full wrapped text reachable by scrolling.
On very small terminals preserve the compact trigger and editor; modal/user
input takes precedence over the expanded body. Keep `Ctrl+O` for historical
cards. Keep mouse capture disabled so terminal selection/copy still works.
A `/todos` disclosure command can alias F2 through the existing command list
for terminals lacking function keys; it reads the same `get_todos` snapshot,
not another todo implementation.

Print mode has no persistent screen: retain ordinary textual output, with no
ANSI card injected into stdout. Its important requirement is that an accepted
same-turn reminder cannot be mistaken for final `TurnFinished`.

## 7. Integration and intentional boundaries

Implementation ownership, in dependency order:

| Area | Changes |
|---|---|
| `agent/src/todo.rs`, `frame.rs` | Blocked/actionable state, mutation invariants, static initial guidance, common rendering. |
| `agent/src/store.rs`, next agent migration | Structured snapshots/version, transaction with tool results, cut/clear/fork/delete semantics. |
| `agent/src/lib.rs`, `supervisor.rs` | `GetTodos`, `Loaded.todos`, `TodoChanged`; activity admission/ingress ownership and single yield decision. |
| `tools/src/{lib,registry,jobs,pty}.rs` | Tools-owned typed protocol and registration; real invocation/job/wait lifetimes; acknowledged completion instead of event-only model delivery. |
| Nested detached producers (`browser`, rendered `web`/`scrape`, and their existing call contexts) | Retain originating operation ownership across caller timeout/late completion; do not use resource session IDs as owners. |
| `agent/src/{loop_,repl,acp_child}.rs` | Same-turn checkpoint, stop classification, limits/provenance; input receipts; structured ACP plans. |
| `rpc/src/{api,export}.rs`, generated bindings | `get_todos`, extended snapshot/event shapes; no custom mobile protocol. |
| `desktop/ui/src/{store,routes,components/chat,rpc}` | Shared persistent disclosure, version reconciliation, reconnect/lag refresh. |
| `tui/src/{model,ui,lib}.rs` | Fixed card, snapshot loading/events, F2 and accessible fallback, height/scroll/focus rules. |

Do not ship the nudge with only partial activity coverage. The UI and persistence
can be implemented independently, but activation of continuation is gated on
the lifecycle acceptance checks below. This is a dependency, not permission to
call a running-set heuristic complete.

Keep public `TurnStarted/TurnFinished` semantics for real turns. A same-turn
reminder adds neither event. Therefore parent result routing, ACP prompt
completion, `wth -p`, schedule completion, and Signal/Slack turn flushing must
not observe an artificial finish. Existing jobs/children may still produce
later normal turns after a scheduling pause; this design does not turn print
mode or ACP into a forever-waiting session client. Do not retrofit the schedule
plugin's separate settle timer as quiescence authority.

Documentation amendments when implementing: 07 §9 loses its deliberate
no-blocking/no-persistence/no-live-card exclusions; 01/14/22 describe admitted
activity and acknowledged completion transfer; 19 documents the adjacent todo
card; 25/31 add it to console parity; 29 includes todo snapshots in cuts/forks.
Older as-built documents remain unchanged by this proposal, rather than being
rewritten to claim the feature already exists.

## 8. Verification and acceptance

These are required implementation checks, **not claimed passing tests**.
Use deterministic barriers at lifecycle handoffs, not sleeps as correctness
proof. Keep regression checks where a false nudge or stale plan is plausible.

### 8.0 Design-stage checks performed

Source inspection traced the current native yield, child admission/result,
job/PTY completion, detached invocation and console projection paths above.
An in-memory protocol model enumerated 840 orderings of execution finish,
result acceptance, the next natural yield, new work admission, cancellation
and two decision checks, respecting finish-before-accept-before-yield. It
checked that reminders never precede the result-driven yield, never follow
already admitted work/cancellation, and never duplicate a checkpoint. All
840 orderings passed. Local Markdown link targets were also checked.
An in-memory SQLite exercise also passed same-anchor clear ordering, visible
cut selection, fork-boundary copying and hidden-tail deletion; a projection
exercise confirmed newer undo state wins over delayed snapshots/events. These
are throwaway design experiments, not tests against the proposed migration.

This checks the proposed ownership/decision rules, not kameo scheduling,
production behavior, persistence or UI rendering. No runtime implementation
was changed, and no application/UI test is claimed by this document.

### 8.1 Runtime acceptance matrix

| Scenario | Expected observation |
|---|---|
| New multi-step request / explicit checklist | Initial prompt requests a complete plan; all enumerated items visible in typed state. Simple answer need not create one. |
| Natural yield, actionable plan, all owners settled | Exactly one appended todo note, next model call in the same turn, no intervening `TurnFinished`. |
| Empty/done/dropped/blocked-only plan | No note or extra model call; blocked reasons remain visible. |
| Running shell; ready/quiet PTY; child waiting on permit | No note, even if no deltas or public running events arrived. |
| Background result finishes | Normal result input wakes the model once. No reminder from the prior checkpoint; a new natural yield can be eligible. |
| Pause producer after terminal row but before completion delivery | No note throughout formatting, bus/mailbox, queued-input and accepted-input boundaries. |
| Inline/grace-boundary result | Output delivered exactly once; no second job wake or zero-owner gap. |
| Cancelled caller, detached/quiet tool still running | Later natural yield remains blocked until genuine settlement; synthetic tool finish cannot authorize it. |
| PTY wait timeout / cancel ack / late reader or reaper | No idle proof from the return alone. Owned tail stays active or unknown. |
| Child follow-up without a job row; idle child with active grandchild | Parent remains blocked; finished idle child with no work does not block forever. |
| Child result resolves a peer waiter | Waiting invocation owns the continuation; no simultaneous todo note. |
| Input/cancel/clear/rewind/new work admitted before decision commit | Stale candidate rejected; no reminder persisted. Inject these at the final check barrier. |
| Input admitted after committed reminder | Preserved in ingress order; no lost input or duplicate continuation. Cancel still stops provider execution. |
| Duplicate yield/settlement messages | At most one reminder/result for each identity; no negative counts or leaked ownership. |
| Transport failure/missing activity recipient/disconnected remote child | Fail closed and expose failure; zero visible rows must not authorize a nudge. |
| Todo-only/no-op response after a reminder | No repeated reminder without state progress. Three-attempt ceiling survives automatic wake turns. |
| Tool-round landing, exhausted salvage/retry, cancelled/error turn | No new allowance or reminder loop, including normal-looking landing text. |
| Other root session is busy | It does not suppress a genuinely quiescent session's candidate. |
| Browser/scrape timeout leaves a delegated command | Originating session blocked until actual completion, regardless of synthetic render-session key. |

A targeted concurrency smoke should exercise real shell/PTY completion and a
real child wake with an open todo. Capture model calls, notes and public turn
events. For `wth -p` and ACP, prove an accepted reminder happens before their
completion boundary. Test actor-mailbox liveness with concurrent ingress and
activity acknowledgements; a deadlocked gate is not safe completion.

### 8.2 Persistence and protocol checks

- Successful native mutation survives reconnect and daemon restart; failed
  mutation/store transaction publishes no new canonical state.
- Clear, undo/redo, fork-at-turn and fork-at-reply select the same plan as their
  visible history. Compaction does not lose blockers or introduce a second list.
- Revision N+1 event arriving before snapshot N never rolls the card backward;
  undo publishes a newer version containing older items. `Lagged` repairs from
  a snapshot, and transcript pagination never replaces the current plan.
- Structured ACP plan survives reopening that child; parent state is unchanged.
- Snapshot/event reads while a model call is running do not wait for that call.
- Restart restores display state but makes no model call until real input.

### 8.3 Actual UI checks

Verify the running shared app at desktop and mobile viewport sizes, and the
actual TUI in a PTY. Screenshots/terminal interaction are the acceptance proof,
not a component mounting without errors.

1. With the transcript scrolled far up, the collapsed card remains visible and
   current while todo updates stream. One click/tap/F2 reveals the list without
   moving the transcript.
2. A long list with long task text/blocker reasons is fully reachable within
   the card; editor and disclosure remain usable on a narrow/short viewport.
3. Collapse stays collapsed through progress, completion, reconnect and child
   results. Expansion is not reset by every event; sessions/harnesses do not
   leak each other's plan or preference.
4. Keyboard/VoiceOver disclosure, focus ring, mobile 44 px target, software
   keyboard and safe-area layouts work. No color-only status indication.
5. TUI F2/Esc/Tab and inner scrolling preserve drafts and transcript anchor;
   approval/secret/question controls keep priority; terminal text copying works.
6. Late attach, reconnect, lag and daemon restart recover the card without
   loading old transcript pages. Unavailable/stale is not displayed as empty.
7. A settled plan stays visible; blocked-only shows waiting without a false
   running spinner; an explicit clear removes it.

## 9. Rejected shortcuts

- **`TurnFinished && open_todos` in a frontend:** too late, frontend-dependent,
  misses work and races print/ACP/parent completion.
- **`ListJobs` + `ListAgents` == empty:** uncoordinated snapshots omit admission,
  quiet tools, queued children and undelivered wakes. Query errors are not empty.
- **Wait 100 ms / two idle checks / inactivity timer:** elapsed time proves
  neither ownership nor delivery. Slow formatting or a busy mailbox breaks it.
- **Exclude ready servers, watched jobs or blocked children:** violates the
  required no-running-work condition. Suppressed delivery is not stopped work.
- **Nudge when the last job finishes:** races its automatic result wake and
  reuses a stale yield. Reevaluate only after the model responds and yields.
- **A new turn for every reminder:** resets limits and exposes false completion.
- **Parse the last todo tool card for the persistent UI:** truncation, compaction,
  paging, failed tools, ACP plans and reconnect make it non-authoritative.
- **A new reminder scheduler, global bus drain, background broker or task DB:**
  unnecessary. Existing owners plus explicit admission/transfer are sufficient.

The feature is complete only when both contracts hold: the user can always
inspect the selected session's current plan without history hunting, and the
harness never adds a yield reminder while owned work or a normal wake is still
outstanding.
