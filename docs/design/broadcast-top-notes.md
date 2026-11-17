# broadcast-top: the broadcast dialog's message box and Send button above the target list

## Sky's words (2026-11-17 11:27)

> the broadcast input ui should be above the list not below it, with the send button at the top as well, so I don't
> have to scroll to the bottom of the list to see it.

## What changed

`crates/desktop/ui/src/components/chat/BroadcastDialog.tsx`:

- Order, top to bottom: header, **Message** textarea, one row with the **Delivery** toggle (left) and **Send to N
  sessions** (right), the preview/receipt line (same text and colours as the old footer line), then **Targets**.
- Only the targets rows scroll. The old single scroll body and the `Footer` prop (`DialogFooter` / `SheetFooter`) are
  gone; `Form` renders two sections in the dialog/sheet flex column: a fixed top section and the Targets section,
  which is `flex min-h-0 flex-1 flex-col` with an inner `min-h-0 flex-1 overflow-auto` region.
- One Send button, at the top.
- Phone sheet: same `Form`, so same order; rows unchanged (`min-h-11`, 44 px, measured in the fixture).
- Behaviour unchanged: `autoFocus={!req.text}`, disabled rule `n === 0 || !text.trim() || sending`, the 300 ms
  `dry_run: true` preview, the receipt summary, toast + close after send. DOM order is the visual order.
- `Form` is exported so the test can render it (the dialog itself renders into a portal).

## Decisions

- **D2: the Targets heading and quick-pick chips stay fixed;** only the Desks/Sessions rows scroll. The chips are
  selection controls the user wants at hand while scrolling the list, and keeping them out of the scroller costs nothing.
  The load error / "No live sessions" lines sit in the scrolling region where the rows would be.
- **No visible "Delivery" heading.** D1 asks for Delivery and Send on one row; a heading would split it. The toggle
  keeps `aria-label="Delivery"` and its two labels ("Steer (inside the running turn)", "Next turn") say what they do.
  The "Message" heading stays (it labels the first thing on the dialog, matches "Targets").
- **Desktop dialog widened from `sm:max-w-lg` (512 px) to `sm:max-w-xl` (576 px).** At 512 px the toggle (~311 px)
  plus Send (~160 px) did not fit the 464 px content width, so Send wrapped under Delivery. At 576 px they share the
  row (fixture: Send top 247 px in an 800 px viewport). Shrinking the toggle/button or shortening the labels would have
  changed copy or tap targets; widening is the boring fix.
- **Phone: the row wraps.** At 390 px (358 px content) the toggle alone takes ~311 px, so the row is `flex-wrap` with
  `ml-auto` on Send: Send drops right-aligned under Delivery, still above the preview line and the list, reachable
  without scrolling (Send top 341 px of 844 px).
- **Targets heading row wraps too** (`flex-wrap`): on the phone the four chips did not fit beside "Targets" and
  "None" was clipped at the right edge (visible in `base-phone.png`); they now drop under the heading.
- **Sheet bottom padding** `pb-[calc(1rem+var(--safe-bottom))]` on `SheetContent`: `SheetFooter` used to provide the
  bottom `p-4`; without it the last list row would sit on the edge. It keeps the safe-area inset the sheet already had.
- **Tab order** (system Chrome, fixture with no prefilled text): textarea (autofocused) → its Dictate mic → Delivery
  toggle → Send → All desks → Running roots → All roots → None → first section checkbox. The Dictate mic is part of
  `Textarea` and was already after the textarea. (WebKit's default skips buttons on Tab, so the order was checked in
  Chrome.)

## Dev fixture

`crates/desktop/ui/src/dev/broadcast-fixture.html` + `.tsx`: `bunx vite`, open `/src/dev/broadcast-fixture.html`
(`?theme=dark`, `?text=` for an empty message). Mounts the real `BroadcastDialog` against no engine: `list_activity` /
`list_sessions` answer 40 sessions (4 desks, 14 more roots, 22 children) and `broadcast` answers a dry-run receipt.

## Gates (crates/desktop/ui, under build-lock)

- `bunx tsc -b --noEmit`: 0 `error TS` (baseline before the change: 0, `/tmp/broadcast-top/tsc-base.txt`).
- `bunx vitest run src/components/chat/BroadcastDialog.test.ts src/lib/no-emoji.test.ts src/components/disclosure.test.ts`:
  3 files, 41 tests passed. New test: textarea, the single Send button and the preview line come before the first
  target checkbox.
- Full suite `BUILD_SLOTS=1 … bunx vitest run src`: 357 files, 2508 tests passed, 0 failed
  (`/tmp/broadcast-top/vitest-full.txt`).

## Proof

`/tmp/broadcast-top/proof.md`, screenshots in `/tmp/broadcast-top/shots/` (WebKit, Playwright webkit-2370).
