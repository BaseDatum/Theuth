# 171 R1 — THEIRS recon: nteract elements, nteract desktop/runtimed, runtimed crates, @uiw/react-codemirror

Trees (shallow clones, read-only): `nteract-elements` 518db87 (2026-03-04), `nteract-nteract` 4524a86, `runtimed-runtimed` 2072564, `uiwjs-react-codemirror` 02e6b5a, under `/Users/Sky.Moore/Develop/spike-171/`.
Cite form: `tree/path:line` (tree = the clone directory name above) or a URL I read. "not found" = I looked and say where.
Report sections follow the R1 brief (1–6); section 7 maps them to the shared AXES 1–14.

## 0. Ten facts the design most depends on (details and cites in the numbered sections)
1. **nteract-elements has no licence** (no LICENSE file, no package.json field; §1.1). nteract/nteract (BSD-3) contains the production successors of the same components (`src/components/*`, §2); `@runtimed/components` is MIT on npm (§2). Licence is the first adopt blocker for the registry.
2. **Elements gives chrome, not a runner**: cells, gutter ribbon, `[▶]:` button, status badge, health dot, `OutputArea` + MIME router + ANSI; run keymaps are only Shift/Mod/Ctrl/Alt-Enter; **no run-line, run-selection, gutter-button or bash language** (§1.3-1.4). `CodeCell` itself is not in `registry.json` (§1.1).
3. **Output shape = nbformat v4 outputs** (`stream|display_data|execute_result|error`) and a MIME bundle with a priority list (§1.5); the Rust `nbformat`/`jupyter-protocol` crates carry the same shapes (§4). HTML/markdown/SVG/plots render only inside a blob-URL iframe without `allow-same-origin` (but CSP `connect-src *`) (§1.5).
4. **nteract's daemon owns kernels and the queue; the UI sends `ExecuteCell(cell_id)`; the daemon reads source from the synced doc; outputs and status are written into a runtime-state doc; the terminal status is written after the last output and consumers read the record, not a broadcast** (§3.1-3.3).
5. **Kernels are python and deno only (Jupyter, ZMQ); no bash**; one kernel per notebook room, language at notebook level; interpreters are daemon-managed uv/conda/pixi envs, not "find the user's python" (§3.2, §3.5).
6. **Stream handling is server-side terminal emulation** (alacritty_terminal; `\r`, backspace, 10 000-line scrollback; stdout/stderr interleave = segments) plus a 1 KB inline / blob split for large outputs; **no stdin (`input()`) support found** (§3.4, §3.12).
7. **Guarded execute**: `ExecuteCellGuarded{cell_id, observed_heads}` runs only if the cell still matches what the human saw; same guard on trust approval (§3.2, §3.6) — the superseding mechanism for chat cells.
8. **Safety in nteract is dependency-trust + env-value redaction at the storage boundary; agents may execute (MCP `execute_cell`, destructive+open-world)** — there is no human-only-run gate to copy; but "editor ≠ compute authority" is stated policy for shared docs (§3.6, §3.11, §3.4).
9. **MCP result doctrine**: agent channel = small summary + pointers, human channel = complete structured payload; `get_results` preview by default, `full_output` opt-in, waits capped at 50 s (§3.7).
10. **Jupyter crates are a codec + sockets, not a manager**: pure-Rust `zeromq` 0.7 (tokio + ring by default), BSD-3; `KernelspecDir::command` builds a `Command` you spawn; so a Rust engine can use them only for python/JS kernels and still needs its own process manager for bash (§4.1, §4.4). `@uiw/react-codemirror` is MIT, themeable via `EditorView.theme`/`createTheme`, has `bash`/`python`/`javascript` through `@uiw/codemirror-extensions-langs` (§5).

## 1. nteract elements inventory

### 1.1 Identity, install, status
- Registry name `nteract`, homepage nteract.io, 35 items, all `registry:component` or `registry:lib` (`nteract-elements/registry.json:1-7`; counted by parsing the file).
- shadcn registry, copy-into-repo: `pnpm dlx shadcn@latest registry add @nteract`, then `npx shadcn@latest add @nteract/media-router`; direct URL form `npx shadcn@latest add https://nteract-elements.vercel.app/r/media-router.json` (https://nteract-elements.vercel.app/docs, read; same in `nteract-elements/README.md`).
- Targets "React 19 (no `forwardRef` usage) and Tailwind CSS 4" (`nteract-elements/README.md`, Prerequisites). Caveat: `CellContainer` still uses `forwardRef` (`nteract-elements/registry/cell/CellContainer.tsx:30`), so the README claim is not literally true. package.json: react ^19.2.4, tailwindcss ^4.1.18, next 16.1.6, shadcn ^3.8.5 (`nteract-elements/package.json`); shadcn style `new-york`, baseColor `neutral`, cssVariables true, icon lib lucide (`nteract-elements/components.json`).
- Docs site says "Work in Progress — We're bringing the nteract components out of hibernation" and "Powers runt — a desktop notebook editor built with Tauri" (https://nteract-elements.vercel.app/docs).
- Maturity: the clone is depth-1 (`git log | wc -l` = 1), last commit 2026-03-04 "feat(cell): add search highlighting support (#197)" so ≥197 PRs/issues have existed; commit count and open issues are not visible from the tree. Tests: vitest unit (`registry/**/__tests__`), Playwright e2e `e2e/{isolated-frame,output-area,outputs,smoke,widgets}.spec.ts`, `integration-tests/registry-install.test.ts` (listing).
- **Licence: not found.** No `LICENSE*` at the root (`ls -a` of the tree), no `license` key in `nteract-elements/package.json`, no licence text in any md/mdx/json (grep for "MIT License|Apache License" empty). Legally unlicensed as it stands; ask upstream or treat as "all rights reserved" until a file appears. (The sibling repos nteract/nteract and runtimed/runtimed do ship LICENSE files, see §3/§4.)
- Registry gap: `CodeCell`, `MarkdownCell`, `CompactExecutionButton`, `useCellKeyboardNavigation`, `useEditorRegistry` exist in `registry/cell/` and the docs page says `<RegistrySetup name="code-cell" />` (`nteract-elements/content/docs/cell/code-cell.mdx:27`) but **no `code-cell` item exists in registry.json** (item names enumerated: ansi-output … selection-card; none is code-cell/markdown-cell). So a host installing from the registry gets pieces, not the assembled cell; copy `CodeCell.tsx` from the repo instead.

### 1.2 Inventory (registry.json items; line = item `name` line)
| Item | Purpose | npm deps pulled | shadcn/registry deps |
|---|---|---|---|
| ansi-output (`registry.json:8`) | ANSI→coloured spans; `AnsiOutput`, `AnsiStreamOutput`, `AnsiErrorOutput` | anser, escape-carriage | – |
| image-output | base64 / URL images | – | – |
| html-output | HTML, **throws unless inside an iframe** | – | – |
| svg-output | SVG | – | – |
| markdown-output | GFM + math + highlight | react-markdown, remark-gfm, remark-math, rehype-katex, rehype-raw, react-syntax-highlighter, katex | – |
| json-output | expandable tree | @uiw/react-json-view | collapsible |
| media-provider / media-router | MIME-bundle dispatcher + context | – | router pulls all renderers above |
| output-area | list of outputs, collapse, maxHeight, auto-isolation | lucide-react | media-router, ansi-output, isolated-frame, widget-store, markdown-output, error-boundary, highlight-text |
| cell-container | focus/selection wrapper with colour "ribbon" gutter | – | – |
| cell-betweener | ribbon continuity spacer | – | cell-container |
| cell-header | slot header (left/right, drag) | – | – |
| cell-controls | move/delete/clear menu | lucide-react | button, dropdown-menu |
| cell-type-button / cell-type-selector | code/markdown/sql/ai buttons + dropdown | lucide-react | button, dropdown-menu |
| execution-count | `[ ]:` / `[3]:` | – | – |
| execution-status | badge queued/running/error | – | badge |
| play-button | Play / Stop / Loading | lucide-react | – |
| runtime-health-indicator | idle/busy/connecting/disconnected + kernelName | lucide-react | – |
| collaborator-avatars, presence-bookmarks | presence UI | – | avatar, hover-card |
| codemirror-editor | CM6 editor wrapper (see §1.4) | @codemirror/{view,state,commands,autocomplete,language,lint,lang-python,lang-markdown,lang-sql,lang-html,lang-javascript,lang-json}, @uiw/react-codemirror, @uiw/codemirror-theme-github | – |
| widget-store, anywidget-view, widget-view, widget-controls (~55 files), ipycanvas | Jupyter ipywidgets / anywidget | – | many shadcn primitives |
| isolated-frame, isolated-renderer | sandboxed iframe + bundled renderer (vite build) | – | – |
| error-boundary, highlight-text, global-find-bar, runtime-icons, selection-card | utilities | lucide-react | – |
| all | meta-bundle of 22 items | – | – |
(All values from `nteract-elements/registry.json`, parsed; item descriptions are the registry `description` strings.) Dependency footprint is real: `package.json` lists 40+ runtime deps (fumadocs, katex, react-markdown stack…) though a host only takes what an item names.

### 1.3 What a host needs, by job
- **Cell chrome**: `cell-container` (+ `gutter-colors.ts`), `cell-header`, `cell-controls`, `execution-count` or the unregistered `CompactExecutionButton`, `execution-status`, `play-button`, `runtime-health-indicator`. All are plain React + Tailwind classes; no kernel logic.
- **Editor**: yes, a CodeMirror editor ships (`codemirror-editor`, §1.4). Run keymaps ship in `useCellKeyboardNavigation` (not registered).
- **Outputs**: `output-area` + `media-router` + `ansi-output` (+ `isolated-frame` only if HTML/markdown/SVG/plot output must render).
- **Terminals**: no terminal component. ANSI-to-spans only; no PTY/xterm in the registry (item list; grep for `xterm` in `nteract-elements/registry` returns nothing — see §1.5).
- **Sidecars**: README says the library targets "terminals, and sidecars" but no sidecar item exists in registry.json (item names above).

### 1.4 Editor facts (elements)
- `CodeMirrorEditor` wraps `useCodeMirror` from @uiw/react-codemirror with `basicSetup: false`, `indentWithTab: false`, extensions assembled by the wrapper (`nteract-elements/registry/editor/codemirror-editor.tsx:213-222`). Props: value, language, keyMap, extensions, baseExtensions, readOnly (via `EditorView.editable.of(false)`, :182-184), theme light/dark/system, lineWrapping, maxHeight (:38-69). Ref API: focus, setCursorPosition, getEditor (:29-36).
- Dark-mode follow: MutationObserver on `<html>` attributes `class|style|data-theme|data-mode` plus `prefers-color-scheme` (:119-145).
- Theme is `githubLight`/`githubDark` from @uiw/codemirror-theme-github (`registry/editor/themes.ts:19-27`), **not** shadcn CSS variables; only `notebookEditorTheme` makes the background transparent and removes the focus outline (`registry/editor/extensions.ts:27-52`). Re-theming to theuth tokens means replacing this with an `EditorView.theme` that reads `var(--…)`.
- Keymaps (`registry/cell/useCellKeyboardNavigation.ts:104-140`): Shift-Enter = execute + focus next, Mod-Enter and Ctrl-Enter = execute, Alt-Enter = execute + insert cell below, Mod-Shift-f = format. There is **no run-current-line, no run-selection, no gutter run button** (grep of `registry/` for "selection" in the keymap file finds none; the only gutter is the cell-level `CompactExecutionButton`, `CodeCell.tsx:231-238`).
- Languages: python (indent 4), ipython (cell-magic aware), markdown, sql, html, javascript, typescript, json, plain (`registry/editor/languages.ts:23-68`). **No bash/shell**: `%%bash` cell magic maps to "plain" "since we don't have shell lang" (`registry/editor/ipython.ts:89-90`). A shell mode must come from @codemirror/legacy-modes (see §5).
- Core setup = history, drawSelection, bracketMatching, closeBrackets, rectangularSelection, defaultKeymap, historyKeymap, completionKeymap, lintKeymap (`registry/editor/extensions.ts:58-77`); `minimalExtensions` drops autocomplete (:83-112).

### 1.5 Outputs: data shape and rendering
- **Output data shape = nbformat v4 output objects**: `execute_result|display_data {data: Record<mime, unknown>, metadata?, execution_count?}`, `stream {name: stdout|stderr, text: string|string[]}`, `error {ename, evalue, traceback: string[]}` (`nteract-elements/registry/cell/OutputArea.tsx:33-50`). `CodeCellData = {id, source, outputs, execution_count|null}` (`registry/cell/CodeCell.tsx:23-28`). So a native theuth protocol must adapt to this shape (or the wrapper replaces `OutputArea`).
- MIME dispatch: `DEFAULT_PRIORITY` = widget view, plotly, vega-lite v3-5, vega, geo+json, text/html, text/markdown, svg, png, jpeg, gif, webp, application/json, text/plain (`registry/outputs/media-router.tsx:43-66`); first present key wins, else first non-null (`OutputArea.tsx:150-162`). Custom `renderers` map and `priority` override are props (`OutputArea.tsx:76-88`).
- **HTML sandboxing**: types text/html, text/markdown, image/svg+xml, widgets, plotly, vega* go to iframe isolation; "if any output needs isolation, ALL outputs go to the iframe" (`OutputArea.tsx:134-145, 182-191`). `HtmlOutput` itself throws if not inside an iframe (`registry/outputs/html-output.tsx:36-49`) and runs scripts via `createContextualFragment` (:54-60). The iframe is a `blob:` URL with sandbox `allow-scripts allow-downloads allow-forms allow-pointer-lock allow-popups allow-popups-to-escape-sandbox allow-modals` and explicitly **no `allow-same-origin`** "this would give the iframe access to the parent's origin and Tauri APIs" (`registry/outputs/isolated/isolated-frame.tsx:139-153`; `sandbox={SANDBOX_ATTRS}` :478). Inside, CSP is `default-src 'self' blob: data:; script-src 'unsafe-inline' 'unsafe-eval' blob: https:; …; connect-src *` (`registry/outputs/isolated/frame-html.ts:51`) — scripts may fetch anywhere, so the sandbox protects the host DOM, not the network. Parent↔frame channel is `postMessage(…, "*")` (`isolated-frame.tsx:246-289`, `frame-html.ts:481`).
- **ANSI/stream**: `anser` with `use_classes: true`, after `escapeCarriageReturn(fixBackspace(input))` (`registry/outputs/ansi-output.tsx:158-165`) — so `\r` overwrite and backspace are collapsed per text blob (a pure function of the whole string; incremental append would re-run it). 16 standard colours use CSS vars; 256/truecolour inline rgb (:202-207). stdout styled `text-gray-700 dark:text-gray-300`, stderr `text-red-600 dark:text-red-400` (:248-250) — hard-coded palette colours, not shadcn tokens. Error block: left border red + ename/evalue + traceback in smaller red (:279-297).
- Images, SVG, JSON tree, Markdown (KaTeX + syntax highlight): separate renderers (registry items above). DataFrames: via `text/html` only, which means iframe; no native table renderer item exists in registry.json (item list §1.2), although `jupyter-protocol` has a typed `DataTable` MIME (§4.2).
- Long output: `OutputArea` has `maxHeight` (scroll) and `collapsed` + toggle (`OutputArea.tsx:58-68, 505-518`), iframe `maxHeight ?? 2000`. No line cap, no virtualisation, no "N lines hidden" truncation found (grep for `MAX_|truncate` in OutputArea.tsx: none besides maxHeight).
- Widgets: ~55 shadcn-backed ipywidget components + anywidget ESM loader + `use-comm-router` for comm_open/msg/close (`registry.json` items widget-store, widget-controls, anywidget-view). Needs a Jupyter comm channel; irrelevant to bash/python/JS cells unless a Jupyter kernel is in play.

### 1.6 Theming
- Components use shadcn semantic tokens (`bg-muted`, `text-muted-foreground`, `text-destructive`, `hover:text-foreground`) in cell chrome (`registry/cell/CompactExecutionButton.tsx:44-55`, `CodeCell.tsx:244`), the standard shadcn variable set in `app/global.css` (`--background … --ring`, `--sidebar-*`, `--radius: 0.625rem`, `nteract-elements/app/global.css:16-50`) — identical names to theuth's expected `--background/--foreground/--muted/--card/--border/--primary/--accent/--radius`. 
- Not tokenised: `CellContainer` ribbons use fixed Tailwind palette (`bg-sky-400`, `emerald`, `amber`, gray) (`registry/cell/gutter-colors.ts:19-59`); `ExecutionStatus` blue/red palette (`registry/cell/ExecutionStatus.tsx:29,40`); `RuntimeHealthIndicator` green/amber/blue/red (`registry/cell/RuntimeHealthIndicator.tsx:15-26`); stream colours above. These need a find-and-replace pass to theuth tokens.
- 24 extra variables `--ansi-black … --ansi-white-bg` defined in `:root` and `.dark`, mirrored into registry.json `cssVars` (`nteract-elements/app/global.css:53-76`; rule in `nteract-elements/contributing/css-variables.md:3-44`).

### 1.7 Install closure (computed from `registry.json` `registryDependencies` and `dependencies`; nothing installed)
| If the host installs | Registry items pulled | shadcn ui | Files | npm packages |
|---|---|---|---|---|
| `ansi-output` only | 1 | – | 1 | anser, escape-carriage |
| cell chrome (`cell-container, cell-header, cell-controls, execution-status, play-button, runtime-health-indicator`) | 6 | button, badge, dropdown-menu | 7 | lucide-react |
| `codemirror-editor` | 1 | – | 7 | 14 (`@codemirror/{view,state,commands,autocomplete,language,lint,lang-python,lang-markdown,lang-sql,lang-html,lang-javascript,lang-json}`, `@uiw/react-codemirror`, `@uiw/codemirror-theme-github`) |
| `media-router` | 8 | collapsible | 9 | 11 (react-markdown, remark-gfm, remark-math, rehype-katex, rehype-raw, react-syntax-highlighter + types, katex, anser, escape-carriage, @uiw/react-json-view) |
| `output-area` | 13 (adds widget-store, isolated-frame, error-boundary, highlight-text) | collapsible | 27 | 12 |
- **Hidden cross-item dependency**: `registry/editor/themes.ts` imports `@/lib/dark-mode` (`nteract-elements/registry/editor/themes.ts:9`), but `lib/dark-mode.ts` is shipped as a file of the `markdown-output` item (`nteract-elements/registry.json:164-165`), not of `codemirror-editor`; installing the editor alone leaves an unresolved import. `markdown-output.tsx` also imports it (`markdown-output.tsx:15`). Plan on copying `lib/dark-mode.ts` by hand (it is in `nteract-elements/lib/`).
- A text-only run view (stdout/stderr/error) therefore needs `ansi-output` (2 npm packages, 1 file) — the MIME router, markdown, KaTeX and the iframe stack are only needed once rich outputs are in scope.

### 1.8 Search, find, presence (axis 6 bits)
- `global-find-bar`: floating notebook-wide find, Enter/Shift+Enter navigation, `lucide-react` only (`registry.json` item description; `registry/cell/GlobalFindBar.tsx`, 139 lines).
- Search plumbing: `CodeCell` forwards `searchQuery`/`searchActiveOffset` to an editor `searchHighlight` extension (`CodeCell.tsx:218-225`), and `OutputArea` forwards it into the iframe (`OutputArea.tsx:446-451`) or highlights in-DOM (`:453-467`) via `highlight-text`; match counts return through `onSearchMatchCount`. A theuth note/notebook search can index source and outputs separately.
- Presence: `collaborator-avatars` and `presence-bookmarks` are display-only (avatar + HoverCard); no sync logic (registry items `registryDependencies: avatar, hover-card`).
- Import/export: elements has none (UI only). nteract uses the `nbformat` crate for `.ipynb` (`runtimed-runtimed/crates/nbformat/src/lib.rs:103-152`) and `.ipynb` save "also formats cells" on explicit save (`crates/runtimed/AGENTS.md:11`).


## 2. @runtimed/components

- **Not in either nteract tree.** `grep -rn '@runtimed/components'` over `*.json|*.md|*.ts|*.tsx` of `nteract-nteract` finds no file (command run; empty). nteract-nteract's own `packages/` are `local-oidc, notebook-host, odometer, pyodide-runtime, runtimed, runtimed-node, runtimed-wasm, sift, sift-csv` (directory listing).
- **Where it lives**: npm package `@runtimed/components`, licence **MIT**, repository `github.com/runtimed/anode` (homepage `…/anode#readme`), "React components for rendering notebook cell outputs", peer React 19+ (https://www.npmjs.com/package/@runtimed/components, read). Components: `SingleOutput` (router), `OutputsContainer`, `RichOutputContent`, `PlainTextOutput`, `MarkdownRenderer`, `HtmlOutput`, `JsonOutput`, `ImageOutput`, `SvgOutput`, `AnsiOutput`, `GeoJsonMapOutput`, AI tool components, an iframe integration and UI components (same page). It takes an `output` object with an `id` (`outputs.map(output => <SingleOutput key={output.id} output={output}/>)`), i.e. anode's own output record, not raw nbformat. I did not install or open the package source.
- **Relation to elements**: same family (runtimed org), earlier generation (anode = the LiveStore-era app). nteract/elements' docs site says it "Powers runt" and calls itself the shadcn-native reboot (https://nteract-elements.vercel.app/docs). `@runtimed/components` ships a stylesheet (`@runtimed/components/styles.css`) rather than copy-in source; no shadcn registry.
- **A third generation lives inside nteract/nteract**: `src/components/{cell,editor,outputs,runtime,…}` plus `apps/elements` (a Next/fumadocs site with ~40 example components and `content/docs/*.mdx`) (`nteract-nteract/apps/elements/package.json`, `ls apps/elements/components`, `ls src/components`). This is the production code, ahead of the standalone elements registry: `CodeCellCurrentLine`, `CompactExecutionButton`, `LanguageMark`, `RuntimeStatusDot`, `traceback-output.tsx`, `safe-mime-types.ts`, `javascript-output.tsx`, plotly/vega/geojson/pdf/audio/video outputs (`ls src/components/{cell,outputs,runtime}`). The docs page `apps/elements/content/docs/cell-execution-language.mdx:8-27` describes the run lane as "notebook punctuation first and status copy second", and says attach/trust problems "belong in host/runtime chrome before the run lane becomes available" — a useful rule for theuth's cell header. Licence of nteract/nteract: BSD-3-Clause (`nteract-nteract/LICENSE:1`).
- Main-DOM allowlist (nteract/nteract): only `text/plain`, raster images, `application/json`, `application/vnd.nteract.traceback+json`, `text/latex`, audio/*, video/* render without an iframe; everything else defaults to iframe isolation (`nteract-nteract/src/components/outputs/safe-mime-types.ts:8-34`). Stricter than the standalone elements list (§1.5), which isolates a named set and lets unknown types through.


## 3. nteract desktop + runtimed daemon architecture

### 3.1 What it is
- "local-first notebook environment where humans, kernels, and AI agents can work with the same live document"; ships as Tauri desktop app + `runtimed` daemon + `nteract` CLI + Python bindings (`nteract-nteract/README.md:3,33-40`). Built on jupyter-zmq-client and jupyter-protocol (`README.md:5`). Licence BSD-3-Clause (`LICENSE:1`).
- Daemon is per-user, installed as launchd (macOS) / systemd user (Linux) / Startup script (Windows), managed with `nteract daemon start|stop|status|logs` (`nteract-nteract/crates/runtimed/AGENTS.md:253-260`).
- **Runtime model in one paragraph** (`README.md:51-55`): notebook content in an Automerge doc, live kernel state in a separate runtime-state doc; "the daemon owns kernel processes, execution queues, output capture, and the write path for execution results"; execution requests "name a synced `cell_id`; the daemon reads the cell source from the shared document". Rule: "To execute a cell, write it to the doc first, then request execution by cell_id. Pass code as a request parameter and you've introduced a correctness bug" (`crates/runtimed/AGENTS.md:9`).

### 3.2 Who owns kernels, and the pipeline
| Question | Answer | Cite |
|---|---|---|
| Who spawns kernels | Only the daemon; "Clients request kernel launch; they never spawn kernels directly" | `crates/runtimed/AGENTS.md:17` |
| Process layout | Each kernel runs under a separate `runtimed runtime-agent` subprocess that connects back as an Automerge peer | `crates/runtimed/AGENTS.md:19` |
| Kernel per what | One kernel per notebook room; room keys are UUIDs; `LaunchKernel{kernel_type: "python"\|"deno", env_source, notebook_path}` | `crates/runtimed/AGENTS.md:143`, `crates/notebook-protocol/src/protocol.rs:720-729` |
| Languages | **python and deno only**; anything else errors "Unsupported kernel type: … Supported types: python, deno". Python = `ipykernel_launcher`; Deno = `deno jupyter --kernel --conn <file>`. No bash kernel. | `crates/runtimed/src/jupyter_kernel.rs:1005-1010, 995-1004` |
| Wire to kernel | Jupyter ZMQ (IPC sockets on unix except Deno) | `jupyter_kernel.rs:1089-1096` |
| Queue | `RuntimeStateDoc` holds `executing_execution_id` + ordered `queued_execution_ids`; per-execution map `queued→running→done/error` with execution_count, success, source, outputs | `crates/runtimed/AGENTS.md:72-78` |
| Request set | `LaunchKernel, ExecuteCell, ExecuteCellGuarded, InterruptExecution, ShutdownKernel, RunAllCells, RunAllCellsGuarded, SendComm, GetHistory, Complete, SaveNotebook, SyncEnvironment, ApproveTrust …` | `crates/notebook-protocol/src/protocol.rs:700-900` |
| Stale-press guard | `ExecuteCellGuarded{cell_id, observed_heads}`: "execute a cell only if it still matches the frontend-observed state captured when a trust gate opened"; `RunAllCellsGuarded` likewise for the code-cell list | `protocol.rs:738-766` |
| Interrupt / restart | `InterruptExecution{}`, `ShutdownKernel{}` over RPC; lifecycle signals (`KernelIdle, ExecutionDone, CellError, KernelDied`) travel an unbounded channel drained before output work "so output pressure cannot delay interrupts or queue release" | `protocol.rs:747-751`, `crates/runtimed/AGENTS.md:21-31`, `docs/adr/execution-pipeline.md:60-98` |
| Liveness | Heartbeat 5 s / 3 failures; no wall-clock watchdog by design ("Detecting Divergence, Not Policing Time") — long runs outlive the window | `docs/memos/execution-liveness.md:23-45` |
| Idle | After all peers disconnect, a delayed (default 30 s) check may shut down the kernel; room persists; peerless kernel-less rooms reaped after 24 h or earlier under a 32-room soft cap. Hosted runtime idle TTL is a constant 30 min and counts only execution activity, not open tabs | `crates/runtimed/AGENTS.md:151-157`, `docs/adr/runtime-idle-ttl-lazy-launch.md:28-61` |
| Terminal env | Kernels get `COLUMNS=128`, `LINES=100` | `jupyter_kernel.rs:1014-1016`, `crates/runtimed/src/terminal_size.rs:10-13` |

### 3.3 Documents (the CRDT layout)
- **NotebookDoc** (Automerge, schema v5): `cells/{cell_id}/{id, cell_type "code|markdown|raw", position (fractional index), source (Automerge Text), execution_count (legacy), metadata map, resolved_assets, attachments}`; `metadata/{runtime, kernelspec, language_info, runt/…}` (`crates/notebook-doc/src/lib.rs:12-42`). Language is a **notebook-level** property (`metadata.runtime`, kernelspec), not per cell.
- **RuntimeStateDoc**: separate doc, frame `0x05`; regular clients read but do not author it. **CommsDoc** `0x09` for widget state; **CommentsDoc** `0x0a` (`crates/runtimed/AGENTS.md:64-70`). Why split: `docs/adr/document-split.md` (372 lines; not read in full).
- Outputs live in RuntimeStateDoc keyed by `execution_id` then `output_id`, not in the notebook doc (`notebook-doc/src/lib.rs:14-16`). **There is no separate "execution result" message**: outputs, counts and terminal status are all RuntimeStateDoc writes; `set_execution_done` is "the commit boundary" written after every output manifest; consumers poll the doc (50 ms) and then wait up to a 500 ms grace for the last outputs (`docs/adr/execution-pipeline.md:11, 40-54`). Broadcasts are advisory (`:30-44`).
- Persistence: `.ipynb` is a checkpoint, autosave 2 s quiet / 10 s max; crash recovery from append-only `.recovery` journals; unknown metadata preserved on round-trip (`crates/runtimed/AGENTS.md:11, 142-149`).

### 3.4 Output storage, size, ANSI
- Manifests are inline Automerge maps with `ContentRef` per MIME: `{"inline": …}` for text ≤ 1 KB (`DEFAULT_INLINE_THRESHOLD = 1024`, `crates/runtimed/src/output_store.rs:97`), else `{"blob": hash, "size": N}`; all binary MIME always blobs, base64-decoded before storage (`crates/runtimed/AGENTS.md:89-136`).
- Blob store: content-addressed, sharded by 2 hex chars, `.meta` sidecar `{media_type,size,created_at}`, under the daemon base dir (`~/.cache/runt/blobs/` on stable), mark-and-sweep with 30-day grace; HTTP server `127.0.0.1:<dynamic>/blob/{hash}` with `Cache-Control: immutable` (`crates/runtimed/AGENTS.md:127-138`).
- **Stream = terminal emulation in the daemon**: stdout/stderr go through `alacritty_terminal` per `(execution_id, stream, segment)` so `\r` progress bars, backspace and cursor moves are resolved server-side; scrollback capped at 10 000 lines; interleaving of the other stream seals a segment so `stdout, stderr, stdout` = three outputs (`crates/runtimed/src/stream_terminal.rs:1-47, 21-23`). Periodic flushes are lossy (bounded mpsc 32, drop on full) but ordering boundaries use an unbounded priority path (`docs/adr/execution-pipeline.md:121-143`; `STREAM_COMMITTER_QUEUE_CAPACITY = 32` cited at `:112`).
- Display-update coalescing by `display_id`; `MAX_PENDING_DISPLAY_IDS = 128` (`execution-pipeline.md:113`).
- **Redaction at the storage boundary**: one `OutputRedactor` per launched kernel built from the kernel's effective environment values (min length 8), Aho-Corasick match, replaced by `[redacted env]` *before* text enters RuntimeStateDoc or the blob store (`crates/runtimed/src/output_redaction.rs:1-24, 16-17`). Non-goals stated: no secret inference from text, candidates never exposed via MCP or synced docs (`docs/memos/runtime-redaction-refresh-design.md:30-36`). Gap noted by their memo: secrets created inside the kernel after launch (dotenv, SDK refreshers) are invisible to it (`:7-17`).

### 3.5 Environments and interpreters
- Env-source vocabulary: `uv:prewarmed, conda:prewarmed, pixi:prewarmed, uv:inline, conda:inline, pixi:inline, uv:pyproject, pixi:toml, conda:env_yml, conda:existing:local, uv:pep723, pixi:pep723, deno` (asserted in `crates/notebook-protocol/src/connection.rs:995-1041`).
- `kernel-env` crate: "Python environment management (UV + Conda)" — uv venvs via `uv`, conda via `rattler`, hash-keyed cache, prewarming; pixi module present (`crates/kernel-env/src/lib.rs:1-45`). Base packages `ipykernel, ipywidgets, anywidget, nbformat, pyarrow>=14, uv` (`docs/adr/kernel-env-trust.md:36-38`). Env hash = sha256 over sorted deps + resolver fields + per-notebook `env_id`, 16 hex chars (`:34`).
- Deno: system `deno` if ≥ min version, else a pinned, checksum-verified download cached by `kernel-launch` (`crates/kernel-launch/src/tools.rs:528-540, 580-610`).
- **Not found**: discovery of an arbitrary user-installed `python` (system, pyenv, `.venv`) by path: kernels run in daemon-managed envs; `conda:existing:local` is the only "bring your own" source, plus `ListCondaEnvironments` (`protocol.rs:702`). Looked in `connection.rs`, `protocol.rs`, `runtime_agent.rs` env_source handling.

### 3.6 Trust model (what is and is not gated)
- Gate is **dependency trust, not per-run trust**: opening a notebook and pressing Run can install declared packages and spawn a kernel "with full OS permissions", so unknown dep lists need approval; approval is per package, per machine, SQLite allowlist `trusted-packages.sqlite`, never in the synced doc (`docs/adr/kernel-env-trust.md:16-27, 51-60`). Daemon computes `needs_approval = !matches!(status, Trusted|NoDependencies)` into RuntimeStateDoc; the UI "never re-derives" (`:102, 143`).
- Stale-approval guard: approving with `observed_heads` fails with `GuardRejected "Dependencies changed while the trust dialog was open"` if a collaborator edited deps meanwhile (`:159`). Same for hot-sync (`:197`).
- Identity: principal (the human) vs operator (`desktop`, `tui:9d2b`, `agent:claude:s1`); "attribution must distinguish operators while the trust gate enforces the user" (`docs/adr/identity-and-trust.md:21, 66`). So an agent acting through MCP is the *same principal* and "the trust gate sees them as the same principal and authorizes both" (`:624`). Runtime peers may not write NotebookDoc and cannot create execution intent except through `ExecuteCell/RunAllCells` (`:161`).
- **Consequence for theuth**: nteract lets agents execute (MCP `execute_cell` annotated `destructive(true).open_world(true)`, `crates/runt-mcp/src/tools/mod.rs:224-230`). The human-press-only rule of design 171 has no analogue there; the closest borrowable pieces are `ExecuteCellGuarded(observed_heads)` and operator attribution.

### 3.7 Agents: MCP tools (28)
Registered in `crates/runt-mcp/src/tools/mod.rs:130-360`, dispatched at `:399-438`. Names with the description string:
`list_active_notebooks`, `list_notebooks`, `connect_notebook` (attach by path or id; handle has no TTL), `create_notebook` (ephemeral), `save_notebook`, `show_notebook` (open in nteract for the user; headless returns a no-display reason), `disconnect_notebook`, `inspect_notebook` (bounded cells; optional wait after cursor), `create_cell` (after_cell_id), `set_cell` (source/type; optionally execute), `delete_cell`, `move_cell`, `execute_cell` ("return execution_id, status, and compact output summary"; params `cell_id`, `timeout_secs`), `run_all_cells` (`timeout_secs` default 300, `wait`), `get_results` (`execution_id`, `timeout_secs` 0–50 never submits, `full_output` default false "preview mode to protect context budget"), `set_bokeh_property`, `interrupt_kernel`, `restart_kernel` ("clearing all state"), `manage_dependencies`, `replace_match`, `replace_regex`, `create_comment`, `reply_comment`, `resolve_comment`, `reopen_comment`, `wait_for_notebook_change`, `get_cell`, `get_all_cells` (params: `crates/runt-mcp/src/tools/execution.rs:27-59`).
- **Two-audience result doctrine** (adopted 2026-07-15): content blocks = agent channel (summarised text, pointers, sizes); `structuredContent` = human render contract, complete and URL-rich; "Does the agent get readable text without fetching anything? … Did anything add tokens … that only the renderer needs?" (`docs/memos/mcp-result-channels.md:5-12, 39-44`). Open question there: whether harnesses honour `audience` annotations (`:24-27`).
- Humans and agents share one live document: both are Automerge peers; "programmatic control observes the same notebook state a human is editing" (`README.md:55`); agent edits attributed via operator labels (§3.6); MCP `wait_for_notebook_change` / `inspect_notebook` give the observation path.

### 3.8 celld, Pages, multi-client
- **celld** is not a kernel daemon: it is `denoland/celld`, a JS runtime host under evaluation as the *hosted room substrate* (replacing Cloudflare Workers) — "Proposal… not an accepted deployment decision" (`docs/memos/celld-hosted-room-substrate.md:1-15`). Irrelevant to local cells.
- **Pages** is a separate "experiment in collaborative documents for people and agents": rich editor, HTTP API and MCP on one Automerge doc, own document model, "does not migrate or replace nteract notebooks" (`docs/memos/authenticated-automerge-app-substrate/experiment/pages/README.md:1-12`). No code execution.
- Multi-client: multiple windows = multiple Automerge peers of one room (`crates/runtimed/AGENTS.md:150`); closing the window does not stop the kernel and a reconnect reattaches to the same execution stream (`docs/memos/execution-liveness.md:33`); if the daemon restarts "clients reconnect and resync" (`AGENTS.md:7`). Execution-id idempotence: `ExecuteCell{execution_id?}` and a `ExecutionIdRejected` response exist (`protocol.rs:732-736, 979`).

### 3.9 Mapping to theuth concepts
| nteract | closest theuth concept | Note |
|---|---|---|
| runtimed daemon (per-user, launchd) | theuth engine (headless `t`) | both own state; engine already owns PTY jobs |
| room (UUID) | note / notebook document id | nteract: one kernel per room |
| NotebookDoc (Automerge) | Yjs/yrs note blocks (design 144) | outputs deliberately **not** in the source doc |
| RuntimeStateDoc | a run record (session-scoped, engine-written) | clients read-only |
| runtime agent subprocess | `start()` PTY job / Monty runtime | process isolation per kernel |
| ExecuteCell(cell_id) reads doc | Run(cell id) reads engine-held source | avoids "code as parameter" bug; chat cells have no doc, so the engine must snapshot source at press |
| ExecuteCellGuarded(observed_heads) | press carries a content hash/revision of the cell | direct fit for superseding |
| OutputRedactor on env values | secrets vault placeholders + output scrub | same idea: value-match, before persistence |
| two-audience MCP result | what Sky sees vs what the agent sees | adopt doctrine |
| MCP tools (agent executes) | **not mapped**: theuth cells are human-press-only | |

### 3.10 How nteract tells agents when to use it (axis 8)
- A Claude/Codex plugin ships a `repl` **skill**; its trigger is in the `description`: "Trigger this skill whenever you're about to run python3 -c, write a throwaway .py script, or chain multiple shell commands for data exploration, analysis, plotting, or iterative computation" (`nteract-nteract/plugins/nteract/skills/repl/SKILL.md:3`).
- Tool preference ladder: direct pi tools (`python`, `python_add_dependencies`, `python_save_notebook`) if present, else MCP (`create_notebook`, `create_cell`, `execute_cell`, `set_cell`, `inspect_notebook`, `get_results`) (`SKILL.md:10-16`). Do not fall back to `python3 -c` when tools are missing; ask the user to check the plugin (`:18-20`).
- Explicit **When NOT to use**: one-shot commands, running existing scripts, non-Python tasks (`SKILL.md:137-141`); **When to use**: exploring data, multi-step state-dependent computation, plots, iterating, "3+ `python3 -c` commands" (`:129-135`).
- Result handling rules given to the agent: `execute_cell`/`and_run` "already wait for an initial result"; if still active continue with `get_results(execution_id, timeout_secs=25)` — "executing again starts another run"; waits default 0 and cap at 50 s; "Canceling observation does not interrupt computation" (`SKILL.md:97-103`).
- Identity of the target: keep `notebook_id`; "Never infer your target from the last connect call… If a mutation's outcome is unknown, inspect the notebook and its execution results before repeating it" (`SKILL.md:84-87`).
- No equivalent of "prepare but do not run" found: grep for `don't run|do not run|prepare` in `plugins/nteract` and `crates/runt-mcp/src/tools/mod.rs` descriptions returned only unrelated hits (the skill is all about the agent running). `show_notebook` ("Open in nteract for the user", `tools/mod.rs`) is the only hand-off to the human.
- Interpreter choice by the agent: via `dependencies` argument on first call; the daemon picks env (`uv:inline` etc., §3.5). The agent never selects a python path (not found in tool schemas listed in §3.7).

### 3.11 Roles and who may run (axis 6/9)
- Hosted ACL scopes `viewer | editor | runtime_peer | owner`, capability sets: editor = viewer + write NotebookDoc fields; runtime_peer = lifecycle/output writes only, "cannot create execution intent"; owner = editor + publish + ACLs (`docs/adr/hosted-room-authorization.md:129-134`).
- **"`editor` means notebook-editing authority, not compute authority. Edit mode in the UI and an `editor` ACL row must not imply that the principal can spend hosted compute or create execution intent"**; run controls appear only if a live runtime_peer is attached and the connection is `owner`; a separate explicit execute capability is the stated long-term model (`hosted-room-authorization.md:136-152`). Direct precedent for design 171: sharing a note with editors must not grant Run.
- Untrusted edits: `CommentsDoc` ingress "validates change actor labels… stripping writes from scopes without comment authority" (`crates/runtimed/AGENTS.md:68`). Execution of someone else's edits is gated only by dependency trust (§3.6), not by author.

### 3.12 Looked for, not found (axes 4, 9, 10, 11)
| Thing | Where I looked | Result |
|---|---|---|
| `input()` / stdin prompts surfaced to the UI | grep `input_request|InputRequest|allow_stdin|stdin` in `crates/runtimed/src`, `crates/notebook-protocol/src` | only port bookkeeping (`jupyter_kernel.rs:182,1184,1312`); no input-reply handling → interactive prompts are not supported in nteract cells |
| Run-selection / run-line | `src`, `apps/notebook/src`, `crates/notebook/src` | none (§5) |
| Danger labels / confirm before run | `crates/runt-mcp/src/tools/mod.rs`, `docs/adr` titles | only tool annotations `destructive(true)`; no per-cell danger UI found |
| Per-run credential scan of what the agent sees | `output_redaction.rs`, `runt-mcp` | only env-value redaction (§3.4); no pattern scan (AKIA…, eyJ…) |
| Windows/other-surface specifics | `README.md:15-31`, `runtimed/AGENTS.md:253-260` | installer for macOS/Linux; Windows = Startup script; no chat-channel (Slack/Signal) surface in the repo tree list |
| Bash/zsh kernel | `jupyter_kernel.rs:1005-1010`, `kernel-launch/src/tools.rs` | not supported |


## 4. runtimed crates (runtimed/runtimed @ 2072564, 2026-10-29)

All licensed **BSD-3-Clause** (`runtimed-runtimed/LICENSE:1`; each `Cargo.toml` `license = "BSD-3-Clause"`). Workspace members: nbformat, runtimelib (deprecated shim), jupyter-zmq-client, jupyter-websocket-client, jupyter-protocol, jupyter-ysync, ollama-kernel, mybinder (`runtimed-runtimed/Cargo.toml:1-11`). Release profile is size-optimised (`opt-level='z'`, lto, `panic="abort"`) (`runtimed-runtimed/Cargo.toml:49-54`).

| Crate | Version | What | Direct deps (Cargo.toml) |
|---|---|---|---|
| jupyter-protocol | 2.0.3 | message structs, MIME bundle, connection info, kernelspec struct; transport-agnostic | async-trait, bytes, chrono, futures, serde, serde_json, uuid, thiserror (`crates/jupyter-protocol/Cargo.toml:9-17`) |
| jupyter-zmq-client | 3.0.1 | ZeroMQ client **and kernel-side** sockets, kernelspec discovery, `kernel.json`→`Command` | zeromq 0.7.0 (tcp+ipc), bytes, data-encoding, dirs, jupyter-protocol, ring *or* aws-lc-rs, tokio *or* async-dispatcher+smol, serde, uuid, thiserror (`crates/jupyter-zmq-client/Cargo.toml:10-35`) |
| nbformat | 3.0.0 | parse/serialize `.ipynb` | anyhow, serde, serde_json, uuid, chrono, jupyter-protocol, thiserror 1 (`crates/nbformat/Cargo.toml:9-16`) |
| jupyter-websocket-client | 3.0.0 | talk to a Jupyter *server* over WebSocket | async-tungstenite 0.35 (`runtimed-runtimed/Cargo.toml:27`) |
| jupyter-ysync | 0.3.0 | y-sync for Jupyter collaborative notebooks (`yrs 0.21`) | yrs 0.21 (`runtimed-runtimed/Cargo.toml:44`) |

### 4.1 Transport crate facts
- **Pure-Rust ZeroMQ**: dependency is the `zeromq` crate 0.7 (not libzmq / no C build), features `tcp-transport`, `ipc-transport`; default features `tokio-runtime` + `ring`; exactly one runtime and one HMAC backend must be chosen — `compile_error!` otherwise, and `--all-features` is unsupported (`crates/jupyter-zmq-client/src/lib.rs:3-12`, README "Installation"). Public types expose `zeromq` 0.7, so a host's own `zeromq` dep must also be 0.7 (README). Needs Rust 1.89 (`MIGRATION.md`, "ZeroMQ 0.7").
- Sockets: `create_client_{iopub,shell,control,stdin,heartbeat}_connection` and the kernel-side mirror `create_kernel_*` (`crates/jupyter-zmq-client/src/connection.rs:456-538`); `Connection<S>::{send(JupyterMessage), read()}` with HMAC key + session id (`:223-250`); `peek_ports(ip, n)` to reserve ports (`:86`); split send/recv halves for Dealer/Router (`:122-127`).
- **It does not spawn anything.** Starting a kernel is the caller's job: `KernelspecDir::command(connection_path, stderr, stdout) -> Command` substitutes `{connection_file}` in `argv`, applies `kernelspec.env`, sets stdin null — returns a `std::process::Command` for the host to spawn (`crates/jupyter-zmq-client/src/kernelspec.rs:24-61`). Discovery: `list_kernelspecs()` scans the standard system/user data dirs for `<dir>/kernels/<name>/kernel.json`, `find_kernelspec(name)`, and `*_with_jupyter_paths` variants that ask `jupyter --paths` (`kernelspec.rs:64-104`; `dirs.rs:14` `ask_jupyter`).
- `jupyter-protocol::ConnectionInfo {ip, transport: IPC|TCP, shell_port, iopub_port, stdin_port, control_port, hb_port, key, signature_scheme, kernel_name?}` — the connection-file JSON (`crates/jupyter-protocol/src/connection_info.rs:48-51, 110-123`). IPC puts a filesystem path prefix in `ip` (`:125-132`).

### 4.2 Message and media types
- `JupyterMessage{zmq_identities, header{msg_id, username, session, date, msg_type, version}, parent_header, metadata, content: JupyterMessageContent, buffers}` (`jupyter-protocol/src/messaging.rs:148-159, 286-298`). Content enum has 35 variants incl. `ExecuteRequest/Reply/Input/Result`, `StreamContent`, `DisplayData`, `UpdateDisplayData`, `ErrorOutput`, `Status`, `InterruptRequest/Reply`, `ShutdownRequest`, `InputRequest/Reply` (stdin), `Complete*`, `Inspect*`, `IsComplete*`, `Comm*`, `Debug*` (`messaging.rs:466-504`).
- `ExecuteRequest{code, silent, store_history, user_expressions, allow_stdin (default false), stop_on_error (default true)}` (`messaging.rs:923-933, 951-953`). Output correlation is by `parent_header.msg_id`.
- MIME bundle = `Media` (a flattened map) with a typed `MediaType` enum: Plain, Html, Latex, Javascript, Markdown, Svg, Png, Jpeg, Gif, Json, GeoJson, DataTable (`application/vnd.dataresource+json`, with `TabularDataResource`), Plotly, WidgetView/State, VegaLite v2–v5, Vega… (`jupyter-protocol/src/media/mod.rs:67-127, 195-204`; `media/datatable.rs`, 299 lines). A native table type exists (`DataTable`), which elements' `media-router` does not render natively (§1.5).
- Size of the protocol surface: `messaging.rs` 2 837 lines, `media/mod.rs` 788, `connection.rs` 930 (`wc -l`).

### 4.3 nbformat
- `parse_notebook(json)` dispatches on `(nbformat, nbformat_minor)`: 4.5 → typed `v4::Notebook` with a quirks-mode that fills fabricated cell ids and reports spec violations; 4.0–4.4 → `legacy`; 3.x → `v3`; else `UnsupportedVersion` (`crates/nbformat/src/lib.rs:103-124`). `serialize_notebook`, `upgrade_legacy_notebook`, `upgrade_v3_notebook` (`:152, 183, 240`). v4 `Output` enum = `stream{name,text} | display_data | execute_result | error` (`crates/nbformat/src/v4.rs:451-467`) — the same four shapes the elements `OutputArea` consumes (§1.5).

### 4.4 What a Rust engine needs to run a cell via these crates
Derived from the crate surface plus how nteract does it (`crates/runtimed/src/jupyter_kernel.rs:549-1200`):
1. A target interpreter with `ipykernel` (python) or `deno` (nteract's base package set includes `ipykernel`: `docs/adr/kernel-env-trust.md:36-38`; python launch uses `ipykernel_launcher` at `jupyter_kernel.rs:600`).
2. Reserve ports (`peek_ports`), write a connection file (`ConnectionInfo`), build the kernel `Command` (from a kernelspec, or by hand as nteract does for Deno `deno jupyter --kernel --conn <file>` `jupyter_kernel.rs:995-1004`), spawn it in the cwd (`cmd.current_dir(&cwd)` `:1012`).
3. Open shell + iopub (+ control for interrupt, + heartbeat) connections; send `ExecuteRequest`; read iopub messages whose `parent_header.msg_id` matches until `Status{idle}` and the `ExecuteReply` arrive.
4. Translate `StreamContent/DisplayData/ExecuteResult/ErrorOutput` into the host's output model.
**Answer to "can they be used without speaking the full protocol everywhere"**: yes — the crates are a codec + sockets; only the engine-side kernel manager needs them. Everything above the engine (RPC to UI, chat, note storage) can use a trimmed native shape. nteract itself does exactly that: the UI never sees ZMQ, it sees Automerge docs (§3.3). Cost of adopting: a Jupyter kernel gives persistent state, rich MIME and interrupt for Python/JS but **no shell language** (no bash kernel in nteract: `jupyter_kernel.rs:1005-1010`); the bash/zsh path would need its own process manager (theuth already has PTY jobs) or a third-party `bash_kernel` kernelspec that `list_kernelspecs()` could discover (not verified; no bash kernelspec is shipped in these trees).
- Dependency weight (declared, not measured; no build was run): jupyter-protocol is light (serde/chrono/uuid/bytes/futures); jupyter-zmq-client adds `zeromq` + `ring` + tokio; nbformat adds only jupyter-protocol. theuth already uses tokio/serde/uuid; the new transitive weight is `zeromq` 0.7 and either `ring` or `aws-lc-rs` (check theuth's lockfile for which TLS backend it already carries).
- Maturity: workspace is actively released (HEAD 2026-10-29 "Prepare Ollama 1.2.5 on the workspace Rustls reqwest (#334)"; `MIGRATION.md` documents 3.0.0 breaking bump to ZeroMQ 0.7); the crates are the same ones nteract desktop ships on (`nteract-nteract/README.md:5`).


## 5. @uiw/react-codemirror (clone 02e6b5a, 2026-09-25; package 4.25.12)

- Package `@uiw/react-codemirror` 4.25.12, licence **MIT** (`uiwjs-react-codemirror/core/package.json`; `uiwjs-react-codemirror/LICENSE:1`). theuth already pins ^4.25.11 (brief). Peer deps: `@codemirror/state >=6`, `@codemirror/view >=6`, `@codemirror/theme-one-dark`, `codemirror >=6`, react/react-dom `>=17`; own deps `@codemirror/commands`, `@codemirror/state`, `@codemirror/theme-one-dark`, `@uiw/codemirror-extensions-basic-setup`, `codemirror` (same file). It is a thin wrapper: no Yjs, no collab, no gutter/run API — `grep -rl 'yjs|y-codemirror|collab'` over the repo's md/package.json found nothing.
- **Props** (`core/src/index.tsx:16-84`): `value`, `height/minHeight/maxHeight/width…`, `autoFocus`, `placeholder`, `theme: 'light'|'dark'|'none'|Extension` (default light), `basicSetup: boolean|BasicSetupOptions`, `editable`, `readOnly`, `indentWithTab` (default **true**), `onChange(value, viewUpdate)`, `onUpdate`, `onCreateEditor(view, state)`, `extensions`, `root` (ShadowRoot), `initialState` (JSON), `selection`. Ref exposes `{editor, state, view}` (`:86-90`). `useCodeMirror` hook is exported for custom containers (`:12`); elements uses it that way (§1.4).
- **Read-only vs non-editable**: `editable=false` adds `EditorView.editable.of(false)`; `readOnly` adds `EditorState.readOnly.of(true)` (`core/src/getDefaultExtensions.ts:57-62`). Read-only keeps selection/copy; use for chat-presented cells until "Edit" is pressed.
- **basicSetup options** (`extensions/basic-setup/src/index.ts:29-55`): lineNumbers, highlightActiveLineGutter, foldGutter, dropCursor, allowMultipleSelections, indentOnInput, bracketMatching, closeBrackets, autocompletion, rectangularSelection, crosshairCursor, highlightActiveLine, highlightSelectionMatches, closeBracketsKeymap, searchKeymap, foldKeymap, completionKeymap, lintKeymap, tabSize (default 2). Setting it `false` (as elements does, §1.4) lets the host assemble a lighter set.
- **Theming**: `theme` accepts any `Extension`; default light theme is just `EditorView.theme({'&':{backgroundColor:'#fff'}})` (`core/src/theme/light.ts`, `getDefaultExtensions.ts:44-56`); `'none'` removes it. `createTheme({theme:'light'|'dark', settings:{background, foreground, caret, selection, selectionMatch, lineHighlight, gutterBackground, gutterForeground, gutterActiveForeground, gutterBorder, fontFamily, fontSize}, styles: TagStyle[]})` builds an `EditorView.theme` + `HighlightStyle` (`themes/theme/src/index.tsx:6-100`). Settings are plain strings fed into CSS rules, so `var(--background)`-style values are possible by construction (I did not find an example in the repo; nteract's own production themes use fixed hex values per theme `classic|cream`, `nteract-nteract/src/components/editor/themes.ts:28-50`, and `highlight-styles.ts`). 30+ prebuilt themes (github, dracula, tokyo-night, nord, …) under `themes/` (listing).
- **Language packages**: `@uiw/codemirror-extensions-langs` maps names to CM6 modes; `bash`/`sh`/`ksh` → `StreamLanguage.define(shell)` from `@codemirror/legacy-modes/mode/shell`; `python`/`py` → `@codemirror/lang-python`; `js/mjs/cjs/jsx/ts/mts` → `@codemirror/lang-javascript` (`extensions/langs/src/index.ts:8,15,80,126,204,269,285`). Powershell also (`:69`). So `langs.bash()`, `langs.python()`, `langs.javascript()` are one-liners. (elements' own `languages.ts` has no shell, §1.4.)
- **Run keymaps**: not provided. Pattern is a `keymap.of([...])` extension (elements does this: `Shift-Enter`, `Mod-Enter`, `Ctrl-Enter`, `Alt-Enter`, `useCellKeyboardNavigation.ts:104-140`). Whole-cell run = `view.state.doc.toString()`; selection = `view.state.sliceDoc(from,to)` over `state.selection.ranges`; current line = `state.doc.lineAt(state.selection.main.head)` — these are CodeMirror 6 core APIs (`@codemirror/state`/`view`), which this package re-exports (`core/src/index.tsx:8-9`); I did not open the CM6 source (not in this clone), the calls are from the CM6 reference. nteract has **no** run-selection or run-line code: grep `runSelection|executeSelection|run current line|Run selection` over `src`, `apps/notebook/src`, `crates/notebook/src` returned nothing.
- **Gutter run buttons** (per-line ▶): CM6 core `gutter()`/`GutterMarker` (from `@codemirror/view`, re-exported here) is the mechanism; not shown in this repo or in elements (elements' only gutter is the cell-level `CompactExecutionButton` outside the editor, `CodeCell.tsx:231-238`). **not found** in elements or nteract's editor: grep `GutterMarker|gutter\(` over `nteract-elements/registry` and `nteract-nteract/src/components/editor` returned no file.
- **Behaviours that matter for streaming/many editors**:
  - README advice: "Define the extensions outside the component for the best performance" (`core/README.md:300`).
  - `onChange` serialises the full doc on every change (`useCodeMirror.ts:73`: `doc.toString()`), fine for cells, costly for huge docs.
  - The reconfigure effect depends on `extensions` identity (`useCodeMirror.ts:155-176`): pass a **memoised** array or the editor reconfigures every render.
  - A controlled `value` that differs from the doc replaces the whole document (`changes:{from:0,to:len,insert:value}`, annotated `ExternalChange`), deferred while the user is typing via a `TimeoutLatch` (`useCodeMirror.ts:178-199`; `core/src/timeoutLatch.ts`). Streaming output should not go through an editor `value`; render it in the output area.
  - Multiple editors per page / performance guidance: **not found** in `core/README.md` or code (grep for "performance|many editors"). nteract renders one `CodeMirrorEditor` per cell via `useCodeMirror` (`nteract-elements/registry/editor/codemirror-editor.tsx:213`) with no virtualisation seen in the registry.
  - Collaboration: no Yjs here; nteract/nteract's editor adds `remote-cursors.ts`, `presence-state.ts`, `text-attribution.ts` (`ls nteract-nteract/src/components/editor`) driven by its Automerge peer, not y-codemirror.
- **Bundle weight**: not measured (no build allowed). Structural facts: wrapper itself is a few hundred lines (`core/src` files: index.tsx 173 lines, useCodeMirror.ts 204, getDefaultExtensions.ts 65); weight comes from `codemirror` (basic setup), `@codemirror/theme-one-dark`, and each language (lazy import advisable; elements itself `lazy()`-loads renderers `media-router.tsx:4-22`, not the editor).


## 6. Adopt / borrow / avoid

### 6.1 Ranked for theuth (facts above; the ranking is my judgement)
**Adopt (copy in, restyle)**
1. `@uiw/react-codemirror` (already a dep) + `@uiw/codemirror-extensions-langs` for `bash` (legacy shell mode), `python`, `javascript`; `basicSetup:false` plus a theuth extension set; `theme` = one `EditorView.theme` reading theuth tokens; own `keymap` for Shift/Mod-Enter, run-line, run-selection (§5). Copy the *shape* of elements' `CodeMirrorEditor` (ref API, dark-mode observer, `readOnly` flag; `codemirror-editor.tsx:29-69, 119-145`).
2. `ansi-output` (+ `anser`, `escape-carriage`): stdout/stderr/ANSI/`\r`/backspace in ~300 lines; 16 colours already CSS variables (`ansi-output.tsx:202-207`, `global.css:53-76`). Replace the hard-coded `text-red-600`/`gray-700` stream colours (`:248-250`) with `text-destructive`/`text-foreground`.
3. `output-area` + `media-router` + `error-boundary` + `highlight-text`: nbformat-shaped output list with collapse, `maxHeight`, MIME priority and custom `renderers` (`OutputArea.tsx:33-88`). Wrap, don't fork: feed it `stream`/`error`/`display_data` items converted from theuth's run events. Skip `widget-*`, `anywidget-view`, `ipycanvas`, `isolated-renderer` for v1.
4. Cell chrome: `cell-container` (re-colour ribbons to theuth tokens; they are fixed sky/emerald/amber, `gutter-colors.ts:19-59`), `cell-header`, `cell-controls`, `execution-status` (re-colour), `runtime-health-indicator` (re-colour), `CompactExecutionButton` (`[▶]:` / `[■]` / `[3]:` states, `CompactExecutionButton.tsx:16-72`; copy from the repo, not in registry).
5. `isolated-frame` only if/when HTML/SVG/markdown outputs must render from a run (sandbox `allow-scripts` without `allow-same-origin`, blob URL; `isolated-frame.tsx:139-153`). For bash/python text outputs it is not needed; render `text/html` as escaped text or via the existing sandboxed-HTML component theuth has, if any.

**Borrow (technique, not code)**
6. **One doc / one source rule**: the engine reads cell source from its stored document at press time, never from the button's payload (`crates/runtimed/AGENTS.md:9`). For chat cells (no note), snapshot the source into the run record when the message is created and send only `cell_id` + revision.
7. **Guarded execute**: `ExecuteCellGuarded{cell_id, observed_heads}` — run only if the cell still matches what the user saw (`protocol.rs:738-766`). Maps to "Run" on a superseded chat cell.
8. **Output ordering contract**: terminal status is written *after* the last output; consumers read the record, broadcasts are hints (`execution-pipeline.md:30-54`). Directly applicable to "agent sees output once the run completes".
9. **Server-side terminal emulation for streams** (`\r` progress bars, backspace; segment per stream interleave; 10 000-line scrollback; `stream_terminal.rs:1-47`) instead of client-side regex clean-up; Sky's agents hand-stripped OSC/CSI and spinner frames in the 10-16 PTY runs (AXES user evidence).
10. **Redaction at the storage boundary** by value-matching the process env (min 8 chars, marker `[redacted env]`, before persistence; `output_redaction.rs:16-17, 1-24`). theuth can match the vault's secret values instead of env values.
11. **Two-audience result**: summary text for the agent, complete payload for the human renderer, pointers + sizes instead of dumping (`mcp-result-channels.md:5-12, 33-37`); `get_results` preview vs `full_output` (`execution.rs:49-59`).
12. **Run-lane language** and "attach/trust problems belong in host chrome before the run lane is available" (`cell-execution-language.mdx:8-17`).
13. **Operator attribution** (`desktop`, `tui:…`, `agent:claude:s1`): who authored a source edit vs who pressed run (`identity-and-trust.md:21, 66`).
14. **Lifecycle never starves behind output** (separate unbounded lifecycle channel, drain first; `execution-pipeline.md:60-98`): Stop/interrupt must stay responsive during output floods.
15. Env-source vocabulary as the interpreter chip text: `uv:…`, `conda:…`, `pixi:…`, `deno` (`connection.rs:995-1041`).

**Avoid**
- **Jupyter kernels as the only runtime**: python/deno only, no shell (`jupyter_kernel.rs:1005-1010`); bash/zsh need their own process manager; adding ZMQ + ipykernel per interpreter is heavy for "run this one-liner". Use jupyter crates only if persistent Python state with rich MIME is wanted, behind the engine.
- **nteract's agent-executes model**: MCP `execute_cell` is destructive/open-world and the daemon authorises agent and human as one principal (`identity-and-trust.md:624`; `tools/mod.rs:224-230`). Design 171 requires human-only press; nothing from nteract enforces that, build it.
- **Automerge/CRDT for cells in chat**: nteract's cost (RuntimeStateDoc, CommsDoc, blob server, 20-odd ADRs) is justified by multi-writer notebooks; chat cells need an append-only run record.
- **Widgets/anywidget/comm** stack (55+ widget files; `registry.json` widget-controls) — Jupyter-comm-dependent, no value for bash/python text cells.
- **Registry install of `all`** (22 items) and `isolated-renderer` (vite sub-build, `package.json build:isolated`).
- **Treating the standalone elements repo as stable**: "Work in Progress", no licence, 1 commit visible, `code-cell` missing from registry (§1.1); the production components moved into `nteract/nteract` (`src/components`, BSD-3).
- **iframe `connect-src *` + `allow-popups-to-escape-sandbox` as defaults** (`frame-html.ts:51`, `isolated-frame.tsx:151`): fine for notebooks of the user's own code, too open for agent-authored HTML.
- Elements' stream colours/ribbons as-is (hard-coded palette: ignores theuth themes).

### 6.2 Licence table
| Component | Licence | Obligation | Evidence |
|---|---|---|---|
| nteract-elements (registry items, repo) | **none found** | cannot rely on any grant; request upstream licence or reimplement | no LICENSE file, no `license` in package.json (`ls -a`; §1.1) |
| nteract/nteract (desktop, runtimed, `src/components`, `apps/elements`) | BSD-3-Clause, © 2024 runtimed | keep copyright + licence text in source/binary redistributions; no endorsement | `nteract-nteract/LICENSE:1-3` |
| runtimed crates (jupyter-protocol, jupyter-zmq-client, nbformat, …) | BSD-3-Clause | same | each `Cargo.toml license`, `runtimed-runtimed/LICENSE:1` |
| @runtimed/components | MIT | keep notice | https://www.npmjs.com/package/@runtimed/components |
| @uiw/react-codemirror | MIT, © 2021 uiw | keep notice | `uiwjs-react-codemirror/LICENSE:1-3`, `core/package.json` |
| @uiw/codemirror-extensions-langs / basic-setup / themes | MIT per the monorepo licence; individual package.json `license` fields not each read | keep notice | `uiwjs-react-codemirror/LICENSE` |
| anser, escape-carriage, react-markdown stack, katex, @uiw/react-json-view, lucide-react, shadcn/radix | not read in this recon (npm packages not opened) | verify with `npm view <pkg> license` / lockfile before adoption | `nteract-elements/package.json` lists them |
| `zeromq` 0.7, `ring` | not read | verify with `cargo` metadata when considered | `jupyter-zmq-client/Cargo.toml:11-14` |

### 6.3 Adopt-vs-build summary for the six design questions
- Q1 grammar: nteract's cell = `{id, source, outputs, execution_count}` in nbformat shape (`CodeCell.tsx:23-28`, `notebook-doc/src/lib.rs:25-35`); language is notebook-level, not per cell (§3.3) — theuth needs per-cell language, so build.
- Q2 kernel model: nteract = one Jupyter kernel per notebook room, daemon-owned, FIFO queue, state persists across cells (§3.2); theuth's bash/python/Monty are different runtimes → build a small engine-side "run" manager; copy lifecycle/ordering rules, not the Jupyter layer.
- Q3 press→run→output: doc-id + cell_id request, RuntimeStateDoc write path, poll/sync; copy the record-then-read discipline (§3.3/§6.1 #8).
- Q4 agent knows when to present: nteract has no analogue (agents run); not found in `crates/runt-mcp` tool descriptions or `plugins/nteract` (see §7 note).
- Q5 safety: dependency-trust gate + env redaction exist; human-only run gate does not (§3.6).
- Q6 adopt: CodeMirror (have), `ansi-output`, `output-area`/`media-router`, cell chrome (restyled); build the engine and the chat fence.


## 7. Axis map (AXES.md 1–14) — what this recon contributes per axis

| # | Axis | Contribution (section) | Strength |
|---|---|---|---|
| 1 | Cell model | nbformat-shaped `{id, source, outputs, execution_count}` (§1.5, §3.3); metadata = Automerge map; language at notebook level (§3.3). No options-as-comments or fence attributes in these trees | medium |
| 2 | Kernel/runtime | one Jupyter kernel per room, daemon-owned, FIFO queue, interrupt/shutdown RPC, 30 s peerless teardown, heartbeat divergence detection (§3.2) | strong |
| 3 | Languages/interpreters | python (managed uv/conda/pixi envs), deno only; no bash; env-source vocabulary; no user-python discovery (§3.2, §3.5); CM6 shell mode available via legacy-modes (§5) | strong for gaps |
| 4 | Output model | nbformat outputs + MIME router; ANSI via anser; daemon terminal emulation; 1 KB inline/blob split; iframe isolation set; no stdin prompts (§1.5, §3.4, §3.12) | strong |
| 5 | Editor | elements' CodeMirrorEditor, keymaps Shift/Mod/Ctrl/Alt-Enter; no run-line/selection/gutter; themes hard-coded github/classic/cream (§1.4, §5) | strong |
| 6 | Notes/notebooks integration | Automerge storage, `.ipynb` as checkpoint, roles and "editor ≠ compute" (§3.3, §3.11); nbformat crate for import/export (§4.3) | medium |
| 7 | Chat integration | not covered by these trees (nteract has no chat); only MCP result channels doctrine (§3.7) | weak (n/a) |
| 8 | Agent interface/prompting | MCP tool list with params, `get_results` preview/full, skill trigger and when-not rules (§3.7, §3.10) | strong |
| 9 | Safety/trust | dependency trust gate, guarded execute, env redaction, operator attribution, sandboxed iframe + `connect-src *` caveat, no human-only-run (§3.4, §3.6, §1.5) | strong |
| 10 | Streaming/multi-client/durability | state in docs not messages; window-independent runs; reconnect resync; lossy stream flush vs priority lifecycle; daemon restart = resync (§3.2, §3.3, §3.8) | strong |
| 11 | Surfaces | Tauri desktop + hosted web viewer (`apps/notebook-cloud`) + CLI + Python bindings + MCP; elements `PresenceBookmarks`; no TUI/Slack here (§3.1, §3.12) | medium |
| 12 | Cognitive load | `[▶]:`/`[■]`/`[n]:` single control, run-lane punctuation, "host chrome before run lane" (§6.1 #12; `CompactExecutionButton.tsx:16-72`) | medium |
| 13 | Prior art | nteract desktop + MCP agent REPL (agent runs; human watches same doc) (§3.7, §3.10) | one data point |
| 14 | Licences/weight | licence table, dependency lists, adopt/avoid (§1.1, §4, §6.2) | strong |

Notes on axis 7: nothing in these four clones renders a chat message or maps a button in a message to an engine request. Closest analogues are (a) `show_notebook` (agent → "open for user"), (b) `ExecuteCellGuarded` (stale-press guard), (c) the MCP two-audience result split (§3.7). A chat design must be built from theuth's own message rendering.


## 8. Appendix A — component APIs worth knowing (nteract-elements, as of 518db87)

### OutputArea props (`registry/cell/OutputArea.tsx:52-121`)
`outputs: JupyterOutput[]`, `collapsed?`, `onToggleCollapse?`, `maxHeight?` (px, enables scroll), `className?`, `renderers?` (per-MIME `({data, metadata, mimeType, className}) => ReactNode`), `priority?` (MIME order), `isolated?: boolean|"auto"` (auto = iframe if any output needs it), `preloadIframe?`, `onLinkClick?(url, newTab)`, `onWidgetUpdate?` (deprecated), `searchQuery?`, `onSearchMatchCount?`.
- `useWidgetStore()` returns `WidgetStoreContextValue | null` (`widget-store-context.tsx:132`), and `OutputArea` calls the nullable form (`OutputArea.tsx:321`), so a widget provider is **not** required unless widget MIME types appear. `IsolatedFrame` "**Requires** `IsolatedRendererProvider`" (`isolated-frame.tsx:162`), which fetches renderer JS/CSS assets (`isolated-renderer-context.tsx:92-97`) — a build step (`isolated-renderer` vite config) to ship.

### CodeCell props (`registry/cell/CodeCell.tsx:30-108`)
`cell: {id, source, outputs, execution_count}`, `language?` (default python; no bash in `SupportedLanguage`), `isFocused`, `isExecuting`, `onFocus`, `onUpdateSource`, `onExecute`, `onInterrupt`, `onDelete`, `onFocusPrevious/Next`, `onInsertCellAfter`, `onFormat`, `isLastCell`, `searchQuery/searchActiveOffset/onSearchMatchCount`, `extensions`, `className`. Layout: `CellContainer` with left gutter = `CompactExecutionButton`, right gutter = delete (hover), code row, output row with ribbon (`CodeCell.tsx:251-284`). Output area is rendered with `preloadIframe` always on (`:276-281`) — it creates a hidden iframe per cell, a cost to avoid in a page with many cells (not measured).

### CellContainer (`registry/cell/CellContainer.tsx:5-28`)
`id, cellType, isFocused, onFocus, codeContent, outputContent, hideOutput, gutterContent, rightGutterContent, customGutterColors, drag handlers, className`; emits `data-slot="cell-container"`, `data-cell-id`, `data-cell-type` (`:65-69`). `customGutterColors` lets a host add cell types ("bash") with its own ribbon classes (`gutter-colors.ts:1-13`, `CellContainer.tsx:23`). Note `isFocused && "-mx-16 px-16"` bleeds 4rem outside its parent (`:73`) — needs a wide-enough container in chat bubbles.

### Execution controls
- `CompactExecutionButton({count|null, isExecuting, onExecute, onInterrupt})`: `[▶]:` never run, `[3]:` run with hover-to-play, `[■]` pulsing while running; titles "Run cell"/"Stop execution"; `data-testid="execute-button"` (`CompactExecutionButton.tsx:23-73`).
- `PlayButton({executionState: idle|queued|running|completed|error, cellType, onExecute, onInterrupt, isAutoLaunching?, gutterMode?, …})` (`PlayButton.tsx:5-18`).
- `ExecutionStatus({executionState})`: null for idle; badges Queued / Running (spinner) / Error (`ExecutionStatus.tsx:11-47`).
- `RuntimeHealthIndicator({status: idle|busy|disconnected|connecting, kernelName?, onClick?, showStatus?})`; idle=green "Connected" (`RuntimeHealthIndicator.tsx:5-39`).
- `CellControls({sourceVisible, onDeleteCell, onClearOutputs, hasOutputs, toggleSourceVisibility, playButton?, onMoveUp/Down/Top/Bottom, canMoveUp/Down, onDeleteAllBelow, …AI-context props})` (`CellControls.tsx:25-45`) — AI-context props are nteract-specific and can be ignored.
- Keyboard map hook `useCellKeyboardNavigation` also handles ArrowUp/Down cell-to-cell navigation based on cursor position and Backspace-to-delete-empty-cell (`useCellKeyboardNavigation.ts:60-100`).

### Output-shape adapter theuth would need
Run events (theuth side, to design) → `JupyterOutput[]`: stdout/stderr text → `{output_type:"stream", name, text}`; non-zero exit or exception → `{output_type:"error", ename, evalue, traceback[]}`; images/tables → `display_data{data:{"image/png":b64, "text/plain":…}}`. `execution_count` maps to a per-cell run counter. (Derived from `OutputArea.tsx:33-50` and `jupyter-protocol` §4.2.)

## 9. Appendix B — nteract protocol and doc vocabulary (for naming only)
| Term | Meaning | Cite |
|---|---|---|
| room | one open notebook = one NotebookDoc + RuntimeStateDoc + CommsDoc + CommentsDoc, UUID keyed | `crates/runtimed/AGENTS.md:64-70, 143` |
| runtime agent | subprocess per kernel, Automerge peer | `AGENTS.md:19` |
| execution_id | per-run key in RuntimeStateDoc `executions`; may be client-supplied (`ExecuteCell.execution_id?`) | `protocol.rs:732-736`; `execution-pipeline.md:32-40` |
| observed_heads | CRDT heads the user saw; basis of guarded execute/approve | `protocol.rs:740-745`, `kernel-env-trust.md:159` |
| frame types | `0x00` NotebookDoc, `0x05` RuntimeStateDoc, `0x09` CommsDoc, `0x0a` CommentsDoc, `0x0b` keyed docs | `crates/runtimed/AGENTS.md:64-67` |
| daemon API gate | clients require semantic daemon API 3; wire protocol v4 | `kernel-env-trust.md:283` |
| pool / prewarm | pre-built envs (`uv:prewarmed`…) for fast kernel start | `connection.rs:995-1003`, `kernel-env/src/lib.rs:7-10` |
| channels | stable vs nightly namespaces (`runt` vs `runt-nightly`) for caches | `kernel-env-trust.md:53` |


## 10. Appendix C — safety and output details worth quoting
- **Runtime not sandboxed**: nteract states "runtime kernel/agent execution is not yet sandboxed" (issue #1307 referenced) — only package *installs* run in a sandbox context and use isolated caches (`nteract-nteract/docs/memos/env-sandbox-policy-design.md:37-49`). A kernel runs with the user's full OS permissions (`docs/adr/kernel-env-trust.md:16`).
- **Directory trust like IDE workspace trust**: a project file (`pyproject.toml`, `pixi.toml`, `environment.yml`) "is a program"; unit of consent = a directory, no content hash and no re-prompt on edits inside it (`crates/runtimed/src/project_trust.rs:1-13`). Relevant to a cwd-bound cell: trusting a cwd subtree is the IDE precedent.
- **Two redaction layers**: producer side (Python launcher scrubs env values from stream, display, execute-result, update-display and error text before send) is "an early hygiene layer, not the durable storage boundary"; daemon redacts before RuntimeStateDoc/blob writes; both skip short/common values and binary MIME, both can be disabled for debugging; runtime-created secrets stay best-effort (`docs/adr/traceback-output-protocol.md:145-176`).
- **Structured traceback MIME** `application/vnd.nteract.traceback+json`: `{ename, evalue, language, frames[{filename, lineno, name, library, lines[{lineno, source, highlight}], execution_id, source_hash, source_ref}], syntax, execution{execution_id, execution_count}, text, raw_text}`; additive-only; renderers prefer structured, fall back to `text`, keep `raw_text` for copy/export; deep stacks clipped to head + sentinel + tail; source windows small (`traceback-output-protocol.md:38-95, 145-157`). Rendered in the main DOM because it is "our own schema" (`src/components/outputs/safe-mime-types.ts:19-21`). The same idea (language-tagged structured error + plain fallback) would suit bash `exit N` + stderr tail.
- **Markdown outputs are raw-HTML capable**: `markdown-output` uses `rehype-raw` (`nteract-elements/registry/outputs/markdown-output.tsx:12,139`) and `SvgOutput` inserts SVG with `insertAdjacentHTML` (`svg-output.tsx:33-34`) — both are only safe because `OutputArea` routes `text/markdown` and `image/svg+xml` into the iframe (`OutputArea.tsx:134-145`). Embedding them directly in theuth's DOM would be an XSS path from any program output.
- **Output frame shell security notes** (nteract/nteract, Draft ADR 2026-10-15): shared frame has no notebook content or daemon credentials; sandbox `allow-scripts allow-downloads allow-forms allow-pointer-lock` (no `allow-popups…escape` in this later version, unlike elements' list); messages bound to `event.source === window.parent`; "not an offline sandbox. `connect-src *`… Forms and downloads are allowed"; native bridges must not be exposed to outputs (`docs/adr/output-frame-security.md:6-39`).
- **Hosted viewers** never auto-run: run controls need `owner` scope and a live runtime peer (`hosted-room-authorization.md:136-143`).
- **Interrupt responsiveness** was a real bug: a chatty progress bar made interrupt "do nothing for 2-5 seconds" until lifecycle signals got their own unbounded channel (`execution-pipeline.md:117-119`).
- **Output race** was a real bug: broadcast fired, the LLM read three of five outputs, the rest landed 20 ms later; fix = durable record + terminal status written last + 500 ms grace poll (`execution-pipeline.md:46-54`).

## 11. Open items I could not resolve from the trees
- Licence of nteract-elements (ask `nteract/elements` maintainers or treat as unlicensed).
- Whether `code-cell` is published at `https://nteract-elements.vercel.app/r/code-cell.json` (registry.json has no such item; I did not fetch `/r/` JSON).
- npm licences of anser, escape-carriage, react-markdown stack, katex, @uiw/react-json-view; crates `zeromq`, `ring` (not opened).
- Bundle sizes (no build allowed): CodeMirror core + 3 languages, `OutputArea` with lazy renderers, elements `isolated-renderer` bundle.
- Whether a `bash_kernel` kernelspec would run under `jupyter-zmq-client` + nteract's launch path (no shell kernelspec in the trees; unverified).
- How nteract's frontend maps a button press to `ExecuteCell` through the Tauri relay (read `crates/notebook`, `packages/notebook-host`; not traced here beyond the protocol types).
- Behaviour of two clients pressing Run on the same cell: queue is FIFO in RuntimeStateDoc and `execution_id` may be client-supplied with an `ExecutionIdRejected` response (`protocol.rs:732-736, 979`); the dedupe rule itself is not read.

## 12. Appendix D — production run-lane and ANSI details (nteract/nteract `src/components`, newer than the standalone elements)
- `CodeCellCurrentLine` props: `languageLabel`, `count|null`, `elapsedMs?`, `isExecuting`, `isQueued`, `queuePriority?`, `isErrored`, `isCancelled` ("dropped from the queue without running"), `isFocused`, `compactIdle`, `activityContent` (slot) (`src/components/cell/CodeCellCurrentLine.tsx:12-26`). State type `idle | ran | queued | running | error | cancelled` (`:28`). The running signal waits 120 ms before animating (`RUNNING_SIGNAL_DELAY_MS`), settles over 320 ms, honours `prefers-reduced-motion` (`:31-40`; page text `apps/elements/content/docs/cell-execution-language.mdx:19-23`). So the production lane shows **language label, run count, elapsed time, queue priority** at the source/result boundary — closest existing UI to design 171's "state, interpreter, cwd at a glance".
- Production `ansi-output` additionally **strips OSC sequences** (complete or tail-truncated, including OSC 8 hyperlinks) and residual C0/C1 control characters before rendering, keeping tab/newline (`src/components/outputs/ansi-output.tsx:180-196`); this is the hand-written regex cleanup Sky's agents did in the 10-16 PTY runs, moved into the renderer. It also has a `terminalFidelity` flag (`renderAnsiEntries(entries, terminalFidelity)` `:202-207`).
- Production `OutputArea`/`ansi-output` carry no line cap or "show more" (grep `truncate|maxLines|Show more|MAX_` found only CSS `truncate` at `ansi-output.tsx:474`); size control is the daemon's blob split and the MCP preview mode (§3.4, §3.7).

## 13. Appendix E — file index (what I opened, for the next reader)
| Tree | Files opened (line-checked unless noted) |
|---|---|
| nteract-elements | `registry.json` (parsed), `README.md`, `package.json`, `components.json`, `app/global.css`, `contributing/css-variables.md`, `registry/cell/{CodeCell,CellContainer,gutter-colors,CompactExecutionButton,ExecutionStatus,RuntimeHealthIndicator,OutputArea,PlayButton,CellControls,useCellKeyboardNavigation}.ts(x)`, `registry/editor/{codemirror-editor,extensions,themes,languages,ipython}.ts(x)`, `registry/outputs/{ansi-output,html-output,svg-output,media-router,markdown-output}.tsx`, `registry/outputs/isolated/{isolated-frame.tsx,frame-html.ts,isolated-renderer-context.tsx}`, `registry/widgets/widget-store-context.tsx` (grep) |
| nteract-nteract | `README.md`, `LICENSE`, `DESIGN.md` (head), `crates/runtimed/AGENTS.md`, `docs/adr/{execution-pipeline,runtime-idle-ttl-lazy-launch,kernel-env-trust,identity-and-trust,hosted-room-authorization,output-frame-security,traceback-output-protocol}.md`, `docs/memos/{execution-engines-and-marimo,execution-liveness,mcp-result-channels,runtime-redaction-refresh-design,env-sandbox-policy-design,celld-hosted-room-substrate}.md`, `crates/notebook-protocol/src/{protocol,connection}.rs` (grep + ranges), `crates/notebook-doc/src/lib.rs`, `crates/runtimed/src/{jupyter_kernel,stream_terminal,output_redaction,output_store,terminal_size,project_trust}.rs`, `crates/kernel-env/src/lib.rs`, `crates/kernel-launch/src/tools.rs` (grep), `crates/runt-mcp/src/tools/{mod,execution}.rs`, `plugins/nteract/skills/repl/SKILL.md`, `src/components/{cell/CodeCellCurrentLine,outputs/ansi-output,outputs/safe-mime-types,editor/themes}.ts(x)`, `apps/elements/content/docs/cell-execution-language.mdx` |
| runtimed-runtimed | `Cargo.toml`, `MIGRATION.md`, `LICENSE`, `crates/{jupyter-protocol,jupyter-zmq-client,nbformat}/Cargo.toml`, `jupyter-zmq-client/{README.md, src/lib.rs, src/connection.rs (grep), src/kernelspec.rs}`, `jupyter-protocol/src/{messaging,media/mod,connection_info}.rs` (grep + ranges), `nbformat/src/{lib,v4}.rs` |
| uiwjs-react-codemirror | `core/package.json`, `core/src/{index.tsx,getDefaultExtensions.ts,useCodeMirror.ts}`, `core/README.md` (grep), `themes/theme/src/index.tsx`, `extensions/basic-setup/src/index.ts`, `extensions/langs/{package.json,src/index.ts}`, `LICENSE` |
| URLs read | https://nteract-elements.vercel.app/docs ; https://www.npmjs.com/package/@runtimed/components |

## 14. Appendix F — press → request in the production UI (nteract desktop)
- The button handler generates the execution id **on the client** (`createClientExecutionId()`), tags the cell with it for performance tracing, then sends `executeCell(cellId, {executionId})`; if the daemon answers `execution_id_rejected` it logs a warning and retries without an id so the daemon assigns one (`nteract-nteract/apps/notebook/src/App.tsx:1536-1572`). The guarded variant `executeCellGuarded(cellId, provenance, {executionId})` has the same retry (`:1576-1613`). Design use: a press can mint the run id, attach the live output stream to it before the engine replies, and fall back safely on collision.
- Capability props gate controls: `CodeCell … canExecute={canExecuteCells}` and `onRequestExecute` appear in a source-shape test for `NotebookView` (`apps/notebook/src/components/__tests__/notebook-view-capabilities-source.test.ts:23-25, 69`) — read-only/viewer hosts render the cell without a run affordance. Matches the hosted rule that run controls need owner scope (§3.11).
- Run-all and per-cell paths both exist as daemon requests (`RunAllCells`, `RunAllCellsGuarded`; `protocol.rs:753-766`); the daemon "reads cell sources from the Automerge doc and queues them" (`protocol.rs:754`) — there is no client-side loop of N `ExecuteCell` calls for run-all, unlike the user's "sequential RPC calls" sketch in AXES.md; ordering and stop-on-error live in the queue.
- Elements' docs example shows what a host supplies: `onExecute={() => runCell(id)}`, `onInterrupt`, `isExecuting`, `onUpdateSource` (`nteract-elements/registry/cell/CodeCell.tsx:120-146`).


## 15. Appendix G — design questions × what these trees say (compact)
| Design question | nteract evidence | Gap for theuth |
|---|---|---|
| Q1 one cell grammar across chat/notes/notebooks/files | nbformat cell `{id, cell_type, source, metadata, outputs}`; `.ipynb` checkpoint; language per notebook (§3.3) | per-cell language + cwd + danger + expected-output are not in nbformat; metadata map can carry them (unknown keys preserved, `runtimed/AGENTS.md:11`) |
| Q2 kernel model & where in engine | daemon owns kernels, one per room, FIFO queue, interrupt/shutdown, idle teardown (§3.2) | bash/zsh state across cells needs a shell session manager; Python via Monty vs local interpreter is a theuth-only choice |
| Q3 press → run → live output → agent context | client-minted `execution_id`, `ExecuteCell(cell_id)`, record-then-read, `get_results` preview (§3.3, §3.7, App F) | chat cells have no doc id: snapshot at message creation; agent sees output only after completion (nteract's agent polls) |
| Q4 when to present, interpreter pick | skill trigger/not-trigger text (§3.10); daemon picks env from `dependencies` | no "prepare, don't run" analogue; theuth prompt must define it (user evidence in AXES.md) |
| Q5 safety without extra confirmations | dependency trust (once per package), guarded execute, env redaction, editor ≠ compute (§3.4, §3.6, §3.11) | human-only run and secrets placeholders are theuth rules; nteract has none of them |
| Q6 adopt vs build | adopt CodeMirror + ansi + output-area (restyled); borrow protocol ideas; build engine runner and chat fence (§6) | licence for elements must be resolved first |

## 16. Adoption traps (each is evidenced above)
1. Elements registry omits `CodeCell`/`MarkdownCell`/`CompactExecutionButton`; the docs mention `code-cell` install (§1.1).
2. `codemirror-editor` needs `lib/dark-mode.ts` shipped by another item (§1.7).
3. Editor theme is GitHub light/dark, not tokens; stream and status colours are hard-coded palette classes (§1.4, §1.6).
4. No bash language in elements' `SupportedLanguage`; `%%bash` degrades to plain (§1.4).
5. `OutputArea` pre-creates a hidden iframe per cell (`CodeCell.tsx:276-281`); with many cells in a chat that is many iframes (not measured).
6. `HtmlOutput`/`SvgOutput`/markdown raw HTML are only safe inside the iframe; rendering them directly is an injection path (Appendix C).
7. `@uiw/react-codemirror` reconfigures the editor whenever the `extensions` array identity changes; memoise (§5).
8. nteract's own lifecycle bugs (interrupt latency under output floods, partial outputs after a completion broadcast) are the failure modes to design out from the start (Appendix C).
9. Jupyter path gives no stdin prompts, no shell; Sky's pulumi arrow-key prompt (AXES.md) cannot be handled by it (§3.12).
10. Standalone elements is explicitly "Work in Progress" with a single visible commit and no licence (§1.1).

## 17. Method and self-check
- Method: read-only. Files opened with `read_file` (ranges), `grep` via `shell`, `registry.json` parsed in the runtime for inventory and install closure (§1.2, §1.7), two URLs read with `read_url`. No package manager, build, test or run was started; no repository file was edited. Only this report was written (plus `/tmp/r1-check.py`, a line-range checker).
- Self-check: a script extracted every `path:line[-line]` token in this report (distinct count printed by the script) and verified the file exists in one of the four clones and has at least that many lines; failures were corrected (the two Cargo.toml dependency ranges). The script checks existence and range, not meaning; meaning was checked at write time by opening the lines, except where the text says "not read", "grep", "listing" or "(derived)".
- Items stated as judgement rather than fact: the ranking in §6.1, "Strength" column in §7, adoption traps' framing in §16.
- Caveats on freshness: nteract-elements HEAD is 2026-03-04 while nteract/nteract HEAD is 2026-11-03; the production components in nteract/nteract (`src/components`) are ahead of the standalone registry.
- Scope note: the brief's item "celld and Pages" is answered in §3.8; "@runtimed/components" in §2; licences in §6.2; open gaps in §11.
- One-line verdict: take the **parts** (CodeMirror, ANSI output, output list, cell chrome) and the **ideas** (guarded execute, record-then-read, server-side stream emulation, storage-boundary redaction, two-audience results); do not take the kernel layer for bash, and do not rely on the standalone elements registry until its licence is settled.
