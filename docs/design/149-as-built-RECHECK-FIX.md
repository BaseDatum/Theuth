# 149 as-built — RECHECK-FIX (Windows twin)

Branch `fix/149w-recheck` off `origin/main 584660bf`; worker `w149-fix`, 2026-10-09 08:04–08:30. Fixes the defects the packaged
recheck of main f41067e2 found (`~/.theuth/briefs/2026-10-09/149-windows/reports/RECHECK.md`): **D1** and notes **N1–N4**.
Everything is C# in `crates/desktop/companion-windows/`; the shell (TS/Rust) is untouched.

## What changed

### D1 — voice key lost on a companion restart while the shell runs (R8 FAIL)
Mechanism (unchanged, confirmed in `crates/desktop/ui/src/App.tsx` `useVoiceKeyBinding` and `crates/desktop/src/companion/windows.rs`
`POLL = 2 s`): the shell re-binds the voice key the moment the companion's mutex vanishes and lets go only on the poll after it
sees the mutex again, so the companion's one `RegisterHotKey` at startup answered 0 and nothing retried.

Fix, in the companion only:
- **Core `HotkeyRetry`** (`src/Theuth.Companion.Core/HotkeyRetry.cs`, new): a clock-fed policy — the failed global rows
  (`HotkeyFailure(Action, Chord, Notice)`), `Interval = 1 s`, `Window = 20 s`, `Attempt(now, register)` tries every pending row
  and drops the ones that register, `NextDelay(now)` (null when done; the remainder when < 1 s is left), `GaveUp` once the window
  closes with rows pending, `Cancel()` when the table is re-applied, `Notice` = "… is held by another app — retrying for up to 20 s"
  while active, the row's original notice after giving up, null once registered.
- **`Hotkeys.Apply`** (`src/Theuth.Companion/Hotkeys.cs`): one id per row (`idOf`), failures collected into a `HotkeyRetry`, a
  one-shot `DispatcherQueueTimer` per attempt (`ScheduleRetry` → `RetryTick`); a retry that wins replaces a fallback default
  registered in the row's slot (and puts the fallback back if the saved chord then fails); `NoticeChanged` event; a new `Apply`
  (Settings › Keys, the meeting gate) cancels the running retry first, so re-applying still works and a genuine third-party holder
  is left alone after 20 s. `Surfaces` forwards `NoticeChanged` to `Store.HotkeyNotice`, which the Keys page and legend already redraw on.
- Log lines: `hotkey: retrying voiceKey=Ctrl+Alt+Space every 1 s for up to 20 s …`, `hotkey voiceKey = Ctrl+Alt+Space registered after
  retry N (x.x s after apply) id=0x137`, `hotkey: gave up after N retries — …`.
- Tests: `tests/Theuth.Companion.Core.Tests/HotkeyRetryTests.cs` (7, fake clock): wins on the third attempt; 20 attempts over 20 s
  then the original notice stands; a tick past the window gives up without trying; rows leave independently; nothing failed = nothing
  to do; cancel; the last delay fits the window.

### N1 — inbox count clipped by the screen edge
`RailPillsWindow.InboxPill`: the gold count is a bold 11 epx `TextBlock` beneath the 16 epx wordmark inside the capsule (§3.1 "the
wordmark with the gold count beneath"), `needs-you-count` UIA id kept; the corner badge and its extra blob in `ApplyShape`'s window
region are gone (the region is now exactly the four capsules).

### N2 — agents pill counted needs-you items without a session
`RailPillsModel.Derive`: a needs-you mark with `Session == null` (hire proposals, seeded or engine-level items) is skipped — it counts in
the inbox badge only; a needs-you item joined to a session is still the gold dot (folded with its agent mark). Test
`ANeedsYouItemWithoutASessionCountsInTheInboxOnly` (3 seeded items + 1 desk → inbox 3, one dot, "Agents, 1").
**Mac parity note:** `crates/desktop/companion/Sources/CompanionCore/RailPillsModel.swift` `derive` still carries *every* `rail.marks`
(needs-you first), so the Mac's agents pill shows a gold dot for a session-less item too. §3.1 says "one dot per agent"; the
Windows pill now follows §3.1 and the Mac may want the same one-line filter — not changed here (out of scope: Swift).

### N3 — dimmed mic/✕ reported `enabled=True`
`AgentsPanelWindow.IconButton(…, out FrameworkElement view)`: a non-actionable row's button is `IsEnabled = false` (UIA
`enabled=False`, name "nothing is running" / the `DisabledReason`), wrapped in a transparent `Border` (`agent-mic-N-hint` /
`agent-stop-N-hint`) that carries the tooltip and the same UIA name, because WinUI drops a disabled control's tooltip. Enabled buttons
are unchanged (tooltip on the button itself).

### N4
- **a. §3.8 escape hatch "pills vs one strip"**: `CompanionSettings.RailPills` (key `railPills`, default true, absent = pills;
  round-trip test `RailPillsEscapeHatchDefaultsOnAndRoundTrips`). Settings › Rail gets the toggle **"Separate pills"**
  (`settings-rail-pills`) with a caption. The pre-149 strip `Windows/RailWindow.cs` is restored from `b84905dd~1` (507 → 526 lines):
  the peek card is gone — a needs-you/finished mark opens the Attention panel on its item, an agent mark opens the agents panel on its
  session (`Surfaces.OnStripMark`); the `FlatButtons` class at its tail was dropped (it lives in `FlatButtons.cs`). **Applied live:**
  both windows are built at startup, one shown, the other suspended (`Suspend()` = hide + stop the 40 ms poll, `Resume()` = refresh +
  show + poll); `Surfaces.ApplySettings` flips them; `LiveRail` picks the shown one's foreground bookkeeping. The strip's
  `ToolWindow` title is "theuth rail strip" so the two are distinguishable in the window list.
- **b.** `SettingsWindow.Combo` labels wrap (`TextWrapping.Wrap`) instead of clipping, and the long option title became
  "Auto — UI Automation reader", so "Read what the pointer touched" is whole at the default 720×520.
- **c.** Island stop stays Ctrl+M (the Mac made the same call) — no change.

## Files
- new: `src/Theuth.Companion.Core/HotkeyRetry.cs`, `src/Theuth.Companion/Windows/RailWindow.cs`, `tests/Theuth.Companion.Core.Tests/HotkeyRetryTests.cs`
- changed: `src/Theuth.Companion.Core/PolicySettings.cs`, `src/Theuth.Companion.Core/RailPillsModel.cs`, `src/Theuth.Companion/Hotkeys.cs`,
  `src/Theuth.Companion/Notifications/NotificationsInstaller.cs`, `src/Theuth.Companion/Settings/SettingsWindow.cs`,
  `src/Theuth.Companion/Settings/SettingsWindow.WW3.cs`, `src/Theuth.Companion/Windows/AgentsPanelWindow.cs`,
  `src/Theuth.Companion/Windows/RailPillsWindow.cs`, `src/Theuth.Companion/Windows/Surfaces.cs`,
  `tests/Theuth.Companion.Core.Tests/PolicyTests.cs`, `tests/Theuth.Companion.Core.Tests/Ww2Tests.cs`

## Gates
- Mac: Core **274/274** (265 + 7 HotkeyRetry + 1 RailPills + 1 settings), Rpc **68/68**; 0 warnings.
- build-B Debug of `Theuth.Companion.slnx` (x64) at e9f77f19: **0 errors / 0 warnings**, 35 s (job 20261009T071809-243).

## Proofs on build-B (Debug build e9f77f19, `--background --shareable`, shell theuth-desktop 15628 up throughout)
Evidence: `~/.theuth/briefs/2026-10-09/149-windows/evidence/fix/`.

- **D1.** `w6-gate quit-installed` (pid 6688 gone 08:19:47) → ~9 s → `w6-gate launch --background --shareable` (pid 6232, 08:19:56):
  ```
  [companion 08:19:57.822 pid 6232] hotkey: Ctrl+Alt+Space is held by another app — no hotkey until you pick another
  [companion 08:19:57.823 pid 6232] hotkey: retrying voiceKey=Ctrl+Alt+Space every 1 s for up to 20 s (the shell lets go on its next poll)
  [companion 08:19:59.826 pid 6232] hotkey voiceKey = Ctrl+Alt+Space registered after retry 2 (2.0 s after apply) id=0x137
  [companion 08:20:28.552 pid 6232] WM_HOTKEY Ctrl+Alt+Space (voiceKey) foreground=0x24052c pid=13996 Notepad …
  [companion 08:20:28.775 pid 6232] point: start — overlays=1 annotate=always
  [companion 08:20:36.958 pid 6232] point: screen_context → 1 line(s), 4 file(s): [screen 1/1 "Notepad" · … pointer: underlined (799,499)→(499,399) …
  ```
  Holding Ctrl+Alt+Space over Notepad (key_down/key_up SendInput) draws the trail and the island says "Ctrl+Alt+Space held · theuth's desk":
  `FIX-D1-ctrl-alt-space-held-after-restart.jpg`. Settings › Keys shows no gold notice: `FIX-D1-settings-keys-no-notice-after-retry.jpg`.
- **N1 / N2.** `seed.ps1 all` (decision, blocked, hire; `rail: needsYou=3 marks=4`): the inbox pill shows the wordmark with a gold **3** beneath,
  whole, inside the capsule; the agents pill shows **one** ring (the desk), not 3 gold dots + 1: `FIX-N1-inbox-count-hover-expanded.jpg`.
- **N3.** Agents panel (one row `theuth · desk`): UIA `agent-mic-1` → `{type: Button, name: "nothing is running", enabled: false}`,
  `agent-stop-1` → `enabled: false`, `agent-mic-1-hint` → `{type: Group, name: "nothing is running", enabled: true}`:
  `FIX-N3-agents-panel-disabled-row-buttons.jpg`.
- **N4a.** Settings › Rail shows "Separate pills" on (`FIX-N4a-settings-rail-separate-pills-toggle.jpg`); toggling it off:
  ```
  [companion 08:23:39.063 pid 6232] rail pills suspended (strip on)
  [companion 08:23:39.066 pid 6232] rail strip shown: rect=(1916,456) 6x120 presence=alwaysSliver side=right
  [companion 08:23:39.066 pid 6232] settings: applied live (keys=same undo=3 rail=alwaysSliver right strip)
  ```
  window list: "theuth rail strip"; hover-expanded it is the pre-149 strip (wordmark · 3 · three gold marks · hairline · one agent ring):
  `FIX-N4a-strip-rail-escape-hatch-off-hover-expanded.jpg`. Toggling back on: `08:24:04.259 rail pills shown … dots=1`, window "theuth rail".
- **N4b.** `FIX-N4b-voice-key-page-label-wraps.jpg` — "Read what the pointer touched" whole at 720×520.

Round closed as instructed: `seed.ps1 clear` (2 tasks cancelled, 1 proposal removed, `attention_list=0`), `kill-mine`, `start-installed`
(pid 8332), `w5-gate reg` → `Run\TheuthCompanion = "C:\Users\user\AppData\Local\theuth\companion\Theuth.Companion.exe" --background`,
desk released.

## Deviations / notes
- The escape hatch is applied **live** (no restart) by building both rail windows and suspending one; cost of the suspended window is a
  hidden HWND with no poll running.
- The Mac `RailPills.derive` parity gap (N2) is recorded above, not changed.
- The **installed** companion on build-B (2.13.0, pre-fix) was restarted by `start-installed` while the shell was up and hit D1 again
  (`08:24:56.597 pid 8332 … held by another app`): its voice key is dead until the packaged fix is installed or a chord is re-recorded.
- Observed, not fixed (not on the list): Settings › Keys row labels ("Voice key — point and say (hold; double-…", "Meeting push-to-talk
  (while a meeting is…") clip at the default window size the same way the Voice-key label did; `KeyRow.Make` is the place.
- The `w6-gate.ps1 grep` regex argument cannot contain `|` or spaces (service arg rule) — one pattern per call.

## §R1 — Settings › Keys row labels clipped at 720×520 (orchestrator R1, 08:29)

`Settings/ShortcutRecorder.cs` `KeyRow.Make`: the label `TextBlock` in the row's star column gets `TextWrapping.Wrap`, so "Voice key —
point and say (hold; double-tap locks)", "Meeting push-to-talk (while a meeting is live)" and "Meeting eye — hide from capture (while
live)" wrap onto two lines instead of clipping; the recorder (170 epx) and the ↺ button keep their widths. The row's verdict line
(`settings-keys-notice-<action>`) wraps too (`MaxWidth = 340`) so a long "Already used by …" cannot run off the page. The other
pages were checked at the default 720×520 on build-B: Rail, Island, Voice key (fixed in the main wave), Notifications, Undo delay,
Launch at login and Engine read whole — Combo labels already wrap since the main wave, every caption wraps, Toggle labels are ≤ 35
chars. No Core change.

- Gates: Mac Core 274/274, Rpc 68/68; build-B Debug x64 @ 4af43cc8 0 errors / 0 warnings (25 s, job 20261009T073017-260).
- Proof (Debug 4af43cc8 `--background --shareable --settings keys`, desk 08:30–08:32): `FIX-R1-settings-keys-labels-wrap.jpg` (every
  Anywhere row whole, two-line labels), `FIX-R1-settings-notifications-720x520.jpg`, `FIX-R1-settings-island-720x520.jpg`.
- Restored: kill-mine → start-installed (pid 15040) → `Run\TheuthCompanion = "…\companion\Theuth.Companion.exe" --background` → desk released.
  (The installed pre-fix companion hits D1 again on this restart, as before — until the packaged fix is installed.)
