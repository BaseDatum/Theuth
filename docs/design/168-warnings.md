# 168: no warnings in the release build

## Swift (companion, `crates/desktop/companion`)

Branch `fix/168-swift-warnings`, cut from main `b3d33c8e`. Toolchain Swift 6.3.2, macOS SDK 26.5 (Sky's log: SDK 26.0).

### Ask

Sky (2026-11-03 16:26), after pasting his release build log: "all of these deprecations and warnings need to be fixed".

### Counts

| Build | `: warning:` lines under `Sources/` or `Tests/` |
|---|---|
| Baseline cold `swift build -c release` (`/tmp/168/sw-baseline-build.log`) | 24 |
| After, cold `swift build -c release` (`/tmp/168/sw-build.log`) | 0 |
| After, `./test.sh` (debug build of every target incl. the three test targets, `/tmp/168/sw-test.log`) | 0 |

Third-party KeyboardShortcuts: no warnings in either log. `./test.sh`: 281 tests in 45 suites passed.

### Each warning and its fix

| Warning | Fix |
|---|---|
| `AgentsPanel.swift:83`, `RailPanel.swift:38`: `W1Hover.shared` (main actor) referenced from a nonisolated context (default argument of `init(store:hover:)`) | Both classes were already `@MainActor`; in Swift 5 mode a default argument is still evaluated nonisolated, so annotating the init would not help. No caller passed `hover:` (`AppDelegate.swift:21`, `W1Install.swift:8` pass `store:` only), so the parameter is gone and the init body reads `W1Hover.shared`. Same object, same behaviour. |
| "conformance of 'NSEvent' to 'Sendable' is unavailable" at `MainActor.assumeIsolated {` in `AgentsPanel`, `AnyAppCard`, `AskBarPanel`, `AttentionPanel`, `IslandPanel`, `AnswerCard`, `Legend` | `assumeIsolated` now returns `consumed: Bool`; the monitor returns `consumed ? nil : e` outside it. The `[weak self]` / `isKeyWindow` guards are unchanged (they now return `false`, i.e. pass the event through, exactly as `return e` did). `Legend`: the close branch returns `true`, the fallthrough `false`. |
| `SystemPasteboard.swift:14`: no throwing call inside `try` | `try` dropped; `throws` kept (required by `CompanionCore.Pasteboard.snapshot()`). |
| `PanelProbe.swift:62,64,65,67,68,69`: implicit `self` in closure | Explicit `self.` on each reference. |
| `PointController.swift:912`: `NSApp` / `effectiveAppearance` from `nonisolated static var accentSRGB` | `accentSRGB` and `accentHex` are no longer `nonisolated` (so main actor, from the class). Every caller is main actor: `PanelProbe` (`PanelController`), `PointProbe` (`@MainActor enum`), and `screenContext`. `nonisolated` existed only because `accentHex` was the default argument of `screenContext(samples:durationMs:color:)`; that parameter is now `String? = nil` and the body uses `color ?? Self.accentHex`. Same colour, still resolved per request. |
| `PointProbe.swift:107`: captured `var reqSamples` in `async let` | `let stopSamples = reqSamples` before the `async let`s (named `stopSamples`, not `samples`: `samples` is already a `var` in that function). |
| `SpeechPlayer.swift:124`: capture of non-Sendable `AVAudioCompressedBuffer` in a `@Sendable` closure; `:1` "add '@preconcurrency'…" | `nonisolated(unsafe) let inBuf` with a one-line comment: the input block is typed `@Sendable` but `convert(to:error:withInputFrom:)` calls it synchronously on the calling thread. The `:1` note was a consequence of `:124` and disappeared with it; no `@preconcurrency import`. |
| `RailPanel.swift:226`: deprecated localized `appendInterpolation` in `.help("engine: \(store.connection)")` | `.help("engine: \(store.connection)" as String)`: the `StringProtocol` overload, identical text. |
| `TheuthKit/RpcClient.swift:150`: no `async` operation inside `await` (not in Sky's list; present in this SDK's baseline) | `await` dropped. `RpcClient` is an actor and the `Task {}` in its method inherits that isolation, so `self.connect()` is a synchronous same-actor call. |

### Escape hatches

One: `nonisolated(unsafe)` on `SpeechPlayer.decode`'s input buffer, commented in place. Restructuring was not possible without copying the buffer: `AVAudioConverterInputBlock` is `@Sendable` in the SDK and must return the `AVAudioBuffer` object itself. No `@preconcurrency`, no `@unchecked Sendable`.

### Deviations

- `W1Hover.shared`: the brief offered making the init `@MainActor` or moving the default into the body. The init was already main actor (class-level `@MainActor`), so the default moved into the body. The unused `hover:` parameter went with it instead of becoming `HoverSync? = nil`, because nothing passed it.
- `PointController.accentSRGB`: making it main actor also needed `screenContext`'s `color` default moved into the body (see above).
- `RpcClient.swift:150`: extra warning, fixed under the brief's "every other warning" rule.
- Runtime: the companion was not launched (Sky's companion is running). Behaviour is covered by the zero-warning builds, the test suite, and the line-by-line equivalence above.
