# As built — SURFACE-TABLE: chat tables scroll sideways, never squish

Branch `fix/surface-table-scroll` off `95f6496a`. Brief: `/tmp/mc-2026-10-02/omp-surface-table.md`.

## Root cause

`crates/desktop/ui/src/components/chat/Transcript.tsx:240` puts `wrap-anywhere` (`overflow-wrap: anywhere`) on the whole
transcript scroller. It is inherited by every table cell; `overflow-wrap: anywhere` also feeds min-content sizing, so
auto table layout was free to shrink a `w-full` column below its longest word and break "Effort" into "Ef / fo / rt".
The `overflow-x-auto` wrapper never engaged because the table never got wider than its container.

## What changed

- `crates/desktop/ui/src/components/chat/surface/registry.tsx`
  - `:185-186` `NO_MID_WORD_BREAK = 'break-normal wrap-normal hyphens-none'` (Tailwind v4 utilities for
    `word-break: normal; overflow-wrap: normal; hyphens: none`) — resets the inherited rule on `th`/`td`.
  - `:187-194` `cellClass(text)`: any whitespace-free run > 48 chars → `wrap-anywhere`; single token or ≤ 16 chars →
    `whitespace-nowrap`; otherwise `''` (wrap at word boundaries).
  - `:196-202` `textOf(node)`: text of a React cell (markdown inlines are elements around strings).
  - `:204-212` `Td`: `<td {NO_MID_WORD_BREAK} {cellClass}>` around `<div class="min-w-[6ch] max-w-[48ch]">`.
  - `:214-217` `MarkdownTd`: streamdown's default `td` classes (`px-4 py-2 text-sm`) + `data-streamdown="table-cell"` on `Td`.
  - `:257-283` `Table`: wrapper `max-w-full overflow-x-auto [scrollbar-width:thin]`; table `w-max min-w-full table-auto
    text-sm`; `th` `whitespace-nowrap` + `NO_MID_WORD_BREAK`; cells via `Td` with `px-2 py-1.5 align-top`.
- `crates/desktop/ui/src/components/chat/MessageAssistant.tsx`
  - `:7,:20` imports; `:34-36` `markdownComponents = { td: MarkdownTd }` passed as `components` to `<Streamdown>` (`:236`).
  - `:238` className gains `[&_table]:w-max [&_table]:min-w-full [&_table_th]:wrap-normal`. Streamdown's own table
    wrapper is already `overflow-x-auto` and its `th` already `whitespace-nowrap`, so markdown tables now get rules 1–5
    with the same helper. (Verified against `node_modules/streamdown/dist/chunk-YOKDWASO.js`: user `components` replace
    streamdown's per tag, so only `td` is overridden — overriding `table` would drop its copy/download/fullscreen bar.)
- `catalog.ts` unchanged (no new props).

## Behaviours and tests (`registry.test.ts`)

- `Table takes natural column widths …`: the five-column screenshot table → wrapper `max-w-full overflow-x-auto`, table
  `w-max min-w-full`, all 10 `th`/`td` carry the three no-break classes, every `th` `whitespace-nowrap`, "Small, one
  crate" cell `whitespace-nowrap`, the prose cell not.
- `cellClass: …`: `'Small, one crate'`/`'Low'`/40-char token → `whitespace-nowrap`; prose → `''`; 60-char token → `wrap-anywhere`.
- `MarkdownTd …`: exact markup — streamdown classes, marker, `style` passthrough, text read through `<strong>`.

## Gates (run in `crates/desktop/ui`)

- `bun install` at `packages/apps` and `crates/desktop/ui` (repo root has no package.json).
- `just ts-desktop` run ONCE: the fresh worktree had no `src/rpc/bindings` (464 files generated); `catalog.test.ts` and
  `tsc` need them.
- `bunx vitest run src/components/chat/surface`: `Test Files  7 passed (7)` / `Tests  93 passed (93)`.
- `bun run test` (`vitest run`): `Test Files  125 passed (125)` / `Tests  698 passed (698)`.

## Manual check (I cannot see the UI)

Paste the screenshot's table as a `json-render` `Table` and as a markdown `| … |` table into a chat. Expect: every header
on one line ("Effort" intact), the table wider than the message with a thin horizontal scrollbar on the table (drag or
two-finger swipe scrolls it), no word broken mid-character; "Small, one crate" on one line; a cell with a long
paragraph wraps at words at ≈48ch; a 60+ char hash/URL cell breaks inside the token instead of widening the table.

## Deviations

- `max-w-[48ch] min-w-[6ch]` sit on a `div` inside the `td`, not on the `td`: WebKit/Blink ignore `max-width` on table
  cells, so on the `td` rule 4 would be a no-op and `w-max` would lay long prose out on one line.
- Scrollbar: the app's only scrollbar utility is `[scrollbar-width:none]` (hidden toolbars), which would hide the
  scrollbar the user wants to see. Used the same arbitrary-property idiom with `thin`.
- Short-cell threshold is `<= 16` (brief: "~16"); the brief's own example "Small, one crate" is exactly 16 chars.
- `MessageAssistant.tsx` edited (not in the "own" list) to pass `components` and the `[&_table]` variants — the brief's
  markdown-table instruction has no other seam.

## Open follow-ups

- `Transcript.tsx:240` `wrap-anywhere` on the whole scroller is a broad default; other descendant tables (OpenUI,
  BoardReader) inherit it too. Out of scope here.
