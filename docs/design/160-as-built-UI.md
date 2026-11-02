# 160 — as built: UI (feat/160-ui)

Worktree `~/Develop/theuth-160-ui`, branch `feat/160-ui`, cut from `9026c641` (design). Scope: everything under
`crates/desktop/ui/**` from design §8; no `.rs` file, no root `bindings/`, no generated binding committed.

## What was built (file level)

| file | what |
|---|---|
| `package.json`, `bun.lock` | `recharts 3.8.0`, `react-day-picker 10.0.2`, `date-fns 4.4.0` (via `bunx shadcn@latest add chart` / `add calendar`) |
| `src/components/ui/chart.tsx`, `calendar.tsx` | shadcn upstream, unmodified (chart = the Recharts-3 `ChartContainer`/`ChartTooltip`/`ChartLegend`). The `card.tsx` the chart registry pulls in was dropped (unused). `button.tsx` was not overwritten. |
| `src/styles.css` | `--chart-1..8` in `:root` (bone) and `.dark` (soot): faience, sand, lapis, malachite, plum, slate, nile, umber (the `--project-*` hexes). Nothing else. |
| `src/rpc/api.ts` | `cost_cube(since)`, `cost_calls(query)`, `usage_limits()`, `usage_limits_refresh(provider)` in `Api` and `METHODS` (sorted). |
| `src/rpc/events.ts` | `CostCubeChanged { epoch, rev }` typed inline and added to `WireEvent`. |
| `src/lib/costs/cube.ts` | the client cube, pure: `merge` (full replaces; same-epoch delta replaces every cell of `hours`, upserts `roots` by `ix`, adopts dictionaries + meta; other epoch / nothing held → `null` = fetch full; stale rev → held), `index(cube, tz)` (typed columns; model collapsed to canonical name; owner/project via the root; per-cell local civil day, month and hour computed once per distinct UTC hour for the injected IANA zone), one-pass queries `series` (top 6 + other, previous-period line), `totals` (per basis), `previousPeriod`, `breakdown(dim)`, `options` (facets: per-option spend under every other filter), `heat` (7 × 24), `tokenSeries`, `sessions`, `toCsv`, `bucketCount` (≤ 120 guard, R1). ISO weeks start Monday. |
| `src/lib/costs/search.ts` | `validateCostsSearch` (range / from / to / gran / by / metric / compare / 7 filter dims / `q`, drops junk and defaults, keeps `sim`), `resolveRange` (presets in the zone), `autoGran` (R1: ≤ 5 days hour, ≤ 120 days day, ≤ 120 weeks week, else month), `granBlock` (reason text for a disabled granularity), `effectiveGran`. |
| `src/lib/costs/format.ts` | `usd` (sub-cent keeps 2 significant digits), `usdShort`, `compact`, `int`, `pct`, `ago`, `when`, `resetIn` (R1: `in 48m` / `in 1h 48m` / `Thu 19:18`). |
| `src/lib/costs/persist.ts` | IndexedDB `theuth-costs` / store `cube` / key `v1:<engine>` (own 40-line wrapper: `link/idb.ts` is bound to the `link` db). |
| `src/lib/costs/queries.ts` | `CostsPrefetch` bridge (mounted once in `routes/__root.tsx` `Root()` next to `RoutineDraftBridge`): hydrate from IndexedDB, `cost_cube(since)`, delta on `CostCubeChanged` (one request in flight, a newer rev seen mid-flight refetches once), `Lagged` → delta, persist 2 s after the last change inside `requestIdleCallback`, `router.preloadRoute('/costs')` + `usage_limits` prefetch at idle. `useCostCube` / `useCostIndex` (index cached per cube × zone), `useUsageLimits` (15 s poll, paused when hidden; `usage_limits_refresh(null)` on mount and every 60 s visible), `useCostCalls` (infinite keyset pages, `keepPreviousData`). `?sim=costs`, and `?sim=costs-file` when a local build sets `VITE_COSTS_FIXTURE_URL`. |
| `src/dev/costs-fixture.ts` | synthetic deterministic fixture: 56 days ending now, 7 models (one alias pair), 3 providers, 7 kinds, 3 vias, owners you / 2 agents / 2 routines, 2 projects, a deleted root, a reported omp + opencode leg with approximate hours, an endless failure loop (unpriced), a Bedrock unpriced model; `cost_calls` over it with the same filters and keyset paging; `limitsFixture` with a provider row in every `LimitStatus` and two budgets. All invented. |
| `src/routes/costs.tsx`, `src/routeTree.gen.ts` | the `/costs` route (lazy chunk, `validateSearch`). |
| `src/components/costs/CostsPage.tsx` | band (title, live dot, `ledger since`, engine name, CSV), range bar, chips, KPIs, main chart, limits, breakdowns, when, tokens, sessions, ledger; coverage notes (H3, H4, H6) in the footer; empty ledger = one line. R1: the controls read `search` (urgent), everything drawn from the cube reads `view = useDeferredValue(search)`; below the chart each card takes the view a stage later (`useStaged`: Breakdowns 1–5, When 7, Tokens 8, Sessions + Ledger 9 frames) so a range change commits in frame-sized pieces. |
| `RangeBar.tsx` | presets Today · 24H · 7D · 30D · MTD · 90D · All, Custom popover (shadcn range `Calendar`, two months / one on phones, quick picks This week · Last week · Last month, "or drag across the chart"), granularity with the disabled reason in a tooltip. |
| `FilterChips.tsx` | Provider · Model · Owner · Kind · Via · Basis · Project chips, each a `Command` multi-select with per-option spend; Session title search; filled + removable when active; Clear all. |
| `Kpis.tsx` | six cards (`@number-flow/react`, fixed minimum size): Spend (Δ vs previous equal period, sparkline), Today (this hour, burn of the last 60 min, yesterday), month (MTD, projection at pace, last month), Calls (per call, failed, repriced), Tokens (output, cache hit, cache saved when > 0), Basis (known / reported / estimated bar, unpriced calls, "unpriced is a lower bound"). |
| `SpendChart.tsx` | stacked bars by Model · Provider · Owner · Kind · Via · Basis · Project, metric $ · Calls · Tokens, dashed previous period, drag-select → custom range (`ReferenceArea`), `Brush` past 40 buckets, bar click → filter, `ChartLegend` entries toggle the series and a filter icon beside each adds the filter. R1: the panel header (urgent controls) is separate from a memoised `Body`; legend toggles and the drag highlight reach their components through context (`LegendCtx`, `DragCtx`) so they never re-render the bars. R2: stock Recharts `<Bar>` series again (R1's one-path-per-series drawing is gone), square tops past 60 buckets; `ChartLegend` renders through its `portal` prop into a node under the chart (Recharts then never measures it and re-lays the plot out), and the chart starts at its real width (`initialDimension` from the last mount, else the sidebar inset), which removes two of Recharts' mount passes. |
| `LimitsPanel.tsx` | one card per provider (RadialBar gauges warn ≥ 75 % / danger ≥ 90 %, rate meters, balance, status tag, `source · observed N ago`), budgets card, `unavailable` / `not configured` as one muted line each, one sentence when the engine cannot answer. R1: under each gauge the window label and the reset (`resetIn`) are one non-wrapping line each (76 px column, full text in `title`). |
| `Breakdowns.tsx` | by model / owner (agent ring, routine square ticks) / kind / project horizontal bars, provider donut + list; click → filter. R1: five memoised `Breakdown` cards, each with its own data memo and stage. |
| `When.tsx` | hour × weekday heat grid (Recharts `ScatterChart`, square `shape` sized from `usePlotArea` so it never overflows) + hour-of-day profile; `Tokens`: input mix 100 % area (empty buckets skipped) + output bars. R1: `When` takes no `gran` (skips granularity changes); the empty grid is one path from the plot area and only cells with spend are Scatter points; `accessibilityLayer` off on this dense chart. |
| `SessionsTable.tsx`, `LedgerTable.tsx` | sortable sessions (deleted root = "Deleted session · <date>", not clickable; owner / model / project cells filter), first 50 + Show all; ledger virtualised with `@tanstack/react-virtual`, next page within 20 rows of the end, hairline only after 150 ms, rows → `/c/$session`. |
| `ui.tsx` | `Panel`, `Seg` (never widens the page: scrolls sideways; R1 `fade` softens the right edge below 768 px, used by the group-by strip), `Tag`, `Swatch`, `OwnerGlyph`, `useStaged`, R2 `Later` (below-the-fold section: a blank block of its usual height until its idle slice, or until a scroll brings it within half a viewport). |
| `Resident.tsx` (R2) | `CostsResident`, mounted in `__root` over the routed column: once the cube is held it pre-renders the page at idle (lazy chunk, `startTransition`) and keeps it mounted when you leave — laid out at the column's size, transparent, `inert` and behind; opening `/costs` flips opacity, z-order and `inert` (no restyle of the page's nodes). The `/costs` route renders nothing under the host (`ResidentCtx`); a popped-out window has no host and renders the page itself. While hidden the page is frozen: the clock does not tick, the cube index is held (`useHeld`), the limits panel does not poll. |
| `CostsPage.tsx` (R2) | takes `search` / `visible` from the host; controls read `pending` (their own state, set synchronously on click) and the URL write runs one frame later inside `startTransition` (rAF → setTimeout → `navigate`); the clock and the cube index are deferred; usage limits, breakdowns, when, tokens, sessions and ledger mount through `Later` (orders 1–5). |
| `routes/__root.tsx` (R2) | the routed page and `CostsResident` share one relative column (`ResidentCtx`); the meeting bar stays below it. |
| `lib/costs/queries.ts` (R2) | `CostsPrefetch` indexes every cube the cache takes, at idle (`indexFor`), so no page render builds one; `useUsageLimits(onScreen)` polls and kicks only while the page is on screen. |
| `lib/costs/cube.ts` (R2) | precomputed `week` and `tokens` columns; `series` picks the metric's column once (not per cell); `sessions` uses a root × model spend matrix (models listed by spend) instead of a `Set` per root. |
| `src/lib/files.ts`, `components/chat/DiagramFrame.tsx` | `saveBlob` moved from `DiagramFrame` to `lib/files` so the CSV export uses the same sink (browser download; desktop: attachment store + reveal). |
| `src/components/hud/scene/{Scene.tsx,model.ts,Scene.test.ts}` | `Target` `{ route: 'costs' }` (Link `preload="intent"`); `Panel` optional `action` drawn right after the title in the 352×248 space (14 × 14 hit square, chart-line icon, `<title>Open costs</title>`, `aria-label`); SPEND uses it; SPEND's big number and the footer's `$… THIS MONTH` go to `/costs` (were `/agents`). |

## Deviations from the design, and why

1. **`CostCubeChanged`** was first typed inline in `rpc/events.ts`; the closer switched it to the generated binding (`bc0a4284`), merged here in R2.
2. **IndexedDB key `v1:<engine scope id>`** (`'local'`, or the paired engine's id; the dev socket URL for a local dev engine). The connection store holds no engine URL; the scope id is stable per engine, and an epoch change replaces a stale snapshot anyway.
3. **Filter ids.** Model filters use the canonical `name` (aliases share one chip; `LedgerTable` maps names back to every raw id for `cost_calls`); owner ids are `<owner_kind>:<owner>` (an agent and a routine may share a name); project `''` = no project.
4. **Previous period** — the KPI delta compares the equal number of hours just before the range; the dashed line plots the equal number of buckets just before (so a partial first day compares with full days). Both documented in `cube.ts`.
5. **Granularity guard** — R2: Hour is offered for ranges of up to 7 local days (≤ 168 buckets; 169 across an autumn DST change), so 7D and a custom week can be hourly (auto still picks Day for 7D); every other choice up to the design's 400-bucket hard cap and (except Hour) at least 2 buckets; auto pick ≤ 5 days → Hour, ≤ 120 days → Day, ≤ 120 weeks → Week, else Month. A disabled choice explains itself (`720 hours — Hour goes up to 7 days`, `one bucket — …`).
6. **Legend click vs §7.3** — §7.5 says the legend toggles series, §7.3 says a legend click adds a filter; the entry toggles, the small filter icon next to it filters.
7. **No palette entry** — D1 asks for `Costs` in the search/command palette "if it lists pages": `SearchDialog` is a content search with no page list, so nothing was added; no sidebar row (D1).
8. **KPIs follow the filters** (Today and the month cards too), so every number on the page answers the same question.
9. **Ledger range** — a range ending now is sent as `to_ms: null`, so the minute clock does not refetch loaded pages.
10. **An empty `full` answer while the engine is still building** never replaces a non-empty held cube (§3.3).
11. **The costs page is resident** (R2): pre-rendered at idle after start and kept mounted, hidden, between visits, so a navigation shows a finished page instead of building one (Recharts needs several commits per chart; that was the ~300 ms first-mount hitch). A `/costs` visit within ≈ 1.5 s of start, before the pre-render, still builds the page on demand. Below-the-fold sections (now including usage limits) mount in idle slices behind height-holding blank blocks.
12. **`?sim=costs-file`** is honoured whenever a local build sets `VITE_COSTS_FIXTURE_URL` (was dev-only), so production-profiling builds can measure on a real-shaped cube; normal builds never set it.

## Tests

- `src/lib/costs/cube.test.ts` (11): merge (full, delta hours + root upserts + dictionaries, epoch change → `null`, nothing held, stale rev), local day / ISO week / month across the autumn DST change in `Europe/London` and `America/New_York` (25-hour days, Sunday ends the ISO week, 31 Oct 23:00 NY is October), the spring change (23-hour days, hour labels skip 01:00), filters (alias collapse, owner via root, project incl. none, title search incl. deleted roots, facets ignore their own dimension), top 6 + other with distinct colours, previous period (line and KPI window), totals per basis + breakdowns + heat cell placement in `Europe/Paris` + token mix + session rows (deleted label, dominant basis, last hour), CSV, URL search (validation, `defaultStringifySearch`/`defaultParseSearch` round trip, preset resolution in a zone, 120-bucket guard reasons), perf bound (100 k cells, full page query < 50 ms), `resetIn` one-line labels.
- `src/components/costs/CostsPage.test.ts` (3): the page from the sim fixture renders every section synchronously with no spinner/skeleton/"Loading" and no literal colour; granularity guard and URL-driven group-by/metric; empty ledger.
- `src/components/hud/scene/Scene.test.ts`: SPEND action (`<a href="/costs" aria-label="Open costs">`, 14-unit rect, chart-line path, tokens only), big number and THIS MONTH → `/costs`, AGENTS still → `/agents`.

## Measured numbers

- **Client cube on a real-shaped cube** (native leg generated with sqlite3 from a `.backup` copy of the ledger, 3,558 cells, 143,611 calls, $18,257.86 — matches §3.4's ≈ 3.5 k rows / ≈ 143 k calls / ≈ $18.2 k): `index` 8.4 ms once per cube; the **whole page's queries** (series, 2 × totals, 5 breakdowns, options, heat, tokens, sessions) **1.27 ms median, 1.77 ms p95**; the main series 0.19 ms.
- **100 k cells**: index 89 ms; full page query 28 ms median (34 ms max); main series 5.5 ms (the vitest bound < 50 ms holds).
- **Interaction cost in the browser, first round** (production build, headless Chrome 1280 × 900, `?sim=costs`; `performance.now()` from the click to the last DOM mutation, median of 6; load average 5–30; a no-op measures 7.4 ms): group-by 49–56 ms, metric 48–55 ms, legend toggle 42–45 ms, range preset 150–170 ms, filter chip add/clear 136–144 ms.
- **R1 — per-step measurements** (below).
- IndexedDB snapshot round trip (fixture cube, 1,304 cells): save 5.5 ms, load 0.8 ms.
- `/costs` chunk: 528 kB (148 kB gzip), preloaded at idle after start and on SPEND hover/focus.

### R1 — getting a control change under one frame

Method: a production build with React's profiling renderer (`react-dom/profiling`, via a throwaway vite config) and temporary `<Profiler>`s (page + each panel; removed before commit), headless Chrome 1280 × 900, the real-shaped cube (3,569 cells from a fresh `.backup` copy; 30D / day / 8 series by model at the start), `?sim=costs-file`. Each control is clicked 6 times (cycling values); per click: **first** = `actualDuration` of the first commit (the control itself), **max** = the longest commit that click caused, **n** = commits; medians. Hover = 20 mouse moves across the main chart. The machine was shared with parallel Rust builds the whole time — `uptime` load averages are listed per run; absolute numbers move with them (step d ran during a spike).

| step | load (1 / 5 / 15 min) | range first / max / n | granularity | group-by | metric | filter chip | legend | hover commits × median (max) |
|---|---|---|---|---|---|---|---|---|
| before | 24.7 20.0 25.4 | 11.2 / 26.0 / 11 | 5.5 / 9.7 / 7 | 4.5 / 7.6 / 5 | 5.7 / 8.9 / 4 | 6.3 / 27.5 / 13 | 4.3 / 6.9 / 4 | 41 × 0.2 (0.8) |
| a — controls urgent, panels on `useDeferredValue` | 51.8 26.2 26.3 | 0.4 / 30.8 / 11 | 0.4 / 15.2 / 5 | 0.3 / 8.4 / 6 | 0.4 / 8.6 / 5 | 0.6 / 24.2 / 11 | 0.1 / 8.5 / 5 | 41 × 0.2 (1.3) |
| b — memo + stable configs, `When` drops `gran`, lower panels staged | 194.0 65.5 39.9 | 0.3 / 14.1 / 19 | 0.2 / 11.7 / 10 | 0.2 / 8.4 / 6 | 0.2 / 10.3 / 5 | 0.7 / 12.5 / 18 | 0.2 / 5.5 / 5 | 50 × 0.3 (16.1) |
| c — `accessibilityLayer` off on the heat grid (animations were already off) | 70.3 57.7 39.3 | 0.5 / 15.9 / 18 | 0.3 / 8.5 / 10 | 0.4 / 10.7 / 6 | 0.5 / 10.4 / 5 | 0.5 / 13.9 / 18 | 0.3 / 10.3 / 5 | 41 × 0.3 (1.5) |
| d — top 6 + other, ≤ 120 buckets, thin bars > 60 | 31.9 44.8 36.9 | 0.7 / 29.0 / 18 | 0.5 / 16.8 / 10 | 0.4 / 17.9 / 6 | 0.6 / 19.7 / 5 | 1.6 / 26.0 / 15 | 0.4 / 17.2 / 5 | 41 × 0.4 (2.5) |
| e — bars as one path per series | 18.2 31.8 33.0 | 0.6 / 25.9 / 18 | 0.4 / 6.1 / 9 | 0.3 / 7.1 / 6 | 0.5 / 7.1 / 4 | 0.9 / 26.3 / 18 | 0.2 / 5.5 / 5 | 41 × 0.3 (1.4) |
| e + per-card staging (final) | 12.2 22.2 28.6 | **0.4 / 14.6** / 41 | **0.3 / 5.7** / 10 | **0.3 / 6.8** / 6 | **0.3 / 5.0** / 4 | **0.7 / 12.0** / 36 | **0.2 / 4.9** / 5 | 46 × 0.3 (1.3) |

Read: the control itself now commits in 0.2–0.7 ms (was 4–11 ms); every commit a change causes is under 16 ms (was up to 27.5 ms); a granularity change no longer touches the limits panel, sessions table, ledger, KPIs or breakdowns (their profilers report 0 ms); hovering the chart commits only the tooltip (≈ 0.3 ms, never the bars). Total render work per change (sum over its commits): group-by 27.3 → 13.8 ms, metric 22.3 → 8.3 ms, legend 20.6 → 9.3 ms, granularity 33.6 → 15.1 ms; range (104 → 131 ms) and filter (90 → 113 ms) do the same work spread over 36–41 frame-sized commits, the last landing ≈ 390 / 540 ms after the click by design (stages are one frame apart). The main chart's DOM went from ≈ 240 `<rect>`s (30 × 8) to 7 `<path>`s.

R2 note: step (e) was reverted on instruction ("do not hand roll the charts") — the bars are stock Recharts `<Bar>`s again; the R1 table above is history.

### R2 — first paint of /costs, click → next frame, stock bars

Method: the production build against the integration engine (`/tmp/160-z/bin/t`, `127.0.0.1:7864`, fresh `.backup` copy of the ledger — the real `cost_cube`), headless Chrome 1280 × 900. **Before** = this branch at the R2 merge (`f140422d`, R1 code + closer fixes) built the same way; **after** = R2. Navigation: from the HUD, a real mouse click on the SPEND chart button, 10 times (back between), time from `pointerdown` to the first animation frame in which the Costs band, controls, KPI row and main chart are on screen (no section above the fold is ever a placeholder; the frame is the first paint). Clicks: 12 controls (6 ranges, 4 group-bys, 2 metrics) twice, `pointerdown` → next `requestAnimationFrame` (the closer's measure). Load averages during the final run: 25.8 27.9 34.2 → 90.1 48.8 41.0.

| measure | before p50 / p95 | after p50 / p95 |
|---|---|---|
| /costs navigation → first paint (10 navigations) | 265 / 513 ms | **29.5 / 33.9 ms** |
| click → next frame (24 clicks) | 8.2 / 10.4 ms here; the closer's run: 20.4 / 42.8 ms | **4.5 / 7.3 ms** |

How the first paint got there, measured on profiling builds along the way (warm navigations, p50): before ≈ 230–270 ms (one long task: the page mounts, then 4–5 nested Recharts passes of ≈ 10–27 ms each, with layout between); below-the-fold sections in idle slices + legend portal + exact initial width ≈ 120–140 ms (the main chart alone was ≈ 80 ms of it, KPIs ≈ 25 ms — Recharts builds a chart in several commits, so a chart-final first paint cannot be one short commit); resident page shown by a class flip ≈ 44 ms (a 35 ms restyle from toggling `visibility`); opacity / z-order / `inert` instead of `visibility` ≈ 38 ms on the profiling build, 29.5 ms on the normal build. The first navigation after a reload pays one extra slice (≈ 46 ms) when the pre-render has just finished. Clicks: the control commits in < 1 ms (its own `pending` state), the URL write and the panels follow after the frame.

100 k-cell query bound (vitest) after the R2 cube tweaks: series 4.9 ms, 2 × totals 4.3, 7 breakdowns 5.1, options 5.3, heat 0.6, tokens 4.0, sessions 1.1 (bun, medians) — the full page query stays under the 50 ms test bound with the 400-bucket cap back.

## Gates (exact commands, in `crates/desktop/ui`)

- `~/.theuth/bin/build-lock bun run check` → exit 0 (no `error TS`).
- `CI=1 ~/.theuth/bin/build-lock bunx vitest run src/lib/costs src/components/costs src/components/hud` → `Test Files  7 passed (7)` · `Tests  62 passed (62)` (R2).
- `BUILD_SLOTS=1 ~/.theuth/bin/build-lock env CI=1 bun run test` → `Test Files  271 passed (271)` · `Tests  1707 passed (1707)` (R2, with the integration tree's bindings: `api.test.ts` is green).
- `~/.theuth/bin/build-lock bunx vite build` → `✓ built in 23.09s` (R2; `CostsPage` chunk 529 kB, 148 kB gzip; the `/costs` route chunk is now 3 kB).

## Screenshots (local only, `/tmp/160-ui-shots/`, not committed)

Real data = scratch engine from `/Applications/theuth.app` on a `.backup` copy of the ledger (HUD) and the generated cube via `?sim=costs-file` (`/costs`); sim = `?sim=costs`. Each at 1280 × 900 (`desk`) and 390 × 844 (`phone`), soot and bone:
`hud-{desk,phone}-{soot,bone}.png`, `hud-spend-panel-desk-{soot,bone}.png` (the SPEND panel with the new button), `costs-top-…`, `costs-mid-…`, `costs-bottom-…`, `costs-custom-range-…`, `costs-filter-chip-…`, `costs-limits-sim-…`, and from R1 `costs-chart-phone-{soot,bone}.png` (the group-by strip at 390 px). R1 re-took every `costs-*` shot; R2 re-took them again on the production build against the integration engine (`/tmp/160-z/bin/t` on a fresh `.backup` copy, real `cost_cube` / `cost_calls` / `usage_limits`), plus `costs-7d-hour-{desk,phone}-{soot,bone}.png` (7D by Hour, 168 stock bars with the Brush); the HUD shots are unchanged. Every engine, preview server and data copy was removed afterwards (`/tmp/160-ui/`, `/tmp/160-ui2/`).

## Open items for the closer

1. Done by the closer: bindings regenerated (`api.test.ts` green in R2), `CostCubeChanged` from its binding (`bc0a4284`).
2. R2: re-measure the first paint and click → frame on Sky's machine when idle — the R2 numbers above were taken with load averages of 25–90.
3. Live proof items the UI could not exercise without ENG/LIM: delta merge on a real `CostCubeChanged`, cold reload painting from IndexedDB before the engine answers, `cost_calls` paging against the engine (exercised only against the fixture: 4 pages, no spinner), real limits rows.
4. `cache_saved_nanos` / `repriced` / `approx_nanos` are shown when the cube carries them (cache saved only when > 0); the real-shaped test cube had them at 0.
5. At 390 px nothing widens the page (an element scan found 0 boxes past the viewport outside intentional horizontal scrollers); the group-by strip scrolls with a faded edge.
