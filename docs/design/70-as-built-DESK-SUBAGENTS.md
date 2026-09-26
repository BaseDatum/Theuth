# DESK-SUBAGENTS as built — a desk's subagents show in the sidebar

Branch `fix/desk-subagents-sidebar`, worktree `theuth-desksub`, base `b9f3264`. Commits `8118725` (fix), `9dd4b11` (this report), then the follow-up requirement (cap at 5 + Show more) as one more code commit and a doc amendment.

## Bug

"an agent spawned a subagent from the desk, and I have no way at all to see what it's doing. doesn't show up in the sidebar anywhere."

## Root cause (confirmed by reading and in the browser)

- `sidebar/SessionList.tsx` `buildRows`: chat rows are `sessions.filter((s) => s.parent === null && s.desk == null)` (UI-FIXES-3 item 1b). Subagents (`parent` set) only ever render under their top-level ancestor's row as `{ tree }` → `<AgentTree/>`, or as the collapsed row's running glyph (`subtrees` → `SessionSubagents`).
- `sidebar/AppSidebar.tsx` `Desks`/`DeskRow`: one flat row per roster agent from `useAgents()`; no sessions query, no children.
- So a subagent whose root is a desk session had no row anywhere: not in the chat list (its root is filtered), not under the desk (the desk row rendered nothing below it). Reproduced against the live engine: desk `stas` had two children (`dr-predrill-AH`, `dr-predrill-AH-2`) with no sidebar row.

## What changed

- **`lib/session-tree.ts`** (new): `rootOf(sessions, id)` and `subtrees(sessions)` moved verbatim out of `SessionList.tsx`; shared by `SessionList`, `Desks` and `AgentDetail`. `SessionList` imports them; nothing else about the chat list changed. Follow-up: `TREE_CAP = 5` and `cappedTree(rows, sessions, expanded, limit)` → `{ shown, hidden }` — running rows first, then newest by the session list's `updated_at` (a row the list has not refetched yet falls back to its own `started_at`); folded = first `limit`, expanded = all.
- **`sidebar/AppSidebar.tsx`**
  - `Desks({ active })` reads `list_sessions` through the same `queryKeys.sessions` query the chat list uses (one cache entry, no new RPC — `SessionSummary` already carries `parent` and `desk`), builds `subtrees(sessions)` once, and hands each `DeskRow` its subtree.
  - `DeskRow`: `isActive = id === active || subtree contains active` → `SidebarMenuButton isActive`. The desk link carries `SessionSubagents` (the same pulsing glyph a collapsed chat row shows while any descendant runs). Below the button: `<AgentTree session={id} active={active} liveOnly={!isActive} />` — a desk that is the open chat (or whose subagent is) shows its whole tree like an open chat row; any other desk shows only its live subagents, so a running child is visible without opening the desk. `id === null` (desk not yet opened) is unchanged: button → `desk_open`.
  - `AppSidebar` passes `session` to `<Desks active={session} />`.
- **`sidebar/AgentTree.tsx`**: `liveOnly?: boolean` prop filters rows with exported `live(state)` = `running | awaiting | approval` (idle, cancelled, error are finished). Row rendering, `list_agents` seeding and live event tracking are untouched, so a child row under a desk is the same `Link to /c/$session` as under a chat, with glyph, cap badge, cost and age. Follow-up: `cap?: readonly SessionSummary[]` (the desk's subtree, for `updated_at`) runs the rows through `cappedTree` with a `useState(false)` fold; below the fifth row a `Show N more` button in the row's own classes with `text-fg-tertiary`, `Show less` once expanded. Component state only — every sidebar mount starts folded. Chat trees (no `cap`) are uncapped as before.
- **`sidebar/SessionItem.tsx`**: `SessionSubagents` exported (was file-local).
- **`agents/AgentDetail.tsx` `Runs`**: `desks.includes(rootOf(all, s.id) ?? s.id)` — the tab now lists every session under a desk at any depth (was: desk + direct children only). Rows already had the running dot and a link, so this is the "current work links the running subagents" item as a one-liner.

## Tests

- `lib/session-tree.test.ts` (new): the `subtrees` roll-up test moved from `SessionList.test.ts`; a subagent whose ancestor is a desk rolls up to the desk at any depth and `rootOf` names the desk; `live()` states; `cappedTree` — seven rows (two running, one unknown to the session list) fold to `[r-new, r-old, f5, f4, f3]` with `hidden = 2`, expand to all seven with `hidden = 0`, and at/under the cap hide nothing.
- `SessionList.test.ts`: the desk-exclusion test also asserts that a desk's child and grandchild never become chat rows or a chat tree, even when the grandchild is the open session.

## Verification

- `just ts-desktop` → bindings regenerated (no Rust changes). `bunx tsc --noEmit` → 0 (after `bun install` in `packages/apps`, fresh worktree). `bunx vitest run` → 81 files, 492 tests green; 493 after the `cappedTree` test. `bunx vite build` → exit 0 (both rounds).
- Browser smoke, `vite --port 5179` against the running engine (`127.0.0.1:57903`), headless Chromium, nothing on the engine created or changed:
  - `/c/<stas desk>`: the `stas` row is highlighted and shows `✗ dr-predrill-AH 21m` and `○ dr-predrill-AH-2 ~$3.602 9m` beneath it — `/tmp/69/desksub-stas-open.png`.
  - Clicking `dr-predrill-AH-2` navigates to `/c/01a0dd48…`; the desk row has `data-active="true"` and the child row has the active classes; the chat list has no row for it — `/tmp/69/desksub-child-active.png`.
  - `/` (desk not open): both children are finished, so the desk shows no rows under it (`liveOnly`).
  - scrollMargin (MOBILE-FIXES): with the `stas` tree expanded, the menu `<ul>` measured 744 px and `SessionList`'s offset was 744 at `scrollTop` 500; the virtual rows were contiguous (no gaps between `li[data-index]` rects). The Desks group is inside the `SidebarMenu` sibling `watchListOffset` already observes, so expansion is covered without changes.
- No desk had a *running* child during the smoke; the live-only path is exercised by the `live()` test and by the observed hiding of finished children when the desk is not open.
- Show more / Show less smoke: `stas` has only two children, so `TREE_CAP` was set to 1 **in the working tree only** (reverted before commit, `grep` confirmed `= 5`) and the dev server restarted. `/c/<stas desk>`: one row `○ dr-predrill-AH-2` (the newer) + `Show 1 more` — `/tmp/69/desksub-show-more-folded.png`; click → both rows, newest first, `Show less` — `/tmp/69/desksub-show-more-expanded.png`; click → back to one row + `Show 1 more`. `SessionList`'s offset followed the menu height 744 → 772 → 744, so the virtualizer's scrollMargin tracks the fold too.

## Deviations / notes

- "Running ones first/always, finished ones per the chat rows' rules": implemented as live rows always, all rows while the desk (or a subagent of it) is the open chat. Order within the tree stays `list_agents` pre-order (the chat tree's order); no re-sorting.
- The desk row itself highlights when a subagent is active (as the brief asked); a chat row does not do this for its children — only the child highlights there. Left the chat rows alone.
- Each `DeskRow` with a session mounts an `AgentTree`, which calls `list_agents(desk)` once and ticks a 1 s clock for the age column, same as the open chat's tree; two or three desks, negligible.
- Not built: no Attention/HUD change — the Attention page has no "current work" slot; `AgentDetail › Runs` got the depth fix instead.
- Follow-up: the capped desk tree is ordered running-first / newest-`updated_at` as required, which supersedes the earlier "`list_agents` pre-order" note for desks; chat trees keep pre-order. Desk row order is stable while folded/unfolded (same sort, sliced). The tab's `ago` column still reads the row's own timestamps, not `updated_at`.
