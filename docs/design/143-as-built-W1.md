# 143 NOTE-FLOW — as built, W1 (sidebar Note button · pop-out `?new` · Meetings/Briefs column · portrait paper)

Branch `feat/143-w1` off main `84548287`. Contract: `143-note-flow.md` D1–D7. Proof against Sky's live engine through a vite dev server on port 1436, read-only (no note or meeting created, nothing sent). Screenshots in `/tmp/143/w1/`.

## What landed where

| Decision | File:line | What |
|---|---|---|
| D1 | `crates/desktop/ui/src/components/sidebar/AppSidebar.tsx` 224–318 | Row order THEUTH · Chat · Talk · **Note** · Meeting (`canRecord` only) · Attention · Agents · Library · Desks · Sessions. Agents/Attention swapped (Attention now first); nothing else in the rows changed. |
| D2 | `AppSidebar.tsx` 52, 269–280 | `NOTE_PLUS = 'text-success'` (exported, one token to swap for `text-project-nile`). Row = `SidebarMenuItem > SidebarMenuButton asChild > <button type="button" aria-label="New note" disabled={createNoteAndOpen.isPending}>`, same `h-9 rounded-[10px] px-2.5`, no `isActive`. Icon `StickyNote size-4` in the Meeting row's `relative inline-flex` wrapper; badge `Plus` `absolute -top-1.5 -right-1.5 size-3`, `strokeWidth={3.5}`, `aria-hidden`. Label `<span>Note</span>`. |
| D3 | `AppSidebar.tsx` 181–190 | `useMutation`: `createNote(api, queryClient, queryClient.getQueryData<DocSummary[]>(queryKeys.notes) ?? [])`; `onSuccess` → `noteOpenMode(isMobile, canPopout()) === 'popout'` ? `openPopout({ kind: 'note', doc, fresh: true })` (main window stays) : `navigate('/notes/$id', search: { new: true })`; `onError` → `toast.error(errorText(e))`. `isMobile` from `useSidebar()`. |
| D3 helper | `crates/desktop/ui/src/lib/popout.ts` 68–69 | `noteOpenMode(isMobile, canPopout): 'popout' \| 'navigate'` — `'popout'` only when `!isMobile && canPopout`. Test: `popout.test.ts` "143 D3". |
| D4 | `popout.ts` 13–14, 48–49, 63–66 | `{ kind: 'note'; doc: string; fresh?: true }`; `pathOf` appends `?new=true` when `fresh`; `popoutUrl` joins with `&` whenever the path already has a `?` (the `meeting-note` special case is gone, same output). `popoutLabel`/`idOf`/`popoutTarget` ignore `fresh`. Tests: `popout.test.ts` "143 D4" — `/notes/d1?new=true&window=popout-note-d1`, parsed target `{kind:'note', doc:'d1'}`, meeting-note URL unchanged, label identical with/without `fresh`. |
| D5 | `components/meeting/MeetingList.tsx` 12, 104–113; `components/briefs/BriefSearch.tsx` 10, 93–113 | The `min-h-0 flex-1 overflow-y-auto px-6 pb-10` scroller is `Column` from `components/library/Page.tsx` (unchanged, no prop needed). `Empty` rows and the `ul`/grid are inside it. `Row` untouched. |
| D6 | `styles.css` 605–617 (comment), 630–643, 651–654 | Portrait sheet — **not** `aspect-ratio` (see deviation 1): a zero-width `::before` float whose percentage `padding-top` gives the sheet an A4 minimum height; `[data-paper="dense"] .bn-editor::before { content: none }`; `.theuth-paper { display: flow-root }` so the non-flex knowledge fallback wraps the float too. `.bn-container`, the canvas and `Editor.tsx` unchanged. |
| D7 | this file; `143-note-flow.md` committed verbatim | |

## Measurements (D6, `.bn-editor.getBoundingClientRect()` in headless Chrome at 2×, read from the live engine)

| Case | URL | width × height | ratio | `scrollHeight` |
|---|---|---|---|---|
| Empty note, 1600×1000 | `/notes/agqrwrzff52ifit22fl6tyu72u` ("Untitled", 1 block) | 800 × 1131.36 | **1.4142** | 1129 |
| Long note, 1600×1000 | `/notes/agqn4a3v2574dmwckfgtcx42q4` ("DR Failover Procedure", 43 blocks, 5946 chars) | 800 × 2841.5 | 3.55 (grows with content) | 2840 (no overflow) |
| Empty note, 390×844 mobile | same empty note | 390 × 635.77 | 1.63 | 634 |
| Dense meeting pane, 1600×1000 | `/meetings/01a0e8f9-…0151` | 547 × 414 | — | `::before` content `none`, 412 |

Phone: the A4 minimum would be 551.5 px; `flex: 1 0 auto` then fills the 720 px `.bn-container` — the sheet gets 636 px and the remaining 84 px go to BlockNote's portal element, which carries the canvas's `flex-1` (BlockNote copies `className` onto its portal root — pre-existing; before 143 this sibling got *more* of the free space because the sheet's flex base was its content height). The phone sheet is edge to edge and looks right (`note-phone.png`).

Synthetic check of the float rule across pane widths (`/tmp/143/w1/ratio-test.html`, same structure and padding as the app): 390 (phone rule) → 1.4142, 552 → 1.4142, 800 → 1.4142, long content → grows (1842–1898 px, `scrollHeight` matched); a 252 px sheet (pane < 300 px, where the `clamp(24px, 8%, 64px)` floor bends the 8 % assumption) → 1.58.

## Screenshots (`/tmp/143/w1/`)

- `sidebar-desktop.png` (1600×1000, `prefers-color-scheme: light` emulated — the headless host is dark, so the light shot is the emulated one) + `sidebar-desktop-crop.png` (2× crop of Talk · Note · Attention · Agents · Library). Rows read Chat · Talk · Note · Attention · Agents · Library (Meeting absent: `useCanRecord()` is false in a browser, expected). Plus glyph: 12×12 at (26,176), note icon 16×16 at (16,182), colour `rgb(47, 93, 74)` = `--status-success-fg` light. Plus sits on the top-right corner; the StickyNote's fold (bottom-right) is clear.
- `sidebar-dark.png` + `sidebar-dark-crop.png` — plus `rgb(74, 138, 110)` on the soot sidebar; reads at a glance.
- `sidebar-phone.png` (390×844 mobile, sheet opened via the `Show sidebar` button) — same order, Note row 275×44 at y=196.
- `meetings-desktop.png` — column 768 px at x=546 (content 720 px at x=570); the row's absolute actions (128 px) end flush at the `ul`'s right edge, no overlap with the state label. 4 rows.
- `briefs-desktop.png` — same column (grid 720 px at x=570), 32 cards in two columns.
- `meetings-phone.png` (390×844) — `ul` 342 px at x=24, `document.scrollWidth` 390 (no horizontal scroll). See finding 1.
- `note-empty.png`, `note-long.png`, `note-phone.png` — the measurements above.
- `meeting-dense.png` — the meeting control center; the dense note pane keeps its box shape.
- `ratio-test.png` + `ratio-test.html` — the synthetic width sweep.

## Gates (under build-lock, `BUILD_LABEL=143-W1`)

- `bun run check` (`tsc -b --noEmit`): exit 0, 0 errors (baseline `tsc-baseline-theuth-143-w1.log`: 0).
- `bunx vitest run src/lib src/components/sidebar src/components/meeting`: exit 0 — 79 files, 487 tests passed (includes the new "143 D4" and "143 D3" tests in `popout.test.ts`).

## Deviations

1. **D6 is not `aspect-ratio`.** As specified (`aspect-ratio: 1 / 1.4142` on the sheet) the empty note measured 800 × 1131 (1.4142) but the long note *also* measured 1131 with `scrollHeight` 2792 — the content overflowed the sheet. Cause: `.bn-editor` is a column-flex item of `.bn-container`; css-flexbox §4.5 makes the automatic minimum height of a flex item with an aspect ratio and no specified size `min(content, transferred)`, so the ratio caps the minimum instead of flooring it. Verified in Chrome with a synthetic page: `aspect-ratio` alone, `+ min-height: max-content`, `+ min-height: fit-content`, `+ flex-basis: max-content`, `+ height: max-content` all stayed at the transferred height. Alternatives rejected: `container-type: inline-size` on `.bn-container` + `min-height: calc(min(100cqw, 800px) * 1.4142)` measured exactly right (1.4142 / 2841 / 1.4142) but BlockNote's floating-UI portal root is the *second child inside* `.bn-container` (the scroller) — layout containment would make the scroller the containing block of every toolbar/menu and clip them; `min-height: calc(min(100%, 800px) * 1.4142)` is wrong (percent min-height is against height). Landed: a zero-width `::before` float with percentage `padding-top` (resolves against the sheet's content width), constants folding the sheet's padding and border back in (derivation in the CSS comment). Exact at every pane width ≥ 300 px; the padding constants are coupled to the sheet's padding values (noted in the comment). `.theuth-paper` gets `display: flow-root` (it is a plain block in `knowledge/Document.tsx` 368, not a flex item) so it contains the float — the one touch outside "the paper rules" strictly read, still inside the 605–632 block.
2. **Plus badge tuned** from the contract's `-top-1 -right-1 size-2.5` to `-top-1.5 -right-1.5 size-3`: at 10 px the glyph read as a speck on the 2× crop; at 12 px centred on the corner it reads at a glance and still leaves the fold clear (crops).
3. **Light sidebar shot is emulated.** Headless Chrome on this host follows the system dark scheme; `sidebar-desktop.png` was taken with `prefers-color-scheme: light` emulated, `sidebar-dark.png` with dark.
4. **`Column` adds `py-4`** (16 px top/bottom) where the old scroller had `pb-10` only — the same frame Tools/Secrets use; accepted for consistency.

## Findings for the orchestrator

1. **Meetings row at 390 px overlaps — pre-existing, not from D5.** The `Row` button's fixed-width spans (`w-36` date + `w-20` + `w-24` + gaps + `pr-32`) need ~500 px, so at a 342 px list width the hover actions (always visible on `pointer-coarse`) sit over "System audio"/duration and the state label is pushed past the edge (`meetings-phone.png`). The list width is identical before and after (old scroller `px-6` → 390 − 48 = 342; `Column` `px-6` → 342), so this is the state on main; D5 says the Row stays as it is. A Row fix (hide the process name / state under `md`, or a two-line row) is a follow-up.
2. **The Tauri pop-out path is unproven here.** `openPopout` → `invoke('open_window', …)` cannot run in a browser; in the browser it is `window.open(url, label)`. Please click Note once in the real app: expect a new 960×720 window at `/notes/<id>?new=true&window=popout-note-<id>` with the title selected, the main window unmoved; a second click creates a second note (distinct label). On the phone/iOS build: navigate to the new note, sheet closed.
3. **Not clicked on the live engine** (it would create a note). The create path is `createNote` exactly as `NotesList.tsx` 33–37 uses it; the mode choice is unit-tested.
4. Vite's file watcher did not fire in this worktree (zero HMR events; stale transforms until restart) — irrelevant to the product, but the dev server was restarted after each CSS/TSX edit for the proof shots.
