# 128 as-built — HUD-SCENE (UI-only wave)

Branch `feat/128-hud-scene`, worktree `theuth-128-hud`, cut from main `e04f1d1a`. Brief: `theuth-128-briefs/brief-HUD-SCENE.md`.

Two sessions built this. Worker 1 finished brief step 1 (`5abf725f`) and had Scene, SceneCompact, Scene.test, the Hud wiring
and the keyframes written when its ACP session was cut off at 10:56 while starting the vitest gate; the orchestrator checkpointed
that tree as `197d0700`. Worker 2 (this document) resumed from those two commits, audited them against brief §1–§5, ran the three
gates, and wrote this file. No source line changed between `197d0700` and the first gate run — the first worker's tree was complete.

R1 (orchestrator review after screenshots at `/?sim=sandbox-ok` and against the live engine, 11:37) asked for four corrections, folded
in as `6fe0cfae`: (1) the roster agent whose `file.name` is `theuth` folds into the sessions ring — one ring, sub-label
`N chats · M open · age`, live if a chat moved in 2 min or that agent is `working`, linked to `/agents/theuth`, and `agent:theuth`
activity routes to it (`ringActor` in `ringByActor`/`ringDots`); (2) `footer.needYou` is `null` while `attention_list` is pending —
the table count and the NEED YOU segment show `—`; loaded-and-empty shows `Nothing needs you` and `00 NEED YOU` in `--fg-tertiary`;
(3) roster sub-labels read `<open_tasks> open · <desk age>` (`<open_tasks> open` without a desk; `paused`/`unreachable`/`invalid`
keep their word); (4) `SceneSwitch` (SceneCompact.tsx) owns the `useIsMobile()` pick so Hud.tsx no longer branches, and the render
test mocks `@/hooks/use-mobile` to prove the < md branch (five 352×248 panels, NEEDS YOU list, no `data-flow`, no arc band, no
1920×1080 SVG) and the ≥ md branch.

R2 (Sky's ruling, relayed 11:44, changes brief §4): "all of that old stuff below the new hud needs to go away, it should all be
folded in and the things that are reachable from it are reachable from the new interface." Folded in as `14e990b4` (+ the test
fix in the as-built commit):
- `Hud.tsx` renders only the `Overview` header row and `SceneSwitch`. Deleted from `/`: the Attention cards, In flight, Board
  pulse, Activity and Spend sections, the mesh chip, the docked new-chat `Composer`, and the `InFlightList`/`ActivityRow`/
  `SpendList` helpers; `useMesh` and the `boardPulse`/`inFlight`/`runningRoutines`/`spend` imports went with them (the functions
  stay in `hud/model.ts`, tested). Kept and exported: `Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList`, `KindActions`
  (components/attention/* import them; `git grep` found no other importer).
- Reachability, desktop and compact: needs-you dots/labels/table rows → `/attention/$id`; NEEDS YOU header and the NEED YOU
  segment → `/attention`; roster rings → `/agents/$name`; the theuth aggregate ring → `/new`; routine rings and arc marks →
  `/routines/$name`; running flows → `/tasks/$id`; outcome flows → `/tasks/$id` or `/routines/$name`; OUTCOMES legend rows →
  `/tasks?state=done|doing|review|blocked|cancelled` (the Board-pulse chips' `{ state, project: '' }` search), a dim
  `<N> backlog · <N> todo` line (from `tasks_list`, not today-filtered) → `/tasks?state=backlog|todo`, the big number → `/tasks`
  (bare: the route recalls this device's filters); SPEND rows → `/agents/$name`, big number → `/agents`; AGENTS rows → the ring's
  link, big number → `/agents`; footer: SESSIONS LIVE → `/new`, AGENTS WORKING → `/agents`, NEED YOU → `/attention`, THIS MONTH →
  `/agents`, OUTCOMES TODAY → `/tasks?state=done`, IN FLIGHT → `/tasks?state=doing` (the right-aligned slot is one `<text>` with
  two linked `<tspan>`s). One `linkFor(target, children, className?, style?)` in Scene.tsx renders every `Target` — SVG `<a>`
  inside the picture, HTML `<a>` in the band and the compact stack; `Target` gained `{ route: 'tasks', state: TaskState | null }`
  and `{ route: 'new' }` (replacing `tasks-doing`).
- STREAM holds all 50 rows (`useActivity`, `ACTIVITY_CAP`, live-prepended by `appendActivity` on `ActivityAppended`), newest
  first, scrolling inside the panel frame; each row links to its subject via `streamLink(entry)` — task/child/escalation kinds →
  `/tasks/$id`, agent/budget kinds → `/agents/$name`, `routine_fired` → `/routines/$name`, else no link. Mechanics: the SVG field
  is now `viewBox="0 0 1920 812"` (the mockup's top 812 px; `FIELD_H`) and the bottom band is an HTML `grid-cols-5` with the
  mockup's margins/gaps as fractions of 1920 (`px-[1.25%] gap-[1.458%]`); four cells are `PanelSvg` (352×248, scaled to the
  cell) and the fifth is the HTML `StreamPanel`, sized in container-query units (`u(px) = px/352·100cqw`, `@container` on the
  box) so its 9 px mono text scales exactly like the SVG panels beside it. Desktop locks `aspect-[352/248]`; SceneCompact caps
  it at `max-h-[40vh]`. The canvas hairline field shrank to 1920×812 with the SVG.
- Tests: Scene.test asserts every destination above by href (the `Link` mock now appends `search` as a query string), the
  `<section aria-label="Stream">` with one `<li>` per feed row and a task row's href, `viewBox="0 0 1920 812"`, four 352×248
  SVGs; Hud.test renders `Hud` (sidebar/breakpoint/hotkeys mocked) and asserts the header + scene and no
  `section[aria-label="Attention"|"In flight"|"Board pulse"|"Activity"|"Spend"]`, no textarea (composer), `— NEED YOU` while
  loading; model.test covers `streamLink` and the theuth ring's `/new` link.

## Per-file

| File | Change |
|---|---|
| `components/hud/scene/model.ts` (new) | Pure scene model in the mockup's 1920-wide coordinates: `C` token table (CSS `var(...)` strings only), `W`/`H`/`FIELD_H` (812 — the SVG field's height after R2), `startOfDay`/`hourOf`/`nowHour`, `rng` (FNV-1a → mulberry32), `theta`/`arcPoint`/`arc` (band, 25 ticks, 12 labels, routine squares, NOW marker), `rings` (theuth + folded roster theuth · agents ranked working → waiting → idle → lapsed → error · running routines · `+N more` fold at 9), `shortId`, `needsYou`, `needFlows`, `outcomes`, `running`, `flowPath`, `layoutEnds`, `hairlines`, `sessionsToday`, `punchCard`, `outcomeMix` + `MIX_ORDER` (with each row's board `state`), `footer` (`needYou: number \| null`), `ringDots`, `streamColor`, `streamLink`, `actorLabel`, `Target` (attention/task/agent/routine/session/agents/tasks{state}/attention-list/new) and `scene(data, now)` composing the whole `SceneModel`. Missing queries draw nothing; the chrome stays. |
| `components/hud/scene/model.test.ts` (new) | 12 vitest cases against the fixture at a fixed local 19:49:19: arc geometry (00:00 tick at (1031.2, 88.1), band path, 12:00 label rotation, filled/hollow routine marks, NOW position), `hourOf` clamping, ring order/styles/geometry/fold, needsYou order/jitter/determinism/≤ 10-char ids, outcomes kinds/order/via/cost/links, running labels, `flowPath` segments, `layoutEnds` (≥ 22 px per column, x cycling, overflow counted in `more`), hairlines (determinism, skips drawn flows, one per session, 600 cap), punchCard (Mon = 0, now cell), outcomeMix, footer numbers and the empty-data chrome. |
| `components/hud/scene/fixtures.ts` (new) | `sceneFixture(now)` reproduces the mockup: 6 needs-you items, 7 rings (theuth, 5 agents incl. a paused one, 1 running routine), 8 done + 3 running, ~230 seeded field lines, a week of punch-card rows, spend rows with one agent over 85 %, 50 stream rows. `T(n)` builds the synthetic task ids the tests assert hrefs against. |
| `components/hud/scene/Scene.tsx` (new) | Desktop scene. Field: `<canvas>` hairlines under an `<svg viewBox="0 0 1920 812" role="img">` (aspect locked, min-height 406). `useClock` ticks each second and pauses on `visibilitychange`; the model is recomputed on the minute so the NOW marker moves per minute. `useField` draws the hairlines at most once a second, colour read from `getComputedStyle(documentElement)` on mount and on every `<html class>` mutation. `linkFor(target, children, className?, style?)` renders every `Target` as a typed TanStack `Link`; `To` wraps an SVG group in it with a `<title>` tooltip. Top bar, corner marks, arc band/ticks/labels/marks/NOW, coloured flows (`data-flow` + `hud-flow-in`), needs-you dots + labels, rings (`RingGlyph`, live disc `hud-pulse`), NEEDS YOU table (header → /attention, 6 rows + `+N`, "Nothing needs you" once loaded empty), footer rule + six linked segments (the right slot is one `<text>` with two linked `<tspan>`s). Band: an HTML `grid-cols-5` under the field — `Panels` renders `PanelSvg`(352×248) cells for `AgentsPanel`, `ArrivalsPanel`, `OutcomesPanel` (legend rows → `/tasks?state=…`, `N backlog · N todo` line, big number → `/tasks`), `SpendPanel` (rows → agent, big number → `/agents`), and the HTML `StreamPanel` (all 50 rows, scrolling, rows linked by `streamLink`, container-query-unit sizing). Error note under the band in `--status-danger-fg`. |
| `components/hud/scene/SceneCompact.tsx` (new) | `SceneSwitch` (Scene ≥ md, SceneCompact below, via `useIsMobile`). SceneCompact: footer pills (all six linked), NEEDS YOU header → /attention and rows → /attention/$id, rings as a list (glyph + name + sub, → the ring's link), then the shared `Panels` stacked at container width with the STREAM box capped at `max-h-[40vh]`. |
| `components/hud/scene/Scene.test.ts` (new) | 3 render tests (`renderToStaticMarkup`, `Link` mocked to `<a href>` per the Hud.test pattern, `@/hooks/use-mobile` mocked to a flippable value): fixture counts (7 rings r=13, 6 need dots r=7, one `data-flow` per laid-out flow, `hud-pulse` only on live rings, no inline `animation`, no literal `#` colour), hrefs for attention/task/agent/routine/footer, text (clock, footer numbers, panel titles), the carnelian spend track, `role="img"` + `aria-labelledby`; loading → `— NEED YOU` and no "Nothing needs you", loaded empty → both in `--fg-tertiary`, error note names the query; SceneCompact pills/rows/rings/5 panels and `SceneSwitch` under/over md. |
| `components/hud/queries.ts` | Added `ACTIVITY_TODAY_KEY`/`ACTIVITY_WEEK_KEY`, `useActivityToday()` (since local midnight, limit 500, refetch 60 s), `useActivityWeek()` (since now − 7 d, limit 500, refetch 5 min) — both prepend `ActivityAppended` into their cache like `useActivity`; `useSessionList()` on the sidebar's `queryKeys.sessions` key so the HUD shares that fetch. All three return the fixture under the existing `simFlag()` (same gate as `ATTENTION_SIM`). |
| `components/hud/Hud.tsx` | R2: renders the `Overview` header row (sidebar toggle) and `<SceneSwitch>`, nothing else. Gone: the four `Metric` cards, the Attention/In flight/Board pulse/Activity/Spend sections, the mesh chip, the docked `Composer`, `InFlightList`/`ActivityRow`/`SpendList`, and their imports (`useMesh`, icons, `boardPulse`/`inFlight`/`runningRoutines`/`spend`/`headline`). Scene data is memoised from `useAttention`/`useTasks`/`useAgents`/`useRoutines`/`useActivity` plus the three new hooks; under `?sim=` the attention/agents/tasks/routines/activity sources come from `sceneFixture`. Query errors are collected into the one-line note. Exports kept for components/attention/*: `Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList` (+ its `AttentionCard`/`EmptyNote`), `KindActions`. |
| `styles.css` | `.hud-pulse` (scale 1 → 1.25, 1.6 s, ease-in-out, infinite, `transform-box: fill-box`), `.hud-flow-in` (600 ms fade), `.hud-scene:has([data-flow]:hover) [data-flow] { opacity: .5 }` + hovered flow at 1, both keyframes, and the `@media (prefers-reduced-motion: reduce)` block setting `animation: none` — the file's existing pattern. |

## Deviations from the brief and why

- `hairlines(activityToday, sessionsToday, rings, drawnIds, now)` takes `rings` (the brief's signature omitted them, its text needs them for the 62 % ring sources) and `now` instead of a separate `seed` — the seed is each entry's own id, which is what makes the picture stable.
- `outcomes(activityToday, routines, rings, now)` takes `routines`, not `tasks`: the error check is on `ScheduleInfo.last_result`; task ids are recognised from `subject` by shape (`looksLikeTask`).
- `needsYou(attention)` has no `now` argument — the dot positions don't depend on time; `needFlows(needs, rings, now)` carries the time-dependent part (arc hour, age label).
- The `+N more` ring uses the `lapsed` style (dashed, `--fg-tertiary`) because the brief asks for "idle, dashed" and the idle style is a solid hollow ring; dashed + dim is the closest token-only rendering and links to `/agents`.
- (superseded by R2) `N SESSIONS LIVE` had no link; now → `/new`. The `theuth` ring linked to `/agents/theuth` after R1; R2 made it `/new` unconditionally.
- (R2) SceneCompact's panels follow the desktop order AGENTS, ARRIVALS, OUTCOMES, SPEND, STREAM via the shared `Panels`, not §3's AGENTS, OUTCOMES, SPEND, ARRIVALS, STREAM — one component for both hosts; reorder with a prop if the phone order matters.
- (R2) `Panel`'s big dim number is a link where the ruling names one (AGENTS/SPEND → `/agents`, OUTCOMES → `/tasks`); ARRIVALS' clock is not.
- (R2) The STREAM panel is HTML, not SVG — the ruling allowed either; a scrollable list is native in HTML and impossible in plain SVG without a foreignObject.
- The hover rule uses `:has()` on `.hud-scene` so no React state is touched per mouse move; the flow fade-in is the `hud-flow-in` class on a `key={flow.id}` group, so a newly arrived flow mounts and fades once.
- The model is recomputed at minute granularity (`clock − clock % 60_000`) while the clock text ticks each second; the brief's "NOW marker moves each minute" is why.
- Panel punch-card dots are `--fg-secondary` (the brief gives radius/opacity but no colour); the current-cell ring is bone as specified.

## Not built

- Nothing from §1–§5 was dropped. Out of scope per the brief and untouched: engine/RPC, new packages, WebGPU, a settings toggle, Attention screens, sidebar, SectionBand, other routes.
- Worker 2 took no browser screenshot (no dev server in the wave); the orchestrator's R1 screenshots at `/?sim=sandbox-ok` and `/` against the live engine are the visual proof of the pre-R2 scene; the R2 band (HTML grid, cqw-sized stream) is proven by render tests only — a screenshot pass on the R2 tree is the one remaining visual check.

## Tests written

- `scene/model.test.ts` — 14 cases (geometry, ordering, determinism, caps, layout spacing, footer, the theuth fold, desk-less sub-label, `needYou: null` while loading, `streamLink`).
- `scene/Scene.test.ts` — 3 cases (Scene with fixture incl. every R2 destination by href and the stream box; Scene loading `—` / quiet tertiary / error note; SceneCompact + `SceneSwitch` both branches with the breakpoint hook mocked).
- `hud/Hud.test.ts` — the existing secret-row case plus the R2 case (Hud = header + scene, no retired sections). `hud/model.test.ts` unchanged.

## Gates (from `crates/desktop/ui`, under the shared lock)

1. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bun run check`
   - exit 1 (run twice, identical: 403 s and 510 s wall incl. lock wait). Zero errors in any file this wave created or edited (`scene/*`, `queries.ts`, `styles.css`, the edited regions of `Hud.tsx`). Every error is pre-existing bindings drift — the `src/rpc/bindings` snapshot copied into the worktree is older than the TS source (`AttentionKind` lacks `ask`/`show`/`resume`, `AttentionItem` lacks `packet`, `ScheduleInfo` lacks `sensor`/`for`/`cooldown`/`timeout`/`model`/`run_timeout`, `RunInfo` lacks `kind`, modules `Item`, `Pointer`, `Packet`, `Client`, `PreviewId`, `PagePreview*`, `SensorInfo`, `SensorArg`, `RunKind`, `PairingLink`, `Telegram*`, `ScreenContext*` are missing) plus unrelated graph/store typing. Regenerating bindings needs cargo, which this wave may not run.
   - Four of them sit inside `src/components/hud/` but in lines this wave did not touch, all blamed to 2026-10-06 commits before the branch point: `Hud.tsx(207)` `KIND_LABEL.ask`, `Hud.tsx(289)` `item.kind === 'ask' || 'show'`, `Hud.tsx(312)` `item.kind === 'resume'` (commits `58f58ea13`, `7cca48707`); `model.test.ts(44)` `packet` on `AttentionItem` (`b04f915ed`). After R1 the same four sit at `Hud.tsx(198, 280, 303)` and `model.test.ts(44)` — the branch removal moved them up 9 lines.
   - R1 rerun on `6fe0cfae`: exit 1, 357 s; `grep components/hud|styles` shows exactly those four lines, nothing in `scene/*`, `queries.ts` or `styles.css`.
   - R2 rerun on the R2 tree: exit 1, 109 s; the same four pre-existing lines, now at `Hud.tsx(136, 218, 241)` and `model.test.ts(44)` (the section removal moved them up 62 lines); nothing in `scene/*`, `queries.ts`, `styles.css` or `Hud.test.ts`.
   - Full pre-existing file list: `components/attention/{AskScreen,Bundle,Bundle.test,DecisionScreen,Pointers,ReviewScreen,Screen,model,model.test}`, `components/chat/{AskCard,MessageTool,surface/Graph,surface/diagrams/GraphDelta.test,surface/registry}`, `components/codemap/CodeMapPanel`, `components/hud/{Hud,model.test}` (above), `components/routines/{RoutineDetail,RoutinesView,helpers,helpers.test}`, `components/settings/{Channels,Channels.test,sandbox-queries}`, `dev/routines-fixture`, `lib/{graph-layout,page-preview,page-preview.test,speech}`, `rpc/{api,events}`, `store/{reduce,reduce.test,sessions}`.
2. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vitest run src/components/hud`
   - `Test Files 4 passed (4) · Tests 24 passed (24)` (lock acquired after 203 s; 15.5 s run). R1 rerun on `6fe0cfae`: `4 passed · Tests 25 passed (25)`, 20.2 s. R2 rerun: `4 passed · Tests 27 passed (27)`, 1.6 s (a first R2 run failed two `/new` href counts — expectation 2 vs the correct 3: ring, its AGENTS row, SESSIONS LIVE — fixed in the test).
3. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vite build`
   - `✓ 13980 modules transformed · ✓ built in 3.54s`; `dist/assets/Hud-DIaVyohp.js 31.80 kB │ gzip 9.39 kB`. Only the pre-existing > 500 kB chunk warnings. R1 rerun on `6fe0cfae`: `✓ built in 6.27s`, `Hud-CWF38TN6.js 31.84 kB │ gzip 9.41 kB`. R2 rerun: `✓ built in 6.92s`, `Hud-B1zoJ_XA.js 24.04 kB │ gzip 7.61 kB` (the retired sections and the composer left the chunk).
