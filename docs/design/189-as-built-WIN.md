# 189w — as-built WIN: the Ask reply streams, catches up from the transcript and renders as markdown

Branch `feat/189w-companion-stream` (cut from main 8f879fbd), pushed to `basedatum`; not merged. The Windows twin of design 189
(the macOS incident: the Ask reply waited forever while the main window showed it). `WIN/` = `crates/desktop/companion-windows/`.
Written 2026-11-11 15:20 (+00:00) by worker `w189-win`. Code head `25a37ae6` (after it only `w189-gate.ps1` and this doc changed).
Decisions D1–D7 were made by the orchestrator; this file says what was built, where, and how it was proven.

**In one screen.** One class, `EventFocus`, is now the only caller of `set_event_focus`: it sends the union of every claim
(the Ask's desk, the voice key's desk), serialized, forced, and re-sent on every ask, every `PendingPrompts` and every reconnect.
While an ask collects, the Store reads the desk's last 8 transcript entries (`load_session`) after a `TurnFinished`, a `Lagged`, a
reconnect and 15 s of silence; the transcript text replaces what streamed, a finished turn can no longer stay "waiting" (its
text, its error row, the streamed text, or "theuth finished without a reply"), and `suspended` is terminal like the other four.
The reply is drawn as markdown that grows block by block under the Ask bar field (and in the answer card's body): the library check
found nothing that fits, so the repo's own pure parser `AnswerMarkdown` got a streaming mode and a block diff, and a small
`MarkdownView` of stock Fluent controls touches only the block being written. Replay probe on build-B: four streamed chunks and the finish; at the finish 8 of 9 blocks were untouched and the transcript's final line replaced the stream. Against a real (model-less) engine: desk, focus
claim per ask, `send_input`, `TurnStarted.turn_seq`, the `Error` event and the `load_session` decode all as designed. **Owed:** a
live model reply on build-B (no provider there).

## 0. The brief's Windows facts, verified on main 8f879fbd

| Claim | Result |
|---|---|
| text-less normal finish ignored | true: `Core/AskRouter.cs:135` `if (r.Text.Length == 0) return false;` — the receipt then waits forever |
| `suspended` not terminal; `timed_out`, `interrupted` are | true: `AskRouter.cs:145` `IsFailure` = error · cancelled · timed_out · interrupted |
| one `set_event_focus` writer | true: one call, `StoreModel.cs:689`, reached from `ResolveDesk` (`:645`, `finally` → `FocusDeltas()` `:674`) and from every `PendingPrompts` (`:705-707`, the first item of each (re)subscription), so nothing on Windows could overwrite the set. **But** `ResolveDesk` returns the cached desk before its `try` (`:647`): a later ask sent no claim of its own, and the point-and-say desk (`PointController`, its own `desk_open`) claimed nothing at all (a desk other than the Ask's was outside the set) |
| "ask failed on the desk" logged | true: `StoreModel.cs:729` |

## 1. Decisions → what changed (file:line at the head)

### D1 — one focus writer (`WIN/src/Theuth.Companion.Core/EventFocus.cs:5-147`)
- `EventFocus` keeps claims by owner (`Claim` `:52`, `Release` `:55`), sends `Union` (sorted, distinct) — never `[]` (`:105-108`: a release or an empty union sends nothing; the engine drops every delta for an empty set, api.rs:10472-10481).
- `SendAsync` (`:68`) is forced (a connection may have lost the set), serialized and coalescing: one send in flight, requests that arrive meanwhile ride the next send, which reads the claims at its own start, so the last claim wins (`Pump` `:95-132`). A refused send is logged, never thrown (`:118-121`).
- `EnsureSinceAsync(mark)` (`:86`): "sent for this ask" — waits for a send requested after the ask's start (the desk's first resolve counts), else makes one.
- Wiring: `StoreModel.cs:93` (the engine call is `focus`'s only sink), `:106` `ClaimDeltasAsync(owner, session, reason)`, `:661-668` `CommitAsk` claims `ask` and awaits `EnsureSinceAsync(mark)` **before** `send_input`, so no delta of the reply can meet a stale set; `:735`/`:743-747` resolve; `:762` every `PendingPrompts`; `:192` every reconnect (`reconnect`).
- Second claimant: `Theuth.Companion/PointAndSay/PointController.cs:696` — the voice key's / command mode's desk joins the union (it used to claim nothing).
- `Core/StoreEngine.cs:68,134` (`LoadSessionAsync`: the interface's default member and `RpcStoreEngine`).

### D2 — catch up from the transcript (`Core/AskCatchUp.cs`, `Core/StoreModel.cs:693-950`)
- RPC: `Rpc/RpcClient.189.cs:13` `LoadSessionAsync(session, limit)` = `load_session(session, null, limit)`; `Rpc/Models.189.cs:10-34` `TranscriptEntry{Role,Text,Turn}`, `SessionTail{History,Running}`; `Rpc/Models.cs:606-608` `StreamDelta.TurnSeq` (`TurnStarted.turn_seq`, kept as the ask's turn id: `StoreModel.cs:774-779`).
- Triggers (`AskCatchUp.cs:7-20`): `turn-finished` (`StoreModel.cs:793-803`), `lagged` (`:230-232`), `reconnect` (`:192-193`), `watchdog` (`:851-870`). 8 entries (`AskCatchUp.Limit` `:58`); `load_session` has a 10 s deadline (`:61`, `LoadTailWithin` `StoreModel.cs:941`) — an engine that never answers counts as "unreadable" and the ask still ends. One catch-up at a time; a trigger that arrives meanwhile runs once after it, a finish outranks the others (`:878-886`).
- `AskCatchUp.Pick` (`:95-173`): by turn id when known, else the latest user row with the ask's text (whitespace-insensitive; its seq becomes the turn id), else the newest assistant entry; the reply is the last assistant entry of that turn (steer rows without a seq belong to it; a later turn's reply never does); an `error` row after the user row is reported when no text is stored. `Decide` (`:183-201`): running → keep collecting; text → **finish with the transcript's text (replaces the streamed text**, `StoreModel.cs:911-913`); error row → fail; nothing stored but streamed text → that; else `NoReply`. One 1.5 s re-check when the transcript has no trace of the ask yet and nothing streamed (`Settle`, `:197-198`).
- Never waiting: `AskReply.FinishWithoutReply` (`AskRouter.cs:234`) → the receipt says `theuth finished without a reply` (`AskCatchUp.NoReplyText` `:67`, `AskRouter.cs:255`) and keeps Open; the row lingers 12 s (+1 s per four words past 20, max 60: `LingerSeconds` `:207`).
- Watchdog `AskWatchdog` (`:225-260`, injected clock): armed at send (`StoreModel.cs:700-701`), touched by every event for the desk (`:771`), fires after 15 s of silence, then re-arms after each catch-up while the turn runs (`:851-870`).
- Spoken replies get the transcript's final text, never a partial (`StoreModel.cs:838-839`).

### D3 — terminal reasons (`Core/AskRouter.cs:102-165`)
`IsFailure` (`:154`): error · cancelled · **suspended** · timed_out · interrupted → `AskReply.Failed` with a readable reason (`FailureReason` `:157-164`: "the turn was suspended — theuth is restarting" etc.); any other stop reason is a normal finish and goes through the catch-up. A failure `TurnFinished` ends the ask at once, with no catch-up (`StoreModel.cs:794-798`); so does an `Error` event (`:790-791`).

### D4 — streaming markdown (surface: the Ask bar's receipt row; also the answer card's body)
- Surface: the Ask bar's reply row. `AskBarModel.ReplyMarkdown` / `ReplyHeader` (`Core/AskBarModel.cs:203-213`), `ReceiptRow.Reply(…, Markdown, Streaming)` (`StoreModel.cs:47`, `ShowReply` `:813-820`), `AskBarWindow.cs:197-216` (`MarkdownView` in a `ScrollViewer` ≤ 320 epx that follows the tail until the user scrolls up), `:520-529` (render + the `kept/updated/built` log line), `:437` retheme, `:566` reset. The header reads `theuth · replying…` while streaming, `theuth` when final; the trailing hint stays `Open Ctrl+O`. Failed turns and "no reply" keep the one-line row.
- Answer card: `Windows/AnswerCardWindow.cs:99` `Streaming`, `:610-620` the expanded body is the same `MarkdownView` (replaces a 66-line builder), `PointAndSay/PointController.cs:1335` passes `Streaming` while the answer is still arriving.
- Parser (Core, Mac-tested): `Core/AnswerCardState.cs:421-666` `AnswerMarkdown.Parse(markdown, streaming)` — headings, nested bullets, numbered items, fenced code, table rows, `**bold**`, `*italic*`, `` `code` ``, `[links](url)`; with `streaming: true` the last block's unclosed marks render as closed (`:511`), a last line that is only a block-marker start (`##`, `-`, `1.`, one or two backticks) is held back (`:467`), an unclosed fence is code so far (`:459`). `MdDiff` (`:673-731`): block signatures; the common prefix stays, the first differing block of the same kind is updated in place.
- View: `Theuth.Companion/Windows/MarkdownView.cs:21-243` — stock `TextBlock`/`Run`/`Hyperlink`/`Border`, theme brushes through `Theme.Brush` (no custom theme); `Set` (`:64`) diffs and returns `(Kept, Updated, Built)`; a chunk that only grows the last run grows that run's text (`TryGrowLast` `:225`). Links use the accent-text brush for the window's theme (`:203`).

### D4 — the library check (2026-11-11; NuGet + the toolkit's sources)
| Candidate | Facts | Verdict |
|---|---|---|
| `CommunityToolkit.Labs.WinUI.Controls.MarkdownTextBlock` 0.1.251217-build.2433 (2025-12-17; Labs, prerelease only: 0.1.251021 / 0.1.251030 / 0.1.251217) | MIT; depends on `Microsoft.WindowsAppSDK` **1.6.250108002** (the meta package), Markdig 0.38.0, HtmlAgilityPack 1.11.71, Roman-Numerals 2.0.1; TFM groups net8.0 / net9.0-windows10.0.19041 (none for net10). `MarkdownTextBlock.xaml.cs` (Labs-Windows main): `OnTextChanged` → `ApplyText(true)` → `_renderer.ReloadDocument()` + `Markdown.Parse(Text)` + `_renderer.Render(...)` | **rejected**: not stable; the meta package collides with this app's split, self-contained packages (`Theuth.Companion.csproj:10,17,19-20,32-35`: net10.0-windows10.0.22621.0, unpackaged, `Microsoft.WindowsAppSDK.WinUI/.Foundation/.Base/.InteractiveExperiences`); and it rebuilds the whole document on every Text change — exactly the full re-layout per chunk the criteria forbid |
| `CommunityToolkit.WinUI.UI.Controls.Markdown` 7.1.2 (2021-11-18, last stable) | MIT; TFM net5.0-windows10.0.18362; depends on `Microsoft.WindowsAppSDK` 1.0.0 and ColorCode.WinUI; no release since 2021 | **rejected**: unmaintained, WinAppSDK 1.0 |
| own: `AnswerMarkdown` + `MdDiff` + `MarkdownView` | the parser shipped with the answer card (153 W153C) and is covered on the Mac; stock Fluent controls; only the block being written is touched | **chosen** |

Criteria: maintained (ours, tested) · permissive licence (none added) · compatible with the app's WinAppSDK split and TFM (no new package) · appends without a full re-layout (block diff; numbers in §4) · native Fluent look, no custom theme (stock controls, `Theme.Brush` keys).

### D6 — logging (`Core/StoreModel.cs`; `ask-probe.log` and `companion-<date>.log`)
`set_event_focus([<id>…]) ok (<reason>)` / `failed` / `skipped` (`EventFocus.cs:107,116,120`; reasons `resolve` · `ask` · `pending-prompts` · `reconnect` · `point-send`) · `ask sent to desk <id> (N chars)` (`:677`) · `ask: the desk started turn_seq=N` (`:778`) · `ask: first TextDelta (N chars) M ms after send` (`:785`) · `ask: TurnFinished (<stop>) for the desk after N streamed chars — catching up from the transcript` (`:801`) · `ask catch-up (<trigger>[, re-check]): N entries, M chars from the transcript (K streamed), running=<bool>, via=<turn|text|newest|stream|error-row|empty|none> → <Finish|Fail|KeepCollecting|Settle>` (`:905-906`) · `ask watchdog: no event for the desk in 15 s — catching up from the transcript` (`:863`) · `ask failed on the desk: …` (`:791,797,917`) · `ask finished (<how>): N chars, K streamed, ok|failed|no reply` (`:836`). Probe-only: `ask-replay markdown: N chars → B blocks: kept …, updated …, built …`, `ask-catchup: …`.

### D7 — tests (Core test project, run on the Mac and on build-B)
`tests/Theuth.Companion.Core.Tests/Ask189Tests.cs` — 62 facts: the picker against history fixtures (turn / text / newest, steer rows, later turns, blank text, error rows, whitespace), the decision table (running, text, error, unreadable, settle, no-reply), terminal reasons and the store ending a `suspended` finish at once, the watchdog on an injected clock (15 s, restart on any event, re-arm), `EventFocus` (sorted union, a second claimant never erases the first, a release never sends `[]`, forced sends, serialized latest-wins, refused send logged, `EnsureSince`), the Store (focus once per ask before the input, again on every later ask / `PendingPrompts` / reconnect, the voice desk joins the union; catch-up fills a text-less finish, replaces the streamed text, keeps collecting while running, ends "without a reply", falls back to the streamed text, 10 s `load_session` deadline, 1.5 s settle, `Lagged` and reconnect triggers, watchdog through the Store, a failed event decode still ends the ask, `turn_seq` becomes the turn id, the log lines, the linger time, spoken reply = transcript text), the Ask bar/receipt markdown rules, the streaming parser (unclosed marks, `_` inside words, held-back marker lines, nested bullets/tables/unclosed fences, every old block kind) and `MdDiff` (a chunk changes only the block being written). `tests/Theuth.Companion.Rpc.Tests/RpcClient189Tests.cs` — 4 facts (`load_session` params and decode, `turn_seq`). `StoreModelTests.cs` +12 lines (fake `load_session`; the receipt row now carries the markdown).
Totals: **Core 562 / 0 failed, Rpc 118 / 0 failed** (the 66 new facts included; no `Theory` rows in the new files).

## 2. Build and test results

| Where | sha | What | Result |
|---|---|---|---|
| Mac | 82d65bb6 (code = 25a37ae6) | `./test.sh -m:2` (one dotnet run) | Core 562 passed, Rpc 118 passed |
| build-B `20261111T145038-1` | ba912de7 | build | **failed**, CS0111 — my second `CapturePng` in `AskBarWindow.cs` next to 153's; fixed in c9b57cc5 (one `CapturePng`, the 153 one now paints the solid base behind the tree) |
| build-B `20261111T145226-2` | c9b57cc5 | build Debug x64, kill `Theuth.Companion` | exit 0, 30 s, 0 warnings, 0 errors |
| build-B `20261111T145310-3` / `-4` | c9b57cc5 | test Core / Rpc | 562 passed / 118 passed |
| build-B `20261111T150003-13` | 9ca853e2 | build | exit 0, 26 s, 0/0 |
| build-B `20261111T151404-28` | 25a37ae6 | build | exit 0, 32 s, 0 warnings, 0 errors |
| build-B `20261111T151436-29` / `-30` | 25a37ae6 | test Core / Rpc | **562 passed, 0 failed** / **118 passed, 0 failed** |

## 3. Probes on build-B (desk lock held, `--shareable`)
- `ask.ps1 run -AskProbe 40 -Replay -Shareable` (`AskProbe.cs:151-183`, recorded stream and transcript `:19-45`; `Store.W4.cs:42-51`): the recorded desk stream (`TextDelta` lines cut mid-mark, 450 ms apart, then `TurnFinished`) goes through the real `Store` path; `load_session` is answered from a recorded transcript (the desk exists on no engine) that holds one more line. Jobs `20261111T145439-5` (c9b57cc5) and `20261111T150925-23` (9ca853e2), exit 0, 46–48 s.
- `-CatchUp` / `-Send "text_with_underscores"` (the build service refuses spaces in args): `--ask-catchup` reads `load_session` from the real engine, `--ask-send` types the text and presses Enter (`AskProbe.cs:185-206` and `:208-212`). `AskFlags.cs`, `ask.ps1`.
- `w189-gate.ps1` (new): `engine`, `engine-start`, `engine-errors`, `tail PATH [N]`, `live-engine-start`, `live-run [S] [text|-]`, `live-engine-stop`, `live-clean`, `companion-log N [word]`.

## 4. Evidence
**Replay** (both passes, `20261111T145439-5` at c9b57cc5 and `20261111T150925-23` at 9ca853e2, print the same counts; the catch-up, finish and first-TextDelta lines are from the pass-1 companion log): chunk by chunk, the view's own diff counts —
`17 chars → 1 block: built 1` · `135 → 4: updated 1, built 3` · `240 → 6: kept 4, built 2` · `283 → 8: kept 6, built 2` · TurnFinished: `catch-up (turn-finished): 3 entries, 309 chars from the transcript (283 streamed), running=false, via=text → Finish` → `309 → 9 blocks: kept 8, built 1` · `ask finished (turn-finished · text): 309 chars, 283 streamed, ok` · `first TextDelta (17 chars) 21 ms after send` · `replaced-by-transcript=true`. Re-renders of unchanged text are `kept N, updated 0, built 0`: nothing is rebuilt.
Screenshots (Mac): `/Users/Sky.Moore/.theuth/briefs/2026-11-11/189w/evidence/` — `189w-ask-replay-1-streaming.png` (heading, paragraph with inline code, bold bullets; header `theuth · replying…`), `-2-streaming.png` (nested bullet with a link, fenced code, numbered list), `-3-finished.png` (header `theuth`; the line "Nothing else is waiting." exists only in the transcript), `contact-sheet-pass2.png` (all three side by side), `build-b-desktop-after-replay.png` (the whole desktop of build-B), `first-pass/` (c9b57cc5). On build-B: `%TEMP%\189w-ask-replay-{1-streaming,2-streaming,3-finished}.png`.
**A real engine (the scratch 2.12.0 binary on a private data dir, no model; jobs `20261111T150540-18` start, `-19` send, `-21` catch-up, `-22` stop):**
```
desk_open(theuth) = 01a1cb25-…  hub=true
set_event_focus([01a1cb25…]) ok (resolve)
ask-catchup: load_session(01a1cb25-…, null, 8) → 0 entries running=false roles=[]
set_event_focus([01a1cb25…]) ok (ask)                      <- D1: sent again for this ask, before the input
ask sent to desk 01a1cb25-… (25 chars)
ask: the desk started turn_seq=0
ask failed on the desk: model: Resolver error for model 'anthropic::claude-sonnet-4-5 (adapter: Anthropic)'.
ask finished (Error event): 0 chars, 0 streamed, failed    <- D3/D6: never left waiting
-- next run, after the failed turn --
ask-catchup: load_session(…) → 2 entries running=false roles=[user#0,error]
ask-catchup: user turn=0: hello from the 189w probe
ask-catchup: error turn=null: model: Resolver error for model 'anthropic::claude-sonnet-4-5 (adapter: Anthropic)'. Cause: ApiKeyEn…
```
The `load_session` shapes (roles `user` / `error`, `turn` on the user row, none on the error row, `running`) decode as `Models.189.cs` assumes.

## 5. Owed and findings
- **A live model reply on build-B is owed.** build-B has no provider (the real turn above fails at the model resolver). Streaming is proven by the replay (real Store, real view, recorded stream); focus, send, `TurnStarted`, `Error` and the transcript decode by the real-engine round. A real streamed answer needs a machine with a model.
- **The answer card's streaming body** (`AnswerCardWindow.cs:610-620`, `PointController.cs:1335`) is covered by the Core tests and the build only; no UI probe drives a point-and-say turn.
- **build-B's scratch engine is broken, and has been for about a week.** Task `theuth-test-engine` (`C:\Users\user\tools\theuth-test\engine-run.ps1`, binary 2.12.0) crash-loops every 5 s with `plugin agent failed to start: run migrations: migration 37 was previously applied but is missing in the resolved migrations`: the default data dir `C:\Users\user\.theuth` was migrated by a newer engine. `engine-run.log` is 25 MB and today's engine log 185 MB; the companion's `theuth engine unreachable` toast on build-B is this. I left the task as I found it (looping). My live round used a private data dir (`C:\Users\user\tools\w189-data`, since removed, as is its one-shot task `w189-engine-once`); a fix is a fresh `THEUTH_DATA_DIR` in `engine-run.ps1` or a newer binary.
- **The login item.** Every companion launch rewrites `HKCU\…\Run\TheuthCompanion` (`launch_at_login` is true in `companion.json`); it now points at this slot's Debug exe (`theuth-ci-w189`), as it did for earlier slots. The next install or launch overwrites it.
- **Link contrast.** The link colour is `AccentTextFillColorPrimaryBrush` (`#3408CD` on build-B, whose system accent is `#1F047A`) on `#202020`: dark, because this machine's accent is dark — the platform's own choice, and what WinUI's default hyperlink gave (same pixels before and after the explicit brush; the explicit brush only ties the link to the window's theme).
- Traps met: the build service refuses spaces, `|` and `:` in script args (use `text_with_underscores`, one word for patterns); `canvas.getImageData` in theuth's headless browser returns zeros, so pixel checks decode the PNG with python3; `Start-Process` with redirected output would hold the service job — the engine is started through a one-shot interactive scheduled task (`schtasks /it`), as `w5-gate.ps1 launch-task` does.
