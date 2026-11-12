# 67 — as built, worker U (desktop/mobile UI: Tasks board, Agents view, Mesh settings, TUI lists)

*Branch `feature/67-agents-ui`, 2026-09-24. Everything below builds against the frozen contract (`67-contract.md`); the engine still answers `not built`, so every screen renders the error as an empty state and can be reviewed with `?mock=1` (or `localStorage.theuth_mock67 = "1"`), which swaps in `src/components/tasks/mock.ts` — 8 tasks across two projects (`sre`, `launch`) plus one without, 3 agents, a two-peer mesh. Mock is never on by default.*

## Per commit

### 1 `feat(ui): Tasks board per project`
- Routes `src/routes/tasks.index.tsx` (`/tasks`, filters in the URL: `project`, `agent`, `engine`, `state`, `cancelled`) and `tasks.$id.tsx`. `routeTree.gen.ts` regenerated with `@tanstack/router-generator` (`bun -e` + `Generator`/`getConfig`, the same code the vite plugin runs) — not hand-edited.
- `components/tasks/Board.tsx`: header in the Notes page shape; a project `Select` reusing `useProjects`/`ProjectIcon` (the New chat palette's data and glyphs) with **All projects** as the last item behind a separator; agent (`agents_list` + "you"), engine (`mesh_status().peers`), state filters; a Cancelled switch; columns `todo · doing · review · blocked · done` (+ `cancelled` when toggled) as a horizontally scrolling, snap-aligned row that works on a phone. `TaskCard`: title, owner chip ("you" or the agent name), `→ reviewer`, `where`/`harness` chips, age, a red `blocked: <first blocker>` badge, a **waiting on N** badge whenever `blockers` is non-empty, and a green progress dot when `checkout` is set.
- `NewTaskSheet.tsx`: title, brief, project (`ProjectPicker`, defaulting to the board's), owner (`OwnerSelect`: you + agents), reviewer defaulting live to the owner's `reports_to` (`defaultReviewer`), `where` (datalist of peers), harness, external links, and a **Blocked by** picker (`TaskPicker.tsx`, a `CommandDialog` searching open tasks of the chosen project by title).
- `helpers.ts` (pure: owner text forms, `defaultReviewer`, `parseBlocker`, `needsPerson`, `awaitsMyReview`, `age`, `columns`, `actionsFor`) with `helpers.test.ts`; `queries.ts` (react-query hooks with the mock branch, `keys`, `useInvalidateTasks`); `mock.ts`.
- `AppSidebar.tsx`: **Tasks** nav entry.

### 2 `feat(ui): task detail — thread, sessions, events, actions`
- `TaskDetail.tsx` under the shared `Frame` (back to `/tasks` on phones): title with project glyph and state badge; brief as markdown (`Streamdown`); **Waiting on N** with task-id blockers as links to `/tasks/$id` and `decision:`/`person:` blockers as text; a **Blocks** list from an optional `blocks` field the engine may add to `task_get`; placement (`where`, `harness`, checkout engine/epoch/progress age) with the reason taken from the newest `placement`/`where` event; the thread document embedded through `useDocument({doc})` + `Live` (read-only follows the harness's status) with a comment box → `task_comment`; sessions (the checkout's plus every session an event names) opening in the chat pane via `/c/$session`, each with `session_cost` → `costShort`; a Cost line summing them or a placeholder; the `task_events` timeline (`age · actor · field · before → after`); a record block (id, created by, parent link, schedule, no-progress count).
- Actions by state: assign/reassign (`OwnerSelect` → `task_update{owner}`), send back with notes (`review→doing`), mark done (`review→done`, disabled unless you are the reviewer or the owner-as-user with no reviewer), move project, cancel (AlertDialog confirm, then back to the board). Errors toast the engine's message verbatim.
- `DocumentPage.tsx`: `Frame.backTo` union widened to `/tasks` (one line, outside the allow-list).

### 3 `feat(ui): Agents view and queue`
- Routes `agents.index.tsx`, `agents.$name.tsx`; `AppSidebar.tsx` **Agents** entry; `Frame.backTo` gains `/agents`.
- `components/agents/tree.ts` (`tree()` by `reports_to` rooted at the built-in `theuth`, unknown managers and cycles fall back to the root, siblings sorted; `budgetUsed`) with `tree.test.ts`.
- `AgentsView.tsx`: Explorer-style list + detail (tree hidden on phones once an agent is open; back chevron in the header); each node: status dot + text (idle/working/blocked/paused/proposed/unreachable), built-in badge for `theuth`, open-task count, budget bar (`spent_month / amount`, hidden for `budget: none`, amber ≥ 80 %, red at 100 %), a **desk** link opening the desk session.
- `AgentDetail.tsx`: `theuth` is a read-only card; every other agent is `agent_get` as a form (title, reports_to, model, effort, tools, mcp, budget type/amount/currency, paused, capabilities, charter body textarea) saved whole with `agent_put`; Pause/Resume via `agent_pause`; a proposed agent (status `proposed` or `proposed_by` set) shows its proposer and an **Approve** button → `agent_approve`.
- `tasks/Queue.tsx`, mounted at the top of both `/tasks` and `/agents`: `blocked` tasks whose blockers start with `decision:`/`person:`, `review` tasks where you are the reviewer, proposed agents — one 32 px row each, one click to the item; renders nothing when empty or while the engine says `not built`.

### 4 `feat(ui): Mesh settings; TUI /tasks and /agents`
- `components/settings/Mesh.tsx`, tab **Mesh** in `SettingsDialog.tsx` (`TABS`, `Pane`, `Share2` icon) and `slash.ts`'s `SettingsTab` union: this engine's id (copyable) and name (from `info().name`), hub preference `Select` → `mesh_set_hub`, current lease holder with reason and age, the peer list (reachable dot, name, hub marker, hub pref, last seen, first address) with remove → `mesh_remove`, **Pair another engine**: `mesh_pairing_string()` as text + `QRCodeSVG` (the Link pane's QR), and an input to paste one → `mesh_add`.
- TUI: `/tasks` is registered by `components/tasks.rs`; its owner-local effect calls `tasks_list` with the selected session's project and renders `tasks_table` (`id · state · owner · title`, heading names the project or `all projects`). `/roster` → `Command::ListRoster` → `UiEvent::Roster` → `agents_table` (`name · status · open`). Both in the `/help` list. Test `tasks_and_agents_slash_use_the_session_project_and_render_tables`.

## Deviations
1. **Blocked-by on create**: `TaskCreate` has no `blockers`; the sheet calls `task_create` then `task_update(id, {blockers})` when any were picked. Adding `blockers` to `TaskCreate` in the tasks crate would make it one call.
2. **Send back with notes** is `task_update{state: doing}` then `task_comment` when notes were typed (two RPCs); every other action is one.
3. **Sessions list** is derived (checkout session ∪ sessions named by events) — the record carries no session list.
4. **Placement reason**: the thread block is a 59 document I cannot parse client-side; the newest `placement`/`where` event's `after` is shown instead, under the where/harness chips.
5. **Mesh**: `Peer` carries no class or availability and `MeshStatus` no own name or hub preference; the pane shows what exists (name from `info()`), and the hub `Select` shows the last value set in this session, else `eligible`. Add `class`, `availability_30d`, `self_pref`/`self_name` to the contract and the columns are a few lines.
6. **`blocks`** is read as an optional field on `Task` (`TaskWithBlocks`) until the engine returns it.
7. **Agent file editor** is a form + charter textarea (as the brief lists), not CodeMirror; the knowledge `Editor` could be dropped in for a raw-file mode later.
8. **Default board project** with no `?project=`: the first known project label; "All projects" is the explicit extra click at the bottom of the select.
9. Reviewer default for an agent whose `reports_to` is `theuth` is **you** (the user closes theuth's line, decision 8).
10. Files touched outside the allow-list, all small and required by the wiring: `components/editor/DocumentPage.tsx` (`Frame.backTo` union), `components/settings/SettingsDialog.tsx` (tab), `components/chat/slash.ts` (`SettingsTab`), `crates/tui/src/lib.rs` (two dispatch arms, two print arms), `crates/tui/Cargo.toml` (`tasks`, `roster` deps — needed to name the reply types).
11. **TUI command is `/roster`, not `/agents`**: `/agents` already opens the subagent picker (`Command::ListAgents(session)`); the roster list is `/roster` → `Command::ListRoster` / `UiEvent::Roster`. The help line says so.
12. Commit 1 alone does not type-check: `routes/tasks.$id.tsx` imports `TaskDetail`, which lands in commit 2. The branch head does.
13. `bun install --frozen-lockfile` was run once in `crates/desktop/ui` (no `node_modules` in the worktree) so `tsc` could run; `just ts-desktop` once for bindings.

## Not built
- Drag between columns (not required). Journal and knowledge counts per agent (§12 — no RPC). Class/availability columns (no data). Signal/Slack anything.

## Tests written (not run)
- `src/components/tasks/helpers.test.ts` — owner text forms, default reviewer up the line, blocker parsing, queue predicates, mark-done gating, column grouping, age buckets.
- `src/components/agents/tree.test.ts` — nesting and sort, unknown-manager/cycle fallback, budget fraction.
- `crates/tui/src/model.rs::tasks_and_agents_slash_use_the_session_project_and_render_tables`.

## Verification run
- `bunx tsc --noEmit` (branch head): 60 errors, all pre-existing (`packages/apps/**`, `AppHost.tsx`, `@openuidev/*`); zero in files under `components/tasks`, `components/agents`, `components/settings/Mesh.tsx`, `routes/tasks*`, `routes/agents*`, `AppSidebar.tsx`, `DocumentPage.tsx`, `SettingsDialog.tsx`, `slash.ts`, `routeTree.gen.ts`.
- `cargo check -p tui --tests`: first run failed on name collisions (`UiEvent::Agents`, `Command::ListAgents` already exist for subagents); after renaming to `UiEvent::Roster` / `Command::ListRoster` a second run (one over the once-per-commit allowance, disclosed) finished `exit 0` with one warning — `/agents` unreachable because the subagent picker owns it — resolved by renaming the command to `/roster` (a match-arm literal change, not re-checked).

DONE U
