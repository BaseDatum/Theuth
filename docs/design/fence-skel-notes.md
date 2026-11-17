# fence-skel — a fence skeleton in the wrong place, and a flickering fence, while a json-render block streams

Branch `fix/fence-skeleton-misplaced`, cut from `e408cf47e`. Worker notes: reproduction, mechanisms, decisions, gates.

## Sky's words (2026-11-17 11:00)

> it seems like there is a UI bug in ~/Develop/theuth/ where when a json render begins being streamed by a model, it
> inserts a skeleton in the chat, but higher in teh chat, not where it will eventually land, so it's in the wrong place.
> then while it's still blinking in the wrong place sometimes the beginning of the json render text block will actually
> be visible. sometimes it renders partially as it's being written with the skeleton still visible, kinda glitches in
> and out of render partially sometimes.
>
> then when it's finally done, the skeleton that's in an entirely different location disappears.

Symptoms used below: **S-a** skeleton away from where the content lands; **S-b** raw json-render text visible; **S-c**
partial render glitching in and out with the skeleton still up; **S-d** the skeleton disappearing when the turn is done.

## Reproduction (base code = control)

`crates/desktop/ui/src/dev/fence-stream-fixture.{html,tsx}` (committed): the real store reducer (`applyEvents` → `route`
→ `apply`) fed scripted `AgentEvent`s, and one real `Message` per entry, so `MessageAssistant`, streamdown 2.6.0,
`JsonRenderFence`, `FenceFallback`, `MermaidFence` and `OpenUIFence` run as in the app. Deltas of 15-40 characters every
30 ms; bodies from `crates/surface/fixtures/json-render/` (`valid-metrics`, `broken-unknown-type`). The settled reload is
`adopt(old, fromHistoryPage(rows))`, as `loadHistory` does. An in-page rAF sampler records every frame's skeletons
(`[aria-busy]`, `.animate-pulse`), streamdown code cards, Suspense fallbacks, surfaces, dim lines and raw fence text
with row and box. Not the virtualised `Transcript` (rows are a plain column): Transcript.tsx is off-limits and the bug
does not need it. Measurement harness, timelines and screenshots live in `/tmp/fence-skel/` (summary: `proof.md`).
Chrome for Testing 1226 and Playwright WebKit 2370 both ran; WebKit reproduces every decisive scene identically.

Engine events between reply A and reply B were read from `crates/agent/src/loop_.rs`: at a round end the engine
publishes `ModelUsageChanged` (`settle`, from `call`) and `ContextUsed` (6744); the correction note is `persist`ed
with no event. Neither event closes the reducer's open tail: `ContextUsed` returns early in `apply`'s outer switch
(`store/reduce.ts:1004`), `ModelUsageChanged` falls to `turn()`'s `default` → `undefined`, which discards the
`closeTail` result (1050). The salvage round (max_tokens) publishes exactly the same pair.

| scene | S-a | S-b | S-c | S-d | evidence (base timeline, ms) |
|---|---|---|---|---|---|
| s1 single reply, valid fence | – | – | – | – | surface 1514-2960 continuous |
| s2 old broken fence + new turn | yes | – | yes | yes | old reply row 1: dim line 58-748, **skeleton 748-2326** (TurnStarted→TurnFinished) while the new surface streams in row 3 |
| s3-prose-none (glued) | yes | yes | yes | yes | A partial 308-515 → skeleton 515-1893; raw `All green, ready to land.```json-render { "root": "r` 739-2218; dim line at TurnFinished 1893, gone at settle 2218 |
| s3-prose-engine (`ModelUsageChanged`+`ContextUsed`) | yes | yes | yes | yes | same as prose-none: the real base engine glues |
| s3-nl-none (A ends "```\n") | yes | – | yes | yes | skeleton 521-1474 in row 1 with B's surface below it in the same row; B blinks out 1231-1273 |
| s3-bare-none (A ends "```") | yes | – | yes | yes | "``````json-render" is no closer: A's fence swallows B; skeleton 526-1264, then nothing until TurnFinished |
| s3-prose-reason | yes | – | yes | – | skeleton 515-1232 until B's first delta; B (row 3) blinks out 1654-1698 |
| s3-prose-tool (Sky's DB shape) | yes | – | yes | – | skeleton above the tool card 530-2696; B blinks out 3129-3170 |
| s4 split opener | – | yes | yes | – | code card `js` 271-384, `json-ren` + raw 384-480; surface blinks out 814-900 |
| s5 mermaid/openui | yes (in place) | – | yes | yes | skeletons for the broken mermaid 2028-2905 and openui 2658-2905 beside live diagrams |

Screenshots: `/tmp/fence-skel/shots/s2-base-skeleton-and-stream.png`, `s3-prose-none-base-raw.png` (Sky's picture
exactly: the skeleton above, `…land.```json-render { "root": "r` as text below), `s3-nl-none-base-skeleton-and-stream.png`,
`s4-base-opener.png`, `s5-base-skeleton-and-stream.png`, each with a `-webkit` twin where run there.

## Mechanisms (base lines)

- **M1 + M2 — the correction round, glued.** `loop_.rs:6776-6833`: A is persisted, the `<surface_error>` note
  persisted without an event, `Round::Tools` streams B. With no tail-closer between (above), `TextDelta`
  (`reduce.ts:1121-1142`) appends B to A's open entry. A's closed broken fence becomes `FenceFallback`'s skeleton
  (`JsonRenderFence.tsx:36`, `running` true) at A's place while B streams below it, raw (mid-line opener) or as a second
  fence in the same entry; at `TurnFinished` the skeleton turns into the dim line, and the settled reload (A, hidden
  note, B as separate rows: A gets `hasLaterAssistant`) removes it. This is S-a…S-d in one scene. When the model thinks
  or calls a tool first (most of the 16 rounds in Sky's DB), B is its own entry and the skeleton sits above, for the
  length of the tool call, until B's first delta.
- **M3 — session-wide `running`.** `MessageAssistant.tsx:326, 338-341`: every broken fence in an earlier turn's last
  reply pulses during every later turn and flips back at its end (s2).
- **M4 — partial info string.** streamdown sees "```js", "```json-ren": a plain code card with a growing language (s4).
- **M5 — blink-outs.** The open fence renders `null` on a prefix `parsePartial` rejects (`JsonRenderFence.tsx:89`): B
  vanished for one delta in every scene with a long enough body (s3-*, s4). The sticky `'open'` `Boundary` (94) did not
  fire in any scene: json-render 0.20 wraps each element in `ElementErrorBoundary`, which renders `null` for that
  element (and logs) and catches catalog throws before our boundary sees them.
- No Suspense fallback (`Xe`) appeared in any scene: nothing in the fence renderers suspends.

## What changed (D1-D5)

- **D1** `FenceFallback` (`surface/JsonRenderFence.tsx`): `if (running || hasLaterAssistant) return null`, else the
  unchanged dim line; the doc comment says why. Mermaid (`ErrorCard`) and openui use the same component, so no change
  there. `FenceFallback.test.ts` asserts nothing in the three hidden states and no `animate-pulse`/`aria-busy` anywhere.
- **D2** `surface/fence-turn.ts`: `ownTurnRunning(entries, id, running, turnSeq)` (entry's `turn === turnSeq`; unknown
  turn → only after the last `user` entry with a `turn`, so steers do not count as openers) and `hasLaterAssistant`
  (moved out of `MessageAssistant` unchanged). `MessageAssistant` feeds both into `FenceContext` through two boolean
  selectors. `FenceContext.running` keeps its name (other readers construct it outside this brief's files) with the new
  meaning documented. `turnRunning` for cells (348) is untouched.
- **D3 — option 2, engine.** Option 1 is impossible: the only events that reach the UI between the two model calls,
  `ModelUsageChanged` and `ContextUsed`, are identical in the salvage round, so closing on them breaks salvage (and
  neither is a tail-closer today, deliberately for `ContextUsed`: `reduce.test.ts` "ContextUsed moves the gauge mid-turn
  without ending the turn"). The correction branch (`loop_.rs`, `if corrects`, surface AND cell rounds) now publishes
  the existing `AgentEvent::InputInjected { id: InputId::new(), turn }` after persisting the note(s). Why this variant:
  it is literally true ("a … input entered the context as a user message"), every frontend already treats it as
  transcript progress (desktop `turn()` → `closeTail`; TUI `agents.rs:303` progress list), and a fresh id matches no
  queued steer, so the desktop (`reduce.ts:1106` returns early, `lastInjected` untouched), TUI, meeting and Composer do
  nothing else. `ReasoningDone { ms: 0 }` was the other candidate; rejected: it claims thinking that did not happen.
  The salvage path is unchanged and still appends (tests below). No wire type, no bindings change.
- **D4** `surface/streaming.ts`: `holdFenceOpener(text)` drops a last line that may still become a fence opener (bare
  "`"/"``"/"~~", or "```info"/"~~~info" without its newline); `MessageAssistant` hands `open ? holdFenceOpener(text) :
  text` to `Streamdown` (all fences; a line delayed by one delta). `JsonRenderFence` keeps the last parsed partial spec
  (`openSpec(code, kept)` = `parsePartial(code) ?? kept`, `kept` in state, read but not a memo dependency so a newly kept
  spec never re-parses). `Boundary` now takes `open`/`retry`: on an open-fence throw it keeps the children of its last
  error-free commit and retries when the spec changes; the closed fence keeps the old fallback and the `'open'`→`'closed'`
  key flip. No per-delta key.
- **D5** none needed beyond D1-D4: every mechanism the harness showed is covered. The fixture's `&engine=fix` models the
  engine's new `InputInjected` for the after-proof.

## What I did not change, and why

- Transcript.tsx, scrolling, the virtualiser: not involved in any reproduced symptom.
- json-render's `ElementErrorBoundary`: library code; an element that throws mid-stream stays blank until the fence
  closes (the existing key flip remounts it). Not observed in any scene.
- `MermaidFence`'s own "last svg that rendered" (a broken chart whose prefixes parse keeps an empty or partial frame,
  title = error): its design, not `FenceFallback`; s5 uses a chart broken from its first character to exercise D1.
- A broken fence's partial preview now disappears when the fence closes broken (D1: nothing while the turn runs); the
  correction appears in the next row. A desktop on this branch against an older engine still glues B onto A live
  (`fixui-oldengine` runs) — D3's engine half is required.
- `hasLaterAssistant` stops at any `user` entry, including a mid-turn steer: pre-existing, unchanged.

## Tests added

- `surface/fence-turn.test.ts`: the D2 table (own turn running, later reply, earlier turn while a later runs, unknown
  turn in and out of the live tail, unknown running turn, settled, missing entry).
- `surface/FenceFallback.test.ts`: rewritten for D1.
- `surface/streaming.test.ts`: every prefix of prose + json-render + mermaid + prose never ends in a partial opener;
  the S4 split; inline-code/strikethrough lines are not held; every prefix of four real fixture bodies never falls back
  to `null` after the first spec.
- `store/reduce.test.ts`: correction round (round end + `InputInjected` with an unknown id) → two assistant entries;
  salvage (round end only) → one appended entry.
- `crates/agent/src/loop_/fence_round_tests.rs`: a real `Turn` against a fake provider: correction round →
  `TextDelta(A), InputInjected, TextDelta(B)`; salvage → `TextDelta(cut), TextDelta(rest)` with nothing between.

## Gates

- `bunx tsc -b --noEmit` (build-lock): 0 `error TS` (`/tmp/fence-skel/tsc.txt`). No baseline diff needed.
- `bunx vitest run src/components/chat src/store src/lib/no-emoji.test.ts`: 92 files, 908 tests passed (`vitest-chat.txt`).
- Full `bunx vitest run` (BUILD_SLOTS=1): 359 files, 2524 tests passed (`vitest-full.txt`).
- `mbx check -p agent`: ok, 0 errors (`check-agent.txt`). `mbx clippy -p agent --no-deps --tests -- -D warnings -A
  clippy::chunks_exact_to_as_chunks -A deprecated`: ok, 0 warnings in agent (only cargo's future-incompat note for the
  `block` dependency) (`clippy-agent.txt`). `mbx test -p agent --no-run`: ok. `mbx test -p agent fence_round`: 1 passed;
  `mbx test -p agent correction_rounds`: 1 passed.
