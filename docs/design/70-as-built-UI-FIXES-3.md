# UI-FIXES-3 as built — desk rows vs chat rows, board drop on release, unblock after "no continuation"

Branch `fix/ui-dogfood-round-3`, worktree `theuth-uifix3`, base `ae467a8`. Four fixes, three commits (items 1 and 4 share one), plus this report.

## Commits

### `0676d79` fix(tasks,ui): a user's unblock clears system blockers … (item 3)

- `crates/tasks/src/ledger.rs`
  - `pub fn system_blocker(b) -> bool`: `no continuation`, or any `prefix:` blocker that is not `decision:` (today `engine:*`, `person:*`). A bare task id and a `decision:*` are not system blockers.
  - `Ledger::move_to`: when `actor == "user"`, the task is `blocked` and the target is `todo | doing`, the system blockers are dropped before the guard runs and travel in the same `TaskPatch` as the state change. One place: the board's `task_move`, the Attention card's Unblock, the `task_move` chat tool all route through it. A refused move changes nothing.
  - `guard` `(Blocked, Doing|Todo)`: the refusal still lists the remaining blockers and adds "; a decision needs its answer" when one is a `decision:*`.
  - `update()`: `blocked → doing|todo` now wakes the owner `WakeReason::Resumed` (the `review → doing|todo` arm widened). The user's back-off reset (`no_progress_count = 0`, `next_wake_after_ms = None`) from UI-FIXES-2 item 5 already runs on every user change in `update()` and is reused, not duplicated.
  - Tests: `user_unblock_clears_system_blockers_and_wakes_resumed` (no-continuation + engine blocker → user `move_to(todo)` → `todo`, blockers empty, back-off reset, `resumed` wake to the owner, `blockers` event; an agent is still refused), `user_unblock_keeps_decision_and_task_blockers` (decision + no-continuation + task blocker → refusal names the decision and the task, not `no continuation`; nothing changes; no `resumed` wake).
- `crates/tasks/src/tools.rs`: `task_move` tool description says what a user's `blocked → todo` does.
- `crates/desktop/ui/src/components/tasks/helpers.ts`: `parseBlocker` — a colon-less blocker with a space (`no continuation`) is `other`, not a task id (it was rendered nowhere in TaskDetail). `actionsFor` gains `unblock: blocked && no decision:* blocker`.
- `crates/desktop/ui/src/components/tasks/TaskDetail.tsx`: Actions offers **Unblock — wake <owner>** → `task_move(todo)`.
- `helpers.test.ts`: the two new cases.
- Attention card: `roster::attention::derive` already emits a `Decision` item (not `Blocked`) whenever a `decision:*` blocker exists, so the card shows the decision and never an Unblock button in that case — no UI change needed.

### `5cad7c9` fix(ui): board card drop lands on release (item 2)

- `crates/desktop/ui/src/components/tasks/Board.tsx`
  - Root cause: `useCardDrag`'s handlers (incl. `draggable: false`) sit on the card `<li>`, but the `<Link>` inside is an `<a href>`, which is draggable by default. Press-and-move started a **native** drag of the link; WebKit then ends the pointer sequence without `pointerup` while the `<li>` keeps pointer capture, so the drag state stayed live and the *next* click's `pointerup` (still captured by the `<li>`) fired the drop. `useSessionDrag` never had this because its handlers are spread on the `<Link>` itself (its comment says so).
  - Fix: `draggable={false}` on the card's `<Link>` plus `onDragStart → preventDefault()` in the hook (covers any draggable descendant). `finish()` now keys on `hasPointerCapture` rather than the drag ref, so a drag cancelled elsewhere still releases capture and swallows the trailing click.
  - Escape: `Board` listens for `keydown Escape` while a drag is in flight and clears it; the following `pointerup` releases capture and drops nothing.
  - No reducer/hit-test was extracted (`columnAt` is a two-line `elementFromPoint` wrapper; vitest runs without a DOM), so no new test.

### `23c0e22` fix(roster,agent,rpc,ui,tui,slack): durable desk mark, Desks group, chat-list filter (items 1 and 4)

- **Durable desk mark on the session row** (the brief's "if the UI cannot tell today, add the field to the session summary"):
  - `crates/agent/migrations/0024_desk.sql`: `ALTER TABLE sessions ADD COLUMN desk TEXT`.
  - `crates/agent/src/store.rs`: `SessionRow.desk`, `session_cols!` selects it, `SessionSummary.desk`, `Store::set_desk(id, Option<&str>)`.
  - `crates/agent/src/lib.rs`: `SessionSummary.desk: Option<String>` (`serde(default)`, `ts(optional)`), new message `MarkDesk { session, agent: Option<String> }`.
  - `crates/agent/src/supervisor.rs`: `impl Message<MarkDesk>` → `store.set_desk` (mirrors `PinSession`).
- **Roster** (`crates/roster/src/lib.rs`):
  - `open_desk`: title is now `"<name> · desk"` (item 4), then `MarkDesk { agent: Some(name) }`, then pin as before. The mark stays on the row through `put_away` (archived past desks remain desks — the agent's Runs tab wants them and the chat list must never show them).
  - New message `OpenDesk(pub String)` → `Result<SessionId, String>` and `Roster::desk_or_open(name)`: an open desk returns its session; otherwise theuth's file or the agent's file, refused with a named gate (`no such agent`, `another engine holds the hub lease; desks open there`, `agent is paused`, `agent is a proposal awaiting approval`, `desk is closing; try again in a moment`), else `open_desk` and the new id (or `desk not opened (agent actor not running)`).
  - Test `desk_or_open_returns_the_open_desk_and_names_its_refusals`.
- **RPC** (`crates/rpc/src/api.rs`): `desk_open(name) -> SessionId` → `roster::OpenDesk`. `crates/rpc/src/forward.rs`: `classify` uses `row.desk.is_some()` instead of the `pinned && title starts with agent:` heuristic (its `ponytail:` note asked for exactly this); test builds the desk row with `set_desk`.
- **Desktop UI**:
  - `sidebar/AppSidebar.tsx` `Desks`: one row per roster agent with status not `proposed`/`invalid`, theuth first, id = `AgentSummary.desk` only — no pinned/title fallback (item 1c). `DeskRow` with a desk is a `Link` to `/c/$session`; without one it is a button → `api.desk_open(name)` → invalidates `['roster']` and `['sessions']` → navigates. Errors toast.
  - `sidebar/SessionList.tsx` `buildRows`: roots with `desk != null` are dropped before pinned/project/archived bucketing (item 1b) — their subtrees are unreachable without the root. Vitest `a desk session never appears in the chat list: pinned, archived, in a project or ungrouped`.
  - `agents/AgentDetail.tsx` `Runs`: desks are `s.desk === name || s.id === desk` (was the title heuristic).
  - `rpc/api.ts`: `desk_open` method + name list entry.
  - The chat header and any row label read `SessionSummary.title`, so an opened desk shows `theuth · desk` (item 4) from the roster's rename.
- **TUI / Slack** (one-line filters, disclosed): `crates/tui/src/model.rs` `session_rows` drops `desk` rows (their trees are only reachable through them); `crates/tui/src/search.rs` Recent drops them; `crates/slack/src/commands.rs` `/sessions` drops them.
- Struct-literal additions `desk: None` in `crates/tui/src/attach.rs`, `crates/tui/src/model.rs` (test fixture), `crates/agent/src/session_read.rs`, `crates/slack/src/ui.rs` (test fixture).

## Deviations

- `person:*` blockers are treated as system-owned and cleared by a user unblock: the brief names `no continuation` and "any other blocker the system wrote — not decision:*, not task blockers"; `person:` is written by an agent's `blocked` disposition and the user is the person in the room, so clearing it on a user unblock is the intended read. `engine:*` (placement missing) likewise.
- A refused user unblock (a decision or task blocker remains) changes nothing — the system blockers are not partially cleared. Conservative; the next unblock after the decision is answered clears them.
- `blocked → doing` by the user goes through the same path as `→ todo`; the Attention card and TaskDetail send `todo`.
- No UI-side heuristic remains for "is a desk" (title or pin). A desk opened by a build before this migration keeps its old `agent:<name>` title and has no `desk` mark until the roster reopens it (next episode close / hub start); until then it shows once as an ordinary chat row. `sync_desks` reopens wanted desks on start, and theuth's on its first wake, so the window is one episode.
- Board: no helper extracted for testing — the bug was an event-plumbing one, not logic.
- Desktop smoke not run: it needs `cargo build` of the binary, which the working rules exclude.

## Not built

- Nothing from the brief left out. The Attention card's "show the decision instead of Unblock" was already the behaviour of `attention::derive` and is unchanged.

## Tests written

- Rust: `tasks::ledger::tests::user_unblock_clears_system_blockers_and_wakes_resumed`, `tasks::ledger::tests::user_unblock_keeps_decision_and_task_blockers`, `roster::tests::desk_or_open_returns_the_open_desk_and_names_its_refusals`; `rpc::forward::tests::classify_user_vs_desk` adapted to the desk mark.
- Vitest: `SessionList.test.ts` desk exclusion; `helpers.test.ts` `parseBlocker('no continuation')` and `actionsFor(...).unblock` cases.

## Verification run

- `just ts-desktop` (bindings regenerated; `SessionSummary.ts` gains `desk?: string`).
- `bunx tsc --noEmit` → 0 errors (after `bun install` in `crates/desktop/ui` and `packages/apps`, both fresh in this worktree).
- `bunx vitest run` → 75 files, 476 tests, all green after the desk test was made independent of age headers (`h:beta:Older`).
- `bunx vite build` → exit 0. (`bun run build` = `tsc -b && vite build`; both halves ran.)
- `cargo check --workspace --tests` → `Finished dev profile` (10m17s), no errors, no new warnings.
- `cargo test -p tasks -p roster -p rpc --lib` → 40 + 30 + 48 passed, 0 failed.
- `cargo check -p tasks --tests` after commit 1 → clean.

DONE UI-FIXES-3
