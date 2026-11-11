# 189 as built: COMPANION-STREAM (macOS companion)

Branch `feat/189-companion-stream` from main `8f879fbd`, two commits:

1. `66676055` feat(189): one event-focus owner and transcript catch-up for the Ask and voice-key replies — D1–D3, D6, D7. Built and tested here.
2. feat(189): SwiftStreamingMarkdown renders the answer card; bundle needs Xcode — D4, D5, this doc. Not compilable here (Command Line Tools only); owed to the release Mac.

No engine change. File:line below is the tree after commit 2.

## D1 One focus owner

- `CompanionCore/FocusCoordinator.swift:8` `FocusCoordinator`: named claims `panel`, `ask`, `point`; `union` (sorted, de-duplicated); `claim` (:32), `resend` (:38), `finished` (:44), `next` (:50). One send in flight; when it lands, the union as it is then goes out if it differs (latest wins). A refused send (engine before 129) counts as sent, so nothing loops.
- `Companion/Store.swift:69` holds the coordinator. `claimFocus` (:134) is the only `set_event_focus` writer; `sendFocus` (:143) serializes; `connectionOpened` (:137) resends on every connection open (wired from `onState`, :90); `PendingPrompts` resends (:107).
- `Companion/AttentionPanel.swift:114` `syncFocus` → `claimFocus(.panel, …)`: the focused card's session while open, nothing when closed (155 §13.5 kept). `sentFocus` and its connection subscription are gone.
- `Companion/AskBar/Store+Ask.swift:89` every `commitAsk` claims `ask = [desk]` before `send_input` (was only on the first resolve; `resolveDesk`'s `defer { focusDeltas() }` and `focusDeltas` are gone). Released on a failed send (:102) and when the answer card hides with the reply finished (`askCardHidden`, :208, called from `AnswerCard.onHide`, `AnswerCard.swift:387/445`, wired at `PointController.swift:156`).
- Voice key: it depended on the same focus and `TextDelta` path (its own `AskRouter`, the desk's deltas; no focus of its own), so it got the same treatment. `PointController.deliver` (:474) claims `point = [desk]` before `steer`/`send_input`; released when the reply finishes (`renderReply`, :770), on a failed send with nothing collecting (:479), and on every reset (`resetReply`, :488 — gesture start, conceal).
- The duplicate `RpcClient.setEventFocus(_:)` in `CompanionCore/AskRpc.swift` is deleted; `TheuthKit/RpcClient.swift:249` remains.
- No other delta consumers exist (`ReasoningDelta`/`ToolCallProgress`/`VisionProgress` are read nowhere), so three claims cover it.

## D2 Catch up from the transcript

- `TheuthKit/Models.swift:494` `Loaded {history, running}`, `:502` `HistoryEntry {role, text, turn}` (other rows decode with `text`/`turn` nil). `TheuthKit/RpcClient.swift:254` `loadSession(session:limit:)` = `load_session(session, null, limit, null)`.
- `CompanionCore/AskRouter.swift`:
  - `AskReply` gains `id` (a catch-up for an earlier send never lands on a later one), `question`, `turnSeq`, `turnEnded`, `stopReason`, `deltas`, `lastEventAt`; `watchdog = 15`, `catchUpLimit = 8`, `noReply`.
  - `observe` (:80): a normal `TurnFinished` sets `turnEnded` instead of finishing (the old "ignore a finish with nothing streamed" guard is gone); `TurnStarted` clears `turnEnded` and records `turn_seq` when its text is the ask's; every desk event moves `lastEventAt`.
  - `caughtUp` (:117): `running` → keep collecting (an empty stream is filled only from our own rows). Not running → finish when a `TurnFinished` was seen, on the watchdog, or when our row and its answer are in the page (a reconnect or `Lagged` that lost the finish). The transcript text replaces the streamed text; no text anywhere → `failure = "theuth finished without a reply"` (shown as is, not "couldn't reply — …", `failureLine` :212). A failed read after a `TurnFinished` finishes on what streamed.
  - `watchdogFired` (:139): due 15 s after the last desk event or catch-up, re-arms from the fire.
  - `TranscriptPick` (:258): anchor = the user row with `turn == turnSeq`, else the latest user row whose text equals the question (whitespace aside), else none (the newest assistant row). When the anchor opens a turn only that turn's assistant rows count: the live desk interleaves peer messages and job results as turns of their own (seen in the smoke below). A steer row (`turn` nil) takes the last assistant row after it.
- Turn id: `TurnFinished.turn` is a `TurnId` that history does not carry; `TurnStarted.turn_seq` is the history's `turn`, so matching uses `TurnStarted`.
- Triggers, Ask (`Store+Ask.swift`): `TurnFinished` (:158), `Lagged` (:145), reconnect (`Store.connectionOpened`), watchdog (`armAskWatchdog`, :196). Voice key (`PointController.swift`): `TurnFinished` (:728), `Lagged` (:704), reconnect (:170), watchdog (`armWatchdog`, :789). `ReplyWatchdog` (`Store+Ask.swift:269`) keeps one timer per reply (a pending timer is kept; the handler re-arms), so a delta stream costs no timer churn.
- Probes (`AskFlags.automated`) never sent anything, so their catch-up takes the "read failed" path and finishes on the fixture stream (`--probe-speak` keeps its assertions).
- Command card: `CommandCard.finish(text:failure:)` (`CommandCard.swift:104`) takes the authoritative text when the reply finishes; the voice key calls it from `renderReply`.

## D3 Terminal stop reasons

`AskReply.terminalFailure` (`AskRouter.swift:235`): `error` "the turn failed", `cancelled` "the turn was cancelled", `suspended` "the turn was suspended", `timed_out` "the turn timed out", `interrupted` "the turn was interrupted"; any other reason is a normal finish. Terminal reasons end the reply at once as a failure (no transcript read: the result is a failure either way). `CommandCard.observe(.turnFinished)` uses the same set.

## D4 Streaming markdown renderer

- `Package.swift:24`: `SwiftStreamingMarkdown` **v0.7.0** (latest tag; `git ls-remote --tags`: v0.1.0…v0.7.0), pinned by its commit `5f7c04e0558df6146f90d482edb62cb456986bda`. `exact: "0.7.0"` does not resolve: SwiftPM refuses a version-pinned package that pins its own dependencies (highlightswift, iosMath) by revision. Product on the Companion executable only (:35). `Package.resolved` gains the transitive pins (equatable 1.4.1, swift-syntax 603.0.2, swift-markdown 0.7.3, swift-cmark 0.9.0, highlightswift and iosMath by revision, SwiftUI-Shimmer 1.5.1). KeyboardShortcuts stays at 1.10.0.
- `AnswerCard.swift:311` `AnswerMarkdown` replaces `MarkdownBlocks` (the `AttributedString(markdown:)` inline pass and the block pass, deleted) in the expanded answer body (:130). It drives `DocumentView` directly: each text change re-parses off the main actor with `MarkdownParserImpl` and `MarkdownParseOption(speculativeRewrite: true, latexMatchingRules: [])` — LaTeX off (the convenience `MarkdownView`/`StreamedMarkdownView` parse with every LaTeX rule on and cannot turn it off); a stale parse is dropped (`Task.isCancelled`).
- `config` (:330): every style spelled out with system fonts (`NSFont.preferredFont(forTextStyle:)` sizes, system and monospaced system faces) and system colours (`labelColor`, `secondaryLabelColor`, `linkColor`, `separatorColor`); code blocks `CodeBlockConfig.Theme.xcode`; citations off; the "Select more text" sheet off; images off; block spacing 8. No shimmer (the streaming shimmer of `MarkdownBlocks` is gone; the command rows' own shimmer is unchanged).
- `shouldAnimateText` stays off (the library's default). Its fade resolves the library's palette colour from `Bundle.module` on every frame of every paragraph (`ParagraphNSView.swift:286` in the library); see risk 1.
- The collapsed two-line preview and the receipt row stay plain `Text`.

## D5 Build

`scripts/bundle-companion.sh:8-12`: refuses unless `xcode-select -p` ends in `.app/Contents/Developer`, with "SwiftStreamingMarkdown needs Xcode's toolchain (#Preview macros); run on the release Mac or install Xcode". Run here: exit 1 with that message, before any build.

## D6 Logging (companion.log via `Log.write` / `AskLog.write` / `PointLog.write`)

- Every union sent: `set_event_focus([01a10df6, …])`, sessions cut to 8 characters (`Store.swift:145`); a refusal logs `set_event_focus failed`.
- First `TextDelta` of each ask: `ask: first TextDelta from …` (`Store+Ask.swift:157`), `point: first TextDelta from …` (`PointController.swift:723`).
- Each catch-up: `ask catch-up (turn finished|lagged|reconnect|watchdog): N chars, running true|false|unknown` (:169), `point catch-up (…)`; a failed read logs `load_session(…) failed` (`Store.swift:156`).
- Watchdog fires: `ask watchdog: no desk event for 15 s` (:200), `point watchdog: …`.
- Finishes: `ask finished: stop end_turn, ok|failed — ‹why›, N chars` (:189), `point finished: …` (:769).

## D7 Tests (Swift Testing, `Tests/CompanionCoreTests/StreamTests.swift`, 18 tests)

- `FocusCoordinatorTests` (5): union, latest wins, a refused send does not loop, reconnect/`PendingPrompts` resend (including one asked for mid-send), closing the panel while an ask collects keeps the desk.
- `TranscriptPickTests` (3): a `load_session` fixture with reasoning and tool rows (turn id wins over text; text match; unanchored → newest), no assistant row after the ask, a later peer turn's answer is not ours, a steer row.
- `CatchUpTests` (7): transcript replaces the stream; deltas all dropped still end in the answer; `running = true` keeps collecting and never shows another answer; no assistant text → `noReply` (also after a failed read); without a finish only our answered row or the watchdog ends it; a later `TurnStarted` undoes an earlier finish; a catch-up for an earlier send is dropped.
- `StopReasonTests` (2): the five terminal reasons (router and command card); every other reason waits for the transcript.
- `WatchdogTests` (1): injected clock — 15 s deadline, re-armed by desk events only, by the fire and by a running catch-up, off when finished or nothing sent.
- `AskTests.replyCollectsOnlyTheDeskUntilItsTurnEnds` updated: a normal finish now waits for the transcript.

Suite: 291 → 309 tests.

## Verified here vs owed to the release Mac

Here (Swift 6.3.2, Command Line Tools):

- Commit 1: `nice -n 10 swift build -j 4` → `Build complete!`; `nice -n 10 ./test.sh -j 4` → `✔ Test run with 309 tests in 51 suites passed`. Plain `swift test` cannot run on this Mac at all (`no such module 'Testing'`, any commit): `test.sh` is the package's wrapper that adds the CLT's Swift Testing framework paths.
- Live engine, read-only (a throwaway client on `TheuthKit`): `load_session(theuth desk, 8)` decodes into `Loaded`; the real page holds `user`/`reasoning`/`tool`/`assistant` rows with interleaved peer-message turns; `TranscriptPick` anchors on the last turn-opening row and returns its 600-char answer.
- Commit 2's Swift was type-checked against v0.7.0's sources with `#Preview` blocks and `@Entry` stubbed out and resources dropped (a /tmp copy; nothing of it is committed): the whole Companion target builds, no warnings in `AnswerCard.swift`. `AnswerMarkdown` hosted off-screen at the card width renders headings, bold/italic/inline code, ordered lists, a code block, a table and `\(x^2)` as text; a half-streamed `**bold` renders bold; `fittingSize` grows with the content.
- `bundle-companion.sh` refuses on the CLT.

Owed to the release Mac (Xcode): compiling commit 2 for real (macros, asset catalogs, xcstrings), `swift test`, `bundle-companion.sh`, and the live card: stream an Ask, open/close the Attention panel mid-reply, confirm the reply keeps streaming and that companion.log shows the union keeping the desk.

## Risks

1. **`Bundle.module` in a shipped app.** SwiftPM's generated accessor (see `.build/…/KeyboardShortcuts.build/DerivedSources/resource_bundle_accessor.swift`) looks only at `‹app›/‹Pkg_Target›.bundle` and the absolute build path; `bundle-companion.sh` copies dependency bundles into `Contents/Resources`, which it never searches (the 130 W6 note in the script). SwiftStreamingMarkdown reads its asset catalog through `Bundle.module` in views the config cannot replace: bullet glyphs (`UnorderedListView`), the block-quote bar, code-block and table copy icons, the fade colour; HighlightSwift loads its JS the same way. On the build Mac the absolute path exists, so it works there; on any other Mac the first list, quote, code block or table `fatalError`s. Options: build with `xcodebuild` (Xcode's accessor also searches `Bundle.main.resourceURL`), or vendor the package with a lookup that does. Not changed here: it is a packaging decision.
2. The bullet glyphs and quote bar keep the library's palette colours (`Color.Theme`); no config reaches them.
3. Toolchain: swift-syntax 603.0.2 (for `@Equatable`) needs a matching Xcode Swift; `CompanionTests` depends on `Companion`, so `swift test` builds the library too.
