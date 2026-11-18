# note-headings: H1 → H4 kept the same size (2026-11-18)

Report (Sky, 14:01): "in notes when I change something from h1 to h4 it stays the same size".

## Cause

CSS cascade, not the schema and not the level change.

- The schema is BlockNote 0.54.2's default `heading` (`HEADING_LEVELS = [1..6]`); no `levels` override is needed.
  H4–H6 are reachable on desktop through BlockNote's formatting-toolbar block type select (Heading 1–6), the
  `#### ` … `###### ` input rules and markdown import. The slash menu (59 §9.3) and the mobile "Aa" picker offer 1–3.
- The level does change: `updateBlock(id, { props: { level: 4 } })` sets `props.level = 4` and the content element
  gets `data-level="4"` (level 1, the default, carries no `data-level`).
- `styles.css` (59 §24.y D) sets `--level: 30px` on `html .bn-editor [data-content-type="heading"]` — specificity
  (0,2,1) — and overrides only levels 2 and 3. BlockNote's own `[data-content-type="heading"][data-level="4"]`
  (0,2,0) loses to the base rule, so levels 4, 5 and 6 all resolved to 30px, the H1 size.

## Fix

`crates/desktop/ui/src/styles.css`: `[data-level="4"|"5"|"6"]` at 18/16/14 px, completing Streamdown's scale
(`text-3xl … text-sm`: 30/24/20/18/16/14), which chat markdown already uses. BlockNote's `--prev-level` (the start of
its 0.2 s font-size transition on a level change, set for one frame on the block's outer) is put on the same scale,
so a change no longer starts from BlockNote's 3em/2em/… sizes. Menus unchanged (59 §9.3 keeps the slash menu at 1–3).
59 §24.y D updated.

Round trip: markdown `####`–`######` already import and export as levels 4–6 (BlockNote's converter); rows added to
`packages/documents/test/convert.test.ts`.

## Tests

- `crates/desktop/ui/src/components/editor/headings.test.ts`: the real BlockNote editor in jsdom over the app's
  `styles.css`; changes one block 1 → 6 and reads `--level` each time (old code: `30,24,20,30,30,30`), then checks
  `--prev-level` on a 6 → 1 change. jsdom cascades by source order, not specificity, so BlockNote's sheet is left out
  and the app sheet must carry all six levels itself.
