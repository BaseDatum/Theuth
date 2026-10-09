# 149 W3 — as built (the native island: notch pill + no-notch fallback, open/detached, drag · snap-back · per-display memory, meeting flows, the eye probe)

Branch `feat/149-w3-island`, worktree `~/Develop/theuth-149-w3`, off the contract commit `0280c0a7`. Swift only.
Contract: `docs/design/149-companion-native.md` §3.6, D9–D13, §5.4 C, §8 (island + PTT/eye rows), §9 (eye), §10 (`--probe-island`,
`--probe-eye`, live check 5), §11 W3, Revision 1 rows 9, 12–14; `docs/design/149-as-built-C.md` (shapes, stubs, insertion points);
recon A §A5 (`/tmp/149/recon-A-notch.md`) as the window-stack reference. `COMP` = `crates/desktop/companion/Sources`.

## What exists (path:line)

### CompanionCore (bodies for the contract's stubs; 29 tests un-disabled, all green)
- `COMP/CompanionCore/MeetingReducer.swift` — `MeetingSlice` gains `activity: String?` (`:41-42`, trailing defaulted init param; `.empty`
  unchanged) for the `TurnStarted/ToolCall*/TurnFinished` spinner the TS slice carries. `MeetingReducer.reduce` `:63-131` is
  `UI/store/meeting.ts:156-220` case for case (15 variants: `Started` resets to a recording slice; `State` for another meeting ignored,
  cues/activity end when not live, `progress` kept only in a `Post` state; `Level` → `level`; `Utterance` partial replaces the open line of
  its track, a labelled final re-publish updates in place, `linesKept 200`; `TopicAdded` replaces by id; `TopicRemoved` drops topic + tap;
  `Brief` = the brief-stack rule `:181-190` verbatim — Auto/Cache at the top shows, stepped back queues + `unseen`, Tap/Request/Deeper/
  Typed/Oracle jump and clear the tap; `Listening`/`ApprovalWaiting`; `PostProgress`; `ReviewReady`; `Summarized` keeps; `Deleted` empties;
  `OracleDeciding`; `OracleDecision` replaces its batch or appends, `oracleKept 8`, clears `deciding`). `seed` `:134-139` (`meeting_status`),
  `hydrate(briefs:lines:cursor:)` `:142-149` (cursor on `status.cursor` else the top), `move` `:152-158` (stepping up eats the badge),
  `markTapped` `:161-164`, `elapsed` `:167-170` (clock only while recording), `mmss` `:173-176`.
- `COMP/CompanionCore/IslandModel.swift` — `visible` `:35-43` (Starting/Recording/Paused/Stopping, or ≤ `linger` 10 min after the last
  topic/brief, or `alwaysShow`; Revision 1 row 13), `dragEnded` `:46-55` (centre within `snapBack` 40 pt of the notch → `.closed` and the
  saved frame cleared; else `.detached` + remembered under the display key), `frame(for:visibleFrames:)` `:58-61` (nil unless it intersects
  one). AppKit-free; frames are screen-relative `CGRect`s, keys are display UUID strings.
- Tests `Tests/CompanionCoreTests/MeetingReducerTests.swift` (21, one per variant + brief-stack + seed/hydrate + elapsed/mmss) and
  `IslandModelTests.swift` (8: live states, the 10-min linger boundary, always-show, hidden states incl. `Post`/`Review`/unknown, 39 pt and
  39.6 pt snap back, 41 pt detaches, off-screen rejected / partly-on kept, UUID-keyed frames survive a topology change and a resolution
  shrink rejects).

### Companion/Island (new directory, W3-owned)
- `COMP/Companion/Island/IslandWindow.swift`
  - `IslandWindow: NSPanel` `:9-52` — `[.borderless, .nonactivatingPanel]`, `level = .statusBar + 1` = **26** (`:10`, re-asserted in
    `show()` `:38-41`), `[.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]`, `hidesOnDeactivate = false`,
    `becomesKeyOnlyIfNeeded = true`, NOT `isFloatingPanel`, `isMovableByWindowBackground = false`, `appearance = .darkAqua`,
    `sharingType = Flags.shareable ? .readOnly : .none`; `canBecomeKey` true / `canBecomeMain` false. `sendEvent` `:45-52` = notchprompt's
    make-key-before-mouse-down (MIT, `notchprompt/OverlayWindowController.swift:16-27`), gated to clicks inside the drawn shape; double-click
    → `onDoubleClick` (snap back).
  - `IslandHostingView` `:55-66` — `acceptsFirstMouse` true (notchprompt `:51-53`), `hitTest` returns nil outside `drawn` (click-through;
    recon A §A5 point 6).
  - `IslandLayout` `:69-78` — attached window 720 × 560 (oversized, D9), open pane 560, detached 440, strip 44, pill wings 130.
  - `IslandGeometry` `:81-131` — `notch(of:)` = `auxiliaryTopLeftArea.maxX … auxiliaryTopRightArea.minX` × `safeAreaInsets.top`;
    `menuBarHeight` = `frame.maxY − visibleFrame.maxY` (the model's 24 when auto-hidden); `targetScreen` = the notched screen else
    `NSScreen.screens[0]`; `anchor(on:forceNoNotch:fakeNotch:)` = the notch, or the 300 × menu-bar pill centred under the menu bar;
    `uuid(of:)` = `CGDisplayCreateUUIDFromDisplayID(NSScreenNumber)`; `relative`/`global` conversions (frames are stored in the screen's own
    coordinates — recon A risk e).
- `COMP/Companion/Island/IslandPanel.swift` — `IslandController` (one per app, `AppDelegate.island` `:14-17`):
  `install()` `:69-91` (drag/double-click hooks, `store.$meeting`, `store.$connection == .open` → `meetingHydrate`, screen-parameter and
  resign-key observers, hotkeys, eye applied, probes); visibility `:96-111` (`IslandModel.visible` over the last-seen slice; `lastActivity`
  advances when topics/briefs grow); `show` `:113-122` (restore → place → `show()`), `hide` `:124-137` (pane closes, `orderOut` 0.45 s
  later — no alpha-0 hack); `restoreOrDefault` `:159-175` (validated frame → `.detached`, a rejected one is **logged and cleared**);
  `place` `:177-193` (attached: the oversized window flush with the screen top, centred on the anchor; detached: the remembered top-left,
  the content's own size); `screensChanged` `:195-199`; hover poll `:219-236` (10 Hz over `NSEvent.mouseLocation`: open after 0.3 s inside,
  close 100 ms after leaving unless pinned by a click or the box has focus); drag `:239-271` (`performDrag(with:)` from the grip, the button
  polled every 50 ms because `performDrag` may never deliver the mouse-up, then `model.dragEnded` → snap-back animates `setFrame` or the
  detached frame is persisted); `snapBack(all:)` `:273-281`; eye `:289-294` (`sharingType` on every companion `NSPanel`: `.none` when hidden
  and not `--shareable`, else `.readOnly`; persisted); hotkeys `:298-323` (`KeyboardShortcuts.Name.meetingPtt` ⌥` key-down/up →
  `meeting_ptt(true/false)`, `.meetingEye` ⌥⇧E toggles the eye; both **disabled unless the meeting is live/starting**); island-scope keys
  `:325-357` (table lookup `keys.action(for:in: .island)`: M pause/resume, ⌃M (= typed ⌃⇧M) stop after the undo-delay hold with "stopping in
  N s · ⎋ undo", E dismisses the hovered/first topic, D deeper on the current brief, ⏎ left to the field, ⎋ `:359-367` leaves the box →
  undoes the hold → closes the pane and hands key back to the previous app); single keys type while the box has focus. Oracle chip poll
  `:376-383` (`decision_mode('meeting')` on show + every 30 s), verdict fade 4 s; `mirrorCursor` `:390-396` (`meeting_cursor` on every cursor
  change while shown). Probes `:400-449` (below). `IslandSettings` `:452-471` — UserDefaults keys `islandAlwaysShow`, `islandEyeHidden`
  (default true), `islandFrames` `{uuid: [x,y,w,h]}` in the `io.theuth.mac.companion` suite. `IslandFixture` `:473-493`. `IslandSettingsTab`
  `:495-523` — Always show · Hide from screen sharing (best effort) · Reset position on this display / on all.
- `COMP/Companion/Island/IslandViews.swift` — `IslandUI` `:7-36` (state, anchor, pinned, hover, boxFocused, menu, transcript, eye, oracle,
  stop hold, request text, the drawn/box rects reported by the views); `IslandRoot` `:38-67` (pill/pane top-centre of the oversized window,
  `.bouncy(0.4)` open / `.smooth(0.4)` close, hangs under the menu bar on a no-notch screen); `ClosedPill` `:72-116` (meters · speaker/"paused"
  · `rec mm:ss` · "N topics" around the notch; idle = ⌸ wordmark, click → `theuth://meetings`); `OpenIsland` `:119-147` (a clear band the
  notch's height, the strip, brief pane, transcript row, request box; bottom-rounded 26 attached / 22 detached); `DetachedIsland` `:149-162`
  (grab handle); `Strip` `:165-236` (meters, clock, oracle chip, topic buttons or the ⋮ pause/resume/stop menu, degradation badges
  `Island.tsx:37-47`, "approval waiting"/"listening…", activity spinner, post progress, mute/stop key hints, eye, ⋮; the left 18 pt and the
  strip's background are `DragGrip`s); `TopicButton` `:238-265` (tapped = 50 %, ⚡ oracle / • cached, hover ×); `BriefPane` `:267-309`
  (title, ≤ 3 bullets, deeper + D hint, newer/older, `↑ N new`, `i/n`); `TranscriptRow` `:311-344` (`▸ Transcript N`, open = last six lines
  `me:`/`Speaker N:`/`them:`); `RequestBox` `:346-380` (⏎ → `meeting_request`, clears and blurs); `Meters` `:382-396`, `RecClock` `:398-412`,
  `OracleChip` `:414-432`, `IslandShape` `:463-473`, `DragGrip` `:475-483` (`mouseDown` → `performDrag`).
- `COMP/Companion/Island/Store+Meeting.swift` — `Store.meetingObserve(data)` `:11-26` (`MeetingEvent.decode` → `reduce`; `TurnStarted`/
  `ToolCallStarted`/`ToolCallFinished`/`TurnFinished` of the meeting's session → `activity`), `meetingHydrate` `:30-45` (`meeting_status`,
  then `meeting_briefs` + `meeting_live` when live), actions `:48-68` (`meetingTap` marks tapped first, `Dismiss`, `Deeper`, `Request`, `Ptt`,
  `Pause`, `Resume`, `Stop`, `Move`, `Cursor`) through `meetingAct` `:70-75` — **suppressed and logged under any probe**. `IslandLog`
  `:79-93` → `/tmp/149-W3-island.log`. `Flags.probeIsland / probeEye / probeSeconds / islandProbe` `:98-110`.

### Shared files — the reserved lines replaced, one addition each
- `COMP/Companion/Store.swift:107` — `meetingObserve(data) // 149 W3 …` (was the reserved comment).
- `COMP/Companion/AppDelegate.swift:38` — `island.install() // 149 W3`.
- `COMP/Companion/Settings/SettingsWindow.swift:62` — the Island `tabItem`.
- `COMP/Companion/Settings/Hotkeys.swift:20` — `.meetingPtt: .meetingPtt, .meetingEye: .meetingEye` appended to `names` (the launch probe
  + KeyboardShortcuts registration now cover the two rows; the controller disables them until a meeting is live).
- `COMP/Companion/Settings/SettingsStore.swift:66` — `anyProbe` gains `|| islandProbe`, so `AskFlags.automated` holds for the island probes
  (no hotkeys, no mic, no sends).
- **`COMP/TheuthKit/Models.swift:665-706` (C-owned, fixed because the island cannot hydrate without it):** the live engine's
  `meeting_status.supported` is `#[serde(tag = "kind", content = "reason")]` (`crates/meeting/src/lib.rs:211-219`) → `{"kind":"Full"}` /
  `{"kind":"Partial","reason":…}`; the contract decoded `"Full"` / `{"Partial": why}` and threw `unknown support ["kind"]` on the first
  `meeting_status` against Sky's engine (seen in `--probe 3`). `MeetingSupport` now decodes the tagged shape (the bare word and the old
  map kept as fallbacks) and encodes the tagged shape; fixtures `meeting_status_149.json:30-33` and `meeting_status_149_idle.json:7-9`
  carry the real wire. After the fix, `--probe 3` against Sky's engine: `hydrated meeting=none state=idle topics=0 briefs=0 lines=0`.

## Gates
- `BUILD_LABEL=149-W3 build-lock bash crates/desktop/companion/test.sh` — ✔ **176 tests in 28 suites passed, 45 skipped** (74 − the 29 W3
  tests now enabled; `/tmp/149-W3-test.log`; the first filtered run `/tmp/149-W3-test1.log`: 29 in 2 suites passed).
- `BUILD_LABEL=149-W3 build-lock swift build -c release --package-path crates/desktop/companion` — ✔ `/tmp/149-W3-build{1,2,3}.log`
  (`Build complete!`; the only warnings are the pre-existing `NSEvent`/`Sendable` class every panel's key monitor has).
- `bash scripts/bundle-companion.sh` — ✔ `Theuth Companion.app` 2.13.0, `codesign --verify --deep --strict` "valid on disk · satisfies its
  Designated Requirement" (`/tmp/149-W3-bundle.log`).
- No permission dialog in any run: no `AVCaptureDevice`, no notification authorization, no ScreenCaptureKit (see the eye deviation), no
  `CGEventTap`; every probe launched from the in-bundle executable, `active=false key=none` at quit, exit 0.

## Probes (`/tmp/149-W3-probe.sh <state> --shareable`, `/tmp/149-W3-eye.sh`; log `/tmp/149-W3-island.log`, stderr `/tmp/149-W3-stderr-*.log`)
The Mac is clamshell: ONE display, `LG ULTRAGEAR+` 2560 × 1440, UUID `0CE5F47F-9626-47F0-BBF3-4C3D2B5A29CC`, menu bar 30 pt, **no notch**
(`safeAreaInsets.top = 0`, auxiliary areas nil). So `closed/open/detached` run with the probe's pretended 200 × 37 notch at the top centre
(logged: "no screen has a notch — a 200 × 37 notch is pretended"), and `nonotch` forces the real fallback. Every run first saves an
off-screen frame `(3060, 100, 440, 300)` under this display's UUID and shows it rejected: `saved frame … intersects no visibleFrame
(0, 0, 2560, 1410) — rejected, notch default used`.
- **Sky's black full-screen Preview rule, confirmed with the first screenshot:** `/tmp/149-W3-island-closed-region.png` is a plain display-region
  `screencapture -R 980,0,600,420` — the pill's text/meters/clock are the only non-black pixels (`bright=186`) over the black Preview, i.e.
  **level 26 renders above a full-screen app**; no fallback to 25 was needed. Preview was never touched.
- `closed` → `/tmp/149-W3-island-closed.png` (window capture `-l 8545`): the bottom-rounded black pill 460 × 37 hugging the (pretended) notch,
  meters · "Speaker 1" · ● 12:33 · "2 topics"; log `drawn=(1100, 1403, 360, 37)` (first build; 460 wide after the wing fix),
  `level=26 sharing=1 hitTestOutside=true key=false active=false`.
- `open` → `/tmp/149-W3-island-open.png`: the 560-wide pane under the notch band — strip (meters, ● 12:33, two topic chips "SRE-326…" •cached and
  "Flipt DR ⚡", badge "headphones reco…", `M mute` · `^M stop`, eye-slash, ⋮), brief "DR task E.4 — hold every west workload at zero" with 3
  bullets, `⌕ deeper` + `D deeper` + "tap", newer/older + `1/1`, `› Transcript 3`, the request box. `drawn=(1000, 1179, 560, 261)`.
- `detached` → `/tmp/149-W3-island-detached.png` (440 × 304): the probe saves `(40, 990, 440, 360)` for the display's UUID, the log shows
  `frame restored for 0CE5F47F-…: (40.0, 990.0, 440.0, 360.0) (screen-relative)` and the window lands at `frame=(40, 1046, 440, 304)` (top-left
  held, height = the content's); grab handle, strip, brief, the transcript open (Speaker 2 / me: / Speaker 1 partial), request box. The probe
  frame is removed at quit.
- `nonotch` → `/tmp/149-W3-island-nonotch.png` + `-region.png`: `anchor=(1130, 1380, 300, 30) notch=false` — the 300 × 30 (menu-bar-height)
  capsule hanging under the menu bar, `drawn=(1130, 1380, 300, 30)` after the hang-under fix (the first build drew it at 1410, over the bar).
- Meeting flows under the probes: `probe: meeting_cursor(b1) suppressed` shows the cursor mirror firing on show; tap/deeper/request/pause/stop
  go through the same `meetingAct` gate (sent only outside probes).
- **`--probe-eye`** (`/tmp/149-W3-eye.sh`, no `--shareable`): `eye hidden=true → sharingType=0 on 4 panels`, CGWindowList reports
  `sharingState=0 layer=26 alpha=1` for window 8646. Apple's `screencapture -x -o -l 8646` → **"could not create image from window", exit 1**;
  the display-region capture of the same area → **`bright=0`** (the `--shareable` run of the same open layout has `bright=1883`). Verdict on
  **macOS 26.6.2 (25G83)**: `NSWindow.SharingType.none` still removes the island from both `screencapture` paths (per-window and display
  region). It remains labelled best effort (Apple: legacy, "don't use this to hide content"); not tested: ScreenCaptureKit consumers
  (Zoom/Meet-class) — see deviations.

## Live check 5 (the orchestrator's; exact steps)
Build/bundle as above; quit Sky's running companion only if the orchestrator owns that session (else run this bundle's executable plainly —
no probe flag — from a terminal; the real companion would double-register ⌃⇧Space, so prefer one instance).
1. In the main window start a meeting (Meetings tab). Expect: the native island appears at the top of the primary display (the real notch
   on a notched Mac; the 300 × menu-bar pill under the bar here) within a second of `Started`; the Tauri island does NOT appear
   (`companion-alive` gate, C). Stderr: `island shown state=closed anchor=… level=26`.
2. Hover the pill 0.3 s → the pane opens; move away → closes after 100 ms; click the pill → stays open (pinned); ⎋ → closes.
3. Drag the strip's left edge (or anywhere on the strip with nothing under the pointer) away from the notch and release > 40 pt from it →
   detached, stderr `drag ended at …: detached, remembered for <UUID> as …`. Quit the companion, relaunch → same place
   (`frame restored for <UUID>`). Drag it back to within 40 pt of the notch centre → animates back (`… — snap back`); double-click the grab
   handle does the same; Settings › Island › Reset also.
4. Change the display resolution (or plug/unplug a display) → `screens changed: …` and the island re-places; a frame that no longer fits is
   logged `rejected, notch default used`, never off-screen.
5. Wait for a topic → the pill says "N topics"; open → tap the chip → it greys (tapped) → `Brief{source: Tap}` arrives → the brief pane shows it;
   `deeper` → a Deeper brief stacks on top; newer/older move the cursor and `↑ N new` appears when an Auto brief lands while stepped back.
   Type in the box, ⏎ → `meeting_request` → a brief. Hold ⌥` → the strip shows "listening…" (the engine's `Listening{held}`), speak, release →
   the request becomes a brief. ⋮ → pause → the clock stops and the pill reads "paused"; resume; M does the same; ⌃⇧M → "stopping in 3 s · ⎋
   undo" → stop. ⌥⇧E toggles the eye glyph and `sharingType` on every panel.
6. On a Mac whose only display has no notch (this one): the pill is 300 × menu-bar height under the menu bar (`--probe-island nonotch` PNG).

## Deviations / open items
- **Eye probe mechanism.** §10 asks for a ScreenCaptureKit self-capture. `SCShareableContent` requires the Screen Recording grant, which this
  companion bundle does not have (130 W6 finding) and which an automated run must not prompt for. The probe therefore puts the island up with
  `.none` and lets Apple's `screencapture` (already privileged) be the capturing consumer on both the per-window and the display-region path,
  with CGWindowList's `sharingState` as the third reading. SCK-based apps are untested; the UI keeps "best effort".
- **No notched display on this Mac**: the `closed/open/detached` probes pretend a 200 × 37 notch (`IslandController.fakeNotch`, probe-only;
  logged). The real-notch geometry path (`IslandGeometry.notch(of:)`) is exercised by code, not by this hardware — live check 5 on a notched
  MacBook is the proof. The `nonotch` fallback is the real path here.
- `MeetingSlice.activity` added (trailing defaulted field) — the Windows twin's `MeetingSlice` mirror should gain it too.
- `AppDelegate`'s line reads `island.install()` as the contract reserved; the controller is reached through a one-line `AppDelegate.island`
  computed property (`IslandPanel.swift:14-17`) because an extension cannot add a stored property and the shared file must stay one line.
- Island settings live under their own UserDefaults keys in the same suite (`IslandSettings`), not as new `CompanionSettings` fields: avoids a
  second shared-file edit (`PolicySettings.swift`) while W1/W2 add theirs; D19's intent (out of the webview's localStorage) holds.
- `Hotkeys.apply` re-registers PTT/eye whenever Settings › Keys changes a row; the controller re-disables them on the next `meeting` change.
  Harmless (a registered ⌥` outside a meeting calls `meeting_ptt`, which the engine refuses when idle); a `KeyboardShortcuts.isEnabled` would
  make it exact — not in 1.10.0.
- Hover detection polls `NSEvent.mouseLocation` at 10 Hz while the island is shown (Notchmeister: tracking areas are unreliable at the notch);
  no hover-open while detached (it is always open there).
- Live check 5 not run by this wave (the orchestrator's; one running companion at a time on this machine).
- **One C-owned file touched** (`COMP/TheuthKit/Models.swift`, `MeetingSupport` only, + two fixtures): a wire-shape bug that made every
  `meeting_status` decode fail on the live engine — the island could never show. Self-contained (one enum), no other wave edits it; the
  closer should keep W3's version. The Windows twin's `Support` mirror should be checked for the same `{kind, reason}` shape.
