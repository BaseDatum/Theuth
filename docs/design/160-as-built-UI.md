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
| `src/lib/costs/cube.ts` | the client cube, pure: `merge` (full replaces; same-epoch delta replaces every cell of `hours`, upserts `roots` by `ix`, adopts dictionaries + meta; other epoch / nothing held → `null` = fetch full; stale rev → held), `index(cube, tz)` (typed columns; model collapsed to canonical name; owner/project via the root; per-cell local civil day, month and hour computed once per distinct UTC hour for the injected IANA zone), one-pass queries `series` (top 7 + other, previous-period line), `totals` (per basis), `previousPeriod`, `breakdown(dim)`, `options` (facets: per-option spend under every other filter), `heat` (7 × 24), `tokenSeries`, `sessions`, `toCsv`, `bucketCount` (≤ 400 guard). ISO weeks start Monday. |
| `src/lib/costs/search.ts` | `validateCostsSearch` (range / from / to / gran / by / metric / compare / 7 filter dims / `q`, drops junk and defaults, keeps `sim`), `resolveRange` (presets in the zone), `autoGran`, `granBlock` (reason text for a disabled granularity), `effectiveGran`. |
| `src/lib/costs/format.ts` | `usd` (sub-cent keeps 2 significant digits), `usdShort`, `compact`, `int`, `pct`, `ago`, `when`. |
| `src/lib/costs/persist.ts` | IndexedDB `theuth-costs` / store `cube` / key `v1:<engine>` (own 40-line wrapper: `link/idb.ts` is bound to the `link` db). |
| `src/lib/costs/queries.ts` | `CostsPrefetch` bridge (mounted once in `routes/__root.tsx` `Root()` next to `RoutineDraftBridge`): hydrate from IndexedDB, `cost_cube(since)`, delta on `CostCubeChanged` (one request in flight, a newer rev seen mid-flight refetches once), `Lagged` → delta, persist 2 s after the last change inside `requestIdleCallback`, `router.preloadRoute('/costs')` + `usage_limits` prefetch at idle. `useCostCube` / `useCostIndex` (index cached per cube × zone), `useUsageLimits` (15 s poll, paused when hidden; `usage_limits_refresh(null)` on mount and every 60 s visible), `useCostCalls` (infinite keyset pages, `keepPreviousData`). `?sim=costs` and the dev-only `?sim=costs-file`. |
| `src/dev/costs-fixture.ts` | synthetic deterministic fixture: 56 days ending now, 7 models (one alias pair), 3 providers, 7 kinds, 3 vias, owners you / 2 agents / 2 routines, 2 projects, a deleted root, a reported omp + opencode leg with approximate hours, an endless failure loop (unpriced), a Bedrock unpriced model; `cost_calls` over it with the same filters and keyset paging; `limitsFixture` with a provider row in every `LimitStatus` and two budgets. All invented. |
| `src/routes/costs.tsx`, `src/routeTree.gen.ts` | the `/costs` route (lazy chunk, `validateSearch`). |
| `src/components/costs/CostsPage.tsx` | band (title, live dot, `ledger since`, engine name, CSV), range bar, chips, KPIs, main chart, limits, then the deferred panels (`useDeferredValue`): breakdowns, when, tokens, sessions, ledger; coverage notes (H3, H4, H6) in the footer; empty ledger = one line. |
| `RangeBar.tsx` | presets Today · 24H · 7D · 30D · MTD · 90D · All, Custom popover (shadcn range `Calendar`, two months / one on phones, quick picks This week · Last week · Last month, "or drag across the chart"), granularity with the disabled reason in a tooltip. |
| `FilterChips.tsx` | Provider · Model · Owner · Kind · Via · Basis · Project chips, each a `Command` multi-select with per-option spend; Session title search; filled + removable when active; Clear all. |
| `Kpis.tsx` | six cards (`@number-flow/react`, fixed minimum size): Spend (Δ vs previous equal period, sparkline), Today (this hour, burn of the last 60 min, yesterday), month (MTD, projection at pace, last month), Calls (per call, failed, repriced), Tokens (output, cache hit, cache saved when > 0), Basis (known / reported / estimated bar, unpriced calls, "unpriced is a lower bound"). |
| `SpendChart.tsx` | stacked bars by Model · Provider · Owner · Kind · Via · Basis · Project, metric $ · Calls · Tokens, dashed previous period, drag-select → custom range (`ReferenceArea`), `Brush` past 40 buckets, bar click → filter, `ChartLegend` entries toggle the series and a filter icon beside each adds the filter. |
| `LimitsPanel.tsx` | one card per provider (RadialBar gauges warn ≥ 75 % / danger ≥ 90 %, rate meters, balance, status tag, `source · observed N ago`), budgets card, `unavailable` / `not configured` as one muted line each, one sentence when the engine cannot answer. |
| `Breakdowns.tsx` | by model / owner (agent ring, routine square ticks) / kind / project horizontal bars, provider donut + list; click → filter. |
| `When.tsx` | hour × weekday heat grid (Recharts `ScatterChart`, square `shape` sized from `usePlotArea` so it never overflows) + hour-of-day profile; `Tokens`: input mix 100 % area (empty buckets skipped) + output bars. |
| `SessionsTable.tsx`, `LedgerTable.tsx` | sortable sessions (deleted root = "Deleted session · <date>", not clickable; owner / model / project cells filter), first 50 + Show all; ledger virtualised with `@tanstack/react-virtual`, next page within 20 rows of the end, hairline only after 150 ms, rows → `/c/$session`. |
| `ui.tsx` | `Panel`, `Seg` (scrolls horizontally on phones), `Tag`, `Swatch`, `OwnerGlyph`. |
| `src/lib/files.ts`, `components/chat/DiagramFrame.tsx` | `saveBlob` moved from `DiagramFrame` to `lib/files` so the CSV export uses the same sink (browser download; desktop: attachment store + reveal). |
| `src/components/hud/scene/{Scene.tsx,model.ts,Scene.test.ts}` | `Target` `{ route: 'costs' }` (Link `preload="intent"`); `Panel` optional `action` drawn right after the title in the 352×248 space (14 × 14 hit square, chart-line icon, `<title>Open costs</title>`, `aria-label`); SPEND uses it; SPEND's big number and the footer's `$… THIS MONTH` go to `/costs` (were `/agents`). |

## Deviations from the design, and why

1. **`CostCubeChanged` is typed inline in `rpc/events.ts`**, not imported from a binding: its generated file name was not known in this tree. The closer can swap it for the ts-rs type once ENG's export exists (same `{ type, epoch, rev }`).
2. **IndexedDB key `v1:<engine scope id>`** (`'local'`, or the paired engine's id; the dev socket URL for a local dev engine). The connection store holds no engine URL; the scope id is stable per engine, and an epoch change replaces a stale snapshot anyway.
3. **Filter ids.** Model filters use the canonical `name` (aliases share one chip; `LedgerTable` maps names back to every raw id for `cost_calls`); owner ids are `<owner_kind>:<owner>` (an agent and a routine may share a name); project `''` = no project.
4. **Previous period** — the KPI delta compares the equal number of hours just before the range; the dashed line plots the equal number of buckets just before (so a partial first day compares with full days). Both documented in `cube.ts`.
5. **Granularity guard** — a choice is allowed when it gives ≤ 400 buckets and (except Hour) at least 2; the auto pick is §6.6's. A disabled choice explains itself (`816 buckets — more than 400…`, `one bucket — …`).
6. **Legend click vs §7.3** — §7.5 says the legend toggles series, §7.3 says a legend click adds a filter; the entry toggles, the small filter icon next to it filters.
7. **No palette entry** — D1 asks for `Costs` in the search/command palette "if it lists pages": `SearchDialog` is a content search with no page list, so nothing was added; no sidebar row (D1).
8. **KPIs follow the filters** (Today and the month cards too), so every number on the page answers the same question.
9. **Ledger range** — a range ending now is sent as `to_ms: null`, so the minute clock does not refetch loaded pages.
10. **An empty `full` answer while the engine is still building** never replaces a non-empty held cube (§3.3).
11. **Performance rule 5 is not met for the main chart in this environment** — see the numbers below; everything else in §6 is implemented.

## Tests

- `src/lib/costs/cube.test.ts` (10): merge (full, delta hours + root upserts + dictionaries, epoch change → `null`, nothing held, stale rev), local day / ISO week / month across the autumn DST change in `Europe/London` and `America/New_York` (25-hour days, Sunday ends the ISO week, 31 Oct 23:00 NY is October), the spring change (23-hour days, hour labels skip 01:00), filters (alias collapse, owner via root, project incl. none, title search incl. deleted roots, facets ignore their own dimension), top 7 + other with distinct colours, previous period (line and KPI window), totals per basis + breakdowns + heat cell placement in `Europe/Paris` + token mix + session rows (deleted label, dominant basis, last hour), CSV, URL search (validation, `defaultStringifySearch`/`defaultParseSearch` round trip, preset resolution in a zone, guard reasons), perf bound (100 k cells, full page query < 50 ms).
- `src/components/costs/CostsPage.test.ts` (3): the page from the sim fixture renders every section synchronously with no spinner/skeleton/"Loading" and no literal colour; granularity guard and URL-driven group-by/metric; empty ledger.
- `src/components/hud/scene/Scene.test.ts`: SPEND action (`<a href="/costs" aria-label="Open costs">`, 14-unit rect, chart-line path, tokens only), big number and THIS MONTH → `/costs`, AGENTS still → `/agents`.

## Measured numbers

- **Client cube on a real-shaped cube** (native leg generated with sqlite3 from a `.backup` copy of the ledger, 3,558 cells, 143,611 calls, $18,257.86 — matches §3.4's ≈ 3.5 k rows / ≈ 143 k calls / ≈ $18.2 k): `index` 8.4 ms once per cube; the **whole page's queries** (series, 2 × totals, 5 breakdowns, options, heat, tokens, sessions) **1.27 ms median, 1.77 ms p95**; the main series 0.19 ms.
- **100 k cells**: index 89 ms; full page query 28 ms median (34 ms max); main series 5.5 ms (the vitest bound < 50 ms holds).
- **Interaction cost in the browser** (production build, headless Chrome 1280 × 900, `?sim=costs`; `performance.now()` from the click to the last DOM mutation, median of 6; machine load average 5–30 from parallel builds; a no-op measures 7.4 ms): group-by 49–56 ms, metric 48–55 ms, legend toggle 42–45 ms, range preset 150–170 ms, filter chip add/clear 136–144 ms (range and filter include the deferred panels re-rendering). A CPU profile attributes it to Recharts rendering the main chart's ≈ 240 bar rectangles (30 buckets × 8 series) and its store dispatches, plus GC — not to the cube (≈ 1 ms). So **§6.5's < 16 ms is not met here for the main chart**; the cube side is well inside its budget. Done to cut it: range memo on primitives (not the search object), stable handlers so memoised panels skip, `hide` instead of unmounting series, one plot-area read for the 168 heat cells, deferred secondary panels.
- IndexedDB snapshot round trip (fixture cube, 1,304 cells): save 5.5 ms, load 0.8 ms.
- `/costs` chunk: 524 kB (147 kB gzip), preloaded at idle after start and on SPEND hover/focus.

## Gates (exact commands, in `crates/desktop/ui`)

- `~/.theuth/bin/build-lock bun run check` → exit 0 (no `error TS`).
- `CI=1 ~/.theuth/bin/build-lock bunx vitest run src/lib/costs src/components/costs src/components/hud` → `Test Files  6 passed (6)` · `Tests  60 passed (60)`.
- `BUILD_SLOTS=1 ~/.theuth/bin/build-lock env CI=1 bun run test` → `Test Files  1 failed | 269 passed (270)` · `Tests  1 failed | 1704 passed (1705)`; the one failure is `src/rpc/api.test.ts > METHODS equals the server method list` (`+ "cost_calls", + "cost_cube", …` — the generated `bindings/methods.json` lacks the four new methods until integration regenerates it), as expected.
- `~/.theuth/bin/build-lock bunx vite build` → `✓ built in 13.62s`.

## Screenshots (local only, `/tmp/160-ui-shots/`, not committed)

Real data = scratch engine from `/Applications/theuth.app` on a `.backup` copy of the ledger (HUD) and the generated cube via the dev-only `?sim=costs-file` (`/costs`); sim = `?sim=costs`. Each at 1280 × 900 (`desk`) and 390 × 844 (`phone`), soot and bone:
`hud-{desk,phone}-{soot,bone}.png`, `hud-spend-panel-desk-{soot,bone}.png` (the SPEND panel with the new button), `costs-top-…`, `costs-mid-…`, `costs-bottom-…`, `costs-custom-range-…`, `costs-filter-chip-…`, `costs-limits-sim-…` (28 + 2 files). The scratch engine, vite, the cube server and `/tmp/160-ui/` (data copy and cube JSON) were removed afterwards.

## Open items for the closer

1. Regenerate bindings: the hand-written stubs (`CostCube`, `CostCells`, `CostRoot`, `CostRootEntry`, `CostModel`, `CostReason`, `CostCubeMeta`, `CostCubeCursor`, `CostBasis`, `CostCalls{Query,Page,Cursor}`, `CostCall`, `UsageLimits`, `ProviderLimits`, `UsageWindow`, `RateLimit`, `Balance`, `BudgetLimit`, `LimitStatus`) must match ENG/LIM's ts-rs output; `api.test.ts` goes green with the new `methods.json`.
2. Optionally import `CostCubeChanged` from its binding (deviation 1).
3. Live proof items the UI could not exercise without ENG/LIM: delta merge on a real `CostCubeChanged`, cold reload painting from IndexedDB before the engine answers, `cost_calls` paging against the engine (exercised only against the fixture: 4 pages, no spinner), real limits rows.
4. Main-chart interaction cost (deviation 11): measure again on an idle machine; if still over budget, the cheapest levers are fewer stacked series by default (top 5 + other) or the Hour/Day bucket count, then Recharts-side (`throttleDelay`, fewer layers per bar).
5. `cache_saved_nanos` / `repriced` / `approx_nanos` are shown when the cube carries them (cache saved only when > 0); the real-shaped test cube had them at 0.
