# 149 — as built, WW3: the native island and native point-and-say on Windows

Branch `feat/149w-ww3` (cut from `integrate/149` 77fa225f; WW1's skeleton 2c965119 merged). Worker: theuth subagent `ww3-island`,
2026-10-09 01:38 → (see Revision history). Design: `docs/design/149-companion-native.md` §3.6, §3.7, §5.4 C/D, §7.4 #1 and #3–#5,
§8 island/pointAndSay scopes, Revision 1 #9–#14.

## 1. What was built — files

### Core (`src/Theuth.Companion.Core`, Mac-tested)
| File | What |
|---|---|
| `PointMachine.cs` | The C# port of `crates/desktop/src/point/machine.rs`: `PointMachine.Step(state, event, now, refused)` with the same constants (`HoldMs 200 · DoubleTapMs 300 · MinNoteMs 800 · SampleHz 60 · DoneMs 6000 · Hint`), `PointState` (Idle · Armed · Holding · Tap1 · Locked · Sending · Replying, `Json` in the shell's tagged snake_case), `PointEvent`, `PointEffect`. |
| `IslandModel.cs` | `PxRect`, `IslandDisplay(Device, WorkArea, Scale, Primary)`, `IslandMode` (Closed · Open · Detached); placement: `DefaultFrame` = top-centre of the work area (+6 epx gap), `Place` = the saved offset per **monitor device name** when it still intersects the work area else the default (an off-screen memory is forgotten), `DragEnded` = snap back within `SnapPx 40 × scale` of the default's centre (memory cleared) else remember, `SnapBack` (double-click / reset), `ResetAll`; visibility D10 / Rev 1 #13: `ShouldShow(state, lastTopicOrBrief, now)` = live (Starting · Recording · Paused · Stopping) ∨ ≤ 10 min since the last topic/brief ∨ Always show, `HideDue`; open/close/pin; persistence (`islandFrames` JSON `{device: [dx, dy]}` + `islandAlwaysShow`); `Clock(ms)` = `mm:ss`. |
| `MeetingReducer.cs` | The port of `UI/store/meeting.ts:156-220`: Core records `MeetingTopic · MeetingBrief · MeetingLine · MeetingOracleRow`, the input union `MeetingChange` (13 meeting events + `Activity`), the immutable `MeetingSnapshot`, `Reduce` (every arm incl. the brief-stack rule `:181-190` verbatim, oracle cap 50, lines cap 200, labelled re-publish), `Move` (↑ eats the badge, ↓ leaves it), `MarkTapped`, `Hydrate`, `OracleSummary`. |
| `MeetingWire.cs` | Engine JSON → the reducer: `Decode(type, utf8)` for the 13 events + `TurnStarted/ToolCall*/TurnFinished` (tagged `type`, `final` defaults true, `{"Post": …}` state, `{"SttDown": "Mic"}` degradation → `SttDown:Mic`), `Hydrate(status, briefs, live)`. Tolerant: missing fields are defaults, malformed input is null. |
| `PointSettings.cs` | Settings › Voice key (D19) under their own keys `voiceKeyEnabled · voiceKeyIncludeScreen · voiceKeyAccessibility (auto/off) · voiceKeyAnnotate (always/mouse) · voiceKeySpeak · voiceKeyHintsShown`; `PointFlow`: `FinalTextMs 8000 · HintMs 2000 · ErrorMs 4000`, `TrailRgb 0x4a8a6e` (malachite dark) + `TrailHex`, `NoteText` / `ComposeBody` (= `lib/voice-note.ts` + `composeBody`), `RefusalHint` (talking → "Meeting running — use the island to ask", already dictating → "dictating elsewhere", screen → "Screenshots off — sent without the screen", hub → "theuth's desk lives on another engine"), `ListeningStatus`. |

Tests (`tests/Theuth.Companion.Core.Tests`): `PointMachineTests.cs` (the 17 Rust tests one for one + a constants test),
`IslandTests.cs` (IslandModel 7 + MeetingReducer 9), `MeetingWireTests.cs` (2), `PointSettingsTests.cs` (2).

### App (`src/Theuth.Companion`, Windows only)
| File | What |
|---|---|
| `Island/IslandWindow.cs` | The island: a `ToolWindow` (W2's base: `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, `OverlappedPresenter` without border, `IsShownInSwitchers=false`, `WDA_EXCLUDEFROMCAPTURE` unless `--shareable`) + `WS_EX_NOACTIVATE`, shown with `AppWindow.Show(false)` (= `SW_SHOWNOACTIVATE`), opaque `#141210` surface, 1 px `#3a3228` rule, radius 18, dark theme forced. Closed pill 300 × 36 epx: rec dot (carnelian recording · sand paused · faience otherwise) · `mm:ss` · listening/approval/activity word · the topic count badge (gold when > 0) · `↑N new`. Open 440 epx wide: strip (grip ⋮⋮ · dot · clock · degradation badges · listening/approval · mute/resume · stop · eye) · topic buttons with ✕ (tapped = 50 %) · brief pane (title · `i/n` · `↑N new` · ≤ 3 bullets · deeper · ‹ older · newer › · source) · the newest transcript line · the request box (⏎ sends `meeting_request`, ⎋ leaves). Hover 0.3 s opens, 100 ms after leave closes unless pinned by a click. Drag: the whole pill / the left 24 epx of the strip / any non-control spot → pointer capture + `AppWindow.Move`; release → `IslandModel.DragEnded` (snap-back or remember + save); double-click the grip → `SnapBack`. Keys: ⎋ (leave the box / close), ⏎ in the box, M mute, E dismiss the first topic, D deeper (single keys only when the box has no focus). Focus hand-back to the previous foreground after every press that is not the request box (the rail's S1 rule). A second subclass forwards `WM_HOTKEY` (point-and-say's Esc) and `WM_DISPLAYCHANGE`/`WM_DPICHANGED` (re-place). Voice mode (`VoiceView`): phase dot · status · live text · the answer card (`working…` / text / "Open Ctrl+O · Esc hides") · hint. UIA ids `island`, `island-pill`, `island-clock`, `island-topics`, `island-topic-<n>`, `island-mute`, `island-stop`, `island-eye`, `island-brief-title`, `island-deeper`, `island-request`, `island-transcript`, `island-voice`, `island-voice-live`, `island-voice-answer`. |
| `Island/IslandController.cs` | Owns the model (loaded from `settings.json`), the snapshot (reduced from WW1's `Store.EventReceived` raw hook through `MeetingWire`; hydrated from `Store.Meeting/Briefs/Live` on `MeetingRefetched`), D10 visibility (+ the hide timer), `meeting_cursor` on cursor change (the companion is the only caller), the actions → `Store.Meeting{Tap,Dismiss,Deeper,Request,Pause,Resume,Stop}Async`, Move (local), the eye (island + overlays, Rev 1 #9), the meeting global keys (`GlobalKeyPressed/Released`: PTT Alt+` → `meeting_ptt(true/false)`, eye Alt+Shift+E) and `GateMeetingRows()` (re-applies W2's `Hotkeys` with the two rows unbound while no meeting is live; the orchestrator's note). `--island-always` = Settings › Island "Always show" for probes. |
| `PointAndSay/PointController.cs` | The voice key through `Hotkeys.Pressed/Released` (`KeyAction.VoiceKey`, RegisterHotKey + MOD_NOREPEAT down, 50 ms GetAsyncKeyState release poll) → `PointMachine`; effects: **Start** = `stop_speaking`, overlays revealed on every monitor, the 60 Hz sampler, Esc registered (RegisterHotKey on the island's HWND, only between press and send), `dictation_start(16000)` + `VoiceRecorder` (the companion's own PCM, Rev 1 #11) → `dictation_audio`, the island's voice mode; **ShowLock**; **Stop** = sampler off, overlays hidden + one dispatcher turn awaited (D13), then `dictation_stop` (final text capped at 8 s) ∥ `screen_context({samples, color, since_ms, accessibility})`, `desk_open("theuth")` cached per engine, `send_input(desk, "[voice note, N s] …\n[screen …]", files, null, clientId)` (the engine steers a busy desk by its default delivery), then the desk's `TurnStarted/TextDelta/ToolCallStarted/TurnFinished/Error` (raw hook, filtered by session) fill the answer card, `speak(desk, text)` on finish when the setting is on (Rev 1 #10), `PanelDone` → Replying → after `DoneMs` → Idle + Conceal; **Discard**/**Refuse** = everything down, the hint for 2 s (the "short"/"double-tap-missed" hint only on the first 3 uses). Refusals: off · disconnected · a live meeting listening ("talking") · the Ask bar dictating. |
| `PointAndSay/OverlayWindow.cs` | One overlay per monitor: a raw Win32 window of class `TheuthCompanionOverlay`, `WS_POPUP` + `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, `WDA_EXCLUDEFROMCAPTURE`, sized to the monitor's bounds in physical pixels, shown with `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE|SWP_SHOWWINDOW)`, painted through `UpdateLayeredWindow` (per-pixel alpha) from a 32-bit top-down DIB. `OverlayWindows` = the set (re-created when the monitor list changes), sample routing by monitor, `Present` once per tick, `Hide`, the eye. |
| `PointAndSay/TrailRaster.cs` | The look of `UI/routes/overlay.tsx`: 6 pt stroke, round caps/joins, alpha 0.85, persisting while held; 14 pt ring 2 pt wide at the newest point, 0.85 pen-down / 0.4 pen-up; signed-distance coverage into premultiplied BGRA, max-alpha union so overlapping caps never darken. |
| `PointAndSay/PointerSampler.cs` | 60 Hz dispatcher timer: `GetCursorPos` (virtual-screen physical px, PerMonitorV2) + `GetAsyncKeyState(VK_LBUTTON)` for `annotate = mouse`; keeps the samples for `screen_context`. |
| `PointAndSay/Monitor.cs` | `ScreenMonitor.All()`: `DisplayArea.FindAll()` (bounds, work area) + `Win32Interop.GetMonitorFromDisplayId` → `GetMonitorInfoW` (device name) + `GetDpiForMonitor` (scale). |
| `PointAndSay/Gdi.cs` | Hand-declared `UpdateLayeredWindow · GetDC · ReleaseDC · ShowWindow · GetMonitorInfoW · MonitorFrom* · GetDpiForMonitor · CreateCompatibleDC · DeleteDC · CreateDIBSection · SelectObject · DeleteObject` (W2's CsWin32 list untouched). |
| `PointAndSay/PointProbe.cs` | `--point-probe`: one simulated press / 1.5 s hold / release through the real path 3 s after start. |
| `WW3Install.cs` + one marked block in `App.xaml.cs` | Installs the island controller and the point controller after W4, before `Store.Start()`; disposes them first at quit. |

Gate helper: `ww3-gate.ps1` (run through the build service's `script` action): `crash` · `log` · `grep` · `island` · `hold-voice [ms]` ·
`hold-draw <name> [ms-after]` (the key held while the pointer loops, a screenshot, the release) · `tap-voice` · `drag x0 y0 x1 y1` (a slow
SendInput drag) · `dblclick` · `esc` · `move` · `shot <name> [x y w h]` · `b64 <name>` (a PNG back through `GET /log`) · `engine-log` ·
`settings-json`.

## 2. How it was verified

### Mac (dotnet 10, `Theuth.Companion.Portable.slnf`)
- Core tests: **228 passed, 0 failed** on the merged tree (WW1's 161 + WW3's 67: PointMachine 17 (the Rust 16 + the constants test),
  IslandModel 7 + MeetingReducer 9 (`IslandTests.cs`), MeetingWire 2, PointSettings 2, and the H3 mirror of the Mac W3's names —
  `MeetingReducerMirrorTests` 21 + `IslandModelMirrorTests` 8 (`MirrorTests.cs`)), 0 warnings (`TreatWarningsAsErrors`).
- Rpc tests unchanged (WW1's 43).

### build-B (Windows 11, 1920 × 1080 at 1.0 scale tonight — the "WinDisc" 1024 × 768 fact was not what the box showed), slot `ww3`
- `dotnet build Theuth.Companion.slnx -c Debug -p:Platform=x64`: **0 warnings, 0 errors** at a1de4e6b (first full build after one fix),
  d7c42deb, 988fbd9c, 37d59af0 (31–44 s each).
- Island, driven by `--island-fixture` (a recording meeting 12:31 in, two topics, one Auto brief, two transcript lines, fed through the
  reducer exactly as the stream would — no real meeting runs on build-B without audio) + `--shareable` (so the capture-excluded windows show
  in screenshots):
  - **closed pill** at the top centre of the work area `rect=(810,6) 300x36` (log), `● 12:41 · 2 topics`, the clock ticking
    (12:41 → 12:56 → 14:09 across the round) — `evidence/ww3/island-closed.png`;
  - **open** on click: strip (grip ⋮⋮ · rec dot · clock · mute · stop · eye), topics `PROJ-12 ✕ Standup ✕`, brief `PROJ-12 — the ingest
    retry 1/1` with three bullets, `deeper · auto`, the transcript line `me: yes, PR 418 lands tomorrow`, the request box —
    `island-open.png`; the mute button flipped the fixture to `paused` / `resume` (log `island: fixture — Mute not sent`);
  - **dragged** by the grip (758,28) → (480,380): `island drag ended: kept at 462,358 on \\.\DISPLAY1 mode=Detached` —
    `island-dragged.png`;
  - **snapped back** by a double-click on the grip: `island snapped back (double-click) to 740,6 on \\.\DISPLAY1` —
    `island-snapped.png`; `settings.json` afterwards carries `islandAlwaysShow` and no `islandFrames` (the memory was cleared by the snap).
  - The exstyle/affinity of the island as shown: `0x08000188 [TOPMOST|TOOLWINDOW|NOACTIVATE]`, `affinity=0x11` (WDA_EXCLUDEFROMCAPTURE)
    without `--shareable`.
- Point-and-say, the real voice key (`ww3-gate.ps1 hold-voice` / `hold-draw`: SendInput Ctrl+Alt+Space, the 50 ms release poll) and the
  `--point-probe` (a simulated press / 1.5 s / release):
  - `WM_HOTKEY Ctrl+Alt+Space (voiceKey)` → `point: idle --Pressed--> armed fx=Start` → `overlays: 1 window(s) — \\.\DISPLAY1 bounds=0,0
    1920x1080 work=0,0 1920x1032 scale=1 primary` → `armed --Tick--> holding` at +200 ms → `dictation <id> live, mic device "Microphone
    Array (Realtek High Definition Audio)" … capture=16000 Hz/1 ch/16-bit pcm`;
  - the island shows the voice mode `● 00:03 · Ctrl+Alt+Space held · theuth's desk` while the overlay draws the trail (6 pt malachite loop
    + the 14 pt ring at the pointer) — `evidence/ww3/trail-and-voice-mode.png` (windows-mcp capture mid-hold);
  - release → `holding --Released--> sending fx=Stop` → overlays hidden, one dispatcher turn, `dictation_stop` ∥ `screen_context` →
    **`screen_context → 1 line(s), 4 file(s): [screen 1/1 · pointer: circled (480,381)–(1120,660); underlined (872,477)→(1036,472);
    underlined (1036,472)→(1114,533); rested on (1120,559) for 760 ms; ended on (1120,560) · accessibility: off]`** (108 samples over
    3166 ms); a second run `… circled (800,460)–(1120,660); rested on (1120,560) for 6763 ms …` 3 files; the probe `51 samples over
    1528 ms` 1 file;
  - nobody spoke (build-B's mic hears the room): `text 0 chars` → "Nothing recognized" → `sending --Cancel--> idle`, nothing sent. The
    send path (`desk_open` + `send_input` + the answer card + `speak`) was NOT exercised live tonight — the engine has no model and no
    speech reached the dictation; it is covered by the code path described in §1 and by the Ask bar's identical `send_input` route.
  - `accessibility: off` as expected until WE1's UIA reader lands in the engine.
- The desk lock was held by `ww3` for the round (02:14 → 02:38) and released; `quit-installed` before, `kill-mine` + `start-installed`
  after; `w5-gate.ps1 reg`: `Run\TheuthCompanion = "%LOCALAPPDATA%\theuth\companion\Theuth.Companion.exe" --background` ✔.
- Multi-display: built (one overlay per monitor from `EnumDisplayMonitors`, island memory keyed by device name, `WM_DISPLAYCHANGE` re-place)
  but verified on ONE display only (build-B has one).

### Stand-down (§3 of the brief)
`crates/desktop/src/companion/windows.rs:166-182` and `macos.rs:123-139` are the same function: on a change `ALIVE.swap`, then
`app.emit(ALIVE_EVENT, Alive { alive })` with `ALIVE_EVENT = "companion-alive"` and `Alive { alive: bool }` from `companion/mod.rs:28-36`;
the `companion_alive` command is `mod.rs:39` and registered once for both platforms in `lib.rs:314`. Names identical; nothing to message
about. The TS gates (C: `App.tsx:386-400`, `Island.tsx:70-72`, `routes/island.tsx:22-26`, `meeting-hotkeys.ts:17-22`) were read, not run
— the orchestrator's packaged recheck exercises them with the shell.

## 3. Deviations from the design
1. **Island memory lives in `settings.json`, not `companion.json`** (§7.4 #1 says companion.json). `<data_dir>\companion.json` is the
   shell-facing file W3's `LoginItem.Mirror` rewrites whole (`{ "launch_at_login": … }`); anything else put there is lost on the next
   toggle. The companion's own settings store (134 D12, 149 D19 "every value lives in CompanionSettings") keeps `islandFrames` /
   `islandAlwaysShow` / `voiceKey*` as extra keys without touching the shared `CompanionSettings` class.
2. **Overlays paint with `UpdateLayeredWindow` from a CPU raster, not Direct2D/Composition** (§7.4 #4). The window styles, the per-monitor
   physical-pixel model, the exclusion from capture and the hide-before-capture rule are as specified; the painter is a ~120-line
   signed-distance rasteriser behind `TrailRaster` because the project has no Direct2D interop and a blind D2D build from the Mac was the
   riskier path tonight. A D2D painter can replace `TrailRaster.Compose` + `OverlayWindow.Present` behind the same seams.
3. **The island's drag is a cursor poll, not XAML pointer capture.** On build-B the WinUI island activates on the press (S1) and the
   non-active window then drops `PointerMoved` / capture, so a press+move+release through XAML events read as a click. A 16 ms
   `GetCursorPos` + `GetAsyncKeyState(VK_LBUTTON)` poll moves the window and ends the drag — the "manual move" §7.4 #1 asks for.
4. **Closed pill 300 × 36 epx** (the Mac's notch-less fallback is 300 × 24, the menu-bar height). Windows has no menu bar; 36 fits the
   13 pt clock and the topic badge with the 6 epx gap under a top taskbar.
5. **The 60 Hz voice-mode re-render is throttled to once a second** (the status clock's rate); the overlay still presents every sample.
6. **Monitor enumeration is `EnumDisplayMonitors`, not `DisplayArea.FindAll()`** — the WinAppSDK call threw `InvalidCastException:
   ClassFactory cannot supply requested class` on build-B's 2.5.1 runtime.
7. **The meeting global rows (PTT Alt+`, eye Alt+Shift+E) are gated by WW1's `Hotkeys.Gate`** (its DONE 0b3de92e, merged): one
   mechanism; WW3's own re-apply was removed so the two never fight. The island only consumes `GlobalKeyPressed/Released`.
8. **`send_input` alone, no `steer` branch**: the engine's default delivery steers a busy desk (`api.rs:190-203`), so the companion does
   not track the desk's running state for this path (the Tauri shell did, from its own event slice).
9. **Shadow** = the DWM frame Windows 11 gives a top-level window (1 px light border + rounded corners), not a drawn drop shadow.

## 4. Open items
- **Settings › Island / Voice key tabs (§3.8)**: the values exist (`IslandModel.AlwaysShow`, `PointSettings.*`) and persist, but no Settings
  page edits them yet — W3's `SettingsWindow` is shared; add two pages in a polish pass (reset position per display / all, Always show,
  voice key enabled / include the screen / accessibility / annotate / speak).
- **Live send + answer card + `speak`** not exercised on build-B (no speech, no model). The code path mirrors the Ask bar's; a Mac-style
  probe that replays `TextDelta/TurnFinished` through `Store.EventReceived` for the desk session would prove the card without a model.
- **Meeting hydrate / live events** run only against the fixture tonight (no meeting on build-B); the engine-fed path (`MeetingRefetched`
  → `Hydrate`, `EventReceived` → `MeetingWire`) is covered by the Core tests over the Rust shapes.
- **Oracle chip** (`decision_mode('meeting')` every 30 s, D11) not drawn; the reducer keeps the verdicts and `OracleSummary` is ready.
- **Level meters** (`MeetingEvent::Level`) not drawn on the strip; `Store.MeetingMicLevel/SystemLevel` are there.
- **Esc as a global hotkey while a gesture is live** registers on the island's HWND; if another app already holds plain Esc the
  registration fails and the log says so (Esc then cancels only with the island focused).
- **Hover-open (0.3 s)** worked once the pill had focus-free pointer events; a synthetic `SetCursorPos` hover did not open it on build-B
  (the click did) — re-check by hand with a real mouse.
- H3 part 1 (the Mac W3 island) arrived 02:41: constants match (10 min, 40 pt, 0.3 s / 0.1 s, device name for the display UUID, off-screen
  frames rejected and logged, M/E/D/Esc, idle pill → `theuth://meetings`); its 21 + 8 test names are mirrored in `MirrorTests.cs`.
  H3 part 2 (the Mac W2 point-and-say) not yet forwarded; `PointMachineTests` already carries the 16 + 1 names of C's stub.
- WE1's UIA reader: once in the engine, the `[screen …]` line gains the element/app/window/URL; nothing changes in the companion.

## R1 (02:41 → ) — the orchestrator's four gaps

### R1.1 Meters + oracle chip (§3.6 / D11)
- `IslandWindow.Meters()`: mic (faience) and system (sand) bars, 3 epx wide, 3–14 epx tall, on the closed pill (before the
  speaker word and the rec dot) and at the head of the open strip; `SetLevels(mic, system)` resizes the two rectangles in place —
  `MeetingEvent::Level` (~6/s) never re-renders the tree (`IslandController.Apply` short-circuits on `MeetingChange.Level`).
- The pill shows `meters · ‹speaker› · ● mm:ss · N topics` (`SpeakerWord` = the newest line's `me` / `Speaker N` / `them`).
- `IslandWindow.OracleChip()` = `Island.tsx OracleChip`: the effective mode word in caps + `deciding` (gold) / `skip` / `brief` /
  `→ model` for 6 s after a verdict; hidden when the mode is off and nothing is live. `IslandController.PollOracle()` calls
  `decision_mode('meeting')` (`Client.CallAsync<JsonElement>`, `effective` as a word or the object's first key) at once and every
  30 s while a meeting is on the island (not in fixture mode).
- `--island-fixture` now also pulses Level at 6/s and seeds an oracle verdict (`shadow` · `brief`).

### R1.2 Answer card (§3.7, D14, Rev 1 #10)
- `TurnStarted` → "theuth is working…", `ToolCallStarted{name}` → "theuth is working… · ‹name›", `TextDelta` streams into the
  card, `TurnFinished` → "done" + `speak(desk, text)` when the setting is on (skipped in replay), `Error` → the message for 4 s.
- "Open Ctrl+O" button in the card footer and Ctrl+O in the voice mode → `theuth://c/<desk>` (`VoiceView.OpenUrl`).
- Refusal hints (`PointFlow.RefusalHint`) show as the voice mode's hint line for 2 s.
- `--point-replay`: 4 s after start `PointController.ReplayAsync` puts the machine in Sending for `replay-desk`, shows the voice
  mode and feeds a recorded `TurnStarted · ToolCallStarted/Finished(screen_context) · TextDelta ×3 · TurnFinished` through
  `Store.HandleEvent` (350 ms apart) — the same path the engine's stream takes (`EventReceived`).

### R1.3 Settings pages (§3.8)
- `Settings/SettingsWindow.WW3.cs` (+ three marked lines in W3's `SettingsWindow.cs`: the page keys, the nav items, the switch) —
  **Island**: Always show · Hidden from capture (eye default, `islandHiddenFromCapture`, applied to the island and the overlays) ·
  the PTT/eye chords as read from the key table ("change them in Keys") · Reset position per monitor (`settings-island-reset-<n>`,
  enabled when a frame is saved) · Reset all. **Voice key**: the chord line · on/off · Send the screen · Read what the pointer
  touched (Auto — the engine's UI Automation reader / Off) · Draw the trail (always / while the mouse button is held) · Speak the
  answer · Reset hints · the 52 §8 disclosure (`settings-voice-disclosure`).
- One backing store: `SettingsModel.Store` (one new internal property) is handed to the island/point settings so no flush drops
  another page's keys; `WW3Install.Current` lets the pages edit the live controller.
- DPI note for WE1: `app.manifest` declares `dpiAwareness PerMonitorV2`, so `PointerSampler`'s `GetCursorPos` values are physical
  virtual-screen pixels — the engine's pointer space; no conversion.

### R1 gates (desk held by ww3 03:19 → 03:28, released; installed companion restarted; Run key = installed exe)
- build-B Debug 0/0 at ffe8f121 (and cb6429e3 minus the dropped edits); Mac Core **236 passed / 0 failed**.
- **Meters + oracle chip**: `--island-fixture --shareable` → the open strip shows the two meters, `● 12:46`, the `SHADOW` oracle chip
  (`r1-island-open-meters-oracle.png`); the closed pill `▌▌ me · ● 13:04 · 2 topics` (`r1-island-pill-meters.png`). `decision_mode`
  is not called in fixture mode (no engine mode on build-B); the poll path logs `island: decision_mode failed: …` on an engine that
  refuses it.
- **Answer card**: `--point-replay --shareable` → log `point-replay: 7 events for replay-desk` → `point: desk turn finished (end_turn) —
  128 chars` → `sending --PanelDone--> replying` → 6 s later `replying --PanelDone--> idle fx=Conceal`. Screenshot mid-stream:
  `● theuth is working… / what is this at the top of my screen / [That's the theuth island — the meeting pill showing a 12-minute
  recording with two topics. Click it to see the brief on PROJ-12.] working… · Open Ctrl+O · Esc hides` (`r1-answer-card-working.png`).
  The finished frame ("done · Open Ctrl+O") lived 03:23:37.8–03:23:43.8 per the log; my three capture attempts landed outside that
  6 s window — the state transitions are in the log, the text in the working frame is already the complete reply.
- **Settings pages**: `--settings` → Island (`r1-settings-island.png`: Always show, Hidden from capture, PTT/eye chords line, per-monitor
  Reset position, Reset all) and Voice key (`r1-settings-voice.png`: chord line, on/off, Send the screen, Read what the pointer touched
  = "Auto — the engine's UI Automation reader", Draw the trail, Speak the answer, the disclosure below the fold). The nav items are
  reached through UIA (`settings-island`, `settings-voice`); synthetic mouse clicks on the NavigationView did not select (same for W3's
  pages — WinUI needs the activation click first).
- **Round-trip**: toggling `settings-island-always` → `settings.json` carries `"islandAlwaysShow": true, "islandHiddenFromCapture": true`
  (read back with `ww3-gate settings-json`); the next launch showed the idle pill `● no meeting · 0 topics` (Always show on).
- **Hover-open recheck**: with windows-mcp `mouse_control move` (SendInput → real `WM_MOUSEMOVE`) the pill opened after 0.3 s and closed
  0.1 s after the pointer left — the earlier miss was `SetCursorPos`, which moves the cursor without a mouse message to the window.
- DPI (WE1's note): `app.manifest` is PerMonitorV2 → `GetCursorPos` is physical; the samples the engine stamped (`circled (480,381)–
  (1120,660)` on the 1920 × 1080 display) confirm it.

## Revision history
- v1 — 02:50: WW1 DONE 0b3de92e merged (Hotkeys.Gate replaces WW3's meeting-row gating; MeetingEvent is one tolerant record, enums string-backed — the island reads the raw stream through MeetingWire, unaffected).
- v0 — 2026-10-09 01:40–02:40: Core + app written, WW1 skeleton (2c965119) and C (0280c0a7) merged, three build-B rounds, the UI round
  with evidence under `~/.theuth/briefs/2026-10-09/149-windows/evidence/ww3/`.
