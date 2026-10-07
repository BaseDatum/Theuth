# 128 as-built — HUD-SCENE (UI-only wave)

Branch `feat/128-hud-scene`, worktree `theuth-128-hud`, cut from main `e04f1d1a`. Brief: `theuth-128-briefs/brief-HUD-SCENE.md`.

Two sessions built this. Worker 1 finished brief step 1 (`5abf725f`) and had Scene, SceneCompact, Scene.test, the Hud wiring
and the keyframes written when its ACP session was cut off at 10:56 while starting the vitest gate; the orchestrator checkpointed
that tree as `197d0700`. Worker 2 (this document) resumed from those two commits, audited them against brief §1–§5, ran the three
gates, and wrote this file. No source line changed between `197d0700` and the gates below — the first worker's tree was complete.

## Per-file

| File | Change |
|---|---|
| `components/hud/scene/model.ts` (new, 556 lines) | Pure scene model in the 1920×1080 viewBox: `C` token table (CSS `var(...)` strings only), `startOfDay`/`hourOf`/`nowHour`, `rng` (FNV-1a → mulberry32), `theta`/`arcPoint`/`arc` (band, 25 ticks, 12 labels, routine squares, NOW marker), `rings` (theuth · agents ranked working → waiting → idle → lapsed → error · running routines · `+N more` fold at 9), `shortId`, `needsYou`, `needFlows`, `outcomes`, `running`, `flowPath`, `layoutEnds`, `hairlines`, `sessionsToday`, `punchCard`, `outcomeMix` + `MIX_ORDER`, `footer`, `ringDots`, `streamColor`, `actorLabel`, and `scene(data, now)` composing the whole `SceneModel`. Missing queries draw nothing; the chrome stays. |
| `components/hud/scene/model.test.ts` (new) | 12 vitest cases against the fixture at a fixed local 19:49:19: arc geometry (00:00 tick at (1031.2, 88.1), band path, 12:00 label rotation, filled/hollow routine marks, NOW position), `hourOf` clamping, ring order/styles/geometry/fold, needsYou order/jitter/determinism/≤ 10-char ids, outcomes kinds/order/via/cost/links, running labels, `flowPath` segments, `layoutEnds` (≥ 22 px per column, x cycling, overflow counted in `more`), hairlines (determinism, skips drawn flows, one per session, 600 cap), punchCard (Mon = 0, now cell), outcomeMix, footer numbers and the empty-data chrome. |
| `components/hud/scene/fixtures.ts` (new) | `sceneFixture(now)` reproduces the mockup: 6 needs-you items, 7 rings (theuth, 5 agents incl. a paused one, 1 running routine), 8 done + 3 running, ~230 seeded field lines, a week of punch-card rows, spend rows with one agent over 85 %, 50 stream rows. `T(n)` builds the synthetic task ids the tests assert hrefs against. |
| `components/hud/scene/Scene.tsx` (new, 398 lines) | Desktop scene: `<canvas>` hairline field under an `<svg viewBox="0 0 1920 1080" role="img">` (aspect locked, min-height 540). `useClock` ticks each second and pauses on `visibilitychange`; the model is recomputed on the minute so the NOW marker moves per minute. `useField` draws the hairlines on the canvas at most once a second, colour read from `getComputedStyle(documentElement)` on mount and on every `<html class>` mutation. `To` wraps any SVG group in a TanStack `Link` (renders an SVG `<a>`, focusable) with a `<title>` tooltip. Top bar, corner marks, arc band/ticks/labels/marks/NOW, coloured flows (each `data-flow` keyed + `hud-flow-in`), needs-you dots + labels, rings (`RingGlyph`, live disc `hud-pulse`), NEEDS YOU table (6 rows + `+N`, "Nothing needs you" once attention has loaded empty), footer rule + 5 segments, the five 352×248 panels (`AgentsPanel`, `ArrivalsPanel`, `OutcomesPanel`, `SpendPanel`, `StreamPanel`) translated to x 24/404/784/1164/1544 at y 812. Error note under the footer in `--status-danger-fg`. |
| `components/hud/scene/SceneCompact.tsx` (new) | < md: footer pills, NEEDS YOU rows (full width, gold, → /attention/$id), rings as a list (glyph + name + sub, → agent/routine), then the five panels each in its own `viewBox="0 0 352 248"` SVG in the order AGENTS, OUTCOMES, SPEND, ARRIVALS, STREAM. |
| `components/hud/scene/Scene.test.ts` (new) | 3 render tests (`renderToStaticMarkup`, `Link` mocked to `<a href>` per the Hud.test pattern): fixture counts (7 rings r=13, 6 need dots r=7, one `data-flow` per laid-out flow, `hud-pulse` only on live rings, no inline `animation`, no literal `#` colour), hrefs for attention/task/agent/routine/footer, text (clock, footer numbers, panel titles), the carnelian spend track, `role="img"` + `aria-labelledby`; empty data → chrome only, "Nothing needs you" only once attention loaded, error note names the query; SceneCompact pills/rows/rings/5 panels. |
| `components/hud/queries.ts` | Added `ACTIVITY_TODAY_KEY`/`ACTIVITY_WEEK_KEY`, `useActivityToday()` (since local midnight, limit 500, refetch 60 s), `useActivityWeek()` (since now − 7 d, limit 500, refetch 5 min) — both prepend `ActivityAppended` into their cache like `useActivity`; `useSessionList()` on the sidebar's `queryKeys.sessions` key so the HUD shares that fetch. All three return the fixture under the existing `simFlag()` (same gate as `ATTENTION_SIM`). |
| `components/hud/Hud.tsx` | The four `Metric` cards and the `Metric` component are gone; `<Scene>` (≥ md) or `<SceneCompact>` (`useIsMobile()`, the app's single md breakpoint) sits above the unchanged `max-w-[880px]` column (Attention · In flight · Board pulse · Activity · Spend). `headline` import dropped (only the cards used it; `model.ts` still exports and tests it). Scene data is memoised from the existing hooks plus the three new ones; under `?sim=` the attention/agents/tasks/routines/activity sources come from `sceneFixture`. Query errors are collected into the one-line note. Every previously exported symbol (`Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList`, `KindActions`) is unchanged. |
| `styles.css` | `.hud-pulse` (scale 1 → 1.25, 1.6 s, ease-in-out, infinite, `transform-box: fill-box`), `.hud-flow-in` (600 ms fade), `.hud-scene:has([data-flow]:hover) [data-flow] { opacity: .5 }` + hovered flow at 1, both keyframes, and the `@media (prefers-reduced-motion: reduce)` block setting `animation: none` — the file's existing pattern. |

## Deviations from the brief and why

- `hairlines(activityToday, sessionsToday, rings, drawnIds, now)` takes `rings` (the brief's signature omitted them, its text needs them for the 62 % ring sources) and `now` instead of a separate `seed` — the seed is each entry's own id, which is what makes the picture stable.
- `outcomes(activityToday, routines, rings, now)` takes `routines`, not `tasks`: the error check is on `ScheduleInfo.last_result`; task ids are recognised from `subject` by shape (`looksLikeTask`).
- `needsYou(attention)` has no `now` argument — the dot positions don't depend on time; `needFlows(needs, rings, now)` carries the time-dependent part (arc hour, age label).
- The `+N more` ring uses the `lapsed` style (dashed, `--fg-tertiary`) because the brief asks for "idle, dashed" and the idle style is a solid hollow ring; dashed + dim is the closest token-only rendering and links to `/agents`.
- `N SESSIONS LIVE` has no link (there was no Metric card for it and no sessions list route); the other four footer segments link as the cards did.
- The `theuth` ring has no link (there is no "all chats" route; `/` is where you already are).
- The hover rule uses `:has()` on `.hud-scene` so no React state is touched per mouse move; the flow fade-in is the `hud-flow-in` class on a `key={flow.id}` group, so a newly arrived flow mounts and fades once.
- The model is recomputed at minute granularity (`clock − clock % 60_000`) while the clock text ticks each second; the brief's "NOW marker moves each minute" is why.
- Panel punch-card dots are `--fg-secondary` (the brief gives radius/opacity but no colour); the current-cell ring is bone as specified.

## Not built

- Nothing from §1–§5 was dropped. Out of scope per the brief and untouched: engine/RPC, new packages, WebGPU, a settings toggle, Attention screens, sidebar, SectionBand, other routes.
- No browser screenshot of the live scene was taken in this session (no Tauri CLI / dev server allowed in the wave); the render tests and the `?sim=` fixture path are the proof surface.

## Tests written

- `scene/model.test.ts` — 12 cases (geometry, ordering, determinism, caps, layout spacing, footer).
- `scene/Scene.test.ts` — 3 cases (Scene with fixture, Scene empty/error, SceneCompact).
- Existing `hud/model.test.ts` and `hud/Hud.test.ts` unchanged and still green.

## Gates (from `crates/desktop/ui`, under the shared lock)

1. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bun run check`
   - exit 1 (run twice, identical: 403 s and 510 s wall incl. lock wait). Zero errors in any file this wave created or edited (`scene/*`, `queries.ts`, `styles.css`, the edited regions of `Hud.tsx`). Every error is pre-existing bindings drift — the `src/rpc/bindings` snapshot copied into the worktree is older than the TS source (`AttentionKind` lacks `ask`/`show`/`resume`, `AttentionItem` lacks `packet`, `ScheduleInfo` lacks `sensor`/`for`/`cooldown`/`timeout`/`model`/`run_timeout`, `RunInfo` lacks `kind`, modules `Item`, `Pointer`, `Packet`, `Client`, `PreviewId`, `PagePreview*`, `SensorInfo`, `SensorArg`, `RunKind`, `PairingLink`, `Telegram*`, `ScreenContext*` are missing) plus unrelated graph/store typing. Regenerating bindings needs cargo, which this wave may not run.
   - Four of them sit inside `src/components/hud/` but in lines this wave did not touch, all blamed to 2026-10-06 commits before the branch point: `Hud.tsx(207)` `KIND_LABEL.ask`, `Hud.tsx(289)` `item.kind === 'ask' || 'show'`, `Hud.tsx(312)` `item.kind === 'resume'` (commits `58f58ea13`, `7cca48707`); `model.test.ts(44)` `packet` on `AttentionItem` (`b04f915ed`).
   - Full pre-existing file list: `components/attention/{AskScreen,Bundle,Bundle.test,DecisionScreen,Pointers,ReviewScreen,Screen,model,model.test}`, `components/chat/{AskCard,MessageTool,surface/Graph,surface/diagrams/GraphDelta.test,surface/registry}`, `components/codemap/CodeMapPanel`, `components/hud/{Hud,model.test}` (above), `components/routines/{RoutineDetail,RoutinesView,helpers,helpers.test}`, `components/settings/{Channels,Channels.test,sandbox-queries}`, `dev/routines-fixture`, `lib/{graph-layout,page-preview,page-preview.test,speech}`, `rpc/{api,events}`, `store/{reduce,reduce.test,sessions}`.
2. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vitest run src/components/hud`
   - `Test Files 4 passed (4) · Tests 24 passed (24)` (lock acquired after 203 s; 15.5 s run).
3. `BUILD_LABEL=128-hud /Users/Sky.Moore/.theuth/bin/build-lock bunx vite build`
   - `✓ 13980 modules transformed · ✓ built in 3.54s`; `dist/assets/Hud-DIaVyohp.js 31.80 kB │ gzip 9.39 kB`. Only the pre-existing > 500 kB chunk warnings.
