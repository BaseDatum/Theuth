# Streaming markdown

Status: shipped · 2026-09-09 · extends `00-architecture.md` §8.

## 1. Problem

Assistant text arrives as `TextDelta`s. Re-rendering the whole message per
delta is O(n²) and flickers; rendering a half-open construct (`**bol`) is
wrong; showing raw markdown forever is ugly.

## 2. Decision

Crate `markdown` (no UI dependency) with two halves:

- **Splitting**: `mdstream` 0.3 (`sync`), the crate jarvis used for the same
  job in its agent, voice and slack surfaces. `append` returns blocks that are
  now *committed* (never change again) and the *pending* tail; `finalize`
  commits the rest. `FootnotesMode::Invalidate`: a live UI cannot retract a
  drawn block.
- **Rendering**: `render(md) -> Vec<Line>`, `Line = Vec<Span>`, `Span { text,
  Style { bold, italic, strike, code, dim, link } }`. A pulldown-cmark 0.13
  event walker, ~200 lines. No colours or escape codes: each frontend maps
  `Style` onto its own primitives (iocraft `MixedTextContent` today; a Tauri
  webview would map to CSS classes or ignore this crate and use
  react-markdown on the raw text, which it also gets).

`markdown::Text` is what a UI holds per assistant message: `push(delta)`
renders newly committed blocks once and records where each ends in the raw
buffer; `finish()` commits the tail; `Text::complete(md)` for history replay;
`raw()` for the verbatim text (copy, line printer).

A third, small half (added in 43, 2026-09-17): `spoken(md) -> String`, the
same event walk reduced to what a listener needs — link text without URLs,
`Code block omitted.` for fences, headings and items as sentences. The `voice`
plugin calls it on a reply before synthesis; nothing in the TUI does.

**Reveal cursor.** Provider deltas are word-sized chunks; the eye wants a
steady flow. `reveal(chars)` advances a cursor over the raw buffer at whatever
pace the UI ticks; `view()` returns the blocks whose end the cursor has passed
(rendered) and the revealed remainder (plain). The same characters type out
plain and then snap into markdown once their block is behind the cursor.
`unrevealed()` tells the UI whether to keep ticking; `reveal_all()` for
history. Timing lives in the UI, not the crate.

Rejected: publishing committed blocks as a bus/RPC event. The UI already has
the deltas, so it can split locally; an event would need a byte-offset
mapping between agent and UI buffers (mdstream normalises newlines) and would
put presentation in the agent. Rejected: `termimad`/ANSI strings - iocraft
strips ANSI from `Text`.

Why a hand-written walker (rule §1.2): no crate renders markdown to a
renderer-agnostic styled span model; every terminal renderer binds to
ratatui, crossterm or ANSI.

## 3. TUI

`Entry::Assistant(Box<markdown::Text>)`. `TextDelta` → `push`; any other event
for the shown session finishes the open text (same rule as reasoning), so text
before and after a tool call are separate entries. A 60 Hz `use_future`
interval calls `Model::tick` only while `revealing()` (`State::read` does not
schedule a render, so idle frames cost nothing); each tick reveals
`max(1, backlog/16)` characters, so the cursor trails the provider by ~16
frames (~250 ms) at any stream rate and settles smoothly. Rendering:
`view().lines()` as `MixedText` rows (bold → `Weight::Bold`, italic, code → cyan, dim/strike →
dark grey (iocraft has no strikethrough), link → underline + OSC 8), a blank
row, then `view().plain` as one wrapped `Text`. The non-TTY line
printer is unchanged: raw text.

Rendered shapes: `# ` dim + bold heading; `• ` / `1. ` list markers, nested by
indent; `│ ` quote bar; `╭ lang` above fenced code; `────────` rule; table
cells padded to their column's display width (`unicode-width`), joined by
` │ `, `─┼─` rule under the header, `:--`/`--:`/`:-:` alignment honoured;
`[x] ` task markers; `[^n]` footnotes.

Ceiling: wrapped continuation lines lose the list/quote indent (iocraft wraps
the whole `MixedText`); fix by measuring width and pre-wrapping in `render`
if it grates.

## 4. Verified

Typewriter (pty sampled at 100 ms, 46 changed frames over a 14 s turn):
`## Riv` at 1.9 s → `## Rivers` rendered at 2.0 s while the next paragraph
types `Ri…`; `**fresh water for drinking**` types out with the stars visible,
then the paragraph snaps to bold once its separator is revealed; `- Rivers…`
list items type plain and become `• Rivers…` at the end.

`cargo test -p markdown` (structure + streaming); `cargo test -p tui`.
Real pty (pyte, 100×40) against Anthropic:

- 1.8 s in: transcript shows `#` - the raw pending tail.
- 3.3 s in: `# The Vital Arteries of Our Planet` rendered (dim `# `, bold
  title), paragraph below still raw with `**shaping the very geography**`.
- Done: paragraphs separated by one blank row, `1.`-`4.` list, `│ "…"` quote,
  no markdown punctuation left.
- Attributes (NO_COLOR unset): `## Plan` → `## ` fg 7f7f7f + `Plan` bold;
  inline code and code block fg 00ffff; only the `very bold` phrase bold in
  its line.
