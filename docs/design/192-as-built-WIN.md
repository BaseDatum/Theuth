# 192w — as-built WIN: picture fences render as pictures, and the model's thinking streams

Branch `feat/192w-companion-pictures` (cut from main 52475368 = 189w + the 189 card-render round), pushed to `basedatum`; not merged.
The Windows twin of design 192 (Sky, 2026-11-11 16:22: "get JSON render and mermaid diagrams … to actually render in the companion … and the
streaming thinking text, so the user is not just looking at the spinner"; his pick for diagrams: pictures from the main app, design 164's renderer).
`WIN/` = `crates/desktop/companion-windows/`. Written 2026-11-11 17:20 (+00:00) by worker `w189-win`. Code head `948fa77c`.

**In one screen.** A reply's `json-render` / `jsonrender` / `mermaid` / `openui` fence is its own block: one "drawing…" line while it still arrives or
is being drawn (never its source), the engine's PNG at content width once `fence_snapshot` answers (a click opens the reply in theuth), or — on
`no_renderer`, `failed`, `timed_out`, `cancelled`, `-32601`, a dead socket, a deadline — the fence as code with the caption "open theuth to see the
diagram". Pictures are cached by (lang, source, width, appearance, scale), two draws at a time. While the model thinks, its `ReasoningDelta` text is a
"Thinking" section (≈ 6 lines that follow the tail, secondary text) in the Ask bar and the answer card; at the first answer text or the finish it folds into
the one-line expander "Thought for N s ›" that holds all of it; no reasoning means today's spinner. Reasoning, the two-line preview and speech never show a
fence body. Replay probe on build-B with a stub renderer shows all of it; **owed:** a live draw (needs the 192 engine and a renderer client).

## 1. Engine contract, as coded
`fence_snapshot(lang, source, width, appearance, scale)` → `{outcome, png, width, height, error}` (positional params; `outcome` done · no_renderer · failed ·
timed_out · cancelled; `png` base64). `Rpc/RpcClient.192.cs:13` (`FenceSnapshotAsync`), `Rpc/Models.192.cs` (`FenceSnapshot`), `Core/StoreEngine.cs:74,144` (default
member "not supported" + `RpcStoreEngine`). `ReasoningDelta {session, turn, text}` and `ReasoningDone {session, turn, ms}` (crates/agent/src/lib.rs:3235-3254;
both reach the companion because 189w's `EventFocus` claims the desk): `Rpc/Models.cs` `StreamDelta.Ms`. An engine without the method answers `-32601`.

## 2. Decisions → what changed (file:line at the head)
**P1 segments.** `Core/PictureFence.cs:12-110`: `Lang` (first word of the info string, any case; json-render ← jsonrender), `IsOpeningStub` (a last line `` ``` `` /
`` ```mer `` while streaming is held back, so no empty code box flashes), `Flatten` (a picture fence, closed or open to the end → one line), `StripForVoice`.
`Core/AnswerCardState.cs`: `MdBlock.Picture(Lang, Source, Closed)` `:425`; fence handling `:487-503` (a picture fence is a picture block once closed, an open one while
streaming, and plain code when the reply ended with it open; other fences stay code); `MdDiff` kind `G` `:723`, signature `:748` (open ≠ closed ≠ another source, so
a fence that closes is "the block being written", updated in place, the text before it untouched).
**P2 pictures.** `Core/PictureCache.cs`: `PictureKey.Of` `:11` (width floor-clamped 240..1200, scale 1..3 rounded to 0.01, appearance dark|light); `PictureOutcome.Map` `:58`
(done + decodable PNG → Ready with its pixel size — the answer's width/height, else the IHDR's; everything else → Fallback with an outcome word and a reason for the log;
`-32601` → `unsupported`); `PictureCache` `:118` — `Request` `:181` (answers at once: Ready, Fallback or Drawing; a new key starts or queues its draw), `Watch` `:207`,
`Reset` `:218`, `MaxInFlight = 2` `:120`, a 30 s client deadline per draw `:126` (an engine that never answers still ends), failures kept too (a fence is asked for once),
`-32601` is asked once until `Reset` (the connection re-opened: `StoreModel.cs:206`), `Capacity = 48` finished results (oldest first). `StoreModel.cs:95-108`
(`Pictures`, the `FetchPictureAsync` seam). View: `Theuth.Companion/Windows/MarkdownView.Pictures.cs` — `PictureHost` `:36` (one element for the block's life), `ShowPicture`
`:59`, `ImageOf` `:138` (shown at `PixelWidth / Scale` epx, both sizes set so the layout does not move when the bitmap decodes; `PictureBitmaps` `:171` decodes each PNG once),
`FallbackOf` `:116` (code block + the caption, secondary text, underlined, clickable), `DrawingLine` `:108`; `MarkdownView.cs` builds/updates/disposes the hosts. Width = content
width in epx (Ask bar `AskBarWindow.cs:46` = 480 − 34 = 446; card `AnswerCardWindow.cs` = 440 − 28 − 14), appearance = the window's `ActualTheme`, scale = `XamlRoot.RasterizationScale`
(`RasterScale`; `UIElement.Scale` is a vector). A click: Ask bar `OpenReply` `AskBarWindow.cs:613` (as Ctrl+O), card `AnswerCardWindow.cs:645` (`OpenUrl`). Re-measure: the Ask bar's
`replyStack.SizeChanged` → `Fit()` + `FollowReplyTail` `:226,604`; the card's content `SizeChanged` → `Place()` (already there). `AnswerCardWindow.Pictures` is set in `App.xaml.cs:62`.
**P3 thinking.** `Core/ThinkingState.cs`: `ThinkingView` (`Header` "Thinking" / "Thought for N s", `Tail` = the last 3000 chars cut at a word), `ThinkingState` `:36` — `Delta` (true for
the first piece), `Done(ms)` (the engine's measure, summed over model calls), `Collapse`, `Seconds` (≥ 1; the clock when no `ReasoningDone` came), 200 000-char cap, text flattened for picture
fences as it is read. Ask bar: `AskReply.Thinking` `AskRouter.cs:220`; `StoreModel.cs` `ReasoningDelta`/`ReasoningDone` `:806-815`, folded at the first `TextDelta` `:802` and at every finish `:857`,
`ReceiptRow.Reply.Thinking` `:48`, `ShowReply` `:847` (a failed turn or "no reply" keeps its one line), `AskBarModel.ReplyView`/`ReplyHeader` `:216-220` ("theuth · thinking…"). Window:
`Windows/ThinkingSection.cs:16` (live: ≤ 6 lines × 16 epx, follows the tail unless the user scrolls up; folded: chevron + "Thought for N s", click opens ≤ 180 epx of all the text);
`AskBarWindow.cs:204-229,555,597`. Card (voice key, command mode — the one shared card): `CardExchange.Thinking`, `AnswerCardState.ReasoningDelta/ReasoningDone/Thinking` `:176-190`
(folds at text, finish, failure, stop), `PointController.cs:959-965` events, `:1349` content; `AnswerCardWindow.cs` renders the section above the body and drops the status spinner
while the text is up `:586`; the expander's open state survives the card's redraws `:140,377`.
**Hardening from design 164's rule** (every surface that shows model text): the two-line receipt/preview uses `Flatten` (`AskRouter.cs:260`: "Here: — (diagram)", never the source — the
marker is `(diagram)`, not `[diagram]`: the markdown plain pass reads a trailing `[label]` as a link being typed and drops the brackets), the reasoning text is flattened (a model that
quotes a fence in its reasoning shows "(diagram)"), and speech skips picture fences (`StoreModel.cs:869`, `PointController.cs:1005`).
**P4 logging.** `picture: fence_snapshot request mermaid (41 chars) width=446 dark x1.25 [(2 out, 0 waiting)]` · `picture: fence_snapshot done json-render in 1021 ms, 1783 bytes (558x279)` ·
`picture: fence_snapshot <outcome> <lang> in N ms: <reason>` · `picture: cache hit <lang> (<state>)` (at most every 5 s per picture) · `picture: this engine has no fence_snapshot (-32601) — fences stay code
until the connection is re-opened` · `ask: first ReasoningDelta (N chars) M ms after send` · `point: first ReasoningDelta …`.
**P5 tests** (portable Core): `tests/.../Ask192Tests.cs` — 52 facts: the segmenter (aliases, closed/open/ended, held-back openings, order, `MdDiff` in-place update), `Flatten` incl. design 164's
"replay a reasoning a few bytes at a time" check, the cache key, `PictureOutcome.Map` incl. -32601, `PictureCache` (shared draws, two in flight, kept failures, hit throttle, -32601 once until reset,
deadline on virtual time, reset, log lines, watch/dispose, capacity), `ThinkingState` and the card's thinking, the Store path (live row, fold at text/finish/seconds from `ReasoningDone` or the clock,
failed turn, watchdog kept quiet by reasoning, fence-quoting reasoning, receipt never shows a source, engine args, reset on reconnect, speech). `tests/Theuth.Companion.Rpc.Tests/RpcClient192Tests.cs` — 4 facts
(`fence_snapshot` params and result decode, `-32601`, `ReasoningDelta`/`ReasoningDone`). `StoreModelTests.cs`: the fake engine gains `fence_snapshot`.
**P6 probe.** `ask.ps1 run -AskProbe N -Replay -Pictures ok|fail|mixed|old -Shareable` (`AskFlags.Pictures`, `AskProbe.Pictures.cs`, `Store.W4.cs` `ReplayPictures`): a recorded desk stream — three
`ReasoningDelta` chunks (one quotes a json-render fence), `ReasoningDone`, then prose with a mermaid and a json-render fence cut mid-fence, `TurnFinished` — through the real Store, cache, mapping and Ask-bar
views; only the renderer is a stub (a raster "diagram" PNG drawn in the probe: BUILD → TEST → SHIP, a STATUS card; `ok` draws both, `mixed` draws json-render and answers no_renderer for mermaid, `fail` answers
no_renderer, `old` throws -32601). PNGs: `%TEMP%\192w-ask-pictures-{1-thinking,2-drawing,3-drawn,4-finished}.png` (`w153c-gate.ps1 b64 192w-<name>.png`).

## 3. Results
| Where | sha | What | Result |
|---|---|---|---|
| Mac | before the WinUI fixes (Core unchanged since) | `./test.sh -m:2` | Core 614 passed (562 + 52), Rpc 122 passed (118 + 4) |
| build-B `20261111T165651-33` | 7df7ccd3 | build (slot w192) | **failed**: CS0108 `MarkdownView.Scale` hides `UIElement.Scale`; CS0509 `Border` is sealed — fixed in 7875780e |
| build-B `20261111T165821-34` / `-40` / `-47` | 7875780e / 6df51041 / 948fa77c | build | exit 0, 0 warnings, 0 errors (33 s · 26 s · 26 s) |
| build-B `20261111T171313-58` / `-59` | 948fa77c | test Core / Rpc | **614 passed, 0 failed** / **122 passed, 0 failed** |
| build-B probes (desk lock, `--shareable`) | | mixed `…170002-35` · ok `…170447-41` · ok `…170854-48` · mixed `…171104-53` · old `…171213-56` | all exit 0 |

Evidence — `/Users/Sky.Moore/.theuth/briefs/2026-11-11/192w/evidence/`: `final-ok/` (four PNGs + `contact-sheet.png`: live "Thinking" with the quoted fence shown as "(diagram)"; folded "Thought for 3 s" with "drawing…";
both pictures drawn, the reply region following the tail; finished, the transcript's last line replacing the stream), `final-mixed/` (shots 3, 4: the mermaid fallback — code + the underlined caption — and the json-render picture),
`pass1-mixed-before-tail-fix/`, `pass2-ok-before-extent-fix/` (the follow-tail bugs below). Picture log (ok run): `request mermaid (41 chars) width=446 dark x1.25` · `request json-render (83 chars) … (2 out, 0 waiting)` · `done mermaid in 1424 ms,
1599 bytes (558x190)` · `done json-render in 1021 ms, 1783 bytes (558x279)`; old run: `this engine has no fence_snapshot (-32601) — fences stay code until the connection is re-opened`, `unsupported mermaid in 1404 ms`,
`unsupported json-render in 1006 ms`, `pictures: 2 draws, unsupported=true`. The thinking: `view … · thinking (46 → 123 → 204 chars)`, then `thought for 3 s (204 chars)`; `fence body shown: false`.

## 4. Owed and findings
- **A live draw is owed.** It needs a full build with the 192 engine and a renderer client (the main desktop app, whose off-screen renderer must answer — the Mac/engine side owes the hidden-window question). build-B's scratch engine is old and
  crash-looping (migration 37), so there `fence_snapshot` is `-32601` at best; the stub renderer proves the client's whole path, not the engine's draw. The PNG size contract (width × scale device pixels) is assumed from the brief.
- The answer card's pictures and thinking are covered by Core tests and the build only: the probe drives the Ask bar, not a point-and-say turn. The click-to-open wiring is by events (`OpenRequested`), not driven in a probe.
- Found by the probe and fixed: (1) the reply region stopped following the tail after a picture arrived — a `ViewChanged` over a grown extent was read as the user scrolling up, and `ExtentHeight` was read before the scroll viewer
  measured it; now the layout is updated first and a view change over a changed extent is ignored (same for the thinking body). (2) The caption in the accent colour was nearly unreadable on build-B's dark accent (#3408CD on #202020); it is secondary text, underlined.
- The Ask bar's reply region still caps at 320 epx: a tall picture scrolls inside it (following the tail).
- Trap for the next worker: `read_file(numbers=False)` cuts near 64 KB without an error — `PointController.cs` is 93 KB; edit it with `edit()`, never read-modify-write.
