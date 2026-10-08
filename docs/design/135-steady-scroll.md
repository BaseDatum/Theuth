# 135 STEADY-SCROLL — nothing the reader is looking at moves

*Status: contract, 2026-10-07. Amends the transcript of 25 §5 / 48 §5 / 65 §4. Branch `feat/135-steady-scroll` off main a1cbe9fa. UI only (`crates/desktop/ui`).*

## 0. The problem

The user's words (2026-10-07): "in the desktop/mobile app of theuth, sometimes on long sessions, as I scroll
back, things will render and that will shift the location I'm scrolled to as they render, sometimes a lot.
this is a frustrating UX and we need an elegant solution to it."

Three verified facts make the shift (read on main a1cbe9fa; `@tanstack/virtual-core` resolves to 3.17.11 under
`@tanstack/react-virtual` ^3.14.13):

1. `components/chat/Transcript.tsx` virtualizes rows with `estimateSize: () => 96`, `overscan: 4`, keys = entry
   ids, `anchorTo: 'end'`. Each row is measured by the library's ResizeObserver (`measureElement`). A row that
   scrolls out of the overscan window unmounts; scrolling back mounts it again and its contents render again.
2. The library compensates a size change above the fold only sometimes (`virtual-core/dist/esm/index.js`
   `resizeItem`, ~896–965): a **first** measurement is compensated whenever `item.start < scrollOffset`; a
   **re-measurement** only when the row ends above the fold **and `scrollDirection !== 'backward'`** (their
   #1218 fix). On **iOS WebKit** (`isIOSWebKit()`: iPhone/iPad UA, or MacIntel with touch points — the iPad app,
   not the Mac app) every compensation is deferred while scrolling or touching (`_iosDeferredAdjustment`) and
   flushed at rest, because a `scrollTop` write stops UIScrollView's deceleration. Prepends (`anchorTo: 'end'`,
   `setOptions` ~312–376) are compensated by the same kind of write, so they are deferred on iOS too.
3. Contents re-render asynchronously on every mount. `Mermaid.tsx` `useRender` has no cache: `svg` starts
   `null`, `DiagramFrame` shows "Rendering diagram…" at `min-h-28` (112 px), then the real svg lands (hundreds
   of px). json-render diagram Views lay out through `lib/diagram/elk.ts` (LRU 64, but a hit still resolves
   through a promise — one placeholder frame). Markdown images have no intrinsic size until loaded.
   `ArtifactCard` text is `null` while loading. `HtmlPage` is the good citizen: its frame is `style={{ height }}`
   from `show{height}` before the page loads.

So: scrolling up re-mounts rows that then grow by hundreds of px; on the Mac the re-measurement is not
compensated while scrolling backward (the content under the pointer shifts); on the iPad the compensation is
deferred (the content shifts during the fling, then jumps at rest). A page prepend while flinging on the iPad is
the biggest single jump.

## 1. Principle

Two layers, both small, both testable without a browser:

- **Remember.** Nothing that has been measured once should have to be measured again to find its size. Rows
  keep their heights per session and width across remounts and app runs; an async surface reserves its last
  height while it renders again; a mermaid chart keeps its svg. Most shifts never happen.
- **Absorb, never fight.** A size change above what the reader is looking at is absorbed by shrinking a
  **slack** region at the top of the virtual space, so nothing visible moves, the total height does not change
  and `scrollTop` is never written while the user scrolls. The slack is refilled (**rebased**) with one
  compensated `scrollTop` write only when the scroller is at rest, where a write is harmless on every platform.
  The library's own heuristics are switched off through its public `shouldAdjustScrollPositionOnItemSizeChange`.

Nothing in this design touches the engine, the RPC or the bindings.

## 2. Remembered heights — `src/lib/measured.ts`

```ts
/** Heights measured in one session at one content width: rows by entry id, surfaces by `<entry>/<hash>`. */
export type Heights = { width: number; h: Map<string, number> }
export function bucket(contentWidth: number): number            // Math.round(w / 16) * 16
export function heights(session: string, width: number): Heights // module LRU of ≤ 32 sessions; a stored
                                                                 // bucket ≠ width → start empty (never restore)
export function remember(session: string, width: number, key: string, px: number): void
export function recall(session: string, width: number, key: string): number | undefined
export function persist(session: string): void                   // localStorage `theuth_heights:<session>`
                                                                 // `{ w, h: Record<key, px> }`, ≤ 4000 most-recent
                                                                 // keys, written debounced ≥ 1 s after the last
                                                                 // `remember`; quota errors are swallowed
export function estimateRow(row: Row, contentWidth: number): number
```

`estimateRow` is the content-based guess for a row never measured: text rows `pad + lines × lineHeight` with
`lines = Σ per paragraph ceil(chars / charsPerLine(contentWidth))` (calibrate `charsPerLine` from the body font,
~7.2 px per character at 15 px); fenced blocks by language: `mermaid` / `json-render` 320, `openui` 240, code
`lines × 20 + 40`, markdown tables `rows × 32 + 40`, an image 240; tool-run rows 56, prompt cards 160, gap rows
36; floor 96. Pure, tested, monotonic in text length. It is a *guess* — the point is to make the first-measure
delta small, not zero.

The virtualizer's `estimateSize` becomes `(i) => recall(session, width, rows[i].key) ?? estimateRow(rows[i], width)`.
After every measurement pass (`onChange`, debounced to rest), every `v.itemSizeCache` entry is `remember`ed (the
map is a public field of `Virtualizer`; `measurementsCache` likewise). `width` is the content column's client
width (the `md:max-w-[768px]` div), observed with a ResizeObserver; a width change (sidebar, window, rotation)
changes the bucket, so stale heights are not restored and the library re-measures as today.

**Surfaces.** `FenceContext` (`surface/JsonRenderFence.tsx`) gains `entry?: string` (the assistant message id;
`MessageAssistant.tsx` provides it). A surface's key is `<entry>/<fnv1a(source)>` — two identical fences in one
message share a key and a height, which is right. A small component carries the reservation:

```tsx
/** Reserves the last measured height of an async block while it renders, then records the settled height. */
export function Reserve({ id, ready, className, children }: { id: string | null; ready: boolean; … })
```

`min-height: recall(id)` while `!ready`; when `ready`, a ResizeObserver (or `offsetHeight` in a layout effect)
`remember`s the box's height. `id: null` (no entry in context — artifact viewers, boards) renders a plain div.
Applied in:

- `DiagramFrame.tsx`: new prop `reserve?: string`; the "Rendering diagram…" placeholder keeps `min-h-28` as a
  floor and takes the remembered height; `ready = content !== null`. Every diagram (mermaid and the 19
  json-render Views) passes through here, so this is the one seam; the json-render Views need only pass the
  key (`diagram-hosts.ts` / `registry.tsx` know the fence source — route the key through the same context).
- `Mermaid.tsx`: a module-level `LruCache<string>` (reuse `lib/diagram/elk.ts`'s class) of
  `fnv1a(chart) + ':' + epoch` → svg, 64 entries; `useRender`'s initial state is the cache hit, so a remounted
  chart paints its svg synchronously at the right height; the render queue stays for streaming and theme flips.
- `MessageAssistant.tsx`: an `img` component that wraps the image in `Reserve` keyed by `<entry>/<src>`
  (`ready` on load/error); `ArtifactCard` text loading likewise if it changes height (check; skip if not).
- `HtmlPage` already reserves; also `remember` the height the page reports so a reopened page starts at it.

## 3. The slack — `src/components/chat/scroll-anchor.ts` + `Transcript.tsx`

### 3.1 State and target

`slack` is the virtualizer's `paddingStart` (a ref read on every render, so `setOptions` from the hook and from
React agree). Its target:

```
S = more && content > viewport ? clamp(PAGE × avgRow, 2 × clientHeight, 40 000) : 0
```

where `avgRow = (totalSize − slack) / count` and `content = totalSize − slack`. The slack lives exactly where
the next page will land, so it is blank only when the reader out-runs a page fetch (today they hit the top and
bounce; same visibility). With `more === false` there is no slack: the oldest row sits at y = 0 and growth above
the fold falls back to the library's write (rare after §2). A short page that cannot scroll has no slack
either, and the existing "fetch until it scrolls" effect compares `totalSize − slack` to `clientHeight`.

### 3.2 Pure core

```ts
export type Anchor = { index: number; offset: number }        // row under the viewport's top edge, and the
                                                              // anchor element's top relative to the scroller
export type State = { slack: number; target: number; anchor: Anchor | null }
export type Event =
  | { kind: 'grow'; index: number; delta: number; moved: number }   // a row resized; `moved` = how far the anchor
                                                                     // element moved in the DOM (0 unless index
                                                                     // === anchor.index)
  | { kind: 'prepend'; estimated: number }                           // sum of estimateSize over the new leading rows
  | { kind: 'rest' }                                                 // scroller at rest: rebase toward target
  | { kind: 'target'; target: number }
export type Effect = { slack: number; write: number; library: boolean }
export function step(s: State, e: Event): [State, Effect]
```

Rules (`library: true` means "let the library's own adjustment run", i.e. the predicate returns `true`):

| Event | Rule |
|---|---|
| `grow` with `index < anchor.index` | the whole delta moves the anchor row at commit: `slack −= delta` if `slack − delta ≥ 0` (a shrink grows the slack), `write 0`, `library false`; else `library true` (slack exhausted — on the Mac the library writes exactly, on the iPad it defers; both are today's behaviour). |
| `grow` with `index === anchor.index` | only `moved` matters (growth below the anchor element is content the reader has not reached): `slack −= moved` under the same guard; if not absorbed, `anchor.offset += moved`. Never `library true` here — the library's rule would drag the viewport on every streaming delta (#1218). |
| `grow` with `index > anchor.index` or no anchor | nothing. |
| `prepend` | `slack = max(0, slack − estimated)`; `library false`. The remainder (`estimated − absorbed`) is handled by `anchorTo: 'end'` automatically, because the library computes its anchor from the new measurements that already include the reduced `paddingStart`. |
| `rest` | `write = target − slack` clamped so the new `scrollTop ≥ 0`; `slack += write`. The write is compensated: positions and `scrollTop` move together before paint, nothing visible changes. |
| `target` | store; a rest event applies it. |

Tests (`scroll-anchor.test.ts`): each row of the table, the guard at exactly zero, shrink growing the slack,
prepend larger than the slack, rest with clamp, target 0 when `more` ends, a streaming anchor row (`moved 0`
repeatedly) never writes.

### 3.3 Wiring (`Transcript.tsx`)

- `useVirtualizer({ …, paddingStart: slack.current, estimateSize, shouldAdjustScrollPositionOnItemSizeChange: predicate })`.
  The predicate maps `(item, delta, v)` to a `grow` event (`item` carries the row's *old* start/size; use
  `item.index`), applies the effect — `slack.current = e.slack; v.setOptions({ ...v.options, paddingStart: e.slack })`
  — and returns `e.library`. `setOptions` inside the predicate is safe: the count is unchanged so the
  `anchorTo` block does nothing, and `getMeasurements` re-derives from `paddingStart` on the next read.
- **Commit before paint.** The library calls `notify(false)` after our predicate returns `false`, which is an
  async re-render. The new positions must land in the same frame: either `queueMicrotask(() => flushSync(bump))`
  from the predicate (a microtask runs after the ResizeObserver callback or after React's commit — the
  `measureElement` ref path — and before paint; `flushSync` is legal there), or the adapter's
  `directDomUpdates: true` (the adapter then owns the row transforms and the container height; remove our
  `style={{ transform }}` and `style={{ height }}`). Pick the one that is flicker-free in a real build; the
  microtask is the recommendation because React keeps owning the DOM.
- **Anchor tracking.** On every scroll event, in a rAF (once per frame): `row = v.getVirtualItemForOffset(scrollTop)`;
  inside its element (`v.elementsCache.get(key)` or `[data-index]`), descend to the deepest child whose box
  straddles the scroller's top edge (cap depth 8, skip zero-height nodes); `anchor = { index, offset }` with
  `offset = rect(el).top − rect(scroller).top`; keep the element in a ref for the `moved` measurement. In the
  predicate for the anchor row: `moved = (rect(el).top − rect(scroller).top) − offset`. Layout is clean inside a
  ResizeObserver callback, so these reads do not force layout.
- **Prepend.** In render, before `useVirtualizer`: if `rows[0].key` changed and the previous first key is at
  index `k > 0`, `estimated = Σ_{i<k} estimateSize(i)`; apply the `prepend` effect to `slack.current` so the same
  `setOptions` carries the new count and the reduced `paddingStart`.
- **Rest.** A 200 ms timer reset on every scroll event and on `touchend`; fires only when `!v.isScrolling`, no
  touch is down (track `touchstart`/`touchend`/`touchcancel` on the scroller), and ≥ 100 ms since `touchend`.
  Applies `rest`: set state → render → `useLayoutEffect` writes `el.scrollTop += write` (same task, before
  paint). Also armed when `more` flips, when `clientHeight` changes and after a prepend lands.
- `onScroll`: the page trigger becomes `el.scrollTop < slack.current + TOP`; the pin logic (`stuck`, `STICK`) is
  unchanged (slack changes never change `scrollHeight`; the rest write moves content and offset together).
- Remember row heights: in `onChange` (debounced to rest) walk `v.itemSizeCache` → `remember`; `persist` on rest
  and on unmount.

### 3.4 What must not change

Pinned-to-bottom while streaming (the library's `wasAtEnd` branch sees a zero total-size delta when the slack
absorbed growth above, and the Transcript's own pin effect sees an unchanged `scrollHeight`); the ruler's
active turn (`getVirtualItemForOffset(scrollTop + h/2)` is in virtual coordinates, which include the slack);
`jump()` / `scrollToIndex`; the `compact` panel variant; the composer, the activity row, the scroll-to-bottom
button. `wrap-anywhere` on the scroller stays (84 §SURFACE-TABLE).

## 4. Gates and proof

- `cd crates/desktop/ui && bun run check && bun run test && bun run build` (= `just desktop-check`), all green.
- New tests: `lib/measured.test.ts` (bucket, LRU, persistence shape and cap, `estimateRow` bounds and
  monotonicity), `components/chat/scroll-anchor.test.ts` (§3.2 table), `Mermaid` cache (second mount has `svg`
  on first render — mock `mermaid`), `Reserve` (reserves while not ready, records when ready; `id: null` is a plain
  div).
- A dev-only console counter is acceptable behind `import.meta.env.DEV` (`steady-scroll: absorbed N px / wrote M`)
  if it helps the manual check; remove any `console.log` from the hot path otherwise.

## 5. Manual check (owed to the user after the rebuild)

1. Open a long session on the Mac, fling back through 20 turns with diagrams and pages: nothing under the pointer
   moves while they render; returning to a region shows its diagrams instantly.
2. Same on the iPad: no drift during the fling, no jump when it stops; a page prepend during a fling does not
   jump.
3. A short, fully loaded session: the oldest row at the very top, no blank above it.
4. Streaming: the view stays pinned to the bottom; reading an earlier message while the agent streams does not
   drag the viewport.
5. Resize the window / toggle the sidebar: the transcript re-flows and settles with no stuck blank region.
6. Quit and reopen: a session scrolls through its history without the first-visit drift.

## 6. As-built

`docs/design/135-as-built.md`: files touched with line pointers, the commit-before-paint choice made and why,
measured numbers (first-measure delta before/after `estimateRow` on a real session, absorbed px during a fling),
gates as run, deviations from this document.


---

## Revision 1 — SMOOTH (2026-10-08)

*Status: contract, 2026-10-08 21:50. Branch `feat/135b-smooth-scroll` off main. UI only.*

### R0. The report

The user's words (2026-10-08 21:50): "ever since you made this change, scrolling and sending messages seems a
little more laggy than before. I'm not sure why. I think this could be optimized further. it still isn't just a
smooth scrolling experience where everything is loaded before the viewport actually arrives to it. there must be
a canonical solution to this."

Two separate defects, both verified in the tree at a11fce5f (the 135 files are unchanged on main since):

1. **A cost regression.** `virtual-core`'s `getMeasurementOptions` memo keys on the *identity* of `getItemKey`
   (`dist/esm/index.js` ~597–609). `Transcript.tsx:261` passes an inline closure, so every render of the
   transcript invalidates the memo, `pendingMin` is nulled and `getMeasurements` runs the **full** pass — and since
   135 that pass calls `estimateSize(i)` → `recall ?? estimateRow` → `estimateText` (a line-by-line parse of the
   message) for **every unmeasured row, on every render**. The transcript renders on every virtualizer range
   change while scrolling and on every streaming delta while a reply arrives. Before 135 the same pass was a free
   `() => 96`. A reopened day-long session has hundreds of loaded-but-never-mounted rows → milliseconds per
   render → "scrolling and sending messages seem laggier". Two smaller costs ride along: `rest()` serialises and
   writes `localStorage` synchronously (`persist`) at every scroll stop and every streaming pause, and
   `track()` walks the anchor row's subtree with `getBoundingClientRect` every scrolled frame, fling or not.
2. **No render-ahead.** `overscan: 4` mounts four rows past the visible range in *both* directions, counted in
   rows, not distance. A fling at 3–5 000 px/s crosses four rows in well under the time a diagram takes to mount,
   so rows become visible before their content is ready — "everything is loaded before the viewport arrives" is
   exactly a lookahead window measured in viewports, biased toward the scroll direction. Also: `useElkLayout`
   (`lib/graph-layout.ts:125`) starts from `null` even when `lib/diagram/elk.ts`'s LRU holds the layout, so a
   remounted json-render diagram shows one placeholder frame (the mermaid cache from 135 already paints on the
   first frame).

The canonical shape for a media-heavy chat log (what Slack- and Discord-class clients do) is: windowed rendering
+ remembered heights (135) + cached heavy renderers (135, completed here) + **distance-based, direction-biased
lookahead** + scroll anchoring that never fights the compositor (135). De-virtualising (render everything,
`content-visibility: auto`) is the other canonical answer; it is rejected here because a long session with
hundreds of SVG diagrams would keep every one of them in the iPad's DOM.

### R1. Cost — `Transcript.tsx`, `lib/measured.ts`

1. **Estimates are computed once per row, not per render.** `estimateRow` results are cached in a
   `WeakMap<object, { width: number; px: number }>` keyed on the row's stable payload — `row.entry` for entry
   rows, `row.run[0]` for run rows (the `run` array is rebuilt by `foldRuns`, its first entry object is not),
   `row.prompt` for prompt rows; gap rows are constant. A streaming entry's object changes per delta, but that
   row is mounted and measured, so `estimateSize` is never asked for it.
2. **Stable callbacks.** `getItemKey` and `estimateSize` are `useCallback`s keyed on `[rows]` / `[rows, width,
   session]`, and `estimateSize` reads the session's `Heights.h` map once (hoisted), not through
   `recall → heights → bucket` per row. The measurement pass then runs only when rows or width change, and it is
   a Float64Array walk with Map lookups.
3. **`persist` is never synchronous in `rest`.** `rest()` only `remember`s the `fresh` keys; `remember` already
   arms the ≥ 1 s debounce. Unmount still persists once.
4. **`track()` reads the DOM only when it can matter.** Every scrolled frame keeps the anchor *row* from
   virtual coordinates (`getVirtualItemForOffset`, no layout). The element walk + `getBoundingClientRect` runs
   only when the scroller moved less than `clientHeight / 4` since the previous tracked frame (a slow scroll or
   the tail of a fling) and once at rest; while flinging fast the anchor element is `null` and the anchor row's
   own growth is treated as `moved: 0` (a few px at 4 000 px/s is below perception; the next slow frame re-anchors).
5. **A DEV counter** `steady-scroll: estimates N · renders M` (in the existing `console.debug` line at rest) so
   the fix is visible: after the first pass `estimates` must stay flat while scrolling and streaming.

### R2. Render-ahead — `Transcript.tsx`, `lib/graph-layout.ts`, `lib/diagram/elk.ts`

1. **Distance-based, direction-biased window** via the virtualizer's public `rangeExtractor` (a stable function
   reading the instance through a ref). Let `h = clientHeight`, `top = scrollOffset`, `dir = scrollDirection`.
   Mounted window in px: `[top − behind, top + h + ahead]` with `ahead = 2h`, `behind = 1h` in the scroll
   direction (`backward` swaps them), and `1.5h` each side when `dir` is null (at rest). Indices come from
   `getVirtualItemForOffset` at both edges; the library's own visible `startIndex..endIndex` are always
   included. `overscan` is set to `0`; the window replaces it. The `behind` side is the keep-alive: a row stays
   mounted for a full viewport after it leaves, so a short back-and-forth never re-mounts anything.
2. **Two priorities.** The rows the library reports visible (plus one on each side) render as today; the rest
   of the window renders through `useDeferredValue` (React 19), so a heavy mount two viewports ahead can never
   block a scroll frame: React yields to the scroll update and resumes the lookahead. Absolute positioning makes
   mount order irrelevant; `measureElement` refs work unchanged. If a row reaches the visible set before its
   deferred mount committed, it simply renders urgently on that frame — today's behaviour as the floor.
3. **Synchronous cache hits for ELK diagrams.** `lib/diagram/elk.ts` exports `elkPeek(hash): T | undefined`
   (the LRU read, no promise); `useElkLayout` initialises `shown` from `elkPeek(hash)` when `enabled`, so a
   remounted Graph / StateMachine / Entities / … paints its layout on the first frame like a cached mermaid
   chart. The render-queue path is unchanged for misses and for streaming. Check the non-ELK Views (Sequence,
   Sankey, charts) for the same first-frame gap and give them the same treatment where a cache exists; where the
   layout is synchronous there is nothing to do.

### R3. What must not change

Everything in §3.4, and the 135 guarantees: zero shift during a fling (the 135 smoke, re-run), remembered
heights (`theuth_heights:<session>`), the slack absorber, the iPad rest-only writes.

### R4. Proof

- Gates: `cd crates/desktop/ui && bun run check && bun run test && bun run build`, all green; new tests for the
  estimate cache (same row object → one `estimateRow` call; a new width → recompute) and for the window math
  (pure: `window(top, h, dir, total) → [from, to]` px edges, both directions and rest), `elkPeek`.
- **Perf smoke** (dev browser against the running engine, as in the 135 as-built, read-only — do not send
  prompts to the user's engine): on a session with ≥ 400 loaded rows after paging back, (a) the DEV counter:
  `estimates` after the first pass vs after 40 scroll steps — must be unchanged; (b) `PerformanceObserver`
  `longtask` count and p95 frame interval over a scripted 60-step fling, main a11fce5f vs this branch; (c) the
  **lead** metric: at each sampled frame, does any row intersecting the viewport show "Rendering diagram…" or
  a `Reserve` that is not ready? Count such frames over the fling at ~2 500 px/s; target 0, report the number.
- Re-run the 135 zero-shift sampling on the same sessions (0 shifts expected).
- As-built: `docs/design/135-as-built.md` gains a "Revision 1" section with the numbers above, the exact
  window constants chosen, and any deviation.
