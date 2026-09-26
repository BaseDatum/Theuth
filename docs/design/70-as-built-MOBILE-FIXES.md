# MOBILE-FIXES as built — no pop-out affordances on phones/tablets; sidebar rows lost past the Desks group

Branch `fix/mobile-popout-sidebar`, worktree `theuth-mobfix`, base `e86f78d`. Two fixes, two commits, plus this report.

## Commits

### `8a77c2b` ui: hide every pop-out affordance where no second window exists (bug 1)

- `crates/desktop/ui/src/lib/popout.ts`: `canPopout(): boolean = container() !== 'ios'`. The one predicate. A desktop shell (`macos`/`linux`/`windows`, Tauri `open_window`) and a plain browser (`web`, `window.open`) have second windows; the iOS shell has one webview. `lib/platform.ts` has no `android` container today (`Container = 'macos' | 'linux' | 'windows' | 'ios' | 'web'`); if one is added it goes in the same expression. `useIsMobile()` (< 768 px) is deliberately not consulted: a narrow desktop window still pops out.
- `lib/header.ts`: `headerOrder(panel, popout: boolean)` — the `popout` item is spread in only when `popout` is true, so it is absent from both the bar and the chevron menu at every tier (the layout model, not the renderer, decides).
- Every site that called `openPopout` or rendered "in new window" (grep `openPopout|popout|new window|AppWindow` over `src`):
  | file | affordance | change |
  |---|---|---|
  | `components/chat/Header.tsx` | bar button + chevron `MenuRow` "Open in new window" | `headerOrder(panel, canPopout() && !popped)`; the render fn keeps only the `!session` guard |
  | `components/chat/ArtifactCard.tsx` `PopoutButton` | artifact card toolbar button (tooltip "Open in new window") | `if (!canPopout()) return null` — one place covers `ArtifactCard` (card), `ArtifactPanel.tsx`, `artifacts/List.tsx` (rail row hover), `artifacts/Viewer.tsx` (toolbar) |
  | `components/chat/SessionMenu.tsx` | "Open meeting in new window" row | `{canPopout() && …}` |
  | `components/sidebar/SessionItem.tsx` | "Open in new window", "Open meeting in new window" rows | `{canPopout() && …}` both |
  | `components/notes/NotesList.tsx` | row hover button + menu row "Open in new window" | `{canPopout() && …}` both |
  | `components/notes/NotePage.tsx` | header `Action` "Open in new window" | `!popped && canPopout()` |
  | `components/meeting/MeetingList.tsx` | row hover "Open note in new window"; menu rows "Open in new window", "Open note in new window" | `const popout = canPopout()`; the button and the two rows gated |
  | `components/meeting/Note.tsx` `MeetingNote` | the "Note" header whose only control is "Open note in new window" (used by `ControlCenter.tsx`) | header rendered only when `canPopout()` |
  | `lib/meeting-link.ts` `openMeeting` | ⌥ on "Open meeting" (SessionMenu, SessionItem) | `if (alt && canPopout())` else `here()` — ⌥ is a plain open on iOS |
- Not changed, on purpose: `artifacts/Viewer.tsx:437` — inside a popped artifact window the chat button calls `openPopout({kind:'chat'})`; a popped window only exists where `canPopout()` is true. `Message.tsx` / `QueuedChips.tsx` use the `AppWindow` icon for the *app* record, not a pop-out. `App.tsx`, `routes/__root.tsx`, `routes/artifacts.tsx`, `routes/meetings.$id.tsx` read `POPOUT`/`isPopout` to render a popped window's shell — that is what a popped window shows, not an affordance to open one.
- Tests: `lib/popout.test.ts` — `canPopout` true for macOS/Windows shells and for a 390 px-wide browser with an iPhone UA (no `__TAURI_INTERNALS__`), false for the iPhone shell and the iPad shell (Macintosh UA, `maxTouchPoints` 5). `lib/header.test.ts` — with `popout=false` the item is in neither bar nor menu at tiers 0–2 and tier 2's bar is `sidebar · title · more · menu · close`; the existing tier tests pass `true`. `lib/meeting-link.test.ts` — under an iOS shell ⌥ calls `here()` and neither `window.open` nor Tauri `invoke`.

### `12bd92d` sidebar: virtualize the chat list from where it starts in the scroller (bug 2)

- Root cause, confirmed by reading and by measuring: `SessionList`'s `useVirtualizer` had **no `scrollMargin` at all** — not stale, absent. The virtualizer culled rows as if the `<ul>` began at the scroller's `scrollTop` 0, but the list is the second child of `SidebarContent`, under the `SidebarMenu` (THEUTH · New chat · top list · Attention · Library · Desks). Measured in the dev build against the live engine: that content is **644 px** tall in the desktop layout and **756 px** in the phone sheet. With `overscan: 8` (≈ 300–320 px of rows) the rendered window started `offset − overscan ≈ 330 px` below the first visible row once `scrollTop > overscan`. At `scrollTop` 994: visible band of the list `[350, 966]` px (list coordinates), rendered band `[696, 1472]` → **346 px of blank sidebar** at the top, exactly the "rows below the last desk disappear" symptom; scrolling back under the overscan threshold restored them. The hypothesis' second half (sticky bar / group header covering rows) is refuted: the selection bar is portaled outside the scroller (`barEl`) and there is no sticky group header in this list. Why it appeared with UI-FIXES-3: the Desks group added a row per active roster agent, pushing the offset past the overscan; before that the ~300 px overscan happened to hide the missing margin. Data-driven late growth made it worse, not different: any offset > overscan shows it.
- Fix, `components/sidebar/SessionList.tsx`:
  - `listOffset(scrollEl, list)` = `list.top − scrollEl.top + scrollEl.scrollTop` from `getBoundingClientRect` (no dependence on `offsetParent`, so `SidebarContent` needs no `position: relative`), clamped ≥ 0, rounded.
  - `watchListOffset(scrollEl, list, onChange, Observer = ResizeObserver)`: reports once, then observes **every sibling before the list** in the scroller (not the list itself — its height changes with every measured row and must not feed back) and reports again on each resize. Returns the disconnect. `Observer` is injectable only so the test can drive it.
  - The component: `useLayoutEffect(() => watchListOffset(scrollEl, list.current, setOffset), [scrollEl])`; `useVirtualizer({ …, scrollMargin: offset })`; rows translate by `item.start − offset` (starts include the margin; `getTotalSize()` already subtracts it in virtual-core 3.17).
  - Holds when: agents resolve late into Desks (the menu `<ul>` resizes → observer → new margin; measured: appending a 300 px block to the menu at `scrollTop` 1200 → offset 948, rendered band `[0, 1176]` covering visible `[252, 868]`, 0 px uncovered; removing it → back to 644); the mobile sheet mounts fresh (`scrollEl` is state set by ref, the effect reruns with the new element; measured offset 756, 0 px uncovered at `scrollTop` 1000); the window resizes (siblings rewrap → observer; measured at 900×500, 0 px uncovered at `scrollTop` 1300).
- Test, `SessionList.test.ts`: `listOffset` is 200 for a list 200 px under the scroller top whether `scrollTop` is 0 or 450 (the scrolled case is where a rect-only formula goes wrong); `watchListOffset` observes exactly the sibling above the list, reports `[200]`, then `[200, 520]` after the content above grows and the observer fires, and disconnects on cleanup.

## Verification

- `just ts-desktop` (bindings regenerated), then in `crates/desktop/ui`: `bun run build` (`tsc -b && vite build`) clean; `bunx vite build` ✓; `bunx vitest run` **80 files, 490 tests passed** (the suite needed `bun install` in `crates/desktop/ui` and `packages/apps` in this fresh worktree — `OpenUIFence.tsx` resolves `@openuidev/*` from `packages/apps/node_modules`).
- Browser smoke (browser fallback, not Tauri): `vite --port 5179` with `VITE_THEUTH_URL=ws://127.0.0.1:57903` / `VITE_THEUTH_TOKEN` from `~/.theuth/token` against the running engine (`~/.theuth/engine.json`), headless Chromium via omp's browser tool. Nothing on the engine was created, changed or deleted; only the sidebar was scrolled and DOM rects read.
  - **Before** (fix temporarily disabled: `scrollMargin: 0`, translate `item.start`), 1280×720, sidebar `scrollTop` 994: 346 px blank under the top of the sidebar, rows only from "DR failover plugin documentation" down — `/tmp/69/mobfix-before-desktop.png`.
  - **After**, same scroll: rows continuous from the top, 0 px uncovered — `/tmp/69/mobfix-after-desktop.png`. Bottom of the list (`scrollTop` 1500): 0 px uncovered.
  - **Phone sheet**, 390×844 touch emulation, sidebar opened with "Show sidebar", `scrollTop` 1000: offset 756, 0 px uncovered — `/tmp/69/mobfix-after-mobile-sheet.png`.
  - Late growth and resize: the numbers under "Holds when" above.
- Bug 1 in the browser: `canPopout()` is true there (container `web`), so the affordances remain; the iOS branch cannot be exercised in Chromium without faking `__TAURI_INTERNALS__` (which would also break the connection path), so it is covered by the unit tests and the `container()` tests in `platform.test.ts`. Not run on a device.

## Deviations

- The spec's example predicate `isTauri() && isDesktop()` for Tauri / `true` for web collapses to `container() !== 'ios'`, since `container()` already returns `web` when `__TAURI_INTERNALS__` is absent. Same truth table, one expression.
- No `android` container exists in `lib/platform.ts`; nothing to treat. Adding one means adding it to `canPopout`.
- `headerOrder` gained a required `popout` parameter (one caller) rather than the component filtering the list, so `header.test.ts` can state the absence against the layout model.
- `MeetingNote` hides its whole "Note" header on iOS, not just the button — the header exists to hold that button (its doc comment); a header with only a label was not worth keeping.
- No Rust changes; `cargo` not needed.

DONE MOBILE-FIXES
