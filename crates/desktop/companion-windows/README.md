# companion-windows — the theuth companion for Windows (design 134)

C# on .NET 10 + WinUI 3, Windows App SDK 2.5.1 (trimmed component set), unpackaged and self-contained. The macOS
companion (`../companion`, SwiftPM) is the reference: `Rpc` ports `TheuthKit`, `Core` ports `CompanionCore` file for file
and test for test; the WinUI app is `src/Theuth.Companion`. Design: `docs/design/134-companion-windows.md`; what each wave
built: `docs/design/134-as-built-W<n>.md`.

| Path | What | Where it builds |
|---|---|---|
| `src/Theuth.Companion.Rpc` | engine discovery, JSON-RPC client (`ClientWebSocket`), wire models, codec | net10.0 — Mac + Windows |
| `src/Theuth.Companion.Core` | rail, panel, undo, key table, cards, notification policy, breakpoints, settings, Ask routing, receipts, dictation | net10.0 — Mac + Windows |
| `src/Theuth.Companion` | the WinUI app (W1: log window, `--probe`, `--register-smoke`, crash reports, alive mutex) | Windows only (build-B) |
| `tests/*` | xUnit; `Rpc.Tests` reads the Swift fixtures in `../companion/Tests/TheuthKitTests/Fixtures` | net10.0 — Mac + Windows |

- Mac: `just companion-windows-test` (or `./test.sh`) — `dotnet test` over `Theuth.Companion.Portable.slnf`; the SDK is
  taken from `~/.dotnet` when present.
- Windows: `pwsh -File build.ps1` publishes to `<repo>\target\release\companion-windows\`, checks the five runtime-MSIX
  files (`src/Theuth.Companion/RuntimeMsixPayload.targets`), the size and `AppNotificationManager.Register()`;
  `pwsh -File probe.ps1 -Seconds 15` runs the probe against the local engine and prints its log. Plain builds:
  `dotnet build src\Theuth.Companion -c Debug -p:Platform=x64`. Kill a running `Theuth.Companion.exe` first (it locks the output).
- Logs: `%LOCALAPPDATA%\theuth\companion\logs\` (`companion-<date>.log`, `probe.log`, `*.crash.json`).
