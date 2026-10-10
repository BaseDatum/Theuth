# 158 as-built — W1W: Windows companion island prompt states (C#)

Branch `feat/158-w1w` from the C158 PIN 715e22d3. Scope: `crates/desktop/companion-windows/**` only. Design §5, §15.4,
§18 contradiction 6; mock `docs/design/assets/158-meetings-around.html` (cells A and D). The C158 PIN carried no C# stubs
(it touched Swift, TS and Rust only), so the C# shapes below mirror `TheuthKit/Models.swift` and `detect/mod.rs` directly.

## Built

**Core (`src/Theuth.Companion.Core`)**
- `MeetingReducer.cs`: `MeetingProcess` (+ `Display` = image basename without `.exe`, §5.2), `MeetingDetection(App, AppId,
  Since, Also)`, `MeetingPrompt = Detected(MeetingDetection) | Stop(Meeting, App, ReleasedForS)` (§15.4 `MeetingSlice.prompt`),
  `MeetingSnapshot.Prompt`; `MeetingChange.Summarized / Detected / DetectionCleared / AppReleased`. Reducer: `Detected` sets
  the prompt; `DetectionCleared` clears it only for the same `app_id`; `AppReleased` sets the stop prompt only for the live
  meeting and the newer `released_for_s` replaces the older; a `State` that leaves live clears the stop prompt (a detection
  prompt is not the meeting's and stays); `Started` empties the slice (prompt included). `Hydrate(…, detection)` seeds the
  prompt from `meeting_status.detection` for a companion connecting mid-prompt.
- `MeetingWire.cs`: `EventTypes` + `Summarized`, `Detected`, `DetectionCleared`, `AppReleased`; decoders tolerant of the
  `#[serde(default)]` fields (`also`, `event`, `project`, `brief`); a `Detected` without `app`/`app_id` is malformed → null.
  `Hydrate` reads `status.detection`.
- `IslandModel.cs`: `HasPrompt`; `ShouldShow(state) = IsLive || AlwaysShow || HasPrompt` (§5.1); `Present(meeting, state,
  prompt)` opens the island for any prompt (it is content, not a pill) and `Collapse()` refuses while a prompt is up
  (answered, not hidden). No-linger stands: the prompt clearing with nothing live hides the window through the existing
  ≤ 300 ms close animation.
- `StoreModel.WW1.cs` / `StoreEngine.cs`: `MeetingStartAsync(StartMeeting)`, `MeetingDetectionDismissAsync(appId, never)`.

**Rpc (`src/Theuth.Companion.Rpc`)**
- `Models.149.cs`: `ProcessInfo`, `Detection`, `StartSource` (`{"kind":"manual"|"detected"|"calendar", app_id?, event?}`),
  `StartMeeting` (the frozen six + 158's optional four; nulls omitted), `MeetingStarted`; `MeetingStatus.Detection`.
- `RpcClient.149.cs`: `MeetingStartAsync`, `MeetingDetectionAsync`, `MeetingDetectionDismissAsync`.

**App (`src/Theuth.Companion`)**
- `Island/IslandWindow.cs`: `IslandAction.StartRecording(MeetingDetection) / NotNow(appId) / NeverForApp(appId) /
  KeepRecording` (`Stop` already existed). `RenderOpen` → (a) `RenderDetected`: `<App> is using your microphone` · Start
  recording (accent) · Not now · Never for <App> · the R3 line verbatim; with no meeting the head row carries the grip and
  the eye. (d) `RenderStopPrompt` under the live strip/pane: `<App> stopped using your microphone N s ago` · Stop recording?
  · Stop · Keep recording. Keys (§5.3): ⏎ = Start on (a), ⏎ = Keep on (d); ⎋ = Not now on (a); N = Never for <App>; Ctrl+M
  (existing) = Stop. UIA ids `island-prompt`, `island-prompt-start/-not-now/-never/-lawful`, `island-stop-prompt-stop/-keep`;
  `AutomationProperties.Name` on the groups and buttons. `WS_EX_NOACTIVATE` / `WDA_EXCLUDEFROMCAPTURE` untouched (the prompt
  is a render path of the same window). `RenderKey` includes the prompt so it re-renders.
- `Island/IslandController.cs`: Start → `meeting_start { process: pid, consent: true, source: detected{app_id} }` and nothing
  else (§5.4); Not now / Never → `meeting_detection_dismiss(app_id, false|true)` with the prompt cleared at once (the engine's
  `DetectionCleared` then finds nothing); Keep → clears the stop prompt locally (the engine re-sends `AppReleased` every
  30 s). Hydration maps `MeetingStatus.Detection`. `--island-prompt` fixture: the Zoom prompt; Start runs the existing fixture
  meeting and raises `AppReleased(30 s)` 8 s in; `--island-prompt-auto` drives prompt → Start → stop prompt → Keep → Stop → Done
  unattended, logging `prompt158:` lines (shown, mode, state, prompt, rect, the rendered TextBlocks via `IslandWindow.RenderedText`)
  and a PNG per state under `%TEMP%\158-prompt-*.png`. Fixture timers are kept in a field: an unreferenced `DispatcherQueueTimer`
  never fired (first probe run).
- `Settings/SettingsWindow.WW3.cs:31`: caption no longer says "for 10 minutes after the last topic or brief" (§18 c6).

## Tests
- `IslandTests.cs`: `DetectedPromptsAndClearsByAppId`, `AppReleasedAsksToStopWhileLiveAndEndsWithTheRecording`,
  `APromptShowsAndOpensTheIslandWithoutAMeeting_AndGoesWithoutLinger`.
- `MeetingWireTests.cs`: `DecodesThePromptEvents` (Detected with/without `also`, malformed, DetectionCleared, AppReleased,
  `Upcoming` → null, `status.detection` hydrates); `Summarized` now decodes. `MirrorTests.SummarizedKeepsTheSlice` re-pinned to
  "decodes, slice unchanged".
- `Models149Tests.cs`: `DetectionDecodesAndStartMeetingEncodesThePromptStart` (the exact `meeting_start` params JSON).

## Deviations
- "Never for <App>" is a third button, not a ⋮ overflow menu: a WinUI `MenuFlyout` hosted in a tool window fail-fasts on
  build-B (`Windows/TrayMenu.cs:12`, the tray's documented case); the mock (cell A) draws three buttons. `N` still works.
- `Upcoming` (island state c) and `PickEvent` / the `[▾]` list are not decoded or rendered: W3 / W2 fill `event`, `project`,
  `brief` and own that surface; `MeetingDetection` carries only what W1 renders. Add fields there when they fill.
- The Rpc `MeetingEvent` DTO / `WireEvent.Typed` map were not widened: the island reads the raw stream hook
  (`StoreModel.EventReceived`), which already delivers unknown types; `MeetingEvent.Types` stays 15 (its test pins it).
- The prompt is cleared optimistically on Not now / Never / Keep; a failed dismiss is logged, not re-shown.

## Hand-offs
- None outside `companion-windows`. The engine's `meeting_detection_dismiss` is still the C158 stub (`detect/mod.rs:132`,
  returns an error) until W1's engine half lands; the companion logs that failure.

## Libraries
- JSON: `System.Text.Json` (already the companion's codec); UI: WinUI 3 controls already in use (`Button`, `StackPanel`, `Grid`).
  No new dependencies; nothing hand-rolled beyond glue.

## Gates
- `./test.sh` (portable slnf, local .NET 10): Core 496/496, Rpc 114/114 passed.
- build-B `POST /build` `build`, slot `158-w1w`, `Theuth.Companion.slnx`, Debug: `20261010T134431-300` (dd5fe753), `20261010T134839-302`,
  `20261010T135241-311`, `20261010T135439-317` (59fd937a) — 0 warnings, 0 errors, WinUI app included.
- build-B `POST /build` `test`: `20261010T134656-301` (dd5fe753) and `20261010T135618-323` (59fd937a) — Core 496/496, Rpc 114/114.
- build-B visible run under the desk lock (owner W1W; the installed companion quit via the gate's `quit-installed` and restarted
  with `start-installed`, pid 6300): `w6-gate.ps1 launch --island-prompt --island-prompt-auto --shareable` (jobs …-306/-307 first
  run, …-31x/-32x final). Log, pid 10948: `detected — shown=True mode=Open state=Idle prompt=Detected rect=(685,8,550x116)
  text="… Zoom is using your microphone · Start recording · Not now · Never for Zoom · By starting the recording you acknowledge
  the recording is lawful."`; `island shown: … exstyle=0x08000188 [TOPMOST|TOOLWINDOW|NOACTIVATE]`; after Start: `state=Recording
  prompt=-`; `stop-prompt — prompt=Stop rect=(…550x368) tail="… Zoom stopped using your microphone 30 s ago · Stop recording? ·
  Stop · Keep recording"`; `keep — prompt=- rect=(…550x305)`; `done — shown=False mode=Closed state=Done` (no linger).
