# 189 as built: COMPANION-STREAM (macOS companion)

Branch `feat/189-companion-stream` from main `8f879fbd`, three commits:

1. `66676055` feat(189): one event-focus owner and transcript catch-up for the Ask and voice-key replies — D1–D3, D6, D7. Built and tested here.
2. `fa4546dc` feat(189): SwiftStreamingMarkdown renders the answer card; bundle needs Xcode — D4, D5, this doc. Not compilable here (Command Line Tools only); owed to the release Mac.
3. fix(189): the fix round F1–F4 below — xcodebuild bundle, `--probe-markdown`, the streaming fade on, the Swift 6.2 / SDK 26.0 pins.

No engine change. File:line below is the tree after commit 3.

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

- `Package.swift:24`: `SwiftStreamingMarkdown` **v0.7.0** (latest tag; `git ls-remote --tags`: v0.1.0…v0.7.0), pinned by its commit `5f7c04e0558df6146f90d482edb62cb456986bda`. `exact: "0.7.0"` does not resolve: SwiftPM refuses a version-pinned package that pins its own dependencies (highlightswift, iosMath) by revision. Product on the Companion executable only (:35). `Package.resolved` gains the transitive pins (equatable 1.4.1, swift-syntax 602.0.0 — F4, see below, swift-markdown 0.7.3, swift-cmark 0.9.0, highlightswift and iosMath by revision, SwiftUI-Shimmer 1.5.1). KeyboardShortcuts stays at 1.10.0.
- `AnswerCard.swift:312` `AnswerMarkdown` replaces `MarkdownBlocks` (the `AttributedString(markdown:)` inline pass and the block pass, deleted) in the expanded answer body (:130). It drives `DocumentView` directly: each text change re-parses off the main actor with `MarkdownParserImpl` and `MarkdownParseOption(speculativeRewrite: true, latexMatchingRules: [])` — LaTeX off (the convenience `MarkdownView`/`StreamedMarkdownView` parse with every LaTeX rule on and cannot turn it off); a stale parse is dropped (`Task.isCancelled`).
- `config(animate:)` (:338): every style spelled out with system fonts (`NSFont.preferredFont(forTextStyle:)` sizes, system and monospaced system faces) and system colours (`labelColor`, `secondaryLabelColor`, `linkColor`, `separatorColor`); code blocks `CodeBlockConfig.Theme.xcode`; citations off; the "Select more text" sheet off; images off; block spacing 8. No shimmer (the streaming shimmer of `MarkdownBlocks` is gone; the command rows' own shimmer is unchanged).
- F3: `shouldAnimateText` is on (`AnswerCard.swift:338` `config(animate:)`, `animated`/`still`), the library's fade-in of appended text; off under Reduce Motion and when the caller passes `animate: false` (:315, only `--probe-markdown`: an off-screen window drives no fade and the text would stay transparent).
- The collapsed two-line preview and the receipt row stay plain `Text`.

## D5 Build

`scripts/bundle-companion.sh:9-13`: refuses unless `xcode-select -p` ends in `.app/Contents/Developer`, with "SwiftStreamingMarkdown needs Xcode's toolchain (#Preview macros); run on the release Mac or install Xcode". Run here: exit 1 with that message, before any build.

## F1 xcodebuild

`scripts/bundle-companion.sh:23-34` builds with

```
xcodebuild -scheme Companion -configuration Release -destination 'generic/platform=macOS' -derivedDataPath "$PKG/.build/xcode" -skipMacroValidation -skipPackagePluginValidation ARCHS=arm64 ONLY_ACTIVE_ARCH=NO CODE_SIGNING_ALLOWED=NO build
```

run in `$PKG`, and takes the executable and every `*.bundle` from `$PKG/.build/xcode/Build/Products/Release` (bundles into Contents/Resources as before). It refuses if `Products/Release/PackageFrameworks` holds a dynamic framework (every package product links statically today; the script copies no frameworks). Info.plist, icons, harness assets, signing and the codesign verify are unchanged. The comment at :42-47 now says what Xcode's accessor searches (`Bundle.main.resourceURL`, i.e. Contents/Resources) and that `Bundle.module` is safe only because of xcodebuild.

## F2 `--probe-markdown`

`Companion/PointAndSay/MarkdownProbe.swift`: `Flags.probeMarkdown` (:6); `main.swift:4` runs it before the single-instance check and the app delegate (activation policy `.prohibited`), so there is no engine connection, no hotkey, no status item. It hosts `AnswerMarkdown` in an `NSHostingView` at the card width inside a borderless window that is never ordered front, runs the main run loop until the size holds for 0.5 s (10 s cap), `cacheDisplay`s it and writes `/tmp/189-probe-markdown.png`. The sample has a heading, bold/italic/inline code/a link, a bullet list, a numbered list, a block quote, fenced Swift, a table and a trailing half-streamed `**bold`. Report lines (companion.log and stderr):

```
probe-markdown: loaded ‹path›/SwiftStreamingMarkdown_SwiftStreamingMarkdown.bundle (Contents/Resources | outside the app's Contents/Resources)
probe-markdown: rendered 440×497 pt (440×497 px) → /tmp/189-probe-markdown.png
probe-markdown: ok
```

Exit 0 on success. Exit 1 with `probe-markdown: FAILED — …` when the sample did not lay out (height ≤ 200 pt) or SwiftStreamingMarkdown's bundle was never loaded. A bundle the accessor cannot find traps inside the library (exit 133, the accessor's "could not load resource bundle" on stderr). Release-Mac gate: `"$APP/Contents/MacOS/Companion" --probe-markdown` with the build tree moved away; every package bundle must report `Contents/Resources`.

## F4 Release toolchain (Xcode 26.0, Swift 6.2, macOS SDK 26.0)

- `Package.resolved`: swift-syntax pinned to 602.0.0 (`4799286537280063c85a32f09884cfbca301b1a1`), the Swift 6.2 line, instead of 603.0.2; equatable allows `600.0.0..<604.0.0`; `swift package resolve` keeps it.
- Commits 1–3 use no Swift 6.3 language or stdlib features and no API newer than macOS 15 (`Bundle.allBundles`, `NSHostingView`, `NSFont.preferredFont(forTextStyle:)`, `Color(nsColor:)`, `accessibilityReduceMotion`).

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
- `bundle-companion.sh` refuses on the CLT (`bash -n` clean; the xcodebuild step itself needs Xcode).
- Commit 3, with the same /tmp stub (now with the library's resources copied raw so its `Bundle.module` is real): the Companion target builds with swift-syntax 602.0.0; `.build/debug/Companion --probe-markdown` → both `HighlightSwift_HighlightSwift.bundle` and `SwiftStreamingMarkdown_SwiftStreamingMarkdown.bundle` loaded (from the build path: a `swift build` binary has no other place to look), `rendered 440×497 pt`, `ok`, exit 0; the PNG shows every block of the sample. With the library's bundle moved away: `Fatal error: could not load resource bundle …`, exit 133 — the crash risk 1 described, now caught by the probe.

Owed to the release Mac (Xcode 26.0): compiling commits 2–3 for real (macros, asset catalogs, xcstrings), `swift test`, `bundle-companion.sh` (the scheme name `Companion` and `Products/Release` layout are Xcode's for a package executable, unverified here), `--probe-markdown` on the bundled app with `$PKG/.build` moved away (every bundle must report `Contents/Resources`), and the live card: stream an Ask, open/close the Attention panel mid-reply, confirm the reply keeps streaming and fades in, and that companion.log shows the union keeping the desk.

## Risks

1. **`Bundle.module` in a shipped app — resolved by F1, pending the release-Mac probe.** `swift build`'s accessor looks only at `‹app›/‹Pkg_Target›.bundle` and the absolute build path, so the library's bullets, quote bar, code/table chrome, fade colour and HighlightSwift's JS would `fatalError` on any Mac without the build tree. xcodebuild's accessor also searches Contents/Resources; `--probe-markdown` with the build tree moved away proves it.
2. **Streaming fade off — resolved by F3** (on, Reduce Motion respected), pending the same proof (the fade reads the library's palette through `Bundle.module`).
3. Open: the bullet glyphs and quote bar keep the library's palette colours (`Color.Theme`); no config reaches them.
4. Toolchain: swift-syntax is pinned to 602.0.0 for Swift 6.2; `CompanionTests` depends on `Companion`, so `swift test` builds the library too.

## Card render round (`fix/189-card-render`, from main `b457a59e`)

Live on 333 (15:33): focus and catch-up worked (`ask catch-up (turn finished): 113 chars`, `ask finished: stop stop, ok`), but the card showed "Test received — I'm here and reading. — Next" and nothing more. The card was collapsed (it auto-hid 6 s after the finish; only a collapsed card does, `AnswerCardState.armsDone`), and the collapsed card is the two-line plain preview: the "—" was its join, not the H2. `--probe-markdown-stream` (commit `7a5b83d1`) replayed the reply and found two more defects in the expanded body: the panel stayed 153 pt for 217 pt of content (the markdown parses and lays out after the model change that re-measured the panel), and the `## Next` heading stayed 0 pt tall (the 40-character delta ended in `#`, an empty heading the library then updated in place).

- **F5 Ask opens expanded** — `AnswerCardState.begin` (`AnswerCardState.swift:97`) sets `expanded` for `.ask`; voice and command cards stay collapsed. Expanded cards never auto-hide (`armsDone`); ⎋ collapses, a second ⎋ dismisses, as before. The collapsed preview (`AskReply.firstTwoLines`, `AskRouter.swift:219`, also the receipt row) drops heading lines, code fences and marker-only lines, strips quote and list markers, renders inline marks away with Foundation's inline markdown parser, and puts each line on its own line: "Test received — I'm here and reading." / "Run the Hans supervision tick now".
- **F6 re-measure on rendered height** — `AnswerMarkdown.onHeight` (`AnswerCard.swift:322`, `onGeometryChange` :329) feeds `AnswerCardModel.onContentHeight` (:22), which `AnswerCard` wires to `scheduleRelayout` (:426-446): one `place()` on the next run-loop turn however many model or height changes land before it (the model's `objectWillChange` sink goes through it too). The static probe hook is gone; the probe reads `AnswerCard.contentHeight`.
- **F7 hold-back** — `MarkdownLines.holdBack` (`AskRouter.swift:263`) drops a trailing line made only of `#`, `-`, `*`, `+`, `>`, digits with `.`/`)` and spaces. `AnswerCardState.markdownText(turn:)` (`AnswerCardState.swift:105`) applies it to the last turn while `turnFinished` is false; finished turns and the finished reply pass whole (`AnswerCard.swift:133`). Deviation: a running catch-up's text is held back the same way (the view cannot tell its source); a stored transcript row is complete, so this only defers a row that genuinely ends in a marker line until the finish.
- Also: H2 was regular weight (`fonts(.headline)` with the default `.regular`), indistinguishable from body; now semibold (`AnswerCard.swift:353`).

Tests (+6, `StreamTests.swift`): `MarkdownHoldBackTests` (the `#`, `## `, `- `, `1.`, `> `, `12) `, `* + -` tails and a lone `#`; normal tails including `1`, `1.5 s`, `**bold` and the empty string pass whole; only the unfinished last turn is held back), `CollapsedPreviewTests` (the 333 reply; markers, rules and fences; no marker without its space; Ask expanded with no auto-hide, voice/command collapsed). `AskTests.replyCollectsOnlyTheDeskUntilItsTurnEnds` now expects the line-break join. 309 → 315.

Verified here: `nice -n 10 swift build -j 4 --target CompanionCore` against the real dependency graph (CompanionCore does not depend on the library) → complete. The whole package with `Package.swift` pointed at the /tmp stripped copy of v0.7.0 (throwaway scripts, nothing committed): `nice -n 10 ./test.sh -j 4` → `✔ Test run with 315 tests in 53 suites passed`; `.build/debug/Companion --probe-markdown-stream` off-screen and `--on-screen`: expanded panel h = fitting h in every frame (145/145 at the delta, 217/217 from catch-up +0.3 s to +5 s), `Next` 16 pt visible, the delta handed to the markdown as `"…reading.\n\n"`, the preview line above. Owed to the release Mac: the same probe on the Xcode build (the stub's raw-copied asset catalog draws no bullet glyphs), and a live Ask.

### Upstream issues (SwiftStreamingMarkdown v0.7.0, not filed)

1. A paragraph or heading that first lays out empty and is then updated in place stays 0 pt tall (probe: `visible …/0 pt`); created with its text it is 16 pt. Candidate cause, unproven: the size cache in `ParagraphView+macOS.swift:53-72` keyed by contents while `measureSize` reads the view's own storage, which returns `.zero` when empty (`ParagraphNSView.swift:91`). Worked around by F7.
2. The per-word fade ends at an absolute alpha of 1.0 (`ParagraphNSView.swift:289`, `withAlphaComponent`), not the colour's own alpha: freshly faded words end brighter than `labelColor` (0.85 in dark mode). Left as is.
