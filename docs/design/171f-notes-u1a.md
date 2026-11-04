# 171F U1a — decisions

UI-only; paths under `crates/desktop/ui/src/components/cells/`.

- **B1** `RunbookBar` is a solid `bg-layer-1` row in normal flow (no `sticky`/`z-10`/`backdrop-blur`), mounted by the
  reply's LAST cell after its `</CellExtras>`. Archive views (no `cellRef`) still get no bar.
- **B8** `followSuperseded(steps, nextStates, specOf)`: a step whose state has `superseded_by` becomes the successor
  (ref, state, spec from `registeredCells`, else the old spec, `seen` = the successor's `digest`) once its state is
  loaded. The bar cell registers successor refs watch-only (`register(…, { spec: null })`, which never overwrites a
  mounted cell's spec). `runAllPairs` sends `seen` for followed steps, `digest(body)` for the reply's own cells
  (unchanged). Unresolved replaced steps are still sent (the engine skips them, as before). All steps replaced and
  unresolved → disabled "Replaced by a newer version ↓". `superseded_by` is the newest successor (`actor.rs` `.rev()`),
  so one hop is enough.
- **B9** `openAsNote(api, qc, session, …)`: module `Set` in-flight guard keyed `session/entry`;
  `cell_reply_note_get` → id present in `doc_list('note', 500)` → `open(id)`; else `doc_create` + `cell_reply_note_set`
  + `open`. The old "Opened as a note · Open" toast is gone: the press now navigates (brief: "then open").
  Ceiling (`ponytail:` comment): a remembered note older than the 500 newest is made again.
- **B11** `Slice.pass` (optional) holds `StateReply.pass`; `lastPass(slice)` hides it while a run of another pass id is
  live (= a new pass started). `RunbookBar` labels from `pass.stopped` when its cell is one of the bar's steps
  (`Run all · stopped before <title> (i/n)` / `… stopped at a failed step (i/n)`, reason = `why`); the run-guessing
  `stopped()` is gone, the boundary prediction stays as the pre-run label. `NoteKernelPill` shows `passLine()`
  (clicking scrolls to `[data-ref-key]`); the stopping `NoteCell` gets "Run all stopped here: <why>" and a `ring-danger`.
  Position in the line: the cell's index among the note's mounted cells, else `ran (+1 when before)`.
- **B10** `NoteCell` row for problem `monty_insufficient` (editable notes only): "A regular Python can run it · Use
  Python" → `withPython(code, 'auto')`. The full reason is already the problem's own danger row in `Cell.tsx`
  (U1b's file renders `p.message`), so it is not repeated. `auto` chosen over the first `cell_interpreters` entry:
  no extra RPC, and `monty_insufficient` only fires on an explicit `python: monty`.
- **B13** converted: `RunbookBar` "why?" line → `useDisclosure(disclosureKey(scopeKey(scope), entry, 'runbook-why'))`.
  The runbook "…" `DropdownMenu` is a transient menu, not a disclosure: left uncontrolled. `NoteCell`'s `review`/
  `ranCode` toggles sit in U1b's disclosure hunk (~:110-180) and were not touched.
- **B26** `kernelLabel(steps, kernels)`: families of the bar's live steps (latest run's kernel spec, else
  `interpreter.kind`, else `spec.lang`), joined with ` + `; phase from the scope's live kernels of those families
  (busy > starting > idle); no matching live kernel → the families alone (`bash`).
