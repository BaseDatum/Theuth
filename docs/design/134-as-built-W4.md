# 134 — as-built W4: the Ask bar, live dictation, receipts, R on cards, hub routing

Branch `feat/134-w4b-ask` (cut from `feat/134-w4-ask` 5b36ae1a, itself from main a1cbe9fa = W1 + W1b), pushed to `basedatum`.
Replacement worker: the first W4 worker (`w4-ask`) was cut by the 23:10:51 engine restart; its commit 5b36ae1a (ask.ps1) and
its two unreviewed files (`Core/AskAudio.cs`, `Core/AskBarModel.cs`) were reviewed, fixed where needed and kept (§6).
Design: `docs/design/134-companion-windows.md` §4.3 (the Ask bar), §5.3 (RPC), §7 W4 row; the macOS original is
`crates/desktop/companion/Sources/Companion/AskBar/**`, `Voice/VoiceRecorder.swift` and `130-as-built-W3/W6.md`.
`WIN/` = `crates/desktop/companion-windows/`. Written 2026-10-08 01:00 (+01:00). Head `7c4af0f3` (merged with main 15e06d8d: W2 + W3 landed).

**In one screen.** The Ask bar is a WinUI window (`AskBar/AskBarWindow.cs`: 480 epx, borderless, topmost, hidden from
Alt+Tab, acrylic, excluded from capture unless `--shareable`) that renders a portable controller, `Core/AskBarModel.cs`
— Swift's `AskBarController` without a UI framework (open/close with the foreground hand-back, the Ask-bar key scope,
hold-to-talk on `AskVoiceMachine`, the ordered `dictation_start/audio/stop` calls with chunks held until the id arrives
and a release before the id remembered, the 5 s "finishing…" cap, the probes, the receipt row's two columns) — so every
rule is tested on the Mac (17 virtual-time tests). `Voice/VoiceRecorder.cs` is the microphone: NAudio.Wasapi 3.1.0's
`WasapiRecorderBuilder` on the default capture endpoint asked for 16 kHz mono i16 (shared mode, `AUTOCONVERTPCM`),
falling back to the mix format + `Core/AskAudio.MonoResampler`, 100 ms `Pcm16Chunker` chunks on the UI thread. The ask
flow itself was already in `StoreModel` (W1b): `SubmitAsk` → 3 s hold → `mesh_status` → `desk_open("theuth")` →
`send_input` → `TextDelta`/`TurnFinished` → `Receipt`. Verified live on build-B against the scratch engine 2.12.0 with the
dictation model junctioned in: the sine streams and comes back as `DictationLevel` RMS 0.212, synthesized speech comes
back as text in `ask-field`, a typed request reaches theuth's desk (`send_input` accepted, hub routing through
`desk_open`), the receipt row shows a two-line desk reply from a recorded stream. Found and fixed live: W1's
`set_event_focus` spread its session list into N positional params (engine: `-32602 Invalid params`), and a WinUI
`TextChanged` that arrives after a programmatic set fed a dictation partial back as an edit. W2's hooks are wired
(`AskBarAvailable`, `Hotkeys.Pressed`/`Released`); the bar is a W2 `ToolWindow`.

## 1. File map (path:line)

| Path | What |
|---|---|
| `WIN/src/Theuth.Companion.Core/AskAudio.cs:10-126` | `AskAudio`: `SampleRate` 16 000, `ChunkSamples` 1 600; `Tone` (`:22`, the Swift probe's 440 Hz / 0.3 sine, phase-continuous across chunks), `Rms` (`:33`), `ToPcm16`, `Decode` (`:52`: PCM 8/16/24/32 and IEEE float → floats, the WASAPI mix-format shapes), `ParseWav` (`:79`, RIFF/WAVE incl. EXTENSIBLE) and `Pcm16Mono16k` (`:113`, a whole WAV → 16 kHz mono i16 — the `--synthetic-speech` input) |
| `AskAudio.cs:129-172` | `Pcm16Chunker`: whole 100 ms chunks out of appends of any size, `Flush` hands out the tail (Swift `VoiceRecorder`: the release sends the tail too) |
| `AskAudio.cs:175-252` | `MonoResampler`: interleaved frames at any rate/channel count → mono floats at 16 kHz (channel average, Blackman-windowed sinc low-pass at 0.45 × 16 kHz when downsampling, linear interpolation) — the fallback for an endpoint that will not convert itself |
| `WIN/src/Theuth.Companion.Core/AskBarModel.cs:7-21` | `IDictationCalls` (prewarm · start → id · audio · stop) — the app's `AskBar/DictationCalls.cs`, a fake in tests |
| `AskBarModel.cs:23-50` | `IAskBarSurface` (Show(activate) · Hide(returnFocus) · StartMicrophoneAsync · StopMicrophone · CancelMicrophone · OpenUrl) — the window implements it |
| `AskBarModel.cs:53-` | `AskBarModel` (Swift `AskBarController` + `AskBarState`): field state (`Text`, `Partial`, `Live`, `Recording`, `Level`, `Status`, `IsOpen`, `VoiceText`, `OpenedByPress`, `FieldText` `:145`, `Placeholder`), `ReceiptLines` (`:154`, the receipt row's words + trailing hint for every `ReceiptRow`), `Open`/`ShowWithoutKey`/`Close`/`Toggle`, `HandleKey` (`:214`: bare printable keys always type, the rest through `KeyScope.AskBar` — Enter sends, Esc undoes a hold else closes with the hand-back, Ctrl+O opens the reply's or the desk's session, Ctrl+/ the legend), `Submit` (`:245`), `FieldEdited` (`:253`: typing over a dictated hold takes the hold back and makes later partials lose), `HotkeyDown`/`HotkeyUp` (a bar opened within the last 100 ms counts as opened by this press — W2 opens it through the Store before the press reaches the bar), the voice machine's 50 ms tick and `Apply` (`:323`: tap · start recording · stop · set text → `SubmitAsk` · fail), the ordered call chain (`Enqueue`/`Chain`, `Drained`), `Prewarm` once per bar, `BeginDictation` (`:412`: chunks before the id held, a release before the id → stop after the start), `Feed` (`:449`), `EndDictation` (`:458`, the 5 s `FinishCap`), `MicrophoneFailed` (`:484`), `DictationChanged` (`:490`: live text, level, finish → `Dictated`/`Failed`; a probe's dictation lands in the field without a send), `ProbeTone` (`:524`) / `ProbeSamples` (`:532`) |
| `WIN/src/Theuth.Companion.Core/AskRpc.cs:22-26` | `SetEventFocusAsync` now sends **one positional list** (`[["<session>"]]`); `SetEventFocusParams` exposes the wire shape for the test. The W1 version passed a `string[]` straight into `params object?[]`, which spread the sessions into N params — the 2.12.0 engine answered `rpc -32602: Invalid params` (seen live, §4.3) |
| `WIN/src/Theuth.Companion.Core/StoreModel.cs:596` | `ResolveDesk()` is `public` (was private): the probe resolves theuth's desk without sending, as Swift's `AskProbe.run` calls `store.resolveDesk()` |
| `WIN/src/Theuth.Companion/AskBar/AskBarWindow.cs` | The window (§4.3), a W2 `Ui.ToolWindow` (borderless `OverlappedPresenter`, `IsAlwaysOnTop`, `IsShownInSwitchers=false`, `WS_EX_TOOLWINDOW|TOPMOST`, the `WM_GETMINMAXINFO` subclass, `WDA_EXCLUDEFROMCAPTURE` unless `--shareable` — C11, Closing → hide): acrylic when `DesktopAcrylicController.IsSupported()` else the surface colour; row 0 glyph · `TextBox` **`ask-field`** (`:108`, Name "Ask theuth", help text, `Dictating: …` while live) · mic `FontIcon` **`ask-mic`** (`:117`, carnelian + scale 1 + level × 0.5 while recording); row 1 the hints as one wrapping line of runs **`ask-hints`** (`:130`: `⏎ send to theuth · it routes`, `hold Ctrl+Shift+Space to talk · release to send in 3 s` from the key table, `Esc cancel/undo`) + **`ask-status`** (`:143`, "listening…" · "finishing…" · a failure) + the `ms-settings:privacy-microphone` link when the status is the privacy refusal; row 2 the receipt row **`receipt-row`** (`:160`, words, 2 lines) + `receipt-hint` (the undo countdown / "Open Ctrl+O") + the 2 px malachite hold bar. `Show` (records the foreground window unless it is ours, places, `ShowActive()` + focus the field), `Hide` (`Win32.Foreground(previous)` — D5's hand-back), `OnActivate` (losing the foreground closes the bar, Swift's `didResignKey`; not under a probe), `OnCloseRequested` (Alt+F4 = Esc), `HotkeyDown(keyCode)` / `HotkeyUp` (the release is seen by the window's `PreviewKeyUp` once it has the foreground, by a 50 ms `GetAsyncKeyState` poll before that, or by W2's `Hotkeys.Released` — idempotent; the held key never types), keys through W2's `KeyInput.From` → `AskBarModel.HandleKey`, `Render`/`RenderReceipt`, `PlaceAtMouse` (top-centre of the display under the mouse, 20 % down, clamped to the work area, physical px from the window's DPI), `Fit` (the window follows its content — the receipt row comes and goes and grows to two lines) |
| `WIN/src/Theuth.Companion/AskBar/AskBarInstall.cs` | `Install(store, dispatcher, quit)` at the W4 line of `App.OnLaunched` (after `Ui.Surfaces.Start`): the window + model (`DispatcherScheduler`, `Log.Write`, `keys: () => store.Keys`, `automated` from the flags), W2's hooks — `Surfaces.AskBarAvailable = () => true` (the hotkey opens the bar when nothing needs you), `Hotkeys.Pressed(OpenPanel)` → `Bar.HotkeyDown(chord.KeyCode)` when `Store.AskBarRequested` fired within 100 ms (the press W2 routed to the bar), `Hotkeys.Released(OpenPanel)` → `Bar.HotkeyUp()` — then `--mic-probe` / `--ask-probe` / `--ask` |
| `WIN/src/Theuth.Companion/AskBar/AskProbe.cs` | `--ask-probe N`: `ask-probe.log`, desk resolved on connect (`ResolveDesk`, nothing sent), the bar **activated** at 1.5 s, `--synthetic-dictation` (2 s of the tone through `ProbeTone`), `--synthetic-speech "text"` (`Windows.Media.SpeechSynthesis.SpeechSynthesizer` → WAV → `Pcm16Mono16k` → `ProbeSamples`; falls back to the tone), `--ask-replay` (`RecordedReply` `:21`: a desk reply in the engine's `TextDelta`/`TurnFinished` wire shapes, 400 ms apart, through `Store.ReplayDeskReply`), every `Dictation*` line logged, quit after N s with exit 0 |
| `WIN/src/Theuth.Companion/AskBar/AskFlags.cs` | `--ask-probe N`, `--synthetic-dictation`, `--synthetic-speech T`, `--ask`, `--mic-probe N`, `--mic-mix-format`, `--ask-replay`; `Automated` = any probe |
| `WIN/src/Theuth.Companion/AskBar/AskLog.cs` | `Log.Write` + `%LOCALAPPDATA%\theuth\companion\logs\ask-probe.log` (truncated per run; ask.ps1 prints it) |
| `WIN/src/Theuth.Companion/AskBar/DictationCalls.cs` | `IDictationCalls` over `Store.Client` through `AskRpc` — `dictation_start(16000)`, never null |
| `WIN/src/Theuth.Companion/AskBar/ReplyField.cs` | R on a card (Swift `ReplyField`): a `StackPanel` with the one-line `TextBox` **`reply-field`** (placeholder = `card.ReplyLabel()`), Enter → `Store.SubmitReply` (the kind's own path after the same hold; empty refused), Esc → `Perform(Escape)`, the hint line; W2's panel places it in the focused card while `Store.Ask.ReplyFor == card.Item.Id` |
| `WIN/src/Theuth.Companion/Voice/VoiceRecorder.cs` | `StartAsync` (`:53`: default communications capture endpoint → `WasapiRecorderBuilder.WithFormat(16 kHz/16-bit/mono)` shared + event sync + 100 ms buffer; on refusal or `--mic-mix-format` the mix format + `MonoResampler`; `E_ACCESSDENIED` → `BlockedStatus` "microphone blocked in Settings › Privacy › Microphone" `:20`/`:101`; no endpoint → "no microphone"), `OnData` (`:135`, the capture thread: bytes → i16 → `Pcm16Chunker` → `OnChunk` on the UI thread; `Silent` buffers zeroed), `Stop` (`:180`, the tail goes out), `Cancel` (`:193`), `RecordingStopped` with an exception → `OnFailed`; `Description` logs device, mix and capture formats and the path |
| `WIN/src/Theuth.Companion/Voice/MicProbe.cs` | `--mic-probe N [--mic-mix-format]` (C8): every capture endpoint with its mix format, the default, then N s of capture with chunks/samples/RMS/peak per second and a total line; exit 0; needs no engine |
| `WIN/src/Theuth.Companion/Store.W4.cs` | `Store.Dictation` (`IDictationCalls` over `Client`), `ReplayDeskReply(session, text, lines)` (`Router.Sent` + a receipt + `HandleEvent` per recorded line) |
| `WIN/src/Theuth.Companion/App.xaml.cs:20,59-60` | The W4 field and the two marked lines (`askBar = AskBar.AskBarInstall.Install(Store, dispatcher, Quit);` before `Store.Start()`) |
| `WIN/src/Theuth.Companion/Theuth.Companion.csproj:40-43`, `WIN/Directory.Build.props:42-45` | W4's own `ItemGroup`: `NAudio.Wasapi` `$(NAudioWasapiVersion)` = 3.1.0 (MIT, net9.0; pulls NAudio.Core + System.Numerics.Tensors) |
| `WIN/ask.ps1` | The W4 gate helper for the build service's `script` action (first worker's, extended): `env` · `run -AskProbe N [-Synthetic] [-Speech words_with_underscores] [-Replay] [-Shareable] [-Ask]` / `run -MicProbe N [-MixFormat]` (prints `ask-probe.log` and the exit code) · `log` · `engine-log -Pattern` · `kill` · `model-link` / `model-unlink` (junction `<data_dir>\models\{nemotron-streaming-en,onnxruntime}` to the September data dir's copies) · `engine-restart` |
| `WIN/tests/Theuth.Companion.Core.Tests/AskAudioTests.cs` (6), `AskBarModelTests.cs` (18) | §2 |

## 2. Tests (Mac, `just companion-windows-test`, .NET 10.0.201)

**154 = Rpc 35 + Core 119** on the merged head (W1b left 124 = 35 + 89; W2 adds 6; W4 adds 24): `AskAudioTests` (6: the probe tone is the Swift one —
peak 0.3 × 32767, RMS 0.3/√2, phase-continuous chunks; the chunker's whole chunks + tail; every WASAPI sample format
decodes; a 48 kHz stereo WAV becomes 16 kHz mono with the 440 Hz intact — 395 ± 5 zero crossings in 0.45 s, RMS 0.21; a
22.05 kHz float mono WAV resamples up; an unknown file is refused; 16 kHz passes through, DC survives the low-pass) and
`AskBarModelTests` (18: a hold past 0.25 s opens the mic, `dictation_start`, audio in order, the release stops it, the
finish lands the text in the 3 s hold and `send_input` goes to the desk; a tap toggles, the press that opened the bar does
not close it; chunks before the id are held and a release before the id stops after the start; a refused start
("already dictating" → "dictating elsewhere") and a microphone that will not open fail the press and stop the dictation; a
microphone dying mid-recording; the 5 s finishing cap; "Nothing recognized" is a failure, not a send; typing during the
voice hold takes the hold back and later partials lose; the synthetic tone streams 20 × 1 600 samples in order then
stops, each at RMS 0.212, and an empty closed utterance leaves the field empty with nothing sent; synthesized speech goes
out in whole chunks plus the tail; the Ask-bar keys — bare letters and `?` type, Enter sends, Esc undoes then closes with
the hand-back, Ctrl+O opens the reply's session, Ctrl+/ the legend; Ctrl+L in the panel opens the bar; closing during a
recording cancels it; the probe never touches the hotkey but a typed Enter still sends; `set_event_focus` sends one
positional list; a bar W2 opened for the same press counts as opened by the press; the receipt row's two columns through hold → sent → a recorded `TextDelta`/`TurnFinished` stream →
"theuth · Plan — handed to w3 — the docs ship tonight" / "Open Ctrl+O", and Failed/Stale/Undone/Sent).
The existing `StoreModelTests.AskTheuthRoutesThroughTheDeskAndStreamsTheReply` (W1b) still covers `mesh_status` →
`desk_open` → `set_event_focus` → `send_input` → reply → focus re-applied on `PendingPrompts`.

## 3. build-B (Windows 11 Pro 25H2 26200, build service slot `w4b`, worktree `C:\Users\user\Develop\theuth-ci-w4b`)

- `dotnet build -c Debug -p:Platform=x64` at the merged head `7c4af0f3`: **0 warnings / 0 errors**, 26 s, 487 files / 172.9 MB
  (W2's 484 + NAudio.Wasapi/NAudio.Core/System.Numerics.Tensors; W4 alone was 464 / 172.4). Rounds on the way: CS0104 (`DispatcherQueue` ambiguous beside `Windows.System`), SYSLIB1062 (`LibraryImport`
  needs `AllowUnsafeBlocks`), CS0618 (`MMDevice.AudioClient` obsolete in NAudio 3.1 → `CreateAudioClient()`).
- `build.ps1` (Release publish): W4 alone 463 files / 172.3 MB (job 20261007T230729-63); the merged head `7c4af0f3` **485 files /
  172.9 MB** (job 20261007T235723-122) → `target\release\companion-windows\`, the five runtime-MSIX files present, no onnxruntime/DirectML,
  `Register()` ok (`Setting=Enabled`), `UnregisterAll()` ok.
- The scratch engine (theuth 2.12.0 headless, `C:\Users\user\.theuth`, RPC 127.0.0.1:7777, no model provider) had **no
  dictation model**: `<data_dir>\models` did not exist. `ask.ps1 model-link` junctioned `models\nemotron-streaming-en`
  (850.5 MB, 3 files) and `models\onnxruntime` (43.7 MB, 5 files) to the September data dir's copies
  (`C:\Users\user\.theuth-old-2026-09\models\`); the engine's lazy load then worked (`DictationModel loading` → `ready` in
  7 s on the first `dictation_start`). `model-unlink` removes exactly those junctions. The engine was not restarted.

## 4. Gates (§7 W4) with evidence

All runs through the build service's `script` action (`ask.ps1 run …`, which starts the exe in Sky's RDP session 1, waits,
prints `ask-probe.log` and the exit code) while holding the desk lock as `w4b`; UIA through `windows-build-b`
(`window_management find "Ask theuth"` → `ui_find automationId` → `ui_read` / `ui_type` / `keyboard_control`).
Exit code 0 on every run; the companion was killed and the desk released after each round.

### 4.1 `--ask-probe 16 --synthetic-dictation` — the sine through `dictation_start/audio/stop` (job 20261007T225812-54, 23:58)

```
ask-probe start seconds=16.0 synthetic-dictation=true …
ask-probe desk=01a11896-cbe4-7252-a03a-4cadee849ae3 this-engine-is-hub=true (nothing sent)
synthetic-dictation start: 2 s of 440 Hz at 16 kHz, 100 ms chunks, microphone untouched, nothing sent
event {"type":"DictationModel","state":{"state":"loading"}}
ask-probe bar shown activated=true visible=true pos=768,250 size=480x78
stop sent; waiting for DictationFinished
after stop: phase=Starting utterances=[] liveText="" error=nil field=""          ← the start was still loading the model
event {"type":"DictationModel","state":{"state":"ready"}}                           (+7.4 s)
event {"type":"DictationStarted","id":"01a11896-e8cf-…"}
event {"type":"DictationLevel","id":"…","levels":[0.2124,0.2108,0.2139,0.2105,0.2130,0.2124,0.2108,0.2139]}   × 3
event {"type":"DictationText","id":"…","utterance":0,"text":"","final":true}
event {"type":"DictationFinished","id":"…","reason":{"reason":"done"}}
ask-probe done after 16.0 s — quitting (connected=true receipts=2 field="" dictation phase=Idle liveText="" error=nil) exit 0
```

Read: the whole path is live — `dictation_prewarm`, `dictation_start(16000)`, 20 × 1 600 samples as base64 LE i16 **held
until the id arrived and then flushed in order, the stop after them** (the model took 7 s to load on this first start; the
Mac W6 run had a warm model), `DictationLevel` reads RMS 0.212 = 0.3/√2 — the engine received the sine intact (the same
number as 130 W6 §2.5). A pure tone is not speech: one closed, empty utterance, so the field stays empty — exactly the
Mac result. `ui_find("ask-field")` during the run reported Name `Dictating: ` (the live field). The desk resolved
through `mesh_status` + `desk_open("theuth")` on this engine as the hub (**hub routing**), nothing sent.

### 4.2 `--ask-probe 24 --synthetic-speech …` — words in `ask-field` (job 20261007T230136-57, 00:01)

The system voice ("Microsoft David", en-US) rendered the text to a 16 kHz/mono/16-bit WAV (41 486 bytes, 1.29 s, RMS 0.043
→ 13 chunks) through `AskAudio.Pcm16Mono16k` and the same `dictation_start/audio/stop` path:
`DictationText {"utterance":1,"text":"Ship","final":true}` → `DictationFinished done` → `after stop: … liveText="Ship"
field="Ship"`; **`ui_read("ask-field")` returned `""` at 00:01:43 and `"Ship"` from 00:01:45 on** (eight reads over 18 s).
Only the first word was spoken: ask.ps1 handed the unquoted sentence to `Start-Process`, so the exe saw `--synthetic-speech
ship the docs …` and took `ship` (the log's `said "ship"`); fixed in ask.ps1 (`-Speech words_with_underscores`, quoted) —
§4.6 re-runs it.

### 4.3 `ui_type` a request + Enter → the request reaches theuth's desk (job 20261007T230246-59, 00:02)

`--ask-probe 40`; at ~00:02:53 `ui_type(ask-field, "what is open right now?", clearFirst)` → `keyboard_control press Enter`:

- `ui_read("receipt-row")` 0.8 s later: **`theuth · what is open right now? — sending`** (the 3 s hold; `receipt-hint`
  counts down `Esc undo · 3`); 4 s later: **`theuth · sent — waiting for the desk`** ("Open Ctrl+O").
- The companion log (the log window, screenshot `w4b-ask-typed.png` in the service's logs dir):
  `desk_open(theuth) = 01a11896-cbe4-7252-a03a-4cadee849ae3 hub=true self=P50 holder=69ceb45674110100…` ·
  `probe: typed send (23 chars)` · **`ask sent to desk 01a11896-cbe4-… (23 chars)`** = `send_input` accepted; the desk's
  turn starts and, with no provider, cannot answer — the live reply is owed (§7).
- **`set_event_focus failed: rpc -32602: Invalid params`** — found here: W1's `SetEventFocusAsync` passed the `string[]`
  straight into `params object?[]`, spreading the sessions into N positional params. Fixed (`AskRpc.cs:22-26`, test) —
  §4.6 shows the re-run.
- The Ask bar was **not** in the full-screen capture (`WDA_EXCLUDEFROMCAPTURE`, C11 — only `--shareable` runs are visible).

### 4.4 The two-line receipt from a recorded `TextDelta`/`TurnFinished` stream (job 20261007T230309-60, 00:03)

`--ask-probe 14 --ask-replay --shareable`: `Store.ReplayDeskReply("replay-desk", …)` + three `TextDelta` lines and one
`TurnFinished` in the engine's wire shapes (`AskProbe.RecordedReply`; `crates/agent/src/lib.rs:2168`, `:2253`) 400 ms apart.
`ui_read("receipt-row")` (three reads): **`theuth · Handed to w3 — the docs ship tonight. — Then the release notes;
nothing else is waiting.`**; `ui_read("receipt-hint")`: `Open Ctrl+O`. The log shows the receipt after each line
(`"theuth · sent — waiting for the desk"` → `"theuth · Handed to w3 —"` → … → `finished=true`). The screenshot
(`w4b-ask-replay.png`) shows the bar: field, mic, hints, the receipt row. It also showed two layout faults fixed in the
next build (§4.6): the hints row overflowed 480 epx (`Ctrl+Shift+Space` is wider than `⌃⇧Space`) and the window did not
re-fit when the reply grew to two lines, so the second line was clipped.

### 4.5 C8 — a real mic capture in the RDP session (job 20261007T230415-61, 00:04)

`--mic-probe 6`: **NAudio reports no capture endpoint in Sky's RDP session** —
`mic-probe capture endpoints: 0` (`MMDeviceEnumerator.EnumerateAudioEndPoints(Capture, All)`),
`mic-probe default (communications) = none`, `mic-probe open: no microphone`, six seconds of `chunks=0 … rms=0.0000`,
`non-zero=false path=not opened exit 0`. `ask.ps1 env` (PnP `AudioEndpoint` class) lists only "Remote Audio", the render
endpoint: the RDP client redirects playback, not a microphone, so a non-zero RMS cannot be produced on this desktop. What
the capture stack does is covered from the other side (the engine received our 16 kHz i16 PCM and read its level, §4.1)
and by the Mac tests of every conversion step; `--mic-probe N` is the one-command check for a session with a microphone
(a redirected mic in the RDP client, or the console session) — owed (§7).

### 4.6 The final round on the merged head (W2 + W3 landed; desk lock `w4b`, 00:51–00:57)

Head `7c4af0f3` = this branch + `basedatum/main` 15e06d8d (W2 b661543e + a9c5da2f, W3 99358089) — Mac 160 (Core 125 + Rpc 35),
build-B `dotnet build` 0/0 (26 s, 487 files / 172.9 MB), `build.ps1` publish 485 files / 172.9 MB with the five MSIX files and
`Register()` ok. W2's rail is on the right edge of every screenshot; W3's notifier registered before the window.

| Run | Evidence |
|---|---|
| `--ask-probe 40 --shareable` + `ui_type(ask-field, "what is open right now?")` + Enter (job …235123-114) | `receipt-row` **`theuth · what is open right now? — sending`** with `receipt-hint` `Esc undo · 3`; 4 s later **`theuth · sent — waiting for the desk`**, the field still `what is open right now?`; companion log: `ask bar: installed (hotkey → bar when nothing needs you; Ctrl+L; tray menu)` · `desk_open(theuth) = 01a11896-cbe4-… hub=true self=P50 holder=69ceb456…` · **`set_event_focus([01a11896…]) ok`** (the -32602 is gone) · `probe: typed send (23 chars)` · **`ask sent to desk 01a11896-cbe4-… (23 chars)`**. Screenshot: the bar at 768,250 (480 × 92 → re-fit), hints on two lines, the receipt row with its countdown. `ask bar: Windows refused the foreground` is logged once: under the probe no user input preceded `SetForegroundWindow` (the hotkey's WM_HOTKEY grant is what makes it legitimate in real use); the harness's `ui_type` activated the window anyway |
| `--ask-probe 9 --synthetic-dictation` (job …235525-118) — the gate's exact flags | model warm: `DictationStarted` 20 ms after the start, 4 × `DictationLevel` (RMS 0.212), `DictationText {"utterance":3,"text":"","final":true}`, `DictationFinished done`; `ui_find("ask-field")` Name `Dictating: ` during the stream, `ui_read` `""` — the tone is not speech (as 130 W6); exit 0 |
| `--ask-probe 22 --synthetic-speech "ship the docs tonight and tell the team"` (job …235300-116, then …235621-120 after the fix below) | "Microsoft David" → 91 566-byte 16 kHz WAV → 29 chunks; `DictationText` utterance 2: `Ship the docks` → `Ship the docks tonight and tell` → final `Ship the docks tonight and tell the`; **`ui_read("ask-field")` over the stream: `""` → `Ship the docks` → `Ship the docks tonight and tell` → `Ship the docks tonight and tell the`** (the field fills with the transcript as it streams; the voice says "docks"). The first of these runs showed the partial repeated five times mid-stream: WinUI raises `TextChanged` asynchronously after a programmatic `Text` set, so the `settingText` flag did not cover it and the model took its own partial back as a user edit; `AskBarWindow` now ignores a change whose text equals the model's `FieldText` (`7c4af0f3`); the second run streamed cleanly |
| `--ask-probe 12 --ask-replay --shareable` (job …235554-119) | `receipt-row` **`theuth · Handed to w3 — the docs ship tonight. — Then the release notes; nothing else is waiting.`**, `receipt-hint` `Open Ctrl+O`, `ask-hints` `⏎ send to theuth · it routes    hold Ctrl+Shift+Space to talk · release to send in 3 s    Esc cancel`; the screenshot shows the row on **two lines** inside a re-fitted window (the clipping of §4.4 is gone) |

Not re-run: `--mic-probe` (no capture endpoint exists in this session, §4.5 stands). The desk was released at 00:57 with no
companion left running.

## 5. Checks C3 · C4 · C8 (design §6, 130 §6.1)

- **C3 — the 3 s undo is universal.** A typed Enter and a dictated release both go through `StoreModel.SubmitAsk` →
  `Hold` → `Commit` (`AskBarModel.Submit` `:245`, `Apply` SetText `:323`); Esc during the hold is `Perform(Escape)` →
  `UndoQueue.Cancel` → "Taken back" and the text stays in the field (test *TheAskBarKeysSendUndoCloseAndOpenTheDesk*);
  typing over a dictated hold takes it back too (*TypingDuringTheVoiceHoldKeepsEditingInsteadOfSending*). Live: §4.3's
  `receipt-hint` counted `Esc undo · 3` during the hold; the 2 px hold bar shrinks with `HoldRemaining`.
- **C4 — one field, one route, no picker.** Everything typed or dictated goes to theuth's desk, resolved once per
  connection (`AskRouter.Plan`: this engine when it holds the lease or has no mesh, a replicated desk id off the hub, else
  a refusal in the receipt row); `--ask-probe` logs `desk_open(theuth) = … hub=true` and `this-engine-is-hub=true` (§4.1,
  §4.3). The bar shows the desk's answer as two lines in the same receipt row (§4.4), never a transcript.
- **C8 — the microphone opens only on a hold.** The recorder is started by `Effect.StartRecording` after a ≥ 250 ms press
  (`Apply` `:323`); taps, Enter, the probes and `--ask-replay` never touch it (*TheProbeNeverTouchesTheHotkey…*,
  *TheSyntheticTone…*: `surface.Events` has no `mic start`). Capture is 16 kHz mono i16 either by WASAPI's own conversion
  or by `MonoResampler`, 100 ms a call. In the RDP session NAudio reports no capture endpoint (§4.5); the real-mic RMS
  is owed. Windows has no microphone prompt: `E_ACCESSDENIED` from the endpoint becomes "microphone blocked in Settings ›
  Privacy › Microphone" with the `ms-settings:privacy-microphone` link in the hints row (`VoiceRecorder.cs:101`,
  `AskBarWindow` `privacyLink`); build-B's ConsentStore values are `Allow` (`ask.ps1 env`).

## 6. Deviations from the design and the brief

1. **Logic in Core, not in `AskBar/`.** The design's §5.2 names `AskBar/` and `Voice/` as W4's folders; the controller
   went to `Core/AskBarModel.cs` (+ `Core/AskAudio.cs`) so the hold-to-talk, dictation ordering, probe framing and
   receipt-row rules run in the Mac suite — the W1b `StoreModel` pattern. New files only; no sibling's file touched.
2. **Two one-line edits to W1 Core files**: `StoreModel.ResolveDesk` `private` → `public` (the probe needs Swift's
   `store.resolveDesk()`), and the `AskRpc.SetEventFocusAsync` param fix (a real bug found live, §4.3).
3. **The probe bar is activated and a typed Enter sends.** Swift's `--ask-probe` showed the bar without key and refused
   every send; the W4 gate types a request into the bar and expects `send_input` to go, so `AskBarModel.Automated` only
   disables the hotkey and the microphone (`Submit` logs `probe: typed send`). The synthetic dictation itself still never
   sends (no voice press → no `SetText`).
4. **The open partial is not muted inside the field.** A WinUI `TextBox` cannot colour a range; while live the field shows
   the finals, a space, then the partial (`FieldText`) — the "listening…" status, the carnelian mic pulse and the UIA name
   `Dictating: …` carry the state. (A `RichEditBox` could mute the range; not worth its Enter/UIA quirks for 480 epx.)
5. **Hints are one wrapping line, not three fixed cells.** `hold Ctrl+Shift+Space` is twice the width of `⌃⇧Space`; the
   row wraps to two lines at 480 epx instead of clipping.
6. **The hotkey's key-up is observed, not delivered.** `RegisterHotKey` has no key-up message; the bar sees the release
   through its own `PreviewKeyUp` once it has the foreground and through a 50 ms `GetAsyncKeyState` poll before that
   (`AskBarWindow.cs:283-318`). W2's `Hotkeys.cs` only has to call `AskBarInstall.Bar.HotkeyDown()` for a press routed to
   the bar (`HotkeyUp()` is idempotent if W2 detects the release itself).
7. **Win32 through W2's CsWin32 after the merge.** Before W2 landed the bar carried six `DllImport`s of its own; they are
   gone — `PInvoke.*`, `Interop.Win32`, `Ui.KeyInput.From`, `Ui.Links.Open` and the `ToolWindow` base are W2's.
8. **The dictation model was linked, not downloaded.** The scratch engine cannot fetch models (no provider, no
   downloader); `ask.ps1 model-link` junctions the September data dir's `nemotron-streaming-en` + `onnxruntime` into
   `C:\Users\user\.theuth\models\`. The gate's "record the exact engine error" branch was therefore not needed; the
   error path is covered by the Mac tests (a refused start → "dictating elsewhere"/the reason in the status).
9. **`--synthetic-speech`** is an addition (the sine cannot produce a transcript): the system `SpeechSynthesizer` → WAV →
   `Pcm16Mono16k` → the same path; it is what proves "the field fills with the transcript" on Windows.
10. **The recorded reply is a fixture in the engine's wire shapes**, not a capture from a live model (none exists on the
    scratch engine): `AskProbe.RecordedReply`'s field names follow `AgentEvent::TextDelta`/`TurnFinished`
    (`crates/agent/src/lib.rs:2168`, `:2253`, incl. `usage` and `context`); `StreamDelta.Decode`/`WireEvent.Decode`
    accept them (the W1b test fixture lacked `context` and was silently dropped by `WireEvent.Decode` — the
    `AskBarModelTests` receipt test carries the full shape).
11. **`ReplyField` is a control, not a placed card element.** The panel is W2's; the R field (`AskBar/ReplyField.cs`) is
    ready for W2 to place in the focused card while `Store.Ask.ReplyFor == card.Item.Id` (as Swift's `PanelView` does).

## 7. Owed

- **The live desk reply** in the receipt row (`TextDelta`/`TurnFinished` from a real turn): the scratch engine has no
  provider; the send is verified live (§4.3), the rendering from the stream in the Mac test and `--ask-replay` (§4.4).
- **A real microphone capture with non-zero RMS (C8)**: no capture endpoint in the RDP session (§4.5). Run
  `ask.ps1 run -MicProbe 6` (and `-MixFormat`) in a session with a microphone — a redirected mic in the RDP client's
  settings, or the console — and read the `mic-probe t=Ns … rms=` lines; the direct-16 kHz vs mix-format path is logged.
- **The hold-to-talk gesture end to end on the real hotkey**: W2's `Hotkeys.Pressed`/`Released` are wired
  (`AskBarInstall`), the machine is Mac-tested (incl. the W2-opens-first ordering) and the dictation path is live-tested, but
  nobody held Ctrl+Shift+Space on build-B — `keyboard_control` cannot hold a chord for 300 ms and the harness has no
  microphone. First real hold: on Sky's machine.
- **The muted partial** (deviation 4) if Sky wants it exact: a `RichEditBox` or a two-run `TextBlock` swapped in while live.
- **Owed by the junction**: `ask.ps1 model-unlink` before W5's install test if the installed app must start with an empty
  `models\` dir (the junctions point into `.theuth-old-2026-09`, nothing was copied).

## 8. For W2 · W3 · W5

- **W2 (landed; for its follow-ups)**: the Ask bar is `AskBar.AskBarInstall.Install(...)`'s `Bar` (`AskBarWindow : Ui.ToolWindow`),
  installed after `Ui.Surfaces.Start` and wired to `Surfaces.AskBarAvailable` (= true), `Hotkeys.Pressed` (a press your
  `OnHotkey` routed to the bar — `Store.AskBarRequested` within 100 ms — becomes the voice press) and `Hotkeys.Released`.
  Your panel already carries the R field (`reply-field` → `Store.SubmitReply`); `AskBar/ReplyField.cs` is the same control
  as a reusable piece if you want one. When the bar is summoned over an open panel the panel's focus-loss hides it; the
  bar's Esc hands the foreground to the window recorded when it opened (never one of ours).
- **W3**: nothing of yours is touched. `Store.UndoDelay` is read at every hold, so the Settings value applies to asks too.
  The Ask bar's status row is where microphone refusals show (no toast).
- **W5**: `NAudio.Wasapi.dll`, `NAudio.Core.dll`, `System.Numerics.Tensors.dll` are now in the publish folder (463 files /
  172.3 MB). The companion needs no audio permission entry (unpackaged); Settings › Privacy › Microphone governs it.
  Before the install test with a fresh data dir, `ask.ps1 model-unlink` if the junctions should not be there.
- **Everyone**: the build service refuses spaces in `args` — ask.ps1's `-Speech` takes `words_with_underscores`.
