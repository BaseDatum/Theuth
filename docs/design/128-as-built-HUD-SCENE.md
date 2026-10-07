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

R3 (orchestrator, 12:24, after screenshots of the R2 tree): two corrections, folded in as `1c07a9bf` (+ the test fixes in the
as-built commit). (1) The docked `Composer` is back under the scene exactly as at `e04f1d1a` (same component, props
`session={null} draftCreate={{ private: false, project: null }} compact`, same `shrink-0 border-t … px-4 pt-3` dock below the
scrolling area, so it sits at the bottom of `/` on desktop and compact alike) — "old stuff below the hud" meant the five report
sections only; R2's removal of the composer is reverted and the R2 note above corrected by this one. (2) `shortId(item)`:
task-born → the task's short id; else `item.agent` when set (`builder`, `ops-desk`); else the first word of `item.title`
(`GITHUB_TOK` from `GITHUB_TOKEN rotation requested`), all cut to 10 chars — the kind is never repeated beside the KIND column
or on the right-hand flow labels. Hud.test asserts the composer is present and docked after the scene (the real `Composer`
reads `window` at render — `lib/mobile.ts` keyboard inset — so the test stubs it with a `data-composer` marker; the scene and
header render for real); model.test asserts the three short-id branches on the fixture (`builder`, `001198`, `001188`,
`ops-desk`, `001203`, `GITHUB_TOK`) and on synthetic items.

R4 (Sky on the R2 screenshot at 1600×1100: "why isn't it actually filling the window?"; relayed 12:31). Cause: a fixed
1920×812 viewBox scaled to width left the remaining window height empty. Folded in as `571cbb49`:
- `/` (desktop) is a flex column: `Overview` header · the field (`flex-1 min-h-[420px]`) · the panel band (natural height) ·
  the docked Composer. Below the minimum the page scrolls (the scroll area's child is `flex min-h-full flex-col`).
- The field measures its box with a ResizeObserver (applied once per animation frame — `useFieldHeight`) and sets
  `viewBox="0 0 1920 hv"`, `hv = fieldHeight(w, h) = clamp(round(1920 · h / w), 600, 1400)`; the canvas uses the same
  `1920 × hv` space and resizes with it. Unmeasured (SSR, tests) `hv` is the mockup's 812.
- Geometry is `geom(hv)` in model.ts, threaded as a trailing `g: Geom = geom()` through `theta`/`arcPoint`/`arc`, `rings`,
  `needsYou`, `flowPath`, `layoutEnds`, `hairlines`, and `scene(data, now, hv)`; `SceneModel.hv` records it. Every y is the
  mockup's y at 812 scaled by hv/812 so hv = 812 reproduces the mockup exactly: arc centre `cy = 420·k`, `θmax =
  asin(sin 14° · k)` (the band keeps its ≈ 82 % share of the height), label band `[60·k, 775·k]`, hairline sources
  `[250·k, 690·k]`, needs cascade from `262·k` in steps of `52·k` (= 0.064·hv) with the step shrunk when twelve dots would
  cross the footer; the footer rule is at `hv − 34`, its text at `hv − 17`, the bottom corner marks at `hv − 54 / hv − 40`.
  Rings are centred on `cy` (`y0 = cy − 53·(n−1)/2`) so nine fit at hv 600 — at 812 that puts the seven fixture rings at
  261 + 53·i, four px above the mockup's 265: the one visible change at the reference height, accepted for the centring.
  Deviations from the relayed numbers, all so that 812 stays the mockup: 0.44 → sin 14°·812/1390 = 0.414 in θmax; the
  needs start 0.24·hv → 262/812 = 0.323·hv; label band [0.06, 0.95] → [0.0739, 0.9544]; hairline sources [0.23, 0.64] →
  [0.308, 0.850]; ends [0.055, 0.95] → the label band.
- Tests: model.test sweeps hv ∈ {600, 812, 1100, 1400}: `geom(812)` equals the mockup constants and 14°; `fieldHeight`
  rounding and clamping; the arc band spans > 78 % of hv and stays between the top bar and the footer; nine rings centred on
  `cy` inside the field; twelve needs-you dots above the footer; `layoutEnds` ends inside the band with ≥ 22 px per column;
  hairline ends inside the band; `scene(...).hv`. Scene.test is unchanged (SSR renders hv 812). SceneCompact has no field
  and is unaffected.

R5 (orchestrator, 12:57, from the final screenshots — desktop 1600×1100 and 1920×1080 fill the window, phone right, live
data renders 28 items cleanly): `shortId` caps at 8 characters (`GITHUB_TOK` overran the ID column into KIND) and skips
hex-like candidates (≥ 6 hex chars — the session prefix `show`/`ask` items carry as agent and title word): task short id →
agent (if not hex-like) → first title word (if not hex-like) → the kind word. Fixture labels are now `builder · 001198 ·
001188 · ops-desk · 001203 · GITHUB_T`; model.test covers the hex fall-through to the kind, a hex agent with a readable
title (`Which`), and a 5-hex name that is kept (`abc12`). Folded in as `97811683`.

R6 (Sky, 12:58: "where does that chat input go, new session? … probably to theuth's desk"; relayed 13:00). The old `/`
composer created a new session on send; now it talks to theuth's desk the way the voice key does (`lib/voice-key.ts`).
Folded in as `c8404adb`:
- `Hud.tsx` `DeskComposer`: the desk id is the sessions list's `desk: 'theuth'` row when present (no RPC), else
  `api.desk_open('theuth')` on the first submit, cached for the mount; the composer stays enabled meanwhile and the send
  awaits it. A known desk renders `<Composer session={desk} onSent=…>` so Composer's own steer/send_input path applies (a
  running desk is steered, an idle one gets `send_input`); an unknown one renders `<Composer session={null}
  resolveSession=…>`. Either way a successful send opens `/c/<desk>` so the reply is visible. Compact uses the same dock.
- `components/chat/Composer.tsx` gained two optional props (the only change outside hud/ in this wave): `resolveSession`
  — with `session` null, the first send goes to the session it resolves instead of `new_session`/`create_session`
  (`markSent` → `send_input`, then the default `/c/$session` navigation; on failure the draft follows the target as it does
  for a created session) — and `onSent(session)` — fired after a bound session took the message, which `DeskComposer` uses
  to navigate. No other caller passes either; `draftCreate` behaviour is untouched.
- Hud.test (Composer stubbed, now capturing its props; `useNavigate` and `desk_open` mocked): unknown desk → the stub is
  unbound with a resolver, `desk_open` not called at render, the resolver returns the id and called `desk_open('theuth')`;
  a `desk: 'theuth'` sessions row seeded in the QueryClient → bound to `desk-1`, no RPC, and `onSent` navigates to
  `/c/$session` with that id. The unknown-desk navigation is Composer's existing created-session path (can't run in node).
- Regenerated bindings (the orchestrator's integration gate) landed during R6: the four pre-existing `AttentionKind`/`packet`
  errors are gone, and `fixtures.ts` was updated for the new required fields (`ScheduleInfo` sensor/for/cooldown/timeout/
  model/run_timeout/created, `AttentionItem` packet/ask/turn).

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
| `components/hud/Hud.tsx` | R2/R3: renders the `Overview` header row (sidebar toggle), `<SceneSwitch>` in the scrolling area, and the docked new-chat `Composer` below it (unchanged from `e04f1d1a`). Gone: the four `Metric` cards, the Attention/In flight/Board pulse/Activity/Spend sections, the mesh chip, `InFlightList`/`ActivityRow`/`SpendList`, and their imports (`useMesh`, icons, `boardPulse`/`inFlight`/`runningRoutines`/`spend`/`headline`). Scene data is memoised from `useAttention`/`useTasks`/`useAgents`/`useRoutines`/`useActivity` plus the three new hooks; under `?sim=` the attention/agents/tasks/routines/activity sources come from `sceneFixture`. Query errors are collected into the one-line note. Exports kept for components/attention/*: `Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList` (+ its `AttentionCard`/`EmptyNote`), `KindActions`. |
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
   - R3 rerun on `1c07a9bf`+tests: exit 1, 15 s; the same four lines at `Hud.tsx(140, 222, 245)` and `model.test.ts(44)`; nothing else under `components/hud`.
   - R4 rerun on `571cbb49`: exit 1, 3 s; the same four lines at `Hud.tsx(140, 222, 245)` and `model.test.ts(44)`; nothing else under `components/hud`.
   - R5 rerun on `97811683`: exit 1, 12 s; the same four lines (the bindings snapshot had not yet been regenerated when it ran).
   - R6, with the regenerated bindings in place: a first run showed only `fixtures.ts(75)` (`ScheduleInfo` sensor/for/cooldown/timeout/model/run_timeout/created) and `fixtures.ts(102)` (`AttentionItem` packet/ask/turn) — the fresh types, fixed in the fixture builders; the rerun (589 s lock wait) printed no error lines and no `exited with code 1`: `bun run check` is clean. The four `AttentionKind`/`packet` lines listed above are gone with the old snapshot.
   - Full pre-existing file list: `components/attention/{AskScreen,Bundle,Bundle.test,DecisionScreen,Pointers,ReviewScreen,Screen,model,model.test}`, `components/chat/{AskCard,MessageTool,surface/Graph,surface/diagrams/GraphDelta.test,surface/registry}`, `components/codemap/CodeMapPanel`, `components/hud/{Hud,model.test}` (above), `components/routines/{RoutineDetail,RoutinesView,helpers,helpers.test}`, `components/settings/{Channels,Channels.test,sandbox-queries}`, `dev/routines-fixture`, `lib/{graph-layout,page-preview,page-preview.test,speech}`, `rpc/{api,events}`, `store/{reduce,reduce.test,sessions}`.
2. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vitest run src/components/hud`
   - `Test Files 4 passed (4) · Tests 24 passed (24)` (lock acquired after 203 s; 15.5 s run). R1 rerun on `6fe0cfae`: `4 passed · Tests 25 passed (25)`, 20.2 s. R2 rerun: `4 passed · Tests 27 passed (27)`, 1.6 s (a first R2 run failed two `/new` href counts — expectation 2 vs the correct 3: ring, its AGENTS row, SESSIONS LIVE — fixed in the test). R3: a first run failed 8 (a stale-anchor edit had dropped `truncate` from model.ts — restored; the real `Composer` threw `window is not defined` in node — stubbed in Hud.test); rerun `4 passed · Tests 27 passed (27)`, 1.6 s.
   - R4 rerun on `571cbb49`: `4 passed · Tests 28 passed (28)`, 1.9 s (first run).
   - R5 rerun on `97811683`: `4 passed · Tests 28 passed (28)`, 1.6 s.
   - R6: `4 passed · Tests 29 passed (29)` (the new desk-composer case), twice — before and after the fixture field update.
3. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vite build`
   - `✓ 13980 modules transformed · ✓ built in 3.54s`; `dist/assets/Hud-DIaVyohp.js 31.80 kB │ gzip 9.39 kB`. Only the pre-existing > 500 kB chunk warnings. R1 rerun on `6fe0cfae`: `✓ built in 6.27s`, `Hud-CWF38TN6.js 31.84 kB │ gzip 9.41 kB`. R2 rerun: `✓ built in 6.92s`, `Hud-B1zoJ_XA.js 24.04 kB │ gzip 7.61 kB` (the retired sections left the chunk). R3 rerun: `✓ built in 8.10s`, `Hud-D-BtYC8C.js 24.25 kB │ gzip 7.68 kB` (the composer is a shared chunk, so its return barely moves Hud's).
   - R4 rerun on `571cbb49`: `✓ built in 5.08s`, `Hud-DPKI8EJ2.js 24.67 kB │ gzip 7.82 kB`.
   - R5 rerun on `97811683`: `✓ built in 4.63s`, `Hud-QhHoFmFe.js 24.67 kB │ gzip 7.82 kB`.
   - R6: `✓ built in 6.16s`, `Hud-CotBYPwd.js 24.98 kB │ gzip 7.92 kB`.
