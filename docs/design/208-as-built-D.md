# 208 as built — D (desktop UI canvas, on fixtures)

Branch `feat/208-attention-graph-ui`, worktree `theuth-graph`, off `98bc9b58b`. Everything runs on the contract §9 fixture
and the §10.1 simulated engine; no engine side existed while this was built.

## Done

- `@xyflow/react` exactly `13.0.2` (`bun add --exact`, `package.json` + `bun.lock`). Imported only by
  `graph/Canvas.tsx` and `graph/edges.tsx`; its CSS only by `Canvas.tsx`; `Region.tsx` loads `Canvas` with
  `lazy(() => import('./Canvas'))` inside `<Suspense>`, rendered only while expanded or full screen. The attribution link
  stays (no `proOptions.hideAttribution`).
- Pure modules with the contract's exported API: `layout.ts` (§8.3), `ttl.ts`, `reducer.ts`, `selectors.ts` (§8.4),
  `prefs.ts` (§8.5), `fixture.ts` (§9 verbatim), plus `elements.ts` (see deviations).
- `queries.ts`: `useAgentGraph`, `GraphSource`, `setGraphSource`, `setGraphClock` (+ `graphNow`): one full snapshot on
  mount, deltas through `applyDelta`, a full re-snapshot on `resnapshot` (one in flight, later asks collapse into one,
  full wins), cursor renewal every 120 s through `mergeSnapshot`, a debounced (1 s) cursor call for an `include` session
  the graph lacks or a wider TTL window (once per id), a full snapshot when the connection returns to `open`.
- Components: `NodeBody` (three LODs, memo; sidebar indicators reused: `GLYPH`, `ago`, `stateTone`, `HarnessGlyph`,
  `shortModel`, `counts`, `WatchDot`, `EyeMark`, `stateText` via `statusText`), `Strip`, `Region` (`GraphRegion`),
  `Canvas`, `edges` (`TreeEdge`, `MessageEdge`), `NodeBar`, `EdgePopover`, `TtlPopover`.
- `model.ts`: `SubtreeFilter`, `ListFilter.subtree`, `EMPTY_FILTER.subtree = null`, `matchesFilter` subtree test.
- `Page.tsx`: the region between the header and the rail/detail row (outside the list's pending/empty branches; hidden
  under md while a detail is open, like the header); `readView` → `subtree: null` (not stored); `filtered` counts the
  subtree, so "Clear filters" clears it; highlight = focused entry's head session, else the open item's; stable
  `onShowInGraph` / `onSubtree` through the `latest` ref; `CONTROL` exported.
- `Toolbar.tsx`: "In <label>" chip with `X` (`aria-label="Clear tree filter"`), first in the kinds row.
- `Card.tsx`: `FrameProps.onShowInGraph`; a ghost icon button (lucide `Network`, `aria-label`/title "Show in graph") at
  the end of the actions row when set and the item has a session; `StackCard` gets it through the shared `Frame` with
  its head's session.
- `AgentTree.tsx`: `stateTone(state)` extracted and exported; `SubagentRow` calls it; class output identical.
- Dev page `src/dev/attention-graph-fixture.{html,tsx}` (§10.1): the real `Page` on fake data, simulated engine with an
  accelerated clock (`?speed=`, default 60), a 2 s event script (spawn, awaiting/approval round trips, finish/fail,
  messages sent → delivered/replied 1 s later, child results, a new root every 30 s, remote root flip every 20 s, one
  deliberate seq gap a minute), five list items moved onto graph sessions (`r1.builder`, `r3`, `r3.sweep`, `r7` ×2),
  `/c/$session` stub route for "Open session", `?theme=dark`, `?still=1` stops the script.

## Deviations (code wins; decided here)

1. **New pure module `graph/elements.ts`** (inside D's `graph/**`): canvas node/edge descriptors — fold detection (D17,
   `FOLD_MIN = 8`), highlight dimming, tree edges, message edge handles, `lodOf(zoom)`. Keeps `Canvas.tsx` thin as the
   brief asks. Tested in `elements.test.ts` (LOD buckets, folds, highlight dimming, handle choice).
2. **Message edge handles**: nodes carry four invisible handles (top/bottom/left/right). A message edge between nodes
   ≥ one box width apart runs right → left; otherwise bottom → top. Folded (inactive) message edges get `zIndex 0` so
   they pass behind nodes; active ones `zIndex 1`.
3. **Dot LOD** is the whole 200 × 64 box filled with the state tone (idle/finished at 45 %), not a 14 px dot: at zoom
   < 0.4 a 14 px dot in flow units is under 6 screen px and tree edges ended in mid-air. The danger ring is drawn
   `ring-8` so it survives the zoom. Compact LOD draws the name at `text-2xl` for the same reason.
4. **First fit** uses `fitViewOptions { padding 0.15, maxZoom 1, minZoom 0.4 }`: a wide forest (8 trees) opens at the
   compact level and pans, instead of specks (it was dot LOD at 1280 px and unreadable at 390 px). Fit (toolbar) and the
   Controls fit still show everything.
5. **Esc in full screen** is handled in a capture-phase window listener: a bare Escape on `<body>` arrives
   `defaultPrevented` from a handler below the window (observed in the browser), so the contract's bubble listener never
   saw it. It still yields to editable targets and to an open Radix popover/menu.
6. **Region hidden on rejection**: `GraphRegion` also returns null when the first snapshot failed and there is no state
   (legacy discovery reports every method as present; an engine without `agent_graph` would otherwise show an empty
   strip).
7. **MiniMap** 150 × 84 and hidden under `sm` (it covered most of a 390 px canvas).
8. **Hover tooltip** is the node's `title` (`label · row 2`), not the path; the path shows as the highlight when the
   node is the focused card's.
9. **`EdgePopover` message filter** uses a small `selectors.endpointId(addr)` (the B `endpoint` rule) to match
   `PeerRecord.from` / `receipt.to` against the other end.
10. **Bindings: `METHODS` and `TOPICS` runtime arrays in `client.ts` not hand-patched** (single lines of 10–4.6 k chars).
    Types are complete; at runtime `useQuerySupport(['agent_graph'])` reads discovery, not `METHODS`. F's regeneration
    adds them.
11. **Show in graph centring waits for the first fit**: a canvas that mounts because of the click runs `fitView` in its
    first frames, which overrode the centring. The centring now runs `FIT_SETTLE_MS = 350` ms after mount at the
    earliest, reading positions through a ref, once per nonce.

## Hand-patched bindings (git-ignored, never committed; F regenerates)

`crates/desktop/ui/src/rpc/bindings/`:
- `method.agent.output.ts`, `method.agent.receiver.ts`: §7.1 types appended, `Contract` entries.
- `method.agent.input.ts`: `AgentGraphParams`, `GraphCursor`, `Contract` entries.
- `event.agent_2egraph.output.ts`, `event.agent_2egraph.receiver.ts`: new (§7.3; receiver with `?:` on default fields).
- `event.agent_2epeer_2dmessage.output.ts` / `.receiver.ts`: `from`, `result`, `to` (receiver optional).
- `client.ts`: imports `Dg`/`Dgr`, `MethodMap.agent_graph`, `ReceiverResults.agent_graph`, `TopicPayloads`,
  `TaggedTopicPayloads`, `TopicReceiverPayloads` `agent.graph@1` (not `METHODS`/`TOPICS`, deviation 10).
- `methods.json`: `agent_graph`.

## Gates (`/tmp/theuth-208-d-gate.sh`, one hold each, `crates/desktop/ui`)

| hold | step | rc | result |
|---|---|---|---|
| h1 21:16 | `bun add @xyflow/react@13.0.2 --exact` | 0 | 4 packages |
| h1 | `bun run check` | 1 | `Page.tsx` `readView` missing `subtree` (expected mid-work; fixed) |
| h1 | vitest attention + sidebar + no-emoji | 0 | 13 files, 97 tests (before the graph tests existed) |
| h2 21:36 | `bun run check` | 0 | |
| h2 | vitest attention + sidebar + no-emoji | 0 | 21 files, 137 tests |
| h3 21:41 | `bun run check` | 0 | |
| h3 | `bun run test` | 0 | 390 files, 2789 tests |
| h3 | `bun run build` | 0 | lazy chunk `Canvas-*.js` 207.46 kB (gzip 72.00 kB) + `Canvas-*.css` 15.99 kB (gzip 2.70 kB); `react-flow`/`xyflow` appear in no other chunk |

## Seen in the browser (headless Chromium, `bunx vite --port 1438`)

Screenshots in `/tmp/theuth-208-shots/`, each in `light` and `dark`: `01-expanded-compact-*-1280`,
`02-lod-dot-*-1280`, `03-lod-full-*-1280`, `04-collapsed-*-1280`, `05-show-in-graph-path-highlight-*-1280`,
`06-node-click-list-filtered-*-1280`, `07-fullscreen-*-1280`, `08-mid-simulation-*-1280`, `09-mobile-collapsed-*-390`,
`10-mobile-fullscreen-*-390` (`.png`). The simulated clock runs at 60× even with `?still=1`, so counts in later shots
differ from the T0 text. Observed:
- Collapsed strip "4 need you · 11 working · 14 finished · +3 older trees" at the fixture instant; `G` collapses and
  expands; `Shift+G` full screen; `Esc` leaves it (after deviation 5).
- Expanded canvas: dotted background (both themes), Controls, MiniMap, attribution link; tidy trees; sticky slots
  (TTL edit grew a tree without moving the others left).
- Three LODs by zoom (`data-lod` = dot/compact/full checked after zoom-out/zoom-in clicks).
- Needs-you red ring + count badge on `builder`, `ops desk`, `nightly-sweep`, `concierge desk`.
- Mid-simulation: 3 animated (dashed, moving) edges, 2 pulse dots in flight, a fading node (0.25 < opacity < 1).
- Show in graph from the first card: `r1.builder` centred exactly (0, 0 px offset) with no zoom change, accent border on
  it and its root, the rest dimmed, the tree edge on the path accent. Also from a collapsed region (the canvas mounts
  because of the click): centred (0, 0) after deviation 11; before it the mount fit won (470 px off). After a 400 px
  drag, a second click re-centres (0, 0).
- Node click on `ops desk`: list "34 items · 2 shown", chip "In ops desk" in toolbar and strip, node bar with "Filter
  list to this tree · Open session · Clear".
- Message edge click: popover with names, per-direction counts, last time; "Load messages" (fixture returns none).
- TTL popover: child TTL 10 → 60 applied at once (31 → 35 nodes) and persisted under `attention.graph`.
- 390 × 844: region starts collapsed; opening it goes full screen with the back chevron.
- No console errors or page errors during a 9 s simulated run.
- Reduced motion (emulated `prefers-reduced-motion: reduce`): 0 animated edges, 0 pulse dots (`animateMotion`), 3
  static in-flight dots on the open/awaited message edges.
- Follow on: the viewport moved on every one of 8 one-second samples while the script ran.

## Not done / not verified

- Folding (D17): implemented in `elements.ts`; the fixture never has 8 unpinned visible descendants under one node, so
  not seen.
- Resize handle drag, height persistence: implemented, not dragged in the browser.
- `onlyRenderVisibleElements` above 50 nodes: set; not measured.
- The edge popover's "Load messages" list with real records (the fixture's `list_peer_messages` returns none).
- Pulse timing is by arrival (each new pulse runs once for 0.9 s), not by `at_ms`.
- Real data path (`api.agent_graph`, `onTopic('agent.graph@1')`) not run against an engine (F §11.4).
