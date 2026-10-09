# 149 W2 — as built (native point-and-say: the voice key, overlays on every display, screen context, the answer card, the spoken reply)

Branch `feat/149-w2-point`, worktree `~/Develop/theuth-149-w2`, off the contract commit `0280c0a7` (`integrate/149`).
Contract: `docs/design/149-companion-native.md` §3.7, §4 (D12 D13 D14 D17 D20 D22), §5.4 D, §6, §8, §10, §11 W2, Revision 1 rows 10–11;
design 96 (`UI/lib/voice-key.ts`, `DESK/point/{machine,mod}.rs`) as the behavioural spec; `docs/design/149-as-built-C.md` for the seams.
`COMP` = `crates/desktop/companion/Sources`, `UI` = `crates/desktop/ui/src`, `DESK` = `crates/desktop/src`.

## What exists (path:line)

### CompanionCore (pure, tested)
- `COMP/CompanionCore/PointMachine.swift:55-102` — `step` is the one-to-one port of `DESK/point/machine.rs:69-144` (same arms, same
  order; `elapsed` is `saturating_sub`); the C shapes and constants untouched. `PointSlice` as C left it.
- `COMP/CompanionCore/OverlayModel.swift:17-23, 33-59` — `OverlayPath` = one contiguous pen-down RUN on one display (local points,
  top-left origin) + the ring when the newest sample is on that run's display; a pen-up sample or a display change ends a run, so a
  display may own several; a sample on no display (a seam) is skipped; a pen-up newest sample on a display with no run yet gets an
  empty run carrying the ring. **Deviation:** C's `Identifiable { id = display }` was dropped — with several runs per display the id
  duplicated; nothing used it.
- Tests `Tests/CompanionCoreTests/PointMachineTests.swift` — the 16 behavioural tests of `machine.rs:158-467` ported one-to-one (same
  states, times, effects, loops) + C's `constantsMatchTheShell` = **17 enabled**; `OverlayModelTests.swift` — the 5 named tests
  (split by display incl. the `x < maxX` edge, local points incl. a display at a negative origin, ring on the newest only and moving
  back, pen-up breaks + ring follows a pen-up pointer, no display / seam skipped). `test.sh`: **176 tests in 28 suites passed, 53
  skipped** (the W1/W3 stubs; was 74) — `/tmp/149-W2-test.log`.

### Companion (AppKit/SwiftUI) — `COMP/Companion/PointAndSay/` (new) + `COMP/Companion/Island/VoiceModeView.swift` (new)
- `PointController.swift`
  - `:7-12` `KeyboardShortcuts.Name.voiceKey` (no library default — the chord is the table's `voiceKey` row) and `.pointEscape`
    (⎋, `enable`d on `Start`, `disable`d on every entry into `idle`/`replying` from an active state: `after(_:_:)` `:184-195`, the
    shell's `mod.rs:342-353` rule).
  - `:16-23` `AppDelegate.installPointAndSay()` (the reserved line `AppDelegate.swift:37`), `:25-28` `Store.pointObserve(_:)`
    (`Store.swift:108`, one added line after W3's reserved comment), `:30-37` `Flags.probePoint`.
  - `:43-487` `PointController`: `send(_:)` `:127-137` steps `PointMachine` on `PointClock` (ms since boot, `DispatchTime`) with
    `refused = "talking"` while `meeting_status` says live (refreshed on connect, on every meeting event and after each gesture;
    `store.meeting?.isLive` once W3 seeds it); `run(_:now:)` `:143-182` executes the effects: `start` → `OverlayWindows.reveal`,
    `PointerSampler.start`, the pill, ⎋ on, `dictation_prewarm` once, `dictation_start(16000)`; `showLock` → the pill's "locked";
    `stop` → `stopSequence` `:229-267`; `discard` → `teardownGesture` `:216-227` + the hint; `conceal` → card/pill away, speech
    stopped; `refuse` → the hint (`hint(for:)` `:204-208`: `talking` → "Meeting running — use the island to ask", `already
    dictating` → "dictating elsewhere").
  - Stop sequence `:229-267` (§5.4 D 4–6): sampler off → **every overlay `orderOut` + one awaited run-loop turn** (`runLoopTurn`
    `:280-282`, D13) → the recorder's tail chunk → `dictation_stop` ∥ (`finalText()` `:370-378` waits `DictationFinished`, cap
    `finalTextMs` 8 s = 96's `FINAL_TEXT_MS`) and `screen_context({samples, color: "#4a8a6e", since_ms, accessibility})`
    `:284-297` (skipped when "Include the screen" is off; a failure → the "Screenshots off — sent without the screen" hint and no
    files) → empty text → "Nothing recognized", nothing sent → `store.resolveDesk()` (the Ask bar's `mesh_status` + `desk_open
    ("theuth")` cache; off the hub its refusal names the hub, nothing sent) → body `[voice note, N s] ‹text›` + the `lines`
    (`noteLine` `:269-271` = `voice-note.ts:5-6`) → `steer(desk, body, clientId, files, nil)` when `SessionSummary.running` for the
    desk, else `send_input(desk, body, files, nil, clientId)` → `AskRouter.sent(on:)`, the card, `panelDone` → `replying`.
  - Dictation `:300-383`: the Ask bar's path (`AskBarPanel.swift:247-292`) with its OWN `AskDictation` reducer and in-order call
    queue — the companion's PCM (`VoiceRecorder`, D17 / Revision 1 row 11); a refused start (`talking`, `already dictating`, no mic)
    cancels the gesture with the mapped hint (`refuse` `:344-353`).
  - Events `:406-434`: `Dictation*` for its own id (live words → the pill + `Store.point.text`), `Speech*` → `SpeechPlayer`,
    `TextDelta`/`TurnFinished`/`Error` on the desk → `AskRouter.observe` → the card; `ToolCallPending/Started` → "working · ‹tool›".
    Reply over → `speak(desk, text)` when "Speak the answer" is on `:455-469`, `doneMs` 6 s after the speech ends (or at once with
    speech off) → `panelDone` → `idle` + `conceal`. ⎋ on the card or a new press → `stop_speaking(id)` + the player stopped.
  - `VoiceKeySettings` `:489-504` — `voiceKeyEnabled/IncludeScreen/Accessibility/AnnotateMouse/Speak` in `Settings.defaults` (D19;
    96's `{enabled true, includeScreen true, accessibility auto, annotate always}` + speak on, as 96 shipped: a voice send always
    entered voice mode, `voice-key.ts:358`). `PointLog` `:512-521` → `/tmp/149-W2-probe.log` during the probe.
- `OverlayWindows.swift` — one borderless non-activating `NSPanel` per `NSScreen` (`displayID` key), `.popUpMenu`, `canJoinAllSpaces
  | fullScreenAuxiliary | stationary | ignoresCycle`, `ignoresMouseEvents` unless `annotate = mouse`, `sharingType .none` (best effort;
  `.readOnly` under `--shareable`), rebuilt per gesture from the current screen set; `OverlayView` (flipped, layer-backed): one
  `CAShapeLayer` per run — `OverlayModel.strokeWidth` 6, round caps/joins, alpha 0.85, `Palette.malachite` — and the 14 pt ring
  (2 pt, the engine's `RING_PT`/`RING_WIDTH_PT`). `conceal()` orders every one out.
- `PointerSampler.swift` — 60 Hz `Timer` in `.common`; `NSEvent.mouseLocation` → the engine's logical global point (top-left origin,
  `flipped`), `displays()` = `NSScreen.frame`s flipped against the primary's top edge; `pressedMouseButtons & 1` is the pen in mouse
  mode; `maxSamples` = 60 × 120 (`mod.rs:42`); `PointClock`.
- `AnswerCard.swift` — `GlassPanel.make` (the Ask bar's glass, `.darkAqua`, `.popUpMenu`) hosting `AnswerCardView` (header
  "theuth · ‹status›" + the speaker glyph while speaking + `esc` cap; the question in sand; the reply text; "O open the desk · ⎋
  dismiss"); takes key without activating (`KeyPanel`, the Ask bar's rule) when the reply starts so ⎋/O work at once, hands key back
  to the previous app on dismiss; `O`/⌘O → `theuth://c/<desk>`.
- `Island/VoiceModeView.swift` — `VoiceModeState` + `VoiceModeView` (rec dot pulsing with `DictationLevel`, finals in ink + the open
  partial in sand between quotes, "· ⌃S held · theuth's desk" / "· locked · tap ⌃S to send" / "· sending…" / a hint); `VoicePill`
  hosts it in a dark capsule near the pointer (follows it each tick; a hint lingers `hintMs` 2 s = 96's `HINT_MS`). W3 hosts the
  same view in the island window when it merges; the pill is the interim host (the contract shipped no stub island window).
- `SpeechPlayer.swift` — see "Spoken reply" below.
- `VoiceKeyTab.swift` — Settings › Voice key (`SettingsWindow.swift:63`, the reserved line): enabled, the `voiceKey` row's recorder
  (`KeyRow`, the Keys tab's collision policy), include the screen, read what the pointer touched, draw the trail always / while the
  mouse button is held, speak the answer, the engine-grant disclosure. `@AppStorage` on `Settings.defaults`.
- `PointProbe.swift` — `--probe-point N --shareable` (below).
- Shared one-liners: `AppDelegate.swift:37`, `Store.swift:108`, `SettingsWindow.swift:63`, `SettingsStore.swift:66` (`anyProbe ||
  probePoint != nil` so a probe run is `AskFlags.automated`: no hotkey, no mic, nothing key), `scripts/bundle-companion.sh:64` (the
  microphone usage string now names the voice key and the agents-row mic).

### Spoken reply (Revision 1 row 10) — a correction to the ruling
`speak(session, text)` is a **client-sink** utterance (`api.rs:5914-5932`: `voice::Sink::Client`): the engine answers with the id and
streams one Opus packet per 80 ms as `SpeechAudio` events; **nothing plays on the engine host's speaker** (that would be
`Sink::Device`, which no RPC exposes — `talk_start` refuses it for remote peers, `api.rs:6025`). The Tauri webview plays only the
utterances it requested (`speech.ts:474-483`, `mine`). So the companion plays them itself: `SpeechPlayer` claims the next
`SpeechStarted` on the desk with `sink == client` (the webview's `talkSession` rule — the id reply can land after the first packet),
decodes each packet with AudioToolbox's Opus decoder (`kAudioFormatOpus` @ 48 kHz mono, variable frames per packet,
`AVAudioConverter` over a one-packet `AVAudioCompressedBuffer`) and schedules the PCM on an `AVAudioPlayerNode`; `SpeechFinished` +
the last buffer's callback end it; `stop()` on ⎋ / a new press. The decode path is verified on real libopus packets: see "Gates".

## `--probe-point` (§10) — what it does and what it saw (`/tmp/149-W2-probe.log`, PNGs `/tmp/149-W2-probe-*.png`)
Run as the in-bundle executable (`open -n` dedupes against Sky's running login-item companion of the same bundle id and starts
nothing): `"…/Theuth Companion.app/Contents/MacOS/Companion" --probe-point 22 --shareable`; driver `/tmp/149-W2-probe.sh`
(screenshots at 4.5 s and 8.5 s). Sky's screen: clamshell, ONE external display (2560×1440), black full-screen Preview — the overlays
and panels sit above it, no Preview change was needed.
1. Overlays revealed on every display (1 here, frame `(0,0,2560,1440)`), the pill beside the pointer, a 3-turn spiral animated at
   60 Hz over 2 s (120 samples) — `/tmp/149-W2-probe-overlays.png` (the spiral, 6 pt malachite, the 14 pt ring at its end, the pill
   "“probe — do not act” · ⌃S held · theuth's desk").
2. **Control** `screen_context` with the overlays UP: the engine stamps the request's path in the request's colour
   (`context.rs:25-29`), so the request carries a 12-sample segment at (15 %, 85 %) of each display in carnelian and the detector
   samples the returned overview PNG along the SPIRAL (green ≥ 55, > red + 25, > blue + 5): **120/120** malachite.
3. The Stop sequence: `dictation_start` → 2 s of a 440 Hz tone at 16 kHz in 100 ms chunks through `dictation_audio` → every overlay
   ordered out + one run-loop turn (`revealed=false`) → `dictation_stop` ∥ a REAL `screen_context` → **0/120** malachite along the
   spiral → `ASSERT … PASS`; the engine's line read `[screen 1/1 "Preview" · "black.jpg" · … · pointer: ended on scrollarea ·
   accessibility: on]` (the engine's AX grant is on). `DictationFinished` text `""` for a tone, as expected.
4. The answer card shown (not key) with a canned text — `/tmp/149-W2-probe-card.png`.
5. **Gesture phase** through the real driver with `dryRun` (stops before `desk_open` even if the model hears words in a tone):
   `pressed → armed [start]` → `tick → holding` at +200 ms → after 400 ms 24 samples (60 Hz over the real pointer), pill following
   → tone → `released → sending [stop(release)]` → `screen_context` → "Nothing recognized" → `cancel → idle`; overlays, pill and card
   all away, `Store.point` nil. Nothing sent; `desk_open` never called; the microphone never opened; `frontmost=Zscaler`, `active=false`.
Settings › Voice key: `--probe-settings voiceKey --shareable` → `/tmp/149-W2-settings.png` (the tab as described, not key).

## Gates
- `BUILD_LABEL=149-W2 build-lock bash crates/desktop/companion/test.sh` ✔ 176 passed / 53 skipped (`/tmp/149-W2-test.log`).
- `BUILD_LABEL=149-W2 build-lock swift build -c release --package-path crates/desktop/companion` ✔ (`/tmp/149-W2-release.log`, the
  pre-existing `NSEvent`/Sendable warnings only).
- Opus round trip (`/tmp/149-W2-opus.swift`, the player's exact decode path on REAL libopus packets — `ffmpeg -c:a libopus
  -frame_duration 80` from a 24 kHz 440 Hz tone, read back with `AudioFileReadPacketData`): with `mFramesPerPacket 0` ✔ 13 packets →
  49 800 frames at 48 kHz, rms 0.347 (the tone's 0.354), 0 failures; with a fixed 3840 AudioToolbox answers NO converter — the player
  uses 0. (AudioToolbox has no Opus *encoder* here: `AVAudioConverter(pcm → opus)` is nil and `afconvert -d opus` fails with `'pck?'`;
  decoding is what the companion needs.)
- `bash scripts/bundle-companion.sh` ✔ `valid on disk · satisfies its Designated Requirement` (`/tmp/149-W2-bundle.log`).
- `--probe-point 22 --shareable` ✔ control 120/120 → stop 0/120, PASS (above); `--probe-settings voiceKey` ✔. No permission dialog
  in any run; nothing sent; nothing took key.

## Live check 6 (the orchestrator's; `~/.theuth/engine.json`, Sky's engine)
1. Quit the stock companion; run the bundle plainly (`open -n` works when no other instance runs, else the in-bundle executable).
   Expect `Log`: `launch … probe=no`; in the main app's console `voice_key_bind("")` on the `companion-alive` edge (C's gate).
2. In another app, hold ⌃S ≥ 1 s while moving the pointer: the trail + ring on THAT display, the pill with the live words beside the
   pointer; release → the pill says "sending…", then the answer card near the pointer: "theuth · working…" → tool names → the reply
   text; the reply is read aloud (the engine's voice through the companion); the desk's transcript in the main window shows the
   `[voice note, N s] …` line and the `[screen 1/1 …]` card. ⎋ on the card dismisses it and stops the speech; O opens `theuth://c/<desk>`.
3. Tap ⌃S twice quickly → "locked · tap ⌃S to send"; speak; tap once → sent. A tap shorter than 200 ms alone → after 300 ms the hint
   "Hold to talk · double-tap to lock". A hold under 800 ms → the same hint. ⎋ mid-hold → the hint, nothing sent.
4. Start a meeting from the main window, then ⌃S → the pill: "Meeting running — use the island to ask", no overlay.
5. Off the hub (a replica engine without theuth's desk): ⌃S + speak → "theuth's desk lives on ‹hub› — open theuth there", nothing sent.
6. Quit the companion → the main app re-binds ⌃S (Tauri's `useVoiceKeyBinding`), the Tauri overlay path serves again.

## Deviations / open items
- **`Hotkeys.names` not extended.** The contract asked each wave to add its `KeyboardShortcuts.Name` to `Hotkeys.names`; for the voice
  key that launch probe (`Hotkeys.start`, `canRegister`) would run while Tauri STILL holds ⌃S (it stands down only after
  `companion-alive`, D22) and persist the row as unassigned — the very bug C's `names` guard avoided. The controller registers the
  table's chord itself (`applyHotkey`, re-applied on every table change and on the Settings toggle); the recorder's collision policy
  for the row is the Keys tab's, unchanged. A probe run registers nothing (a Carbon hot key would swallow ⌃S for every app).
- **The spoken reply plays in the companion**, not on "the engine host's device sink" (Revision 1 row 10 names a sink that the
  `speak` RPC does not have); the Opus player above. Not heard live (no mic/speaker in an automated run); the codec path is proven by
  the round trip and the event plumbing by code — live check 6 step 2 is where it is heard.
- **Stroke 6 pt** (the design's §3.7 and C's `OverlayModel.strokeWidth`), not the brief's "4 pt" (the mock's SVG).
- **`VoiceMode` host:** no stub island window exists in the contract, so `VoicePill` hosts `VoiceModeView` beside the pointer; W3 can
  host the same `VoiceModeState`/`VoiceModeView` in the island and drop the pill.
- **The answer card takes key** (non-activating, like the Ask bar) when the reply starts, so ⎋/O work without a click; the previous app
  is re-activated on dismiss. If that steals a keystroke in practice, make it key only on hover/click.
- **One display here**: "overlays N displays" was exercised with N = 1 (Sky's clamshell + one external); the per-screen code path is
  the same loop (`OverlayWindows.reveal`), the multi-display split is unit-tested in `OverlayModelTests`.
- `AskRpc.swift` needed no change: C put `screenContext`/`sendInput(files:)`/`speak`/`stopSpeaking` on `RpcClient` already.
- The `pointAndSay` KeyTable scope (⎋ fixed) is honoured by the global ⎋ registration between press and send and by the card's ⎋;
  `KeyGrammar.intent` is not consulted for it (the row is fixed and has no rebinding).
