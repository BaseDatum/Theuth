# 149 as-built — voice-key gesture invariants, Windows (fix/149w-voicekey)

Branch `fix/149w-voicekey` off `origin/main 189432c5`; worker `w149-fix`, 2026-10-09 10:03–10:20. The Windows twin of the Mac's
`docs/design/149-as-built-voice-key-fix.md`: after Sky's 09:45 report (the key "flips between dictating and not dictating, records
nothing, draws nothing", then "dictating elsewhere"), both companions must hold four invariants. Only
`crates/desktop/companion-windows/` changed.

## The invariants and where each holds on Windows

**(a) one physical press = one gesture.** `Hotkeys.Register` adds `MOD_NOREPEAT`, so a held chord raises one `WM_HOTKEY`; the release is
the 50 ms `GetAsyncKeyState` poll. A refused start ends the gesture for the rest of that hold: `DictationLease.StartFailed` closes the
lease and `PointMachine` gets `Cancel` once; nothing can re-arm until the key is released and pressed again.

**(b) every abort/finish path stops the engine dictation it started, including a start still in flight.**
- The GAP the orchestrator read on main (send path): a hold released before `store.Dictation.StartAsync()` returned made `FinalText()`
  see `dictationId == null` and return the live text without stopping anything; nothing bumped `gesture`, so when the start returned it
  was kept with no owner — the Mac's stuck "dictating elsewhere".
- Fix: a Core **`DictationLease`** (`src/Theuth.Companion.Core/DictationLease.cs`, new) owns the one dictation per gesture:
  `Begin()` → generation (Starting) · `Started(g, id)` → `Keep` while that generation is open, `Stop` when the lease was closed meanwhile or
  the answer is from an older press · `StartFailed(g)` → true only for the open lease · `Close()` → the id to stop now (null when nothing
  is live yet; a late `Started` then answers `Stop`), idempotent.
- `PointController`: `Start()` = `lease.Begin()` + keeps the start task in `starting`; `StartDictation` resolves through the lease on the
  UI thread (`OnDispatcher`, awaited, so `starting` covers the lease update) — a `Stop` verdict logs
  `point: dictation <id> started after the gesture ended — stopping it` and calls `dictation_stop` + `recorder.Cancel()`.
  **Send path** `FinalText()`: if the lease is still Starting, wait for `starting` bounded by `PointFlow.StartWaitMs = 1500`; then
  `lease.Close()` → stop that id (and wait for the final text as before), or nothing (the late start stops itself). `Discard()`
  (cancel, Esc, short, double-tap-missed, refusal, mic failure, disconnect) and `Dispose()` → `lease.Close()` → stop.
- New abort paths wired to the same Discard: **mic failure** mid-hold (`recorder.OnFailed` and a failed `recorder.StartAsync` → `Cancel`)
  and **engine disconnect** (`Store.Connection` leaves `Open` while the machine is not Idle → hint + `Cancel`; the stop is best effort,
  the engine's side died with the connection).
- `dictationId` now stays set until the final text lands (the first live round showed the `DictationFinished` filter dropping it:
  8 s "final text not flushed"; fixed in 73802ba7 — stop → text in 0.5 s again).

**(c) mic start/stop idempotent.** `VoiceRecorder.Stop()`/`Cancel()` already null-check in `TearDown`; `Start()` now tears down a capture
still open from an earlier gesture before taking the new one (no leaked WASAPI client).

**(d) a permanent companion log.** Windows had one (`%LOCALAPPDATA%\theuth\companion\logs\companion-<date>.log`). Added the proof line:
~1.2 s after every gesture end the controller asks the engine `dictation_status` and writes
`point: dictation_status after <path> → active=none` (or `active=<id> (LEAK …)`).

Also fixed while auditing: `(Sending, Cancel)` → Idle has no effect of its own, so after every "Nothing recognized" the Esc hotkey
(`RegisterHotKey(Esc)` on the island window) stayed registered **system-wide** until the next gesture — `Step()` now unregisters Esc
whenever the machine lands on Idle.

## Files
- new: `src/Theuth.Companion.Core/DictationLease.cs`, `tests/Theuth.Companion.Core.Tests/DictationLeaseTests.cs` (6)
- changed: `src/Theuth.Companion.Core/PointSettings.cs` (`PointFlow.StartWaitMs`), `src/Theuth.Companion/PointAndSay/PointController.cs`,
  `src/Theuth.Companion/Voice/VoiceRecorder.cs`

## Gates
- Mac: Core **289/289** (283 + 6 lease tests), Rpc **68/68**; 0 warnings.
- build-B Debug `Theuth.Companion.slnx` x64 @ 73802ba7: **0 errors / 0 warnings** (41 s, job 20261009T091157-358), while `theuth-pkg` packaged
  189432c5 alongside.

## Live on build-B (Debug 73802ba7 `--background --shareable`; shell up; desk 10:09–10:17)
Every gesture end was followed by the audit line with `active=none`:
```
10:13:13.792 point: idle --Pressed--> armed fx=Start · 10:13:14.165 dictation 01a11ff0-2394-… live, mic "Microphone Array (Realtek …)"
10:13:16.859 holding --Released--> sending fx=Stop · 10:13:17.384 stop (release) — 100 samples over 3002 ms, text 0 chars, screen lines=1 files=3
10:13:17.388 sending --Cancel--> idle
10:13:18.598 point: dictation_status after stop (nothing recognized) → active=none            ← hold-and-release
10:13:47.407 idle --Pressed--> armed · 10:13:47.508 armed --Released--> tap1 · 10:13:47.665 dictation 01a11ff0-a6b7-… live
10:13:47.822 tap1 --Tick--> idle fx=Discard · 10:13:47.849 discard (double-tap-missed)
10:13:49.059 point: dictation_status after discard (double-tap-missed) → active=none          ← abort after the start returned (Discard path = Esc's path)
10:15:50.671 dictation_start → 01a11ff2-880d-… (Ask bar hold-to-talk, Ctrl+Shift+Space held)
10:15:51.929 WM_HOTKEY Ctrl+Alt+Space (voiceKey) · 10:15:51.936 point: refused — already dictating   ← refused start: no gesture, nothing started
```
Not reproducible live: (1) **Esc mid-hold** — with Ctrl+Alt still down the keystroke is the chord Ctrl+Alt+Esc, which the plain-Esc
hotkey does not match (same for a person: Esc mid-hold works in the Locked/hands-free state and in Sending); the Esc path is the same
`Discard` that the double-tap-missed abort exercised. (2) **a release before `dictation_start` returns** — the engine answers in
~150–260 ms here, faster than any injected release; that race is the lease test `ALateStartAfterTheGestureEndedIsStopped` and the
`Stop` branch in `StartDictation`. (3) **an engine-level `already dictating` refusal** — needs a second RPC client dictating (the shell's
Ctrl+D; its window is hidden on build-B); covered by `ARefusedStartCancelsOnlyTheOpenLease` and the `StartFailed` branch.
Round closed: kill-mine → start-installed (pid 17532) → `Run\TheuthCompanion = "…\companion\Theuth.Companion.exe" --background` → desk released.

## Deviations / notes
- Esc while the chord is held is not a plain Esc on Windows (modifiers down) — fixed in §R3 below.
- `StartWaitMs` 1.5 s is only reached when the engine is slow; the normal start returns in < 300 ms and the send path stops the id directly.

## §R3 — "Esc cancels" during the hold (orchestrator R3, 10:19)

While the voice key is held the user's Esc reaches Windows as *the chord's modifiers*+Esc (Ctrl+Alt+Esc for the default), which the
plain-Esc hotkey never matched. Now:
- Core `EscDuringHold` (`src/Theuth.Companion.Core/EscDuringHold.cs`, new): `Modifiers(voiceKey)` derives the modifiers to register Esc
  with from the bound chord (a rebound key works), drops Win (the OS's), and answers null for a bare chord and for exactly **Ctrl+Shift**
  — Ctrl+Shift+Esc is Task Manager and is never registered; `SkipReason` gives the log line. Tests `EscDuringHoldTests` (4): default →
  Ctrl+Alt, Ctrl+Shift skipped / Ctrl+Shift+Alt registered, bare/null → nothing, Win dropped.
- `PointController.RegisterEsc` registers a second hotkey `EscHeldHotkeyId = 0x014A` with those modifiers (MOD_NOREPEAT) wherever plain Esc
  is registered (Start, StopAndSend's Sending re-register); `UnregisterEsc` drops both wherever plain Esc went (Discard, Conceal, Dispose and
  the Idle-landing in `Step`). `WM_HOTKEY` for either id → the same `PointEvent.Esc`. A skipped combination is logged once.
- Gates: Mac Core **293/293**, Rpc 68/68; build-B Debug x64 @ 902ca850 **0/0** (46 s, job 20261009T092108-369).
- Live on build-B (Debug 902ca850, hold Ctrl+Alt+Space → Esc key_down/key_up while still holding → release):
  ```
  10:22:41.161 point: idle --Pressed--> armed fx=Start · 10:22:41.660 dictation 01a11ff8-cbe4-… live, mic "Microphone Array …"
  10:22:43.117 point: holding --Esc--> idle fx=Discard
  10:22:43.147 point: discard (esc)
  10:22:44.353 point: dictation_status after discard (esc) → active=none
  ```
  A plain Esc pressed afterwards while idle produced no `point:` line (both Esc hotkeys unregistered on the Idle landing).
- Restored: kill-mine → start-installed (pid 11168) → Run value = the installed exe `--background` → desk released; theuth-pkg and the release clone untouched.
