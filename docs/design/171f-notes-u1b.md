# 171F-U1b — decisions (cell body: edit, gutter, heights, timeout text, queued order)

## B21 — a chat cell could not be edited
- Cause: `CellEditor` kept CodeMirror `readOnly` until an invented edit mode was entered (Enter, or a double-click on
  the wrapper): a click followed by typing did nothing, which is what Sky saw. The edit buffer was also `useState` in
  `ChatCell`, so any remount of the reply's markdown dropped a typed edit.
- Fix: the editor is `readOnly` only while `locked`; `locked = !chatEditable(view, runnable)` (`view.ts`): runnable and
  not running/queued/superseded/invalid/guest. A pending reply (`runnable` false) stays locked so a streaming fence and
  an edit never fight. The Enter-to-edit binding and the double-click are gone; Enter types a newline, Esc leaves the
  editor (blur). "Esc returns an unedited cell to read-only" now means: the cell keeps no edit state of its own, so
  leaving it unedited is exactly the read-only look (no "edited" chip).
- The edit lives in `components/cells/edits.ts` (`memory.edits`, by `refKey`); typing back to the original text drops
  it. The press already sends it as `cell_run(…, edited)` (F4); unchanged.
- Touched in `CellFence.tsx`: the edit-state lines (`edited`, `onEdit`, `locked`) and two import lines for them.

## B2 — the ► vanished on the way to it
- `hoverAfter(current, event)` (exported, tested): a move (text or gutter) sets the line; leaving the text for a node
  inside `v.dom` keeps it; leaving the editor clears it. The gutter got a `mousemove`; the wrapper `div` clears on
  `mouseleave` (covers the line-number gutter, which has no handler).
- `NoteCell` `onMove`: off the code text is a plain `return`; the slot's `onMouseLeave` clears.

## B3 — trailing blank rows
- `trimBlankTail` (exported from `Cell.tsx`) drops trailing newline/space-only rows and keeps SGR codes, applied to the
  stored text and to the "show all" page, so old runs shrink too. The 28rem cap stays.

## B12 — live terminal height
- `Cell.tsx` `LiveTerminal` dropped `h-[32rem]` (keeps `max-h-[60vh]`). `Terminal` (cell source only) sets its own
  height: `liveRows(used, 12, 30) × rowH + 8 px`, grows monotonically on `onWriteParsed` (alternate buffer or
  `baseY > 0` → 30, else `cursorY + 1`), row height measured from `.xterm-screen` (15 px until measured). The PTY is
  never resized; pty sources are unchanged. The cell host scrolls only sideways (the unused grid rows are not
  scrollable into view).
- The box is `flex-initial` for a cell source: under the column parent `flex-1`'s 0% basis ignored `height` (seen in
  the first AFTER shot: 440 px instead of 181 px).
- Proof (fixture, 1280×900): two-line run 181 px box (12 rows) vs 440 px before; 20 rows of output → 296 px.

## B5 — timeout text
- Notice keys on `latest.state`: stopped → "stopped by you", timeout → "timed out after 5.3 s" (`duration`), killed →
  "killed — the kernel was closed; the next run starts fresh". `exitText` returns "timed out" / "killed" / "lost" /
  "cancelled" before the exit code (mirrors `deliver::exit_word`). Kill's tooltip: "Kill — closes the kernel; the next
  run starts fresh".

## B7 — queued chips above the kernel strip
- `Composer` `strip?: ReactNode`, rendered right before the box; the route passes
  `strip={<KernelStrip session embedded />}`; the `watching` branch keeps a bare strip above `ObservedBar`.
  `KernelStrip embedded` drops `mx-auto px-4 md:max-w-[768px]`.

## B27 — auto python chip
- `interpreterText(cell, interpreter)` (`view.ts`): python cell with `python:` absent or `auto` →
  `auto → python 3.14.8 (system)`. Used by the chat chip (`Cell.tsx`) and the note `PythonChip` text.

## B13 — disclosure sites converted (useDisclosure / store)
- `Cell.tsx`: show-all (`refKey|run|all`, the full text through react-query, cached by run), output shown/hidden
  (`refKey|output`), fold (`refKey|fold`), raw `#|` header (`refKey|raw`), history menu (`refKey|history`); picked
  history run → `edits.ts` `picked`; Stop time (Kill arming) → `edits.ts` `stopped` by run id (visible: Stop stays
  disabled and Kill keeps its 5 s after a remount). `focused` and the aria announce stay local (not visible state).
- `KernelStrip.tsx`: "N kernels" (`session|kernels`).
- `ansi-output.tsx`: stream reveal (`OutputDisclosure base|stream|name|length`); `json-output.tsx`: each path
  (`base|json|path`). The base key comes from a new `OutputDisclosure` context (`refKey|run`) that `Cell`'s
  `RunOutput` provides. Ceiling (ponytail comments): `OutputArea` passes no output index, so two same-length streams
  or two JSON outputs of one run share a toggle.
- `NoteCell.tsx`: python menu (`refKey|python`), member review (`refKey|review`), ran code (`refKey|ran-code`).
  `PythonChip` takes a new `at` prop; its one call site (the `chip:` line of `extras`, just above U1a's rows hunk)
  passes `at={key}` — one token, listed for Z in case of a conflict.
