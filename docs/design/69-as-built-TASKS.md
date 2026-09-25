# 69 TASKS — the tasks board and the chat-first task flow (as built)

Branch `feat/tasks-backlog-and-chat-flow`. Three feature commits + this document.

## Sky's rulings (verbatim intent) and where each landed

| Ruling | Built |
|---|---|
| Board is "fully functional but primarily about observation; tasks are created interactively in chat" | `task_draft` tool + tasks prompt block (Part 3); board keeps every action but the sheet creates in `backlog` |
| A new task must not start executing on creation | `TaskState::Backlog` is the first state; `task_create` defaults to it (RPC, agent tool, sheet); no wake, no placement, not in `task_inbox` |
| A task sent to an agent must show as in progress | chips: `todo` + agent owner → "queued for ‹agent›'s desk"; `doing` → "at ‹agent›'s desk · open session" (detail chip links to the checkout session, names the node) |
| `where` is a dropdown of nodes, "theuth decides" default | sheet: `<select>` of this node + mesh peers (`MeshStatus.self_name`, `peers[]`), `''` → `where: null`; board filter says "Node" |
| External links field goes away | dropped from the sheet and TaskDetail; `Task.external` stays on the record, unused (see Deviations) |
| UI shows blocks AND blocked-by | `TaskChips` in the sheet and TaskDetail (`Relations`): both editable, chips open the other task |
| New project name creates the project | `ProjectCombobox` with an explicit "New project ‹name›" row; the task carries the label and `useBoardProjects` (sessions ∪ tasks) lists it everywhere on the board (see Deviations) |
| Agents draft tasks as cards, user approves, backlog or todo per intent | `task_draft` → OpenUI card set, three `@ToAssistant` buttons per card |
| theuth moves tasks on the user's behalf | `task_move` RPC + agent tool, transition table enforced, refusal lists the allowed states |
| Vocabulary node / mesh / device | UI text says node; record fields (`where`, `Checkout.engine`) unchanged |

## Part 1 — contract (commit `8b0c08b`, marker `/tmp/69/tasks-contract.done`)

`feat(tasks): TaskState::Backlog as the first state; task_create takes state (default backlog); task_move RPC + agent tool honouring the transition table`

- `crates/tasks/src/types.rs` — `TaskState::Backlog` (serde `backlog`, first variant, `as_str`); `TaskCreate.state: TaskState` with serde default `backlog` (`TaskCreate::default_state`). TS binding: `TaskCreate.state` is a required key (ts_rs does not make serde-default keys optional; every UI/RPC caller passes it, JSON callers may omit it).
- `crates/tasks/src/store.rs` — `state_of("backlog")`. **Migration: none.** The `tasks.state` column is free text without a CHECK; existing rows stay `todo`.
- `crates/tasks/src/ledger.rs`
  - `create`: refuses any state but `backlog | todo`; wakes the owner only for `todo` (backlog: no wake → no dispatch → no placement; `task_inbox`'s default filter and the roster's `open_tasks` filter never included it).
  - `guard` (the 67 §7.2 transition table): `(Backlog, Todo) | (Todo, Backlog) → Ok`; `(Backlog, _) → Err("a backlog task is started first (todo), then checked out")`; `(Review, Todo)` joins `(Review, Doing)` as the reviewer's send-back. `checkout` refuses backlog ("task is backlog") through the existing state match.
  - `update`: `backlog → todo` wakes the owner `assigned`; `review → todo` wakes `comment` like `review → doing`.
  - `allowed_moves(t, actor)` (every target the guard accepts now) and `move_to(id, to, note, actor, session)`: refuses with `"<from> → <to> is not allowed (<guard reason>); from <from> you may move to: a, b"`, else `update` + the note as a thread comment.
- `crates/tasks/src/lib.rs` — `MoveTask { id, state, note, actor, session }` actor message → `ledger.move_to`.
- `crates/tasks/src/tools.rs` — `task_create` gains `state: Option<TaskState>` (default backlog, `todo` only when the user asked to start now; other states refused by `create_state`); output line reports the state. New `task_move { task, state, note? }` tool (`TaskMoveTool`, Mutating), registered with the others.
- `crates/rpc/src/api.rs` — `task_move(id, state, note?) → Task` (actor `user`).
- `task_checkout` verified: `Ledger::checkout` sets `Doing` (line ~644) and logs a `state` event. The UI was reflecting `doing` only as a raw state badge and a green dot; the "in progress" wording was missing — fixed in Part 2's chips.
- One-line struct-literal changes outside the tasks crate (disclosed): `crates/roster/src/lib.rs` (budget task), `crates/roster/src/proposals.rs` (hire task), `crates/signal/src/channel.rs`, `crates/slack/src/channel.rs` — all `state: TaskState::Todo` (they dispatch to an agent at once, as before). `create_for_schedule` likewise.
- `just ts-desktop` regenerated `crates/desktop/ui/src/rpc/bindings/` (`TaskState`, `TaskCreate`, `methods.json` with `task_move`).

## Part 2 — board UI (commit `d5eae1f`)

`feat(ui/tasks): Backlog column first; Start + drag backlog↔todo via task_move; New task sheet: node select, blocks/blocked-by chips, project combobox with New project row, no external links; state chips per Sky's rulings`

- `helpers.ts` — `COLUMNS = [backlog, todo, doing, review, blocked, done]`; `DRAG_STATES`/`canDrag` (backlog ↔ todo only); `stateChip(t)`; `nodesOf(mesh)` (this node first, peers after, self de-duplicated, `reachable` carried); `actionsFor` gains `start` (backlog) and `toBacklog` (todo).
- `queries.ts` — `useBoardProjects()` (sessions' labels ∪ tasks' labels), `useMoveTask()` (`task_move` mutation; refusal toasts verbatim; invalidates lists + events).
- `rpc/api.ts` — `task_move(id, state, note)` + `METHODS` entry.
- `Board.tsx` — Backlog column first; native HTML5 drag (`draggable` card, `onDragOver/onDrop` on the column, ring on the droppable column) between Backlog and Todo → `task_move`; Start button on backlog cards; state chip under the title; Node filter lists this node + peers.
- `NewTaskSheet.tsx` — rewritten: no External links (hint "put links here" on the brief), Where = node `<select>` ("theuth decides" default, unreachable peers disabled), Blocked by + Blocks chips (`TaskChips`), `ProjectCombobox` (Popover + cmdk, "New project ‹name›" row when the typed name matches nothing), submit "Create in backlog" → `task_create({ state: 'backlog', … })`, then `task_update` for this task's blockers and for each blocked task's `blockers` (+ this id).
- `TaskChips.tsx` (new) — shared editable chips; `TaskPicker.tsx` takes a `title` and lists backlog tasks too.
- `TaskDetail.tsx` — `StateBadge` (doing → link to `/c/$session`, node name appended); `Relations` (blocked-by / blocks side by side, editable, non-task blockers as text); External section removed; the `TaskWithBlocks` shim removed (`Task.blocks` is in the binding); Actions: Start / Back to backlog / Send back (note as comment) / Mark done / Cancel all via `task_move`; field patches stay on `task_update`.
- `mock.ts` — a backlog fixture; `helpers.test.ts` — see Tests.

## Part 3 — chat-first flow (commit `f2056ac`)

`feat(app,tasks): task_draft tool renders approval cards (Add to backlog / Start now / Discard via @ToAssistant); tasks prompt block steers draft-then-approve and task_move; desk manual leaves backlog to the user`

- `crates/app/src/draft.rs` (new) — `task_draft { tasks: [{title, brief, owner?, project?, where?, blocks?, blocked_by?}] }` (`TaskDraftTool`, ReadOnly). Builds OpenUI source (`source(tasks, suffix)`): `root = Stack([intro, c1…])`, one `Card` per draft with `CardHeader(title, "draft <id>")`, the brief, a meta line (owner · project · where · blocked by · blocks) and `Buttons([Add to backlog, Start now, Discard])`, each `@ToAssistant("draft <id>: add to backlog | start now | discard")`. Draft id = `d<n>-<last 4 alnum of call_id>`; lives only in the card. Commits through the `app` tool's `commit_source` under the tool call's id (`Origin::Tool`, app name `task-drafts`, a new version per call) so the card renders inline and in the artifact panel; the receipt tells the model to stop and wait, and what to do per choice. Strings are JSON literals (the OpenUI tokenizer parses `"…"` as JSON).
- `crates/app/src/lib.rs` — module + registration next to `AppTool`.
- `crates/desktop/ui/src/components/chat/MessageTool.tsx` — `task_draft` results render as `ArtifactCard` like `show`/`app`. The press reaches chat through the existing `AppHost.onAction` → `assistantInput` → `send_input`/`steer` path (`[app task-drafts v<n> · <record_call_id> · action …]\ndraft d1-…: start now`).
- `crates/tasks/src/tools.rs` — `PROMPT` registered as `RegisterPrompt { plugin: "tasks" }` from `TasksPlugin::start`: draft with `task_draft` and wait; `task_create` `backlog`/`todo` per choice; `task_move` when asked to start / pull back / drop / approve / send back. `task_create`'s description points at `task_draft` for chat-described work.
- `crates/roster/src/desk.rs` — desk manual: backlog tasks are not in the inbox and the desk does not `task_move` them to todo; the user starts them.
- Tool sets: there is no named default/desk tool list — every session sees every registry tool (capped by risk via `SetCap`), so `task_move` (Mutating) and `task_draft` (ReadOnly) are available to theuth and to desks under the same caps as `task_create`/`task_update`.

## Tests written

- `crates/tasks/src/types.rs` — `create_state_defaults_to_backlog_on_the_wire` (JSON without `state` → `Backlog`; `Backlog` serialises as `"backlog"`); `where_is_renamed_on_the_wire` updated.
- `crates/tasks/src/ledger.rs` — `backlog_is_not_dispatched_and_moves_by_intent`: create in backlog wakes nobody; create refuses other states; backlog → doing/review refused, checkout refused; `move_to` refusal names from/to, the guard reason and the allowed list (no `doing`); backlog → todo wakes `assigned` and the note lands in the thread; todo → backlog; review → todo is the reviewer's; `allowed_moves` from todo includes backlog. `transition_table` untouched and still green.
- `crates/tasks/src/tools.rs` — `task_create_defaults_to_backlog_and_refuses_other_states`.
- `crates/app/src/draft.rs` — `card_buttons_carry_the_draft_id_and_choice` (ids, root, every button's `@ToAssistant` message per card, header, JSON escaping of quotes/newlines, meta line, empty brief, one statement per line).
- `crates/desktop/ui/src/components/tasks/helpers.test.ts` — actions incl. `start`/`toBacklog`; columns start with backlog; `stateChip` text for backlog/todo/doing per owner kind; `canDrag`; `nodesOf`.

## Verification (run)

- `cargo check -p tasks -p rpc -p roster -p signal -p slack --tests` → exit 0 (Part 1).
- `just ts-desktop` → `export_bindings` ok, bindings copied (Part 1).
- `cargo test -p tasks -p app --lib` → tasks 39 passed, app 36 passed, 0 failed (Part 3).
- `crates/desktop/ui`: `bun run check` (tsc -b) → 0; `bun run test` (vitest) → all files passed; `bun run build` → exit 0 (Part 2; re-run `check` after Part 3's MessageTool change → 0). Both `crates/desktop/ui` and `packages/apps` need `bun install` in a fresh worktree (the `@openuidev` type errors otherwise are the missing `packages/apps/node_modules`, as 69 UI noted).
- `cargo test -p agent --lib` not run: no file in `crates/agent` changed (the task tools live in `crates/tasks`, the draft tool in `crates/app`); `monty` not built.
- `cargo check --workspace --tests` → exit 0 (the first attempt failed in `wavekat-vad`'s build script downloading its ONNX model — network, unrelated; the retry was clean).

## Deviations

- **Project creation.** There is no "create project" RPC: the `projects` table is the style table (`set_project_style(name, null, null)` deletes the row) and a project exists when a session or task carries the label (51 §3). "New project ‹name›" therefore creates the project by creating the task under that label; `useBoardProjects()` lists tasks' labels alongside sessions', so the board's project palette and both pickers show it at once. No style row is written (that would invent an icon/colour). The chat `ProjectPicker` (sessions only) is unchanged and is still what TaskDetail's "Move" uses.
- **`Task.external` / `TaskCreate.external` / `TaskPatch.external` stay in the contract** (Rust and TS), as the brief said; the sheet sends `[]`, TaskDetail no longer shows the section. The `task_update` tool still accepts `external`.
- **`task_move` refusal text** also carries the guard's reason in parentheses (`review → todo is not allowed (send back is the reviewer's (agent:lead)); from review you may move to: cancelled`) — the allowed list alone hid why a listed-looking move failed for this actor.
- **Send back / Mark done / Cancel in TaskDetail now go through `task_move`** instead of `task_update` + `task_comment`, so the user sees the same allowed-list refusal the agent would; field edits (owner, project, blockers) stay on `task_update`.
- **`task_draft` lives in `crates/app`**, not `crates/tasks`: it needs the app crate's private `commit_source` (validation + artifact commit + `ArtifactShown` joined to the tool call) to render inline; a public bus message for that would have been god-wiring for one caller. It touches no task state.
- **Drag/drop is native HTML5** (`draggable`, `onDragOver`, `onDrop`); no DnD library is installed and only one pair of columns is draggable.
- **Draft ids are `d<n>-<4 chars of the call id>`**, not uuids: short enough for the model to echo, unique per card set, and the approval message's `[app … record_call_id]` header disambiguates across sets.
- The tasks prompt block is a `RegisterPrompt` from the tasks plugin (like `app`'s), not an edit to `crates/agent/src/frame.rs`: plugins own their prompt paragraphs.

## Not built / deferred

- No visual browser check of the board (engine gate; see Verification).
- No `task_draft` end-to-end run (needs a live engine, model and the OpenUI helper); the card source is unit-tested and goes through the same validated commit path as `app`.
- `Checkout.engine` → node display name mapping via mesh peers: the chip shows `Checkout.engine` as stored (the mesh peer `name` is the same string theuth writes into `where`/`engine`), so no extra lookup was added.
- Project style row on "New project" (icon/colour) — the user can style it from the sidebar as today.

DONE TASKS
