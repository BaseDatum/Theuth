# 171F-E1 notes — engine lifecycle, Monty reasons, Ask-theuth dedupe

Decisions taken where the brief left room, and changes Z must apply outside E1's column.

## Decisions

- **B16** `ShellKernel::kill` sends `CellKill{close:true}`; the actor's post-kill shutdown task is gone (the watchdog's own Kill → shutdown chain is unchanged). Python kills its process tree and Monty cancels its checkout as before.
- **B5** `stop_after(prev, how)` (actor.rs): a later timeout keeps any earlier user stop — `Stopped` *and* `Killed` (a user Kill is also the user's reason); Kill still overrides Stop; nothing overrides a timeout.
- **B18** `crockford(u128)` encodes the UUIDv7 as 26 Crockford chars, most significant first (first char `0-7`, like a ULID).
- **B19** the problem has `key: None` (the missing program is not one header key).
- **B11** `last_pass` is keyed by `json(scope)`; `CellsChanged{scope}` is published when a pass clears an old end and when it ends. `why` texts: the refusal's `Display` (`this cell is invalid: …`, `folder not found — …`, the `after` reason, `no pwsh on this machine`), `danger — runs from its button only`, the lint chip's text, `stopped by you` (a pass the user stopped between cells), and for a run that did not succeed: `exit N` / `signal S` / `expect missed M/N` / `timed out` / `killed` / `stopped` / `cancelled` / `lost`. `ran` counts runs that reached an end (superseded cells are skipped, so a clean pass may have `ran < total` in chat).
- **B10** `monty_reason` reads both ty renderings (full `error[rule]: msg` + ` --> file:L:C`, concise `file:L:C: error[rule] msg`); text with no recognisable diagnostic stays its own reason (first line), so an unknown format never turns into "sufficient". `monty_scan` reasons now carry `line N:` and the way out (`— Monty runs the standard library only; use a local python`; banned calls: `— Monty has no files, processes or network; use a local python`).
- **B10** `validate` also treats a note's `python: monty` default as Monty for `monty_insufficient` (inspect_note already ran the check for it; the result was ignored).
- **B30** names are deduped, ranked by edit distance (prefix match breaks ties), at most 5 then `… and N more`. "Close" = prefix either way, or distance ≤ max(2, len/3). None close: `` `after: x`: no cell named `x` in this chat|note ``.
- **B22** two layers: a ticketed `UserInput` (a wake delivery) removes parked quiet copies of its run (parked or passive steer); `join_quiet` drops ticket-less copies of a run another input of the same batch carries with a ticket.
- The brief's B10 test line was truncated after "a cell that onl…"; implemented as "a cell that only uses a name from an earlier cell is sufficient" (monty.rs `type_check_decides_sufficiency`, repl.rs `monty_sufficient_scans_then_type_checks_without_running`).

## For Z (outside E1's column — not edited)

- `crates/document/src/note_cells.rs:589` expectation becomes
  ``cell 2: after_unknown — `after: nope` names no cell in scope; cells here: one, preview``
- `crates/document/tests/actor.rs:2687` expectation becomes
  ``cell 1: after_unknown — `after: nope`: no cell named `nope` in this note``
- `crates/cells/src/kernel.rs:154` doc of `Kernel::kill`: "SIGKILL to the PTY's foreground group (the shell survives)" → "close the shell kernel (171F B16: the next run starts a fresh shell)".
- `crates/cells/cells.md:81,218` quote "Monty can't run this: no module named pandas"; the text is now "Monty can't run this cell: line 1: no module named pandas — Monty runs the standard library only; use a local python".

## Proposed design wording (Z edits `171-cells.md`)

- §7.3 Kill: "Kill closes the cell's kernel (the process group and the shell); the run ends `killed`, and the next run starts a fresh kernel — variables and cwd reset."
- §7.2 Run all: "Its end is on `cell_state.pass` (`PassEnd{pass, total, ran, stopped}`); `stopped.before` tells a boundary from a failed run."
