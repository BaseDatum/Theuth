# 59 step 6 as-built — mobile editor (§10)

Branch `feature/documents-mobile` off `feature/documents` 3005e67 (steps 1–5). Commits: `feat(desktop): mobile editor — coarse-pointer rules, keyboard inset, MobileToolbar (59 §10)`, `fix(desktop): MobileToolbar — refocus editor after the sheet's focus trap unmounts; pin keyboard-down, scroll the groups`, `test(desktop): mobile toolbar tests; iPhone-emulation smoke script`, `docs(59): step 6 as-built`. No Rust change; no file outside the allow-list.

## Built

- `crates/desktop/ui/src/lib/mobile.ts` — `usePointerCoarse()` (`useSyncExternalStore` over `matchMedia('(pointer: coarse)')`, re-evaluated on `change`); `keyboardInset(innerHeight, {height, offsetTop})` (pure, clamped ≥ 0); `useKeyboardInset()` (listens `visualViewport` `resize`/`scroll` and window `resize`; honours the test hook `window.__theuthKbInset` when defined).
- `components/editor/toolbar.ts` — pure toolbar logic: `GRACE_MS = 150`; visibility reducer `nextVisibility(state, event)` over `'hidden' | 'shown' | 'grace'` with events `focus | blur | elapsed`; `toolbarVisible(state, readonly, sheetOpen)`; `BLOCK_TYPES` (paragraph, h1–h3, bullet, numbered, check, quote, code → BlockNote `type` + `props`), `blockTypeKey(block)` for the active row (`null` for blocks the sheet does not offer).
- `components/editor/MobileToolbar.tsx` — portal to `document.body`; `role="toolbar" data-mobile-toolbar`; `position: fixed; bottom: var(--kb-inset)` with `--kb-inset: calc(<inset>px + var(--safe-bottom))` set on the element; `onPointerDown → preventDefault` so a tap does not blur the editor; `focusin`/`focusout` on `document` filtered to `editor.domElement` drive the reducer; `editor.onSelectionChange` → `scrollIntoView({block: 'nearest'})` on the cursor block's `.bn-block-content`. Groups exactly as §10: **Aa** (bottom `Sheet`, one 44 px row per `BLOCK_TYPES` entry, current type marked `aria-pressed`) · **B I S `<>`** (`toggleStyles`, active from `getActiveStyles()`) · indent/outdent (`nestBlock`/`unnestBlock`, disabled from `canNestBlock`/`canUnnestBlock`) · link (bottom `Sheet`: URL input prefilled from `getSelectedLinkUrl()`, apply = `createLink`/`editLink`, remove = `deleteLink`) · undo/redo · keyboard-down (`editor.domElement.blur()`; falls back to `document.activeElement.blur()`). `readonly` → renders nothing. Every button ≥ 44 × 44, `aria-label`, `aria-pressed` where stateful.
- `components/editor/Editor.tsx` — `coarse = usePointerCoarse()`: `sideMenu`, `formattingToolbar`, `slashMenu`, `linkToolbar`, `tableHandles` all `{!coarse}`; `showCursorLabels: coarse ? 'always' : 'activity'`; on coarse the Suggestion/SideMenu controllers are replaced by `<MobileToolbar editor readonly={!editable} onVisible=…>`; root gets `data-toolbar` while the bar is shown (drives the bottom padding below). Pointer-fine path unchanged.
- `components/editor/FrontmatterBlock.tsx` — "+ add key" wrapper `pointer-coarse:hidden` (fields stay editable).
- `styles.css` (additive) — `@media (pointer: coarse)`: `.bn-side-menu`/drag handle hidden, `.bn-editor` and frontmatter inputs `font-size: 16px` (no iOS focus zoom), `[data-toolbar] > .bn-container { padding-bottom: 60px; scroll-padding-bottom: 60px }` so the last block can scroll above the bar.
- Tests: `components/editor/toolbar.test.ts` (3 tests: `keyboardInset` math incl. clamp and `offsetTop`; the visibility state machine incl. grace/readonly/sheet; block-type mapping both ways). `scripts/mobile-smoke.ts` (Puppeteer-shaped `PageLike`, exports `smoke`, `smokePhone`, `smokeDesk`; standalone entry `bun scripts/mobile-smoke.ts --ui … --rpc … --token … --puppeteer <module>`).

## BlockNote 0.54 API facts (from `node_modules` typings)

- `BlockNoteView` props: `sideMenu`, `formattingToolbar`, `slashMenu`, `linkToolbar`, `tableHandles`, `editable` (booleans; `false` unmounts the controller). There is no `dragHandle` prop: the handle is part of the side menu, so `sideMenu={false}` unmounts it.
- `BlockNoteEditor`: `undo()`, `redo()`, `focus()`, `isFocused()`, `domElement` (the ProseMirror view root, class `bn-editor`), `toggleStyles`, `getActiveStyles`, `getSelectedLinkUrl`, `createLink`, `editLink` (in `linkToolbar` extension), `deleteLink`, `canNestBlock`/`nestBlock`, `canUnnestBlock`/`unnestBlock`, `getTextCursorPosition().block`, `updateBlock`, `onSelectionChange(cb)` returns an unsubscribe.
- `CollaborationOptions.showCursorLabels: 'always' | 'activity'`.
- `@blocknote/react` ships `ExperimentalMobileFormattingToolbarController` (positions via `--bn-mobile-keyboard-offset`). Not used: §10 specifies our own bar and groups.
- Block DOM: `.bn-block-outer > .bn-block[data-id] > .bn-block-content[data-content-type]`; the trailing empty block is `.bn-trailing-block.ProseMirror-widget`; remote cursor labels are `.ProseMirror-widget` inside `.bn-inline-content`.

## Deviations

1. **`--kb-inset` composition.** The design adds `env(safe-area-inset-bottom)`; the repo already owns that value as `--safe-bottom` (zeroed while `data-keyboard-open`, 31 §8), so `--kb-inset = calc(<visualViewport inset>px + var(--safe-bottom))`. Set on the toolbar element itself, not the editor root: the bar is portalled to `body` and cannot inherit from the root.
2. **Also `linkToolbar={false}` and `tableHandles={false}` on coarse.** §10 names only side/formatting/slash; the other two are hover/anchor popovers that overlap the bar and have no touch affordance. Table cells still accept typing (§10 "not on mobile" list).
3. **Toolbar width.** 12 targets × 44 px > 390 px: the groups scroll horizontally; keyboard-down is pinned at the right behind a separator so it is always reachable.
4. **Focus handling.** Radix `Sheet` traps focus and returns it to the trigger on close, which blurred the editor and hid the bar → editor refocused in `onCloseAutoFocus`; the bar stays mounted while a sheet is open (`sheetOpen` in `toolbarVisible`) and does not blink during the close animation.
5. **Test hook** `window.__theuthKbInset` (brief F): `useKeyboardInset` returns it when it is a number. The smoke also injects `html { --visible-band: 508px !important }` because `lib/viewport.ts` (outside the allow-list) computes the band from the real `visualViewport`, which headless Chromium cannot shrink.
6. **Puppeteer is not a UI dependency.** The smoke script types against a structural `PageLike`; the standalone entry imports the module named by `--puppeteer` at runtime (commented as runtime-selected). Tonight it ran inside the omp headless-Chromium tab runtime (same Puppeteer `Page`), in two phases, one tab each: a page created by the script from another tab was re-emulated as a phone and did not receive key events. `page.evaluate` runs in an isolated world there, so the hook is set by injecting a `<script>` element.
7. **Export check.** No `doc_markdown` RPC exists; the check reads the derived file from `doc_open(...).meta.derived` under `--data-dir`.
8. **Desk phase.** The second RPC socket in the same headless Chromium sometimes stayed at "connecting"; `smokeDesk` reloads once when the footer is not `live` after 10 s. Smoke tooling only — the product path is unaffected.
9. **`readonly` path** (viewer / not live) is not exercisable before step 7: covered by the reducer test only; `readonly` → the component returns `null`.
10. **Test data**: the note is created with body `'Start here.'` so typing lands in a paragraph (at the end of an H1 the heading type is kept, which broke the bullet check).
11. **Environment**: harness built into `../theuth-documents/target` (same commit, the sibling worktree already had the build); bindings copied from that worktree rather than `just ts-desktop`; RPC on 7823 (7821/7822 reported "address in use" with nothing listening).

## Checks

- `tsc --noEmit`: green after every commit.
- `vitest run`: 38 files, 208 passed, 1 skipped (pre-existing), 6.3 s.
- `vite build`: green (≈ 2 min on the loaded machine).
- Smoke (`/tmp/theuth-59-smoke6/report.json`, 20 checks, 13.5 s; screenshots `phone-toolbar.png`, `phone-typed.png`, `phone-block-type-sheet.png`, `phone-link-sheet.png`, `desktop-no-toolbar.png`), phone = 390×844 DPR 3 touch, UA iPhone, `__theuthKbInset = 336`:
  - pointer coarse + touch; no side menu / formatting toolbar / drag handle in the DOM; toolbar hidden while unfocused;
  - toolbar bottom edge at 508 px (844 − 336); all targets ≥ 44 px; editor text ≥ 16 px;
  - 200 words typed with the bar up, cursor block fully above the bar; no slash menu after `/`;
  - Aa sheet → paragraph becomes `bulletListItem`; bar still shown after the sheet closes; toolbar Undo restores the paragraph; Bold `aria-pressed` follows the active style; link sheet applies a link; keyboard-down blurs and hides the bar; no `console.error`;
  - desktop 1280 px pointer fine: no toolbar, typing lands, side menu on hover; phone and desktop editors read identical text (1076 chars); derived markdown contains the desk-typed text and `**bold`.

## Physical-iPhone procedure (Sky, from §10 "Check (step 6)")

1. Mac harness live; iPhone Tauri build attached over the relay; open a live meeting note on both.
2. iPhone: tap into the note. Expect: no side menu, no drag handles, no formatting popover; the toolbar rises with the keyboard and its bottom edge sits exactly on the keyboard (no gap, nothing hidden under it); rotating or the predictive bar toggling keeps it attached (`visualViewport` resize).
3. Focus a frontmatter field and the editor: no zoom on focus (16 px inputs).
4. Type ≈ 200 words with the keyboard up: the cursor block stays above the bar as it wraps; Enter creates a new block.
5. Tap **Aa** → Bullet: the paragraph becomes a bullet; the bar stays; tap Undo: it reverts. Bold a word; B lights up; tap link, enter a URL, Apply.
6. Background the app, wait ≥ 30 s, return: footer returns to `live`, the Mac shows every edit, the iPhone shows edits made on the Mac meanwhile; the Mac's cursor label is shown without a hover.
7. Keyboard-down button: keyboard dismisses, bar disappears; tapping the text brings both back.
8. Desktop unchanged: on the Mac nothing from this step is visible (no bar, side menu and formatting toolbar as before).

## Left

- Physical-iPhone check (above). Not reproducible headless: real keyboard `visualViewport` geometry, WebKit selection handles, safe-area inset on a notch device, and the app-background/return path.
- Step 7 (`readonly` viewer, members/links) will exercise the `readonly` prop for real.
- `useKeyboardInset` reads `visualViewport` only; if iOS reports the keyboard through the Tauri plugin instead, `lib/viewport.ts` (not in this step's allow-list) is where that value already lands — the hook can read `--visible-band` there.
