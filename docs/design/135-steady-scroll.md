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
