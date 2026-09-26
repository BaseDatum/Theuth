# PTY-POPOUT as built — an agent-created terminal in its own window

Branch `feat/pty-popout`, worktree `theuth-ptypop`, base `b9f3264`. One code commit plus this report. Builds on `lib/popout.ts` (70-as-built-POPOUT.md, -2.md): same `?window=<label>` mechanism, same `open_window` command, no host (Rust) changes — `popout-*` already covers the new label, `on_document_title_changed` already syncs the title.

## Commit `0d7f8c9` feat(ui): pop an agent PTY out into its own window

- `lib/popout.ts`: `PopoutTarget` gains `{kind:'pty', session, job}` → `/pty/<session>/<job>?window=popout-pty-<session>-<job>` (both ids through `safe()`; `j.1` → `j_1`). `popoutTarget` parses `/pty/$session/$job` (no job → `null`). New `popoutSession(t)`: the session a `chat` or `pty` window belongs to, used by `App.tsx` for `SessionDeleted`.
- `routes/pty.$session.$job.tsx` (new): `PoppedTerminal` from the route params. **Route choice:** a new file route rather than `/c/$session?job=` — the `Popped` root is route-agnostic (`SidebarProvider` + `Outlet`), so a route needs nothing to be popped; a query param on the chat route would have meant reading `?job` once at boot and branching `LiveSession`, which POPOUT-2 only did for the meeting note because `/meetings/$id` already owned it. No terminal route existed, so the file route is the smaller change. Without `?window=` the route renders under the ordinary shell (sidebar beside a full-height terminal) — harmless, not linked from anywhere.
- `components/chat/TerminalViewer.tsx`:
  - `Terminal({session, job, label, popped?})` (exported): the per-job xterm that was inline in `Body` — `subscribe_pty`/`pty_input`/`pty_resize`, fit + `ResizeObserver`, touch-scroll shim, the mobile key strip, and the status line `label · cols×rows · exit`. `Body` (the panel) and `PoppedTerminal` both render it; one attach implementation. xterm's class is imported as `Xterm` to free the name.
  - `PoppedTerminal({session, job})` (exported): `list_terminals(session, true)` (same polled query as the panel, `terminalsQuery`) for the job's `name ?? command · state`, then `<Terminal popped />` full height. With `popped` the status line is also `document.title` (Tauri sets the native title from it). The window stays up after the job exits — the status turns to `… · exit N` / `killed` (amber), no auto-close.
  - `Body`: selector row is now `select` + `PopoutTerminalButton`; when the selected job is popped elsewhere it shows `<label> · opened in its own window` with a **Focus window** button (`openPopout` → focus-if-open) instead of an xterm.
  - `PopoutTerminalButton({session, job, className?})` (exported): the "Open in new window" icon; `null` when `canPopout()` is false. Used by the panel toolbar and the Jobs sheet.
  - `TerminalButton` (header): ⌥-click → `list_terminals(session, true)[0]` popped out; plain click opens the panel as before. The compact-menu `row` reads ⌥ from `MenuRow`'s `onClick` (a ref) since `onSelect` has no event.
- `components/chat/SessionMenu.tsx` `JobsSheet`: rows with `pty: true` get the same button before **Kill**.
- `App.tsx` `useEventSideEffects`: `popoutSession(POPOUT)` replaces the `kind === 'chat'` check — `SessionDeleted` for the PTY's owning session (a subagent's, for a subagent terminal) closes the window. Job end does not.

## The two-viewer size rule

Exactly one viewer sizes a PTY. Whoever owns the size sends `pty_resize` on fit and restores the default grid (`DEFAULT_ROWS`/`DEFAULT_COLS`, 40×120) on leaving.

- Popped window: always owns. On mount `Terminal` writes `localStorage['pty-popout:<session>/<job>'] = '1'`; on `pagehide` (window close, reload) and on unmount it removes the mark and sends the default grid.
- Panel (`Body`, in main or in a popped chat): defers. `usePoppedElsewhere(job)` (`useSyncExternalStore` over the `storage` event — the same cross-window channel `composer-drafts` uses) is true while the mark exists, and the panel renders the stub instead of an xterm: no subscription, no resize. Its `Terminal`'s `leave` skips the default-grid restore while the mark exists, so the panel unmounting *because* the window opened does not clobber the window's size (the storage event lands after the popped window has already resized).
- Two panels (main + popped chat) on one job still fight — unchanged from before (`ponytail:` note in the file); the popped terminal is the fix for that too.
- Ceilings: a popped webview killed without `pagehide` leaves a stale mark — the panel keeps showing the stub; **Focus window** reopens the window, which rewrites the mark and clears it on its own close (self-healing, no manual reset). `pty_resize` on `pagehide` is fire-and-forget over the WebSocket; in the browser smoke it landed (120×40 after close).

## Tests written

- `lib/popout.test.ts`: `pty` label (`popout-pty-01J8ZK-abc-j_1`), url, url→target round trip incl. encoded ids, `/pty/s1` (no job) and no-`?window=` → `null`; `popoutSession` for pty/chat/meeting/null.
- `components/chat/TerminalViewer.test.ts`: `PopoutTerminalButton` renders the "Open in new window" control under a macOS Tauri shell and nothing under the iOS shell (`canPopout()` false).

## Verification

- `just ts-desktop`: ok (5 export tests; bindings copied — the worktree had none). `bun install --frozen-lockfile` in `crates/desktop/ui` and `packages/apps` (no `node_modules` in this worktree).
- `bunx tsc --noEmit`: 0 errors. `bunx vite build`: built (route tree regenerated → `routeTree.gen.ts`). `CI=1 bunx vitest run`: 81 files, 492 tests passed.
- Browser smoke (browser fallback, not Tauri): `vite --port 5181` with `VITE_THEUTH_URL`/`VITE_THEUTH_TOKEN` at the running engine (`~/.theuth/engine.json` 127.0.0.1:57903, `~/.theuth/token`), headless Chromium. A **new** chat of my own (`new_session` + `send_input` "run start(command=\"top\")") gave job `j1` (`top`, running, 40×120). Sky's sessions/jobs untouched.
  - Header Terminals → panel: selector `top · running`, pop-out button beside it, live `top`, status `top · running · 162×37`.
  - Pop-out button → popup `/pty/<session>/j1?window=popout-pty-<session>-j1`: no sidebar, no selector, xterm full window, `document.title` = `top · running · 133×42`, engine `list_terminals` reported 133×42 (`/tmp/69/ptypop-2-window.png`, live `top` output). Panel switched to `top · running · opened in its own window` + **Focus window** within the same second.
  - **Focus window** → still one `/pty/` page (named `window.open` reused = the browser's focus-if-open; Tauri's `open_window` does the same by label).
  - Closed the popup → engine grid back to 120×40; panel returned to an xterm.
  - ⌥-click on the header Terminals icon → popup opened directly (no panel dialog), sized to its window (171×47).
  - ⋯ → Jobs: row `j1 top running` shows the window icon before Kill; click → popup with the terminal.
  - Kill (my job) → popup stayed open, title/status `top · running · 171×47 · killed` (`/tmp/69/ptypop-6-window-exited.png`).
  - `delete_session` on my smoke chat → the popped terminal window closed (`SessionDeleted`); only the main tab remained.
  - Not exercised in Tauri (`cargo tauri dev` not run — same reasons as POPOUT): the native title sync and `open_window` focus are the host paths POPOUT already verified; the label and URL shapes are the tested part.

## Deviations

- `Terminal` takes `label` (and `popped`) beyond the brief's `{session, job}`: the name/state come from the caller's `list_terminals` query (the panel already had it for the selector) rather than a second query per terminal.
- The `start` tool result card renders no "open terminal" affordance today (checked `components/chat`: only `/terminals` and the header reach the panel), so it got none; the Jobs sheet (the one list of jobs) got the row action.
- Compact-menu `row` variant of TerminalButton: ⌥ is read from the row's click event via a ref; keyboard selection is a plain open.
- `Popped` root's doc comment (`__root.tsx`) still says "one chat or one artifact" — left untouched (another worker is near that tree).
- Header.tsx and lib/header.ts: untouched — the ⌥ behaviour lives in `TerminalButton` (TerminalViewer.tsx), which the header already renders.

## Not built

- A menu row "Open terminal in new window" in the header's compact menu: ⌥ on the existing row covers it.
- Mirroring (read-only second xterm) in the panel while popped: the stub + Focus is the "keep it simple" option the brief allowed.

DONE PTY-POPOUT
