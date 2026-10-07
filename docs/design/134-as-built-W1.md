# 134 — as-built W1: skeleton, Rpc + Core ports, the WinUI shell with `--probe`, the runtime-MSIX extraction

Status: built 2026-10-07 on `feat/134-w1-skeleton` (cut from main `3d931f2c`), pushed to `basedatum`. Design:
`docs/design/134-companion-windows.md` §0 D1, D2, D6, D11, D12; §2 A3–A5, C1, C2b, T1–T5; §5.1–§5.7; §7 row W1.
Port source: the macOS companion `crates/desktop/companion` (SwiftPM) at the same base — `Sources/TheuthKit` → `Rpc`,
`Sources/CompanionCore` (13 files) → `Core`, `Tests/TheuthKitTests` + `Tests/CompanionCoreTests` (81 tests) → two xUnit
projects. Toolchain: Mac .NET SDK 10.0.201 in `~/.dotnet` (portable projects); build-B (Windows 11 Pro 25H2 26200, .NET
SDK 10.0.201) through the build service on `:8933` (`build`/`script`/`test` jobs, slot `w1`) and the `windows-build-b`
MCP tools for the UIA gate. Nothing outside `crates/desktop/companion-windows/**` and the one `justfile` recipe changed.

## 1. File map (path:line)

`WIN/` = `crates/desktop/companion-windows/`.

| Path | What |
|---|---|
| `WIN/Theuth.Companion.slnx` | All five projects; the app pinned to x64 |
| `WIN/Theuth.Companion.Portable.slnf` | The Mac filter: Rpc, Core and both test projects (never the WinUI app) |
| `WIN/Directory.Build.props:4-20` | `LangVersion latest`, `Nullable`, `ImplicitUsings`, `TreatWarningsAsErrors` (NuGet feed/audit NU1603/NU190x kept as warnings), `DefaultItemExcludes` `bin/**;obj/**;bin-*/**;obj-*/**` (the spike's variant-output trap) |
| `WIN/Directory.Build.props:22-28` | `Version` = `[workspace.package] version` read from the repo's `Cargo.toml` by regex (2.12.1 at W1; assembly informational version `2.12.1+<sha>`) |
| `WIN/Directory.Build.props:30-40` | Windows App SDK 2.5.1 trimmed pins: WinUI 2.3.9, Foundation 2.3.12, Base 2.0.4, InteractiveExperiences 2.1.9 (WinUI asks for 2.1.8, never published — the NU1603 of A2), Runtime 2.5.1 (download-only) |
| `WIN/src/Theuth.Companion.Rpc/Discovery.cs:11-54` | `EngineLocation(Url, Token)`: `DataDir()` (`THEUTH_DATA_DIR`, else `%USERPROFILE%\.theuth`), `Discover()` (token required → `NoTokenException`; `engine.json` `listen`, else `127.0.0.1:7777`) — the Swift semantics (D2) |
| `WIN/src/Theuth.Companion.Rpc/RpcClient.cs:30-420` | `RpcClient` on `ClientWebSocket`: `Authorization: Bearer` + `X-Theuth-Client: companion` on the upgrade (`:148-149`), proxy off (local engine), positional params + numeric ids, `{"id","jsonrpc","method","params"}` with sorted keys, one `subscribe_events(null, null)` re-issued on every open (the subscription id is recorded inside the dispatch of its reply, before the next frame), back-off 1 s → 30 s (`:201`), engine re-located before every dial, pending calls failed on loss/close, events delivered in arrival order from one receive loop; `RpcException` prints `rpc <code>: <message>` |
| `WIN/src/Theuth.Companion.Rpc/RpcClient.cs:422-452` | The twelve calls: `attention_list`, `attention_snooze`, `attention_dismiss`, `agents_list`, `list_activity`, `packet_get`, `answer_ask`, `resolve_approval`, `task_decide`, `task_move`, `send_input(session, text, [], null, null)` (+ `subscribe_events`) |
| `WIN/src/Theuth.Companion.Rpc/Models.cs` | Every `Models.swift` shape: `AttentionKind` (`:27`, `Other(word)` for unknown kinds), `AttentionItem` (`:104`, E2/E3 fields, `WaitingSince`), `AttentionChanged`, `AgentStatus`/`AgentFile`/`AgentSummary`, `AgentState` (tagged), `SessionActivity`, `AgentInfo` (`:242`, `Activity` defaults from `State` on pre-62 engines), `Risk`, `Decision`, approvals, `Choice`/`Question`/`Answer` (`:331`)/asks, `Packet` (`:440`, `PreferredOption`), `PendingPrompts`, `TurnFinished`, `Lagged`, `WireEvent` (`:499`, typed payload or `Other(type)`, the 13 `AttentionTriggers`); plus `StreamDelta` (`:558`) and `DictationEvent` (`:571`) typed for W4 |
| `WIN/src/Theuth.Companion.Rpc/Json.cs` | `ValueList<T>` (`:12`, a `List<T>` with value equality — Swift arrays in `Equatable` structs), `JsonValue` (`:48`, Swift's `JSONValue`), `TheuthJson` (`:104`: `SnakeCaseLower`, nulls omitted when writing, `RespectNullableAnnotations` + `[JsonRequired]` = Swift's missing/null rules, snake-case enum strings without integers, `EncodeParams`), `Rfc3339DateConverter` (`:166`, 0–9 fractional digits), `NullAsEmptyListConverter` (Swift's `decodeIfPresent ?? []`) |
| `WIN/src/Theuth.Companion.Core/RailModel.cs` | `MarkState`/`Mark`/`Rail`, `RailPresence` (+ raw words), `RailCollapse` (`:98`), `RailModel.Derive` (`:133`), `Ago` (`:250`) |
| `WIN/src/Theuth.Companion.Core/PanelModel.cs` | Order, focus, `Move`, `FocusOn(id)` (Swift `focus(id:)`), `Update`, `Headline`, `Position`, the Done-zone `ReceiptCount`/`ReceiptFocus` |
| `WIN/src/Theuth.Companion.Core/UndoQueue.cs` | `UndoQueue<TAction>`: hold/displace, cancel, fire once, 0–5 s clamp, injected `now` |
| `WIN/src/Theuth.Companion.Core/KeyBindings.cs` | Windows form of the one key table (D6): `ChordModifiers` (`:13`, = Win32 `MOD_ALT/CONTROL/SHIFT/WIN`), `VirtualKeys` (`:23`; `NumpadEnter` = `VK_RETURN|0x100`, `:36`), `Chord` (`:48`, chars for printable keys, VK code otherwise, `Symbol` "Ctrl+Shift+Space"), `KeyNames`, `KeyScope`, `KeyAction` (+ persisted camelCase ids), `KeyBinding`, `KeyTable` (`:260`: `Defaults` `:276` with ⌘ → Ctrl, aliases, `ActionFor`, `Check` `:344` with `Verdict.ReservedByWindows`, `Set`/`Move`/`Reset`, `Json` `:382`, `ResolveGlobal` `:447`) |
| `WIN/src/Theuth.Companion.Core/KeyGrammar.cs` | `KeyPress` (`:11`), `Intent`, `KeyGrammar.IntentFor` (`:96`), `DeepLink` (`:106`, urlPathAllowed ∪ `:` escaping), `StaleGuard` |
| `WIN/src/Theuth.Companion.Core/CardModel.cs` | `CardAction` (`:6`, eleven cases), `CardOption`, `Chip`, `Card.Make` (`:67`, P13 pre-picks) |
| `WIN/src/Theuth.Companion.Core/Policy.cs` | `NotificationClass`, `NotifyMode`, `NotificationPrefs` (value type; `TimeSensitiveAvailable` `:46` = the Windows "errors break through Do not disturb" opt-in → Urgent), `Banner`, `PolicyOutput`, `NotificationPolicy` (`:95`; `Update` `:163`, `Breakpoint`, `Tick`, `Pause`, `Engine` `:266`) |
| `WIN/src/Theuth.Companion.Core/PolicySettings.cs` | `SettingsStore` (`:13`, `settings.json` with `UserDefaults` getter semantics; `DefaultPath` `%LOCALAPPDATA%\theuth\companion\settings.json`), `CompanionSettings` (`:125`; `Load`/`Save`, the `railCollapse` migration, `LoginItemJson` `:203`) |
| `WIN/src/Theuth.Companion.Core/Breakpoint.cs` | `BreakpointDetector` (`:7`), `Modifiers` (`:64`, Alt/Win in place of ⌥/⌘), `DoubleTapDetector` (`:80`, double-tap **Alt**) |
| `WIN/src/Theuth.Companion.Core/AskRouter.cs` | `MeshStatus` (`:6`), `AskDesk`, `AskPlan`, `AskRouter` (`:75`, `Plan` `:88`), `ReplyEvent`, `AskReply` (`FirstTwoLines` `:146`), `AskModel`, `CardReplies.ReplyAction`/`ReplyLabel` (`:194`) |
| `WIN/src/Theuth.Companion.Core/AskRpc.cs` | `steer`, `mesh_status`, `desk_open`, `activity_list`, `set_event_focus`, `dictation_prewarm/start(16000)/audio/stop` as `RpcClient` extensions (`:31`, `:35`: PCM16 little-endian → base64) |
| `WIN/src/Theuth.Companion.Core/AskVoice.cs` | `AskVoiceMachine` (`:11`), `AskDictation` (`:112`) |
| `WIN/src/Theuth.Companion.Core/ReceiptModel.cs` | `ActivityEntry`, `ActivityFilter` (`:22`), `Receipt` (`FromActivity`), `ReceiptModel` (`:66`) |
| `WIN/src/Theuth.Companion/Theuth.Companion.csproj` | Hand-written (the spike's shape): `net10.0-windows10.0.22621.0`, min 10.0.19041, `UseWinUI`, `WindowsPackageType=None`, `WindowsAppSDKSelfContained`, `SelfContained`, `win-x64`, `Platforms x64`, `DISABLE_XAML_GENERATED_MAIN`; the four component references + the download-only `Microsoft.WindowsAppSDK.Runtime` (`ExcludeAssets="all"`, `GeneratePathProperty`) (`:32-37`) |
| `WIN/src/Theuth.Companion/RuntimeMsixPayload.targets` | `ExtractRuntimeMsixPayload` (`:42`, after `Build` → `$(OutDir)`) and `ExtractRuntimeMsixPayloadToPublishDir` (`:50`, after `Publish` → `$(PublishDir)`): `Unzip` of the five entries from `$(PkgMicrosoft_WindowsAppSDK_Runtime)\tools\MSIX\win10-x64\Microsoft.WindowsAppRuntime.2.msix` into `obj\…\runtime-msix-payload\`, an error per missing entry |
| `WIN/src/Theuth.Companion/Program.cs:13-25` | `Main` (returns `App.ExitCode`); the marked seam for W2's `AppInstance` redirect |
| `WIN/src/Theuth.Companion/App.xaml(.cs)` | `OnLaunched` (`:29`): log start, launch/runtime lines, `--register-smoke`, the alive mutex `Local\io.theuth.companion` (`:43`), the log window (shown without activation under `--probe`), the engine probe; hook comments for W3 (`:46`) and W2 (`:51`); `Quit(code)` |
| `WIN/src/Theuth.Companion/EngineProbe.cs` | The Swift W1 probe without surfaces: `Locate` before every dial (`:60`), events (`:86`: first `PendingPrompts` logged; PendingPrompts/AttentionChanged/Lagged refetch at once, other typed events and the 13 triggers after 250 ms), `Refetch` (`:135`: the three lists, `RailModel.Derive`, `PanelModel.Headline`), `Finish` (`:176`: "probe done … exit 0", 2 s on screen, exit 0) |
| `WIN/src/Theuth.Companion/LogWindow.cs` | The only W1 window: read-only TextBox, `AutomationId="log"` (`:32`), Name "Companion log" |
| `WIN/src/Theuth.Companion/{Log,Flags,AppInfo,CrashReport}.cs` | `[companion HH:mm:ss.fff pid N] …` lines to `%LOCALAPPDATA%\theuth\companion\logs\companion-<date>.log` (newest 7 kept), the window, stderr under `--probe`/`--register-smoke`, `logs\probe.log` per probe; `--probe N`, `--shareable`, `--background`, `--register-smoke`; `.crash.json` on unhandled exceptions (version, engine version from `engine.json`) |
| `WIN/tests/Theuth.Companion.Rpc.Tests/` | `FixtureTests.cs` (the 9 Swift tests + one case per shared fixture + 3), `RpcClientTests.cs` (8, against a loopback Kestrel "engine"); the fixtures are linked from `../../../companion/Tests/TheuthKitTests/Fixtures/*.json` — one set, two companions |
| `WIN/tests/Theuth.Companion.Core.Tests/` | `CompanionCoreTests.cs` (15), `PolicyTests.cs` (27 + 1), `AskTests.cs` (10), `ReceiptTests.cs` (6), `W6Tests.cs` (14 + 2) |
| `WIN/test.sh`, `justfile` (`companion-windows-test`) | `dotnet test Theuth.Companion.Portable.slnf` with `~/.dotnet/dotnet` (or `$DOTNET`, or `dotnet` on PATH) |
| `WIN/build.ps1` | Kill a running exe → `dotnet publish -c Release -p:Platform=x64 -o <repo>\target\release\companion-windows` → the five files, no ML DLLs, no meta/ML/AI package in the graph, size → `--register-smoke` |
| `WIN/probe.ps1` | `Theuth.Companion.exe --probe N` in the interactive session; prints `engine.json` (never the token), `probe.log`, the exit code; `PROBE OK` when the Swift W1 shape and exit 0 are there |
| `WIN/README.md` | Layout and the two loops |

## 2. Tests

Mac (`just companion-windows-test` and `WIN/test.sh`, .NET SDK 10.0.201, 2026-10-07 22:12): **Rpc 35/35, Core 75/75 — 110
passed, 0 failed** (Swift: 9 + 72 = 81). The same 110 pass on build-B (`test` job on `Theuth.Companion.slnx`, `f079a288`,
19 s: Core 75 in 486 ms, Rpc 35 in 4 s, including the Kestrel client tests).

| Swift suite (tests) | C# | Count |
|---|---|---|
| `FixtureTests` (9) | `FixtureTests`: the 9, test-for-test | 9 |
| — | `EveryFixtureDecodes` (one theory case per fixture file, 15; a new fixture without a shape fails), `TheSharedFixtureDirectoryIsLinked`, `AnAnswerSubtypeEncodesThroughTheConverter`, `AMissingRequiredFieldOrANullOneFailsAsInSwift` | 18 |
| — | `RpcClientTests` (upgrade headers, positional params of every call, subscription incl. an item right behind the reply, error reply, not connected, loss → pending failed → redial + resubscribe, no token → backoff + re-locate, close) | 8 |
| `CompanionCoreTests` (15) | `RailModelTests` 3, `PanelModelTests` 3, `UndoQueueTests` 2, `KeyGrammarTests` 2, `CardModelTests` 5 | 15 |
| `PolicyTests` (27) | `PolicyClassTests` 4, `PolicyDeferralTests` 5, `PolicyFloodPauseWithdrawTests` 4, `BreakpointDetectorTests` 2, `DoubleTapDetectorTests` 10, `SettingsPersistenceTests` 2 + `TheSettingsFileRoundTripsAndToleratesGarbage` | 28 |
| `AskTests` (10) | `AskRouterTests` 6, `AskVoiceTests` 4 | 10 |
| `ReceiptTests` (6) | `ReceiptModelTests` 6 | 6 |
| `W6Tests` (14) | `KeyTableTests` 7 + `WinKeyChordsBelongToTheOperatingSystemInEveryScope` + `ModifiersAreWin32ModFlagsAndKeyCapsAreWindowsNames`, `RailPresenceTests` 4, `AskDictationTests` 3 | 16 |

Every Swift case is kept; the key-table cases use Win32 virtual-key codes, Ctrl for ⌘, Alt for ⌥ and Windows key caps
(§4.1 below), the breakpoint cases Windows exe paths for bundle ids, the settings cases the in-memory `SettingsStore`.

## 3. build-B (Windows 11 Pro 25H2 26200, build service slot `w1`, worktree `C:\Users\user\Develop\theuth-ci-w1`)

| Job | sha | Result |
|---|---|---|
| `build` Debug x64 of `src/Theuth.Companion` (`20261007T212218-4`) | `e1a8486a` | **0 errors, 0 warnings**, 16.5 s (job 20 s); `ExtractRuntimeMsixPayload: 5 runtime-MSIX files -> bin\x64\Debug\…\win-x64\`; output 461 files / 171.1 MB (PDBs included) |
| `script build.ps1` Release (`20261007T212245-5`) | `e1a8486a` | `dotnet publish` 14.1 s → `target\release\companion-windows\`: **460 files, 171.0 MB**; the five files in build **and** publish output; no `onnxruntime.dll`/`DirectML.dll`; graph = WebView2 1.0.3719.77, Base 2.0.4, Foundation 2.3.12, InteractiveExperiences 2.1.9, Runtime 2.5.1 (download-only), WinUI 2.3.9 — no meta/ML/AI package; largest files `Microsoft.Windows.SDK.NET.dll` 24.9, `System.Private.CoreLib.dll` 15.3, `Microsoft.ui.xaml.dll` 14.6 MB; **`Register()` smoke ok** |
| `script probe.ps1 -Seconds 15` (`20261007T212331-6`) | `e1a8486a` | **PROBE OK, exit code 0** against the scratch engine 2.12.0 (`engine.json` listen `127.0.0.1:7777`, pid 5216 alive, token 64 chars) |
| `test` `Theuth.Companion.slnx` (`20261007T212631-7`) | `f079a288` | 110/110 (above) |

The five files, as published (sizes from build.ps1; byte-identical to the 2.5.1 MSIX entries):

```
ok  Microsoft.WindowsAppRuntime.Insights.Resource.dll  34,104 bytes
ok  WindowsAppRuntime.DeploymentExtensions.OneCore.dll  427,912 bytes
ok  WindowsAppSdk.AppxDeploymentExtensions.Desktop.dll  671,112 bytes
ok  WindowsAppSdk.AppxDeploymentExtensions.Desktop-EventLog-Instrumentation.dll  45,448 bytes
ok  WindowsAppRuntime.png  232 bytes
```

The `Register()` smoke on the published layout (stderr of `Theuth.Companion.exe --register-smoke`):

```
[companion 22:23:05.044 pid 9868] register-smoke IsSupported=True
[companion 22:23:05.088 pid 9868] register-smoke Register() ok Setting=Enabled
[companion 22:23:05.107 pid 9868] register-smoke UnregisterAll() ok
[companion 22:23:05.110 pid 9868] terminate (exit 0)
```

The probe (`%LOCALAPPDATA%\theuth\companion\logs\probe.log`, the published exe, `--probe 15`) — the Swift W1 shape
(`130-as-built-W1.md` §2) minus the surface lines W2 adds:

```
[companion 22:23:34.473 pid 1836] launch exe=C:\Users\user\Develop\theuth-ci-w1\target\release\companion-windows\Theuth.Companion.exe version=2.12.1+e1a8486aaeebd2715fc4dca35f43b46d3c85449e probe=15.0 shareable=false background=false
[companion 22:23:34.486 pid 1836] runtime Microsoft.WinUI 3.0.0.0; .NET 10.0.5; Microsoft Windows 10.0.26200; log C:\Users\user\AppData\Local\theuth\companion\logs\companion-2026-10-07.log
[companion 22:23:34.884 pid 1836] connection: connecting
[companion 22:23:34.891 pid 1836] engine ws://127.0.0.1:7777/ (token 64 chars)
[companion 22:23:35.016 pid 1836] connection: open
[companion 22:23:35.100 pid 1836] probe PendingPrompts approvals=0 asks=0 secrets=0 first=none
[companion 22:23:35.269 pid 1836] probe attention_list=0 [] agents_list=1 list_activity=0 live=0 rail: needsYou=0 working=0 marks=1 quiet=true headline="Nothing needs you"
[companion 22:23:49.794 pid 1836] probe done after 15.0 s — quitting (engine connected=true, refetches=1) exit 0
[companion 22:23:49.804 pid 1836] connection: closed
[companion 22:23:51.793 pid 1836] terminate (exit 0)
exit code: 0
PROBE OK
```

The scratch engine is fresh (no attention items, one roster agent — theuth's desk — and no live sessions), so the counts
are small; the upgrade with the bearer token and `X-Theuth-Client: companion` was admitted and the three lists decoded.

UIA gate (the `windows-build-b` MCP, desk lock held and released): `app` launched the published exe with `--probe 12`
(window "theuth companion — log", class `WinUIDesktopWin32WindowClass`, not activated); `ui_find(AutomationId="log")`
→ Edit "Companion log"; `ui_read` returned the same lines from `launch` to
`probe done after 12.0 s — quitting (engine connected=true, refetches=1) exit 0` and `connection: closed`; the process
was gone afterwards (`process list` → 0).

## 4. Deviations from the design and the brief

1. **The key table is in its Windows form, not a literal copy of the Swift values** (D6 "port verbatim; ⌘ becomes Ctrl",
   §4.6 "Win-key chords refused"). A literal port (Carbon key codes, ⌃ and ⌘ as separate bits) could not be used on
   Windows without changing the table — ⌃⇧Space and ⌘O both land on Ctrl — and W2 does not own `Core`. So: `KeyCode`
   is the Win32 virtual-key code, `ChordModifiers` carries Win32's `MOD_*` values (pass straight to `RegisterHotKey`),
   the defaults are the Mac table with ⌘/⌃ → Ctrl, `Symbol` prints Windows caps ("Ctrl+Shift+Space", "Enter", "Esc",
   "Num Enter"), Win chords are `Verdict.ReservedByWindows` in every scope (Swift `reservedByMacOS` for system and bare
   global chords keeps its rule), `KeyPress(chars, keyCode, ctrl:)` replaces `command:`. The numeric keypad's Enter,
   one VK on Windows, is encoded `VK_RETURN|0x100` (`VirtualKeys.NumpadEnter`) so the alias stays a key of its own. Same
   test cases with Windows codes and caps.
2. **Breakpoint modifiers**: `Modifiers.Alt`/`Win` replace ⌥/⌘ and `DoubleTapDetector` detects Alt alone (§3: double-tap
   Alt); `CompanionSettings.doubleTapOption` → `DoubleTapAlt` (store key `doubleTapAlt`).
3. **Settings storage**: `UserDefaults` → `SettingsStore`, a JSON object at `%LOCALAPPDATA%\theuth\companion\settings.json`
   (D12) whose getters keep `UserDefaults`' rules so `Load`/`Save` port unchanged; `loginItemJSON` (Data) → `LoginItemJson`
   (string, same bytes).
4. **Placement of shapes**: `MeshStatus`, `ActivityEntry`, `ActivityFilter` stay in `Core` where Swift has them (file for
   file), though §5.3 lists `MeshStatus` among the Rpc shapes. `Rpc` additionally types `StreamDelta` and `DictationEvent`
   (Swift's app decodes them ad hoc in `Store+Ask.swift`) so W4 starts from typed events.
5. **Discovery = Swift's, not all of §2 T2**: `engine.json` `listen` else `127.0.0.1:7777`; no pid-liveness check and no
   `config.yaml` `rpc.listen` fallback — exactly what the Swift client does (D2 "identical semantics"); the client re-locates
   before every dial, so a stale record costs one failed dial.
6. **Logging without `Microsoft.Extensions.Logging`** (§5.5 names it): a static `Log` with the same layout (dated file,
   newest 7, stderr under `--probe`) — no dependency, and the Swift `Log.write` call shape for the later ports.
7. **No CsWin32 yet**: the W1 app makes no Win32 call (DisplayArea/AppWindow cover the log window); W2 adds the package
   and `Interop/NativeMethods.txt` with its first P/Invoke.
8. **Probe lines**: no rail/panel/CGWindowList lines (no surfaces until W2); the done line reads
   `probe done after 15.0 s — quitting (engine connected=…, refetches=…) exit 0` and stays on screen 2 s so a UIA read sees
   it; without `--probe` the same engine monitor runs and logs `state …` lines (the W1 shell; W2's Store replaces it).
9. **Earlier than planned**: the alive mutex `Local\io.theuth.companion` (§5.5; W5's alive check runs in parallel) and
   `Program.Main` (`DISABLE_XAML_GENERATED_MAIN`) as the seam for W2's single instance are in W1.
10. **build.ps1 command line**: `dotnet publish … -p:Platform=x64 -o …` without `-r win-x64 --self-contained` — the csproj
    sets both (a CLI `-r` would flow a global RID into the libraries); `--register-smoke` ends with `UnregisterAll()` so a
    build folder leaves no AUMID registration behind.
11. **Codec detail not copied**: Swift's `convertFromSnakeCase` also rewrote keys inside free-form objects (`args`,
    secrets); the C# codec keeps those keys verbatim. No reader depends on it (`args["command"]` has no underscore).

## 5. For the next waves

- **Everyone**: `TreatWarningsAsErrors` is on (the app built with 0 warnings); add packages to
  `src/Theuth.Companion/Theuth.Companion.csproj` in your own `ItemGroup`; kill `Theuth.Companion` before a build (the
  service's `kill`); a lone `_` lambda parameter is a real parameter (CS0029 in the first build). Lists in shapes are
  `ValueList<T>` (value equality, collection expressions work). `probe.ps1` is the template for a gate script; take the
  desk lock for anything that puts a window up.
- **W2 (tray · rail · panel · hotkey)**: single instance goes at the marked line in `Program.Main`; surfaces start at the
  marked line in `App.OnLaunched`; replace `EngineProbe` with the Store (keep its probe lines — the gate reads them).
  `KeyPress` from `KeyDown`: `Chars` = the key's character without modifiers (`ToUnicodeEx` with an empty key state),
  `KeyCode` = `(ushort)e.Key`, `VirtualKeys.NumpadEnter` when `KeyStatus.IsExtendedKey` and the key is Enter, modifiers from
  the key states. `RegisterHotKey(hwnd, id, (uint)chord.Modifiers | MOD_NOREPEAT, chord.KeyCode)`; `KeyTable.ResolveGlobal`
  with `canRegister` = a register/unregister probe. `DeepLink` returns `Uri` (`OriginalString` is Swift's `absoluteString`).
- **W3 (notifications · policy · settings)**: `Register()` before any window at the marked line in `App.OnLaunched`; the
  register smoke proves the trimmed layout registers. `BannerLevel.TimeSensitive` ↔ `AppNotificationScenario.Urgent`, on
  when `NotificationPrefs.TimeSensitiveAvailable` (the "Errors break through Do not disturb" switch, off by default).
  `CompanionSettings`/`SettingsStore` (`DefaultPath`) and `LoginItemJson` are in `Core/PolicySettings.cs`, which W3 co-owns.
  `BreakpointDetector` takes the foreground process identity (exe path) and the companion's own; `DoubleTapDetector` wants
  `Modifiers.Alt` alone.
- **W4 (Ask bar · dictation)**: `AskRpc` has the calls (`DictationStartAsync` defaults to 16000 — never null;
  `DictationAudioAsync` takes `ReadOnlySpan<short>`); `StreamDelta.Decode` / `DictationEvent.Decode` for the stream;
  `AskRouter`, `AskDictation`, `AskVoiceMachine`, `ReceiptModel` are ported with their tests; `SetEventFocusAsync`
  answers -32601 on pre-129 engines (ignore it).
- **UI tests on build-B's scratch engine** (no model provider): seed real items with
  `pwsh -File crates\desktop\companion-windows\seed.ps1 all | decision [--count N] [--title TEXT] | list | clear` — through
  the build service as a `script` job with `args` — decision (option-less), blocked and hire can be seeded; review,
  approval, ask, show, finished need a model turn (§7.3). The Store every wave builds on is described in §7.1.
- **W5 (shell · bundle)**: the publish folder is `target\release\companion-windows\` (460 files, 171.0 MB, exe
  `Theuth.Companion.exe`); the alive mutex is `Local\io.theuth.companion`; logs live in `%LOCALAPPDATA%\theuth\companion\logs`.

## 6. Open items

- E4's `user@companion` stamping is exercised only by the header test (Kestrel) and the engine's acceptance of the
  upgrade; the probe never calls a write RPC. W2's first keystroke answer shows it in `activity_list`.
- The counts above come from a fresh scratch engine; a populated engine (items of every kind) is W2's panel gate.
- Not built in W1 by design: single instance, tray, rail, panel, hotkeys, notifications, settings window, Run key, Ask
  bar, voice (W2–W4); the NSIS/bundle step (W5).

## 7. W1b — the Store, and the seeding tool

Added on the same branch after W1 (orchestrator's W1b brief): the macOS Store (`SWIFT/Companion/Store.swift` and the
store half of `AskBar/Store+Ask.swift`) ported so W2, W3 and W4 start from one shared state object, plus a tool that puts
real attention items on build-B's scratch engine.

### 7.1 Store contract (for W2–W4)

- **Shape.** `Theuth.Companion.Core.StoreModel` (portable, `Core/StoreModel.cs`; engine, timers and clock injected through
  `IStoreEngine` / `IStoreScheduler` / `Func<DateTimeOffset>`, `Core/StoreEngine.cs`) holds all state and logic.
  `Theuth.Companion.Store : StoreModel` (`src/Theuth.Companion/Store.cs`) is the thin UI-thread adapter and is
  **`public sealed partial`**: a wave adds members in its own `Store.W2.cs` / `Store.W3.cs` / `Store.W4.cs`, never in
  `Store.cs`. The app's instance is `((App)Application.Current).Store`, created in `App.OnLaunched`
  (`Store.Create(dispatcher, EngineProbe.Locate)`, subscribe, then `Store.Start()`).
- **Threading rule.** One owner: the UI thread (App's `DispatcherQueue`). Call every member from it; every
  `PropertyChanged` and every event below is raised on it. The `RpcClient` runs on the thread pool and its callbacks are
  marshalled with `TryEnqueue` before `HandleState`/`HandleEvent`; timers are `DispatcherQueueTimer`s; async work resumes
  on the UI thread through Program.Main's `DispatcherQueueSynchronizationContext`. Code on another thread (a toast's
  `NotificationInvoked`, a hook callback) marshals with `store.Dispatcher.TryEnqueue(...)` first.
- **Change notification.** `INotifyPropertyChanged` (WinUI-bindable), property name = the member name. Published:
  `Items`, `Agents`, `Activity` (the three lists), `Approvals` (id → `ApprovalRequested`, from the stream), `Packets`
  (key → `Packet`, `packet_get` once per decision/review), `Rail`, `Panel` (a `PanelModel` mutated in place — the
  notification follows every Store mutation), `Connection` (`RpcClient.State`), `Receipt` (`ReceiptRow?`: `Holding(Held)`
  · `Sent(text)` · `Undone` · `Stale` = already answered elsewhere · `Failed(error)` · `Reply(who, text, session)`),
  `Now` (100 ms during a hold), `NoteFor` (settable), `Ask` (`AskModel`: router, receipts, `ReplyFor`, `Current`,
  `Dictation`; mutated in place). Read-only helpers: `HoldRemaining`, `HeldNow`, `FocusedCard`, `FocusedReceipt`,
  `CardFor(item)`. Settable: `UndoDelay` (seconds, read at every hold — W3 wires Settings), `AskProbe` (W4 logging).
- **Events.** `CloseRequested`, `OpenUrlRequested(Uri)` (D · O · Ctrl+O, Done-zone sessions), `PendingPromptsReceived`,
  `Refetched`, `AskBarRequested` (Ctrl+L), `LegendRequested`, `SettingsRequested`, `DictationChanged(AskDictation, take)`,
  `DictationWire(raw)`.
- **Actions.** W2: `Perform(Intent)` (the whole panel grammar incl. R, Ctrl+L, Esc order note → hold → close, Done-zone
  E/O), `Pick(option, card)`, `FocusOn(id)`, `Hold(Held)`, `SubmitNote(text)`. W3: `AnswerNow(held, note?, session?)` —
  a toast button commits at once through the same E5 path (re-read `attention_list`, then send; `false` from the engine
  or a vanished item → `Receipt = Stale`, never retried); subscribe to `Items` / `Connection` / `Receipt` /
  `PendingPromptsReceived`. W4: `SubmitAsk(text)` (hold → `mesh_status` → `desk_open("theuth")` or the replicated desk →
  `send_input` → reply streamed from `TextDelta`/`TurnFinished` into `Receipt` and `Ask.Receipts`; `set_event_focus([desk])`
  re-applied on every `PendingPrompts`), `SubmitReply(text)`, `SetReceipt`, `SetReceiptCount`, `ExpireReceipt(s)`;
  `Client` for the dictation calls (`AskRpc`). Also `Refetch()` (one in flight, a request during it runs once after),
  `Close()`.
- **Refetch triggers** (Swift's): `PendingPrompts` and `Lagged` at once; `ApprovalRequested`/`Resolved`, `AskRequested`/
  `Resolved`, `AttentionChanged`, `TurnFinished`, the 13 attention triggers, `TurnStarted`, `SubagentSpawned` after 250 ms;
  each refetch then loads missing packets and the 24 h `activity_list` receipts (`user@companion`).
- **Tests.** `Core.Tests/StoreModelTests.cs` (14, virtual time + a recording engine): debounce and immediate triggers,
  coalesced refetch, lists/rail/panel/packets/receipts, hold → tick → re-read → send, Esc undo then close,
  answered-elsewhere (vanished and `false`, no retry), failure text, displaced hold, send-back note, snooze/approve,
  toast `AnswerNow` with a note, Ask → desk routing + streamed reply + focus re-apply, R field, hooks.

### 7.2 Two Swift defects not copied (deviations)

1. `Store.hold` recreated the `UndoQueue` before holding (to read the delay setting), so a second keystroke during a hold
   silently dropped the first action instead of committing it (130 §5.4, `UndoQueue.hold`'s contract). The port commits the
   displaced action at once, then holds the new one.
2. `Store.refetch`'s `defer { fetching = false }` runs after its `if fetchAgain { refetch() }`, so the follow-up refetch
   always found `fetching == true` and never ran. The port clears `fetching` first; a request during a refetch runs exactly
   once after it.

### 7.3 The seeding tool (`tools/Theuth.Companion.Seed`, `seed.ps1`)

A net10.0 console on `Theuth.Companion.Rpc` (in the portable filter; not part of the app). It finds the engine like the
companion (engine.json + token) and uses only the engine's RPCs. Seeded tasks are owned and reviewed by the user (no desk
is woken) with project `companion-seed`; seeded hires are agent files `seed-<hhmmss>-<n>`.

```
pwsh -File crates\desktop\companion-windows\seed.ps1 all                 # decision + blocked + hire (+ the review refusal line)
pwsh -File crates\desktop\companion-windows\seed.ps1 decision --count 3 [--title TEXT]
pwsh -File crates\desktop\companion-windows\seed.ps1 list
pwsh -File crates\desktop\companion-windows\seed.ps1 clear              # cancel companion-seed tasks, remove seed- proposals, dismiss leftovers
```

Through the build service: `{"action":"script","target":"crates/desktop/companion-windows/seed.ps1","args":["all"]}`.
Every run prints what it created and the resulting `attention_list` kinds and ids.

| Kind | How | Card |
|---|---|---|
| `decision` | `task_create` (todo) + `task_update` state blocked, blockers `["decision:<uuid>"]` | **no options**: the packet with options/recommendation is written only by an agent's `escalate` tool — there is no packet RPC — so the card shows the question line only |
| `blocked` | `task_update` state blocked on a non-decision blocker | Blocked (no options, as in Swift) |
| `hire` | `agent_put` with `proposed_by: agent:theuth`, `paused: true` | Hire |
| `review` | **not seedable without a desk turn**: the ledger allows `doing → review` only, and `doing` is `task_checkout` by a session (`backlog → review` and `todo → review` are refused) | — |
| `approval`, `ask`, `show`, `finished`, `secret`, `handoff`, `resume`, `budget`, `node` | not seedable without a model turn (or model spend / a second engine); the tool says so per kind | — |

Verified on build-B (scratch engine 2.12.0): `seed.ps1 all` →
`attention_list=4 [hire,hire,blocked,decision]` (one hire from an earlier run); the Store-backed probe then logged
`probe attention_list=4 [hire,hire,blocked,decision] agents_list=3 … rail: needsYou=4 working=0 marks=5 quiet=false
headline="4 need you"` and exit 0; `seed.ps1 clear` → cancelled the five `companion-seed` tasks (three backlog leftovers of a first attempt included), removed both proposals, `attention_list=0 []`.

### 7.4 W1b gates

Mac (`just companion-windows-test`): **Rpc 35/35, Core 89/89 — 124 passed** (110 + 14 Store tests). build-B: the app with
the Store builds with **0 errors, 0 warnings** (`bdebdd48`, 14.5 s); `probe.ps1 -From debug -Seconds 12` → the same probe
lines through the Store (above), exit code 0, PROBE OK; seed → list → clear as above.
