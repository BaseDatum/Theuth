# 134 — as-built W3: notifications (DND-respecting), breakpoints, the Settings window, launch at login

Wave W3 of design 134 (`docs/design/134-companion-windows.md` §4.4, §4.6, §5.5, §7 W3; R2 rulings D8/D12), built in the
worktree `theuth-134-w3b` on branch `feat/134-w3b-notify-settings` (cut from main `a1cbe9fa`, W2 `b661543e` merged in before
the final gate round). Replacement worker for the first W3 session (cut by an engine restart; it had written nothing). The
Swift originals are `crates/desktop/companion/Sources/Companion/{Notifications,Breakpoints,Settings}/**` (130 W2/W6).

`WIN/` = `crates/desktop/companion-windows/`, `APP/` = `WIN/src/Theuth.Companion/`, `CORE/` = `WIN/src/Theuth.Companion.Core/`.

## 1. File map

| Path | What |
|---|---|
| `CORE/NotificationReplay.cs` | The `--probe-notify` replay as a pure step list (`Run(items, prefs, t0)` → `Step(Segment, Label, Output)`): `all` = the live items at once (flood when > 10), `one` = the first ≤ 10 one at a time, tick +60 s, breakpoint +61 s (the held batch), re-add +70 s, breakpoint +80 s inside the rate window, tick +181 s, answered elsewhere (withdrawals), then `engine` = the engine lost (the error class; Urgent under the opt-in). `Describe(step)` is the Swift replay log line |
| `CORE/PolicySettings.cs:154-160` | `CompanionSettings.Key.ErrorsBreakThroughDnd = "errorsBreakThroughDnd"` — the Windows opt-in (D8, R2) persisted → `NotificationPrefs.TimeSensitiveAvailable`, off by default; read/written in `Load`/`Save` |
| `CORE/PolicySettings.cs:236-275` | `LoginItemRegistration`: `RunKey`, `ValueName = "TheuthCompanion"` (W5's NSIS hook deletes exactly this), `RunValue(exe)` = `"<exe>" --background`, `ExeOf(value)`, `PointsAt(value, exe)` (case-insensitive), `Status()` = `enabled` · `not_registered` |
| `APP/Notifications/Notifier.cs` | The port of Notifier.swift on `AppNotificationManager`: `RegisterEarly()` (handler attached, then `Register()`, before any window; logs `Setting`, `IsUrgentScenarioSupported`, an elevation warning — C1/C2), `UnregisterAtExit()` (`Unregister()`, never `UnregisterAll()`, so a later click launches a new process), policy driving from the Store (`Items` → `Update` — the first snapshot seeds; `Connection` → `Engine(lost/back)`; `Receipt` → receipt toast), `Breakpoint()`/`Tick()`/`Pause()`, `Post` (group = agent, tag = banner id, buttons from the card) and `Withdraw` (`RemoveByTagAndGroupAsync`), `ReconcileAsync` (stale toasts of an earlier run removed after the first snapshot), `NotificationInvoked` on a background thread → `TryEnqueue` → `Handle` (open · later · clear · reply/deny-note/send-back through `CardReplies.ReplyAction` · opt-1..3) → `Store.AnswerNow` (no hold) → receipt toast (withdrawn after 8 s); activations before the first refetch (the relaunch path) are held until `Refetched`; `Redirected(AppActivationArguments)` for W2's single-instance redirect; `RequestUrgentConsent()`; `SettingText`; `ProbeLine` → `logs\probe-notify.log` |
| `APP/Notifications/ToastBuilder.cs` | `Banner` → `AppNotification`: arguments `action` · `banner` · `item` · `kind`, two text lines, `SetTag`/`SetGroup` (64-char cap with a hash tail), `MuteAudio()` unless the banner sounds, `SetScenario(Urgent)` only for `BannerLevel.TimeSensitive` (the opt-in) — everything else `Default`, so Do not disturb is respected; the Swift category table per toast: option buttons (≤ 3, the card's labels that need no note) + Question: text box + Reply · Approval: text box + Deny with note · Review: text box + Send back + Open · Finished: Clear · Batch/flood: Open panel + Later · Error: Open; receipts carry no buttons |
| `APP/Notifications/NotifyProbe.cs` | `--probe-notify N [--stay]`: real policy live for N s, Settings › Notifications shown without activation at N−3 s, then the replay posted for real with one toast group per segment (`probe-all` · `probe-one` · `probe-engine`), `GetAllAsync` counts by group, the C6 line ("flood segment … in-centre=1 → exactly one summary"), quit 0 or stay |
| `APP/Notifications/NotificationsInstaller.cs` | W3's one hook into `App`: `RegisterNotifications()` before any window; `Install()` after W2's surfaces and before `Store.Start()` — builds the Notifier, `BreakpointSource`, `DoubleTapSource`, `SettingsController`; wires W2's `Surfaces.Current` hooks (`PauseRequested`, `LaunchAtLoginToggled`/`LaunchAtLoginState`, `Redirected`, `OpenPanelOn`, `TogglePanel`, `ApplySettings`); `LoginItem.Reconcile`; `--request-notifications`, `--probe-notify`, `--settings [page]`; `Dispose` = hooks off + `Unregister()` |
| `APP/Notifications/W3Flags.cs` | `--probe-notify N`, `--stay`, `--request-notifications`, `--settings [page]`, `AnyProbe` — W3's switches kept out of W1's `Flags` |
| `APP/Breakpoints/BreakpointSource.cs:13-88` | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)` on the UI thread (delivered through its message loop) → the foreground process's exe path → `BreakpointDetector.Signal.AppActivated` (the companion's own exe is `own`); a 1 Hz `DispatcherQueueTimer` → `GetLastInputInfo` idle seconds → `Signal.Idle`; `Breakpoint` and `Tick` events |
| `APP/Breakpoints/BreakpointSource.cs:90-166` | `DoubleTapSource`: `WH_KEYBOARD_LL` feeding `DoubleTapDetector` with the modifier state (Alt alone, twice); any other key-down cancels; installed only while the Settings toggle is on |
| `APP/Breakpoints/W3Native.cs` | W3's Win32 declarations (`DllImport`, blittable): WinEvent hook, `GetLastInputInfo`, `QueryFullProcessImageName`, the LL keyboard hook, `RegisterHotKey` probe, `ToUnicodeEx`, `GetDpiForWindow`, `GetMonitorInfoW` |
| `APP/Settings/SettingsModel.cs` | The observable `CompanionSettings` (`Shared`, loaded from `SettingsStore.DefaultPath`), `Update(edit)` → save → `Changed(old, new)`; the recorder's `RowNotices` |
| `APP/Settings/LoginItem.cs` | The Run value (`Microsoft.Win32.Registry`, HKCU) written/deleted by `Apply(enabled)`, mirrored into `<data_dir>\companion.json` by `Mirror()`; `Reconcile()` at every start: a value whose exe no longer exists is deleted, a missing/foreign value is rewritten while the toggle is on, companion.json written when missing or stale; `Status()` |
| `APP/Settings/SettingsWindow.cs` | `SettingsController` (one window, re-shown; `Show(activate, page)`) and `SettingsWindow`: a `NavigationView` with the six §4.6 pages — `settings-rail` (side · display · presence), `settings-keys` (every `KeyTable` row by scope with a `ShortcutRecorder`, reset per row / all, collision notices, double-tap Alt toggle), `settings-notifications` (per class immediate / at break / off, sound on errors, **Errors break through Do not disturb**, the Windows setting + "Open Windows notification settings", pause 1 h / until tomorrow / resume), `settings-undo` (0–5 s slider), `settings-login` (toggle + Run value status), `settings-engine` (url, connection, data dir, client, hub, counts, version, log); every control `settings-<page>-<what>`, every page root `settings-page-<page>` |
| `APP/Settings/ShortcutRecorder.cs` | `HotkeyProbe.CanRegister` (exclusive `RegisterHotKey` probe on this thread), `ShortcutRecorder : Button` (click → "Type a shortcut…", key-down records through `KeyTable.Check`, Backspace/Delete clears, Esc cancels, Tab moves on; chars from `ToUnicodeEx` with an empty key state, numpad Enter kept apart), `KeyRow` (label · recorder or fixed chord · reset ↺ · the notice line with "Use here instead") |
| `APP/Store.W3.cs` | `OpenItemRequested(string?)` + `RequestOpen(id)` (toast body → the panel), `ApplySettings` (undo delay) |
| `APP/App.xaml.cs` (W3 lines) | `RegisterNotifications()` at the marked line before any window; `Install(...)` after `Ui.Surfaces.Start` and before `Store.Start()`; `w3?.Dispose()` in `Quit` |
| `APP/Windows/Surfaces.cs` (integration) | Two public methods added to W2's composition root: `OpenPanelOn(itemId)` (toast body/Open → the panel on the item, or the oldest), `TogglePanel()` (double-tap Alt = the hotkey) |
| `WIN/w3-gate.ps1` | The gate helper for the build service's `script` action: `probe -Seconds N [-Stay]`, `launch -Args …`, `stop`, `log`, `reg`, `settings-get`, `settings-set`, `engine-stop`, `processes` (UTF-8 output) |
| `WIN/tests/Theuth.Companion.Core.Tests/W3Tests.cs` | `NotificationReplayTests` 3 (C6: exactly one summary in the flood segment; the opt-in makes the replayed error time-sensitive; the quiet-day batch/rate-window shape), `LoginItemRegistrationTests` 2, `DndOptInSettingTests` 1 |

## 2. Tests

Mac (`just companion-windows-test`, .NET SDK 10.0.201): **Rpc 35/35, Core 101/101 — 136 passed, 0 failed** on the merged head
(W1b 124 + W2's 6 + W3's 6). W3's six are C# additions (the Swift companion has no replay type and registers its login item
through the shell): `NotificationReplayTests` ×3 — the flood segment of eleven blocking items posts exactly one summary and the
one-at-a-time segment posts 10 + 9 (re-added) and withdraws 10 (C6); the opt-in makes the replayed engine-lost error
`TimeSensitive`; the 130 W2 live shape (nine reviews + a decision) shows the immediate silent question, the held batch at the
breakpoint, the rate window and the +181 s batch — `LoginItemRegistrationTests` ×2 — `DndOptInSettingTests` ×1. The Policy (28),
Breakpoint and settings suites W1 ported are unchanged and green; `PolicySettings.cs` gained one key without breaking them.

## 3. build-B (Windows 11 Pro 25H2 26200, build service slot `w3b`, worktree `C:\Users\user\Develop\theuth-ci-w3b`)

| Step | Result |
|---|---|
| Debug build (`dotnet build -c Debug -p:Platform=x64`) | first W3 build `a505cf95` 14.4 s, **0 warnings / 0 errors**; merged head `2a63ce8e` 21.9 s, 0/0 |
| `build.ps1` Release publish | `target\release\companion-windows\` **482 files, 171.6 MB** (W2's tray icons added 22 files to W1's 460), the five runtime-MSIX files present, no onnxruntime/DirectML, **`Register()` ok `Setting=Enabled`** on the published layout at every publish (`250a895d`, `ef16beb5`, `2a63ce8e`) |
| `--probe-notify 12` (11 seeded decisions) | see §4.1 |
| Service `kill` and `build.ps1` | until W2's `a9c5da2f` (merged from main at `2a63ce8e`) every `build.ps1` killed all companions machine-wide; the UI round was run under the desk lock with that in mind |

## 4. Gates (§7 W3) with evidence

All UI runs through the `windows-build-b` MCP tools and `w3-gate.ps1` under the desk lock (owner `w3b`, 00:21–00:41), on the
published exe of `b9280313` (the toast-argument fix); the merged head `2a63ce8e` (main `473b7a0e`/`a11fce5f` folded in: W2's
`build.ps1` kill-scope fix and docs, no app code) was re-built 0/0, re-published (482 files, `Register()` ok) and re-gated in a
second desk turn (00:44–00:46): the 11-decision `--probe-notify 10` replay again gave `GetAllAsync total=2 [probe-all×1
probe-engine×1]` / `flood segment … in-centre=1 → exactly one summary (C6 ok)`, exit 0, and `--settings` opened the Rail page
with Side · Display ("Main") · Rail presence ("Sliver when quiet") and both captions (the `48e19c72` fix).

### 4.1 `--probe-notify` replay → toasts counted with `GetAllAsync` (C6)

`seed.ps1 decision --count 11` → `w3-gate.ps1 probe -Seconds 12` (`logs\probe-notify.log`, 00:29):

```
start probe-notify 12.0 s … prefs=(blocking: immediate, deferrable: atBreak, error: immediate, soundOnErrors: true, breakThroughDnd: false) registered=true setting=enabled urgentSupported=true
settings window shown page=notifications active=false
live items=11 [decision×11] classes=[blocking×11] new-while-running→posted={} flood=true held=0 paused=false connection=open breakpoints: switches=0 idle=1
replay all 11 at once: post=[batch:flood active] withdraw=[]
replay item 1 decision (blocking): post=[question:decision:… active] … item 10 …
replay tick +60 s (cap not elapsed, held=0) · breakpoint +61 s (held=0) · items re-added +70 s: post=[9 × question] · breakpoint +80 s · tick +181 s: post=[]
replay answered elsewhere (all gone): withdraw=[10 ids]
replay engine lost +183 s: post=[error:engine active sound]
GetAllAsync total=2 by-group=[probe-all×1 probe-engine×1] posted-by-segment=[all×1 engine×1 one×19]
flood segment (11 items at once): posted=1 in-centre=1 → exactly one summary (C6 ok)
done after 12.0 s — quitting 0
```

The live policy seeded on the first snapshot (eleven items already waiting → flood reading, nothing posted); the replay
posted the toasts for real: one flood summary ("theuth · 11 agents need you — open the panel"), nineteen per-item question toasts
in the one-at-a-time segment (the nine re-added ones replace their earlier toasts by tag) all withdrawn by "answered elsewhere",
and the engine-lost error toast ("theuth engine unreachable", Default scenario + sound — screenshot: a normal banner with Open).
The companion exited 0; `Unregister()` ran at exit.

### 4.2 A toast button answers a seeded decision on the scratch engine

`seed.ps1 decision --count 1` → `probe -Seconds 6 -Stay` → the decision toast ("theuth · decision — user asks for a decision on
'Seed decision 1: …'") with the Reply text box. Clicked into the box (mouse by screen coordinates), typed `LayoutB`
(`w3-gate.ps1 sendkeys` — `ShellExperienceHost` is not a `windows-mcp` target), clicked **Reply**:

```
notifications: NotificationInvoked thread=11 argument='action=reply;banner=decision:01a118b4-…;item=decision:01a118b4-…' input=1
notifications: action reply banner=decision:01a118b4-… items=[decision:01a118b4-…] text=7 chars
notifications: [receipt] post batch level=passive … title="theuth" body="replied — sent"
committed Decide { Decision = decision:2098cc93-…, Answer = LayoutB } ok=true
state attention_list=0 [] … headline="Nothing needs you"
```

`NotificationInvoked` on a background thread (11), marshalled, `AnswerNow` → `task_decide` → the item left `attention_list`
(seed `list` → 0), the receipt toast followed and was withdrawn after 8 s. The first attempt found a real bug: a button click
delivers only the button's arguments (not the toast's launch arguments), so `item=` was lost — fixed in `b9280313` (every button
repeats `banner` + `item`; a single-item banner id resolves to its item).

### 4.3 Do not disturb on → the error toast goes to the centre only

DND switched on in Settings › System › Notifications (`ui_click` on the "Do not disturb" button, the tray showed the moon),
opt-in off → `probe -Seconds 5 -Stay`: `[probe-engine] post error level=active sound=true … toastId=110` with no banner on
screen; the notification centre (clock click) listed "Do not disturb is on — You'll only see banners for priority notifications"
and, under **Theuth 12:40 AM**, "theuth engine unreachable — The companion lost the engine; reconnecting." DND switched off again.

### 4.4 Opt-in on → Urgent + Windows' one-time consent

Settings › Notifications › **Errors break through Do not disturb** switched on (`ui_click settings-notifications-urgent`):
`RequestUrgentConsent()` posted the Urgent toast (red "!" badge: "theuth companion — Errors will break through Do not disturb.
Windows asks once whether to allow that.") and Windows raised **"Important Notification Request — Allow important notifications
from Theuth? … when do not disturb is on" (Allow / Not Now)**; Allow pressed. The switch persists as `errorsBreakThroughDnd`
in settings.json; switched back off afterwards (the default, R2).

### 4.5 `ui_click("settings-<page>")` opens each page

`launch --settings` → window "theuth Companion" (`WinUIDesktopWin32WindowClass`); `ui_find(automationId="settings-<page>")`
found all six `NavigationViewItem`s (`settings-rail` · `settings-keys` · `settings-notifications` · `settings-undo` ·
`settings-login` · `settings-engine`), `ui_click` switched pages: Keys (every scope with its rows, chords "Ctrl+Shift+Space",
"unassigned", 1/2/3/Enter/J/K/S…, reset ↺ per row), Notifications (three pickers, Sound on errors, the DND switch + help text,
"Windows notifications for this app: enabled", Open Windows notification settings, pause row), Undo delay
(`settings-undo-slider` "Undo delay in seconds", `settings-undo-value` "3.0 s"), Launch at login (toggle + the live Run value
line), Engine (`ws://127.0.0.1:7777/`, open, data dir, client, hub, 0 · 1 · 3, version `2.12.1+b9280313…`, log path). Rail
rendered only its Side row on `b9280313` (a `Guard` exception, §6 item 7) — fixed in `48e19c72`.

### 4.6 Run value present/absent after the toggle, companion.json mirrors it

Settings › Launch at login toggled on → `w3-gate.ps1 reg` (a repo .ps1 through the build service's `script` action):

```
reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v TheuthCompanion
    TheuthCompanion    REG_SZ    "C:\Users\user\Develop\theuth-ci-w3b\target\release\companion-windows\Theuth.Companion.exe" --background
reg exit: 0 (0 = present, 1 = absent)
C:\Users\user\.theuth\companion.json = { "launch_at_login": true }
```

toggled off → `ERROR: The system was unable to find the specified registry key or value.` · `reg exit: 1` ·
`companion.json = { "launch_at_login": false }`. At every start `LoginItem.Reconcile` logs
`login item: status=not_registered run=absent file=C:\Users\user\.theuth\companion.json`.

### 4.7 Breakpoints

`breakpoints: WinEvent hook on EVENT_SYSTEM_FOREGROUND installed (own=…\Theuth.Companion.exe)` at every start;
`breakpoint: idle 78 s` fired once at the first 1 Hz sample of the untouched RDP session (the `idleFired` latch keeps it from
repeating); the probe line counts `breakpoints: switches=N idle=M`. An app switch is `breakpoint: app switch → <exe>` (seen
during the Settings round when the Settings app took the foreground).

## 5. Checks (§6)

| # | Windows form | Status |
|---|---|---|
| C6 | ≤ 1 deferrable toast per 2 min; flood above 10; blocking immediate; errors immediate with sound | Policy tests (28) + `NotificationReplayTests`; live: the flood segment yielded exactly one summary (§4.1) |
| C7 | quiet day = sliver, no gold chrome | W2's rail (its gate); W3 adds nothing to the rail's chrome |
| C13 | "Animation effects" / "Transparency effects" switches | not exercised in this round (the Settings window is a plain window; the rail/panel are W2's) — owed to the integration pass |

## 6. Deviations from the design and the brief

1. **The Windows `--probe-notify` posts its replay for real.** The Mac's is a dry run ("would post"); the W3 gate counts toasts
   with `GetAllAsync`, so the replay posts every step, one toast group per segment (`probe-all` · `probe-one` · `probe-engine`),
   and leaves them in the centre as evidence; `RemoveAllAsync` at the next probe's start and the first-snapshot reconcile clean
   them up. An **engine-lost segment** was added to the replay (not in the Swift one) so the error class — Default + sound, or
   Urgent — can be exercised without stopping the scratch engine.
2. **W3's Win32 calls are hand-written `DllImport`s in `Breakpoints/W3Native.cs`**, not CsWin32 `NativeMethods.txt` entries
   (design §5.2): W2 owns that file and the two waves ran in parallel; the integrator may fold the ~15 names into it.
3. **The opt-in is a settings.json key.** `CompanionSettings` gained `errorsBreakThroughDnd` (off by default) → `TimeSensitiveAvailable`;
   on the Mac that flag is an entitlement fact and was not persisted.
4. **Six Settings pages, not four tabs.** §4.6 lists Rail · Keys · Notifications · Undo delay · Launch at login · Engine, so the
   `NavigationView` has six items (the Swift app folded undo delay into Keys and launch at login into Notifications). The design's
   "show finished" rail switch has no Core key and no reader; not built. The Mac's "Enable notifications" button became the Windows
   setting line + "Open Windows notification settings" (`ms-settings:notifications`); pause controls sit on the page as well as in
   W2's tray menu.
5. **`--request-notifications` raises the urgent consent, not an authorization dialog** (Windows has none for normal toasts): it
   registers, logs the Setting, and posts one Urgent toast so Windows asks its one-time question; Settings' opt-in switch does the same.
6. **Receipt toasts are withdrawn after 8 s** (Windows keeps toasts in the centre until dismissed; the Mac's passive receipt stays).
   **Double-tap Alt has no mouse-down cancel** (the Mac cancelled on mouse-down too): only a WH_KEYBOARD_LL hook is installed.
7. **Two defects found and fixed by the gates.** (a) The first probe crashed with 0xC000027B in CoreMessagingXP: `foreach` over
   `DisplayArea.FindAll()` throws `InvalidCastException: ClassFactory cannot supply requested class` through CsWinRT, and an exception
   escaping a `DispatcherQueueTimer.Tick` fails fast without reaching `Application.UnhandledException` → every W3 callback now runs
   through `Guard` (logged stack, process stays up) and the display list is indexed. (b) A toast button's arguments replace the
   launch arguments, so `item=` must be on every button (§4.2).
8. **Integration edits outside W3's seams**: two public methods on W2's `Surfaces` (`OpenPanelOn`, `TogglePanel`) and the W2/W3
   ordering in `App.OnLaunched` (`Register()` → `Ui.Surfaces.Start` → W3 `Install` → `Store.Start()`); `CORE/NotificationReplay.cs`
   and `W3Tests.cs` are new Core files.

## 7. For W4 and W5

- **W4**: `Store.OpenItemRequested`/`RequestOpen` and the Notifier's `Receipt` handling share the Store's `Receipt` row with the Ask
  bar — a toast action sets `receiptFor` and consumes the next `Sent/Stale/Failed`; the Ask bar's `Reply` rows are ignored by it.
  `W3Flags.AnyProbe` covers `--probe` and `--probe-notify`; add `--ask-probe` to W1's `Flags` or your own flags class. The settings
  model is `Settings.SettingsModel.Shared` (`Update(edit)` saves and raises `Changed`); the Ask bar hotkey row is in Settings › Keys
  already. `Guard.Run` is the pattern for any WinRT callback (NAudio callbacks included).
- **W5**: the companion writes `HKCU\…\Run\TheuthCompanion = "<exe>" --background` itself and `<data_dir>\companion.json`
  (`{ "launch_at_login": bool }`); the NSIS uninstall hook must keep deleting exactly that value name. The AUMID is derived from
  the exe path — the install path must stay stable or Windows re-asks the urgent consent (and the register smoke's `UnregisterAll`
  removes the registration, so never run it against the installed exe). `--background` is accepted and logged; nothing else
  changes under it yet (W2's surfaces decide what a background start shows).
- **Integration**: the Rail page lists displays by `\\.\DISPLAYn` device name (`RailDisplay`); W2's rail should resolve that name or
  fall back to the primary display. The hotkey "held by another app" probe (`HotkeyProbe.CanRegister`) registers on the UI thread with
  id `0x7433`; W2's own probe id is `0x7FFF`. Stale consent toasts from the spike (`SpikeHost`) still sit in build-B's notification
  centre and steal the foreground for `keyboard_control` — click "Not Now" first.

## 8. Open items

- C13 (animation/transparency switches) and a Narrator pass over the Settings window were not run in W3's desk turn.
- The relaunch-by-click path (toast pressed while the companion is not running → `Kind=Launch`, arguments after `Register()`, the
  action held until the first refetch) is implemented and unit-reasoned but was not exercised live: every click in the round hit a
  running instance. `Redirected` (W2's single-instance redirect carrying `AppNotification` data) likewise.
- Approvals, asks, reviews and finished items cannot be seeded on the model-less scratch engine: their toast buttons (Allow · Deny
  with note · Approve · Send back · next steps · Clear) are built from the same card options as the panel and were not clicked live.