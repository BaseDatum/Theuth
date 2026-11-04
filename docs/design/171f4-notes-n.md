# 171F4-N notes: note cells re-read on edit, line ▶ hover grace, folded `#|` header, export with outputs, ⌘⇧Enter, queued attachments

Base `64ef1290`, branch `fix/171f4-n`. Files: `crates/desktop/ui/src/components/cells/{NoteCell,NoteKernelPill,Cell,
CellEditor}.tsx`, `cells/{line-run,note-edit}.ts`, `cells/note-cells.css`, `components/editor/CodeBlock.tsx`,
`crates/artifact/src/tools.rs`, `crates/artifact/tests/document.rs` + the cells tests.

## N1 "ran an earlier version" after an edit
- `NoteCell` re-reads the scope's record 800 ms after the last change of the block's text, code or header
  (`rereadAfterEdit`: each change restarts the wait; one `cell_state` per settled edit). It replaces 171F B10's
  header-only re-read.
- The footer writes the engine's stale mark as `doc_read` does: `ran 15:13 · ran an earlier version`. The cell's
  existing muted "ran an earlier version" notice (shared `Cell`, chat too) stays as it is.

## N2 line ▶ hover grace in notes
- Cause (reproduced in Playwright WebKit = Sky's WKWebView; Chromium is fine): over the padding band left of the
  code text, where the ▶ sits, WebKit's `caretRangeFromPoint` answers an offset in the first header line, so the
  hover became "no code line" and the ▶ vanished as the pointer travelled to it.
- Fix: the chat gutter's rule `hoverAfter` (171F B2) moved from `CellEditor.tsx` to `line-run.ts` and is shared;
  the note's `noteHover` feeds it: a move left of the code text (`code` element's left edge) is "left the text, still
  inside the editor" → the line is kept; over the text it follows the code line; leaving the slot clears it.

## N3 folded `#|` header
- Rule (`headerFolded`, `note-edit.ts`): folded unless the selection touches the header (caret on a header line, its
  end included) or the "#|" pill is on; never folded when no code follows the header (a cell being typed).
- Display only: `NoteCell` measures the header's height (top of code line 1 − top of header line 1, real rects, so
  wrapped lines count) and sets `--header-fold`; `note-cells.css` pulls the `code` element up by it and clips its top
  band (`clip-path`, no paint, no pointer). The text, `doc_read`, save and the caret keep the lines: ArrowUp from
  code line 1 enters the header, which unfolds.
- The pill is the cell's existing `#|` toggle (same disclosure key as chat). Chat shows the header as a read-only
  copy under the chips; in a note (`CellExtras.headerInEditor`) the toggle unfolds the editable lines instead, no copy.
- Typing at the top of a cell kept the caret only by luck: a `#` typed before `#| name` makes `##|` (plain code
  until the `|`), the block re-routes, its content moves to another element and the browser caret fell to the
  block's start (reproduced on the base in WebKit and Chromium: `#| env: A=1` came out `| env: A=1\n##| name…`; a
  plain block gaining `#|` likewise). `CodeBlock` now puts the caret back where ProseMirror has it
  (`prosemirrorView.focus()` in a layout effect on a route change while the block holds the focused caret).

## N4 `doc_export(format=markdown)` with outputs
- The app's export (RPC `doc_export`) already went through `document::reader_markdown`; the agent's `doc_export`
  tool (`crates/artifact/src/tools.rs`) asked `document::Markdown` directly. It now calls `reader_markdown` too
  (one call; `annotate` stays PDF-only). Private cells: the cells hook withholds a private run's text, as for
  `doc_read`.
- **Deviation**: the fix lives in `crates/artifact`, outside the brief's file set (not the other worker's either):
  the export is built there, not in `packages/documents` nor `crates/document`. No `crates/document` change.
- Test `markdown_export_carries_cell_outputs_and_run_lines` (artifact `tests/document.rs`, real helper): run cell →
  `output` fence + run line, quiet cell → run line and no fence, private cell → run line and no output. The two
  tests of that binary now share a `SERIAL` lock (the document actor's global name).

## N5 ⌘⇧Enter in the note
- `NoteKernelPill` listens on the document (capture): ⌘⇧↵ (Ctrl+Shift+Enter off Apple) with the focus inside the
  BlockNote editor (`.bn-editor`) that holds this note's cells (`data-doc` on each note cell) runs the same
  `runAllNote` as the button. The pill row's own handler stays.
- Conflicts checked: none. BlockNote/tiptap/ProseMirror keymaps bind Shift-Enter and Mod-Enter only (exact
  modifiers), the window shortcuts (`lib/shortcuts.ts`) have no Mod+Shift+Enter, the note cell's key handler ignores
  Shift+Mod+Enter, the chat runbook bar's binding is chat-only.
- §11.7 row updated in `171-cells.md`.

## N7 a queued message's attachments (chat)
- Cause: a steer sent while the turn runs showed through `InputQueued`, which carries the text only, and became its
  `you` row on `InputInjected` from that text, so the row had no `files` until the settled reload after the turn
  (the stored row's attachments). An idle send shows them at once through `markSent(…, files)`.
- The data is client-side: the composer's `expand` gives the attached paths, the same `files` a send passes to
  `markSent`. `Composer.submit` puts them on its own `InputQueued` (client-side `files?`, `rpc/events.ts`, like
  `ToolCallFinished.files`); the reducer keeps them on the queued item (the engine's echo, same id, changes nothing);
  `QueuedChips` shows them with `Attachments` (thumbnails capped at `max-h-16`, file chips); `InputInjected` moves them
  onto the `you` row, which `MessageUser` renders like a send's — no gap, the same paths. A reload while queued keeps
  them by id (`list_queued` has none).
- Also fixed on the way: ⌘↩ "deliver now" withdrew the queued steers and re-sent them with `files: []`, so their
  attachments were lost for the model too; it re-sends each with its own files now.
- Not fixed: clicking a queued chip to edit it (`withdraw` → composer) brings back the text only, not the
  attachments (the composer's paste shelf is not rebuilt).
- Visual proof: throwaway harness `/tmp/171f4-n/fx/queue.tsx` (QueuedChips above an input + the store's user rows,
  fed the Composer's events); shots `{before,after}-n7-{1-queued,2-delivered}-{desktop,phone}.png`.

## Visual proof
Throwaway harness `/tmp/171f4-n/` (not committed): vite dev on :5207 with the repo config, Playwright WebKit
(WKWebView's engine) on the repo's `cells-notes-fixture.html`, and a copy answering `cell_run_all` for the ⌘⇧↵ check.
BEFORE = base, AFTER = branch; shots in `/tmp/171f4-n/shots/` (1280×900 and 390×844). WebKit instead of Chrome:
the N2 bug does not occur in Chromium.

## For the other worker (E)
- Optional (N7): `QueuedInput` / `list_queued` carry no attachments, so after an app reload a queued steer's chip
  shows no thumbnail until it is delivered. Adding its file paths there would close that; the UI reads `files` from
  the queued item already.
