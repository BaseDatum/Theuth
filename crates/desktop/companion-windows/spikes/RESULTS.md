# Spike results — Windows companion feasibility (design 134)

Run 2026-10-07 20:41–21:07 on **build-B** (Windows 11 Pro 25H2 build 26200.9457 x64, Sky's RDP session 1,
2016×1300 at 96 dpi / scale 1.00, VS 2026 + WinUI workload, .NET SDK 10.0.201). App: `SpikeHost/`
(unpackaged, self-contained WinUI 3, .NET 10, Windows App SDK 1.8.260317003), built with
`dotnet build -c Debug -p:Platform=x64` (12–20 s; first build 18 s). Driven over `windows-mcp`
(UI Automation + injected input). Every log line below is from `%LOCALAPPDATA%\theuth-spikes\<mode>.log`,
read back through the `SpikeLog` TextBox. Final code: branch `spike/134-windows`.

| spike | verdict |
|---|---|
| S0 smoke (toolchain + UIA) | **works** — on WASDK 1.8, on 2.5.1 and on the 1.8 "WinUI component only" split |
| S1 rail (edge tool window) | **works with caveats** — size/topmost/no-taskbar/capture-exclusion/pass-through all work; **WS_EX_NOACTIVATE is not honoured by the WinUI island** (a click activates the rail) and must be worked around |
| S2 toasts (unpackaged, self-contained) | **works** on the full 1.8 package — buttons, urgent scenario, `NotificationInvoked`; **does not work on 2.5.1 or the 1.8 split** (`Register()` → 0x8007007E missing `Microsoft.WindowsAppRuntime.Insights.Resource.dll`) |
| S3 hotkey + tray + focus | **works** — `RegisterHotKey`, `Shell_NotifyIcon`, cross-process `SetForegroundWindow` first try, Esc restores the previous foreground |
| S4 foreground hook + idle | **works** — `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` across processes, `GetLastInputInfo` |

## Project facts (all spikes)

- Hand-written csproj works without `dotnet new` templates: `net10.0-windows10.0.22621.0`,
  `TargetPlatformMinVersion 10.0.19041.0`, `UseWinUI`, `WindowsPackageType=None`,
  `WindowsAppSDKSelfContained=true`, `SelfContained=true`, `RuntimeIdentifier win-x64`, `Platforms x64`,
  `EnableMsixTooling=true` (harmless, template default), `app.manifest` with `PerMonitorV2`.
  `Microsoft.Windows.SDK.BuildTools` need not be referenced (0 warnings). Windows in code (no XAML pages)
  derived from `Microsoft.UI.Xaml.Window` compile and run; only `App.xaml` exists.
- Runtime log line: `Microsoft.WinUI 3.0.0.0; .NET 10.0.5`. `GetCurrentProcessExplicitAppUserModelID` is
  unset (hr 0x80004005) in every mode, even after `AppNotificationManager.Register()`.
- Compile-time gotchas hit: `Grid.SetRow` wants a `FrameworkElement` (not `UIElement`); variant builds with
  `-p:BaseOutputPath=bin-x\ -p:BaseIntermediateOutputPath=obj-x\` pull the default `obj\` tree into the
  `Compile` glob (duplicate `App.g.i.cs`, `AssemblyInfo`) unless
  `<DefaultItemExcludes>$(DefaultItemExcludes);bin/**;obj/**;bin-*/**;obj-*/**</DefaultItemExcludes>`.
  A running `SpikeHost.exe` locks the output and fails the copy step (MSB3021/MSB3027) — kill before building.
- Sizes (Debug, win-x64 dir, PDBs included) — full 1.8 meta package: 508 files / 214.8 MB (Release publish);
  2.5.1 meta: 519 files / 229.2 MB; 1.8 **WinUI component only**: 452 files / **165.1 MB**, no
  `onnxruntime.dll` (20.7 MB) / `DirectML.dll` (17.8 MB). Largest remaining files: `Microsoft.Windows.SDK.NET.dll`
  24.9 MB, `System.Private.CoreLib.dll` 15.3 MB, `Microsoft.ui.xaml.dll` 14.6 MB.
- Test-harness facts: an **elevated foreground window (auto-elevated Task Manager) silently discards injected
  input from a Medium-integrity process (UIPI)** — `SendInput` moves/clicks "succeed" but the cursor does not
  move, `SetForegroundWindow` from the tool fails, and `Window.Activate()` of a freshly launched Medium app does
  not take the foreground; UIA reads keep working. windows-mcp's `window_management list` ignores its
  `processName` filter (filter client-side) — closing "every listed window" killed the gateway's own pwsh console.
  Hidden/`WS_EX_NOACTIVATE` tool windows are not reliably closable with `window_management close`; kill by process.

## S0 smoke — works

- `SmokeButton` (Invoke via UIA) → `SmokeText` = `clicked 1` on all three builds. Log:
  `S0 window shown … dpi=96 scale=1.00 rect=(628,366)-(1388,886) 760x520px exstyle=0x00000100 []`.
- `AppWindow.MoveAndResize` is in physical pixels; `DisplayArea.Primary.WorkArea` = 2016×1252 (taskbar 48 px).

## S1 rail — works with caveats

Code: `RailSpike.cs`. Window: `OverlappedPresenter.Create()` + `SetBorderAndTitleBar(false,false)`,
`IsResizable/IsMaximizable/IsMinimizable=false`, `IsAlwaysOnTop=true`, `AppWindow.IsShownInSwitchers=false`,
`SetWindowLongPtr(GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST)` before the first show,
`SetWindowPos(HWND_TOPMOST, …FRAMECHANGED|NOACTIVATE)`, `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`,
shown with `AppWindow.Show(false)` (never `Window.Activate()`).

Evidence:

- **Styles stick**: `exstyle before=0x00000108 [TOPMOST] … after=0x08000188 [TOPMOST|TOOLWINDOW|NOACTIVATE]`
  (WinUI itself sets `WS_EX_TOPMOST` for `IsAlwaysOnTop`; nothing resets our bits later).
- **Minimum size trap**: the first build asked for 26×400 and got `136x400px` — the default
  `WM_GETMINMAXINFO` answer clamps a frameless top-level to `SM_CXMINTRACK` (136 px). Fixed by
  `OverlappedPresenter.PreferredMinimumWidth/Height = 26/400` (compiles on 1.8) **plus** a `WM_GETMINMAXINFO`
  subclass handler installed on the top-level HWND *before* `SetPresenter`/`MoveAndResize`; result
  `got (1990,426)-(2016,826) 26x400px`. (`WM_GETMINMAXINFO` is the only message the top-level HWND received
  before showing.)
- **Capture exclusion works**: `SetWindowDisplayAffinity(0x11) -> True; GetWindowDisplayAffinity -> 0x11`.
  A region screenshot of the right edge (GDI capture by windows-mcp) is black where the rail sits; the control run
  (`--rail hittest capturable`, affinity not set) shows the orange 26×400 strip with the three marks
  (WinUI draws rounded corners even on a frameless window). Works in an RDP session.
- **Mouse pass-through**:
  - `WM_NCHITTEST → HTTRANSPARENT` (SetWindowSubclass on the top-level and the three WinUI child HWNDs
    `InputNonClientPointerSource` / `Microsoft.UI.Content.DesktopChildSiteBridge` / `InputSiteWindowClass`):
    **does not pass clicks to other processes.** The hit test arrives on the `DesktopChildSiteBridge` child first,
    then the top-level; returning HTTRANSPARENT from both makes our own `WindowFromPoint` report the desktop
    (`explorer SysListView32 "FolderView"`), but the click still landed on the rail and activated it
    (HTTRANSPARENT only forwards within the same thread).
  - `WS_EX_LAYERED | WS_EX_TRANSPARENT` with `SetLayeredWindowAttributes(0,255,LWA_ALPHA)`, toggling
    `WS_EX_TRANSPARENT` from a 40 ms `DispatcherQueueTimer` cursor poll (`GetCursorPos` vs mark rects):
    **works.** Non-mark click → `under point=Program Manager`, click reached the desktop, rail untouched; mark
    click → `layered toggle … WS_EX_TRANSPARENT OFF` then `mark 2 clicked`. The layered WinUI window renders
    normally (screenshot of `--rail layered capturable`).
  - `SetWindowRgn` clipped to the three mark rects: **works** (`SetWindowRgn -> 1`; non-mark click went to
    the desktop, mark click recognised). No polling, but the visible shape is the marks only.
- **Activation — the failure**: in every variant a click on a mark activated the rail
  (`WM_NCACTIVATE`/`WM_ACTIVATE wParam=1` on the top-level, `WM_SETFOCUS` on `InputSiteWindowClass`,
  `GetForegroundWindow()` = rail, `foregroundChanged=True`) despite `WS_EX_NOACTIVATE`. **`WM_MOUSEACTIVATE`
  never reaches any of the four window procedures**, and neither do `WM_LBUTTONDOWN`/`WM_POINTER*` — the
  Windows App SDK input stack takes pointer input before `DispatchMessage` (`ContentPreTranslateMessage`) and
  activates the island's top-level itself. `WM_MOUSEMOVE`/`WM_SETCURSOR`/`WM_NCHITTEST` do pass through the
  subclass. `IsHitTestVisible=false` on the XAML root (`nohit`) does **not** prevent the activation.
  Working mitigation (`restore`): in the top-level subclass, on `WM_ACTIVATE` with `LOWORD(wParam)!=0`, call
  `DefSubclassProc` then `SetForegroundWindow(previousForeground)` (allowed, we are foreground at that instant):
  `restore: WM_ACTIVATE(wParam=0x1) -> SetForegroundWindow(prev … "SpikeHost S0") = True … restored=True`,
  then `mark 2 clicked … railIsForeground=False foregroundChanged=False`; the other process never showed as
  losing the foreground to an external observer, but it does receive a deactivate/activate pair (focus flicker).
- Child HWNDs enumerate with `EnumChildWindows` right after `Loaded`; `SetWindowSubclass` succeeds on all.
- Variants and tokens: `--rail [hittest|layered|region|all] [capturable] [restore] [nohit]`.

**Implication for the design**: a WinUI/XAML window cannot be a true non-activating rail; the island activates
on press. Choose one of: (1) rail = plain Win32 HWND (`WS_EX_NOACTIVATE|TOOLWINDOW|TOPMOST|LAYERED`) drawn
with Direct2D/GDI or a `Microsoft.UI.Composition` `DesktopWindowTarget`, XAML only for panels — the robust
answer, same shape as the Tauri island; or (2) WinUI rail + `restore` on `WM_ACTIVATE` + layered toggle (or
region) for pass-through — simplest, with a sub-frame focus flicker in the user's app. Either way: set the ex
styles before the first show, answer `WM_GETMINMAXINFO`, use `AppWindow.Show(false)`, `WDA_EXCLUDEFROMCAPTURE`
is a one-liner that works.

## S2 toasts — works on the full 1.8 package; fails on 2.5.1 and on the split package set

Code: `ToastSpike.cs`. Order (per Microsoft Learn quickstart): subscribe `NotificationInvoked` →
`AppNotificationManager.Default.Register()` → `AppInstance.GetCurrent().GetActivatedEventArgs()` → `Show()`.
For an unpackaged app `Register()` needs nothing else (no manifest, no AUMID, no COM registration by hand): it
creates the AUMID/CLSID registration under `HKCU\Software\Classes` pointing the activator at this exe with
`----AppNotificationActivated:`; display name = exe name ("SpikeHost"), default icon. `Register(displayName,
iconUri)` exists for a custom name/icon (not used here). Learn: when the app is **not** running, a click
relaunches it and the activation kind is reported as `Launch`, not `AppNotification`; the arguments then come
through `NotificationInvoked` after `Register()`; `Register()` must precede `GetActivatedEventArgs()`.

Evidence (1.8 full):

- `IsSupported()=True`, `Register() ok`, `Setting = Enabled`, `GetActivatedEventArgs().Kind = Launch`,
  `Show(normal) ok id=53`, `IsUrgentScenarioSupported()=True`, `Show(urgent) ok id=54`.
- Screenshot: both toasts rendered bottom-right with Approve/Deny buttons; the **urgent** one carries a red
  "!" and Windows showed a separate consent toast *"Important Notification Request — Allow important
  notifications from SpikeHost? You will get important notifications from SpikeHost when do not disturb is on"*
  (Allow / Not Now) — urgent breakthrough needs a one-time user consent.
- Clicking **Approve** on the urgent toast (mouse, at the toast's screen position):
  `NotificationInvoked thread=3 argument='action=approve;spike=s2-urgent' {action=approve, spike=s2-urgent}
  input={}` — **the handler runs on a background thread**, dispatch to the UI thread yourself.
- Toasts live in `ShellExperienceHost` "New notification" (not enumerable as a top-level window via
  windows-mcp; UIA click of the button was not reachable — mouse coordinates were). Banners stay ~6 s.
- GitHub issue microsoft/WindowsAppSDK#6821 (2026-09-29, same OS build 26200.9457, WASDK 2.4/2.5.1 official
  unpackaged sample): Register/Show succeed, Setting Enabled, **nothing displayed**. Not reproduced here on 1.8:
  the toasts displayed. Keep this in mind when moving to 2.x.
- Not tested: relaunch-by-toast after exit, `UnregisterAll()`, input (text box) actions.

Evidence (2.5.1 meta and 1.8 split): `Register EXCEPTION hr=0x8007007E COMException: The specified module could
not be found. Unable to load resource dll. Microsoft.WindowsAppRuntime.Insights.Resource.dll` — self-contained
output lacks that DLL; `Setting` still reads `Enabled`, nothing is shown. File diff of the output dirs against
the working full-1.8 dir: both failing builds lack the same five deployment/insights files —
`Microsoft.WindowsAppRuntime.Insights.Resource.dll` (~343 KB), `WindowsAppRuntime.DeploymentExtensions.OneCore.dll`,
`WindowsAppRuntime.png`, `WindowsAppSdk.AppxDeploymentExtensions.Desktop.dll`,
`WindowsAppSdk.AppxDeploymentExtensions.Desktop-EventLog-Instrumentation.dll`. None of them exists as a loose
file anywhere in the NuGet cache: they ship **inside the runtime MSIX** of `Microsoft.WindowsAppSDK.Runtime`
(`tools\MSIX\win10-x64\Microsoft.WindowsAppRuntime.1.8.msix`, 290 entries, and `…Runtime.2.msix` for 2.5.1,
302 entries — both lines contain all five). The self-contained payload step of the full 1.8 meta build copies
them out; the 2.5.1 meta build and the 1.8 split build do not (2.5.1's output instead adds 18 AI/Search files).
**Confirmed fix**: copying the five files from the full-1.8 output into the split output made `Register() ok`,
`Show(normal) ok id=60`, `Show(urgent) ok id=61` and the toasts appeared (new AUMID → the urgent consent prompt
again). So a slim deployment = component packages + a build step that extracts those five entries from the
runtime MSIX (or ships them as content).

**Implication**: toasts with action buttons from an unpackaged self-contained exe are viable today on WASDK 1.8
(full package). For 2.x or a slimmed package set, either ship `Microsoft.WindowsAppRuntime.Insights.Resource.dll`
alongside (verify which component package carries it) or go framework-dependent; re-test on the target OS build
because of #6821. Design the handler for a non-UI thread and for the relaunch path (`----AppNotificationActivated:`
argument, `Kind = Launch`).

## S3 hotkey + tray — works

Code: `HotkeyTraySpike.cs`. `Shell_NotifyIconW` via P/Invoke (`NOTIFYICONDATAW` 976 bytes, `NIF_MESSAGE|ICON|TIP|
SHOWTIP`, `NIM_SETVERSION 4`, `LoadIcon(IDI_APPLICATION)`), callback `WM_APP+1` and `RegisterHotKey(hwnd, id,
MOD_CONTROL|MOD_SHIFT|MOD_NOREPEAT, VK_SPACE)` both on the log window's HWND, whose procedure is subclassed with
comctl32 `SetWindowSubclass`. No library (H.NotifyIcon not needed; it is MIT-licensed, 2.x, if a context menu
and icon resources are wanted later).

Evidence:

- `SetWindowSubclass -> True`, `RegisterHotKey -> True err=0`, `Shell_NotifyIcon(NIM_ADD) -> True`,
  `NIM_SETVERSION 4 -> True`.
- Ctrl+Shift+Space with another **process** in the foreground (a second SpikeHost, pid 6256):
  `WM_HOTKEY received`, `foreground before = … pid=6256 "SpikeHost S0"`, `AppWindow.Show(true) +
  SetForegroundWindow -> True err=0; foreground now = … "SpikeHost S3 panel" isPanel=True`, `+300ms … isPanel=True;
  TextBox.Focus()=True FocusState=Keyboard`. **No fallback (ALT tap / AttachThreadInput) was needed**: the
  thread that receives `WM_HOTKEY` has foreground permission. Typing landed in the panel's TextBox (`hello`,
  read back over UIA).
- Esc (`UIElement.PreviewKeyDown`): `AppWindow.Hide()` then `SetForegroundWindow(prev) -> True … restored=True`
  back to the other process.
- Cleanup on close: `UnregisterHotKey`, `NIM_DELETE`, `RemoveWindowSubclass` (run on `Window.Closed`).
- Panel: `OverlappedPresenter` with border but no title bar, `IsAlwaysOnTop`, `IsShownInSwitchers=false` +
  `WS_EX_TOOLWINDOW` → no taskbar button; created hidden, shown with `AppWindow.Show(true)`.
- Not tested: the tray left-click path (the icon lands in the taskbar overflow; driving the shell's overflow UI
  was out of bounds). Hotkey while an elevated window is foreground was not tested (Task Manager was closed by
  then); expect `WM_HOTKEY` to arrive but `SetForegroundWindow` to be subject to UIPI.

**Implication**: hotkey + panel focus is straightforward; keep the hotkey HWND alive for the app's lifetime,
subclass it, and record the previous foreground on `WM_HOTKEY` for Esc.

## S4 signals — works

Code: `SignalsSpike.cs`. `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0, proc, 0, 0,
WINEVENT_OUTOFCONTEXT)` on the UI thread (delegate kept in a field), `GetLastInputInfo` every 2 s, stop after 60 s.

Evidence: hook `-> 0x660517 err=0`; alternating activations between a second process and our two windows gave
one callback each, on our UI thread: `foreground -> 0x30424 pid=6256 SpikeHost … "SpikeHost S0" (event
thread=9728, callback thread=1744)`, `… "SpikeHost S4 second"`, etc.; `60 s over: UnhookWinEvent -> True`.
Idle: `idle 131.9s` while input was blocked by UIPI, `idle 0.4s` right after injected keystrokes, then
+2.0 s per tick — window activations by `SetForegroundWindow` do **not** reset the idle counter (only real/
injected input does).

**Implication**: both signals are free (no permissions, no elevation), work in an RDP session and across
processes; the hook callback already arrives on the hooking thread's message loop.

## Addendum A1 — Windows App SDK 2.x vs 1.8, and the ML component

- Latest stable on nuget.org (2026-10-07): **Microsoft.WindowsAppSDK 2.5.1** (components Base 2.0.4, Foundation
  2.3.12, InteractiveExperiences 2.1.9, WinUI 2.3.9, DWrite 2.1.0, Widgets 2.0.5, AI 2.5.5, ML 2.1.94, Search
  2.5.5, Runtime 2.5.1). 1.8.260317003 = Base 1.8.251216001, Foundation 1.8.260222000, InteractiveExperiences
  1.8.260125001, WinUI 1.8.260224000, DWrite, Widgets, AI 1.8.53, ML 1.8.2141, Runtime.
- Switch in the csproj: `-p:WasdkLine=2` (2.5.1 meta; defines `WASDK2`), `-p:WasdkSplit=true` (reference only
  `Microsoft.WindowsAppSDK.WinUI`, which depends on Base + Foundation + InteractiveExperiences + WebView2).
  Both built with **0 warnings / 0 errors, no source change** (`PreferredMinimumWidth/Height`,
  `AppNotification*`, `AppInstance`, `OverlappedPresenter`, `DisplayArea` identical).
- Runtime: S0 works on all three. **S2 `Register()` fails on 2.5.1 and on the 1.8 split** (missing
  `Microsoft.WindowsAppRuntime.Insights.Resource.dll`, see S2). S1/S3/S4 were run on 1.8 full; S3 also ran on the
  1.8 split (hotkey/tray/focus fine).
- The `onnxruntime.dll` + `DirectML.dll` (38 MB) come from `Microsoft.WindowsAppSDK.ML` (which depends on
  `Microsoft.Windows.AI.MachineLearning`), pulled only by the meta package. Excluding it = reference component
  packages instead of the meta package: `Microsoft.WindowsAppSDK.WinUI` (+ `Microsoft.WindowsAppSDK.Foundation`
  explicitly if you want its version pinned). Measured: 165.1 MB vs 229.2 MB (Debug dirs). Caveat: add the five
  deployment/insights files from the runtime MSIX (see S2) or `AppNotificationManager.Register()` fails — verified
  that with those five files copied in, toasts work on the split build.
- **Recommendation**: build on 1.8 (toasts verified, no 2.x-only API needed by the spikes); treat 2.5.1 as a
  drop-in that compiles but needs the same five-file fix for toasts and a re-test against WindowsAppSDK#6821 on
  the target OS build.

## Addendum A2 — toasts on Windows App SDK 2.5.1 with the five runtime-MSIX files (decides the 2.x question)

Both 2.5.1 builds re-run on build-B at 21:21–21:22 after the five files (`Microsoft.WindowsAppRuntime.Insights.Resource.dll`,
`WindowsAppRuntime.DeploymentExtensions.OneCore.dll`, `WindowsAppRuntime.png`,
`WindowsAppSdk.AppxDeploymentExtensions.Desktop.dll`, `…Desktop-EventLog-Instrumentation.dll`) were extracted from
**2.5.1's own** `microsoft.windowsappsdk.runtime\2.5.1\tools\MSIX\win10-x64\Microsoft.WindowsAppRuntime.2.msix`
into the output directory:

| build | Register() | toasts on screen | Approve → NotificationInvoked |
|---|---|---|---|
| (a) full 2.5.1 meta (`-p:WasdkLine=2`, bin-wasdk2, 519 files / 229 MB) | `Register() ok`, `Setting = Enabled` | both visible (normal + urgent with the "!" badge; urgent consent toast raised) — `Show(normal) ok id=63`, `Show(urgent) ok id=64` | `NotificationInvoked thread=3 argument='action=approve;spike=s2-normal' {action=approve, spike=s2-normal}` |
| (b) trimmed 2.5.1 (`-p:WasdkLine=2 -p:WasdkSplit=true`, bin-split2, 452 files / 169.6 MB, no onnxruntime/DirectML; Base 2.0.4 + Foundation 2.3.12 + WinUI 2.3.9 + InteractiveExperiences 2.1.9, NU1603 warning for 2.1.8) | `Register() ok`, `Setting = Enabled` | both visible — `Show(normal) ok id=73`, `Show(urgent) ok id=74` | `NotificationInvoked thread=3 argument='action=approve;spike=s2-normal' {action=approve, spike=s2-normal}` |

So microsoft/WindowsAppSDK#6821 ("2.x unpackaged toasts never display") did **not** reproduce on this box with
either 2.5.1 build once the five files were present; the only 2.x defect found is the missing payload copy for
self-contained output (also missing for any component-only reference on 1.8). Every API the spikes use compiled
unchanged on 2.5.1 (`PreferredMinimumWidth/Height`, `AppNotification*`, `AppInstance`, `OverlappedPresenter`,
`DisplayArea`, `AppWindow.Show(bool)`), and S0 ran. **Verdict for the design**: 2.5.1 is usable for the companion
provided the build adds a step that extracts those five entries from the runtime MSIX (`tools\MSIX\win10-x64\…msix`
is a zip) into the output — the same step the trimmed package set needs on either line; without it toasts fail with
0x8007007E. Side observation: each distinct exe path registers a new AUMID and the urgent scenario asks the user
once per AUMID ("Important Notification Request"), so keep the install path stable.
