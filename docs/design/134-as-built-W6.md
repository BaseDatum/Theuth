# 134 — as-built W6: polish on build-B (pin tip, backdrops, C13 live, C12 pass, startup flicker, failed-turn receipt, toast relaunch, pause state)

Branch `feat/134-w6-polish` off main ad153b20; worker `w6-win` (theuth subagent), 2026-10-08 04:03–05:20 BST. Verified on
build-B (Windows 11 Pro 25H2 26200, console session 1, 1920×1080) with the installed theuth f51e3bb3 running (shell +
engine, no model credential) and Debug builds from slot `w6` (`C:\Users\user\Develop\theuth-ci-w6\…\Theuth.Companion.exe`).
Three desk rounds: 04:30–04:48, 04:53–05:09, 05:11–05:16. Screenshots and UIA snapshots named below live in
`~/.theuth/briefs/2026-10-08/134-w6/evidence/` on Sky's Mac (30 files); build-B copies under `C:\Users\user\tools\pkg\w6-*.png`.

## 1. Files

| Area | File | What |
|---|---|---|
| Core | `src/Theuth.Companion.Core/PinTip.cs` (new) | item 1 verdict (`Show · Promoted · NoEntryYet · AlreadyShown · TooEarly · Paused · Off`), texts, `IsShownFor`, `EntryFor`, `Describe` |
| Core | `PolicySettings.cs` | `CompanionSettings.PinTipShownFor` ↔ key `pinTipShownFor` (string = exe path; absent when empty), in `Key.All`, `Equals`, `GetHashCode` |
| Core | `AskRouter.cs` | item 7: `ReplyEvent.Error`, `TurnFinished.StopReason`, `AskRouter.IsFailure`, `AskReply.Error/Failed`, `FirstTwoLines` = "theuth couldn't reply — <first line ≤ 120>" |
| Core | `StoreModel.cs` | `AskObserve` maps `Error` and `stop_reason`; logs `ask failed on the desk: …` |
| Core | `ToastActivation.cs` (new) | item 8 parse + `HoldUntilRefetch(launched, refetchedOnce, items)`; item 9 `PauseMenu.State/Title` (the Mac rule) |
| Rpc | `Models.cs` | `StreamDelta.StopReason`, `.Message` |
| App | `Notifications/PinTipScheduler.cs` (new) | 60 s after the first NIM_ADD, registry read, re-read before posting, retries, persist |
| App | `Notifications/Notifier.cs` | `ShowPinTip`, `IsPaused`, `DeferrableOff`; item 8: `GetActivatedEventArgs()` read right after `Register()`, replayed in `Start()`, 5 s held-action fallback; `ToastActivation` parsing |
| App | `Notifications/ToastBuilder.cs` | `BuildPinTip` (muted, Default scenario, body = `later`, button `SetInvokeUri(ms-settings:taskbar)`) |
| App | `Notifications/NotificationsInstaller.cs` | wires the scheduler (not in probes), `ui.PausedUntil` |
| App | `Tray.cs`, `Windows/Surfaces.cs` | `Tray.AddedAt`, `Surfaces.TrayAddedAt`, `PausedUntil` hook, pause check marks + "(until HH:mm)", `SystemEffects` start/dispose order |
| App | `Windows/SystemEffects.cs` (new) | item 3: `UISettings.AdvancedEffectsEnabledChanged` / `AnimationsEnabledChanged` + host `WM_SETTINGCHANGE` → `Changed`; `ApplyTransitions` (`DWMWA_TRANSITIONS_FORCEDISABLED`) |
| App | `Windows/Backdrop.cs` (new) | item 2: controller API (`MicaController` / `DesktopAcrylicController`) with `SystemBackdropConfiguration.IsInputActive = true` held, `FallbackColor` = surface, theme kept in step from every Render, disposed in `OnClosingForGood` |
| App | `Windows/RailWindow.cs`, `Windows/AttentionPanelWindow.cs`, `Windows/ToolWindow.cs`, `Windows/MarkGlyph.cs` | live effects, backdrop opt-ins (`--rail-acrylic`, `--panel-mica`), `OnClosingForGood`, Done rows in the content view, `panel theme:` log line |
| App | `Settings/SettingsWindow.cs` | item 4: Launch at login / Engine pages update text in place (no 2 s rebuild), toggle flips in place, `settings-login-status` is a polite live region, Engine value rows named |
| App | `Windows/SingleInstance.cs` | the fresh instance no longer touches `GetActivatedEventArgs` before `Register()` (see item 8) |
| App | `AskBar/AskBarWindow.cs` | C12: hint/status TextBlocks read as their text |
| Shell | `crates/desktop/src/companion/windows.rs` | item 5: `interval_at` (no immediate first poll) + `LAUNCH_GRACE` 15 s while the spawned child lives; pure `verdict()` + 3 unit tests |
| Tests | `tests/Theuth.Companion.Core.Tests/PolishTests.cs` (new) | 16 tests: PinTip 7, failed-turn receipt 5, ToastActivation 2, PauseMenu 2 |
| Tools | `w6-gate.ps1` (new) | build-service `script` helper: shell/companion logs, graceful WM_CLOSE quit of the installed or the slot companion, Debug launch, NotifyIconSettings read/promote/unpromote, `tip-reset`, `fx` (transparency via registry + `ImmersiveColorSet` broadcast, animations via `SPI_SETCLIENTAREAANIMATION` by value), `toast-reg` (AUMID/CLSID footprint, Application Error events, crash files), `hotkey`, `shot` |

Mac: `bash crates/desktop/companion-windows/test.sh` → Core **146/146** (125 + 16 new + 5 item-10), Rpc 35/35. `cargo fmt --check -p desktop` clean;
`cargo clippy -p desktop --no-deps -- -D warnings` clean (windows.rs is `cfg(windows)`, so it is not compiled on the Mac).
build-B: `cargo check -p desktop` at dd5a51c6 (the windows.rs change) 0 errors / 0 warnings in 2 m 35 s (job 20261008T032959-140);
Debug builds 0 errors / 0 warnings — 5faef575 (job 20261008T041442-191, 18 s); item 10: 6805fa26 (20261008T061028-243, 27 s) and 877606e9 (20261008T061916-253, 24 s).

## 2. Items

### Item 1 — first-run "pin theuth to the taskbar corner" tip — PASS
- Core decision (`PinTip.Decide`): already shown → off (Deferrable = Off) → too early (< 60 s after the first `NIM_ADD`) → no
  `NotifyIconSettings` row for this exe → promoted (`IsPromoted = 1`) → paused → **show**. `Show` and `Promoted` persist
  `pinTipShownFor = <exe path>` (once per install: another path shows it again; the key is removed from the file when empty).
  `NoEntryYet`/`Paused`/`TooEarly` re-check every 60 s (no-entry ≤ 5 times per run). 7 unit tests.
- App: the clock is `Tray.AddedAt` (first successful `NIM_ADD`); the registry is read at the check and **again just before
  posting**; the toast honours W3's rules — nothing while paused, nothing when the deferrable class is Off, Default scenario
  (Windows' Do not disturb applies), muted; one button "Open taskbar settings" → `ms-settings:taskbar` by protocol activation.
- Live (round 1, Debug dd5a51c6, a new exe path so no row was promoted): `04:32:03.668 pin tip: armed — first check in 59s
  (NIM_ADD at 04:32:02.997)` → `04:33:03.013 pin tip: check 1 verdict=Show sinceAdd=60s entry=IsPromoted=absent entries=24
  paused=false` → `notifications: [tip] post tip … title="Keep theuth in view" button="Open taskbar settings"
  uri=ms-settings:taskbar toastId=150` → `pin tip: shown — pinTipShownFor = …\theuth-ci-w6\…\Theuth.Companion.exe`. The toast
  on screen at 04:33: `r1-0433-pin-tip-toast.jpg`. Later launches from the same path logged `pin tip: already shown for this exe`.
  The flag was removed from Sky's settings.json at the end (`tip-reset: removed=True`), so the installed companion shows the tip
  once when it carries this code.

### Item 2 — backdrops (A7) — rail: cannot work on the layered window (kept opaque); panel: Mica attached but wrong colour (kept opaque)
Both materials are implemented on the controller API (`Backdrop.cs`): `DesktopAcrylicController` Thin for the rail,
`MicaController` Base (→ `DesktopAcrylicController` Base where Mica is unsupported) for the panel, `IsInputActive = true`
held, `FallbackColor` = the surface colour, theme from the palette the content was rendered with. Both were tested live and
both are **off by default** after what build-B showed; `--rail-acrylic` / `--panel-mica` re-attach them for a retest.

Rail, Thin acrylic on the `WS_EX_LAYERED` rail (dd5a51c6, 04:35–04:39): (a) **no material rendered** — over Paint's white canvas
the rail interior stayed the fallback/black (`r1-0439-rail-acrylic-over-white-canvas-no-material.png`); (b) **the window dropped
out of the topmost band once the controller was attached** — it vanished behind a maximized Paint and behind an activated Notepad
(`r1-0435-…hidden-behind-maximized-paint.png`, `r1-0436-…behind-activated-notepad.png`) while the same build launched with the
opaque surface (`--no-rail-acrylic` then) stayed above both (`r1-0438-rail-opaque-above-maximized-paint.png`). The pass-through
needs the layered style (spike S1), so the rail keeps W2's opaque surface. Not retried: `SetWindowRgn` pass-through instead of
layering (a W2-sized change, out of polish scope).

Panel, Mica Base: attached and logged every time (`panel: backdrop mica-base attached (IsInputActive held true, fallback
#1C1915, theme Dark)`; `panel theme: host.ActualTheme=Dark app=Dark backdrop=mica-base`). Round 1's first open (04:40) was
dark (`r1-0440-panel-mica-dark.jpg`). Rounds 2 and 3 (04:55, 05:11, 05:13 — three fresh processes) painted the material **light**:
pixel read of `r3-0513-…-defect.png` → background #F3F3F3/#F3F2F2 (Mica's light tint) under dark-palette text (#E8DCC8 = Ink
dark), and an 82 % dark surface tint on the host background did not show over it. With transparency off the same panel is the
correct dark surface (`r2-0456-panel-hire-card-transparency-off.png`). Cause not found in the time available (theme is Dark at
every level we can read; the controller's rendering disagrees); the panel ships opaque (`r3-0515-panel-opaque-final-5faef575.png`).

### Item 3 — C13 live — PASS
`SystemEffects` follows both switches without a restart; evidence from the companion log of pid 4812 (dd5a51c6) with the panel open:
- Transparency off (`fx transparency off` = registry `EnableTransparency=0` + `WM_SETTINGCHANGE "ImmersiveColorSet"`, what Settings
  does): `04:42:45.861 effects: changed via UISettings.AdvancedEffectsEnabledChanged: transparency on→off` → `rail: backdrop
  detached — opaque surface` → `panel: backdrop detached — opaque surface`; screens `r1-0442-transparency-off-panel-opaque.jpg`,
  `r1-0443-transparency-off-rail-opaque-over-white.png`. Back on: `04:43:32.690 … transparency off→on` → both re-attached.
- Animations off (`SystemParametersInfo(SPI_SETCLIENTAREAANIMATION, FALSE, SPIF_UPDATEINIFILE|SPIF_SENDCHANGE)`):
  `04:44:34.996 effects: changed via WM_SETTINGCHANGE: … animations on→off` → `rail: effects applied — … animations=off` →
  `panel: effects applied — … animations=off`; the rail re-renders its marks without the 380 ms beat and
  `DWMWA_TRANSITIONS_FORCEDISABLED` is set on rail, panel, legend and peek card; back on at 04:45:15. (No agent was working on
  build-B, so the static beat has no screenshot — the code path is `MarkGlyph.Create(…, animate)` with `animate` re-read on change.)
- Recorded before and restored after, both rounds: `fx: EnableTransparency=1 ClientAreaAnimation=1` at 04:42:25 / 04:45:15 and
  05:16:20. Note: the first `fx animations off` of the night set TRUE by mistake (pvParam passed by reference) — fixed in
  `w6-gate.ps1` before the measured run; nothing was left changed.

### Item 4 — C12 UIA/Narrator pass — PASS; Launch-at-login toggle fixed
Cause of the known defect: `SettingsWindow` rebuilt the **login** and **engine** pages every 2 s (`refresh.Tick → ShowPage`),
so the `ToggleSwitch` a client found was detached from the tree by the time it clicked; a detached element reports off-screen
and `ui_click` refuses. Fix: the tick updates the live texts in place (`live` updaters), the toggle flips in place on a settings
change, the page is never rebuilt on a timer. Live (dd5a51c6, 04:41): `ui_find settings-login-toggle` → wait 3.5 s (two ticks)
→ `ui_click` **dispatched**, status line `Run value: absent · status=not_registered` → `ui_click` again on the same element id
→ `Run value: "…\Theuth.Companion.exe" --background · status=enabled`; same element ids before and after (`…26c`, `…26d`).
Toggled twice, so the setting was left as found (the installed companion rewrote the Run value to its own exe on restart:
`05:08:36 login item: Run value is another install — rewriting for this exe`).

Snapshots (all in `evidence/uia-*.json`): rail (root `rail` "theuth rail", marks named "<agent>, <state>, <since>"), panel empty
(35 elements) and with a hire card (50: `Hire, key 1`, `Decline, key 2`, lines, Done rows named "who, text, ago"), Done zone, all
six Settings pages (rail 32 · keys 117 · notifications 40 · undo 31 · login · engine 44), Ask bar (8), legend (82). **No
interactive element without a Name** on any surface. Changed: Ask bar `ask-hints` / `ask-status` lose their fixed Names
("Ask bar hints"/"Ask bar status") so Narrator reads the text; `ask-status` is a polite live region; `settings-login-status`
is a polite live region; Settings › Engine value TextBlocks carry their row label as Name; Done-zone rows are
`AccessibilityView.Content`. Tray menu: a Win32 popup — Narrator reads it natively; items verified by screenshot
(`r1-0445-tray-menu-debug.png`).

### Item 5 — startup flicker — PASS (shell side; the orchestrator rebuilds)
Mechanism from the logs: shell `desktop-stdout.log` (UTC) `01:42:46.178 companion: launched` → `.180 alive=true; tray hidden` →
`.181 alive=false; tray shown` → `01:42:48.186 alive=true; tray hidden`; companion log for that pid (9396, BST) `02:42:46.694 launch
exe=…` (the mutex is created right after this line) → `02:42:46.803 host window` → `.822 NIM_ADD`. The companion never released
or re-took its mutex (one `launch`, no `terminate`, no single-instance line): the shell read the mutex **3 ms after spawning**
the process — `tokio::time::interval(POLL)`'s first tick completes immediately — while the companion needs ~0.5 s to reach
`OnLaunched`. Fix in `windows.rs`: `interval_at(now + POLL, POLL)` (first poll after 2 s) and a 15 s `LAUNCH_GRACE`: while the
spawned `Child` still runs and no mutex is seen, `observe()` keeps the optimistic `true`; a child that exits, or the grace
running out, ends it (`verdict()` is pure, 3 tests in the file; `cargo test -p desktop` on Windows runs them). Not run live
(the installed shell is f51e3bb3).

### Item 6 — panel + tray-menu recheck — PASS
Final Debug 5faef575 (round 3): tray menu in order — Open theuth · Ask theuth… · Pause notifications ▸ (For 1 hour · Until
tomorrow · ✓ Off) · ✓ Launch at login · ― · Keyboard shortcuts… Ctrl+/ · Settings… · ― · **Quit theuth · Quit theuth Companion**
(`w6-r3-tray-menu-final.png` on build-B; `r2-0507-pause-off-restored.png`). Panel opens by tray click (every round) and by the
hotkey (Ctrl+Shift+Space via the panel's keyboard path; the `hotkey` SendInput from the headless service did not reach the hotkey
— `keyboard_control` from windows-mcp did); Esc closes and hands the foreground back (log `panel closed — foreground back …`).
Done zone shows the receipts and the failed-turn line (`r1-0447-done-zone-failed-receipt.png`); legend Ctrl+/ (`r1-0447-legend.png`).
"Quit theuth Companion" used at 05:08:02 and 05:15:36: the process ended in < 4 s with **no Application Error event** (the
04:47:40 / 05:00:43 quits of the earlier Debug builds had faulted in Microsoft.UI.Xaml.dll — a window destroyed under a live
backdrop controller; fixed by `OnClosingForGood`).

### Item 7 — Ask bar receipt shows a failed desk turn — PASS
Core: the desk's `Error { message }` event, or `TurnFinished` with `stop_reason` `error` · `cancelled` · `timed_out` ·
`interrupted`, ends the in-flight reply as failed, text streamed or not; `FirstTwoLines` = "theuth couldn't reply — <first line
of the message, clipped to 120>"; a normal finish is unchanged (5 tests incl. the wire shape). Live (dd5a51c6, 04:46): Ask bar
"W6 receipt test, ignore" → `04:46:16.506 ask sent to desk 01a11896-…` → `04:46:16.561 ask failed on the desk: model: Resolver
error for model 'anthropic::claude-sonnet-4-5 (adapter: Anthropic)'.` → the Done zone row reads (UIA) `theuth, theuth couldn't
reply — model: Resolver error for model 'anthropic::claude-sonnet-4-5 (adapter: Anthropic)'., 46 s` with the `Ctrl+O session`
hint kept (`r1-0447-done-zone-failed-receipt.png`). One ask sent (one failed turn added to Sky's desk).

### Item 8 — relaunch by toast click — PASS (after a crash found and fixed on the way)
First attempt (d7c80279) read `GetActivatedEventArgs()` in `Program.Main` before `Register()`: every "Open panel" click
(05:01:22, 05:02:28, 05:03:06) launched the exe, which fail-fasted at once — Application Error 1000, `Microsoft.WindowsAppRuntime.dll`
0xc0000409 — before its first log line (`toast-reg` action). Fix: read it right after `AppNotificationManager.Register()` in
`RegisterEarly` (the quickstart's order), replay in `Notifier.Start()` with the launch rule (hold everything but `later` until the
first refetch, 5 s fallback). Live (b3a748cc, 05:06:30 click on "Open panel" of the `seed-045812-1 · hire` toast in Notification
Center with no companion running): `05:06:31.885 launch` (0.9 s) → `.922 Register() ok` → `.928 launched by a toast click —
argument='action=open;banner=hire:seed-045812-1;item=hire:seed-045812-1'` → `05:06:32.460 action open held until the first
refetch (launch path)` → `.772 action open … items=[hire:seed-045812-1] (after relaunch)` → `.825 panel open (Mouse) … active=true`;
UIA: panel "2 need you, 2 of 2", focused card `seed-045812-1 … Hire, key 1 · Decline, key 2` (`uia-panel-after-toast-launch-r2.json`).
The toast-launched process has no `--shareable`, so no screenshot shows it (C11) — UIA is the evidence.

### Item 9 — Pause notifications ▸ shows its state — PASS
The Mac menu (`NotificationsInstaller.swift appendMenu`) puts a check mark on the active row ("1 hour" vs "Until tomorrow" by
`isUntilTomorrow`: ends tomorrow and > 1 h away; "Off" when not paused) — mirrored as `PauseMenu.State` (2 tests) plus the parent
row reads "Pause notifications (until HH:mm)". Live (d7c80279, 05:07): ✓ Off → "For 1 hour" → menu reads **Pause notifications
(until 06:07)** with ✓ For 1 hour (`r2-0507-pause-for-1-hour-checked-until-0607.png`) → Off → ✓ Off (`r2-0507-pause-off-restored.png`).

### Item 10 — spurious "theuth engine unreachable" toast on every intentional quit/update — PASS

**Cause (packaged 2.12.3 log, orchestrator):** `06:49:56.502 quit theuth requested -> theuth://quit` → `.562 connection: reconnecting` → `.594 notifications: [live] post error … "theuth engine unreachable"` → `.596 host: WM_CLOSE — quitting`. The shell's quit handshake stops the engine (webview RPC shutdown) before `RunEvent::Exit` closes the companion, so the companion saw the engine drop ~60 ms before it was asked to quit and posted the error toast — after every whole-app quit and every update.

**Fix (companion only, no shell change):**
- Core `NotificationPolicy`: `Engine(connected: false)` now only records the loss (`engineLostAt`); the 1 Hz `Tick` posts the error banner once `EngineGrace` (5 s) has passed with the engine still gone. A reconnect inside the grace clears it silently. `Quitting()` drops a pending loss and suppresses later ones; `EngineLossPending` exposes the held state. The grace is 5 s because the shell's quit, an engine restart from the Settings page and an update swap each take well under that, while a real outage still notifies within a human "it's gone" interval (the old immediate toast was the only Error-class banner and fired on every 60 ms blip). `Policy.cs`.
- App: `Surfaces.QuitRequested` fires before `theuth://quit` (tray › Quit theuth) and on tray › Quit theuth Companion; `NotificationsInstaller` wires it to `Notifier.Quitting()`, which marks the policy and logs `notifications: quitting — engine-loss toast suppressed`. The Notifier logs `engine lost — held for 5 s before the error toast` once per outage. `Surfaces.cs`, `Notifier.cs`, `NotificationsInstaller.cs`.
- Tests (Core `EngineLossGraceTests`, 5): held inside the grace; posted by the tick after it (once, with sound); silent reconnect inside the grace; `Quitting()` before the drop → nothing, and after a drop → the pending loss is dropped; the `--probe-notify` replay's engine segment ticks past the grace so its "one banner" reading is unchanged. The W3 `EngineLostIsOneErrorBannerWithdrawnOnReconnect` test now ticks past the grace. Core **146/146**.

**Live proof (Debug 6805fa26 on build-B, 07:12–07:16, desk held, installed companion quit first, restored after):**
- Notification Center before: newest "theuth engine unreachable" at 6:12 AM (`r4-0712-notification-center-before.png`).
- tray › **Quit theuth** at 07:13:47 → log `tray menu: Quit theuth` → `.160 notifications: quitting — engine-loss toast suppressed` → `.234 quit theuth requested -> theuth://quit` → `.300 connection: reconnecting` → `.306 host: WM_CLOSE — quitting` → `.346 terminate (exit 0)`; **no `[live] post error` line**; theuth-desktop and the companion both gone. Notification Center after: unchanged, newest still 6:12 AM (`r4-0714-notification-center-after-quit-theuth.png`).
- Lasting loss still notifies: the Debug companion launched with no engine running at 07:14:43 → `07:14:45.735 connection: reconnecting` → `.736 engine lost — held for 5 s` → `07:14:51.660 [live] post error … "theuth engine unreachable"` (5.9 s = grace + the next 1 Hz tick) → toast on screen (`r4-0714-lasting-loss-toast-after-grace.png`).
- tray › **Quit theuth Companion** at 07:15:49 → companion gone, no toast (`r4-0715-after-quit-companion-no-toast.png`).
- Restore: `w5-gate.ps1 launch-logged` @ cab3145a → installed theuth-desktop 16972 + Theuth.Companion 2120 + 3 × t.exe at 07:16:03; desk released 07:17.

Not exercised: a mid-grace reconnect on a live engine (unit-tested only — no safe way to drop and restore the engine inside 5 s without killing processes I do not own).

## 3. Deviations from the brief
1. The installed companion was quit with `WM_CLOSE` to its `TheuthCompanionRail` host window (`w6-gate quit-installed`) in rounds
   2–3 — the same `quit(0)` path as tray › "Quit theuth Companion" (used by hand in round 1 and for the Debug builds), chosen
   because the tray is unreachable through UIA and a coordinate click is fragile. Never "Quit theuth"; theuth-desktop + engine
   kept running throughout (pid 15188, 3 `t.exe`).
2. Rail acrylic and panel Mica are **opt-in flags**, not defaults (§2 above) — the design's A7 materials are implemented but not
   shipped on by default.
3. The Debug companion was launched with windows-mcp `app()` (as the brief says) and with `--shareable` for the screenshots.
4. Seeds (`seed.ps1 hire`, twice) were added to Sky's installed engine for items 8/C12 and cleared (`seed.ps1 clear`, exit 0).

## 4. Open / for Sky
- **Panel Mica renders light under a Dark configuration on build-B** (rounds 2–3; round 1 was dark). Opaque until understood;
  `--panel-mica` for a retest. Candidates not checked: Windows App SDK 2.5.1 trimmed set, the `IsInputActive` held-true
  configuration vs the framework's `MicaBackdrop`, something about a window first shown after Notification Center had focus.
- **Rail acrylic on a layered window**: no material and a lost topmost band (Windows App SDK 2.5.1, Windows 11 26200). A region
  (`SetWindowRgn`) pass-through would be the way to a translucent rail.
- Item 5's fix is in the shell; the installed f51e3bb3 still flickers until the package is rebuilt and re-checked (expected shell
  log: `launched` → `alive=true` and no `alive=false` within the first 15 s).
- The pin tip persists its flag in `%LOCALAPPDATA%\theuth\companion\settings.json`, which survives uninstall (W5 noted the
  folder is left) — a reinstall to the same path will not show the tip again. Sky may prefer the flag in the data dir.
- Recorded for the orchestrator, not changed (per the 04:33 addendum): hire/blocked toasts carrying only "Open panel · Later"
  are by design; Unblock refused by the engine while blockers exist is product-level.

## 5. Integration and packaged gate (orchestrator, 2026-10-08 05:25–07:45)

**Integrated tree.** `integrate/134-w6` cab3145a merges, in this order:
- feat/134-w6-polish 64fa0054 (this wave);
- feat/134-w6-tools 1df28454: `w5-gate.ps1 pkg-start SHA` / `launch-task`, plus the desktop tray tooltip;
- feat/130-parity 89d0ad3e: the macOS companion's parity fixes, see `130-as-built-PARITY.md`;
- origin/main 4606b76e;
- `w5-gate.ps1 launch-logged` / `stdout`.

No conflicts. 40 files against main.

**Gates on the integrated tree.**

| Gate | Result |
|---|---|
| C# Core / Rpc (Mac `test.sh`) | 141/141 · 35/35 |
| Swift `test.sh` / `swift build -c release` | 92 tests in 21 suites · Build complete |
| `cargo fmt --check -p desktop` · `cargo clippy -p desktop --no-deps -D warnings` · `cargo test -p desktop --lib` | clean · clean · 48 passed |
| tray tooltip (`App.tsx`) | `tsc -b --noEmit` baseline diff: 0 new errors, none in App.tsx (273 pre-existing from stale bindings in an unseeded tree) |
| build-B companion Debug build · Windows `cargo check -p desktop` | 0 errors / 0 warnings · 0 / 0 |
| build-B release package (`pkg-start cab3145a`, v2.12.3 → full workspace rebuild) | exit 0 at 06:25 (58 min), setup 309.7 MB |

The `verdict()` tests in `companion/windows.rs` compile only on Windows, and the build service's `cargo check` does not build tests. They were read, not run.

**Update path, packaged.** Setup ran over the running 2.12.2+f51e3bb3 app, its engine and its companion: exit 0 in 44 s. Nothing was left running afterwards; the hook stopped the companion and the job object took the engine with the shell. Then 488 companion files, the scheme registered, and `companion.json` kept.

**Startup flicker, packaged — fixed.** `launch-logged` produced exactly `05:33:44.691 companion: launched …` and `05:33:44.696 companion: alive=true; tray hidden`, with no `alive=false` afterwards. Re-read at 06:35. Before the fix the same log showed alive=true → false → true within 2 s.

**Dev-machine caveat (not a user bug).** With `launch_at_login` true in the shared `~\.theuth\companion.json`, any companion instance rewrites `Run\TheuthCompanion` to ITS OWN exe on start, including a Debug build or a packaging run started from `target\release\companion-windows\`. After the package build the Run value pointed into the release clone. The installed companion wrote it back on its next start (verified 06:35). A later fix could write the value only from the installed location, or only on a user toggle.

**Packaged recheck, first package (2.12.3+cab3145a, 06:36–06:50, fresh windows-mcp subagent): 7/7 PASS.**
- R1: the pin tip stayed silent because the icon was promoted (log `verdict=Promoted … IsPromoted=1`), and settings JSON loads in the trimmed publish.
- R2: the shell tray tooltip reads `theuth · nothing needs you`.
- R3: the menu order ends with Quit theuth · Quit theuth Companion, and paused shows `Pause notifications (until 07:41)` plus check marks.
- R4: `theuth couldn't reply — model: Resolver error …` arrived 34 ms after the ask.
- R5: the Launch-at-login toggle took `ui_click` twice with the same element id.
- R6: a toast click launched the companion, the action was replayed after the first refetch, and the panel opened on the hire card about 1.5 s after the click.
- R7: Quit theuth ended every process in about 1 s, with no Application Error.

The recheck also found item 10 (below): a spurious engine-unreachable toast on every intentional quit.

**Item 10, packaged (2.12.3+7fd8068f, installed over the running app at 07:35, exit 0 in 42 s, flicker-free start):** R2′ PASS: "Quit theuth Companion" logged `quitting — engine-loss toast suppressed`, no toast, the shell tooltip correct, and the Run value written back to the installed exe on relaunch. R7′ PASS: "Quit theuth" took 160 ms from click to exit (`quitting — engine-loss toast suppressed` → `theuth://quit` → `WM_CLOSE` → exit 0), every process was gone, and Notification Center gained no new "engine unreachable" entry (newest stayed 7:19 AM) and no crash. The update over the running app also produced no toast: the companion held two transient losses during the shell's engine handover (`engine lost — held for 5 s before the error toast (item 10)` at 07:35:50 and 07:35:51) and dropped them on reconnect.

**What landed vs what was packaged.** The packaged and rechecked build is 7fd8068f. The landed tree adds two things. One is 877606e9: two lines in `Notifier.cs` that log a held engine loss once per outage, not per reconnect attempt. It is Debug-built 0/0 on build-B, and Core is 146/146 on the final tree (my run). The other is 594343f4, which is this document. The Rpc code and tests are byte-identical to the 35/35 run on cab3145a.

**For Sky.**
1. Backdrops (§2): retry the rail acrylic with a region-based pass-through instead of the layered window, and chase the panel Mica light-under-Dark anomaly on WinAppSDK 2.5.1, or accept opaque surfaces.
2. Unblock is a product-level issue on all three surfaces (desktop inbox, Windows, macOS). The engine refuses `blocked → todo` while blockers exist. Knowledge `facts/theuth-attention-unblock-refused-while-blockers-exist-2026-10-08` has the options.
3. build-B's installed app has no model credential: Library › Secrets is empty, every turn fails `ApiKeyEnvNotFound ANTHROPIC_API_KEY`, and the picker lists no provider. Add a key there to use it. Nothing was copied over.
4. The pin-tip flag lives in `companion\settings.json`, which survives uninstall, so a reinstall to the same path will not show the tip again.
