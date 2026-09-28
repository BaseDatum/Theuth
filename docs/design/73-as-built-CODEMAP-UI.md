# 73 — as built: CODEMAP-UI (U1–U3, `feature/73-codemap-ui`)

Built 2026-09-28 against design 73 at `5079abaf`, UI only (`crates/desktop/ui/**`), engine side (E1–E2) not landed: the panel runs on the `mock` adapter over the §5 fixtures and switches to the engine by flag.

## Commits

| # | Subject | What |
|---|---|---|
| `39f4e8e2` | `docs(73): side pane per Sky's ruling` | §6.7, §10, §12.1 amended: the CodeMap is a real side pane in `Panes`, not a Sheet |
| `6d0050fc` | `feat(ui): codemap slice, side pane host, Here lens (73 U1)` | slice + reducer, §5 types, mock/rpc adapter, fixtures, header button, docked pane + phone Sheet, Here |
| `87bd21bc` | `feat(ui): codemap Fit and Change lenses, lens state machine, panel body (73 U2)` | lens machine, hand layouts + slots, GraphSvg additions, Fit/Change + list forms, panel body, interactions |
| `7a83ef6a` | `feat(ui): codemap pop-out route and target (73 U3)` | `PopoutTarget 'codemap'`, route, regenerated route tree |
| (this) | `feat(ui): codemap smoke fixes + as-built (73 U3)` | trail dedupe, label disambiguation, band badge room, narrow-header rules, as-built |

U1/U2 are one compile unit (the panel body in U2 is imported by the host in U1); each step's *files* are separate, the tree is green from U3 on. `bun run check`, `bun run test` (run alone: 110 files, 627 tests), `bun run build` (regenerates `routeTree.gen.ts`) all pass.

## What changed (file:line)

Store and events
- `src/lib/codemap/types.ts` — every §5 type hand-typed (`CodeToolRan`, `CodeReconciled`, `LensMeta`, `MapRow`, `DirRow`, `HereLens`, `FitNode/Edge/Lens`, `ChangeSym/File/Lens`, `CodemapApi`) + the §4 slice (`CodemapSlice`, `TrailRow`). Field names exactly as §5; nothing added to the wire shapes.
- `src/lib/codemap/slice.ts` — pure slice reducer: `codeToolRan` (roots ∪= root ≤ 8, `last`, trail ≤ 20 newest-first, deduped by `call_id`), `trailTarget` (definition `path#Lstart` → outline `path` → first focus → impact `old..new` → first `files`), `codeEdited` (`edit`/`write_file` → root-relative path under its root, ≤ 200), `codeFromHistory` (`code_*` in history → `roots = ["?"]`).
- `src/store/sessions.ts:232-234` `codemap`, `codemapPanel`; `:357-360, 366, 381` history load; `:505-511` `setCodemapPanel`. `src/store/reduce.ts:50-51` defaults, `:593-594` `case "CodeToolRan"`, `:794` edit tracking. `src/rpc/events.ts:36, 91-93` `WireEvent |= CodeToolRan | CodeReconciled`.
- `src/lib/codemap/api.ts` — `rpcCodemap(client)` calls `codemap_here/fit/change/touch` by flat name through `RpcClient.call` (so `METHODS`/`methods.json` stay the engine's); `mockCodemap()` serves the fixtures (`mockFit` re-centres on a neighbour so a smoke can watch survivors); `codemapMock()` = `VITE_CODEMAP_MOCK=1` or `localStorage.codemap-mock`.
- `src/components/codemap/__fixtures__/{here,fit,change,events}.json` — the artifact contract with the engine worker, shaped exactly as §5 (fit: 20 nodes, 19 edges/7 dashed, `ambiguous 3`, `more_callers 14`; change: 3/12/4/8 with `unindexed`); `generate.ts` `bigHere(n)` for the perf tests.

Header and hosts
- `src/lib/header.ts:40, 66, 79` `HeaderId 'codemap'`, `headerOrder` slot. `src/components/chat/Header.tsx:171, 196, 253, 319` availability, priority `artifacts → codemap → browser → terminals`, button, phone Sheet mount.
- `src/components/codemap/CodeMapHost.tsx` — `useCodemapAvailable` (`codemap.roots.length > 0`, the `browser`/`terminal` pattern), `CodeMapButton` (toggles `codemapPanel` on the last root; dot while `last.seq` is unseen), `CodeMapPane` (docked right of the chat leaf, 6 px drag strip, 360–920 px clamped to `innerWidth/2`, `codemap-panel-width` persisted, default 480), `CodeMapSheet` (< 768 px bottom Sheet). `src/routes/c.$session.tsx:57-65` wraps `LiveSession` in `CodeMapPane`; `:93-95` dev replay hook.

Lenses
- `src/lib/codemap/labels.ts` — `BADGE`/`GRAPH_STATE` (changed→danger, impacted→active, transitive→muted, test→done, stale→hatch), `kindGlyph` (`ƒ ◇ ≡ # =`), `shortLabels` (shortest distinguishing suffix), `anchorOf`, `basename`.
- `src/lib/codemap/lens.ts` — §6.1 machine: `lensFor`, `onCodeToolRan` (errors never switch; pinned keeps the lens and moves `auto`), `pin`, `follow`, `replay`.
- `src/lib/codemap/layout.ts` — `columnsLayout` (three fixed columns, focus centred, bundle nodes last, ORTHOGONAL single-bend edges, `×N` labels, dashed < 1.0, file grouping `×N`, repeated names carry the file suffix), `bandsLayout` (four bands + test lane as `groups`, pills wrap at width, caps with `+N in <dir>` bundles, `edges: []`), `placeSlot`/`Slots` (a node keeps its `(layer, index, w, h)` for the life of the picture; freed slots are never reused, so nothing slides).
- `src/lib/codemap/here.ts` — `buildHereRows` (dir = first two segments, open when ≥ 3 % of rows or holding a focus/edited/marker file, ≤ 12 symbols then `+N more`, coverage dirs `(N files)`, ≤ 12 open dirs then `+N directories`), `applyHereDelta` (key-preserving), `focusMatches`.
- `src/components/codemap/Here.tsx` — `@tanstack/react-virtual`, 22 px rows, overscan 8, keys = moniker; `►` marker, `▎` focus bar, `~` edited, `→N ↑N new ◆entry test`; keyboard ↑/↓/←/→/Enter; `HereList` static form for phones/tests.
- `src/components/chat/surface/Graph.tsx:91-115, 127-138, 154, 169-185` — `GraphSvg` gains optional `markerId: string | null` (null = no `<marker>`, no `markerEnd`), `badges`, `hatch` (+ `patternId`), `nodeClass`; `GraphView` and its tests untouched.
- `src/components/codemap/Fit.tsx`, `Change.tsx`, `codemap.css` — graph forms ≥ 600 px panel width, list forms below; Change draws the selection's edges in an overlay `<svg>` (accent 2 px), never at rest; `.codemap-node { transition: opacity 200ms }` + `@starting-style` fade-in, none under `prefers-reduced-motion`.
- `src/components/codemap/CodeMapPanel.tsx` — status header (root `▾` when > 1 root, `files · symbols` from `codeindex_stats`, `fresh`/`stale · N pending`/`walk capped`/`computing…`, `N calls`, tabs with the auto-dot, `follow`, ⊡, ⤢, ✕), `suspect_root` banner + **Reconcile** (`codemap_touch(root, null)`), `1 of N candidates`, body, trail (collapsible; expanded ≥ 1080 px; click = replay as pinned; producing row underlined), detail line (`path:a-b · kind · name · #rank · →N ↑N · provenance confidence`). Data through one `createRenderQueue(…, 200)`; replies older than the shown `seq` dropped; `on('CodeReconciled')` filtered by root and by `reparsed ∩ visible`; `codemap_touch` 500 ms coalesced from `codemap.edited`; keyboard `1/2/3`, `p`, Escape. `PoppedCodeMap` sets `document.title = "CodeMap · <root> · <lens>"`.
- Interactions: `src/components/chat/Composer.tsx:79-83` `appendDraft(session, text, head)`; **Ask about this** appends `` `path#La-Lb` `` with a `[codemap <root> · <lens> · <focus>]` header only when the draft is empty; **How it fits** → `pin('fit', {symbol})`; **What changes** → `pin('change', {files:[path]})`; **Snapshot to board** → `src/lib/codemap/fence.ts` `toGraphFence` (≤ 12 nodes, `ref` on all, `parseSpec`-valid) → clipboard or `prefillDraft` ask.
- Pop-out: `src/lib/popout.ts:21-22, 33-34, 46-47, 64, 85-86`; `src/routes/codemap.$session.$root.tsx`; `routeTree.gen.ts` regenerated by the Vite plugin.
- Dev scaffold: `src/dev/codemap-fixture.ts` replays `events.json` through `dispatch` once per session when `VITE_CODEMAP_MOCK=1` (delete with E1).

## Numbers (§7, node/vitest on this Mac; the UI has no jsdom)

| Path | Budget | Measured |
|---|---|---|
| first paint, 2 000 rows | < 100 ms after data | `buildHereRows(bigHere(2000))` → 684 tree rows **0.32 ms**; `renderToStaticMarkup(HereList × 40 visible rows)` **6.78 ms**; sum ≈ 7 ms before React commit (virtualised: ~40 DOM rows) |
| delta applied | < 16 ms | `applyHereDelta` 2 000 rows / 50 changed **1.09 ms**, survivors identical objects |
| hand layout | < 1 ms | `columnsLayout`/`bandsLayout` over the fixtures run inside the 34 codemap tests in ≈ 2 s total incl. transform; not timed separately — O(n) over ≤ 60 nodes |
| motion | opacity only ≤ 200 ms | browser: `getComputedStyle([data-node]).transition === "opacity 0.2s"`; after a `CodeReconciled` refetch of the Fit fixture **0 of 20 nodes moved** (`getBoundingClientRect` before/after) |
| events | rAF + 200 ms | one `createRenderQueue(…, 200)` per panel; `codemap_touch` 500 ms |

Not measured: engine-side budgets (`codemap_*` warm < 30 ms, WORKTREE < 300 ms) — E2's job; the mock answers in ~0 ms.

## Tests written (all `vitest`, node env, `.test.ts`)

- `store/reduce.test.ts` (+2): `CodeToolRan: roots ∪= root, last is the newest call, trail newest-first with a target per tool, capped at 20`; `ToolCallStarted edit/write_file appends the root-relative path under its root; a path outside every root with no root is ignored`.
- `lib/codemap/lens.test.ts` (4): the §6.1 table (12 rows + `is_error` never switches), pinned keeps lens/updates auto, follow, replay.
- `lib/codemap/layout.test.ts` (7): `placeSlot` first-free-index; columns over the fixture (3 columns, grouping, bundles last, dashed, badges); `limit` folds into bundle counts; **stability** (reorder moves nothing, new caller takes the first free index, removed leaves a gap); `nodeSize`; bands (groups, states, hatch, glyphs, no edges); band caps → `+N in <dir>`.
- `lib/codemap/labels.test.ts` (4): `shortLabels` two `tools.rs`; glyph table; `anchorOf`; five states each with its badge word.
- `lib/codemap/fence.test.ts` (3): every lens snapshot passes `parseSpec`, ≤ 12 nodes, `ref` on all, labels ≤ 80; fit/change/here node states and edges.
- `lib/codemap/here.test.ts` (9): collapse rule, ≤ 12 symbols + more, coverage dirs, `+N directories`, marker/focus, delta identity, perf (prints the numbers above), fixture render (`ƒ start`, `→14`, `↑9`, `◆entry`, `►`, `▎`, `(412 files)`, no `→0`).
- `components/codemap/Fit.test.ts` (3): 20 `data-node` incl. `bundle:0/2`, exactly 7 `stroke-dasharray`, zero `marker-end`, `?3`, `+14 callers`, `impacted` badge; `FitList` counts and `+N more`; < 600 px → list.
- `components/codemap/Change.test.ts` (4): four groups with counts, no `<path` at rest, hatch on `lens.rs`, chip `worktree · since 22b3945`, footer; selection lights partners (`api.rs → [index.rs]`, `<import>` → all three) as overlay lines; `modeChip`; `ChangeList`.
- `lib/popout.test.ts` (+1): codemap label/url/round trip with a root containing spaces; no root → null; `popoutSession`.

Browser smoke (Vite dev + mock, headless Chromium, 1440×900 / 960×720 pop-out / 390×844 iPhone UA): button appears after the first `CodeToolRan`; Here with `~`, `◆entry`, badges, trail; `code_refs` → Fit (no arrowheads, dashed package/unique, `impacted · ?3`, `test`, `+14 callers`, `+1 callees`, `×2`/`×4`); `code_impact` → Change (bands, `worktree · since 22b3945`, hatch `impacted · stale`, footer, selecting `api.rs` lights `index.rs`, detail `via Stats, reconcile · depth 1`); `2` pins Fit with the auto-dot on `change` and a `follow` button; right-click → menu → **Ask about this** puts `` [codemap theuth · fit]\n`crates/agent/src/loop_.rs#L3790-L3900` `` in the composer; ⊡ → Copy fence → clipboard holds a 12-node Graph fence with `ref` on every node; pop-out URL opens the panel alone, titled `CodeMap · theuth · here`, no sidebar, no ⤢; 390 px → bottom Sheet, list forms, no svg.

## Deviations

1. **Side pane, not Sheet** (Sky's ruling) — `CodeMapPane` wraps each chat leaf in `SessionRoute.render`; `Panes`/`lib/panes.ts` unchanged (the split tree still holds sessions only). Default width 480 (not 640): the pane shares the leaf with the transcript.
2. **`▶` → `►`** — the repo's `no-emoji.test.ts` rejects U+25B6 (`Extended_Pictographic`); the marker is U+25BA. Same glyph shape, no emoji font fallback (S §2.2 colour/glyph rule intact).
3. **Trail rows keep `args` and `summary`** (§4 listed `target` only) so a trail click can replay the call's lens (§6.1) without the engine; rows are also deduped by `call_id` (a replayed event must not double the trail).
4. **Removed nodes unmount immediately** (§6.6 rule 5 asked for a fade-out). Fade-in is `@starting-style`, positions never animate; a fade-out would need a "leaving" set and a timer for a ≤ 200 ms effect — skipped, add if it reads as flicker.
5. **Fit re-centre is a new picture** (§6.6 rule 6): slots reset on a new focus, so §11's "no node translation" is met as *no animated translation* (transition is opacity-only) — the browser assertion of unchanged rects was run on the same-focus refetch path, where it holds for 20/20 nodes.
6. **Repeated symbol names in a Fit column carry their file's shortest suffix** (`ƒ register scrape/src/tools.rs`); the design drew file boxes with the symbol inside — five identical `ƒ register` boxes were unreadable in the smoke (S §2.2 "text is the map").
7. **Header counts (`files · symbols`, `N calls`) hide under 520 px panel width** and the ⤢ button is absent in the phone Sheet; the design hid ⤢ only via `canPopout()` (iOS shell). A plain phone browser still cannot pop a Sheet out usefully.
8. **Perf is measured in node** (`renderToStaticMarkup` + pure functions); the repo has no jsdom and no prior perf-test convention (grep for `performance.now` in tests found none) — the numbers are pre-commit costs, the 65 as-built's 15.3 ms React commit is the remaining budget.
9. **`Slot` gained `w`/`h`** so bands can wrap by the widths of reserved slots; `bandsLayout` stacks bands (a row added to band k shifts k+1 down) — within a band nothing moves, and Change pictures are re-keyed per mode/range anyway.
10. **`GraphSvg` gained four optional props** rather than an overlay for badges/hatch — one renderer (65 §0.4), existing callers untouched.
11. `codeindex_stats(root)` against the current engine fails for the fixture root (not indexed) → the header shows no counts; with the mock flag this is expected, the query is `staleTime: Infinity` and invalidated by `CodeReconciled`.

## Waiting on engine (E1–E2)

- `CodeToolRan` / `CodeReconciled` on `subscribe_events` (`bridge.rs`, `export.rs`) — until then the slice fills only from the dev replay; the `ts_rs` `CodeindexEvent.ts` should replace `lib/codemap/types.ts`'s event types (import in `rpc/events.ts`).
- `codemap_here / codemap_fit / codemap_change / codemap_touch` RPCs — `rpcCodemap` already calls them by name; when they land, add them to `Api`/`METHODS` in `rpc/api.ts` (the `api.test.ts` methods.json check) and switch `useCodemapApi` to `api` if preferred. `codemap_fit(symbol)` is called with the definition **moniker** when known, else the tool's `symbol` name — E2 should accept both (§5.3 monikers).
- `lens.rs` types → `ts_rs` bindings replacing the hand-typed lens types; `IndexStats` is already used for the status header.
- Fields from siblings (`importers`, `callers`, `recent`, `entry`, `test_link`, `suspect_root`/`hint`): the UI hides badges at 0/`null` (§5.4), so absent fields render as nothing, not zero.
- Engine budgets (§7 warm < 30 ms, WORKTREE < 300 ms, `cache_hit`) — not observable from the UI yet.

## Not built

- Fade-out of removed nodes (dev. 4); the ≥ 1080 px Fit "context column" listing every caller file (§6.9) — the bundle node's tap → list form covers it; matrix fallback, treemap (§12).
- `←/→` column navigation inside the Fit svg (nodes are `role=button tabIndex=0` via `GraphSvg`, Tab order works; arrow keys only in Here).
- Screenshots at three widths are in this session's transcript, not committed (no binary assets in `docs/`).
- Real-engine smoke (`code_map` in a scratch chat → button): needs E1; the mock replay stands in.
