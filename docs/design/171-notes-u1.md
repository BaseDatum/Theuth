# 171 CELLS — notes of UI worker U1 (chat)

Decisions taken where the design, the brief or the skeleton was silent or contradictory (simpler option, contract intact).

## Contract points siblings must know

1. **`CellRef.Chat.entry` = the stored assistant message's seq, as a decimal string** (`ContextEntry.target.seq`). `FenceContext.entry` is a client-minted `nextId()` that changes on every reload, so it cannot identify a stored message for the engine. A reply not yet stored (no `context`, e.g. live-streamed until the settled reload) is not runnable: it shows "ready when the reply ends". **E1** must resolve `entry` as the message seq of `session`.
2. **`ordinal`** = 1-based position among the reply's *cells* (fences with a `#|` header in a runnable language) in document order, from `fences.ts` `replyCells` over the stored markdown (CommonMark fence rule; opener indentation and `>` prefixes stripped). E1/A1's parser must agree (pulldown-cmark order = document order).
3. **`seen`** = `grammar.ts` `digest` of the fence body streamdown hands the renderer (same rule as `surface::cell::digest`), computed at press time.
4. **`RunMode` offsets** (`line`, `selection.from/to`) are code-relative (header excluded); `line` is 1-based; selection offsets are **UTF-8 byte offsets** into the code that runs (edited code when edited), so `code[from..to]` slices in Rust.
5. Run ids `r_` + ULID and Run-all pass ids `p_` + ULID are client-minted (`store.ts` `mintId`); a transport failure retries the **same** id up to 3×; an engine `RpcError` is final.
6. `cell_run_all` is sent every reply cell in order with its digest; the engine applies the §7.2 boundaries.
7. `<cell_result>` card: `injected.ts` keys on `run="` (L2) and reads ` name="`, ` cell="`, ` message="`, ` exit="`, ` secs="` (leading space in the key so `run="` never matches inside another attribute).

## Deviations (STUB-DEVIATION / ADD)

- `CellCallbacks.onAttach` (frozen) is not called by `Cell`: the live xterm attaches itself through `Terminal`'s `source={{kind:'cell', run}}` (one path for chat and notes). `CellFence` supplies a no-op for the type. `CellCallbacks.onRun(mode, edited?)`: chat binds the edit from its own state, ignoring the second argument.
- `CellExtras` context (in `Cell.tsx`) carries what `CellProps` lacks: `open` (latest cell of the reply stays unfolded), `afterLabel`, `queuePos`, `notice`, `hint`, `pending`. U2's `NoteCell` provides it the same way.
- `// 171:U1-ADD` in my own frozen-named props: `RunbookBarProps.cells[].spec`, `RunbookBarProps.runnable?`, `ResultCardProps.{name?, cell?, exit, secs}`, `QuitSheetProps.api`.
- `theme.ts` `cellTheme` is an `Extension[]` (was `{}`), spread into the editor's extensions as the stub comment says.
- Files outside my row, minimal and marked `// 171:U1-ADD`: `store/sessions.ts` (Entry union += `cell_result`), `store/reduce.ts` (two `case "cell_result"` lines), `components/chat/Message.tsx` (import + `case 'cell_result'` → `ResultCard`). Without them the new `Injected` variant breaks `tsc` (exhaustive switch) and the card never renders.
- `CellEvent` is not in `rpc/events.ts` `WireEvent` (C did not add it); `store.ts` subscribes through `on` re-typed locally (`onCell`). The bus dispatches by `type` at runtime, so `run_queued`… reach it.
- Vitest only collects `src/**/*.test.ts` (node, no DOM): C's `CellFence.test.tsx` never ran; it is now `CellFence.test.ts` (same `createElement` body plus the full streamdown-path comparison). DOM-only behaviour (CodeMirror keymaps, xterm) is tested through extracted functions (`keyRun`, `attachSource`).

## Decisions

- `CellFence` is registered only in `MessageAssistant`'s own `assistantPlugins`; the shared `plugins` export (artifacts viewer, attention packets, harness cards, tool results, notebook ask view) is unchanged, so no other surface can mount a cell. Inside the renderer a missing `CellOrdinals` provider also falls through to code.
- The header-less fall-through renders streamdown's `CodeBlock` with `className="language-<lang>"`, line numbers / `startLine` from `meta`, and the copy/download controls from `StreamdownContext.controls` — byte-identical to streamdown's default path (test).
- `turnRunning` = the session runs **and** this reply belongs to the current turn (`turn === turnSeq`); replies of earlier turns stay runnable while a new turn runs.
- Chat editor: CodeMirror read-only until Enter / double-click; Esc leaves while unedited; ▶ gutter on the hovered line and the selection pill are absent on danger cells and their keys explain ("press <label> to run this"). `#|` toggle shows the raw header.
- Fold: a `done` cell folds unless it is the reply's last cell with a run; `failed` stays open.
- Queue position "queued · 2nd" = rank among the scope's queued runs on the same kernel by `queued_ms`.
- Static output: shell runs (all-stream outputs) show the last 200 lines (`cells.ui_tail_lines` default) with "show all N lines" paging `cell_output(ansi=true)`; copy pages `cell_output(ansi=false)`. Rich outputs go through the vendored `OutputArea`/media router; HTML/SVG/markdown only through `HtmlPage` (VendorNteract notes: markdown via the hoisted transitive `marked`).
- "Open log" / "Open log folder" open `<info.data_dir>/cells/runs/<run>` / `<data_dir>/cells/kernels` (L10), desktop only.
- Notifications: `store.ts` posts `notifyIfHidden("<title> · exit 0 · 2:14", cwd)` once per run when a re-read shows a run this client saw live end after > 30 s.
- Open as note: markdown = the reply with an ```` ```output ```` block (8 KiB head 6 KiB + tail 2 KiB) and the `ran 14:38 · exit 0 · 33 s · expect ✓` line after each finished cell; `doc_create(label of cell 1 | "Runbook", md)`; a toast with **Open** stands in for the chat link card.
- Deep link: `deep_link_route` keeps `cell=` only when it is `[a-z0-9-]{1,48}` (a slug or an ordinal); the route's `validateSearch` accepts the same (string or number); `scrollToCell` focuses the newest matching `[data-cell-name]` / `[data-cell-ordinal]` once mounted (≤ 10 s).
- Quit: `App.tsx` `quit()` awaits `confirmQuit(api)` before 163's `requested`; no live run / no engine → unchanged.

## Known gaps

- **Shell-initiated quits (⌘Q, tray, `theuth://quit`) are not held by the sheet**: `crates/desktop/src/quit.rs` `request` starts the quit worker after `ACK_TIMEOUT` (3 s) whether or not the webview acks. The sheet fully works for in-app quits (`/exit`, `theuth:quit`, the startup screen); for shell quits it needs a shell-side hold (e.g. a `quit_hold` command that cancels the timer) — outside U1's region of `lib.rs`.
- Host chip: always `null` in chat (no client-visible "session owner is another engine" field found).
- No wire field for "N masked for theuth", the env-capture warning (the strip shows it only when *Refresh environment* fails), python `input()` requests, or execution counts (python `ExecutionCount` shows `[•]`).
- Notification click focuses the app (Tauri default); it does not scroll to the cell.
- Visual check: `src/dev/cells-fixture.{html,tsx}` (vite dev only, `/src/dev/cells-fixture.html`), screenshots in `/tmp/171-u1-shots/`.

## Tests

`grammar.test.ts` (all 104 `cells.json` cells parse + digest, 5 note-defaults), `CellFence.test.ts` (snippets identical to streamdown's path, language claim, a `<subagent_result>` fence stays code, runnable rules: session / stored entry / running turn / streaming, ordinals), `Cell.test.ts` (all 14 §11.2 states, danger chip + label as accessible name, danger refuses keyboard runs), `RunbookBar.test.ts` (§3.8 "stops before Apply to ir-prod1", lint/cwd/superseded/after boundaries, "stopped at a failed step (2/3)"), `KernelStrip.test.ts` (none / one / collapse), `ResultCard.test.ts` (named and unnamed `<cell_result>`, hidden `<cell_error>`, the quiet line), `QuitSheet.test.ts` (fake clock: none, cancel, wait, stop→kill 5 s), `store.test.ts` (ULID shape and order, same id on retry, refusal final), `chat/TerminalViewer.test.ts` (cell source: `cell_attach`/`cell_input`, no `pty_*`, key row), `Outputs.test.ts` (VendorNteract).
