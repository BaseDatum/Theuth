# 171 CELLS — notes of UI worker U2 (notes)

Decisions taken where the design, the brief or the skeleton was silent (simpler option, contract intact).

## Contract points siblings must know

1. **`cell_output_file(run, "code")`** is how the UI reads the code a run ran (§8.4 `runs/<run>/code`): the stale cell's "show the code that ran" and the member "Review diff". **E1** must serve `code` (base64 of the file, placeholders unexpanded) to the owner like any other run file.
2. **`member_edit` is a pub8**; the UI names it from `DocMeta.members` (`pub.startsWith(pub8)`), else shows the pub8. The press sends the same pub8 as `member_ack`.
3. **`seen` for a note cell** = `grammar.ts` `digest` of the block's whole text (header + code) as BlockNote holds it — what `CodeBlockText` must digest (E2).
4. **Run-line / run-selection offsets** follow U1's rule: `line` is the 1-based code line (header excluded), selections are UTF-8 byte offsets into the code (`note-edit.ts` `modeOf`).
5. **Note Run all** sends `cell_run_all(Doc, [(ref, digest)…], p_…)` with the note's mounted cells in DOM (= document) order; the engine applies the §7.2 boundaries.
6. **`cell_latest` row id** is read as `"<doc>:<name or block id>"` (the `NoteKey` value), exactly §13.2.

## Deviations (STUB-DEVIATION / ADD)

- `Cell.tsx` (U1), marked `171:U2-ADD`:
  - `CellExtras` gains `runText` (button text, "Run Alice's edit"), `rows` (note notice rows), `chip` (the python chip as a menu), `below` (guest output under the editor), `footer` ("ran 14:02").
  - `Chip` and `Notice` are exported.
  - The chrome around the editor slot is `contentEditable={false}` (header/notices/raw, below/footer/fold, output). Inside a note the cell is a BlockNote node view; ProseMirror cannot host a nested editing host, so only the code content may take the caret. No-op in chat.
- Frozen props, additive in my own files (`171:U2-ADD`): `NoteCellProps.{block, blocknote}`, `CellDefaultsProps.{second, onOpen}`, `GuestCellProps.{cellRef, editor}`. `NoteCellProps.code` carries the block's whole text.
- L11 fields are typed locally as optional extensions: `CellState.host` → host chip; `KernelState.env_warning` → the pill's "Refresh" chip. `CellRun.masked` / `CellRun.input` are left to U1's `Cell` rendering (L11); `NoteCell` passes `onInput` (`cell_input`, base64 like chat).
- Files outside my row: `src/dev/cells-notes-fixture.{html,tsx}` — a separate dev page beside U1's `cells-fixture`, so the two never conflict.
- New files in my directory: `note-edit.ts` (pure helpers: routing, Make it a cell/plain, slugs, run modes, diff, python header), `note-context.ts` (`NoteHost`), `note-cells.css`, and four `*.test.ts`.

## Decisions

- **Where the context comes from.** `Editor` wraps BlockNote in `NoteHost {provider, guest}`. The block node views are React portals inside `BlockNoteView`, so they see it. `Editor` gains `guest?: Api`, which `GuestApp` and `NotebookGuest` pass; every other mount is the owner's (`useApi`). Cells therefore work in any document the `Editor` shows (notes, notebook panes, pop-outs, knowledge, meeting notes): the scope is `Doc` and the engine decides.
- **Routing** is one pure function, `routeOf(language, text, host, selected)`: cell / guest / defaults line / today's path, before the diagram branch. Mermaid and plain code are untouched.
- **The code shows its header.** BlockNote's content is one text node, so the `#|` lines cannot be hidden inside it; the chips repeat them. CodeMirror is not used over the Y.Doc (D14).
- **A note cell never folds.** The folded row has no editor slot and BlockNote needs its content mounted, so `extras.open` is always true.
- **Keys**: one capture-phase `keydown` listener per cell on `document`, active only while the ProseMirror selection is inside that block. Shift+Enter runs the cell and moves the caret to the next note cell. Mod+Enter runs the selection, else the caret line. Mod+. sends Stop (Kill stays on the button). Danger, disarmed and invalid cells refuse with "press <label> to run this".
- **Gutter ▶**: a positioned overlay. `caretRangeFromPoint` gives the hovered text offset, so wrapped lines resolve to the right code line. The selection pill "Run selection ⌘↵" sits top right while the selection is non-empty.
- **Make it a cell / Make it plain code** are block-menu items (the drag handle's menu: BlockNote's defaults plus ours), each one `updateBlock` (one transaction, one undo step — verified in the browser). On BlockNote's `shellscript` (what its Shell entry and the ```` ```bash ```` input rule store), Make it a cell also switches the language to `bash`, because only `bash` is a cell language. Phones have no side menu, so neither item is there.
- **Paper styling** lives in `note-cells.css`: the code block's well gives way to the card (`:has(> [data-note-cell])`), the cell sits on `--card` (the paper; there is no `--sheet` token), and the code area is a light `--muted` mix.
- **Defaults line**: the first `cells` block shows "Cell defaults · cwd · shell label · python". The shell label comes from `cell_interpreters(cwd)` matched by path; without an engine, the path's basename. Clicking moves the caret into the block, which reveals the text (the source stays mounted, parked, like a diagram's). A later `cells` block reads "a second `cells` block is ignored" (the engine's `second_defaults` text).
- **Header pill**: one per live kernel, "python · .venv 3.12 · idle", with Run all; Restart/Close in its ⋯ menu. With no live kernel and cells on the page, one quiet "cells · Run all". Mod+Shift+Enter works while focus is in the pill row. Mounted in `NoteView`'s title row (note page, pop-outs, the meeting pane) and `NotePane`'s header.
- **Guests**: `GuestCell` = U1's `Cell` in view `guest` (no button, no keyboard run, "Only the owner can run cells"), plus the replicated text and "exit 0 · 33 s · expect ✓ · latest run · owner · 14:31". A private cell reads "output kept private". Guests get no cell events, so `cell_latest(doc)` is re-polled every 20 s while a cell of the doc is mounted (`ponytail:` note in `GuestCell.tsx`). `GuestApp` now brings its own `QueryClient` (the cell chrome uses react-query), as `NotebookGuest` already did.
- **Python chip** (owner, python cells): a menu of `cell_interpreters(cwd)`, python and Monty only. A pick rewrites/adds `#| python:` as one `updateBlock`. The shell chip's "write note default `shell:`" (§6.3) is not built.
- Monty: while a Monty run is in flight the cell shows "Monty shows output when the run ends".

## Known gaps

- `shellscript` blocks with a `#|` header are not cells (the grammar's language table has no `shellscript`). Users who type ```` ```bash ```` in a note get `shellscript` from BlockNote's input rule. Make it a cell converts them, but a hand-typed header stays code. Fix belongs to `surface::cell` + `grammar.ts` (an alias) if wanted.
- Switching a block between plain code and cell remounts the `<code>` element (a different React tree). BlockNote moves its contentDOM into the new element. Fine in the browser check; watch for caret jumps when typing the first `#|`.
- No live update of guest outputs beyond the 20 s poll; no images for guests (wave 3).
- The pill's Run all orders by DOM, so a note open twice in one window would send its cells once (deduped by ref).

## Tests

- `note-edit.test.ts`: CodeBlock routing (cell / guest / defaults / plain / mermaid / no host). Make it a cell and Make it plain code are each one change on a headless BlockNote editor; name from the nearest heading; `shellscript` → `bash`. Slash "Cell" content. `Note{doc, Name|Block}` refs. Run-line/run-selection offsets incl. UTF-8. Member diff, member names, python header, first defaults block.
- `NoteCell.test.ts`: the press (Note ref, `seen` digest, `member_ack` only when given). Member edit "Changed by Alice · Review diff · Run Alice's edit". Never run "Written by Alice · Review". Stale "show the code that ran" + "ran 14:02". Never folds, chrome non-editable.
- `NoteKernelPill.test.ts`: one pill per live kernel with Run all, nothing without kernels or cells, the quiet pill; Run all = one `cell_run_all` with ordered digests and a `p_` id.
- `GuestCell.test.ts`: latest output + "latest run · owner · 14:31", never a Run/Stop/Ask button. Private / no run yet. The defaults line and the second-block warning.
- Existing editor/notes/notebooks/share tests unchanged and green.
- Visual: `src/dev/cells-notes-fixture.html`, screenshots in `/tmp/171-u2-shots/`. Checked in the browser: the hover ▶ shows "Run line 2"; the selection pill appears; Mod+Enter reaches `cell_run` with no newline inserted; Make it a cell names the cell `checks` and one ⌘Z undoes it; the defaults line reveals its text; Review diff and "show the code that ran" work. At 390 px, "Run Alice's edit" is full width and 48 px tall.
