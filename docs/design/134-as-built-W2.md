# 134 — as-built W2: tray, rail with focus hand-back, Attention panel, legend, hotkeys, single instance, `theuth://` links

Branch `feat/134-w2-surfaces` (from main `a1cbe9fa`), worktree `~/Develop/theuth-134-w2`. Built and driven on build-B (Windows 11
Pro 25H2 26200, RDP session, 2016 × 1300 at 96 dpi) through the build service (slot `w2`) and `windows-build-b`, against the
scratch engine 2.12.0 with items from `seed.ps1`. `WIN/` = `crates/desktop/companion-windows/`, `APP/` = `WIN/src/Theuth.Companion/`.

## 1. File map

| Path | What |
|---|---|
| `APP/Interop/NativeMethods.txt` | The whole Win32 surface (CsWin32 0.3.358 generates `PInvoke.*` and the types; nothing hand-declared): windows/focus, ex-styles, layering, capture exclusion, DPI, comctl32 subclassing, the host window, hotkeys, keys, tray, the tray's popup menu |
| `APP/Interop/Win32.cs` | Helpers: HWND of a WinUI window, ex-style get/set, `IsOurs`/`IsShellChrome` (taskbar, overflow, shell popups), `Describe` for logs, rects, cursor, `Foreground(hwnd)`, `OurWindows()` (probe) |
| `APP/Interop/HostWindow.cs` | Hidden top-level window of class **`TheuthCompanionRail`** on the UI thread: the shell's `WM_CLOSE` target (W5 `FindWindowW(class)`), the tray callback (`WM_APP+1`, version-4 LOWORD/anchor), `WM_HOTKEY`, `TaskbarCreated`, `WM_SETTINGCHANGE`/`WM_DISPLAYCHANGE` |
| `APP/Tray.cs` | `Shell_NotifyIcon` NIM_ADD + NIM_SETVERSION 4 on the host window, sprite per count (`Assets/tray/tray-<dark|light>-<0…9|9plus>.ico`, light/dark taskbar from `SystemUsesLightTheme`), tooltip "theuth · N need you", `Shell_NotifyIconGetRect` for the panel anchor, left click / Enter → panel, right click / menu key → menu, re-added on `TaskbarCreated`, NIM_DELETE on quit |
| `APP/Assets/tray/` | 22 ICOs (PNG entries 16/20/24/32/40/48 px) + `make-tray-sprites.swift` (CoreGraphics on the Mac: the `tray.svg` glyph + the gold ●N badge, palette gold `#c47a2c`/`#b4531e`) |
| `APP/Hotkeys.cs` | `RegisterHotKey` for every `KeyScope.Anywhere` row of the key table, `MOD_NOREPEAT` added to the chord's `MOD_*`; `KeyTable.ResolveGlobal` with a register/unregister probe (saved → default with a notice → none); `Pressed`, and `Released` by 50 ms `GetAsyncKeyState` polling (W4's hold-to-talk) |
| `APP/Windows/ToolWindow.cs` | Base of every surface window: `OverlappedPresenter` without border/title, not resizable, `IsAlwaysOnTop`, `IsShownInSwitchers=false`, `WS_EX_TOOLWINDOW|TOPMOST` + caller's ex-styles **before the first show**, `SetWindowSubclass` installed **before** `SetPresenter`/`MoveAndResize` answering `WM_GETMINMAXINFO` with the requested size + `PreferredMinimumWidth/Height` (the 136 px clamp), `WM_ACTIVATE` hook, `WDA_EXCLUDEFROMCAPTURE` unless `--shareable`, Closing → hide |
| `APP/Windows/RailWindow.cs` | The rail (§4.1): `WS_EX_NOACTIVATE|LAYERED|TRANSPARENT`, 26 epx at the right/left edge of `DisplayArea.Primary.WorkArea`, vertically centred, height = content; wordmark (dim while the engine is away) · gold count · needs-you marks · hairline · agent marks · `+N` · connection dot; 4 × 120 epx sliver per `RailCollapse` (presence/side from settings.json); 40 ms poll: last foreign foreground, hover expand, `WS_EX_TRANSPARENT` off over a mark row only; **hand-back** on `WM_ACTIVATE` |
| `APP/Windows/MarkGlyph.cs` | The six glyphs (gold ring notched at the kind's clock, faience disc with the 380 ms beat — static when `UISettings.AnimationsEnabled` is off, idle ring, malachite tick, carnelian bar, dashed sand ring), the wordmark |
| `APP/Windows/PeekCardWindow.cs` | Agent-mark peek card: name · state · title/open tasks · Open session; `WS_EX_NOACTIVATE`, shown passive, own hand-back; closes on an outside press, a second click, or 8 s after the pointer left |
| `APP/Windows/AttentionPanelWindow.cs` | The panel (§4.2): 372 × ≤560 epx, Mica (opaque when unsupported/transparency off), `TOOLWINDOW|TOPMOST` only; header → card → ≤6 lines → Done zone (24 h `user@companion` receipts) → hints → receipt row; options are Buttons `option-<n>` named "<label>, key <n>" (`option-open` "Open in theuth, key O" for option-less cards); note field (send back), R field (`Store.SubmitReply`); `PreviewKeyDown` → `KeyInput` → `KeyGrammar.IntentFor(…, Store.Keys)` → `Store.Perform`; opens activated, Esc/`CloseRequested` hides and gives the foreground back, focus loss hides; anchors tray / rail mark / top-centre ≈20 % down |
| `APP/Windows/LegendWindow.cs` | Keyboard shortcuts rendered from the key table by scope (aliases, unassigned, hold-to-talk, hotkey notice, "Change in Settings… Ctrl+,"), activated, Esc / Ctrl+/ / ? close and give the foreground back |
| `APP/Windows/TrayMenu.cs` | The tray menu as a Win32 popup (`TrackPopupMenuEx`, see Deviations): Open theuth · Ask theuth… · Pause notifications ▸ · Launch at login ✓ · Keyboard shortcuts…  Ctrl+/ · Settings… · Quit theuth |
| `APP/Windows/KeyInput.cs` | WinUI `KeyDown` → Core `KeyPress`: VK, `NumpadEnter` from the extended flag, modifiers from key states, chars by `ToUnicodeEx` with Shift/Caps only (flag 0x4) — AppKit's `charactersIgnoringModifiers` |
| `APP/Windows/Links.cs` | `theuth://` via `Launcher.LaunchUriAsync` (ShellExecute fallback) after `AllowSetForegroundWindow(ASFW_ANY)`; Open theuth = `..\theuth-desktop.exe` beside `companion\` else `theuth://new` |
| `APP/Windows/Palette.cs` | The palette as ONE `ResourceDictionary` of theme brushes (Dark/Light `ThemeDictionaries`, keys `TheuthGold` …) merged into `Application.Resources`; code reads the same dictionary per `ActualTheme` |
| `APP/Windows/SingleInstance.cs` | `AppInstance.FindOrRegisterForKey("io.theuth.companion")`; a second process grants `AllowSetForegroundWindow`, redirects its activation off the STA, logs, returns 0; `--register-smoke` never redirects |
| `APP/Windows/Surfaces.cs` | Composition root: settings (keys, undo delay, presence, side) → host, tray, hotkeys, rail, peek, panel, legend, menu; Store events; redirected activation → panel; `--probe` / `--probe-legend` lines; hooks for W3/W4 |
| `APP/Store.W2.cs` | `Store.Keys` (the one key table, raises `Keys`), `Store.HotkeyNotice`, `OldestNeedsYou`, `FocusOldest()` |
| `APP/Program.cs` (W2 line) | `if (!Ui.SingleInstance.Claim(args)) return 0;` before any XAML |
| `APP/App.xaml.cs` (W2 block) | The log window only under `--probe`; `Ui.Surfaces.Start(Store, dispatcher, Quit)` before `Store.Start()`; `Ui.Surfaces.Current?.Dispose()` in `Quit` |
| `APP/Theuth.Companion.csproj` (W2 group) | `AllowUnsafeBlocks` (CsWin32's pointers), `Microsoft.Windows.CsWin32` (`$(CsWin32Version)` in `WIN/Directory.Build.props`), `AdditionalFiles Interop\NativeMethods.txt`, tray ICOs as Content |
| `WIN/src/Theuth.Companion.Core/{CardModel,StoreModel,StoreEngine}.cs` | Hire (1 Hire · 2 Decline) and blocked (1 Unblock) per-kind options from 130 §4.2 through the same hold → E5 re-read → send (`agent_approve`, `agent_remove`, `task_move todo`); `IStoreEngine` default members so other engines still compile |
| `WIN/tests/Theuth.Companion.Core.Tests/W2Tests.cs` | 6 tests (cards, hold → re-read → approve, decline/unblock RPCs, answered elsewhere, engine without the W2 calls) |
| `WIN/w2-gate.ps1` | build-B helper: notepad · launch · second · kill · log · promote (tray icon visible) · attention · activity · rpc · crash |

## 2. Checks and gates

**Mac** (`./test.sh`, .NET 10.0.201): Core **95/95** (89 + 6 `W2Tests`; one W1 assertion updated: hire cards now carry options), Rpc **35/35**.
The WinUI project also type-checks on the Mac (a throwaway csproj over `APP/**/*.cs` with `EnableWindowsTargeting`, CsWin32 and the
WinUI reference assemblies, `UseWinUI=false`, `AppxGeneratePriEnabled=false`) — every build-B build below compiled first time.

**build-B builds** (service, slot `w2`, `-c Debug -p:Platform=x64`): `c2429a36` 0 errors / 0 warnings 41 s · `330cfda0` 0/0 22 s ·
`8aa73790` 0/0 17 s; output 484 files / 171.5 MB (W1's 461 + 22 tray ICOs + …), the five runtime-MSIX files extracted.
**build.ps1 publish** (Release, `8aa73790`, slot `w2`): 482 files / 171.5 MB in 16.3 s → `target\release\companion-windows\`, the five runtime-MSIX files present, no ML DLLs, `Register()` smoke ok on the published layout. **probe.ps1 -Seconds 15 -From publish**: `PROBE OK`, exit 0, W1's lines unchanged plus `probe rail rect=(1990,541) 26x170 … affinity=0x11 collapsed=false marks=5; panel rect=(822,250) 372x356 visible=true affinity=0x11 foreground=<not ours>`, `probe windows: […]`, `probe surfaces: tray=added "theuth · 2 need you" hotkeys=[openPanel=Ctrl+Shift+Space] rail marks=5 panel visible=true … ours=false` (the last two land just after EngineProbe's done line, inside its 2 s linger). Core gates ran on `330cfda0`; the menu, legend, single-instance, publish and probe rows on `8aa73790` (only the tray menu and the peek card height changed between).

**Gates** (§7 W2 row as briefed; seeded items `seed.ps1 all` + `hire` = decision · blocked · hire · hire; Notepad started
un-elevated by `w2-gate.ps1 notepad`; evidence = `windows-build-b` replies and the companion log, `[companion HH:mm:ss pid N]`):

| Gate | Result | Evidence |
|---|---|---|
| `ui_find(AutomationId="rail")` | **pass** | Group "theuth rail"; children wordmark, `needs-you-count` "4 need you", marks `mark-1…6` named e.g. "seed-234832-1, hire, 0 s", "theuth, idle, " |
| C11 capture without / with `--shareable` | **pass** | without: log `affinity=0x11`, UIA finds the rail at (2003,626) but a region capture of the right edge is black; the panel (foreground at 822,250 372×392) is absent from its region capture (Notepad shows through). With `--shareable`: rail and panel visible in the same captures (`affinity=0x0`) |
| `keyboard_control` Ctrl+Shift+Space → `ui_find("panel")`, panel foreground | **pass** | `WM_HOTKEY Ctrl+Shift+Space (openPanel) foreground=… Notepad` → `panel open (Mouse) prev=Notepad active=true`; `get_foreground` = "theuth — Attention"; `ui_find(panel)` = Group "Attention panel" |
| `ui_click("option-1")` → `ui_read("receipt-row")` shows the 3 s hold, then the item leaves `attention_list`, stamped `user@companion` | **pass** | (J J to the hire) `option-1` = "Hire, key 1"; receipt-row read "seed-234832-1 · Hire — sending · Esc undo · 3"; a second run polled it every 0.25 s: "… · 3" → "· 2" → "· 1" → "Sent · seed-234846-1 · Decline"; log `committed HireAgent … ok=true`; `w2-gate.ps1 attention` → 2 items left (blocked, decision); `activity` → `23:50:21 user@companion agent_hired seed-234832-1 — hired seed-234832-1`; the panel's Done zone shows "seed-234832-1 · hired seed-234832-1" (only `user@companion` rows reach it) |
| Esc → Notepad foreground, `ui_type` lands in it (C10) | **pass** | log `panel closed — foreground back to … Notepad -> true; now … Notepad`; `get_foreground` = Notepad; `ui_type` into Notepad's "Text editor" then `ui_read` = "[W2 gate 23:51] Esc returned the foreground; …" |
| Agent mark → `ui_find("peek-card")`, Notepad still foreground (hand-back) | **pass** | real click (move, 0.4 s, click) on `mark-4`: `rail pass-through: cursor (2002,673) overMark=true -> WS_EX_TRANSPARENT off` → `rail: activated by a press on agent mark seed-234832-1 — hand-back to … Notepad -> true; foreground now … Notepad` → `peek card: seed-234832-1 (idle)`; `get_foreground` = Notepad. `ui_click(mark-3)` (UIA SetFocus + Invoke): `rail: activated without a press (automation/keyboard) — hand-back to … Notepad -> true`, `ui_find(peek-card)` = "theuth, idle", foreground Notepad |
| `ui_snapshot` lists every option with its Name (C12) | **pass** | Buttons in the panel snapshot: "Hire, key 1", "Decline, key 2", lines "decision, user asks for a decision on …, 1 m", "blocked, …", "seed-234846-1, …"; AutomationIds `option-1`/`option-2` resolve with `ui_find`; option-less cards: "Open in theuth, key O" (`option-open`) |
| Tray icon with the ●N count | **pass** | log `tray: Shell_NotifyIcon(NIM_ADD) -> true, NIM_SETVERSION 4 -> true` and `tray: ●4 -> tray-dark-4.ico "theuth · 4 need you" NIM_MODIFY -> true`, ●3/●2 as items left; after `w2-gate.ps1 promote` (IsPromoted=1) the taskbar corner shows the glyph with the gold "2"; a left click opened the panel above the icon (`panel open (Tray)`, 1620,996 372×248) |
| Second launch exits, brings the first forward | **pass** (shell kept the foreground) | `w2-gate.ps1 second debug`: second process pid 5088 exited 0 after 306 ms; companions before [6800] after [6800]; first instance logged `activation redirected from another launch (kind=Launch)` → `panel open (Mouse) … active=false foreground=… Windows.UI.Core.CoreWindow "New notification"` — the panel came up but Windows left the foreground on a stale shell notification window (the spike's SpikeHost consent toasts); with an app in front the same path activates (hotkey row) | |

Also checked: the tray right-click menu (Win32, `8aa73790`) lists Open theuth · Ask theuth… · Pause notifications (greyed until W3 subscribes) · Launch at login (greyed) · Keyboard shortcuts…  Ctrl+/ · Settings… · Quit theuth; choosing Keyboard shortcuts… opened the legend (`ui_find(legend)` = "Keyboard shortcuts": every table row by scope with its chord and aliases, "unassigned" for the Ask-bar hotkey, the hold-to-talk line). The MenuFlyout version (`330cfda0`) ended the process at the right-click (Deviation 2). Twice a sibling's build or `build.ps1` killed the W2 test instance mid-round (the service and build.ps1 kill every `Theuth.Companion` by name).

### C1–C5, C9–C12

| # | Status |
|---|---|
| C1 glance questions | Gold present (wordmark/count/rings + tray ●N), who (mark names, card "who"), what they need (question line, options), what landed (Done zone, receipt row) — on the screenshots above; no reviewer pass yet |
| C2 ≤ 3 options, pre-pick | Core cards (Swift-tested) + W2 hire/blocked sets; no pre-pick on hire (P13); Enter inert without one |
| C3 keystrokes | 2 from anywhere (Ctrl+Shift+Space opens on the oldest needs-you item, then a digit) — the J J in the gate only because the target was the third item |
| C4 hotkey → sent < 5 s | 3 s hold + E5 re-read: `ui_click` at 23:50:18 → `committed` 23:50:21.1 |
| C5 ≤ 4 glyph states per zone, colour not the only channel | Zone 1 only needs-you rings (notch position per kind); zone 2 working/idle/finished/error/lapsed differ in shape (disc, ring, tick, bar, dashed) — RailModel tests (W1); greyscale capture not taken |
| C9 undo on every keystroke, no hold on a notification tap | Hold + Esc undo through the Store (W1b tests); receipt row "Esc undo · 3…1"; notification taps are W3's `AnswerNow` |
| C10 never takes foreground until summoned, returns it on Esc | gate above; rail/peek presses hand the foreground straight back (logged) |
| C11 hidden from capture | gate above (rail, panel; legend/peek/menu windows carry the same affinity) |
| C12 names for Narrator/UIA | marks "<agent>, <state>, <since>", options "<label>, key <n>", receipt row named by its text (live region polite); a Narrator pass not done |

## 3. Deviations

1. **The `TheuthCompanionRail` class is on a hidden host window, not on the rail's HWND.** A WinUI `Window`'s class is fixed
   (`WinUIDesktopWin32WindowClass`); the shell (W5) only needs `FindWindowW("TheuthCompanionRail")` + `WM_CLOSE`, so a hidden
   top-level `WS_POPUP` of that class on the UI thread receives it (and the tray callback and `WM_HOTKEY`) and quits the app (exit 0).
2. **Tray menu = Win32 popup menu**, not a WinUI `MenuFlyout`: hosted in a 1 × 1 tool window the flyout ended the process at the
   right-click on build-B (no managed exception, no `.crash.json`); `TrackPopupMenuEx` has the same items in the same order. Pause and
   Launch at login are greyed until W3 subscribes (`Surfaces.PauseRequested`, `LaunchAtLoginToggled`/`LaunchAtLoginState`).
3. **Hire and blocked cards got options in Core** (130 §4.2 per-kind table; the Swift port had none): `CardModel`, `StoreModel.Commit`,
   `IStoreEngine` (default members) — outside W2's listed seams, small and tested; the brief's "use the hire/blocked/decision items'
   per-kind actions" needed them. A seeded decision stays option-less (no packet RPC) and shows "Open in theuth, key O".
4. **Rail hand-back for non-press activations is deferred 150 ms** (UI Automation's SetFocus before Invoke activates the island with the
   pointer elsewhere): a panel-opening mark cancels it, anything else gets the foreground back. Real presses decide at `WM_ACTIVATE` as
   designed. Finished marks (they carry an item) open the panel like needs-you marks (Swift).
5. **Rail background is the opaque surface colour**, no `DesktopAcrylicController` on the layered window (untested combination); the
   panel uses `MicaBackdrop` (Base), opaque surface when Mica is unsupported or transparency effects are off.
6. **Settings are read, not written**: keys, undo delay, rail presence/side come from `settings.json` at start; a `ResolveGlobal`
   fallback is used for the run and shown in the legend, not persisted (W3 owns writes; `Surfaces.ApplySettings` applies a change live).
7. **Not built in W2**: the first-run "pin theuth to the taskbar corner" tip (needs a persisted flag → W3's settings), hover-to-open on
   the sliver beyond expand, Snap Assist edge interplay (not exercised), a Narrator pass, a greyscale C5 capture.
8. **"Quit theuth"** quits the companion (the Tauri shell owns the main app's quit); **Open theuth** starts `..\theuth-desktop.exe`.
9. `--probe` keeps W1's lines (EngineProbe untouched) and adds `probe rail …; panel …` (1.5 s, panel shown passive + click-through),
   `probe windows: […]` and `probe surfaces: …` just before the done line; `--probe-legend` shows the legend passive for 3 s then exits 0.

## 4. For W3, W4, W5

- **Window class / ids.** Shell `WM_CLOSE` target: hidden window class `TheuthCompanionRail` (title "theuth companion"). Windows:
  "theuth rail", "theuth — Attention", "theuth — Keyboard shortcuts", "theuth peek". UIA: `rail`, `mark-<n>` (1-based display
  order), `needs-you-count`, `rail-sliver`, `panel`, `panel-headline`, `card`, `card-question`, `card-why`, `option-<n>`,
  `option-open`, `line-<n>`, `done-<n>`, `note-field`, `reply-field`, `receipt-row`, `panel-hold-hint`, `peek-card`, `peek-open`,
  `legend`, `legend-<actionId>`, `legend-settings`.
- **Store (W2 members, `Store.W2.cs`).** `Keys` (assign a new `KeyTable` → hotkeys re-register, grammar/legend follow),
  `HotkeyNotice`, `OldestNeedsYou`, `FocusOldest()`. W2 handles `CloseRequested`, `OpenUrlRequested`, `LegendRequested`; it logs
  `SettingsRequested`/`AskBarRequested` (W3/W4 subscribe).
- **`Ui.Surfaces.Current` hooks.** W3: `PauseRequested("1h"|"tomorrow"|"off")`, `LaunchAtLoginToggled(bool)` + `LaunchAtLoginState`,
  `ApplySettings(CompanionSettings)`, `Redirected(AppActivationArguments)` (a redirected toast activation; W2 handles plain launches).
  W4: `AskBarAvailable = () => true` makes the hotkey open the Ask bar when nothing needs you; `Hotkeys.Released(action, held)` for
  hold-to-talk; the R field lives in the panel and calls `Store.SubmitReply`.
- **App.xaml.cs**: W2 changed the block after the W3 comment (log window only under `--probe`) and added one line in `Quit`; the W3
  `Register()` goes before `Ui.Surfaces.Start`.
- **build-B**: every build job with `kill: ["Theuth.Companion"]` and every `build.ps1` run kills *all* companions by name — the other
  waves' test instances too, even under the desk lock (it killed W2's twice). Tray icons land in the overflow: `w2-gate.ps1 promote`
  sets `IsPromoted` for this exe path. Stale "Important Notification Request" consent toasts from the spike's SpikeHost sit over the
  bottom-right corner.
