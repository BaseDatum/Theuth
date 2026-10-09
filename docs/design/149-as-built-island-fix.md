# 149i as-built — the meeting island: clicks, drag, open-by-default, close when the meeting ends

Branch `fix/149-island`, worktree `~/Develop/theuth-149i`, off `origin/main` ece44049. Swift only
(`crates/desktop/companion/**` + this file). Every build/test under `env BUILD_LABEL=149i build-lock`. The owner's running
companion, desktop app and engine were never touched; synthetic input was posted only at the probe's own island window.

## The report (owner, 2026-10-09 15:50)
> "seems like none of the clicking options in the meeting window work. as in, I can't click through different briefs, I can't
> click the eye icon, I can't reposition the meeting window on an external monitor like I should be able to. also I don't think
> that the original one collapsed like this where I had to mouse over it to get it to expand again. stopping the meeting with
> control M doesn't close the meeting window. after recording is finished there's no need for the meeting window at all."

Clamshell, one external display without a notch, macOS 26.6.2.

## Rulings (decided in the brief)
- **D1** Open while live, no hover gating: Starting/Recording/Paused/Stopping → the strip + brief pane at once, like the Tauri
  original. The hover-open/hover-close model is deleted. The closed pill exists only as the idle wordmark ("Always show", nothing
  live) or when the user clicks the strip/pill to collapse; collapsed is remembered for the current meeting only.
- **D2** Visibility = the original's rule (`Island.tsx:73`): shown iff live, or "Always show". No `linger`. Done/Failed/Idle →
  close animation (≤ 300 ms) → `orderOut`; ⌃M closes it within ~1 s; the engine's post-meeting work never shows an island.
- **D3** Every control clickable in every state on every display; the hit-test covers everything visible; the window-level click
  handler never swallows or pre-empts; the panel takes key only for the request box; the grip drags/detaches anywhere, snaps back
  within `snapBack`, remembers per display UUID, double-click resets.
- **D4** Keys unchanged.

## What was wrong (main 6d151b8a / ece44049)
1. **Every control was click-through** — `Island/IslandWindow.swift:60-64` `IslandHostingView.hitTest` converted the point into
   the hosting view's own coordinates (`convert(point, from: superview)`) and tested it against `drawn`, which
   `IslandPanel.swift:212` set to `drawnAppKit` = the SwiftUI frame **flipped into AppKit bottom-left**. `NSHostingView.isFlipped`
   is `true` (probe: `hosting.isFlipped=true`), so a top-left point was tested against a bottom-left rect: nothing drawn at the
   top of the oversized 720 × 560 window was ever inside `drawn`; only the empty mirror region 300 pt below it was. The BEFORE
   probe logs `hitTest=nil (click-through)` at the centre of every control (topics, eye, ⋮, deeper, older/newer, transcript,
   box, grip) — `/tmp/149i-probe-before.log`, 1/14 PASS (the one pass is a vacuous `newer` on a cursor already at the top).
2. **The hover model** — `IslandPanel.swift:217-234` `pollHover`: open after 0.3 s inside `drawnGlobal`, close 100 ms after the
   pointer left unless `pinned`. `ClosedPill.onTapGesture` pinned (and could not be clicked anyway, see 1). This is the
   "collapsed … had to mouse over it to get it to expand again".
3. **The linger** — `CompanionCore/IslandModel.swift:25,35-43` `visible` kept the island `linger = 10 * 60` s after the last
   topic/brief when the meeting was Done. `MeetingEvent.decode` and `MeetingReducer.reduce(.state)` were honest all along: the BEFORE
   `stop` probe fed `{"type":"State","meeting":…,"state":"Done"}` through `Store.meetingObserve` → `slice.state=done`
   (PASS) and the island stayed visible (FAIL) — so the decode needed no change; the linger did.
4. **The grip NSView never got the press** — `IslandViews.swift:181` the 18 pt grip was `Color.clear…contentShape(Rectangle())`
   with `DragGrip` as its `.background`; the detached handle was a `Capsule…contentShape` over a `.background(DragGrip)`. SwiftUI's
   hit-testable content in front of an `NSViewRepresentable` takes the hit, so `hitTest` answered `IslandHostingView`, not
   `GripView`, and `mouseDown` → `performDrag` never ran (probe: drag `moved=0.0 dragPoll=false`).
5. **A grip click on the open pane would have detached it** — `IslandModel.dragEnded` measured the pane's *centre* against the
   notch's centre; the open pane's centre is ~150 pt below the notch, beyond `snapBack` = 40, so a press-and-release with no
   movement would have become `.detached` in place.
6. **The window-level double-click** — `IslandWindow.sendEvent` :46 called `onDoubleClick` for any double-click inside `drawn`
   except the box: a quick double-click on a topic would have snapped the island back (pre-empting the control).
7. **Key on any click** — `wantsKeyOnClick` :71 made the panel key for any click inside `drawn`; `KeyBindings.swift:98` says the
   island takes key only in its request box.

## The fixes
- `CompanionCore/IslandModel.swift` — `linger` and `lastActivity` gone. `live(_:)` (Starting/Recording/Paused/Stopping),
  `visible(_:alwaysShow:) = alwaysShow || live` (:35-44). `attached(live:collapsed:)` → `.open` / `.closed` (:47).
  `collapsed` + `toggleCollapsed(meeting:)` / `meetingChanged(to:)` — forgotten when the meeting id changes (:50-56).
  `dragEnded` compares **top-centres** (`frame.midX/maxY` vs the anchor's): the attached content, open or closed, hangs from the
  anchor's top-centre, so a click never detaches and a drag ≤ 40 pt still snaps back (:61).
- `Island/IslandPanel.swift` — the hover timers, `hoverSince/hoverLeftAt`, `pollHover`, `ui.pinned/hover` deleted.
  `meetingChanged` → `model.meetingChanged(to:)` → `refreshVisibility()` (D2) → `applyLayout()` (:97-120: attached →
  `IslandModel.attached(live:collapsed:)`; detached untouched). `toggleCollapsed()` (:123) from the live pill's tap and from a
  grip press-and-release with no movement (`dragEnded` :251, `dragStart` recorded in `beginDrag`). `hide()` (:142): 0.3 s alpha
  fade via `NSAnimationContext`, then `orderOut` + alpha back to 1 (nothing stays on screen at alpha 0). `show()` resets the
  alpha, restores/defaults the frame, applies the layout. The 1 s tick now only advances `ui.now` for the clock.
  `uiChanged` (:226): `hosting.drawn = ui.isDetached ? nil : ui.drawn` — the SwiftUI global frame **as reported**, since the
  hosting view is flipped. `wantsKeyOnClick` (:73): true only inside the request-box row (`ui.rects["box"]`). `ui.onGrip` (:70):
  click count 2 → `snapBack()`, else `beginDrag`. `snapBack` → `.closed` then `applyLayout()` (open again while live).
  `escape()` no longer closes the pane (D1 has no hover close); it leaves the box / undoes a stop hold / closes the ⋮ menu and
  hands key back, as before. Settings › Island caption no longer promises 10 minutes.
- `Island/IslandWindow.swift` — `onDoubleClick` removed; `sendEvent` only makes key where the content asks and always calls
  `super`. `IslandHostingView.drawn` documented as the view's own (flipped) coordinates.
- `Island/IslandViews.swift` — `IslandUI`: `pinned`, `hover`, `boxRect`, `onDragStart` gone; `rects: [String: CGRect]` (not
  published) filled by `.islandRect(name, ui)` on every control; `onGrip`, `onCollapseToggle`. The strip's 18 pt grip is the
  `DragGrip` NSView itself; the detached handle's `DragGrip` is an `.overlay`. `GripView.mouseDown` forwards every click count.
  `ClosedPill` tap while live → `ui.onCollapseToggle`. `OpenIsland.onHover` gone.
- `Island/Store+Meeting.swift` — `meetingAct` records the suppressed label in `Store.meetingActs` under a probe; `--probe-log
  <path>` chooses the probe log file.
- `Island/IslandProbe.swift` (new) — the `clicks` / `stop` / `open-no-hover` scenarios (`-nonotch` suffix forces the fallback
  pill anchor, the owner's topology).

### Verified, not assumed
- **Buttons fire on the first click without key**: the AFTER log shows `key=false` throughout the topic/eye/⋮/stop/older/newer/
  deeper/dismiss/transcript clicks (all PASS) and `key=true` only after the box click; ⎋ releases it.
- **`MeetingEvent` decode**: `State{Done}` through the real decoder → `.done` → hidden within 1 s (`stop` PASS 3/3).

## Probe evidence
Probe: `--probe-island <clicks|clicks-nonotch|stop|open-no-hover> --shareable --probe-log <file>`; the fixture meeting (recording
12:31, two topics, two briefs for the walk, three lines) on this Mac's only display (LG, 2560 × 1440, no notch — a 200 × 37 notch is
pretended unless `-nonotch`). `AXIsProcessTrusted=true` for the probe (debug executable and the in-worktree bundle alike), so
CGEvents were posted at `.cghidEventTap`; every target rect is logged (`probe rect <name>: swiftui=… global=… centre=… hitTest=…`)
before the post; the eye default and the frame memory the probe flips are restored before it quits; the pointer is put back.

| | BEFORE `/tmp/149i-probe-before.log` | AFTER `/tmp/149i-probe-after.log` |
|---|---|---|
| open-no-hover (pointer parked at the far corner, 2 s) | FAIL `state=closed drawn=(1050, 1403, 460, 37)` (pill) | PASS `state=open drawn=(1000, 1198, 560, 242)` |
| hitTest at every control's centre | `nil (click-through)` × 11 | `IslandHostingView` (SwiftUI content) / `GripView` at the grips |
| topic click (t2) | FAIL `tapped=[] acts=[cursor]` | PASS `tapped=["t2"] focusedTopic=1 acts=[…, meeting_tap(t2)]` |
| eye click | FAIL `false → false` | PASS `false → true` (`sharingType` stays 1 under `--shareable`, as designed) |
| ⋮ → menu → stop | FAIL `menuOpen=false`, stop never drawn | PASS `menuOpen=true` → `meeting_stop` suppressed+logged, menu closed |
| older / newer (walks the briefs) | FAIL cursor stuck at 1 / vacuous | PASS cursor 1 → 0 → 1 |
| deeper | FAIL | PASS `meeting_deeper(b2)` |
| hover t1 → dismiss x | FAIL | PASS `meeting_dismiss(t1)` |
| transcript | FAIL | PASS `transcriptOpen=true` |
| strip click collapses → pill click opens | n/a (pinned forcing) | PASS / PASS |
| box takes key → ⎋ releases | FAIL `boxFocused=false`; ⎋ closed the pane | PASS `key=true boxFocused=true` → `key=false boxFocused=false state=open` |
| grip drag (down, 10 × 20 px, up) | FAIL `moved=0.0 dragPoll=false` | PASS `moved=200.0 state=detached` frame remembered for the display UUID |
| grip double-click | FAIL (no detached grip) | PASS `state=open frames=[:]` back at the screen top |
| **total** | **1/14** | **16/16** (`clicks`, twice from the bundle), **16/16** (`clicks-nonotch`, 3 consecutive bundle runs; one earlier run 15/16 — the hover flake below, before the retrying sweep) |
| `stop`: Done through the decoder → hidden ≤ 1 s | decoded PASS, hidden FAIL `visible=true` | 3/3 PASS `visible=false shown=false` |

PNGs (window captures of the probe's own window): `/tmp/149i-clicks-start.png` (open, pointer far away), `-menu.png`
(⋮ open: pause · stop · ×), `-collapsed.png` (the pill after the strip click), `-detached.png` (200 px down, detached layout),
`-snapped.png`; `/tmp/149i-clicks-nonotch-*.png` the same on the fallback anchor; `/tmp/149i-stop-start.png` /
`-after.png` (the latter is `screencapture`'s empty result of a window that is no longer on screen); `/tmp/149i-open-no-hover-start.png`;
the fixture probes `/tmp/149i-island-closed.png` (collapsed pill over the pretended notch), `-open.png` (the strip + pane at once),
`-detached.png`, `-nonotch.png` (open under the menu bar on the notchless display).

## Gates (all `env BUILD_LABEL=149i build-lock …`)
- `bash crates/desktop/companion/test.sh` — **192 tests in 30 suites passed** (`/tmp/149i-test.log`; 190 + `IslandModelTests`
  reshaped: `visibleWhileLive`, `visibleWhenAlwaysShow`, `hiddenTheMomentTheMeetingIsNotLive`,
  `attachedLayoutIsOpenWhileLiveUnlessCollapsed`, `collapseIsRememberedForTheCurrentMeetingOnly`,
  `aGripClickOnTheOpenPaneSnapsBackNotDetaches`; the drag/snap-back/UUID/off-screen tests unchanged).
- `swift build -c release --package-path crates/desktop/companion` — Build complete (inside the bundle script).
- `bash scripts/bundle-companion.sh` + `codesign --verify --deep --strict "target/release/companion/Theuth Companion.app"` —
  `valid on disk · satisfies its Designated Requirement` (`/tmp/149i-bundle.log`, 2.14.0).
- hex grep (`0x……`, `"#……"`, `NSColor(hex`/`Color(hex`) over `Sources` → empty; emoji grep over `Text("…")` and the island files →
  empty. Native look holds: SF Symbols, system colours, materials; the notch pill alone is `Color.black`.

## Deviations
- The probe writes to the shared `Settings.defaults` suite (the owner's) for the eye default and the frame memory while it runs —
  as the W3 probes did — and restores both before quitting (`settings restored (eyeHidden=… frames=…)` in the log).
- The synthetic hover for the topic's dismiss × is a 9-step pointer sweep retried up to three times (`probe hover topic:t1: no
  dismiss after sweep N` logs a retry); a single jump, and once even a single sweep, flipped SwiftUI's `onHover` only sometimes
  (probe-only flakiness of the tracking area after the ⋮ menu re-created the topic buttons; a real mouse wiggles — the E key
  dismisses too).
- The probe posts `mouseMoved` to park the pointer away from the island and to restore it afterwards; those two moves are not at
  the probe's window (the brief asked for the park).
- `escape()` no longer closes the pane; with no hover close there is nothing for it to undo. Keys otherwise unchanged (D4).
- The `closed` fixture probe now shows the pill via the D1 collapse (`toggleCollapsed()`), since a live meeting opens by default.

## Live check (the owner, 4 steps)
1. Start a meeting. The island is open at once — strip and brief pane — without hovering; the pointer anywhere.
2. Click two topic pills (the brief changes / the topic greys while its brief arrives) and the eye (glyph flips).
3. Drag the island by its left grip onto the external display and let go: it stays there, detached; double-click the handle at
   its top: it returns to the top of the screen.
4. ⌃M to stop (or Stop in ⋮): the island fades and is gone within a second; the notes/summary afterwards show no island.
