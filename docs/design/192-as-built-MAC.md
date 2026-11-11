# 192 as built: COMPANION-PICTURES + THINKING (macOS companion)

Branch `feat/192-companion-pictures` from main `52475368`. Coded against the engine contract of `feat/192-engine` (`fence_snapshot`); no engine change here. File:line is the tree at the commit.

## P1 Segments

`CompanionCore/Pictures.swift:40` `AnswerSegments.split(text, finished:)` → `[AnswerSegment]`: `.markdown` runs (every non-picture fence stays inside them, rendered by SwiftStreamingMarkdown), `.picture(PictureFence)` for a closed ```` ```json-render ```` / ```` ```jsonrender ```` (alias, canonicalised to `json-render`) / ```` ```mermaid ```` / ```` ```openui ```` fence, `.drawing(lang, title)` for one still open while the reply streams. CommonMark fences: ≤ 3 spaces of indent, ≥ 3 backticks or tildes, closed by a run of the same character at least as long; a fence inside another fence is its text. An unclosed picture fence of a finished reply goes back into the markdown (a code block). `jsonTitle` (:109) finds the first `"title": "…"` for the placeholder ("drawing Spend this week…"; else "drawing diagram…" / "drawing picture…", secondary colour). The collapsed preview already drops fences (189 F5).

`Companion/PointAndSay/AnswerBody.swift:8` `AnswerBody` draws the segments in order; `AnswerCard.swift:141` uses it for the expanded body (after 189 F7's hold-back).

## P2 Pictures

- `TheuthKit/RpcClient.swift:261` `fenceSnapshot(lang:source:width:appearance:scale:)` = `fence_snapshot` with positional `[lang, source, width, appearance, scale]`; `TheuthKit/Models.swift:511` `FenceSnapshotReply {outcome, png, width, height, error}` (CSS sizes decoded as numbers).
- `FenceKey` (`Pictures.swift:119`): lang, source, width (`AnswerCard.contentWidth` = 440 − 32 = 408 pt, `AnswerCard.swift:399`), appearance (`dark`/`light` from the view's `colorScheme`), scale (`displayScale`, the window's backing scale).
- `FenceCache` (:164): a key is drawn once (failures cached too), at most `maxInFlight = 2` requests, the rest queued. `FencePictureState.from` (:147): `done` with a decodable PNG and a size → `.image`; `no_renderer`, `failed`, `timed_out`, `cancelled`, `done` without a picture, RPC -32601 ("no fence_snapshot on this engine") or any other error → `.fallback`.
- `FenceSnapshots` (`AnswerBody.swift:86`, `@MainActor`, shared): runs the RPCs the cache starts through `draw` (wired in `Store.start`, `Store.swift:98`; probes stub it), decodes the PNG to an `NSImage` once (an undecodable PNG falls back).
- `FencePictureView` (:40): the image at content width with the PNG's aspect ratio; a click calls the card's Open action (`AnswerCardModel.onOpen`, wired to `AnswerCard.onOpen`, `AnswerCard.swift:440`); a fallback is the fence as a code block plus "open theuth to see the diagram" (caption, secondary); "drawing…" while requested.
- Re-measure: the whole expanded scroll content now reports its height (`AnswerCard.swift:148`), so a picture arriving, a markdown parse landing or the thinking growing all reach `scheduleRelayout` (189 F6's per-instance hook, moved up from `AnswerMarkdown`).
- `place()` (`AnswerCard.swift:534`): every frame change now goes through the animator (0 s for small changes) against the last requested frame. A plain `setFrame` during a running 0.18 s resize used to be overwritten when that animation ended; `--probe-thinking` caught the panel stuck at 160 pt for 224 pt of content after the thinking collapsed.

## P3 Thinking

- Wire (`crates/agent/src/lib.rs:3235-3254`): `ReasoningDelta {session, turn, text}`, `ReasoningDone {session, turn, ms}`. `ReasoningDelta` is a delta class the focus filter passes for focused sessions, and 189's `FocusCoordinator` claims the desk.
- `Thinking` (`AnswerCardState.swift:151`): text, start, end, summed `ReasoningDone.ms`; `live` (streamed, answer not started), `summary` "Thought for N s" (the engine's ms when given, else first reasoning → end; at least 1 s). Per turn (`Turn.thinking`, `AnswerCardState.thinking` :80), so a follow-up thinks anew.
- `AskRouter.observe` (`AskRouter.swift:84-111`): `.reasoningDelta` appends, `.reasoningDone` adds ms, the first non-empty `TextDelta`, `TurnFinished`, `Error`, the finish and a running catch-up's text end it; a `TurnStarted` before any answer text restarts it (the turn we steered into ended its thinking). Later reasoning after the answer started is kept, collapsed.
- Ask: `Store+Ask.swift:153-154` map the events (`ReasoningDone`'s `ms` is decoded on its own, `ReasoningDoneWire` `Models.swift:523`, so the shared event decoders never fail on it), `:188` copies `reply.thinking` to the card. Voice key: `PointController.swift:702-730`, `:765`.
- `ThinkingView` (`AnswerBody.swift:130`): live → "Thinking" (caption) and the text in `.callout`, secondary, in a six-line scroll view that follows the tail (`defaultScrollAnchor(.bottom, for: .sizeChanges)`); ended → a `DisclosureGroup` "Thought for N s" with the whole text. Shown in place of the spinner in both anatomies (`AnswerCard.swift:111`, `:137`); no reasoning keeps the spinner. Its own height changes (the disclosure) reach `scheduleRelayout` through `onResize`.

## P4 Logging (companion.log)

`ask: first ReasoningDelta from ‹8›` (`Store+Ask.swift:160`), `point: first ReasoningDelta from ‹8›` (`PointController.swift:728`); `fence_snapshot(‹lang›, ‹w› pt, ‹appearance›, @‹scale›x, N chars) requested`, `fence_snapshot(‹lang›): done in ‹ms› ms, ‹bytes› bytes` / `no picture in ‹ms› ms — ‹reason›`, `fence_snapshot cache hit (…)` (`AnswerBody.swift:96-125`).

## P5 Tests (`Tests/CompanionCoreTests/PicturesTests.swift`, 14)

`AnswerSegmentsTests` (4): open vs closed vs finished-unclosed; the `jsonrender` alias, case, the title while drawing; two fences (backticks and tildes) with text between; other fences untouched, a mermaid fence quoted in a markdown fence, 4-space indent, a shorter closing run, whitespace only. `FenceCacheTests` (2): the key's five parts; no second request, two in flight, the queue, failures cached. `FenceOutcomeTests` (3): done → image, the four failure outcomes with their reason, done without a picture or with bad base64, -32601 and not-connected, the wire. `ThinkingTests` (5): first text collapses with its clock duration and later reasoning stays collapsed; the engine's ms wins; no reasoning → nothing; a finish without text (and a watchdog finish) collapses; per-turn on the card. Suite 315 → 329.

## P6 Probes (release Mac)

- `Companion --probe-pictures [--on-screen]` (`PicturesProbe.swift`): a stub `fence_snapshot` (300 ms; a fixed 408 × 180 PNG for json-render, `failed` for anything else — by lang, so the two parallel requests are deterministic). Reply: a closed json-render, a closed mermaid, a streaming mermaid; frames `stream+0.2s/0.6s/1.2s`, then the third closes and the reply finishes (`finished+0.6s/1.5s`), then a second card with the same reply (`reopened+0.6s`: cache hits, no request). PNGs `/tmp/192-pictures-<tag>.png`; per frame the fence states and the card heights.
- `Companion --probe-thinking [--on-screen]`: Ask (expanded) then voice key (collapsed): spinner → 16 reasoning fragments over 2.4 s (frames `thinking+0.6s/1.5s/2.2s`) → the first text (`answer+0.4s`) → the finish (`finished+0.6s`). PNGs `/tmp/192-thinking-<ask|voice>-<tag>.png`.
- `--probe-markdown-stream` keeps working; its "markdown h" column is now "body h" (the whole expanded body).

## Verified here vs owed

Here (Swift 6.3.2, Command Line Tools): `nice -n 10 swift build -j 4 --target CompanionCore` against the real dependency graph → complete. With `Package.swift` pointed at the /tmp stripped copy of SwiftStreamingMarkdown v0.7.0 (throwaway, nothing committed): `nice -n 10 ./test.sh -j 4` → `✔ Test run with 329 tests in 57 suites passed`; the three probes off-screen, exit 0:

- pictures: `json-render: done in ~440 ms, 7515 bytes`, `mermaid: no picture … failed: stub…` (twice), three `cache hit` lines on the reopened card and no request; panel = fitting from 0.6 s on (546, 657); at 0.2 s 260 vs 262 (the markdown parse landing that frame). The PNGs show the picture, two code blocks with the caption, "drawing diagram…" while the third streams.
- thinking: live 275 → 607 → 887 chars with the tail on screen, then `Thought for 3 s`; panel = fitting in every frame (ask 149 → 204 → 234 → 224; voice 115 → 146 → 176 → 134).
- markdown-stream: unchanged from 189 (expanded 145 → 217, panel = fitting).

Off-screen the window's backing scale is 1 (`@1.0x` in the requests); on screen it is the display's. Owed to the release Mac: the Xcode build, the probes (with `--on-screen` too), and live against `feat/192-engine`: a real json-render and mermaid fence, an engine without the method (-32601 → code block + caption), a reasoning model's thinking.
