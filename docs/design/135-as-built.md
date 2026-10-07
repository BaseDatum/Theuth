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
7. Dev build only: the console prints `steady-scroll: absorbed N px / wrote M` at each rest — `absorbed` grows while
   you fling, `wrote` is the compensated rebase.
