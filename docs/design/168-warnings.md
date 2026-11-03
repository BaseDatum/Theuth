# 168: no warnings in the release build

## Rust

### Ask
Sky (2026-11-03 16:26), after pasting the log of his release build: "all of these deprecations and warnings need to be fixed".

### Warnings and fixes (rustc 1.99.0, rmcp 3.8.0)

| Site | Warning | Fix |
|---|---|---|
| `crates/link/src/types.rs:323` | deprecated `Atomic::<u8>::fetch_update` | `try_update`: same arguments, same `Result` |
| `crates/agent/src/loop_/thinking_tests.rs:52` (test, found by the sweep) | same | same |
| `crates/mcp-server/src/catalog.rs:13, 1042, 1043, 1092, 1133` | deprecated alias `rmcp::model::ServerInfo` | `ServerConfig` |
| `crates/theuth/src/mcp.rs:21, 115, 116` | same | same |
| `crates/mcp-server/tests/catalog.rs:11, 344, 345` (test, found by the sweep) | deprecated alias `rmcp::model::ClientInfo` | `ClientConfig` |

rmcp 3.8.0 `src/model.rs`: `pub type ServerConfig = InitializeResult;` and the deprecated `pub type ServerInfo = InitializeResult;`; likewise `ClientConfig` / `ClientInfo` = `InitializeRequestParams`. Same type, so `new`, `with_server_info` and `with_instructions` are the same methods and produce the same value: no behaviour change. These two aliases are the only `#[deprecated]` items in rmcp 3.8.0. Doc comments that named `ServerInfo` now say `ServerConfig`. There is no `#[allow(deprecated)]`.

Clippy, which runs on files I touched (gate 4), flagged two lints in `crates/mcp-server/tests/catalog.rs`. Both are fixed: `useless_format` (a `format!` with no arguments became a `&str` literal) and `await_holding_lock` (the catalogue is now cloned out of the mutex before `.await`). No other clippy lint came up in link, mcp-server, theuth or agent.

### Sweep, before and after
- Baseline `/tmp/168/rs-baseline-check.log` (`mbx check -p link -p mcp-server -p theuth --all-targets`) had 12 distinct rustc warnings from `crates/`: link 1, mcp-server lib 5, mcp-server test `catalog` 3, theuth `t` 3. The count excludes the summary and duplicate lines.
- `/tmp/168/rs-check-bins.log` (`mbx check -p theuth -p desktop`) has 0 warnings from `crates/`. Its one `warning:` line is the third-party future-incompat note for `block v0.1.6`.
- `/tmp/168/rs-check-all.log` (`mbx check -p link -p mcp-server -p theuth -p agent --all-targets`) has 0 `warning:` lines.
- `/tmp/168/rs-clippy.log` (`mbx clippy -p link -p mcp-server -p theuth -p agent --all-targets --no-deps --keep-going -- -D warnings`) has 0 warnings and rc 0.

### Third-party leftovers
`block v0.1.6`: the future-incompat note under `desktop`. The chain (`cargo tree -i block@0.1.6 -p desktop -e normal`):

```
block v0.1.6
├── cocoa v0.26.1
│   └── tauri-nspanel v2.0.1 (git ahkohd/tauri-nspanel branch v2 #18ffb9a2)
│       └── desktop
├── cocoa-foundation v0.2.1
│   └── cocoa v0.26.1 (*)
├── objc-foundation v0.1.1
│   └── tauri-nspanel v2.0.1 (*)
└── tauri-nspanel v2.0.1 (*)
```

`tauri-nspanel` at that revision depends on `cocoa 0.26`, `objc 0.2`, `objc_id` and `objc-foundation`. The note goes away only when `tauri-nspanel` moves to a revision built on `objc2`/`block2`, or when we replace it. Not bumped here, as the brief says.

### Tests
- `mbx test -p link -p mcp-server --lib`: 27 + 3 passed.
- `mbx test -p mcp-server --test catalog`: 5 passed (this test file changed).
- `mbx test -p theuth --bin t mcp`: 1 passed.
- `mbx test -p agent --lib thinking`: 11 passed (this test file changed).

### Deviations and decisions
- The sweep also covered `ClientInfo`, the sibling deprecated alias in rmcp. It was not in Sky's log, but it appeared in the baseline's `--all-targets` output, so it is in scope under brief item 3.
- Gates 3 and 4 ran on all four touched crates (`link`, `mcp-server`, `theuth`, `agent`) in a single `-p` set.
- Tests also ran for the two changed test targets (`mcp-server --test catalog`, `agent --lib thinking`) beyond the `--lib` sets the brief lists.
- The engine restarted three times during the work, and the first run of gate 2 died with SIGTERM in the middle of the build. After that I ran the gates through `nohup` and wrote a marker file at the end of each. Every lock acquisition still covered a single `mbx` command.
- My first edits were in the tree while the prep baseline was still compiling. To keep the baseline on main's sources, I checkpoint-committed them, restored main's files until `rs-baseline.done` appeared, and then put the edits back.

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