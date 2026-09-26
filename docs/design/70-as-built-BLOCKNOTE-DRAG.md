# BLOCKNOTE-DRAG as built — block drag handle: menu on click, drag works in the Tauri webview

Branch `fix/blocknote-block-drag`, worktree `theuth-bndrag`, base `cf37ce5`. Fix `0e9142a`, this report follows.

Bug (user): "blocknotes blocks should be able to be dragged around within the document to different position, and when I
click and hold on one, it opens a context menu and dragging and dropping doesn't work at all." Every BlockNote editor in
the desktop app renders through one component, `components/editor/Editor.tsx` (notes, meeting notes, charter, threads),
so one fix covers all of them.

## Confirmed root causes (two, independent)

### 1. Tauri's file-drop hook swallowed every HTML5 drop in WKWebView

`wry` (0.55.1, `src/wkwebview/drag_drop.rs`) subclasses `WKWebView` and overrides the four `NSDraggingDestination`
methods. Each one calls the app's `drag_drop_handler` and only falls through to `super` (WebKit's own implementation,
which is what dispatches DOM `dragenter`/`dragover`/`drop`) when that handler returns `false`. Tauri's handler
(`tauri-runtime-wry` 2.11.4, `lib.rs:4862-4896`) is installed whenever `dragDropEnabled` is true (the default) and
**always returns `true`** — for every drag, not only file drags. Result: the block handle's `dragstart` fires (WebKit
still runs the drag *source*), the drag image follows the pointer, but WebKit never sees the destination side; no drop
indicator, and releasing does nothing. Upstream: tauri-apps/tauri#14373, wry PR #1829 (open) forwards non-file drags.

Evidence, real mouse in the dev build (`cargo tauri dev`, throwaway `THEUTH_DATA_DIR=/tmp/69/theuth-data`), a
document-level capture listener logging through the vite-forwarded webview console, and a temporarily instrumented
local `wry` (eprintln in `drag_drop.rs`, not committed):

- Handler enabled: DOM `mousedown → dragstart → dragend`, no `dragenter/dragover/drop`; wry saw `draggingEntered`
  and `performDragOperation` with `handled_by_listener=true`, `super` never called; block order unchanged.
- Handler disabled: `draggingEntered … handled_by_listener=false → super`, `draggingUpdated super=Move`,
  `performDragOperation → super returned YES`; DOM `dragenter → dragover → drop → dragend`; block moved.

The board (`Board.tsx`) and sidebar had already been rewritten as pointer-based drags because of this (UI-FIXES-3
blamed `dragstart`; it is the drop side that is swallowed). The composer's `application/x-theuth-attach` row drop
(design 66 §7.8) was silently dead on macOS for the same reason.

The first attempt at the fix edited only `tauri.conf.json`; the dev build still created the main webview *with* the
handler. `tauri.macos.conf.json` redefines `app.windows` wholesale (platform configs replace arrays, they do not
merge), so the flag has to live there too.

### 2. The block menu opened on `mousedown`

BlockNote 0.54's shadcn flavour (`@blocknote/shadcn`, switched from Radix to Base UI in 0.53) makes the drag handle a
Base UI `Menu.Trigger`. `MenuTrigger.js:161-167` uses floating-ui `useClick` with `event: 'mousedown'`: pressing the
handle opens the Delete/Colors menu immediately, before any drag starts. That is the "click and hold opens a context
menu". Reproduced in Chromium too (headless, `page.mouse.down()` → `[role=menu]` present after 600 ms), so it is not
a webview quirk. Theuth has no long-press/`contextmenu` logic in the editor path (grep: only `Composer`'s Send hold and
`AttachPicker`'s touch long-press).

## Fix

- `crates/desktop/tauri.conf.json`, `crates/desktop/tauri.macos.conf.json`: `"dragDropEnabled": false` on `main`.
  The `island` keeps the default (nothing drops there).
- `crates/desktop/src/commands.rs` `open_window`: `.disable_drag_drop_handler()` on the pop-out builder (same flag,
  same wry path).
- `crates/desktop/ui/src/components/editor/Editor.tsx`: `DocSideMenu` now renders `<SideMenu>` with `AddBlockButton`
  and a local `DragHandle` — BlockNote's `DragHandleButton` verbatim except two handlers on the `SideMenu.Button`:
  `onPointerDown`/`onMouseDown` call Base UI's `event.preventBaseUIHandler()` (merged-handler escape hatch,
  `mergeProps.js`), which skips `useClick`'s mousedown branch and its pointer-type latch, so `useClick`'s `onClick`
  opens the menu instead. A press that moves is a native drag (WebKit/Chromium fire no `click` after a drag), a press
  that does not is the menu. Icon is lucide `GripVertical` (the original's `react-icons` is not a direct dependency).
  `SideMenu.Button`'s prop type has no mouse handlers; the two are spread through an `as object` cast (the shadcn
  button forwards `...rest`).
- `crates/desktop/ui/src/components/chat/Composer.tsx`: the `getCurrentWebview().onDragDropEvent` listener is gone
  (the native hook no longer fires). The existing HTML5 `onDrop` receives OS file drops as `File`s and uploads them via
  `addBlobs` — the same path the browser and iOS already used — and the `application/x-theuth-attach` row drop starts
  working on macOS. Host paths (files referenced in place, folders) still come from the **Attach** button's native
  dialog (`hostFiles`); a *dropped* file is now uploaded rather than referenced. Deliberate: WebKit exposes no path
  for a dropped `File`, and Tauri's hook cannot coexist with in-page drops on this wry.

Not done: no pointer-based block drag (the spec's fallback). Native DnD is reliable once the hook is off, and the
BlockNote drop indicator, multi-block selection drag and cross-editor drag come for free. Mobile is untouched: on
coarse pointers `Editor` renders `MobileToolbar` and no `SideMenuController`, so there is no handle to hold.

## Verification

- Chromium (vite dev server against the live engine, scratch note, deleted afterwards): hold on the handle 700 ms →
  no menu; release → menu `DeleteColors`; Escape closes; synthetic `dragstart/dragover/drop` sequence moves the block
  (`Untitled, Bravo, Charlie, Alpha`); no menu after the drop.
- Tauri webview (dev build, scratch data dir, real mouse via `cliclick`): before/after as in §1; final sequence dragged
  `Bravo` below `Charlie`; engine `doc_markdown` went from `Bravo block\n\nCharlie block` to
  `Charlie block\n\nBravo block`. Hold 3.6 s on the handle → `[role=menu]` absent; release → present.
  Screenshots `/tmp/69/bndrag-01-before.png`, `-02-after.png`, `-03-after-bravo.png`, `-04-menu-on-click.png`.
- Pop-out window: same builder flag, not exercised separately.
- `bunx tsc --noEmit` 0, `bunx vite build` 0, `vitest` 83 files / 503 tests green, `cargo check --workspace --tests`,
  `cargo test -p desktop`.

## Notes for later

- wry PR #1829 (forward non-file drags, keep path-based file drops) would let the composer regain host-path drops
  with `dragDropEnabled` back on; until then the Attach dialog is the host-path route.
- The board and sidebar pointer drags could return to native DnD now, but they work; not touched.
