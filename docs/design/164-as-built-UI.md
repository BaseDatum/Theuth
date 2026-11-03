# 164 CHANNEL-PICTURES — as built, client side (164-UI)

Branch `feat/164-ui`. Contract: `164-channel-pictures.md` (D3, D4, TS contract). This file records what the client
runner does, every decision taken while building it, and what is left.

## What was built

| File | Role |
|---|---|
| `crates/desktop/ui/src/lib/fence-snapshot.ts` | `installFenceSnapshot()`: the event wiring, the per-request lifecycle (answer `started`, queue, render, answer `done`/`failed`, drop on `FenceSnapshotResolved`), and the pure parts: `admit`/`release` (queue, `MAX_RUNNING` 2), `settleAt` (300 ms quiet, 8 s cap), `scales`/`fitPng` (D4 ladder and caps), `pngSize` (IHDR), `base64`. |
| `crates/desktop/ui/src/lib/fence-snapshot.test.ts` | Vitest for the pure parts (queue order/slots/duplicates, settle timing, ladder, side and byte caps, PNG size, base64 across chunks). |
| `crates/desktop/ui/src/components/chat/FenceSnapshot.tsx` | Lazily imported host: mounts the fence off-screen, forces the appearance, detects settle, rasterizes. |
| `crates/desktop/ui/src/lib/snapshot-mode.ts` | `SnapshotContext` (default `null` = off), `useSnapshot()`, `useThemeTokens()` (diagram tokens off the app theme, or off the snapshot frame). |
| `DiagramFrame.tsx`, `Mermaid.tsx`, `OpenUIFence.tsx`, `JsonRenderFence.tsx` (`FenceFallback`) | The minimal reads of the snapshot context (below). |
| `lib/diagram-theme.ts`, `lib/graph-layout.ts`, `lib/diagram/chart/frame.tsx`, 8 diagram Views | Token reads routed through `useThemeTokens()` (below). |
| `rpc/api.ts`, `rpc/events.ts`, `App.tsx` | `answer_fence_snapshot` (type + METHODS), the two events in the union, the install line. |

Never committed (gitignored): the five binding stubs in `src/rpc/bindings/` and `answer_fence_snapshot` in the
repo-root `bindings/methods.json`.

## Runner (D3)

- Installed in `App.tsx` next to `installPagePreview` with the same `AUX` guard (main window only); `installFenceSnapshot`
  itself returns a no-op on `container() === 'ios'`, so only desktop shells and the web client render. The `client`
  field reuses `page-preview`'s `clientKind()`.
- A request is ignored when no RPC `api` is connected (nobody to answer). Otherwise: `started` at once, a 15 s
  `failed` timer from receipt (queue wait included; the engine's 20 s budget stays ahead), then the queue. At most two
  render at a time; the rest wait in arrival order and move up as slots free. A repeated id is ignored.
- `FenceSnapshotResolved` cancels a queued or running job without answering; the slot passes on.
- The host module is `import()`ed on the first request: the vitest-run module stays free of React/DOM code and the app
  pays nothing until a channel asks (the module graph it pulls — MessageAssistant, the renderers — is already loaded by
  the chat).

## Off-screen mount (D3 "same component")

- The component is looked up in `MessageAssistant`'s exported streamdown `plugins.renderers` by language, so it is by
  construction the one the chat uses (`JsonRenderFence`, `MermaidFence`, `OpenUIFence`), rendered with
  `{ code: source, language, isIncomplete: false }` under the default `FenceContext` (settled transcript case).
- Frame: `position:fixed; left:-100000px; opacity:0; pointer-events:none; z-index:-1; aria-hidden`, exactly `width` CSS
  px. Not `display:none` (no layout, page-preview's rule) and **not `visibility:hidden`**: that value inherits, so every
  computed style copied into the HTML picture would be hidden. Inside, the rasterized box is
  `display:flow-root; padding: 0 16px` (the fences' own `my-4` margins become the top/bottom breathing room).
- Appearance: the frame carries the requested palette's theme variables, read at runtime from the stylesheet's own
  rules whose selector list names `:root` (the token block, the shadcn aliases and Tailwind's `:root, :host` theme layer,
  whose `--color-*` aliases such as `--color-fg-accent` resolve at the root) plus `.dark` on top for `dark` — every custom
  property, `color-scheme` too — and sets `color`/`background`/`font` from them. No second colour table, no `.dark`
  class-lift on `<html>` (that would fire every `subscribeDark` observer and re-initialise mermaid for the whole chat).
- `dark:` utilities (R1): `styles.css:7` is now
  `@custom-variant dark (&:is(.dark *):not(:where([data-snapshot-light], [data-snapshot-light] *)));`. A light frame
  carries `data-snapshot-light`, a dark frame the `.dark` class. `:where()` keeps the added specificity at zero, so every
  existing `dark:` utility cascades exactly as before, and nothing outside a snapshot frame carries the attribute.
- JS-side tokens: `diagramTokens(el = document.documentElement)` now takes the element to resolve on.
  `useThemeTokens()` resolves on the snapshot frame when a snapshot is mounted, else exactly as before (memoised on the
  `.dark` flip). `graph-layout`'s exported `useDiagramTokens` (six ELK Views import it) is now that hook re-exported under
  its existing name; the chart kernel's private `useTokens` was replaced by it; the eight Views that inlined
  `useMemo(diagramTokens, [dark])` (Journey, Layers, Sankey, Fishbone, Treemap, Venn, Loop, Sequence) call it.
- Mermaid: closed charts in a snapshot neither read nor fill `svgCache` (its palette may differ from the app's). When the
  snapshot's appearance differs from the app's, the render runs between `initialize(config(snapshot tokens))` and
  `initialize(config())`; all mermaid renders (chat and snapshot) go through one promise chain so the swap never reaches a
  chat render. Mermaid serialises renders internally anyway, so chat throughput is unchanged.
- Chrome hidden by the context (default off, so the chat is unchanged): DiagramFrame's hover toolbar (Copy/Save/
  Fullscreen), OpenUI's `Open as app`/`Show source` toolbar, and (R2) the Excerpt's `Copy excerpt` and the Code block's
  `Copy code` buttons are not rendered; OpenUI's `ThemeProvider` takes the requested appearance. A snapshot Excerpt also
  shows every row with no "Show all N lines" button (its 18-row cap is a vertical scroller; a picture cannot scroll or
  click — the same rule as widening). Searched `components/chat/surface` and `lib/diagram` for other per-block chrome:
  none left. Kept on purpose: the Image block's `Load image` button (a remote `src` loads only on the reader's click,
  design 30 §11; a snapshot must not fetch it either), Accordion/Collapsible/Tabs (their drawn state is content).
  Focus rings, hover tooltips and selection need nothing: nothing is focused, hovered or selected off-screen. The HTML
  picture also drops scrollbars (`scrollbar-width:none` on every clone).
- Errors: `FenceFallback` (the shared closed-fence failure state of all three fences, also what the json-render error
  boundary renders) calls `snapshot.fail(reason)` in an effect → `failed { reason }`. A render throw outside it reaches
  `createRoot`'s `onUncaughtError` → `failed`.

## Settle (no rAF, no ResizeObserver)

- Mount after `document.fonts.ready` (bounded to 1 s, and each embedded font fetch to 3 s: a font load that never
  finishes must not hold both render slots). Ready = content mounted and no `[data-snapshot-pending]` element — DiagramFrame's
  "Rendering diagram…" placeholder carries that attribute in snapshot mode only (ELK layout pending, mermaid promise
  pending). A MutationObserver (subtree/childList/attributes/characterData) re-arms the timer: draw `QUIET_MS` (300 ms)
  after the last change once ready, never later than `SETTLE_MS` (8 s) after mount; still pending then → `failed`.
- None of the diagram Views measures its container (no ResizeObserver/rAF anywhere under `components/chat/surface`,
  `lib/diagram`); charts and ELK views size from their own viewBox, so no explicit width plumbing was needed. The only
  ResizeObserver on the path is `Reserve`, inert without a `MeasuredContext` (none in the snapshot root).

## Rasterization path per content type (D4)

| Content | Path |
|---|---|
| The content is one DiagramFrame whose `<svg>` holds all its text (Graph, Sequence, StateMachine, Gantt, charts, ELK family, mermaid) | `standalone(svg.outerHTML, page background)` + `rasterize()` from `lib/diagram-export.ts` — the frame's own Copy PNG path. The picture is the diagram alone (no card chrome) at its natural width, i.e. `frameFor(width, natural)`: never narrower than its viewBox while that fits 1600 CSS px (R1 addendum: the chat lets you pan/zoom to every part; a picture cannot), scaled down to 1600 beyond. |
| Anything else: Kanban/Heatmap (HTML diagrams), Table, Card, MetricGrid, Checklist, mixed Stacks, a diagram next to other content, openui | In-house DOM → SVG `<foreignObject>` (`data:` URL) → `rasterize()` → PNG. Deep clone with every standard computed property inline (custom properties on the root, subtree-set custom properties kept), `::before`/`::after` with content as generated rules, input/textarea state, canvases as data-URL images, same-origin `<img>` bitmaps inlined, `@font-face` rules of the families actually used embedded as data URLs. Root background = the appearance's page background (opaque). |

- "Sole diagram" test: exactly one `[role=application]` surface, and the content's text equals the card label plus the
  svg's text, and no svg outside the card. Anything that fails it takes the HTML path, so nothing visible is dropped.
- Widening (R1 addendum): a picture cannot scroll, so after settle the HTML path measures the largest horizontal
  overflow of the content or any descendant (`scrollWidth − clientWidth`), and re-lays the frame at `frameFor(width,
  width + overflow)` — the requested width when it fits, else the full scroll width capped at `MAX_FRAME` = 1600 CSS px
  (up to three passes for nested scrollers; layout is synchronous). Only content wider than 1600 px still clips, at
  1600, as the chat clips it. The decision is the pure `frameFor` in `lib/fence-snapshot.ts`, tested. D4's PNG caps
  then apply as before by lowering the scale.
- Caps: the ladder is the requested scale (clamped 1–3), then 1.5, then 1; a scale is skipped without drawing when a
  side would exceed 8000 px, and dropped after drawing when the PNG exceeds 4 MB; nothing fits → `failed`. `width`/`height`
  in the answer come from the PNG's IHDR, `png` is standard base64 without prefix.
- In-house rather than a dependency (`modern-screenshot` etc.): ~150 lines in the lazy host chunk, no new package, no
  `package.json`/`bun.lock` change, so gate 3 does not apply. First attempt diffed computed styles against per-tag
  defaults to shrink the markup; that is wrong whenever a skipped value only holds together with another (a default
  `0px` border width with a non-default `solid` style draws 3 px borders) — the proof showed it, so every property is
  written. Pictures stay well under the caps (largest proof PNG 98 KB).

## Proof (throwaway, `/tmp/163-164/ui-proof/`, never committed)

Harness: `/tmp/163-164/ui-proof/harness.{html,tsx}` served by this worktree's own vite config through a wrapper
(`vite.config.ts` there, port 5199); it installs the real runner with a fake `answer_fence_snapshot` sink and feeds
`FenceSnapshotRequested` events through the real `dispatch`. Driver `run.mjs`. Cases: Graph (`valid-graph-extended`),
Sequence, StateMachine, Gantt, Kanban, Excerpt, Code (fixtures), a 30-line Excerpt, a Stack of MetricGrid + Checklist +
Table, a 6-column Table, a mermaid flowchart, an openui card, a truncated json-render fence; widths 560 and 720; scale 2;
light unless stated.

| Run | Result (26 requests) |
|---|---|
| Chrome (CfT 1226, headless) visible | 24 `done`, both broken → `failed` (`Interactive view unavailable: …`), all in ≈ 6.6 s |
| Chrome hidden | same, ≈ 9.7 s (see the one stall below) |
| WebKit (Playwright 2370, headless) visible | same, ≈ 6.1 s; foreignObject pictures draw (no taint, fonts right) |
| WebKit hidden | same, ≈ 5.9 s |
| Chrome and WebKit, app in `.dark`, light requested | all 24 pictures per engine **byte-identical** (`cmp`) to the light-app run's, Stack, Excerpt, Code and Table included |
| Chrome, dark requested | dark pictures (Excerpt with the shiki dark tokens through the frame's `.dark`) |

Copy chrome: the Excerpt and Code pictures carry no copy button; the 30-line Excerpt is 1326 px tall at scale 2 with all
30 rows (the chat caps it at 18 behind "Show all").

Widening, both engines: `kanban-560` is 1264×412 px (632 CSS px: all three columns, "Done" included, where it used to
clip at 560); `table6-560` is 1580/1582×326 px (790/791 CSS px, all six columns, no wrapping into a scroller); at 720
the Kanban fits and stays 720, the Table widens to the same 790 px. Svg pictures now keep their natural width
(Graph 728, StateMachine 879, mermaid 1082 CSS px at either request width).

Before the R1 fixes the Chrome dark-app Excerpt differed from the light-app one in 160 pixels (max channel delta 39): the
marked row's `shadow-[inset_2px_0_0_0_var(--color-fg-accent)]` bar, whose `--color-fg-accent` came from Tailwind's
`:root, :host` theme layer that the frame did not yet re-declare. Fixed by the selector-list match above.

One Chrome hidden run (the first one with the Excerpt/Code cases) stalled: after 14 answers both render slots stopped
progressing and the 8 remaining requests answered `failed` (`not drawn within 15 s`) — the runner's own deadline, so
the engine still got an answer. Not reproduced in the six Chrome hidden runs since. Unbounded `document.fonts.ready`
waits are the one shared wait both slots had, so they are now bounded (above); cause not proven.

"Hidden": both headless engines keep `document.visibilityState === 'visible'` after a second tab is opened and
activated (also tried a CDP background target and a minimised window), so the hidden runs open and activate the second
tab **and** emulate what a hidden Mac window does to the page through an init script: `requestAnimationFrame` never
fires, `ResizeObserver` never delivers, `visibilityState` is `hidden`. All answers still arrive, which is the property
that matters (the runner uses neither). Timer throttling of a really hidden page is not emulated (see risks).

Chat unchanged: the chat's own Streamdown rendering of Graph + Release-readiness (MetricGrid + Table) + Excerpt + Code,
screenshotted from an untouched copy of the tree (`git archive e9eebf92` under `/tmp/163-164/ui-base`, own vite server)
and from this branch, app light and app dark, re-taken after R2: `chat-before-light.png` = `chat-after-light.png` and
`chat-before-dark.png` = `chat-after-dark.png`, byte for byte. The chat keeps its buttons: both trees' DOM has
`Copy excerpt` 1, `Copy code` 1, `Copy PNG` 1, `Fullscreen` 1.

Artifacts: `/tmp/163-164/ui-proof/png/` (168 PNGs: `<engine>-<mode>-<case>-<width>.png`),
`/tmp/163-164/ui-proof/contact-sheet.html`, `/tmp/163-164/ui-proof/result-*.json`, `chat-{before,after}-{light,dark}.png`.

## Deviations from the brief

- `App.tsx`: besides the one `useEffect` line, one `import` line (`installFenceSnapshot`) next to the `page-preview`
  import — the install line cannot compile without it. Nothing else in the file.
- Files outside the brief's list, all one-hook reads needed to force the appearance on diagram tokens:
  `lib/diagram-theme.ts` (`diagramTokens(el)` parameter), `lib/graph-layout.ts` (`useDiagramTokens` is now the shared
  hook), `lib/diagram/chart/frame.tsx` (private `useTokens` removed in favour of it).
- Frame hidden with `opacity:0` off-screen instead of page-preview's `visibility:hidden` (reason above).
- `styles.css:7` (R1 decision): the `dark` custom variant skips `[data-snapshot-light]` subtrees.
- `components/chat/surface/Excerpt.tsx` and `registry.tsx` (`Code`), R2 decision: one `useSnapshot()` read each to drop
  the copy buttons (and the Excerpt row cap) in pictures.
- Svg-path pictures are the diagram alone (no card label/border) at natural width up to 1600 CSS px; HTML pictures are
  `width` wide with 16 px side padding, wider when the content scrolls horizontally (widening above).

## Contract deviation: `FenceSnapshotId`

- The contract's `SnapshotId` collided with an existing ts-rs export (`crates/sandbox/src/vm/mod.rs:250`
  `pub struct SnapshotId(pub String)` → `bindings/SnapshotId.ts`). Orchestrator amendment (2026-11-03, binding): the
  artifact type is `FenceSnapshotId(pub uuid::Uuid)` in Rust, exported as `bindings/FenceSnapshotId.ts` =
  `export type FenceSnapshotId = string;`; `FenceSnapshotRequested.id` / `FenceSnapshotResolved.id` are
  `FenceSnapshotId`; the RPC is `answer_fence_snapshot(id: FenceSnapshotId, answer: FenceSnapshotAnswer)`. The UI stubs
  (`FenceSnapshotId.ts`, `FenceSnapshotRequested.ts`, `FenceSnapshotResolved.ts`) and `api.ts` follow it; the sandbox's
  `SnapshotId.ts` is untouched. `docs/design/164-channel-pictures.md` is amended at integration.

## Residual risks

- A really hidden page's timers: Chrome throttles background tabs to ≥ 1 s per timer and, after 5 min hidden,
  chained timers to once a minute; WebKit throttles hidden windows. The runner's timers start from event and
  MutationObserver tasks, not timer chains, so it should stay inside the engine's 20 s, but this was not observable in
  headless engines.
- OpenUI `BarChart`/`LineChart`/`ScatterChart`/`RadarChart` are upstream recharts (`ResponsiveContainer`, a
  ResizeObserver): in a hidden window they may lay out at 0 size. json-render charts are unaffected.
- External images inside a surface (cross-origin `<img>`) cannot be read back and stay blank in HTML pictures.
- Content wider than 1600 CSS px (a very wide Kanban/Table/openui, a huge diagram) is clipped at 1600 (HTML) or scaled
  down to 1600 (svg); the picture drops scrollbars.
- Headless WebKit (Playwright) is close to but not the same as WKWebView; the Mac app itself was not driven (forbidden).
