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

Never committed (gitignored): the four new binding stubs in `src/rpc/bindings/` and `answer_fence_snapshot` in the
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
  `:root` rules (plus `.dark` on top for `dark`) — every custom property, `color-scheme` too — and sets
  `color`/`background`/`font` from them. No second colour table, no `.dark` class-lift on `<html>` (that would fire every
  `subscribeDark` observer and re-initialise mermaid for the whole chat).
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
  Fullscreen) and OpenUI's `Open as app`/`Show source` toolbar are not rendered; OpenUI's `ThemeProvider` takes the
  requested appearance. Focus rings, hover tooltips and selection need nothing: nothing is focused, hovered or selected
  off-screen. The HTML picture also drops scrollbars (`scrollbar-width:none` on every clone).
- Errors: `FenceFallback` (the shared closed-fence failure state of all three fences, also what the json-render error
  boundary renders) calls `snapshot.fail(reason)` in an effect → `failed { reason }`. A render throw outside it reaches
  `createRoot`'s `onUncaughtError` → `failed`.

## Settle (no rAF, no ResizeObserver)

- Mount after `document.fonts.ready`. Ready = content mounted and no `[data-snapshot-pending]` element — DiagramFrame's
  "Rendering diagram…" placeholder carries that attribute in snapshot mode only (ELK layout pending, mermaid promise
  pending). A MutationObserver (subtree/childList/attributes/characterData) re-arms the timer: draw `QUIET_MS` (300 ms)
  after the last change once ready, never later than `SETTLE_MS` (8 s) after mount; still pending then → `failed`.
- None of the diagram Views measures its container (no ResizeObserver/rAF anywhere under `components/chat/surface`,
  `lib/diagram`); charts and ELK views size from their own viewBox, so no explicit width plumbing was needed. The only
  ResizeObserver on the path is `Reserve`, inert without a `MeasuredContext` (none in the snapshot root).

## Rasterization path per content type (D4)

| Content | Path |
|---|---|
| The content is one DiagramFrame whose `<svg>` holds all its text (Graph, Sequence, StateMachine, Gantt, charts, ELK family, mermaid) | `standalone(svg.outerHTML, page background)` + `rasterize()` from `lib/diagram-export.ts` — the frame's own Copy PNG path. The picture is the diagram alone (no card chrome), width `min(natural, width)` with the aspect kept, i.e. the chat's `max-width:100%` against the full frame width. |
| Anything else: Kanban/Heatmap (HTML diagrams), Table, Card, MetricGrid, Checklist, mixed Stacks, a diagram next to other content, openui | In-house DOM → SVG `<foreignObject>` (`data:` URL) → `rasterize()` → PNG. Deep clone with every standard computed property inline (custom properties on the root, subtree-set custom properties kept), `::before`/`::after` with content as generated rules, input/textarea state, canvases as data-URL images, same-origin `<img>` bitmaps inlined, `@font-face` rules of the families actually used embedded as data URLs. Root background = the appearance's page background (opaque). |

- "Sole diagram" test: exactly one `[role=application]` surface, and the content's text equals the card label plus the
  svg's text, and no svg outside the card. Anything that fails it takes the HTML path, so nothing visible is dropped.
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
Sequence, StateMachine, Gantt, Kanban (fixtures), a Stack of MetricGrid + Checklist + Table, a mermaid flowchart, an
openui card, a truncated json-render fence; widths 560 and 720; light; scale 2.

| Run | Result (18 requests) |
|---|---|
| Chrome (CfT 1226, headless) visible | 16 `done`, both broken → `failed` (`Interactive view unavailable: …`), all in ≈ 4 s |
| Chrome hidden | same, ≈ 5.8 s |
| WebKit (Playwright 2370, headless) visible | same, ≈ 3.9 s; foreignObject pictures draw (no taint, fonts right) |
| WebKit hidden | same, ≈ 3.9 s |
| Chrome, app in `.dark`, light requested | every PNG byte-identical to the light-app run (tokens, CSS and mermaid palette forced) |
| Chrome, dark requested | dark pictures |

"Hidden": both headless engines keep `document.visibilityState === 'visible'` after a second tab is opened and
activated (also tried a CDP background target and a minimised window), so the hidden runs open and activate the second
tab **and** emulate what a hidden Mac window does to the page through an init script: `requestAnimationFrame` never
fires, `ResizeObserver` never delivers, `visibilityState` is `hidden`. All answers still arrive, which is the property
that matters (the runner uses neither). Timer throttling of a really hidden page is not emulated (see risks).

Chat unchanged: `chat-before.png` (taken on the untouched tree) and `chat-after.png` of the chat's own Streamdown
rendering of the Graph fixture are byte-identical.

Artifacts: `/tmp/163-164/ui-proof/png/` (96 PNGs: `<engine>-<mode>-<case>-<width>.png`),
`/tmp/163-164/ui-proof/contact-sheet.html`, `/tmp/163-164/ui-proof/result-*.json`, `chat-before.png`, `chat-after.png`.

## Deviations from the brief

- `App.tsx`: besides the one `useEffect` line, one `import` line (`installFenceSnapshot`) next to the `page-preview`
  import — the install line cannot compile without it. Nothing else in the file.
- Files outside the brief's list, all one-hook reads needed to force the appearance on diagram tokens:
  `lib/diagram-theme.ts` (`diagramTokens(el)` parameter), `lib/graph-layout.ts` (`useDiagramTokens` is now the shared
  hook), `lib/diagram/chart/frame.tsx` (private `useTokens` removed in favour of it).
- Frame hidden with `opacity:0` off-screen instead of page-preview's `visibility:hidden` (reason above).
- Svg-path pictures are the diagram alone (no card label/border), scaled to fit `width`; HTML pictures are the full
  `width` with 16 px side padding.

## Contract problems

- `SnapshotId` collides with an existing ts-rs export: `crates/sandbox/src/vm/mod.rs:250` `pub struct SnapshotId(pub
  String)` already writes `bindings/SnapshotId.ts`. The stub was therefore not written (the existing file is the same
  `string`); `api.ts` imports `@/rpc/bindings/SnapshotId`. If 164-ENG renames the artifact type's TS name, `api.ts`'s
  import follows in one line. Reported to the parent session.

## Residual risks

- A really hidden page's timers: Chrome throttles background tabs to ≥ 1 s per timer and, after 5 min hidden,
  chained timers to once a minute; WebKit throttles hidden windows. The runner's timers start from event and
  MutationObserver tasks, not timer chains, so it should stay inside the engine's 20 s, but this was not observable in
  headless engines.
- `dark:` Tailwind variants follow `<html class="dark">`, not the frame: in a light picture taken while the app is dark,
  the few `dark:` utilities in surface components (badge/button/input shades, Excerpt code colours) use their dark form.
  Theme variables, which carry nearly all colour, are forced.
- OpenUI `BarChart`/`LineChart`/`ScatterChart`/`RadarChart` are upstream recharts (`ResponsiveContainer`, a
  ResizeObserver): in a hidden window they may lay out at 0 size. json-render charts are unaffected.
- External images inside a surface (cross-origin `<img>`) cannot be read back and stay blank in HTML pictures.
- Horizontally scrolling content (wide Kanban, wide tables) is clipped at the frame width, as the chat clips it at that
  width; the picture just drops the scrollbars.
- Headless WebKit (Playwright) is close to but not the same as WKWebView; the Mac app itself was not driven (forbidden).
