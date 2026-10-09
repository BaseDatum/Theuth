# 149 — as built: the Windows island fix (Sky 2026-10-09 15:50 · 16:06 · 16:12)

Branch `fix/149w-island` (cut from origin/main ece44049; main cfe7eb13 + 6e395770 folded in), basedatum only. Worker: theuth
subagent `w149w-island`. The Windows twin of the macOS fix on `fix/149-island` (its as-built, `149-as-built-island-fix.md`, was
not on basedatum/main while this was built; parity is with the rulings as relayed).

Sky, 15:50: *"none of the clicking options in the meeting window work … I can't click through different briefs, I can't click
the eye icon, I can't reposition the meeting window on an external monitor … I don't think that the original one collapsed like
this where I had to mouse over it to get it to expand again. stopping the meeting with control M doesn't close the meeting
window. after recording is finished there's no need for the meeting window at all."* Addenda: §E eye tint (16:06), §N J/K/↑/↓
walk the briefs (16:12).

## What changed

| Ruling | Windows before (main ece44049, measured on build-B) | Now |
|---|---|---|
| D1 open while live | A live meeting showed the 300×36 closed pill; hover 0.3 s opened it (a synthetic SendInput move did **not** open it on build-B), 100 ms after PointerExited closed it unless "pinned" by a click; dragging the pill turned it open-sized (`IslandMode.Detached` meant both "moved" and "open"). | `IslandModel.Present(meeting, state)`: Starting · Recording · Paused · Stopping → **Open** (strip + pane) at once, unless THIS meeting was collapsed by an explicit click (strip ˄ `island-collapse`, or Esc on the island) — remembered per meeting id; a new meeting opens again; a click on the collapsed pill opens it and forgets the collapse. No hover/leave timers; `PointerLeft()` never collapses. `Detached` is a separate bool. The closed pill otherwise only exists as the idle wordmark ("Always show"). |
| D2 visibility, no linger | `ShouldShow` = live ∨ a topic/brief within **10 min** ∨ Always show (+ a hide timer). After Done the island stayed (proved: fixture Stop → Done, still shown 2 s later, `fix-before-06`). | `ShouldShow(state)` = live ∨ Always show. Done · Failed · Post · Idle → `HideIsland()`: an 180 ms ease-in fade (`IslandModel.CloseAnimation`, ≤ 300 ms; skipped when Windows' Animation effects are off), then the window is hidden. Linger + hide timer removed. New: if the engine connection stays closed `EngineLostGrace` = 3 s while live, the meeting is presented as `Failed` (gone); a reconnect hydrates the real state back. |
| D3 every control clickable | No region/HTTRANSPARENT on the island (un-regioned NOACTIVATE tool window, shape = HWND size), so the Mac's stale pass-through rect has no Windows twin. The Windows causes found: (a) **every snapshot change rebuilt every control** (`body.Children.Clear()`), and a live meeting changes on every partial utterance — a press landing between a rebuild loses its Click, and the request box lost focus and text; (b) a click on the NOACTIVATE island does **not** make it foreground on build-B: typing into the request box went to the app behind it (Notepad), so the box, Enter, Ctrl+M and any island key were unreachable; (c) a mid-drag `WM_DPICHANGED`/`WM_DISPLAYCHANGE` called `Relayout()`, which re-placed the window on the *old* monitor's memory — a drag onto a monitor of another scale was yanked back; (d) the degradation badges ran under the mute/stop/eye buttons. | (a) `RenderKey()` — only a structural change rebuilds; a partial utterance updates the transcript row (and the clock) in place; a rebuild that lands while the left button is down over the island waits for the release (`renderRetry`, 40 ms); the request box keeps its draft and focus across a rebuild. (b) Every press (handledEventsToo) calls `TakeForeground()` (`SetForegroundWindow`, remembering the window it took it from); a click on the strip/pane focuses a focusable host (`island-focus`, a ContentControl around the content) not the box; Esc / a drag hands the foreground back. (c) Mid-drag display changes keep the window under the cursor at the new scale (`IslandModel.ScaleGrab`) and never re-place; the release picks the monitor and its memory; a scale change re-measures. (d) Badges + the activity line moved to their own row under the strip. Plus: the send arrow beside the box (`island-send`), one `island: click <id>` log line per control, and a topic pill whose brief exists walks to it. |
| D4 keys | Ctrl+M (island scope) only in a non-box focus, which (b) made unreachable. | Unchanged bindings. Ctrl+M now also works from the request box (the one place the island keeps focus); Esc's "close" is now the explicit collapse. §N adds J/↓ next · K/↑ previous (wrapping) — two **new** key-table rows (`IslandNextBrief` J, `IslandPreviousBrief` K; arrows fixed); in the box J/K type and ↑/↓ walk only while the box is empty. |
| §E eye tint | Plain glyph. | Visible in capture: View `E890` tinted `SystemFillColorCautionBrush`, "Visible in screen sharing — viewers can see the island"; hidden: Hide `ED1A` tinted `SystemFillColorSuccessBrush`, "Hidden from screen sharing (best effort)". Glyph tint only; resolved live per theme. Same helpers (`IslandWindow.EyeGlyph/EyeTint/EyeLabel`) drive the strip's eye and a new state row under Settings › Island's eye toggle (`settings-island-eye-state`). |

Also fixed on the way: the clock froze during talk (`DispatcherQueueTimer.Start()` on a running timer restarts its interval and
the transcript updates arrive faster than 1 s) — started only when stopped, and refreshed with every in-place update.

## Files (this branch's own)

- `src/Theuth.Companion.Core/IslandModel.cs` — `IslandMode {Closed, Open}` + `Detached`; `ShouldShow(state)`; `Present` ·
  `Collapse` · `Expand` · `PointerLeft` · `IsCollapsed` · `Meeting`; `CloseAnimation`; `EngineLostGrace` + `EffectiveState`;
  `ScaleGrab`. `PendingWindow`, `HideDue`, `Open/Close/Pinned` removed.
- `src/Theuth.Companion.Core/MeetingReducer.cs` — `ShowAt`, `CycleIndex` (wrapping), `TopicBriefIndex` (one brief-walk path).
- `src/Theuth.Companion.Core/KeyBindings.cs` — two additive rows (island J / K). `NativeLook.cs` — `Glyph.ChevronUp` (E70E).
- `src/Theuth.Companion/Island/IslandWindow.cs`, `IslandController.cs` — as above; `IslandAction.Cycle`.
- `src/Theuth.Companion/Settings/SettingsWindow.WW3.cs` — the eye state row (additive).
- Probe tooling: `tools/Theuth.Companion.Seed` `seed meeting start|pause|resume|stop|status` (a REAL local meeting on a model-less
  engine: all system audio, consent given); `--island-fixture` now has 3 briefs, talks (a partial utterance every 350 ms, the
  churn a real meeting makes) and simulates `meeting_stop` (Stopping → Done after 500 ms); `ww3-gate.ps1` `click x y [ms]`
  (human-paced, 120 ms hold), `key <name>`, `ctrl-m`, `type <text>`.
- Tests: `IslandTests.cs` (+7 island-fix facts: shown iff live / no linger / close ≤ 300 ms; open in every live state; pointer
  leave never collapses; explicit collapse per meeting; per-display memory across a drag between monitors incl. a moved monitor
  and a per-display reset; grab offset across DPI; engine-gone grace; brief walk wraps + pills walk to their brief), `MirrorTests.cs`
  (linger test inverted: `HiddenAtOnceWhenDone`; `OpenWhileLiveCollapsedOnlyByAClick`), `WW1Tests.cs` (island key row list).

## Gates

- Mac: `dotnet test tests/Theuth.Companion.Core.Tests` **309 passed, 0 failed**; `tests/Theuth.Companion.Rpc.Tests` **68 passed**.
- build-B Debug x64 `Theuth.Companion.slnx` at 286fd4b5 (and the final head): **Build succeeded, 0 Warning(s), 0 Error(s)**
  (TreatWarningsAsErrors).

## Proofs on build-B (one display, 1920×1080, Win 11 25H2; companion log lines verbatim, local time)

Before = main's island code (cb74778c: probe tooling only). After = 17d5cc03 / 286fd4b5. Clicks are human-paced SendInput clicks
(`ww3-gate click`, 120 ms hold) made while the fixture talks every 350 ms; keys are SendInput key down/up.

1. **Open on start, no hover.** Before: real meeting → `island shown: rect=(810,6) 300x36 … mode=Closed` (`fix-before-07`).
   After: `16:13:01.634 island: presented state=Recording meeting=fixture-meeting mode=Open` → `island shown: rect=(740,6)
   440x218 … mode=Open`; real engine meeting `16:29:08.716 presented state=Starting … mode=Open` → `island shown: rect=(740,6)
   440x99 … mode=Open` (`fix-after-01`, `-20`, `-31`).
2. **Pointer away does not collapse.** SendInput move over the island and away, 1.5 s later still `[740, 6, 440, 219]`
   (`fix-after-02`).
3. **Every control, Recording and Paused** (one log line each): `click island-topic-2 (state=Recording …)` → `brief 2/3 via pill
   t2`; `click island-topic-1` → `brief 3/3 via pill t1`; `click island-older` → `brief 2/3 via older`; `click island-eye` →
   `eye → hidden from capture` / `→ visible in capture`; `click island-mute` → `presented state=Paused`; in Paused: `click
   island-topic-1 (state=Paused …)` → `brief 3/3`; `click island-topic-2-dismiss (state=Paused …)`; `click island-deeper
   (state=Paused …)`; `click island-collapse (state=Paused …)` → `collapsed by click`; pill click → `expanded by click`; `click
   island-mute (state=Paused …)` → Recording; box: `key Enter island-request`, `click island-send`; real engine: `click
   island-mute (state=Recording …)` (that call failed: the engine had died, see Findings). §N keys after a click on the strip
   (`took the foreground for keys (ok)`): J `brief 1/3` (wrapped from 3/3) · J `2/3` · K `1/3` · ↓ `2/3` · ↑ `1/3` · K `3/3`
   (wrapped from 1/3); J in the box typed "j" (`fix-after-12`) and ↓ in the non-empty box walked nothing.
4. **Explicit collapse is per meeting.** Fixture: collapse → pill (`fix-after-07`, `-13` by Esc, `-16` light); click → open.
   Real engine m1 `01a1214c…`: `click island-collapse` → pill (`fix-after-32`), stop → `presented state=Stopping … mode=Closed
   (collapsed for this meeting)` (it stayed collapsed through Stopping), then m2 `01a1214e…` started **open**
   (`fix-after-33`). Earlier real meeting: collapsed, engine `meeting_resume`, still collapsed (`fix-after-22`).
5. **Stop → gone within ~1 s.** Fixture Ctrl+M from the box: `16:24:36.728 key Ctrl+M → stop (from the request box)` →
   `.729 presented state=Stopping` → `37.253 presented state=Done … mode=Closed` → `.259 closing (180 ms fade)` → `37.467 island
   hidden` = **739 ms** (500 ms of it is the fixture's simulated drain). Real engine stop: `16:34:54.570 presented
   state=Stopping` → `54.976 presented state=Post` → `54.980 closing (180 ms fade)` → `55.185 island hidden` = **615 ms**.
   Real Ctrl+M with the engine dying at the stop: `16:35:59.946 key Ctrl+M → stop` → `16:36:01.963 meeting_stop failed:
   connection closed` → `16:36:05.021 engine connection closed for 3.1 s while Recording — the meeting is presented as ended`
   → `05.217 island hidden` (5.3 s; before the engine-lost rule the island stayed on "Stopping" forever, `fix-mid-23`).
6. **No linger after Done.** Before: fixture Stop → `fixture state → Done` at 16:02:09.164, the island still shown at
   `[740, 6, 440, 254]` 2 s later (`fix-before-06`). After: hidden 214 ms after Done (above).
7. **Drag, snap-back, double-click reset** (timed SendInput drag, 40 steps): `island drag ended: kept at 440,506 on
   \\.\DISPLAY1 mode=Open detached=True frames=1`; drag back near the default → `snapped back at 740,6 … detached=False
   frames=0`; drag away, double-click the grip → `island snapped back (double-click) to 740,6` (`fix-after-10`). Second monitor:
   **cannot be shown live — build-B has one display**; covered by `PlacementMemoryIsPerDisplayAcrossADragBetweenMonitors` and
   `GrabOffsetScalesWhenADragCrossesIntoADifferentDpi`. No drivers installed.

Light + dark: open `fix-after-01` (dark) / `-14` (light); paused `-05` / `-15`; collapsed `-07` / `-16`; eye visible (Caution)
in every open shot. Eye hidden: the island is excluded from capture even under `--shareable` once the eye is toggled, so the
hidden state is shown by its tooltip "Hidden from screen sharing (best effort) — Alt+Shift+E" (`fix-after-17`, light) and the
blank capture (`fix-after-04`). The Settings › Island state row could not be screenshotted: a Windows "Let's get you signed in"
(Microsoft account) dialog sat in front of the Settings window and did not close (`fix-after-30`); the row uses the same three
helpers as the strip.

## Deviations / what cannot hold as written on Windows

- **No ⋮ menu** on the Windows island: pause/resume and stop are the strip's own buttons (`island-mute`, `island-stop`), as
  before; both proven clickable. The eye tint rule is applied to the strip's eye and the Settings row (there is no menu row).
- **Ctrl+M needs keyboard focus on the island.** It is an island-scope key (D4 unchanged), so it works once the island has
  been clicked (strip, pane or box); it is not a global hotkey on Windows.
- **The island now takes the foreground on a press** (it is `WS_EX_NOACTIVATE`). This is what makes the box, Enter, Ctrl+M and
  J/K reach it on build-B (before, typed text went to the window behind). Esc and a drag hand the foreground back.
- **Engine-lost rule** (new, not in the rulings): a live meeting whose engine stays unreachable 3 s is ended on the island.
- **build-B only one display**: second-monitor drag and per-display memory are Core-tested, not shown.

## Findings outside the island (not fixed here)

- **The installed engine (2.14.0 / 480f2cb5) on build-B dies during a model-less meeting**, twice at `meeting_stop` and once
  ~10 s into recording; its log ends with `WARN meeting::diar: meeting: live diarization failed: WebGPU validation failed.
  [CommandEncoder (unlabeled)] is already finished.` and no panic line. The theuth-desktop shell does not restart it; each time
  the desktop was killed and relaunched (`w5-gate launch-task`) to restore the box.
- build-B's system accent is a dark indigo (`AccentFillColorDefaultBrush [Dark] = #FF2C06AB`, `AccentTextFillColorPrimaryBrush
  [Dark] = #FF3408CD`), so accent-text badges are low-contrast in dark mode there (`fix-after-31`); the island follows the
  system accent by rule (native look), so this is the box's accent, not a fixed colour.
- Before the fix, the island's first frame after `ShowPassive` was black until the first pointer message (`fix-before-01`);
  after the fix (render before placement/show) it painted on show in every round.
- Test keystrokes (j, k, arrows) landed in an open, unsaved Notepad on build-B during the before/foreground diagnosis; it was
  not saved.

## §R1 — legibility (orchestrator review 16:41)

1. **Text on an accent fill.** The collapsed pill's topic count already used `TextOnAccentFillColorPrimaryBrush`; the defect is
   that Windows derives it per theme (black in dark, white in light), and on build-B's very dark indigo accent the dark-theme
   pair is `fill=#2C06AB on=#000000` = 1.7:1 (logged: `island: text on accent fill — Dark fg=#FF000000 fill=#2C06AB on=#000000
   readable=False`). Now `OnAccentText()` keeps the system on-accent colour when it reaches WCAG 4.5:1 on the fill
   (`NativeLook.OnAccentReadable`) and otherwise uses white or black, whichever contrasts more (`NativeLook.BestTextOn`).
   Measured trap: asking the *other* theme's dictionary for `TextOnAccentFillColorPrimary` in a running app returns the
   *current* theme's colour (`#000000` in dark), so the fallback is computed, not looked up. The only filled accent element on
   the island is the topic count; "N new", the oracle verdict, listening/approval and the badge labels are accent *text* on a
   neutral background (`AccentTextFillColorPrimary`, unchanged), fills stay `AccentFillColorDefault`. Core test
   `TextOnAnAccentFillStaysReadable`. Evidence: `fix-r1-02-collapsed-dark` (white "2 topics" on the indigo fill),
   `fix-r1-03-collapsed-light`.
   Observation, not changed (the R1 rule keeps it): accent *text* on the dark neutral background is also low-contrast with this
   accent (`AccentTextFillColorPrimaryBrush [Dark] = #FF3408CD`, the badge labels in `fix-r1-01-open-badges-dark`); the same
   contrast guard could be applied there if wanted.
2. **Whole badge labels.** The badges left the strip (286fd4b5) for their own rows, two per row, each label whole; a label over
   196 epx ends in an ellipsis with the full text as tooltip and UIA name — never a hard cut. The fixture now carries the two
   build-B degradations (`MicNoAec`, `ModelDown`). Evidence: `fix-r1-01-open-badges-dark`, `fix-r1-04-open-badges-light`
   ("no echo cancel · model down" whole).

## §P — the voice pill stays in the island area (Sky 16:44)

**What it was on Windows.** Nothing followed the cursor: point-and-say's live text and answer card are the island window's
voice mode (`IslandController.SetVoice` → `IslandWindow.RenderVoice`), shown where the island is placed — the island's saved
per-display offset when it had been dragged, else top-centre — on the monitor the island last used (not necessarily the one
under the pointer). With a meeting live, the voice mode REPLACED the meeting content. Only the trail/ring overlays (one layered
window per monitor) follow the pointer.

**Now.** At gesture start with no meeting, the pill is placed top-centre of the display under the pointer
(`IslandModel.DefaultFrame`, the island's own anchor code), ignoring any saved offset, and stays there for the whole gesture
(`voiceAnchor`; a display change during the gesture re-places to the same top-centre). With a meeting live, the island keeps its
place and the voice pane renders directly below the meeting strip (`RenderStrip()` + `RenderVoice`). When the gesture ends the
island goes back to its own place ("Always show") or hides. One log line per voice phase with the pill's rect after it is shown.

**Proof (build-B, `ww3-gate hold-sweep`: Ctrl+Alt+Space held while SendInput moves sweep the pointer across the screen).**
- No meeting: `16:57:30.059 island: voice pill phase=listening rect=(740,6) 440x64 on \\.\DISPLAY1 (top centre, fixed for the
  gesture)` → `16:57:33.172 phase=sending rect=(740,6) 440x64`; screenshots `fix-p-11-voice-start` / `-12-voice-mid` (trail
  drawn far from the pill) / `-13-voice-end`: the pill never moves.
- Meeting live (fixture): `16:54:25.566 island: voice pill phase=listening rect=(740,6) 440x91 … (below the meeting strip)` →
  `16:54:28.408 phase=sending rect=(740,6) 440x91`; `fix-p-01/02/03-meeting-voice-*`: strip + badges, the pill under them, the
  trail following the pointer.
- The answer card could not be shown: the engine has no model, the desk turn ends in "Nothing recognized"; it renders in the
  same voice pane, so it inherits the placement.

## Gates (final)

Mac Core.Tests 310/310 · Rpc.Tests 68/68 · build-B Debug x64 slnx on the final head: 0 warnings, 0 errors.

Also on build-B during these rounds: a Windows "t.exe – Application Error" dialog (access violation at 0x7FFD2941D325, from one
of the crashed engine processes) and a "Let's get you signed in" Microsoft-account dialog were left on screen; the running
engine (3 × t.exe since 16:36) and the installed companion are healthy, Run value restored.
