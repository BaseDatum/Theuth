# CELLPROMPT — a running cell's prompt is invisible until answered

Sky, 2026-11-06 (2.18.5, note cell, bash on a PTY): a `[Y/n]:` prompt with no trailing newline never shows while the
cell waits; it shows once the answer's newline pushes it up a row.

## Reproduction (base 5d2136c5, scratch engine `/tmp/cellprompt/eng`, installed `t` 2.18.5, vite from this tree)

| case | result on base | shot |
|---|---|---|
| note bash cell, 8 rows then `read -r -p '… [Y/n]: '` | prompt visible (cursor on row 8 of a 12-row box) | `before-note-short-waiting.png` |
| note bash cell, 40 coloured rows then `python3 -c "input('Continue? [Y/n]: ')"`, 1280×800 | **hidden**: row 30 (the cursor row) is under the box's horizontal scrollbar | `before-note-long-waiting.png` |
| same, 1280×620 (60vh < 30 rows) | **hidden**: rows 26–30 clipped below the box | `before-note-small-waiting.png` |
| chat cell (`CellFence`, the 171 fixture fed 30 rows + the prompt) | **hidden**, as the note cell | `before-chat.png` |
| note **python** cell, `input('… [Y/n]: ')` | **no input line at all**: "running…" only, nothing to answer with | (probe, `cell_live` shows `input` set) |

Screenshots are in `/tmp/cellprompt/shots/`. Sky's tool printed far more than 30 lines before the prompt, so the grid was
full and the prompt sat on the bottom row — the hidden case.

## Which layer drops it

Not the engine's byte path: the prompt bytes reach the xterm (its row 29 reads `Continue? [Y/n]:` in every hidden case).
`shell.rs` `LiveCut::feed` holds back only a tail that may start `ESC]133;D`; `actor.rs` `live_bytes`/`attach` forward
raw chunks; `render.rs` `render_capped` keeps the cursor row (visible rows, only blank trailing rows dropped) and is the
finished-run path anyway.

1. **Shell cells — the UI box** (`crates/desktop/ui/src/components/chat/TerminalViewer.tsx:330-334` on base). The cell
   box had a fixed height of exactly the rows in use (`rows × rowH + 8`, `overflow-hidden`), and the xterm host inside
   it was `overflow-x-auto overflow-y-hidden`. The 100-column grid is wider than a note/chat column, so the host shows
   a horizontal scrollbar, which takes its height out of that fixed box and covers the last row — the cursor row when
   the grid is full (or when exactly the rows in use are shown). Under `LiveTerminal`'s `max-h-[60vh]`
   (`Cell.tsx:249`) the `flex-initial` box also shrinks and clips its bottom rows. `overflow-y-hidden` made both
   unscrollable.
2. **Python cells — `cell_state`** (`crates/cells/src/actor.rs:1892` on base). `state()` took `latest` from the store
   row; the `input()` prompt lives only on the live record (`waiting_input` sets `Live.record.input`; the store writes
   `input: None`, `store.rs:119,592`). The UI's `InputLine` (`Cell.tsx:533`) reads `latest.input` from `cell_state`,
   so it never appeared.

## Fix

1. `TerminalViewer.tsx` (cell source only; the pty source is unchanged): the box has no height of its own and is the
   scroll container (`flex flex-col-reverse overflow-auto`); the xterm host carries the rows' height
   (`w-max shrink-0 overflow-hidden`, `height: rows × rowH`). `Cell.tsx` `LiveTerminal` puts the 60vh cap on the box
   itself (`className="max-h-[60vh]"`) and its parent is a plain block (the old `flex max-h-[60vh] flex-col` wrapper is
   gone): WebKit (WKWebView) does not grow a *flex item* by its own horizontal scrollbar, a block child it does (R1-1,
   seen by the reviewer in the real WKWebView). The box is then rows + its own scrollbar, so the bar never covers a row;
   capped by 60vh it scrolls, and `column-reverse` starts at and stays on the bottom while the user has not scrolled up
   (native scroll anchoring, no JS). `colorScheme: 'dark'` keeps the scrollbar on the soot canvas under the light app
   theme. xterm still renders and updates the partial row in place (it is the emulator), so carriage-return progress
   and later output behave as before.
2. `actor.rs` `state()`: `latest` is the live record when the cell has one (`live_of_cell`, as `run()` and `find_run`
   already do), else the store's.

Finished runs are untouched (render, B3 padding trim, B14 echo cut, `^C`).

## Tests (each fails on base, passes with the fix)

- UI `TerminalViewer.test.ts`: `the cursor row of a waiting cell stays in view: …` (box has no fixed height, scrolls
  from the bottom); `171F B12: …` updated (the rows are the host's height, 180px for 12 rows).
- Engine `actor/tests.rs` `input_prompt_is_shown_until_answered`: now a note cell read through `cell_state`, with the
  run's store row inserted first (no prompt), so it fails for store-first and for `store.or(live)` (checked: that
  mutation fails `left: None`). The prompt shows on `latest.input` while waiting and is gone after `cell_input`.

## WebKit check (R1-1; Playwright's cached WebKit `webkit-2370`, headless, forced classic 17 px scrollbar)

Chat fixture `CellFence` (temporary copy, deleted), 1280 wide:

| case | host (rows) | box | bar | vertical overflow | prompt row | row 1 |
|---|---|---|---|---|---|---|
| 8 rows + prompt, 1280×1000 | 180 | 205 (= 180 + 8 + 17) | 17 | 0 | visible | visible |
| 30-row grid, 1280×1000 | 450 | 475 (= 450 + 8 + 17) | 17 | 0 | visible | visible |
| 30-row grid, 1280×620 (cap 372) | 450 | 372 | 17 | 103 | visible, pinned (scrollTop 0) | reached by scrolling up |

This Playwright WebKit build gives 205 for the pre-R1 flex layout too, so it does not reproduce the WKWebView 188 px case.
The R1 layout is measured correct here, and the reviewer saw it correct in the real WKWebView. Shots:
`after-webkit-*.png`.

## Live proof after the fix (`target/debug/t` from this tree, same scratch engine, vite, headless Chrome via CDP)

- Note bash cell, 40 coloured rows + `Continue? [Y/n]: `: the prompt row is visible above the scrollbar while the cell
  waits (1280×800 dark, 1280×620 light — the box scrolls and sits on the bottom); typed `y` + Enter into the live xterm
  (the `cell_input` typing path): `got y`, `done`, exit 0. `after-note-long-*.png`, `after-note-small-light-*.png`.
- Chat cell (fixture `CellFence`): `after-chat-dark.png`, `after-chat-light.png` — the prompt shows on row 30.
- Note python cell: the input line `Do you want to … [Y/n]:` + Send shows while it waits; Send `y` → `INFO got y`,
  exit 0. `after-note-python-*.png`.
- `\r` progress then a prompt: live rows `progress 100%` / `Continue? [Y/n]:` (no duplicate), record the same.
  `after-note-progress-*.png`.
- A python cell's own output still shows when the run ends (unchanged §7 behaviour; only shell cells stream live).

## Decisions

- No `render.rs` change and no render test: the renderer keeps the cursor row already; a test there could not fail on
  base. `out.txt` trims trailing blanks (documented contract), so its text ends `[Y/n]:`, not `[Y/n]: `.
- CSS `flex-col-reverse` over a JS stick-to-bottom: WebKit (Tauri) and Chromium both anchor column-reverse scrollers to
  the end. Consequence: a user scrolled up stays at the same distance from the bottom when the box grows a row.
- The chat-cell proof uses a throwaway copy of the 171 dev fixture (real `MessageAssistant → CellFence → Cell →
  Terminal`, fake RPC): a real chat cell needs a model reply, and the brief forbids a provider key. Deleted before commit.
