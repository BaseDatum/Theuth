# 128e as-built — BOARD-PANEL (UI-only, design 128 Revision 6)

Branch `feat/128e-board-panel`, worktree `theuth-128e-board`, cut from main `a1cbe9fa`.
Design: `docs/design/128-hud-scene.md` → `## Revision 6 — BOARD panel (2026-10-07)`.

Sky, 2026-10-07 — "why isn't the hud outcomes showing the actual state of all the tasks in all projects, it just shows backlog
tasks, nothing on the grid is populated, and everything says 0 when in reality there are a lot of tasks in a lot of states."

## Cause (as found on main)

`OutcomesPanel` drew `outcomeMix()`: tasks that **moved today** (`updated_ms ≥ startOfDay(now)`) in done/review/blocked/cancelled
plus every `doing` task. `tasks_list` with the empty filter returns every task in every project (`crates/tasks/src/store.rs`
`list`), so the data was complete — the definition was not what the slot is for. Sky's board: 80 tasks (review 41, backlog 24,
blocked 10, cancelled 3, done 2) over four projects, nothing moved since 2026-10-06 18:34 → 0/0/0/0/0, "24 backlog · 0 todo", an
empty waffle.

## Per-file changes

### `crates/desktop/ui/src/components/hud/scene/model.ts`
- `Target` gains `{ route: 'tasks-project'; project: string | null }`.
- `OutcomeMix`, `MIX_ORDER`, `outcomeMix` removed (no consumer left — the footer's `N OUTCOMES TODAY` is `outcomes().total`,
  `boardPulse` in `hud/model.ts` counts on its own). Replaced by:
  - `BOARD_ORDER: { state, color, opacity? }[]` — backlog `C.tertiary` @ 0.55, todo `C.secondary`, doing `C.info`, review
    `C.primary`, blocked `C.accent`, done `C.success`, cancelled `C.tertiary`.
  - `Board`, `BoardState`, `BoardProject`; `boardSummary(tasks, now)` — `total`, `byState` in board order, `projects` largest
    first (ties by display name, `null` kept as `null`), `movedToday` (any state, `updated_ms ≥ midnight`).
  - `WAFFLE_CELLS = 60`; `waffle(counts, cells = 60)` — identity when `total ≤ cells`; else one cell per non-zero state and the
    remaining `cells − nonZero` shared over `n − 1` by largest remainder. Sums to exactly `min(total, cells)`; no non-zero state
    vanishes.
  - `projectName(name)` (`null` → `unfiled`, else `truncate(name, 14)`); `projectLine(projects, budget = 70)` → `{ shown, more }`.
- `SceneModel.board: Board`; `scene()` sets `board: boardSummary(tasks, now)` (`mix` is gone).

### `crates/desktop/ui/src/components/hud/scene/Scene.tsx`
- `linkFor`: `case 'tasks-project'` → `<Link to="/tasks" search={project === null ? {} : { project }} />`. `parseBoardSearch` /
  `Board.tsx` treat `project: ''` as all projects and have no "no project" filter, so `unfiled` opens the bare board.
- `OutcomesPanel({ mix, tasks })` → `BoardPanel({ board })`: title `BOARD`, meta `${projects.length} PROJECTS`, big = `total` →
  bare board. Waffle cells from `waffle(byState.map(n))` in board order, `fill` + `fillOpacity` per state, hollow
  `C.borderStrong` when empty. Legend: seven `To` rows at cy `49 + 14k` / y `52 + 14k` (49 → 133), word 10 px secondary, count
  right-aligned at 338, title `N state · open the board at state`. `${movedToday} moved today` at (20, 136), 9 px tertiary →
  bare board. The PROJECT line: one `<text x=20 y=148 fontSize=9 data-projects>` holding `linkFor(tasks-project, <><title/><tspan/></>)`
  per project with ` · ` tspans between and a ` +K` tspan when folded (the footer's `<a>`-in-`<text>` pattern).
- `Panels` renders `<BoardPanel board={m.board} />`; the `Task` import is gone; `Fragment` imported.

### `crates/desktop/ui/src/components/hud/scene/fixtures.ts`
- `task(…, progress_at_ms, project = null)`; 20 tasks over `hud` (9), `relay` (5), unfiled (6): backlog 4, todo 2, doing 3,
  review 2, blocked 1, done 7, cancelled 1 (12 moved today). The three `doing` tasks and today's `done` rows are unchanged, so
  the flows, footer and IN FLIGHT numbers the other tests pin are unchanged.

### `SceneCompact.tsx`, `Hud.tsx`, `queries.ts`, `styles.css`
- Unchanged (the compact stack reaches the panel through `Panels`).

### Tests
- `model.test.ts` — the `outcomeMix` test is replaced by `R6 — boardSummary` (fixture totals, state order + colours + backlog
  opacity, projects `hud 9 · unfiled 6 · relay 5`, `movedToday` 12, name ties, the empty board), `R6 — waffle` (identity, a
  single full column, Sky's 41/24/10/3/2 → `[18,0,0,30,8,2,2]`, 1000-vs-six keeps the six, `[7,7]` on 10 → `[5,5]`) and
  `R6 — projectLine` (fit, twelve projects fold within 70 chars, 14-char ellipsis, empty).
- `Scene.test.ts` — titles now `BOARD`; the new `Revision 6 — BOARD` test: 60 cells / 20 filled in order with the right fills and
  opacities / 40 hollow; the seven legend rows `[state, cy, y, n]`; `12 moved today` → `/tasks`; the project line's text,
  per-project `<title>`s and hrefs (`?project=hud`, bare for `unfiled`, `?project=relay`), no `+`; Sky's 80-task board rendered
  — `4 PROJECTS`, `80`, 60 filled cells over four colours, `0 moved today`, `DR failover 50 · unfiled 27 · sre-2379 2 · theuth 1`,
  `?project=DR+failover`; twelve projects → ` +K` within 70 characters.

## Deviations from the brief

- F/G asked to keep `outcomeMix()` "for the footer and chips". Grep showed neither uses it (footer: `outcomes().total`; chips:
  `boardPulse`), so after the cutover it would have been a dead export with a dead test; removed. Footer and chips untouched.
- E: the board cannot filter on "no project" (`''` = all), so `unfiled` links to the bare board, as the brief allowed.
- E: the `+K` tail is plain text, not a link (nothing to filter on).
- The ` · ` separators and `+K` sit inside one `<text>`; the per-project tooltips are `<title>` children of each SVG `<a>`.

## Gates (from `crates/desktop/ui`, one foreground command under `build-lock`, label `128e`, fresh bindings present after `TS-DONE`)

1. `bun run check` — 0 errors.
2. `bunx vitest run src/components/hud` — 4 files, **47 passed** (was 44 + the removed `outcomeMix` test; 3 model tests and 1 Scene test added).
3. `bunx vite build` — **built in 43.29 s** (the usual > 500 kB chunk warning). An earlier run caught the missing `tasks-project` member of `Target` and an unused lambda parameter; fixed before the commit.

Visual smoke: not run — no Tauri shell in this wave and `?sim=` needs it; the static render is asserted element by element in
`Scene.test.ts` instead (including Sky's real counts).

## Commits

- `5be77dce` model/Scene/fixtures/tests
- (this file + the Revision 6 section) — see `DONE BOARD-PANEL <sha>`.
