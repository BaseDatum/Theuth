# 154 ASK-GROW — as built (2026-10-09, branch `fix/154-ask-grow` off main 6e395770)

## The ask (Sky, 2026-10-09, verbatim)

> Small UI bug in the desktop slash mobile app for the ASC user tool if the answer that I give in the custom answer box is longer than the width of the box. The box doesn't get longer to show me the entire answer that I input

## Mechanism

`QuestionBlock`'s `Type your own…` box (`AskCard.tsx` `OwnAnswer`) was the single-line `Input` (`h-8`): an `<input>` never wraps, so a long answer scrolled sideways and its start disappeared (base S3 at 1280: `scrollWidth` 1704 in a 1109 px box; at 390: 1922 in 207). The answered/locked state was the same one-line read-only `Input`, so a sent answer was cut off too. `QuestionBlock` is shared with the Attention `AskScreen`, so one fix covers both.

## D1–D9 as built

| | Decision | Where |
|---|---|---|
| D1 | `OwnAnswer` renders `Textarea` (`oneLine`, `rows={1}`, dictation mic kept); grows by `field-sizing: content`, no JS; `[overflow-wrap:anywhere]` wraps URLs; `resize-none` | `AskCard.tsx:47-66`, `textarea.tsx:13` |
| D2 | One line = 32 px at both sizes: `min-h-8 py-[3px] md:py-[5px]` (20 px line + 10 + 2 border at ≥ md; 24 + 6 + 2 below). Computed style confirmed (`cn` drops `min-h-16`/`py-2`): paddingTop 5 px / 3 px, minHeight 32 px. Mic centred at the trailing edge (`top-1/2 right-0.5 -translate-y-1/2`, Input's placement) in every state — bottom-right would sit 10 px high on a 44 px coarse-pointer button in a 32 px box | `textarea.tsx:13,46` |
| D3 | `max-h-[200px] pointer-coarse:max-h-[160px]`, then scrolls inside (S4: 200 / 160) | `textarea.tsx:13` |
| D4 | Textarea default: Enter / Shift+Enter insert a newline; nothing sends; `toAnswer`, `ownPick`, `ask-drafts.ts` untouched | — |
| D5 | Row `items-start`; on coarse pointers the box and Skip carry `pointer-coarse:mt-1.5` so the one-line row is pixel-identical to before (○ 185..229, box 191..223, Skip 191..223 at 390, same as base) and the first line stays level when the box grows | `AskCard.tsx:62,155-177` |
| D6 | Locked: a `<div>` with the box look (`min-h-8 rounded-md border border-input px-3 py-[3px] md:py-[5px] text-base md:text-sm shadow-xs dark:bg-input/30`), `whitespace-pre-wrap break-words`, no cap, selectable; typed → `border-fg-accent`; skipped → "Skipped" in `text-muted-foreground`. S6 shows all of it (height 52 / 176, scrollHeight == clientHeight); S7 is 32 px | `AskCard.tsx:143-154` |
| D7 | `box` and `OwnAnswer`'s `ref` are `HTMLTextAreaElement`; focus re-pick, ○ on a blank box, kept/dimmed words unchanged | `AskCard.tsx:52,97` |
| D8 | `textarea.tsx` gained ONE optional prop, `oneLine` (default `false` → every existing caller's class strings and mic placement unchanged: `cn(FIELD, false, …)` yields the same string). Needed because `splitLayout` moves `min-h-*`/`max-h-*` onto the wrapper (so a caller cannot cap or shrink the field) and the mic's class is not reachable from a caller. `Bundle.tsx` and `DecisionScreen.tsx` still have the one-line `Input` inside a `<form>` where Enter submits — left for Sky; `oneLine` would fit them once that is decided. Composer, `input.tsx`, native companions untouched | `textarea.tsx:8-25` |
| D9 | No fallback: WebKit < 26.2 and Firefox keep one line (comment at `textarea.tsx:9-12`, `AskCard.tsx:48-50`) | — |

Files changed: `crates/desktop/ui/src/components/chat/AskCard.tsx`, `crates/desktop/ui/src/components/ui/textarea.tsx`, this file.

## Visual proof (headless Chrome 2x over CDP, real `QuestionBlock`, `/tmp/154/harness`, not committed)

Box = textarea (S1–S5, S8) or locked block (S6, S7, S8L); card = the `FRAME` div. Light and dark gave identical numbers; one row each. S2 is a 17-character one-liner (a 40-character one wraps to 3 lines at 390). S3b (130 chars) is extra: at 390 a 300-character answer exceeds the 160 px cap by design (D3), so S3b is the under-cap "grows and shows all" case there; it still caps (198 > 158) — six 24 px lines fit under 160 px, S3b needs eight.

| state | vp | height | scrollH | clientH | scrollW | clientW | card sW/cW | pass |
|---|---|---|---|---|---|---|---|---|
| S1 / S2 | 1280 | 32 (base 32) | 30 | 30 | 1109 | 1109 | 1246/1246 | ±1 ✓ |
| S3 | 1280 | 52 | 50 | 50 | 1109 | 1109 | 1246/1246 | > 32, all visible ✓ |
| S4 | 1280 | 200 | 210 | 198 | 1109 | 1109 | 1246/1246 | cap, scrolls ✓ |
| S5 | 1280 | 52 | 50 | 50 | 1109 | 1109 | 1246/1246 | dimmed, ○ ✓ |
| S6 | 1280 | 52 | 50 | 50 | 1212 | 1212 | 1246/1246 | whole answer ✓ |
| S7 | 1280 | 32 | 30 | 30 | 1212 | 1212 | 1246/1246 | ✓ |
| S8 | 1280 | 32 | 30 | 30 | 1109 | 1109 | 1246/1246 | no sideways overflow ✓ |
| S1 / S2 | 390 | 32 (base 32) | 30 | 30 | 207 | 207 | 356/356 | ±2 ✓ |
| S3 | 390 | 160 | 366 | 158 | 207 | 207 | 356/356 | cap (300 chars > 6 lines) |
| S3b | 390 | 160 | 198 | 158 | 207 | 207 | 356/356 | cap |
| S4 | 390 | 160 | 2022 | 158 | 207 | 207 | 356/356 | cap, scrolls ✓ |
| S6 | 390 | 176 | 174 | 174 | 322 | 322 | 356/356 | whole answer ✓ |
| S7 | 390 | 32 | 30 | 30 | 322 | 322 | 356/356 | ✓ |
| S8 | 390 | 160 | 174 | 158 | 207 | 207 | 356/356 | no sideways overflow ✓ (base S3: 1922 > 207) |
| S8L | 390 | 104 | 102 | 102 | 322 | 322 | 356/356 | URL wraps in the locked block ✓ |

Mic: 1280 one line 169..193 in box 165..197 (base identical); grown S3 179..203 in 165..217, S4 253..277 in 165..365; 390 185..229 in 191..223 (base identical). ○/Skip 165..197 at 1280 in every state; by eye level with the first line (S3/S4), mic never over text.
Screenshots in `/tmp/154/shots/`: `after-{S1,S2,S3,S3b,S4,S5,S6,S7,S8,S8L}-{1280,390}-{light,dark}.png` (40) and `base-{S1,S2,S3,S6}-{1280,390}-{light,dark}.png` (16).

## Gates

- tsc: `/tmp/154/tsc-after.txt` has 1 `error TS` line, the same `autoresearch/fixtures.ts(344,9)` stale-binding line as `tsc-before.txt`; `comm -13` prints nothing; nothing mentions AskCard.tsx or textarea.tsx.
- vitest `src/components/chat src/components/attention src/components/ui`: 61 files, 555 tests passed.

## Not verified

Real WKWebView (macOS, iPad, iPhone): `field-sizing` needs Safari/WebKit ≥ 26.2; below that the box stays one line (D9). Headless Chrome stood in for both pointer types.
