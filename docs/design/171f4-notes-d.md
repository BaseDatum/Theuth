# 171F4-D notes: note outputs after Open as note, superseded "armed by", line-run feedback

Base `a76092c4`, branch `fix/171f4-d`. Files: `crates/document/src/note_cells.rs`, `crates/document/tests/actor.rs`,
`crates/desktop/ui/src/components/cells/{Cell,CellEditor,CellFence,NoteCell}.tsx`,
`cells/{open-as-note,note-edit,line-run}.ts` + tests.

## T2-3 one output per note cell
- **Decision: replace, newest wins.** The history Open as note writes after a cell (an `output` code block and/or the
  `ran …` paragraph right after it) gives way as soon as the cell has a run of its own in the note. Two places, so
  every reader agrees:
  - `doc_read` markdown (`note_cells::with_runs`): when the hook has a run for a cell, the following `output` fence and
    the following run-line paragraph (`ran HH:MM · …` / `ran — · …`, block comments skipped) are dropped and the
    run's own fence + line written instead. A cell without a run keeps its history, except an empty `output` fence,
    which is never shown (2.18.2 notes already hold them).
  - The note itself (`NoteCell`): on the cell's first record with a run, the owner's editor removes those blocks
    (`historyAfter` in `open-as-note.ts`), so the UI model, members and the derived `note.md` lose the duplicate too.
    Only an editable (owner) view edits; guests never write.
- **No empty fence**: Open as note writes only the run line for a cell whose run printed nothing.
- **One time source**: the UI `runLine` now writes the engine's line character for character
  (`crates/cells/src/note.rs` `run_line`): the run's end time (`ended_ms`, else `started_ms`) to the minute,
  `exit <code|signal|timeout|killed|lost|cancelled>`, seconds `{:.1}` under 10 s else `{:.0}`. The old UI line fell
  back to `queued_ms` and wrote `exit` only for codes. Ceiling (ponytail comment): JS `toFixed` vs Rust `{:.1}` differ
  on an exact binary tie only.
- Why not merge: the static block is a chat run (another kernel, another time); showing both was the bug.

## T2-4 superseded "armed by"
- `Cell.tsx`: the armed line renders only when `superseded_by` is null. The replacing cell keeps its own line.

## T2-6 UI half — line ▶ feedback
- `line-run.ts`: pure state machine (`pressed`/`started`/`settled`/`refused`) + `useLineRun(latest)`. A line or
  selection run marks its (first) line `running` (◌, pulsing); a repeat press of that line while running is ignored
  (no `cell_run`); when the run ends with no output (`output_bytes` 0/null) the mark becomes ✓ (exit 0) or ✗ for
  1.5 s, then the normal ►. Output, a newer run of the cell, or a refused press clears the mark.
- Chat (`CellEditor`, CodeMirror gutter marker) and notes (`NoteCell`, positioned overlay at the line) share it.
  `CellFence.run` / `NoteCell.run` now return the press promise so the mark learns its run id.
- Monty bare-expression echo is worker E's; once it prints, the mark simply clears (output answers).

## Visual proof
Throwaway harness `/tmp/171f4-d/fx/` (not committed): vite dev on :5199 with the repo config, headless Chromium.
BEFORE = base `Cell/CellEditor/CellFence` checked out temporarily, AFTER = branch. Shots in `/tmp/171f4-d/shots/`.
Base: 3 presses on a running line → 3 `cell_run`s, no gutter mark; superseded apply shows "armed by". After:
1 `cell_run`, ◌ on the running line, ✓ on the quiet line clearing after 1.5 s; superseded apply shows only
"Replaced by a newer version ↓". The note-cell overlay was not screenshotted (same hook; unit-tested).

## T2-8 `<child_stalled>` card (added 15:05)
- `injected.ts`: `CARD.child_stalled` and `CARD.peer_state` (key `child="`) → one `child` kind (`event: stalled|state`,
  child, session, word, secs, `last?`, clock, text); attributes read with a leading space (`quiet_secs` never matches
  `secs`), escapes undone. `HarnessCards.ChildStateCard` follows `ChildLostCard`: hourglass (stalled) or activity
  (state) icon, the child chip linking to its session, the phase/state in words (`no first update`, `gone quiet`,
  `done`, `not running`), `quiet 1 min` / `waited 6 min`, the time; `last: …` as the muted line; the guidance
  sentence is the folded body. `childLine` is the label.
- `<peer_state>` (155 §5.3, a `wait_peer` settled) rendered raw the same way: same card.
- `<notebook_changes>` (157 D2) rendered raw: hidden like its sibling `<notebook>` (the context line summarises).
- Ratchet `lib/injected-wrappers.test.ts`: scans `crates/{agent,acp,autoresearch,cells,decision,double,dreaming,
  knowledge,meeting,roster,schedule,surface,talk,tasks,tools}/src` (test modules and `tests.rs` excluded) for a string
  literal opening `"<name` whose `</name>` closes in a string literal of the same file. Each must be
  `isClassified` (CARD/HIDDEN/notices/skill/resume) or in `NOT_A_ROW` with its reason; stale entries fail too.
  Ceiling: wrappers built with `format!("<{name}>")` (loop notes) or piecewise `push_str` are invisible to it.
- **Not fixed**: `<study …>` — an autoresearch experiment session's opening brief followed by instructions; a card
  needs the brief's shape split from the instructions (a user prompt, not an injected wake). Listed in `NOT_A_ROW`.
- The TUI's own classifier (`crates/tui/src/model.rs`) still shows these as text: not in my file set.

## For the other worker (E)
- Nothing required. If `run_line` in `crates/cells/src/note.rs` ever changes, `open-as-note.ts` `runLine` must follow.
- `CellRun.output_bytes` 0/None is how the UI decides "quiet"; keep it accurate for bare-expression echoes.
