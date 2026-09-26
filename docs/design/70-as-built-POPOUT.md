# POPOUT as built — any chat pane or artifact in its own desktop window

Branch `feat/popout-windows`, worktree `theuth-popout`, base `9870ceb`. Three commits plus this report.

## Commits

### `29ec23c` feat(desktop): open_window command for popout windows; popouts close for real; capture protection covers every window

- `crates/desktop/src/commands.rs`
  - `pub const POPOUT_PREFIX = "popout-"` (desktop).
  - `open_window(app, label, url)`: refuses a label without the prefix; a window with that label already open is shown and focused (focus-if-exists); otherwise `WebviewWindowBuilder::new(app, label, WebviewUrl::App(url))`, 960×720, min 480×360, `on_document_title_changed → set_title` (Tauri does not sync the document title by default; main's title is hidden so it never needed it). `url` is the app route plus `?window=<label>`; Tauri's asset resolver falls back to `index.html` for unknown paths, so `tauri://localhost/c/<id>?window=…` loads the bundle at that route.
  - Size and position per label: `tauri-plugin-window-state` (already a dependency) restores any window not on its denylist in `on_window_ready` and caches geometry on `CloseRequested`, so a popout reopened with the same label comes back where it was. No new persisted map.
- `crates/desktop/src/lib.rs`: `open_window` registered; `on_window_event` `CloseRequested` returns early for `popout-*` labels — main and island keep the prevent-close-and-hide tray behaviour, a popout just closes.
- `crates/desktop/src/island/mod.rs`, `island/macos.rs`: `set_hidden` now iterates `app.webview_windows()` instead of `["main", "island"]`, so the meeting "hide from capture" toggle covers open popouts.
- `crates/desktop/capabilities/default.json`: `windows: ["main", "island", "popout-*"]` — the popout webviews get the main window's permission set (window show/focus/title/close, events, opener, dialog open/save, notification, window-state…). Clipboard is `navigator.clipboard` in this app (no clipboard plugin), unchanged.
- `cargo check -p desktop`: `Finished dev profile … in 51.34s`, no warnings from the crate.

### `57ab8d5` feat(ui): popout windows — popped shell, Open in new window on pane header, artifact surfaces and sidebar rows; drafts sync across windows

- `crates/desktop/ui/src/lib/popout.ts` (new)
  - `PopoutTarget = {kind:'chat', session} | {kind:'artifact', callId}`.
  - `popoutLabel(t)` → `popout-chat-<id>` / `popout-artifact-<id>`, ids filtered to `[A-Za-z0-9_-]` (Tauri label charset; `toolu_….1` → `…_1`).
  - `popoutUrl(t)` → `/c/<id>?window=<label>` or `/artifacts/<id>?window=<label>`.
  - `popoutTarget(href)`: the pure URL → target function; `null` without `?window=`, for a route with no popped form, or a missing id.
  - `POPOUT` read once from `location.href` at module load (the route's `validateSearch` drops unknown search keys on its first navigation); `isPopout()`.
  - `openPopout(t)`: Tauri → `invoke('open_window', {label, url})`; otherwise `window.open(url, label)` — a browser reuses the named window, so opening twice focuses it there too. (`main.tsx` overrides `window.open` only inside Tauri, where this path is not taken.)
- `App.tsx`: `AUX = ISLAND || POPOUT !== null`. A popout boots the engine exactly like main (`hasLocal()` → `connection()`, cached in Rust `Launch`; paired engines via `LinkTransport`; `active` engine shared through IndexedDB) but registers none of main's singletons: no tray/menu `navigate` listener, quit handshake, update listener, notifications, global shortcuts (a second `register` throws), tray badge. It does re-apply `setHiddenFromCapture(meetingSettings.hidden)` once, since it did not exist at the last toggle. Event side effects in a popout: `Lagged` reload, and `SessionDeleted` for its own chat closes the window.
- `routes/__root.tsx`: `component: isIsland() ? Outlet : isPopout() ? Popped : Root`. `Popped` = `SidebarProvider open={false}` + `SidebarInset` + `Outlet` — no `AppSidebar`, `SearchHost`, `DragOverlay`, `MeetingBar`, hotkeys. The provider stays because `Header` reads `useSidebar()`.
- `routes/artifacts.tsx`: `component: isPopout() ? Outlet : Explorer` — the `/artifacts/$callId` viewer alone, full height, no rail.
- `lib/header.ts`: `HeaderId` gains `'popout'`; `headerOrder` places `{ id: 'popout', slot: 'bar' }` (not shed) after `pending`, before `more` — inline at all three tiers.
- `components/chat/Header.tsx`: `popout` item = `AppWindow` icon button "Open in new window" (bar) / `MenuRow` (chevron); hidden for the new-chat pane (`session === null`) and in a popped window. `popped` also turns off `nav` (sidebar toggle, search trigger, macOS traffic-light inset — popouts have a normal title bar). Model picker, title, context, cost, panels, ⋯ menu all render as in main.
- Artifact surfaces (Sky's clarification: every place an artifact is seen):
  - `components/chat/ArtifactCard.tsx`: `PopoutButton({artifact, className})` (exported beside `RevealButton`; `preventDefault` + `stopPropagation` so it works inside the clickable card and the list `Link`). Inline card toolbar: Reveal · Open in panel · **Open in new window** · ⋯.
  - `components/chat/ArtifactPanel.tsx`: the sheet header the card opens: Version · Reveal · **Open in new window** · ⋯ · Close.
  - `components/artifacts/List.tsx`: each `/artifacts` rail row gets the button at its trailing edge, visible on hover/focus (`group/row`).
  - `components/artifacts/Viewer.tsx`: `/artifacts/$callId` toolbar: … Open chat · **Open in new window** · ⋯ · details. In a popped viewer the button is absent, the details rail starts closed, `document.title = artifact.title`, and "Open chat" opens the chat as its own popout rather than turning the artifact window into a chat.
- `components/sidebar/SessionItem.tsx`: the chat row's right-click / ⋯ menu gets "Open in new window" (second item).
- `lib/drafts.ts`: `storage` listener on `composer-drafts` → `useDrafts.setState(load())`. Two windows share the key (same origin, shared WebKit data store); `save` writes the whole map, so without this a window's next save would overwrite the other's drafts with its stale snapshot. With it every window reloads on every write: last writer wins per keystroke, and the composer (which renders straight from the store) shows the other window's text live.
- Tests: `lib/popout.test.ts` (route selection from URL incl. encoded ids, no-query, `?seq=`, routes without a popped form, missing id; label charset and `popoutUrl → popoutTarget` round trip), `lib/header.test.ts` (tier-1/tier-2 bar arrays include `popout`; `popout` inline at every tier).

### `25aa8aa` fix(ui): chat window title falls back to the list row's name like the header

- `routes/c.$session.tsx` `LiveSession`: `document.title` used only the session slice's `title`, which is empty until `TitleChanged` (the smoke showed `""` for a titled chat in both windows; main hides its title so nobody noticed). Now `title || chatName(row)` from the shared `list_sessions` query (deduped with `Header`'s). The popout's native title follows through `on_document_title_changed`.

## Concurrency

- Streaming: each webview opens its own WebSocket to the same engine (`RpcClient.local(c.url, c.token)`); the engine already fans events out to every client (TUI + desktop). Steering/interrupt are RPC calls from whichever window issues them. Not exercised end to end in this run (see Verification) — the popout rendered the live transcript, todo card and composer of a real session identically to main.
- Drafts: last-writer-wins with a `storage` change event (above). Edits-in-progress (`useDrafts.edits`) ride the same key and sync the same way.
- Other persisted stores (`note-chat`, `meeting-settings`, `shortcuts`, `show-harness-nudges`, theme) are read once per window and written whole; a popout changing a setting is not seen by main until reload. Unchanged: none of them is edited from a chat pane or artifact viewer.

## Verification

- `just ts-desktop`: ok (5 export tests passed; bindings copied).
- `bunx tsc --noEmit`: 0 errors (after `bun install` in `crates/desktop/ui` and `packages/apps`, which had no `node_modules` in this worktree).
- `bunx vitest run`: 77 files, 482 tests passed.
- `bun run build` (`tsc -b && vite build`): exit 0.
- `cargo check -p desktop`: Finished, no crate warnings.
- Smoke (browser fallback, not Tauri): `vite --port 1421` with `VITE_THEUTH_URL/TOKEN` pointing at the running local engine (`~/.theuth/engine.json`, 127.0.0.1:53703), headless Chromium via omp's browser tool:
  - Sidebar row right-click → "Open in new window" → second page at `/c/<id>?window=popout-chat-<id>`; it rendered the chat alone (header: model picker · title · context · cost · approvals off · ⋯; no sidebar, no sidebar toggle, no popout button; composer present); `document.title` = the chat's name after the fix.
  - Typed in the popout → main's composer showed the text within a second; typed in main → the popout followed; `localStorage['composer-drafts']` held the merged text.
  - `/artifacts` rail row hover button → `/artifacts/<callId>?window=popout-artifact-<callId_1>`; main stayed on `/artifacts` (no navigation); popup showed the viewer alone with `document.title` = artifact title, no rail/sidebar, no popout button.
  - Main: `/artifacts/$callId` toolbar shows "Open in new window"; a chat with artifacts shows it on the pane header and on both inline cards; the artifact sheet header shows it.
  - Not sent: no message was sent to the real engine's sessions, so "both windows stream one turn" was not observed. Manual check in the app: `cargo tauri dev` (or the built app), open a chat, header → Open in new window, type in the main window and send; the popout shows the streaming turn; press stop in the popout; both windows show the interruption. `cargo tauri dev` was not run here: it needs a fresh `target/debug/t` build (another worktree was mid-`cargo build -p theuth`) and the tray/global-shortcut registration of a second app instance beside the installed one.

## Deviations

- Popouts use normal window decorations (title visible) rather than main's macOS `Overlay` + hidden title: the brief wants the window title to be the session/artifact name, and a standard title bar shows it; the header's traffic-light inset is therefore turned off in popped mode.
- No `?window=` query parsing in `routes` `validateSearch`: the value is read once at boot from `location.href` (`POPOUT`), which is why it survives the router dropping unknown keys.
- The sidebar's "Open in new window" is the second menu item (after Rename) rather than a dedicated context menu: the row's right-click already opens the same `DropdownMenu`.
- Popped artifact viewer's "Open chat" opens a chat popout instead of navigating in place (otherwise the artifact-labelled window would become a chat and its focus-if-exists label would lie).
- `set_hidden` (capture protection) widened to every window — one behaviour change outside the brief, disclosed: without it a popout would be the one window visible in a screen share while the meeting eye is on.
- Report paths: the brief's header names `/tmp/67/…` and `docs/design/67-as-built-…`; its item 5 names `/tmp/69/report-POPOUT.md` and `docs/design/70-as-built-POPOUT.md`. The latter (more specific, matches the other 70-series reports) was used.

## Not built

- ⌘⇧N shortcut (optional in the brief).
- A "navigate to another session" menu in the popped header: no such title menu exists today, so per the brief the popped chat window is tab-less (its ⋯ Session menu and parent breadcrumb still navigate within the window).
- window-state denylisting for popouts: not wanted — per-label memory is the requested behaviour.

## Tests written

- `crates/desktop/ui/src/lib/popout.test.ts`: 2 tests (route selection; label derivation + round trip).
- `crates/desktop/ui/src/lib/header.test.ts`: the ordering test extended (popout in the tier-1/2 bars, inline at every tier).

DONE POPOUT
