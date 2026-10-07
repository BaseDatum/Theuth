# Windows companion spikes (design 134)

`SpikeHost/` is ONE unpackaged, self-contained WinUI 3 app (C#, .NET 10, Windows App SDK 1.8) that
runs one of five modes, chosen by its first `--` argument. Each mode shows its log in a read-only
TextBox (UI Automation id `SpikeLog`) and appends it to `%LOCALAPPDATA%\theuth-spikes\<mode>.log`.

| arg | spike | log file | what it proves |
|---|---|---|---|
| *(none)* | S0 smoke | `smoke.log` | toolchain + UIA: button `SmokeButton` sets `SmokeText` to `clicked N` |
| `--rail [hittest|layered|region|all]` | S1 rail | `rail.log` | 26×400 dip tool window on the right edge: no frame, no taskbar button, topmost, `WS_EX_NOACTIVATE`, capture-excluded, mouse pass-through except over 3 marks |
| `--toast` | S2 toast | `toast.log` | `AppNotificationManager` from an unpackaged self-contained exe: Approve/Deny buttons, urgent scenario, `NotificationInvoked` |
| `--hotkey-tray` | S3 | `hotkey-tray.log` | `Shell_NotifyIcon` tray icon + `RegisterHotKey` Ctrl+Shift+Space → panel to foreground with keyboard focus; Esc restores the previous foreground |
| `--signals` | S4 | `signals.log` | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` + `GetLastInputInfo` every 2 s for 60 s |

## Build (Windows 11, VS 2026 + WinUI workload, .NET SDK 10)

```powershell
cd crates\desktop\companion-windows\spikes\SpikeHost
dotnet build -c Debug -p:Platform=x64
# → bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\SpikeHost.exe
```

No `dotnet new` template is used; the csproj is hand-written:
`WindowsPackageType=None` (unpackaged), `WindowsAppSDKSelfContained=true` + `SelfContained=true`
(the Windows App Runtime and .NET are copied next to the exe, nothing to install),
`TargetFramework=net10.0-windows10.0.22621.0`, `TargetPlatformMinVersion=10.0.19041.0`,
`app.manifest` with PerMonitorV2 DPI awareness. It cannot be built on macOS/Linux (the XAML
compiler and the Windows SDK projections are Windows-only).

## Run

```powershell
$exe = "bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\SpikeHost.exe"
& $exe                      # S0
& $exe --rail               # S1, WM_NCHITTEST→HTTRANSPARENT variant (default)
& $exe --rail layered       # S1, WS_EX_LAYERED|WS_EX_TRANSPARENT toggled by a cursor poll
& $exe --rail region        # S1, SetWindowRgn clipped to the three marks
& $exe --toast              # S2
& $exe --hotkey-tray        # S3
& $exe --signals            # S4 (stops itself after 60 s; close the window to exit)
Get-Content $env:LOCALAPPDATA\theuth-spikes\*.log
```

Every mode exits when its main/log window is closed (S3 unregisters the hotkey and removes the
tray icon first). S2 leaves the toast registration in `HKCU\Software\Classes` (press
`UnregisterAll` in its window to remove it).

## Files

- `SpikeHost.csproj`, `app.manifest`, `App.xaml(.cs)` – project, DPI manifest, mode dispatch
- `Native.cs` – every P/Invoke used (user32/comctl32/shell32/gdi32/kernel32)
- `SpikeLog.cs`, `Ui.cs`, `LogWindow.cs` – log sink + file, helpers, the shared log window
- `SmokeSpike.cs`, `RailSpike.cs`, `ToastSpike.cs`, `HotkeyTraySpike.cs`, `SignalsSpike.cs` – S0..S4
- `../RESULTS.md` – verdicts and evidence from build-B
