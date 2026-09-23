# 65 — As built: Graph component and mermaid 12, steps 1–4

Status: as-built, 2026-09-23, branch `feature/graph-diagrams`. Steps 1–4 of 65 §9; step 5 (steering measurement) not started. Every number below was measured on the M3 Max in a headless Chromium driven against the Vite dev server (dev build) unless marked "production build"; nothing was measured on iOS, Windows or Linux (see Unverified).

## Step 1 — mermaid 12 on the desktop (`0899ff1`)

Files: `crates/desktop/ui/src/components/chat/Mermaid.tsx` (lazy import, `CONFIG`, theme observer, queue-driven `useRender`), `components/chat/DiagramFrame.tsx` (pan/zoom, toolbar, fullscreen — extracted verbatim from `Mermaid.tsx`, content is either svg markup or React children), `lib/render-queue.ts` (`createRenderQueue`), `lib/render-queue.test.ts`, `lib/diagram-export.ts` (renamed from `mermaid-export.ts`, same functions), `lib/diagram-theme.ts` (`diagramTokens()` reads the CSS variables once per theme change), `lib/theme.ts` (`matchMedia` evaluated on use, not at import), `vite.config.ts:68`, `tauri.ios.conf.json:10`, `gen/apple/desktop.xcodeproj/project.pbxproj:307,369`.

- `mermaid ^12.0.0` installed (`12.0.0`); `@streamdown/mermaid` removed. `bun install` resolved: `streamdown` 2.6.0's installed `package.json` does not depend on it (the `workspace:*` link is monorepo-internal), so nothing had to be kept.
- Production build (`bun run build`) chunk report — mermaid and ELK are outside the entry:

  | chunk | size |
  |---|---|
  | `mermaid.core-*.js` | 86.65 kB (gzip 29.44) |
  | `mermaid-*.js` | 29.50 kB (gzip 3.64) |
  | `mermaid-parser.core-*.js` | 16.58 kB (gzip 3.92) |
  | `elk-276RUBZZ-*.js` | 1,456.41 kB (gzip 453.23) |
  | entry `index-*.js` | 3,283.75 kB (gzip 1,026.85) |

  `grep -c` of `mermaidAPI`, `flowchart-v2`, `elk.layered` in the entry chunk: 0, 0, 0. The pre-change entry size was not built (the pre-change tree cannot install against the new lockfile without a second `node_modules`); 30 §1/65 §2.1 record that mermaid 11 was in the main chunk before.
- Smoke (throwaway page mounting `MermaidFence`, since deleted): a two-subgraph flowchart renders with ELK — `elk-*.js` is fetched only when the first diagram mounts, `aria-roledescription="flowchart-v2"`, 2 `.cluster`, 6 `.node`, 0 `<foreignObject>`, node labels in the app font stack. Warm re-renders (theme class flipped four times): 57, 57, 75, 47 ms. Dark palette verified by screenshot.
- Queue: vitest `render-queue.test.ts` — 50 requests over 400 ms with an instant renderer → between 3 and 6 renders, last request rendered last; a superseded render sees `isCurrent() === false`; `dispose` drops the trailing request. In the browser the same 50 edits produced 1 committed render whose content was the 50th edit: a dev-build mermaid+ELK render of that chart took ≈600 ms, so every intermediate render was superseded before it could commit (the "trailing latest wins" path), never displayed stale.
- `bun run test`: 253 passed (48 files). The `pairing.test.ts` failure seen once was a flake (3/3 green in isolation).

Deviations: `themeVariables` map `primaryColor: --card`, `primaryTextColor/primaryBorderColor/lineColor: --foreground/--primary`, `secondaryColor/clusterBkg: --muted`, `tertiaryColor: --card`, `clusterBorder: --border`, `edgeLabelBackground: --background` — the brief listed the variables, not the pairing; `--primary` is the ink colour, so it paints borders and lines, not fills. `theme.ts` gained a one-line laziness fix because `MessageAssistant → Mermaid.tsx → theme.ts` now reaches node-run tests (`matchMedia` undefined at import).

## Step 2 — `Graph` in `crates/surface` (`0b848f7`)

Files: `crates/surface/src/spec.rs:419-532` (`GraphProps`, `GraphNode`, `GraphKind`, `GraphState`, `GraphEdge`, `GraphEdgeStyle`, `GraphGroup`, `GraphDirection`; `Component::Graph` at `:76`), `lib.rs:42-49` (`MAX_GRAPH_NODES` 120, `MAX_GRAPH_EDGES` 240, `MAX_GRAPH_GROUPS` 12, `MAX_GRAPH_LABEL_CHARS` 80), `validate.rs:239-311` (`graph()`), `graph.rs` (`to_mermaid`, `adjacency_lines`), `render.rs:20-33` (`spec_lines` / `spec_lines_text`), `render.rs:566-572` (the arm), `flatten.rs:17` (text channels take the list), `catalog.rs` (19 names), `prompt.rs:85-88,96-97`, `visualize.md:15,24-26,46,55-75`, fixtures `fixtures/json-render/graph-{architecture,dependency-lr,pipeline-tb,broken-unknown-endpoint,broken-dup-id}.json` (the over-cap case is generated in `validate.rs` tests).

- `cargo test -p surface`: 33 passed; `cargo clippy -p surface --all-targets`: no warnings.
- Compact signature as the generator spells it (optionals sorted, optional arrays marked after the bracket): `Graph(nodes[{id, label, group?, kind?, state?}], edges[{from, to, label?, style?}], direction?, groups[{id, label}]?)`.
- Every valid fixture's `to_mermaid()` draws under merman at 100 columns (the fixture oracle); adjacency snapshot at 40 columns is asserted verbatim in `graph.rs`; `render.rs` asserts box drawing at 100, the list at 40, and `flatten` → the list.

Deviations: `actor` lowers to `(("label"))` (circle), not `[/"label"/]` — merman 0.7 returns `UnsupportedFeature` for the parallelogram (bisected: every other shape, `subgraph x["l"]`, `-.->|"l"|`, self-loops and `#quot;`/`#lt;` entities draw), and `to_mermaid` has no consumer but merman, so emitting a shape it cannot draw would only turn every graph with an actor into the adjacency list. The bindings under `bindings/` and `crates/desktop/ui/src/rpc/bindings` are gitignored, so "regenerate" is a local step (`just ts-desktop`), not a commit; between the step-2 and step-3 commits the desktop `catalog.test.ts` is red against regenerated bindings (18 ≠ 19), by construction of the step order.

## Step 3 — desktop `Graph` renderer (`1490e16`)

Files: `crates/desktop/ui/src/lib/graph-layout.ts` (`structuralHash`, `wrapLabel`, `LruCache`, `layout()`, `toElk`/`fromElk`), `lib/graph-layout.worker.ts` (ELK's own worker: `import 'elkjs/lib/elk-worker.min.js'`), `lib/graph-layout.test.ts`, `components/chat/surface/Graph.tsx` (`GraphView`, `GraphSvg`, `adjacencyText`), `Graph.test.ts`, `__fixtures__/graph-*.json`, `catalog.ts:14-29,82` (`graphProps`), `parse.ts:14,52,61-90` (closed-fence structural checks at the Rust paths), `registry.tsx:253`, `catalog.test.ts:125` (19).

- `elkjs@0.9.3`: one install under `node_modules/elkjs`; mermaid's `^0.9.3` resolves to it (`bun pm ls`).
- Production build: ELK appears in two lazy, non-entry chunks — `elk-276RUBZZ-*.js` 1,456.41 kB (mermaid's main-thread copy) and `graph-layout.worker-*.js` 1,423.26 kB (the layout worker). Entry `index-*.js` 3,298.42 kB (+14.7 kB over step 1: `Graph.tsx`, `graph-layout.ts`, `elk-api`). `org.eclipse.elk` occurs in exactly those two files.
- Measured (dev build, headless Chromium, M3 Max), `graph-architecture` (6 nodes, 7 edges, 2 groups): ELK in the worker, warm, five runs after a warm-up: 12.2, 20.7, 15.0, 11.9, 12.7 ms → **median 12.7 ms** (first call 19.9 ms). `layout()` with an unchanged hash: 0.1, 0.0, 0.6, 0.0, 0.0 ms (cache hit, worker untouched). React re-render of the smoke page (mermaid + three graphs) to the next frame after a state bump: median 15.3 ms. Export of the React svg through `standalone()`/`rasterize()`: 983×172, PNG 57,010 bytes. Dark theme and fullscreen (Copy PNG · Save PNG… · Save SVG… · Reset view · Close) verified by screenshot.
- `bun run test`: 264 passed (50 files) — includes hash stability under label whitespace, LRU eviction at 65, the three fixtures rendered through `GraphSvg` with a fixed layout, byte-identity of the fixture copies, highlight/onSelect painting, and the broken fixtures rejected by `parseSpec` at the Rust paths.

Deviations: the worker is ELK's own (`elk-api` + `workerFactory` → a module worker that is `elk-worker.min.js`), not a worker of ours importing `elk.bundled.js`: inside a Worker `elk.bundled.js` detects `typeof document === 'undefined'` and installs itself as ELK's worker instead of exporting the class (`_Worker is not a constructor`), and Vite bundles a worker as its own module graph anyway, so "one chunk shared with mermaid" is not reachable with layout off the main thread — one copy per thread is what ships. Node colours are inline `fill`/`stroke` attributes read from the CSS tokens (`diagramTokens()`), not Tailwind classes: the exported svg/PNG has no stylesheet to inherit from. `active` uses `--fg-accent` (the `Timeline` dot vocabulary), not `--primary`, which is the ink colour and would not differ from the default stroke. Edge coordinates from ELK are relative to the edge's container (the LCA: the group when both ends share one), so `fromElk` offsets those by the group origin — the doc's "root coordinates" needed that one rule. `parse.ts` gained the Graph structural checks so the desktop rejects the same closed fences as Rust (30 §8); open fences still drop dangling edges without error.

## Step 4 — `Graph` in apps (`68575df`)

Files: `packages/apps/src/library.ts:92-296` (`graphProps` with `superRefine`, `GraphHostProps`, `setGraphRenderer`, `graphText`, `bindValue`, the `Graph` component, `LOCAL_COMPONENTS`), `library.ts:312-321,369,429-430` (local components join `keep`; their refs join every child union that holds component refs), `src/check.ts:271-290` (`$value` scoped to a Graph's `onSelect`), `src/index.ts`, `test/graph.test.tsx`, `test/fixtures/graph/*.json`, `generated/{library.json,catalog.txt,library-hash.txt}`, `crates/app/apps.md:149-164` (walkthrough example), `crates/desktop/ui/src/components/apps/AppHost.tsx:43` (`setGraphRenderer(GraphView)`).

- Library hash: `8dc9c11b7273e72b9e7feabddc5b976fb2714c2afaed2e907b6db838e2cc46c4` → `8b1fac40c6fe65b8a674d92f4b066f8e8c13b50833ac5b633ed2d8bb62b3ae5f`. Every committed app re-checks on next open (46 §8.3).
- Catalog line: `Graph(nodes: {…}[], edges: {…}[], groups?: {id: string, label: string}[], direction?: "LR" | "TB", highlight?: $binding<string[]>, onSelect?: ActionExpression)`; `Card.children` now lists `Graph` (`Stack.children` is `any[]` upstream and accepted it already).
- Spike: `steps[$step].ids` (index a literal array by state, then member access) evaluates as written; the walkthrough steps through its three highlight states, clamps at both ends, and a node press runs `@Set($selected, $value)` then `@ToAssistant` (the host's `onAction` receives `continue_conversation`). `check()` passes the walkthrough with no diagnostics; a `$value` outside a Graph `onSelect` is still `state-undeclared`.
- `bun test` (packages/apps): 88 passed, 0 failed (7 files); `bunx tsc --noEmit` clean. Desktop after `AppHost.tsx`: `tsc -b` clean, 264 vitest passed. `cargo test -p app` (embeds the new hash and catalog): 32 passed — `helper::tests::wrong_hash_or_missing_binary_never_serves` failed once under full-machine load and passed 1/1 in isolation (it spawns shell scripts; unrelated to the hash).

Deviations: the id reaches the action as **`$value`**, not `id`. A bare `id` is a `Ref`; upstream's materialiser lists it in `meta.unresolved` and the Renderer reports it, so the doc's `@Set($selected, id)` cannot publish without changing the parser. `$value` is upstream's own name for a component's produced value in binding expressions (`resolveStateField` evaluates the bound expression with `extraScope: { $value }`); the component substitutes it into every deferred step expression (`bindValue`) before `triggerAction`. Only `@Set` defers its value; upstream evaluates `@ToAssistant`/`@OpenUrl` arguments when the props are, so the skill example routes the selection through `$selected`. `reactive()` is applied inside `.describe()` (describe clones and would lose the WeakSet marker). The host renderer is injected (`setGraphRenderer`) because `packages/apps` cannot import `crates/desktop/ui`; without a host the component renders the adjacency text in a `<pre data-graph>`, which is what the headless helper and the tests see. The `superRefine` carries the structural rules for the parity test; the runtime never zod-parses props, so streaming partial graphs is unaffected.

## Unverified

- iOS 17.4 simulator, Windows (WebView2) and the Linux WebKitGTK ≥ 2.44 host: no rendering was attempted; the floors are set (`safari17`, `17.4`) but not exercised.
- `look: 'neo'` vs `classic` (65 §6): only `classic` was looked at.
- Pre-change entry-chunk size (before mermaid left it).
- Mermaid 12 cold first render including the chunk fetch: not timed separately from the smoke page's first paint; warm renders are 47–75 ms.
- `cargo check` of every `surface` dependent: `rpc` compiled (bindings export), `app` tests run; `theuth`/`tui`/`slack`/`signal`/`agent` were interrupted at the 900 s job limit under load. None of them matches on `Component` (`git grep Component::` outside `crates/surface` hits unrelated types) and the public signatures they call (`spec_lines`, `fence`, `flatten`, `mermaid_lines`) are unchanged.
- The 40-node fixture of 65 §5 does not exist; the 6-node `graph-architecture` was measured instead.
