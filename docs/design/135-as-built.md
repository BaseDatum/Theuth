# 135 STEADY-SCROLL — as built

*Branch `feat/135-steady-scroll` off main a1cbe9fa; contract `docs/design/135-steady-scroll.md`. UI only
(`crates/desktop/ui/src/**`). Gates green: `tsc -b --noEmit` 0 errors · vitest 190 files / 1312 tests passed, 1 skipped
(pre-existing) · `vite build` ok (3.2 s).*

## Files touched

Pure logic (plain `.ts`, vitest next to it):

- `src/lib/measured.ts` — §2 remembered heights. `bucket` :33, `heights` :49 (module `LruCache` of 32 sessions, a stored
  bucket ≠ width starts empty), `remember` :63 / `recall` :72, `persist` :77 (localStorage `theuth_heights:<session>` =
  `{ w, h }`, ≤ 4000 most-recent keys, one timer per session re-armed until ≥ 1 s after the last `remember`, quota
  errors swallowed), `MeasuredContext` :92 (`{ session, width }` the transcript provides to `Reserve`), `estimateText`
  :105 and `estimateRow` :153 (paragraph lines at 7.2 px/char × 26 px, fences by language, tables × 32 + 40, images 240,
  tool/run 56, prompt 160, gap 36, assistant action bar 44, floor 96). Tests `measured.test.ts` (bucket, LRU, persistence
  shape/cap/debounce/quota, estimate bounds and monotonicity).
- `src/components/chat/scroll-anchor.ts` — §3.2 `step(State, Event) → [State, Effect]` :21, every row of the table.
  Tests `scroll-anchor.test.ts` (each rule, the guard at exactly zero, shrink grows the slack, prepend larger than the
  slack, rest clamp, target 0, a streaming anchor row never writes).

Components:

- `src/components/chat/Transcript.tsx` — §3.3 wiring. `rowEntry`/`rowHas` :42, `anchorElement` :50 (deepest child
  straddling the top edge, depth ≤ 8), `setSlack` :68 (`paddingStart` change with a full position recompute, see
  deviations), `estimateSize` :202 (`recall ?? estimateRow`), anchor re-resolution by entry :210, prepend :216,
  `predicate` :235 (`grow` → `step`, `moved` for the anchor row, `queueMicrotask(flushSync)`), `useVirtualizer` :256 with
  `paddingStart: slack.current`, the predicate assigned as the instance field :268, pin effect :272 (unchanged) and the
  rest-write effect :280, `rest` :288 (remember `fresh` keys → `persist` → target → `step('rest')` → `setSlack` +
  `flushSync`), `armRest` :314, column-width ResizeObserver :321 (bucket, `fresh` reset, `v.measure()` on the first
  bucket), touch tracking, `track` :393 (rAF anchor), `onScroll` :411 (page trigger `scrollTop < slack + TOP`),
  `MeasuredContext` provider :442. `wrap-anywhere` on the scroller kept.
- `src/components/chat/Reserve.tsx` — §2 `Reserve({ id, ready, className, children })`: `min-height: recall(id)` while
  `!ready` (one-cell `grid` so the placeholder fills it), a ResizeObserver `remember`s the border-box height when ready;
  `id: null` or no `MeasuredContext` → plain div. Test `Reserve.test.ts` (reserves, not when ready/unknown/other
  bucket/null/outside a transcript).
- `src/components/chat/DiagramFrame.tsx` :267–285 — reads `FenceContext.entry`, wraps the content box in
  `Reserve id={entry/name}` (not in `fill` mode), placeholder keeps `min-h-28` as the floor.
- `src/components/chat/Mermaid.tsx` :71–101 — `svgCache` (`LruCache<string>(64)` of `fnv(chart):epoch` → svg, closed
  fences only), `useRender`'s initial state is the hit, a hit skips the render queue; `useSyncExternalStore` got its server
  snapshot (static render). Test `Mermaid.test.ts` (placeholder first, cached chart has its svg on the first render).
- `src/components/chat/MessageAssistant.tsx` :37–54, :229 — `Img` (`components.img`): `Reserve` keyed `<entry>/<src>`,
  `ready` on load/error/already complete; `FenceContext.entry = id`.
- `src/components/chat/surface/JsonRenderFence.tsx` :14 — `FenceContext.entry?: string`.
- `src/components/chat/HtmlPage.tsx` :29, :106, :116, :148 — the last `size-changed` height per `callId` in a module map;
  a remount starts at it instead of the 240 px reserve.
- `src/lib/injected.ts` :35, :68, `src/components/chat/HarnessCards.tsx` `WakeCard`, `src/lib/injected.test.ts` — the
  wake's task id was spread over the entry id (`{ id: nextId(), ...c }`), so every wake row for one task shared a React
  key; the field is now `task`. Found by the smoke below (duplicate keys corrupt the virtualized rows: React kept stale
  nodes at stale transforms). Pre-existing on main.

## Commit-before-paint (§3.3)

The microtask: the predicate applies the slack and calls `queueMicrotask(() => flushSync(bump))`. A microtask after the
ResizeObserver callback runs before the frame paints; on the `measureElement` ref path it runs after React's commit,
where `flushSync` is legal (the adapter itself guards `flushSync` with `measuringFromRef` only for its *synchronous*
call). React keeps owning the DOM, nothing to remove. `directDomUpdates` was not needed. Verified flicker-free in the
browser smoke (below): no element under a fixed point moved by even 1 px across three prepends and ~190 absorbed
resizes while the DOM was sampled every 120 ms.

The `rest` path uses `flushSync(bump)` directly from the timer, with the compensated `scrollTop += write` in a
layout effect of the same commit (skipped when pinned: the pin effect already owns the position and the bottom moved by
the same amount).

## Deviations from the contract

1. **`shouldAdjustScrollPositionOnItemSizeChange` is an instance field, not an option** in 3.17.11
   (`Virtualizer.shouldAdjustScrollPositionOnItemSizeChange`, d.ts :118; `useVirtualizer` rejects it as an option). It is
   assigned on the instance after every render so it closes over the current rows and anchor.
2. **`getMeasurements` is private; positions are incremental.** `getMeasurements` recomputes from `pendingMin` (the lowest
   row resized since the last pass), so a `paddingStart` change applied inside `resizeItem` — before the library sets
   `pendingMin = index` — would never reach rows above `index`. `setSlack` brackets `setOptions` with two public
   `getTotalSize()` reads: the first drains a pending partial pass at the old padding, the second runs the full pass at
   the new one (`pendingMin` is null by then). One O(rows) Float64Array pass per absorbed change.
3. **`rest` carries `scrollTop`, and the slack always lands on `target`.** The contract's "write clamped so the new
   `scrollTop ≥ 0`, `slack += write`" would strand a residual blank above the oldest row whenever the reader is inside
   the slack when `more` ends (a fling that out-runs the last page). Built: `write = max(target − slack, −scrollTop)`,
   `slack = target`; when the viewport top is in the blank slack the rows slide up into view (what was visible was
   blank), everywhere else positions and `scrollTop` move together.
4. **Anchor and prepend are keyed by entry id, not row key.** Row keys change when a page's last tool calls fold into the
   first row's run (`foldRuns`): the old first key vanishes, so neither the contract's `rows[0].key` test nor the
   library's `anchorTo: 'end'` key anchor can find it. The anchor stores the row's first entry id and re-resolves through
   `rowHas`; the prepend is detected by `entries[0]` and, when the first row was re-keyed, counts its new estimate over its
   old measured size in `estimated`. Smoke-verified on a session whose page boundary split a run (first prepend was
   un-absorbed and un-anchored before this).
5. **No `reserve` prop on `DiagramFrame`, no change to the 19 Views.** Every caller already passes `name` = a content
   hash (`diagramName(code)` is fnv1a; the Views pass `<kind>-<structuralHash|fnv>`), so `DiagramFrame` builds the key
   `<FenceContext.entry>/<name>` itself. Same key semantics as the contract (two identical fences share a height).
6. **Markdown `img` loses streamdown's download control.** Streamdown's built-in image component is not exported, so the
   `components.img` override renders a plain `<img className="max-w-full rounded-lg">` inside `Reserve`.
7. **`ArtifactCard` text**: its preview slot is a fixed 56×72 box — no height change, nothing to reserve (contract said
   "check; skip if not").
8. **Remember pass walks `fresh`, not the whole `itemSizeCache`.** After a width-bucket change, `itemSizeCache` still
   holds unmounted rows' heights from the old width; remembering them under the new bucket would poison it. Every size
   change passes through the predicate, so `fresh` (keys resized since the last pass, cleared on a bucket change) is
   exactly the set that is valid at the current width.
9. **First bucket**: the column's width is only known after the first measurement pass, so the first bucket calls
   `v.measure()` once (public: clears sizes, bumps the version, full recompute) so the remembered heights drive
   `estimateSize` from the first frame; mounted rows are re-measured by the library's ResizeObserver in the same frame.
10. The `Reserve` "records when ready" half runs through a ResizeObserver; there is no DOM in the test runner
    (`react-dom/server` only, as the repo's other component tests), so that half is covered by the browser smoke
    (`P315(315px)` below), not a vitest.

## Smoke (dev browser against the running engine, `VITE_THEUTH_URL=ws://127.0.0.1:55301`, headless Chromium 1200×800)

Scripted fling: `scrollTop −= 600` every 120 ms, sampling the element under a fixed viewport point before/after each
120 ms pause; a shift is |Δtop| > 0.5 px.

- Session with 161 rows across three pages, **first visit** (no remembered heights, every row first-measured): 63
  samples, **0 shifts**; the frame log shows the slack absorbing first-measure deltas (`paddingStart` 18554 → 20828 →
  19405 → 17938 with `scrollHeight` and `scrollTop` constant) and the first prepend absorbed in full (17938 → 11278, 31
  rows, `scrollTop` unchanged). Before the wake-key and entry-id fixes the same run showed shifts of 7153 / 1781 / 8179 px.
- Same session, **second visit**: `scrollHeight` 36093 → 36093 over 40 steps (every row recalled exactly), 0 shifts;
  dev counter `absorbed 36 px` for the whole pass.
- Session with 352 json-render fences (graph/sequence blocks), first visit: 66 samples, 0 shifts, 0 console errors.
  Graph block round trip (mount → settle 315 px → scroll 15 000 px away, unmount → back): frames
  `P315(315px) S315 S315 …` — the placeholder frame already has `min-height: 315px` from `Reserve`, the svg lands at 315.
  `theuth_heights:<session>` holds the row keys plus `e752/sequence-8ce1b7e1: 434`, `e853/graph-2aa19262: 251`, … at
  `w: 768`.
- `more: false` session (265 rows): loads pinned (`scrollTop = max`); after a fling to the top `paddingStart` 0, row 0 at
  `translateY(0)`, `scrollTop` 0 — no blank above the oldest row.
- Resize 1200 → 820 → 1200 px: column 768 → 560 → 768, mounted rows contiguous (0 gaps) each time, no errors.
- Out-running a page into the slack: the rest after the page lands writes the compensated refill
  (`scrollTop` 6184 → 34231 with `scrollHeight` +21017 … i.e. positions and offset together), no shift sampled.

Not exercised here: iPad (no touch device; the touch listeners and the ≥ 100 ms-after-`touchend` rule are untested),
streaming (no turn was sent to the user's engine), the mermaid cache in the browser (no session on this engine has a
` ```mermaid ` fence; the unit test covers the first-render hit).

## How to verify by hand (design §5)

1. Open a long session (one of the DR ones), fling back through 20 turns with diagrams: nothing under the pointer moves
   while blocks render; return to a region you passed — diagrams are there instantly (no "Rendering diagram…").
2. iPad: same fling — no drift during the fling, no jump when it stops; let a page load mid-fling.
3. A short, fully loaded session: the oldest row sits at the very top, nothing blank above it.
4. While the agent streams: the view stays pinned; scroll up to read an earlier message — the viewport does not drag.
5. Resize the window / toggle the sidebar: the transcript re-flows and settles without a stuck blank region.
6. Quit, reopen, open the same session: scrolling back has no first-visit drift (heights come from
   `localStorage theuth_heights:<session>`).
7. Dev build only: the console prints `steady-scroll: absorbed N px / wrote M · estimates E · renders R` at each rest —
   `absorbed` grows while you fling, `wrote` is the compensated rebase, `estimates` stays flat after the first pass (R1).

---

## Revision 1 — SMOOTH (as built)

*Branch `feat/135b-smooth-scroll` off main 8cd9c323 (contract commit 8340ac6d); contract `135-steady-scroll.md` §Revision 1.
UI only. Gates green: `tsc -b --noEmit` 0 errors · vitest 206 files / 1419 tests passed, 1 skipped (pre-existing) · `vite build`
ok (17.4 s, 14 012 modules).*

### Files touched

- `src/lib/measured.ts` — R1.1 `counters` :153 (DEV `estimates` = cache misses), `estimated` :155 (`WeakMap<entry object,
  { width, px }>`), `estimateRow` :158 reads the cache before `estimateText`. Test `measured.test.ts` "one computation per
  entry object and width" (same entry object → one computation, a spread row wrapper hits, a new width or a new entry object
  recomputes).
- `src/components/chat/lookahead.ts` (new) — R2.1 window math: `AHEAD = 2`, `BEHIND = 1`, `REST = 1.5` viewports;
  `window(top, h, dir, total) → [from, to]` px, clamped to `[0, total]`. Test `lookahead.test.ts` (both directions, rest,
  clamps).
- `src/lib/diagram/elk.ts` :181 — R2.3 `elkPeek<T>(hash)`: the LRU read, no promise.
- `src/lib/graph-layout.ts` :126 — `useElkLayout` initialises `shown` from `elkPeek(hash)` when `enabled` (:128); `have`
  :146 skips the queue request for a hash that is already shown with a layout. Test `lib/diagram/elk.test.ts` (ELK mocked as
  an echo, `document.fonts.ready` stubbed): `elkPeek` undefined → defined after `elkLayout`; a `Probe` component rendered
  with `react-dom/server` reads `miss` before and `hit` after the layout is cached.
- `src/components/chat/Transcript.tsx`:
  - R1.2 `getItemKey` :206 (`useCallback` on `[rows, width]`), `known` :207 (the session's `Heights.h` hoisted once per
    `[session, width]`), `estimateSize` :208 (`known.get(key) ?? estimateRow`). R1.5 `stats.renders` :216.
  - R2.1 `rangeExtractor` :269 (stable, reads `vRef.current`: `scrollOffset`, `scrollRect.height`, `scrollDirection`,
    `getTotalSize`, `getVirtualItemForOffset` at both edges, union with the library's `startIndex..endIndex`); `overscan: 0`
    :284.
  - R1.4 `track(settled)` :318 — the anchor row from `getVirtualItemForOffset` every frame; the element walk only when
    `|scrollTop − last tracked| < clientHeight / 4` or `settled`; otherwise `anchorEl = null`, `offset 0` (→ `moved: 0`).
  - R1.3/R1.5 `rest` :341 — `track(true)` first, `remember` the `fresh` keys (no `persist`), the DEV line now
    `steady-scroll: absorbed N px / wrote M · estimates E · renders R` at every rest.
  - R2.2 two priorities :470–516 — `lo..hi` = `v.range` ± 1 rendered from the fresh `items`; `ahead` = the keys of the window
    outside `lo..hi` (identity kept while the list is unchanged); `deferred = useDeferredValue(ahead)`; each deferred key is
    resolved through `indexOf` (`Map<key, index>` memoised on `rows`) to `v.measurementsCache[i]`, so a deferred row always
    carries the current `data-index` and `translateY`; a key now inside `lo..hi` is skipped there (it renders urgently).
    `cell(item, row)` :477 is the one row renderer for both lists.

### Window constants and shape

`[top − behind·h, top + h + ahead·h]` with `(behind, ahead) = (1, 2)` scrolling forward, `(2, 1)` backward, `(1.5, 1.5)` when
`scrollDirection` is null. Measured in the browser (`clientHeight` 664): forward → 678 px mounted above the viewport, 1 336
below; backward → 1 410 above, 694 below; rows contiguous in every sample. The library runs the extractor only when the
visible range changes (its memo keys on `rangeExtractor`, `overscan`, `count`, `startIndex`, `endIndex`), so the rest window
applies on mount, after a jump and after any range change while `scrollDirection` is null — a plain stop keeps the last
biased window (a superset of what is needed; nothing re-mounts).

### Deviations from the contract

1. **Only entry rows are cached in the `WeakMap`.** Run, prompt and gap rows are constants (96 / 160 / 96 after the floor);
   caching a constant would cost more than computing it. `counters.estimates` counts entry computations only.
2. **`getItemKey` also depends on `width`.** The library's measurement memo keys on `getItemKey` only (not `estimateSize`);
   a stable key extractor alone would leave every unmounted row's estimate at the old bucket after a resize. Depending on
   `width` makes the full pass run exactly when the contract says: when rows or the width change.
3. **`useElkLayout` skips the queue request on a hit.** The contract says "initialises `shown` from `elkPeek`"; without the
   skip the mount effect would still request the same hash and `setShown` an identical layout one frame later (an extra
   render per remount). Misses, streaming (hash changes) and time-outs are unchanged.
4. **The deferred value is a list of keys, not of `VirtualItem`s.** Deferring the items would mount a row with a stale
   `data-index` after a prepend, and `measureElement` resolves the key through `data-index` — a resize observed in that
   window would be written under the wrong key. Keys are stable; positions are read fresh.
5. **No non-ELK View changed.** Survey of the 19 Views + `Graph`/`Sequence`: the six ELK-family Views (Graph, StateMachine,
   Entities, Swimlane, GraphDelta, Tree nodes|org) all go through `useElkLayout`; Sequence, Sankey, every chart, Tree nested,
   Heatmap/Kanban lay out synchronously in render — no first-frame gap, nothing to do (contract R2.3 last sentence).

### Perf smoke (dev server `VITE_THEUTH_URL=ws://127.0.0.1:53379`, headless Chromium 1200×800, read-only; `theuth_heights:*`
cleared before every run, so every run is a first visit)

"Before" = the contract commit's `Transcript.tsx` / `measured.ts` / `graph-layout.ts` with a temporary (uncommitted)
`estimateRow` call counter and render counter; "after" = branch tip, numbers from the DEV `steady-scroll` line. Load the
session, page back 8× (scroll to 0, wait for `scrollHeight` to grow), rest at the bottom (first pass), 40 steps of
`scrollTop −= 600` every 120 ms, rest; then from the bottom a 60-step fling at ~2 500 px/s (−300 / 120 ms) and one at
~5 000 px/s (−600 / 120 ms), each sampled every rAF (`PerformanceObserver` `longtask`, frame intervals, the lead test on
every row intersecting the viewport: "Rendering diagram…" text, a `Reserve` holding `min-height` (not ready), an `img` with
`!complete`) and the 135 shift sampling (`elementFromPoint` 30 ms after each step, its `top` again 90 ms later; shift =
|Δ| > 0.5 px). The machine was shared with other builds (load ~40) during every run.

(a) `estimates` (session `01a0c956` "DR failover", 556 rows loaded before / 479 after; `01a0d418` "Decision models", 640 / 682 rows):

| | after first pass | after 40 steps | after both flings |
|---|---|---|---|
| before, DR | 170 609 calls (556 renders) | 271 772 (796 renders) — **+421 per render** | 407 192 (1 208 renders) |
| after, DR | **354** (554 renders) | **354** (890 renders) | **354** (1 518 renders) |
| before, Decision models | 212 316 (552) | 356 005 (848) | 507 827 (1 290) |
| after, Decision models | **537** (610) | **537** (1 020) | **537** (1 762) |

Flat after the first pass, as required. (Renders are higher after: the deferred lookahead commits are counted too.)

(b) `longtask` count / total ms and p95 frame interval over the 60-step fling:

| session, speed | before | after |
|---|---|---|
| DR, 2 500 px/s | 15 tasks / 1 117 ms · p95 41.3 ms · max 101 | 10 / 710 ms · p95 25.0 ms · max 108 |
| DR, 5 000 px/s | 18 / 1 532 ms · p95 34.1 · max 193 | 10 / 885 ms · p95 26.5 · max 184 |
| Decision models, 2 500 | 15 / 862 ms · p95 33.3 · max 84 | 9 / 638 ms · p95 24.6 · max 75 |
| Decision models, 5 000 | 23 / 1 818 ms · p95 49.7 · max 159 | 16 / 1 209 ms · p95 48.6 · max 209 |
| Diagram lowerings (`01a11356`, 239 rows = whole history), 2 500 | 0 / 0 · p95 24.9 · max 43 | 0 / 0 · p95 25.0 · max 42 |
| Diagram lowerings, 5 000 | 8 / 499 ms · p95 33.4 · max 91 | 2 / 124 ms · p95 25.1 · max 75 |

p50 is 16.7 ms everywhere (60 Hz). The max frame is noise from the shared machine (one ~180 ms task in both DR runs).

(c) Lead frames (a visible row showing "Rendering diagram…" / an un-ready `Reserve` / an incomplete `img`): **0 in every
run at 2 500 px/s, before and after.** At 5 000 px/s before: 1 frame (Decision models: placeholder + reserve), 1 frame
(Diagram lowerings: placeholder); after: **0 / 0**. Frames with an uncovered part of the viewport (a gap between mounted
rows, outside the slack): 0 everywhere. The 2 500 px/s target was already met by `overscan: 4` on this machine; the
difference shows at twice the speed.

ELK first frame (Diagram lowerings, Graph block of 10 svgs, row height 1 324): mount → scroll 5 viewports + 3 000 px away
(row unmounted) → back; the `MutationObserver` on the rows container saw `S10:1324 S10:1324` — the svgs and the full
height on the first DOM frame, no placeholder (the 135 smoke saw `P315(315px) S315` for a Graph block round trip: a
placeholder frame at the reserved height, then the svg — `elkPeek` removes the `P` frame).

Zero-shift re-run (R3): 60 + 60 samples per session × 3 sessions, after: **0 shifts** everywhere, 0 console errors. Before:
0 everywhere except one 713 px sample in Diagram lowerings at 5 000 px/s when the fling out-ran the whole history to
`scrollTop 0` — the end-of-history slack removal of deviation 3 above (the rows slide up into blank slack), not a content
shift; the after run of the same fling sampled 0. Prepends mid-fling (DR with one page loaded, pages landing during both
flings, `scrollHeight` 15 776 → 60 766): 120 samples, 0 shifts; `estimates` grew only with the new rows (16 → 109 → 138).

### Not exercised

iPad / touch (no device); streaming and sending (read-only against the user's engine — the "sending messages" half of the
report is covered by the same fix: a streaming delta re-renders the transcript, and the pass is now a Map walk with zero
`estimateRow` calls); the mermaid cache in the browser (no mermaid fence in the sessions' loaded ranges; the 135 unit test
stands); `useDeferredValue` yielding under a real pointer fling (the scripted fling steps `scrollTop` every 120 ms, so the
deferred commits had idle time between steps — the headless profile shows the lower longtask totals, not interrupted
transitions).
