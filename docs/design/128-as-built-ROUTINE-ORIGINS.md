# 128b as-built — ROUTINE-ORIGINS (UI-only follow-up to design 128)

Branch `feat/128b-routines-ring`, worktree `theuth-128b-hud`, cut from main `5be737a9`. Brief: `theuth-128-briefs/brief-ROUTINE-ORIGINS.md`.
Design: `docs/design/128-hud-scene.md` → `## Revision 1 — routine origins`.

Sky, 2026-10-07 — 14:37 "why aren't routines drawing lines to their starting points on the ruler?"; 14:40 "each routine should
have its own origin. don't lump them all together."

## Cause (as found on main)

`outcomes()` set `via: ringByActor(rings, e.actor)`; a `routine_fired` row's actor is `system`, so `via` and `src` were both null
and `flowPath` took its last branch — the line STARTED at the arc (`M ax ay`): a square on the ruler and a right-hand segment,
nothing leading in. `rings()` made a ring only for RUNNING schedules, so even those were never the origin of their firings.

## Per-file changes (all under `crates/desktop/ui/src/components/hud/scene/`)

### model.ts
- `RING` gains `gap: 20` (the space before the routine group) and `cap(hv) = max(9, ⌊(hv − 150) / 53⌋)` (the column's capacity;
  9 at hv 600, 12 at 812, 23 at 1400). `RING.max = 9` stays the agent group's own fold cap.
- `Target` gains `{ route: 'routines' }` (the `+N routines` fold ring → `/routines`).
- `RingStyle` gains `'failed'` — hollow carnelian, NOT dashed (`error` stays the dashed agent state). `ringColor('failed') = C.danger`,
  `ringDashed('failed') = false`, `RING_RANK.failed = 5`.
- `isError` moved above the rings section (shared by `rings()` and `outcomes()`).
- New `dueAt(s, now)`: `next` as ms when it falls in `[now, midnight + 24h)`, else null. `arc()` now uses it for the hollow squares
  (same behaviour as the inline check it replaces); `dueFlows()` uses it so the connector lands exactly where the square sits.
- New `routinePresence(routines, activityToday, now)` → `{ running, fired, due }`: running = `ScheduleInfo.running`; fired = every
  `routine_fired` subject today (newest ts per name, the schedule's `last_result` attached; keyed by the row's subject so a deleted
  schedule still has its origin) that is not running; due = `dueAt` non-null, not running, not fired today. Running first, fired
  newest-first, due soonest-first. One group per routine.
- `rings(agents, sessions, routines, activityToday, attention, now, g)` — new `activityToday` parameter. Agent rings and their
  `+N more` fold unchanged. Then the routine group: `ring:routine:<name>`, label `routine · <name>`, `link: /routines/$name`;
  running → `live`, sub `running · <age(last)>`; fired → `idle`, sub `ran HH:MM`, or `failed`, sub `failed HH:MM` when
  `isError(last_result)`; due → `lapsed` (dashed `--fg-tertiary`, the existing grammar), sub `next HH:MM`. Routines beyond
  `RING.cap(hv) − agentRings` fold into ONE `ring:routines:more` (`+N routines`, `lapsed`, → `/routines`) in the last slot, so every
  shown ring is a distinct origin. Column centred on `g.cy` with the gap inside the span: `y = y0 + 53·i + (i ≥ nAgents ? 20 : 0)`,
  `y0 = cy − (53·(n − 1) + gap) / 2`; no routines → no gap (the old geometry exactly).
  Ceiling (`ponytail:` comment): a full agent column (9 agent rings at hv 600) leaves 0 slots — the routine group is not drawn there,
  because the fold ring alone would cross the footer rule. Upgrade path: shrink `RING.dy` under hv 700 or fold agents one deeper.
- New `ringByRoutine(rings, name)` → the ring with id `ring:routine:<name>` or null. `ringByActor`/`ringByName` unchanged.
- `FlowKind` gains `'due'`.
- `outcomes()`: `via = e.kind === 'routine_fired' ? ringByRoutine(rings, e.subject) : ringByActor(rings, e.actor)`. The path of
  every routine_fired outcome now starts at `M <ring.x + 14> <ring.y>`; when the ring is folded away `via` is null and the line
  starts at the arc as before. Outcome rule unchanged (error fires are not outcomes).
- `running(tasks, routines, rings, now)` — new `routines` parameter. Adds one faience in-flight line per running routine:
  `running:routine:<name>`, via its ring, hour at `last` (else now), label `running · <elapsed>`, title `<name> · running · <elapsed>`,
  link `/routines/$name`. Tasks and routines are sorted together newest-first and capped at `RUNNING_MAX` (8).
- New `dueFlows(routines, activityToday, rings, now, g)` → complete `Flow[]` (`end`, `d` set): for each due routine with a ring,
  `d = M <ring.x+14> <ring.y> C … <square>` where `square = arcPoint(hourOf(next), R + 9, g)` — the identical point `arc()` draws the
  hollow mark at. `kind: 'due'`, `color: C.tertiary`, `label: ''`, no quadratic segment. `layoutEnds` never sees them (they are
  appended to `flows` after layout in `scene()`), so they take no label slot.
- `cubic()` hoisted out of `flowPath` to module scope (shared by `dueFlows`).
- `hairlines()`: `line(id, own: Ring | null, time)` — the caller resolves the ring: `routine_fired` rows → `ringByRoutine(rings,
  e.subject)`, everything else `ringByActor` as before (the 62 % ring-sourced rule and seeding unchanged). A failed fire's grey line
  therefore leaves its routine's ring.
- `scene()`: passes `today` to `rings()`, `routines` to `running()`, appends `dueFlows(...)` to `flows`. `footer.inFlight` already
  counted `routines.filter(r => r.running)` — the running routine's line and the IN FLIGHT number agree.

### Scene.tsx
- `linkFor`: `case 'routines'` → `<Link to="/routines">`.
- Flow rendering: a `kind === 'due'` flow draws only its path — `stroke={C.tertiary}` (the flow's colour), `strokeOpacity 0.5`,
  `strokeDasharray "3 3"`, `strokeWidth 1.1`; no end dot, no label. Same `To` wrapper (tooltip `<name> · due HH:MM`, link to the
  routine), same `data-flow` and `hud-flow-in` class. Nothing else changed; `RingGlyph` already renders `failed` correctly through
  `ringColor`/`ringDashed`.

### SceneCompact.tsx
- No change: its rings list maps `m.rings`, so the routine group appears there with the same glyphs and sub-labels.

### fixtures.ts
- Routines: `morning-brief` (fired 06:00, ok), `standup-notes` (fired 09:00, `last_result: 'error: no notes'`), `nightly-digest`
  (running, last 18 min ago, also a fire row), `knowledge-sync` (next 23:30 today). `lunch-digest` removed. The fixture scene now has
  10 rings (6 agents + 4 routines), 9 outcomes, 4 running lines (3 tasks + nightly-digest) and 1 due connector.

### model.test.ts (19 tests) / Scene.test.ts (3 tests)
- rings: the 10 names/styles/subs/ids in order, geometry with the gap, no gap without routines, one ring for a routine that is both
  running and fired, fired-with-a-later-`next` is fired not due; `ringByRoutine` hit/miss.
- rings capacity: `RING.cap` at 600/812/1400; 12 running + 3 fired routines at hv 600 → `routine · r0`, `routine · r1`,
  `+13 routines` → `/routines`, centred and inside the field; at hv 1400 all 15 fit; agent fold first (`+5 more`) then routines;
  full agent column at hv 600 → no routine rings.
- outcomes: total 9; nightly/morning fires `via.id === 'ring:routine:<name>'`; every routine flow's `d` starts at `M <via.x+14> `;
  folded ring → `via` null and the path starts at the arc point.
- running: the running routine's line between the 12 m and 40 m tasks, its via/link/title/hour; `last: null` → timed at now.
- dueFlows: one connector, dashed-ready (`color C.tertiary`, `label ''`, no `Q`), `end` equals the hollow mark's `[x, y]` at hv 812
  and 1100; none when the ring is folded away.
- hairlines: a failed fire's grey line leaves the routine ring when its seed says ring-sourced (several seeds).
- scene: footer `inFlight: 3 + 1`, 10 rings, 20 flows, the `due:knowledge-sync` flow, `running:routine:nightly-digest` via its ring;
  without the running routine `inFlight` is 3.
- Scene render: 10 `r="13"` circles (desktop and compact); the failed ring is a danger-stroke circle without `stroke-dasharray`;
  `failed 09:00` / `ran 06:00` / `next 23:30` / `running · 18m` text; every routine-linked flow's `d` starts at its ring; the due
  flow's `<g>` holds one dashed `--fg-tertiary` path at opacity 0.5 and no `<text>`/`<circle>`; the running routine's `<g>` carries
  the `running · 18m` label; `9 OUTCOMES TODAY`, `4 IN FLIGHT`; compact list links `/routines/knowledge-sync`.
- Pre-existing tests updated for the fixture: arc marks (`lunch-digest` gone), outcome totals 11 → 9, all `rings()`/`running()`
  call sites take the new parameter.

## Gates (from `crates/desktop/ui`, each one foreground command under `build-lock`, label `128b-hud`)

1. `bun run check` — **0 errors in `components/hud/**`**. The run exits 1 on 73 pre-existing errors outside this work: the
   worktree's gitignored `src/rpc/bindings` is stale against main `5be737a9` (no `Advisor*`, `AdvisoryNote`, `SyncBacklog`,
   `SessionKind` bindings while `src/lib/advisor-format.ts`, `src/store/reduce.ts`, `src/rpc/api.ts`, `src/lib/session-tree.ts` …
   import them; `AttentionKind` carries `finished` while `Hud.tsx:156` / `attention/Screen.tsx:18` maps predate it). Regenerating
   the bindings needs cargo, which this brief forbids; no file outside `hud/scene` was touched.
2. `bunx vitest run src/components/hud` — 4 files, **32 passed**.
3. `bunx vite build` — **built in 4.16 s** (the usual > 500 kB chunk warning).

Visual smoke: the web build outside Tauri stops at mesh pairing, so `/` was not reachable in a browser; instead the fixture `Scene`
was rendered to static HTML with the `.dark` tokens (throwaway vitest file, deleted) and screenshotted — routine group one gap under
the agents; nightly-digest filled faience with its in-flight line; standup-notes hollow carnelian `failed 09:00`; morning-brief hollow
faience `ran 06:00` with its malachite line into the 06:00 square; knowledge-sync dashed with the faint dashed connector ending on
the hollow square at 23:30.

## Commits

- `3ca7fd24` model/Scene/fixtures + first test pass
- `2ac23bd8` tests
- `9b6af537` this file + the Revision 1 section (`DONE ROUTINE-ORIGINS 9b6af537`)

# R2 as-built — CONTEXT-RING (same worktree, follow-up commits)

Sky, 2026-10-07 15:05 — "can we make the desk icons outer ring on the hud be a context window indicator of the desk session?"
Design: `docs/design/128-hud-scene.md` → `## Revision 2 — context gauge on the desk rings`.

## Per-file changes

### scene/model.ts
- `Ctx = { fill, used, window, live }`; `Ring.ctx: Ctx | null` (set on agent rings — the theuth ring included when its roster row has
  a desk — whose desk session has an entry in `SceneData.contexts`; `null` otherwise and on routine/fold rings).
- `contextFill(breakdown, session, models)` → `Ctx | null`: `used = last_provider_context ?? total_estimate`; `window =
  session.context_size ?? models.find(id === session.model)?.context`; `window ≤ 0` → null; `fill` clamped to [0, 1].
  Precedence note: the addendum named the catalog first and `context_size` as the fallback; the chat's `ContextMeter` trusts the
  harness's reported `context_size` first, and this follows the house rule. The two differ only when both exist and disagree.
- `kTokens(n)` (`8.2k` under 10k, `118k` above), `ctxTitle(ctx)` → `context ≈ 118k of 200k (59 %)`.
- `GAUGE_R = 17`. `gaugeEnd(cx, cy, r, fill)` → the sweep's end point; `gaugePath(cx, cy, r, fill)` → `''` at ≤ 0, one `A r r 0 L 1`
  arc with `L = fill > 0.5 ? 1 : 0`, two half arcs at ≥ 1.
- `rings(…, g, contexts = NO_CONTEXTS)` — trailing optional `ReadonlyMap<SessionId, Ctx>`. The desk session = `sessions.find(id ===
  a.desk) ?? sessions.find(desk === name)`. Sub-label gets ` · ctx N %` (rounded) when `ctx` is set.
- `SceneData.contexts?: ReadonlyMap<string, Ctx>`; `scene()` passes it through.

### scene/Scene.tsx
- Each ring with `ctx`: `<g data-gauge={ring.id}>` holding the track `<circle r=17 fill=none stroke=C.border stroke-width=1>`, the
  sweep `<path d=gaugePath(x, y, 17, fill) stroke-width=2 stroke-linecap=round>` (omitted at fill 0) and the 1.6 px end dot — sweep
  and dot in `C.info` when the ring is `live`, `C.secondary` otherwise (never `C.accent`, never `C.danger`). Tooltip appends
  ` · ${ctxTitle(ctx)}`. Flow anchors unchanged (±14). Spacing: 53 − 34 = 19 px between tracks, so r 17 stands.

### scene/SceneCompact.tsx
- No change: the row's sub-label already carries ` · ctx N %`; no gauge in the compact list (the 32 px glyph box has no room).

### hud/queries.ts
- `useDeskContexts(sessions)`: `useQueries` over `sessions.filter(desk && !archived)` — `queryKey ['context', 'breakdown', id]`,
  `api.session_context_breakdown(id)`, `staleTime 20_000`, `refetchInterval 30_000`, `enabled: !sim`; a module-level `combine`
  (`rs.map(r => r.data)`) keeps the result referentially stable between unrelated renders. One `on(...)` subscription each for
  `TurnFinished`, `Compacted`, `ContextUsed` (all carry `session`) invalidating that session's key. Folds with `useModels()`
  (`components/agents/queries.ts`, `queryKeys.models`) through `contextFill` into a `Map<SessionId, Ctx>` (memoised on desks, data,
  models).

### hud/Hud.tsx
- `const contexts = useDeskContexts(sessions.data)`; `sceneData.contexts = fixture?.contexts ?? contexts` (+ dependency).

### scene/fixtures.ts
- `contexts`: `desk-ops` 124k/200k live, `desk-research` 62k/200k live, `desk-builder` 176k/200k not live; `desk-analyst` absent
  (a desk with no breakdown → no gauge). `sceneFixture` stays `Required<SceneData>`.

### Tests (model.test.ts 22, Scene.test.ts 3 — 35 in `src/components/hud`)
- gaugePath at 0 / −0.2 / 0.25 / 0.5 / 0.75 / 1 / 1.4 (exact strings; flag flips above 0.5; full circle at ≥ 1); `gaugeEnd` at 0,
  0.25, 1.
- contextFill: last bill over estimate; `context_size` over the catalog and usable without models; clamp to 1; null without models,
  with an unknown model, with `context: 0`. `kTokens`, `ctxTitle`.
- ring.ctx: ops-desk `{0.62 …}` and sub `4 open · 1h · ctx 62 %`; builder 0.88; analyst (desk, no entry) null and plain sub;
  docs-desk (no desk) null; theuth null; every routine ring null; no map → all null, labels as before; desk found by
  `SessionSummary.desk === name` when the roster row has no desk id; the `+N more` fold ring null; `scene(F).rings` with ctx =
  ops-desk, research, builder.
- Scene render: exactly 3 `data-gauge` groups / 3 `r="17"` tracks; ops-desk's group has the border-token track, a large-arc faience
  sweep (width 2, round cap) and a faience end dot; builder's sweep is `--fg-secondary` with no `--fg-accent`; no gauge for analyst,
  nightly-digest, theuth; sub-label and tooltip text. Compact: the `ctx 62 %` text and zero `r="17"` circles.

## Gates (as before, under `build-lock`, label `128b-hud`)

1. `bun run check` — 0 errors outside the pre-existing stale-bindings set described above (same 73 lines; nothing in `hud/**`).
2. `bunx vitest run src/components/hud` — 4 files, **35 passed**.
3. `bunx vite build` — **built in 7.98 s**.

Visual smoke (static render of the fixture `Scene` with the `.dark` tokens, as for R1): faience sweeps on ops-desk (62 %) and
research (31 %), builder's 88 % sweep in the secondary ink around its gold waiting ring, nothing on analyst / docs-desk / theuth /
the routine group.

## Commits

- `7194d58f` model/Scene/queries/Hud/fixtures/tests
- (this section + the Revision 2 section) — see `DONE CONTEXT-RING <sha>`.
